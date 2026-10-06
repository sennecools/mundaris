//! Focused CPU benchmark for the production Moon resident-tile builder.
//!
//! Usage: cargo run --locked --release -p mundaris_app --example resident_tile_overhead -- [output.json]
//!
//! Each measured tile is a synchronous, single-threaded, uncached builder call.
//! This does not launch the renderer, exercise a native frame, or measure GPU work.

use anyhow::{Context, Result, ensure};
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity};
use mundaris_app::solar_system::{SolarBody, content};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

const CELLS: u32 = 32;
const REPRESENTATIVE_LODS: [u8; 4] = [4, 8, 12, 16];
const ADJACENT_TILES_PER_LOD: u32 = 4;

#[derive(Default)]
struct Fnv64(u64);

impl Fnv64 {
    fn bytes(&mut self, bytes: &[u8]) {
        if self.0 == 0 {
            self.0 = 0xcbf2_9ce4_8422_2325;
        }
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }

    fn finish(&self) -> u64 {
        if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        }
    }
}

fn payload_hash(tile: &mundaris_app::resident_terrain::TileData) -> String {
    let mut hash = Fnv64::default();
    let key = &tile.key;
    hash.u64(key.body_identity);
    hash.u64(key.radius_bits);
    hash.u64(key.surface_revision);
    hash.u64(key.material_revision);
    hash.u32(key.format_version);
    hash.u32(key.filter_version);
    hash.u32(key.cells);
    hash.u32(key.address.face() as u32);
    hash.u32(u32::from(key.address.level()));
    let [x, y] = key.address.coordinates();
    hash.u32(x);
    hash.u32(y);
    hash.u64(key.definition_words.len() as u64);
    for word in &key.definition_words {
        hash.u64(*word);
    }
    hash.u64(tile.texels.len() as u64);
    hash.u64(tile.anchor_radius_m.to_bits());
    for value in tile.min_max_radial_offset_m {
        hash.u64(value.to_bits());
    }
    for texel in &tile.texels {
        hash.u32(texel.radial_offset_m.to_bits());
        for value in texel.material {
            hash.u32(value.to_bits());
        }
    }
    format!("fnv1a64:{:016x}", hash.finish())
}

fn payload_bits_equal(
    left: &mundaris_app::resident_terrain::TileData,
    right: &mundaris_app::resident_terrain::TileData,
) -> bool {
    left.key == right.key
        && left.anchor_radius_m.to_bits() == right.anchor_radius_m.to_bits()
        && left
            .min_max_radial_offset_m
            .iter()
            .zip(right.min_max_radial_offset_m)
            .all(|(left, right)| left.to_bits() == right.to_bits())
        && left.texels.len() == right.texels.len()
        && left.texels.iter().zip(&right.texels).all(|(left, right)| {
            left.radial_offset_m.to_bits() == right.radial_offset_m.to_bits()
                && left
                    .material
                    .iter()
                    .zip(right.material)
                    .all(|(left, right)| left.to_bits() == right.to_bits())
        })
}

fn address(level: u8, x: u32, y: u32) -> Result<CubePatchAddress> {
    Ok(CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y)?)
}

fn distribution(values: &[f64]) -> Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let sum = sorted.iter().sum::<f64>();
    let p95_index = sorted
        .len()
        .saturating_mul(95)
        .div_ceil(100)
        .saturating_sub(1);
    json!({
        "sample_count": sorted.len(),
        "mean_ms": sum / sorted.len() as f64,
        "median_ms": sorted[sorted.len() / 2],
        "p95_ms_nearest_rank": sorted[p95_index],
        "min_ms": sorted[0],
        "max_ms": sorted[sorted.len() - 1],
    })
}

fn main() -> Result<()> {
    let output = std::env::args_os().nth(1).map(PathBuf::from);
    if let Some(path) = &output {
        ensure!(!path.exists(), "refusing to overwrite {}", path.display());
    }

    let body = content(SolarBody::Moon);
    let seed = body
        .rocky_terrain_seed
        .context("production Moon seed is missing")?;
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(seed),
        TerrainSeed(seed),
        SurfaceAlgorithm::RockyV5,
    );
    let radius_m = body.real_mean_radius_m;
    let generator = SurfaceGenerator::new(&definition, radius_m)?;
    let geological_parameters = definition.terrain().parameters();
    // `SolarSystemSetup::create` inserts Moon fifth (runtime identity 5), and
    // the body begins at terrain revision zero. `planetary::bind` uses that
    // revision for both terrain and material revisions.
    let identity = TileBuildIdentity {
        body_identity: 5,
        surface_revision: 0,
        material_revision: 0,
    };

    let session_started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_millis();

    // One unreported warmup absorbs first-call code/data-page effects. It does
    // not share an address with the measured patches and has no tile cache.
    let warmup_address = address(8, 64, 64)?;
    let (warmup_tile, warmup_diagnostics) =
        ResidentTileBuilder::build(&generator, identity, warmup_address, CELLS)?;
    let warmup_payload_hash = payload_hash(&warmup_tile);
    drop(warmup_tile);

    let mut lod_results = Vec::new();
    let mut all_elapsed_ms = Vec::new();
    for level in REPRESENTATIVE_LODS {
        let midpoint = 1u32 << (level - 1);
        let y = midpoint;
        let first_x = midpoint;
        let mut tiles = Vec::new();
        let mut level_times = Vec::new();
        let mut hashes = Vec::new();
        let mut first_address = None;
        let mut first_tile = None;

        for offset in 0..ADJACENT_TILES_PER_LOD {
            let tile_address = address(level, first_x + offset, y)?;
            first_address.get_or_insert(tile_address);
            let wall_start = Instant::now();
            let (tile, diagnostics) =
                ResidentTileBuilder::build(&generator, identity, tile_address, CELLS)?;
            let wall_elapsed_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
            let hash = payload_hash(&tile);
            level_times.push(wall_elapsed_ms);
            all_elapsed_ms.push(wall_elapsed_ms);
            hashes.push(hash.clone());
            if offset == 0 {
                first_tile = Some(tile.clone());
            }
            tiles.push(json!({
                "address": {
                    "face": "positive_z",
                    "level": level,
                    "x": first_x + offset,
                    "y": y,
                },
                "elapsed_ms_wall": wall_elapsed_ms,
                "builder_elapsed_ms": diagnostics.elapsed.as_secs_f64() * 1000.0,
                "authoritative_query_count": diagnostics.authoritative_query_count,
                "payload_bytes": diagnostics.payload_bytes,
                "payload_hash": hash,
                "numerical_authority": {
                    "anchor_radius_m": tile.anchor_radius_m,
                    "min_max_radial_offset_m": tile.min_max_radial_offset_m,
                    "nominal_grid_spacing_m": diagnostics.nominal_grid_spacing_m,
                    "actual_chart_center_grid_spacing_m": diagnostics.actual_chart_center_grid_spacing_m,
                    "filter_nominal_width_m": diagnostics.filter_nominal_width_m,
                    "filter_actual_surface_offsets_m": diagnostics.filter_actual_surface_offsets_m,
                },
            }));
            drop(tile);
        }

        // Rebuild the first key after all four adjacent builds. Equality means
        // every keyed and derived payload bit hashed identically.
        let deterministic_recheck_address = first_address.context("LOD had no measured address")?;
        let (recheck_tile, recheck_diagnostics) =
            ResidentTileBuilder::build(&generator, identity, deterministic_recheck_address, CELLS)?;
        let recheck_hash = payload_hash(&recheck_tile);
        let first_hash = hashes.first().context("LOD had no payload hash")?;
        let bitwise_payload_match = payload_bits_equal(
            first_tile
                .as_ref()
                .context("LOD first tile was not retained")?,
            &recheck_tile,
        );

        lod_results.push(json!({
            "level": level,
            "cells": CELLS,
            "adjacent_tile_count": tiles.len(),
            "timing": distribution(&level_times),
            "independent_four_worker_implied_tiles_per_second":
                4.0 * 1000.0 / (level_times.iter().sum::<f64>() / level_times.len() as f64),
            "independent_four_worker_note": "Arithmetic extrapolation from synchronous single-thread mean; not a parallel, native-frame, GPU, or end-to-end throughput measurement.",
            "tiles": tiles,
            "determinism_recheck": {
                "address": {
                    "face": "positive_z",
                    "level": level,
                    "x": first_x,
                    "y": y,
                },
                "first_payload_hash": first_hash,
                "recheck_payload_hash": recheck_hash,
                "bitwise_payload_match": bitwise_payload_match,
                "recheck_elapsed_ms": recheck_diagnostics.elapsed.as_secs_f64() * 1000.0,
            },
        }));
    }

    let result = json!({
        "schema": "mundaris.resident_tile_overhead.v1",
        "benchmark_kind": "synchronous_cpu_builder_not_native_or_gpu",
        "session_started_unix_ms": session_started_unix_ms,
        "build": {
            "profile": if cfg!(debug_assertions) { "debug" } else { "optimized_release" },
            "package_version": env!("CARGO_PKG_VERSION"),
            "target": std::env::consts::ARCH,
            "worker_count": 1,
            "threading": "serial calls on the main thread; no benchmark workers spawned",
        },
        "fixture": {
            "body_name": body.name,
            "solar_body": "Moon",
            "radius_m": radius_m,
            "body_identity_runtime_order": identity.body_identity,
            "surface_revision": identity.surface_revision,
            "material_revision": identity.material_revision,
            "surface_definition": {
                "identity": definition.identity().0,
                "seed": definition.seed().0,
                "algorithm": definition.terrain().algorithm().name(),
                "configuration_identity": definition.configuration_identity(),
                "geometry_identity": definition.geometry_identity(),
                "material_identity": definition.material_identity(),
                "atmosphere": "Airless",
                "geological_parameters": {
                    "age": geological_parameters.age,
                    "activity": geological_parameters.activity,
                    "resurfacing_fraction": geological_parameters.resurfacing_fraction,
                    "impact_retention": geological_parameters.impact_retention,
                    "relief_fraction": geological_parameters.relief_fraction,
                    "feature_scale_fraction": geological_parameters.feature_scale_fraction,
                    "orientation_radians": geological_parameters.orientation_radians,
                },
            },
        },
        "workload": {
            "cells": CELLS,
            "payload_dimensions": [CELLS + 3, CELLS + 3],
            "representative_levels": REPRESENTATIVE_LODS,
            "same_face_adjacent_tiles_per_level": ADJACENT_TILES_PER_LOD,
            "cache_policy": "no application/GPU/cache insertion; each tile output dropped after hashing; generator shared immutable across synchronous calls",
            "warmup": {
                "excluded": true,
                "address": { "face": "positive_z", "level": 8, "x": 64, "y": 64 },
                "elapsed_ms": warmup_diagnostics.elapsed.as_secs_f64() * 1000.0,
                "payload_hash": warmup_payload_hash,
            },
        },
        "timing_all_measured_tiles": distribution(&all_elapsed_ms),
        "lods": lod_results,
        "limitations": [
            "CPU builder timing excludes selector, scheduling, boundary preparation, GPU upload, rendering, and frame pacing.",
            "Builder and caller timings are separately recorded; neither is a native end-to-end measurement.",
            "Payload hash is stable FNV-1a 64 over key and numeric payload bit patterns; it is a determinism check, not a cryptographic integrity claim.",
            "The four-worker throughput figure is a linear estimate only and omits contention, shared work, queueing, and scaling limits.",
        ],
    });
    let text = serde_json::to_string_pretty(&result)?;
    if let Some(path) = output {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, &text)?;
        println!("{}", path.display());
    } else {
        println!("{text}");
    }
    Ok(())
}
