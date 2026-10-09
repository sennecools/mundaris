//! Point-evaluated branching gully filter on a periodic height grid.
//!
//! An original implementation after the approach described by Rune Skovbo
//! Johansen ("Fast and Gorgeous Erosion Filter", 2026): each octave blends sine
//! stripes that run down the local gradient from jittered pivots on a periodic
//! cell grid; the stripes fade toward a ridge/valley target where the slope is
//! small; and every octave's gully slope steers the next, finer octave so new
//! gullies branch off the coarser ones. No water is simulated: the filter adds
//! sub-drainage detail that the following erosion stage then settles.
//!
//! Every quantity is periodic over the tile (cell counts per tile are integers
//! and hashing uses wrapped cell indices), so the output still tiles.

use crate::derive::{abs_quantile, box_blur};
use crate::noise::pcg;
use crate::recipe::GullyRecipe;
use std::f64::consts::TAU;

/// Ease-out used for slope masks: 0 at 0, 1 at and above 1, zero slope at 1.
fn ease(t: f64) -> f64 {
    let v = 1.0 - t.clamp(0.0, 1.0);
    1.0 - v * v
}

fn unit_hash(x: u32, y: u32, seed: u32) -> f64 {
    f64::from(pcg(x ^ pcg(y ^ pcg(seed)))) / f64::from(u32::MAX)
}

/// Add gullies to `height_m` (an `n²` periodic grid with `cell_m` spacing) in place.
pub fn apply(height_m: &mut [f64], n: usize, cell_m: f64, recipe: &GullyRecipe, seed: u32) {
    let footprint_m = n as f64 * cell_m;
    let wrap = |v: i64| v.rem_euclid(n as i64) as usize;

    // Ridge/valley fade target in [-1, 1] from relief relative to a local mean,
    // normalized by a robust tile-wide scale.
    let radius = ((recipe.fade_radius_m / cell_m).round() as usize).clamp(1, n / 2);
    let local_mean = box_blur(height_m, n, radius);
    let relief: Vec<f64> = height_m
        .iter()
        .zip(&local_mean)
        .map(|(h, m)| h - m)
        .collect();
    let relief_scale = abs_quantile(&relief, 0.9).max(1.0e-9);

    let octaves: Vec<(f64, f64, usize, f64, u32)> = (0..recipe.octaves)
        .map(|o| {
            let wavelength = recipe.wavelength_m / recipe.lacunarity.powi(o as i32);
            let amplitude = recipe.amplitude_m * recipe.gain.powi(o as i32);
            let cells = ((footprint_m / (wavelength * recipe.cell_ratio)).round() as usize).max(1);
            (
                wavelength,
                amplitude,
                cells,
                footprint_m / cells as f64,
                pcg(seed ^ o.wrapping_mul(0x9e37_79b9)),
            )
        })
        .collect();

    let mut delta = vec![0.0; n * n];
    for y in 0..n {
        for x in 0..n {
            let i = y * n + x;
            let gx = (height_m[y * n + wrap(x as i64 + 1)] - height_m[y * n + wrap(x as i64 - 1)])
                / (2.0 * cell_m);
            let gy = (height_m[wrap(y as i64 + 1) * n + x] - height_m[wrap(y as i64 - 1) * n + x])
                / (2.0 * cell_m);
            let (px, py) = ((x as f64 + 0.5) * cell_m, (y as f64 + 0.5) * cell_m);
            let mut gully_gradient = (gx, gy);
            let mut fade = (relief[i] / relief_scale).clamp(-1.0, 1.0);
            let mut combined_mask = 1.0;
            let mut added = 0.0;
            for &(wavelength, amplitude, cells, cell_size, octave_seed) in &octaves {
                let slope = gully_gradient.0.hypot(gully_gradient.1);
                if slope < 1.0e-9 || combined_mask <= 0.0 {
                    break;
                }
                // Stripes run along the gradient, so phase varies across it.
                let across = (-gully_gradient.1 / slope, gully_gradient.0 / slope);
                let (cx, cy) = (
                    (px / cell_size).floor() as i64,
                    (py / cell_size).floor() as i64,
                );
                let (mut c, mut s, mut total) = (0.0, 0.0, 0.0);
                for j in -1..=1 {
                    for k in -1..=1 {
                        let (ux, uy) = (cx + k, cy + j);
                        let (hx, hy) = (
                            ux.rem_euclid(cells as i64) as u32,
                            uy.rem_euclid(cells as i64) as u32,
                        );
                        let pivot_x = (ux as f64 + unit_hash(hx, hy, octave_seed)) * cell_size;
                        let pivot_y =
                            (uy as f64 + unit_hash(hx, hy, octave_seed ^ 0x68e3_1da4)) * cell_size;
                        let (rx, ry) = (px - pivot_x, py - pivot_y);
                        let distance = rx.hypot(ry) / cell_size;
                        let w = (1.0 - distance / 1.5).max(0.0).powi(2);
                        if w > 0.0 {
                            let phase = TAU * (rx * across.0 + ry * across.1) / wavelength;
                            c += w * phase.cos();
                            s += w * phase.sin();
                            total += w;
                        }
                    }
                }
                if total <= 0.0 {
                    continue;
                }
                // Blended stripes shrink where pivots disagree; renormalize only
                // the part above half length to avoid loop artefacts.
                let (c, s) = (c / total, s / total);
                let length = c.hypot(s);
                let scale = if length > 0.0 {
                    (2.0 * length).min(1.0) / length
                } else {
                    0.0
                };
                let (gully, gully_slope) = (c * scale, s * scale);

                let mask = ease(slope / recipe.slope_full) * combined_mask;
                let shaped = fade + mask * (gully - fade);
                added += amplitude * (shaped - fade);
                // Straight-gully steering: a constant-magnitude slope of the
                // gully's sign (triangle-wave equivalent, 4a/λ) across the gradient.
                let steer = -gully_slope.signum() * mask * 4.0 * amplitude / wavelength;
                gully_gradient.0 += steer * across.0;
                gully_gradient.1 += steer * across.1;
                // Finer octaves avoid this octave's ridges and creases.
                if recipe.detail > 0.0 {
                    combined_mask *= ease(gully_slope.abs() * 2.0).powf(recipe.detail);
                }
                fade = shaped;
            }
            delta[i] = added;
        }
    }
    for (h, d) in height_m.iter_mut().zip(&delta) {
        *h += d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> GullyRecipe {
        GullyRecipe {
            wavelength_m: 80.0,
            octaves: 3,
            lacunarity: 2.0,
            amplitude_m: 6.0,
            gain: 0.5,
            cell_ratio: 1.0,
            slope_full: 0.3,
            detail: 1.0,
            fade_radius_m: 100.0,
        }
    }

    #[test]
    fn flat_ground_is_unchanged_and_slopes_are_cut_periodically() {
        let n = 64;
        let cell = 8.0;
        let mut flat = vec![5.0; n * n];
        apply(&mut flat, n, cell, &recipe(), 3);
        assert!(flat.iter().all(|&h| h == 5.0));

        // A periodic ridge/valley pattern along x.
        let base: Vec<f64> = (0..n * n)
            .map(|i| 100.0 * (TAU * (i % n) as f64 / n as f64).sin())
            .collect();
        let mut cut = base.clone();
        apply(&mut cut, n, cell, &recipe(), 3);
        let max_change = cut
            .iter()
            .zip(&base)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max);
        assert!(max_change > 1.0 && max_change < 2.0 * 6.0 * 2.0);

        // Wrap neighbours change as smoothly as interior neighbours.
        let d: Vec<f64> = cut.iter().zip(&base).map(|(a, b)| a - b).collect();
        let step = |a: usize, b: usize| (d[a] - d[b]).abs();
        let wrap_max = (0..n)
            .map(|y| step(y * n + n - 1, y * n))
            .chain((0..n).map(|x| step((n - 1) * n + x, x)))
            .fold(0.0, f64::max);
        let interior_max = (0..n)
            .flat_map(|y| (0..n - 1).map(move |x| (y, x)))
            .map(|(y, x)| step(y * n + x, y * n + x + 1))
            .fold(0.0, f64::max);
        assert!(
            wrap_max <= interior_max * 1.5,
            "{wrap_max} vs {interior_max}"
        );
    }
}
