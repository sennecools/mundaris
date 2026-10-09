//! Field-overlay data of world-map bodies (pipeline §18.1, M1 planet editor):
//! the GPU climate page (temperature °C, moisture, wind east, wind north) vs
//! the CPU oracle. Runs by default on a hardware or software adapter.
//!
//! Tolerances:
//! - Storage is `rgba16float`: half a unit in the last place is a relative
//!   error of 2^-11 (about 4.9e-4). The GPU samples in f32 and the oracle in
//!   f64 over the same cube-map blend, so the remainder is f32 rounding of
//!   the sampling weights. Allowed per channel: 1.5e-3 · |reference| + 1e-3
//!   (about 0.1 °C at 40 °C; 1e-3 of moisture or wind speed near zero).
//! - Temperature and moisture read the climate mip matching the node
//!   (`maps.mip_for`, as the albedo); wind always reads level 0.
//! - Bodies without a world map (the Moon) must write all zeros.
mod common;

use astrum_app::{planet_lod::select, shared_system::SharedTestSystem};
use astrum_math::surface::{CubeFace, CubePatchAddress};
use astrum_renderer::TerrainAtlasConfig;
use astrum_world::terrain::{SurfaceGenerator, producer::ProducerRecipe, producer::tile_texel_m};

const RELATIVE_TOLERANCE: f64 = 1.5e-3;
const ABSOLUTE_TOLERANCE: f64 = 1.0e-3;

#[test]
fn gpu_climate_pages_match_the_cpu_oracle() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(11).unwrap()).unwrap();
    let policy = loaded.lod;
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    let mut rng = common::Rng(0x0c11_a7e5);
    // Level 0 (the six roots) up to level 12.
    let mut nodes: Vec<_> = CubeFace::ALL
        .iter()
        .map(|&f| CubePatchAddress::root(f))
        .collect();
    while nodes.len() < 30 {
        let level = 1 + rng.below(12) as u8;
        let count = 1u32 << level;
        let face = CubeFace::ALL[rng.below(6) as usize];
        nodes.push(
            CubePatchAddress::try_new(face, level, rng.below(count), rng.below(count)).unwrap(),
        );
    }
    let nside = config.normal_side() as usize;
    let ncells = (config.cells * config.normal_scale) as usize;
    let (mut world_bodies, mut empty_bodies) = (0, 0);
    for (_, body) in loaded.system.bodies().filter(|(_, b)| b.has_surface()) {
        let radius = body.properties().reference_radius_m();
        let recipe = SurfaceGenerator::new(body.surface_definition().unwrap(), radius)
            .unwrap()
            .producer_recipe()
            .unwrap();
        common::provide_gpu_world(&context, &recipe);
        let tiles = common::produce(&context, config, &recipe, radius, &nodes);
        let ProducerRecipe::World(world) = &recipe else {
            for tile in &tiles {
                assert!(
                    tile.climate.iter().all(|c| *c == [0.0; 4]),
                    "{}: climate page is not zero",
                    body.name()
                );
            }
            empty_bodies += 1;
            continue;
        };
        let maps = world.field.maps().unwrap();
        let mut worst = [0.0f64; 4];
        let mut samples = 0usize;
        for (node, tile) in nodes.iter().zip(&tiles) {
            let chart = select::chart(*node);
            let texel = tile_texel_m(radius, node.level(), config.cells);
            let level = maps.mip_for(texel, radius);
            for j in (0..nside).step_by(5) {
                for i in (0..nside).step_by(5) {
                    let st = [
                        (i as f64 - 1.0) / ncells as f64,
                        (j as f64 - 1.0) / ncells as f64,
                    ];
                    let d = chart.direction(st);
                    let (temperature, moisture) = maps.climate(level, d);
                    let reference = [
                        temperature,
                        moisture,
                        maps.fields.wind_east.bilinear(d),
                        maps.fields.wind_north.bilinear(d),
                    ];
                    let gpu = tile.climate[j * nside + i];
                    for k in 0..4 {
                        let error = (f64::from(gpu[k]) - reference[k]).abs()
                            / (ABSOLUTE_TOLERANCE + RELATIVE_TOLERANCE * reference[k].abs());
                        assert!(
                            error <= 1.0,
                            "{} {node:?} ({i},{j}) channel {k}: gpu {gpu:?} vs {reference:?}",
                            body.name()
                        );
                        worst[k] = worst[k].max(error);
                    }
                    samples += 1;
                }
            }
        }
        println!(
            "{}: {} pages, {samples} texels, max climate error of tolerance: temperature {:.3}, \
             moisture {:.3}, wind east {:.3}, wind north {:.3}",
            body.name(),
            nodes.len(),
            worst[0],
            worst[1],
            worst[2],
            worst[3]
        );
        world_bodies += 1;
    }
    assert_eq!(world_bodies, 1);
    assert!(empty_bodies >= 1, "a body without a world map is checked");
}
