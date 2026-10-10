#![allow(clippy::needless_range_loop)] // index-parallel spectrum tables
//! Palette sheet (G1 self-check): the planet palette of a body under its own
//! star, and the same planet under F, G, K and M stars for comparison.
//!
//! cargo run -p astrum_flora --release --example palette_sheet -- [body] [out_dir]
//! Defaults: rust, target/flora-palette. Writes `palette.png` and
//! `palette.md` (OKLCH values, contrast and separation checks).
//!
//! Each row: surface photon spectrum (white), pigment absorption (red) and
//! leaf reflectance (green) over 380–1000 nm; swatches (physical leaf,
//! styled foliage, dry, cold, bark, accent, ground); then per species its
//! leaf, tip, bark and accent swatches and a grown thumbnail.

use std::fmt::Write as _;
use std::path::PathBuf;

use astrum_flora::palette::{BINS, Palette, PlanetLife, apply, load_planet, srgb_to_oklab, surface_spectrum, to_lch, wavelength};
use astrum_flora::raster::{Camera, Canvas, draw, text};
use astrum_flora::{Kit, grow_plant, load_species_dir, variant_seed};
use glam::{Vec2, Vec3};

const ROW_H: usize = 170;
const PLOT_W: usize = 260;
const SW: usize = 34;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let body = args.get(1).map(String::as_str).unwrap_or("rust");
    let out = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("target/flora-palette"));
    std::fs::create_dir_all(&out).unwrap();
    let base = load_planet(std::path::Path::new("content/flora/planets"), body).unwrap();
    let species_files = load_species_dir(std::path::Path::new("content/flora/species")).unwrap();
    let kit = Kit::builtin();
    let at = |s: f64| {
        let mut p = base.clone();
        p.strangeness = s;
        p.realism = None;
        p
    };
    let rows: Vec<(String, PlanetLife)> = std::iter::once((format!("{} {:.0}K", body.to_uppercase(), base.star_temperature_k), base.clone()))
        .chain([("S0.4 USER", at(0.4)), ("S0.7 PROPOSAL", at(0.7))].into_iter().map(|(n, p)| (n.to_string(), p)))
        .chain([("F 6800K", 6800.0), ("G 5772K", 5772.0), ("K 4500K", 4500.0), ("M 3200K", 3200.0)].into_iter().map(|(n, t)| {
            let mut p = base.clone();
            p.star_temperature_k = t;
            (n.to_string(), p)
        }))
        .collect();
    let width = PLOT_W + 7 * SW + species_files.len() * (4 * SW + 150) + 40;
    let mut c = Canvas::new(width, rows.len() * ROW_H + 10, Vec3::new(0.18, 0.18, 0.2));
    let mut md = String::from("# Planet palette sheet\n\n| row | foliage L C h | dry | cold | bark | accent | ground | ΔL foliage–ground | min species ΔE |\n|---|---|---|---|---|---|---|---|---|\n");
    let mut fails = Vec::new();
    for (r, (name, planet)) in rows.iter().enumerate() {
        let y0 = r * ROW_H + 5;
        let pal = Palette::for_planet(planet);
        // Spectrum plot.
        let s = surface_spectrum(planet);
        c.fill_rect(5, y0, 5 + PLOT_W - 10, y0 + ROW_H - 30, Vec3::splat(0.05));
        let px = |i: usize| 5 + (i * (PLOT_W - 10)) / BINS;
        let py = |v: f64| y0 + ROW_H - 31 - ((v.clamp(0.0, 1.0)) * (ROW_H - 40) as f64) as usize;
        for i in 0..BINS {
            let l = wavelength(i);
            let x = px(i);
            for (v, col) in [
                (s[i], Vec3::ONE),
                (pal.pigment.absorption(l), Vec3::new(1.0, 0.2, 0.2)),
                (pal.pigment.reflectance(l) * 1.6, Vec3::new(0.2, 1.0, 0.3)),
            ] {
                c.fill_rect(x, py(v), x + (PLOT_W - 10) / BINS, py(v) + 3, col);
            }
        }
        text(&mut c, 8, y0 + ROW_H - 26, name, 2, Vec3::ONE);
        text(&mut c, 140, y0 + ROW_H - 26, "380-1000NM", 1, Vec3::splat(0.7));
        // Planet swatches.
        let lin = |lch: [f64; 3]| astrum_flora::palette::from_lch(lch);
        let sw = [pal.physical, lin(pal.foliage), lin(pal.dry), lin(pal.cold), lin(pal.bark), lin(pal.accent), planet.ground_albedo];
        let labels = ["PHYS", "LEAF", "DRY", "COLD", "BARK", "ACC", "GRND"];
        for (k, (col, lab)) in sw.iter().zip(labels).enumerate() {
            let x = PLOT_W + k * SW;
            c.fill_rect(x, y0 + 10, x + SW - 4, y0 + 10 + SW * 2, Vec3::new(col[0] as f32, col[1] as f32, col[2] as f32));
            text(&mut c, x, y0 + 14 + SW * 2, lab, 1, Vec3::ONE);
        }
        // Alien hue families at the realism's chroma cap.
        for (k, h) in pal.hue_families(planet).iter().enumerate() {
            let col = lin([0.55, astrum_flora::palette::chroma_cap(planet.realism()), *h]);
            let x = PLOT_W + k * SW;
            c.fill_rect(x, y0 + 30 + SW * 2, x + SW - 4, y0 + 40 + SW * 3, Vec3::new(col[0] as f32, col[1] as f32, col[2] as f32));
        }
        text(&mut c, PLOT_W + 3 * SW, y0 + 34 + SW * 2, "ALIEN HUES", 1, Vec3::ONE);
        // Species.
        let mut species = species_files.clone();
        apply(&mut species, planet);
        let mut labs = Vec::new();
        for (k, sp) in species.iter().enumerate() {
            let x0 = PLOT_W + 7 * SW + 20 + k * (4 * SW + 150);
            let look = &sp.look;
            for (j, col) in [look.organ, look.organ_tip, look.bark, look.accent].iter().enumerate() {
                let x = x0 + j * SW;
                c.fill_rect(x, y0 + 10, x + SW - 4, y0 + 10 + SW * 2, Vec3::from(*col));
            }
            text(&mut c, x0, y0 + 14 + SW * 2, &sp.name, 1, Vec3::ONE);
            let lch = to_lch(look.organ.map(|v| v as f64));
            text(&mut c, x0, y0 + 24 + SW * 2, &format!("L{:.2} C{:.2} H{:.0}", lch[0], lch[1], lch[2]), 1, Vec3::splat(0.8));
            labs.push(srgb_to_oklab(look.organ.map(|v| v as f64)));
            // Thumbnail on a ground-coloured backdrop.
            let p = grow_plant(sp, &kit, variant_seed(sp, 0));
            let (lo, hi) = p.lods[0].bounds();
            let ext = (hi - lo).max_element().max(0.1);
            let tx = x0 + 4 * SW + 5;
            let g = planet.ground_albedo;
            c.fill_rect(tx, y0 + 5, tx + 140, y0 + ROW_H - 10, Vec3::new(0.35, 0.42, 0.5));
            c.fill_rect(tx, y0 + ROW_H - 30, tx + 140, y0 + ROW_H - 10, Vec3::new(g[0] as f32, g[1] as f32, g[2] as f32));
            let cam = Camera {
                azimuth: 0.5,
                elevation: 0.12,
                target: Vec3::ZERO,
                anchor_px: Vec2::new(tx as f32 + 70.0, (y0 + ROW_H - 30) as f32),
                px_per_m: (ROW_H as f32 - 45.0) / ext,
                clip: [tx, y0 + 5, tx + 140, y0 + ROW_H - 10],
            };
            draw(&mut c, &p.lods[0], &cam);
        }
        // Checks.
        let ground_l = to_lch(planet.ground_albedo)[0];
        let dl = (pal.foliage[0] - ground_l).abs();
        let mut min_de = f64::MAX;
        for a in 0..labs.len() {
            for b in a + 1..labs.len() {
                let d = ((labs[a][0] - labs[b][0]).powi(2) + (labs[a][1] - labs[b][1]).powi(2) + (labs[a][2] - labs[b][2]).powi(2)).sqrt();
                min_de = min_de.min(d);
            }
        }
        let f = |v: [f64; 3]| format!("{:.2} {:.3} {:.0}", v[0], v[1], v[2]);
        let _ = writeln!(md, "| {name} | {} | {} | {} | {} | {} | {} | {dl:.3} | {min_de:.3} |", f(pal.foliage), f(pal.dry), f(pal.cold), f(pal.bark), f(pal.accent), f(to_lch(planet.ground_albedo)));
        if r == 0 {
            if dl < 0.05 {
                fails.push(format!("foliage and ground lightness too close ({dl:.3})"));
            }
            if min_de < 0.02 {
                fails.push(format!("species colours too close (ΔE {min_de:.3})"));
            }
        }
    }
    let _ = writeln!(md, "\nChecks on the body's own row: ΔL foliage–ground ≥ 0.05, species ΔE (OKLab) ≥ 0.02. {}", if fails.is_empty() { "All pass.".to_string() } else { format!("FAIL: {}", fails.join("; ")) });
    std::fs::write(out.join("palette.md"), &md).unwrap();
    let file = std::fs::File::create(out.join("palette.png")).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), c.width as u32, c.height as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&c.to_srgb8()).unwrap();
    print!("{md}");
    if !fails.is_empty() {
        std::process::exit(1);
    }
}
