//! One bounded background compiler over already resident, immutable tile geometry.
use crate::{
    TileKey, cluster::*, cluster_gpu::ClusterGpu, cluster_region::*,
    regional_resident::RegionalResidentDraw,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::JoinHandle,
    time::Instant,
};

#[derive(Clone, PartialEq, Eq, Hash)]
struct Identity(TileKey, u64);
struct Job {
    identity: Identity,
    input: RegionInput,
    queued: Instant,
}
struct Completion {
    identity: Identity,
    result: Result<RegionProduct, ClusterBuildError>,
    queue_wait_micros: u64,
    build_micros: u64,
}
const PRODUCT_RESERVATION: usize = 8 * 1024 * 1024;

pub(crate) struct ClusterRuntime {
    gpu: ClusterGpu,
    jobs: Option<SyncSender<Job>>,
    completed: Option<Receiver<Completion>>,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    pending: HashSet<Identity>,
    cache: HashMap<Identity, Arc<RegionProduct>>,
    failed: HashSet<Identity>,
    draw_slots: HashMap<usize, usize>,
    report: ClusterReport,
    builds: u64,
    build_micros: u64,
    frozen_identities: Option<HashSet<Identity>>,
    selected_products: HashSet<Identity>,
    all_build_micros: u64,
    queue_wait_micros: u64,
    max_queue_wait_micros: u64,
    completed_builds: u64,
    failed_builds: u64,
    evicted_before_selection: u64,
}
impl ClusterRuntime {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        Self {
            gpu: ClusterGpu::new(device, format, layout),
            jobs: None,
            completed: None,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            pending: HashSet::new(),
            cache: HashMap::new(),
            failed: HashSet::new(),
            draw_slots: HashMap::new(),
            report: ClusterReport::default(),
            builds: 0,
            build_micros: 0,
            frozen_identities: None,
            selected_products: HashSet::new(),
            all_build_micros: 0,
            queue_wait_micros: 0,
            max_queue_wait_micros: 0,
            completed_builds: 0,
            failed_builds: 0,
            evicted_before_selection: 0,
        }
    }
    fn start(&mut self) -> bool {
        if self.worker.is_some() {
            return true;
        }
        let (jobs, receive) = mpsc::sync_channel::<Job>(BUILD_QUEUE_CAP);
        let (complete, completed) = mpsc::sync_channel(COMPLETION_CAP);
        let cancel = Arc::clone(&self.cancel);
        let worker = std::thread::Builder::new()
            .name("finite-region-cluster".into())
            .spawn(move || {
                while let Ok(job) = receive.recv() {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let queue_wait_micros = micros(job.queued);
                    let started = Instant::now();
                    let result = compile(job.input, &cancel);
                    let build_micros = micros(started);
                    if complete
                        .send(Completion {
                            identity: job.identity,
                            result,
                            queue_wait_micros,
                            build_micros,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        match worker {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => {
                self.report.reason = Some(format!("cluster worker unavailable: {error}"));
                return false;
            }
        }
        self.jobs = Some(jobs);
        self.completed = Some(completed);
        true
    }
    fn bytes(&self) -> usize {
        // GPU slots retain products too. Count unique immutable products once.
        let mut retained: HashMap<Identity, usize> = self
            .cache
            .iter()
            .map(|(id, product)| (id.clone(), product.resident_bytes))
            .collect();
        for product in self.gpu.retained_products() {
            retained.insert(
                Identity(product.input.key.clone(), product.input.boundary_version),
                product.resident_bytes,
            );
        }
        retained
            .values()
            .sum::<usize>()
            .saturating_add(self.pending.len() * PRODUCT_RESERVATION)
            .saturating_add(self.gpu.auxiliary_cpu_bytes())
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        projection: crate::CelestialProjection,
        draw: &RegionalResidentDraw,
        active: &[usize],
        mut settings: ClusterSettings,
    ) {
        self.draw_slots.clear();
        self.gpu.begin_frame();
        if !self.start() {
            return;
        }
        if settings.freeze && !self.report.freeze_active && self.report.resident_regions == 0 {
            settings.freeze = false;
            self.report.reason = Some("freeze waits for a usable resident cut".into());
        }
        if settings.mode == ClusterMode::Reference {
            self.frozen_identities = None;
            self.gpu
                .encode_selection(encoder, queue, projection, &[], settings);
            self.report = self.gpu.report();
            self.report.cpu_bytes = self.bytes();
            return;
        }
        for _ in 0..PUBLICATION_CAP_PER_FRAME {
            let Some(completion) = self
                .completed
                .as_ref()
                .and_then(|receiver| receiver.try_recv().ok())
            else {
                break;
            };
            self.pending.remove(&completion.identity);
            self.completed_builds += 1;
            self.all_build_micros = self
                .all_build_micros
                .saturating_add(completion.build_micros);
            self.queue_wait_micros = self
                .queue_wait_micros
                .saturating_add(completion.queue_wait_micros);
            self.max_queue_wait_micros =
                self.max_queue_wait_micros.max(completion.queue_wait_micros);
            match completion.result {
                Ok(product) => {
                    self.build_micros = self.build_micros.saturating_add(product.build_micros);
                    if product.resident_bytes <= PRODUCT_RESERVATION
                        && self.bytes().saturating_add(product.resident_bytes)
                            <= CPU_PRODUCT_CAP_BYTES
                    {
                        self.cache.insert(completion.identity, Arc::new(product));
                    } else {
                        self.failed.insert(completion.identity);
                    }
                }
                Err(error) => {
                    self.failed_builds += 1;
                    self.report.reason = Some(error.to_string());
                    self.failed.insert(completion.identity);
                }
            }
        }
        let mut candidates: Vec<usize> = active
            .iter()
            .copied()
            .filter(|index| {
                draw.patches.get(*index).is_some_and(|patch| {
                    patch.morph_fraction == 1.0
                        && patch.boundary_fraction == 1.0
                        && patch.own.tile.key.cells == 32
                })
            })
            .collect();
        candidates.sort_by(|a, b| {
            let visible_a = projection
                .project_marker(draw.patches[*a].own.anchor_view_m)
                .ok()
                .flatten()
                .is_some();
            let visible_b = projection
                .project_marker(draw.patches[*b].own.anchor_view_m)
                .ok()
                .flatten()
                .is_some();
            visible_b.cmp(&visible_a).then_with(|| {
                draw.patches[*a]
                    .own
                    .anchor_view_m
                    .length_squared()
                    .total_cmp(&draw.patches[*b].own.anchor_view_m.length_squared())
            })
        });
        if settings.freeze {
            if let Some(frozen) = &self.frozen_identities {
                let current: HashSet<_> = candidates
                    .iter()
                    .map(|index| {
                        let p = &draw.patches[*index];
                        Identity(p.own.tile.key.clone(), p.boundary_endpoints.version)
                    })
                    .collect();
                if !frozen.is_subset(&current) {
                    settings.freeze = false;
                    self.report.reason = Some("frozen dependencies changed; cut released".into());
                    self.frozen_identities = None;
                } else {
                    candidates.retain(|index| {
                        let p = &draw.patches[*index];
                        frozen.contains(&Identity(
                            p.own.tile.key.clone(),
                            p.boundary_endpoints.version,
                        ))
                    });
                }
            }
        } else {
            self.frozen_identities = None;
        }
        candidates.truncate(REGION_CAP);
        let wanted: HashSet<Identity> = candidates
            .iter()
            .map(|index| {
                let patch = &draw.patches[*index];
                Identity(patch.own.tile.key.clone(), patch.boundary_endpoints.version)
            })
            .collect();
        if !settings.freeze {
            self.gpu.retire_unwanted(
                &wanted
                    .iter()
                    .map(|id| (id.0.clone(), id.1))
                    .collect::<Vec<_>>(),
            );
            for id in self.cache.keys().filter(|id| !wanted.contains(*id)) {
                if !self.selected_products.remove(id) {
                    self.evicted_before_selection += 1;
                }
            }
            self.cache.retain(|id, _| wanted.contains(id));
            self.failed.retain(|id| wanted.contains(id));
        }
        let mut views = Vec::with_capacity(REGION_CAP);
        let mut uploaded = 0;
        for index in candidates {
            let patch = &draw.patches[index];
            let identity = Identity(patch.own.tile.key.clone(), patch.boundary_endpoints.version);
            if let Some(product) = self.cache.get(&identity) {
                // Existing GPU products are cheap hits; only one new upload per frame.
                let resident =
                    self.gpu.retained_products().iter().any(|p| {
                        p.input.key == identity.0 && p.input.boundary_version == identity.1
                    });
                if resident || uploaded < PUBLICATION_CAP_PER_FRAME {
                    match self.gpu.prepare_region(device, queue, product) {
                        Ok(slot) => {
                            if !resident {
                                uploaded += 1;
                            }
                            self.draw_slots.insert(index, slot);
                            views.push(RegionView {
                                slot,
                                anchor_view_m: patch.own.anchor_view_m,
                                body_to_view: patch.own.body_to_view,
                                sun_body: patch.own.sun_body,
                                appearance: patch.own.appearance,
                                quality_fallback: patch.quality_fallback,
                                pending_finer: patch.quality_fallback,
                            });
                        }
                        Err(error) => self.report.reason = Some(error.to_string()),
                    }
                }
            } else if !settings.freeze
                && !self.pending.contains(&identity)
                && !self.failed.contains(&identity)
                && self.bytes().saturating_add(PRODUCT_RESERVATION) <= CPU_PRODUCT_CAP_BYTES
            {
                if let Ok(input) = input_from_patch(patch) {
                    match self.jobs.as_ref().expect("started").try_send(Job {
                        identity: identity.clone(),
                        input,
                        queued: Instant::now(),
                    }) {
                        Ok(()) => {
                            self.pending.insert(identity);
                            self.builds += 1;
                        }
                        Err(TrySendError::Full(_)) => break,
                        Err(TrySendError::Disconnected(_)) => {
                            self.report.reason = Some("cluster worker disconnected".into());
                            break;
                        }
                    }
                } else {
                    self.failed.insert(identity);
                }
            }
        }
        let reason = self.report.reason.take();
        self.gpu
            .encode_selection(encoder, queue, projection, &views, settings);
        for (patch, slot) in &self.draw_slots {
            if self.gpu.selected_slots().contains(slot) {
                let patch = &draw.patches[*patch];
                self.selected_products.insert(Identity(
                    patch.own.tile.key.clone(),
                    patch.boundary_endpoints.version,
                ));
            }
        }
        if settings.freeze && !views.is_empty() && self.frozen_identities.is_none() {
            self.frozen_identities = Some(wanted);
        }
        self.report = self.gpu.report();
        self.report.pending_regions = self.pending.len();
        self.report.cpu_bytes = self.bytes();
        self.report.build_micros = self.build_micros;
        self.report.rebuilds = self.builds;
        self.report.all_build_micros = self.all_build_micros;
        self.report.queue_wait_micros = self.queue_wait_micros;
        self.report.max_queue_wait_micros = self.max_queue_wait_micros;
        self.report.completed_builds = self.completed_builds;
        self.report.failed_builds = self.failed_builds;
        self.report.evicted_before_selection = self.evicted_before_selection;
        self.report.fallback_regions = active.len().saturating_sub(self.draw_slots.len());
        if self.report.reason.is_none() {
            self.report.reason = reason;
        }
    }
    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &wgpu::BindGroup,
        patch: usize,
    ) -> bool {
        let Some(slot) = self.draw_slots.get(&patch) else {
            return false;
        };
        self.gpu.draw_region(pass, projection, *slot);
        true
    }
    pub(crate) fn report(&self) -> ClusterReport {
        self.report.clone()
    }
    pub(crate) fn fallback_mode(&self, patch: &crate::RegionalPatchDraw) -> u32 {
        let identity = Identity(patch.own.tile.key.clone(), patch.boundary_endpoints.version);
        if self.failed.contains(&identity) {
            14
        } else if self.pending.contains(&identity) || self.cache.contains_key(&identity) {
            13
        } else {
            12
        }
    }
    pub(crate) fn on_submitted(&mut self, queue: &wgpu::Queue) {
        self.gpu.on_submitted(queue);
    }
    #[cfg(feature = "developer-tools")]
    pub(crate) fn has_cluster_draw(&self) -> bool {
        !self.draw_slots.is_empty()
    }
}
impl Drop for ClusterRuntime {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.jobs.take();
        self.completed.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn input_from_patch(
    patch: &crate::RegionalPatchDraw,
) -> Result<RegionInput, crate::TileGeometryError> {
    let n = patch.own.tile.key.cells;
    let mut vertices = Vec::with_capacity(((n + 1) * (n + 1)) as usize);
    let mut locked = Vec::with_capacity(vertices.capacity());
    for y in 0..=n {
        for x in 0..=n {
            let node = crate::regional_edges::evaluate_node(
                &patch.own.tile,
                Some(&patch.boundary_endpoints.own_fine),
                [x, y],
            )?;
            vertices.push(RegionVertex {
                position: node.position_local_m,
                normal: node.normal_varying_body,
                material: node.material,
                uv: [f64::from(x) / f64::from(n), f64::from(y) / f64::from(n)],
            });
            locked.push(x == 0 || y == 0 || x == n || y == n);
        }
    }
    let mut indices = Vec::with_capacity((n * n * 6) as usize);
    for y in 0..n {
        for x in 0..n {
            let a = y * (n + 1) + x;
            let b = a + 1;
            let c = a + n + 1;
            let d = c + 1;
            indices.extend([a, b, c, b, d, c]);
        }
    }
    Ok(RegionInput {
        key: patch.own.tile.key.clone(),
        boundary_version: patch.boundary_endpoints.version,
        vertices,
        indices,
        locked,
    })
}

fn micros(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}
