//! Diagnostic (ignored by default): GPU cost of producing world tiles with
//! and without the landform recipes (M2 Shape), on a matched set of 256
//! level-12 tiles around Rust's highest terrain. Each run builds a fresh
//! producer (pipelines from the driver cache, Tier A bake) and produces all
//! tiles; the minimum of several runs per variant is reported, and the
//! difference attributes the landform cost. Run with `--ignored --nocapture`.
mod common;

use astrum_app::shared_system::SharedTestSystem;
use astrum_math::{
    Direction3,
    surface::{CubePatchAddress, SurfaceLocation},
};
use astrum_renderer::TerrainAtlasConfig;
use astrum_world::terrain::{SurfaceGenerator, producer::ProducerRecipe};
use glam::DVec3;

fn highest_direction(recipe: &ProducerRecipe) -> DVec3 {
    let samples = 2000;
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    (0..samples)
        .map(|k| {
            let y = 1.0 - 2.0 * (k as f64 + 0.5) / samples as f64;
            let r = (1.0 - y * y).sqrt();
            let a = golden * k as f64;
            DVec3::new(r * a.cos(), y, r * a.sin())
        })
        .max_by(|a, b| {
            let h = |d: &DVec3| {
                recipe
                    .evaluate(*d, 20_000.0)
                    .map_or(f64::MIN, |s| s.height_m)
            };
            h(a).total_cmp(&h(b))
        })
        .unwrap()
}

#[test]
#[ignore]
fn producer_tile_cost_with_and_without_landforms() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(5).unwrap()).unwrap();
    let policy = loaded.lod;
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    let (_, body) = loaded
        .system
        .bodies()
        .find(|(_, b)| b.name() == "Rust")
        .unwrap();
    let radius = body.properties().reference_radius_m();
    let with = body.surface_definition().unwrap().clone();
    let world = with.world().unwrap().clone();
    assert!(world.landforms.is_some(), "Rust has landforms");
    let mut bare = world.clone();
    bare.landforms = None;
    let without = with.clone().with_world(bare).unwrap();
    let recipe = |definition| {
        SurfaceGenerator::new(definition, radius)
            .unwrap()
            .producer_recipe()
            .unwrap()
    };
    let (with, without) = (recipe(&with), recipe(&without));
    let peak = highest_direction(&with);
    let (face, uv) = SurfaceLocation::new(Direction3::try_new(peak).unwrap()).face_uv();
    let level = 12u8;
    let count = 1u32 << level;
    let index = |c: f64| (((c + 1.0) * 0.5 * f64::from(count)) as u32).min(count - 16);
    let (x0, y0) = (index(uv[0]), index(uv[1]));
    let nodes: Vec<CubePatchAddress> = (0..16)
        .flat_map(|j| (0..16).map(move |i| (i, j)))
        .map(|(i, j)| CubePatchAddress::try_new(face, level, x0 + i, y0 + j).unwrap())
        .collect();
    let mut report = Vec::new();
    for (name, recipe) in [("without landforms", &without), ("with landforms", &with)] {
        common::provide_gpu_world(&context, recipe);
        let source =
            std::sync::Arc::new(astrum_app::planet_lod::producer::atlas_source(recipe).unwrap());
        let sources = vec![(1u64, source)];
        let batch: Vec<Vec<_>> = nodes
            .chunks(config.layers as usize)
            .map(|chunk| common::jobs(recipe, radius, chunk, config.cells))
            .collect();
        let warmup = 2;
        let rounds = 4;
        let batches: Vec<_> = batch[..warmup]
            .iter()
            .cloned()
            .chain((0..rounds).flat_map(|_| batch.iter().cloned()))
            .collect();
        let times = astrum_renderer::producer_batch_seconds_for_validation(
            &context.device,
            &context.queue,
            config,
            &sources,
            &batches,
            warmup,
        )
        .unwrap();
        let best = times
            .chunks(batch.len())
            .map(|round| round.iter().sum::<f64>())
            .fold(f64::INFINITY, f64::min);
        println!(
            "{name}: {} tiles in {} batches, best round {:.1} ms ({:.3} ms per tile)",
            nodes.len(),
            batch.len(),
            1000.0 * best,
            1000.0 * best / nodes.len() as f64
        );
        report.push(best);
    }
    println!(
        "landform cost: {:.3} ms per tile",
        1000.0 * (report[1] - report[0]) / nodes.len() as f64
    );
}
