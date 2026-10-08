//! Matched CPU field-density experiment. Timings include fresh source preparation.
use anyhow::{Context, Result};
use mundaris_app::resident_terrain::{
    FieldDensity, ResidentTileBuilder, SharedFieldPages, TileBuildIdentity,
};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Instant};

fn main() -> Result<()> {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("provide a new output JSON path")?,
    );
    anyhow::ensure!(!path.exists(), "refusing to replace experiment evidence");
    let identity = TileBuildIdentity {
        body_identity: 5,
        surface_revision: 1,
        material_revision: 1,
    };
    let mut cases = Vec::new();
    for radius in [109_081.776_8, 1_737_400.0] {
        for level in [4, 12, 16] {
            let count = 1u32 << level;
            let address =
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, count / 2, count / 2)?;
            let definition = SurfaceDefinition::generated(
                TerrainIdentity(0x4d4f_4f4e),
                TerrainSeed(2),
                SurfaceAlgorithm::MoonFieldsV1,
            );
            let source = SurfaceGenerator::new(&definition, radius)?;
            let exact_started = Instant::now();
            let (exact, exact_cost) = ResidentTileBuilder::build(&source, identity, address, 32)?;
            let exact_wall_ms = exact_started.elapsed().as_secs_f64() * 1000.0;
            for repetition in 0..3 {
                for density in [
                    FieldDensity::Cells32,
                    FieldDensity::Cells64,
                    FieldDensity::Cells128,
                ] {
                    let started = Instant::now();
                    let generator = Arc::new(SurfaceGenerator::new(&definition, radius)?);
                    let pages = SharedFieldPages::new(
                        generator.clone(),
                        identity,
                        density,
                        8 * 1024 * 1024,
                    )?;
                    let preparation_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let (tile, cost) = ResidentTileBuilder::build_fields(
                        &generator, identity, address, 32, &pages,
                    )?;
                    let cold_ms = started.elapsed().as_secs_f64() * 1000.0;
                    let warm_started = Instant::now();
                    let (warm, warm_cost) = ResidentTileBuilder::build_fields(
                        &generator, identity, address, 32, &pages,
                    )?;
                    let warm_ms = warm_started.elapsed().as_secs_f64() * 1000.0;
                    anyhow::ensure!(tile == warm, "nondeterministic rebuild");
                    let mut max_radial = 0.0f64;
                    let mut radial_squared = 0.0;
                    let mut max_material = 0.0f64;
                    for (a, b) in tile.texels.iter().zip(&exact.texels) {
                        let delta = (tile.anchor_radius_m + f64::from(a.radial_offset_m)
                            - exact.anchor_radius_m
                            - f64::from(b.radial_offset_m))
                        .abs();
                        max_radial = max_radial.max(delta);
                        radial_squared += delta * delta;
                        for (x, y) in a.material.into_iter().zip(b.material) {
                            max_material = max_material.max((f64::from(x) - f64::from(y)).abs());
                        }
                    }
                    let metrics = ResidentTileBuilder::measure_approximation(&generator, &tile)?;
                    let stats = pages.statistics();
                    cases.push(json!({"radius_m":radius,"level":level,"face":"PositiveZ","coordinates":address.coordinates(),
                        "seed":2,"repetition":repetition,"mesh_cells":32,"field_cells":density.cells(),
                        "filter_version":density.filter_version(),"source_preparation_ms":preparation_ms,
                        "cold_including_preparation_ms":cold_ms,"warm_ms":warm_ms,"exact_filtered_ms":exact_wall_ms,
                        "exact_queries":exact_cost.authoritative_query_count,"cold_queries":cost.authoritative_query_count,
                        "warm_queries":warm_cost.authoritative_query_count,
                        "field_vs_exact_filtered_max_height_m":max_radial,
                        "field_vs_exact_filtered_rms_height_m":(radial_squared/tile.texels.len() as f64).sqrt(),
                        "field_vs_exact_filtered_max_material_component":max_material,
                        "represented_vs_full_source_max_height_m":metrics.max_texel_radial_error_m,
                        "represented_vs_full_source_max_normal_radians":metrics.max_normal_angular_error_radians,
                        "mesh_triangle_centroid_max_error_m":metrics.max_triangle_centroid_error_m,
                        "field_capacity_bytes":stats.capacity_bytes,"field_retained_bytes":stats.retained_bytes,
                        "page_allocations":stats.page_allocations,"evictions":stats.evictions,
                        "target_certifiable":false}));
                }
            }
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&json!({"schema_version":1,"algorithm":"MoonFieldsV1",
        "note":"CPU-only density discrimination; not native acceptance or matched CPU/GPU performance", "cases":cases}))?,
    )?;
    Ok(())
}
