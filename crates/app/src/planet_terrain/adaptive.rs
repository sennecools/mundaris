//! Desired selection, bounded generation and atomic ready-cover publication.
use super::*;
use mundaris_renderer::RenderPreparationError;

/// Coarse update-stage timings; existing selection/stitch fields remain parent scopes.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct AdaptiveTerrainProfile {
    pub identity_reset: Duration,
    pub morph_bookkeeping: Duration,
    pub stale_destination_validation: Duration,
    pub pinning_reservation: Duration,
    pub selector_setup: Duration,
    pub selector: Duration,
    pub successor_preparation_prefetch: Duration,
    pub cache_generation: Duration,
    pub cover_publication: Duration,
    pub construction_submission: Duration,
    pub visibility: Duration,
    pub diagnostics: Duration,
    pub final_reservation: Duration,
    pub total: Duration,
}

/// Camera-radial demand is distinct from complete coverage and cache residency.
#[derive(Debug, Default, Clone, Copy)]
pub struct TerrainConvergenceDiagnostic {
    pub desired_local_lod: Option<u8>,
    pub ready_local_lod: Option<u8>,
    pub rendered_local_lod: Option<u8>,
    pub target_certifiable: bool,
    pub local_error: SurfaceErrorContributions,
    pub diagnostic_cpu: Duration,
    pub useful_target_lod: Option<u8>,
    pub certificate_limited_target_lod: Option<u8>,
    pub desired_patch_width_m: f64,
    pub desired_sample_spacing_m: f64,
    pub rendered_sample_spacing_m: f64,
    pub terrain_footprint_m: f64,
    pub projected_depth_floor_m: f64,
    pub pixel_footprint_m: f64,
    pub desired_error: SurfaceErrorContributions,
    pub desired_error_pixels: [f64; 6],
    pub desired_total_pixels: f64,
    pub dominant_term: &'static str,
    pub represented_height_bound_m: f64,
}

type CachedCertificate = (CubePatchAddress, SurfaceExtent, SurfaceErrorContributions);
struct CertificateCache {
    entries: [Option<CachedCertificate>; 256],
    cursor: usize,
}
impl Default for CertificateCache {
    fn default() -> Self {
        Self {
            entries: [None; 256],
            cursor: 0,
        }
    }
}
impl CertificateCache {
    fn get(
        &mut self,
        generator: &NativeTerrainGenerator,
        address: CubePatchAddress,
        metadata: PatchMetadata,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions)> {
        if let Some((_, extent, error)) = self
            .entries
            .iter()
            .flatten()
            .find(|(a, _, _)| *a == address)
        {
            return Ok((*extent, *error));
        }
        let (extent, error) = generator.surface_certificate(address, metadata)?;
        self.entries[self.cursor] = Some((address, extent, error));
        self.cursor = (self.cursor + 1) % self.entries.len();
        Ok((extent, error))
    }
}

fn physical_resolution(address: CubePatchAddress, radius_m: f64) -> Result<(f64, f64)> {
    let n = |x, y| -> Result<glam::DVec3> { Ok(address.sample_direction(x, y, 16)?.unit()) };
    let arc = |a: glam::DVec3, b: glam::DVec3| radius_m * a.cross(b).length().atan2(a.dot(b));
    let width = arc(n(0, 8)?, n(16, 8)?).max(arc(n(8, 0)?, n(8, 16)?));
    let spacing = arc(n(8, 8)?, n(9, 8)?).max(arc(n(8, 8)?, n(8, 9)?));
    Ok((width, spacing))
}

fn boundary_profile_bound(
    generator: &NativeTerrainGenerator,
    address: CubePatchAddress,
    metadata: PatchMetadata,
    global_bounds: &[f64; 31],
) -> Result<f64> {
    if !generator.has_local_profile_bounds() {
        return Ok(global_bounds[usize::from(address.level())]);
    }
    let footprint =
        |level| TerrainFootprint::new(generator.radius_m() * 2.0 / (16.0 * (1u64 << level) as f64));
    let (axis, alpha) = metadata.cap();
    // Stitch owners share the same canonical direction, contained in this cap.
    // First-two-row corrections are convex combinations of these edge deltas,
    // so no neighboring chart's spatial support is needed for this bound.
    generator.profile_difference_bound_for_region_m(
        DirectionalCap::new(Direction3::try_new(axis)?, alpha)?,
        footprint(address.level())?,
        footprint(address.level().saturating_sub(2))?,
    )
}

/// Temporary mutable bridge. Renderer receives only certificates/readiness, never
/// a terrain definition, generator seed or procedural query interface.
pub struct TerrainSelectionPolicy<'a> {
    cache: &'a mut TerrainPatchCache,
    identity: &'a TerrainGeometryIdentity,
    generator: &'a NativeTerrainGenerator,
    boundary_bounds: &'a [f64; 31],
    morph_remaining_m: f64,
    required: Vec<CubePatchAddress>,
    replacements: usize,
    changed_parents: [Option<(CubePatchAddress, bool)>; 32],
    frozen: bool,
    reservation_base: usize,
    certificates: &'a mut CertificateCache,
    representation_clearance_m: Option<f64>,
    split_pixels: f64,
}
impl SurfaceGeometryPolicy for TerrainSelectionPolicy<'_> {
    fn prioritize_observer_patch(&self) -> bool {
        self.representation_clearance_m.is_none()
    }
    fn refinement_demand_pixels(
        &mut self,
        input: SurfaceRefinementInput,
    ) -> std::result::Result<f64, RenderPreparationError> {
        Ok(self.generator.representation_demand_pixels(
            input,
            self.representation_clearance_m,
            self.split_pixels,
        ))
    }
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
            self.certificates
                .get(self.generator, address, metadata)
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
        };
        // Edge balance also bounds a four-quadrant vertex's coarsest owner by
        // two levels (the edge-neighbor graph has diameter two). Cube corners
        // have three mutually adjacent faces. No global-height morph allowance.
        let boundary =
            boundary_profile_bound(self.generator, address, metadata, self.boundary_bounds)
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
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
        !self.frozen
            && self.replacements
                < if self.representation_clearance_m.is_some() {
                    MAX_REFINEMENT_REPLACEMENTS
                } else {
                    1
                }
    }
    fn allow_coarsening_replacement(&self) -> bool {
        // Retire independent sibling groups together after a view change rather
        // than constructing and morphing an entire cover for every single merge.
        // Every merge still passes readiness, balance, and memory admission.
        !self.frozen
            && self.replacements
                < if self.representation_clearance_m.is_some() {
                    32
                } else {
                    1
                }
    }
    fn replacement_committed(&mut self) {
        self.replacements += 1;
    }
    fn refinement_committed(&mut self, parent: CubePatchAddress) {
        self.changed_parents[self.replacements] = Some((parent, true));
        self.replacements += 1;
    }
    fn coarsening_committed(&mut self, parent: CubePatchAddress) {
        self.changed_parents[self.replacements] = Some((parent, false));
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
    #[cfg(feature = "surface-profile")]
    pub profile: AdaptiveTerrainProfile,
    identity: Option<TerrainGeometryIdentity>,
    generator: Option<NativeTerrainGenerator>,
    boundary_bounds: [f64; 31],
    lod: Option<SurfaceLodSession>,
    active: Vec<ActiveSurfacePatch>,
    visible: Vec<ActiveSurfacePatch>,
    stitched: Option<Arc<StitchedSurface>>,
    morph: Option<ActiveMorph>,
    /// At most one exact successor, built from the active morph's immutable endpoint.
    /// It cannot display until that endpoint has been promoted to the source.
    queued: Option<workers::CoverOutput>,
    queued_duration: Duration,
    queued_parent: Option<(CubePatchAddress, bool)>,
    construction: Option<u64>,
    construction_from_morph: bool,
    construction_duration: Duration,
    last_construction_budget: usize,
    construction_parent: Option<CubePatchAddress>,
    construction_obsolete: bool,
    construction_refining: bool,
    unpublished_parent: Option<(CubePatchAddress, bool)>,
    unpublished_changes: [Option<(CubePatchAddress, bool)>; 32],
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
    local_certificates: [Option<CachedCertificate>; 31],
    certificates: CertificateCache,
    prefetch_parents: [Option<CubePatchAddress>; MAX_REFINEMENT_REPLACEMENTS],
    prefetch_dependencies: Vec<CubePatchAddress>,
    pub peak_transition_bytes: usize,
    pub peak_cpu_bytes: usize,
}
const TRANSITION_RESERVATION: usize = 16 * 1024 * 1024;
const MAX_TRANSITION_RESERVATION: usize = 32 * 1024 * 1024;
const SELECTOR_SCRATCH_RESERVATION: usize = 8 * 1024 * 1024;
const MAX_REFINEMENT_REPLACEMENTS: usize = 8;
// `required` and its publication snapshot remain live through raw generation.
// Bound their combined transient allocation before selection.
const SELECTOR_TEMPORARY_RESERVATION: usize =
    2 * MAX_TERRAIN_PATCHES * size_of::<CubePatchAddress>();
fn mark_generation_reservation_rejection(report: &mut LodReport, before: u64, after: u64) {
    if after > before {
        report.budget_constrained = true;
        report.quality_pending = true;
        report.settled = false;
    }
}
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
    /// Captured display duration; changing the debug setting never retimes this mesh.
    pub fn active_morph_duration(&self) -> Option<Duration> {
        self.morph.as_ref().map(|m| m.duration)
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
    /// A complete successor exists but cannot display before the current endpoint.
    pub fn successor_ready(&self) -> bool {
        self.queued.is_some()
    }
    /// Current bounded overlay reservation, including a retained retry allowance.
    pub fn construction_budget_bytes(&self) -> usize {
        self.transition_budget()
    }
    /// Last admitted construction's actual envelope, not its larger retry ceiling.
    pub fn last_construction_budget_bytes(&self) -> usize {
        self.last_construction_budget
    }
    pub fn topology(&self) -> Option<&SurfaceTopology> {
        self.lod.as_ref().map(SurfaceLodSession::topology)
    }
    /// Coordinator-owned charge. A pending job/completion owns the shared source
    /// charge separately; sum with cache residency for the aggregate accounting.
    pub fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.generator.as_ref().map_or(0, |generator| generator.resident_heap_bytes() + generator.query_workspace_bytes())
            + self.prefetch_dependencies.capacity() * size_of::<CubePatchAddress>()
            + (self.active.capacity() + self.visible.capacity()) * size_of::<ActiveSurfacePatch>()
            // An admitted cover reservation owns this same shared allocation
            // until publication or cancellation acknowledgement, not both owners.
            + if self.construction.is_some() && !self.construction_from_morph { 0 } else {
                self.stitched.as_ref().map_or(0, shared_surface_bytes)
            }
            + self.lod.as_ref().map_or(0, |l| l.cache_usage().1)
            + self.report.scratch_bytes
            + self.morph.as_ref().map_or(0, ActiveMorph::resident_bytes)
            - if self.construction.is_some() && self.construction_from_morph {
                self.morph
                    .as_ref()
                    .map_or(0, |m| shared_surface_bytes(&m.destination))
            } else {
                0
            }
            + self.queued.as_ref().map_or(0, |output| {
                size_of::<workers::CoverOutput>()
                    + shared_surface_bytes(&output.surface)
                    + output.cover.capacity() * size_of::<ActiveSurfacePatch>()
                    + output
                        .transition
                        .as_ref()
                        .map_or(0, SurfaceTransition::resident_bytes)
            })
    }
    fn endpoint_cover(&self) -> &[ActiveSurfacePatch] {
        self.morph
            .as_ref()
            .map_or(&self.active, |m| &m.destination_cover)
    }
    fn restore_endpoint(&mut self) -> Result<()> {
        let cover = self.endpoint_cover().to_vec();
        self.lod
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("missing selector"))?
            .restore_published_cover(&cover)?;
        self.prefetch_parents = [None; MAX_REFINEMENT_REPLACEMENTS];
        self.prefetch_dependencies.clear();
        Ok(())
    }
    fn publish_output(&mut self, output: workers::CoverOutput, duration: Duration) {
        if let Some(mesh) = output.transition {
            self.peak_transition_bytes = self.peak_transition_bytes.max(mesh.resident_bytes());
            self.morph = Some(ActiveMorph {
                mesh,
                destination: output.surface,
                destination_cover: output.cover,
                elapsed: Duration::ZERO,
                duration,
            });
        } else {
            self.stitched = Some(output.surface);
            self.active = output.cover;
        }
    }
    fn display_duration(
        mesh: &SurfaceTransition,
        input: &SurfaceViewInput<'_, '_>,
        ordinary: Duration,
    ) -> Result<Duration> {
        if ordinary.is_zero() {
            return Ok(Duration::ZERO);
        }
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        let origin = source.observer_in_source().metres();
        let axes = [glam::DVec3::X, glam::DVec3::Y, glam::DVec3::Z].map(|axis| {
            mundaris_math::Direction3::try_new(axis)
                .map_err(|_| anyhow::anyhow!("invalid camera basis"))
                .and_then(|direction| {
                    source
                        .view_direction(direction)
                        .map(|d| d.unit())
                        .map_err(Into::into)
                })
        });
        let [x, y, z] = axes;
        let (x, y, z) = (x?, y?, z?);
        let rotation = glam::DQuat::from_mat3(&glam::DMat3::from_cols(x, y, z));
        let pixels = mesh.projected_displacement_pixels(input.projection, origin, rotation);
        if !pixels.is_finite() || pixels >= 8.0 {
            return Ok(ordinary);
        }
        let floor = Duration::from_millis(64).min(ordinary);
        // Geometry alone does not certify invisible shading/classification
        // changes. Keep a continuous positive visual interval for all shortened
        // transitions; this floor is a policy, not a perceptual acceptance proof.
        let fraction = (pixels / 8.0).clamp(0.0, 1.0);
        Ok(floor + ordinary.saturating_sub(floor).mul_f64(fraction))
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
        #[cfg(feature = "surface-profile")]
        let profile_started = Instant::now();
        #[cfg(feature = "surface-profile")]
        {
            self.profile = AdaptiveTerrainProfile::default();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        if self.identity.as_ref() != Some(identity) {
            self.abandon_construction(cache);
            cache.invalidate_body(identity);
            self.active.clear();
            self.visible.clear();
            self.stitched = None;
            self.morph = None;
            self.queued = None;
            self.queued_parent = None;
            self.construction = None;
            self.construction_from_morph = false;
            self.last_construction_budget = 0;
            self.construction_parent = None;
            self.construction_obsolete = false;
            self.unpublished_parent = None;
            self.unpublished_changes = [None; 32];
            self.local_metadata = [None; 31];
            self.local_certificates = [None; 31];
            self.certificates = CertificateCache::default();
            self.prefetch_parents = [None; MAX_REFINEMENT_REPLACEMENTS];
            self.prefetch_dependencies.clear();
            self.transition_deferred = false;
            self.transition_budget_step = 0;
            self.generator = Some(NativeTerrainGenerator::new(
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
        #[cfg(feature = "surface-profile")]
        {
            self.profile.identity_reset = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
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
            // The worker now owns the promoted source, not an active destination.
            self.construction_from_morph = false;
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.morph_bookkeeping = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        let representation_clearance_m = self
            .generator
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing terrain generator"))?
            .representation_clearance_m(
                input
                    .view
                    .prepare_source(input.body_fixed_frame)?
                    .observer_in_source()
                    .metres(),
            )?;
        if (self.construction.is_some() || self.queued.is_some())
            && !self.construction_obsolete
            && let Some((parent, refining)) = self
                .construction_parent
                .map(|p| (p, self.construction_refining))
                .or(self.queued_parent)
            && self.stitched.is_some()
        {
            let mut changes = self.unpublished_changes;
            if changes[0].is_none() {
                changes[0] = Some((parent, refining));
            }
            for (parent, refining) in changes.into_iter().flatten() {
                let topology = self
                    .lod
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing selector"))?
                    .topology();
                let metadata = PatchMetadata::build(parent, topology)?;
                let generator = self
                    .generator
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("missing generator"))?;
                let (mut extent, mut error) = self.certificates.get(generator, parent, metadata)?;
                error.boundary_constraint_m =
                    (boundary_profile_bound(generator, parent, metadata, &self.boundary_bounds)?
                        + error.numeric_m)
                        .next_up();
                extent.min_height_m =
                    (extent.min_height_m - error.boundary_constraint_m).next_down();
                extent.max_height_m = (extent.max_height_m + error.boundary_constraint_m).next_up();
                let (center, ball) = metadata.ball(identity.radius_m, extent)?;
                let source = input.view.prepare_source(input.body_fixed_frame)?;
                let center = source
                    .view_displacement(mundaris_math::FramePosition::new(
                        input.body_fixed_frame,
                        mundaris_math::LocalPosition::try_metres(center)?,
                    ))?
                    .metres();
                let visible = !input.projection.rejects_ball(center, ball)?;
                let projected =
                    metadata.projected_total_error(error, center, ball, input.projection)?;
                let demand = generator.representation_demand_pixels(
                    SurfaceRefinementInput {
                        address: parent,
                        metadata,
                        reference_radius_m: identity.radius_m,
                        center_view: center,
                        ball_radius_m: ball,
                        projection: input.projection,
                        certified_error_pixels: projected,
                    },
                    representation_clearance_m,
                    settings.split_pixels(),
                );
                let obsolete = if refining {
                    !visible || demand <= settings.split_pixels()
                } else {
                    visible && demand >= settings.merge_pixels()
                };
                if obsolete {
                    // Keep construction ownership until the cancellation acknowledgement;
                    // the source Arc must not be double charged while the worker holds it.
                    if self.construction.is_some() {
                        self.construction_obsolete = true;
                        cache.cancel_workers(|i, address| i == identity && address.is_none());
                    } else {
                        self.queued = None;
                        self.queued_parent = None;
                        self.restore_endpoint()?;
                    }
                    break;
                }
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.stale_destination_validation = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        if self.morph.is_none()
            && let Some(output) = self.queued.take()
        {
            self.queued_parent = None;
            let duration = output
                .transition
                .as_ref()
                .map_or(Ok(self.queued_duration), |mesh| {
                    Self::display_duration(mesh, input, self.queued_duration)
                })?;
            self.publish_output(output, duration);
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
            && self.lod.as_ref().is_some_and(|lod| {
                !lod.covering_leaves()
                    .eq(self.endpoint_cover().iter().map(|p| p.address))
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
        // actual derived geometry, within the configured aggregate CPU cap.
        // Renderer staging is shared with smooth bodies and retains capacities
        // across frames. Reserve its full outgoing (64 MiB) and boundary (8 MiB)
        // caps, not a typical-view payload, before admitting terrain allocations.
        const STAGING_ALLOWANCE: usize = 72 * 1024 * 1024 + 32 * 1024;
        let transition_budget = self.transition_budget();
        // `resident_bytes` already includes the previous selector's retained
        // scratch. Reserve only the remaining growth to its hard bound, plus a
        // bound for the short-lived address lists that survive into generation.
        let selector_scratch_growth =
            SELECTOR_SCRATCH_RESERVATION.saturating_sub(self.report.scratch_bytes);
        let transition_reserve = if self.morph_duration.is_zero()
            || self.queued.is_some()
            || self.construction.is_some()
        {
            0
        } else {
            transition_budget
        };
        let mut reserve = self.resident_bytes()
            + STAGING_ALLOWANCE
            + selector_scratch_growth
            + SELECTOR_TEMPORARY_RESERVATION
            + transition_reserve;
        cache.operational_reserve_bytes = transition_reserve
            + self.morph.as_ref().map_or(0, |m| {
                m.mesh.resident_bytes() + shared_surface_bytes(&m.destination)
            });
        let admitted = cache.reserve_external(reserve);
        #[cfg(feature = "surface-profile")]
        {
            self.profile.pinning_reservation = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
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
            // Selection describes the immutable endpoint, not the displayed
            // interpolation. Remaining displayed displacement is reported separately.
            morph_remaining_m: 0.0,
            required: Vec::new(),
            replacements: 0,
            changed_parents: [None; 32],
            frozen: !admitted
                || self.stitched.is_none()
                || self.queued.is_some()
                || self.construction.is_some()
                || self.transition_deferred
                || unpublished_target,
            reservation_base: reserve,
            certificates: &mut self.certificates,
            representation_clearance_m,
            split_pixels: settings.split_pixels(),
        };
        #[cfg(feature = "surface-profile")]
        {
            self.profile.selector_setup = stage_started.elapsed();
        }
        let selection_start = Instant::now();
        self.report = lod.update_with_policy(input, settings, &mut policy)?;
        if self.report.splits != 0 || self.report.merges != 0 {
            self.unpublished_changes = policy.changed_parents;
            self.unpublished_parent = policy.changed_parents.into_iter().flatten().last();
        }
        self.selection_preparation = selection_start.elapsed();
        #[cfg(feature = "surface-profile")]
        {
            self.profile.selector = self.selection_preparation;
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        if !admitted || self.transition_deferred {
            self.report.budget_constrained = true;
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        let mut required = policy.required;
        if unpublished_target {
            required.extend(lod.covering_leaves());
        }
        let publishing_required = required.clone();
        // Prepare a bounded priority-ordered group of independent closures so
        // ready splits can share one complete cover construction and morph.
        // Prefetch never advances topology or relaxes the quality certificate.
        let mut next_parents = [None; MAX_REFINEMENT_REPLACEMENTS];
        if representation_clearance_m.is_some() {
            for (slot, parent) in next_parents.iter_mut().zip(lod.refinement_parents()) {
                *slot = Some(parent);
            }
        } else if self.construction.is_some() || self.morph.is_some() {
            let source = input.view.prepare_source(input.body_fixed_frame)?;
            next_parents[0] = Direction3::try_new(source.observer_in_source().metres())
                .ok()
                .and_then(|direction| {
                    let (face, uv) = SurfaceLocation::new(direction).face_uv();
                    lod.active_visible()
                        .iter()
                        .find(|p| {
                            p.error_pixels > settings.split_pixels()
                                && p.address.face() == face
                                && p.address.patch_local(uv).is_ok_and(|st| {
                                    st[0] >= 0.0 && st[0] <= 1.0 && st[1] >= 0.0 && st[1] <= 1.0
                                })
                        })
                        .map(|p| p.address)
                });
        }
        for slot in &mut next_parents {
            let Some(parent) = *slot else { continue };
            let source = input.view.prepare_source(input.body_fixed_frame)?;
            let metadata = lod
                .active_visible()
                .iter()
                .find(|p| p.address == parent)
                .ok_or_else(|| anyhow::anyhow!("missing prefetch parent"))?
                .metadata;
            let (mut extent, mut error) = self.certificates.get(generator, parent, metadata)?;
            error.boundary_constraint_m =
                (boundary_profile_bound(generator, parent, metadata, &self.boundary_bounds)?
                    + error.numeric_m)
                    .next_up();
            extent.min_height_m = (extent.min_height_m - error.boundary_constraint_m).next_down();
            extent.max_height_m = (extent.max_height_m + error.boundary_constraint_m).next_up();
            let (center, ball) = metadata.ball(identity.radius_m, extent)?;
            let center = source
                .view_displacement(mundaris_math::FramePosition::new(
                    input.body_fixed_frame,
                    mundaris_math::LocalPosition::try_metres(center)?,
                ))?
                .metres();
            // An active transition's remaining displacement is not unresolved
            // endpoint terrain and must not manufacture speculative refinement.
            let demand = generator.representation_demand_pixels(
                SurfaceRefinementInput {
                    address: parent,
                    metadata,
                    reference_radius_m: identity.radius_m,
                    center_view: center,
                    ball_radius_m: ball,
                    projection: input.projection,
                    certified_error_pixels: metadata.projected_total_error(
                        error,
                        center,
                        ball,
                        input.projection,
                    )?,
                },
                representation_clearance_m,
                settings.split_pixels(),
            );
            if input.projection.rejects_ball(center, ball)? || demand <= settings.split_pixels() {
                *slot = None;
            }
        }
        // Balance dependencies belong to this complete cover, not just to the
        // requested roots. Even unchanged roots need new closures after a merge
        // or an independent split changes their incident neighbors.
        if self.prefetch_parents != next_parents
            || self.report.splits != 0
            || self.report.merges != 0
        {
            self.prefetch_dependencies.clear();
            for parent in next_parents.into_iter().flatten() {
                match lod.replacement_dependencies(parent, settings) {
                    Ok(dependencies) => {
                        for address in dependencies {
                            if !self.prefetch_dependencies.contains(&address) {
                                if self.prefetch_dependencies.len() == MAX_PENDING_PATCHES {
                                    break;
                                }
                                self.prefetch_dependencies.push(address);
                            }
                        }
                    }
                    Err(RenderPreparationError::InvalidBudget) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            self.prefetch_parents = next_parents;
        }
        for &address in &self.prefetch_dependencies {
            cache.request_pinned(identity, address);
            required.push(address);
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.successor_preparation_prefetch = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        // Preserve the highest projected-error blocking closure. Everything else
        // is discardable, including an obsolete partially generated builder.
        cache.retain_required(identity, &required);
        // Selection has finished. Its unused growth allowance is no longer
        // needed; retain the actual new selector capacity and the address lists
        // that remain live while raw patches are generated.
        let selector_temporaries = required
            .capacity()
            .saturating_add(publishing_required.capacity())
            .saturating_mul(size_of::<CubePatchAddress>());
        reserve =
            self.resident_bytes() + STAGING_ALLOWANCE + transition_reserve + selector_temporaries;
        let generation_admitted = cache.reserve_external(reserve);
        if !generation_admitted {
            self.report.budget_constrained = true;
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        let ready_before = publishing_required
            .iter()
            .filter(|&&a| cache.peek(identity, a).is_some())
            .count();
        let rejected_before = cache.report().reservation_rejected;
        let mut work = if generation_admitted {
            cache.generate(vertex_budget, GENERATION_MICROBATCH, wall_budget)?
        } else {
            TerrainWorkReport {
                pending_patches: cache.pending(),
                ..TerrainWorkReport::default()
            }
        };
        mark_generation_reservation_rejection(
            &mut self.report,
            rejected_before,
            cache.report().reservation_rejected,
        );
        #[cfg(feature = "surface-profile")]
        {
            self.profile.cache_generation = stage_started.elapsed();
        }
        let ready_after = publishing_required
            .iter()
            .filter(|&&a| cache.peek(identity, a).is_some())
            .count();
        work.replacement_useful_completed = ready_after.saturating_sub(ready_before);
        if self.report.refinement_local {
            work.local_useful_completed = work.replacement_useful_completed;
        }
        self.result_publication = Duration::ZERO;
        if let Some(completion) = self
            .construction
            .and_then(|id| cache.take_cover_completion(id))
        {
            let start = Instant::now();
            #[cfg(feature = "surface-profile")]
            {
                work.profile.include_worker(completion.metrics);
                work.profile.cover_coordinator_wait += completion.received.elapsed();
            }
            if self.construction == Some(completion.id) && completion.identity == *identity {
                let completed_parent = self
                    .construction_parent
                    .map(|p| (p, self.construction_refining));
                self.construction = None;
                self.construction_parent = None;
                if self.construction_obsolete {
                    self.construction_obsolete = false;
                    self.unpublished_parent = None;
                    self.restore_endpoint()?;
                } else {
                    match completion.result {
                        Ok(workers::Output::Cover(output)) => {
                            self.unpublished_parent = None;
                            self.transition_deferred = false;
                            self.transition_budget_step = 0;
                            self.worker_stitch_cpu = output.stitch_cpu;
                            self.worker_morph_cpu = output.morph_cpu;
                            if self.morph.is_some() {
                                anyhow::ensure!(
                                    self.queued.is_none(),
                                    "duplicate queued terrain endpoint"
                                );
                                self.queued_duration = self.construction_duration;
                                self.queued_parent = completed_parent;
                                self.queued = Some(output);
                            } else {
                                let duration = output.transition.as_ref().map_or(
                                    Ok(self.construction_duration),
                                    |mesh| {
                                        Self::display_duration(
                                            mesh,
                                            input,
                                            self.construction_duration,
                                        )
                                    },
                                )?;
                                self.publish_output(output, duration);
                            }
                        }
                        Err(error)
                            if error.downcast_ref::<RenderPreparationError>().is_some_and(
                                |e| matches!(e, RenderPreparationError::InvalidBudget),
                            ) =>
                        {
                            self.transition_budget_step =
                                self.transition_budget_step.saturating_add(1);
                            self.transition_deferred = self.transition_budget_step > 2;
                            self.report.budget_constrained = true;
                            self.report.quality_pending = true;
                            self.report.settled = false;
                        }
                        Err(error) => return Err(error),
                        Ok(workers::Output::Cancelled) => {
                            self.unpublished_parent = None;
                            if self.stitched.is_some() {
                                self.restore_endpoint()?;
                            }
                        }
                        _ => anyhow::bail!("unexpected terrain cover completion"),
                    }
                }
            }
            self.result_publication = start.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        {
            // Only the consumed result's publication, not the preceding raw
            // cache scheduling/publication parent interval.
            self.profile.cover_publication = self.result_publication;
        }
        // Publication restores coordinator ownership before any new allocation.
        // A failed overlay retries only within the fixed aggregate cap.
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        let transition_budget = (TRANSITION_RESERVATION
            + usize::from(self.transition_budget_step) * 8 * 1024 * 1024)
            .min(MAX_TRANSITION_RESERVATION);
        #[cfg(test)]
        let transition_budget = self.transition_budget_override.unwrap_or(transition_budget);
        let transition_reserve = if self.morph_duration.is_zero()
            || self.queued.is_some()
            || self.construction.is_some()
        {
            0
        } else {
            transition_budget
        };
        let reserve =
            self.resident_bytes() + STAGING_ALLOWANCE + 8 * 1024 * 1024 + transition_reserve;
        cache.operational_reserve_bytes = transition_reserve
            + self.morph.as_ref().map_or(0, |m| {
                m.mesh.resident_bytes() + shared_surface_bytes(&m.destination)
            });
        let construction_admitted = cache.reserve_external(reserve);
        #[cfg(feature = "surface-profile")]
        {
            self.profile.pinning_reservation += stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let construction_started = Instant::now();
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
        let endpoint_cover = self
            .morph
            .as_ref()
            .map_or(&self.active, |m| &m.destination_cover);
        let endpoint_surface = self
            .morph
            .as_ref()
            .map(|m| &m.destination)
            .or(self.stitched.as_ref());
        let changed = endpoint_cover.len() != addresses.len()
            || endpoint_cover
                .iter()
                .zip(&addresses)
                .any(|(p, a)| p.address != *a);
        // Retry allowances are ceilings, not a requirement to reserve unused
        // overlay bytes. A complete target can otherwise remain frozen forever
        // when a 24 MiB retry plus its full construction workspace just exceeds
        // the cap, even though the exact overlay fits the remaining envelope.
        let cover_workspace = if cache.workers.is_some() {
            StitchedSurface::construction_bytes(addresses.len())
                + (endpoint_cover.len() + addresses.len())
                    * (size_of::<ActiveSurfacePatch>() + size_of::<Arc<GeneratedSurfacePatch>>())
                + (endpoint_cover.len() + 2 * addresses.len()) * size_of::<&GeneratedSurfacePatch>()
                + 256
        } else {
            StitchedSurface::construction_bytes(addresses.len())
                + addresses.len()
                    * (size_of::<ActiveSurfacePatch>() + size_of::<&GeneratedSurfacePatch>())
        };
        let reserve_without_overlay = reserve.saturating_sub(transition_reserve);
        let available_overlay = cache
            .cap_bytes
            .saturating_sub(cache.resident_bytes() + reserve_without_overlay + cover_workspace);
        let transition_budget = transition_budget.min(available_overlay);
        let envelope_available = transition_reserve == 0
            || transition_budget >= transition_reserve.min(TRANSITION_RESERVATION);
        let transition_reserve = if transition_reserve == 0 {
            0
        } else {
            transition_budget
        };
        let reserve = reserve_without_overlay + transition_reserve;
        let construction_admitted =
            envelope_available && (construction_admitted || cache.reserve_external(reserve));
        self.stitch_preparation = Duration::ZERO;
        self.morph_preparation = Duration::ZERO;
        if changed
            && construction_admitted
            && !self.transition_deferred
            && self.queued.is_none()
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
                    + endpoint_surface.map_or(0, shared_surface_bytes)
                    + (endpoint_cover.len() + active.len())
                        * (size_of::<ActiveSurfacePatch>()
                            + size_of::<Arc<GeneratedSurfacePatch>>())
                    + (endpoint_cover.len() + 2 * active.len()) * size_of::<&GeneratedSurfacePatch>()
                    + 256 // shared allocation headers and result envelope
                    + if self.morph_duration.is_zero() || endpoint_surface.is_none() {
                        0
                    } else {
                        transition_budget
                    };
                let source_bytes = endpoint_surface.map_or(0, shared_surface_bytes);
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
                    self.last_construction_budget = transition_budget;
                    let clone_raw = |patches: &[ActiveSurfacePatch]| -> Result<Vec<Arc<GeneratedSurfacePatch>>> {
                        patches.iter().map(|p| cache.entry_index(identity, p.address)
                            .map(|i| cache.entries[i].patch.clone())
                            .ok_or_else(|| anyhow::anyhow!("missing pinned worker input"))).collect()
                    };
                    let job = workers::CoverJob {
                        old: endpoint_surface.cloned(),
                        old_cover: endpoint_cover.clone(),
                        old_raw: clone_raw(endpoint_cover)?,
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
                    self.construction_from_morph = self.morph.is_some();
                    self.construction_parent = self.unpublished_parent.map(|(parent, _)| parent);
                    self.construction_obsolete = false;
                    self.construction_refining = self
                        .unpublished_parent
                        .is_some_and(|(_, refining)| refining);
                    cache.record_peak();
                } else {
                    self.report.budget_constrained = true;
                }
                self.stitch_preparation = start.elapsed();
            } else {
                self.last_construction_budget = transition_budget;
                let surface = Arc::new(StitchedSurface::build(&active, &geometry, lod.topology())?);
                self.stitch_preparation = start.elapsed();
                if !self.morph_duration.is_zero()
                    && let Some(old) = endpoint_surface
                {
                    let start = Instant::now();
                    let prepared = SurfaceTransition::build(
                        endpoint_cover,
                        old,
                        &active,
                        &surface,
                        lod.topology(),
                        transition_budget,
                    );
                    self.morph_preparation = start.elapsed();
                    match prepared {
                        Ok(mesh) => {
                            let completed_parent = self.unpublished_parent;
                            self.unpublished_parent = None;
                            self.transition_budget_step = 0;
                            let output = workers::CoverOutput {
                                surface,
                                cover: active,
                                transition: Some(mesh),
                                stitch_cpu: self.stitch_preparation,
                                morph_cpu: self.morph_preparation,
                            };
                            if self.morph.is_some() {
                                self.queued_duration = self.morph_duration;
                                self.queued_parent = completed_parent;
                                self.queued = Some(output);
                            } else {
                                let duration = output
                                    .transition
                                    .as_ref()
                                    .map_or(Ok(self.morph_duration), |mesh| {
                                        Self::display_duration(mesh, input, self.morph_duration)
                                    })?;
                                self.publish_output(output, duration);
                            }
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
                    let completed_parent = self.unpublished_parent;
                    self.unpublished_parent = None;
                    if self.morph.is_some() {
                        self.queued_duration = self.morph_duration;
                        self.queued_parent = completed_parent;
                        self.queued = Some(workers::CoverOutput {
                            surface,
                            cover: active,
                            transition: None,
                            stitch_cpu: self.stitch_preparation,
                            morph_cpu: Duration::ZERO,
                        });
                    } else {
                        self.stitched = Some(surface);
                        self.active = active;
                    }
                }
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.construction_submission = construction_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        for p in &self.active {
            cache.pin(identity, p.address);
        }
        if let Some(morph) = &self.morph {
            for p in &morph.destination_cover {
                cache.pin(identity, p.address);
            }
        }
        if let Some(output) = &self.queued {
            for p in &output.cover {
                cache.pin(identity, p.address);
            }
        }
        if self.construction.is_some() {
            for address in self
                .lod
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("missing selector"))?
                .covering_leaves()
            {
                cache.pin(identity, address);
            }
        }
        if self.construction.is_some() || self.morph.is_some() || changed {
            self.report.quality_pending = true;
            self.report.settled = false;
        }
        #[cfg(feature = "surface-profile")]
        {
            self.profile.pinning_reservation += stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        self.prepare_visible(input)?;
        #[cfg(feature = "surface-profile")]
        {
            self.profile.visibility = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        self.update_convergence(cache, identity, input, settings, representation_clearance_m)?;
        #[cfg(feature = "surface-profile")]
        {
            self.profile.diagnostics = stage_started.elapsed();
        }
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
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        anyhow::ensure!(
            cache.reserve_external(external),
            "terrain cover exceeded aggregate CPU budget"
        );
        self.peak_cpu_bytes = cache.report().peak_aggregate_bytes;
        anyhow::ensure!(
            self.peak_cpu_bytes <= TERRAIN_CPU_CAP_BYTES,
            "terrain aggregate peak exceeded the configured CPU cap"
        );
        #[cfg(feature = "surface-profile")]
        {
            self.profile.final_reservation = stage_started.elapsed();
            self.profile.total = profile_started.elapsed();
        }
        Ok(work)
    }
    fn update_convergence(
        &mut self,
        cache: &TerrainPatchCache,
        identity: &TerrainGeometryIdentity,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        representation_clearance_m: Option<f64>,
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
            if diagnostic.desired_local_lod.is_some() {
                continue;
            }
            let slot = &mut self.local_metadata[usize::from(level)];
            if slot.is_none_or(|(old, _)| old != address) {
                *slot = Some((address, PatchMetadata::build(address, topology)?));
            }
            let metadata = slot
                .ok_or_else(|| anyhow::anyhow!("missing local certificate metadata"))?
                .1;
            let (mut extent, mut error) = if let Some(g) = geometry {
                (g.extent(), g.error())
            } else {
                let slot = &mut self.local_certificates[usize::from(level)];
                if slot.is_none_or(|(a, _, _)| a != address) {
                    let (extent, error) = self.certificates.get(generator, address, metadata)?;
                    *slot = Some((address, extent, error));
                }
                let (_, extent, error) =
                    slot.ok_or_else(|| anyhow::anyhow!("missing local certificate"))?;
                (extent, error)
            };
            error.boundary_constraint_m =
                (boundary_profile_bound(generator, address, metadata, &self.boundary_bounds)?
                    + error.numeric_m)
                    .next_up();
            extent.min_height_m = (extent.min_height_m - error.boundary_constraint_m).next_down();
            extent.max_height_m = (extent.max_height_m + error.boundary_constraint_m).next_up();
            let (center, radius) = metadata.ball(identity.radius_m, extent)?;
            let center = source
                .view_displacement(mundaris_math::FramePosition::new(
                    input.body_fixed_frame,
                    mundaris_math::LocalPosition::try_metres(center)?,
                ))?
                .metres();
            let pixels = metadata.projected_total_error(error, center, radius, input.projection)?;
            let demand = generator.representation_demand_pixels(
                SurfaceRefinementInput {
                    address,
                    metadata,
                    reference_radius_m: identity.radius_m,
                    center_view: center,
                    ball_radius_m: radius,
                    projection: input.projection,
                    certified_error_pixels: pixels,
                },
                representation_clearance_m,
                settings.split_pixels(),
            );
            if demand <= settings.split_pixels() || level == 30 {
                diagnostic.desired_local_lod = Some(level);
                diagnostic.target_certifiable = pixels <= settings.split_pixels();
                diagnostic.certificate_limited_target_lod =
                    diagnostic.target_certifiable.then_some(level);
                // Compositional demand is a resolution guide. Keep proof of
                // complete reconstruction separate from useful mesh readiness.
                diagnostic.useful_target_lod = (representation_clearance_m.is_some()
                    || diagnostic.target_certifiable)
                    .then_some(level);
                let (width, spacing) = physical_resolution(address, identity.radius_m)?;
                diagnostic.desired_patch_width_m = width;
                diagnostic.desired_sample_spacing_m = spacing;
                diagnostic.terrain_footprint_m =
                    identity.radius_m * 2.0 / (16.0 * (1u64 << level) as f64);
                diagnostic.represented_height_bound_m = generator.represented_height_bound_m(
                    DirectionalCap::new(Direction3::try_new(metadata.cap().0)?, metadata.cap().1)?,
                    TerrainFootprint::new(diagnostic.terrain_footprint_m)?,
                )?;
                diagnostic.projected_depth_floor_m =
                    (-center.z - radius).max(input.projection.near_m());
                diagnostic.pixel_footprint_m =
                    diagnostic.projected_depth_floor_m / input.projection.focal_pixels();
                diagnostic.desired_error = error;
                diagnostic.desired_total_pixels = pixels;
                let terms = [
                    error.sphere_m,
                    error.filtered_interpolation_m,
                    error.unresolved_m,
                    error.boundary_constraint_m,
                    error.morph_remaining_m,
                    error.numeric_m,
                ];
                let names = [
                    "sphere",
                    "interpolation",
                    "unresolved terrain",
                    "boundary",
                    "morph",
                    "numerical",
                ];
                let mut dominant = 0;
                for (i, &term) in terms.iter().enumerate() {
                    if term > terms[dominant] {
                        dominant = i;
                    }
                    diagnostic.desired_error_pixels[i] = metadata.projected_total_error(
                        SurfaceErrorContributions {
                            sphere_m: term,
                            ..Default::default()
                        },
                        center,
                        radius,
                        input.projection,
                    )?;
                }
                diagnostic.dominant_term = names[dominant];
            }
        }
        if let Some(level) = diagnostic.rendered_local_lod
            && let Some(geometry) = cache.peek(identity, address_at(level)?)
        {
            diagnostic.rendered_sample_spacing_m =
                physical_resolution(address_at(level)?, identity.radius_m)?.1;
            diagnostic.local_error = geometry.error();
            let address = address_at(level)?;
            let metadata = PatchMetadata::build(address, topology)?;
            diagnostic.local_error.boundary_constraint_m =
                boundary_profile_bound(generator, address, metadata, &self.boundary_bounds)?;
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
    fn stale_merge_batch_checks_non_final_parent() {
        stale_batch_checks_non_final_parent(false);
    }

    #[test]
    fn stale_refinement_batch_checks_non_final_parent() {
        stale_batch_checks_non_final_parent(true);
    }

    fn stale_batch_checks_non_final_parent(refining: bool) {
        use mundaris_world::terrain::{SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator};
        let radius = 109_081.776_8;
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(91_515),
            TerrainSeed(71),
            SurfaceAlgorithm::RockyV5,
        );
        let mut world =
            CelestialSystem::new(NonZeroU64::new(91_515).unwrap(), SimulationInstant::ZERO);
        let body = world
            .insert_body(
                "merge batch fixture",
                BodyProperties::new(1.0, radius).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
        world
            .edit_surface_definition(body, Some(definition.clone()))
            .unwrap();
        let identity = TerrainGeometryIdentity::from_body(body, world.body(body).unwrap()).unwrap();
        let oracle = SurfaceGenerator::new(&definition, radius).unwrap();
        let surface_radius = oracle
            .evaluate_point(SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap()))
            .unwrap()
            .radius_m();
        let tree = FrameTree::new(NonZeroU64::new(91_515).unwrap());
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    tree.root(),
                    LocalPosition::try_metres(DVec3::Z * (surface_radius + 1.0)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: tree.root(),
            reference_radius_m: radius,
            projection: CelestialProjection::try_new(128, 96, 60.0_f64.to_radians(), 0.1).unwrap(),
        };
        let settings = LodSettings::default().with_limits(128, 128, 0).unwrap();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 128).unwrap();
        let mut cover = AdaptiveTerrainCover::default();
        cover.set_morph_duration(Duration::ZERO).unwrap();
        for _ in 0..40 {
            cover
                .update(&mut cache, &identity, &input, &settings, 4096, None)
                .unwrap();
            if cover.ready() {
                break;
            }
        }
        assert!(cover.ready());
        let front = CubePatchAddress::try_new(CubeFace::PositiveZ, 8, 127, 127).unwrap();
        let back = CubePatchAddress::try_new(CubeFace::NegativeZ, 8, 127, 127).unwrap();
        let generator = cover.generator.as_ref().unwrap();
        let metadata = PatchMetadata::build(back, cover.lod.as_ref().unwrap().topology()).unwrap();
        let (extent, error) = generator.surface_certificate(back, metadata).unwrap();
        let (center, ball) = metadata.ball(radius, extent).unwrap();
        let center = input
            .view
            .prepare_source(tree.root())
            .unwrap()
            .view_displacement(FramePosition::new(
                tree.root(),
                LocalPosition::try_metres(center).unwrap(),
            ))
            .unwrap()
            .metres();
        assert!(
            generator.representation_demand_pixels(
                SurfaceRefinementInput {
                    address: back,
                    metadata,
                    reference_radius_m: radius,
                    center_view: center,
                    ball_radius_m: ball,
                    projection: input.projection,
                    certified_error_pixels: metadata
                        .projected_total_error(error, center, ball, input.projection)
                        .unwrap(),
                },
                Some(1.0),
                settings.split_pixels()
            ) < settings.merge_pixels()
        );
        // A held construction isolates cancellation before worker completion.
        cover.construction = Some(u64::MAX);
        let (obsolete, relevant) = if refining {
            (back, front)
        } else {
            (front, back)
        };
        cover.construction_parent = Some(relevant);
        cover.construction_refining = refining;
        cover.unpublished_changes[0] = Some((obsolete, refining));
        cover.unpublished_changes[1] = Some((relevant, refining));
        cover
            .update(&mut cache, &identity, &input, &settings, 0, None)
            .unwrap();
        assert!(
            cover.construction_obsolete,
            "the earlier changed parent invalidates the whole batch"
        );
    }

    #[test]
    fn raw_generation_reservation_rejection_keeps_quality_pending_honest() {
        let mut report = LodReport {
            settled: true,
            ..LodReport::default()
        };
        mark_generation_reservation_rejection(&mut report, 7, 7);
        assert!(!report.budget_constrained);
        assert!(!report.quality_pending);
        assert!(report.settled);

        mark_generation_reservation_rejection(&mut report, 7, 8);
        assert!(report.budget_constrained);
        assert!(report.quality_pending);
        assert!(!report.settled);
    }

    #[test]
    fn publication_view_selects_duration_but_never_instantly_switches_normals() {
        let topology = SurfaceTopology::new();
        let addresses: Vec<_> = mundaris_math::surface::CubeFace::ALL
            .into_iter()
            .map(mundaris_math::surface::CubePatchAddress::root)
            .collect();
        let active = active_surface_cover(&addresses, &topology).unwrap();
        let make_surface = |normal_change: f64| {
            let patches: Vec<_> = addresses
                .iter()
                .map(|&address| {
                    GeneratedSurfacePatch::new(
                        address,
                        1000.0,
                        1.0,
                        (0..GRID_SAMPLES)
                            .map(|index| {
                                let radial = address
                                    .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                                    .unwrap()
                                    .unit();
                                SurfaceGeometrySample {
                                    position_body_m: radial * 1000.0,
                                    normal_body: (radial
                                        + normal_change * (DVec3::X - radial * radial.x))
                                        .normalize(),
                                }
                            })
                            .collect(),
                        SurfaceExtent {
                            min_height_m: -1.0,
                            max_height_m: 1.0,
                            guaranteed_opaque_radius_m: 0.0,
                        },
                        SurfaceErrorContributions::default(),
                    )
                    .unwrap()
                })
                .collect();
            StitchedSurface::build(&active, &patches.iter().collect::<Vec<_>>(), &topology).unwrap()
        };
        let old = make_surface(0.0);
        let new = Arc::new(make_surface(0.1));
        let mesh = SurfaceTransition::build(
            &active,
            &old,
            &active,
            &new,
            &topology,
            TRANSITION_RESERVATION,
        )
        .unwrap();
        assert!(!mesh.triangles().is_empty());
        assert_eq!(mesh.max_displacement_m(), 0.0);
        let tree = FrameTree::new(NonZeroU64::new(511).unwrap());
        let evaluation = tree.evaluate();
        let make_view = |distance| {
            PreparedView::new(
                &evaluation,
                FramePose::new(
                    FramePosition::new(
                        tree.root(),
                        LocalPosition::try_metres(DVec3::Z * distance).unwrap(),
                    ),
                    UnitRotation::identity(),
                ),
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap()
        };
        let far = make_view(1e6);
        let close = make_view(1000.1);
        let far_input = SurfaceViewInput {
            view: &far,
            body_fixed_frame: tree.root(),
            reference_radius_m: 1000.0,
            projection: CelestialProjection::try_new(320, 240, 1.0, 0.1).unwrap(),
        };
        let close_input = SurfaceViewInput {
            view: &close,
            ..far_input
        };
        let ordinary = Duration::from_millis(150);
        let far_duration =
            AdaptiveTerrainCover::display_duration(&mesh, &far_input, ordinary).unwrap();
        assert!(far_duration >= Duration::from_millis(64) && far_duration < ordinary);
        let publication_duration =
            AdaptiveTerrainCover::display_duration(&mesh, &close_input, ordinary).unwrap();
        assert_eq!(
            publication_duration, ordinary,
            "new publication view must invalidate a prior short estimate"
        );
        let mut coordinator = AdaptiveTerrainCover::default();
        coordinator.publish_output(
            workers::CoverOutput {
                surface: new,
                cover: active,
                transition: Some(mesh),
                stitch_cpu: Duration::ZERO,
                morph_cpu: Duration::ZERO,
            },
            publication_duration,
        );
        coordinator.set_morph_duration(Duration::ZERO).unwrap();
        assert_eq!(coordinator.active_morph_duration(), Some(ordinary));
        assert_eq!(coordinator.transition().unwrap().1, 0.0);
    }

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
        // A retry ceiling must not freeze a ready target when a smaller ordinary
        // envelope fits. Charge source, staging and full synchronous workspace
        // before assigning overlay headroom; below the initial floor, do not build.
        cover.transition_budget_override = None;
        cover.transition_budget_step = 1;
        cover.transition_deferred = false;
        let workspace = StitchedSurface::construction_bytes(target.len())
            + target.len()
                * (size_of::<ActiveSurfacePatch>() + size_of::<&GeneratedSurfacePatch>());
        let staging_allowance = 72 * 1024 * 1024 + 32 * 1024;
        let non_overlay = cover.resident_bytes() + staging_allowance + 8 * 1024 * 1024;
        let constrained_cap = cache.resident_bytes() + non_overlay + workspace;
        cache.cap_bytes = constrained_cap + TRANSITION_RESERVATION - 1;
        let previous_budget = cover.last_construction_budget;
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &input,
                &refined,
                64,
                None,
                Duration::ZERO,
            )
            .unwrap();
        assert!(cover.transition().is_none());
        assert_eq!(cover.last_construction_budget, previous_budget);
        assert_eq!(
            cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
            source
        );
        cache.cap_bytes = constrained_cap + 20 * 1024 * 1024;
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &input,
                &refined,
                64,
                None,
                Duration::ZERO,
            )
            .unwrap();
        assert!(cover.transition().is_some());
        assert!(!cover.transition_deferred);
        assert!(cover.last_construction_budget >= TRANSITION_RESERVATION);
        assert!(cover.last_construction_budget < TRANSITION_RESERVATION + 8 * 1024 * 1024);
        assert!(cache.resident_bytes() + cache.external_bytes <= cache.cap_bytes);
        cache.cap_bytes = TERRAIN_CPU_CAP_BYTES;
        let captured_duration = cover.active_morph_duration().unwrap();
        assert!(captured_duration >= Duration::from_millis(64));
        assert!(captured_duration <= Duration::from_millis(150));
        cover.set_morph_duration(Duration::ZERO).unwrap();
        assert_eq!(cover.active_morph_duration(), Some(captured_duration));
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &input,
                &refined,
                64,
                None,
                captured_duration,
            )
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
            // Keep the transaction in flight through the center-view and source
            // accounting assertions regardless of worker scheduling speed.
            cache.workers.as_ref().unwrap().set_cover_jobs_paused(true);
            let refined = LodSettings::default().with_limits(128, 128, 1).unwrap();
            while !cover.construction_pending() {
                cover
                    .update(&mut cache, &identity, &input, &refined, 1, None)
                    .unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            // A body-center observer has no camera-radial prefetch direction,
            // but must not invalidate an in-flight source/target transaction.
            let center = PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(fixed, LocalPosition::origin()),
                    UnitRotation::identity(),
                ),
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let center_input = SurfaceViewInput {
                view: &center,
                ..input
            };
            let source: Vec<_> = cover.active.iter().map(|p| p.address).collect();
            let job = cover.construction;
            cover
                .update(&mut cache, &identity, &center_input, &refined, 0, None)
                .unwrap();
            assert_eq!(cover.construction, job);
            assert_eq!(
                cover.active.iter().map(|p| p.address).collect::<Vec<_>>(),
                source
            );
            assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
            let job = cover.construction.unwrap();
            assert!(cache.report().worker_reserved_bytes >= source_bytes);
            let charged_cover_bytes = cover.resident_bytes();
            cover.construction = None;
            assert_eq!(cover.resident_bytes(), charged_cover_bytes + source_bytes);
            cover.construction = Some(job);
            cache.cancel_body_work(body);
            cache.workers.as_ref().unwrap().set_cover_jobs_paused(false);
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
            let away = PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(
                        fixed,
                        LocalPosition::try_metres(DVec3::Z * (identity.radius_m + 1_000_000.0))
                            .unwrap(),
                    ),
                    UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_y(
                        std::f64::consts::PI,
                    ))
                    .unwrap(),
                ),
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let away_input = SurfaceViewInput {
                view: &away,
                ..input
            };
            for unpublished in [false, true] {
                while !cover.construction_pending() {
                    cover
                        .update(&mut cache, &identity, &input, &refined, 1, None)
                        .unwrap();
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                let obsolete_job = cover.construction.unwrap();
                let source: Vec<_> = cover.active.iter().map(|p| p.address).collect();
                if unpublished {
                    while !cache.completed_covers.iter().any(|c| c.id == obsolete_job) {
                        cache.generate(0, GENERATION_MICROBATCH, None).unwrap();
                        assert!(Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                let cancellations = cache.report().cancellations;
                while cover.construction == Some(obsolete_job) {
                    cover
                        .update(&mut cache, &identity, &away_input, &refined, 0, None)
                        .unwrap();
                    assert!(cover.transition().is_none());
                    assert_eq!(
                        cover.active.iter().map(|p| p.address).collect::<Vec<_>>(),
                        source
                    );
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert!(cache.report().cancellations > cancellations);
                assert_eq!(
                    cover
                        .lod
                        .as_ref()
                        .unwrap()
                        .covering_leaves()
                        .collect::<Vec<_>>(),
                    source
                );
                assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
            }
            // Active morphs finish, but a newly requested unpublished merge
            // must reverse when its parent is visibly above the merge threshold.
            while cover.transition().is_none() {
                cover
                    .update(&mut cache, &identity, &input, &refined, 1, None)
                    .unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            cover
                .update_with_elapsed(
                    &mut cache,
                    &identity,
                    &input,
                    &refined,
                    0,
                    None,
                    Duration::from_secs(1),
                )
                .unwrap();
            assert!(cover.active.len() > 6);
            let source: Vec<_> = cover.active.iter().map(|p| p.address).collect();
            while !cover.construction_pending() {
                cover
                    .update(&mut cache, &identity, &away_input, &refined, 0, None)
                    .unwrap();
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(!cover.construction_refining);
            let merge_job = cover.construction.unwrap();
            while cover.construction == Some(merge_job) {
                cover
                    .update(&mut cache, &identity, &input, &refined, 0, None)
                    .unwrap();
                assert!(cover.transition().is_none());
                assert_eq!(
                    cover.active.iter().map(|p| p.address).collect::<Vec<_>>(),
                    source
                );
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(
                cover
                    .lod
                    .as_ref()
                    .unwrap()
                    .covering_leaves()
                    .collect::<Vec<_>>(),
                source
            );
            cover.abandon_construction(&mut cache);
            drop(cover);
            cache.reserve_external(0);
            cache.cancel_body_work(body);
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
