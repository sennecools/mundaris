//! Bounded CPU reference compiler for one finite, charted terrain region.
//!
//! This module owns disposable geometry only. Level indices reference
//! `RegionProduct::input.vertices`; transition indices reference their own
//! common-refinement vertices. Source surfaces stay authoritative elsewhere.

use crate::resident_tile::TileKey;
use glam::{DVec2, DVec3};
use meshopt::{
    SimplifyOptions, VertexDataAdapter, build_meshlets, simplify_with_attributes_and_locks,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    mem::size_of,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

pub const MAX_REGION_VERTICES: usize = 1_089;
pub const MAX_REGION_INDICES: usize = 6_144;
pub const MAX_REGION_LEVELS: usize = 2;
pub const MAX_CLUSTERS_PER_LEVEL: usize = 256;
pub const MAX_RESIDENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_BUILD_RESERVATION_BYTES: usize = 80 * 1024 * 1024;
pub const MAX_SCRATCH_BYTES: usize = MAX_TOTAL_BUILD_RESERVATION_BYTES - MAX_RESIDENT_BYTES;
pub const MAX_TRANSITION_VERTICES: usize = 60_000;
pub const MAX_TRANSITION_INDICES: usize = 180_000;
pub const MAX_TRANSITION_PAIR_ATTEMPTS: usize = 2_000_000;

const CHART_EPSILON: f64 = 1.0e-10;
const AREA_EPSILON: f64 = 1.0e-12;
const UV_DEDUP_SCALE: f64 = 1.0e12;
const SPATIAL_BINS: usize = 16;
const MESHLET_MAX_VERTICES: usize = 64;
const MESHLET_MAX_TRIANGLES: usize = 128;
// 3 normal + 4 material + 2 chart UV + 3 physical position components.
// Meshopt's attribute stride is measured in bytes.
const SIMPLIFY_ATTRIBUTE_FLOATS: usize = 12;
const MAX_FALLBACK_REASON_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionVertex {
    pub position: DVec3,
    pub normal: DVec3,
    pub material: [f64; 4],
    pub uv: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionInput {
    pub key: TileKey,
    pub boundary_version: u64,
    pub vertices: Vec<RegionVertex>,
    pub indices: Vec<u32>,
    pub locked: Vec<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cluster {
    pub first_index: u32,
    pub index_count: u32,
    pub sphere_center_local: DVec3,
    pub sphere_radius_m: f64,
    pub stable_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClusterLevel {
    pub clusters: Vec<Cluster>,
    pub indices: Vec<u32>,
    /// meshoptimizer's planar-chart simplification heuristic, scaled in meters.
    /// This is not a physical-surface deviation bound; use the common-refinement bounds.
    pub simplifier_error_m: f64,
    /// Certified bounds for this finite input mesh, populated only when its
    /// complete UV common refinement validates. These do not bound the world source.
    pub mesh_deviation_bound_m: Option<f64>,
    pub normal_deviation_bound_rad: Option<f64>,
    pub material_deviation_bound: Option<f64>,
    /// Maximum discrepancy among the explicit UV samples evaluated by this compiler.
    /// This is sampled evidence, not a certified maximum-error bound.
    pub sampled_deviation_m: f64,
    pub sampled_normal_error_rad: f64,
    pub sampled_material_error: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionVertex {
    pub fine: RegionVertex,
    pub coarse: RegionVertex,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransitionMesh {
    pub vertices: Vec<TransitionVertex>,
    /// Indices into this transition's `vertices`, not the region source array.
    pub indices: Vec<u32>,
    pub clusters: Vec<Cluster>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionProduct {
    pub input: RegionInput,
    pub levels: Vec<ClusterLevel>,
    pub transitions: Vec<TransitionMesh>,
    /// Why coarse replacement was rejected while retaining the fine product.
    pub fallback_reason: Option<String>,
    pub bounds_center_local: DVec3,
    pub bounds_radius_m: f64,
    pub resident_bytes: usize,
    /// Per-build scratch budget reservation. This is not allocator instrumentation
    /// or a verified upper bound on meshopt's opaque native allocation peak.
    pub scratch_bound_bytes: usize,
    pub build_micros: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClusterBuildError {
    #[error("region input is malformed or has unsupported chart topology")]
    InvalidInput,
    #[error("region input or compiler output exceeds the fixed resource cap")]
    ResourceCap,
    #[error("region compilation was cancelled")]
    Cancelled,
    #[error("meshoptimizer rejected a validated position buffer")]
    MeshoptAdapter,
}

#[derive(Clone, Copy)]
struct Triangle {
    indices: [u32; 3],
    uv: [DVec2; 3],
    min: DVec2,
    max: DVec2,
}

impl Triangle {
    fn from_indices(vertices: &[RegionVertex], indices: [u32; 3]) -> Self {
        let uv = indices.map(|index| {
            let value = vertices[index as usize].uv;
            DVec2::new(value[0], value[1])
        });
        Self {
            indices,
            uv,
            min: DVec2::new(
                uv.iter().map(|point| point.x).fold(f64::INFINITY, f64::min),
                uv.iter().map(|point| point.y).fold(f64::INFINITY, f64::min),
            ),
            max: DVec2::new(
                uv.iter()
                    .map(|point| point.x)
                    .fold(f64::NEG_INFINITY, f64::max),
                uv.iter()
                    .map(|point| point.y)
                    .fold(f64::NEG_INFINITY, f64::max),
            ),
        }
    }
}

pub fn compile(
    input: RegionInput,
    cancel: &AtomicBool,
) -> Result<RegionProduct, ClusterBuildError> {
    let started = Instant::now();
    check_cancel(cancel)?;
    let scratch_bound_bytes = scratch_reservation_bytes();
    if MAX_RESIDENT_BYTES.saturating_add(scratch_bound_bytes) > MAX_TOTAL_BUILD_RESERVATION_BYTES {
        return Err(ClusterBuildError::ResourceCap);
    }
    validate_input(&input)?;
    let center = input
        .vertices
        .iter()
        .fold(DVec3::ZERO, |sum, vertex| sum + vertex.position)
        / input.vertices.len() as f64;
    let radius = input
        .vertices
        .iter()
        .map(|vertex| vertex.position.distance(center))
        .fold(0.0, f64::max);
    if !(radius as f32).is_finite() {
        return Err(ClusterBuildError::InvalidInput);
    }

    let chart_scale = (2.0 * radius.max(1.0e-3)) as f32;
    if !chart_scale.is_finite() {
        return Err(ClusterBuildError::InvalidInput);
    }
    let mut chart_position_f32 = Vec::with_capacity(input.vertices.len() * 3);
    let mut attributes = Vec::with_capacity(input.vertices.len() * SIMPLIFY_ATTRIBUTE_FLOATS);
    for vertex in &input.vertices {
        let local = vertex.position - center;
        let physical_position = [local.x as f32, local.y as f32, local.z as f32];
        let chart_position = [
            ((vertex.uv[0] - 0.5) as f32 * chart_scale),
            ((vertex.uv[1] - 0.5) as f32 * chart_scale),
            0.0,
        ];
        let vertex_attributes = [
            vertex.normal.x as f32,
            vertex.normal.y as f32,
            vertex.normal.z as f32,
            vertex.material[0] as f32,
            vertex.material[1] as f32,
            vertex.material[2] as f32,
            vertex.material[3] as f32,
            vertex.uv[0] as f32,
            vertex.uv[1] as f32,
            physical_position[0],
            physical_position[1],
            physical_position[2],
        ];
        if physical_position
            .iter()
            .chain(chart_position.iter())
            .chain(vertex_attributes.iter())
            .any(|value| !value.is_finite())
        {
            return Err(ClusterBuildError::InvalidInput);
        }
        chart_position_f32.extend(chart_position);
        attributes.extend(vertex_attributes);
    }
    let adapter = VertexDataAdapter::new(
        meshopt::typed_to_bytes(&chart_position_f32),
        3 * size_of::<f32>(),
        0,
    )
    .map_err(|_| ClusterBuildError::MeshoptAdapter)?;

    let fine_indices = input.indices.clone();
    let (fine_clusters, fine_indices) = make_clusters(
        &fine_indices,
        &input.vertices,
        center,
        &input.key,
        input.boundary_version,
        0,
        cancel,
    )?;
    let fine = ClusterLevel {
        clusters: fine_clusters,
        indices: fine_indices,
        simplifier_error_m: 0.0,
        mesh_deviation_bound_m: Some(0.0),
        normal_deviation_bound_rad: Some(0.0),
        material_deviation_bound: Some(0.0),
        sampled_deviation_m: 0.0,
        sampled_normal_error_rad: 0.0,
        sampled_material_error: 0.0,
    };

    let mut levels = vec![fine];
    let mut transitions = Vec::new();
    let mut fallback_reason = None;
    if input.indices.len() >= 6 {
        check_cancel(cancel)?;
        let target_count = ((input.indices.len() / 2) / 3 * 3).max(3);
        let mut simplifier_error = 0.0_f32;
        let simplified = simplify_with_attributes_and_locks(
            &input.indices,
            &adapter,
            &attributes,
            &[
                0.15, 0.15, 0.15, 0.25, 0.25, 0.25, 0.25, 0.08, 0.08, 1.0, 1.0, 1.0,
            ],
            SIMPLIFY_ATTRIBUTE_FLOATS * size_of::<f32>(),
            &input.locked,
            target_count,
            (radius.max(1.0e-3) * 0.025) as f32,
            SimplifyOptions::LockBorder | SimplifyOptions::ErrorAbsolute,
            Some(&mut simplifier_error),
        );

        if simplified.len() < 3 || simplified.len() >= input.indices.len() {
            fallback_reason = Some(String::from("simplifier-no-progress"));
        } else if !simplified.len().is_multiple_of(3)
            || validate_index_stream(&input, &simplified).is_err()
        {
            fallback_reason = Some(String::from("invalid-coarse-topology"));
        } else {
            let coarse = make_clusters(
                &simplified,
                &input.vertices,
                center,
                &input.key,
                input.boundary_version,
                1,
                cancel,
            );
            let (coarse_clusters, coarse_indices) = match coarse {
                Ok(product) => product,
                Err(ClusterBuildError::Cancelled) => return Err(ClusterBuildError::Cancelled),
                Err(ClusterBuildError::ResourceCap) => {
                    fallback_reason = Some(String::from("coarse-cluster-resource-cap"));
                    let resident_bytes = estimate_resident_bytes(
                        &input,
                        &levels,
                        &transitions,
                        fallback_reason.as_ref(),
                    );
                    return build_fine_fallback(
                        input,
                        levels,
                        transitions,
                        fallback_reason,
                        center,
                        radius,
                        resident_bytes,
                        scratch_bound_bytes,
                        started,
                    );
                }
                Err(ClusterBuildError::InvalidInput | ClusterBuildError::MeshoptAdapter) => {
                    fallback_reason = Some(String::from("coarse-cluster-build-failed"));
                    let resident_bytes = estimate_resident_bytes(
                        &input,
                        &levels,
                        &transitions,
                        fallback_reason.as_ref(),
                    );
                    return build_fine_fallback(
                        input,
                        levels,
                        transitions,
                        fallback_reason,
                        center,
                        radius,
                        resident_bytes,
                        scratch_bound_bytes,
                        started,
                    );
                }
            };
            let fixed_resident = estimate_resident_bytes(&input, &levels, &transitions, None)
                .saturating_add(MAX_FALLBACK_REASON_BYTES)
                .saturating_add(size_of::<ClusterLevel>())
                .saturating_add(coarse_clusters.capacity() * size_of::<Cluster>())
                .saturating_add(coarse_indices.capacity() * size_of::<u32>())
                .saturating_add(size_of::<TransitionMesh>())
                .saturating_add(MAX_CLUSTERS_PER_LEVEL * size_of::<Cluster>())
                .saturating_add(MAX_TRANSITION_INDICES * size_of::<u32>());
            let transition_vertex_limit = MAX_TRANSITION_VERTICES.min(
                MAX_RESIDENT_BYTES.saturating_sub(fixed_resident) / size_of::<TransitionVertex>(),
            );
            match build_transition(
                &input,
                &input.indices,
                &simplified,
                transition_vertex_limit,
                cancel,
            ) {
                Ok((transition, metrics)) => {
                    if levels.len() >= MAX_REGION_LEVELS {
                        return Err(ClusterBuildError::ResourceCap);
                    }
                    levels.push(ClusterLevel {
                        clusters: coarse_clusters,
                        indices: coarse_indices,
                        simplifier_error_m: f64::from(simplifier_error),
                        mesh_deviation_bound_m: Some(metrics.mesh_deviation_bound_m),
                        normal_deviation_bound_rad: metrics.normal_deviation_bound_rad,
                        material_deviation_bound: Some(metrics.material_deviation_bound),
                        sampled_deviation_m: metrics.sampled_deviation_m,
                        sampled_normal_error_rad: metrics.sampled_normal_error_rad,
                        sampled_material_error: metrics.sampled_material_error,
                    });
                    transitions.push(transition);
                }
                Err(TransitionFailure::Cancelled) => return Err(ClusterBuildError::Cancelled),
                Err(TransitionFailure::InvalidCoverage) => {
                    fallback_reason = Some(String::from("transition-invalid-coverage"));
                }
                Err(TransitionFailure::ResourceCap) => {
                    fallback_reason = Some(String::from("transition-resource-cap"));
                }
            }
        }
    } else {
        fallback_reason = Some(String::from("insufficient-fine-triangles"));
    }

    let resident_bytes =
        estimate_resident_bytes(&input, &levels, &transitions, fallback_reason.as_ref());
    if resident_bytes > MAX_RESIDENT_BYTES
        || scratch_bound_bytes > MAX_SCRATCH_BYTES
        || resident_bytes.saturating_add(scratch_bound_bytes) > MAX_TOTAL_BUILD_RESERVATION_BYTES
    {
        return Err(ClusterBuildError::ResourceCap);
    }
    Ok(RegionProduct {
        input,
        levels,
        transitions,
        fallback_reason,
        bounds_center_local: center,
        bounds_radius_m: radius,
        resident_bytes,
        scratch_bound_bytes,
        build_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
    })
}

#[allow(clippy::too_many_arguments)] // Retain the complete build transaction on failed replacement.
fn build_fine_fallback(
    input: RegionInput,
    levels: Vec<ClusterLevel>,
    transitions: Vec<TransitionMesh>,
    fallback_reason: Option<String>,
    bounds_center_local: DVec3,
    bounds_radius_m: f64,
    resident_bytes: usize,
    scratch_bound_bytes: usize,
    started: Instant,
) -> Result<RegionProduct, ClusterBuildError> {
    if resident_bytes > MAX_RESIDENT_BYTES
        || resident_bytes.saturating_add(scratch_bound_bytes) > MAX_TOTAL_BUILD_RESERVATION_BYTES
    {
        return Err(ClusterBuildError::ResourceCap);
    }
    Ok(RegionProduct {
        input,
        levels,
        transitions,
        fallback_reason,
        bounds_center_local,
        bounds_radius_m,
        resident_bytes,
        scratch_bound_bytes,
        build_micros: started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64,
    })
}

fn validate_input(input: &RegionInput) -> Result<(), ClusterBuildError> {
    if input.vertices.len() < 4
        || input.vertices.len() > MAX_REGION_VERTICES
        || input.indices.len() < 6
        || input.indices.len() > MAX_REGION_INDICES
        || !input.indices.len().is_multiple_of(3)
        || input.locked.len() != input.vertices.len()
    {
        return Err(ClusterBuildError::ResourceCap);
    }
    if input.vertices.iter().any(|vertex| {
        !vertex.position.is_finite()
            || !vertex.normal.is_finite()
            || vertex.normal.length_squared() <= 1.0e-24
            || [
                vertex.normal.x as f32,
                vertex.normal.y as f32,
                vertex.normal.z as f32,
            ]
            .iter()
            .any(|value| !value.is_finite())
            || vertex.material.iter().any(|value| !value.is_finite())
            || vertex
                .material
                .iter()
                .any(|value| !(*value as f32).is_finite())
            || vertex.uv.iter().any(|value| !value.is_finite())
            || vertex
                .uv
                .iter()
                .any(|value| *value < -CHART_EPSILON || *value > 1.0 + CHART_EPSILON)
            || ![
                vertex.position.x as f32,
                vertex.position.y as f32,
                vertex.position.z as f32,
            ]
            .iter()
            .all(|value| value.is_finite())
    }) {
        return Err(ClusterBuildError::InvalidInput);
    }
    validate_index_stream(input, &input.indices)
}

fn validate_index_stream(input: &RegionInput, indices: &[u32]) -> Result<(), ClusterBuildError> {
    if indices.len() < 3 || !indices.len().is_multiple_of(3) {
        return Err(ClusterBuildError::InvalidInput);
    }
    let mut edges = BTreeMap::<(u32, u32), u8>::new();
    let mut used_vertices = BTreeSet::<u32>::new();
    let mut total_area = 0.0_f64;
    for triangle in indices.as_chunks::<3>().0 {
        let ids = [triangle[0], triangle[1], triangle[2]];
        if ids
            .iter()
            .any(|index| *index as usize >= input.vertices.len())
            || ids[0] == ids[1]
            || ids[1] == ids[2]
            || ids[2] == ids[0]
        {
            return Err(ClusterBuildError::InvalidInput);
        }
        used_vertices.extend(ids);
        let a = uv(input.vertices[ids[0] as usize]);
        let b = uv(input.vertices[ids[1] as usize]);
        let c = uv(input.vertices[ids[2] as usize]);
        let cross = cross2(b - a, c - a);
        if !cross.is_finite() || cross <= AREA_EPSILON {
            return Err(ClusterBuildError::InvalidInput);
        }
        total_area += cross.abs() * 0.5;
        for (from, to) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
            let edge = if from < to { (from, to) } else { (to, from) };
            let count = edges.entry(edge).or_default();
            *count = count.saturating_add(1);
            if *count > 2 {
                return Err(ClusterBuildError::InvalidInput);
            }
        }
    }
    if (total_area - 1.0).abs() > 1.0e-7 {
        return Err(ClusterBuildError::InvalidInput);
    }

    let edge_count = edges.len();
    if used_vertices
        .len()
        .saturating_add(indices.len() / 3)
        .checked_sub(edge_count)
        != Some(1)
    {
        return Err(ClusterBuildError::InvalidInput);
    }
    let mut adjacency = BTreeMap::<u32, Vec<u32>>::new();
    for &(a, b) in edges.keys() {
        adjacency.entry(a).or_default().push(b);
        adjacency.entry(b).or_default().push(a);
    }
    let Some(&first_vertex) = used_vertices.iter().next() else {
        return Err(ClusterBuildError::InvalidInput);
    };
    let mut reached = BTreeSet::new();
    let mut pending = vec![first_vertex];
    while let Some(vertex) = pending.pop() {
        if reached.insert(vertex) {
            pending.extend(adjacency.get(&vertex).into_iter().flatten().copied());
        }
    }
    if reached.len() != used_vertices.len() {
        return Err(ClusterBuildError::InvalidInput);
    }

    let mut perimeter = [0.0_f64; 4];
    for ((a, b), count) in edges {
        if count == 1 {
            let va = input.vertices[a as usize];
            let vb = input.vertices[b as usize];
            if !input.locked[a as usize] || !input.locked[b as usize] {
                return Err(ClusterBuildError::InvalidInput);
            }
            let side = boundary_side(va.uv, vb.uv).ok_or(ClusterBuildError::InvalidInput)?;
            perimeter[side] += (uv(va) - uv(vb)).length();
        }
    }
    if perimeter
        .iter()
        .any(|length| (*length - 1.0).abs() > 1.0e-7)
    {
        return Err(ClusterBuildError::InvalidInput);
    }
    if input
        .vertices
        .iter()
        .enumerate()
        .any(|(index, vertex)| is_outer_boundary(vertex.uv) && !input.locked[index])
    {
        return Err(ClusterBuildError::InvalidInput);
    }
    Ok(())
}

fn boundary_side(a: [f64; 2], b: [f64; 2]) -> Option<usize> {
    if a[0].abs() <= CHART_EPSILON && b[0].abs() <= CHART_EPSILON {
        Some(0)
    } else if (a[0] - 1.0).abs() <= CHART_EPSILON && (b[0] - 1.0).abs() <= CHART_EPSILON {
        Some(1)
    } else if a[1].abs() <= CHART_EPSILON && b[1].abs() <= CHART_EPSILON {
        Some(2)
    } else if (a[1] - 1.0).abs() <= CHART_EPSILON && (b[1] - 1.0).abs() <= CHART_EPSILON {
        Some(3)
    } else {
        None
    }
}

fn is_outer_boundary(uv: [f64; 2]) -> bool {
    uv[0].abs() <= CHART_EPSILON
        || (uv[0] - 1.0).abs() <= CHART_EPSILON
        || uv[1].abs() <= CHART_EPSILON
        || (uv[1] - 1.0).abs() <= CHART_EPSILON
}

fn make_clusters(
    source_indices: &[u32],
    vertices: &[RegionVertex],
    center: DVec3,
    key: &TileKey,
    boundary_version: u64,
    level: u32,
    cancel: &AtomicBool,
) -> Result<(Vec<Cluster>, Vec<u32>), ClusterBuildError> {
    check_cancel(cancel)?;
    let mut positions = Vec::with_capacity(vertices.len() * 3);
    for vertex in vertices {
        let point = vertex.position - center;
        positions.extend([point.x as f32, point.y as f32, point.z as f32]);
    }
    let adapter =
        VertexDataAdapter::new(meshopt::typed_to_bytes(&positions), 3 * size_of::<f32>(), 0)
            .map_err(|_| ClusterBuildError::MeshoptAdapter)?;
    let meshlets = build_meshlets(
        source_indices,
        &adapter,
        MESHLET_MAX_VERTICES,
        MESHLET_MAX_TRIANGLES,
        0.0,
    );
    if meshlets.len() > MAX_CLUSTERS_PER_LEVEL {
        return Err(ClusterBuildError::ResourceCap);
    }
    let mut output_indices = Vec::with_capacity(source_indices.len());
    let mut clusters = Vec::with_capacity(meshlets.len());
    for (ordinal, meshlet) in meshlets.iter().enumerate() {
        check_cancel(cancel)?;
        let first_offset = output_indices.len();
        let first_index =
            u32::try_from(first_offset).map_err(|_| ClusterBuildError::ResourceCap)?;
        for local_index in meshlet.triangles.as_chunks::<3>().0 {
            for local in local_index {
                let global = *meshlet
                    .vertices
                    .get(usize::from(*local))
                    .ok_or(ClusterBuildError::InvalidInput)?;
                output_indices.push(global);
            }
        }
        let index_count = u32::try_from(output_indices.len() - first_offset)
            .map_err(|_| ClusterBuildError::ResourceCap)?;
        let point_iter = output_indices[first_index as usize..]
            .iter()
            .map(|index| vertices[*index as usize].position);
        let (sphere_center, sphere_radius) = enclosing_sphere(point_iter);
        clusters.push(Cluster {
            first_index,
            index_count,
            sphere_center_local: sphere_center,
            sphere_radius_m: sphere_radius,
            stable_id: stable_id(key, boundary_version, level, ordinal as u32),
        });
    }
    if output_indices.len() != source_indices.len() || clusters.is_empty() {
        return Err(ClusterBuildError::InvalidInput);
    }
    Ok((clusters, output_indices))
}

fn enclosing_sphere(points: impl Iterator<Item = DVec3>) -> (DVec3, f64) {
    let mut values = Vec::new();
    let mut min = DVec3::splat(f64::INFINITY);
    let mut max = DVec3::splat(f64::NEG_INFINITY);
    for point in points {
        min = min.min(point);
        max = max.max(point);
        values.push(point);
    }
    let center = (min + max) * 0.5;
    let radius = values
        .into_iter()
        .map(|point| point.distance(center))
        .fold(0.0, f64::max);
    (center, radius)
}

fn build_transition(
    input: &RegionInput,
    fine_indices: &[u32],
    coarse_indices: &[u32],
    max_vertices: usize,
    cancel: &AtomicBool,
) -> Result<(TransitionMesh, DeviationMetrics), TransitionFailure> {
    let fine = triangles(input, fine_indices);
    let coarse = triangles(input, coarse_indices);
    let mut bins = vec![Vec::<usize>::new(); SPATIAL_BINS * SPATIAL_BINS];
    let mut bin_refs = 0usize;
    for (index, triangle) in coarse.iter().enumerate() {
        for bin in bins_for(triangle.min, triangle.max) {
            bin_refs += 1;
            if bin_refs > 128_000 {
                return Err(TransitionFailure::ResourceCap);
            }
            bins[bin].push(index);
        }
    }

    if max_vertices == 0 {
        return Err(TransitionFailure::ResourceCap);
    }
    let mut vertices = Vec::<TransitionVertex>::with_capacity(max_vertices);
    let mut indices = Vec::<u32>::with_capacity(MAX_TRANSITION_INDICES);
    let mut unique = BTreeMap::<(i64, i64), u32>::new();
    let mut pair_attempts = 0usize;
    let mut area_sum = 0.0_f64;
    let mut fine_covered_area = vec![0.0_f64; fine.len()];
    let mut coarse_covered_area = vec![0.0_f64; coarse.len()];
    let mut sampled_deviation = 0.0_f64;
    let mut sampled_normal = 0.0_f64;
    let mut sampled_material = 0.0_f64;
    let mut mesh_deviation_bound = 0.0_f64;
    let mut normal_deviation_bound: Option<f64> = Some(0.0);
    let mut material_deviation_bound = 0.0_f64;

    for (fine_index, fine_triangle) in fine.iter().enumerate() {
        check_cancel(cancel).map_err(|_| TransitionFailure::Cancelled)?;
        let mut candidate_set = BTreeSet::new();
        for bin in bins_for(fine_triangle.min, fine_triangle.max) {
            candidate_set.extend(bins[bin].iter().copied());
        }
        for coarse_index in candidate_set {
            pair_attempts += 1;
            if pair_attempts > MAX_TRANSITION_PAIR_ATTEMPTS {
                return Err(TransitionFailure::ResourceCap);
            }
            let coarse_triangle = &coarse[coarse_index];
            if !bbox_overlaps(fine_triangle, coarse_triangle) {
                continue;
            }
            let polygon = intersect_triangles(fine_triangle, coarse_triangle);
            if polygon.len() < 3 {
                continue;
            }
            let area = polygon_area(&polygon);
            if area <= AREA_EPSILON {
                continue;
            }
            area_sum += area;
            fine_covered_area[fine_index] += area;
            coarse_covered_area[coarse_index] += area;
            let mut patch_normals = Vec::with_capacity(polygon.len());
            let mut patch_normal_differences = Vec::with_capacity(polygon.len());
            for point in &polygon {
                let a = interpolate(input, fine_triangle, *point)
                    .ok_or(TransitionFailure::InvalidCoverage)?;
                let b = interpolate(input, coarse_triangle, *point)
                    .ok_or(TransitionFailure::InvalidCoverage)?;
                mesh_deviation_bound = mesh_deviation_bound.max(a.position.distance(b.position));
                patch_normals.extend([a.normal, b.normal]);
                patch_normal_differences.push(a.normal.distance(b.normal));
                for channel in 0..4 {
                    material_deviation_bound = material_deviation_bound
                        .max((a.material[channel] - b.material[channel]).abs());
                }
            }
            let axis_sum = patch_normals
                .iter()
                .copied()
                .fold(DVec3::ZERO, |sum, n| sum + n);
            if axis_sum.length_squared() <= 1.0e-24 {
                normal_deviation_bound = None;
            } else if let Some(axis) = axis_sum.try_normalize() {
                let minimum_axis_dot = patch_normals
                    .iter()
                    .map(|normal| normal.dot(axis))
                    .fold(f64::INFINITY, f64::min);
                let max_raw_difference =
                    patch_normal_differences.iter().copied().fold(0.0, f64::max);
                if !minimum_axis_dot.is_finite() || minimum_axis_dot <= 0.0 {
                    normal_deviation_bound = None;
                } else if let Some(bound) = normal_deviation_bound.as_mut() {
                    *bound =
                        bound.max(2.0 * (max_raw_difference / minimum_axis_dot).min(1.0).asin());
                }
            } else {
                normal_deviation_bound = None;
            }
            let mut sample_points = polygon.clone();
            sample_points.push(
                polygon.iter().copied().fold(DVec2::ZERO, |sum, p| sum + p) / polygon.len() as f64,
            );
            for point in sample_points {
                let a = interpolate(input, fine_triangle, point)
                    .ok_or(TransitionFailure::InvalidCoverage)?;
                let b = interpolate(input, coarse_triangle, point)
                    .ok_or(TransitionFailure::InvalidCoverage)?;
                sampled_deviation = sampled_deviation.max(a.position.distance(b.position));
                sampled_normal = sampled_normal.max(normal_angle(a.normal, b.normal));
                for channel in 0..4 {
                    sampled_material =
                        sampled_material.max((a.material[channel] - b.material[channel]).abs());
                }
            }

            let polygon_indices = polygon
                .into_iter()
                .map(|point| {
                    let key = (
                        (point.x.clamp(0.0, 1.0) * UV_DEDUP_SCALE).round() as i64,
                        (point.y.clamp(0.0, 1.0) * UV_DEDUP_SCALE).round() as i64,
                    );
                    if let Some(index) = unique.get(&key) {
                        return Ok(*index);
                    }
                    if vertices.len() >= max_vertices {
                        return Err(TransitionFailure::ResourceCap);
                    }
                    let uv =
                        DVec2::new(key.0 as f64 / UV_DEDUP_SCALE, key.1 as f64 / UV_DEDUP_SCALE);
                    let fine_value = interpolate(input, fine_triangle, uv)
                        .ok_or(TransitionFailure::InvalidCoverage)?;
                    let coarse_value = interpolate(input, coarse_triangle, uv)
                        .ok_or(TransitionFailure::InvalidCoverage)?;
                    let index = u32::try_from(vertices.len())
                        .map_err(|_| TransitionFailure::ResourceCap)?;
                    vertices.push(TransitionVertex {
                        fine: fine_value,
                        coarse: coarse_value,
                    });
                    unique.insert(key, index);
                    Ok(index)
                })
                .collect::<Result<Vec<_>, TransitionFailure>>()?;
            for index in 1..polygon_indices.len() - 1 {
                if indices.len() + 3 > MAX_TRANSITION_INDICES {
                    return Err(TransitionFailure::ResourceCap);
                }
                indices.extend([
                    polygon_indices[0],
                    polygon_indices[index],
                    polygon_indices[index + 1],
                ]);
            }
        }
    }
    if vertices.is_empty()
        || indices.is_empty()
        || (area_sum - 1.0).abs() > 1.0e-7
        || fine
            .iter()
            .zip(fine_covered_area)
            .any(|(triangle, covered)| (triangle_area(triangle) - covered).abs() > 1.0e-9)
        || coarse
            .iter()
            .zip(coarse_covered_area)
            .any(|(triangle, covered)| (triangle_area(triangle) - covered).abs() > 1.0e-9)
    {
        return Err(TransitionFailure::InvalidCoverage);
    }
    let transition_positions: Vec<f32> = vertices
        .iter()
        .flat_map(|vertex| {
            [
                vertex.fine.position.x as f32,
                vertex.fine.position.y as f32,
                vertex.fine.position.z as f32,
            ]
        })
        .collect();
    let adapter = VertexDataAdapter::new(
        meshopt::typed_to_bytes(&transition_positions),
        3 * size_of::<f32>(),
        0,
    )
    .map_err(|_| TransitionFailure::InvalidCoverage)?;
    let meshlets = build_meshlets(
        &indices,
        &adapter,
        MESHLET_MAX_VERTICES,
        MESHLET_MAX_TRIANGLES,
        0.0,
    );
    if meshlets.len() > MAX_CLUSTERS_PER_LEVEL {
        return Err(TransitionFailure::ResourceCap);
    }
    let mut clustered_indices = Vec::with_capacity(indices.len());
    let mut clusters = Vec::with_capacity(meshlets.len());
    for (ordinal, meshlet) in meshlets.iter().enumerate() {
        check_cancel(cancel).map_err(|_| TransitionFailure::Cancelled)?;
        let first_offset = clustered_indices.len();
        let first_index =
            u32::try_from(first_offset).map_err(|_| TransitionFailure::ResourceCap)?;
        let mut endpoint_points = Vec::new();
        for local in meshlet.triangles.as_chunks::<3>().0.iter().flatten() {
            let global = *meshlet
                .vertices
                .get(usize::from(*local))
                .ok_or(TransitionFailure::InvalidCoverage)?;
            clustered_indices.push(global);
            let vertex = vertices
                .get(global as usize)
                .ok_or(TransitionFailure::InvalidCoverage)?;
            endpoint_points.extend([vertex.fine.position, vertex.coarse.position]);
        }
        let (sphere_center, sphere_radius) = enclosing_sphere(endpoint_points.into_iter());
        clusters.push(Cluster {
            first_index,
            index_count: u32::try_from(clustered_indices.len() - first_offset)
                .map_err(|_| TransitionFailure::ResourceCap)?,
            sphere_center_local: sphere_center,
            sphere_radius_m: sphere_radius,
            stable_id: stable_id(&input.key, input.boundary_version, 2, ordinal as u32),
        });
    }
    if clustered_indices.len() != indices.len() {
        return Err(TransitionFailure::InvalidCoverage);
    }
    Ok((
        TransitionMesh {
            vertices,
            indices: clustered_indices,
            clusters,
        },
        DeviationMetrics {
            mesh_deviation_bound_m: mesh_deviation_bound,
            normal_deviation_bound_rad: normal_deviation_bound,
            material_deviation_bound,
            sampled_deviation_m: sampled_deviation,
            sampled_normal_error_rad: sampled_normal,
            sampled_material_error: sampled_material,
        },
    ))
}

#[derive(Debug, Clone, Copy)]
struct DeviationMetrics {
    mesh_deviation_bound_m: f64,
    normal_deviation_bound_rad: Option<f64>,
    material_deviation_bound: f64,
    sampled_deviation_m: f64,
    sampled_normal_error_rad: f64,
    sampled_material_error: f64,
}

#[derive(Debug, Clone, Copy)]
enum TransitionFailure {
    InvalidCoverage,
    ResourceCap,
    Cancelled,
}

fn triangles(input: &RegionInput, indices: &[u32]) -> Vec<Triangle> {
    indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|triangle| {
            Triangle::from_indices(&input.vertices, [triangle[0], triangle[1], triangle[2]])
        })
        .collect()
}

fn bins_for(min: DVec2, max: DVec2) -> impl Iterator<Item = usize> {
    let x0 = (min.x.clamp(0.0, 1.0) * SPATIAL_BINS as f64).floor() as usize;
    let y0 = (min.y.clamp(0.0, 1.0) * SPATIAL_BINS as f64).floor() as usize;
    let x1 = ((max.x.clamp(0.0, 1.0) * SPATIAL_BINS as f64).floor() as usize).min(SPATIAL_BINS - 1);
    let y1 = ((max.y.clamp(0.0, 1.0) * SPATIAL_BINS as f64).floor() as usize).min(SPATIAL_BINS - 1);
    (y0.min(SPATIAL_BINS - 1)..=y1)
        .flat_map(move |y| (x0.min(SPATIAL_BINS - 1)..=x1).map(move |x| y * SPATIAL_BINS + x))
}

fn bbox_overlaps(a: &Triangle, b: &Triangle) -> bool {
    a.min.x <= b.max.x + CHART_EPSILON
        && a.max.x + CHART_EPSILON >= b.min.x
        && a.min.y <= b.max.y + CHART_EPSILON
        && a.max.y + CHART_EPSILON >= b.min.y
}

fn intersect_triangles(a: &Triangle, b: &Triangle) -> Vec<DVec2> {
    let mut subject = oriented_triangle(a.uv).to_vec();
    let clip = oriented_triangle(b.uv);
    for edge in 0..3 {
        let start = clip[edge];
        let end = clip[(edge + 1) % 3];
        if subject.is_empty() {
            break;
        }
        let mut output = Vec::with_capacity(subject.len() + 1);
        let mut previous = *subject.last().unwrap();
        let mut previous_inside = cross2(end - start, previous - start) >= -AREA_EPSILON;
        for current in subject.iter().copied() {
            let current_inside = cross2(end - start, current - start) >= -AREA_EPSILON;
            if current_inside != previous_inside
                && let Some(intersection) = line_intersection(previous, current, start, end)
            {
                output.push(intersection);
            }
            if current_inside {
                output.push(current);
            }
            previous = current;
            previous_inside = current_inside;
        }
        subject = dedup_polygon(output);
    }
    subject
}

fn oriented_triangle(mut triangle: [DVec2; 3]) -> [DVec2; 3] {
    if cross2(triangle[1] - triangle[0], triangle[2] - triangle[0]) < 0.0 {
        triangle.swap(1, 2);
    }
    triangle
}

fn line_intersection(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> Option<DVec2> {
    let r = b - a;
    let s = d - c;
    let denominator = cross2(r, s);
    if denominator.abs() <= AREA_EPSILON {
        return None;
    }
    let t = cross2(c - a, s) / denominator;
    Some(a + t * r)
}

fn dedup_polygon(points: Vec<DVec2>) -> Vec<DVec2> {
    let mut result = Vec::with_capacity(points.len());
    for point in points {
        if result
            .last()
            .is_none_or(|last: &DVec2| last.distance_squared(point) > 1.0e-24)
        {
            result.push(point);
        }
    }
    if result.len() > 1 && result[0].distance_squared(*result.last().unwrap()) <= 1.0e-24 {
        result.pop();
    }
    result
}

fn polygon_area(points: &[DVec2]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let double_area = (0..points.len())
        .map(|i| cross2(points[i], points[(i + 1) % points.len()]))
        .sum::<f64>();
    double_area.abs() * 0.5
}

fn triangle_area(triangle: &Triangle) -> f64 {
    cross2(
        triangle.uv[1] - triangle.uv[0],
        triangle.uv[2] - triangle.uv[0],
    )
    .abs()
        * 0.5
}

fn interpolate(input: &RegionInput, triangle: &Triangle, point: DVec2) -> Option<RegionVertex> {
    let a = triangle.uv[0];
    let b = triangle.uv[1];
    let c = triangle.uv[2];
    let denominator = cross2(b - a, c - a);
    if denominator.abs() <= AREA_EPSILON {
        return None;
    }
    let wb = cross2(point - a, c - a) / denominator;
    let wc = cross2(b - a, point - a) / denominator;
    let wa = 1.0 - wb - wc;
    if wa < -1.0e-8 || wb < -1.0e-8 || wc < -1.0e-8 {
        return None;
    }
    let va = input.vertices[triangle.indices[0] as usize];
    let vb = input.vertices[triangle.indices[1] as usize];
    let vc = input.vertices[triangle.indices[2] as usize];
    Some(RegionVertex {
        position: va.position * wa + vb.position * wb + vc.position * wc,
        normal: va.normal * wa + vb.normal * wb + vc.normal * wc,
        material: std::array::from_fn(|channel| {
            va.material[channel] * wa + vb.material[channel] * wb + vc.material[channel] * wc
        }),
        uv: [point.x, point.y],
    })
}

fn normal_angle(a: DVec3, b: DVec3) -> f64 {
    let dot = a.normalize().dot(b.normalize()).clamp(-1.0, 1.0);
    dot.acos()
}

fn uv(vertex: RegionVertex) -> DVec2 {
    DVec2::new(vertex.uv[0], vertex.uv[1])
}

fn cross2(a: DVec2, b: DVec2) -> f64 {
    a.x * b.y - a.y * b.x
}

fn stable_id(key: &TileKey, boundary_version: u64, level: u32, ordinal: u32) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    let mut feed = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u32::from(byte);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    };
    feed(key.body_identity);
    for word in &key.definition_words {
        feed(*word);
    }
    feed(key.radius_bits);
    feed(key.surface_revision);
    feed(key.material_revision);
    feed(u64::from(key.format_version));
    feed(u64::from(key.filter_version));
    feed(key.address.face() as u64);
    feed(u64::from(key.address.level()));
    let [address_x, address_y] = key.address.coordinates();
    feed(u64::from(address_x));
    feed(u64::from(address_y));
    feed(u64::from(key.cells));
    feed(boundary_version);
    feed(u64::from(level));
    feed(u64::from(ordinal));
    hash
}

fn estimate_resident_bytes(
    input: &RegionInput,
    levels: &Vec<ClusterLevel>,
    transitions: &Vec<TransitionMesh>,
    fallback_reason: Option<&String>,
) -> usize {
    size_of::<RegionProduct>()
        + size_of::<RegionInput>()
        + input.key.definition_words.capacity() * size_of::<u64>()
        + input.vertices.capacity() * size_of::<RegionVertex>()
        + input.indices.capacity() * size_of::<u32>()
        + input.locked.capacity() * size_of::<bool>()
        + fallback_reason.map_or(0, String::capacity)
        + levels.capacity() * size_of::<ClusterLevel>()
        + levels
            .iter()
            .map(|level| {
                level.clusters.capacity() * size_of::<Cluster>()
                    + level.indices.capacity() * size_of::<u32>()
            })
            .sum::<usize>()
        + transitions.capacity() * size_of::<TransitionMesh>()
        + transitions
            .iter()
            .map(|transition| {
                transition.vertices.capacity() * size_of::<TransitionVertex>()
                    + transition.indices.capacity() * size_of::<u32>()
                    + transition.clusters.capacity() * size_of::<Cluster>()
            })
            .sum::<usize>()
}

fn scratch_reservation_bytes() -> usize {
    // Reserve the entire remainder of the declared 80 MiB per-build budget
    // after the product cap. This is a capacity reservation policy, not a
    // measured or verified upper bound on opaque meshopt allocator behavior.
    MAX_SCRATCH_BYTES
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ClusterBuildError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ClusterBuildError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::surface::{CubeFace, CubePatchAddress};

    fn grid(cells: u32, bump: f64) -> RegionInput {
        let mut vertices = Vec::new();
        let mut locked = Vec::new();
        for y in 0..=cells {
            for x in 0..=cells {
                let u = f64::from(x) / f64::from(cells);
                let v = f64::from(y) / f64::from(cells);
                let h = bump * (u * std::f64::consts::PI).sin() * (v * std::f64::consts::PI).sin();
                vertices.push(RegionVertex {
                    position: DVec3::new(u, v, h),
                    normal: DVec3::new(-0.1 * u, -0.1 * v, 1.0).normalize(),
                    material: [u, v, 1.0 - u, 0.25 + 0.5 * v],
                    uv: [u, v],
                });
                locked.push(x == 0 || y == 0 || x == cells || y == cells);
            }
        }
        let mut indices = Vec::new();
        for y in 0..cells {
            for x in 0..cells {
                let a = y * (cells + 1) + x;
                let b = a + 1;
                let c = a + cells + 1;
                let d = c + 1;
                indices.extend([a, b, d, a, d, c]);
            }
        }
        RegionInput {
            key: TileKey {
                body_identity: 7,
                definition_words: vec![1, 2, 3],
                radius_bits: 1.0_f64.to_bits(),
                surface_revision: 4,
                material_revision: 5,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveX),
                cells,
            },
            boundary_version: 1,
            vertices,
            indices,
            locked,
        }
    }

    #[test]
    fn common_refinement_preserves_both_endpoint_surfaces() {
        let product = compile(grid(8, 0.2), &AtomicBool::new(false)).unwrap();
        assert_eq!(product.levels.len(), 2);
        assert_eq!(product.transitions.len(), 1);
        assert_eq!(product.fallback_reason, None);
        let transition = &product.transitions[0];
        assert!(!transition.indices.is_empty());
        for vertex in &transition.vertices {
            assert!(vertex.fine.position.is_finite());
            assert!(vertex.coarse.position.is_finite());
            assert!((vertex.fine.uv[0] - vertex.coarse.uv[0]).abs() <= 1.0 / UV_DEDUP_SCALE);
            assert!((vertex.fine.uv[1] - vertex.coarse.uv[1]).abs() <= 1.0 / UV_DEDUP_SCALE);
            let point = uv(vertex.fine);
            let source_fine = find_surface_value(&product.input, &product.input.indices, point);
            let source_coarse =
                find_surface_value(&product.input, &product.levels[1].indices, point);
            assert!(source_fine.position.distance(vertex.fine.position) <= 2.0e-10);
            assert!(source_coarse.position.distance(vertex.coarse.position) <= 2.0e-10);
            assert!((source_fine.normal - vertex.fine.normal).length() <= 2.0e-10);
            assert!((source_coarse.normal - vertex.coarse.normal).length() <= 2.0e-10);
            for channel in 0..4 {
                assert!(
                    (source_fine.material[channel] - vertex.fine.material[channel]).abs()
                        <= 2.0e-10
                );
                assert!(
                    (source_coarse.material[channel] - vertex.coarse.material[channel]).abs()
                        <= 2.0e-10
                );
            }
        }
        assert!(product.levels[1].sampled_deviation_m.is_finite());
        assert!(
            product.levels[1].mesh_deviation_bound_m.unwrap()
                >= product.levels[1].sampled_deviation_m
        );
        assert!(
            product.levels[1].normal_deviation_bound_rad.unwrap()
                >= product.levels[1].sampled_normal_error_rad
        );
        assert!(
            product.levels[1].material_deviation_bound.unwrap()
                >= product.levels[1].sampled_material_error
        );
        assert!(product.resident_bytes <= MAX_RESIDENT_BYTES);
        assert!(product.scratch_bound_bytes <= MAX_SCRATCH_BYTES);
    }

    #[test]
    fn finite_mesh_bounds_cover_barycentric_transition_samples() {
        let product = compile(grid(8, 0.2), &AtomicBool::new(false)).unwrap();
        let transition = &product.transitions[0];
        let level = &product.levels[1];
        let position_bound = level.mesh_deviation_bound_m.unwrap();
        let normal_bound = level.normal_deviation_bound_rad.unwrap();
        let material_bound = level.material_deviation_bound.unwrap();
        for triangle in transition.indices.as_chunks::<3>().0 {
            let vertices = [triangle[0], triangle[1], triangle[2]]
                .map(|index| transition.vertices[index as usize]);
            for weights in [[0.2, 0.3, 0.5], [1.0 / 3.0; 3], [0.6, 0.1, 0.3]] {
                let fine_position = (0..3).fold(DVec3::ZERO, |sum, i| {
                    sum + vertices[i].fine.position * weights[i]
                });
                let coarse_position = (0..3).fold(DVec3::ZERO, |sum, i| {
                    sum + vertices[i].coarse.position * weights[i]
                });
                assert!(fine_position.distance(coarse_position) <= position_bound + 1.0e-10);

                let fine_normal = (0..3).fold(DVec3::ZERO, |sum, i| {
                    sum + vertices[i].fine.normal * weights[i]
                });
                let coarse_normal = (0..3).fold(DVec3::ZERO, |sum, i| {
                    sum + vertices[i].coarse.normal * weights[i]
                });
                assert!(normal_angle(fine_normal, coarse_normal) <= normal_bound + 1.0e-10);
                for channel in 0..4 {
                    let fine_material = (0..3)
                        .map(|i| vertices[i].fine.material[channel] * weights[i])
                        .sum::<f64>();
                    let coarse_material = (0..3)
                        .map(|i| vertices[i].coarse.material[channel] * weights[i])
                        .sum::<f64>();
                    assert!((fine_material - coarse_material).abs() <= material_bound + 1.0e-10);
                }
            }
        }
    }

    #[test]
    fn outer_edge_vertices_are_locked_and_topology_is_closed() {
        let input = grid(4, 0.0);
        validate_input(&input).unwrap();
        let mut invalid = input;
        invalid.locked[0] = false;
        assert_eq!(
            validate_input(&invalid),
            Err(ClusterBuildError::InvalidInput)
        );
    }

    #[test]
    fn pre_cancelled_build_returns_without_a_partial_product() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            compile(grid(4, 0.1), &cancel),
            Err(ClusterBuildError::Cancelled)
        );
    }

    #[test]
    fn fully_locked_mesh_terminates_at_the_fine_level() {
        let mut input = grid(4, 0.1);
        input.locked.fill(true);
        let product = compile(input, &AtomicBool::new(false)).unwrap();
        assert_eq!(product.levels.len(), 1);
        assert!(product.transitions.is_empty());
        assert_eq!(
            product.fallback_reason.as_deref(),
            Some("simplifier-no-progress")
        );
    }

    #[test]
    fn curved_bump_three_tenths_retains_a_legal_common_cut() {
        let product = compile(grid(8, 0.3), &AtomicBool::new(false)).unwrap();
        assert_eq!(product.levels.len(), 2);
        assert_eq!(product.transitions.len(), 1);
        assert_eq!(product.fallback_reason, None);
        validate_index_stream(&product.input, &product.levels[1].indices).unwrap();
        assert!(product.levels[1].mesh_deviation_bound_m.is_some());
    }

    #[test]
    fn curved_full_resolution_chart_simplification_stays_uv_legal() {
        let product = compile(grid(32, 0.3), &AtomicBool::new(false)).unwrap();
        if product.levels.len() == 2 {
            assert_eq!(product.transitions.len(), 1);
            validate_index_stream(&product.input, &product.levels[1].indices).unwrap();
            assert!(product.levels[1].mesh_deviation_bound_m.is_some());
        } else {
            assert!(product.transitions.is_empty());
            assert_ne!(
                product.fallback_reason.as_deref(),
                Some("invalid-coarse-topology")
            );
        }
    }

    #[test]
    fn oversized_region_is_rejected_before_meshopt_work() {
        let mut input = grid(4, 0.0);
        let duplicate = input.vertices[0];
        input.vertices.resize(MAX_REGION_VERTICES + 1, duplicate);
        assert_eq!(
            compile(input, &AtomicBool::new(false)),
            Err(ClusterBuildError::ResourceCap)
        );
    }

    fn find_surface_value(input: &RegionInput, indices: &[u32], point: DVec2) -> RegionVertex {
        for triangle in indices.as_chunks::<3>().0 {
            let triangle =
                Triangle::from_indices(&input.vertices, [triangle[0], triangle[1], triangle[2]]);
            if let Some(value) = interpolate(input, &triangle, point) {
                return value;
            }
        }
        panic!("common-refinement vertex is outside the tested surface")
    }
}
