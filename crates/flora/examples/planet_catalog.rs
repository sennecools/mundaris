//! Planet catalog sheet (self-check for per-planet species generation, in
//! the style of the user's reference line-up): every generated species of a
//! planet, a true-scale line-up on top, then one row per species with three
//! variants, and checks (budgets, silhouette diversity, hue families,
//! saturation).
//!
//! cargo run -p astrum_flora --release --example planet_catalog -- [body] [out_dir] [seed] [strangeness] [realism] [count]
//! Defaults: rust, target/flora-catalog, the planet file's seed, strangeness
//! and realism, 10 species. Writes `catalog.png` and `catalog.md`.

use std::fmt::Write as _;
use std::path::PathBuf;

use astrum_flora::palette::{apply, load_planet, srgb_to_oklab, to_lch};
use astrum_flora::raster::{Camera, Canvas, draw, text};
use astrum_flora::{Kit, LOD_COUNT, Mesh, grow_meshes, load_species_dir, species_gen, variant_seed};
use glam::{Vec2, Vec3};

const CELL: usize = 210;
const ROW_H: usize = 230;
const LINEUP_H: usize = 330;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let body = args.get(1).map(String::as_str).unwrap_or("rust");
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("target/flora-catalog"));
    std::fs::create_dir_all(&out).unwrap();
    let mut planet = load_planet(std::path::Path::new("content/flora/planets"), body).unwrap();
    if let Some(s) = args.get(3).and_then(|s| s.parse().ok()) {
        planet.seed = s;
    }
    if let Some(s) = args.get(4).and_then(|s| s.parse().ok()) {
        planet.strangeness = s;
    }
    if let Some(r) = args.get(5).and_then(|s| s.parse().ok()) {
        planet.realism = Some(r);
    }
    let count: usize = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(10);
    let mut templates = load_species_dir(std::path::Path::new("content/flora/species")).unwrap();
    apply(&mut templates, &planet);
    let mut species = species_gen::generate(&planet, &templates, count);
    // Woody species keep the palette-derived colours of their templates.
    let woody: Vec<usize> = (0..species.len()).filter(|&i| species[i].plan.is_none()).collect();
    {
        let mut w: Vec<_> = woody.iter().map(|&i| species[i].clone()).collect();
        apply(&mut w, &planet);
        for (j, &i) in woody.iter().enumerate() {
            species[i].look = w[j].look.clone();
        }
    }
    let kit = Kit::builtin();
    let grown: Vec<Vec<[Mesh; LOD_COUNT]>> =
        species.iter().map(|sp| (0..3).map(|v| grow_meshes(sp, &kit, variant_seed(sp, v)).1).collect()).collect();

    let width = 3 * CELL + 260;
    let lineup_w = (species.len() * 120).max(width);
    let width = width.max(lineup_w);
    let height = LINEUP_H + species.len() * ROW_H + 20;
    let bg = Vec3::new(0.085, 0.085, 0.09);
    let mut c = Canvas::new(width, height, bg);
    let title = format!(
        "{} SEED {} STRANGENESS {:.2} REALISM {:.2} STAR {:.0}K",
        planet.body.to_uppercase(),
        planet.seed,
        planet.strangeness,
        planet.realism(),
        planet.star_temperature_k
    );
    text(&mut c, 8, 6, &title, 2, Vec3::splat(0.85));
    // True-scale line-up of variant 0.
    let tallest = grown.iter().map(|g| bounds_h(&g[0][0])).fold(0.5f32, f32::max);
    let px = (LINEUP_H as f32 - 60.0) / tallest;
    let mut x = 20.0f32;
    for g in &grown {
        let (lo, hi) = g[0][0].bounds();
        let w = (hi.x - lo.x).max(hi.y - lo.y) * px;
        let cx = x + w * 0.5;
        let cam = Camera {
            azimuth: 0.5,
            elevation: 0.2,
            target: Vec3::ZERO,
            anchor_px: Vec2::new(cx, LINEUP_H as f32 - 15.0),
            px_per_m: px,
            clip: [0, 20, width, LINEUP_H],
        };
        draw(&mut c, &g[0][0], &cam);
        x += w.max(12.0) + 10.0;
    }
    // Rows.
    let mut md = format!("# Planet catalog: {title}\n\n| # | species | plan | layer | height m | tris L0/L1/L2 | organ L C h |\n|---|---|---|---|---|---|---|\n");
    let mut fails = Vec::new();
    for (i, (sp, g)) in species.iter().zip(&grown).enumerate() {
        let y0 = LINEUP_H + i * ROW_H;
        let plan = sp.plan.as_ref().map_or("tree/shrub", |p| species_gen::plan_name(p.plan));
        text(&mut c, 8, y0 + 8, &sp.name, 2, Vec3::splat(0.9));
        text(&mut c, 8, y0 + 24, &format!("{} {:?}", plan, sp.niche.layer), 2, Vec3::splat(0.6));
        let tris: Vec<usize> = g[0].iter().map(|m| m.triangles()).collect();
        text(&mut c, 8, y0 + 40, &format!("{}/{}/{} TRIS", tris[0], tris[1], tris[2]), 2, Vec3::splat(0.6));
        for (j, col) in [sp.look.organ, sp.look.organ_tip, sp.look.accent, sp.look.bark].iter().enumerate() {
            c.fill_rect(8 + j * 30, y0 + 60, 8 + j * 30 + 26, y0 + 86, Vec3::from(*col));
        }
        let h = g.iter().map(|v| bounds_h(&v[0])).fold(0.1f32, f32::max);
        let wmax = g.iter().map(|v| {
            let (lo, hi) = v[0].bounds();
            (hi.x - lo.x).max(hi.y - lo.y)
        }).fold(0.1f32, f32::max);
        let px = ((ROW_H as f32 - 30.0) / h).min((CELL as f32 - 20.0) / wmax);
        for (v, lods) in g.iter().enumerate() {
            let cx = 260 + v * CELL;
            let cam = Camera {
                azimuth: 0.5 + v as f32 * 0.7,
                elevation: 0.25,
                target: Vec3::ZERO,
                anchor_px: Vec2::new(cx as f32 + CELL as f32 * 0.5, (y0 + ROW_H - 15) as f32),
                px_per_m: px,
                clip: [cx, y0, cx + CELL, y0 + ROW_H],
            };
            draw(&mut c, &lods[0], &cam);
        }
        let budget = if sp.plan.is_some() { astrum_flora::bodyplan::PLAN_BUDGET[0] } else { 13_000 };
        if tris[0] > budget || tris[1] > tris[0] || tris[2] > tris[1] || tris[2] == 0 {
            fails.push(format!("{}: tris {tris:?}", sp.name));
        }
        let lch = to_lch(sp.look.organ.map(|v| v as f64));
        let _ = writeln!(md, "| {i} | {} | {plan} | {:?} | {:.1} | {}/{}/{} | {:.2} {:.3} {:.0} |", sp.name, sp.niche.layer, bounds_h(&g[0][0]), tris[0], tris[1], tris[2], lch[0], lch[1], lch[2]);
    }
    // Silhouette diversity: 64² masks of variant 0, normalised to bounds.
    let masks: Vec<Vec<bool>> = grown.iter().map(|g| mask(&g[0][0])).collect();
    let mut max_iou: (f64, usize, usize) = (0.0, 0, 0);
    for a in 0..masks.len() {
        for b in a + 1..masks.len() {
            let inter = masks[a].iter().zip(&masks[b]).filter(|(x, y)| **x && **y).count();
            let uni = masks[a].iter().zip(&masks[b]).filter(|(x, y)| **x || **y).count().max(1);
            let iou = inter as f64 / uni as f64;
            if iou > max_iou.0 {
                max_iou = (iou, a, b);
            }
        }
    }
    // Hue families among the species (30° bins of saturated organ colours).
    let mut bins = std::collections::BTreeSet::new();
    let mut chroma = 0.0;
    for sp in &species {
        let lch = to_lch(sp.look.organ.map(|v| v as f64));
        chroma += lch[1];
        if lch[1] > 0.04 {
            bins.insert((lch[2] / 30.0) as i32);
        }
    }
    chroma /= species.len() as f64;
    let plans: std::collections::BTreeSet<_> = species.iter().map(|s| s.plan.as_ref().map(|p| p.plan.index() as i32).unwrap_or(-1)).collect();
    let _ = writeln!(
        md,
        "\nChecks: body plans {} (≥ 5), hue bins {} (≥ 3), mean organ chroma {:.3}, most similar silhouettes {} / {} IoU {:.2} (< 0.8).",
        plans.len(),
        bins.len(),
        chroma,
        species[max_iou.1].name,
        species[max_iou.2].name,
        max_iou.0
    );
    let _ = lab_unused();
    if plans.len() < 5 {
        fails.push(format!("only {} body plans", plans.len()));
    }
    if bins.len() < 3 {
        fails.push(format!("only {} hue families", bins.len()));
    }
    if max_iou.0 >= 0.8 {
        fails.push(format!("silhouettes too similar ({:.2})", max_iou.0));
    }
    let _ = writeln!(md, "{}", if fails.is_empty() { "All pass.".to_string() } else { format!("FAIL: {}", fails.join("; ")) });
    std::fs::write(out.join("catalog.md"), &md).unwrap();
    let file = std::fs::File::create(out.join("catalog.png")).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), c.width as u32, c.height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&c.to_srgb8()).unwrap();
    print!("{md}");
    if !fails.is_empty() {
        std::process::exit(1);
    }
}

fn lab_unused() -> [f64; 3] {
    srgb_to_oklab([0.0; 3])
}

fn bounds_h(m: &Mesh) -> f32 {
    let (lo, hi) = m.bounds();
    (hi.z - lo.z.min(0.0)).max(0.05)
}

fn mask(m: &Mesh) -> Vec<bool> {
    const N: usize = 64;
    let (lo, hi) = m.bounds();
    let ext = (hi - lo).max(Vec3::splat(1e-3));
    let mut c = Canvas::with_samples(N, N, Vec3::ZERO, 1);
    let cam = Camera {
        azimuth: 0.0,
        elevation: 0.0,
        target: Vec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, lo.z),
        anchor_px: Vec2::new(N as f32 * 0.5, N as f32 - 1.0),
        px_per_m: (N as f32 - 2.0) / ext.x.max(ext.y).max(ext.z),
        clip: [0, 0, N, N],
    };
    draw(&mut c, m, &cam);
    c.coverage.clone()
}
