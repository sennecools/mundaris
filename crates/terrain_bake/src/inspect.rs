//! Terrain inspection: diagnostic images plus organisation metrics for any
//! heightmap (a library bundle, a raw r16 or a raw f32), periodic or not.
//!
//! The goal is to make terrain quality legible to a reviewer (human or AI).
//! Panels are rendered side by side at the same scale:
//! - two hillshades with different light directions (no crater/dome illusion
//!   from a single light)
//! - slope, curvature (ridges red, valleys blue), drainage area, closed
//!   depressions (craters, pits)
//! - a gradient-orientation rose
//! - 1:1 full-resolution crops
//!
//! Metrics describe structure, not just energy: slope and curvature
//! distributions, grid bias, flow alignment of fine texture, drainage-area and
//! slope–area statistics, depression (crater) statistics, ridge and valley
//! continuity, and spatial heterogeneity. See `score.rs` for comparison.

use crate::font;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs;
use std::path::Path;

/// A height grid in metres.
pub struct Grid {
    pub width: usize,
    pub height: usize,
    pub cell_m: f64,
    pub periodic: bool,
    pub h: Vec<f64>,
    /// Preprocessing applied before analysis, recorded in `metrics.json` so
    /// `score` can refuse mismatched comparisons.
    pub protocol: Protocol,
}

/// Preprocessing applied to a grid (see [`Grid::detrend`], [`Grid::smooth_in_place`]).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Protocol {
    /// A least-squares plane was removed (non-periodic grids only).
    pub plane_removed: bool,
    /// Gaussian smoothing standard deviation in metres (0 = none).
    pub smooth_m: f64,
}

/// How to read a heightmap.
pub struct Source {
    pub path: std::path::PathBuf,
    /// `r16le`, `r16be` or `f32le`; ignored for bundle directories.
    pub format: String,
    pub width: usize,
    pub height: usize,
    pub cell_m: f64,
    /// r16 only: physical relief spanned by the sample range.
    pub relief_m: f64,
    pub periodic: bool,
    /// GeoTIFF only: explicit window (x, y, w, h) in pixels.
    pub window: Option<(usize, usize, usize, usize)>,
    /// GeoTIFF only: pick the `index`-th fully valid square window of this size
    /// (default: the middle one in scan order).
    pub auto_window: Option<(usize, Option<usize>)>,
}

impl Grid {
    pub fn load(source: &Source) -> Result<Self> {
        if source.path.is_dir() {
            return Self::load_bundle(&source.path);
        }
        let ext = source
            .path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "tif" || ext == "tiff" {
            return Self::load_tiff(source);
        }
        let bytes =
            fs::read(&source.path).with_context(|| format!("reading {}", source.path.display()))?;
        let (w, hgt) = (source.width, source.height);
        ensure!(w >= 16 && hgt >= 16, "grid must be at least 16×16");
        ensure!(source.cell_m > 0.0, "cell size must be positive");
        let h: Vec<f64> = match source.format.as_str() {
            "r16le" | "r16be" => {
                ensure!(
                    bytes.len() == w * hgt * 2,
                    "r16 size does not match {w}×{hgt}"
                );
                let raw: Vec<f64> = bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&p| {
                        f64::from(if source.format == "r16be" {
                            u16::from_be_bytes(p)
                        } else {
                            u16::from_le_bytes(p)
                        })
                    })
                    .collect();
                let (lo, hi) = extrema(&raw);
                let scale = source.relief_m / (hi - lo).max(1.0);
                raw.iter().map(|v| (v - lo) * scale).collect()
            }
            "f32le" => {
                ensure!(
                    bytes.len() == w * hgt * 4,
                    "f32 size does not match {w}×{hgt}"
                );
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|&p| f64::from(f32::from_le_bytes(p)))
                    .collect()
            }
            other => bail!("unknown format {other} (r16le, r16be, f32le)"),
        };
        ensure!(h.iter().all(|v| v.is_finite()), "heights must be finite");
        Ok(Self {
            width: w,
            height: hgt,
            cell_m: source.cell_m,
            periodic: source.periodic,
            h,
            protocol: Protocol::default(),
        })
    }

    fn load_tiff(source: &Source) -> Result<Self> {
        let raster = crate::tiff::read(&source.path)?;
        let cell_m = raster.pixel_scale.unwrap_or(source.cell_m);
        ensure!(cell_m > 0.0, "GeoTIFF has no pixel scale; pass --cell");
        let valid = |v: f32| v.is_finite() && v > -1.0e30;
        let (rw, rh) = (raster.width, raster.height);
        let (x0, y0, w, h) = if let Some(win) = source.window {
            win
        } else if let Some((size, index)) = source.auto_window {
            ensure!(
                size <= rw && size <= rh,
                "window {size} larger than raster {rw}×{rh}"
            );
            // Prefix count of invalid cells per row for fast window checks.
            let mut candidates = Vec::new();
            let stride = (size / 4).max(1);
            let bad_rows: Vec<Vec<u32>> = (0..rh)
                .map(|y| {
                    let mut acc = vec![0u32; rw + 1];
                    for x in 0..rw {
                        acc[x + 1] = acc[x] + u32::from(!valid(raster.data[y * rw + x]));
                    }
                    acc
                })
                .collect();
            let mut y = 0;
            while y + size <= rh {
                let mut x = 0;
                while x + size <= rw {
                    if (y..y + size).all(|yy| bad_rows[yy][x + size] == bad_rows[yy][x]) {
                        candidates.push((x, y));
                    }
                    x += stride;
                }
                y += stride;
            }
            ensure!(
                !candidates.is_empty(),
                "no fully valid {size}² window in {}",
                source.path.display()
            );
            let k = index.unwrap_or(candidates.len() / 2);
            let (x, y) = *candidates
                .get(k)
                .with_context(|| format!("window index {k} of {}", candidates.len()))?;
            eprintln!("window {k}/{} at ({x}, {y}) size {size}", candidates.len());
            (x, y, size, size)
        } else {
            (0, 0, rw, rh)
        };
        ensure!(
            x0 + w <= rw && y0 + h <= rh && w >= 16 && h >= 16,
            "window outside raster"
        );
        let mut out = Vec::with_capacity(w * h);
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                let v = raster.data[y * rw + x];
                ensure!(valid(v), "window contains no-data at ({x}, {y})");
                out.push(f64::from(v));
            }
        }
        Ok(Self {
            width: w,
            height: h,
            cell_m,
            periodic: false,
            h: out,
            protocol: Protocol::default(),
        })
    }

    /// Remove the least-squares plane (regional tilt of real DEM windows).
    ///
    /// A periodic tile has no regional tilt by construction (it wraps), and
    /// removing a plane would put a step along the wrap seam that every
    /// periodic stencil then reads as a long sharp crease: fake fine-band
    /// roughness, kurtosis, grid bias and valley continuity, and a broken
    /// flow alignment. So periodic grids only lose their mean.
    pub fn detrend(&mut self) {
        if self.periodic {
            let mean = self.h.iter().sum::<f64>() / self.h.len().max(1) as f64;
            self.h.iter_mut().for_each(|z| *z -= mean);
            return;
        }
        self.protocol.plane_removed = true;
        let (w, h) = (self.width, self.height);
        let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        let (mut sx, mut sy, mut sz) = (0.0, 0.0, 0.0);
        for (i, &z) in self.h.iter().enumerate() {
            let (x, y) = ((i % w) as f64 - cx, (i / w) as f64 - cy);
            sx += x * z;
            sy += y * z;
            sz += z;
        }
        // Centred coordinates sum to zero, so the normal equations decouple.
        let sxx = (0..w).map(|x| (x as f64 - cx).powi(2)).sum::<f64>() * h as f64;
        let syy = (0..h).map(|y| (y as f64 - cy).powi(2)).sum::<f64>() * w as f64;
        let (a, b, c) = (sx / sxx, sy / syy, sz / (w * h) as f64);
        for (i, z) in self.h.iter_mut().enumerate() {
            let (x, y) = ((i % w) as f64 - cx, (i / w) as f64 - cy);
            *z -= a * x + b * y + c;
        }
    }

    /// Smooth with standard deviation `sigma_m`, e.g. to match the effective
    /// resolution of stereo DTMs.
    pub fn smooth_in_place(&mut self, sigma_m: f64) {
        if sigma_m > 0.0 {
            self.h = self.smooth(&self.h, sigma_m);
            self.protocol.smooth_m = sigma_m;
        }
    }

    fn load_bundle(dir: &Path) -> Result<Self> {
        let meta: Value = serde_json::from_slice(&fs::read(dir.join("bundle.json"))?)?;
        let n = meta["resolution"].as_u64().context("bundle resolution")? as usize;
        let spacing = meta["spacing_m"].as_f64().context("bundle spacing")?;
        let offset = meta["height"]["offset_m"]
            .as_f64()
            .context("height offset")?;
        let range = meta["height"]["range_m"].as_f64().context("height range")?;
        let bytes = fs::read(dir.join("height.r16"))?;
        ensure!(bytes.len() == n * n * 2, "height.r16 size mismatch");
        let h = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&p| offset + f64::from(u16::from_le_bytes(p)) / 65535.0 * range)
            .collect();
        Ok(Self {
            width: n,
            height: n,
            cell_m: spacing,
            periodic: meta["wrap"].as_str() == Some("periodic"),
            h,
            protocol: Protocol::default(),
        })
    }

    fn idx(&self, x: i64, y: i64) -> Option<usize> {
        let (w, h) = (self.width as i64, self.height as i64);
        if self.periodic {
            Some((y.rem_euclid(h) * w + x.rem_euclid(w)) as usize)
        } else if x < 0 || y < 0 || x >= w || y >= h {
            None
        } else {
            Some((y * w + x) as usize)
        }
    }

    /// Index without wrapping (None outside the grid).
    fn idx_bounded(&self, x: i64, y: i64) -> Option<usize> {
        let (w, h) = (self.width as i64, self.height as i64);
        (x >= 0 && y >= 0 && x < w && y < h).then(|| (y * w + x) as usize)
    }

    /// Value at (x, y), clamped at borders for non-periodic grids.
    fn at(&self, v: &[f64], x: i64, y: i64) -> f64 {
        let (w, h) = (self.width as i64, self.height as i64);
        let i = if self.periodic {
            (y.rem_euclid(h) * w + x.rem_euclid(w)) as usize
        } else {
            (y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize
        };
        v[i]
    }

    /// Central-difference gradient (metres per metre).
    fn gradient(&self, v: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let (w, h) = (self.width, self.height);
        let mut gx = vec![0.0; w * h];
        let mut gy = vec![0.0; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let i = y as usize * w + x as usize;
                gx[i] = (self.at(v, x + 1, y) - self.at(v, x - 1, y)) / (2.0 * self.cell_m);
                gy[i] = (self.at(v, x, y + 1) - self.at(v, x, y - 1)) / (2.0 * self.cell_m);
            }
        }
        (gx, gy)
    }

    /// Laplacian (1/m): positive in valleys (concave), negative on ridges.
    fn laplacian(&self, v: &[f64]) -> Vec<f64> {
        let (w, h) = (self.width, self.height);
        let mut out = vec![0.0; w * h];
        let c2 = self.cell_m * self.cell_m;
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                out[y as usize * w + x as usize] = (self.at(v, x + 1, y)
                    + self.at(v, x - 1, y)
                    + self.at(v, x, y + 1)
                    + self.at(v, x, y - 1)
                    - 4.0 * self.at(v, x, y))
                    / c2;
            }
        }
        out
    }

    /// Separable box mean of radius r (sliding window; wraps or clamps).
    fn box_blur(&self, v: &[f64], r: usize) -> Vec<f64> {
        if r == 0 {
            return v.to_vec();
        }
        let (w, h) = (self.width, self.height);
        let ri = r as i64;
        let norm = 1.0 / (2 * r + 1) as f64;
        let mut tmp = vec![0.0; w * h];
        for y in 0..h as i64 {
            let mut sum: f64 = (-ri..=ri).map(|k| self.at(v, k, y)).sum();
            for x in 0..w as i64 {
                tmp[y as usize * w + x as usize] = sum * norm;
                sum += self.at(v, x + ri + 1, y) - self.at(v, x - ri, y);
            }
        }
        let mut out = vec![0.0; w * h];
        for x in 0..w as i64 {
            let mut sum: f64 = (-ri..=ri).map(|k| self.at(&tmp, x, k)).sum();
            for y in 0..h as i64 {
                out[y as usize * w + x as usize] = sum * norm;
                sum += self.at(&tmp, x, y + ri + 1) - self.at(&tmp, x, y - ri);
            }
        }
        out
    }

    /// Gaussian-like smoothing with standard deviation `sigma_m` (3 box passes).
    fn smooth(&self, v: &[f64], sigma_m: f64) -> Vec<f64> {
        let sigma = sigma_m / self.cell_m;
        let r = ((sigma * sigma + 1.0).sqrt() - 1.0).round().max(0.0) as usize;
        let r = r.min(self.width.min(self.height) / 4);
        self.box_blur(&self.box_blur(&self.box_blur(v, r), r), r)
    }

    /// Cells used for statistics: everything for periodic grids, otherwise
    /// excluding a border of `margin` cells.
    fn valid(&self, margin: usize) -> Vec<bool> {
        let (w, h) = (self.width, self.height);
        (0..w * h)
            .map(|i| {
                self.periodic || {
                    let (x, y) = (i % w, i / w);
                    x >= margin && y >= margin && x + margin < w && y + margin < h
                }
            })
            .collect()
    }
}

fn extrema(v: &[f64]) -> (f64, f64) {
    v.iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| {
            (a.min(x), b.max(x))
        })
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Fixed-edge normalized histogram, so histograms are comparable across inputs.
#[derive(Serialize)]
struct Histogram {
    lo: f64,
    hi: f64,
    counts: Vec<f64>,
}

fn histogram(values: impl Iterator<Item = f64>, lo: f64, hi: f64, bins: usize) -> Histogram {
    let mut counts = vec![0.0; bins];
    let mut total = 0.0;
    for v in values {
        let t = ((v - lo) / (hi - lo) * bins as f64).floor();
        let b = (t.max(0.0) as usize).min(bins - 1);
        counts[b] += 1.0;
        total += 1.0;
    }
    if total > 0.0 {
        for c in &mut counts {
            *c /= total;
        }
    }
    Histogram { lo, hi, counts }
}

struct Key(f64, u32);
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Key {}
impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Key {
    // Min-heap on height, ties by index.
    fn cmp(&self, other: &Self) -> Ordering {
        other.0.total_cmp(&self.0).then(other.1.cmp(&self.1))
    }
}

const NEIGHBOURS: [(i64, i64, f64); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, std::f64::consts::SQRT_2),
    (1, -1, std::f64::consts::SQRT_2),
    (-1, 1, std::f64::consts::SQRT_2),
    (1, 1, std::f64::consts::SQRT_2),
];

/// Priority-flood with an epsilon gradient, draining to the grid border (also
/// for periodic tiles: a periodic tile has no outlet, so every crater would be
/// part of one closed basin). Depressions touching the border are dropped later.
fn fill(grid: &Grid) -> Vec<f64> {
    let (w, h) = (grid.width, grid.height);
    let eps = 1.0e-5 * grid.cell_m;
    let mut f = grid.h.clone();
    let mut closed = vec![false; w * h];
    let mut heap = BinaryHeap::new();
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
            closed[i] = true;
            heap.push(Key(f[i], i as u32));
        }
    }
    while let Some(Key(level, index)) = heap.pop() {
        let i = index as usize;
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        for &(dx, dy, _) in &NEIGHBOURS {
            if let Some(j) = grid.idx_bounded(x + dx, y + dy)
                && !closed[j]
            {
                closed[j] = true;
                if f[j] < level + eps {
                    f[j] = level + eps;
                }
                heap.push(Key(f[j], j as u32));
            }
        }
    }
    f
}

/// MFD (Freeman, exponent 1.1) drainage area in m² over a filled surface.
fn drainage(grid: &Grid, filled: &[f64]) -> Vec<f64> {
    let (w, h) = (grid.width, grid.height);
    let mut order: Vec<u32> = (0..(w * h) as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        filled[b as usize]
            .total_cmp(&filled[a as usize])
            .then(a.cmp(&b))
    });
    let cell_area = grid.cell_m * grid.cell_m;
    let mut area = vec![cell_area; w * h];
    let mut targets = [(0usize, 0.0f64); 8];
    for &c in &order {
        let i = c as usize;
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let mut count = 0;
        let mut total = 0.0;
        for &(dx, dy, d) in &NEIGHBOURS {
            if let Some(j) = grid.idx_bounded(x + dx, y + dy) {
                let drop = filled[i] - filled[j];
                if drop > 0.0 {
                    let wgt = (drop / d).powf(1.1);
                    targets[count] = (j, wgt);
                    count += 1;
                    total += wgt;
                }
            }
        }
        if total > 0.0 {
            let share = area[i] / total;
            for &(j, wgt) in &targets[..count] {
                area[j] += share * wgt;
            }
        }
    }
    area
}

/// 8-connected components of `mask`; returns per-component cell lists.
fn components(grid: &Grid, mask: &[bool]) -> Vec<Vec<usize>> {
    let (w, h) = (grid.width, grid.height);
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut cells = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            cells.push(i);
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            for &(dx, dy, _) in &NEIGHBOURS {
                if let Some(j) = grid.idx(x + dx, y + dy)
                    && mask[j]
                    && !seen[j]
                {
                    seen[j] = true;
                    stack.push(j);
                }
            }
        }
        out.push(cells);
    }
    out
}

/// Least-squares slope of y over x.
fn fit_slope(points: &[(f64, f64)]) -> Option<f64> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), p| (a + p.0, b + p.1));
    let (mx, my) = (sx / n, sy / n);
    let (num, den) = points.iter().fold((0.0, 0.0), |(a, b), p| {
        (a + (p.0 - mx) * (p.1 - my), b + (p.0 - mx) * (p.0 - mx))
    });
    (den > 0.0).then(|| num / den)
}

/// Everything computed once and shared by metrics and panels.
struct Analysis {
    gx: Vec<f64>,
    gy: Vec<f64>,
    slope: Vec<f64>,
    curvature16: Vec<f64>,
    area: Vec<f64>,
    depression: Vec<f64>,
    valid: Vec<bool>,
}

fn analyse(grid: &Grid) -> Analysis {
    let (gx, gy) = grid.gradient(&grid.h);
    let slope: Vec<f64> = gx.iter().zip(&gy).map(|(a, b)| a.hypot(*b)).collect();
    let curvature16 = grid.laplacian(&grid.smooth(&grid.h, 8.0));
    let filled = fill(grid);
    let area = drainage(grid, &filled);
    let depression = filled.iter().zip(&grid.h).map(|(f, h)| f - h).collect();
    let margin = ((40.0 / grid.cell_m).ceil() as usize).max(4);
    Analysis {
        gx,
        gy,
        slope,
        curvature16,
        area,
        depression,
        valid: grid.valid(margin),
    }
}

/// Drainage area with cells inside filled closed depressions (deeper than
/// 5 cm) replaced by −1, for the drainage panel.
fn basin_masked(a: &Analysis) -> Vec<f64> {
    a.area
        .iter()
        .zip(&a.depression)
        .map(|(&area, &d)| if d > 0.05 { -1.0 } else { area })
        .collect()
}

/// Organisation metrics as JSON (stable keys; see `score.rs`).
fn metrics(grid: &Grid, a: &Analysis, label: &str) -> Value {
    let (w, h) = (grid.width, grid.height);
    let cell = grid.cell_m;
    let valid_idx: Vec<usize> = (0..w * h).filter(|&i| a.valid[i]).collect();
    let km2 = valid_idx.len() as f64 * cell * cell / 1.0e6;
    let (lo, hi) = extrema(&valid_idx.iter().map(|&i| grid.h[i]).collect::<Vec<_>>());
    let relief = (hi - lo).max(1e-9);

    // Slope.
    let mut slopes: Vec<f64> = valid_idx.iter().map(|&i| a.slope[i]).collect();
    slopes.sort_by(f64::total_cmp);
    let slope_deg = histogram(
        valid_idx.iter().map(|&i| a.slope[i].atan().to_degrees()),
        0.0,
        70.0,
        35,
    );

    // Hypsometry.
    let heights: Vec<f64> = valid_idx
        .iter()
        .map(|&i| (grid.h[i] - lo) / relief)
        .collect();
    let hypsometric_integral = heights.iter().sum::<f64>() / heights.len().max(1) as f64;
    let hypsometry = histogram(heights.iter().copied(), 0.0, 1.0, 20);

    // Curvature at native scale (sharpness) and at 16 m (structure).
    let lap = grid.laplacian(&grid.h);
    let native: Vec<f64> = valid_idx.iter().map(|&i| lap[i] * cell).collect();
    let mean = native.iter().sum::<f64>() / native.len().max(1) as f64;
    let var = native.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / native.len().max(1) as f64;
    let kurtosis = native.iter().map(|v| (v - mean).powi(4)).sum::<f64>()
        / native.len().max(1) as f64
        / (var * var).max(1e-30);
    let mut c16: Vec<f64> = valid_idx.iter().map(|&i| a.curvature16[i].abs()).collect();
    c16.sort_by(f64::total_cmp);
    let c16_scale = percentile(&c16, 0.5).max(1e-12);
    let curvature16 = histogram(
        valid_idx
            .iter()
            .map(|&i| a.curvature16[i] / (4.0 * c16_scale)),
        -2.0,
        2.0,
        40,
    );

    // Grid bias and anisotropy from slope-weighted gradient orientation.
    let mut rose = vec![0.0; 72];
    let (mut c2, mut s2, mut wsum) = (0.0, 0.0, 0.0);
    for &i in &valid_idx {
        let m = a.slope[i];
        if m < 1e-6 {
            continue;
        }
        let theta = a.gy[i].atan2(a.gx[i]).rem_euclid(std::f64::consts::PI);
        let b = (((theta.to_degrees() + 1.25) / 2.5).floor() as usize) % 72;
        rose[b] += m;
        c2 += m * (2.0 * theta).cos();
        s2 += m * (2.0 * theta).sin();
        wsum += m;
    }
    let rose_total: f64 = rose.iter().sum::<f64>().max(1e-30);
    for r in &mut rose {
        *r /= rose_total;
    }
    let axis_bins = [0usize, 18, 36, 54];
    let grid_bias = axis_bins.iter().map(|&b| rose[b]).sum::<f64>() / (4.0 / 72.0);
    let anisotropy = c2.hypot(s2) / wsum.max(1e-30);

    // Flow alignment: fine texture (8 m high-pass) lines should run downslope,
    // so its gradient should be perpendicular to the 30 m downslope direction.
    let coarse = grid.smooth(&grid.h, 30.0);
    let (cgx, cgy) = grid.gradient(&coarse);
    let fine: Vec<f64> = grid
        .h
        .iter()
        .zip(grid.smooth(&grid.h, 8.0))
        .map(|(a, b)| a - b)
        .collect();
    let (fgx, fgy) = grid.gradient(&fine);
    let (mut across, mut total) = (0.0, 0.0);
    for &i in &valid_idx {
        let m = cgx[i].hypot(cgy[i]);
        if m < 0.05 {
            continue;
        }
        let (tx, ty) = (-cgy[i] / m, cgx[i] / m);
        let dot = fgx[i] * tx + fgy[i] * ty;
        across += dot * dot;
        total += fgx[i] * fgx[i] + fgy[i] * fgy[i];
    }
    let flow_alignment = if total > 0.0 { across / total } else { 0.5 };

    // Drainage area exceedance P(A ≥ a) ∝ a^−τ and slope–area concavity.
    let mut areas: Vec<f64> = valid_idx.iter().map(|&i| a.area[i]).collect();
    areas.sort_by(f64::total_cmp);
    let a_lo = 20.0 * cell * cell;
    let a_hi = 0.01 * km2 * 1.0e6;
    let mut exceed = Vec::new();
    let mut a_probe = a_lo;
    while a_probe < a_hi {
        let above = areas.len() - areas.partition_point(|&v| v < a_probe);
        if above > 0 {
            exceed.push((a_probe.ln(), (above as f64 / areas.len() as f64).ln()));
        }
        a_probe *= 1.5;
    }
    let area_exponent = fit_slope(&exceed).map(|s| -s);
    let channel_a = 2.0e4_f64.max(200.0 * cell * cell);
    let mut sa_bins: Vec<Vec<f64>> = vec![Vec::new(); 24];
    for &i in &valid_idx {
        if a.area[i] >= channel_a && a.slope[i] > 1e-4 {
            let b = ((a.area[i] / channel_a).log2() * 2.0) as usize;
            if b < sa_bins.len() {
                sa_bins[b].push(a.slope[i].ln());
            }
        }
    }
    let sa_points: Vec<(f64, f64)> = sa_bins
        .iter_mut()
        .enumerate()
        .filter(|(_, v)| v.len() >= 20)
        .map(|(b, v)| {
            v.sort_by(f64::total_cmp);
            (
                (channel_a * 2f64.powf(b as f64 / 2.0 + 0.25)).ln(),
                percentile(v, 0.5),
            )
        })
        .collect();
    let concavity = fit_slope(&sa_points).map(|s| -s);
    let channel_fraction = valid_idx
        .iter()
        .filter(|&&i| a.area[i] >= channel_a)
        .count() as f64
        / valid_idx.len().max(1) as f64;

    // Closed depressions (craters, pits).
    let depth_min = (0.002 * relief).max(0.3);
    let mask: Vec<bool> = (0..w * h)
        .map(|i| a.valid[i] && a.depression[i] > depth_min)
        .collect();
    let mut dep_d = Vec::new();
    let mut dep_ratio = Vec::new();
    for comp in components(grid, &mask) {
        let touches_border = comp.iter().any(|&i| {
            let (x, y) = (i % w, i / w);
            x == 0 || y == 0 || x == w - 1 || y == h - 1
        });
        if comp.len() < 4 || touches_border {
            continue;
        }
        let area_m2 = comp.len() as f64 * cell * cell;
        let diameter = 2.0 * (area_m2 / std::f64::consts::PI).sqrt();
        let depth = comp.iter().map(|&i| a.depression[i]).fold(0.0, f64::max);
        dep_d.push(diameter);
        dep_ratio.push(depth / diameter);
    }
    let size_edges: Vec<f64> = (0..12).map(|k| 4.0 * cell * 2f64.powi(k)).collect();
    let dep_density: Vec<f64> = size_edges
        .iter()
        .map(|&e| dep_d.iter().filter(|&&d| d >= e).count() as f64 / km2.max(1e-9))
        .collect();
    let mut ratios = dep_ratio.clone();
    ratios.sort_by(f64::total_cmp);
    let pits = (0..w * h)
        .filter(|&i| a.valid[i])
        .filter(|&i| {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            NEIGHBOURS.iter().all(|&(dx, dy, _)| {
                grid.idx(x + dx, y + dy)
                    .is_none_or(|j| grid.h[j] > grid.h[i])
            })
        })
        .count() as f64
        / km2.max(1e-9);

    // Ridge / valley continuity at 16 m: area-weighted mean component area.
    let t = 2.0 * c16_scale;
    let continuity = |sign: f64| {
        let mask: Vec<bool> = (0..w * h)
            .map(|i| a.valid[i] && sign * a.curvature16[i] > t && a.slope[i] > 0.05)
            .collect();
        let comps = components(grid, &mask);
        let total: f64 = comps.iter().map(|c| c.len() as f64).sum();
        let weighted: f64 = comps.iter().map(|c| (c.len() as f64).powi(2)).sum();
        if total > 0.0 {
            weighted / total * cell * cell
        } else {
            0.0
        }
    };
    let ridge_continuity_m2 = continuity(-1.0);
    let valley_continuity_m2 = continuity(1.0);

    // Spatial heterogeneity: variation of local relief and slope at 256 m.
    let win = ((128.0 / cell).round() as usize).max(2);
    let m1 = grid.box_blur(&grid.h, win);
    let sq: Vec<f64> = grid.h.iter().map(|v| v * v).collect();
    let m2 = grid.box_blur(&sq, win);
    let local_sd: Vec<f64> = valid_idx
        .iter()
        .map(|&i| (m2[i] - m1[i] * m1[i]).max(0.0).sqrt())
        .collect();
    let cv = |v: &[f64]| {
        let m = v.iter().sum::<f64>() / v.len().max(1) as f64;
        let s = (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt();
        if m > 0.0 { s / m } else { 0.0 }
    };
    let local_slope = grid.box_blur(&a.slope, win);
    let slope_local: Vec<f64> = valid_idx.iter().map(|&i| local_slope[i]).collect();

    // Band RMS (difference of smoothings), metres.
    let mut bands = Vec::new();
    let mut prev = grid.h.clone();
    let mut sigma = 2.0 * cell;
    while sigma <= (w.min(h) as f64) * cell / 8.0 {
        let next = grid.smooth(&grid.h, sigma);
        let ms = valid_idx
            .iter()
            .map(|&i| (prev[i] - next[i]).powi(2))
            .sum::<f64>()
            / valid_idx.len().max(1) as f64;
        bands.push(json!([sigma, ms.sqrt()]));
        prev = next;
        sigma *= 2.0;
    }

    json!({
        "schema": "mundaris.terrain-inspect.v1",
        "label": label,
        "width": w, "height": h, "cell_m": cell, "periodic": grid.periodic,
        "protocol": grid.protocol,
        "area_km2": km2,
        "relief_m": relief,
        "scalars": {
            "slope_p10": percentile(&slopes, 0.1),
            "slope_p50": percentile(&slopes, 0.5),
            "slope_p90": percentile(&slopes, 0.9),
            "slope_p99": percentile(&slopes, 0.99),
            "hypsometric_integral": hypsometric_integral,
            "curvature_kurtosis": kurtosis,
            "grid_bias": grid_bias,
            "anisotropy": anisotropy,
            "flow_alignment": flow_alignment,
            "area_exponent": area_exponent,
            "concavity": concavity,
            "channel_fraction": channel_fraction,
            "depressions_per_km2": dep_d.len() as f64 / km2.max(1e-9),
            "depression_depth_ratio_p50": percentile(&ratios, 0.5),
            "depression_depth_ratio_p90": percentile(&ratios, 0.9),
            "pits_per_km2": pits,
            "ridge_continuity_m2": ridge_continuity_m2,
            "valley_continuity_m2": valley_continuity_m2,
            "relief_heterogeneity": cv(&local_sd),
            "slope_heterogeneity": cv(&slope_local),
        },
        "histograms": {
            "slope_deg": slope_deg,
            "hypsometry": hypsometry,
            "curvature16": curvature16,
            "orientation": Histogram { lo: 0.0, hi: 180.0, counts: rose.clone() },
        },
        "depression_cumulative_per_km2": size_edges.iter().zip(&dep_density).map(|(e, d)| json!([e, d])).collect::<Vec<_>>(),
        "band_rms_m": bands,
    })
}

// ---------- rendering ----------

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Image {
    pub fn new(w: usize, h: usize, fill: [u8; 3]) -> Self {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for _ in 0..w * h {
            rgb.extend_from_slice(&fill);
        }
        Self { w, h, rgb }
    }

    pub fn set(&mut self, x: usize, y: usize, c: [u8; 3]) {
        if x < self.w && y < self.h {
            let i = (y * self.w + x) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn get(&self, x: usize, y: usize) -> [u8; 3] {
        let i = (y * self.w + x) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    pub fn blit(&mut self, src: &Image, ox: usize, oy: usize) {
        for y in 0..src.h {
            for x in 0..src.w {
                self.set(ox + x, oy + y, src.get(x, y));
            }
        }
    }

    /// Text in a 5×7 bitmap font at integer `scale`.
    pub fn text(&mut self, x: usize, y: usize, s: &str, scale: usize, c: [u8; 3]) {
        for (k, ch) in s.chars().enumerate() {
            let glyph = font::glyph(ch);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits >> (4 - col) & 1 == 1 {
                        for sy in 0..scale {
                            for sx in 0..scale {
                                self.set(x + (k * 6 + col) * scale + sx, y + row * scale + sy, c);
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let file =
            fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
        let mut enc =
            png::Encoder::new(std::io::BufWriter::new(file), self.w as u32, self.h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&self.rgb)?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let decoder = png::Decoder::new(std::io::BufReader::new(fs::File::open(path)?));
        let mut reader = decoder.read_info()?;
        let mut buf = vec![0; reader.output_buffer_size().context("png size")?];
        let info = reader.next_frame(&mut buf)?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgb = match info.color_type {
            png::ColorType::Rgb => buf[..w * h * 3].to_vec(),
            png::ColorType::Grayscale => buf[..w * h].iter().flat_map(|&g| [g, g, g]).collect(),
            png::ColorType::Rgba => buf[..w * h * 4]
                .chunks(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
            other => bail!("unsupported png colour type {other:?}"),
        };
        Ok(Self { w, h, rgb })
    }
}

fn lerp_color(stops: &[[u8; 3]], t: f64) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (t.floor() as usize).min(stops.len() - 2);
    let f = t - i as f64;
    let (a, b) = (stops[i], stops[i + 1]);
    [0, 1, 2].map(|k| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f).round() as u8)
}

/// Render a scalar field (box-downsampled to at most `size` px) through `color`.
fn panel(grid: &Grid, v: &[f64], size: usize, color: impl Fn(f64) -> [u8; 3]) -> Image {
    let factor = grid.width.max(grid.height).div_ceil(size).max(1);
    let (pw, ph) = (grid.width / factor, grid.height / factor);
    let mut img = Image::new(pw, ph, [0, 0, 0]);
    for py in 0..ph {
        for px in 0..pw {
            let mut s = 0.0;
            for y in 0..factor {
                for x in 0..factor {
                    s += v[(py * factor + y) * grid.width + px * factor + x];
                }
            }
            img.set(px, py, color(s / (factor * factor) as f64));
        }
    }
    img
}

fn crop(
    grid: &Grid,
    v: &[f64],
    x0: usize,
    y0: usize,
    size: usize,
    color: impl Fn(f64) -> [u8; 3],
) -> Image {
    let mut img = Image::new(size, size, [0, 0, 0]);
    for y in 0..size.min(grid.height - y0) {
        for x in 0..size.min(grid.width - x0) {
            img.set(x, y, color(v[(y0 + y) * grid.width + x0 + x]));
        }
    }
    img
}

fn hillshade(a: &Analysis, light: [f64; 2]) -> Vec<f64> {
    let lz = std::f64::consts::FRAC_1_SQRT_2;
    let (lx, ly) = (light[0] * lz, light[1] * lz);
    a.gx.iter()
        .zip(&a.gy)
        .map(|(gx, gy)| {
            ((-gx * lx - gy * ly + lz) / (gx * gx + gy * gy + 1.0).sqrt()).clamp(0.0, 1.0)
        })
        .collect()
}

fn grey(v: f64) -> [u8; 3] {
    let g = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [g, g, g]
}

const DIVERGING: [[u8; 3]; 5] = [
    [33, 102, 172],
    [146, 197, 222],
    [247, 247, 247],
    [244, 165, 130],
    [178, 24, 43],
];
const SEQUENTIAL: [[u8; 3]; 5] = [
    [20, 20, 60],
    [40, 80, 160],
    [40, 170, 170],
    [180, 220, 80],
    [255, 250, 180],
];

fn rose_image(rose: &[f64], size: usize) -> Image {
    let mut img = Image::new(size, size, [24, 24, 28]);
    let c = size as f64 / 2.0;
    let max = rose.iter().copied().fold(1e-30, f64::max);
    let uniform = 1.0 / rose.len() as f64;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f64 - c, c - y as f64);
            let r = dx.hypot(dy) / (c * 0.95);
            let theta = dy.atan2(dx).rem_euclid(std::f64::consts::PI);
            let b = (((theta.to_degrees() + 1.25) / 2.5).floor() as usize) % rose.len();
            let ring = (r - uniform / max).abs() < 0.008;
            if r <= rose[b] / max {
                img.set(x, y, [230, 160, 60]);
            } else if ring {
                img.set(x, y, [120, 120, 130]);
            }
        }
    }
    img
}

/// Inspect `grid` and write panels, sheets and `metrics.json` into `out`.
pub fn inspect(
    grid: &Grid,
    label: &str,
    out: &Path,
    crop_at: Option<(usize, usize)>,
) -> Result<Value> {
    fs::create_dir_all(out.join("panels"))?;
    let a = analyse(grid);
    let m = metrics(grid, &a, label);
    fs::write(out.join("metrics.json"), serde_json::to_string_pretty(&m)?)?;

    let size = 512;
    let (lo, hi) = extrema(&grid.h);
    let relief = (hi - lo).max(1e-9);
    let nw = hillshade(&a, [-1.0, -1.0]);
    let ne = hillshade(&a, [1.0, -1.0]);
    let scale16 = {
        let mut v: Vec<f64> = a.curvature16.iter().map(|c| c.abs()).collect();
        v.sort_by(f64::total_cmp);
        percentile(&v, 0.9).max(1e-12)
    };
    let area_max = a.area.iter().copied().fold(1.0, f64::max).ln();
    let area_min = (grid.cell_m * grid.cell_m).ln();
    let dep_scale = a.depression.iter().copied().fold(1e-9, f64::max);
    let panels: Vec<(&str, &str, Image)> = vec![
        (
            "HEIGHT",
            "height",
            panel(grid, &grid.h, size, |v| grey((v - lo) / relief)),
        ),
        ("HILLSHADE NW", "hillshade", panel(grid, &nw, size, grey)),
        ("HILLSHADE NE", "hillshade_ne", panel(grid, &ne, size, grey)),
        (
            "SLOPE 0-60 DEG",
            "slope",
            panel(grid, &a.slope, size, |s| grey(s.atan().to_degrees() / 60.0)),
        ),
        (
            "CURVATURE RIDGE RED VALLEY BLUE",
            "curvature",
            panel(grid, &a.curvature16, size, |c| {
                lerp_color(&DIVERGING, 0.5 - 0.5 * (c / scale16))
            }),
        ),
        (
            "DRAINAGE LOG (GREY = FILLED BASIN)",
            "drainage",
            // Inside filled closed depressions the routing over the fill's
            // epsilon gradient draws straight fans that are not terrain, so
            // those cells are greyed out (negative marker) instead of coloured.
            panel(grid, &basin_masked(&a), size, |v| {
                if v < 0.0 {
                    [70, 70, 78]
                } else {
                    lerp_color(&SEQUENTIAL, (v.ln() - area_min) / (area_max - area_min))
                }
            }),
        ),
        (
            "DEPRESSIONS (CRATERS PITS)",
            "depressions",
            panel(grid, &a.depression, size, |d| {
                lerp_color(&SEQUENTIAL, (d / dep_scale).sqrt())
            }),
        ),
        (
            "ORIENTATION ROSE",
            "rose",
            rose_image(
                m["histograms"]["orientation"]["counts"]
                    .as_array()
                    .map(|v| v.iter().filter_map(Value::as_f64).collect::<Vec<_>>())
                    .unwrap_or_default()
                    .as_slice(),
                size.min(512),
            ),
        ),
    ];
    for (_, file, img) in &panels {
        img.save(&out.join("panels").join(format!("{file}.png")))?;
    }
    let panels: Vec<(&str, Image)> = panels.into_iter().map(|(n, _, i)| (n, i)).collect();
    // Close-ups at 1:1 (one source cell per pixel).
    let cs = 512.min(grid.width).min(grid.height);
    let (cx, cy) = crop_at.unwrap_or(((grid.width - cs) / 2, (grid.height - cs) / 2));
    let (cx, cy) = (cx.min(grid.width - cs), cy.min(grid.height - cs));
    let crops: Vec<(&str, Image)> = vec![
        ("1:1 HILLSHADE NW", crop(grid, &nw, cx, cy, cs, grey)),
        ("1:1 HILLSHADE NE", crop(grid, &ne, cx, cy, cs, grey)),
        (
            "1:1 SLOPE",
            crop(grid, &a.slope, cx, cy, cs, |s| {
                grey(s.atan().to_degrees() / 60.0)
            }),
        ),
        (
            "1:1 CURVATURE",
            crop(grid, &a.curvature16, cx, cy, cs, |c| {
                lerp_color(&DIVERGING, 0.5 - 0.5 * (c / scale16))
            }),
        ),
    ];
    crops[0]
        .1
        .save(&out.join("panels").join("crop_hillshade_nw.png"))?;
    crops[2]
        .1
        .save(&out.join("panels").join("crop_slope.png"))?;

    let title = format!(
        "{} | {}X{} CELLS {:.2} M | {:.0} M RELIEF | {}",
        label.to_uppercase(),
        grid.width,
        grid.height,
        grid.cell_m,
        relief,
        if grid.periodic {
            "PERIODIC"
        } else {
            "NON-PERIODIC"
        }
    );
    let sheet = |items: &[(&str, Image)], cols: usize, subtitle: &str| {
        let tile = items.iter().map(|(_, i)| i.w.max(i.h)).max().unwrap_or(512);
        let rows = items.len().div_ceil(cols);
        let label_h = 22;
        let mut img = Image::new(
            cols * (tile + 6) + 6,
            30 + rows * (tile + label_h + 6),
            [24, 24, 28],
        );
        img.text(8, 8, &format!("{title} | {subtitle}"), 2, [255, 255, 255]);
        for (k, (name, p)) in items.iter().enumerate() {
            let (x, y) = (
                6 + (k % cols) * (tile + 6),
                30 + (k / cols) * (tile + label_h + 6),
            );
            img.text(x + 2, y + 4, name, 2, [220, 220, 220]);
            img.blit(p, x, y + label_h);
        }
        img
    };
    sheet(&panels, 4, "OVERVIEW").save(&out.join("overview.png"))?;
    sheet(&crops, 4, &format!("CROP AT {cx},{cy}")).save(&out.join("closeup.png"))?;
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(n: usize, f: impl Fn(f64, f64) -> f64) -> Grid {
        Grid {
            width: n,
            height: n,
            cell_m: 2.0,
            periodic: true,
            h: (0..n * n)
                .map(|i| f((i % n) as f64, (i / n) as f64))
                .collect(),
            protocol: Protocol::default(),
        }
    }

    #[test]
    fn axis_aligned_ridges_show_grid_bias_and_anisotropy() {
        let tau = std::f64::consts::TAU;
        let n = 128;
        let lines = grid(n, |x, _| 20.0 * (tau * 8.0 * x / n as f64).sin());
        let a = analyse(&lines);
        let m = metrics(&lines, &a, "lines");
        assert!(m["scalars"]["grid_bias"].as_f64().unwrap() > 5.0);
        assert!(m["scalars"]["anisotropy"].as_f64().unwrap() > 0.9);
        // Three equal waves 60° apart (integer wave vectors keep it periodic).
        let blobs = grid(n, |x, y| {
            let t = tau / n as f64;
            10.0 * ((6.0 * x * t).sin()
                + ((3.0 * x + 5.0 * y) * t + 1.0).sin()
                + ((-3.0 * x + 5.0 * y) * t + 2.0).sin())
        });
        let a = analyse(&blobs);
        let m = metrics(&blobs, &a, "blobs");
        assert!(m["scalars"]["anisotropy"].as_f64().unwrap() < 0.7);
    }

    #[test]
    fn a_bowl_is_one_depression_with_its_depth() {
        let n = 128;
        let c = 64.0;
        let bowl = grid(n, |x, y| {
            let r = (x - c).hypot(y - c);
            if r < 20.0 {
                -10.0 * (1.0 - (r / 20.0).powi(2))
            } else {
                0.0
            }
        });
        let a = analyse(&bowl);
        let m = metrics(&bowl, &a, "bowl");
        let count =
            m["scalars"]["depressions_per_km2"].as_f64().unwrap() * m["area_km2"].as_f64().unwrap();
        assert!((count - 1.0).abs() < 1e-6, "count {count}");
        let ratio = m["scalars"]["depression_depth_ratio_p50"].as_f64().unwrap();
        // depth 10 m over an 80 m bowl (radius 20 cells × 2 m).
        assert!((ratio - 0.125).abs() < 0.03, "d/D {ratio}");
    }

    #[test]
    fn detrend_leaves_periodic_tiles_seamless() {
        // A periodic surface whose least-squares plane is far from zero: one
        // sawtooth-free wave per axis, phase-shifted so it correlates with x and y.
        let n = 128;
        let tau = std::f64::consts::TAU;
        let make = || {
            grid(n, |x, y| {
                5.0 * (tau * x / n as f64 + 1.0).sin() + 3.0 * (tau * y / n as f64 + 2.0).sin()
            })
        };
        let (plain, mut detrended) = (make(), make());
        detrended.detrend();
        assert!(!detrended.protocol.plane_removed);
        let shift = plain.h[0] - detrended.h[0];
        for (a, b) in plain.h.iter().zip(&detrended.h) {
            assert!(
                (a - b - shift).abs() < 1e-9,
                "periodic detrend must only remove the mean"
            );
        }
        // The same surface treated as a real (non-periodic) window loses its plane.
        let mut window = make();
        window.periodic = false;
        window.detrend();
        assert!(window.protocol.plane_removed);
    }
}
