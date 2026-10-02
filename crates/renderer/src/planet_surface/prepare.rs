//! CPU f64 evaluation, source subtraction, clipped precision proof and byte layouts.
use super::{ActiveSurfacePatch, GRID_SAMPLES, SurfaceTopology};
use crate::{CelestialProjection, PreparedView, RenderPreparationError};
use glam::DVec3;
use mundaris_math::{FramePosition, LocalPosition, surface::CubeSampleKey};
use std::{collections::BTreeMap, ops::Range};
const STAGING_CAP: usize = 64 * 1024 * 1024;
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfaceStyle {
    pub borders: bool,
    pub lod_colors: bool,
    pub face_colors: bool,
    pub underside: bool,
}
#[derive(Debug, Default, Clone, Copy)]
pub struct SurfacePreparationReport {
    pub patches: usize,
    pub samples: usize,
    pub triangles: usize,
    pub fallback_triangles: usize,
    pub draws: usize,
    pub uploaded_bytes: usize,
    pub allocated_staging_bytes: usize,
    pub boundary_bytes: usize,
    pub max_component_error_m: f64,
    pub max_projected_error_pixels: f64,
    pub max_gpu_projection_error_pixels: f64,
}
#[derive(Default)]
pub(crate) struct SurfaceStaging {
    pub samples: Vec<u8>,
    pub instances: Vec<u8>,
    pub fallback: Vec<u8>,
    pub buckets: [Range<u32>; 16],
    boundary: BTreeMap<CubeSampleKey, ([f32; 3], [f32; 3])>,
    pub report: SurfacePreparationReport,
    pub underside: bool,
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
impl SurfaceStaging {
    pub fn clear(&mut self) {
        self.samples.clear();
        self.instances.clear();
        self.fallback.clear();
        self.boundary.clear();
        self.buckets = std::array::from_fn(|_| 0..0);
        self.report = SurfacePreparationReport::default();
        self.underside = false;
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
        // Complete preflight includes pessimistic clipped polygons (up to 8 vertices,
        // 6 triangles per input). Do not grow power-of-two past the aggregate cap.
        let outgoing = self.samples.len() + self.instances.len() + self.fallback.len();
        let regular = patches.len() * (GRID_SAMPLES * 32 + 64);
        if outgoing + regular > STAGING_CAP {
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.underside |= style.underside;
        let prepared = view.prepare_source(source)?;
        let mut positions = [DVec3::ZERO; GRID_SAMPLES];
        let mut normals = [DVec3::ZERO; GRID_SAMPLES];
        let mut gpu_positions = [[0.0; 3]; GRID_SAMPLES];
        let mut gpu_normals = [[0.0; 3]; GRID_SAMPLES];
        // Instance buckets are appended as a complete batch. Rebucket aggregate
        // instances after appending; sample bases are unaffected by their order.
        let mut records: Vec<(u8, [u8; 64])> =
            Vec::with_capacity(self.instances.len() / 64 + patches.len());
        for mask in 0..16 {
            for i in self.buckets[mask].clone() {
                records.push((
                    mask as u8,
                    self.instances[i as usize * 64..(i as usize + 1) * 64]
                        .try_into()
                        .expect("instance stride"),
                ));
            }
        }
        for patch in patches {
            for index in 0..GRID_SAMPLES {
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
                let packed = if i == 0 || i == 16 || j == 0 || j == 16 {
                    *self.boundary.entry(key).or_insert_with(|| {
                        (
                            positions[index].as_vec3().to_array(),
                            normals[index].as_vec3().to_array(),
                        )
                    })
                } else {
                    (
                        positions[index].as_vec3().to_array(),
                        normals[index].as_vec3().to_array(),
                    )
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
            let indices = topology.indices(patch.stitch_mask);
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
            let arithmetic_bound =
                0.5 * (w as f64).hypot(h as f64) * (coefficient_error + 8.0 * f32::EPSILON as f64);
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
                    let gpu = triangle
                        .map(|i| DVec3::from_array(gpu_positions[usize::from(i)].map(f64::from)));
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
            let patch_color = if style.lod_colors {
                let t = patch.address.level() as f32 / 20.0;
                [0.2 + t * 0.6, 0.6 - t * 0.3, 1.0 - t * 0.7, 1.0]
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
            if fallback {
                for triangle in indices.as_chunks::<3>().0 {
                    let points = triangle.map(|i| positions[usize::from(i)]);
                    let polygon = clip_triangle(points, projection);
                    if polygon.len() < 3 {
                        continue;
                    }
                    let needed = (polygon.len() - 2) * 3 * 64;
                    if self.samples.len() + records.len() * 64 + self.fallback.len() + needed
                        > STAGING_CAP
                    {
                        return Err(RenderPreparationError::InvalidBudget);
                    }
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
                                    .chain([normal[0], normal[1], normal[2], 0.0])
                                    .chain(patch_color)
                                    .chain([
                                        uv[0] as f32,
                                        uv[1] as f32,
                                        f32::from(style.borders),
                                        0.0,
                                    ]),
                            );
                        }
                        self.report.fallback_triangles += 1;
                    }
                }
            } else {
                let base = (self.samples.len() / 32) as u32;
                for (p, n) in gpu_positions.into_iter().zip(gpu_normals) {
                    pack_floats(
                        &mut self.samples,
                        [p[0], p[1], p[2], 1.0, n[0], n[1], n[2], 0.0],
                    );
                }
                let mut record = [0; 64];
                for (value, out) in [
                    base,
                    u32::from(patch.address.level()),
                    patch.address.face() as u32,
                    u32::from(style.borders),
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
                records.push((patch.stitch_mask, record));
                self.report.samples += GRID_SAMPLES;
                self.report.triangles += indices.len() / 3;
            }
            self.report.patches += 1;
        }
        records.sort_by_key(|r| r.0);
        self.instances.clear();
        let mut cursor = 0u32;
        for mask in 0..16 {
            let start = cursor;
            for (_, record) in records.iter().filter(|r| r.0 == mask) {
                self.instances.extend_from_slice(record);
                cursor += 1;
            }
            self.buckets[usize::from(mask)] = start..cursor;
        }
        self.report.draws = self.buckets.iter().filter(|r| !r.is_empty()).count()
            + usize::from(!self.fallback.is_empty());
        self.report.uploaded_bytes =
            self.samples.len() + self.instances.len() + self.fallback.len();
        self.report.allocated_staging_bytes =
            self.samples.capacity() + self.instances.capacity() + self.fallback.capacity();
        // BTreeMap nodes have private std layout; use a conservative 128-byte
        // allocation envelope per entry including pointers/alignment, not payload only.
        self.report.boundary_bytes = self.boundary.len() * 128;
        if self.report.allocated_staging_bytes > STAGING_CAP
            || self.report.boundary_bytes > 8 * 1024 * 1024
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        Ok(())
    }
}
fn pack_floats(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = f32>) {
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
}
