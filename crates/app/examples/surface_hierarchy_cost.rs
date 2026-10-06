//! Fixed-input complete-query cost observations, separate from rendering.
use anyhow::{Context, Result, bail};
use glam::DVec3;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::{
    ShapeDefinition, SurfaceAlgorithm, SurfaceAtmosphere, SurfaceDefinition, SurfaceGenerator,
    SurfaceMaterialDefinition, SurfaceTerrainDefinition, TerrainIdentity, TerrainSeed,
};
use serde_json::json;
use std::{env, fs, hint::black_box, path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let output = PathBuf::from(
        env::args_os()
            .nth(1)
            .context("provide a new output JSON path")?,
    );
    if output.exists() {
        bail!("refusing existing evidence {}", output.display());
    }
    let count = 4096;
    let points: Vec<_> = (0..count)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / count as f64;
            let angle = i as f64 * std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
            let radial = (1.0 - y * y).sqrt();
            Ok(SurfaceLocation::new(Direction3::try_new(DVec3::new(
                radial * angle.cos(),
                y,
                radial * angle.sin(),
            ))?))
        })
        .collect::<Result<_>>()?;
    let mut rows = Vec::new();
    for (parent, successor) in [
        (SurfaceAlgorithm::RockyV4, SurfaceAlgorithm::RockyV5),
        (SurfaceAlgorithm::IcyV2, SurfaceAlgorithm::IcyV3),
        (SurfaceAlgorithm::VolcanicV2, SurfaceAlgorithm::VolcanicV3),
    ] {
        for seed in [0, 7, 19] {
            let phenotype =
                SurfaceTerrainDefinition::generated(successor, TerrainSeed(seed)).parameters();
            for radius_m in [80_000.0, 109_000.0, 1_200_000.0] {
                let generators: Vec<_> = [parent, successor]
                    .into_iter()
                    .map(|algorithm| {
                        let definition = SurfaceDefinition::new(
                            TerrainIdentity(0x511b02),
                            TerrainSeed(seed),
                            ShapeDefinition::sphere(),
                            SurfaceTerrainDefinition::new(algorithm, phenotype)?,
                            SurfaceMaterialDefinition::new(
                                SurfaceDefinition::generated(
                                    TerrainIdentity(0x511b02),
                                    TerrainSeed(seed),
                                    algorithm,
                                )
                                .material()
                                .version(),
                                0.5,
                                0.4,
                            )?,
                            SurfaceAtmosphere::Airless,
                        )?;
                        Ok(SurfaceGenerator::new(&definition, radius_m)?)
                    })
                    .collect::<Result<_>>()?;
                // Warm compilation/runtime paths equally before alternating order.
                for generator in &generators {
                    for point in &points[..64] {
                        black_box(generator.evaluate_point(*point)?);
                    }
                }
                for repetition in 0..3 {
                    for index in if repetition % 2 == 0 { [0, 1] } else { [1, 0] } {
                        let generator = &generators[index];
                        let start = Instant::now();
                        let mut cells = 0u64;
                        let mut candidates = 0u64;
                        let mut accepted = Some(0u64);
                        let mut checksum = 0.0;
                        for point in &points {
                            let sample = black_box(generator.evaluate_point(*point)?);
                            cells += u64::from(sample.work().cells_visited);
                            candidates += u64::from(sample.work().candidate_features);
                            accepted = accepted
                                .zip(sample.work().accepted_features)
                                .map(|(a, b)| a + u64::from(b));
                            checksum += sample.terrain().height_m() + sample.normal().x;
                        }
                        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                        rows.push(json!({
                            "algorithm":generator.definition().terrain().algorithm().name(),
                            "seed":seed,"radius_m":radius_m,"repetition":repetition,
                            "query_count":count,"elapsed_ms":elapsed_ms,"microseconds_per_query":elapsed_ms * 1000.0/count as f64,
                            "cells_visited":cells,"candidate_features":candidates,"accepted_features":accepted,
                            "checksum":checksum,"geometry_identity":generator.definition().geometry_identity(),
                            "parameters":{"age":phenotype.age,"activity":phenotype.activity,"resurfacing_fraction":phenotype.resurfacing_fraction,
                                "impact_retention":phenotype.impact_retention,"relief_fraction":phenotype.relief_fraction,
                                "feature_scale_fraction":phenotype.feature_scale_fraction,"orientation_radians":phenotype.orientation_radians}
                        }));
                    }
                }
            }
        }
    }
    fs::write(
        output,
        serde_json::to_vec_pretty(&json!({
            "scope":"Single-process complete f64 surface queries; identical authored identity, seed, radius, material controls and successor phenotype for each parent/successor pair. Three order-alternated scans. Excludes generator construction, diagnostics, rasterization and GPU/native work.",
            "point_distribution":"4096 fixed equal-area Fibonacci directions","rows":rows
        }))?,
    )?;
    Ok(())
}
