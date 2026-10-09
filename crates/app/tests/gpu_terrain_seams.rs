//! Seam test (pipeline §19.2): neighbouring GPU tiles of the same level agree
//! along their shared edge, including across cube faces. The adjacent-level
//! case (fine edges on the coarse line) is checked by the f64 draw replica in
//! `planet_lod::tests`, since it depends on draw-time morphing.
mod common;

use astrum_app::shared_system::SharedTestSystem;
use astrum_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use astrum_renderer::TerrainAtlasConfig;
use astrum_world::terrain::{SurfaceGenerator, producer::tile_texel_m};

/// Each tile is within the oracle tolerance of `gpu_terrain_oracle.rs`
/// (max(1 mm, 1e-5 texel) + 1e-6 |h|), so two tiles agree within twice that.
fn seam_tolerance_m(texel_m: f64, height_m: f64) -> f64 {
    2.0 * (1.0e-3_f64.max(1.0e-5 * texel_m) + 1.0e-6 * height_m.abs())
}

/// Height texel `(i, j)` of grid point `k` along `edge` (one-texel apron).
fn edge_texel(edge: PatchEdge, k: u32, cells: u32) -> (usize, usize) {
    let [i, j] = edge.grid(k, cells);
    (i as usize + 1, j as usize + 1)
}

#[test]
fn neighbouring_gpu_tiles_agree_along_shared_edges() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(9).unwrap()).unwrap();
    let policy = loaded.lod;
    let finest = policy.max_level - policy.data_level_offset();
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    // Pairs: random interior edges plus face-boundary nodes whose neighbour
    // lies on another cube face.
    let mut rng = common::Rng(0x5ea_5eed);
    let mut pairs: Vec<(CubePatchAddress, PatchEdge)> = Vec::new();
    for index in 0..48 {
        let level = 1 + rng.below(u32::from(finest)) as u8;
        let count = 1u32 << level;
        let face = CubeFace::ALL[rng.below(6) as usize];
        let edge = PatchEdge::ALL[rng.below(4) as usize];
        let (x, y) = if index % 2 == 0 {
            // On the face boundary in the chosen edge's direction.
            let along = rng.below(count);
            match edge {
                PatchEdge::UMin => (0, along),
                PatchEdge::UMax => (count - 1, along),
                PatchEdge::VMin => (along, 0),
                PatchEdge::VMax => (along, count - 1),
            }
        } else {
            (rng.below(count), rng.below(count))
        };
        pairs.push((CubePatchAddress::try_new(face, level, x, y).unwrap(), edge));
    }
    let mut nodes = Vec::new();
    for &(node, edge) in &pairs {
        nodes.push(node);
        nodes.push(node.neighbor(edge).address);
    }
    let mut checked_bodies = 0;
    for (_, body) in loaded.system.bodies().filter(|(_, b)| b.has_surface()) {
        let definition = body.surface_definition().unwrap();
        let radius = body.properties().reference_radius_m();
        let recipe = SurfaceGenerator::new(definition, radius)
            .unwrap()
            .producer_recipe()
            .unwrap();
        common::provide_gpu_world(&context, &recipe);
        let tiles = common::produce(&context, config, &recipe, radius, &nodes);
        let side = config.height_side() as usize;
        let (mut worst, mut worst_ratio, mut cross_face) = (0.0f64, 0.0f64, 0);
        for (index, &(node, edge)) in pairs.iter().enumerate() {
            let neighbour = node.neighbor(edge);
            if neighbour.address.face() != node.face() {
                cross_face += 1;
            }
            let a = &tiles[2 * index];
            let b = &tiles[2 * index + 1];
            let texel = tile_texel_m(radius, node.level(), config.cells);
            for k in 0..=config.cells {
                let (ai, aj) = edge_texel(edge, k, config.cells);
                let kb = if neighbour.reversed {
                    config.cells - k
                } else {
                    k
                };
                let (bi, bj) = edge_texel(neighbour.edge, kb, config.cells);
                let ha = f64::from(a.heights[aj * side + ai]);
                let hb = f64::from(b.heights[bj * side + bi]);
                let gap = (ha - hb).abs();
                let allowed = seam_tolerance_m(texel, ha);
                assert!(
                    gap <= allowed,
                    "{} {node:?} {edge:?} k={k}: {ha} vs {hb} ({gap} m > {allowed} m)",
                    body.name()
                );
                worst = worst.max(gap);
                worst_ratio = worst_ratio.max(gap / allowed);
            }
        }
        println!(
            "{}: {} edge pairs ({cross_face} across cube faces), max seam gap {worst:.6} m \
             ({worst_ratio:.2} of tolerance)",
            body.name(),
            pairs.len()
        );
        assert!(cross_face >= 10);
        checked_bodies += 1;
    }
    assert_eq!(checked_bodies, 2);
}
