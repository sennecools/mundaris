//! Coastline stability across LOD levels (diagnostic, ignored): on coastal
//! samples of Rust (|macro elevation| < 200 m, a Fibonacci lattice over the
//! sphere), the CPU oracle composition per level band limit, split into the
//! macro (Tier A mip), landform relief and detail noise (unblended), the
//! candidate coast blend applied to them (not in the producer), and the
//! oracle itself. Reports per level
//! the land fraction and the share of samples whose land/ocean flips from the
//! previous level, for the macro alone, macro + landforms, and the full
//! surface; that is the coastline "pop" a viewer sees when tiles refine.
//! Run with `cargo test -p astrum_app --test coast_stability -- --ignored
//! --nocapture` (ASTRUM_COAST_SAMPLES to change the lattice size).
mod common;

use astrum_app::shared_system::SharedTestSystem;
use astrum_world::terrain::{
    SurfaceGenerator,
    producer::{ProducerRecipe, tile_texel_m},
};
use glam::DVec3;

#[test]
#[ignore]
fn coastline_flips_between_lod_levels() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(5).unwrap()).unwrap();
    let cells = loaded.lod.tile_cells;
    let (_, body) = loaded
        .system
        .bodies()
        .find(|(_, b)| b.name() == "Rust")
        .expect("Rust in the canonical system");
    let radius = body.properties().reference_radius_m();
    let recipe = SurfaceGenerator::new(body.surface_definition().unwrap(), radius)
        .unwrap()
        .producer_recipe()
        .unwrap();
    common::provide_gpu_world(&context, &recipe);
    let ProducerRecipe::World(world) = &recipe else {
        panic!("Rust is a world-map body");
    };
    let maps = world.field.maps().unwrap();
    let total: usize = std::env::var("ASTRUM_COAST_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400_000);
    let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
    let coastal: Vec<DVec3> = (0..total)
        .map(|k| {
            let y = 1.0 - 2.0 * (k as f64 + 0.5) / total as f64;
            let r = (1.0 - y * y).sqrt();
            let a = golden * k as f64;
            DVec3::new(r * a.cos(), y, r * a.sin())
        })
        .filter(|d| maps.sample(0, *d).0.abs() < 200.0)
        .collect();
    // ASTRUM_COAST_NEAR="x,y,z": the closest shoreline sample (|macro| < 3 m).
    if let Ok(v) = std::env::var("ASTRUM_COAST_NEAR") {
        let c: Vec<f64> = v.split(',').map(|x| x.trim().parse().unwrap()).collect();
        let target = DVec3::new(c[0], c[1], c[2]).normalize();
        let shore = coastal
            .iter()
            .filter(|d| maps.sample(0, **d).0.abs() < 3.0)
            .max_by(|a, b| a.dot(target).total_cmp(&b.dot(target)))
            .unwrap();
        println!(
            "shoreline near target: {:.6} {:.6} {:.6} ({:.1} km away)",
            shore.x,
            shore.y,
            shore.z,
            shore.angle_between(target) * radius / 1000.0
        );
    }
    println!(
        "Rust radius {radius:.0} m, tile cells {cells}: {} coastal samples of {total}",
        coastal.len()
    );
    println!(
        "level  texel_m  mip | land% macro  +landform  full  blended  oracle | flips% macro  +landform  full  blended  oracle"
    );
    // Candidate fix: relief scaled by m²/(m² + A²) near sea level (A from
    // ASTRUM_COAST_BLEND_M, default 40 m): no flips while A ≥ |relief|/2.
    let blend: f64 = std::env::var("ASTRUM_COAST_BLEND_M")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(40.0);
    let mut previous: Option<[Vec<bool>; 5]> = None;
    for level in 2u8..=14 {
        let texel = tile_texel_m(radius, level, cells);
        let mip = maps.mip_for(texel, radius);
        let mut land: [Vec<bool>; 5] = Default::default();
        for d in &coastal {
            let macro_h = maps.sample(mip, *d).0;
            let landform = world
                .field
                .landform_relief(*d, texel)
                .unwrap()
                .map_or(0.0, |r| r.value);
            let detail = world
                .detail_noise
                .as_ref()
                .map_or(0.0, |n| n.evaluate(*d * radius, Some(texel)).0);
            land[0].push(macro_h >= 0.0);
            land[1].push(macro_h + landform >= 0.0);
            land[2].push(macro_h + landform + detail >= 0.0);
            let keep = macro_h * macro_h / (macro_h * macro_h + blend * blend);
            land[3].push(macro_h + (landform + detail) * keep >= 0.0);
            land[4].push(recipe.evaluate(*d, texel).unwrap().height_m >= 0.0);
        }
        let percent = |v: &[bool]| 100.0 * v.iter().filter(|&&b| b).count() as f64 / v.len() as f64;
        let flips = |a: &[bool], b: &[bool]| {
            100.0 * a.iter().zip(b).filter(|(x, y)| x != y).count() as f64 / a.len() as f64
        };
        let f = previous.as_ref().map_or([0.0; 5], |p| {
            std::array::from_fn(|i| flips(&p[i], &land[i]))
        });
        println!(
            "{level:>5} {texel:>8.0} {mip:>4} | {:>10.2} {:>10.2} {:>5.2} {:>8.2} {:>7.2} | {:>11.2} {:>10.2} {:>5.2} {:>8.2} {:>7.2}",
            percent(&land[0]),
            percent(&land[1]),
            percent(&land[2]),
            percent(&land[3]),
            percent(&land[4]),
            f[0],
            f[1],
            f[2],
            f[3],
            f[4]
        );
        if level == 3
            && let Some(p) = &previous
        {
            // Where the unblended surface pops most: coarse direction bins.
            let mut bins: std::collections::HashMap<[i32; 3], usize> = Default::default();
            for (k, d) in coastal.iter().enumerate() {
                if p[2][k] != land[2][k] {
                    *bins
                        .entry((*d * 12.0).round().as_ivec3().to_array())
                        .or_default() += 1;
                }
            }
            let mut top: Vec<_> = bins.into_iter().collect();
            top.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
            let lit = glam::DVec3::new(-0.92, 0.148, 0.36).normalize();
            for (bin, count) in top.iter().take(40) {
                let d = glam::DVec3::new(bin[0] as f64, bin[1] as f64, bin[2] as f64).normalize();
                println!(
                    "  pop cluster {count:>5} flips near {:.3} {:.3} {:.3} (lit dot {:.2})",
                    d.x,
                    d.y,
                    d.z,
                    d.dot(lit)
                );
            }
        }
        previous = Some(land);
    }
}
