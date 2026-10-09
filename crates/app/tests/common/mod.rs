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

/// Producer jobs for `nodes` exactly as the runtime builds them.
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
