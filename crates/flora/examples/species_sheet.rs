//! Contact sheet: line-ups of N seeds per species, plus LOD1/LOD2 of the
//! first seed, with metrics printed under each plant.
//!
//! cargo run -p astrum_flora --release --example species_sheet -- \
//!     [species_dir] [out_dir] [seeds] [planets_dir] [body]
//! Defaults: content/flora/species, target/flora-sheets, 6.
//! Writes `<species>.png` per species and `metrics.md` (table) to out_dir.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use astrum_flora::metrics::Bands;
use astrum_flora::raster::{Camera, Canvas, draw, text};
use astrum_flora::{Kit, LOD_COUNT, grow_plant, load_species_for_body, variant_seed};
use glam::{Vec2, Vec3};

const CELL_W: usize = 240;
const CELL_H: usize = 300;
const LABEL_H: usize = 46;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(
        args.get(1)
            .map(String::as_str)
            .unwrap_or("content/flora/species"),
    );
    let out = PathBuf::from(
        args.get(2)
            .map(String::as_str)
            .unwrap_or("target/flora-sheets"),
    );
    let seeds: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
    std::fs::create_dir_all(&out).unwrap();
    let kit = Kit::builtin();
    let planets = PathBuf::from(args.get(4).map(String::as_str).unwrap_or("content/flora/planets"));
    let body = args.get(5).map(String::as_str).unwrap_or("rust");
    let species = load_species_for_body(&dir, &planets, body).unwrap_or_else(|e| panic!("{e}"));
    let mut md = String::from(
        "| species | seed | grow ms | nodes | organs | tris L0/L1/L2 | KB L0/L1/L2 | height m | width m | base r m | aspect | fill | fd | base w | intersect | shed | fails |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for sp in &species {
        let mut plants = Vec::new();
        for v in 0..seeds {
            let seed = variant_seed(sp, v);
            let t = Instant::now();
            let p = grow_plant(sp, &kit, seed);
            let ms = t.elapsed().as_secs_f64() * 1e3;
            plants.push((v, p, ms));
        }
        let cols = seeds as usize + LOD_COUNT - 1;
        let mut c = Canvas::new(
            cols * CELL_W,
            CELL_H + LABEL_H + 18,
            Vec3::new(0.62, 0.68, 0.74),
        );
        // Shared scale per sheet so sizes compare.
        let (mut eh, mut ew) = (0.1f32, 0.1f32);
        for (_, p, _) in &plants {
            let (lo, hi) = p.lods[0].bounds();
            eh = eh.max(hi.z - lo.z.min(0.0));
            ew = ew.max((hi.x - lo.x).max(hi.y - lo.y));
        }
        let px = ((CELL_H as f32 - 24.0) / eh).min((CELL_W as f32 - 8.0) / ew);
        let ground = CELL_H as f32 - 8.0;
        c.fill_rect(
            0,
            ground as usize,
            cols * CELL_W,
            CELL_H,
            Vec3::new(0.16, 0.13, 0.1),
        );
        let draw_cell = |c: &mut Canvas, col: usize, mesh: &astrum_flora::Mesh| {
            let cam = Camera {
                azimuth: 0.5,
                elevation: 0.12,
                target: Vec3::ZERO,
                anchor_px: Vec2::new(col as f32 * CELL_W as f32 + CELL_W as f32 * 0.5, ground),
                px_per_m: px,
                clip: [col * CELL_W, 0, (col + 1) * CELL_W, CELL_H],
            };
            draw(c, mesh, &cam);
        };
        let white = Vec3::ONE;
        let red = Vec3::new(1.0, 0.25, 0.2);
        for (v, p, ms) in &plants {
            let col = *v as usize;
            draw_cell(&mut c, col, &p.lods[0]);
            let m = &p.metrics;
            let fails = m.failures(&Bands::HERO);
            let x = col * CELL_W + 6;
            let y = CELL_H + 4;
            text(
                &mut c,
                x,
                y,
                &format!("SEED {v} L0 {} TRIS", m.triangles[0]),
                2,
                white,
            );
            text(
                &mut c,
                x,
                y + 14,
                &format!(
                    "FD {:.2} FILL {:.2} {:.0}MS",
                    m.silhouette.fractal_dimension, m.silhouette.crown_fill, ms
                ),
                2,
                white,
            );
            if !fails.is_empty() {
                text(
                    &mut c,
                    x,
                    y + 28,
                    &fails.join(" ").chars().take(28).collect::<String>(),
                    2,
                    red,
                );
            }
            let s = &m.structure;
            let _ = writeln!(
                md,
                "| {} | {v} | {ms:.1} | {} | {} | {}/{}/{} | {:.0}/{:.0}/{:.0} | {:.1} | {:.1} | {:.3} | {:.2} | {:.2} | {:.2} | {:.2} | {} | {} | {} |",
                sp.name,
                s.nodes,
                s.organs,
                m.triangles[0],
                m.triangles[1],
                m.triangles[2],
                m.bytes[0] as f64 / 1024.0,
                m.bytes[1] as f64 / 1024.0,
                m.bytes[2] as f64 / 1024.0,
                s.height_m,
                s.width_m,
                s.base_radius_m,
                m.silhouette.aspect,
                m.silhouette.crown_fill,
                m.silhouette.fractal_dimension,
                m.silhouette.base_width_ratio,
                s.intersecting,
                s.shed_branches,
                if fails.is_empty() {
                    "-".into()
                } else {
                    fails.join(", ")
                },
            );
        }
        // LOD1 and LOD2 of seed 0.
        let p0 = &plants[0].1;
        for l in 1..LOD_COUNT {
            let col = seeds as usize + l - 1;
            draw_cell(&mut c, col, &p0.lods[l]);
            text(
                &mut c,
                col * CELL_W + 6,
                CELL_H + 4,
                &format!("SEED 0 LOD{l} {} TRIS", p0.metrics.triangles[l]),
                2,
                white,
            );
        }
        text(&mut c, 6, 6, &sp.name, 3, Vec3::new(0.05, 0.05, 0.05));
        text(
            &mut c,
            6,
            26,
            &format!("SCALE BAR 1M = {:.0}PX", px),
            2,
            Vec3::new(0.05, 0.05, 0.05),
        );
        c.fill_rect(6, 42, 6 + px as usize, 46, Vec3::new(0.05, 0.05, 0.05));
        let path = out.join(format!("{}.png", sp.name));
        write_png(&path, &c);
        println!("wrote {}", path.display());
    }
    std::fs::write(out.join("metrics.md"), &md).unwrap();
    print!("{md}");
}

fn write_png(path: &std::path::Path, c: &Canvas) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(file),
        c.width as u32,
        c.height as u32,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&c.to_srgb8())
        .unwrap();
}
