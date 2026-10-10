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
    let normal_scale = f64::from(loaded.lod.normal_scale);
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
    // Coast coverage (the producer's page_water + the draw's water mask):
    // expected water coverage per sample from the contour widened by k·σ.
    let k = world.field.inputs().params.coast_coverage_k;
    let smoothstep = |e0: f64, e1: f64, x: f64| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let mut previous: Option<[Vec<bool>; 5]> = None;
    let mut previous_coverage: Option<Vec<f64>> = None;
    let mut coverage_report = Vec::new();
    for level in 2u8..=14 {
        let texel = tile_texel_m(radius, level, cells);
        let mip = maps.mip_for(texel, radius);
        let mut land: [Vec<bool>; 5] = Default::default();
        let mut lanes = [0.0f64; 4];
        if let Some(l) = world.field.landforms() {
            for (lane, (landform, params)) in
                l.set.landforms().iter().zip(&l.params).take(4).enumerate()
            {
                lanes[lane] = landform.program.unresolved_bound_m(params, texel);
            }
        }
        let detail_unresolved = world
            .detail_noise
            .as_ref()
            .map_or(0.0, |n| n.unresolved_bound_m(texel));
        let mut coverage = Vec::with_capacity(coastal.len());
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
            let sample = recipe.evaluate(*d, texel).unwrap();
            land[4].push(sample.height_m >= 0.0);
            let w = world
                .field
                .landform_weights(*d)
                .unwrap()
                .unwrap_or([0.0; 4]);
            let sigma = (0.25
                * (w.iter()
                    .zip(&lanes)
                    .map(|(w, u)| w.max(0.0) * u)
                    .sum::<f64>()
                    + detail_unresolved))
                .min(15.0);
            let c = sample.normal.dot(*d).clamp(1e-3, 1.0);
            let slope = ((1.0 - c * c).sqrt() / c).max(1e-3);
            // Page texels; coverage fades in from 40 to 160 m page texels.
            let page_texel = texel / normal_scale;
            let geometric = slope * page_texel * 2.0;
            let blur = k * sigma * smoothstep(40.0, 160.0, page_texel);
            let width = geometric.hypot(blur);
            let softness = blur / width;
            let contour = (-sample.height_m / width).clamp(-1.0, 1.0);
            let edge = 0.03 + (0.9 - 0.03) * softness;
            coverage.push(smoothstep(-edge, edge, contour));
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
        if let Some(p) = &previous_coverage {
            let change = 100.0
                * p.iter()
                    .zip(&coverage)
                    .map(|(a, b)| (a - b).abs())
                    .sum::<f64>()
                / coverage.len() as f64;
            coverage_report.push((level, change));
        }
        previous_coverage = Some(coverage);
        previous = Some(land);
    }
    // Mean |coverage change| per transition (%), with the coast coverage of
    // k = coast_coverage_k; compare with the oracle flips% column (k = 0).
    for (level, change) in &coverage_report {
        println!("coverage change into level {level}: {change:.3} % (k = {k})");
    }
}

/// Coast coverage on a dense patch (diagnostic, ignored): around
/// ASTRUM_COAST_PATCH="x,y,z" (default: the user's 512 km coast), a grid of
/// ASTRUM_COAST_SPACING_M (100 m) over ±ASTRUM_COAST_HALF_KM (20 km). Per LOD
/// level, the water coverage a viewer sees, hard (k = 0: water where h < 0)
/// and with the coast coverage of `coast_coverage_k`. The pop from level L to
/// L + 1 is the mean |Δ coverage| averaged over blocks of two level-L texels
/// (what one coarse pixel shows). ASTRUM_COAST_PNG=<dir> writes the coverage
/// maps per level.
#[test]
#[ignore]
fn coast_coverage_pops_between_lod_levels() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(5).unwrap()).unwrap();
    let cells = loaded.lod.tile_cells;
    let normal_scale = f64::from(loaded.lod.normal_scale);
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
    let env = |name: &str, default: f64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let centre = std::env::var("ASTRUM_COAST_PATCH").map_or(
        DVec3::new(-0.138, 0.391, 0.910).normalize(),
        |v| {
            let c: Vec<f64> = v.split(',').map(|x| x.trim().parse().unwrap()).collect();
            DVec3::new(c[0], c[1], c[2]).normalize()
        },
    );
    let spacing = env("ASTRUM_COAST_SPACING_M", 100.0);
    let half = env("ASTRUM_COAST_HALF_KM", 20.0) * 1000.0;
    let n = (2.0 * half / spacing) as usize;
    let east = DVec3::Y.cross(centre).normalize();
    let north = centre.cross(east);
    let directions: Vec<DVec3> = (0..n * n)
        .map(|k| {
            let (i, j) = (k % n, k / n);
            let u = (i as f64 + 0.5) * spacing - half;
            let v = half - (j as f64 + 0.5) * spacing;
            (centre + (east * u + north * v) / radius).normalize()
        })
        .collect();
    let k = env(
        "ASTRUM_COAST_K",
        world.field.inputs().params.coast_coverage_k,
    );
    let smoothstep = |e0: f64, e1: f64, x: f64| {
        let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let water_only = std::env::var("ASTRUM_COAST_WATER_ONLY").is_ok();
    let png_dir = std::env::var("ASTRUM_COAST_PNG").ok();
    let levels: Vec<u8> = (1..=9).collect();
    // Per level: (hard, soft) coverage per sample.
    let mut maps: Vec<(Vec<f64>, Vec<f64>)> = Vec::new();
    for &level in &levels {
        let texel = tile_texel_m(radius, level, cells);
        let mut lanes = [0.0f64; 4];
        if let Some(l) = world.field.landforms() {
            for (lane, (landform, params)) in
                l.set.landforms().iter().zip(&l.params).take(4).enumerate()
            {
                lanes[lane] = landform.program.unresolved_bound_m(params, texel);
            }
        }
        let detail_unresolved = world
            .detail_noise
            .as_ref()
            .map_or(0.0, |n| n.unresolved_bound_m(texel));
        let (mut hard, mut soft) = (Vec::with_capacity(n * n), Vec::with_capacity(n * n));
        for d in &directions {
            let sample = recipe.evaluate(*d, texel).unwrap();
            let w = world
                .field
                .landform_weights(*d)
                .unwrap()
                .unwrap_or([0.0; 4]);
            let sigma = (0.25
                * (w.iter()
                    .zip(&lanes)
                    .map(|(w, u)| w.max(0.0) * u)
                    .sum::<f64>()
                    + detail_unresolved))
                .min(15.0);
            let c = sample.normal.dot(*d).clamp(1e-3, 1.0);
            let slope = ((1.0 - c * c).sqrt() / c).max(1e-3);
            // Page texels; coverage fades in from 40 to 160 m page texels.
            let page_texel = texel / normal_scale;
            let geometric = slope * page_texel * 2.0;
            let mask = |kk: f64| {
                let mut blur = kk * sigma * smoothstep(40.0, 160.0, page_texel);
                if water_only && sample.height_m >= 0.0 {
                    blur = 0.0;
                }
                let width = geometric.hypot(blur);
                let softness = blur / width;
                let contour = (-sample.height_m / width).clamp(-1.0, 1.0);
                let edge = 0.03 + (0.9 - 0.03) * softness;
                smoothstep(-edge, edge, contour)
            };
            hard.push(mask(0.0));
            soft.push(mask(k));
        }
        if let Some(dir) = &png_dir {
            for (name, values) in [("hard", &hard), ("soft", &soft)] {
                let rgb: Vec<u8> = values
                    .iter()
                    .flat_map(|c| {
                        let land = [0.85, 0.78, 0.55];
                        let water = [0.1, 0.45, 0.6];
                        (0..3).map(move |i| ((land[i] + (water[i] - land[i]) * c) * 255.0) as u8)
                    })
                    .collect();
                let path = std::path::Path::new(dir).join(format!("coverage-{name}-L{level}.png"));
                std::fs::create_dir_all(dir).unwrap();
                let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
                let mut encoder = png::Encoder::new(file, n as u32, n as u32);
                encoder.set_color(png::ColorType::Rgb);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&rgb)
                    .unwrap();
            }
        }
        maps.push((hard, soft));
    }
    println!(
        "patch ±{:.0} km at {spacing:.0} m around {centre:?}, k = {k}",
        half / 1000.0
    );
    println!("transition  block_m | pop% hard  pop% soft");
    for (index, pair) in maps.windows(2).enumerate() {
        let level = levels[index];
        let block = (2.0 * tile_texel_m(radius, level, cells) / spacing)
            .ceil()
            .max(1.0) as usize;
        let blocks = n / block;
        let pop = |a: &[f64], b: &[f64]| {
            let mut total = 0.0;
            for by in 0..blocks {
                for bx in 0..blocks {
                    let (mut sa, mut sb) = (0.0, 0.0);
                    for y in by * block..(by + 1) * block {
                        for x in bx * block..(bx + 1) * block {
                            sa += a[y * n + x];
                            sb += b[y * n + x];
                        }
                    }
                    total += (sa - sb).abs() / (block * block) as f64;
                }
            }
            100.0 * total / (blocks * blocks).max(1) as f64
        };
        println!(
            "{level:>2} -> {:<4} {:>8.0} | {:>9.3} {:>10.3}",
            level + 1,
            block as f64 * spacing,
            pop(&pair[0].0, &pair[1].0),
            pop(&pair[0].1, &pair[1].1)
        );
    }
}

fn read_rgb(path: &std::path::Path) -> (usize, usize, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.line_size / w;
    let rgb = (0..w * h)
        .flat_map(|k| {
            let o = (k / w) * info.line_size + (k % w) * channels;
            [buffer[o], buffer[o + 1], buffer[o + 2]]
        })
        .collect();
    (w, h, rgb)
}

/// Capture metrics (diagnostic, ignored).
/// - ASTRUM_DIFF="a.png,b.png": pixels changed by more than 8/255, the mean
///   and largest channel difference.
/// - ASTRUM_SWEEP="dir": per `*-sweep-AAA` nadir capture (altitude AAA km),
///   water fraction and coast edges per pixel of a central crop covering the
///   ground the 100 km frame shows; jumps between steps are pops.
#[test]
#[ignore]
fn capture_metrics() {
    if let Ok(pair) = std::env::var("ASTRUM_DIFF") {
        let paths: Vec<&str> = pair.split(',').collect();
        let (w, h, a) = read_rgb(std::path::Path::new(paths[0]));
        let (_, _, b) = read_rgb(std::path::Path::new(paths[1]));
        let (mut changed, mut sum, mut max) = (0usize, 0.0f64, 0u8);
        for k in 0..w * h {
            let d = (0..3)
                .map(|c| a[3 * k + c].abs_diff(b[3 * k + c]))
                .max()
                .unwrap();
            if d > 8 {
                changed += 1;
            }
            sum += f64::from(d);
            max = max.max(d);
        }
        println!(
            "diff: {:.3} % pixels changed (> 8), mean {:.3}, max {max}",
            100.0 * changed as f64 / (w * h) as f64,
            sum / (w * h) as f64
        );
    }
    if let Ok(dir) = std::env::var("ASTRUM_SWEEP") {
        let mut frames: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let km = name.rsplit("-sweep-").next()?.parse().ok()?;
                name.contains("-sweep-")
                    .then(|| (km, e.path().join("viewport.png")))
            })
            .collect();
        frames.sort_by_key(|f| std::cmp::Reverse(f.0));
        let mut previous: Option<(f64, f64)> = None;
        for (km, path) in frames {
            let (w, h, rgb) = read_rgb(&path);
            let side = ((h as f64) * 100.0 / f64::from(km)).round().max(8.0) as usize;
            let (x0, y0) = ((w - side) / 2, (h - side) / 2);
            let water = |x: usize, y: usize| {
                let o = 3 * ((y0 + y) * w + x0 + x);
                i32::from(rgb[o + 2]) > i32::from(rgb[o]) + 12
            };
            let (mut wet, mut edges) = (0usize, 0usize);
            for y in 0..side {
                for x in 0..side {
                    wet += usize::from(water(x, y));
                    if x + 1 < side && water(x, y) != water(x + 1, y) {
                        edges += 1;
                    }
                    if y + 1 < side && water(x, y) != water(x, y + 1) {
                        edges += 1;
                    }
                }
            }
            let fraction = 100.0 * wet as f64 / (side * side) as f64;
            // Edges per unit length of the footprint (scale free).
            let density = edges as f64 / side as f64;
            let jump = previous.map_or(String::new(), |(f, e)| {
                format!(
                    "  Δwater {:+.2} pp, Δedges {:+.2}",
                    fraction - f,
                    density - e
                )
            });
            println!(
                "{km:>4} km: crop {side:>4} px, water {fraction:6.2} %, edges/px {density:6.2}{jump}"
            );
            previous = Some((fraction, density));
        }
    }
}
