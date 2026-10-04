//! CPU f64 evaluation, source subtraction, clipped precision proof and byte layouts.
use super::GeneratedSurfacePatch;
use super::{ActiveSurfacePatch, GRID_SAMPLES, SurfaceTopology};
use crate::{CelestialProjection, PreparedView, RenderPreparationError};
use glam::DVec3;
use mundaris_math::{FrameId, FramePosition, LocalPosition};
use std::ops::Range;
const STAGING_CAP: usize = 64 * 1024 * 1024;
const BOUNDARY_CAP: usize = 8 * 1024 * 1024;
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfaceStyle {
    pub borders: bool,
    pub lod_colors: bool,
    pub face_colors: bool,
    pub underside: bool,
    /// Derived sample elevation diagnostic, never a shader terrain query.
    pub elevation_colors: bool,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfacePreparationReport {
    pub patches: usize,
    pub samples: usize,
    pub triangles: usize,
    pub fallback_triangles: usize,
    /// Temporary common-refinement triangles, not precision fallbacks.
    pub morph_triangles: usize,
    pub draws: usize,
    pub uploaded_bytes: usize,
    pub allocated_staging_bytes: usize,
    pub boundary_bytes: usize,
    pub proof_cache_hits: usize,
    pub proof_cache_misses: usize,
    pub max_component_error_m: f64,
    pub max_projected_error_pixels: f64,
    pub max_gpu_projection_error_pixels: f64,
    #[cfg(feature = "surface-profile")]
    pub profile: SurfacePreparationProfile,
}
/// Opt-in CPU stages. Regular samples have patch-batched conversion,
/// classification and narrowing timers, avoiding clocks inside vertex loops.
/// Parent timings include these children; do not add them twice.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfacePreparationProfile {
    /// Calling-thread CPU for the whole regular append, not only sample work.
    /// Last append batch only; the app clock covers the aggregate preparation.
    /// OS accounting resolution can be coarse.
    pub regular_thread_cpu_last_batch: Option<std::time::Duration>,
    /// Whole transition append CPU: setup, interpolation, conversion,
    /// classification, clipping, packing and growth, for the last batch.
    pub transition_thread_cpu_last_batch: Option<std::time::Duration>,
    /// Parent covering boundary setup, shared lookup, and associated staging.
    pub boundary: std::time::Duration,
    /// Canonical shared-boundary key construction, sorting and compaction.
    pub boundary_keys: std::time::Duration,
    /// Boundary lookup/narrowing work within sample processing.
    pub boundary_narrow: Option<std::time::Duration>,
    pub setup: std::time::Duration,
    /// Parent duration for regular coordinate conversion and classification.
    pub samples: std::time::Duration,
    pub coordinate_conversion: Option<std::time::Duration>,
    /// Generated terrain classification. Smooth-sphere diagnostic octahedral
    /// coordinates are computed with canonical directions in conversion instead.
    pub classification: Option<std::time::Duration>,
    /// Lookup includes cache key matching and equality validation.
    pub proof_lookup: std::time::Duration,
    /// Cache miss proof work, including cache insertion.
    pub proof_calculation: std::time::Duration,
    pub proof: std::time::Duration,
    /// Wall time inside owned-vector growth calls (also included in parent stages).
    /// No allocator/driver allocation inference is made for zero-growth frames.
    pub allocation: std::time::Duration,
    pub packing: std::time::Duration,
    pub grouping: std::time::Duration,
    pub total: std::time::Duration,
    pub whole_patch_proofs: usize,
    pub clipped_patch_proofs: usize,
    /// Number of owned vectors whose capacity grew, not allocator calls.
    pub capacity_growths: usize,
    /// Parent CPU transform/classify duration for transition vertices. Interpolation
    /// is included; finer children are intentionally not isolated per vertex.
    pub transition_samples: std::time::Duration,
    pub transition_clipping: std::time::Duration,
    pub transition_packing: std::time::Duration,
    pub transition_input_triangles: usize,
    pub transition_emitted_triangles: usize,
    pub transition_bytes_staged: usize,
    pub transition_capacity_growths: usize,
    pub fallback_growths: usize,
    pub fallback_growth_bytes: usize,
    /// Sum of old capacities at growth points; an upper copy-work proxy,
    /// not allocator instrumentation (an allocator may extend in place).
    pub fallback_growth_copy_upper_bytes: usize,
}
#[derive(Default)]
pub(crate) struct SurfaceStaging {
    pub lighting: super::TerrainLighting,
    pub samples: Vec<u8>,
    pub instances: Vec<u8>,
    pub fallback: Vec<u8>,
    proof_cache: Vec<PrecisionProofEntry>,
    proof_cursor: usize,
    pub buckets: [Range<u32>; 16],
    boundary_keys: Vec<u128>,
    boundary_samples: Vec<([f32; 3], [f32; 3])>,
    boundary_ready: Vec<bool>,
    records: Vec<(u8, [u8; 64])>,
    pub report: SurfacePreparationReport,
    pub underside: bool,
}

#[derive(Clone, Copy)]
struct PrecisionProofResult {
    fallback: bool,
    whole: bool,
    component_m: f64,
    pixels: f64,
    gpu_pixels: f64,
}
struct PrecisionProofEntry {
    source: FrameId,
    address: mundaris_math::surface::CubePatchAddress,
    mask: u8,
    projection: ([u32; 2], u64, u64),
    positions: [DVec3; GRID_SAMPLES],
    gpu_positions: [[f32; 3]; GRID_SAMPLES],
    result: PrecisionProofResult,
}
fn same_positions(a: &[DVec3; GRID_SAMPLES], b: &[DVec3; GRID_SAMPLES]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.to_array().map(f64::to_bits) == b.to_array().map(f64::to_bits))
}
fn same_gpu_positions(a: &[[f32; 3]; GRID_SAMPLES], b: &[[f32; 3]; GRID_SAMPLES]) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.map(f32::to_bits) == b.map(f32::to_bits))
}
#[derive(Clone, Copy)]
struct ClipVertex {
    position: DVec3,
    weights: DVec3,
}
struct ClipPolygon {
    vertices: [ClipVertex; 8],
    len: usize,
}
impl std::ops::Deref for ClipPolygon {
    type Target = [ClipVertex];
    fn deref(&self) -> &Self::Target {
        &self.vertices[..self.len]
    }
}
impl<'a> IntoIterator for &'a ClipPolygon {
    type Item = &'a ClipVertex;
    type IntoIter = std::slice::Iter<'a, ClipVertex>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
fn clip_triangle(points: [DVec3; 3], projection: CelestialProjection) -> ClipPolygon {
    let empty = ClipVertex {
        position: DVec3::ZERO,
        weights: DVec3::ZERO,
    };
    let mut polygon = ClipPolygon {
        vertices: [empty; 8],
        len: 3,
    };
    for (i, (position, weights)) in points
        .into_iter()
        .zip([DVec3::X, DVec3::Y, DVec3::Z])
        .enumerate()
    {
        polygon.vertices[i] = ClipVertex { position, weights };
    }
    for (normal, offset) in projection.frustum_planes() {
        let mut next = ClipPolygon {
            vertices: [empty; 8],
            len: 0,
        };
        if polygon.is_empty() {
            return polygon;
        }
        let mut previous = *polygon.last().expect("nonempty polygon");
        let mut previous_d = normal.dot(previous.position) + offset;
        for &current in &polygon {
            let d = normal.dot(current.position) + offset;
            if (d > 0.0 && previous_d < 0.0) || (d < 0.0 && previous_d > 0.0) {
                let t = previous_d / (previous_d - d);
                next.vertices[next.len] = ClipVertex {
                    position: previous.position.lerp(current.position, t),
                    weights: previous.weights.lerp(current.weights, t),
                };
                next.len += 1;
            }
            if d >= 0.0 {
                next.vertices[next.len] = current;
                next.len += 1;
            }
            previous = current;
            previous_d = d;
        }
        polygon = next;
    }
    polygon
}
fn screen(projection: CelestialProjection, p: DVec3) -> [f64; 2] {
    let f = projection.focal_pixels();
    [f * p.x / -p.z, f * p.y / -p.z]
}
fn gpu_screen(projection: CelestialProjection, p: [f32; 3]) -> [f64; 2] {
    let c = projection.gpu_clip(p);
    let [w, h] = projection.viewport();
    [
        f64::from(c[0]) / f64::from(c[3]) * w as f64 * 0.5,
        f64::from(c[1]) / f64::from(c[3]) * h as f64 * 0.5,
    ]
}
fn physical_budget(p: DVec3) -> f64 {
    let d = p.length();
    if d <= 100.0 {
        1e-5
    } else if d <= 1000.0 {
        1e-4
    } else if d <= 10000.0 {
        1e-3
    } else {
        f64::INFINITY
    }
}
pub(super) fn encode_octahedral(direction: DVec3) -> [f32; 2] {
    // Octahedral encoding is invariant to positive scale; the radial direction is
    // already available, so avoid another per-sample normalization.
    let denominator = direction.abs().element_sum().max(1e-30);
    let mut p = direction / denominator;
    let sign_not_zero = |x: f64| if x >= 0.0 { 1.0 } else { -1.0 };
    if p.z < 0.0 {
        p = DVec3::new(
            (1.0 - p.y.abs()) * sign_not_zero(p.x),
            (1.0 - p.x.abs()) * sign_not_zero(p.y),
            p.z,
        );
    }
    [p.x as f32, p.y as f32]
}
impl SurfaceStaging {
    fn boundary_capacity_bytes(&self) -> usize {
        self.boundary_keys.capacity() * std::mem::size_of::<u128>()
            + self.boundary_samples.capacity() * std::mem::size_of::<([f32; 3], [f32; 3])>()
            + self.boundary_ready.capacity() * std::mem::size_of::<bool>()
    }
    fn drop_proofs_for_boundary_growth(&mut self, predicted_boundary_bytes: usize) {
        if predicted_boundary_bytes.saturating_add(
            self.proof_cache.capacity() * std::mem::size_of::<PrecisionProofEntry>(),
        ) > BOUNDARY_CAP
        {
            self.proof_cache = Vec::new();
            self.proof_cursor = 0;
        }
    }
    fn prepare_proof_cache(&mut self, patches: usize) {
        let boundary = self.boundary_capacity_bytes();
        self.drop_proofs_for_boundary_growth(boundary);
        let limit = (BOUNDARY_CAP.saturating_sub(boundary)
            / std::mem::size_of::<PrecisionProofEntry>())
        .min(512);
        let desired = patches
            .checked_next_power_of_two()
            .unwrap_or(512)
            .min(limit);
        if patches != 0 && desired > self.proof_cache.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.proof_cache
                .reserve_exact(desired - self.proof_cache.len());
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        // Allocator capacity, rather than requested capacity, is authoritative.
        // This optional disposable optimization can never reject valid geometry.
        self.drop_proofs_for_boundary_growth(boundary);
    }
    fn allocated_outgoing_bytes(&self) -> usize {
        self.samples.capacity()
            + self.instances.capacity()
            + self.fallback.capacity()
            + self.records.capacity() * std::mem::size_of::<(u8, [u8; 64])>()
    }
    fn reserve_fallback(&mut self, required: usize) -> Result<(), RenderPreparationError> {
        let other = self.allocated_outgoing_bytes() - self.fallback.capacity();
        let needed = self.fallback.len().saturating_add(required);
        if other.saturating_add(self.fallback.capacity().max(needed)) > STAGING_CAP {
            return Err(RenderPreparationError::InvalidBudget);
        }
        if needed > self.fallback.capacity() {
            // Exact per-triangle growth repeatedly copies the already staged
            // overlay. Grow geometrically, clipped to the remaining aggregate
            // allowance; retained capacity is still fully accounted.
            let available = STAGING_CAP - other;
            let target = needed
                .max(self.fallback.capacity().saturating_mul(2))
                .min(available);
            #[cfg(feature = "surface-profile")]
            let old = self.fallback.capacity();
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.fallback.reserve_exact(target - self.fallback.len());
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.fallback_growths += 1;
                self.report.profile.fallback_growth_bytes += self.fallback.capacity() - old;
                self.report.profile.fallback_growth_copy_upper_bytes += old;
            }
        }
        if self.allocated_outgoing_bytes() > STAGING_CAP {
            return Err(RenderPreparationError::InvalidBudget);
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub fn append_transition(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        body: crate::CelestialRenderBody,
        mesh: &super::SurfaceTransition,
        fraction: f64,
        style: SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        #[cfg(feature = "surface-profile")]
        let thread_timer = super::CpuStageTimer::new();
        #[cfg(feature = "surface-profile")]
        let transition_growths_before = self.report.profile.fallback_growths;
        #[cfg(feature = "surface-profile")]
        let transition_bytes_before = self.fallback.len();
        if !fraction.is_finite()
            || !(0.0..=1.0).contains(&fraction)
            || !body.color.iter().all(|c| c.is_finite())
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let source = view.prepare_source(body.body_fixed_frame)?;
        self.underside |= style.underside;
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.transition_input_triangles += mesh.triangles().len();
        }
        for triangle in mesh.triangles() {
            #[cfg(feature = "surface-profile")]
            let sample_start = std::time::Instant::now();
            let mut positions = [DVec3::ZERO; 3];
            let mut normals = [DVec3::ZERO; 3];
            let mut elevations = [0.0; 3];
            let mut classifications = [[0.0; 4]; 3];
            let mut radial_directions = [DVec3::ZERO; 3];
            let mut colors = [body.color; 3];
            for (i, vertex) in triangle.iter().enumerate() {
                let sample = vertex.sample(fraction)?;
                positions[i] = source
                    .view_displacement(FramePosition::new(
                        body.body_fixed_frame,
                        LocalPosition::try_metres(sample.position_body_m)?,
                    ))?
                    .metres();
                normals[i] = sample.normal_body;
                let radial = sample.position_body_m.normalize_or_zero();
                radial_directions[i] = radial;
                let oct = encode_octahedral(radial);
                classifications[i] = [
                    (sample.position_body_m.length() - body.reference_radius_m) as f32,
                    radial
                        .cross(sample.normal_body)
                        .length()
                        .atan2(radial.dot(sample.normal_body)) as f32,
                    oct[0],
                    oct[1],
                ];
                elevations[i] =
                    vertex.old_elevation + (vertex.new_elevation - vertex.old_elevation) * fraction;
                if style.lod_colors {
                    let level = vertex.old_reference.address.level() as f64
                        + (vertex.new_reference.address.level() as f64
                            - vertex.old_reference.address.level() as f64)
                            * fraction;
                    colors[i] = super::lod_color(level.clamp(0.0, 255.0) as u8);
                }
            }
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.transition_samples += sample_start.elapsed();
            }
            #[cfg(feature = "surface-profile")]
            let clipping_start = std::time::Instant::now();
            let polygon = clip_triangle(positions, projection);
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.transition_clipping += clipping_start.elapsed();
            }
            if polygon.len() < 3 {
                continue;
            }
            #[cfg(feature = "surface-profile")]
            let packing_start = std::time::Instant::now();
            let required = (polygon.len() - 2) * 3 * 80;
            self.reserve_fallback(required)?;
            for i in 1..polygon.len() - 1 {
                for v in [polygon[0], polygon[i], polygon[i + 1]] {
                    let f = projection.focal_pixels();
                    let [w, h] = projection.viewport();
                    let clip = [
                        v.position.x * f * 2.0 / w as f64,
                        v.position.y * f * 2.0 / h as f64,
                        projection.near_m(),
                        -v.position.z,
                    ];
                    let packed = clip.map(|c| c as f32);
                    let expected = screen(projection, v.position);
                    let actual = [
                        f64::from(packed[0]) / f64::from(packed[3]) * w as f64 * 0.5,
                        f64::from(packed[1]) / f64::from(packed[3]) * h as f64 * 0.5,
                    ];
                    let error = (expected[0] - actual[0]).hypot(expected[1] - actual[1]);
                    if !packed.iter().all(|c| c.is_finite()) || !error.is_finite() || error > 0.05 {
                        return Err(RenderPreparationError::PrecisionBudgetExceeded {
                            error_m: error,
                            limit_m: 0.05,
                        });
                    }
                    self.report.max_projected_error_pixels =
                        self.report.max_projected_error_pixels.max(error);
                    let normal = normals[0] * v.weights.x
                        + normals[1] * v.weights.y
                        + normals[2] * v.weights.z;
                    let elevation = elevations[0] * v.weights.x
                        + elevations[1] * v.weights.y
                        + elevations[2] * v.weights.z;
                    let color = std::array::from_fn::<_, 4, _>(|i| {
                        colors[0][i] * v.weights.x as f32
                            + colors[1][i] * v.weights.y as f32
                            + colors[2][i] * v.weights.z as f32
                    });
                    pack_floats(
                        &mut self.fallback,
                        packed
                            .into_iter()
                            .chain([
                                normal.x as f32,
                                normal.y as f32,
                                normal.z as f32,
                                elevation as f32,
                            ])
                            .chain(color)
                            .chain([
                                0.5,
                                0.5,
                                (4 | if style.elevation_colors && !style.lod_colors {
                                    2
                                } else {
                                    0
                                } | if style.lod_colors { 8 } else { 0 })
                                    as f32,
                                0.0,
                            ])
                            .chain({
                                let radial = radial_directions[0] * v.weights.x
                                    + radial_directions[1] * v.weights.y
                                    + radial_directions[2] * v.weights.z;
                                let oct = if self.lighting.mode()
                                    == crate::planet_surface::TerrainRenderMode::Natural
                                {
                                    encode_octahedral(radial)
                                } else {
                                    [0.0; 2]
                                };
                                [
                                    classifications[0][0] * v.weights.x as f32
                                        + classifications[1][0] * v.weights.y as f32
                                        + classifications[2][0] * v.weights.z as f32,
                                    classifications[0][1] * v.weights.x as f32
                                        + classifications[1][1] * v.weights.y as f32
                                        + classifications[2][1] * v.weights.z as f32,
                                    oct[0],
                                    oct[1],
                                ]
                            }),
                    );
                }
                self.report.morph_triangles += 1;
                #[cfg(feature = "surface-profile")]
                {
                    self.report.profile.transition_emitted_triangles += 1;
                }
            }
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.transition_packing += packing_start.elapsed();
            }
        }
        self.report.patches += mesh.affected_new().len();
        self.report.draws = self.buckets.iter().filter(|r| !r.is_empty()).count()
            + usize::from(!self.fallback.is_empty());
        self.report.uploaded_bytes =
            self.samples.len() + self.instances.len() + self.fallback.len();
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.transition_bytes_staged +=
                self.fallback.len().saturating_sub(transition_bytes_before);
            self.report.profile.transition_capacity_growths +=
                self.report.profile.fallback_growths - transition_growths_before;
        }
        self.report.allocated_staging_bytes = self.samples.capacity()
            + self.instances.capacity()
            + self.fallback.capacity()
            + self.records.capacity() * std::mem::size_of::<(u8, [u8; 64])>();
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.transition_thread_cpu_last_batch = thread_timer.thread_elapsed();
        }
        if self.report.allocated_staging_bytes > STAGING_CAP
            || self.report.boundary_bytes > BOUNDARY_CAP
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        Ok(())
    }
    pub fn clear(&mut self) {
        self.samples.clear();
        self.instances.clear();
        self.fallback.clear();
        self.boundary_keys.clear();
        self.boundary_samples.clear();
        self.boundary_ready.clear();
        self.records.clear();
        self.buckets = std::array::from_fn(|_| 0..0);
        self.report = SurfacePreparationReport::default();
        self.underside = false;
        self.lighting = super::TerrainLighting::default();
    }
    pub fn append(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        body: crate::CelestialRenderBody,
        patches: &[ActiveSurfacePatch],
        topology: &SurfaceTopology,
        style: SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        self.append_inner(view, projection, body, patches, topology, style, None)
    }
    // Preserve the established append boundary; only borrowed geometry is added.
    #[allow(clippy::too_many_arguments)]
    pub fn append_generated(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        body: crate::CelestialRenderBody,
        patches: &[ActiveSurfacePatch],
        geometry: &[&GeneratedSurfacePatch],
        topology: &SurfaceTopology,
        style: SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        // Raw cache grids are not reconciled. Keep their diagnostic route uniform;
        // the production adaptive route requires a validated complete cover.
        if patches.iter().any(|p| p.stitch_mask != 0)
            || geometry.first().is_some_and(|first| {
                geometry.iter().any(|g| {
                    g.address().level() != first.address().level()
                        || g.footprint_m() != first.footprint_m()
                })
            })
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        self.append_inner(
            view,
            projection,
            body,
            patches,
            topology,
            style,
            Some(geometry),
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn append_stitched(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        body: crate::CelestialRenderBody,
        patches: &[ActiveSurfacePatch],
        surface: &super::StitchedSurface,
        topology: &SurfaceTopology,
        style: SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        if self.report.patches.saturating_add(patches.len()) > 4096 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let geometry = patches
            .iter()
            .map(|p| {
                let index = surface
                    .patches()
                    .binary_search_by_key(&p.address, GeneratedSurfacePatch::address)
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                if surface.stitch_mask(index) != Some(p.stitch_mask) {
                    return Err(RenderPreparationError::InvalidDebugGeometry);
                }
                Ok(&surface.patches()[index])
            })
            .collect::<Result<Vec<_>, RenderPreparationError>>()?;
        self.append_inner(
            view,
            projection,
            body,
            patches,
            topology,
            style,
            Some(&geometry),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn append_inner(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        body: crate::CelestialRenderBody,
        patches: &[ActiveSurfacePatch],
        topology: &SurfaceTopology,
        style: SurfaceStyle,
        geometry: Option<&[&GeneratedSurfacePatch]>,
    ) -> Result<(), RenderPreparationError> {
        if self.report.patches.saturating_add(patches.len()) > 4096 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        #[cfg(feature = "surface-profile")]
        let total_start = super::CpuStageTimer::new();
        #[cfg(feature = "surface-profile")]
        let capacities = self.profile_capacities();
        #[cfg(feature = "surface-profile")]
        let stage_start = std::time::Instant::now();
        #[cfg(feature = "surface-profile")]
        let boundary_keys_start = std::time::Instant::now();
        // Canonical tuples are shared within one source/radius batch. Another
        // body has the same unit tuples but different observer-relative samples.
        self.boundary_keys.clear();
        let needed = patches.len() * 64;
        self.drop_proofs_for_boundary_growth(
            self.boundary_keys.capacity().max(needed) * std::mem::size_of::<u128>()
                + self.boundary_samples.capacity() * std::mem::size_of::<([f32; 3], [f32; 3])>()
                + self.boundary_ready.capacity() * std::mem::size_of::<bool>(),
        );
        if needed > self.boundary_keys.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.boundary_keys.reserve_exact(needed);
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        for patch in patches {
            for index in 0..GRID_SAMPLES {
                let i = index as u32 % 17;
                let j = index as u32 / 17;
                if i == 0 || i == 16 || j == 0 || j == 16 {
                    self.boundary_keys.push(
                        patch
                            .address
                            .sample_key(i, j, 16)
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
                            .compact_key(),
                    );
                }
            }
        }
        self.boundary_keys.sort_unstable();
        // Store only keys shared by two patches. Unique boundary samples already
        // have one evaluation. Raw keys ≤4096×64×16 bytes; shared values ≤half
        // that count ×24 bytes, fitting the aggregate 8 MiB boundary cap.
        let mut read = 0;
        let mut write = 0;
        while read < self.boundary_keys.len() {
            let key = self.boundary_keys[read];
            let mut end = read + 1;
            while end < self.boundary_keys.len() && self.boundary_keys[end] == key {
                end += 1;
            }
            if end - read > 1 {
                self.boundary_keys[write] = key;
                write += 1;
            }
            read = end;
        }
        self.boundary_keys.truncate(write);
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.boundary_keys += boundary_keys_start.elapsed();
        }
        self.boundary_samples.clear();
        self.boundary_ready.clear();
        self.drop_proofs_for_boundary_growth(
            self.boundary_keys.capacity() * std::mem::size_of::<u128>()
                + self.boundary_samples.capacity().max(write)
                    * std::mem::size_of::<([f32; 3], [f32; 3])>()
                + self.boundary_ready.capacity().max(write) * std::mem::size_of::<bool>(),
        );
        if write > self.boundary_samples.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.boundary_samples.reserve_exact(write);
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        if write > self.boundary_ready.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.boundary_ready.reserve_exact(write);
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        self.boundary_samples.resize(write, ([0.0; 3], [0.0; 3]));
        self.boundary_ready.resize(write, false);
        self.prepare_proof_cache(patches.len());
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.boundary += stage_start.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_start = std::time::Instant::now();
        let source = body.body_fixed_frame;
        let radius = body.reference_radius_m;
        let color = body.color;
        if !radius.is_finite()
            || radius <= 0.0
            || !color.iter().all(|c| c.is_finite())
            || self.report.patches + patches.len() > 4096
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        if let Some(geometry) = geometry
            && (geometry.len() != patches.len()
                || geometry
                    .iter()
                    .zip(patches)
                    .any(|(g, p)| g.address() != p.address)
                || geometry.iter().enumerate().any(|(i, g)| {
                    geometry[..i]
                        .iter()
                        .any(|prior| prior.address() == g.address())
                })
                || geometry.iter().any(|g| g.reference_radius_m() != radius))
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        // Complete preflight includes pessimistic clipped polygons (up to 8 vertices,
        // 6 triangles per input). Do not grow power-of-two past the aggregate cap.
        let needed_samples = self.samples.len() + patches.len() * GRID_SAMPLES * 48;
        let needed_instances = self.instances.len() + patches.len() * 64;
        let needed_records = self.instances.len() / 64 + patches.len();
        let allocated = self.samples.capacity().max(needed_samples)
            + self.instances.capacity().max(needed_instances)
            + self.fallback.capacity()
            + self.records.capacity().max(needed_records) * std::mem::size_of::<(u8, [u8; 64])>();
        if allocated > STAGING_CAP {
            return Err(RenderPreparationError::InvalidBudget);
        }
        if needed_samples > self.samples.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.samples
                .reserve_exact(needed_samples - self.samples.len());
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        if needed_instances > self.instances.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.instances
                .reserve_exact(needed_instances - self.instances.len());
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        if needed_records > self.records.capacity() {
            #[cfg(feature = "surface-profile")]
            let started = std::time::Instant::now();
            self.records
                .reserve_exact(needed_records - self.records.len());
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.allocation += started.elapsed();
            }
        }
        if self.allocated_outgoing_bytes() > STAGING_CAP {
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.underside |= style.underside;
        let prepared = view.prepare_source(source)?;
        let mut positions = [DVec3::ZERO; GRID_SAMPLES];
        let mut normals = [DVec3::ZERO; GRID_SAMPLES];
        let mut gpu_positions = [[0.0; 3]; GRID_SAMPLES];
        let mut gpu_normals = [[0.0; 3]; GRID_SAMPLES];
        let mut elevations = [0.0f32; GRID_SAMPLES];
        // Readability data is transient per-frame GPU payload. Position-derived
        // radial slope is analytic for generated samples; reconciled edges use
        // their interpolated visual normal and are therefore diagnostic estimates.
        let mut classifications = [[0.0f32; 4]; GRID_SAMPLES];
        let mut radial_directions = [DVec3::ZERO; GRID_SAMPLES];
        // Instance buckets are appended as a complete batch. Rebucket aggregate
        // instances after appending; sample bases are unaffected by their order.
        self.records.clear();
        for mask in 0..16 {
            for i in self.buckets[mask].clone() {
                self.records.push((
                    mask as u8,
                    self.instances[i as usize * 64..(i as usize + 1) * 64]
                        .try_into()
                        .expect("instance stride"),
                ));
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.setup += stage_start.elapsed();
        }
        for (patch_index, patch) in patches.iter().enumerate() {
            #[cfg(feature = "surface-profile")]
            let sample_start = std::time::Instant::now();
            if patch.stitch_mask > 15 {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            #[cfg(feature = "surface-profile")]
            let conversion_start = std::time::Instant::now();
            for index in 0..GRID_SAMPLES {
                if let Some(geometry) = geometry {
                    let sample = geometry[patch_index].samples()[index];
                    positions[index] = prepared
                        .view_displacement(FramePosition::new(
                            source,
                            LocalPosition::try_metres(sample.position_body_m)?,
                        ))?
                        .metres();
                    // Terrain shading and normal diagnostics use body-fixed axes.
                    // Position conversion remains source-centred and camera-relative.
                    normals[index] = sample.normal_body;
                } else {
                    let i = index as u32 % 17;
                    let j = index as u32 / 17;
                    let key = patch
                        .address
                        .sample_key(i, j, 16)
                        .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                    let direction = key.direction();
                    positions[index] = prepared
                        .view_displacement(FramePosition::new(
                            source,
                            LocalPosition::try_metres(direction.unit() * radius)?,
                        ))?
                        .metres();
                    normals[index] = prepared.view_direction(direction)?.unit();
                    let oct = if geometry.is_some()
                        || self.lighting.mode() == crate::planet_surface::TerrainRenderMode::Natural
                    {
                        radial_directions[index] = direction.unit();
                        encode_octahedral(direction.unit())
                    } else {
                        [0.0; 2]
                    };
                    classifications[index] = [0.0, 0.0, oct[0], oct[1]];
                }
            }
            #[cfg(feature = "surface-profile")]
            {
                *self
                    .report
                    .profile
                    .coordinate_conversion
                    .get_or_insert_default() += conversion_start.elapsed();
            }
            #[cfg(feature = "surface-profile")]
            let classification_start = std::time::Instant::now();
            if let Some(geometry) = geometry {
                for index in 0..GRID_SAMPLES {
                    let sample = geometry[patch_index].samples()[index];
                    if style.elevation_colors && !style.lod_colors {
                        let extent = geometry[patch_index].extent();
                        let envelope = extent.min_height_m.abs().max(extent.max_height_m.abs());
                        elevations[index] = if envelope == 0.0 {
                            0.0
                        } else {
                            ((sample.position_body_m.length() - radius) / envelope).clamp(-1.0, 1.0)
                                as f32
                        };
                    }
                    let radial = sample.position_body_m.normalize_or_zero();
                    radial_directions[index] = radial;
                    let oct = encode_octahedral(radial);
                    classifications[index] = [
                        (sample.position_body_m.length() - radius) as f32,
                        radial
                            .cross(sample.normal_body)
                            .length()
                            .atan2(radial.dot(sample.normal_body)) as f32,
                        oct[0],
                        oct[1],
                    ];
                }
            }
            #[cfg(feature = "surface-profile")]
            {
                *self.report.profile.classification.get_or_insert_default() +=
                    classification_start.elapsed();
            }
            #[cfg(feature = "surface-profile")]
            let narrowing_start = std::time::Instant::now();
            for index in 0..GRID_SAMPLES {
                let i = index as u32 % 17;
                let j = index as u32 / 17;
                // Recompute this pure tuple in the narrowing batch rather than
                // adding a second patch-sized stack buffer to the staging bound.
                let key = patch
                    .address
                    .sample_key(i, j, 16)
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                let packed = (
                    positions[index].as_vec3().to_array(),
                    normals[index].as_vec3().to_array(),
                );
                let packed = if (i == 0 || i == 16 || j == 0 || j == 16)
                    && let Ok(at) = self.boundary_keys.binary_search(&key.compact_key())
                {
                    if !self.boundary_ready[at] {
                        self.boundary_samples[at] = packed;
                        self.boundary_ready[at] = true;
                    }
                    self.boundary_samples[at]
                } else {
                    packed
                };
                gpu_positions[index] = packed.0;
                gpu_normals[index] = packed.1;
                if !positions[index].is_finite()
                    || !normals[index].is_finite()
                    || !packed.0.iter().chain(&packed.1).all(|c| c.is_finite())
                {
                    return Err(RenderPreparationError::InvalidDebugGeometry);
                }
            }
            #[cfg(feature = "surface-profile")]
            {
                *self.report.profile.boundary_narrow.get_or_insert_default() +=
                    narrowing_start.elapsed();
            }
            let indices = topology.indices(patch.stitch_mask);
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.samples += sample_start.elapsed();
            }
            #[cfg(feature = "surface-profile")]
            let proof_start = std::time::Instant::now();
            let projection_key = (
                projection.viewport(),
                projection.vertical_fov_rad().to_bits(),
                projection.near_m().to_bits(),
            );
            #[cfg(feature = "surface-profile")]
            let proof_lookup_start = std::time::Instant::now();
            let proof_slot = self
                .proof_cache
                .iter()
                .position(|entry| entry.source == source && entry.address == patch.address);
            let cached = proof_slot.and_then(|index| {
                let entry = &self.proof_cache[index];
                (entry.mask == patch.stitch_mask
                    && entry.projection == projection_key
                    && same_positions(&entry.positions, &positions)
                    && same_gpu_positions(&entry.gpu_positions, &gpu_positions))
                .then_some(entry.result)
            });
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.proof_lookup += proof_lookup_start.elapsed();
            }
            let (fallback, whole_proof) = if let Some(result) = cached {
                self.report.proof_cache_hits += 1;
                self.report.max_component_error_m =
                    self.report.max_component_error_m.max(result.component_m);
                self.report.max_projected_error_pixels =
                    self.report.max_projected_error_pixels.max(result.pixels);
                self.report.max_gpu_projection_error_pixels = self
                    .report
                    .max_gpu_projection_error_pixels
                    .max(result.gpu_pixels);
                (result.fallback, result.whole)
            } else {
                #[cfg(feature = "surface-profile")]
                let proof_calculation_start = std::time::Instant::now();
                self.report.proof_cache_misses += 1;
                let previous_errors = [
                    self.report.max_component_error_m,
                    self.report.max_projected_error_pixels,
                    self.report.max_gpu_projection_error_pixels,
                ];
                self.report.max_component_error_m = 0.0;
                self.report.max_projected_error_pixels = 0.0;
                self.report.max_gpu_projection_error_pixels = 0.0;
                let mut fallback = false;
                // Convex interpolation cannot exceed the maximum vertex perturbation.
                // Front-of-near patches prove all clipped triangles at once using the
                // projection differential over the expanded frustum. Only uncertain
                // crossings require individual triangle clipping.
                let mut delta_max: f64 = 0.0;
                let mut component_max: f64 = 0.0;
                let mut depth_min = f64::MAX;
                for (p, gpu) in positions.iter().zip(gpu_positions) {
                    let delta = DVec3::from_array(gpu.map(f64::from)) - *p;
                    delta_max = delta_max.max(delta.length());
                    component_max = component_max.max(delta.abs().max_element());
                    depth_min = depth_min.min(-p.z);
                }
                let [w, h] = projection.viewport();
                let ty = (projection.vertical_fov_rad() * 0.5).tan();
                let tx = ty * w as f64 / h as f64;
                let safe = depth_min - delta_max;
                let pixel_bound = if safe > 0.0 {
                    projection.focal_pixels() * delta_max / safe
                        * (1.0 + (tx + delta_max / safe).powi(2) + (ty + delta_max / safe).powi(2))
                            .sqrt()
                } else {
                    f64::INFINITY
                };
                let scales = projection.gpu_clip([1.0, 1.0, -1.0]);
                let coefficient_error = (scales[0] as f64 * tx - 1.0)
                    .abs()
                    .max((scales[1] as f64 * ty - 1.0).abs());
                let arithmetic_bound = 0.5
                    * (w as f64).hypot(h as f64)
                    * (coefficient_error + 8.0 * f32::EPSILON as f64);
                let whole_proof = depth_min >= projection.near_m()
                    && pixel_bound <= 0.05
                    && arithmetic_bound <= 0.05
                    && component_max <= physical_budget(DVec3::new(0.0, 0.0, -depth_min));
                if whole_proof {
                    self.report.max_component_error_m =
                        self.report.max_component_error_m.max(component_max);
                    self.report.max_projected_error_pixels =
                        self.report.max_projected_error_pixels.max(pixel_bound);
                    self.report.max_gpu_projection_error_pixels = self
                        .report
                        .max_gpu_projection_error_pixels
                        .max(arithmetic_bound);
                }
                if !whole_proof {
                    for triangle in indices.as_chunks::<3>().0 {
                        let points = triangle.map(|i| positions[usize::from(i)]);
                        let polygon = clip_triangle(points, projection);
                        if polygon.is_empty() {
                            continue;
                        }
                        let gpu = triangle.map(|i| {
                            DVec3::from_array(gpu_positions[usize::from(i)].map(f64::from))
                        });
                        let mut max_delta: f64 = 0.0;
                        let mut zmin = f64::MAX;
                        for v in &polygon {
                            let round =
                                gpu[0] * v.weights.x + gpu[1] * v.weights.y + gpu[2] * v.weights.z;
                            let delta = round - v.position;
                            max_delta = max_delta.max(delta.length());
                            zmin = zmin.min(-v.position.z);
                            let component = delta.abs().max_element();
                            if component > physical_budget(v.position) {
                                fallback = true;
                            }
                            self.report.max_component_error_m =
                                self.report.max_component_error_m.max(component);
                        }
                        let [w, h] = projection.viewport();
                        let ty = (projection.vertical_fov_rad() * 0.5).tan();
                        let tx = ty * w as f64 / h as f64;
                        let safe = zmin - max_delta;
                        let error = if safe > 0.0 {
                            projection.focal_pixels() * max_delta / safe
                                * (1.0
                                    + (tx + max_delta / safe).powi(2)
                                    + (ty + max_delta / safe).powi(2))
                                .sqrt()
                        } else {
                            f64::INFINITY
                        };
                        fallback |= error > 0.05;
                        // Projection multiplication rounding is measured independently.
                        for v in &polygon {
                            let exact = screen(projection, v.position);
                            let narrowed = v.position.as_vec3().to_array();
                            let rounded =
                                screen(projection, DVec3::from_array(narrowed.map(f64::from)));
                            let projected = gpu_screen(projection, narrowed);
                            let arithmetic =
                                (rounded[0] - projected[0]).hypot(rounded[1] - projected[1]);
                            self.report.max_gpu_projection_error_pixels =
                                self.report.max_gpu_projection_error_pixels.max(arithmetic);
                            fallback |= arithmetic > 0.05;
                            let total = (exact[0] - projected[0]).hypot(exact[1] - projected[1]);
                            if !total.is_finite() {
                                fallback = true;
                            }
                        }
                        if !fallback {
                            self.report.max_projected_error_pixels =
                                self.report.max_projected_error_pixels.max(error);
                        }
                    }
                }
                let result = PrecisionProofResult {
                    fallback,
                    whole: whole_proof,
                    component_m: self.report.max_component_error_m,
                    pixels: self.report.max_projected_error_pixels,
                    gpu_pixels: self.report.max_gpu_projection_error_pixels,
                };
                self.report.max_component_error_m = previous_errors[0].max(result.component_m);
                self.report.max_projected_error_pixels = previous_errors[1].max(result.pixels);
                self.report.max_gpu_projection_error_pixels =
                    previous_errors[2].max(result.gpu_pixels);
                if self.proof_cache.capacity() != 0 {
                    let entry = PrecisionProofEntry {
                        source,
                        address: patch.address,
                        mask: patch.stitch_mask,
                        projection: projection_key,
                        positions,
                        gpu_positions,
                        result,
                    };
                    if let Some(index) = proof_slot {
                        self.proof_cache[index] = entry;
                    } else if self.proof_cache.len() < self.proof_cache.capacity() {
                        self.proof_cache.push(entry);
                    } else {
                        self.proof_cache[self.proof_cursor] = entry;
                        self.proof_cursor = (self.proof_cursor + 1) % self.proof_cache.len();
                    }
                }
                #[cfg(feature = "surface-profile")]
                {
                    self.report.profile.proof_calculation += proof_calculation_start.elapsed();
                }
                (fallback, whole_proof)
            };
            #[cfg(not(feature = "surface-profile"))]
            let _ = whole_proof;
            let patch_color = if style.lod_colors {
                super::lod_color(patch.address.level())
            } else if style.face_colors {
                [
                    [1.0, 0.3, 0.3, 1.0],
                    [0.5, 0.1, 0.1, 1.0],
                    [0.3, 1.0, 0.3, 1.0],
                    [0.1, 0.5, 0.1, 1.0],
                    [0.3, 0.3, 1.0, 1.0],
                    [0.1, 0.1, 0.5, 1.0],
                ][patch.address.face() as usize]
            } else {
                color
            };
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.proof += proof_start.elapsed();
                self.report.profile.whole_patch_proofs += usize::from(whole_proof);
                self.report.profile.clipped_patch_proofs += usize::from(!whole_proof);
            }
            #[cfg(feature = "surface-profile")]
            let packing_start = std::time::Instant::now();
            if fallback {
                for triangle in indices.as_chunks::<3>().0 {
                    let points = triangle.map(|i| positions[usize::from(i)]);
                    let polygon = clip_triangle(points, projection);
                    if polygon.len() < 3 {
                        continue;
                    }
                    let needed = (polygon.len() - 2) * 3 * 80;
                    self.reserve_fallback(needed)?;
                    for i in 1..polygon.len() - 1 {
                        for v in [polygon[0], polygon[i], polygon[i + 1]] {
                            let f = projection.focal_pixels();
                            let [w, h] = projection.viewport();
                            let clip = [
                                v.position.x * f * 2.0 / w as f64,
                                v.position.y * f * 2.0 / h as f64,
                                projection.near_m(),
                                -v.position.z,
                            ];
                            let gpu = clip.map(|c| c as f32);
                            let expected = screen(projection, v.position);
                            let actual = [
                                gpu[0] as f64 / gpu[3] as f64 * w as f64 * 0.5,
                                gpu[1] as f64 / gpu[3] as f64 * h as f64 * 0.5,
                            ];
                            let error = (expected[0] - actual[0]).hypot(expected[1] - actual[1]);
                            if !gpu.iter().all(|c| c.is_finite()) || error > 0.05 {
                                return Err(RenderPreparationError::PrecisionBudgetExceeded {
                                    error_m: error,
                                    limit_m: 0.05,
                                });
                            }
                            self.report.max_projected_error_pixels =
                                self.report.max_projected_error_pixels.max(error);
                            let normal = triangle
                                .iter()
                                .zip(v.weights.to_array())
                                .map(|(&j, t)| normals[usize::from(j)] * t)
                                .sum::<DVec3>()
                                .as_vec3()
                                .to_array();
                            let uv = triangle.iter().zip(v.weights.to_array()).fold(
                                [0.0; 2],
                                |mut uv, (&j, t)| {
                                    uv[0] += f64::from(j % 17) / 16.0 * t;
                                    uv[1] += f64::from(j / 17) / 16.0 * t;
                                    uv
                                },
                            );
                            pack_floats(
                                &mut self.fallback,
                                gpu.into_iter()
                                    .chain([
                                        normal[0],
                                        normal[1],
                                        normal[2],
                                        triangle
                                            .iter()
                                            .zip(v.weights.to_array())
                                            .map(|(&j, t)| {
                                                f64::from(elevations[usize::from(j)]) * t
                                            })
                                            .sum::<f64>()
                                            as f32,
                                    ])
                                    .chain(patch_color)
                                    .chain([
                                        uv[0] as f32,
                                        uv[1] as f32,
                                        (u32::from(style.borders)
                                            | if style.elevation_colors
                                                && !style.lod_colors
                                                && geometry.is_some()
                                            {
                                                2
                                            } else {
                                                0
                                            }
                                            | if geometry.is_some() { 4 } else { 0 }
                                            | if style.lod_colors { 8 } else { 0 })
                                            as f32,
                                        0.0,
                                    ])
                                    .chain({
                                        let radial = triangle
                                            .iter()
                                            .zip(v.weights.to_array())
                                            .map(|(&j, t)| radial_directions[usize::from(j)] * t)
                                            .sum::<DVec3>();
                                        let oct = if self.lighting.mode()
                                            == crate::planet_surface::TerrainRenderMode::Natural
                                        {
                                            encode_octahedral(radial)
                                        } else {
                                            [0.0; 2]
                                        };
                                        [
                                            triangle
                                                .iter()
                                                .zip(v.weights.to_array())
                                                .map(|(&j, t)| {
                                                    f64::from(classifications[usize::from(j)][0])
                                                        * t
                                                })
                                                .sum::<f64>()
                                                as f32,
                                            triangle
                                                .iter()
                                                .zip(v.weights.to_array())
                                                .map(|(&j, t)| {
                                                    f64::from(classifications[usize::from(j)][1])
                                                        * t
                                                })
                                                .sum::<f64>()
                                                as f32,
                                            oct[0],
                                            oct[1],
                                        ]
                                    }),
                            );
                        }
                        self.report.fallback_triangles += 1;
                    }
                }
            } else {
                let base = (self.samples.len() / 48) as u32;
                for (index, ((p, n), elevation)) in gpu_positions
                    .into_iter()
                    .zip(gpu_normals)
                    .zip(elevations)
                    .enumerate()
                {
                    // One bounded append per sample, rather than eight independent
                    // vector length/capacity checks. Classification adds sixteen bytes.
                    let mut sample = [0; 48];
                    for (value, out) in [p[0], p[1], p[2], 1.0, n[0], n[1], n[2], elevation]
                        .into_iter()
                        .chain(classifications[index])
                        .zip(sample.as_chunks_mut::<4>().0)
                    {
                        out.copy_from_slice(&value.to_le_bytes());
                    }
                    self.samples.extend_from_slice(&sample);
                }
                let mut record = [0; 64];
                for (value, out) in [
                    base,
                    u32::from(patch.address.level()),
                    patch.address.face() as u32,
                    u32::from(style.borders)
                        | if style.elevation_colors && !style.lod_colors && geometry.is_some() {
                            2
                        } else {
                            0
                        }
                        | if geometry.is_some() { 4 } else { 0 }
                        | if style.lod_colors { 8 } else { 0 },
                ]
                .into_iter()
                .zip(record[..16].as_chunks_mut::<4>().0)
                {
                    out.copy_from_slice(&value.to_le_bytes());
                }
                for (value, out) in patch_color
                    .into_iter()
                    .zip(record[16..32].as_chunks_mut::<4>().0)
                {
                    out.copy_from_slice(&value.to_le_bytes());
                }
                self.records.push((patch.stitch_mask, record));
                self.report.samples += GRID_SAMPLES;
                self.report.triangles += indices.len() / 3;
            }
            self.report.patches += 1;
            #[cfg(feature = "surface-profile")]
            {
                self.report.profile.packing += packing_start.elapsed();
            }
        }
        #[cfg(feature = "surface-profile")]
        let grouping_start = std::time::Instant::now();
        // The mask pass below is already a stable bucket grouping. Sorting first
        // adds work and stable-sort scratch without changing the resulting order.
        self.instances.clear();
        let mut cursor = 0u32;
        for mask in 0..16 {
            let start = cursor;
            for (_, record) in self.records.iter().filter(|r| r.0 == mask) {
                self.instances.extend_from_slice(record);
                cursor += 1;
            }
            self.buckets[usize::from(mask)] = start..cursor;
        }
        self.report.draws = self.buckets.iter().filter(|r| !r.is_empty()).count()
            + usize::from(!self.fallback.is_empty());
        self.report.uploaded_bytes =
            self.samples.len() + self.instances.len() + self.fallback.len();
        self.report.allocated_staging_bytes = self.samples.capacity()
            + self.instances.capacity()
            + self.fallback.capacity()
            + self.records.capacity() * std::mem::size_of::<(u8, [u8; 64])>();
        self.report.boundary_bytes = self.report.boundary_bytes.max(
            self.boundary_capacity_bytes()
                + self.proof_cache.capacity() * std::mem::size_of::<PrecisionProofEntry>(),
        );
        if self.report.allocated_staging_bytes > STAGING_CAP
            || self.report.boundary_bytes > BOUNDARY_CAP
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        #[cfg(feature = "surface-profile")]
        {
            self.report.profile.grouping += grouping_start.elapsed();
            self.report.profile.capacity_growths += capacities
                .into_iter()
                .zip(self.profile_capacities())
                .filter(|(before, after)| after > before)
                .count();
            self.report.profile.total += total_start.wall_elapsed();
            self.report.profile.regular_thread_cpu_last_batch = total_start.thread_elapsed();
        }
        Ok(())
    }
    #[cfg(feature = "surface-profile")]
    fn profile_capacities(&self) -> [usize; 8] {
        [
            self.samples.capacity(),
            self.instances.capacity(),
            self.fallback.capacity(),
            self.boundary_keys.capacity(),
            self.boundary_samples.capacity(),
            self.boundary_ready.capacity(),
            self.records.capacity(),
            self.proof_cache.capacity(),
        ]
    }
}
fn pack_floats(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::{surface::*, *};
    use std::num::NonZeroU64;
    #[test]
    fn octahedral_round_trip_remains_continuous_across_lower_hemisphere_folds() {
        for direction in [
            DVec3::new(1e-12, -0.7, -0.7),
            DVec3::new(-1e-12, -0.7, -0.7),
            DVec3::new(0.7, 1e-12, -0.7),
            DVec3::new(0.7, -1e-12, -0.7),
            DVec3::new(0.0, 0.0, -1.0),
        ] {
            let encoded = encode_octahedral(direction);
            let mut decoded = DVec3::new(
                f64::from(encoded[0]),
                f64::from(encoded[1]),
                1.0 - f64::from(encoded[0]).abs() - f64::from(encoded[1]).abs(),
            );
            if decoded.z < 0.0 {
                let sign = |v: f64| if v >= 0.0 { 1.0 } else { -1.0 };
                decoded = DVec3::new(
                    (1.0 - decoded.y.abs()) * sign(decoded.x),
                    (1.0 - decoded.x.abs()) * sign(decoded.y),
                    decoded.z,
                );
            }
            assert!(direction.normalize().dot(decoded.normalize()) > 0.9999999);
        }
    }
    fn patch(face: CubeFace, topology: &SurfaceTopology) -> ActiveSurfacePatch {
        let address = CubePatchAddress::root(face);
        ActiveSurfacePatch {
            address,
            metadata: super::super::PatchMetadata::build(address, topology).unwrap(),
            stitch_mask: 0,
            error_pixels: 0.0,
        }
    }
    #[test]
    fn exact_precision_proofs_reuse_only_identical_inputs_and_keep_packed_bits() {
        let tree = FrameTree::new(NonZeroU64::new(501).unwrap());
        let source = tree.root();
        let view_at = |height: f64| {
            PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(
                        source,
                        LocalPosition::try_metres(DVec3::Z * height).unwrap(),
                    ),
                    UnitRotation::identity(),
                ),
                crate::RenderPrecisionBudget::near_debug(),
            )
            .unwrap()
        };
        let view = view_at(1000.05);
        let topology = SurfaceTopology::new();
        let mut patches = [patch(CubeFace::PositiveZ, &topology)];
        let projection = CelestialProjection::try_new(128, 96, 60.0_f64.to_radians(), 0.1).unwrap();
        let body = crate::CelestialRenderBody {
            body_fixed_frame: source,
            reference_radius_m: 1000.0,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: false,
        };
        let mut storage = SurfaceStaging::default();
        storage
            .append(
                &view,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_misses, 1);
        let expected = (
            storage.samples.clone(),
            storage.instances.clone(),
            storage.fallback.clone(),
        );
        let errors = [
            storage.report.max_component_error_m,
            storage.report.max_projected_error_pixels,
            storage.report.max_gpu_projection_error_pixels,
        ];
        storage.clear();
        storage
            .append(
                &view,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_hits, 1);
        assert_eq!(
            (&storage.samples, &storage.instances, &storage.fallback),
            (&expected.0, &expected.1, &expected.2)
        );
        assert_eq!(
            [
                storage.report.max_component_error_m,
                storage.report.max_projected_error_pixels,
                storage.report.max_gpu_projection_error_pixels
            ],
            errors
        );
        storage.proof_cache = Vec::new();
        storage.clear();
        storage
            .append(
                &view,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_hits, 0);
        assert_eq!(
            (&storage.samples, &storage.instances, &storage.fallback),
            (&expected.0, &expected.1, &expected.2)
        );
        let moved = view_at(1001.05);
        storage.clear();
        storage
            .append(
                &moved,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_misses, 1);
        let resized = CelestialProjection::try_new(160, 96, 60.0_f64.to_radians(), 0.1).unwrap();
        storage.clear();
        storage
            .append(
                &moved,
                resized,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_misses, 1);
        patches[0].stitch_mask = 8;
        storage.clear();
        storage
            .append(
                &moved,
                resized,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.report.proof_cache_misses, 1);
        assert!(storage.report.boundary_bytes <= BOUNDARY_CAP);
        storage.drop_proofs_for_boundary_growth(BOUNDARY_CAP);
        assert_eq!(storage.proof_cache.capacity(), 0);
        let a = [DVec3::ZERO; GRID_SAMPLES];
        let mut b = a;
        b[0].x = -0.0;
        assert!(!same_positions(&a, &b));
        let a = [[0.0f32; 3]; GRID_SAMPLES];
        let mut b = a;
        b[0][0] = -0.0;
        assert!(!same_gpu_positions(&a, &b));
    }

    #[test]
    fn triangle_staging_growth_is_geometric_and_warm_storage_is_reused() {
        let mut storage = SurfaceStaging::default();
        let mut growths = 0;
        for triangle in 0..10_000usize {
            let old = storage.fallback.capacity();
            storage.reserve_fallback(240).unwrap();
            growths += usize::from(storage.fallback.capacity() != old);
            storage
                .fallback
                .extend(std::iter::repeat_n((triangle % 251) as u8, 240));
            assert!(storage.allocated_outgoing_bytes() <= STAGING_CAP);
        }
        assert!(growths <= 16, "per-triangle reallocations: {growths}");
        let bytes = storage.fallback.clone();
        let capacity = storage.fallback.capacity();
        storage.fallback.clear();
        for triangle in 0..10_000usize {
            storage.reserve_fallback(240).unwrap();
            storage
                .fallback
                .extend(std::iter::repeat_n((triangle % 251) as u8, 240));
        }
        assert_eq!(storage.fallback, bytes);
        assert_eq!(storage.fallback.capacity(), capacity);
    }

    #[test]
    fn retained_capacities_and_records_are_preflighted_before_fallback_growth() {
        let mut storage = SurfaceStaging {
            samples: Vec::with_capacity(STAGING_CAP - 1024),
            records: Vec::with_capacity(4),
            ..Default::default()
        };
        let remaining = STAGING_CAP - storage.allocated_outgoing_bytes();
        storage.reserve_fallback(remaining).unwrap();
        assert!(storage.allocated_outgoing_bytes() <= STAGING_CAP);
        let before = storage.fallback.capacity();
        assert!(matches!(
            storage.reserve_fallback(remaining + 1),
            Err(RenderPreparationError::InvalidBudget)
        ));
        assert_eq!(storage.fallback.capacity(), before);
        assert!(storage.samples.is_empty() && storage.records.is_empty());
    }
    #[test]
    fn sample_bytes_and_interleaved_masks_preserve_stable_bucket_order() {
        let tree = FrameTree::new(NonZeroU64::new(17).unwrap());
        let source = tree.root();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    source,
                    LocalPosition::try_metres(DVec3::Z * 1000.0).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let topology = SurfaceTopology::new();
        let mut patches = [
            patch(CubeFace::PositiveX, &topology),
            patch(CubeFace::PositiveZ, &topology),
            patch(CubeFace::NegativeX, &topology),
        ];
        patches[0].stitch_mask = 8;
        patches[2].stitch_mask = 8;
        let body = crate::CelestialRenderBody {
            body_fixed_frame: source,
            reference_radius_m: 10.0,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        let mut storage = SurfaceStaging::default();
        storage
            .append(
                &view,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert!(storage.fallback.is_empty());
        assert_eq!(
            storage.report.boundary_bytes,
            storage.boundary_keys.capacity() * std::mem::size_of::<u128>()
                + storage.boundary_samples.capacity() * std::mem::size_of::<([f32; 3], [f32; 3])>()
                + storage.boundary_ready.capacity() * std::mem::size_of::<bool>()
                + storage.proof_cache.capacity() * std::mem::size_of::<PrecisionProofEntry>()
        );
        assert_eq!(storage.buckets[0], 0..1);
        assert_eq!(storage.buckets[8], 1..3);
        let bases: Vec<_> = storage
            .instances
            .as_chunks::<64>()
            .0
            .iter()
            .map(|r| u32::from_le_bytes(r[..4].try_into().unwrap()))
            .collect();
        assert_eq!(bases, [289, 0, 578]);
        for (patch_index, patch) in patches.iter().enumerate() {
            for index in 0..GRID_SAMPLES {
                let unit = patch
                    .address
                    .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                    .unwrap()
                    .unit();
                let position = (unit * 10.0 - DVec3::Z * 1000.0).as_vec3().to_array();
                let normal = unit.as_vec3().to_array();
                let expected: Vec<_> = [
                    position[0],
                    position[1],
                    position[2],
                    1.0,
                    normal[0],
                    normal[1],
                    normal[2],
                    0.0,
                ]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect();
                let mut expected = expected;
                expected.extend([0u8; 16]);
                let offset = (patch_index * GRID_SAMPLES + index) * 48;
                assert_eq!(&storage.samples[offset..offset + 48], expected);
            }
        }
        storage
            .append(
                &view,
                projection,
                body,
                &patches,
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        let bases: Vec<_> = storage
            .instances
            .as_chunks::<64>()
            .0
            .iter()
            .map(|r| u32::from_le_bytes(r[..4].try_into().unwrap()))
            .collect();
        assert_eq!(bases, [289, 1156, 0, 578, 867, 1445]);
        #[cfg(feature = "surface-profile")]
        {
            assert!(storage.report.profile.capacity_growths > 0);
            storage.clear();
            storage
                .append(
                    &view,
                    projection,
                    body,
                    &patches,
                    &topology,
                    SurfaceStyle::default(),
                )
                .unwrap();
            assert_eq!(storage.report.profile.capacity_growths, 0);
            assert_eq!(storage.report.profile.whole_patch_proofs, 3);
            assert_eq!(storage.report.profile.clipped_patch_proofs, 0);
            let p = storage.report.profile;
            assert!(p.boundary + p.setup + p.samples + p.proof + p.packing + p.grouping <= p.total);
        }
    }
    #[test]
    fn layouts_shared_edges_and_distinct_body_boundary_namespaces() {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let a = tree
            .insert(
                root,
                FrameState::stationary(RigidTransform::new(
                    Displacement3::try_metres(DVec3::new(0.0, 0.0, -1000.0)).unwrap(),
                    UnitRotation::identity(),
                )),
            )
            .unwrap();
        let b = tree
            .insert(
                root,
                FrameState::stationary(RigidTransform::new(
                    Displacement3::try_metres(DVec3::new(30.0, 0.0, -1500.0)).unwrap(),
                    UnitRotation::identity(),
                )),
            )
            .unwrap();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(root, LocalPosition::origin()),
                UnitRotation::identity(),
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
        let topology = SurfaceTopology::new();
        let mut storage = SurfaceStaging::default();
        let body = crate::CelestialRenderBody {
            body_fixed_frame: a,
            reference_radius_m: 10.0,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: false,
        };
        storage
            .append(
                &view,
                projection,
                body,
                &[
                    patch(CubeFace::PositiveX, &topology),
                    patch(CubeFace::PositiveZ, &topology),
                ],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_eq!(storage.samples.len(), 2 * 289 * 48);
        assert_eq!(storage.instances.len(), 2 * 64);
        assert!(storage.fallback.is_empty());
        assert!(storage.instances[32..64].iter().all(|&b| b == 0));
        for j in 0..=16 {
            let first = j * 17;
            let second = 289 + j * 17 + 16;
            assert_eq!(
                &storage.samples[first * 48..(first + 1) * 48],
                &storage.samples[second * 48..(second + 1) * 48]
            );
        }
        let mut other = body;
        other.body_fixed_frame = b;
        other.reference_radius_m = 20.0;
        storage
            .append(
                &view,
                projection,
                other,
                &[
                    patch(CubeFace::PositiveX, &topology),
                    patch(CubeFace::PositiveZ, &topology),
                ],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        let position = |index: usize| {
            DVec3::from_array(std::array::from_fn(|axis| {
                f64::from(f32::from_le_bytes(
                    storage.samples[index * 48 + axis * 4..index * 48 + axis * 4 + 4]
                        .try_into()
                        .unwrap(),
                ))
            }))
        };
        assert_eq!(position(289 + 144), DVec3::new(0.0, 0.0, -990.0));
        assert_eq!(position(3 * 289 + 144), DVec3::new(30.0, 0.0, -1480.0));
        assert_eq!(storage.report.draws, 1);
        assert!(storage.report.boundary_bytes <= 8 * 1024 * 1024);
        eprintln!(
            "topology CPU={} bytes; sample48 instance64; payload={} staging={} boundary={}",
            topology.allocated_bytes(),
            storage.report.uploaded_bytes,
            storage.report.allocated_staging_bytes,
            storage.report.boundary_bytes
        );
    }
    #[test]
    fn clipped_material_coordinates_reconstruct_the_body_point() {
        let tree = FrameTree::new(NonZeroU64::new(503).unwrap());
        let root = tree.root();
        let radius = 6.4e6;
        let topology = SurfaceTopology::new();
        let patch = patch(CubeFace::PositiveZ, &topology);
        let [a, b, c] = topology.indices(0).as_chunks::<3>().0[272].map(|i| {
            patch
                .address
                .sample_direction(u32::from(i % 17), u32::from(i / 17), 16)
                .unwrap()
                .unit()
                * radius
        });
        let normal = (b - a).cross(c - a).normalize();
        let observer = (a + b + c) / 3.0 + normal * 0.11;
        let orientation =
            UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_arc(DVec3::Z, normal))
                .unwrap();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(root, LocalPosition::try_metres(observer).unwrap()),
                orientation,
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
        let samples = (0..GRID_SAMPLES)
            .map(|index| {
                let direction = patch
                    .address
                    .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                    .unwrap()
                    .unit();
                super::super::SurfaceGeometrySample {
                    position_body_m: direction * radius,
                    normal_body: direction,
                }
            })
            .collect();
        let geometry = super::super::GeneratedSurfacePatch::new(
            patch.address,
            radius,
            0.0,
            samples,
            super::super::SurfaceExtent::smooth(radius),
            Default::default(),
        )
        .unwrap();
        let body = crate::CelestialRenderBody {
            body_fixed_frame: root,
            reference_radius_m: radius,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: false,
        };
        let mut storage = SurfaceStaging {
            lighting: super::super::TerrainLighting::default()
                .with_mode(super::super::TerrainRenderMode::Natural),
            ..Default::default()
        };
        storage
            .append_generated(
                &view,
                projection,
                body,
                &[patch],
                &[&geometry],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert!(storage.report.fallback_triangles > 0);
        assert!(storage.samples.is_empty());
        let focal = 1.0 / (0.5_f64).tan();
        for vertex in storage.fallback.as_chunks::<80>().0 {
            let value = |offset: usize| {
                f64::from(f32::from_le_bytes(
                    vertex[offset..offset + 4].try_into().unwrap(),
                ))
            };
            // Homogeneous clipping may rescale the record. Undo projection by
            // its near/z factor rather than assuming clip.w is a metre depth.
            let scale = projection.near_m() / value(8);
            let body_point = observer
                + orientation.quaternion()
                    * DVec3::new(
                        value(0) * scale * 1.6 / focal,
                        value(4) * scale / focal,
                        -value(12) * scale,
                    );
            let expected = encode_octahedral(body_point.normalize());
            assert!((value(72) - f64::from(expected[0])).abs() < 2e-6);
            assert!((value(76) - f64::from(expected[1])).abs() < 2e-6);
        }
    }

    #[test]
    fn generated_flat_matches_sphere_and_displacement_changes_packed_samples() {
        let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(root, LocalPosition::try_metres(DVec3::Z * 1000.0).unwrap()),
                UnitRotation::identity(),
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
        let topology = SurfaceTopology::new();
        let patch = patch(CubeFace::PositiveZ, &topology);
        let address = patch.address;
        let samples: Vec<_> = (0..GRID_SAMPLES)
            .map(|index| {
                let direction = address
                    .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                    .unwrap()
                    .unit();
                super::super::SurfaceGeometrySample {
                    position_body_m: direction * 10.0,
                    normal_body: direction,
                }
            })
            .collect();
        let extent = super::super::SurfaceExtent::smooth(10.0);
        let geometry = super::super::GeneratedSurfacePatch::new(
            address,
            10.0,
            0.0,
            samples.clone(),
            extent,
            Default::default(),
        )
        .unwrap();
        let body = crate::CelestialRenderBody {
            body_fixed_frame: root,
            reference_radius_m: 10.0,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: false,
        };
        let mut sphere = SurfaceStaging::default();
        sphere
            .append(
                &view,
                projection,
                body,
                &[patch],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        let mut generated = SurfaceStaging::default();
        generated
            .append_generated(
                &view,
                projection,
                body,
                &[patch],
                &[&geometry],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        for (generated_sample, sphere_sample) in generated
            .samples
            .as_chunks::<48>()
            .0
            .iter()
            .zip(sphere.samples.as_chunks::<48>().0.iter())
        {
            assert_eq!(&generated_sample[..32], &sphere_sample[..32]);
        }
        for mode in super::super::TerrainRenderMode::ALL {
            let mut variant = SurfaceStaging {
                lighting: super::super::TerrainLighting::default().with_mode(mode),
                ..Default::default()
            };
            variant
                .append_generated(
                    &view,
                    projection,
                    body,
                    &[patch],
                    &[&geometry],
                    &topology,
                    SurfaceStyle::default(),
                )
                .unwrap();
            assert_eq!(
                variant.samples, generated.samples,
                "mode {mode:?} changed sample payload"
            );
            assert_eq!(
                variant.instances, generated.instances,
                "mode {mode:?} changed instances"
            );
        }
        assert_eq!(
            u32::from_le_bytes(generated.instances[12..16].try_into().unwrap()),
            4
        );
        // Moving/rotating the observer changes positions, not body-fixed terrain
        // normals or their interpretation in the normal diagnostic.
        for rotation in [
            glam::DQuat::from_rotation_z(0.7),
            glam::DQuat::from_rotation_x(0.1),
        ] {
            let rotated_view = PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(
                        root,
                        LocalPosition::try_metres(DVec3::new(5.0, 0.0, 1000.0)).unwrap(),
                    ),
                    UnitRotation::try_from_quaternion(rotation).unwrap(),
                ),
                crate::RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let mut rotated = SurfaceStaging::default();
            rotated
                .append_generated(
                    &rotated_view,
                    projection,
                    body,
                    &[patch],
                    &[&geometry],
                    &topology,
                    SurfaceStyle::default(),
                )
                .unwrap();
            assert_eq!(rotated.samples.len(), generated.samples.len());
            for (a, b) in rotated
                .samples
                .as_chunks::<48>()
                .0
                .iter()
                .zip(generated.samples.as_chunks::<48>().0)
            {
                assert_eq!(&a[16..28], &b[16..28]);
                assert_eq!(
                    &a[40..48],
                    &b[40..48],
                    "body material coordinates swam with the observer"
                );
            }
            assert_ne!(&rotated.samples[..12], &generated.samples[..12]);
        }
        let mut mismatched_body = body;
        mismatched_body.reference_radius_m = 10.0001;
        assert!(
            SurfaceStaging::default()
                .append_generated(
                    &view,
                    projection,
                    mismatched_body,
                    &[patch],
                    &[&geometry],
                    &topology,
                    SurfaceStyle::default(),
                )
                .is_err()
        );
        let raised: Vec<_> = samples
            .into_iter()
            .map(|mut sample| {
                sample.position_body_m *= 1.05;
                sample.normal_body = (sample.normal_body + DVec3::X * 0.1).normalize();
                sample
            })
            .collect();
        let raised_geometry = super::super::GeneratedSurfacePatch::new(
            address,
            10.0,
            0.0,
            raised,
            super::super::SurfaceExtent {
                min_height_m: -1.0,
                max_height_m: 1.0,
                guaranteed_opaque_radius_m: 0.0,
            },
            Default::default(),
        )
        .unwrap();
        let mut displaced = SurfaceStaging::default();
        displaced
            .append_generated(
                &view,
                projection,
                body,
                &[patch],
                &[&raised_geometry],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert_ne!(displaced.samples, sphere.samples);
        assert_ne!(&displaced.samples[0..12], &sphere.samples[0..12]);
        assert_ne!(&displaced.samples[16..28], &sphere.samples[16..28]);
        let mut diagnostic = SurfaceStaging::default();
        diagnostic
            .append_generated(
                &view,
                projection,
                body,
                &[patch],
                &[&raised_geometry],
                &topology,
                SurfaceStyle {
                    elevation_colors: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(&diagnostic.samples[..28], &displaced.samples[..28]);
        assert_eq!(
            f32::from_le_bytes(diagnostic.samples[28..32].try_into().unwrap()),
            0.5
        );
        let mut stitched = patch;
        stitched.stitch_mask = 1;
        assert!(
            SurfaceStaging::default()
                .append_generated(
                    &view,
                    projection,
                    body,
                    &[stitched],
                    &[&geometry],
                    &topology,
                    SurfaceStyle::default(),
                )
                .is_err()
        );
    }
    #[test]
    fn cold_coarse_parent_uses_bounded_clipped_triangles_and_bad_mask_rejects() {
        let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let radius = 6.4e6;
        let topology = SurfaceTopology::new();
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let [a, b, c] = topology.indices(0).as_chunks::<3>().0[272].map(|i| {
            address
                .sample_direction(u32::from(i % 17), u32::from(i / 17), 16)
                .unwrap()
                .unit()
                * radius
        });
        let normal = (b - a).cross(c - a).normalize();
        // Explicit below-reference-radius diagnostic: close to a coarse triangle
        // interior, so endpoint rounding cannot prove its near-plane crossing.
        let observer = (a + b + c) / 3.0 + normal * 0.11;
        let orientation =
            UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_arc(DVec3::Z, normal))
                .unwrap();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(root, LocalPosition::try_metres(observer).unwrap()),
                orientation,
            ),
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let mut storage = SurfaceStaging::default();
        let body = crate::CelestialRenderBody {
            body_fixed_frame: root,
            reference_radius_m: radius,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        storage
            .append(
                &view,
                projection,
                body,
                &[patch(CubeFace::PositiveZ, &topology)],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert!(storage.report.fallback_triangles > 0);
        assert_eq!(
            storage.fallback.len(),
            storage.report.fallback_triangles * 3 * 80
        );
        assert!(storage.report.max_projected_error_pixels <= 0.05);
        assert!(storage.report.allocated_staging_bytes < 64 * 1024 * 1024);
        let geometry = super::super::GeneratedSurfacePatch::new(
            address,
            radius,
            0.0,
            (0..GRID_SAMPLES)
                .map(|i| {
                    let n = address
                        .sample_direction(i as u32 % 17, i as u32 / 17, 16)
                        .unwrap()
                        .unit();
                    super::super::SurfaceGeometrySample {
                        position_body_m: n * radius,
                        normal_body: n,
                    }
                })
                .collect(),
            super::super::SurfaceExtent::smooth(radius),
            Default::default(),
        )
        .unwrap();
        let mut terrain = SurfaceStaging::default();
        terrain
            .append_generated(
                &view,
                projection,
                body,
                &[patch(CubeFace::PositiveZ, &topology)],
                &[&geometry],
                &topology,
                SurfaceStyle::default(),
            )
            .unwrap();
        assert!(terrain.report.fallback_triangles > 0);
        for vertex in terrain.fallback.as_chunks::<80>().0 {
            let n = DVec3::new(
                f32::from_le_bytes(vertex[16..20].try_into().unwrap()) as f64,
                f32::from_le_bytes(vertex[20..24].try_into().unwrap()) as f64,
                f32::from_le_bytes(vertex[24..28].try_into().unwrap()) as f64,
            );
            assert!(n.is_finite() && n.length() > 0.5);
            assert!(n.z > 0.5, "clipped terrain normals must stay in body axes");
            assert_eq!(f32::from_le_bytes(vertex[56..60].try_into().unwrap()), 4.0);
        }
        let mut bad = patch(CubeFace::PositiveZ, &topology);
        bad.stitch_mask = 16;
        assert!(
            storage
                .append(
                    &view,
                    projection,
                    body,
                    &[bad],
                    &topology,
                    SurfaceStyle::default()
                )
                .is_err()
        );
    }
}
