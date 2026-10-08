//! Explicit post-frame accuracy sampling of the actual retained regional draw.
//!
//! This CPU pass is intentionally opt-in and separate from frame timing. Its
//! fixed 9x9 sample grid describes sampled visible error, not a whole-view max.

use glam::{DVec2, DVec3};
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_renderer::{
    CelestialProjection,
    regional_resident::{RegionalResidentDraw, reconstruct_patch_node},
};
use mundaris_world::terrain::SurfaceGenerator;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Instant};

const SIDE: usize = 9;
const SAMPLE_COUNT: usize = SIDE * SIDE;
const MAX_ACTIVE_PATCHES: usize = 16_384;
const MAX_RAY_CANDIDATES: usize = 256;
const MAX_RECONSTRUCTED_PATCHES: usize = 4_096;
const MAX_TRIANGLE_TESTS: u64 = 50_000_000;
const MAX_RECONSTRUCTED_NODES: u64 = 5_000_000;
const MAX_RETAINED_MESH_BYTES: usize = 256 * 1024 * 1024;
const MAX_CELLS: u32 = 32;

#[derive(Clone, Copy)]
struct Vertex {
    view: DVec3,
    normal_body: DVec3,
    material: [f64; 4],
}

struct PatchMesh {
    patch_index: usize,
    vertices: Vec<Vertex>,
}

/// Compare actual retained regional triangles against complete generator
/// authority at 81 fixed physical-pixel-center rays. Run only after timed frames.
/// Inputs are clones of the renderer's last-submitted draw and active subset.
pub fn sample_visible_terrain_accuracy(
    draw: RegionalResidentDraw,
    active_patch_indices: Vec<usize>,
    fallback_active: bool,
    projection: CelestialProjection,
    generator: SurfaceGenerator,
    sample_interval_s: f64,
) -> Value {
    let started = Instant::now();
    if !sample_interval_s.is_finite() || sample_interval_s <= 0.0 {
        return failure("invalid_sample_interval", started);
    }
    if draw.cells == 0 || draw.cells > MAX_CELLS {
        return failure("cell_count_exceeds_accuracy_cap", started);
    }
    if active_patch_indices.is_empty() || active_patch_indices.len() > MAX_ACTIVE_PATCHES {
        return failure("active_patch_count_out_of_range", started);
    }
    if active_patch_indices
        .iter()
        .any(|index| *index >= draw.patches.len())
    {
        return failure("active_patch_index_out_of_range", started);
    }

    let viewport = projection.viewport();
    let origin = projection.origin();
    let focal = projection.focal_pixels();
    let rays: Vec<DVec3> = (0..SAMPLE_COUNT)
        .map(|index| {
            let x = index % SIDE;
            let y = index / SIDE;
            projection
                .unproject_ray([
                    f64::from(origin[0]) + (x as f64 + 0.5) * f64::from(viewport[0]) / SIDE as f64,
                    f64::from(origin[1]) + (y as f64 + 0.5) * f64::from(viewport[1]) / SIDE as f64,
                ])
                .map_err(|_| ())
        })
        .collect::<Result<_, _>>()
        .unwrap_or_else(|_| Vec::new());
    if rays.len() != SAMPLE_COUNT {
        return failure("ray_generation_failed", started);
    }

    // All rendered triangles/morphs are convex combinations of own/parent
    // vertices and the explicit edge endpoints. Enclose both complete tiles
    // around their actual view anchors; endpoint norms include corrected edges.
    let mut bounds = Vec::with_capacity(active_patch_indices.len());
    for &patch_index in &active_patch_indices {
        let patch = &draw.patches[patch_index];
        let tile_bound = |tile: &mundaris_renderer::resident_tile::TileData| {
            4.0 * tile.anchor_radius_m / (1u64 << tile.key.address.level()) as f64
                + tile.min_max_radial_offset_m[0]
                    .abs()
                    .max(tile.min_max_radial_offset_m[1].abs())
        };
        let edge_bound = |boundary: &mundaris_renderer::regional_edges::TileBoundary| {
            boundary
                .edges
                .iter()
                .flatten()
                .map(|vertex| vertex.position_local_m.length())
                .fold(0.0f64, f64::max)
        };
        let own_bound = tile_bound(&patch.own.tile)
            .max(edge_bound(&patch.boundary_endpoints.own_coarse))
            .max(edge_bound(&patch.boundary_endpoints.own_fine));
        let parent_bound =
            tile_bound(&patch.parent.tile).max(edge_bound(&patch.boundary_endpoints.parent));
        let own_anchor_delta = (patch.parent.anchor_view_m - patch.own.anchor_view_m).length();
        let bound_radius = (own_bound + own_anchor_delta).max(parent_bound) + 1.0;
        if !bound_radius.is_finite() || bound_radius <= 0.0 {
            return failure("invalid_patch_bound", started);
        }
        bounds.push((patch_index, patch.own.anchor_view_m, bound_radius));
    }

    let cells = draw.cells;
    let side = cells as usize + 1;
    let mut rough_candidates = BTreeSet::new();
    for ray in &rays {
        for (patch_index, center, radius) in &bounds {
            if ray_sphere(*ray, *center, *radius) {
                rough_candidates.insert(*patch_index);
            }
        }
    }
    if rough_candidates.len() > MAX_RECONSTRUCTED_PATCHES {
        return failure_with_work(
            "rough_patch_reconstruction_cap_exceeded",
            started,
            rough_candidates.len(),
            0,
            0,
            0,
            0,
        );
    }

    let mut reconstructed_nodes = 0u64;
    let mut triangle_tests = 0u64;
    let mut retained_mesh_bytes = 0usize;
    let mut peak_mesh_bytes = 0usize;
    let mut meshes: Vec<PatchMesh> = Vec::new();
    let mut aabb_candidate_lists: Vec<Vec<usize>> = vec![Vec::new(); SAMPLE_COUNT];
    for patch_index in rough_candidates.iter().copied() {
        let patch = &draw.patches[patch_index];
        let mut vertices = Vec::with_capacity(side * side);
        let transient_bytes = vertices
            .capacity()
            .saturating_mul(std::mem::size_of::<Vertex>());
        peak_mesh_bytes = peak_mesh_bytes.max(retained_mesh_bytes.saturating_add(transient_bytes));
        if retained_mesh_bytes.saturating_add(transient_bytes) > MAX_RETAINED_MESH_BYTES {
            return failure_with_work(
                "transient_mesh_memory_cap_exceeded",
                started,
                rough_candidates.len(),
                reconstructed_nodes,
                triangle_tests,
                retained_mesh_bytes,
                peak_mesh_bytes,
            );
        }
        let mut minimum = DVec3::splat(f64::INFINITY);
        let mut maximum = DVec3::splat(f64::NEG_INFINITY);
        for y in 0..=cells {
            for x in 0..=cells {
                reconstructed_nodes = reconstructed_nodes.saturating_add(1);
                if reconstructed_nodes > MAX_RECONSTRUCTED_NODES {
                    return failure_with_work(
                        "reconstructed_node_work_cap_exceeded",
                        started,
                        rough_candidates.len(),
                        reconstructed_nodes,
                        triangle_tests,
                        retained_mesh_bytes,
                        peak_mesh_bytes,
                    );
                }
                let reconstructed = match reconstruct_patch_node(patch, [x, y], cells) {
                    Ok(vertex) => vertex,
                    Err(error) => {
                        return failure_with_work(
                            &format!("reconstruction_error:{error}"),
                            started,
                            rough_candidates.len(),
                            reconstructed_nodes,
                            triangle_tests,
                            retained_mesh_bytes,
                            peak_mesh_bytes,
                        );
                    }
                };
                let view = patch.parent.anchor_view_m
                    + patch.parent.body_to_view * reconstructed.position_local_m;
                minimum = minimum.min(view);
                maximum = maximum.max(view);
                vertices.push(Vertex {
                    view,
                    normal_body: reconstructed.normal_varying_body,
                    material: reconstructed.material,
                });
            }
        }
        // Every rendered point is piecewise-linear interpolation of these
        // reconstructed nodes. This exact node AABB encloses every triangle;
        // no chart corners or proxy sphere are used for final ray rejection.
        let hit_rays: Vec<usize> = rays
            .iter()
            .enumerate()
            .filter_map(|(ray_index, ray)| ray_aabb(*ray, minimum, maximum).then_some(ray_index))
            .collect();
        if hit_rays.is_empty() {
            continue;
        }
        let mesh_bytes = vertices
            .capacity()
            .saturating_mul(std::mem::size_of::<Vertex>());
        peak_mesh_bytes = peak_mesh_bytes.max(retained_mesh_bytes.saturating_add(mesh_bytes));
        if meshes.len() >= MAX_RECONSTRUCTED_PATCHES
            || retained_mesh_bytes.saturating_add(mesh_bytes) > MAX_RETAINED_MESH_BYTES
        {
            return failure_with_work(
                "retained_candidate_mesh_cap_exceeded",
                started,
                rough_candidates.len(),
                reconstructed_nodes,
                triangle_tests,
                retained_mesh_bytes,
                peak_mesh_bytes,
            );
        }
        retained_mesh_bytes += mesh_bytes;
        let mesh_index = meshes.len();
        meshes.push(PatchMesh {
            patch_index,
            vertices,
        });
        for ray_index in hit_rays {
            aabb_candidate_lists[ray_index].push(mesh_index);
            if aabb_candidate_lists[ray_index].len() > MAX_RAY_CANDIDATES {
                return failure_with_work(
                    "ray_aabb_candidate_cap_exceeded",
                    started,
                    rough_candidates.len(),
                    reconstructed_nodes,
                    triangle_tests,
                    retained_mesh_bytes,
                    peak_mesh_bytes,
                );
            }
        }
    }

    let first_patch = match active_patch_indices
        .first()
        .and_then(|index| draw.patches.get(*index))
    {
        Some(patch) => patch,
        None => return failure("no_active_patch", started),
    };
    let parent_anchor_body = match first_patch.parent.tile.anchor_position_body() {
        Ok(anchor) => anchor,
        Err(error) => return failure(&format!("invalid_parent_anchor:{error}"), started),
    };
    let body_center_view =
        first_patch.parent.anchor_view_m - first_patch.parent.body_to_view * parent_anchor_body;
    let body_to_view = first_patch.parent.body_to_view;
    let body_to_view_inv = body_to_view.transpose();

    let mut samples = Vec::with_capacity(SAMPLE_COUNT);
    let mut represented = 0usize;
    let mut certified_sky = 0usize;
    let mut projected_count = 0usize;
    let mut radial_abs_sum = 0.0;
    let mut radial_abs_max: f64 = 0.0;
    let mut projected_sum = 0.0;
    let mut projected_max: f64 = 0.0;
    let mut slope_sum = 0.0;
    let mut normal_sum = 0.0;
    let mut material_l1_sum = 0.0;
    for sample_index in 0..SAMPLE_COUNT {
        let ray = rays[sample_index];
        let mut nearest: Option<(f64, usize, DVec3, DVec3, [f64; 4])> = None;
        for mesh_index in &aabb_candidate_lists[sample_index] {
            let Some(mesh) = meshes.get(*mesh_index) else {
                continue;
            };
            let vertices = &mesh.vertices;
            for y in 0..cells as usize {
                for x in 0..cells as usize {
                    let a = y * side + x;
                    let b = a + 1;
                    let c = a + side;
                    let d = c + 1;
                    for indices in [[a, b, c], [b, d, c]] {
                        triangle_tests = triangle_tests.saturating_add(1);
                        if triangle_tests > MAX_TRIANGLE_TESTS {
                            return failure_with_work(
                                "triangle_intersection_work_cap_exceeded",
                                started,
                                rough_candidates.len(),
                                reconstructed_nodes,
                                triangle_tests,
                                retained_mesh_bytes,
                                peak_mesh_bytes,
                            );
                        }
                        let v0 = vertices[indices[0]];
                        let v1 = vertices[indices[1]];
                        let v2 = vertices[indices[2]];
                        if let Some((distance, bary)) = ray_triangle(ray, v0.view, v1.view, v2.view)
                            && distance > projection.near_m()
                            && nearest.as_ref().is_none_or(|current| distance < current.0)
                        {
                            let interpolate = |field: fn(Vertex) -> DVec3| {
                                field(v0) * bary[0] + field(v1) * bary[1] + field(v2) * bary[2]
                            };
                            let normal =
                                interpolate(|vertex| vertex.normal_body).normalize_or_zero();
                            let material: [f64; 4] = std::array::from_fn(|i| {
                                v0.material[i] * bary[0]
                                    + v1.material[i] * bary[1]
                                    + v2.material[i] * bary[2]
                            });
                            nearest = Some((
                                distance,
                                mesh.patch_index,
                                ray * distance,
                                normal,
                                material,
                            ));
                        }
                    }
                }
            }
        }
        let Some((distance, patch_index, view_hit, drawn_normal, drawn_material)) = nearest else {
            if !ray_sphere(
                ray,
                body_center_view,
                generator.conservative_radius_envelope_m()[1],
            ) {
                certified_sky += 1;
                samples.push(json!({"index":sample_index,"status":"outside_complete_source_envelope","weight":1.0/SAMPLE_COUNT as f64,"projected_error_pixels":0.0}));
                continue;
            }
            samples.push(json!({"index":sample_index,"status":"unrepresented","weight":1.0/SAMPLE_COUNT as f64,"error":null}));
            continue;
        };
        let body_hit = body_to_view_inv * (view_hit - body_center_view);
        let direction = match Direction3::try_new(body_hit) {
            Ok(direction) => direction,
            Err(_) => {
                samples.push(json!({"index":sample_index,"status":"source_error","error":"invalid_hit_direction"}));
                continue;
            }
        };
        let location = SurfaceLocation::new(direction);
        let source = match generator.evaluate_point(location) {
            Ok(source) => source,
            Err(error) => {
                samples.push(
                    json!({"index":sample_index,"status":"source_error","error":error.to_string()}),
                );
                continue;
            }
        };
        let radial_error = body_hit.length() - source.radius_m();
        let source_body = source.position(location);
        let source_view = body_center_view + body_to_view * source_body;
        let pixel_error = match (
            project(view_hit, origin, viewport, focal, projection.near_m()),
            project(source_view, origin, viewport, focal, projection.near_m()),
        ) {
            (Some(draw_px), Some(source_px)) => Some(draw_px.distance(source_px)),
            _ => None,
        };
        let source_normal = source.normal().normalize_or_zero();
        let normal_error = drawn_normal.dot(source_normal).clamp(-1.0, 1.0).acos();
        let radial_direction = direction.unit();
        let drawn_slope = drawn_normal.dot(radial_direction).clamp(-1.0, 1.0).acos();
        let source_slope = source_normal.dot(radial_direction).clamp(-1.0, 1.0).acos();
        let slope_error = (drawn_slope - source_slope).abs();
        let source_material = source.material_weights();
        let material_l1: f64 = drawn_material
            .iter()
            .zip(source_material)
            .map(|(a, b)| (a - b).abs())
            .sum();
        represented += 1;
        radial_abs_sum += radial_error.abs();
        radial_abs_max = radial_abs_max.max(radial_error.abs());
        slope_sum += slope_error;
        normal_sum += normal_error;
        material_l1_sum += material_l1;
        if let Some(pixels) = pixel_error {
            projected_count += 1;
            projected_sum += pixels;
            projected_max = projected_max.max(pixels);
        }
        samples.push(json!({
            "index":sample_index,"status":"hit","weight":1.0/SAMPLE_COUNT as f64,
            "patch_index":patch_index,"ray_distance_m":distance,"direction_body":direction.unit().to_array(),
            "drawn_radius_m":body_hit.length(),"source_radius_m":source.radius_m(),
            "radial_error_m":radial_error,"absolute_radial_error_m":radial_error.abs(),
            "projected_error_pixels":pixel_error,
            "drawn_slope_rad":drawn_slope,"source_slope_rad":source_slope,"slope_error_rad":slope_error,
            "normal_error_rad":normal_error,"drawn_material_weights":drawn_material,
            "source_material_weights":source_material,"material_l1_error":material_l1
        }));
    }
    let unrepresented = SAMPLE_COUNT - represented - certified_sky;
    let complete = unrepresented == 0;
    json!({
        "schema":"mundaris.visible_terrain_accuracy.v1",
        "status":if complete {"complete_sample_grid"} else {"incomplete_sample_grid"},
        "complete":complete,"scope":"81 fixed weighted rays against actual retained regional triangle geometry",
        "whole_view_max_error_certified":false,
        "retained_draw":{"cells":draw.cells,"active_patch_count":active_patch_indices.len(),"fallback_active":fallback_active},
        "candidate_work":{"broadphase":"conservative patch spheres; final rejection uses the AABB of every CPU-reconstructed retained node","rough_sphere_candidate_patches":rough_candidates.len(),"reconstructed_patches":rough_candidates.len(),"aabb_retained_candidate_patches":meshes.len(),"reconstructed_nodes":reconstructed_nodes,"triangle_tests":triangle_tests,"retained_vertex_payload_bytes":retained_mesh_bytes,"peak_retained_plus_transient_vertex_payload_bytes":peak_mesh_bytes,"caps":{"rough_patches":MAX_RECONSTRUCTED_PATCHES,"reconstructed_nodes":MAX_RECONSTRUCTED_NODES,"ray_aabb_candidates":MAX_RAY_CANDIDATES,"triangle_tests":MAX_TRIANGLE_TESTS,"retained_plus_transient_vertex_payload_bytes":MAX_RETAINED_MESH_BYTES}},
        "grid":{"width":SIDE,"height":SIDE,"sample_count":SAMPLE_COUNT,"weight_per_sample":1.0/SAMPLE_COUNT as f64,"sample_interval_s":sample_interval_s},
        "coverage":{"represented_samples":represented,"certified_sky_samples":certified_sky,"unrepresented_samples":unrepresented,"represented_weight":represented as f64/SAMPLE_COUNT as f64},
        "summary":{"mean_absolute_radial_error_m":(represented>0).then_some(radial_abs_sum/represented as f64),"sampled_max_absolute_radial_error_m":(represented>0).then_some(radial_abs_max),"mean_projected_error_pixels_among_projected_hits":(projected_count>0).then_some(projected_sum/projected_count as f64),"sampled_max_projected_error_pixels":(projected_count>0).then_some(projected_max),"sampled_projected_pixel_seconds":(complete && projected_count + certified_sky == SAMPLE_COUNT).then_some(projected_sum/SAMPLE_COUNT as f64*sample_interval_s),"known_sample_weighted_projected_error_pixels":projected_sum/SAMPLE_COUNT as f64,"unknown_weight":unrepresented as f64/SAMPLE_COUNT as f64,"projected_hits":projected_count,"mean_slope_error_rad":(represented>0).then_some(slope_sum/represented as f64),"mean_normal_error_rad":(represented>0).then_some(normal_sum/represented as f64),"mean_material_l1_error":(represented>0).then_some(material_l1_sum/represented as f64)},
        "fidelity":{"reconstruction":"cpu_reconstruction_f64","gpu_parity":"not_bit_identical; shader storage and interpolation narrow to f32","framebuffer":"selected-body regional mesh only; excludes compositing/occlusion by other scene objects"},
        "audit_compute_ms":started.elapsed().as_secs_f64()*1000.0,
        "samples":samples
    })
}

fn ray_sphere(ray: DVec3, center: DVec3, radius: f64) -> bool {
    let along = center.dot(ray);
    if along + radius <= 0.0 {
        return false;
    }
    let closest_sq = (center.length_squared() - along * along).max(0.0);
    closest_sq <= radius * radius
}

/// Forward ray versus the exact AABB of every reconstructed node in a patch.
/// The node box encloses all triangles because each triangle is linear between
/// its three vertices. Origins inside the box return true.
fn ray_aabb(ray: DVec3, minimum: DVec3, maximum: DVec3) -> bool {
    let mut near_t: f64 = 0.0;
    let mut far_t = f64::INFINITY;
    for axis in 0..3 {
        let origin = 0.0;
        let direction = ray[axis];
        let min = minimum[axis];
        let max = maximum[axis];
        if direction.abs() <= f64::EPSILON {
            if origin < min || origin > max {
                return false;
            }
            continue;
        }
        let a = min / direction;
        let b = max / direction;
        near_t = near_t.max(a.min(b));
        far_t = far_t.min(a.max(b));
        if far_t < near_t {
            return false;
        }
    }
    far_t >= 0.0
}

fn ray_triangle(ray: DVec3, a: DVec3, b: DVec3, c: DVec3) -> Option<(f64, [f64; 3])> {
    let edge1 = b - a;
    let edge2 = c - a;
    let p = ray.cross(edge2);
    let determinant = edge1.dot(p);
    if determinant.abs() <= 1.0e-12 {
        return None;
    }
    let inverse = determinant.recip();
    let tvec = -a;
    let u = tvec.dot(p) * inverse;
    if !(-1.0e-10..=1.0 + 1.0e-10).contains(&u) {
        return None;
    }
    let q = tvec.cross(edge1);
    let v = ray.dot(q) * inverse;
    if v < -1.0e-10 || u + v > 1.0 + 1.0e-10 {
        return None;
    }
    let distance = edge2.dot(q) * inverse;
    (distance.is_finite() && distance > 0.0).then_some((distance, [1.0 - u - v, u, v]))
}

fn project(
    point: DVec3,
    origin: [u32; 2],
    viewport: [u32; 2],
    focal: f64,
    near: f64,
) -> Option<DVec2> {
    let depth = -point.z;
    if !point.is_finite() || depth < near {
        return None;
    }
    Some(DVec2::new(
        f64::from(origin[0]) + f64::from(viewport[0]) * 0.5 + focal * point.x / depth,
        f64::from(origin[1]) + f64::from(viewport[1]) * 0.5 - focal * point.y / depth,
    ))
}

fn failure(reason: &str, started: Instant) -> Value {
    json!({"schema":"mundaris.visible_terrain_accuracy.v1","status":"error","complete":false,"whole_view_max_error_certified":false,"error":reason,"audit_compute_ms":started.elapsed().as_secs_f64()*1000.0})
}

fn failure_with_work(
    reason: &str,
    started: Instant,
    rough_candidate_patches: usize,
    reconstructed_nodes: u64,
    triangle_tests: u64,
    retained_mesh_bytes: usize,
    peak_mesh_bytes: usize,
) -> Value {
    json!({
        "schema":"mundaris.visible_terrain_accuracy.v1",
        "status":"truncated",
        "complete":false,
        "whole_view_max_error_certified":false,
        "error":reason,
        "candidate_work":{
            "rough_sphere_candidate_patches":rough_candidate_patches,
            "reconstructed_nodes":reconstructed_nodes,
            "triangle_tests":triangle_tests,
            "retained_vertex_payload_bytes":retained_mesh_bytes,
            "peak_retained_plus_transient_vertex_payload_bytes":peak_mesh_bytes,
            "caps":{
                "reconstructed_patches":MAX_RECONSTRUCTED_PATCHES,
                "reconstructed_nodes":MAX_RECONSTRUCTED_NODES,
                "ray_aabb_candidates":MAX_RAY_CANDIDATES,
                "triangle_tests":MAX_TRIANGLE_TESTS,
                "retained_plus_transient_vertex_payload_bytes":MAX_RETAINED_MESH_BYTES
            }
        },
        "audit_compute_ms":started.elapsed().as_secs_f64()*1000.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_hits_preserve_interpolated_attributes_and_nearest_depth() {
        let ray = -DVec3::Z;
        let triangle = [
            DVec3::new(-1.0, -1.0, -2.0),
            DVec3::new(1.0, -1.0, -2.0),
            DVec3::new(0.0, 1.0, -2.0),
        ];
        let (distance, bary) = ray_triangle(ray, triangle[0], triangle[1], triangle[2]).unwrap();
        assert!((distance - 2.0).abs() < 1e-12);
        assert!((bary.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        let hit = triangle[0] * bary[0] + triangle[1] * bary[1] + triangle[2] * bary[2];
        assert!((hit - ray * distance).length() < 1e-12);
        assert!(ray_triangle(ray, -triangle[0], -triangle[1], -triangle[2]).is_none());
        assert!(ray_triangle(ray, triangle[0], triangle[0], triangle[2]).is_none());
    }

    #[test]
    fn conservative_sphere_test_keeps_inside_observers_and_rejects_clear_sky() {
        assert!(ray_sphere(-DVec3::Z, DVec3::ZERO, 2.0));
        assert!(ray_sphere(-DVec3::Z, DVec3::new(0.0, 0.0, -4.0), 2.0));
        assert!(!ray_sphere(-DVec3::Z, DVec3::new(0.0, 0.0, 4.0), 2.0));
        assert!(!ray_sphere(-DVec3::Z, DVec3::new(4.0, 0.0, -4.0), 2.0));
    }

    #[test]
    fn ray_aabb_handles_inside_parallel_and_miss_cases() {
        let min = DVec3::new(-1.0, -1.0, -3.0);
        let max = DVec3::new(1.0, 1.0, -2.0);
        assert!(ray_aabb(-DVec3::Z, min, max));
        assert!(ray_aabb(DVec3::X, DVec3::ZERO, DVec3::ONE));
        assert!(!ray_aabb(-DVec3::Z, DVec3::new(2.0, -1.0, -3.0), max));
        assert!(!ray_aabb(DVec3::X, min, max));
    }
}
