//! PROTOTYPE (M3 Water) scorer and preview: bakes a Tier A CPU world map,
//! runs the macro hydrology and prints drainage, lake and river statistics
//! against the §19.4 checks, then writes PNGs: a world map with rivers and
//! lakes, and a close-up of the largest basin with the Tier B carve applied
//! to the bicubic macro elevation.
//!
//! `cargo run --release -p astrum_world --example hydrology_preview -- <archetype.ron> <seed> <face_cells> <out_dir> [radius_m]`
use astrum_world::terrain::{
    archetype::PlanetArchetype,
    hydrology::{self, HydrologyInput, HydrologyParams, NO_RECEIVER, flood::NO_LAKE},
    tier_a::{TierAInputs, bake_full, erosion},
    world_map::CubeMap,
};
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

fn land_colour(h: f64) -> [f64; 3] {
    if h < 0.0 {
        ramp(
            h,
            &[(-4000.0, [0.02, 0.05, 0.18]), (0.0, [0.18, 0.4, 0.65])],
        )
    } else {
        ramp(
            h,
            &[
                (0.0, [0.3, 0.5, 0.28]),
                (1000.0, [0.55, 0.52, 0.35]),
                (2500.0, [0.5, 0.42, 0.36]),
                (4500.0, [0.93, 0.93, 0.93]),
            ],
        )
    }
}

fn least_squares(points: &[(f64, f64)]) -> (f64, f64) {
    let count = points.len() as f64;
    if points.len() < 3 {
        return (0.0, 0.0);
    }
    let (mx, my) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / count, b + y / count));
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in points {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    (sxy / sxx.max(1e-300), sxy * sxy / (sxx * syy).max(1e-300))
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let archetype: PlanetArchetype =
        ron::from_str(&std::fs::read_to_string(&args[1]).expect("read archetype"))
            .expect("parse archetype");
    archetype.validate().expect("valid archetype");
    let seed: u64 = args[2].parse().expect("seed");
    let n: usize = args[3].parse().expect("face cells");
    let out = Path::new(&args[4]);
    let radius_m: f64 = args
        .get(5)
        .map_or(338_950.0, |r| r.parse().expect("radius"));
    std::fs::create_dir_all(out).expect("output dir");
    let inputs = TierAInputs {
        params: archetype.sample(seed),
        stages: archetype.stages.clone(),
        radius_m,
        pole: DVec3::Y,
        face_cells: n,
    };
    let started = std::time::Instant::now();
    let output = bake_full(&inputs).expect("bake");
    println!("bake {n}^2: {:.2} s", started.elapsed().as_secs_f64());
    let fields = &output.fields;
    let params = HydrologyParams::prototype();
    let started = std::time::Instant::now();
    let hydro = hydrology::run(&HydrologyInput {
        elevation: &fields.elevation,
        moisture: &fields.moisture,
        radius_m,
        params,
    });
    println!("hydrology: {:.3} s", started.elapsed().as_secs_f64());

    // §19.4 drainage checks.
    let fill = &hydro.fill;
    let before = erosion::drainage(&fields.elevation, radius_m);
    let mut filled = CubeMap::new(n, 0.0f32);
    for (o, f) in filled.data_mut().iter_mut().zip(&fill.filled) {
        *o = *f as f32;
    }
    let land: Vec<usize> = (0..6 * n * n).filter(|&k| !fill.ocean[k]).collect();
    let land_area: f64 = land.iter().map(|&k| erosion_area(k, n, radius_m)).sum();
    let mut endorheic_area = 0.0;
    let mut ocean_area = 0.0;
    // Fate of each land texel: ocean or endorheic lake.
    let mut fate: Vec<u8> = fill.ocean.iter().map(|o| u8::from(*o)).collect();
    for lake in fill.lakes.iter().filter(|l| l.endorheic) {
        fate[lake.outlet as usize] = 2;
    }
    // Receivers before donors.
    for &k in fill.order.iter().rev() {
        let k = k as usize;
        if fate[k] == 0 {
            fate[k] = fate[fill.receiver[k] as usize];
        }
    }
    for &k in &land {
        let a = erosion_area(k, n, radius_m);
        match fate[k] {
            1 => ocean_area += a,
            2 => endorheic_area += a,
            _ => {}
        }
    }
    println!(
        "drainage (steepest descent on the baked map): to ocean {:.3}, land pits {} of {}",
        before.draining_fraction, before.land_pits, before.land_texels
    );
    println!(
        "after fill: to ocean {:.3}, to endorheic lakes {:.3}, land pits 0 (target: to ocean >= 0.85, pits drop >= 5x)",
        ocean_area / land_area,
        endorheic_area / land_area
    );
    let lake_area: f64 = fill.lakes.iter().map(|l| l.area_km2).sum();
    let endorheic = fill.lakes.iter().filter(|l| l.endorheic).count();
    let deepest = fill
        .lakes
        .iter()
        .map(|l| l.level_m - l.bottom_m)
        .fold(0.0, f64::max);
    println!(
        "lakes {} ({} endorheic), {:.2} % of land, deepest {:.0} m",
        fill.lakes.len(),
        endorheic,
        100.0 * lake_area / land_area.max(1e-9),
        deepest
    );
    let graph = &hydro.rivers;
    let max_q = graph.vertices.iter().map(|v| v.q_m3s).fold(0.0, f64::max);
    let max_w = graph.vertices.iter().map(|v| v.width_m).fold(0.0, f64::max);
    println!(
        "rivers: {} vertices, {} segments, {} mouths, {:.0} km of channel, largest {:.0} m3/s ({:.0} m wide); hash {}^2 x 6 cells, {} entries; carve depth bound {:.1} m",
        graph.vertices.len(),
        graph.segment_count(),
        graph.mouth_count(),
        graph.length_km(radius_m),
        max_q,
        max_w,
        hydro.hash.cells,
        hydro.hash.segments.len(),
        hydro.carve_depth_bound_m()
    );
    // Slope–area on channel texels of the filled surface (expected −0.9..−0.2).
    let fit = erosion::slope_area_fit(&filled, &hydro.discharge, radius_m, 50.0, |_| Some(1.0));
    println!(
        "slope-area exponent {:.2} (R² {:.2}, {} texels; target -0.9..-0.2)",
        fit.exponent, fit.r2, fit.texels
    );
    // Hack's law: longest upstream length against drainage area per vertex.
    let mut longest = vec![0.0f64; graph.vertices.len()];
    for i in graph.topological_order() {
        let (i, v) = (i as usize, &graph.vertices[i as usize]);
        if v.downstream != NO_RECEIVER {
            let d = v.downstream as usize;
            let length = (v.pos - graph.vertices[d].pos).length() * radius_m * 1e-3;
            longest[d] = longest[d].max(longest[i] + length);
        }
    }
    let points: Vec<(f64, f64)> = graph
        .vertices
        .iter()
        .zip(&longest)
        .filter(|(v, l)| **l > 0.0 && v.discharge_km2 > 0.0)
        .map(|(v, l)| (v.discharge_km2.ln(), l.ln()))
        .collect();
    let hack = least_squares(&points);
    println!(
        "Hack exponent {:.2} (R² {:.2}; real rivers 0.5-0.6)",
        hack.0, hack.1
    );

    // World map with rivers and lakes.
    let lake_level = fill.lake_level_map();
    let (w, h) = (2048u32, 1024u32);
    let mut image = vec![0u8; (w * h * 3) as usize];
    let light = DVec3::new(-1.0, 1.0, 0.6).normalize();
    let shade_at = |dir: DVec3, height: &dyn Fn(DVec3) -> f64, step: f64, exaggerate: f64| {
        let east = DVec3::Y.cross(dir).normalize_or(DVec3::X);
        let north = dir.cross(east);
        let ge = (height((dir + east * step).normalize())
            - height((dir - east * step).normalize()))
            / (2.0 * step * radius_m);
        let gn = (height((dir + north * step).normalize())
            - height((dir - north * step).normalize()))
            / (2.0 * step * radius_m);
        let normal = DVec3::new(-ge * exaggerate, -gn * exaggerate, 1.0).normalize();
        (0.35 + 0.65 * normal.dot(light).max(0.0)).min(1.2)
    };
    let macro_h = |d: DVec3| fields.elevation.bicubic(d);
    for y in 0..h {
        let lat = std::f64::consts::FRAC_PI_2
            - std::f64::consts::PI * (f64::from(y) + 0.5) / f64::from(h);
        for x in 0..w {
            let lon = std::f64::consts::TAU * (f64::from(x) + 0.5) / f64::from(w);
            let dir = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let elevation = macro_h(dir);
            let mut colour = land_colour(elevation);
            if elevation >= 0.0 {
                let s = shade_at(dir, &macro_h, 0.5 / n as f64, 4.0);
                colour = colour.map(|c| c * s);
            }
            let level = f64::from(lake_level.nearest(dir));
            if level > f64::from(f32::MIN) && level > elevation {
                let lake = fill.lake_id[nearest_index(dir, n)];
                colour = if lake != NO_LAKE && fill.lakes[lake as usize].endorheic {
                    [0.85, 0.82, 0.7]
                } else {
                    [0.25, 0.55, 0.85]
                };
            }
            let k = ((y * w + x) * 3) as usize;
            for c in 0..3 {
                image[k + c] = (colour[c].clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    // Rivers drawn as lines, brightness by discharge.
    let to_pixel = |d: DVec3| {
        let lat = d.y.clamp(-1.0, 1.0).asin();
        let lon = d.z.atan2(d.x).rem_euclid(std::f64::consts::TAU);
        (
            lon / std::f64::consts::TAU * f64::from(w),
            (std::f64::consts::FRAC_PI_2 - lat) / std::f64::consts::PI * f64::from(h),
        )
    };
    let q_scale = max_q.max(1.0).ln();
    for (a, b) in graph.segments() {
        let (va, vb) = (&graph.vertices[a as usize], &graph.vertices[b as usize]);
        let (p0, p1) = (to_pixel(va.pos), to_pixel(vb.pos));
        if (p0.0 - p1.0).abs() > f64::from(w) / 2.0 {
            continue;
        }
        let t = (va.q_m3s.max(1.0).ln() / q_scale).clamp(0.0, 1.0);
        let colour = [0.05, 0.25 + 0.4 * t, 0.6 + 0.4 * t];
        let steps = ((p1.0 - p0.0).abs().max((p1.1 - p0.1).abs()) * 2.0)
            .ceil()
            .max(1.0) as usize;
        for s in 0..=steps {
            let f = s as f64 / steps as f64;
            let (px, py) = (p0.0 + (p1.0 - p0.0) * f, p0.1 + (p1.1 - p0.1) * f);
            let (px, py) = (px as i64, py as i64);
            if px < 0 || py < 0 || px >= i64::from(w) || py >= i64::from(h) {
                continue;
            }
            let k = ((py as u32 * w + px as u32) * 3) as usize;
            for c in 0..3 {
                image[k + c] = (colour[c] * 255.0) as u8;
            }
        }
    }
    write_png(&out.join(format!("rivers_{n}_{seed}.png")), w, h, &image);

    // Close-up of the largest river's lower course: tangent-plane view,
    // macro (bicubic) vs carved, with channels and lakes as water.
    let mouth = graph
        .vertices
        .iter()
        .filter(|v| v.mouth == hydrology::rivers::Mouth::Ocean)
        .max_by(|a, b| a.discharge_km2.total_cmp(&b.discharge_km2))
        .expect("a river mouth");
    // Centre a little upstream of the mouth: walk up the largest tributary.
    let mut centre = mouth.pos;
    for _ in 0..3 {
        let up = graph
            .vertices
            .iter()
            .filter(|v| {
                v.downstream != NO_RECEIVER && graph.vertices[v.downstream as usize].pos == centre
            })
            .max_by(|a, b| a.discharge_km2.total_cmp(&b.discharge_km2));
        match up {
            Some(v) => centre = v.pos,
            None => break,
        }
    }
    let gorge = graph
        .vertices
        .iter()
        .max_by(|a, b| a.incision_m.total_cmp(&b.incision_m))
        .expect("a river");
    println!(
        "deepest incision {:.0} m at {:?}",
        gorge.incision_m, gorge.pos
    );
    for (name, centre, extent_m) in [
        ("basin", centre, 120_000.0),
        ("valley", centre, 30_000.0),
        ("gorge", gorge.pos, 30_000.0),
    ] {
        let size = 1024u32;
        let east = DVec3::Y.cross(centre).normalize_or(DVec3::X);
        let north = centre.cross(east);
        let mut macro_image = vec![0u8; (size * size * 3) as usize];
        let mut carved_image = macro_image.clone();
        let pixel_m = extent_m / f64::from(size);
        let carved_h = |d: DVec3| hydro.carve(d, macro_h(d)).height_m;
        for y in 0..size {
            for x in 0..size {
                let (ex, ny) = (
                    (f64::from(x) + 0.5 - f64::from(size) / 2.0) * pixel_m,
                    (f64::from(size) / 2.0 - f64::from(y) - 0.5) * pixel_m,
                );
                let d = (centre + (east * ex + north * ny) / radius_m).normalize();
                let k = ((y * size + x) * 3) as usize;
                for (image, carving) in [(&mut macro_image, false), (&mut carved_image, true)] {
                    let sample = hydro.carve(d, macro_h(d));
                    let height = if carving { sample.height_m } else { macro_h(d) };
                    let step = pixel_m / radius_m;
                    let s = if carving {
                        shade_at(d, &carved_h, step, 2.0)
                    } else {
                        shade_at(d, &macro_h, step, 2.0)
                    };
                    let mut colour =
                        land_colour(height).map(|c| if height >= 0.0 { c * s } else { c });
                    let level = f64::from(lake_level.nearest(d));
                    if level > f64::from(f32::MIN) && level > height {
                        colour = [0.25, 0.55, 0.85];
                    }
                    if carving && sample.water_m.is_some_and(|w| w > height) {
                        colour = [0.15, 0.45, 0.9];
                    }
                    for c in 0..3 {
                        image[k + c] = (colour[c].clamp(0.0, 1.0) * 255.0) as u8;
                    }
                }
            }
        }
        write_png(
            &out.join(format!("{name}_macro_{n}_{seed}.png")),
            size,
            size,
            &macro_image,
        );
        write_png(
            &out.join(format!("{name}_carved_{n}_{seed}.png")),
            size,
            size,
            &carved_image,
        );
    }
    println!(
        "close-ups centred on {:?} (largest mouth {:.0} m3/s)",
        centre, mouth.q_m3s
    );
}

fn nearest_index(d: DVec3, n: usize) -> usize {
    let (face, u, v) = astrum_world::terrain::world_map::locate(d);
    let to = |c: f64| (((c + 1.0) * 0.5 * n as f64) as usize).min(n - 1);
    (face * n + to(v)) * n + to(u)
}

fn erosion_area(k: usize, n: usize, radius_m: f64) -> f64 {
    let (u, v) = astrum_world::terrain::tier_a::texel_uv(k % n, (k / n) % n, n);
    erosion::texel_area_km2(u, v, n, radius_m)
}
