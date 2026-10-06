//! CPU reference samples for the canonical resident terrain tile fixture.
//!
//! Usage: cargo run --locked -p mundaris_app --example resident_tile_samples -- <output.json>

use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity, TileData};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

const BODY_IDENTITY: u64 = 5_931_033_225_171_238_913;
const RADIUS_M: f64 = 80_000.0;
const CELLS: u32 = 64;

fn vec3(value: DVec3) -> [f64; 3] {
    value.to_array()
}

fn query(generator: &SurfaceGenerator, direction: Direction3) -> Result<Value> {
    let sample = generator.evaluate_point(SurfaceLocation::new(direction))?;
    Ok(json!({
        "position_m": vec3(sample.position(SurfaceLocation::new(direction))),
        "normal": vec3(sample.normal()),
        "material": sample.material_weights(),
        "radius_m": sample.radius_m(),
    }))
}

/// Match the builder's reduced signed-dyadic chart coordinates for 64-cell nodes.
fn canonical_direction(
    address: CubePatchAddress,
    i: i64,
    j: i64,
    cells: u32,
) -> Result<Direction3> {
    let denominator = u64::from(cells) << address.level();
    let [x, y] = address.coordinates();
    let a = 2 * (i64::from(cells) * i64::from(x) + i) - denominator as i64;
    let b = 2 * (i64::from(cells) * i64::from(y) + j) - denominator as i64;
    let [normal, u, v] = address.face().basis();
    let mut xyz = [0i64; 3];
    for axis in 0..3 {
        xyz[axis] =
            normal[axis] as i64 * denominator as i64 + u[axis] as i64 * a + v[axis] as i64 * b;
    }
    let mut divisor = denominator;
    while divisor > 1 && divisor.is_multiple_of(2) && xyz.iter().all(|component| component % 2 == 0)
    {
        xyz = xyz.map(|component| component / 2);
        divisor /= 2;
    }
    Ok(Direction3::try_new(DVec3::new(
        xyz[0] as f64,
        xyz[1] as f64,
        xyz[2] as f64,
    ))?)
}

fn node_sample(generator: &SurfaceGenerator, tile: &TileData, i: u32, j: u32) -> Result<Value> {
    let st = [
        f64::from(i) / f64::from(CELLS),
        f64::from(j) / f64::from(CELLS),
    ];
    let direction = canonical_direction(tile.key.address, i64::from(i), i64::from(j), CELLS)?;
    let complete = query(generator, direction)?;
    let complete_position: [f64; 3] = serde_json::from_value(complete["position_m"].clone())?;
    let complete_normal: [f64; 3] = serde_json::from_value(complete["normal"].clone())?;
    let complete_material: [f64; 4] = serde_json::from_value(complete["material"].clone())?;
    let tile_position = tile.anchor_position_body()? + tile.position_local(st)?;
    let tile_normal = tile.normal_local([i, j])?;
    let tile_material = tile.material(st)?;
    let position_error_m = (DVec3::from(complete_position) - tile_position).length();
    let normal_error_radians = DVec3::from(complete_normal)
        .dot(tile_normal)
        .clamp(-1.0, 1.0)
        .acos();
    let material_error_l2 = complete_material
        .iter()
        .zip(tile_material)
        .map(|(a, b)| (a - f64::from(b)).powi(2))
        .sum::<f64>()
        .sqrt();
    Ok(json!({
        "grid_index": [i, j],
        "patch_st": st,
        "direction_body": vec3(direction.unit()),
        "complete": complete,
        "tile": {
            "position_body_m": vec3(tile_position),
            "position_local_m": vec3(tile.position_local(st)?),
            "normal": vec3(tile_normal),
            "material": tile_material,
        },
        "residual": {
            "position_euclidean_m": position_error_m,
            "normal_angle_radians": normal_error_radians,
            "material_l2": material_error_l2,
        }
    }))
}

fn triangle_sample(
    generator: &SurfaceGenerator,
    tile: &TileData,
    cell: [u32; 2],
    triangle_index: usize,
) -> Result<Value> {
    let [x, y] = cell;
    let st = |i: u32, j: u32| {
        [
            f64::from(i) / f64::from(CELLS),
            f64::from(j) / f64::from(CELLS),
        ]
    };
    let corners = [
        ([x, y], tile.position_local(st(x, y))?),
        ([x + 1, y], tile.position_local(st(x + 1, y))?),
        ([x, y + 1], tile.position_local(st(x, y + 1))?),
        ([x + 1, y + 1], tile.position_local(st(x + 1, y + 1))?),
    ];
    // Match renderer grid topology: [p00,p10,p01] then [p10,p11,p01].
    let indices = if triangle_index == 0 {
        [0, 1, 2]
    } else {
        [1, 3, 2]
    };
    let vertices = indices.map(|index| corners[index]);
    let local_centroid = vertices.iter().map(|(_, p)| *p).sum::<DVec3>() / 3.0;
    let body_centroid = tile.anchor_position_body()? + local_centroid;
    let centroid_direction = Direction3::try_new(body_centroid)?;
    let complete = query(generator, centroid_direction)?;
    let complete_position: [f64; 3] = serde_json::from_value(complete["position_m"].clone())?;
    let complete_normal: [f64; 3] = serde_json::from_value(complete["normal"].clone())?;
    let complete_material: [f64; 4] = serde_json::from_value(complete["material"].clone())?;
    let vertex_normals = vertices.map(|([i, j], _)| tile.normal_local([i, j]));
    let vertex_normals = [vertex_normals[0]?, vertex_normals[1]?, vertex_normals[2]?];
    let rendered_normal = (vertex_normals[0] + vertex_normals[1] + vertex_normals[2]).normalize();
    let vertex_materials = vertices.map(|([i, j], _)| tile.material(st(i, j)));
    let vertex_materials = [
        vertex_materials[0]?,
        vertex_materials[1]?,
        vertex_materials[2]?,
    ];
    let rendered_material = std::array::from_fn::<_, 4, _>(|channel| {
        (vertex_materials[0][channel] + vertex_materials[1][channel] + vertex_materials[2][channel])
            / 3.0
    });
    let complete_position = DVec3::from(complete_position);
    Ok(json!({
        "cell": cell,
        "triangle": triangle_index,
        "grid_vertices": vertices.map(|(index, _)| index),
        "centroid_direction_body": vec3(centroid_direction.unit()),
        "complete": complete,
        "rendered_triangle": {
            "centroid_body_m": vec3(body_centroid),
            "interpolated_normal": vec3(rendered_normal),
            "interpolated_material": rendered_material,
        },
        "residual": {
            "centroid_position_euclidean_m": (complete_position - body_centroid).length(),
            "normal_angle_radians": DVec3::from(complete_normal)
                .dot(rendered_normal).clamp(-1.0, 1.0).acos(),
            "material_l2": complete_material.iter().zip(rendered_material)
                .map(|(a, b)| (a - f64::from(b)).powi(2)).sum::<f64>().sqrt(),
        }
    }))
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: cargo run --locked -p mundaris_app --example resident_tile_samples -- <output.json>")?;
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );

    let definition = SurfaceDefinition::generated(
        TerrainIdentity(BODY_IDENTITY),
        TerrainSeed(0),
        SurfaceAlgorithm::RockyV5,
    );
    let generator = SurfaceGenerator::new(&definition, RADIUS_M)?;
    let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39)?;
    let identity = TileBuildIdentity {
        body_identity: BODY_IDENTITY,
        surface_revision: 2,
        material_revision: definition.material_identity(),
    };
    let (tile, build) = ResidentTileBuilder::build(&generator, identity, address, CELLS)?;
    let approximation = ResidentTileBuilder::measure_approximation(&generator, &tile)?;

    let selected_points = [
        [0, 0],
        [CELLS, 0],
        [0, CELLS],
        [CELLS, CELLS],
        [32, 32],
        [16, 16],
        [48, 16],
        [16, 48],
        [48, 48],
        [32, 12],
        [32, 52],
    ];
    let points = selected_points
        .into_iter()
        .map(|[i, j]| node_sample(&generator, &tile, i, j))
        .collect::<Result<Vec<_>>>()?;

    let mut authoritative_material_min = [f64::INFINITY; 4];
    let mut authoritative_material_max = [f64::NEG_INFINITY; 4];
    for j in 0..=CELLS {
        for i in 0..=CELLS {
            let direction = canonical_direction(address, i64::from(i), i64::from(j), CELLS)?;
            let weights = generator
                .evaluate_point(SurfaceLocation::new(direction))?
                .material_weights();
            for channel in 0..4 {
                authoritative_material_min[channel] =
                    authoritative_material_min[channel].min(weights[channel]);
                authoritative_material_max[channel] =
                    authoritative_material_max[channel].max(weights[channel]);
            }
        }
    }

    let triangles = [[16, 16], [31, 31], [47, 47]]
        .into_iter()
        .flat_map(|cell| [0, 1].map(move |triangle| (cell, triangle)))
        .map(|(cell, triangle)| triangle_sample(&generator, &tile, cell, triangle))
        .collect::<Result<Vec<_>>>()?;

    let result = json!({
        "schema": "mundaris.resident_tile_samples.v1",
        "fixture": {
            "body_identity": BODY_IDENTITY,
            "definition": {
                "identity": definition.identity().0,
                "seed": definition.seed().0,
                "algorithm": definition.terrain().algorithm().name(),
                "configuration_identity": definition.configuration_identity(),
            },
            "radius_m": RADIUS_M,
            "address": { "face": "positive_z", "level": 9, "x": 157, "y": 39 },
            "cells": CELLS,
            "surface_revision": identity.surface_revision,
            "material_revision": identity.material_revision,
        },
        "tile_key": {
            "body_identity": tile.key.body_identity,
            "definition_words": tile.key.definition_words,
            "radius_bits": tile.key.radius_bits,
            "surface_revision": tile.key.surface_revision,
            "material_revision": tile.key.material_revision,
            "format_version": tile.key.format_version,
            "filter_version": tile.key.filter_version,
            "address": { "face": "positive_z", "level": 9, "x": 157, "y": 39 },
            "cells": tile.key.cells,
        },
        "build_diagnostics": {
            "elapsed_ms": build.elapsed.as_secs_f64() * 1000.0,
            "authoritative_query_count": build.authoritative_query_count,
            "texel_dimensions": build.texel_dimensions,
            "patch_grid_vertex_dimensions": build.patch_grid_vertex_dimensions,
            "payload_bytes": build.payload_bytes,
            "builder_stack_scratch_bytes_estimate": build.builder_stack_scratch_bytes_estimate,
            "surface_query_heap_scratch_bytes": build.surface_query_heap_scratch_bytes,
            "surface_generator_retained_heap_bytes": build.surface_generator_retained_heap_bytes,
            "surface_generator_heap_bound_bytes": build.surface_generator_heap_bound_bytes,
            "nominal_grid_spacing_m": build.nominal_grid_spacing_m,
            "actual_chart_center_grid_spacing_m": build.actual_chart_center_grid_spacing_m,
            "filter_nominal_width_m": build.filter_nominal_width_m,
            "filter_actual_surface_offsets_m": build.filter_actual_surface_offsets_m,
        },
        "tile": {
            "anchor_radius_m": tile.anchor_radius_m,
            "min_max_radial_offset_m": tile.min_max_radial_offset_m,
            "material_min_interior_authoritative": authoritative_material_min,
            "material_max_interior_authoritative": authoritative_material_max,
        },
        "approximation_metrics": {
            "texel_query_count": approximation.texel_query_count,
            "triangle_centroid_query_count": approximation.triangle_centroid_query_count,
            "max_texel_radial_error_m": approximation.max_texel_radial_error_m,
            "rms_texel_radial_error_m": approximation.rms_texel_radial_error_m,
            "max_normal_angular_error_radians": approximation.max_normal_angular_error_radians,
            "rms_normal_angular_error_radians": approximation.rms_normal_angular_error_radians,
            "max_triangle_centroid_error_m": approximation.max_triangle_centroid_error_m,
            "rms_triangle_centroid_error_m": approximation.rms_triangle_centroid_error_m,
        },
        "selected_grid_points": points,
        "rendered_triangle_centroids": triangles,
    });

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_vec_pretty(&result)?)
        .with_context(|| format!("writing {}", output.display()))?;
    Ok(())
}
