//! Bounded regional terrain selection, CPU tile reuse, and asynchronous builds.
//!
//! This layer deliberately keeps desired patches, CPU payloads, GPU-resident
//! patches, and the published draw cover as separate sets. Projected error is
//! an explicitly approximate selector: it combines a grid-cell spherical
//! sagitta term with one quarter of the authored geological relief fraction,
//! scaled by patch angular width. Proximity uses the closest point on the
//! angular patch footprint and its local relief envelope instead of the global
//! height bound, which would make distant tiles appear uniformly close. This
//! local residual proxy is deterministic but does not certify unsampled error.

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use glam::DVec3;
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_world::terrain::SurfaceGenerator;
use serde::Serialize;

use crate::resident_terrain::{
    ResidentTileBuilder, TileBuildDiagnostics, TileBuildIdentity, TileData, TileKey,
};

/// Prototype limits for one regional selector/scheduler instance.
#[derive(Debug, Clone)]
pub struct RegionalConfig {
    pub roots: Vec<CubePatchAddress>,
    pub max_level: u8,
    pub cells: u32,
    pub cpu_tile_cap: usize,
    pub cpu_byte_cap: usize,
    pub worker_count: usize,
    pub worker_delay: Duration,
    pub queue_cap: usize,
    pub completion_cap: usize,
    pub admission_cap_per_tick: usize,
    pub max_desired_patches: usize,
    pub upload_tile_cap: usize,
    pub upload_byte_cap: usize,
    pub publication_cap_per_tick: usize,
    pub transition_cap: usize,
    pub split_threshold_px: f64,
    pub merge_threshold_px: f64,
    pub prediction_seconds: f64,
    pub high_speed_mps: f64,
}

impl Default for RegionalConfig {
    fn default() -> Self {
        Self {
            roots: vec![CubePatchAddress::root(CubeFace::PositiveZ)],
            max_level: 8,
            cells: 32,
            cpu_tile_cap: 96,
            cpu_byte_cap: 64 * 1024 * 1024,
            worker_count: 4,
            worker_delay: Duration::ZERO,
            queue_cap: 32,
            completion_cap: 8,
            admission_cap_per_tick: 4,
            max_desired_patches: 512,
            upload_tile_cap: 4,
            upload_byte_cap: 8 * 1024 * 1024,
            publication_cap_per_tick: 4,
            transition_cap: 8,
            split_threshold_px: 0.15,
            merge_threshold_px: 0.075,
            prediction_seconds: 0.75,
            high_speed_mps: 250.0,
        }
    }
}

/// Body-fixed observer state consumed by regional selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalView {
    pub body_position_m: DVec3,
    pub body_velocity_mps: DVec3,
    /// Focal/projection scale in pixels.
    pub projection_scale_px: f64,
}

/// One immutable CPU tile admitted to the app/runtime upload queue.
#[derive(Debug, Clone)]
pub struct RegionalUpload {
    pub address: CubePatchAddress,
    pub tile: Arc<TileData>,
    pub bytes: usize,
}

/// A local resident replacement that preserves a complete balanced cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionalPublication {
    Split {
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        cover: Vec<CubePatchAddress>,
    },
    Merge {
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        cover: Vec<CubePatchAddress>,
    },
}

impl RegionalPublication {
    pub fn cover(&self) -> &[CubePatchAddress] {
        match self {
            Self::Split { cover, .. } | Self::Merge { cover, .. } => cover,
        }
    }
}

/// Typed diagnostics; address strings retain stable `Debug` spelling without
/// adding serialization to the shared math address type.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalSnapshot {
    pub tick: u64,
    pub configuration: RegionalConfigSnapshot,
    pub desired: Vec<RegionalPatchSnapshot>,
    pub resident: Vec<String>,
    pub drawable: Vec<String>,
    pub upload_queue: Vec<RegionalQueueSnapshot>,
    pub build_requests: Vec<RegionalRequestSnapshot>,
    pub desired_count: usize,
    pub resident_count: usize,
    pub drawable_count: usize,
    pub cpu_cached_tiles: usize,
    pub cpu_cached_bytes: usize,
    pub worker_queued: usize,
    pub worker_running: usize,
    pub completion_backlog: usize,
    pub completion_drain_micros: u64,
    pub selection_time_micros: u64,
    pub scheduler_time_micros: u64,
    pub cpu_tile_cap: usize,
    pub cpu_byte_cap: usize,
    pub max_desired_patches: usize,
    pub worker_queue_capacity: usize,
    pub completion_capacity: usize,
    pub upload_tile_capacity: usize,
    pub upload_byte_capacity: usize,
    pub estimated_worker_scratch_bytes: usize,
    pub upload_backlog_tiles: usize,
    pub upload_backlog_bytes: usize,
    /// Estimate of completed desired CPU tiles awaiting upload or residency.
    /// This is derived from the bounded CPU cache and excludes queued uploads.
    pub estimated_completed_unpublished_bytes: usize,
    pub publication_candidates: usize,
    pub active_transitions_reported: usize,
    /// Root-area-normalized area-weighted mean projected error, in pixels.
    pub desired_projected_error_px: f64,
    /// Root-area-normalized area-weighted mean projected error of drawable cover.
    pub drawable_projected_error_px: f64,
    /// Positive excess drawable mean error over desired mean error, in pixels.
    pub refinement_debt: f64,
    pub queue_pressure: bool,
    pub cpu_cache_pressure: bool,
    pub upload_pressure: bool,
    pub publication_pressure: bool,
    pub desired_capacity_pressure: bool,
    pub selector_fixed_point_reused: bool,
    pub requests_issued: u64,
    pub jobs_started: u64,
    pub cancelled_before_start: u64,
    pub cancelled_during_work: u64,
    pub stale_completions: u64,
    pub completed_unused_tiles: u64,
    /// Bytes from a CPU build instance discarded before GPU-residency
    /// acknowledgement. Cached instances still pending publication are excluded.
    pub bytes_built_but_unused: u64,
    /// Builder-reported kernel time for CPU build instances discarded before
    /// any successful GPU-residency acknowledgement. Excludes artificial delay.
    pub build_time_discarded_micros: u64,
    pub build_time_micros: u64,
    pub worker_elapsed_micros: u64,
    /// Bounded rolling sample of completed worker elapsed time, in microseconds.
    pub recent_build_elapsed_micros: Vec<u64>,
    pub build_failures: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub rebuilds: u64,
    pub reuploads: u64,
    pub last_admitted_priorities: Vec<RegionalPrioritySnapshot>,
    pub selector_error_model: &'static str,
}

/// Settings that materially determine a regional terrain snapshot. Runtime
/// captures include these values so worker-pressure comparisons are explicit.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalConfigSnapshot {
    pub roots: Vec<String>,
    pub max_level: u8,
    pub cells: u32,
    pub max_desired_patches: usize,
    pub worker_count: usize,
    pub worker_delay_millis: u64,
    pub queue_cap: usize,
    pub completion_cap: usize,
    pub admission_cap_per_tick: usize,
    pub upload_tile_cap: usize,
    pub upload_byte_cap: usize,
    pub publication_cap_per_tick: usize,
    pub transition_cap: usize,
    pub split_threshold_px: f64,
    pub merge_threshold_px: f64,
    pub prediction_seconds: f64,
    pub high_speed_mps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalPatchSnapshot {
    pub address: String,
    pub level: u8,
    pub projected_error_px: f64,
    pub priority: f64,
    /// absent, requested, building, built_cpu, queued_upload, resident, drawable
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalQueueSnapshot {
    pub address: String,
    pub bytes: usize,
    pub priority: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalRequestSnapshot {
    pub address: String,
    pub priority: f64,
    pub state: &'static str,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalSplitFrontier {
    pub parent: CubePatchAddress,
    pub children: [CubePatchAddress; 4],
    pub parent_resident: bool,
    pub ready_children: usize,
    pub aggregate_priority: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalPrioritySnapshot {
    pub address: String,
    pub projected_error_px: f64,
    pub approach_multiplier: f64,
    pub high_speed_multiplier: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalError {
    InvalidConfig,
    InvalidView,
    InvalidTile,
    CpuCapacity,
    TileBuild,
    InvalidCover,
    NotResident,
    PublicationBudget,
}

impl std::fmt::Display for RegionalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfig => "regional terrain configuration is invalid",
            Self::InvalidView => "regional view contains a non-finite or negative projection value",
            Self::InvalidTile => "regional tile does not match this surface identity",
            Self::CpuCapacity => "regional CPU tile cache has no capacity for the seeded tile",
            Self::TileBuild => "regional tile content key could not be constructed",
            Self::InvalidCover => "regional cover is incomplete, overlapping, or unbalanced",
            Self::NotResident => "regional publication references a nonresident tile",
            Self::PublicationBudget => "regional publication budget is exhausted",
        })
    }
}

impl std::error::Error for RegionalError {}

#[derive(Debug)]
struct CacheEntry {
    tile: Arc<TileData>,
    bytes: usize,
    last_use: u64,
    build_elapsed_micros: Option<u64>,
    ever_resident: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheInsertResult {
    Inserted,
    AlreadyCached,
    Rejected,
}

#[derive(Debug)]
struct BuildToken {
    cancelled: AtomicBool,
    started: AtomicBool,
}

#[derive(Debug)]
struct BuildJob {
    address: CubePatchAddress,
    key: TileKey,
    token: Arc<BuildToken>,
}

#[derive(Debug)]
struct BuildResult {
    address: CubePatchAddress,
    key: TileKey,
    token: Arc<BuildToken>,
    result: Option<(Arc<TileData>, TileBuildDiagnostics)>,
    elapsed: Duration,
}

struct WorkerPool {
    sender: SyncSender<BuildJob>,
    _worker_count: usize,
}

/// Regional adaptive selector and bounded asynchronous CPU tile builder.
///
/// GPU allocation and submission safety remain owned by the renderer. The app
/// acknowledges residency only after upload publication succeeds, and only a
/// complete resident 2:1 cover may become drawable.
pub struct RegionalTerrain {
    generator: Arc<SurfaceGenerator>,
    identity: TileBuildIdentity,
    config: RegionalConfig,
    workers: WorkerPool,
    completions: Receiver<BuildResult>,
    completion_backlog: Arc<AtomicUsize>,
    worker_running: Arc<AtomicUsize>,
    tick: u64,
    view: RegionalView,
    desired: BTreeMap<CubePatchAddress, RegionalPatchSnapshot>,
    drawable: Vec<CubePatchAddress>,
    resident: BTreeSet<CubePatchAddress>,
    external_pins: BTreeSet<CubePatchAddress>,
    cache: HashMap<TileKey, CacheEntry>,
    cache_bytes: usize,
    cache_clock: u64,
    in_flight: HashMap<TileKey, Arc<BuildToken>>,
    uploads: Vec<RegionalUpload>,
    upload_bytes: usize,
    upload_allowlist: Option<BTreeSet<CubePatchAddress>>,
    publications: Vec<RegionalPublication>,
    last_priorities: Vec<RegionalPrioritySnapshot>,
    publications_this_tick: usize,
    desired_addresses: Vec<CubePatchAddress>,
    last_selection_view: Option<RegionalView>,
    selector_can_reuse_fixed_point: bool,
    selector_last_capacity_pressure: bool,
    selector_fixed_point_reused: bool,
    reserved_split_parent: Option<CubePatchAddress>,
    resident_history: HashSet<CubePatchAddress>,
    seen_cache_requests: HashSet<TileKey>,
    stats: Counters,
    pressure: Pressure,
}

#[derive(Debug, Default)]
struct Counters {
    requests_issued: u64,
    jobs_started: u64,
    cancelled_before_start: u64,
    cancelled_during_work: u64,
    stale_completions: u64,
    completed_unused_tiles: u64,
    bytes_built_but_unused: u64,
    build_time_discarded_micros: u64,
    build_time_micros: u64,
    worker_elapsed_micros: u64,
    recent_build_elapsed_micros: VecDeque<u64>,
    build_failures: u64,
    completion_drain_micros: u64,
    selection_time_micros: u64,
    scheduler_time_micros: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
    rebuilds: u64,
    reuploads: u64,
}

#[derive(Debug, Default)]
struct Pressure {
    queue: bool,
    cpu: bool,
    upload: bool,
    publication: bool,
    desired_capacity: bool,
}

impl RegionalTerrain {
    pub fn new(
        generator: SurfaceGenerator,
        identity: TileBuildIdentity,
        config: RegionalConfig,
    ) -> Result<Self, RegionalError> {
        validate_config(&config)?;
        let generator = Arc::new(generator);
        let worker_count = config.worker_count;
        let (job_sender, job_receiver) = mpsc::sync_channel::<BuildJob>(config.queue_cap);
        let (completion_sender, completion_receiver) =
            mpsc::sync_channel::<BuildResult>(config.completion_cap);
        let job_receiver = Arc::new(Mutex::new(job_receiver));
        let completion_backlog = Arc::new(AtomicUsize::new(0));
        let worker_running = Arc::new(AtomicUsize::new(0));

        for _ in 0..worker_count {
            let receiver = Arc::clone(&job_receiver);
            let sender = completion_sender.clone();
            let generator = Arc::clone(&generator);
            let delay = config.worker_delay;
            let running = Arc::clone(&worker_running);
            let backlog = Arc::clone(&completion_backlog);
            thread::Builder::new()
                .name("regional-terrain-builder".into())
                .stack_size(4 * 1024 * 1024)
                .spawn(move || {
                    loop {
                        let job = {
                            let receiver = receiver
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            receiver.recv()
                        };
                        let Ok(job) = job else { break };
                        if job.token.cancelled.load(AtomicOrdering::Acquire) {
                            backlog.fetch_add(1, AtomicOrdering::AcqRel);
                            if sender
                                .send(BuildResult {
                                    address: job.address,
                                    key: job.key,
                                    token: job.token,
                                    result: None,
                                    elapsed: Duration::ZERO,
                                })
                                .is_err()
                            {
                                backlog.fetch_sub(1, AtomicOrdering::AcqRel);
                                break;
                            }
                            continue;
                        }
                        job.token.started.store(true, AtomicOrdering::Release);
                        running.fetch_add(1, AtomicOrdering::AcqRel);
                        let started = std::time::Instant::now();
                        let mut remaining = delay;
                        while !remaining.is_zero()
                            && !job.token.cancelled.load(AtomicOrdering::Acquire)
                        {
                            let slice = remaining.min(Duration::from_millis(5));
                            thread::sleep(slice);
                            remaining = remaining.saturating_sub(slice);
                        }
                        let result = if job.token.cancelled.load(AtomicOrdering::Acquire) {
                            None
                        } else {
                            ResidentTileBuilder::build(
                                &generator,
                                identity,
                                job.address,
                                job.key.cells,
                            )
                            .ok()
                            .map(|(tile, diagnostics)| (Arc::new(tile), diagnostics))
                        };
                        let elapsed = started.elapsed();
                        running.fetch_sub(1, AtomicOrdering::AcqRel);
                        backlog.fetch_add(1, AtomicOrdering::AcqRel);
                        if sender
                            .send(BuildResult {
                                address: job.address,
                                key: job.key,
                                token: job.token,
                                result,
                                elapsed,
                            })
                            .is_err()
                        {
                            backlog.fetch_sub(1, AtomicOrdering::AcqRel);
                            break;
                        }
                    }
                })
                .map_err(|_| RegionalError::InvalidConfig)?;
        }
        drop(completion_sender);

        Ok(Self {
            generator,
            identity,
            config,
            workers: WorkerPool {
                sender: job_sender,
                _worker_count: worker_count,
            },
            completions: completion_receiver,
            completion_backlog,
            worker_running,
            tick: 0,
            view: RegionalView {
                body_position_m: DVec3::ZERO,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: 1.0,
            },
            desired: BTreeMap::new(),
            drawable: Vec::new(),
            resident: BTreeSet::new(),
            external_pins: BTreeSet::new(),
            cache: HashMap::new(),
            cache_bytes: 0,
            cache_clock: 0,
            in_flight: HashMap::new(),
            uploads: Vec::new(),
            upload_bytes: 0,
            upload_allowlist: None,
            publications: Vec::new(),
            last_priorities: Vec::new(),
            publications_this_tick: 0,
            desired_addresses: Vec::new(),
            last_selection_view: None,
            selector_can_reuse_fixed_point: false,
            selector_last_capacity_pressure: false,
            selector_fixed_point_reused: false,
            reserved_split_parent: None,
            resident_history: HashSet::new(),
            seen_cache_requests: HashSet::new(),
            stats: Counters::default(),
            pressure: Pressure::default(),
        })
    }

    /// Update selection and admit bounded work. This method never waits for a
    /// worker, tile build, upload, GPU completion, or transition.
    pub fn tick(&mut self, view: RegionalView, _dt: Duration) -> Result<(), RegionalError> {
        if !view.body_position_m.is_finite()
            || !view.body_velocity_mps.is_finite()
            || !view.projection_scale_px.is_finite()
            || view.projection_scale_px < 0.0
        {
            return Err(RegionalError::InvalidView);
        }
        self.tick = self.tick.saturating_add(1);
        self.publications_this_tick = 0;
        self.pressure = Pressure::default();
        self.view = view;
        let phase_started = std::time::Instant::now();
        self.select_desired();
        self.update_split_reservation();
        self.stats.selection_time_micros = phase_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        self.drain_completions();
        self.touch_pinned_cache_entries();
        let scheduler_started = std::time::Instant::now();
        self.cancel_obsolete_work();
        self.admit_builds();
        self.admit_uploads();
        self.refresh_publications();
        self.stats.scheduler_time_micros = scheduler_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        Ok(())
    }

    pub fn desired(&self) -> &[CubePatchAddress] {
        // BTreeMap keeps the public cover deterministic. The scratch vector is
        // held in a stable field so the slice borrow cannot escape mutation.
        &self.desired_addresses
    }

    pub fn drawable(&self) -> &[CubePatchAddress] {
        &self.drawable
    }

    pub fn config(&self) -> &RegionalConfig {
        &self.config
    }

    pub fn tile(&self, address: CubePatchAddress) -> Option<Arc<TileData>> {
        let key = self.key_for(address).ok()?;
        let entry = self.cache.get(&key)?;
        Some(Arc::clone(&entry.tile))
    }

    /// Insert a caller-provided prebuilt tile into the exact CPU cache.
    pub fn seed_tile(&mut self, tile: Arc<TileData>) -> Result<(), RegionalError> {
        if tile.validate().is_err() || self.key_for(tile.key.address).as_ref() != Ok(&tile.key) {
            return Err(RegionalError::InvalidTile);
        }
        if let Some(token) = self.in_flight.get(&tile.key) {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
        if self.cache_insert(tile, false, None) == CacheInsertResult::Rejected {
            return Err(RegionalError::CpuCapacity);
        }
        self.admit_uploads();
        Ok(())
    }

    pub fn queued_uploads(&self) -> &[RegionalUpload] {
        &self.uploads
    }

    /// Restrict queued and future upload admission to the supplied addresses.
    /// CPU cache entries, desired state, and worker builds are unaffected. Pass
    /// `None` to restore the default unrestricted upload frontier.
    pub fn set_upload_allowlist(&mut self, allowlist: Option<&[CubePatchAddress]>) {
        self.upload_allowlist = allowlist.map(|addresses| addresses.iter().copied().collect());
        let mut retained_bytes = 0usize;
        self.uploads.retain(|upload| {
            let retain = self
                .upload_allowlist
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&upload.address));
            if retain {
                retained_bytes = retained_bytes.saturating_add(upload.bytes);
            }
            retain
        });
        self.upload_bytes = retained_bytes;
        self.admit_uploads();
    }

    /// Acknowledge a completed renderer upload. The caller must invoke this
    /// only after the renderer accepts the exact keyed tile.
    pub fn ack_resident(&mut self, address: CubePatchAddress) {
        let newly_resident = self.resident.insert(address);
        if let Ok(key) = self.key_for(address)
            && let Some(entry) = self.cache.get_mut(&key)
        {
            entry.ever_resident = true;
        }
        let queued = self
            .uploads
            .iter()
            .position(|upload| upload.address == address);
        if !newly_resident && queued.is_none() {
            return;
        }
        if let Some(index) = queued {
            let upload = self.uploads.remove(index);
            self.upload_bytes = self.upload_bytes.saturating_sub(upload.bytes);
        }
        if newly_resident && !self.resident_history.insert(address) {
            self.stats.reuploads = self.stats.reuploads.saturating_add(1);
        }
        if self.drawable.is_empty()
            && self
                .config
                .roots
                .iter()
                .all(|root| self.resident.contains(root))
        {
            self.drawable.clone_from(&self.config.roots);
            self.drawable.sort();
        }
        self.refresh_publications();
    }

    /// Reflect renderer eviction so future upload admission can restore this tile.
    pub fn mark_not_resident(&mut self, address: CubePatchAddress) {
        if !self.resident.remove(&address) {
            return;
        }
        if self.drawable.contains(&address) {
            // Keep the last acknowledged cover until the runtime supplies a
            // replacement; publication validation prevents adding holes.
            self.pressure.publication = true;
        }
        self.refresh_publications();
    }

    /// Cancel queued/delayed requests without waiting for a worker.
    pub fn request_cancel_all(&mut self) {
        for token in self.in_flight.values() {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
    }

    /// Drain worker outcomes without changing view selection or admitting new
    /// requests. Disabled fixtures can keep pumping this until `is_idle()`.
    pub fn drain_cancelled(&mut self) {
        self.drain_completions();
    }

    /// True after every request result was drained and workers left their build
    /// section. Runtime reconfiguration can drop the core safely at this point.
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
            && self.worker_running.load(AtomicOrdering::Acquire) == 0
            && self.completion_backlog.load(AtomicOrdering::Acquire) == 0
    }

    /// Return local resident replacements. The returned vector is capped by
    /// both the per-tick publication and transition limits.
    pub fn publication_candidates(&self) -> Vec<RegionalPublication> {
        self.publications.clone()
    }

    /// Return locally desired sibling groups whose trial split preserves a
    /// complete balanced cover. Residency is reported but is not required, so
    /// callers can choose upload work before the group is GPU-ready.
    pub fn split_frontiers(&self) -> Vec<RegionalSplitFrontier> {
        self.split_frontier_groups()
    }

    /// Validate that `cover` completely and non-overlappingly covers the
    /// configured roots and satisfies the configured 2:1 level constraint.
    /// This is a pure geometry check; it does not inspect tile residency.
    pub fn cover_is_valid(&self, cover: &[CubePatchAddress]) -> bool {
        valid_cover(&self.config.roots, cover, self.config.max_level)
    }

    /// Publish a complete resident 2:1 cover. No partial or hole-bearing cover
    /// can replace the current drawable set.
    pub fn ack_drawable(&mut self, cover: &[CubePatchAddress]) -> Result<(), RegionalError> {
        if self.publications_this_tick >= self.config.publication_cap_per_tick {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        if !valid_cover(&self.config.roots, cover, self.config.max_level) {
            return Err(RegionalError::InvalidCover);
        }
        if cover.iter().any(|address| !self.resident.contains(address)) {
            return Err(RegionalError::NotResident);
        }
        let changed = cover
            .iter()
            .filter(|address| !self.drawable.contains(address))
            .count()
            + self
                .drawable
                .iter()
                .filter(|address| !cover.contains(address))
                .count();
        if changed.div_ceil(5) > self.config.transition_cap {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        self.drawable = cover.to_vec();
        self.drawable.sort();
        if self.reserved_split_parent.is_some_and(|parent| {
            !self.drawable.contains(&parent)
                && parent.children().is_ok_and(|children| {
                    children.iter().all(|child| self.drawable.contains(child))
                })
        }) {
            self.reserved_split_parent = None;
        }
        self.publications_this_tick += 1;
        self.refresh_publications();
        Ok(())
    }

    /// Additional caller-owned pins protect transition/fallback dependencies.
    pub fn set_external_pins(&mut self, addresses: &[CubePatchAddress]) {
        self.external_pins.clear();
        self.external_pins.extend(addresses.iter().copied());
    }

    pub fn snapshot(&self) -> RegionalSnapshot {
        let queued_upload_addresses: HashSet<_> =
            self.uploads.iter().map(|upload| upload.address).collect();
        let desired: Vec<_> = self
            .desired
            .iter()
            .map(|(address, patch)| {
                let state = if self.drawable.contains(address) {
                    "drawable"
                } else if self.resident.contains(address) {
                    "resident"
                } else if queued_upload_addresses.contains(address) {
                    "queued_upload"
                } else if self
                    .key_for(*address)
                    .ok()
                    .is_some_and(|key| self.cache.contains_key(&key))
                {
                    "built_cpu"
                } else if let Some(token) = self
                    .in_flight
                    .iter()
                    .find_map(|(key, token)| (key.address == *address).then_some(token))
                {
                    if token.cancelled.load(AtomicOrdering::Acquire) {
                        "absent"
                    } else if token.started.load(AtomicOrdering::Acquire) {
                        "building"
                    } else {
                        "requested"
                    }
                } else {
                    "absent"
                };
                let mut snapshot = patch.clone();
                snapshot.state = state;
                snapshot
            })
            .collect();
        // Normalize by this regional cover's own configured-root area so a
        // deep root reports pixels, not its tiny fraction of an entire face.
        let root_area = self
            .config
            .roots
            .iter()
            .map(|root| patch_area(*root))
            .sum::<f64>()
            .max(f64::MIN_POSITIVE);
        let desired_error = self
            .desired
            .iter()
            .map(|(address, patch)| patch_area(*address) * patch.projected_error_px)
            .sum::<f64>()
            / root_area;
        let drawable_error = self
            .drawable
            .iter()
            .map(|address| patch_area(*address) * self.score(*address).0)
            .sum::<f64>()
            / root_area;
        let area_debt = (drawable_error - desired_error).max(0.0);
        let running = self.worker_running.load(AtomicOrdering::Acquire);
        let in_flight = self.in_flight.len();
        let upload_queue = self
            .uploads
            .iter()
            .map(|upload| RegionalQueueSnapshot {
                address: format!("{:?}", upload.address),
                bytes: upload.bytes,
                priority: self
                    .desired
                    .get(&upload.address)
                    .map(|patch| patch.priority)
                    .unwrap_or_else(|| self.score(upload.address).1),
            })
            .collect();
        let mut build_requests: Vec<_> = self
            .in_flight
            .iter()
            .map(|(key, token)| RegionalRequestSnapshot {
                address: format!("{:?}", key.address),
                priority: self
                    .desired
                    .get(&key.address)
                    .map(|patch| patch.priority)
                    .unwrap_or_else(|| self.score(key.address).1),
                state: if token.cancelled.load(AtomicOrdering::Acquire) {
                    "cancelled"
                } else if token.started.load(AtomicOrdering::Acquire) {
                    "building"
                } else {
                    "requested"
                },
                cancelled: token.cancelled.load(AtomicOrdering::Acquire),
            })
            .collect();
        build_requests.sort_by(|a, b| {
            b.priority
                .total_cmp(&a.priority)
                .then(a.address.cmp(&b.address))
        });
        let upload_backlog_addresses: HashSet<_> =
            self.uploads.iter().map(|upload| upload.address).collect();
        let estimated_completed_unpublished_bytes = self
            .desired
            .keys()
            .filter(|address| {
                !self.resident.contains(address) && !upload_backlog_addresses.contains(address)
            })
            .filter_map(|address| self.key_for(*address).ok())
            .filter_map(|key| self.cache.get(&key))
            .map(|entry| entry.bytes)
            .sum();
        let resident: Vec<_> = self.resident.iter().map(|a| format!("{a:?}")).collect();
        let drawable: Vec<_> = self.drawable.iter().map(|a| format!("{a:?}")).collect();
        RegionalSnapshot {
            tick: self.tick,
            configuration: RegionalConfigSnapshot {
                roots: self
                    .config
                    .roots
                    .iter()
                    .map(|root| format!("{root:?}"))
                    .collect(),
                max_level: self.config.max_level,
                cells: self.config.cells,
                max_desired_patches: self.config.max_desired_patches,
                worker_count: self.config.worker_count,
                worker_delay_millis: self
                    .config
                    .worker_delay
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                queue_cap: self.config.queue_cap,
                completion_cap: self.config.completion_cap,
                admission_cap_per_tick: self.config.admission_cap_per_tick,
                upload_tile_cap: self.config.upload_tile_cap,
                upload_byte_cap: self.config.upload_byte_cap,
                publication_cap_per_tick: self.config.publication_cap_per_tick,
                transition_cap: self.config.transition_cap,
                split_threshold_px: self.config.split_threshold_px,
                merge_threshold_px: self.config.merge_threshold_px,
                prediction_seconds: self.config.prediction_seconds,
                high_speed_mps: self.config.high_speed_mps,
            },
            desired_count: desired.len(),
            desired,
            resident,
            drawable,
            upload_queue,
            build_requests,
            resident_count: self.resident.len(),
            drawable_count: self.drawable.len(),
            cpu_cached_tiles: self.cache.len(),
            cpu_cached_bytes: self.cache_bytes,
            worker_queued: in_flight.saturating_sub(running),
            worker_running: running,
            completion_backlog: self.completion_backlog.load(AtomicOrdering::Acquire),
            completion_drain_micros: self.stats.completion_drain_micros,
            selection_time_micros: self.stats.selection_time_micros,
            scheduler_time_micros: self.stats.scheduler_time_micros,
            cpu_tile_cap: self.config.cpu_tile_cap,
            cpu_byte_cap: self.config.cpu_byte_cap,
            max_desired_patches: self.config.max_desired_patches,
            worker_queue_capacity: self.config.queue_cap,
            completion_capacity: self.config.completion_cap,
            upload_tile_capacity: self.config.upload_tile_cap,
            upload_byte_capacity: self.config.upload_byte_cap,
            estimated_worker_scratch_bytes: self
                .generator
                .query_workspace_bytes()
                .saturating_mul(self.config.worker_count),
            upload_backlog_tiles: self.uploads.len(),
            upload_backlog_bytes: self.upload_bytes,
            estimated_completed_unpublished_bytes,
            publication_candidates: self.publications.len(),
            active_transitions_reported: 0,
            desired_projected_error_px: desired_error,
            drawable_projected_error_px: drawable_error,
            refinement_debt: area_debt,
            queue_pressure: self.pressure.queue,
            cpu_cache_pressure: self.pressure.cpu,
            upload_pressure: self.pressure.upload,
            publication_pressure: self.pressure.publication,
            desired_capacity_pressure: self.pressure.desired_capacity,
            selector_fixed_point_reused: self.selector_fixed_point_reused,
            requests_issued: self.stats.requests_issued,
            jobs_started: self.stats.jobs_started,
            cancelled_before_start: self.stats.cancelled_before_start,
            cancelled_during_work: self.stats.cancelled_during_work,
            stale_completions: self.stats.stale_completions,
            completed_unused_tiles: self.stats.completed_unused_tiles,
            bytes_built_but_unused: self.stats.bytes_built_but_unused,
            build_time_discarded_micros: self.stats.build_time_discarded_micros,
            build_time_micros: self.stats.build_time_micros,
            worker_elapsed_micros: self.stats.worker_elapsed_micros,
            recent_build_elapsed_micros: self
                .stats
                .recent_build_elapsed_micros
                .iter()
                .copied()
                .collect(),
            build_failures: self.stats.build_failures,
            cache_hits: self.stats.cache_hits,
            cache_misses: self.stats.cache_misses,
            cache_evictions: self.stats.cache_evictions,
            rebuilds: self.stats.rebuilds,
            reuploads: self.stats.reuploads,
            last_admitted_priorities: self.last_priorities.clone(),
            selector_error_model: "spherical grid sagitta plus local relief residual; closest angular patch footprint for distance; heuristic, not sampled-error certificate",
        }
    }

    fn key_for(&self, address: CubePatchAddress) -> Result<TileKey, RegionalError> {
        ResidentTileBuilder::tile_key(&self.generator, self.identity, address, self.config.cells)
            .map_err(|_| RegionalError::TileBuild)
    }

    fn touch_pinned_cache_entries(&mut self) {
        let pinned = self.pinned_addresses();
        for entry in self.cache.values_mut() {
            if pinned.contains(&entry.tile.key.address) {
                self.cache_clock = self.cache_clock.saturating_add(1);
                entry.last_use = self.cache_clock;
            }
        }
    }

    fn select_desired(&mut self) {
        self.selector_fixed_point_reused = false;
        if self.selector_can_reuse_fixed_point && self.last_selection_view == Some(self.view) {
            self.pressure.desired_capacity = self.selector_last_capacity_pressure;
            self.selector_fixed_point_reused = true;
            return;
        }

        let mut cover = self.config.roots.clone();
        cover.sort();
        let previous: BTreeSet<_> = self.desired.keys().copied().collect();
        let previous_cover: Vec<_> = previous.iter().copied().collect();
        let mut previously_refined = HashSet::with_capacity(previous.len());
        for &old in &previous {
            let mut ancestor = old.parent();
            while let Some(address) = ancestor {
                previously_refined.insert(address);
                ancestor = address.parent();
            }
        }
        let mut score_cache = HashMap::new();
        let mut selected_priorities = Vec::new();

        loop {
            let mut candidates = Vec::new();
            for address in &cover {
                let (error, priority, approach, speed) =
                    self.selection_score(*address, &mut score_cache);
                let was_refined = previously_refined.contains(address);
                let threshold = if was_refined {
                    self.config.merge_threshold_px
                } else {
                    self.config.split_threshold_px
                };
                if address.level() >= self.config.max_level || error <= threshold {
                    continue;
                }
                if self.too_transient(*address) {
                    continue;
                }
                let Ok(children) = address.children() else {
                    continue;
                };
                let total = priority;
                candidates.push((total, error, *address, children, approach, speed));
            }
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
            let mut accepted = None;
            let had_split_candidate = !candidates.is_empty();
            for (total, error, parent, children, approach, speed) in candidates {
                let mut proposed: Vec<_> = cover
                    .iter()
                    .copied()
                    .filter(|item| *item != parent)
                    .collect();
                proposed.extend(children);
                if let Some(balanced) = balance_cover(
                    &self.config.roots,
                    proposed,
                    self.config.max_level,
                    self.config.max_desired_patches,
                ) && balanced.len() > cover.len()
                {
                    accepted = Some((balanced, total, error, parent, approach, speed));
                    break;
                }
            }
            let Some((next, total, error, parent, approach, speed)) = accepted else {
                self.pressure.desired_capacity |= had_split_candidate;
                break;
            };
            selected_priorities.push(RegionalPrioritySnapshot {
                address: format!("{parent:?}"),
                projected_error_px: error,
                approach_multiplier: approach,
                high_speed_multiplier: speed,
                total,
            });
            cover = next;
        }

        self.desired.clear();
        for address in &cover {
            let (error, priority, approach, speed) =
                self.selection_score(*address, &mut score_cache);
            self.desired.insert(
                *address,
                RegionalPatchSnapshot {
                    address: format!("{address:?}"),
                    level: address.level(),
                    projected_error_px: error,
                    priority,
                    state: "absent",
                },
            );
            if self.last_priorities.len() < self.config.admission_cap_per_tick {
                let record = RegionalPrioritySnapshot {
                    address: format!("{address:?}"),
                    projected_error_px: error,
                    approach_multiplier: approach,
                    high_speed_multiplier: speed,
                    total: priority,
                };
                if !selected_priorities
                    .iter()
                    .any(|entry| entry.address == record.address)
                {
                    selected_priorities.push(record);
                }
            }
        }
        selected_priorities
            .sort_by(|a, b| b.total.total_cmp(&a.total).then(a.address.cmp(&b.address)));
        self.last_priorities = selected_priorities;
        self.desired_addresses = self.desired.keys().copied().collect();
        let active_addresses: HashSet<_> = self.desired.keys().copied().collect();
        self.seen_cache_requests
            .retain(|key| active_addresses.contains(&key.address));
        self.selector_can_reuse_fixed_point = self.desired_addresses == previous_cover;
        self.selector_last_capacity_pressure = self.pressure.desired_capacity;
        self.last_selection_view = Some(self.view);
    }

    fn selection_score(
        &self,
        address: CubePatchAddress,
        cache: &mut HashMap<CubePatchAddress, (f64, f64, f64, f64)>,
    ) -> (f64, f64, f64, f64) {
        *cache.entry(address).or_insert_with(|| self.score(address))
    }

    fn score(&self, address: CubePatchAddress) -> (f64, f64, f64, f64) {
        let [normal, u, v] = address.face().basis();
        let [x, y] = address.coordinates();
        let scale = (1u64 << address.level()) as f64;
        let center = (normal
            + u * (2.0 * (f64::from(x) + 0.5) / scale - 1.0)
            + v * (2.0 * (f64::from(y) + 0.5) / scale - 1.0))
            .normalize();
        let radius = self.generator.radius_m();
        let center_m = center * radius;
        let to_patch = center_m - self.view.body_position_m;
        let distance = to_patch.length().max(radius * 1.0e-9);
        let angular_width = 2.0 / scale;
        let grid_step = angular_width / f64::from(self.config.cells);
        let half_cell_diagonal = (2.0f64).sqrt() * grid_step * 0.5;
        let patch_half_diagonal = (2.0f64).sqrt() * angular_width * 0.5;
        let curvature_m = radius * (1.0 - half_cell_diagonal.cos());
        // Relief is a patch-scale envelope proxy, not a derivative bound; it
        // decreases as subdivision narrows the region. Cell spacing controls
        // the separate spherical curvature term above.
        let authored_relief_m = self.generator.radius_m()
            * self
                .generator
                .definition()
                .terrain()
                .parameters()
                .relief_fraction
            * 0.25;
        let relief_envelope_m =
            authored_relief_m.min(self.generator.conservative_absolute_height_bound_m());
        let relief_m = relief_envelope_m * patch_half_diagonal.sin().abs();
        let geometric_error_m = curvature_m + relief_m;
        let approach_speed = self
            .view
            .body_velocity_mps
            .dot(to_patch / distance)
            .max(0.0);
        // Approximate the closest point on this patch's angular footprint. A
        // global absolute-height bound here overwhelms the footprint distance
        // for every patch at low altitude and destroys spatial LOD variation.
        let camera_radius = self.view.body_position_m.length();
        let clearance = (camera_radius - radius).max(1.0);
        let camera_direction = self.view.body_position_m.normalize_or_zero();
        let patch_angle = camera_direction.dot(center).clamp(-1.0, 1.0).acos();
        let nearest_angle = (patch_angle - patch_half_diagonal).max(0.0);
        let local_surface_radius = radius + relief_m;
        let closest_surface_distance = (camera_radius * camera_radius
            + local_surface_radius * local_surface_radius
            - 2.0 * camera_radius * local_surface_radius * nearest_angle.cos())
        .max(0.0)
        .sqrt();
        let nearest_patch_distance = closest_surface_distance.max(clearance);
        let predicted_distance = (nearest_patch_distance
            - approach_speed * self.config.prediction_seconds)
            .max(clearance);
        let error_px = geometric_error_m * self.view.projection_scale_px / predicted_distance;
        let approach_multiplier = 1.0
            + (approach_speed * self.config.prediction_seconds / nearest_patch_distance.max(1.0))
                .clamp(0.0, 2.0);
        let speed = self.view.body_velocity_mps.length();
        let high_speed_multiplier = if speed > self.config.high_speed_mps && address.level() > 0 {
            (self.config.high_speed_mps / speed).clamp(0.15, 1.0)
        } else {
            1.0
        };
        let priority = error_px * approach_multiplier * high_speed_multiplier;
        (
            error_px,
            priority,
            approach_multiplier,
            high_speed_multiplier,
        )
    }

    fn too_transient(&self, address: CubePatchAddress) -> bool {
        let speed = self.view.body_velocity_mps.length();
        if speed <= self.config.high_speed_mps || address.level() == 0 {
            return false;
        }
        let angular_width = 2.0 / (1u64 << address.level()) as f64;
        let patch_width_m = self.generator.radius_m() * angular_width;
        patch_width_m / speed < self.config.prediction_seconds * 0.25
    }

    fn cancel_obsolete_work(&mut self) {
        let mut needed = self.scheduled_dependencies();
        needed.extend(self.external_pins.iter().copied());
        for (key, token) in &self.in_flight {
            if !needed.contains(&key.address) && !token.cancelled.swap(true, AtomicOrdering::AcqRel)
            {
                if token.started.load(AtomicOrdering::Acquire) {
                    self.stats.cancelled_during_work =
                        self.stats.cancelled_during_work.saturating_add(1);
                } else {
                    self.stats.cancelled_before_start =
                        self.stats.cancelled_before_start.saturating_add(1);
                }
            }
        }
    }

    fn admit_builds(&mut self) {
        let merge_frontier = self.merge_parent_dependencies();
        let addresses: Vec<_> = self.scheduled_dependencies().into_iter().collect();
        let mut candidates = Vec::new();
        for address in addresses {
            if self.resident.contains(&address) {
                continue;
            }
            if self.reserved_split_parent.is_some_and(|parent| {
                !self.drawable.contains(&address)
                    && !merge_frontier.contains(&address)
                    && parent
                        .children()
                        .is_ok_and(|children| !children.contains(&address))
            }) {
                continue;
            }
            let in_current_frontier = if self.drawable.is_empty() {
                self.config.roots.contains(&address)
            } else {
                self.drawable.contains(&address)
                    || merge_frontier.contains(&address)
                    || address
                        .parent()
                        .is_some_and(|parent| self.drawable.contains(&parent))
            };
            if !in_current_frontier {
                continue;
            }
            let Ok(key) = self.key_for(address) else {
                continue;
            };
            if self.cache.contains_key(&key) {
                if self.seen_cache_requests.insert(key) {
                    self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
                }
                continue;
            }
            if self.in_flight.contains_key(&key)
                || self.uploads.iter().any(|upload| upload.address == address)
            {
                continue;
            }
            let score = self
                .desired
                .get(&address)
                .map(|patch| patch.priority)
                .unwrap_or_else(|| self.score(address).1);
            candidates.push((score, address, key));
        }
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        if !candidates.is_empty() && !self.cpu_cache_can_admit_tile() {
            self.pressure.cpu = true;
            return;
        }
        self.pressure.queue = self.in_flight.len() >= self.config.queue_cap;
        let mut admitted = 0;
        self.last_priorities.clear();
        for (priority, address, key) in candidates {
            if admitted >= self.config.admission_cap_per_tick {
                break;
            }
            let token = Arc::new(BuildToken {
                cancelled: AtomicBool::new(false),
                started: AtomicBool::new(false),
            });
            let job = BuildJob {
                address,
                key: key.clone(),
                token: Arc::clone(&token),
            };
            match self.workers.sender.try_send(job) {
                Ok(()) => {
                    self.in_flight.insert(key, token);
                    self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
                    self.stats.requests_issued = self.stats.requests_issued.saturating_add(1);
                    admitted += 1;
                    let (error, _, approach, speed) = self.score(address);
                    self.last_priorities.push(RegionalPrioritySnapshot {
                        address: format!("{address:?}"),
                        projected_error_px: error,
                        approach_multiplier: approach,
                        high_speed_multiplier: speed,
                        total: priority,
                    });
                }
                Err(TrySendError::Full(_)) => {
                    self.pressure.queue = true;
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.pressure.queue = true;
                    break;
                }
            }
        }
        if candidates_remain(&self.desired, &self.cache, &self.in_flight, &self.resident) {
            self.pressure.queue |= admitted == self.config.admission_cap_per_tick;
        }
    }

    fn drain_completions(&mut self) {
        let drain_started = std::time::Instant::now();
        let needed = self.scheduled_dependencies();
        while let Ok(result) = self.completions.try_recv() {
            self.completion_backlog.fetch_sub(1, AtomicOrdering::AcqRel);
            self.in_flight.remove(&result.key);
            self.stats.worker_elapsed_micros = self
                .stats
                .worker_elapsed_micros
                .saturating_add(result.elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
            self.stats
                .recent_build_elapsed_micros
                .push_back(result.elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
            if self.stats.recent_build_elapsed_micros.len() > 256 {
                self.stats.recent_build_elapsed_micros.pop_front();
            }
            let still_needed =
                needed.contains(&result.address) || self.external_pins.contains(&result.address);
            if result.token.started.load(AtomicOrdering::Acquire) {
                self.stats.jobs_started = self.stats.jobs_started.saturating_add(1);
            }
            if result.token.cancelled.load(AtomicOrdering::Acquire) && !still_needed {
                self.stats.stale_completions = self.stats.stale_completions.saturating_add(1);
            }
            if let Some((tile, diagnostics)) = result.result {
                self.stats.rebuilds = self.stats.rebuilds.saturating_add(1);
                self.stats.build_time_micros = self.stats.build_time_micros.saturating_add(
                    diagnostics.elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
                );
                if !still_needed {
                    self.stats.completed_unused_tiles =
                        self.stats.completed_unused_tiles.saturating_add(1);
                }
                let bytes = tile_bytes(&tile);
                let build_elapsed_micros =
                    diagnostics.elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
                match self.cache_insert(tile, !still_needed, Some(build_elapsed_micros)) {
                    CacheInsertResult::Inserted => {}
                    CacheInsertResult::AlreadyCached | CacheInsertResult::Rejected => {
                        self.record_discarded_build(bytes, build_elapsed_micros);
                    }
                }
            } else if !result.token.cancelled.load(AtomicOrdering::Acquire) {
                self.stats.build_failures = self.stats.build_failures.saturating_add(1);
            }
        }
        self.stats.completion_drain_micros = drain_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
    }

    fn admit_uploads(&mut self) {
        let merge_frontier = self.merge_parent_dependencies();
        let queued_addresses: HashSet<_> =
            self.uploads.iter().map(|upload| upload.address).collect();
        let mut wanted: BTreeSet<_> = self.desired.keys().copied().collect();
        wanted.extend(self.config.roots.iter().copied());
        wanted.extend(self.scheduled_dependencies());
        let priorities: BTreeMap<_, _> = self
            .desired
            .iter()
            .map(|(address, patch)| (*address, patch.priority))
            .collect();
        let mut candidates: Vec<_> = wanted
            .into_iter()
            .filter(|address| {
                !self.resident.contains(address) && !queued_addresses.contains(address)
            })
            .filter(|address| {
                if self.drawable.is_empty() {
                    self.config.roots.contains(address)
                } else {
                    self.drawable.contains(address)
                        || merge_frontier.contains(address)
                        || address
                            .parent()
                            .is_some_and(|parent| self.drawable.contains(&parent))
                }
            })
            .filter(|address| {
                self.reserved_split_parent.is_none_or(|parent| {
                    self.drawable.contains(address)
                        || merge_frontier.contains(address)
                        || parent
                            .children()
                            .is_ok_and(|children| children.contains(address))
                })
            })
            .filter(|address| {
                self.upload_allowlist
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(address))
            })
            .filter_map(|address| {
                let key = self.key_for(address).ok()?;
                let priority = priorities
                    .get(&address)
                    .copied()
                    .unwrap_or_else(|| self.score(address).1);
                self.cache
                    .get(&key)
                    .map(|entry| (priority, address, Arc::clone(&entry.tile)))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, address, tile) in candidates {
            if self.uploads.len() >= self.config.upload_tile_cap {
                self.pressure.upload = true;
                break;
            }
            let bytes = tile_bytes(&tile);
            if self.upload_bytes.saturating_add(bytes) > self.config.upload_byte_cap {
                self.pressure.upload = true;
                continue;
            }
            self.uploads.push(RegionalUpload {
                address,
                tile,
                bytes,
            });
            self.upload_bytes += bytes;
        }
    }

    fn refresh_publications(&mut self) {
        self.publications.clear();
        let publication_limit = self
            .config
            .publication_cap_per_tick
            .min(self.config.transition_cap);
        if self.drawable.is_empty() || publication_limit == 0 {
            return;
        }
        for &parent in &self.drawable {
            if !self
                .desired
                .keys()
                .any(|desired| parent.contains(*desired) && *desired != parent)
            {
                continue;
            }
            let Ok(children) = parent.children() else {
                continue;
            };
            if !self.resident.contains(&parent)
                || children.iter().any(|child| !self.resident.contains(child))
            {
                continue;
            }
            let mut cover: Vec<_> = self
                .drawable
                .iter()
                .copied()
                .filter(|address| *address != parent)
                .collect();
            cover.extend(children);
            cover.sort();
            if valid_cover(&self.config.roots, &cover, self.config.max_level) {
                self.publications.push(RegionalPublication::Split {
                    parent,
                    children,
                    cover,
                });
            }
        }
        let parents: BTreeSet<_> = self
            .drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect();
        for parent in parents {
            if !self.desired.keys().any(|desired| desired.contains(parent)) {
                continue;
            }
            let Ok(children) = parent.children() else {
                continue;
            };
            if !self.resident.contains(&parent)
                || children.iter().any(|child| !self.drawable.contains(child))
            {
                continue;
            }
            let mut cover: Vec<_> = self
                .drawable
                .iter()
                .copied()
                .filter(|address| !children.contains(address))
                .collect();
            cover.push(parent);
            cover.sort();
            if valid_cover(&self.config.roots, &cover, self.config.max_level) {
                self.publications.push(RegionalPublication::Merge {
                    parent,
                    children,
                    cover,
                });
            }
        }
        self.publications.sort_by(|a, b| {
            publication_parent(a)
                .cmp(&publication_parent(b))
                .then_with(|| match (a, b) {
                    (RegionalPublication::Split { .. }, RegionalPublication::Merge { .. }) => {
                        Ordering::Less
                    }
                    (RegionalPublication::Merge { .. }, RegionalPublication::Split { .. }) => {
                        Ordering::Greater
                    }
                    _ => Ordering::Equal,
                })
        });
        self.publications.truncate(publication_limit);
    }

    fn cache_insert(
        &mut self,
        tile: Arc<TileData>,
        count_hit: bool,
        build_elapsed_micros: Option<u64>,
    ) -> CacheInsertResult {
        let key = tile.key.clone();
        if self.cache.contains_key(&key) {
            if count_hit {
                self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
            }
            self.cache_clock = self.cache_clock.saturating_add(1);
            if let Some(entry) = self.cache.get_mut(&key) {
                entry.last_use = self.cache_clock;
            }
            return CacheInsertResult::AlreadyCached;
        }
        let bytes = tile_bytes(&tile);
        if bytes > self.config.cpu_byte_cap || self.config.cpu_tile_cap == 0 {
            self.pressure.cpu = true;
            return CacheInsertResult::Rejected;
        }
        while self.cache.len() >= self.config.cpu_tile_cap
            || self.cache_bytes.saturating_add(bytes) > self.config.cpu_byte_cap
        {
            let pins = self.pinned_addresses();
            let victim = self
                .cache
                .iter()
                .filter(|(_, entry)| !pins.contains(&entry.tile.key.address))
                .min_by_key(|(_, entry)| entry.last_use)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else {
                self.pressure.cpu = true;
                return CacheInsertResult::Rejected;
            };
            if let Some(removed) = self.cache.remove(&victim) {
                self.cache_bytes = self.cache_bytes.saturating_sub(removed.bytes);
                self.seen_cache_requests.remove(&victim);
                self.stats.cache_evictions = self.stats.cache_evictions.saturating_add(1);
                if self.resident.remove(&removed.tile.key.address) {
                    if self.drawable.contains(&removed.tile.key.address) {
                        self.pressure.publication = true;
                    }
                    self.refresh_publications();
                }
                if !removed.ever_resident {
                    self.record_discarded_build(
                        removed.bytes,
                        removed.build_elapsed_micros.unwrap_or(0),
                    );
                }
            }
        }
        self.cache_clock = self.cache_clock.saturating_add(1);
        self.cache_bytes += bytes;
        self.cache.insert(
            key,
            CacheEntry {
                tile,
                bytes,
                last_use: self.cache_clock,
                build_elapsed_micros,
                ever_resident: false,
            },
        );
        CacheInsertResult::Inserted
    }

    fn record_discarded_build(&mut self, bytes: usize, build_elapsed_micros: u64) {
        self.stats.bytes_built_but_unused = self
            .stats
            .bytes_built_but_unused
            .saturating_add(bytes as u64);
        self.stats.build_time_discarded_micros = self
            .stats
            .build_time_discarded_micros
            .saturating_add(build_elapsed_micros);
    }

    fn cpu_cache_can_admit_tile(&self) -> bool {
        let Some(tile_bytes) = expected_tile_bytes(self.config.cells) else {
            return false;
        };
        if tile_bytes > self.config.cpu_byte_cap || self.config.cpu_tile_cap == 0 {
            return false;
        }
        if self.cache.len() < self.config.cpu_tile_cap
            && self.cache_bytes.saturating_add(tile_bytes) <= self.config.cpu_byte_cap
        {
            return true;
        }

        // LRU can make room only by evicting unpinned entries. If the pinned
        // remainder plus one exact-size tile exceeds either cap, further jobs
        // cannot be retained; stop admitting duplicates of known failures.
        let pinned = self.pinned_addresses();
        let (pinned_tiles, pinned_bytes) = self
            .cache
            .values()
            .filter(|entry| pinned.contains(&entry.tile.key.address))
            .fold((0usize, 0usize), |(count, bytes), entry| {
                (count.saturating_add(1), bytes.saturating_add(entry.bytes))
            });
        pinned_tiles.saturating_add(1) <= self.config.cpu_tile_cap
            && pinned_bytes.saturating_add(tile_bytes) <= self.config.cpu_byte_cap
    }

    fn pinned_addresses(&self) -> HashSet<CubePatchAddress> {
        if self.reserved_split_parent.is_some() {
            let mut pins: HashSet<_> = self
                .config
                .roots
                .iter()
                .chain(self.drawable.iter())
                .chain(self.external_pins.iter())
                .chain(self.uploads.iter().map(|upload| &upload.address))
                .copied()
                .collect();
            pins.extend(self.merge_parent_dependencies());
            if let Some(parent) = self.reserved_split_parent
                && let Ok(children) = parent.children()
            {
                pins.extend(children);
            }
            return pins;
        }
        let mut pins: HashSet<_> = self
            .desired
            .keys()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .chain(self.uploads.iter().map(|upload| &upload.address))
            .copied()
            .collect();
        pins.extend(self.dependency_addresses());
        pins
    }

    fn scheduled_dependencies(&self) -> BTreeSet<CubePatchAddress> {
        let mut dependencies = self.dependency_addresses();
        let Some(parent) = self.reserved_split_parent else {
            return dependencies;
        };
        let mut retained: BTreeSet<_> = self
            .config
            .roots
            .iter()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .copied()
            .collect();
        retained.extend(self.merge_parent_dependencies());
        if let Ok(children) = parent.children() {
            retained.extend(children);
        }
        dependencies.retain(|address| retained.contains(address));
        dependencies.extend(retained);
        dependencies
    }

    /// When the CPU cache cannot hold every sibling frontier, serialize split
    /// work around one complete local group. This prevents aggregate per-tile
    /// priority from leaving several parents with two cached children each.
    fn update_split_reservation(&mut self) {
        let groups = self.split_frontier_groups();
        if groups.is_empty() {
            self.reserved_split_parent = None;
            return;
        }

        let mut anchors: BTreeSet<_> = self
            .config
            .roots
            .iter()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .chain(self.uploads.iter().map(|upload| &upload.address))
            .copied()
            .collect();
        anchors.extend(self.merge_parent_dependencies());
        let all_children: BTreeSet<_> = groups.iter().flat_map(|group| group.children).collect();
        let Some(tile_bytes) = expected_tile_bytes(self.config.cells) else {
            self.reserved_split_parent = None;
            return;
        };
        let required = anchors.union(&all_children).count();
        let constrained = required > self.config.cpu_tile_cap
            || required.saturating_mul(tile_bytes) > self.config.cpu_byte_cap;
        if !constrained {
            self.reserved_split_parent = None;
            return;
        }

        if let Some(parent) = self.reserved_split_parent
            && groups.iter().any(|group| group.parent == parent)
        {
            return;
        }

        let mut candidates = groups;
        candidates.sort_by(|a, b| {
            b.parent_resident
                .cmp(&a.parent_resident)
                .then_with(|| b.ready_children.cmp(&a.ready_children))
                .then_with(|| b.aggregate_priority.total_cmp(&a.aggregate_priority))
                .then_with(|| a.parent.cmp(&b.parent))
        });
        self.reserved_split_parent = candidates
            .into_iter()
            .find(|group| {
                let needed = anchors.union(&group.children.into_iter().collect()).count();
                needed <= self.config.cpu_tile_cap
                    && needed.saturating_mul(tile_bytes) <= self.config.cpu_byte_cap
            })
            .map(|group| group.parent);
    }

    fn split_frontier_groups(&self) -> Vec<RegionalSplitFrontier> {
        let mut groups = Vec::new();
        for &parent in &self.drawable {
            let Ok(children) = parent.children() else {
                continue;
            };
            let child_priorities: Vec<_> = children
                .iter()
                .map(|child| {
                    self.desired
                        .iter()
                        .filter(|(desired, _)| child.contains(**desired))
                        .map(|(_, patch)| patch.priority)
                        .max_by(f64::total_cmp)
                })
                .collect();
            if child_priorities.iter().any(Option::is_none) {
                continue;
            }
            let mut trial_cover: Vec<_> = self
                .drawable
                .iter()
                .copied()
                .filter(|address| *address != parent)
                .collect();
            trial_cover.extend(children);
            if !valid_cover(&self.config.roots, &trial_cover, self.config.max_level) {
                continue;
            }
            let aggregate_priority = child_priorities.iter().flatten().copied().sum::<f64>();
            let ready_children = children
                .iter()
                .filter(|child| {
                    self.resident.contains(child)
                        || self
                            .key_for(**child)
                            .is_ok_and(|key| self.cache.contains_key(&key))
                        || self.uploads.iter().any(|upload| upload.address == **child)
                        || self
                            .key_for(**child)
                            .is_ok_and(|key| self.in_flight.contains_key(&key))
                })
                .count();
            groups.push(RegionalSplitFrontier {
                parent,
                children,
                parent_resident: self.resident.contains(&parent),
                ready_children,
                aggregate_priority,
            });
        }
        groups.sort_by(|a, b| {
            b.aggregate_priority
                .total_cmp(&a.aggregate_priority)
                .then_with(|| a.parent.cmp(&b.parent))
        });
        groups
    }

    /// Desired descendants cannot publish directly: every ancestor level must
    /// become resident first so split proposals can advance from the current
    /// drawable cover without holes. Keep that bounded path schedulable and
    /// cancellable as one dependency closure.
    fn dependency_addresses(&self) -> BTreeSet<CubePatchAddress> {
        let mut dependencies = BTreeSet::new();
        for &desired in self.desired.keys() {
            let mut cursor = desired;
            loop {
                if !self.config.roots.iter().any(|root| root.contains(cursor)) {
                    break;
                }
                dependencies.insert(cursor);
                if self.config.roots.contains(&cursor) {
                    break;
                }
                let Some(parent) = cursor.parent() else {
                    break;
                };
                cursor = parent;
            }
        }
        dependencies.extend(self.merge_parent_dependencies());
        dependencies
    }

    /// Coarsening an already drawable four-sibling group needs its parent tile
    /// resident even though that parent is not a child of the drawable cover.
    /// Include only complete local groups with a desired ancestor so retreat
    /// repair stays bounded to actual merge candidates.
    fn merge_parent_dependencies(&self) -> BTreeSet<CubePatchAddress> {
        let drawable: HashSet<_> = self.drawable.iter().copied().collect();
        self.drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|parent| self.desired.keys().any(|desired| desired.contains(*parent)))
            .filter(|parent| {
                parent
                    .children()
                    .is_ok_and(|children| children.iter().all(|child| drawable.contains(child)))
            })
            .collect()
    }
}

impl Drop for RegionalTerrain {
    fn drop(&mut self) {
        // Queued jobs are cheap to abandon. Running workers observe cancellation
        // before build and the dropped result receiver releases bounded senders.
        for token in self.in_flight.values() {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
    }
}

fn validate_config(config: &RegionalConfig) -> Result<(), RegionalError> {
    if config.roots.is_empty()
        || config.max_level > 30
        || config
            .roots
            .iter()
            .any(|root| root.level() > config.max_level)
        || config.cells == 0
        || !config.cells.is_power_of_two()
        || config.cells > TileData::MAX_CELLS
        || config.cpu_tile_cap == 0
        || config.cpu_byte_cap == 0
        || config.worker_count == 0
        || config.worker_count > 32
        || config.queue_cap == 0
        || config.completion_cap == 0
        || config.max_desired_patches < config.roots.len()
        || config.upload_tile_cap == 0
        || config.upload_byte_cap == 0
        || config.publication_cap_per_tick == 0
        || config.transition_cap == 0
        || !config.split_threshold_px.is_finite()
        || config.split_threshold_px <= 0.0
        || !config.merge_threshold_px.is_finite()
        || config.merge_threshold_px < 0.0
        || config.merge_threshold_px >= config.split_threshold_px
        || !config.prediction_seconds.is_finite()
        || config.prediction_seconds < 0.0
        || !config.high_speed_mps.is_finite()
        || config.high_speed_mps < 0.0
        || !valid_cover(config.roots.as_slice(), &config.roots, config.max_level)
    {
        return Err(RegionalError::InvalidConfig);
    }
    Ok(())
}

fn tile_bytes(tile: &TileData) -> usize {
    tile.texels
        .len()
        .saturating_mul(std::mem::size_of::<crate::resident_terrain::TileTexel>())
}

fn patch_area(address: CubePatchAddress) -> f64 {
    4.0f64.powi(-i32::from(address.level()))
}

fn expected_tile_bytes(cells: u32) -> Option<usize> {
    let side = usize::try_from(cells.checked_add(3)?).ok()?;
    side.checked_mul(side)?
        .checked_mul(std::mem::size_of::<crate::resident_terrain::TileTexel>())
}

fn candidates_remain(
    desired: &BTreeMap<CubePatchAddress, RegionalPatchSnapshot>,
    cache: &HashMap<TileKey, CacheEntry>,
    in_flight: &HashMap<TileKey, Arc<BuildToken>>,
    resident: &BTreeSet<CubePatchAddress>,
) -> bool {
    desired.keys().any(|address| {
        !resident.contains(address)
            && !cache.keys().any(|key| key.address == *address)
            && !in_flight.keys().any(|key| key.address == *address)
    })
}

fn publication_parent(publication: &RegionalPublication) -> CubePatchAddress {
    match publication {
        RegionalPublication::Split { parent, .. } | RegionalPublication::Merge { parent, .. } => {
            *parent
        }
    }
}

fn balance_cover(
    roots: &[CubePatchAddress],
    mut cover: Vec<CubePatchAddress>,
    max_level: u8,
    patch_cap: usize,
) -> Option<Vec<CubePatchAddress>> {
    cover.sort();
    cover.dedup();
    loop {
        if cover.len() > patch_cap {
            return None;
        }
        if !complete_nonoverlapping_cover(roots, &cover, max_level) {
            return None;
        }
        let mut too_coarse = BTreeSet::new();
        for &patch in &cover {
            for edge in PatchEdge::ALL {
                for &neighbor in &cover {
                    if patch != neighbor
                        && adjacent_across_edge(patch, edge, neighbor)
                        && patch.level() > neighbor.level() + 1
                    {
                        too_coarse.insert(neighbor);
                    }
                }
            }
        }
        if too_coarse.is_empty() {
            return Some(cover);
        }
        let needed = too_coarse.len().saturating_mul(3);
        if cover.len().saturating_add(needed) > patch_cap
            || too_coarse.iter().any(|patch| patch.level() >= max_level)
        {
            return None;
        }
        let mut next = Vec::with_capacity(cover.len() + needed);
        for patch in cover {
            if too_coarse.contains(&patch) {
                next.extend(patch.children().ok()?);
            } else {
                next.push(patch);
            }
        }
        next.sort();
        cover = next;
    }
}

fn valid_cover(roots: &[CubePatchAddress], cover: &[CubePatchAddress], max_level: u8) -> bool {
    complete_nonoverlapping_cover(roots, cover, max_level)
        && cover.iter().all(|patch| {
            !cover
                .iter()
                .any(|other| patch != other && (patch.contains(*other) || other.contains(*patch)))
        })
        && cover_is_balanced(cover)
}

fn complete_nonoverlapping_cover(
    roots: &[CubePatchAddress],
    cover: &[CubePatchAddress],
    max_level: u8,
) -> bool {
    if roots.is_empty() || cover.is_empty() || cover.iter().any(|patch| patch.level() > max_level) {
        return false;
    }
    if roots.iter().enumerate().any(|(index, root)| {
        roots
            .iter()
            .skip(index + 1)
            .any(|other| root.contains(*other) || other.contains(*root))
    }) {
        return false;
    }
    for patch in cover {
        if !roots.iter().any(|root| root.contains(*patch)) {
            return false;
        }
    }
    for root in roots {
        let leaves: Vec<_> = cover
            .iter()
            .filter(|patch| root.contains(**patch))
            .collect();
        if leaves.is_empty() {
            return false;
        }
        for (index, patch) in leaves.iter().enumerate() {
            if leaves
                .iter()
                .skip(index + 1)
                .any(|other| patch.contains(**other) || other.contains(**patch))
            {
                return false;
            }
        }
        let depth = u32::from(max_level - root.level());
        let target_area = 1u128
            .checked_shl(depth.saturating_mul(2))
            .unwrap_or(u128::MAX);
        let mut area = 0u128;
        for patch in leaves {
            let patch_depth = u32::from(max_level - patch.level());
            area = area.saturating_add(
                1u128
                    .checked_shl(patch_depth.saturating_mul(2))
                    .unwrap_or(0),
            );
        }
        if area != target_area {
            return false;
        }
    }
    true
}

fn cover_is_balanced(cover: &[CubePatchAddress]) -> bool {
    for &patch in cover {
        for edge in PatchEdge::ALL {
            for &neighbor in cover {
                if patch != neighbor
                    && adjacent_across_edge(patch, edge, neighbor)
                    && patch.level().abs_diff(neighbor.level()) > 1
                {
                    return false;
                }
            }
        }
    }
    true
}

fn adjacent_across_edge(
    patch: CubePatchAddress,
    edge: PatchEdge,
    neighbor: CubePatchAddress,
) -> bool {
    let relation = patch.neighbor(edge);
    if neighbor.level() >= patch.level() {
        let mut ancestor = neighbor;
        while ancestor.level() > patch.level() {
            let Some(parent) = ancestor.parent() else {
                return false;
            };
            ancestor = parent;
        }
        if ancestor != relation.address {
            return false;
        }
        let depth = neighbor.level() - patch.level();
        let [x, y] = neighbor.coordinates();
        let side = 1u32 << depth;
        match relation.edge {
            PatchEdge::UMin => x == 0,
            PatchEdge::UMax => x + 1 == side,
            PatchEdge::VMin => y == 0,
            PatchEdge::VMax => y + 1 == side,
        }
    } else {
        let mut ancestor = patch;
        while ancestor.level() > neighbor.level() {
            let Some(parent) = ancestor.parent() else {
                return false;
            };
            ancestor = parent;
        }
        let depth = patch.level() - neighbor.level();
        let [x, y] = patch.coordinates();
        let side = 1u32 << depth;
        let lies_on_parent_edge = match edge {
            PatchEdge::UMin => x % side == 0,
            PatchEdge::UMax => x % side + 1 == side,
            PatchEdge::VMin => y % side == 0,
            PatchEdge::VMax => y % side + 1 == side,
        };
        if !lies_on_parent_edge {
            return false;
        }
        ancestor.neighbor(edge).address == neighbor
    }
}
