//! Minimal reusable indexed debug spheres and actual-history line preparation.
use crate::{
    CelestialProjection, DebugLine, PreparedView, RenderPreparationError, reference_sphere_occludes,
};
use astrum_math::{Direction3, FrameId, FramePosition, LocalPosition};
use glam::DVec3;
use std::collections::BTreeMap;

/// Deterministic level-3 icosphere: 642 unit vertices, 1280 outward triangles.
/// Planar facets have chordal error below 0.005 of physical reference radius.
pub struct Icosphere {
    vertices: Vec<DVec3>,
    indices: Vec<u32>,
}
impl Default for Icosphere {
    fn default() -> Self {
        Self::new()
    }
}
impl Icosphere {
    pub fn vertices(&self) -> &[DVec3] {
        &self.vertices
    }
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
    pub fn new() -> Self {
        let t = (1.0 + 5.0_f64.sqrt()) * 0.5;
        let mut vertices = vec![
            DVec3::new(-1.0, t, 0.0),
            DVec3::new(1.0, t, 0.0),
            DVec3::new(-1.0, -t, 0.0),
            DVec3::new(1.0, -t, 0.0),
            DVec3::new(0.0, -1.0, t),
            DVec3::new(0.0, 1.0, t),
            DVec3::new(0.0, -1.0, -t),
            DVec3::new(0.0, 1.0, -t),
            DVec3::new(t, 0.0, -1.0),
            DVec3::new(t, 0.0, 1.0),
            DVec3::new(-t, 0.0, -1.0),
            DVec3::new(-t, 0.0, 1.0),
        ];
        for v in &mut vertices {
            *v = v.normalize();
        }
        let mut triangles = vec![
            [0, 11, 5],
            [0, 5, 1],
            [0, 1, 7],
            [0, 7, 10],
            [0, 10, 11],
            [1, 5, 9],
            [5, 11, 4],
            [11, 10, 2],
            [10, 7, 6],
            [7, 1, 8],
            [3, 9, 4],
            [3, 4, 2],
            [3, 2, 6],
            [3, 6, 8],
            [3, 8, 9],
            [4, 9, 5],
            [2, 4, 11],
            [6, 2, 10],
            [8, 6, 7],
            [9, 8, 1],
        ];
        for _ in 0..3 {
            let mut edges = BTreeMap::new();
            let mut next = Vec::with_capacity(triangles.len() * 4);
            for [a, b, c] in triangles {
                let mut midpoint = |x: u32, y: u32| {
                    let key = (x.min(y), x.max(y));
                    *edges.entry(key).or_insert_with(|| {
                        let index = vertices.len() as u32;
                        vertices.push((vertices[x as usize] + vertices[y as usize]).normalize());
                        index
                    })
                };
                let ab = midpoint(a, b);
                let bc = midpoint(b, c);
                let ca = midpoint(c, a);
                next.extend([[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
            }
            triangles = next;
        }
        Self {
            vertices,
            indices: triangles.into_iter().flatten().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrum_math::*;
    #[test]
    fn vertex_uniform_layout_is_explicit_and_complete() {
        let tree = FrameTree::new(std::num::NonZeroU64::new(1).unwrap());
        let observer = FramePose::new(
            FramePosition::new(
                tree.root(),
                LocalPosition::try_metres(DVec3::Z * 4.0).unwrap(),
            ),
            UnitRotation::identity(),
        );
        let view = PreparedView::new(
            &tree.evaluate(),
            observer,
            crate::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let sphere = Icosphere::new();
        let mut storage = CelestialStaging::default();
        let mut frame = CelestialFrame::new(
            &view,
            &mut storage,
            CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap(),
            &sphere,
        );
        frame
            .append_bodies(&[CelestialRenderBody {
                body_fixed_frame: tree.root(),
                reference_radius_m: 1.0,
                color: [0.1, 0.2, 0.3, 1.0],
                unlit: true,
                selected: true,
                material: crate::SurfaceMaterial::default(),
                emission_nits: 0.0,
            }])
            .unwrap();
        assert_eq!(frame.staging.vertices.len(), 642 * 32);
        assert_eq!(frame.staging.uniforms.len(), 256);
        assert_eq!(
            f32::from_le_bytes(frame.staging.vertices[12..16].try_into().unwrap()),
            1.0
        );
        assert_eq!(
            f32::from_le_bytes(frame.staging.vertices[28..32].try_into().unwrap()),
            0.0
        );
        assert_eq!(
            u32::from_le_bytes(frame.staging.uniforms[16..20].try_into().unwrap()),
            1
        );
        assert_eq!(
            u32::from_le_bytes(frame.staging.uniforms[20..24].try_into().unwrap()),
            1
        );
        assert!(frame.staging.uniforms[24..].iter().all(|&b| b == 0));
        assert_eq!(super::DRAW_UNIFORM_BYTES, 48);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CelestialRenderBody {
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    /// Presentation colour; for emitters, the emission chromaticity.
    pub color: [f32; 4],
    /// Emitters (stars) are drawn with `emission_nits` and receive no light.
    pub unlit: bool,
    pub selected: bool,
    /// Reflectance of lit bodies.
    pub material: crate::SurfaceMaterial,
    /// Emitter disk luminance in cd/m².
    pub emission_nits: f32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SphereRepresentation {
    PhysicalSphere,
    Surface,
    SubpixelMarker,
    RangeMarker,
    PrecisionMarker,
    Culled,
}
#[derive(Debug, Clone, Copy)]
pub struct CelestialMarker {
    pub request_index: usize,
    pub screen_pixels: Option<[f32; 2]>,
    pub distance_m: f64,
    pub depth_m: f64,
    pub occluded: bool,
    pub representation: SphereRepresentation,
    pub apparent_diameter_pixels: f64,
    pub center_in_view_m: DVec3,
}
/// Hit radius in physical pixels; ties: distance, f64 depth, stable request order.
pub fn select_marker(markers: &[CelestialMarker], point: [f32; 2]) -> Option<usize> {
    markers
        .iter()
        .filter_map(|m| {
            m.screen_pixels.map(|p| {
                (
                    m,
                    (f64::from(p[0] - point[0])).hypot(f64::from(p[1] - point[1])),
                )
            })
        })
        .filter(|(_, d)| *d <= 8.0)
        .min_by(|(a, ad), (b, bd)| {
            ad.total_cmp(bd)
                .then(a.depth_m.total_cmp(&b.depth_m))
                .then(a.request_index.cmp(&b.request_index))
        })
        .map(|(m, _)| m.request_index)
}

#[derive(Default)]
pub struct CelestialStaging {
    sky: Option<crate::sky::SkyPrepared>,
    vertices: Vec<u8>,
    uniforms: Vec<u8>,
    polylines: Vec<u8>,
    draws: Vec<u32>,
    markers: Vec<CelestialMarker>,
    centers: Vec<(DVec3, f64)>,
    terrain_atlas: Option<crate::TerrainAtlasFrame>,
    lighting: Option<crate::FrameLighting>,
    view_mode: u32,
    passthrough: bool,
    line_style: crate::LineStyleScale,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct CelestialPreparationReport {
    pub sky: Option<crate::sky::SkyPreparationReport>,
    pub triangles: usize,
    pub markers: usize,
    pub trail_segments: usize,
    pub polyline_segments: usize,
    pub subpixel_bodies: usize,
    pub culled_bodies: usize,
    pub precision_fallbacks: usize,
    pub max_narrowing_error_m: f64,
    pub max_projected_error_pixels: f64,
}
pub struct CelestialFrame<'view, 'tree, 'storage> {
    view: &'view PreparedView<'tree>,
    staging: &'storage mut CelestialStaging,
    projection: CelestialProjection,
    sphere: &'view Icosphere,
    failed: bool,
    range_m: f64,
    report: CelestialPreparationReport,
}
impl<'view, 'tree, 'storage> CelestialFrame<'view, 'tree, 'storage> {
    pub fn new(
        view: &'view PreparedView<'tree>,
        staging: &'storage mut CelestialStaging,
        projection: CelestialProjection,
        sphere: &'view Icosphere,
    ) -> Self {
        staging.vertices.clear();
        staging.uniforms.clear();
        staging.polylines.clear();
        staging.lighting = None;
        staging.view_mode = crate::TerrainViewMode::Lit.shader_mode();
        staging.passthrough = false;
        staging.line_style = crate::LineStyleScale::default();
        staging.draws.clear();
        staging.markers.clear();
        staging.centers.clear();
        staging.terrain_atlas = None;
        staging.sky = None;
        Self {
            view,
            staging,
            projection,
            sphere,
            failed: false,
            range_m: 1e12,
            report: CelestialPreparationReport::default(),
        }
    }
    pub fn set_representation_range_m(
        &mut self,
        range_m: f64,
    ) -> Result<(), RenderPreparationError> {
        if !range_m.is_finite() || range_m <= 0.0 {
            self.failed = true;
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.range_m = range_m;
        Ok(())
    }

    /// Physically ordered light of this frame (docs/RENDER_PIPELINE_HDR.md §3).
    pub fn set_lighting(
        &mut self,
        lighting: crate::FrameLighting,
    ) -> Result<(), RenderPreparationError> {
        if lighting.validate().is_err() {
            self.failed = true;
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        self.staging.lighting = Some(lighting);
        Ok(())
    }

    /// Debug view applied to every surface and the post chain.
    pub fn set_view_mode(&mut self, mode: crate::TerrainViewMode) {
        self.staging.view_mode = mode.shader_mode();
        self.staging.passthrough = mode.passthrough();
    }

    /// Session width/opacity scale for guide lines prepared after this call.
    pub fn set_line_style(&mut self, style: crate::LineStyleScale) {
        self.staging.line_style = style;
    }

    /// Stages atlas producer jobs and instanced terrain draws (ADR 0016).
    pub fn set_terrain_atlas(&mut self, atlas: crate::TerrainAtlasFrame) {
        self.staging.terrain_atlas = Some(atlas);
    }

    /// Content projection used for this frame, including viewport and origin.
    pub fn set_distant_sky(
        &mut self,
        inertial_frame: FrameId,
        definition: std::sync::Arc<crate::sky::SkyDefinition>,
        settings: crate::sky::SkySettings,
    ) -> Result<(), RenderPreparationError> {
        let prepared = crate::sky::SkyPrepared::new(
            self.view,
            inertial_frame,
            definition,
            settings,
            self.projection,
        );
        match prepared {
            Ok(sky) => {
                self.staging.sky = Some(sky);
                Ok(())
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }

    /// Content projection used for this frame, including viewport and origin.
    pub fn projection(&self) -> CelestialProjection {
        self.projection
    }
    pub fn report(&self) -> CelestialPreparationReport {
        CelestialPreparationReport {
            sky: self.staging.sky.as_ref().map(|s| s.report()),
            ..self.report
        }
    }
    pub fn markers(&self) -> &[CelestialMarker] {
        &self.staging.markers
    }
    pub fn append_bodies(
        &mut self,
        bodies: &[CelestialRenderBody],
    ) -> Result<(), RenderPreparationError> {
        let result = self.bodies_checked(bodies, &[]);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn bodies_checked(
        &mut self,
        bodies: &[CelestialRenderBody],
        surface_available: &[bool],
    ) -> Result<(), RenderPreparationError> {
        self.validate()?;
        for body in bodies {
            if !body.reference_radius_m.is_finite()
                || body.reference_radius_m <= 0.0
                || !body.color.iter().all(|c| c.is_finite())
                || !body.material.validate()
                || !body.emission_nits.is_finite()
                || body.emission_nits < 0.0
            {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let center = self
                .view
                .prepare_source(body.body_fixed_frame)?
                .view_displacement(FramePosition::new(
                    body.body_fixed_frame,
                    LocalPosition::origin(),
                ))?
                .metres();
            self.staging.centers.push((center, body.reference_radius_m));
        }
        let base = self.staging.markers.len();
        for (i, body) in bodies.iter().enumerate() {
            let prepared = self.view.prepare_source(body.body_fixed_frame)?;
            let (center, radius) = self.staging.centers[base + i];
            let distance = center.x.hypot(center.y).hypot(center.z);
            if !distance.is_finite() {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let screen = self.projection.project_marker(center)?;
            let diameter = self
                .projection
                .sphere_apparent_diameter_pixels(center, radius)?;
            if !diameter.is_finite() {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let representation = if surface_available.get(i).copied().unwrap_or(false) {
                SphereRepresentation::Surface
            } else if -center.z + radius <= self.projection.near_m() {
                SphereRepresentation::Culled
            } else if distance - radius > self.range_m {
                SphereRepresentation::RangeMarker
            } else if diameter < 2.0 {
                SphereRepresentation::SubpixelMarker
            } else {
                SphereRepresentation::PhysicalSphere
            };
            let mut representation = representation;
            if representation == SphereRepresentation::PhysicalSphere {
                let start = self.staging.vertices.len();
                let mut fallback = false;
                for &unit in &self.sphere.vertices {
                    let point = FramePosition::new(
                        body.body_fixed_frame,
                        LocalPosition::try_metres(radius * unit)?,
                    );
                    let delta = prepared.view_displacement(point)?.metres();
                    // Representation fallback cannot hide invalid later f64 vertices.
                    if fallback {
                        continue;
                    }
                    match self.projection.narrow(delta, 0.001 * radius, self.range_m) {
                        Ok((position, error, pixels)) => {
                            let normal = prepared
                                .view_direction(Direction3::try_new(unit)?)?
                                .unit()
                                .as_vec3()
                                .to_array();
                            pack(position, normal, &mut self.staging.vertices, 0.0);
                            self.report.max_narrowing_error_m =
                                self.report.max_narrowing_error_m.max(error);
                            self.report.max_projected_error_pixels =
                                self.report.max_projected_error_pixels.max(pixels);
                        }
                        Err(
                            RenderPreparationError::PrecisionBudgetExceeded { .. }
                            | RenderPreparationError::OutsideRenderRange { .. },
                        ) => {
                            fallback = true;
                        }
                        Err(error) => return Err(error),
                    }
                }
                if fallback {
                    self.staging.vertices.truncate(start);
                    representation = SphereRepresentation::PrecisionMarker;
                    self.report.precision_fallbacks += 1;
                } else {
                    self.staging.draws.push((start / 32) as u32);
                    let offset = self.staging.uniforms.len();
                    self.staging.uniforms.resize(offset + 256, 0);
                    let albedo = body.material.albedo;
                    let color = if body.unlit {
                        body.color
                    } else {
                        [albedo[0], albedo[1], albedo[2], 1.0]
                    };
                    let emission = if body.unlit {
                        [
                            body.color[0] * body.emission_nits,
                            body.color[1] * body.emission_nits,
                            body.color[2] * body.emission_nits,
                            0.0,
                        ]
                    } else {
                        [0.0; 4]
                    };
                    let brdf = body.material.brdf.shader_index() as u32;
                    let flags = [u32::from(body.unlit), u32::from(body.selected), brdf, 0];
                    let words = color
                        .iter()
                        .map(|v| v.to_le_bytes())
                        .chain(flags.iter().map(|v| v.to_le_bytes()))
                        .chain(emission.iter().map(|v| v.to_le_bytes()));
                    for (bytes, out) in words.zip(
                        self.staging.uniforms[offset..offset + 48]
                            .as_chunks_mut::<4>()
                            .0,
                    ) {
                        out.copy_from_slice(&bytes);
                    }
                    self.report.triangles += 1280;
                }
            }
            if representation == SphereRepresentation::SubpixelMarker {
                self.report.subpixel_bodies += 1;
            }
            if representation == SphereRepresentation::Culled {
                self.report.culled_bodies += 1;
            }
            let occluded = self
                .staging
                .centers
                .iter()
                .enumerate()
                .any(|(j, (other, r))| {
                    j != base + i && reference_sphere_occludes(center, *other, *r)
                });
            self.staging.markers.push(CelestialMarker {
                request_index: base + i,
                screen_pixels: screen,
                distance_m: distance,
                depth_m: -center.z,
                occluded,
                representation,
                apparent_diameter_pixels: diameter,
                center_in_view_m: center,
            });
            self.report.markers += usize::from(screen.is_some());
        }
        Ok(())
    }
    /// One dense observation/overlay identity mapping, with explicitly app-requested
    /// opaque responsibility. Ready surfaces suppress only the corresponding sphere.
    pub fn append_body_observations(
        &mut self,
        bodies: &[CelestialRenderBody],
        surface_available: &[bool],
    ) -> Result<(), RenderPreparationError> {
        let result = if bodies.len() != surface_available.len() {
            Err(RenderPreparationError::LengthMismatch {
                input: bodies.len(),
                output: surface_available.len(),
            })
        } else {
            self.bodies_checked(bodies, surface_available)
        };
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    pub fn append_historical_lines(
        &mut self,
        source: FrameId,
        lines: &[DebugLine],
    ) -> Result<(), RenderPreparationError> {
        let result = self.lines_checked(source, lines);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Semantics belong to the app. These generic curves never enter history storage.
    pub fn append_polylines(
        &mut self,
        lines: &[crate::CelestialPolyline<'_>],
    ) -> Result<(), RenderPreparationError> {
        let result = (|| {
            self.validate()?;
            crate::celestial_lines::prepare_polylines(
                self.view,
                self.projection,
                lines,
                self.staging.line_style,
                &mut self.staging.polylines,
            )
        })();
        match result {
            Ok(report) => {
                self.report.polyline_segments += report.segments;
                self.report.max_projected_error_pixels = self
                    .report
                    .max_projected_error_pixels
                    .max(report.max_projected_error_pixels);
                Ok(())
            }
            Err(error) => {
                self.failed = true;
                Err(error)
            }
        }
    }
    fn lines_checked(
        &mut self,
        source: FrameId,
        lines: &[DebugLine],
    ) -> Result<(), RenderPreparationError> {
        self.validate()?;
        let prepared = self.view.prepare_source(source)?;
        let style = self.staging.line_style;
        for line in lines {
            if !line.color.iter().all(|c| c.is_finite()) {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let points = [
                prepared.view_displacement(line.endpoints[0])?.metres(),
                prepared.view_displacement(line.endpoints[1])?.metres(),
            ];
            let mut report = crate::PolylinePreparationReport::default();
            crate::celestial_lines::emit_polyline(
                self.projection,
                &points,
                &[line.color; 2],
                AXIS_LINE_WIDTH_PX * style.width,
                style.opacity,
                crate::CelestialLineStyle::Solid,
                &mut self.staging.polylines,
                &mut report,
            )?;
            self.report.trail_segments += report.segments;
            self.report.max_projected_error_pixels = self
                .report
                .max_projected_error_pixels
                .max(report.max_projected_error_pixels);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), RenderPreparationError> {
        if self.failed {
            Err(RenderPreparationError::FailedDebugFrame)
        } else {
            Ok(())
        }
    }
}
/// Width of axis and history lines in physical pixels.
const AXIS_LINE_WIDTH_PX: f32 = 1.5;
fn pack(position: [f32; 3], normal: [f32; 3], bytes: &mut Vec<u8>, normal_w: f32) {
    for v in [
        position[0],
        position[1],
        position[2],
        1.0,
        normal[0],
        normal[1],
        normal[2],
        normal_w,
    ] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}

/// Shadow cascade state of the latest frame, for snapshots and the Studio.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShadowReport {
    pub cascades: u32,
    pub casters: [u32; 4],
    pub splits_m: [f32; 4],
    pub texel_m: [f32; 4],
}

pub(crate) struct CelestialRenderer {
    sky: crate::sky::SkyRenderer,
    sphere_pipeline: crate::aa::MsaaPipeline,
    polyline_pipeline: wgpu::RenderPipeline,
    projection: wgpu::Buffer,
    projection_layout: wgpu::BindGroupLayout,
    projection_group: wgpu::BindGroup,
    uniform_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    uniform_capacity: u64,
    vertices: wgpu::Buffer,
    vertex_capacity: u64,
    polylines: wgpu::Buffer,
    polyline_capacity: u64,
    indices: wgpu::Buffer,
    terrain_atlas: Option<crate::terrain_atlas::TerrainAtlasRenderer>,
    post: crate::post::PostProcess,
    shadows: crate::shadows::ShadowMaps,
    lighting_layout: wgpu::BindGroupLayout,
    lighting: wgpu::Buffer,
    lighting_group: wgpu::BindGroup,
    settings: crate::RenderSettings,
    size: [u32; 2],
    shadow_report: ShadowReport,
    /// Sample counts the adapter supports (bit mask, crate::aa).
    msaa_support: u32,
    /// Main-pass samples in use.
    samples: u32,
    aa_report: AaReport,
}

/// Anti-aliasing state for diagnostics: what the setting asks for, what the
/// adapter allows, and the one-time pipeline compile cost of the last switch.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AaReport {
    pub requested_samples: u32,
    pub samples: u32,
    pub supported_mask: u32,
    pub fxaa: bool,
    /// Wall time spent compiling pipeline variants at the last sample-count switch.
    pub last_compile_ms: f64,
}
impl CelestialRenderer {
    pub(crate) fn last_sky_resource_report(&self) -> crate::sky::SkyResourceReport {
        self.sky.report()
    }
    pub(crate) fn shadow_report(&self) -> ShadowReport {
        self.shadow_report
    }
    pub(crate) fn on_submitted(&mut self) {
        if let Some(atlas) = &mut self.terrain_atlas {
            atlas.on_submitted();
        }
    }
    pub(crate) fn take_ready_sources(&mut self) -> Vec<(u64, Option<String>)> {
        self.terrain_atlas
            .as_mut()
            .map_or_else(Vec::new, |atlas| atlas.take_ready_sources())
    }
    pub(crate) fn take_world_fields(&mut self) -> Vec<crate::AtlasWorldFields> {
        self.terrain_atlas
            .as_mut()
            .map_or_else(Vec::new, |atlas| atlas.take_world_fields())
    }
    pub(crate) fn set_world_rivers(&mut self, source: u64, words: Vec<u32>) {
        if let Some(atlas) = &mut self.terrain_atlas {
            atlas.set_world_rivers(source, words);
        }
    }
    pub(crate) fn take_collision_pages(&mut self) -> Vec<crate::AtlasCollisionPage> {
        self.terrain_atlas
            .as_mut()
            .map_or_else(Vec::new, |atlas| atlas.take_collision_pages())
    }
    pub(crate) fn take_atlas_bounds(&mut self) -> Vec<crate::AtlasBounds> {
        self.terrain_atlas
            .as_mut()
            .map_or_else(Vec::new, |atlas| atlas.take_bounds())
    }
    pub(crate) fn atlas_report(&self) -> crate::TerrainAtlasReport {
        self.terrain_atlas
            .as_ref()
            .map_or_else(Default::default, |atlas| atlas.report())
    }
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        msaa_support: u32,
    ) -> Self {
        let settings = crate::RenderSettings::default();
        let projection = buffer(
            device,
            64,
            wgpu::BufferUsages::UNIFORM,
            "Celestial projection",
        );
        let projection_layout = layout(device, 64, false, wgpu::ShaderStages::VERTEX);
        let projection_group = binding(device, &projection_layout, &projection, 64);
        let uniform_layout = layout(
            device,
            DRAW_UNIFORM_BYTES,
            true,
            wgpu::ShaderStages::FRAGMENT,
        );
        let uniforms = buffer(
            device,
            256,
            wgpu::BufferUsages::UNIFORM,
            "Celestial per-draw material",
        );
        let uniform_group = binding(device, &uniform_layout, &uniforms, DRAW_UNIFORM_BYTES);
        let lighting_layout = lighting_layout(device);
        let lighting = buffer(
            device,
            crate::lighting::LIGHTING_BYTES as u64,
            wgpu::BufferUsages::UNIFORM,
            "Scene lighting",
        );
        let post = crate::post::PostProcess::new(device, queue, output_format);
        let shadows = crate::shadows::ShadowMaps::new(device, settings.shadows.resolution);
        let lighting_group = lighting_group(device, &lighting_layout, &lighting, &post, &shadows);
        let sphere_pipeline = scene_pipeline(
            device,
            &[&projection_layout, &uniform_layout, &lighting_layout],
            concat!(
                include_str!("shaders/lighting.wgsl"),
                include_str!("shaders/celestial.wgsl")
            ),
        );
        let polyline_pipeline = overlay_pipeline(device, output_format);
        let indices = buffer(
            device,
            1280 * 3 * 4,
            wgpu::BufferUsages::INDEX,
            "Reusable icosphere indices",
        );
        let mut bytes = Vec::with_capacity(1280 * 3 * 4);
        for index in Icosphere::new().indices {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        queue.write_buffer(&indices, 0, &bytes);
        let mut renderer = Self {
            sky: crate::sky::SkyRenderer::new(device, &crate::post::SCENE_TARGETS),
            sphere_pipeline,
            polyline_pipeline,
            projection,
            projection_layout,
            projection_group,
            uniform_layout,
            uniforms,
            uniform_group,
            uniform_capacity: 256,
            vertices: buffer(device, 32, wgpu::BufferUsages::VERTEX, "Celestial vertices"),
            vertex_capacity: 32,
            polylines: buffer(
                device,
                48,
                wgpu::BufferUsages::VERTEX,
                "Celestial styled curves",
            ),
            polyline_capacity: 48,
            indices,
            terrain_atlas: None,
            post,
            shadows,
            lighting_layout,
            lighting,
            lighting_group,
            settings,
            size: [width.max(1), height.max(1)],
            shadow_report: ShadowReport::default(),
            msaa_support,
            samples: 1,
            aa_report: AaReport::default(),
        };
        renderer.apply_samples(device);
        renderer.ensure_post(device);
        renderer
    }
    /// Selects the main-pass sample count from the anti-aliasing setting and
    /// the adapter, compiling pipeline variants on first use.
    fn apply_samples(&mut self, device: &wgpu::Device) {
        let requested = self.settings.anti_aliasing.samples();
        let samples = crate::aa::clamp_samples(requested, self.msaa_support);
        if samples != self.samples {
            let started = std::time::Instant::now();
            self.sky.set_samples(device, samples);
            self.sphere_pipeline.set_samples(device, samples);
            if let Some(atlas) = &mut self.terrain_atlas {
                atlas.set_samples(device, samples);
            }
            self.samples = samples;
            self.aa_report.last_compile_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        self.aa_report.requested_samples = requested;
        self.aa_report.samples = samples;
        self.aa_report.supported_mask = self.msaa_support;
        self.aa_report.fxaa = self.settings.anti_aliasing.fxaa();
    }
    fn ensure_post(&mut self, device: &wgpu::Device) {
        self.post.ensure(
            device,
            self.size,
            self.settings.ao.half_res,
            self.samples,
            self.settings.anti_aliasing.fxaa(),
        );
    }
    pub(crate) fn aa_report(&self) -> AaReport {
        self.aa_report
    }
    pub(crate) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.size = [width.max(1), height.max(1)];
        self.ensure_post(device);
    }
    /// Applies validated settings; reallocates only what changed.
    pub(crate) fn set_settings(&mut self, device: &wgpu::Device, settings: crate::RenderSettings) {
        if settings.shadows.resolution != self.shadows.resolution() {
            self.shadows = crate::shadows::ShadowMaps::new(device, settings.shadows.resolution);
            self.lighting_group = lighting_group(
                device,
                &self.lighting_layout,
                &self.lighting,
                &self.post,
                &self.shadows,
            );
        }
        self.settings = settings;
        self.apply_samples(device);
        self.ensure_post(device);
    }
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &CelestialFrame<'_, '_, '_>,
        timestamps: Option<&crate::gpu_profile::CelestialQueries>,
    ) -> Result<u32, RenderPreparationError> {
        use crate::gpu_profile::pair;
        frame.validate()?;
        let storage = &frame.staging;
        let settings = self.settings;
        self.ensure_post(device);
        self.sky.upload(device, queue, storage.sky.as_ref());
        if let Some(atlas_frame) = &storage.terrain_atlas
            && let Some(config) = atlas_frame.config
        {
            if self
                .terrain_atlas
                .as_ref()
                .is_none_or(|atlas| atlas.config() != config)
            {
                self.terrain_atlas = Some(
                    crate::terrain_atlas::TerrainAtlasRenderer::new(
                        device,
                        &crate::post::SCENE_TARGETS,
                        &self.projection_layout,
                        &self.lighting_layout,
                        &self.shadows.light_layout,
                        config,
                    )
                    .map_err(RenderPreparationError::TerrainAtlas)?,
                );
                if let Some(atlas) = &mut self.terrain_atlas {
                    atlas.set_samples(device, self.samples);
                }
            }
            if let Some(atlas) = &mut self.terrain_atlas {
                atlas
                    .prepare(device, queue, encoder, atlas_frame)
                    .map_err(RenderPreparationError::TerrainAtlas)?;
            }
        }
        grow(
            device,
            &mut self.vertices,
            &mut self.vertex_capacity,
            storage.vertices.len(),
            wgpu::BufferUsages::VERTEX,
        );
        grow(
            device,
            &mut self.polylines,
            &mut self.polyline_capacity,
            storage.polylines.len(),
            wgpu::BufferUsages::VERTEX,
        );
        if storage.uniforms.len() as u64 > self.uniform_capacity {
            self.uniform_capacity = (storage.uniforms.len() as u64).next_power_of_two();
            self.uniforms = buffer(
                device,
                self.uniform_capacity,
                wgpu::BufferUsages::UNIFORM,
                "Celestial draw uniforms",
            );
            self.uniform_group = binding(
                device,
                &self.uniform_layout,
                &self.uniforms,
                DRAW_UNIFORM_BYTES,
            );
        }
        queue.write_buffer(&self.projection, 0, &frame.projection.gpu_bytes());
        for (buffer, bytes) in [
            (&self.vertices, &storage.vertices),
            (&self.polylines, &storage.polylines),
            (&self.uniforms, &storage.uniforms),
        ] {
            if !bytes.is_empty() {
                queue.write_buffer(buffer, 0, bytes);
            }
        }

        // Lighting and sun shadow cascades of the nearest drawn surface.
        let near = frame.projection.near_m();
        let atlas_frame = storage.terrain_atlas.as_ref();
        let shadow = atlas_frame
            .and_then(|a| a.shadow.as_ref())
            .filter(|_| settings.shadows.enabled && storage.lighting.is_some())
            .filter(|_| !storage.passthrough || storage.view_mode == 11);
        let cascades = shadow.map(|s| s.cascades);
        queue.write_buffer(
            &self.lighting,
            0,
            &crate::lighting::pack_lighting(
                storage.lighting.as_ref(),
                &settings,
                cascades.as_ref(),
                near,
                storage.view_mode,
            ),
        );
        let mut scope_mask = 0u32;
        self.shadow_report = ShadowReport::default();
        if let (Some(shadow), Some(atlas)) = (shadow, self.terrain_atlas.as_mut()) {
            let cascades = shadow.cascades;
            let count = (cascades.count as usize).min(4);
            let lists = &shadow.casters[..count];
            atlas.prepare_shadows(device, queue, lists);
            self.shadows.write_projections(queue, &cascades);
            for c in 0..count {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Sun shadow cascade"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.shadows.layer_views[c],
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: timestamps
                        .and_then(|q| q.pass_writes_partial(pair::SHADOWS, c == 0, c + 1 == count)),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                atlas.draw_shadow(&mut pass, &self.shadows.light_group, c);
            }
            scope_mask |= 1 << pair::SHADOWS;
            self.shadow_report = ShadowReport {
                cascades: cascades.count,
                casters: std::array::from_fn(|c| lists.get(c).map_or(0, |l| l.len() as u32)),
                splits_m: cascades.splits,
                texel_m: cascades.texel_m,
            };
        }

        let Some(([direct, normal, ambient], depth)) = self.post.scene_views() else {
            return Err(RenderPreparationError::InvalidProjection);
        };
        let clear = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Celestial scene: terrain, bodies, and sky"),
            color_attachments: &[clear(direct), clear(normal), clear(ambient)],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps.map(|queries| queries.pass_writes(pair::SCENE)),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let [x, y] = frame.projection.origin();
        let [w, h] = frame.projection.viewport();
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_scissor_rect(x, y, w, h);
        let inside = timestamps.filter(|q| q.inside_passes());
        if let Some(sky) = &storage.sky {
            let drawn = sky.report().stars_drawn || sky.report().background_drawn;
            if drawn && let Some(queries) = inside {
                queries.write_scope(&mut pass, pair::SKY);
            }
            self.sky.draw(&mut pass, sky);
            if drawn && let Some(queries) = inside {
                queries.end_scope(&mut pass, pair::SKY);
                scope_mask |= 1 << pair::SKY;
            }
        }
        if !storage.draws.is_empty() {
            if let Some(queries) = inside {
                queries.write_scope(&mut pass, pair::SPHERES);
            }
            for (i, &start) in storage.draws.iter().enumerate() {
                self.bind_sphere_state(&mut pass, i as u32, start);
                pass.draw_indexed(0..3840, 0, 0..1);
            }
            if let Some(queries) = inside {
                queries.end_scope(&mut pass, pair::SPHERES);
                scope_mask |= 1 << pair::SPHERES;
            }
        }
        if storage.terrain_atlas.is_some()
            && let Some(atlas) = &self.terrain_atlas
        {
            if let Some(queries) = inside {
                queries.write_scope(&mut pass, pair::TERRAIN);
            }
            atlas.draw(&mut pass, &self.projection_group, &self.lighting_group);
            if let Some(queries) = inside {
                queries.end_scope(&mut pass, pair::TERRAIN);
                scope_mask |= 1 << pair::TERRAIN;
            }
        }
        drop(pass);
        scope_mask |= 1 << pair::SCENE;

        scope_mask |= self.post.encode(
            queue,
            encoder,
            view,
            &settings,
            crate::post::PostFrame {
                near_m: near,
                focal_px: frame.projection.focal_pixels(),
                view_mode: storage.view_mode,
                passthrough: storage.passthrough,
            },
            timestamps,
        );

        if !storage.polylines.is_empty()
            && let Some(depth) = self.post.depth_view()
        {
            let mut overlays = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Celestial guides and overlays"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamps.map(|queries| queries.pass_writes(pair::OVERLAY)),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            overlays.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
            overlays.set_scissor_rect(x, y, w, h);
            overlays.set_pipeline(&self.polyline_pipeline);
            overlays.set_vertex_buffer(0, self.polylines.slice(..));
            overlays.draw(
                0..(storage.polylines.len() / crate::celestial_lines::LINE_VERTEX_BYTES) as u32,
                0..1,
            );
            scope_mask |= 1 << pair::OVERLAY;
        }
        Ok(scope_mask)
    }

    // Each section owns its complete draw state. In particular, a new pass has
    // no bindings.
    fn bind_sphere_state(&self, pass: &mut wgpu::RenderPass<'_>, draw: u32, start: u32) {
        pass.set_pipeline(self.sphere_pipeline.get());
        pass.set_bind_group(0, &self.projection_group, &[]);
        pass.set_bind_group(1, &self.uniform_group, &[draw * 256]);
        pass.set_bind_group(2, &self.lighting_group, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_vertex_buffer(
            0,
            self.vertices
                .slice(start as u64 * 32..(start as u64 + 642) * 32),
        );
    }
}
/// Per-draw sphere uniform: albedo/colour, flags, emission.
const DRAW_UNIFORM_BYTES: u64 = 48;
pub(crate) fn lighting_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let fragment = wgpu::ShaderStages::FRAGMENT;
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Scene lighting"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: fragment,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(crate::lighting::LIGHTING_BYTES as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: fragment,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: fragment,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: fragment,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
        ],
    })
}
fn lighting_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    lighting: &wgpu::Buffer,
    post: &crate::post::PostProcess,
    shadows: &crate::shadows::ShadowMaps,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Scene lighting"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lighting.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: post.exposure_buffer().as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&shadows.array_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&shadows.compare_sampler),
            },
        ],
    })
}
fn buffer(
    device: &wgpu::Device,
    size: u64,
    usage: wgpu::BufferUsages,
    label: &str,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
fn grow(
    device: &wgpu::Device,
    buf: &mut wgpu::Buffer,
    capacity: &mut u64,
    bytes: usize,
    usage: wgpu::BufferUsages,
) {
    if bytes as u64 > *capacity {
        *capacity = (bytes as u64).next_power_of_two();
        *buf = buffer(
            device,
            *capacity,
            usage,
            "Celestial reusable staging buffer",
        );
    }
}
fn layout(
    device: &wgpu::Device,
    size: u64,
    dynamic: bool,
    visibility: wgpu::ShaderStages,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Celestial uniform layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        }],
    })
}
fn binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    size: u64,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Celestial uniform binding"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size),
            }),
        }],
    })
}
fn scene_pipeline(
    device: &wgpu::Device,
    layouts: &[&wgpu::BindGroupLayout],
    source: &str,
) -> crate::aa::MsaaPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Celestial sphere pipeline layout"),
        bind_group_layouts: &layouts.iter().copied().map(Some).collect::<Vec<_>>(),
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Observer-relative lit spheres"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    // One variant per MSAA sample count (crate::aa).
    crate::aa::MsaaPipeline::new(device, move |device, samples| {
        let targets: Vec<_> = crate::post::SCENE_TARGETS
            .iter()
            .map(|&format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Celestial spheres reverse-Z pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::post::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::aa::multisample(samples),
            multiview_mask: None,
            cache: None,
        })
    })
}
/// Anti-aliased guide quads over the tonemapped image, depth-tested against
/// the scene so bodies occlude the orbits behind them.
fn overlay_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Celestial guide pipeline layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Anti-aliased guide lines"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/celestial_lines.wgsl").into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Celestial anti-aliased guides"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: crate::celestial_lines::LINE_VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: crate::post::DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}
