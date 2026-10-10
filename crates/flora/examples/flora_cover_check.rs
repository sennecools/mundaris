//! Checks the far canopy tint's coverage model against placed plants: for a
//! grid of climates, places every plant of a 64×64-cell window the way the
//! GPU scatter does (one jittered candidate per 10 m cell, species pick,
//! scale), stamps each plant's real LOD0 silhouette seen from above, and
//! compares the covered share with the model `Placement::forest_view`
//! (straight down) averaged over the same cells; then thins the plants to
//! keep 0.3 and compares the tint strength the model gives with the share
//! of the remaining view the left-out plants hide.
//!
//! cargo run -p astrum_flora --release --example flora_cover_check -- [out.md]

use std::fmt::Write as _;

use astrum_flora::niche::{Layer, Site};
use astrum_flora::scatter::{Placement, pcg3d, unit};
use astrum_flora::{Kit, grow_meshes, variant_seed};

const CELL_M: f64 = 10.0;
const CELLS: usize = 64;
/// Window raster pixel (m).
const PX: f64 = 0.25;
/// Silhouette mask pixel (m) at scale 1.
const MASK_PX: f64 = 0.1;
/// Share of plants kept by thinning for the tint check.
const KEEP: f64 = 0.3;

struct Mask {
    /// Half extent (m at scale 1), grid side, covered pixels.
    r: f64,
    n: usize,
    hit: Vec<bool>,
}

fn silhouette(mesh: &astrum_flora::Mesh) -> Mask {
    let p: Vec<[f64; 2]> = mesh.vertices.iter().map(|v| [v.position[0] as f64, v.position[1] as f64]).collect();
    let r = p.iter().map(|q| q[0].abs().max(q[1].abs())).fold(0.0, f64::max) + MASK_PX;
    let n = (2.0 * r / MASK_PX).ceil() as usize;
    let mut hit = vec![false; n * n];
    let edge = |a: [f64; 2], b: [f64; 2], x: f64, y: f64| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
    for t in mesh.indices.as_chunks::<3>().0 {
        let v = [0, 1, 2].map(|i| {
            let q = p[t[i] as usize];
            [(q[0] + r) / MASK_PX, (q[1] + r) / MASK_PX]
        });
        let d = edge(v[0], v[1], v[2][0], v[2][1]);
        if d.abs() < 1e-12 {
            continue;
        }
        let lo_x = v.iter().map(|q| q[0]).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let hi_x = (v.iter().map(|q| q[0]).fold(f64::MIN, f64::max).ceil() as usize).min(n);
        let lo_y = v.iter().map(|q| q[1]).fold(f64::MAX, f64::min).floor().max(0.0) as usize;
        let hi_y = (v.iter().map(|q| q[1]).fold(f64::MIN, f64::max).ceil() as usize).min(n);
        for y in lo_y..hi_y {
            for x in lo_x..hi_x {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                if [edge(v[1], v[2], cx, cy), edge(v[2], v[0], cx, cy), edge(v[0], v[1], cx, cy)]
                    .iter()
                    .all(|&w| w * d.signum() >= 0.0)
                {
                    hit[y * n + x] = true;
                }
            }
        }
    }
    Mask { r, n, hit }
}

fn main() {
    let out = std::env::args().nth(1);
    let dir = std::path::Path::new("content/flora");
    let species = astrum_flora::load_species_for_body(&dir.join("species"), &dir.join("planets"), "rust").unwrap();
    let kit = Kit::builtin();
    let masks: Vec<Mask> = species.iter().map(|sp| silhouette(&grow_meshes(sp, &kit, variant_seed(sp, 0)).1[0])).collect();
    let placement = Placement { species: &species };
    let mut md = String::from(
        "| T °C | M | tree | shrub | model cover | placed cover | placed / model | model tint (keep 0.3) | placed tint (keep 0.3) |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    let (mut worst, mut n_rows) = (0.0f64, 0);
    let side = (CELLS as f64 * CELL_M / PX) as usize;
    for t in [-5.0, 5.0, 12.0, 20.0, 28.0] {
        for m in [0.3, 0.5, 0.7, 0.9] {
            let s = Site { temperature_c: t, moisture: m, height_m: 400.0, slope: 0.05, sediment: 0.5 };
            let mut raster = vec![false; side * side];
            let mut kept = vec![false; side * side];
            let (mut model, mut model_tint, mut tree, mut shrub) = (0.0, 0.0, 0.0, 0.0);
            for cj in 0..CELLS {
                for ci in 0..CELLS {
                    let (gi, gj) = (5000.0 + ci as f64, 7000.0 + cj as f64);
                    let f = placement.forest_view(2, gi, gj, 0.0, &s, [1.0 / (CELL_M * CELL_M), 0.0, 1.0]);
                    model += f.cover;
                    model_tint += placement.forest_view(2, gi, gj, 0.0, &s, [1.0 / (CELL_M * CELL_M), KEEP, 1.0]).cover;
                    tree += f.tree;
                    shrub += f.shrub;
                    let h = pcg3d([5000 + ci as u32, 7000 + cj as u32, 2 * 64 + 63]);
                    let h2 = pcg3d([h[0] ^ 0x5ca77e5, h[1] ^ 0x5ca77e5, h[2] ^ 0x5ca77e5]);
                    let roll = unit(h[2]);
                    if roll >= f.tree + f.shrub {
                        continue;
                    }
                    let canopy = roll < f.tree;
                    let layer = if canopy { Layer::Canopy } else { Layer::Shrub };
                    let Some((k, su)) = placement.pick(layer, &s, unit(h2[0])) else { continue };
                    let (s0, s1) = species[k].niche.scale;
                    let mut scale = s0 + (s1 - s0) * unit(h2[1]);
                    if canopy {
                        scale *= (0.45 + 0.55 * su) * (0.6 + 0.4 * f.core);
                    }
                    let x0 = (ci as f64 + 0.15 + 0.7 * unit(h[0])) * CELL_M;
                    let y0 = (cj as f64 + 0.15 + 0.7 * unit(h[1])) * CELL_M;
                    let yaw = unit(h2[2]) * std::f64::consts::TAU;
                    let (sn, cs) = yaw.sin_cos();
                    let is_kept = unit(h2[2] ^ h[0]) < KEEP;
                    // Point-sample the window pixels the plant spans in its mask.
                    let mk = &masks[k];
                    let reach = scale * mk.r * std::f64::consts::SQRT_2;
                    let lo = |c: f64| ((c - reach) / PX).floor().max(0.0) as usize;
                    let hi = |c: f64| (((c + reach) / PX).ceil().max(0.0) as usize).min(side);
                    for py in lo(y0)..hi(y0) {
                        for px in lo(x0)..hi(x0) {
                            let (dx, dy) = ((px as f64 + 0.5) * PX - x0, (py as f64 + 0.5) * PX - y0);
                            let (u, v) = ((cs * dx + sn * dy) / scale, (-sn * dx + cs * dy) / scale);
                            let (mx, my) = ((u + mk.r) / MASK_PX, (v + mk.r) / MASK_PX);
                            if mx >= 0.0 && my >= 0.0 && (mx as usize) < mk.n && (my as usize) < mk.n && mk.hit[my as usize * mk.n + mx as usize] {
                                raster[py * side + px] = true;
                                kept[py * side + px] |= is_kept;
                            }
                        }
                    }
                }
            }
            // Ignore a 3-cell border (plants reaching in from outside are missing).
            let b = (3.0 * CELL_M / PX) as usize;
            let (mut hit, mut hit_kept, mut all) = (0usize, 0usize, 0usize);
            for y in b..side - b {
                for x in b..side - b {
                    all += 1;
                    hit += raster[y * side + x] as usize;
                    hit_kept += kept[y * side + x] as usize;
                }
            }
            let n = (CELLS * CELLS) as f64;
            let (model, placed) = (model / n, hit as f64 / all as f64);
            let ratio = placed / model.max(1e-9);
            let d = hit_kept as f64 / all as f64;
            let placed_tint = (placed - d) / (1.0 - d).max(1e-9);
            let model_tint = model_tint / n;
            if model > 0.1 {
                worst = f64::max(worst, (ratio - 1.0).abs());
                n_rows += 1;
            }
            let _ = writeln!(
                md,
                "| {t} | {m} | {:.3} | {:.3} | {model:.3} | {placed:.3} | {ratio:.2} | {model_tint:.3} | {placed_tint:.3} |",
                tree / n,
                shrub / n
            );
        }
    }
    let _ = writeln!(
        md,
        "\nWorst |placed / model − 1| over {n_rows} climates with model cover > 0.1: {:.1} %. Placed = real LOD0 silhouettes from above (random yaw, GPU scale rule), {CELL_M} m cells, {PX} m raster.",
        100.0 * worst
    );
    print!("{md}");
    if let Some(o) = out {
        std::fs::write(o, &md).unwrap();
    }
}
