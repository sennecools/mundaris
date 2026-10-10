//! Surface colour of world-map bodies (pipeline §10, M1 World map and planet
//! editor): GPU albedo pages vs the CPU oracle, the flat-water mask, colour
//! stability between nested pages and the archetype's declared average colour.
//! Runs by default on a hardware or software adapter.
//!
//! Tolerances:
//! - Albedo pages hold the land colour everywhere (water is laid over it in
//!   the draw through the coast contour in the normal page's w: the signed
//!   distance to sea level in two-texel units, clamped; its sign is checked).
//! - Albedo: pages store sRGB in 8 bits. Half a step in linear colour is
//!   about 0.0045 · c^0.58 (0.0045 at albedo 1, 0.0012 at 0.1), plus f32
//!   climate sampling; allowed 0.001 + 0.005 · c^0.6 per channel. Texels whose height is within 5 cm of sea level are skipped:
//!   f32 height differences there may legitimately flip water and land.
//! - Stability: a coarse page and the finer pages nested in it must agree in
//!   mean colour over the same area. They read climate mips one or more levels
//!   apart and the LUT is not linear, so the means differ slightly; allowed
//!   0.01 per channel in linear colour (about 4 % of a mid-grey).
//! - Average colour: area-weighted mean albedo of the body vs the archetype's
//!   declared `average_colour`, shown while the bake runs (§6.1); 0.005 per
//!   channel (about 5 % of these channels), so changing the LUT, snow, water
//!   or archetype without updating the declared colour fails here.
mod common;

use astrum_app::{planet_lod::select, shared_system::SharedTestSystem};
use astrum_math::surface::{CubeFace, CubePatchAddress};
use astrum_renderer::TerrainAtlasConfig;
use astrum_world::terrain::{SurfaceGenerator, producer::ProducerRecipe, producer::tile_texel_m};
use glam::DVec3;

const ALBEDO_TOLERANCE: f64 = 0.001;
const ALBEDO_TOLERANCE_SCALE: f64 = 0.005;
const SEA_LEVEL_MARGIN_M: f64 = 0.05;
/// `WATER_CONTOUR_TEXELS` in terrain_atlas_produce.wgsl.
const WATER_CONTOUR_TEXELS: f64 = 2.0;
const STABILITY_TOLERANCE: f64 = 0.01;
const AVERAGE_TOLERANCE: f64 = 0.005;

struct Body {
    name: String,
    radius: f64,
    recipe: ProducerRecipe,
    /// Archetype `average_colour` of a world-map body.
    average: Option<[f64; 3]>,
}

fn bodies(context: &astrum_renderer::GpuContext) -> (Vec<Body>, TerrainAtlasConfig) {
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(11).unwrap()).unwrap();
    let policy = loaded.lod;
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    let bodies = loaded
        .system
        .bodies()
        .filter(|(_, b)| b.has_surface())
        .map(|(_, body)| {
            let radius = body.properties().reference_radius_m();
            let recipe = SurfaceGenerator::new(body.surface_definition().unwrap(), radius)
                .unwrap()
                .producer_recipe()
                .unwrap();
            common::provide_gpu_world(context, &recipe);
            let average = body.surface_definition().unwrap().world().map(|w| {
                let c = w.archetype.average_colour;
                [c.0, c.1, c.2].map(f64::from)
            });
            Body {
                name: body.name().to_string(),
                radius,
                recipe,
                average,
            }
        })
        .collect();
    (bodies, config)
}

fn max_channel(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0, f64::max)
}

#[test]
fn gpu_albedo_pages_match_the_cpu_oracle_and_water_follows_sea_level() {
    let Some(context) = common::gpu() else {
        return;
    };
    let (bodies, config) = bodies(&context);
    let mut rng = common::Rng(0xa1be_d0c0);
    let mut nodes: Vec<_> = CubeFace::ALL
        .iter()
        .map(|&f| CubePatchAddress::root(f))
        .collect();
    while nodes.len() < 40 {
        let level = 1 + rng.below(12) as u8;
        let count = 1u32 << level;
        let face = CubeFace::ALL[rng.below(6) as usize];
        nodes.push(
            CubePatchAddress::try_new(face, level, rng.below(count), rng.below(count)).unwrap(),
        );
    }
    let mut world_bodies = 0;
    for body in &bodies {
        let tiles = common::produce(&context, config, &body.recipe, body.radius, &nodes);
        let owns_colour = matches!(body.recipe, ProducerRecipe::World(_));
        let nside = config.normal_side() as usize;
        let ncells = (config.cells * config.normal_scale) as usize;
        let (mut worst, mut water, mut land, mut skipped) = (0.0f64, 0usize, 0usize, 0usize);
        for (node, tile) in nodes.iter().zip(&tiles) {
            if !owns_colour {
                assert!(
                    tile.albedo.iter().all(|a| a[3] == 0.0) && tile.water.iter().all(|w| *w == 0.0),
                    "{}: page claims colour or water",
                    body.name
                );
                continue;
            }
            let chart = select::chart(*node);
            let texel = tile_texel_m(body.radius, node.level(), config.cells);
            let page_texel = texel / f64::from(config.normal_scale);
            for j in (0..nside).step_by(5) {
                for i in (0..nside).step_by(5) {
                    let st = [
                        (i as f64 - 1.0) / ncells as f64,
                        (j as f64 - 1.0) / ncells as f64,
                    ];
                    let d = chart.direction(st);
                    let sample = body.recipe.evaluate(d, texel).unwrap();
                    if sample.height_m.abs() < SEA_LEVEL_MARGIN_M {
                        skipped += 1;
                        continue;
                    }
                    let reference = body.recipe.page_albedo(d, texel).unwrap().unwrap();
                    let a = tile.albedo[j * nside + i];
                    // World pages always own their colour; alpha carries
                    // 1 − coast softness (coast coverage) for the draw.
                    assert!(
                        (0.0..=1.0).contains(&a[3]),
                        "{} {node:?}: alpha {} is no coast softness",
                        body.name,
                        a[3]
                    );
                    let gpu = [a[0], a[1], a[2]].map(f64::from);
                    let error = (0..3)
                        .map(|k| {
                            (gpu[k] - reference[k]).abs()
                                / (ALBEDO_TOLERANCE
                                    + ALBEDO_TOLERANCE_SCALE * reference[k].powf(0.6))
                        })
                        .fold(0.0, f64::max);
                    assert!(
                        error <= 1.0,
                        "{} {node:?} ({i},{j}) h {:.3}: gpu {a:?} vs {reference:?}",
                        body.name,
                        sample.height_m
                    );
                    worst = worst.max(error);
                    // The coast contour in the normal page's w: signed distance
                    // to sea level in units of two page texels (height over
                    // slope), clamped. Its sign defines the coastline; the
                    // magnitude depends on the slope, which a 0.4° normal
                    // difference changes a lot on gentle shelves.
                    let cosine = sample.normal.dot(d.normalize()).clamp(1e-3, 1.0);
                    let slope = ((1.0 - cosine * cosine).sqrt() / cosine).max(1e-3);
                    let contour = (-sample.height_m / (slope * page_texel * WATER_CONTOUR_TEXELS))
                        .clamp(-1.0, 1.0);
                    let stored = f64::from(tile.water[j * nside + i]);
                    if contour.abs() > 0.05 {
                        assert_eq!(
                            stored > 0.0,
                            contour > 0.0,
                            "{} {node:?} ({i},{j}) h {:.3}: contour {stored} vs {contour}",
                            body.name,
                            sample.height_m
                        );
                    }
                    if sample.height_m < 0.0 {
                        water += 1;
                    } else {
                        land += 1;
                    }
                }
            }
        }
        if owns_colour {
            println!(
                "{}: {} pages, max albedo error {worst:.2} of tolerance; {water} water and {land} land texels \
                 ({skipped} within {SEA_LEVEL_MARGIN_M} m of sea level skipped)",
                body.name,
                nodes.len()
            );
            // Both branches are exercised: the water mask follows sea level.
            assert!(water > 100 && land > 100);
            world_bodies += 1;
        }
    }
    assert_eq!(world_bodies, 1);
}

#[test]
fn nested_pages_agree_in_mean_colour() {
    let Some(context) = common::gpu() else {
        return;
    };
    let (bodies, config) = bodies(&context);
    let body = bodies
        .iter()
        .find(|b| matches!(b.recipe, ProducerRecipe::World(_)))
        .expect("a world-map body");
    let nside = config.normal_side() as usize;
    let ncells = (config.cells * config.normal_scale) as usize;
    // Mean linear colour of a page over its chart (cell centres, each the
    // average of its four corner texels; the apron is excluded).
    let mean = |tile: &astrum_renderer::ProducedTileReadback| {
        let mut sum = [0.0f64; 3];
        let mut count = 0.0;
        for j in 1..=ncells {
            for i in 1..=ncells {
                // Texel t sits at st = (t - 1) / ncells; cell i spans texels i, i + 1.
                for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let a = tile.albedo[(j + dj) * nside + (i + di)];
                    for k in 0..3 {
                        sum[k] += f64::from(a[k]);
                    }
                    count += 1.0;
                }
            }
        }
        sum.map(|s| s / count)
    };
    let mut rng = common::Rng(0x57ab_1e00);
    let (mut worst, mut pairs) = (0.0f64, 0);
    for _ in 0..12 {
        let level = 2 + rng.below(6) as u8;
        let count = 1u32 << level;
        let face = CubeFace::ALL[rng.below(6) as usize];
        let coarse =
            CubePatchAddress::try_new(face, level, rng.below(count), rng.below(count)).unwrap();
        // The four children of `coarse` cover its quadrants.
        let children: Vec<_> = (0..4)
            .map(|q| {
                let [x, y] = coarse.coordinates();
                CubePatchAddress::try_new(face, level + 1, 2 * x + q % 2, 2 * y + q / 2).unwrap()
            })
            .collect();
        let mut nodes = vec![coarse];
        nodes.extend(&children);
        let tiles = common::produce(&context, config, &body.recipe, body.radius, &nodes);
        let coarse_mean = mean(&tiles[0]);
        let mut fine = [0.0f64; 3];
        for tile in &tiles[1..] {
            let m = mean(tile);
            for k in 0..3 {
                fine[k] += 0.25 * m[k];
            }
        }
        let error = max_channel(coarse_mean, fine);
        assert!(
            error <= STABILITY_TOLERANCE,
            "{coarse:?}: coarse mean {coarse_mean:?} vs nested {fine:?}"
        );
        worst = worst.max(error);
        pairs += 1;
    }
    println!(
        "{}: {pairs} coarse pages vs their four children, max mean colour difference {worst:.5}",
        body.name
    );
}

#[test]
fn declared_average_colour_matches_the_baked_body() {
    let Some(context) = common::gpu() else {
        return;
    };
    let (bodies, _) = bodies(&context);
    let body = bodies
        .iter()
        .find(|b| matches!(b.recipe, ProducerRecipe::World(_)))
        .expect("a world-map body");
    // Uniform directions over the sphere (Fibonacci lattice) at the coarsest
    // footprint: the colour seen when the whole body is a few pixels.
    let samples = 20_000;
    let texel = 2.0 * body.radius;
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let mut sum = [0.0f64; 3];
    for k in 0..samples {
        let y = 1.0 - 2.0 * (k as f64 + 0.5) / samples as f64;
        let r = (1.0 - y * y).sqrt();
        let a = golden * k as f64;
        let d = DVec3::new(r * a.cos(), y, r * a.sin());
        let c = body.recipe.albedo(d, texel).unwrap().unwrap();
        for i in 0..3 {
            sum[i] += c[i] / samples as f64;
        }
    }
    let declared = body.average.expect("declared average colour");
    let error = max_channel(sum, declared);
    println!(
        "{}: measured average albedo {:.4?}, declared {declared:?} (max channel diff {error:.4})",
        body.name, sum
    );
    assert!(error <= AVERAGE_TOLERANCE);
}
