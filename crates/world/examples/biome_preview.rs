//! W2 biome catalog and heightmap variants: compose the world map with
//! library detail on the CPU and write hillshade crops at chosen spots.
//!
//! cargo run --release -p astrum_world --example biome_preview -- \
//!     <catalog.json> <world_map_recipe.json> <out_dir> --library <dir> [--library <dir> ...]
//!
//! Spots are chosen automatically: the most interior point of each biome and
//! a mare/highland border. Per spot it writes `<spot>_8km.png` (composed
//! hillshade, 2048 px), `<spot>_8km_macro.png` (world map only) and
//! `<spot>_8km_biomes.png` (biome colours × hillshade), plus one 40 km
//! overview, and `preview.json` with spot positions, weights and detail RMS.

use glam::DVec3;
use astrum_world::terrain::world_map::{
    BiomeCatalog, ComposedSurface, DetailTile, MOON_BIOMES, MoonWorldMapRecipe, bake_moon,
    texel_directions,
};
use std::fs;
use std::path::{Path, PathBuf};

const BIOME_COLOURS: [[f64; 3]; 4] = [
    [0.80, 0.79, 0.76],
    [0.30, 0.32, 0.38],
    [0.62, 0.52, 0.40],
    [0.86, 0.62, 0.36],
];

fn save_png(path: &Path, w: usize, h: usize, rgb: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgb)?;
    Ok(())
}

/// Evaluate `f` on a `px × px` grid on the tangent plane at `centre`.
fn grid(
    centre: DVec3,
    size_m: f64,
    px: usize,
    radius: f64,
    f: &(dyn Fn(DVec3) -> f64 + Sync),
) -> Vec<f64> {
    let e1 = centre.any_orthonormal_vector();
    let e2 = centre.cross(e1);
    let mut out = vec![0.0; px * px];
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get());
    let rows_per = px.div_ceil(threads);
    std::thread::scope(|scope| {
        for (c, chunk) in out.chunks_mut(rows_per * px).enumerate() {
            scope.spawn(move || {
                for (k, v) in chunk.iter_mut().enumerate() {
                    let (row, col) = (c * rows_per + k / px, k % px);
                    let x = ((col as f64 + 0.5) / px as f64 - 0.5) * size_m;
                    let y = (0.5 - (row as f64 + 0.5) / px as f64) * size_m;
                    *v = f((centre + (e1 * x + e2 * y) / radius).normalize());
                }
            });
        }
    });
    out
}

fn hillshade(h: &[f64], px: usize, spacing: f64) -> Vec<f64> {
    let at = |x: usize, y: usize| h[y.min(px - 1) * px + x.min(px - 1)];
    (0..px * px)
        .map(|k| {
            let (x, y) = (k % px, k / px);
            let gx = (at(x + 1, y) - at(x.saturating_sub(1), y)) / (2.0 * spacing);
            let gy = (at(x, y.saturating_sub(1)) - at(x, y + 1)) / (2.0 * spacing);
            ((gx * 0.6 - gy * 0.6 + 0.7) / (gx * gx + gy * gy + 1.0).sqrt()).clamp(0.0, 1.0)
        })
        .collect()
}

fn grey(s: &[f64]) -> Vec<u8> {
    s.iter()
        .flat_map(|v| [(v * 255.0).round() as u8; 3])
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let usage =
        "usage: biome_preview <catalog.json> <world_map_recipe.json> <out_dir> --library <dir>...";
    if args.len() < 6 {
        return Err(usage.into());
    }
    let (catalog_path, recipe_path, out) = (&args[1], &args[2], PathBuf::from(&args[3]));
    let libraries: Vec<PathBuf> = args[4..]
        .chunks(2)
        .map(|p| match p {
            [flag, dir] if flag == "--library" => Ok(PathBuf::from(dir)),
            _ => Err(usage),
        })
        .collect::<Result<_, _>>()?;
    let catalog_bytes = fs::read(catalog_path)?;
    let catalog: BiomeCatalog = serde_json::from_slice(&catalog_bytes)?;
    catalog.validate(&MOON_BIOMES)?;
    let recipe: MoonWorldMapRecipe = serde_json::from_slice(&fs::read(recipe_path)?)?;
    let t0 = std::time::Instant::now();
    let map = bake_moon(&recipe)?;
    let bake_s = t0.elapsed().as_secs_f64();

    // Load each distinct variant once.
    let t1 = std::time::Instant::now();
    let mut tiles: Vec<DetailTile> = Vec::new();
    for v in catalog
        .biomes
        .iter()
        .flat_map(|b| &b.sub_biomes)
        .flat_map(|s| &s.variants)
    {
        if tiles
            .iter()
            .any(|t| t.bundle_id == v.bundle && t.height_sha256 == v.height_sha256)
        {
            continue;
        }
        let dir = libraries
            .iter()
            .map(|l| l.join(&v.bundle))
            .find(|d| d.join("bundle.json").exists())
            .ok_or_else(|| format!("bundle {} not found in {libraries:?}", v.bundle))?;
        tiles.push(DetailTile::load(
            &dir,
            Some(&v.height_sha256),
            catalog.band_limit_m,
        )?);
    }
    let load_s = t1.elapsed().as_secs_f64();
    let tile_info: Vec<_> = tiles
        .iter()
        .map(|t| serde_json::json!({"bundle": t.bundle_id, "provenance": t.provenance, "height_sha256": t.height_sha256, "detail_rms_m": t.rms_m}))
        .collect();
    let radius = recipe.radius_m;
    let surface = ComposedSurface::new(
        radius,
        map.elevation.clone(),
        &map.biomes,
        catalog.clone(),
        tiles,
    )?;
    fs::create_dir_all(&out)?;

    // Spots: for each biome, the texel whose surroundings (±4 km) are most
    // purely that biome; plus a mare/highland border.
    let dirs = texel_directions(map.biomes.n());
    let purity = |d: DVec3, b: usize| {
        let e1 = d.any_orthonormal_vector();
        let e2 = d.cross(e1);
        [(0.0, 0.0), (4e3, 0.0), (-4e3, 0.0), (0.0, 4e3), (0.0, -4e3)]
            .iter()
            .map(|(x, y)| surface.biome_weights((d + (e1 * *x + e2 * *y) / radius).normalize())[b])
            .fold(f64::INFINITY, f64::min)
    };
    let mut spots: Vec<(String, DVec3)> = Vec::new();
    for (b, biome) in catalog.biomes.iter().enumerate() {
        let best = dirs
            .iter()
            .step_by(7)
            .filter(|d| surface.biome_weights(**d)[b] > 0.9)
            .map(|d| (purity(*d, b), *d))
            .max_by(|a, c| a.0.total_cmp(&c.0));
        if let Some((_, d)) = best {
            spots.push((biome.id.clone(), d));
        }
    }
    let border = dirs.iter().step_by(7).copied().find(|d| {
        let w = surface.biome_weights(*d);
        (0.4..0.6).contains(&w[1]) && w[0] > 0.3
    });
    if let Some(d) = border {
        spots.push(("mare_highland_border".into(), d));
    }

    let mut report = Vec::new();
    for (name, centre) in &spots {
        let (size, px) = (8000.0, 2048usize);
        let t = std::time::Instant::now();
        let composed = grid(*centre, size, px, radius, &|d| surface.height(d));
        let macro_h = grid(*centre, size, px, radius, &|d| surface.macro_height(d));
        let secs = t.elapsed().as_secs_f64();
        let spacing = size / px as f64;
        let shade = hillshade(&composed, px, spacing);
        save_png(&out.join(format!("{name}_8km.png")), px, px, &grey(&shade))?;
        save_png(
            &out.join(format!("{name}_8km_macro.png")),
            px,
            px,
            &grey(&hillshade(&macro_h, px, spacing)),
        )?;
        // Dominant biome on a coarse 256² grid, tinting the hillshade.
        let dominant = grid(*centre, size, 256, radius, &|d| {
            let w = surface.biome_weights(d);
            (0..w.len())
                .max_by(|a, b| w[*a].total_cmp(&w[*b]))
                .unwrap_or(0) as f64
        });
        let tinted: Vec<u8> = (0..px * px)
            .flat_map(|k| {
                let (x, y) = (k % px * 256 / px, k / px * 256 / px);
                let b = dominant[y * 256 + x] as usize;
                let l = 0.35 + 0.9 * shade[k];
                BIOME_COLOURS[b.min(3)].map(|c| ((c * l).clamp(0.0, 1.0) * 255.0).round() as u8)
            })
            .collect();
        save_png(&out.join(format!("{name}_8km_biomes.png")), px, px, &tinted)?;
        let detail: Vec<f64> = composed.iter().zip(&macro_h).map(|(c, m)| c - m).collect();
        let rms = (detail.iter().map(|v| v * v).sum::<f64>() / detail.len() as f64).sqrt();
        let lat = centre.y.asin().to_degrees();
        let lon = (-centre.z).atan2(centre.x).to_degrees();
        report.push(serde_json::json!({
            "spot": name, "lat_deg": lat, "lon_deg": lon,
            "biome_weights_at_centre": surface.biome_weights(*centre),
            "detail_rms_m": rms, "seconds": secs,
        }));
        println!("{name}: lat {lat:.2} lon {lon:.2}, detail rms {rms:.2} m, {secs:.1} s");
    }
    if let Some((name, centre)) = spots
        .iter()
        .find(|(n, _)| n == "mare_highland_border")
        .or(spots.first())
    {
        let (size, px) = (40_000.0, 2048usize);
        let composed = grid(*centre, size, px, radius, &|d| surface.height(d));
        save_png(
            &out.join(format!("{name}_40km.png")),
            px,
            px,
            &grey(&hillshade(&composed, px, size / px as f64)),
        )?;
    }
    let meta = serde_json::json!({
        "catalog": catalog.id,
        "catalog_identity_sha256": catalog.identity_sha256(),
        "world_map_recipe": recipe.id,
        "world_map_stats": map.stats,
        "tiles": tile_info,
        "spots": report,
        "bake_seconds": bake_s,
        "tile_load_seconds": load_s,
        "notes": "hillshade light from NW, no vertical exaggeration; crops on the tangent plane, game metres",
    });
    fs::write(
        out.join("preview.json"),
        serde_json::to_string_pretty(&meta)?,
    )?;
    Ok(())
}
