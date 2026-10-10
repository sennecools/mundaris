//! Rock contact sheet and scorer (G5 self-check): every archetype in
//! `content/flora/rocks`, 5 variants dry and young (top row) and wet and old
//! (bottom row, rounded, mossy), plus LOD1/LOD2 of variant 0.
//!
//! cargo run -p astrum_flora --release --example rock_sheet -- [out_dir]
//! Writes `<archetype>.png` and `rocks.md` (triangles per LOD, closed-edge
//! fraction, size, grow time). Exits non-zero when a check fails.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use astrum_flora::hash::{derive, name_key};
use astrum_flora::palette::{Palette, from_lch, load_planet};
use astrum_flora::raster::{Camera, Canvas, draw, text};
use astrum_flora::rock::{Weathering, closed_edges_fraction, grow_rock, load_rocks_dir};
use glam::{Vec2, Vec3};

const CELL: usize = 200;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("target/flora-rocks"));
    std::fs::create_dir_all(&out).unwrap();
    let rocks = load_rocks_dir(std::path::Path::new("content/flora/rocks")).unwrap();
    let planet = load_planet(std::path::Path::new("content/flora/planets"), "rust").unwrap();
    let pal = Palette::for_planet(&planet);
    let moss = from_lch([pal.foliage[0] - 0.08, pal.foliage[1] * 0.8, pal.foliage[2]]).map(|v| v as f32);
    let states = [
        ("DRY YOUNG", Weathering { wetness: 0.1, age: 0.2, moss, moss_cover: 0.0 }),
        ("WET OLD", Weathering { wetness: 0.9, age: 0.9, moss, moss_cover: 0.8 }),
    ];
    let mut md = String::from("| rock | state | variant | ms | tris L0/L1/L2 | closed L0 | size m |\n|---|---|---|---|---|---|---|\n");
    let mut fails = Vec::new();
    for rock in &rocks {
        let cols = 5 + 2;
        let mut c = Canvas::new(cols * CELL, 2 * CELL + 30, Vec3::new(0.55, 0.62, 0.7));
        text(&mut c, 6, 6, &rock.name, 3, Vec3::splat(0.05));
        for (row, (label, w)) in states.iter().enumerate() {
            let y0 = 30 + row * CELL;
            c.fill_rect(0, y0 + CELL - 30, cols * CELL, y0 + CELL, Vec3::new(0.2, 0.16, 0.12));
            text(&mut c, 6, y0 + 4, label, 2, Vec3::splat(0.05));
            for v in 0..5u64 {
                let seed = derive(name_key(&rock.name), v);
                let t = Instant::now();
                let lods = grow_rock(rock, seed, w);
                let ms = t.elapsed().as_secs_f64() * 1e3;
                let tris: Vec<usize> = lods.iter().map(|m| m.triangles()).collect();
                let closed = closed_edges_fraction(&lods[0]);
                let (lo, hi) = lods[0].bounds();
                let size = (hi - lo).max_element();
                let _ = writeln!(md, "| {} | {label} | {v} | {ms:.1} | {}/{}/{} | {closed:.3} | {size:.2} |", rock.name, tris[0], tris[1], tris[2]);
                if closed < 0.98 {
                    fails.push(format!("{} v{v} {label}: closed {closed:.3}", rock.name));
                }
                if tris[0] > 3000 || tris[2] > tris[1] || tris[1] > tris[0] || tris[2] == 0 {
                    fails.push(format!("{} v{v}: tris {tris:?}", rock.name));
                }
                let draws: Vec<usize> = if v == 0 { vec![0, 1, 2] } else { vec![0] };
                for l in draws {
                    let col = if l == 0 { v as usize } else { 4 + l };
                    let px = (CELL as f32 - 40.0) / (rock.size_m as f32 * 1.4);
                    let cam = Camera {
                        azimuth: 0.6,
                        elevation: 0.35,
                        target: Vec3::ZERO,
                        anchor_px: Vec2::new(col as f32 * CELL as f32 + CELL as f32 * 0.5, (y0 + CELL - 40) as f32),
                        px_per_m: px,
                        clip: [col * CELL, y0, (col + 1) * CELL, y0 + CELL],
                    };
                    draw(&mut c, &lods[l], &cam);
                    if l > 0 {
                        text(&mut c, col * CELL + 4, y0 + 20, &format!("LOD{l} {}", tris[l]), 2, Vec3::splat(0.05));
                    }
                }
            }
        }
        let path = out.join(format!("{}.png", rock.name));
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), c.width as u32, c.height as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&c.to_srgb8()).unwrap();
    }
    let _ = writeln!(md, "\nChecks: closed-edge fraction ≥ 0.98, LOD0 ≤ 3000 triangles, LODs decreasing. {}", if fails.is_empty() { "All pass.".into() } else { format!("FAIL: {}", fails.join("; ")) });
    std::fs::write(out.join("rocks.md"), &md).unwrap();
    print!("{md}");
    if !fails.is_empty() {
        std::process::exit(1);
    }
}
