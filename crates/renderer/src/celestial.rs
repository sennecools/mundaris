//! Minimal reusable indexed debug spheres and actual-history line preparation.
use crate::{
    CelestialProjection, DebugLine, PreparedView, RenderPreparationError, reference_sphere_occludes,
};
use glam::DVec3;
use mundaris_math::{Direction3, FrameId, FramePosition, LocalPosition};
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
    use mundaris_math::*;
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
    }
    #[test]
    fn generated_surface_missing_geometry_poisons_celestial_frame() {
        use mundaris_math::surface::{CubeFace, CubePatchAddress};
        use std::num::NonZeroU64;
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
        let topology = crate::planet_surface::SurfaceTopology::new();
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let patch = crate::planet_surface::ActiveSurfacePatch {
            address,
            metadata: crate::planet_surface::PatchMetadata::build(address, &topology).unwrap(),
            stitch_mask: 0,
            error_pixels: 0.0,
        };
        let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
        let body = CelestialRenderBody {
            body_fixed_frame: root,
            reference_radius_m: 10.0,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: false,
        };
        let mut staging = CelestialStaging::default();
        let sphere = Icosphere::new();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        assert!(
            frame
                .append_generated_surface(
                    body,
                    &[patch],
                    &[],
                    &topology,
                    crate::planet_surface::SurfaceStyle::default()
                )
                .is_err()
        );
        assert!(matches!(
            frame.validate(),
            Err(RenderPreparationError::FailedDebugFrame)
        ));
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CelestialRenderBody {
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    pub color: [f32; 4],
    pub unlit: bool,
    pub selected: bool,
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
    surface: crate::planet_surface::SurfaceStaging,
    vertices: Vec<u8>,
    uniforms: Vec<u8>,
    lines: Vec<u8>,
    polylines: Vec<u8>,
    draws: Vec<u32>,
    markers: Vec<CelestialMarker>,
    centers: Vec<(DVec3, f64)>,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct CelestialPreparationReport {
    pub surface: crate::planet_surface::SurfacePreparationReport,
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
        staging.lines.clear();
        staging.polylines.clear();
        staging.draws.clear();
        staging.markers.clear();
        staging.centers.clear();
        staging.surface.clear();
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

    /// Sets renderer-only terrain shading for this frame. Directions and terrain
    /// normals use body-fixed axes; this does not change reusable geometry.
    pub fn set_terrain_lighting(&mut self, lighting: crate::planet_surface::TerrainLighting) {
        self.staging.surface.lighting = lighting;
    }

    /// Content projection used for this frame, including viewport and origin.
    pub fn projection(&self) -> CelestialProjection {
        self.projection
    }
    pub fn report(&self) -> CelestialPreparationReport {
        CelestialPreparationReport {
            surface: self.staging.surface.report,
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
                    for (v, out) in body.color.iter().zip(
                        self.staging.uniforms[offset..offset + 16]
                            .as_chunks_mut::<4>()
                            .0,
                    ) {
                        out.copy_from_slice(&v.to_le_bytes());
                    }
                    for (v, out) in [u32::from(body.unlit), u32::from(body.selected), 0, 0]
                        .iter()
                        .zip(
                            self.staging.uniforms[offset + 16..offset + 32]
                                .as_chunks_mut::<4>()
                                .0,
                        )
                    {
                        out.copy_from_slice(&v.to_le_bytes());
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
    pub fn append_surface(
        &mut self,
        body: CelestialRenderBody,
        patches: &[crate::planet_surface::ActiveSurfacePatch],
        topology: &crate::planet_surface::SurfaceTopology,
        style: crate::planet_surface::SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        let result = self.validate().and_then(|()| {
            self.staging
                .surface
                .append(self.view, self.projection, body, patches, topology, style)
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Appends borrowed, pre-generated body-fixed terrain through the same
    /// source-centred precision and clipping path as smooth surface patches.
    pub fn append_generated_surface(
        &mut self,
        body: CelestialRenderBody,
        patches: &[crate::planet_surface::ActiveSurfacePatch],
        geometry: &[&crate::planet_surface::GeneratedSurfacePatch],
        topology: &crate::planet_surface::SurfaceTopology,
        style: crate::planet_surface::SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        let result = self.validate().and_then(|()| {
            self.staging.surface.append_generated(
                self.view,
                self.projection,
                body,
                patches,
                geometry,
                topology,
                style,
            )
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Adaptive terrain accepts only geometry reconciled against a complete
    /// ready cover, so visibility cannot accidentally omit a boundary owner.
    pub fn append_stitched_surface(
        &mut self,
        body: CelestialRenderBody,
        patches: &[crate::planet_surface::ActiveSurfacePatch],
        surface: &crate::planet_surface::StitchedSurface,
        topology: &crate::planet_surface::SurfaceTopology,
        style: crate::planet_surface::SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        let result = self.validate().and_then(|()| {
            self.staging.surface.append_stitched(
                self.view,
                self.projection,
                body,
                patches,
                surface,
                topology,
                style,
            )
        });
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Draws a compatible overlay, not unrelated interpolated grid arrays.
    pub fn append_surface_transition(
        &mut self,
        body: CelestialRenderBody,
        transition: &crate::planet_surface::SurfaceTransition,
        fraction: f64,
        style: crate::planet_surface::SurfaceStyle,
    ) -> Result<(), RenderPreparationError> {
        let result = self.validate().and_then(|()| {
            self.staging.surface.append_transition(
                self.view,
                self.projection,
                body,
                transition,
                fraction,
                style,
            )
        });
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
        for line in lines {
            if !line.color.iter().all(|c| c.is_finite()) {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            let a = prepared.view_displacement(line.endpoints[0])?.metres();
            let b = prepared.view_displacement(line.endpoints[1])?.metres();
            if let Some(clipped) = self.projection.clip_segment([a, b])? {
                let start = self.staging.lines.len();
                let mut outside = false;
                for point in clipped {
                    match self.projection.narrow(point, f64::MAX, self.range_m) {
                        Ok((position, error, pixels)) => {
                            for v in [position[0], position[1], position[2], 1.0]
                                .into_iter()
                                .chain(line.color)
                            {
                                self.staging.lines.extend_from_slice(&v.to_le_bytes());
                            }
                            self.report.max_narrowing_error_m =
                                self.report.max_narrowing_error_m.max(error);
                            self.report.max_projected_error_pixels =
                                self.report.max_projected_error_pixels.max(pixels);
                        }
                        Err(RenderPreparationError::OutsideRenderRange { .. }) => {
                            outside = true;
                            break;
                        }
                        Err(error) => return Err(error),
                    }
                }
                if outside {
                    self.staging.lines.truncate(start);
                } else {
                    self.report.trail_segments += 1;
                }
            }
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

pub(crate) struct CelestialRenderer {
    surface: crate::planet_surface::PlanetSurfaceRenderer,
    sphere_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    polyline_pipeline: wgpu::RenderPipeline,
    projection: wgpu::Buffer,
    projection_group: wgpu::BindGroup,
    uniform_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    uniform_capacity: u64,
    vertices: wgpu::Buffer,
    vertex_capacity: u64,
    lines: wgpu::Buffer,
    line_capacity: u64,
    polylines: wgpu::Buffer,
    polyline_capacity: u64,
    indices: wgpu::Buffer,
    depth: wgpu::TextureView,
}
impl CelestialRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let projection = buffer(
            device,
            64,
            wgpu::BufferUsages::UNIFORM,
            "Celestial projection",
        );
        let projection_layout = layout(device, 64, false, wgpu::ShaderStages::VERTEX);
        let projection_group = binding(device, &projection_layout, &projection, 64);
        let uniform_layout = layout(device, 32, true, wgpu::ShaderStages::FRAGMENT);
        let uniforms = buffer(
            device,
            256,
            wgpu::BufferUsages::UNIFORM,
            "Celestial per-draw color/flags",
        );
        let uniform_group = binding(device, &uniform_layout, &uniforms, 32);
        let sphere_pipeline = pipeline(
            device,
            format,
            &[&projection_layout, &uniform_layout],
            include_str!("shaders/celestial.wgsl"),
            false,
        );
        let line_pipeline = pipeline(
            device,
            format,
            &[&projection_layout],
            include_str!("shaders/celestial_trails.wgsl"),
            true,
        );
        let polyline_pipeline = pipeline(
            device,
            format,
            &[],
            include_str!("shaders/celestial_lines.wgsl"),
            true,
        );
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
        Self {
            surface: crate::planet_surface::PlanetSurfaceRenderer::new(
                device,
                queue,
                format,
                &projection_layout,
            ),
            sphere_pipeline,
            line_pipeline,
            polyline_pipeline,
            projection,
            projection_group,
            uniform_layout,
            uniforms,
            uniform_group,
            uniform_capacity: 256,
            vertices: buffer(device, 32, wgpu::BufferUsages::VERTEX, "Celestial vertices"),
            vertex_capacity: 32,
            lines: buffer(device, 32, wgpu::BufferUsages::VERTEX, "Historical lines"),
            line_capacity: 32,
            polylines: buffer(
                device,
                32,
                wgpu::BufferUsages::VERTEX,
                "Celestial styled curves",
            ),
            polyline_capacity: 32,
            indices,
            depth: depth(device, width, height),
        }
    }
    pub(crate) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        self.depth = depth(device, width, height);
    }
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &CelestialFrame<'_, '_, '_>,
    ) -> Result<(), RenderPreparationError> {
        frame.validate()?;
        let storage = &frame.staging;
        self.surface.upload(device, queue, &storage.surface)?;
        grow(
            device,
            &mut self.vertices,
            &mut self.vertex_capacity,
            storage.vertices.len(),
            wgpu::BufferUsages::VERTEX,
        );
        grow(
            device,
            &mut self.lines,
            &mut self.line_capacity,
            storage.lines.len(),
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
            self.uniform_group = binding(device, &self.uniform_layout, &self.uniforms, 32);
        }
        queue.write_buffer(&self.projection, 0, &frame.projection.gpu_bytes());
        for (buffer, bytes) in [
            (&self.vertices, &storage.vertices),
            (&self.lines, &storage.lines),
            (&self.polylines, &storage.polylines),
            (&self.uniforms, &storage.uniforms),
        ] {
            if !bytes.is_empty() {
                queue.write_buffer(buffer, 0, bytes);
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Debug celestial shading / actual history"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.025,
                        g: 0.035,
                        b: 0.06,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let [x, y] = frame.projection.origin();
        let [w, h] = frame.projection.viewport();
        pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
        pass.set_scissor_rect(x, y, w, h);
        pass.set_pipeline(&self.sphere_pipeline);
        pass.set_bind_group(0, &self.projection_group, &[]);
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for (i, &start) in storage.draws.iter().enumerate() {
            pass.set_bind_group(1, &self.uniform_group, &[(i as u32) * 256]);
            pass.set_vertex_buffer(
                0,
                self.vertices
                    .slice(start as u64 * 32..(start as u64 + 642) * 32),
            );
            pass.draw_indexed(0..3840, 0, 0..1);
        }
        self.surface
            .draw(&mut pass, &self.projection_group, &storage.surface);
        if !storage.lines.is_empty() {
            pass.set_pipeline(&self.line_pipeline);
            pass.set_vertex_buffer(0, self.lines.slice(..));
            pass.draw(0..(storage.lines.len() / 32) as u32, 0..1);
        }
        if !storage.polylines.is_empty() {
            pass.set_pipeline(&self.polyline_pipeline);
            pass.set_vertex_buffer(0, self.polylines.slice(..));
            pass.draw(0..(storage.polylines.len() / 32) as u32, 0..1);
        }
        Ok(())
    }
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
fn depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Celestial infinite reverse-Z"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layouts: &[&wgpu::BindGroupLayout],
    source: &str,
    lines: bool,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Celestial debug pipeline layout"),
        bind_group_layouts: layouts,
        push_constant_ranges: &[],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Observer-relative celestial shader"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Celestial reverse-Z pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: if lines {
                    Some(wgpu::BlendState::ALPHA_BLENDING)
                } else {
                    None
                },
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology: if lines && !layouts.is_empty() {
                wgpu::PrimitiveTopology::LineList
            } else {
                wgpu::PrimitiveTopology::TriangleList
            },
            cull_mode: if lines { None } else { Some(wgpu::Face::Back) },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: !lines,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
