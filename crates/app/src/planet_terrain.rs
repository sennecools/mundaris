//! Bounded, disposable CPU terrain geometry with serial and worker generation.
//! Neither observer motion nor renderer resource lifetime participates in identity.
use anyhow::{Result, bail};
use mundaris_math::{Direction3, surface::*};
use mundaris_renderer::planet_surface::*;
use mundaris_world::{BodyId, terrain::*};
use std::{
    mem::size_of,
    sync::Arc,
    time::{Duration, Instant},
};

mod certificate;
pub use certificate::terrain_surface_certificate;
mod adaptive;
pub use adaptive::{AdaptiveTerrainCover, TerrainConvergenceDiagnostic, TerrainSelectionPolicy};
mod workers;

pub const TERRAIN_CPU_CAP_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_TERRAIN_PATCHES: usize = 4096;
pub const MAX_PENDING_PATCHES: usize = 256;
pub const GENERATION_MICROBATCH: usize = 8;
/// Validation permits larger chunks without making them the live default.
pub const MAX_GENERATION_BATCH: usize = 64;

#[cfg(test)]
mod publication_pin_tests {
    use super::*;
    use mundaris_math::*;
    use mundaris_world::{BodyProperties, BodyState, CelestialSystem};
    use std::num::NonZeroU64;

    #[test]
    fn completed_siblings_are_pinned_before_the_next_builder_admission() {
        let mut world = CelestialSystem::new(NonZeroU64::new(81).unwrap(), SimulationInstant::ZERO);
        let body = world
            .insert_body(
                "publication pin fixture",
                BodyProperties::new(1.0, 1000.0).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
        let definition = checkpoint_terrain_definition(1000.0).unwrap();
        world.edit_terrain(body, Some(definition.clone())).unwrap();
        let identity = TerrainGeometryIdentity::new(
            body,
            definition,
            world.body(body).unwrap().terrain_revision(),
            1000.0,
        )
        .unwrap();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 4).unwrap();
        let siblings = CubePatchAddress::root(CubeFace::PositiveZ)
            .children()
            .unwrap();
        let extra = CubePatchAddress::root(CubeFace::PositiveX);
        for address in siblings.into_iter().chain([extra]) {
            assert!(cache.request_pinned(&identity, address));
        }
        let before = cache.resident_bytes();
        let work = cache
            .generate(5 * GRID_SAMPLES, GENERATION_MICROBATCH, None)
            .unwrap();
        assert_eq!(work.patches_completed, 4);
        assert_eq!(
            work.allocation_delta_bytes,
            cache.resident_bytes() as isize - before as isize
        );
        assert_eq!(cache.report().pinned_patches, 4);
        assert_eq!(cache.report().evictions, 0);
        assert!(
            siblings
                .iter()
                .all(|&address| cache.peek(&identity, address).is_some())
        );
        assert!(cache.peek(&identity, extra).is_none());
        assert_eq!(cache.pending(), 1);
        cache.unpin_all();
        let work = cache
            .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
            .unwrap();
        assert_eq!(work.patches_completed, 1);
        assert_eq!(cache.report().evictions, 1);
        assert!(cache.peek(&identity, extra).is_some());
        assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
        // A soft waterline must not permanently block a pinned replacement
        // when only indispensable data remains and the hard quota still fits.
        cache.set_soft_residency_headroom(TERRAIN_CPU_CAP_BYTES);
        let critical = CubePatchAddress::root(CubeFace::NegativeX);
        cache.request_pinned(&identity, critical);
        let work = cache
            .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
            .unwrap();
        assert_eq!(work.patches_completed, 1);
        assert!(cache.peek(&identity, extra).is_some());
        assert!(cache.peek(&identity, critical).is_some());
        assert!(cache.report().soft_waterline_misses > 0);
        assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
    }
}

/// Checkpoint authoring preset, intentionally not an Earth shoreline model.
pub fn checkpoint_terrain_definition(radius_m: f64) -> Result<TerrainDefinition> {
    checkpoint_terrain_definition_version(radius_m, TerrainGeneratorVersion::V2)
}

/// Deterministic A/B authoring with identical macro/range seed and camera inputs.
pub fn checkpoint_terrain_definition_version(
    radius_m: f64,
    version: TerrainGeneratorVersion,
) -> Result<TerrainDefinition> {
    let bands = [
        TerrainBandConfig::new(
            radius_m * 0.0004,
            TerrainScale::Angular {
                lowest_cycles_per_body: 2.0,
            },
            2,
        )?,
        TerrainBandConfig::new(
            radius_m * 0.0005,
            TerrainScale::Metres {
                longest_wavelength_m: (radius_m * 0.25).max(64.0),
            },
            3,
        )?,
        TerrainBandConfig::new(
            radius_m * 0.0001,
            TerrainScale::Metres {
                longest_wavelength_m: 128_000.0,
            },
            4,
        )?,
        TerrainBandConfig::new(
            radius_m * 0.00001,
            TerrainScale::Metres {
                longest_wavelength_m: 4096.0,
            },
            4,
        )?,
        TerrainBandConfig::new(
            radius_m * 0.0000002,
            TerrainScale::Metres {
                longest_wavelength_m: 64.0,
            },
            4,
        )?,
    ];
    Ok(TerrainDefinition::new(
        TerrainIdentity(0x415552454c4941),
        TerrainSeed(17),
        version,
        TerrainConfig::new(
            bands,
            TerrainControls::new(-0.15, 2.0, 0.4, 0.85, 0.2, 0.15)?,
        )?,
    ))
}

/// Exact immutable truth binding. Runtime body association is not generation salt.
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainGeometryIdentity {
    pub body: BodyId,
    pub definition: TerrainDefinition,
    pub revision: TerrainRevision,
    pub radius_m: f64,
}

/// Interim safe display restriction: whole uniform-level covers only. The Phase
/// 4 desired selection still drives the requested level; mixed-profile ownership
/// and common-refinement morphing are intentionally not approximated here.
#[derive(Default)]
pub struct TerrainReadyCover {
    identity: Option<TerrainGeometryIdentity>,
    active: Vec<ActiveSurfacePatch>,
    target: Vec<CubePatchAddress>,
    target_level: u8,
    visible: Vec<ActiveSurfacePatch>,
}
impl TerrainReadyCover {
    pub fn active(&self) -> &[ActiveSurfacePatch] {
        &self.active
    }
    pub fn visible(&self) -> &[ActiveSurfacePatch] {
        &self.visible
    }
    pub fn ready(&self) -> bool {
        !self.active.is_empty()
    }
    pub fn bookkeeping_bytes(&self) -> usize {
        size_of::<Self>()
            + (self.active.capacity() + self.visible.capacity()) * size_of::<ActiveSurfacePatch>()
            + self.target.capacity() * size_of::<CubePatchAddress>()
    }
    pub fn update(
        &mut self,
        cache: &mut TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        desired_level: u8,
        vertex_budget: usize,
        wall_budget: Option<Duration>,
    ) -> Result<TerrainWorkReport> {
        if self.identity.as_ref() != Some(identity) {
            cache.invalidate_body(identity);
            self.active.clear();
            self.visible.clear();
            self.target.clear();
            self.identity = Some(identity.clone());
        }
        // All retained covers fit the 4096-entry ceiling, including old+new
        // uniform covers and six roots. This is diagnosed coarse quality, not a
        // changed Phase4 selection threshold or a crack-free transition claim.
        let level = desired_level.min(4);
        if self.target.is_empty() || self.target_level != level {
            self.target.clear();
            self.target_level = level;
            let side = 1u32 << level;
            for face in CubeFace::ALL {
                for y in 0..side {
                    for x in 0..side {
                        self.target
                            .push(CubePatchAddress::try_new(face, level, x, y)?);
                    }
                }
            }
            self.target.sort_unstable();
        }
        // App has one aggregate live cover coordinator. Pin all active geometry
        // and fallback roots before admitting any replacement allocations.
        cache.unpin_body(identity.body);
        cache.retain_required(identity, &self.target);
        for p in &self.active {
            cache.pin(identity, p.address);
        }
        for face in CubeFace::ALL {
            let root = CubePatchAddress::root(face);
            cache.pin(identity, root);
            cache.request(identity, root);
        }
        if self
            .active
            .first()
            .is_none_or(|p| p.address.level() != level)
        {
            for &address in &self.target {
                cache.pin(identity, address);
                cache.request(identity, address);
            }
        }
        let report = cache.generate(vertex_budget, GENERATION_MICROBATCH, wall_budget)?;
        if self.active.is_empty()
            && CubeFace::ALL
                .into_iter()
                .all(|face| cache.peek(identity, CubePatchAddress::root(face)).is_some())
        {
            for face in CubeFace::ALL {
                let address = CubePatchAddress::root(face);
                self.active.push(ActiveSurfacePatch {
                    address,
                    stitch_mask: 0,
                    metadata: cache.metadata(identity, address)?,
                    error_pixels: f64::INFINITY,
                });
            }
        }
        if self
            .active
            .first()
            .is_none_or(|p| p.address.level() != level)
            && self
                .target
                .iter()
                .all(|&a| cache.peek(identity, a).is_some())
        {
            let mut replacement = Vec::with_capacity(self.target.len());
            for &address in &self.target {
                replacement.push(ActiveSurfacePatch {
                    address,
                    stitch_mask: 0,
                    metadata: cache.metadata(identity, address)?,
                    error_pixels: f64::INFINITY,
                });
            }
            self.active = replacement;
        }
        for p in &self.active {
            cache.pin(identity, p.address);
        }
        Ok(report)
    }
    /// No reference-sphere horizon rejection for displaced terrain. Only expanded
    /// ball frustum rejection; displayed quality uses the complete certificate.
    pub fn prepare_visible(
        &mut self,
        cache: &TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        input: &SurfaceViewInput<'_, '_>,
    ) -> Result<()> {
        self.visible.clear();
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        for p in &self.active {
            let geometry = cache
                .peek(identity, p.address)
                .ok_or_else(|| anyhow::anyhow!("active terrain geometry was evicted"))?;
            let (center, radius) = p.metadata.ball(identity.radius_m, geometry.extent())?;
            let center = source
                .view_displacement(mundaris_math::FramePosition::new(
                    input.body_fixed_frame,
                    mundaris_math::LocalPosition::try_metres(center)?,
                ))?
                .metres();
            if input.projection.rejects_ball(center, radius)? {
                continue;
            }
            let mut p = *p;
            p.error_pixels = p.metadata.projected_total_error(
                geometry.error(),
                center,
                radius,
                input.projection,
            )?;
            self.visible.push(p);
        }
        Ok(())
    }
}
impl TerrainGeometryIdentity {
    pub fn new(
        body: BodyId,
        definition: TerrainDefinition,
        revision: TerrainRevision,
        radius_m: f64,
    ) -> Result<Self> {
        definition.validate_radius(radius_m)?;
        Ok(Self {
            body,
            definition,
            revision,
            radius_m,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TerrainWorkReport {
    pub vertices_generated: usize,
    pub patches_completed: usize,
    pub elapsed: Duration,
    pub allocation_delta_bytes: isize,
    pub pending_patches: usize,
    /// CPU time for completed worker calculations; not main-thread latency.
    pub worker_cpu: Duration,
    pub worker_samples_completed: usize,
    pub scheduling: Duration,
    pub publication: Duration,
    /// Newly ready raw patches in the currently selected replacement closure.
    pub replacement_useful_completed: usize,
    pub local_useful_completed: usize,
    /// Detailed worker/cache measurements; present only in profiling builds.
    #[cfg(feature = "surface-profile")]
    pub profile: TerrainWorkProfile,
}
/// Per-frame terrain measurements. Legacy `*_cpu` stage names below measure
/// elapsed worker/app intervals (including descheduling), not scheduled CPU time.
/// `worker_scheduled_cpu` separately reads the OS thread clock, whose short-stage
/// resolution can be coarse. Waits are elapsed time between distinct events.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TerrainWorkProfile {
    pub worker_scheduled_cpu: Duration,
    pub worker_cpu_clock_unavailable_jobs: usize,
    pub cover_coordinator_wait: Duration,
    /// Worker time creating raw samples, excluding certificate construction.
    pub worker_raw_generation_cpu: Duration,
    pub worker_certificate_cpu: Duration,
    pub worker_stitch_cpu: Duration,
    pub worker_morph_cpu: Duration,
    /// Submit-to-worker-start delay for consumed jobs.
    pub worker_queue_wait: Duration,
    /// Worker-finish-to-cache-reception delay for consumed jobs.
    pub cache_publication_wait: Duration,
    pub cancelled_work_cpu: Duration,
    pub cancelled_work_count: usize,
    /// Time and number of cache reservation/admission attempts during scheduling.
    pub cache_reservation_cpu: Duration,
    pub cache_reservation_attempts: usize,
    /// Admission retries after an eviction attempt; rejected admissions are included.
    pub cache_reservation_retries: usize,
}
#[cfg(feature = "surface-profile")]
impl TerrainWorkProfile {
    pub(crate) fn include_worker(&mut self, metrics: workers::WorkerMetrics) {
        if let Some(cpu) = metrics.scheduled_cpu {
            self.worker_scheduled_cpu += cpu;
        } else {
            self.worker_cpu_clock_unavailable_jobs += 1;
        }
        self.worker_raw_generation_cpu += metrics.raw_generation_cpu;
        self.worker_certificate_cpu += metrics.certificate_cpu;
        self.worker_stitch_cpu += metrics.stitch_cpu;
        self.worker_morph_cpu += metrics.morph_cpu;
        self.worker_queue_wait += metrics.queue_wait;
        self.cache_publication_wait += metrics.cache_publication_wait;
        if metrics.cancelled {
            self.cancelled_work_cpu += metrics.cpu;
            self.cancelled_work_count += 1;
        }
    }
}
#[derive(Debug, Clone, Copy, Default)]
pub struct TerrainCacheReport {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident_patches: usize,
    pub resident_bytes: usize,
    pub peak_bytes: usize,
    pub pinned_patches: usize,
    pub pinned_bytes: usize,
    pub external_bytes: usize,
    pub peak_aggregate_bytes: usize,
    pub worker_count: usize,
    pub worker_jobs: usize,
    pub queued_patches: usize,
    pub worker_reserved_bytes: usize,
    pub worker_fixed_bytes: usize,
    pub completed_unpublished_bytes: usize,
    pub cancellations: u64,
    /// Generation admissions blocked by entry capacity or the effective quota.
    /// The effective quota can be the experimental soft target or the hard cap.
    pub reservation_rejected: u64,
    /// Attempts to reclaim an unpinned, cache-owned raw patch.
    pub eviction_attempts: u64,
    /// Accounted bytes reclaimed by those evictions.
    pub eviction_freed_bytes: u64,
    pub soft_waterline_misses: u64,
}
struct Entry {
    identity: TerrainGeometryIdentity,
    patch: Arc<GeneratedSurfacePatch>,
    access: u64,
    pinned: bool,
    metadata: PatchMetadata,
    valid: bool,
}
struct Request {
    identity: TerrainGeometryIdentity,
    address: CubePatchAddress,
    pin_when_ready: bool,
}
struct Builder {
    request: Request,
    generator: TerrainGenerator,
    samples: Vec<SurfaceGeometrySample>,
}

/// One aggregate cache/queue for all enabled bodies. All retained capacities and
/// builders, worker stacks/scratch, inputs and outputs are charged to the same cap.
pub struct TerrainPatchCache {
    entries: Vec<Entry>,
    requests: Vec<Request>,
    building: Option<Builder>,
    cap_bytes: usize,
    soft_headroom_bytes: usize,
    operational_reserve_bytes: usize,
    external_bytes: usize,
    max_entries: usize,
    sequence: u64,
    report: TerrainCacheReport,
    topology: SurfaceTopology,
    workers: Option<workers::TerrainWorkers>,
    // A cover reservation also owns the shared source charge until its
    // coordinator consumes the result, including cancellation acknowledgements.
    completed_covers: Vec<workers::Completion>,
}
impl TerrainPatchCache {
    pub fn new(cap_bytes: usize, max_entries: usize) -> Result<Self> {
        if max_entries == 0
            || max_entries > MAX_TERRAIN_PATCHES
            || cap_bytes > TERRAIN_CPU_CAP_BYTES
        {
            bail!("invalid terrain cache quota");
        }
        // Fixed bookkeeping capacity makes allocation preflight exact. Geometry
        // buffers are allocated only on demand, never a 128 MiB eager arena.
        let cache = Self {
            entries: Vec::with_capacity(max_entries),
            requests: Vec::with_capacity(MAX_PENDING_PATCHES),
            building: None,
            cap_bytes,
            soft_headroom_bytes: 0,
            operational_reserve_bytes: 0,
            external_bytes: 0,
            max_entries,
            sequence: 0,
            report: TerrainCacheReport::default(),
            topology: SurfaceTopology::new(),
            workers: None,
            completed_covers: Vec::with_capacity(4),
        };
        if cache.resident_bytes() > cap_bytes {
            bail!("terrain quota cannot hold bookkeeping");
        }
        Ok(cache)
    }
    /// Zero keeps the exact operation-budget serial reference path for tests.
    pub fn new_with_workers(cap_bytes: usize, max_entries: usize, count: usize) -> Result<Self> {
        let mut cache = Self::new(cap_bytes, max_entries)?;
        if count != 0 {
            anyhow::ensure!(
                (1..=4).contains(&count),
                "terrain worker count must be 0..=4"
            );
            anyhow::ensure!(
                cache
                    .resident_bytes()
                    .saturating_add(workers::TerrainWorkers::required_bytes(count))
                    <= cap_bytes,
                "terrain quota cannot hold workers"
            );
            cache.workers = Some(workers::TerrainWorkers::new(count)?);
            anyhow::ensure!(
                cache.resident_bytes() <= cap_bytes,
                "terrain quota cannot hold workers"
            );
            cache.record_peak();
        }
        Ok(cache)
    }
    pub fn worker_count(&self) -> usize {
        self.workers
            .as_ref()
            .map_or(0, workers::TerrainWorkers::count)
    }
    /// Keep this many bytes of the hard aggregate quota available for later
    /// derived-terrain reservations. This is a soft target: pinned and externally
    /// held data is never evicted to satisfy it. Defaults to zero, preserving the
    /// historical admission behavior. Values above the hard cap are clamped.
    pub fn set_soft_residency_headroom(&mut self, bytes: usize) {
        self.soft_headroom_bytes = bytes.min(self.cap_bytes);
    }
    fn soft_admission_cap(&self) -> usize {
        // A preflighted transition/worker output already occupies operational
        // headroom. Do not reserve the same working space a second time by
        // subtracting the entire waterline from an aggregate containing it.
        let operational = self
            .operational_reserve_bytes
            .saturating_add(
                self.workers
                    .as_ref()
                    .map_or(0, workers::TerrainWorkers::reservations),
            )
            .saturating_add(
                self.completed_covers
                    .iter()
                    .map(|c| c.reserved_bytes)
                    .sum::<usize>(),
            );
        self.cap_bytes
            .saturating_sub(self.soft_headroom_bytes.saturating_sub(operational))
    }
    pub fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            // Fixed resumed-query/location/output and certificate stack allowance.
            // Charge it even while idle so generation cannot bypass admission.
            + 64 * 1024
            + self.entries.capacity() * size_of::<Entry>()
            + self.requests.capacity() * size_of::<Request>()
            + self.completed_covers.capacity() * size_of::<workers::Completion>()
            + self.topology.allocated_bytes()
            - size_of::<SurfaceTopology>()
            + self
                .entries
                .iter()
                .map(|e| {
                    e.patch.resident_heap_capacity_bytes()
                        + size_of::<GeneratedSurfacePatch>()
                        + 2 * size_of::<usize>() // Arc strong/weak allocation counters
                })
                .sum::<usize>()
            + self.building.as_ref().map_or(0, |b| {
                b.samples.capacity() * size_of::<SurfaceGeometrySample>()
                    + size_of::<GeneratedSurfacePatch>()
                    + 2 * size_of::<usize>()
            })
            + self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::bytes)
            + self
                .completed_covers
                .iter()
                .map(|c| c.reserved_bytes)
                .sum::<usize>()
    }
    fn record_peak(&mut self) {
        self.report.peak_bytes = self.report.peak_bytes.max(self.resident_bytes());
        self.report.peak_aggregate_bytes = self
            .report
            .peak_aggregate_bytes
            .max(self.resident_bytes() + self.external_bytes);
    }
    pub fn report(&self) -> TerrainCacheReport {
        TerrainCacheReport {
            resident_patches: self.entries.len(),
            resident_bytes: self.resident_bytes(),
            pinned_patches: self.entries.iter().filter(|e| e.pinned).count(),
            pinned_bytes: self
                .entries
                .iter()
                .filter(|e| e.pinned)
                .map(|e| {
                    size_of::<Entry>()
                        + e.patch.resident_heap_bytes()
                        + size_of::<GeneratedSurfacePatch>()
                        + 2 * size_of::<usize>()
                })
                .sum(),
            external_bytes: self.external_bytes,
            worker_count: self.worker_count(),
            worker_jobs: self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::in_flight),
            queued_patches: self.requests.len(),
            worker_reserved_bytes: self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::reservations),
            worker_fixed_bytes: self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::fixed_bytes),
            completed_unpublished_bytes: self
                .completed_covers
                .iter()
                .map(|c| c.reserved_bytes)
                .sum(),
            ..self.report
        }
    }
    pub fn pending(&self) -> usize {
        self.requests.len()
            + usize::from(self.building.is_some())
            + self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::in_flight)
            + self.completed_covers.len()
    }
    pub fn pending_for_body(&self, body: BodyId) -> usize {
        self.requests
            .iter()
            .filter(|r| r.identity.body == body)
            .count()
            + usize::from(
                self.building
                    .as_ref()
                    .is_some_and(|b| b.request.identity.body == body),
            )
            + self
                .workers
                .as_ref()
                .map_or(0, |w| w.pending_for_body(body))
            + self
                .completed_covers
                .iter()
                .filter(|c| c.identity.body == body)
                .count()
    }
    /// Admit derived cover/transition capacity into the same aggregate CPU cap.
    /// Pressure only evicts unpinned entries; failure retains valid old coverage.
    pub fn reserve_external(&mut self, bytes: usize) -> bool {
        while self.resident_bytes().saturating_add(bytes) > self.cap_bytes {
            if !self.evict_one() {
                return false;
            }
        }
        self.external_bytes = bytes;
        self.record_peak();
        true
    }
    pub fn unpin_all(&mut self) {
        for e in &mut self.entries {
            e.pinned = false;
        }
    }
    pub fn unpin_body(&mut self, body: BodyId) {
        for e in &mut self.entries {
            if e.identity.body == body {
                e.pinned = false;
            }
        }
    }
    /// Stop inactive-body generation while retaining reusable unpinned raw patches.
    pub fn cancel_body_work(&mut self, body: BodyId) {
        self.unpin_body(body);
        self.cancel_workers(|identity, _| identity.body == body);
        self.requests.retain(|r| r.identity.body != body);
        if self
            .building
            .as_ref()
            .is_some_and(|b| b.request.identity.body == body)
        {
            self.building = None;
        }
    }
    fn cancel_workers(
        &mut self,
        predicate: impl Fn(&TerrainGeometryIdentity, Option<CubePatchAddress>) -> bool,
    ) {
        if let Some(pool) = &mut self.workers {
            self.report.cancellations += pool.cancel_where(&predicate) as u64;
        }
        for completion in &mut self.completed_covers {
            if predicate(&completion.identity, None)
                && !matches!(completion.result, Ok(workers::Output::Cancelled))
            {
                completion.result = Ok(workers::Output::Cancelled);
                self.report.cancellations += 1;
            }
        }
    }
    fn take_cover_completion(&mut self, id: u64) -> Option<workers::Completion> {
        self.completed_covers
            .iter()
            .position(|c| c.id == id)
            .map(|index| self.completed_covers.remove(index))
    }
    /// The coordinator is about to drop its source. In-flight work retains its
    /// charge until acknowledgement; an already completed reservation can go now.
    fn abandon_cover(&mut self, id: u64) {
        self.take_cover_completion(id);
        if let Some(pool) = &mut self.workers {
            self.report.cancellations += u64::from(pool.abandon_cover(id));
        }
    }
    fn retain_required(
        &mut self,
        identity: &TerrainGeometryIdentity,
        required: &[CubePatchAddress],
    ) {
        let keep = |address: CubePatchAddress| address.level() == 0 || required.contains(&address);
        let before = self.requests.len();
        self.requests
            .retain(|r| r.identity.body != identity.body || keep(r.address));
        self.report.cancellations += (before - self.requests.len()) as u64;
        if self
            .building
            .as_ref()
            .is_some_and(|b| b.request.identity.body == identity.body && !keep(b.request.address))
        {
            self.building = None;
            self.report.cancellations += 1;
        }
        self.cancel_workers(|i, address| {
            i.body == identity.body && address.is_some_and(|a| !keep(a))
        });
    }
    pub fn pin(&mut self, identity: &TerrainGeometryIdentity, address: CubePatchAddress) -> bool {
        if let Some(index) = self.entry_index(identity, address) {
            self.entries[index].pinned = true;
            true
        } else {
            false
        }
    }
    /// Cancel obsolete work and remove stale geometry for this body before any
    /// new-revision result can be observed. Other bodies retain their entries.
    pub fn invalidate_body(&mut self, identity: &TerrainGeometryIdentity) {
        self.cancel_workers(|i, _| i.body == identity.body && i != identity);
        for e in &mut self.entries {
            if e.identity.body == identity.body && e.identity != *identity {
                // Keep worker-held input bytes charged, but never expose stale truth.
                e.valid = false;
                e.pinned = false;
            }
        }
        self.entries
            .retain(|e| e.valid || Arc::strong_count(&e.patch) > 1);
        self.requests
            .retain(|r| r.identity.body != identity.body || r.identity == *identity);
        if self.building.as_ref().is_some_and(|b| {
            b.request.identity.body == identity.body && b.request.identity != *identity
        }) {
            self.building = None;
        }
    }
    pub fn get(
        &mut self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> Result<Option<&GeneratedSurfacePatch>> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("terrain access sequence overflow"))?;
        if let Some(index) = self.entry_index(identity, address) {
            let e = &mut self.entries[index];
            e.access = self.sequence;
            self.report.hits += 1;
            Ok(Some(&e.patch))
        } else {
            self.report.misses += 1;
            Ok(None)
        }
    }
    pub fn peek(
        &self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> Option<&GeneratedSurfacePatch> {
        self.entry_index(identity, address)
            .map(|index| self.entries[index].patch.as_ref())
    }
    // Compact sorted storage avoids an O(patches²) settled-view scan without
    // unaccounted hash-table buckets. Equal addresses still verify full truth.
    fn entry_index(
        &self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> Option<usize> {
        let start = self
            .entries
            .partition_point(|e| e.patch.address() < address);
        self.entries[start..]
            .iter()
            .take_while(|e| e.patch.address() == address)
            .position(|e| e.valid && e.identity == *identity)
            .map(|offset| start + offset)
    }
    fn metadata(
        &self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> Result<PatchMetadata> {
        self.entry_index(identity, address)
            .map(|index| self.entries[index].metadata)
            .ok_or_else(|| anyhow::anyhow!("missing ready terrain metadata"))
    }
    /// Returns false on bounded queue pressure; absence remains pending, never a
    /// fabricated Ready state. Request ordering is explicit and deterministic.
    pub fn request(
        &mut self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> bool {
        if self.peek(identity, address).is_some()
            || self
                .requests
                .iter()
                .any(|r| r.identity == *identity && r.address == address)
            || self
                .building
                .as_ref()
                .is_some_and(|b| b.request.identity == *identity && b.request.address == address)
            || self
                .workers
                .as_ref()
                .is_some_and(|w| w.patch_pending(identity, address))
        {
            return true;
        }
        if self.pending() >= MAX_PENDING_PATCHES {
            return false;
        }
        self.requests.push(Request {
            identity: identity.clone(),
            address,
            pin_when_ready: false,
        });
        true
    }
    /// Replacement dependencies are pinned at publication, not only at the next
    /// frame boundary. A long generation opportunity cannot evict a sibling it
    /// just completed while allocating the next sibling under pressure.
    fn request_pinned(
        &mut self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> bool {
        let admitted = self.request(identity, address);
        self.pin(identity, address);
        for request in &mut self.requests {
            if request.identity == *identity && request.address == address {
                request.pin_when_ready = true;
            }
        }
        if let Some(builder) = &mut self.building
            && builder.request.identity == *identity
            && builder.request.address == address
        {
            builder.request.pin_when_ready = true;
        }
        if let Some(pool) = &mut self.workers {
            pool.pin(identity, address);
        }
        admitted
    }
    fn evict_one(&mut self) -> bool {
        self.report.eviction_attempts += 1;
        let victim = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.pinned && Arc::strong_count(&e.patch) == 1)
            .min_by_key(|(_, e)| (e.access, e.patch.address()))
            .map(|(i, _)| i);
        if let Some(i) = victim {
            // Entry vector capacity is retained, so its slot is not freed.
            let bytes = self.entries[i].patch.resident_heap_capacity_bytes()
                + size_of::<GeneratedSurfacePatch>()
                + 2 * size_of::<usize>();
            self.entries.remove(i);
            self.report.evictions += 1;
            self.report.eviction_freed_bytes = self
                .report
                .eviction_freed_bytes
                .saturating_add(bytes as u64);
            true
        } else {
            false
        }
    }
    /// Wall cutoff is checked between <=32-sample batches. Headless callers use
    /// None for an exact reproducible operation schedule. One batch may overshoot.
    pub fn generate(
        &mut self,
        vertex_budget: usize,
        batch_size: usize,
        wall_budget: Option<Duration>,
    ) -> Result<TerrainWorkReport> {
        if !(1..=MAX_GENERATION_BATCH).contains(&batch_size) {
            bail!("invalid terrain microbatch size");
        }
        if self.workers.is_some() {
            return self.generate_workers(vertex_budget != 0);
        }
        let start = Instant::now();
        let before = self.resident_bytes();
        let mut work = TerrainWorkReport::default();
        while work.vertices_generated < vertex_budget
            && !wall_budget.is_some_and(|d| start.elapsed() >= d)
        {
            if self.building.is_none() {
                if self.requests.is_empty() {
                    break;
                }
                let bytes = GRID_SAMPLES * size_of::<SurfaceGeometrySample>()
                    + size_of::<GeneratedSurfacePatch>()
                    + 2 * size_of::<usize>();
                let admission_cap = self.soft_admission_cap();
                while self.entries.len() >= self.max_entries
                    || self.resident_bytes() + self.external_bytes + bytes > admission_cap
                {
                    if !self.evict_one() {
                        if self.requests.first().is_some_and(|r| r.pin_when_ready)
                            && self.entries.len() < self.max_entries
                            && self.resident_bytes() + self.external_bytes + bytes <= self.cap_bytes
                        {
                            self.report.soft_waterline_misses += 1;
                            break;
                        }
                        self.report.reservation_rejected += 1;
                        work.pending_patches = self.pending();
                        work.elapsed = start.elapsed();
                        work.allocation_delta_bytes =
                            self.resident_bytes() as isize - before as isize;
                        return Ok(work);
                    }
                }
                let request = self.requests.remove(0);
                let generator =
                    TerrainGenerator::new(&request.identity.definition, request.identity.radius_m)?;
                self.building = Some(Builder {
                    request,
                    generator,
                    samples: Vec::with_capacity(GRID_SAMPLES),
                });
                self.record_peak();
            }
            let builder = self
                .building
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("missing terrain builder"))?;
            let count = batch_size
                .min(vertex_budget - work.vertices_generated)
                .min(GRID_SAMPLES - builder.samples.len());
            let first = builder.samples.len();
            let address = builder.request.address;
            let radius = builder.request.identity.radius_m;
            let footprint =
                TerrainFootprint::new(radius * 2.0 / (16.0 * (1u64 << address.level()) as f64))?;
            let generator = &builder.generator;
            let mut locations =
                [SurfaceLocation::new(Direction3::try_new(glam::DVec3::X)?); MAX_GENERATION_BATCH];
            let mut output = [TerrainSample::default(); MAX_GENERATION_BATCH];
            for (offset, location) in locations[..count].iter_mut().enumerate() {
                let index = first + offset;
                *location = SurfaceLocation::new(
                    address
                        .sample_key(index as u32 % 17, index as u32 / 17, 16)?
                        .direction(),
                );
            }
            generator.evaluate_batch(&locations[..count], footprint, &mut output[..count])?;
            for (location, sample) in locations[..count].iter().zip(&output[..count]) {
                builder.samples.push(SurfaceGeometrySample {
                    position_body_m: location.direction().unit() * (radius + sample.height_m()),
                    normal_body: sample.normal_body(*location, radius)?.unit(),
                });
            }
            work.vertices_generated += count;
            if builder.samples.len() == GRID_SAMPLES {
                let metadata = PatchMetadata::build(address, &self.topology)?;
                let (extent, error) = certificate::certificate_for_samples(
                    generator,
                    address,
                    metadata,
                    &builder.samples,
                )?;
                let builder = self
                    .building
                    .take()
                    .ok_or_else(|| anyhow::anyhow!("missing complete builder"))?;
                let patch = GeneratedSurfacePatch::new(
                    address,
                    radius,
                    footprint.metres(),
                    builder.samples,
                    extent,
                    error,
                )?;
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("terrain access sequence overflow"))?;
                let insertion = self
                    .entries
                    .partition_point(|e| e.patch.address() <= address);
                self.entries.insert(
                    insertion,
                    Entry {
                        identity: builder.request.identity,
                        patch: Arc::new(patch),
                        access: self.sequence,
                        pinned: builder.request.pin_when_ready,
                        metadata,
                        valid: true,
                    },
                );
                work.patches_completed += 1;
                self.record_peak();
            }
        }
        work.elapsed = start.elapsed();
        work.allocation_delta_bytes = self.resident_bytes() as isize - before as isize;
        work.pending_patches = self.pending();
        Ok(work)
    }
    fn generate_workers(&mut self, schedule: bool) -> Result<TerrainWorkReport> {
        let start = Instant::now();
        let before = self.resident_bytes();
        self.entries
            .retain(|e| e.valid || Arc::strong_count(&e.patch) > 1);
        let mut work = TerrainWorkReport::default();
        while let Some(completion) = self
            .workers
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing workers"))?
            .take_next()?
        {
            work.worker_cpu += completion.cpu;
            if completion.address.is_none() {
                anyhow::ensure!(
                    self.completed_covers.len() < 4,
                    "terrain cover completion capacity exceeded"
                );
                self.completed_covers.push(completion);
                continue;
            }
            #[cfg(feature = "surface-profile")]
            work.profile.include_worker(completion.metrics);
            match completion.result {
                Ok(workers::Output::Patch(patch, metadata, ..)) => {
                    let address = patch.address();
                    anyhow::ensure!(
                        completion.address == Some(address),
                        "worker patch identity mismatch"
                    );
                    self.sequence = self
                        .sequence
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("terrain access sequence overflow"))?;
                    let insertion = self
                        .entries
                        .partition_point(|e| e.patch.address() <= address);
                    self.entries.insert(
                        insertion,
                        Entry {
                            identity: completion.identity,
                            patch: Arc::new(patch),
                            metadata,
                            access: self.sequence,
                            pinned: completion.pin_when_ready,
                            valid: true,
                        },
                    );
                    work.patches_completed += 1;
                    work.worker_samples_completed += GRID_SAMPLES;
                }
                Ok(workers::Output::Cover(_)) => bail!("unexpected terrain patch cover result"),
                Ok(workers::Output::Cancelled) => {}
                Err(error) => return Err(error),
            }
        }
        work.publication = start.elapsed();
        #[cfg(feature = "surface-profile")]
        if let Some(workers) = &mut self.workers {
            for metrics in workers.take_abandoned_metrics().into_iter().flatten() {
                work.profile.include_worker(metrics);
            }
        }
        let scheduling_start = Instant::now();
        while schedule
            && !self.requests.is_empty()
            && self.workers.as_ref().is_some_and(|w| w.idle())
        {
            #[cfg(feature = "surface-profile")]
            let reservation_start = Instant::now();
            #[cfg(feature = "surface-profile")]
            {
                work.profile.cache_reservation_attempts += 1;
            }
            let in_flight = self
                .workers
                .as_ref()
                .map_or(0, workers::TerrainWorkers::in_flight);
            let admission_cap = self.soft_admission_cap();
            while self.entries.len() + in_flight >= self.max_entries
                || self.resident_bytes() + self.external_bytes + workers::PATCH_RESERVATION
                    > admission_cap
            {
                #[cfg(feature = "surface-profile")]
                {
                    work.profile.cache_reservation_retries += 1;
                }
                if !self.evict_one() {
                    break;
                }
            }
            let critical = self.requests.first().is_some_and(|r| r.pin_when_ready);
            let admission_cap = if critical {
                if self.resident_bytes() + self.external_bytes + workers::PATCH_RESERVATION
                    > admission_cap
                {
                    self.report.soft_waterline_misses += 1;
                }
                self.cap_bytes
            } else {
                admission_cap
            };
            if self.entries.len() + in_flight >= self.max_entries
                || self.resident_bytes() + self.external_bytes + workers::PATCH_RESERVATION
                    > admission_cap
            {
                self.report.reservation_rejected += 1;
                #[cfg(feature = "surface-profile")]
                {
                    work.profile.cache_reservation_cpu += reservation_start.elapsed();
                    work.profile.cache_reservation_retries += 1;
                }
                break;
            }
            let request = self.requests.remove(0);
            self.workers
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!("missing workers"))?
                .submit_patch(&request)?;
            #[cfg(feature = "surface-profile")]
            {
                work.profile.cache_reservation_cpu += reservation_start.elapsed();
            }
            self.record_peak();
        }
        work.scheduling = scheduling_start.elapsed();
        work.elapsed = start.elapsed();
        work.pending_patches = self.pending();
        work.allocation_delta_bytes = self.resident_bytes() as isize - before as isize;
        self.record_peak();
        Ok(work)
    }
}
