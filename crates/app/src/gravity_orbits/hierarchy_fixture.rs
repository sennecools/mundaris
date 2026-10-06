//! Bounded asynchronous app-side work and state for the fixed Slice 2B fixture.
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use mundaris_math::surface::CubePatchAddress;
use mundaris_renderer::{
    ResidentHierarchyDraw, ResidentHierarchyReport, TileData, TileDraw, TilePublicationToken,
    TileSlotState,
};
use mundaris_world::terrain::SurfaceGenerator;

use crate::resident_terrain::{
    ResidentTileBuilder, TileApproximationMetrics, TileBuildDiagnostics, TileBuildIdentity,
};

const WORKER_COUNT: usize = 4;
const MAX_DELAY_MS: u64 = 5_000;
const MAX_MORPH_MS: u64 = 10_000;
const RESULT_CAPACITY: usize = 32;

fn parent_endpoint_was_submitted(
    refine_target: bool,
    morph_fraction: f32,
    report: Option<&ResidentHierarchyReport>,
) -> bool {
    !refine_target
        || morph_fraction != 0.0
        || report.is_some_and(|report| report.draw_children && report.morph_fraction == 0.0)
}

fn should_draw_children(
    enabled: bool,
    refine_target: bool,
    morph_fraction: f32,
    ready: bool,
) -> bool {
    enabled && (refine_target || morph_fraction > 0.0) && ready
}

fn key_snapshot(key: &mundaris_renderer::TileKey) -> serde_json::Value {
    serde_json::json!({
        "body_identity": key.body_identity,
        "definition_words": key.definition_words,
        "radius_m": f64::from_bits(key.radius_bits),
        "surface_revision": key.surface_revision,
        "material_revision": key.material_revision,
        "format_version": key.format_version,
        "filter_version": key.filter_version,
        "face": format!("{:?}", key.address.face()),
        "level": key.address.level(),
        "xy": key.address.coordinates(),
        "cells": key.cells,
    })
}

#[derive(Clone)]
struct BuildJob {
    epoch: u64,
    child: usize,
    requested_at: Instant,
    delay: Duration,
    generator: SurfaceGenerator,
    identity: TileBuildIdentity,
    address: CubePatchAddress,
    cells: u32,
    parent_key: mundaris_renderer::TileKey,
    expected_key: mundaris_renderer::TileKey,
}

struct BuildResult {
    epoch: u64,
    child: usize,
    parent_key: mundaris_renderer::TileKey,
    expected_key: mundaris_renderer::TileKey,
    result: Result<(TileData, TileBuildDiagnostics, TileApproximationMetrics), String>,
    requested_at: Instant,
    worker_started_at: Instant,
    started_at: Instant,
    completed_at: Instant,
}

#[derive(Clone, Copy)]
pub(super) struct HierarchySettings {
    pub enabled: bool,
    pub refine: bool,
    pub morph_duration_ms: u64,
    pub child_delays_ms: [u64; 4],
    pub request_mask: u8,
    pub cancel_pending: bool,
    pub diagnostic_validate: bool,
}

enum WorkerEvent {
    Started {
        epoch: u64,
        child: usize,
        requested_at: Instant,
        started_at: Instant,
    },
    Completed(Box<BuildResult>),
}

fn publish_worker_event(
    sender: &SyncSender<WorkerEvent>,
    stop: &AtomicBool,
    mut event: WorkerEvent,
) -> bool {
    loop {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if stop.load(Ordering::Acquire) {
                    return false;
                }
                event = returned;
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

struct HierarchyWorkers {
    senders: Option<[SyncSender<Box<BuildJob>>; WORKER_COUNT]>,
    results: Receiver<WorkerEvent>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl HierarchyWorkers {
    fn new() -> Result<Self> {
        let (results_tx, results) = mpsc::sync_channel(RESULT_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let mut senders = Vec::with_capacity(WORKER_COUNT);
        let mut threads = Vec::with_capacity(WORKER_COUNT);
        for index in 0..WORKER_COUNT {
            let (sender, receiver) = mpsc::sync_channel::<Box<BuildJob>>(1);
            let result_tx = results_tx.clone();
            let stop = Arc::clone(&stop);
            let thread = thread::Builder::new()
                .name(format!("resident-hierarchy-{index}"))
                .stack_size(4 * 1024 * 1024)
                .spawn(move || {
                    while let Ok(job) = receiver.recv() {
                        let job = *job;
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        let worker_started_at = Instant::now();
                        if !publish_worker_event(
                            &result_tx,
                            &stop,
                            WorkerEvent::Started {
                                epoch: job.epoch,
                                child: job.child,
                                requested_at: job.requested_at,
                                started_at: worker_started_at,
                            },
                        ) {
                            break;
                        }
                        let mut delay_left = job.delay;
                        while !delay_left.is_zero() && !stop.load(Ordering::Acquire) {
                            let interval = delay_left.min(Duration::from_millis(10));
                            thread::sleep(interval);
                            delay_left = delay_left.saturating_sub(interval);
                        }
                        let started_at = Instant::now();
                        let result = if stop.load(Ordering::Acquire) {
                            Err("worker_shutdown_before_build".into())
                        } else {
                            ResidentTileBuilder::build(
                                &job.generator,
                                job.identity,
                                job.address,
                                job.cells,
                            )
                            .and_then(|(tile, diagnostics)| {
                                let approximation = ResidentTileBuilder::measure_approximation(
                                    &job.generator,
                                    &tile,
                                )
                                .map_err(|_| {
                                    crate::resident_terrain::TileBuildError::InvalidSample
                                })?;
                                Ok((tile, diagnostics, approximation))
                            })
                            .map_err(|error| error.to_string())
                        };
                        let completed_at = Instant::now();
                        let requested_at = job.requested_at;
                        let completed = BuildResult {
                            epoch: job.epoch,
                            child: job.child,
                            parent_key: job.parent_key.clone(),
                            expected_key: job.expected_key.clone(),
                            result,
                            requested_at,
                            worker_started_at,
                            started_at,
                            completed_at,
                        };
                        if !publish_worker_event(
                            &result_tx,
                            &stop,
                            WorkerEvent::Completed(Box::new(completed)),
                        ) {
                            break;
                        }
                    }
                })
                .context("starting fixed hierarchy worker")?;
            senders.push(sender);
            threads.push(thread);
        }
        let senders = senders
            .try_into()
            .map_err(|_| anyhow::anyhow!("fixed hierarchy requires four worker slots"))?;
        drop(results_tx);
        Ok(Self {
            senders: Some(senders),
            results,
            stop,
            threads,
        })
    }

    fn try_dispatch(
        &self,
        child: usize,
        job: BuildJob,
    ) -> std::result::Result<(), TrySendError<Box<BuildJob>>> {
        self.senders.as_ref().expect("live worker senders")[child].try_send(Box::new(job))
    }
}

impl Drop for HierarchyWorkers {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.senders.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[derive(Default)]
struct ChildSlot {
    address: Option<CubePatchAddress>,
    key: Option<mundaris_renderer::TileKey>,
    publication: Option<TilePublicationToken>,
    tile: Option<Arc<TileData>>,
    build: Option<TileBuildDiagnostics>,
    approximation: Option<TileApproximationMetrics>,
    state: &'static str,
    requested_at: Option<Instant>,
    started_at: Option<Instant>,
    completed_at: Option<Instant>,
    error: Option<String>,
    worker_queue_ms: f64,
    cpu_build_ms: f64,
}

pub(super) struct ResidentHierarchyFixture {
    pub enabled: bool,
    pub refine_target: bool,
    pub morph_duration: Duration,
    pub morph_fraction: f32,
    pub request_mask: u8,
    pub child_delays: [Duration; WORKER_COUNT],
    pub diagnostic_validate: bool,
    pub validation_pending: Option<usize>,
    pub validation: Option<serde_json::Value>,
    pub validation_history: Vec<serde_json::Value>,
    parent: Option<Arc<TileData>>,
    parent_key: Option<mundaris_renderer::TileKey>,
    children: [ChildSlot; WORKER_COUNT],
    slot_states: [TileSlotState; WORKER_COUNT],
    pending_dispatch: [Option<BuildJob>; WORKER_COUNT],
    request_epoch: u64,
    rejected_late_results: u64,
    authority_invalidated: bool,
    frame_interval_count: u64,
    frame_interval_total_ms: f64,
    frame_interval_max_ms: f64,
    frame_interval_samples_ms: Vec<f64>,
    skip_next_pending_interval: bool,
    frame_interval_source: &'static str,
    workers: Option<HierarchyWorkers>,
}

impl Default for ResidentHierarchyFixture {
    fn default() -> Self {
        Self {
            enabled: false,
            refine_target: false,
            morph_duration: Duration::from_millis(150),
            morph_fraction: 0.0,
            request_mask: 0x0f,
            child_delays: [Duration::ZERO; WORKER_COUNT],
            diagnostic_validate: false,
            validation_pending: None,
            validation: None,
            validation_history: Vec::new(),
            parent: None,
            parent_key: None,
            children: std::array::from_fn(|_| ChildSlot::default()),
            slot_states: std::array::from_fn(|_| TileSlotState::default()),
            pending_dispatch: std::array::from_fn(|_| None),
            request_epoch: 0,
            rejected_late_results: 0,
            authority_invalidated: false,
            frame_interval_count: 0,
            frame_interval_total_ms: 0.0,
            frame_interval_max_ms: 0.0,
            frame_interval_samples_ms: Vec::with_capacity(256),
            skip_next_pending_interval: true,
            frame_interval_source: "native_wall_elapsed",
            workers: None,
        }
    }
}

impl ResidentHierarchyFixture {
    pub fn disable(&mut self) {
        self.enabled = false;
        self.refine_target = false;
        self.cancel_epoch();
        self.validation_pending = None;
        self.validation_history.clear();
    }

    pub fn configure(
        &mut self,
        settings: HierarchySettings,
        parent: Option<Arc<TileData>>,
        generator: Option<SurfaceGenerator>,
        identity: Option<TileBuildIdentity>,
    ) -> Result<()> {
        let HierarchySettings {
            enabled,
            refine,
            morph_duration_ms,
            child_delays_ms,
            request_mask,
            cancel_pending,
            diagnostic_validate,
        } = settings;
        ensure!(
            request_mask <= 0x0f,
            "hierarchy request mask must be 0..=15"
        );
        ensure!(
            morph_duration_ms <= MAX_MORPH_MS,
            "hierarchy morph duration exceeds 10000ms prototype limit"
        );
        ensure!(
            child_delays_ms.iter().all(|delay| *delay <= MAX_DELAY_MS),
            "hierarchy child delay exceeds 5000ms prototype limit"
        );
        self.enabled = enabled;
        self.authority_invalidated = false;
        self.refine_target = enabled && refine;
        self.morph_duration = Duration::from_millis(morph_duration_ms);
        self.child_delays = child_delays_ms.map(Duration::from_millis);
        self.request_mask = request_mask;
        self.diagnostic_validate = diagnostic_validate;
        self.frame_interval_count = 0;
        self.frame_interval_total_ms = 0.0;
        self.frame_interval_max_ms = 0.0;
        self.frame_interval_samples_ms.clear();
        self.skip_next_pending_interval = true;
        if !enabled {
            self.cancel_epoch();
            self.refine_target = false;
            self.validation_pending = None;
            return Ok(());
        }
        let parent = parent.context("GPU hierarchy requires an active resident parent tile")?;
        let generator =
            generator.context("GPU hierarchy requires the current published surface")?;
        let identity = identity.context("GPU hierarchy requires current authority identity")?;
        if self.parent_key.as_ref() != Some(&parent.key) {
            self.cancel_epoch();
            self.children = std::array::from_fn(|_| ChildSlot::default());
            self.parent_key = Some(parent.key.clone());
            self.morph_fraction = 0.0;
        }
        self.parent = Some(Arc::clone(&parent));
        if cancel_pending {
            self.cancel_epoch();
        }
        if self.refine_target {
            let children = parent.key.address.children()?;
            if self.workers.is_none() {
                self.workers = Some(HierarchyWorkers::new()?);
            }
            for (index, address) in children.into_iter().enumerate() {
                let key =
                    ResidentTileBuilder::tile_key(&generator, identity, address, parent.key.cells)?;
                let slot = &mut self.children[index];
                slot.address = Some(address);
                slot.key = Some(key.clone());
                let publication = self.slot_states[index].request(&key)?;
                slot.publication = Some(publication);
                if slot.tile.as_ref().is_some_and(|tile| tile.key == key) {
                    slot.state = "cpu_ready";
                    continue;
                }
                if self.request_mask & (1 << index) == 0 {
                    slot.state = "not_requested_by_mask";
                    continue;
                }
                let job = BuildJob {
                    epoch: self.request_epoch,
                    child: index,
                    requested_at: Instant::now(),
                    delay: self.child_delays[index],
                    generator: generator.clone(),
                    identity,
                    address,
                    cells: parent.key.cells,
                    parent_key: parent.key.clone(),
                    expected_key: key,
                };
                if !matches!(slot.state, "queued" | "building") {
                    slot.state = "queued";
                    slot.error = None;
                    self.pending_dispatch[index] = Some(job);
                }
            }
            self.dispatch_pending();
        } else {
            self.cancel_epoch();
        }
        self.validation_pending = diagnostic_validate.then_some(0);
        self.validation = None;
        self.validation_history.clear();
        Ok(())
    }

    fn cancel_epoch(&mut self) {
        self.request_epoch = self.request_epoch.wrapping_add(1).max(1);
        self.pending_dispatch = std::array::from_fn(|_| None);
        for child in &mut self.children {
            if matches!(child.state, "queued" | "building") {
                child.state = "cancelled";
            }
        }
    }

    pub fn invalidate_parent_authority(&mut self) {
        if self.authority_invalidated {
            return;
        }
        self.cancel_epoch();
        self.refine_target = false;
        self.morph_fraction = 0.0;
        self.validation_pending = None;
        self.authority_invalidated = true;
        for child in &mut self.children {
            child.state = "stale_authority";
        }
    }

    pub fn poll_and_advance(
        &mut self,
        elapsed: Duration,
        report: Option<&ResidentHierarchyReport>,
        host_deterministic: bool,
    ) {
        let workers_pending_at_frame_start = self.pending_dispatch.iter().any(Option::is_some)
            || self
                .children
                .iter()
                .any(|child| matches!(child.state, "queued" | "building"));
        if let Some(workers) = &self.workers {
            while let Ok(event) = workers.results.try_recv() {
                match event {
                    WorkerEvent::Started {
                        epoch,
                        child,
                        requested_at,
                        started_at,
                    } => {
                        if epoch == self.request_epoch && child < WORKER_COUNT {
                            let state = &mut self.children[child];
                            if state.state == "queued" {
                                state.state = "building";
                                state.requested_at = Some(requested_at);
                                state.started_at = Some(started_at);
                            }
                        }
                    }
                    WorkerEvent::Completed(completed) => {
                        let completed = *completed;
                        let index = completed.child;
                        if completed.epoch != self.request_epoch
                            || self.parent_key.as_ref() != Some(&completed.parent_key)
                            || index >= WORKER_COUNT
                        {
                            self.rejected_late_results =
                                self.rejected_late_results.saturating_add(1);
                            continue;
                        }
                        let accepted = completed.result.as_ref().is_ok_and(|(tile, _, _)| {
                            tile.key == completed.expected_key
                                && self.children[index].key.as_ref() == Some(&tile.key)
                                && self.children[index]
                                    .publication
                                    .as_ref()
                                    .is_some_and(|token| {
                                        self.slot_states[index].accept_publication(token, &tile.key)
                                    })
                        });
                        let child = &mut self.children[index];
                        match completed.result {
                            Ok((tile, build, approximation)) if accepted => {
                                child.tile = Some(Arc::new(tile));
                                child.build = Some(build);
                                child.approximation = Some(approximation);
                                child.state = "cpu_ready";
                                child.requested_at = Some(completed.requested_at);
                                child.started_at = Some(completed.started_at);
                                child.completed_at = Some(completed.completed_at);
                                child.worker_queue_ms = completed
                                    .worker_started_at
                                    .duration_since(completed.requested_at)
                                    .as_secs_f64()
                                    * 1000.0;
                                child.cpu_build_ms = build.elapsed.as_secs_f64() * 1000.0;
                                child.error = None;
                            }
                            Ok(_) => {
                                self.rejected_late_results =
                                    self.rejected_late_results.saturating_add(1);
                                child.state = "rejected_stale";
                            }
                            Err(error) => {
                                child.state = "failed";
                                child.error = Some(error);
                                child.completed_at = Some(completed.completed_at);
                            }
                        }
                    }
                }
            }
        }
        self.dispatch_pending();
        self.frame_interval_source = if host_deterministic {
            "fixed_offscreen_elapsed"
        } else {
            "native_wall_elapsed"
        };
        if self.enabled && workers_pending_at_frame_start {
            if self.skip_next_pending_interval {
                self.skip_next_pending_interval = false;
            } else {
                let interval_ms = elapsed.as_secs_f64() * 1000.0;
                self.frame_interval_count = self.frame_interval_count.saturating_add(1);
                self.frame_interval_total_ms += interval_ms;
                self.frame_interval_max_ms = self.frame_interval_max_ms.max(interval_ms);
                if self.frame_interval_samples_ms.len() == 256 {
                    self.frame_interval_samples_ms.remove(0);
                }
                self.frame_interval_samples_ms.push(interval_ms);
            }
        }
        if !self.enabled || !self.children_gpu_ready(report) {
            return;
        }
        if self.morph_duration.is_zero() {
            self.morph_fraction = if self.refine_target { 1.0 } else { 0.0 };
            return;
        }
        if !parent_endpoint_was_submitted(self.refine_target, self.morph_fraction, report) {
            // Submit the exact parent endpoint first. Start advancing only after
            // the renderer confirms that endpoint was actually drawn.
            return;
        }
        let step = elapsed.as_secs_f64() / self.morph_duration.as_secs_f64();
        if self.refine_target {
            self.morph_fraction = (f64::from(self.morph_fraction) + step).clamp(0.0, 1.0) as f32;
        } else {
            self.morph_fraction = (f64::from(self.morph_fraction) - step).clamp(0.0, 1.0) as f32;
        }
    }

    fn dispatch_pending(&mut self) {
        let Some(workers) = &self.workers else { return };
        for child in 0..WORKER_COUNT {
            let Some(job) = self.pending_dispatch[child].take() else {
                continue;
            };
            match workers.try_dispatch(child, job) {
                Ok(()) => {}
                Err(TrySendError::Full(job)) => self.pending_dispatch[child] = Some(*job),
                Err(TrySendError::Disconnected(job)) => {
                    self.children[child].state = "failed";
                    self.children[child].error = Some("hierarchy_worker_disconnected".into());
                    let _ = job;
                }
            }
        }
    }

    pub fn observe_uploads(&mut self, report: Option<&ResidentHierarchyReport>) {
        if self.authority_invalidated {
            return;
        }
        let Some(report) = report else { return };
        for (index, child) in self.children.iter_mut().enumerate() {
            let (Some(tile), Some(publication)) = (&child.tile, &child.publication) else {
                continue;
            };
            if report.resident_keys[index + 1].as_ref() == Some(&tile.key)
                && report.slot_generations[index + 1] == publication.generation()
            {
                child.state = "gpu_ready";
            }
        }
    }

    pub fn children_gpu_ready(&self, report: Option<&ResidentHierarchyReport>) -> bool {
        if self.authority_invalidated {
            return false;
        }
        let (Some(parent), Some(report)) = (&self.parent_key, report) else {
            return false;
        };
        report.parent_pinned
            && report.resident_keys[0].as_ref() == Some(parent)
            && self.children.iter().enumerate().all(|(index, child)| {
                let (Some(tile), Some(publication)) = (&child.tile, &child.publication) else {
                    return false;
                };
                report.resident_keys[index + 1].as_ref() == Some(&tile.key)
                    && report.slot_generations[index + 1] == publication.generation()
            })
    }

    fn gpu_ready_child_count(&self, report: &ResidentHierarchyReport) -> usize {
        if self.authority_invalidated {
            return 0;
        }
        self.children
            .iter()
            .enumerate()
            .filter(|(index, child)| {
                let (Some(tile), Some(publication)) = (&child.tile, &child.publication) else {
                    return false;
                };
                report.resident_keys[index + 1].as_ref() == Some(&tile.key)
                    && report.slot_generations[index + 1] == publication.generation()
            })
            .count()
    }

    fn parent_drawable(&self, report: &ResidentHierarchyReport) -> bool {
        !self.authority_invalidated
            && self.parent_key.as_ref().is_some_and(|parent| {
                report.parent_pinned && report.resident_keys[0].as_ref() == Some(parent)
            })
    }

    pub fn draw(
        &self,
        parent_draw: TileDraw,
        report: Option<&ResidentHierarchyReport>,
    ) -> Result<ResidentHierarchyDraw> {
        let parent_tile = self
            .parent
            .as_ref()
            .context("hierarchy parent unavailable")?;
        ensure!(
            parent_draw.tile.key == parent_tile.key,
            "hierarchy parent draw key changed"
        );
        let parent_anchor = parent_tile.anchor_position_body()?;
        let mut children = std::array::from_fn(|_| None);
        for (index, child) in self.children.iter().enumerate() {
            let (Some(tile), Some(publication)) = (&child.tile, &child.publication) else {
                continue;
            };
            let delta = tile.anchor_position_body()? - parent_anchor;
            children[index] = Some(TileDraw {
                tile: Arc::clone(tile),
                publication: publication.clone(),
                anchor_view_m: parent_draw.anchor_view_m + parent_draw.body_to_view * delta,
                body_to_view: parent_draw.body_to_view,
                mode: parent_draw.mode,
                sun_body: parent_draw.sun_body,
            });
        }
        let draw_children = should_draw_children(
            self.enabled,
            self.refine_target,
            self.morph_fraction,
            self.children_gpu_ready(report),
        ) && children.iter().all(Option::is_some);
        let fraction = if draw_children {
            self.morph_fraction
        } else {
            0.0
        };
        let draw = ResidentHierarchyDraw {
            parent: parent_draw,
            children,
            draw_children,
            morph_fraction: fraction,
        };
        draw.validate()?;
        Ok(draw)
    }

    pub fn snapshot(
        &self,
        report: Option<&ResidentHierarchyReport>,
        frame: u64,
    ) -> Option<serde_json::Value> {
        if !self.enabled {
            return None;
        }
        let report = report.cloned().unwrap_or_default();
        let observed_at = Instant::now();
        let children: Vec<_> = self.children.iter().enumerate().map(|(index, child)| {
            serde_json::json!({
                "index": index,
                "address": child.address.map(|a| serde_json::json!({"face": format!("{:?}", a.face()), "level": a.level(), "xy": a.coordinates()})),
                "state": child.state,
                "request_epoch": self.request_epoch,
                "key_matches_report": child.key.as_ref() == report.resident_keys[index + 1].as_ref(),
                "slot_generation": report.slot_generations[index + 1],
                "publication_generation": child.publication.as_ref().map(TilePublicationToken::generation),
                "content_upload_bytes": report.slot_content_upload_bytes[index + 1],
                "cpu_payload_bytes": child.tile.as_ref().map(|t| t.retained_payload_bytes()),
                "cpu_build_ms": child.build.map(|_| child.cpu_build_ms),
                "authoritative_query_count": child.build.map(|b| b.authoritative_query_count),
                "worker_queue_ms": child.worker_queue_ms,
                "configured_delay_ms": self.child_delays[index].as_secs_f64() * 1000.0,
                "request_age_ms": child.requested_at.map(|at| observed_at.saturating_duration_since(at).as_secs_f64() * 1000.0),
                "worker_start_age_ms": child.started_at.map(|at| observed_at.saturating_duration_since(at).as_secs_f64() * 1000.0),
                "completion_age_ms": child.completed_at.map(|at| observed_at.saturating_duration_since(at).as_secs_f64() * 1000.0),
                "approximation": child.approximation.map(|m| serde_json::json!({
                    "max_texel_radial_error_m": m.max_texel_radial_error_m,
                    "rms_texel_radial_error_m": m.rms_texel_radial_error_m,
                    "max_normal_angular_error_radians": m.max_normal_angular_error_radians,
                    "max_triangle_centroid_error_m": m.max_triangle_centroid_error_m,
                    "rms_triangle_centroid_error_m": m.rms_triangle_centroid_error_m,
                })),
                "error": child.error,
            })
        }).collect();
        let parent = self.parent.as_ref();
        let resources = &report.resources;
        let resident_keys = report
            .resident_keys
            .iter()
            .map(|key| key.as_ref().map(key_snapshot))
            .collect::<Vec<_>>();
        let cpu_ready_child_count = self
            .children
            .iter()
            .filter(|child| {
                child.tile.is_some() && matches!(child.state, "cpu_ready" | "gpu_ready")
            })
            .count();
        let gpu_ready_child_count = self.gpu_ready_child_count(&report);
        let child_cpu_payload_bytes: usize = self
            .children
            .iter()
            .filter_map(|child| child.tile.as_ref())
            .map(|tile| tile.retained_payload_bytes())
            .sum();
        Some(serde_json::json!({
            "enabled": self.enabled,
            "fixture": "one_parent_four_children_fixed_cube_patch",
            "frame_number": frame,
            "request_epoch": self.request_epoch,
            "rejected_late_results": self.rejected_late_results,
            "authority_invalidated": self.authority_invalidated,
            "parent_key": parent.map(|p| key_snapshot(&p.key)),
            "parent_drawable": self.parent_drawable(&report),
            "parent_pinned": report.parent_pinned,
            "parent_cpu_payload_bytes": parent.map(|p| p.retained_payload_bytes()),
            "child_cpu_payload_bytes": child_cpu_payload_bytes,
            "parent_content_upload_bytes": report.slot_content_upload_bytes[0],
            "child_states": children,
            "cpu_ready_child_count": cpu_ready_child_count,
            "gpu_ready_child_count": gpu_ready_child_count,
            "request_mask": self.request_mask,
            "diagnostic_validation_requested": self.diagnostic_validate,
            "morph_target": if self.refine_target { 1.0 } else { 0.0 },
            "requested_morph_fraction": self.morph_fraction,
            "morph_fraction": report.morph_fraction,
            "morph_duration_ms": self.morph_duration.as_secs_f64() * 1000.0,
            "draw_children": report.draw_children,
            "resident_keys": resident_keys,
            "slot_generations": report.slot_generations,
            "slot_content_upload_bytes": report.slot_content_upload_bytes,
            "draw_children_gpu_ready": self.children_gpu_ready(Some(&report)),
            "per_frame_content_upload_bytes": report.slot_content_upload_bytes.iter().sum::<u64>(),
            "aggregate_resources": {
                "tile_content_upload_bytes": resources.tile_content_upload_bytes,
                "tile_content_upload_count": resources.tile_content_upload_count,
                "cumulative_content_upload_bytes": resources.cumulative_content_upload_bytes,
                "cumulative_content_upload_count": resources.cumulative_content_upload_count,
                "tile_content_pack_bytes": resources.tile_content_pack_bytes,
                "tile_content_pack_ms": resources.tile_content_pack_duration.as_secs_f64() * 1000.0,
                "tile_content_upload_api_ms": resources.tile_content_upload_api_duration.as_secs_f64() * 1000.0,
                "metadata_upload_bytes": resources.metadata_upload_bytes,
                "allocation_count": resources.allocation_count,
                "allocation_capacity_bytes": resources.allocation_capacity_bytes,
                "cpu_retained_payload_bytes": resources.cpu_retained_payload_bytes,
                "gpu_tile_payload_bytes": resources.gpu_tile_payload_bytes,
                "grid_bytes": resources.grid_bytes,
            },
            "worker_pool": {
                "worker_count": WORKER_COUNT,
                "requested_stack_bytes": WORKER_COUNT * 4 * 1024 * 1024,
                "per_worker_queued_jobs": 1,
                "result_event_capacity": RESULT_CAPACITY,
                "maximum_job_copies": 12,
                "thread_stack_bytes_are_virtual_reservations": true,
            },
            "atomic_child_publication_reason": "all_four_quadrants_share_one_parent_grid_and_one_morph_fraction; partial_overlay_would_leave_unbalanced_parent_child_edges",
            "ordinary_frame_build_waits": 0,
            "ordinary_frame_gpu_waits": 0,
            "worker_pending_frame_intervals": {
                "source": self.frame_interval_source,
                "count": self.frame_interval_count,
                "total_ms": self.frame_interval_total_ms,
                "max_ms": self.frame_interval_max_ms,
                "samples_ms_last_256": self.frame_interval_samples_ms,
                "first_interval_after_configure_skipped": true,
            },
            "validation": self.validation,
            "validation_history": self.validation_history,
            "validation_readback_bytes": report.validation_readback_bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WorkerEvent, parent_endpoint_was_submitted, publish_worker_event, should_draw_children,
    };
    use mundaris_renderer::ResidentHierarchyReport;
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc},
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn morph_start_waits_for_parent_endpoint_and_reversal_keeps_progress() {
        assert!(!parent_endpoint_was_submitted(true, 0.0, None));

        let parent_only = ResidentHierarchyReport::default();
        assert!(!parent_endpoint_was_submitted(
            true,
            0.0,
            Some(&parent_only)
        ));

        let first_child_submission = ResidentHierarchyReport {
            draw_children: true,
            morph_fraction: 0.0,
            ..ResidentHierarchyReport::default()
        };
        assert!(parent_endpoint_was_submitted(
            true,
            0.0,
            Some(&first_child_submission)
        ));

        let mid_morph = ResidentHierarchyReport {
            draw_children: true,
            morph_fraction: 0.4,
            ..ResidentHierarchyReport::default()
        };
        assert!(parent_endpoint_was_submitted(false, 0.4, Some(&mid_morph)));
        assert!(should_draw_children(true, false, 0.4, true));
        assert!(!should_draw_children(true, false, 0.0, true));
    }

    #[test]
    fn shutdown_unblocks_worker_when_result_queue_is_full() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(WorkerEvent::Started {
                epoch: 1,
                child: 0,
                requested_at: Instant::now(),
                started_at: Instant::now(),
            })
            .expect("fill bounded result channel");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_sender = sender.clone();
        let worker = thread::spawn(move || {
            publish_worker_event(
                &worker_sender,
                &worker_stop,
                WorkerEvent::Started {
                    epoch: 1,
                    child: 1,
                    requested_at: Instant::now(),
                    started_at: Instant::now(),
                },
            )
        });

        thread::sleep(Duration::from_millis(10));
        // HierarchyWorkers::drop sets stop before joining while retaining the
        // receiver. A full queue must therefore let the publisher observe stop.
        stop.store(true, std::sync::atomic::Ordering::Release);
        let deadline = Instant::now() + Duration::from_secs(1);
        while !worker.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        assert!(
            worker.is_finished(),
            "worker remained blocked on a full queue"
        );
        assert!(!worker.join().expect("worker thread panicked"));
        drop(receiver);
    }

    #[test]
    fn stale_parent_invalidation_advances_epoch_once_and_rejects_pending_state() {
        let mut fixture = super::ResidentHierarchyFixture {
            enabled: true,
            refine_target: true,
            request_epoch: 7,
            ..super::ResidentHierarchyFixture::default()
        };
        for child in &mut fixture.children {
            child.state = "building";
        }

        fixture.invalidate_parent_authority();
        assert_eq!(fixture.request_epoch, 8);
        assert!(!fixture.refine_target);
        assert!(fixture.authority_invalidated);
        assert!(
            fixture
                .children
                .iter()
                .all(|child| child.state == "stale_authority")
        );
        fixture.observe_uploads(Some(&ResidentHierarchyReport::default()));
        assert!(
            fixture
                .children
                .iter()
                .all(|child| child.state == "stale_authority")
        );
        assert!(!fixture.children_gpu_ready(Some(&ResidentHierarchyReport::default())));

        fixture.invalidate_parent_authority();
        assert_eq!(fixture.request_epoch, 8);
    }
}
