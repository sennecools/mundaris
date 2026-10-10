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

/// Gnomonic patch around one direction (ignored test): hillshades of the
/// final elevation, the pre-erosion elevation and the tectonic relief alone
/// (sampled per pixel), plus the nearest boundary class and the plates. Run
/// with `ASTRUM_TIER_A_PATCH=<dir> ASTRUM_TIER_A_CENTRE=x,y,z
/// [ASTRUM_TIER_A_PATCH_KM=400] [ASTRUM_TIER_A_CELLS=256]`.
#[test]
#[ignore]
fn patch() {
    use super::tectonics::{BoundaryClass, plates, tectonic_sample};
    let Ok(dir) = std::env::var("ASTRUM_TIER_A_PATCH") else {
        return;
    };
    let n: usize = std::env::var("ASTRUM_TIER_A_CELLS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let half_km: f64 = std::env::var("ASTRUM_TIER_A_PATCH_KM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400.0);
    let centre: Vec<f64> = std::env::var("ASTRUM_TIER_A_CENTRE")
        .expect("ASTRUM_TIER_A_CENTRE=x,y,z")
        .split(',')
        .map(|v| v.trim().parse().expect("number"))
        .collect();
    let centre = DVec3::new(centre[0], centre[1], centre[2]).normalize();
    let out = Path::new(&dir);
    std::fs::create_dir_all(out).unwrap();
    let input = inputs(n, 7);
    let (p, r) = (&input.params, input.radius_m);
    let output = bake_full(&input).unwrap();
    let (f, d) = (&output.fields, &output.diagnostics);
    let set = plates(p, p.tectonic_seed);
    let size = 512usize;
    // Pixel (x, y) → direction; x east-ish, y north-ish around `centre`.
    let east = DVec3::Y.cross(centre).normalize();
    let north = centre.cross(east);
    let span = half_km * 1000.0 / r;
    let direction = |x: usize, y: usize| {
        let u = (2.0 * (x as f64 + 0.5) / size as f64 - 1.0) * span;
        let v = (1.0 - 2.0 * (y as f64 + 0.5) / size as f64) * span;
        (centre + east * u + north * v).normalize()
    };
    let pixel_m = 2.0 * half_km * 1000.0 / size as f64;
    let field = |value: &dyn Fn(DVec3) -> f64| -> Vec<f64> {
        (0..size * size)
            .map(|k| value(direction(k % size, k / size)))
            .collect()
    };
    let samples: Vec<_> = (0..size * size)
        .map(|k| tectonic_sample(direction(k % size, k / size), &set, p, r))
        .collect();
    let hillshade = |h: &[f64], exaggeration: f64| -> Vec<u8> {
        let light = DVec3::new(-1.0, 1.0, 0.8).normalize();
        let mut image = vec![0u8; size * size * 3];
        for y in 0..size {
            for x in 0..size {
                let at = |x: usize, y: usize| h[y.min(size - 1) * size + x.min(size - 1)];
                let gx = (at(x + 1, y) - at(x.saturating_sub(1), y)) / (2.0 * pixel_m);
                let gy = (at(x, y.saturating_sub(1)) - at(x, y + 1)) / (2.0 * pixel_m);
                let normal = DVec3::new(-gx * exaggeration, -gy * exaggeration, 1.0).normalize();
                let shade = normal.dot(light).clamp(0.0, 1.0);
                let base = if at(x, y) < 0.0 { [0.3, 0.4, 0.7] } else { [0.8, 0.75, 0.6] };
                for c in 0..3 {
                    image[(y * size + x) * 3 + c] = (base[c] * shade * 255.0) as u8;
                }
            }
        }
        image
    };
    let final_h = field(&|dir| f.elevation.bilinear(dir));
    let pre_h = field(&|dir| d.pre_erosion.bilinear(dir));
    let dh: Vec<f64> = samples.iter().map(|s| s.dh_m).collect();
    let crust: Vec<f64> = samples.iter().map(|s| 1000.0 * s.crust).collect();
    let mut classes = vec![0u8; size * size * 3];
    for (k, s) in samples.iter().enumerate() {
        let colour = match s.class {
            BoundaryClass::Interior => [40, 40, 40],
            BoundaryClass::Collision => [230, 120, 40],
            BoundaryClass::Subduction => [200, 60, 160],
            BoundaryClass::IslandArc => [160, 60, 220],
            BoundaryClass::Ridge => [60, 140, 230],
            BoundaryClass::Rift => [60, 200, 200],
            BoundaryClass::Transform => [230, 230, 60],
        };
        // Plate boundaries in black.
        let x = k % size;
        let edge = x + 1 < size && samples[k + 1].plate != s.plate
            || k + size < samples.len() && samples[k + size].plate != s.plate;
        let colour = if edge { [0, 0, 0] } else { colour };
        classes[k * 3..k * 3 + 3].copy_from_slice(&colour);
    }
    let s = size as u32;
    write_png(&out.join("final.png"), s, s, &hillshade(&final_h, 20.0));
    write_png(&out.join("pre_erosion.png"), s, s, &hillshade(&pre_h, 20.0));
    write_png(&out.join("tectonic_dh.png"), s, s, &hillshade(&dh, 20.0));
    write_png(&out.join("crust.png"), s, s, &hillshade(&crust, 20.0));
    write_png(&out.join("classes.png"), s, s, &classes);
    println!("patch ±{half_km} km around {centre:?}, {pixel_m:.0} m pixels");
}

/// Plate boundaries drawn over a native capture (ignored test): every pixel's
/// camera ray meets the sphere, plate changes are painted red and the
/// nearest boundary class tints the image. Run with
/// `ASTRUM_TIER_A_OVERLAY=<viewport.png> ASTRUM_TIER_A_OUT=<png>
/// ASTRUM_TIER_A_POSE=px,py,pz,qx,qy,qz,qw` (body metres, camera −Z forward,
/// 60° vertical field of view).
#[test]
#[ignore]
fn overlay_boundaries() {
    use super::tectonics::{plates, tectonic_sample};
    let Ok(capture) = std::env::var("ASTRUM_TIER_A_OVERLAY") else {
        return;
    };
    let out = std::env::var("ASTRUM_TIER_A_OUT").expect("ASTRUM_TIER_A_OUT");
    let pose: Vec<f64> = std::env::var("ASTRUM_TIER_A_POSE")
        .expect("ASTRUM_TIER_A_POSE")
        .split(',')
        .map(|v| v.trim().parse().expect("number"))
        .collect();
    let position = DVec3::new(pose[0], pose[1], pose[2]);
    let rotation = glam::DQuat::from_xyzw(pose[3], pose[4], pose[5], pose[6]);
    let decoder = png::Decoder::new(std::io::BufReader::new(File::open(&capture).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = info.line_size / w;
    let input = inputs(16, 7);
    let (p, r) = (&input.params, input.radius_m);
    let set = plates(p, p.tectonic_seed);
    let tan = (30.0f64).to_radians().tan();
    let aspect = w as f64 / h as f64;
    let mut plate = vec![u16::MAX; w * h];
    let mut rgb = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let k = y * w + x;
            for c in 0..3 {
                rgb[k * 3 + c] = buffer[y * info.line_size + x * channels + c];
            }
            let ray = rotation
                * DVec3::new(
                    (2.0 * (x as f64 + 0.5) / w as f64 - 1.0) * tan * aspect,
                    (1.0 - 2.0 * (y as f64 + 0.5) / h as f64) * tan,
                    -1.0,
                )
                .normalize();
            // Nearest ray/sphere intersection.
            let b = position.dot(ray);
            let c = position.length_squared() - r * r;
            let disc = b * b - c;
            if disc < 0.0 {
                continue;
            }
            let t = -b - disc.sqrt();
            if t <= 0.0 {
                continue;
            }
            let d = (position + ray * t).normalize();
            plate[k] = u16::from(tectonic_sample(d, &set, p, r).plate);
        }
    }
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let k = y * w + x;
            if plate[k] != u16::MAX
                && (plate[k] != plate[k + 1] && plate[k + 1] != u16::MAX
                    || plate[k] != plate[k + w] && plate[k + w] != u16::MAX)
            {
                rgb[k * 3..k * 3 + 3].copy_from_slice(&[255, 0, 0]);
            }
        }
    }
    // Optional: the M3 river graph of a CPU bake at ASTRUM_TIER_A_RIVERS
    // face cells, segments drawn in cyan where they face the camera.
    if let Some(n) = std::env::var("ASTRUM_TIER_A_RIVERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        use crate::terrain::hydrology::{self, HydrologyInput, HydrologyParams};
        let input = inputs(n, 7);
        let fields = bake_full(&input).unwrap().fields;
        let hydro = hydrology::run(&HydrologyInput {
            elevation: &fields.elevation,
            moisture: &fields.moisture,
            radius_m: r,
            params: HydrologyParams::prototype(),
        });
        let inverse = rotation.inverse();
        let mut project = |d: DVec3| {
            let world = d * r;
            if d.dot(position - world) <= 0.0 {
                return;
            }
            let c = inverse * (world - position);
            if c.z >= 0.0 {
                return;
            }
            let sx = c.x / -c.z / (tan * aspect);
            let sy = c.y / -c.z / tan;
            let x = ((sx + 1.0) * 0.5 * w as f64) as i64;
            let y = ((1.0 - sy) * 0.5 * h as f64) as i64;
            if (0..w as i64).contains(&x) && (0..h as i64).contains(&y) {
                let k = y as usize * w + x as usize;
                rgb[k * 3..k * 3 + 3].copy_from_slice(&[0, 255, 255]);
            }
        };
        for (a, b) in hydro.rivers.segments() {
            let (pa, pb) = (
                hydro.rivers.vertices[a as usize].pos,
                hydro.rivers.vertices[b as usize].pos,
            );
            for s in 0..=200 {
                project(pa.lerp(pb, f64::from(s) / 200.0).normalize());
            }
        }
    }
    write_png(Path::new(&out), w as u32, h as u32, &rgb);
}
