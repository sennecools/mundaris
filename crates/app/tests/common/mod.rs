//! Shared GPU access for terrain validation tests.
#![allow(dead_code)]

use astrum_app::planet_lod::{producer, select};
use astrum_math::surface::CubePatchAddress;
use astrum_renderer::{AtlasProduceJob, AtlasSource, GpuContext, TerrainAtlasConfig};
use astrum_world::terrain::producer::ProducerRecipe;
use std::sync::Arc;

/// Hardware adapter first, then the platform software adapter (WARP on
/// Windows). With neither, the test fails unless `ASTRUM_SKIP_GPU_TESTS=1`
/// explicitly allows skipping, so a missing GPU is never a silent pass.
pub fn gpu() -> Option<GpuContext> {
    if let Ok(context) = GpuContext::new().or_else(|_| GpuContext::new_software()) {
        let info = context.adapter.get_info();
        println!("adapter: {} ({:?})", info.name, info.backend);
        return Some(context);
    }
    assert!(
        std::env::var("ASTRUM_SKIP_GPU_TESTS").as_deref() == Ok("1"),
        "no GPU or software adapter; set ASTRUM_SKIP_GPU_TESTS=1 to skip explicitly"
    );
    println!("SKIPPED: no adapter and ASTRUM_SKIP_GPU_TESTS=1");
    None
}

/// For a world-map recipe, bake Tier A on the GPU and hand the read-back fields
/// to the CPU oracle, so tile tests compare Tier B on identical Tier A input
/// (Tier A itself is compared in `gpu_tier_a.rs`). No-op for other recipes.
pub fn provide_gpu_world(context: &GpuContext, recipe: &ProducerRecipe) {
    use astrum_world::terrain::{tier_a::TierAFields, world_map::CubeMap};
    let ProducerRecipe::World(world) = recipe else {
        return;
    };
    let inputs = world.field.inputs();
    let gpu = astrum_renderer::tier_a::tier_a_for_validation(
        &context.device,
        &context.queue,
        &astrum_app::planet_lod::tier_a::bake_inputs(inputs),
    )
    .unwrap();
    let n = inputs.face_cells;
    let map = |run: usize| {
        let mut map = CubeMap::new(n, 0.0f32);
        map.data_mut().copy_from_slice(gpu.run(run));
        map
    };
    let words = |run: usize| {
        let mut map = CubeMap::new(n, 0u32);
        map.data_mut().copy_from_slice(&gpu.run_words(run));
        map
    };
    let mut boundary_coord = CubeMap::new(n, 0i32);
    for (o, w) in boundary_coord.data_mut().iter_mut().zip(gpu.run_words(5)) {
        *o = w as i32;
    }
    // The shape runs feed the landform recipes (M2 Shape).
    let shape = astrum_world::terrain::tier_a::TierAShape {
        boundary_coord,
        aux0: words(6),
        aux1: words(7),
        landform: words(8),
    };
    world.field.provide_with_shape(
        TierAFields {
            elevation: map(0),
            temperature: map(1),
            moisture: map(2),
            wind_east: map(3),
            wind_north: map(4),
            sea_level: f64::from(gpu.sea_level),
            noise_low: f64::from(gpu.noise_low),
            noise_high: f64::from(gpu.noise_high),
            ocean_fraction: f64::NAN,
        },
        &shape,
    );
}

/// Producer jobs for `nodes` exactly as the runtime builds them (noise
/// octaves as ladder anchors per tile).
pub fn jobs(
    recipe: &ProducerRecipe,
    radius_m: f64,
    nodes: &[CubePatchAddress],
    cells: u32,
) -> Vec<AtlasProduceJob> {
    nodes
        .iter()
        .enumerate()
        .map(|(index, &node)| {
            let chart = select::chart(node);
            AtlasProduceJob {
                source: 1,
                layer: index as u32,
                token: index as u64 + 1,
                chart: producer::atlas_chart(&chart),
                radius_m: radius_m as f32,
                kind: producer::tile_kind(recipe, node, &chart, cells).unwrap(),
                octaves: producer::detail_octaves(recipe, node, &chart, cells).unwrap(),
            }
        })
        .collect()
}

/// Produce `nodes` (in batches that fit a small validation atlas) and read
/// every tile back.
pub fn produce(
    context: &GpuContext,
    config: TerrainAtlasConfig,
    recipe: &ProducerRecipe,
    radius_m: f64,
    nodes: &[CubePatchAddress],
) -> Vec<astrum_renderer::ProducedTileReadback> {
    let source = Arc::new(producer::atlas_source(recipe).unwrap());
    let sources: Vec<(u64, Arc<AtlasSource>)> = vec![(1, source)];
    let mut out = Vec::with_capacity(nodes.len());
    for batch in nodes.chunks(config.layers as usize) {
        let jobs = jobs(recipe, radius_m, batch, config.cells);
        out.extend(
            astrum_renderer::produce_for_validation(
                &context.device,
                &context.queue,
                config,
                &sources,
                &jobs,
            )
            .unwrap(),
        );
    }
    out
}

/// Deterministic xorshift for reproducible node and input sets.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n)) as u32
    }
}
