//! Desired selection, bounded generation and atomic ready-cover publication.
use super::*;
use mundaris_renderer::RenderPreparationError;

/// Camera-radial demand is distinct from complete coverage and cache residency.
#[derive(Debug, Default, Clone, Copy)]
pub struct TerrainConvergenceDiagnostic {
    pub desired_local_lod: Option<u8>,
    pub ready_local_lod: Option<u8>,
    pub rendered_local_lod: Option<u8>,
    pub target_certifiable: bool,
    pub local_error: SurfaceErrorContributions,
    pub diagnostic_cpu: Duration,
}

/// Temporary mutable bridge. Renderer receives only certificates/readiness, never
/// a terrain definition, generator seed or procedural query interface.
pub struct TerrainSelectionPolicy<'a> {
    cache: &'a mut TerrainPatchCache,
    identity: &'a TerrainGeometryIdentity,
    generator: &'a TerrainGenerator,
    boundary_bounds: &'a [f64; 31],
    morph_remaining_m: f64,
    required: Vec<CubePatchAddress>,
    replacements: usize,
    frozen: bool,
    reservation_base: usize,
}
impl SurfaceGeometryPolicy for TerrainSelectionPolicy<'_> {
    fn certificate(
        &mut self,
        address: CubePatchAddress,
        metadata: PatchMetadata,
        radius_m: f64,
    ) -> std::result::Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError>
    {
        if radius_m != self.identity.radius_m {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let (mut extent, mut error) = if let Some(g) = self.cache.peek(self.identity, address) {
            (g.extent(), g.error())
        } else {
            terrain_surface_certificate(self.generator, address, metadata)
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
        };
        // Edge balance also bounds a four-quadrant vertex's coarsest owner by
        // two levels (the edge-neighbor graph has diameter two). Cube corners
        // have three mutually adjacent faces. No global-height morph allowance.
        let boundary = self.boundary_bounds[usize::from(address.level())];
        error.boundary_constraint_m = (boundary + error.numeric_m).next_up();
        error.morph_remaining_m = self.morph_remaining_m;
        let allowance = (error.boundary_constraint_m + error.morph_remaining_m).next_up();
        extent.min_height_m = (extent.min_height_m - allowance).next_down();
        extent.max_height_m = (extent.max_height_m + allowance).next_up();
        Ok((extent, error))
    }
    fn request_ready(&mut self, address: CubePatchAddress) -> bool {
        if !self.required.contains(&address) {
            self.required.push(address);
        }
        self.cache.request_pinned(self.identity, address);
        self.cache.pin(self.identity, address)
    }
    fn allow_replacement(&self) -> bool {
        !self.frozen && self.replacements == 0
    }
    fn replacement_committed(&mut self) {
        self.replacements += 1;
    }
    fn admit_replacement(&mut self, patches: usize) -> bool {
        self.cache.reserve_external(
            self.reservation_base
                + StitchedSurface::construction_bytes(patches)
                + patches * (size_of::<ActiveSurfacePatch>() + size_of::<&GeneratedSurfacePatch>()),
        )
    }
}

/// Production adaptive ready surface. Replacements are complete balanced local
/// transactions; pending geometry never replaces an existing covering leaf.
#[derive(Default)]
pub struct AdaptiveTerrainCover {
    identity: Option<TerrainGeometryIdentity>,
    generator: Option<TerrainGenerator>,
    boundary_bounds: [f64; 31],
    lod: Option<SurfaceLodSession>,
    active: Vec<ActiveSurfacePatch>,
    visible: Vec<ActiveSurfacePatch>,
    stitched: Option<Arc<StitchedSurface>>,
    morph: Option<ActiveMorph>,
    construction: Option<u64>,
    construction_duration: Duration,
    morph_duration: Duration,
    /// Overlay rejection keeps the complete source and private target frozen.
    /// Bounded larger-budget retries never bypass aggregate admission.
    pub transition_deferred: bool,
    transition_budget_step: u8,
    #[cfg(test)]
    transition_budget_override: Option<usize>,
    pub report: LodReport,
    /// Selector/certificate/readiness traversal, excluding generation and stitching.
    pub selection_preparation: Duration,
    pub stitch_preparation: Duration,
    pub morph_preparation: Duration,
    /// Last completed calculation timings, measured on the worker, not this frame.
    pub worker_stitch_cpu: Duration,
    pub worker_morph_cpu: Duration,
    pub result_publication: Duration,
    pub convergence: TerrainConvergenceDiagnostic,
    local_metadata: [Option<(CubePatchAddress, PatchMetadata)>; 31],
    pub peak_transition_bytes: usize,
    pub peak_cpu_bytes: usize,
}
const TRANSITION_RESERVATION: usize = 16 * 1024 * 1024;
const MAX_TRANSITION_RESERVATION: usize = 32 * 1024 * 1024;
fn shared_surface_bytes(surface: &Arc<StitchedSurface>) -> usize {
    surface.resident_bytes() + 2 * size_of::<usize>()
}
struct ActiveMorph {
    mesh: SurfaceTransition,
    destination: Arc<StitchedSurface>,
    destination_cover: Vec<ActiveSurfacePatch>,
    elapsed: Duration,
    duration: Duration,
}
impl ActiveMorph {
    fn fraction(&self) -> f64 {
        (self.elapsed.as_secs_f64() / self.duration.as_secs_f64()).clamp(0.0, 1.0)
    }
    fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.mesh.resident_bytes()
            + shared_surface_bytes(&self.destination)
            + self.destination_cover.capacity() * size_of::<ActiveSurfacePatch>()
    }
}
impl AdaptiveTerrainCover {
    /// Zero disables morphs for the independent static-stitching checkpoint.
    /// An active transition always finishes with its captured duration.
    pub fn set_morph_duration(&mut self, duration: Duration) -> Result<()> {
        anyhow::ensure!(
            duration <= Duration::from_secs(1),
            "terrain morph duration exceeds debug limit"
        );
        if self.morph_duration != duration {
            self.transition_deferred = false;
            self.transition_budget_step = 0;
        }
        self.morph_duration = duration;
        Ok(())
    }
    pub fn transition(&self) -> Option<(&SurfaceTransition, f64)> {
        self.morph.as_ref().map(|m| (&m.mesh, m.fraction()))
    }
    pub fn active(&self) -> &[ActiveSurfacePatch] {
        &self.active
    }
    pub fn visible(&self) -> &[ActiveSurfacePatch] {
        &self.visible
    }
    pub fn ready(&self) -> bool {
        self.stitched.is_some()
    }
    pub fn surface(&self) -> Option<&StitchedSurface> {
        self.stitched.as_deref()
    }
    pub fn construction_pending(&self) -> bool {
        self.construction.is_some()
    }
    pub fn topology(&self) -> Option<&SurfaceTopology> {
        self.lod.as_ref().map(SurfaceLodSession::topology)
    }
    /// Coordinator-owned charge. A pending job/completion owns the shared source
    /// charge separately; sum with cache residency for the aggregate accounting.
    pub fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + (self.active.capacity() + self.visible.capacity()) * size_of::<ActiveSurfacePatch>()
            // An admitted cover reservation owns this same shared allocation
            // until publication or cancellation acknowledgement, not both owners.
            + if self.construction.is_some() { 0 } else {
                self.stitched.as_ref().map_or(0, shared_surface_bytes)
            }
            + self.lod.as_ref().map_or(0, |l| l.cache_usage().1)
            + self.report.scratch_bytes
            + self.morph.as_ref().map_or(0, ActiveMorph::resident_bytes)
    }
    pub(crate) fn abandon_construction(&mut self, cache: &mut TerrainPatchCache) {
        if let Some(id) = self.construction.take() {
            cache.abandon_cover(id);
        }
    }
    fn transition_budget(&self) -> usize {
        #[cfg(test)]
        if let Some(bytes) = self.transition_budget_override {
            return bytes;
        }
        (TRANSITION_RESERVATION + usize::from(self.transition_budget_step) * 8 * 1024 * 1024)
            .min(MAX_TRANSITION_RESERVATION)
    }
    pub fn update(
        &mut self,
        cache: &mut TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        vertex_budget: usize,
        wall_budget: Option<Duration>,
    ) -> Result<TerrainWorkReport> {
        self.update_with_elapsed(
            cache,
            identity,
            input,
            settings,
            vertex_budget,
            wall_budget,
            Duration::ZERO,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn update_with_elapsed(
        &mut self,
        cache: &mut TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        vertex_budget: usize,
        wall_budget: Option<Duration>,
        elapsed: Duration,
    ) -> Result<TerrainWorkReport> {
        if self.identity.as_ref() != Some(identity) {
            self.abandon_construction(cache);
            cache.invalidate_body(identity);
            self.active.clear();
            self.visible.clear();
            self.stitched = None;
            self.morph = None;
            self.construction = None;
            self.local_metadata = [None; 31];
            self.transition_deferred = false;
            self.transition_budget_step = 0;
            self.generator = Some(TerrainGenerator::new(
                &identity.definition,
                identity.radius_m,
            )?);
            let generator = self
                .generator
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing terrain generator"))?;
            for level in 0u8..=30 {
                let footprint = |l| {
                    TerrainFootprint::new(identity.radius_m * 2.0 / (16.0 * (1u64 << l) as f64))
                };
                self.boundary_bounds[usize::from(level)] = generator.profile_difference_bound_m(
                    footprint(level)?,
                    footprint(level.saturating_sub(2))?,
                )?;
            }
            self.lod = Some(SurfaceLodSession::new(MAX_TERRAIN_PATCHES)?);
            self.identity = Some(identity.clone());
        }
        if let Some(morph) = self.morph.as_mut() {
            morph.elapsed = morph.elapsed.saturating_add(elapsed);
        }
        if self.morph.as_ref().is_some_and(|m| m.elapsed >= m.duration) {
            let morph = self
                .morph
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing completed terrain morph"))?;
            self.stitched = Some(morph.destination);
            self.active = morph.destination_cover;
        }
        cache.unpin_body(identity.body);
        for p in &self.active {
            cache.pin(identity, p.address);
        }
        if let Some(morph) = &self.morph {
            for p in &morph.destination_cover {
                cache.pin(identity, p.address);
            }
        }
        if self.construction.is_some()
            && let Some(lod) = &self.lod
        {
            for address in lod.covering_leaves() {
                cache.pin(identity, address);
            }
        }
        let unpublished_target = self.stitched.is_some()
            && self.morph.is_none()
            && self.lod.as_ref().is_some_and(|lod| {
                !lod.covering_leaves()
                    .eq(self.active.iter().map(|p| p.address))
            });
        if unpublished_target && let Some(lod) = &self.lod {
            // Budget retries must not evict their own immutable destination and
            // then freeze forever waiting for geometry nobody requests again.
            for address in lod.covering_leaves() {
                cache.request_pinned(identity, address);
            }
        }
        let roots = CubeFace::ALL.map(CubePatchAddress::root);
        for root in roots {
            cache.pin(identity, root);
            cache.request_pinned(identity, root);
        }
        // Reserve an explicit renderer-facing staging allowance in addition to
        // actual derived geometry. Never raise the aggregate 128 MiB cap.
        // Renderer staging is shared with smooth bodies and retains capacities
        // across frames. Reserve its full outgoing (64 MiB) and boundary (8 MiB)
        // caps, not a typical-view payload, before admitting terrain allocations.
        const STAGING_ALLOWANCE: usize = 72 * 1024 * 1024 + 32 * 1024;
        let transition_budget = self.transition_budget();
        // Selector scratch is separately bounded by 8 MiB; account for capacity
        // growth before its metadata/balance traversal, not after allocation.
        let transition_reserve =
            if self.morph_duration.is_zero() || self.morph.is_some() || self.construction.is_some()
            {
                0
            } else {
                transition_budget
            };
        let reserve =
            self.resident_bytes() + STAGING_ALLOWANCE + 8 * 1024 * 1024 + transition_reserve;
        let admitted = cache.reserve_external(reserve);
        let generator = self
            .generator
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing terrain generator"))?;
        let lod = self
            .lod
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing adaptive selector"))?;
        let mut policy = TerrainSelectionPolicy {
            cache,
            identity,
            generator,
            boundary_bounds: &self.boundary_bounds,
            morph_remaining_m: self
                .morph
                .as_ref()
                .map_or(0.0, |m| m.mesh.max_displacement_m() * (1.0 - m.fraction())),
            required: Vec::new(),
            replacements: 0,
            frozen: !admitted
                || self.stitched.is_none()
                || self.morph.is_some()
                || self.construction.is_some()
                || self.transition_deferred
                || unpublished_target,
            reservation_base: reserve,
        };
        let selection_start = Instant::now();
        self.report = lod.update_with_policy(input, settings, &mut policy)?;
        self.selection_preparation = selection_start.elapsed();
        if !admitted || self.transition_deferred {
            self.report.budget_constrained = true;
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        let mut required = policy.required;
        if unpublished_target {
            required.extend(lod.covering_leaves());
        }
        // Preserve the highest projected-error blocking closure. Everything else
        // is discardable, including an obsolete partially generated builder.
        cache.retain_required(identity, &required);
        let work = cache.generate(vertex_budget, GENERATION_MICROBATCH, wall_budget)?;
        self.result_publication = Duration::ZERO;
        if let Some(completion) = self
            .construction
            .and_then(|id| cache.take_cover_completion(id))
        {
            let start = Instant::now();
            if self.construction == Some(completion.id) && completion.identity == *identity {
                self.construction = None;
                match completion.result {
                    Ok(workers::Output::Cover(output)) => {
                        self.transition_deferred = false;
                        self.transition_budget_step = 0;
                        self.worker_stitch_cpu = output.stitch_cpu;
                        self.worker_morph_cpu = output.morph_cpu;
                        if let Some(mesh) = output.transition {
                            self.peak_transition_bytes =
                                self.peak_transition_bytes.max(mesh.resident_bytes());
                            self.morph = Some(ActiveMorph {
                                mesh,
                                destination: output.surface,
                                destination_cover: output.cover,
                                elapsed: Duration::ZERO,
                                duration: self.construction_duration,
                            });
                        } else {
                            self.stitched = Some(output.surface);
                            self.active = output.cover;
                        }
                    }
                    Err(error)
                        if error
                            .downcast_ref::<RenderPreparationError>()
                            .is_some_and(|e| {
                                matches!(e, RenderPreparationError::InvalidBudget)
                            }) =>
                    {
                        self.transition_budget_step = self.transition_budget_step.saturating_add(1);
                        self.transition_deferred = self.transition_budget_step > 2;
                        self.report.budget_constrained = true;
                        self.report.quality_pending = true;
                        self.report.settled = false;
                    }
                    Err(error) => return Err(error),
                    Ok(workers::Output::Cancelled) => {}
                    _ => anyhow::bail!("unexpected terrain cover completion"),
                }
            }
            self.result_publication = start.elapsed();
        }
        // Publication restores coordinator ownership before any new allocation.
        // A failed overlay retries only within the fixed aggregate cap.
        let transition_budget = (TRANSITION_RESERVATION
            + usize::from(self.transition_budget_step) * 8 * 1024 * 1024)
            .min(MAX_TRANSITION_RESERVATION);
        #[cfg(test)]
        let transition_budget = self.transition_budget_override.unwrap_or(transition_budget);
        let transition_reserve =
            if self.morph_duration.is_zero() || self.morph.is_some() || self.construction.is_some()
            {
                0
            } else {
                transition_budget
            };
        let reserve =
            self.resident_bytes() + STAGING_ALLOWANCE + 8 * 1024 * 1024 + transition_reserve;
        let construction_admitted = cache.reserve_external(reserve);
        if !construction_admitted {
            self.report.budget_constrained = true;
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        let lod = self
            .lod
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing adaptive selector"))?;
        let addresses: Vec<_> = lod.covering_leaves().collect();
        let changed = self.active.len() != addresses.len()
            || self
                .active
                .iter()
                .zip(&addresses)
                .any(|(p, a)| p.address != *a);
        self.stitch_preparation = Duration::ZERO;
        self.morph_preparation = Duration::ZERO;
        if changed
            && construction_admitted
            && !self.transition_deferred
            && self.morph.is_none()
            && self.construction.is_none()
            && addresses.iter().all(|&a| cache.peek(identity, a).is_some())
            && cache.reserve_external(
                reserve
                    + StitchedSurface::construction_bytes(addresses.len())
                    + addresses.len()
                        * (size_of::<ActiveSurfacePatch>() + size_of::<&GeneratedSurfacePatch>()),
            )
        {
            let start = Instant::now();
            let mut active = Vec::with_capacity(addresses.len());
            for &address in &addresses {
                let mut mask = 0;
                for edge in PatchEdge::ALL {
                    let mut n = Some(address.neighbor(edge).address);
                    while let Some(neighbor) = n {
                        if addresses.binary_search(&neighbor).is_ok() {
                            if neighbor.level() + 1 == address.level() {
                                mask |= edge.bit();
                            }
                            break;
                        }
                        n = neighbor.parent();
                    }
                }
                active.push(ActiveSurfacePatch {
                    address,
                    stitch_mask: mask,
                    metadata: cache.metadata(identity, address)?,
                    error_pixels: 0.0,
                });
            }
            // Build against the COMPLETE ready cover before frustum selection.
            let geometry = addresses
                .iter()
                .map(|&a| {
                    cache
                        .peek(identity, a)
                        .ok_or_else(|| anyhow::anyhow!("incomplete ready replacement"))
                })
                .collect::<Result<Vec<_>>>()?;
            if cache.workers.is_some() {
                // Raw Arc inputs remain charged in cache entries until the final
                // worker handle drops, even after invalidation. Source stitching
                // is reserved here as well so a session switch cannot unaccount it.
                let bytes = StitchedSurface::construction_bytes(active.len())
                    + self.stitched.as_ref().map_or(0, shared_surface_bytes)
                    + (self.active.len() + active.len())
                        * (size_of::<ActiveSurfacePatch>()
                            + size_of::<Arc<GeneratedSurfacePatch>>())
                    + (self.active.len() + 2 * active.len()) * size_of::<&GeneratedSurfacePatch>()
                    + 256 // shared allocation headers and result envelope
                    + if self.morph_duration.is_zero() || self.stitched.is_none() {
                        0
                    } else {
                        transition_budget
                    };
                let source_bytes = self.stitched.as_ref().map_or(0, shared_surface_bytes);
                let worker_external = reserve.saturating_sub(transition_reserve + source_bytes);
                if cache.workers.as_ref().is_some_and(|w| w.idle())
                    && cache.completed_covers.len()
                        + cache
                            .workers
                            .as_ref()
                            .map_or(0, workers::TerrainWorkers::pending_covers)
                        < 4
                    && cache.reserve_external(worker_external + bytes)
                {
                    let clone_raw = |patches: &[ActiveSurfacePatch]| -> Result<Vec<Arc<GeneratedSurfacePatch>>> {
                        patches.iter().map(|p| cache.entry_index(identity, p.address)
                            .map(|i| cache.entries[i].patch.clone())
                            .ok_or_else(|| anyhow::anyhow!("missing pinned worker input"))).collect()
                    };
                    let job = workers::CoverJob {
                        old: self.stitched.clone(),
                        old_cover: self.active.clone(),
                        old_raw: clone_raw(&self.active)?,
                        raw: clone_raw(&active)?,
                        cover: active,
                        transition_budget,
                        morph: !self.morph_duration.is_zero(),
                    };
                    // Transfer the preflight reservation from external to pool ownership.
                    anyhow::ensure!(
                        cache.reserve_external(worker_external),
                        "terrain worker reservation transfer failed"
                    );
                    self.construction = Some(
                        cache
                            .workers
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("missing workers"))?
                            .submit_cover(identity, job, bytes)?,
                    );
                    self.construction_duration = self.morph_duration;
                    cache.record_peak();
                } else {
                    self.report.budget_constrained = true;
                }
                self.stitch_preparation = start.elapsed();
            } else {
                let surface = Arc::new(StitchedSurface::build(&active, &geometry, lod.topology())?);
                self.stitch_preparation = start.elapsed();
                if !self.morph_duration.is_zero()
                    && let Some(old) = &self.stitched
                {
                    let start = Instant::now();
                    let prepared = SurfaceTransition::build(
                        &self.active,
                        old,
                        &active,
                        &surface,
                        lod.topology(),
                        transition_budget,
                    );
                    self.morph_preparation = start.elapsed();
                    match prepared {
                        Ok(mesh) => {
                            self.transition_budget_step = 0;
                            self.peak_transition_bytes =
                                self.peak_transition_bytes.max(mesh.resident_bytes());
                            self.morph = Some(ActiveMorph {
                                mesh,
                                destination: surface,
                                destination_cover: active,
                                elapsed: Duration::ZERO,
                                duration: self.morph_duration,
                            });
                        }
                        Err(RenderPreparationError::InvalidBudget) => {
                            // The selector's private target is frozen; it must not
                            // advance another level beyond the still-published source.
                            self.transition_budget_step =
                                self.transition_budget_step.saturating_add(1);
                            self.transition_deferred = self.transition_budget_step > 2;
                            self.report.budget_constrained = true;
                            self.report.quality_pending = true;
                            self.report.settled = false;
                        }
                        Err(error) => return Err(error.into()),
                    }
                } else {
                    self.stitched = Some(surface);
                    self.active = active;
                }
            }
        }
        for p in &self.active {
            cache.pin(identity, p.address);
        }
        if let Some(morph) = &self.morph {
            for p in &morph.destination_cover {
                cache.pin(identity, p.address);
            }
        }
        if self.construction.is_some() {
            for address in lod.covering_leaves() {
                cache.pin(identity, address);
            }
        }
        if self.construction.is_some() || self.morph.is_some() || changed {
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        self.prepare_visible(input)?;
        self.update_convergence(cache, identity, input, settings)?;
        if self.transition_deferred {
            // Handoff quality must describe what remains displayed, not the
            // finer unpublished target whose overlay could not be admitted.
            self.report.max_error_pixels = self
                .visible
                .iter()
                .map(|p| p.error_pixels)
                .fold(0.0, f64::max);
        }
        let external = self.resident_bytes() + STAGING_ALLOWANCE;
        anyhow::ensure!(
            cache.reserve_external(external),
            "terrain cover exceeded aggregate CPU budget"
        );
        self.peak_cpu_bytes = cache.report().peak_aggregate_bytes;
        anyhow::ensure!(
            self.peak_cpu_bytes <= TERRAIN_CPU_CAP_BYTES,
            "terrain aggregate peak exceeded 128 MiB"
        );
        Ok(work)
    }
    fn update_convergence(
        &mut self,
        cache: &TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
    ) -> Result<()> {
        let start = Instant::now();
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        let Ok(direction) = Direction3::try_new(source.observer_in_source().metres()) else {
            self.convergence = TerrainConvergenceDiagnostic::default();
            return Ok(());
        };
        let (face, uv) = SurfaceLocation::new(direction).face_uv();
        let address_at = |level: u8| -> Result<CubePatchAddress> {
            let count = 1u32 << level;
            let coordinate = |value: f64| {
                (((value + 1.0) * 0.5 * f64::from(count)).floor() as u32).min(count - 1)
            };
            Ok(CubePatchAddress::try_new(
                face,
                level,
                coordinate(uv[0]),
                coordinate(uv[1]),
            )?)
        };
        let local = address_at(30)?;
        let mut diagnostic = TerrainConvergenceDiagnostic {
            rendered_local_lod: self
                .active
                .iter()
                .find(|p| p.address.contains(local))
                .map(|p| p.address.level()),
            ..TerrainConvergenceDiagnostic::default()
        };
        let generator = self
            .generator
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing diagnostic generator"))?;
        let topology = self
            .lod
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing diagnostic topology"))?
            .topology();
        for level in 0..=30 {
            let address = address_at(level)?;
            let geometry = cache.peek(identity, address);
            if geometry.is_some() {
                diagnostic.ready_local_lod = Some(level);
            }
            if diagnostic.target_certifiable {
                continue;
            }
            let slot = &mut self.local_metadata[usize::from(level)];
            if slot.is_none_or(|(old, _)| old != address) {
                *slot = Some((address, PatchMetadata::build(address, topology)?));
            }
            let metadata = slot
                .ok_or_else(|| anyhow::anyhow!("missing local certificate metadata"))?
                .1;
            let (mut extent, mut error) = geometry
                .map(|g| (g.extent(), g.error()))
                .map(Ok)
                .unwrap_or_else(|| terrain_surface_certificate(generator, address, metadata))?;
            error.boundary_constraint_m =
                (self.boundary_bounds[usize::from(level)] + error.numeric_m).next_up();
            extent.min_height_m = (extent.min_height_m - error.boundary_constraint_m).next_down();
            extent.max_height_m = (extent.max_height_m + error.boundary_constraint_m).next_up();
            let (center, radius) = metadata.ball(identity.radius_m, extent)?;
            let center = source
                .view_displacement(mundaris_math::FramePosition::new(
                    input.body_fixed_frame,
                    mundaris_math::LocalPosition::try_metres(center)?,
                ))?
                .metres();
            if metadata.projected_total_error(error, center, radius, input.projection)?
                <= settings.split_pixels()
            {
                diagnostic.desired_local_lod = Some(level);
                diagnostic.target_certifiable = true;
            } else if level == 30 {
                diagnostic.desired_local_lod = Some(30);
            }
        }
        if let Some(level) = diagnostic.rendered_local_lod
            && let Some(geometry) = cache.peek(identity, address_at(level)?)
        {
            diagnostic.local_error = geometry.error();
            diagnostic.local_error.boundary_constraint_m = self.boundary_bounds[usize::from(level)];
            diagnostic.local_error.morph_remaining_m = self
                .morph
                .as_ref()
                .map_or(0.0, |m| m.mesh.max_displacement_m() * (1.0 - m.fraction()));
        }
        diagnostic.diagnostic_cpu = start.elapsed();
        self.convergence = diagnostic;
        Ok(())
    }
    /// Re-culls the published cover without selection, generation, or publication.
    pub(crate) fn prepare_visible(&mut self, input: &SurfaceViewInput<'_, '_>) -> Result<()> {
        self.visible.clear();
        let Some(surface) = &self.stitched else {
            return Ok(());
        };
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        for (p, g) in self.active.iter().zip(surface.patches()) {
            // The common-refinement group is drawn instead, including its new
            // visibility. Never double-draw the source triangles beneath it.
            if self
                .morph
                .as_ref()
                .is_some_and(|m| m.mesh.affected_old().binary_search(&p.address).is_ok())
            {
                continue;
            }
            let (center, radius) = p.metadata.ball(g.reference_radius_m(), g.extent())?;
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
            p.error_pixels =
                p.metadata
                    .projected_total_error(g.error(), center, radius, input.projection)?;
            self.visible.push(p);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use mundaris_math::*;
    use mundaris_renderer::*;
    use mundaris_world::*;
    use std::num::NonZeroU64;

    #[test]
    fn overlay_budget_rejection_retains_source_and_freezes_private_target() {
        let mut world = CelestialSystem::new(NonZeroU64::new(82).unwrap(), SimulationInstant::ZERO);
        let body = world
            .insert_body(
                "deferred morph fixture",
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
        let tree = FrameTree::new(NonZeroU64::new(82).unwrap());
        let fixed = tree.root();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(fixed, LocalPosition::try_metres(DVec3::Z * 1100.0).unwrap()),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: fixed,
            reference_radius_m: 1000.0,
            projection: CelestialProjection::try_new(128, 96, 60.0f64.to_radians(), 0.1).unwrap(),
        };
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 128).unwrap();
        let mut cover = AdaptiveTerrainCover::default();
        cover
            .set_morph_duration(Duration::from_millis(150))
            .unwrap();
        let roots = LodSettings::default().with_limits(128, 128, 0).unwrap();
        for _ in 0..40 {
            cover
                .update(&mut cache, &identity, &input, &roots, 64, None)
                .unwrap();
            if cover.ready() {
                break;
            }
        }
        assert_eq!(cover.active().len(), 6);
        let source = cover.active().iter().map(|p| p.address).collect::<Vec<_>>();
        // A post-publication camera correction re-culls only derived visibility.
        let samples = cover.surface().unwrap().patches()[0].samples().as_ptr();
        let cache_before_recull = cache.report();
        let away = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(DVec3::Z * 10_000.0).unwrap(),
                ),
                UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_y(
                    std::f64::consts::PI,
                ))
                .unwrap(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        cover
            .prepare_visible(&SurfaceViewInput {
                view: &away,
                ..input
            })
            .unwrap();
        assert!(cover.visible().is_empty());
        assert_eq!(
            cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
            source
        );
        assert_eq!(
            cover.surface().unwrap().patches()[0].samples().as_ptr(),
            samples
        );
        assert_eq!(
            cache.report().resident_bytes,
            cache_before_recull.resident_bytes
        );
        assert_eq!(cache.report().evictions, cache_before_recull.evictions);
        cover.prepare_visible(&input).unwrap();
        assert!(!cover.visible().is_empty());
        cover.transition_budget_override = Some(1);
        let refined = LodSettings::default().with_limits(128, 128, 1).unwrap();
        for _ in 0..100 {
            cover
                .update(&mut cache, &identity, &input, &refined, 64, None)
                .unwrap();
            if cover.transition_deferred {
                break;
            }
        }
        assert!(cover.transition_deferred && cover.ready());
        assert!(cover.transition().is_none());
        assert!(cache.report().pinned_patches >= source.len());
        assert_eq!(
            cover.report.max_error_pixels,
            cover
                .visible()
                .iter()
                .map(|p| p.error_pixels)
                .fold(0.0, f64::max)
        );
        let target = cover
            .lod
            .as_ref()
            .unwrap()
            .covering_leaves()
            .collect::<Vec<_>>();
        assert!(target.len() > source.len());
        let retry_dependency = *target.iter().find(|a| !source.contains(a)).unwrap();
        let index = cache.entry_index(&identity, retry_dependency).unwrap();
        cache.entries.remove(index);
        for _ in 0..10 {
            cover
                .update_with_elapsed(
                    &mut cache,
                    &identity,
                    &input,
                    &refined,
                    64,
                    None,
                    Duration::from_secs(1),
                )
                .unwrap();
            assert_eq!(
                cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
                source
            );
            assert_eq!(
                cover
                    .lod
                    .as_ref()
                    .unwrap()
                    .covering_leaves()
                    .collect::<Vec<_>>(),
                target
            );
            assert!(
                cover.report.budget_constrained
                    && cover.report.quality_pending
                    && !cover.report.settled
            );
        }
        assert!(
            cache.peek(&identity, retry_dependency).is_some(),
            "a frozen private target must reconstruct an evicted retry dependency"
        );
        cover.set_morph_duration(Duration::ZERO).unwrap();
        cover
            .update(&mut cache, &identity, &input, &refined, 64, None)
            .unwrap();
        assert!(!cover.transition_deferred);
        assert!(cover.active().len() > source.len());
        assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
    }

    #[test]
    fn worker_source_charge_survives_cancellation_and_cover_abandonment() {
        let world = crate::solar_system::SolarSystemPreset::gameplay()
            .create(NonZeroU64::new(5108).unwrap())
            .unwrap();
        let (body, state) = world
            .bodies()
            .nth(crate::solar_system::SolarBody::Earth as usize)
            .unwrap();
        let identity = TerrainGeometryIdentity::new(
            body,
            state.terrain().unwrap().clone(),
            state.terrain_revision(),
            state.properties().reference_radius_m(),
        )
        .unwrap();
        let tree = FrameTree::new(NonZeroU64::new(5108).unwrap());
        let fixed = tree.root();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(DVec3::Z * (identity.radius_m + 100.0)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: fixed,
            reference_radius_m: identity.radius_m,
            projection: CelestialProjection::try_new(128, 96, 60.0f64.to_radians(), 0.1).unwrap(),
        };
        for count in [1, 2, 4] {
            let mut cache =
                TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 128, count).unwrap();
            let mut cover = AdaptiveTerrainCover::default();
            cover
                .set_morph_duration(Duration::from_millis(150))
                .unwrap();
            let roots = LodSettings::default().with_limits(128, 128, 0).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !cover.ready() {
                cover
                    .update(&mut cache, &identity, &input, &roots, 1, None)
                    .unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            let source_bytes = shared_surface_bytes(cover.stitched.as_ref().unwrap());
            let refined = LodSettings::default().with_limits(128, 128, 1).unwrap();
            while !cover.construction_pending() {
                cover
                    .update(&mut cache, &identity, &input, &refined, 1, None)
                    .unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            let job = cover.construction.unwrap();
            assert!(cache.report().worker_reserved_bytes >= source_bytes);
            let charged_cover_bytes = cover.resident_bytes();
            cover.construction = None;
            assert_eq!(cover.resident_bytes(), charged_cover_bytes + source_bytes);
            cover.construction = Some(job);
            cache.cancel_body_work(body);
            // Publication into the completion queue must not release the source
            // charge before the coordinator consumes its cancellation ack.
            while !cache.completed_covers.iter().any(|c| c.id == job) {
                cache.generate(0, GENERATION_MICROBATCH, None).unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(cache.report().completed_unpublished_bytes >= source_bytes);
            assert!(matches!(
                cache.completed_covers[0].result,
                Ok(workers::Output::Cancelled)
            ));
            cover
                .update(&mut cache, &identity, &input, &refined, 0, None)
                .unwrap();
            assert_ne!(cover.construction, Some(job));
            assert!(cover.ready());
            assert!(
                cache.report().resident_bytes + cache.report().external_bytes
                    <= TERRAIN_CPU_CAP_BYTES
            );
            cover.abandon_construction(&mut cache);
            drop(cover);
            cache.reserve_external(0);
            while cache.pending() != 0 {
                cache.generate(0, GENERATION_MICROBATCH, None).unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(cache.completed_covers.is_empty());
            assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
        }
    }
}
