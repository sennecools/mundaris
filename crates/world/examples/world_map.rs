//! W1 Moon world-map generation: bake a world map from a recipe and write the
//! fields plus viewable equirectangular images.
//!
//! cargo run --release -p astrum_world --example world_map -- <recipe.json> <out_dir>
//!
//! Writes `elevation.f32` (6·n² little-endian f32, faces in `CubeFace::ALL`
//! order, rows along +v), `biomes.rgba8` (weights × 255 in `MOON_BIOMES` order),
//! `world_map.json` (recipe, layout, hashes, stats) and PNGs: `elevation.png`,
//! `hillshade.png`, `biomes.png` (biome colours × hillshade) and `legend.txt`.

use glam::DVec3;
use astrum_world::terrain::world_map::{CubeMap, MOON_BIOMES, MoonWorldMapRecipe, bake_moon};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

const BIOME_COLOURS: [[f64; 3]; 4] = [
    [0.80, 0.79, 0.76], // highland: bright anorthosite
    [0.30, 0.32, 0.38], // mare: dark basalt
    [0.62, 0.52, 0.40], // crater floor
    [0.86, 0.62, 0.36], // crater rim and ejecta
];

fn save_png(path: &Path, w: usize, h: usize, rgb: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

fn direction(lon: f64, lat: f64) -> DVec3 {
    DVec3::new(lat.cos() * lon.cos(), lat.sin(), -lat.cos() * lon.sin())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (recipe_path, out) = match args.as_slice() {
        [_, r, o] => (Path::new(r), Path::new(o)),
        _ => return Err("usage: world_map <recipe.json> <out_dir>".into()),
    };
    let recipe_bytes = fs::read(recipe_path)?;
    let recipe: MoonWorldMapRecipe = serde_json::from_slice(&recipe_bytes)?;
    let t0 = std::time::Instant::now();
    let map = bake_moon(&recipe)?;
    let seconds = t0.elapsed().as_secs_f64();
    fs::create_dir_all(out)?;

    let elevation: Vec<u8> = map
        .elevation
        .data()
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let biomes: Vec<u8> = map
        .biomes
        .data()
        .iter()
        .flat_map(|w| w.map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8))
        .collect();
    fs::write(out.join("elevation.f32"), &elevation)?;
    fs::write(out.join("biomes.rgba8"), &biomes)?;
    let hex = |b: &[u8]| format!("{:x}", Sha256::digest(b));
    let meta = serde_json::json!({
        "format": "astrum.world-map.v1",
        "status": "W1 preview output; runtime format not yet defined (W3)",
        "generator": "astrum_world::terrain::world_map::bake_moon",
        "recipe_sha256": hex(&recipe_bytes),
        "recipe": recipe,
        "radius_m": recipe.radius_m,
        "face_resolution": recipe.face_resolution,
        "faces": "CubeFace::ALL order (+X, -X, +Y, -Y, +Z, -Z); direction = normalize(N + u U + v V); rows along +v, columns along +u; texel centres",
        "channels": [
            {"file": "elevation.f32", "encoding": "f32 little-endian", "units": "m above radius_m", "sha256": hex(&elevation)},
            {"file": "biomes.rgba8", "encoding": "u8 x4 per texel, weight * 255", "channels": MOON_BIOMES, "sha256": hex(&biomes)},
        ],
        "stats": map.stats,
        "bake_seconds": seconds,
    });
    fs::write(
        out.join("world_map.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;

    // Equirectangular previews: longitude across, latitude down.
    let (w, h) = (2048usize, 1024usize);
    let r = recipe.radius_m;
    let mut elev = vec![0.0f64; w * h];
    let mut weights = vec![[0.0f64; 4]; w * h];
    let biome_maps: Vec<CubeMap<f32>> = (0..4)
        .map(|k| {
            let mut m = CubeMap::new(map.biomes.n(), 0.0f32);
            for (d, s) in m.data_mut().iter_mut().zip(map.biomes.data()) {
                *d = s[k];
            }
            m
        })
        .collect();
    for y in 0..h {
        let lat = std::f64::consts::FRAC_PI_2 - (y as f64 + 0.5) / h as f64 * std::f64::consts::PI;
        for x in 0..w {
            let lon = (x as f64 + 0.5) / w as f64 * std::f64::consts::TAU - std::f64::consts::PI;
            let d = direction(lon, lat);
            elev[y * w + x] = map.elevation.bilinear(d);
            for (k, m) in biome_maps.iter().enumerate() {
                weights[y * w + x][k] = m.bilinear(d);
            }
        }
    }
    let (lo, hi) = (map.stats.elevation_min_m, map.stats.elevation_max_m);
    let mut shade = vec![0.0f64; w * h];
    let dlon = std::f64::consts::TAU / w as f64;
    let dlat = std::f64::consts::PI / h as f64;
    for y in 0..h {
        let lat = std::f64::consts::FRAC_PI_2 - (y as f64 + 0.5) * dlat;
        let ew = r * lat.cos().max(0.05) * dlon;
        for x in 0..w {
            let at = |xx: usize, yy: usize| elev[yy.min(h - 1) * w + xx % w];
            let gx = (at(x + 1, y) - at(x + w - 1, y)) / (2.0 * ew);
            let gy = (at(x, y.saturating_sub(1)) - at(x, y + 1)) / (2.0 * r * dlat);
            // Light from the north-west, 3× vertical exaggeration for readability.
            let (gx, gy) = (3.0 * gx, 3.0 * gy);
            let s = (gx * 0.6 - gy * 0.6 + 0.7) / (gx * gx + gy * gy + 1.0).sqrt();
            shade[y * w + x] = s.clamp(0.0, 1.0);
        }
    }
    let to8 = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let elevation_png: Vec<u8> = elev
        .iter()
        .flat_map(|e| {
            let t = (e - lo) / (hi - lo).max(1.0);
            [
                to8(0.15 + 0.85 * t),
                to8(0.12 + 0.8 * t),
                to8(0.25 + 0.6 * t),
            ]
        })
        .collect();
    let shade_png: Vec<u8> = shade.iter().flat_map(|s| [to8(*s); 3]).collect();
    let biome_png: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let mut c = [0.0; 3];
            for (k, col) in BIOME_COLOURS.iter().enumerate() {
                for (ch, v) in c.iter_mut().zip(col) {
                    *ch += weights[i][k] * v;
                }
            }
            let l = 0.35 + 0.9 * shade[i];
            c.map(|v| to8(v * l))
        })
        .collect();
    save_png(&out.join("elevation.png"), w, h, &elevation_png)?;
    save_png(&out.join("hillshade.png"), w, h, &shade_png)?;
    save_png(&out.join("biomes.png"), w, h, &biome_png)?;
    // Globe views: orthographic discs looking at longitudes 0, 90, 180, 270°,
    // biome colours lit by a sun from the upper left (3× relief exaggeration).
    let disc = 720usize;
    let mut globe = vec![16u8; 2 * disc * 2 * disc * 3];
    let biome_at = |d: DVec3| {
        let mut c = [0.0; 3];
        for (k, m) in biome_maps.iter().enumerate() {
            let wk = m.bilinear(d);
            for (ch, v) in c.iter_mut().zip(&BIOME_COLOURS[k]) {
                *ch += wk * v;
            }
        }
        c
    };
    for (view, &lon0) in [0.0f64, 90.0, 180.0, 270.0].iter().enumerate() {
        let lon0 = lon0.to_radians();
        // Camera frame: forward = towards the viewer's sub-point, right, up.
        let forward = direction(lon0, 0.0);
        let up = DVec3::Y;
        let right = up.cross(forward).normalize();
        let (ox, oy) = ((view % 2) * disc, (view / 2) * disc);
        for py in 0..disc {
            for px in 0..disc {
                let x = (px as f64 + 0.5) / disc as f64 * 2.0 - 1.0;
                let y = 1.0 - (py as f64 + 0.5) / disc as f64 * 2.0;
                let rr = x * x + y * y;
                if rr >= 1.0 {
                    continue;
                }
                let d = (forward * (1.0 - rr).sqrt() + right * x + up * y).normalize();
                // Surface normal from the elevation gradient along screen axes.
                let eps = 2.0 / (disc as f64);
                let ta = (right - d * d.dot(right)).normalize();
                let tb = (up - d * d.dot(up)).normalize();
                let hgt = |v: DVec3| map.elevation.bilinear(v.normalize());
                let ga = (hgt(d + ta * eps) - hgt(d - ta * eps)) / (2.0 * eps * r);
                let gb = (hgt(d + tb * eps) - hgt(d - tb * eps)) / (2.0 * eps * r);
                let normal = (d - 3.0 * (ta * ga + tb * gb)).normalize();
                let sun = (forward * 0.55 - right * 0.55 + up * 0.62).normalize();
                let light = 0.12 + 0.95 * normal.dot(sun).max(0.0);
                let c = biome_at(d);
                let o = ((oy + py) * 2 * disc + ox + px) * 3;
                for k in 0..3 {
                    globe[o + k] = to8(c[k] * light);
                }
            }
        }
    }
    save_png(&out.join("globe.png"), 2 * disc, 2 * disc, &globe)?;
    fs::write(
        out.join("legend.txt"),
        format!(
            "Equirectangular, longitude -180..180 across, latitude +90..-90 down.\n\
             elevation.png: dark = {lo:.0} m, bright = {hi:.0} m.\n\
             hillshade.png: light from NW, 3x vertical exaggeration.\n\
             biomes.png: highland light grey, mare dark blue-grey, crater floor tan, rim/ejecta orange; x hillshade.\n"
        ),
    )?;
    println!("{}", serde_json::to_string_pretty(&meta["stats"])?);
    println!("baked in {seconds:.1} s");
    Ok(())
}
