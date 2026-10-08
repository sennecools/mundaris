//! Matched scalar-versus-prepared resident tile workloads for Phase 2F.
//!
//! Usage: cargo run --locked --release -p mundaris_app --example phase2f_generation -- <new-output.json>
//! Timings are serial CPU builder measurements, not native-frame or GPU results.

use anyhow::{Context, Result, ensure};
use mundaris_app::resident_terrain::{
    DerivedFieldStats, ResidentTileBuilder, SharedDerivedField, TileBuildIdentity, TileData,
};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, SurfaceQueryContext, TerrainIdentity,
    TerrainSeed,
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, time::Instant};

const CELLS: u32 = 32;
const LEVELS: [u8; 4] = [4, 8, 12, 16];
const REAL_RADIUS_M: f64 = 1_737_400.0;
const GAMEPLAY_RADIUS_M: f64 = REAL_RADIUS_M * (400_000.0 / 6_371_000.0);

fn address(level: u8, x: u32, y: u32) -> Result<CubePatchAddress> {
    Ok(CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y)?)
}

fn addresses(workload: &str, level: u8) -> Result<Vec<CubePatchAddress>> {
    let midpoint = 1u32 << (level - 1);
    let y = midpoint;
    let x = midpoint;
    let mut output = Vec::new();
    match workload {
        "isolated_cold" => output.push(address(level, x, y)?),
        "siblings" => {
            let child_level = level + 1;
            let parent_x = 1u32 << (child_level - 1);
            let parent_y = 1u32 << (child_level - 1);
            for dy in 0..2 {
                for dx in 0..2 {
                    output.push(address(child_level, parent_x + dx, parent_y + dy)?);
                }
            }
        }
        "neighbors" => {
            for offset in 0..4 {
                output.push(address(level, x + offset, y)?);
            }
        }
        "coarse_footprint" => {
            // Four neighboring LOD-4 patches approximate a small visible footprint.
            for dy in 0..2 {
                for dx in 0..2 {
                    output.push(address(4, 8 + dx, 8 + dy)?);
                }
            }
        }
        "movement_neighborhoods" => {
            // Two four-tile neighborhoods share one persistent worker context.
            for shift in [0, 4] {
                for offset in 0..4 {
                    output.push(address(level, x + shift + offset, y)?);
                }
            }
        }
        _ => anyhow::bail!("unknown workload {workload}"),
    }
    Ok(output)
}

fn same_tile_bits(left: &TileData, right: &TileData) -> bool {
    left.key == right.key
        && left.anchor_radius_m.to_bits() == right.anchor_radius_m.to_bits()
        && left
            .min_max_radial_offset_m
            .iter()
            .zip(right.min_max_radial_offset_m)
            .all(|(a, b)| a.to_bits() == b.to_bits())
        && left.texels.len() == right.texels.len()
        && left.texels.iter().zip(&right.texels).all(|(a, b)| {
            a.radial_offset_m.to_bits() == b.radial_offset_m.to_bits()
                && a.material
                    .iter()
                    .zip(b.material)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
        })
}

fn distribution(values: &[f64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let p95 = sorted
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    json!({
        "count": sorted.len(),
        "mean_ms": sorted.iter().sum::<f64>() / sorted.len() as f64,
        "median_ms": sorted[sorted.len() / 2],
        "p95_ms_nearest_rank": sorted[p95],
        "min_ms": sorted[0],
        "max_ms": sorted[sorted.len() - 1],
    })
}

fn profile_json(context: &SurfaceQueryContext<'_>) -> Value {
    let profile = context.profile_stats();
    json!({
        "samples": profile.samples,
        "surface_total_ns": profile.surface_total_ns,
        "shape_ns": profile.shape_ns,
        "geology_ns": profile.geology_ns,
        "legacy_history_ns": profile.legacy_history_ns,
        "legacy_discovery_and_profile_ns": profile.legacy_discovery_and_profile_ns,
        "legacy_chronology_ns": profile.legacy_chronology_ns,
        "province_exclusive_ns": profile.province_exclusive_ns,
        "hierarchy_exclusive_ns": profile.hierarchy_exclusive_ns,
        "final_material_normal_ns": profile.final_material_normal_ns,
    })
}

fn query_stats_json(context: &SurfaceQueryContext<'_>) -> Value {
    let stats = context.stats();
    json!({
        "workspace_bytes": stats.workspace_bytes,
        "feature_hits": stats.feature_hits,
        "feature_misses": stats.feature_misses,
        "controls_hits": stats.controls_hits,
        "controls_misses": stats.controls_misses,
        "prepared_page_hits": stats.prepared_page_hits,
        "prepared_page_misses": stats.prepared_page_misses,
        "prepared_page_lease_acquisitions": stats.prepared_page_lease_acquisitions,
        "prepared_cells": stats.prepared_cells,
        "candidate_discoveries": stats.candidate_discoveries,
        "sample_evaluations": stats.sample_evaluations,
        "preparation_bytes": stats.preparation_bytes,
    })
}

fn store_stats_json(context: &SurfaceQueryContext<'_>) -> Value {
    let stats = context.store_stats();
    json!({
        "capacity_bytes": stats.capacity_bytes,
        "retained_bytes": stats.retained_bytes,
        "high_water_bytes": stats.high_water_bytes,
        "transient_build_reservation_bytes": stats.transient_build_reservation_bytes,
        "pages_built": stats.pages_built,
        "page_hits": stats.page_hits,
        "page_misses": stats.page_misses,
        "page_evictions": stats.page_evictions,
        "capacity_fallbacks": stats.capacity_fallbacks,
        "prepared_cells": stats.prepared_cells,
    })
}

fn derived_stats_json(stats: DerivedFieldStats) -> Value {
    json!({
        "batches": stats.batches,
        "raw_node_queries": stats.raw_node_queries,
        "requested_nodes": stats.requested_nodes,
        "unique_nodes": stats.unique_nodes,
        "cache_hits": stats.cache_hits,
        "cache_misses": stats.cache_misses,
        "cache_evictions": stats.cache_evictions,
        "duplicate_node_evaluations": stats.duplicate_node_evaluations,
        "retained_cache_bytes": stats.retained_cache_bytes,
        "cache_capacity_bytes": stats.cache_capacity_bytes,
        "last_batch_scratch_bytes": stats.last_batch_scratch_bytes,
        "batch_scratch_bound_bytes": stats.batch_scratch_bound_bytes,
    })
}

fn tile_row(
    generators: [&SurfaceGenerator; 3],
    identity: TileBuildIdentity,
    address: CubePatchAddress,
    contexts: [&mut SurfaceQueryContext<'_>; 2],
    derived_field: &SharedDerivedField,
) -> Result<Value> {
    let [scalar_generator, prepared_generator, derived_generator] = generators;
    let [prepared_context, derived_context] = contexts;
    let scalar_started = Instant::now();
    let (scalar, scalar_diagnostics) =
        ResidentTileBuilder::build_uncached(scalar_generator, identity, address, CELLS)?;
    let scalar_wall_ms = scalar_started.elapsed().as_secs_f64() * 1000.0;
    let prepared_started = Instant::now();
    let (prepared, prepared_diagnostics) = ResidentTileBuilder::build_prepared(
        prepared_generator,
        identity,
        address,
        CELLS,
        prepared_context,
        || false,
    )?;
    let prepared_wall_ms = prepared_started.elapsed().as_secs_f64() * 1000.0;
    let prepared_bitwise_match = same_tile_bits(&scalar, &prepared);

    let derived_started = Instant::now();
    let (derived, derived_diagnostics) = ResidentTileBuilder::build_derived(
        derived_generator,
        identity,
        address,
        CELLS,
        derived_context,
        derived_field,
        || false,
    )?;
    let derived_wall_ms = derived_started.elapsed().as_secs_f64() * 1000.0;
    let mut max_radial_difference_m = 0.0f64;
    let mut radial_square_difference_sum = 0.0f64;
    let mut max_material_difference = 0.0f64;
    for (exact, reconstructed) in scalar.texels.iter().zip(&derived.texels) {
        let difference =
            (f64::from(exact.radial_offset_m) - f64::from(reconstructed.radial_offset_m)).abs();
        max_radial_difference_m = max_radial_difference_m.max(difference);
        radial_square_difference_sum += difference * difference;
        for (a, b) in exact.material.iter().zip(reconstructed.material) {
            max_material_difference =
                max_material_difference.max((f64::from(*a) - f64::from(b)).abs());
        }
    }
    let radial_sample_count = scalar.texels.len().max(1) as f64;
    let anchor_radius_bitwise_match =
        scalar.anchor_radius_m.to_bits() == derived.anchor_radius_m.to_bits();
    let derived_rebuild_started = Instant::now();
    let (derived_rebuild, derived_rebuild_diagnostics) = ResidentTileBuilder::build_derived(
        derived_generator,
        identity,
        address,
        CELLS,
        derived_context,
        derived_field,
        || false,
    )?;
    let derived_rebuild_wall_ms = derived_rebuild_started.elapsed().as_secs_f64() * 1000.0;
    let derived_rebuild_bitwise_match = same_tile_bits(&derived, &derived_rebuild);
    Ok(json!({
        "address": {
            "face": "positive_z",
            "level": address.level(),
            "x": address.coordinates()[0],
            "y": address.coordinates()[1],
        },
        "scalar": {
            "wall_ms": scalar_wall_ms,
            "builder_ms": scalar_diagnostics.elapsed.as_secs_f64() * 1000.0,
            "evaluation_ms": scalar_diagnostics.evaluation_elapsed.as_secs_f64() * 1000.0,
            "filter_packing_ms": scalar_diagnostics.filter_packing_elapsed.as_secs_f64() * 1000.0,
            "authoritative_queries": scalar_diagnostics.authoritative_query_count,
        },
        "prepared": {
            "wall_ms": prepared_wall_ms,
            "builder_ms": prepared_diagnostics.elapsed.as_secs_f64() * 1000.0,
            "evaluation_ms": prepared_diagnostics.evaluation_elapsed.as_secs_f64() * 1000.0,
            "filter_packing_ms": prepared_diagnostics.filter_packing_elapsed.as_secs_f64() * 1000.0,
            "authoritative_queries": prepared_diagnostics.authoritative_query_count,
            "prepared_page_hits": prepared_diagnostics.prepared_page_hits,
            "prepared_page_misses": prepared_diagnostics.prepared_page_misses,
            "prepared_cells": prepared_diagnostics.prepared_cells,
            "candidate_discoveries": prepared_diagnostics.candidate_discoveries,
            "preparation_bytes": prepared_diagnostics.preparation_bytes,
            "leased_preparation_bytes": prepared_diagnostics.leased_preparation_bytes,
            "retained_generator_bytes": prepared_diagnostics.surface_generator_retained_heap_bytes,
            "generator_heap_bound_bytes": prepared_diagnostics.surface_generator_heap_bound_bytes,
        },
        "prepared_full_tile_payload_bitwise_match": prepared_bitwise_match,
        "derived": {
            "wall_ms": derived_wall_ms,
            "builder_ms": derived_diagnostics.elapsed.as_secs_f64() * 1000.0,
            "evaluation_ms": derived_diagnostics.evaluation_elapsed.as_secs_f64() * 1000.0,
            "filter_packing_ms": derived_diagnostics.filter_packing_elapsed.as_secs_f64() * 1000.0,
            "actual_authoritative_queries": derived_diagnostics.authoritative_query_count,
            "requested_filter_taps": derived_diagnostics.requested_filter_tap_count,
            "retained_field_bytes": derived_diagnostics.derived_field_retained_bytes,
            "field_capacity_bytes": derived_diagnostics.derived_field_capacity_bytes,
            "anchor_radius_bitwise_match": anchor_radius_bitwise_match,
            "max_filtered_radial_difference_m": max_radial_difference_m,
            "rms_filtered_radial_difference_m": (radial_square_difference_sum / radial_sample_count).sqrt(),
            "max_material_weight_difference": max_material_difference,
            "representation_filter_version": derived.key.filter_version,
            "field_stats_after_build": derived_stats_json(derived_field.stats()?),
        },
        "derived_warm_rebuild": {
            "wall_ms": derived_rebuild_wall_ms,
            "builder_ms": derived_rebuild_diagnostics.elapsed.as_secs_f64() * 1000.0,
            "actual_authoritative_queries": derived_rebuild_diagnostics.authoritative_query_count,
            "requested_filter_taps": derived_rebuild_diagnostics.requested_filter_tap_count,
            "payload_bitwise_match": derived_rebuild_bitwise_match,
            "field_stats_after_rebuild": derived_stats_json(derived_field.stats()?),
        },
    }))
}

fn profiled_scalar_layer_sample(
    generator: &SurfaceGenerator,
    identity: TileBuildIdentity,
    address: CubePatchAddress,
) -> Result<Value> {
    let mut context = generator.profiled_query_context();
    let (tile, diagnostics) = ResidentTileBuilder::build_prepared(
        generator,
        identity,
        address,
        CELLS,
        &mut context,
        || false,
    )?;
    Ok(json!({
        "note": "Separate opt-in profile sample through the scalar feature path; excluded from primary timings.",
        "elapsed_ms_profiled": diagnostics.elapsed.as_secs_f64() * 1000.0,
        "profile": profile_json(&context),
        "query_counters": query_stats_json(&context),
        "tile_payload_bytes": tile.texels.len() * std::mem::size_of::<mundaris_app::resident_terrain::TileTexel>(),
    }))
}

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("provide a new output JSON path")?,
    );
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );

    let identity_word = 0x4d4f_4f4e;
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(identity_word),
        TerrainSeed(identity_word),
        SurfaceAlgorithm::RockyV5,
    );
    let build_identity = TileBuildIdentity {
        body_identity: 5,
        surface_revision: 0,
        material_revision: 0,
    };
    let mut rows = Vec::new();
    for (radius_name, radius_m) in [("gameplay", GAMEPLAY_RADIUS_M), ("real", REAL_RADIUS_M)] {
        let profile_generator = SurfaceGenerator::new(&definition, radius_m)?;
        let mut profiled = Vec::new();
        for level in LEVELS {
            profiled.push(json!({
                "level": level,
                "sample": profiled_scalar_layer_sample(
                    &profile_generator,
                    build_identity,
                    address(level, 1u32 << (level - 1), 1u32 << (level - 1))?,
                )?,
            }));
        }
        for level in LEVELS.into_iter().chain([20]) {
            for workload in [
                "isolated_cold",
                "siblings",
                "neighbors",
                "coarse_footprint",
                "movement_neighborhoods",
            ] {
                let tile_addresses = addresses(workload, level)?;
                // A fresh generator gives each group an empty shared page store.
                // Contexts within the group remain persistent across its jobs.
                let scalar_generator = SurfaceGenerator::new(&definition, radius_m)?;
                let prepared_generator = SurfaceGenerator::new(&definition, radius_m)?;
                let derived_generator = SurfaceGenerator::new(&definition, radius_m)?;
                let mut prepared_context = prepared_generator.prepared_query_context();
                let mut derived_context = derived_generator.prepared_query_context();
                let derived_field = SharedDerivedField::new(&derived_generator)?;
                let mut tile_rows = Vec::with_capacity(tile_addresses.len());
                let mut scalar_times = Vec::with_capacity(tile_addresses.len());
                let mut prepared_times = Vec::with_capacity(tile_addresses.len());
                let mut bitwise_matches = true;
                for tile_address in tile_addresses {
                    let row = tile_row(
                        [&scalar_generator, &prepared_generator, &derived_generator],
                        build_identity,
                        tile_address,
                        [&mut prepared_context, &mut derived_context],
                        &derived_field,
                    )?;
                    scalar_times.push(row["scalar"]["wall_ms"].as_f64().unwrap_or_default());
                    prepared_times.push(row["prepared"]["wall_ms"].as_f64().unwrap_or_default());
                    bitwise_matches &= row["prepared_full_tile_payload_bitwise_match"] == true;
                    tile_rows.push(row);
                }
                rows.push(json!({
                    "radius_fixture": radius_name,
                    "radius_m": radius_m,
                    "level": level,
                    "measurement_tier": if level == 20 { "derived_scaling" } else { "baseline" },
                    "workload": workload,
                    "cells": CELLS,
                    "tile_count": tile_rows.len(),
                    "scalar_wall": distribution(&scalar_times),
                    "prepared_wall": distribution(&prepared_times),
                    "full_payloads_bitwise_match": bitwise_matches,
                    "store_stats_after_workload": store_stats_json(&prepared_context),
                    "worker_context_stats_after_workload": query_stats_json(&prepared_context),
                    "derived_field_stats_after_workload": derived_stats_json(derived_field.stats()?),
                    "tiles": tile_rows,
                }));
            }
        }
        rows.push(json!({
            "radius_fixture": radius_name,
            "radius_m": radius_m,
            "scalar_layer_profiles": profiled,
        }));
    }
    let result = json!({
        "schema": "mundaris.phase2f.generation-comparison.v1",
        "benchmark_kind": "serial_cpu_resident_tile_builder_scalar_prepared_and_derived",
        "build": {
            "profile": if cfg!(debug_assertions) { "debug" } else { "optimized_release" },
            "package_version": env!("CARGO_PKG_VERSION"),
            "target_arch": std::env::consts::ARCH,
            "worker_count": 1,
            "threading": "serial tile builds; each workload group shares one persistent worker context",
        },
        "fixture": {
            "definition_identity": identity_word,
            "terrain_seed": identity_word,
            "algorithm": "RockyV5",
            "body_identity": build_identity.body_identity,
            "surface_revision": build_identity.surface_revision,
            "material_revision": build_identity.material_revision,
            "real_radius_m": REAL_RADIUS_M,
            "gameplay_radius_m": GAMEPLAY_RADIUS_M,
            "baseline_levels": LEVELS,
            "derived_scaling_levels": [20],
            "cells": CELLS,
            "workloads": ["isolated_cold", "siblings", "neighbors", "coarse_footprint", "movement_neighborhoods"],
        },
        "batch_semantics": "The app batches bounded rows. SurfaceQueryContext::evaluate_batch traverses each point in input order; prepared legacy spatial descriptor pages/windows are shared across nearby points and later tile jobs. Point-dependent support/profile/gradient/composition work remains per sample.",
        "limitations": [
            "Serial CPU builder measurements exclude scheduler queues, native frames, GPU upload, rendering and parallel contention.",
            "Scalar wall timing uses the uncached exact builder; prepared timing includes lazy descriptor page construction on misses.",
            "Layer timings use separate opt-in scalar-context runs and are not added to each other or treated as independently causal stages.",
            "Coarse footprint is four adjacent LOD-4 patches; movement is two sequential four-neighbor groups sharing a worker context.",
        ],
        "rows": rows,
    });
    if let Some(parent) = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, serde_json::to_string_pretty(&result)?)?;
    println!("{}", output.display());
    Ok(())
}
