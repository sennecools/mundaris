//! Developer preview of a Tier A CPU bake (ignored test): equirectangular
//! PNGs of hillshaded elevation, moisture, uplift and sediment/flow, plus the
//! erosion and drainage statistics. Run with
//! `ASTRUM_TIER_A_PREVIEW=<dir> [ASTRUM_TIER_A_CELLS=128] cargo test --release
//! -p astrum_world --lib tier_a::preview -- --ignored --nocapture`.
use super::{bake_full, erosion, tests::inputs};
use glam::DVec3;
use std::{fs::File, io::BufWriter, path::Path};

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create png"));
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgb))
        .expect("write png");
}

fn ramp(t: f64, stops: &[(f64, [f64; 3])]) -> [f64; 3] {
    let t = t.clamp(stops[0].0, stops[stops.len() - 1].0);
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            let f = (t - a) / (b - a).max(1e-12);
            return std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * f);
        }
    }
    stops[stops.len() - 1].1
}

#[test]
#[ignore]
fn preview() {
    let Ok(dir) = std::env::var("ASTRUM_TIER_A_PREVIEW") else {
        return;
    };
    let n: usize = std::env::var("ASTRUM_TIER_A_CELLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    let seed: u64 = std::env::var("ASTRUM_TIER_A_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let out = Path::new(&dir);
    std::fs::create_dir_all(out).unwrap();
    let mut input = inputs(n, seed);
    // Overrides: ASTRUM_TIER_A_SET="name=value,name=value".
    if let Ok(set) = std::env::var("ASTRUM_TIER_A_SET") {
        for pair in set.split(',').filter(|p| !p.is_empty()) {
            let (name, value) = pair.split_once('=').expect("name=value");
            let value: f64 = value.trim().parse().expect("number");
            let p = &mut input.params;
            match name.trim() {
                "flow_exponent" => p.erosion_flow_exponent = value,
                "capacity" => p.erosion_capacity = value,
                "thermal_rate" => p.thermal_rate = value,
                "area_exponent" => p.erosion_area_exponent = value,
                "sediment_depth_m" => p.sediment_depth_m = value,
                name => p.set(name, value).expect("known parameter in range"),
            }
        }
    }
    if std::env::var("ASTRUM_TIER_A_PARAMS").is_ok() {
        println!("{:#?}", input.params);
    }
    let started = std::time::Instant::now();
    let output = bake_full(&input).unwrap();
    let d = &output.diagnostics;
    println!("bake {n}²: {:.1} s", started.elapsed().as_secs_f64());
    println!("erosion {:?}", d.erosion);
    let r = input.radius_m;
    println!("drainage before {:?}", erosion::drainage(&d.pre_erosion, r));
    println!("drainage after  {:?}", erosion::drainage(&d.eroded, r));
    for q in [50.0, 200.0, 1000.0] {
        for u in [0.0f32, 0.3, 0.6] {
            println!(
                "slope-area Q>{q} uplift>{u}: {:?}",
                erosion::slope_area_fit(&d.eroded, &d.discharge, r, q, |k| {
                    let up = d.uplift.data()[k];
                    (up >= u).then(|| f64::from((1.0 - 0.8 * d.hardness.data()[k]) / up.max(0.05)))
                })
            );
        }
    }
    // Relief change on land: mean and 95th percentile of lowering.
    let mut lowering: Vec<f64> = d
        .pre_erosion
        .data()
        .iter()
        .zip(d.eroded.data())
        .filter(|(a, _)| **a >= 0.0)
        .map(|(a, b)| f64::from(*a) - f64::from(*b))
        .collect();
    lowering.sort_by(f64::total_cmp);
    let pick = |q: f64| lowering[((lowering.len() - 1) as f64 * q) as usize];
    println!(
        "land lowering m: p5 {:.0} p50 {:.0} p95 {:.0} p99 {:.0}; max elevation {:.0}",
        pick(0.05),
        pick(0.5),
        pick(0.95),
        pick(0.99),
        output
            .fields
            .elevation
            .data()
            .iter()
            .fold(0.0f32, |a, b| a.max(*b))
    );
    let f = &output.fields;
    let (w, h) = (1024u32, 512u32);
    let mut images = vec![vec![0u8; (w * h * 3) as usize]; 4];
    let light = DVec3::new(-1.0, 1.0, 0.6).normalize();
    for y in 0..h {
        let lat = std::f64::consts::FRAC_PI_2
            - std::f64::consts::PI * (f64::from(y) + 0.5) / f64::from(h);
        for x in 0..w {
            let lon = std::f64::consts::TAU * (f64::from(x) + 0.5) / f64::from(w);
            let dir = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let elevation = f.elevation.bicubic(dir);
            // Hillshade from east/north differences (exaggerated 4×).
            let east = DVec3::Y.cross(dir).normalize_or(DVec3::X);
            let north = dir.cross(east);
            let step = 0.5 / n as f64;
            let ge = (f.elevation.bicubic((dir + east * step).normalize())
                - f.elevation.bicubic((dir - east * step).normalize()))
                / (2.0 * step * r);
            let gn = (f.elevation.bicubic((dir + north * step).normalize())
                - f.elevation.bicubic((dir - north * step).normalize()))
                / (2.0 * step * r);
            let normal = (DVec3::new(-ge * 4.0, -gn * 4.0, 1.0)).normalize();
            let shade = (0.35 + 0.65 * normal.dot(light).max(0.0)).min(1.2);
            let base = if elevation < 0.0 {
                ramp(
                    elevation / input.params.ocean_depth_m,
                    &[(-1.5, [0.01, 0.03, 0.12]), (0.0, [0.2, 0.45, 0.7])],
                )
            } else {
                ramp(
                    elevation,
                    &[
                        (0.0, [0.25, 0.5, 0.25]),
                        (1200.0, [0.6, 0.55, 0.35]),
                        (3000.0, [0.55, 0.45, 0.4]),
                        (5000.0, [0.95, 0.95, 0.95]),
                    ],
                )
            };
            let shaded = if elevation < 0.0 {
                base
            } else {
                base.map(|c| c * shade)
            };
            let moisture = ramp(
                f.moisture.bilinear(dir),
                &[
                    (0.0, [0.8, 0.6, 0.3]),
                    (0.5, [0.3, 0.7, 0.3]),
                    (1.0, [0.1, 0.3, 0.8]),
                ],
            );
            let uplift = ramp(
                d.uplift.bilinear(dir),
                &[(0.0, [0.0; 3]), (1.0, [1.0, 0.3, 0.1])],
            );
            let flow =
                (1.0 + d.discharge.bilinear(dir).max(0.0)).ln() * erosion::flow_normaliser(r);
            let sediment =
                1.0 - (-d.deposit.bilinear(dir).max(0.0) / input.params.sediment_depth_m).exp();
            let land = if elevation < 0.0 { 0.3 } else { 1.0 };
            let water = [
                sediment * land,
                (flow * flow * 1.4).min(1.0) * land,
                (flow * 1.2).min(1.0) * land,
            ];
            let k = ((y * w + x) * 3) as usize;
            for (image, colour) in images.iter_mut().zip([shaded, moisture, uplift, water]) {
                for c in 0..3 {
                    image[k + c] = (colour[c].clamp(0.0, 1.0) * 255.0) as u8;
                }
            }
        }
    }
    for (name, image) in ["relief", "moisture", "uplift", "flow"].iter().zip(&images) {
        write_png(&out.join(format!("{name}_{n}_{seed}.png")), w, h, image);
    }
}
