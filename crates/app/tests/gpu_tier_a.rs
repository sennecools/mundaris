//! Tier A world-map bake: GPU (the authority, ADR 0023) vs the CPU oracle
//! (`astrum_world::terrain::tier_a`), pipeline §6 and §19.3–19.4.
//!
//! Tolerances: the GPU evaluates in f32. Continent noise differs by ~1e-7
//! relative; elevation scales noise by land height / noise range (~10 km per
//! unit), so f32 alone moves it by millimetres. Sea level can shift by a
//! fraction of a histogram bin if a texel's noise lands on the other side of a
//! bin edge. Temperature and wind are analytic per texel; moisture accumulates
//! f32 rounding over ~100 advection steps.
mod common;

use astrum_app::planet_lod::tier_a::bake_inputs;
use astrum_world::terrain::{
    archetype::PlanetArchetype,
    tier_a::{TierAInputs, area_weight, bake, texel_uv},
};
use glam::DVec3;

const ELEVATION_TOLERANCE_M: f64 = 0.1;
const TEMPERATURE_TOLERANCE_C: f64 = 1.0e-3;
const MOISTURE_TOLERANCE: f64 = 2.0e-4;
const WIND_TOLERANCE: f64 = 5.0e-5;

fn terra() -> PlanetArchetype {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/archetypes/terra.ron");
    ron::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn gpu_tier_a_matches_the_cpu_oracle() {
    let Some(context) = common::gpu() else {
        return;
    };
    let archetype = terra();
    for (n, seed) in [(64usize, 7u64), (128, 11)] {
        let inputs = TierAInputs {
            params: archetype.sample(seed),
            stages: archetype.stages.clone(),
            radius_m: 338_950.0,
            pole: DVec3::Y,
            face_cells: n,
        };
        let cpu = bake(&inputs).unwrap();
        let gpu = astrum_renderer::tier_a::tier_a_for_validation(
            &context.device,
            &context.queue,
            &bake_inputs(&inputs),
        )
        .unwrap();
        let worst = |run: usize, cpu: &[f32]| {
            gpu.run(run)
                .iter()
                .zip(cpu)
                .map(|(g, c)| (f64::from(*g) - f64::from(*c)).abs())
                .fold(0.0f64, f64::max)
        };
        let errors = [
            worst(0, cpu.elevation.data()),
            worst(1, cpu.temperature.data()),
            worst(2, cpu.moisture.data()),
            worst(3, cpu.wind_east.data()),
            worst(4, cpu.wind_north.data()),
        ];
        // Area-weighted ocean fraction of the GPU elevation.
        let (mut ocean, mut total) = (0.0, 0.0);
        for (k, h) in gpu.run(0).iter().enumerate() {
            let (u, v) = texel_uv(k % n, (k / n) % n, n);
            let w = f64::from(area_weight(u, v));
            total += w;
            if *h < 0.0 {
                ocean += w;
            }
        }
        let ocean = ocean / total;
        println!(
            "n={n} seed={seed}: sea level cpu {:.6} gpu {:.6}; max |d| elevation {:.4} m, \
             temperature {:.5} C, moisture {:.6}, wind {:.2e}/{:.2e}; GPU ocean {:.4} (target {:.4})",
            cpu.sea_level,
            gpu.sea_level,
            errors[0],
            errors[1],
            errors[2],
            errors[3],
            errors[4],
            ocean,
            inputs.params.ocean_coverage
        );
        assert!((f64::from(gpu.sea_level) - cpu.sea_level).abs() < 2.0 / 4096.0);
        assert!(
            errors[0] <= ELEVATION_TOLERANCE_M,
            "elevation {}",
            errors[0]
        );
        assert!(
            errors[1] <= TEMPERATURE_TOLERANCE_C,
            "temperature {}",
            errors[1]
        );
        assert!(errors[2] <= MOISTURE_TOLERANCE, "moisture {}", errors[2]);
        assert!(
            errors[3].max(errors[4]) <= WIND_TOLERANCE,
            "wind {errors:?}"
        );
        // §19.4: ocean coverage within ±1 % of the target.
        assert!((ocean - inputs.params.ocean_coverage).abs() < 0.01);
        // Field mips (elevation, temperature, moisture): 2×2 averages per
        // face, same on both sides.
        let fields = [
            (&cpu.elevation, ELEVATION_TOLERANCE_M),
            (&cpu.temperature, TEMPERATURE_TOLERANCE_C),
            (&cpu.moisture, MOISTURE_TOLERANCE),
        ];
        for (field, (map, tolerance)) in fields.into_iter().enumerate() {
            let cpu_mips = astrum_world::terrain::world_field::field_mips(map);
            for (level, mip) in cpu_mips.iter().enumerate() {
                let worst = gpu
                    .field_mip(field, level)
                    .iter()
                    .zip(mip.data())
                    .map(|(g, c)| (f64::from(*g) - f64::from(*c)).abs())
                    .fold(0.0f64, f64::max);
                assert!(worst <= tolerance, "field {field} mip {level}: {worst}");
            }
        }
    }
}
