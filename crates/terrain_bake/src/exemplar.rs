//! Example-based terrain synthesis: a Laplacian pyramid of exemplar windows cut
//! from public-domain DTMs (NASA LROC NAC), recombined by Efros-Freeman patch
//! quilting (Efros and Freeman 2001) with minimum-error toroidal cuts, feathered
//! seams, coarse-level height offsets and a coarse-shape guide term (after Zhou
//! et al. 2007). The output is periodic by construction (all patch placement and
//! cuts wrap), so it tiles like any other library tile.
//!
//! Ported from experiment E4 (`ai/experiments/heightmap-diagnosis/e4-example-based`).
//! For the same sources, windows, parameters and seed it reproduces E4 exactly.
//!
//! Determinism: every candidate is evaluated independently into its own result
//! slot, and the winner is chosen from the slots in index order with a seeded
//! generator, so the output bytes do not depend on the thread count.
//!
//! Recipe levels are listed finest first: `levels[k]` works at
//! `resolution >> k` and uses the exemplar pyramid level `k` (the exemplar
//! downsampled by `2^k`). Each exemplar window must be divisible by
//! `2^(levels - 1)`; windows may differ in size.

use crate::tiff::Layout;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

/// Credit line published with bundles derived from LROC NAC DTMs.
pub const CREDIT: &str =
    "Elevation data: NASA/GSFC/Arizona State University (LROC NAC DTM), public domain";

/// Largest quilting patch (the guide scratch holds the stride-2 samples).
const MAX_PATCH_PX: u32 = 192;
const MAX_EXEMPLARS: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExemplarRecipe {
    /// Output grid side; equals the stage resolution.
    pub resolution: u32,
    pub sources: Vec<ExemplarSource>,
    /// Finest level first; at most 8.
    pub levels: Vec<ExemplarLevel>,
    /// Weight of the coarse-shape guide term at levels below the coarsest.
    pub alpha: f64,
    /// Weight of the squared height offset at the coarsest level.
    pub lambda: f64,
    /// Seed of the synthesis generator (independent of the recipe seed).
    pub seed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExemplarSource {
    /// Product name, e.g. `APOLLO16` (NASA LROC NAC DTM `NAC_DTM_<product>`).
    pub product: String,
    /// GeoTIFF path. A relative path resolves against `ASTRUM_DEM_ROOT`.
    pub path: String,
    /// Lower-case hex SHA-256 of the file; verified before any use.
    pub sha256: String,
    /// Exemplar windows `[x, y, size]` in raster pixels, each fully valid
    /// (no no-data). Plane-detrended on load.
    pub windows: Vec<[u32; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExemplarLevel {
    pub patch_px: u32,
    pub overlap_px: u32,
    /// Random candidate patches evaluated per placement.
    pub candidates: u32,
    /// Box-blur radius of the seam feathering.
    pub feather_px: u32,
}

impl ExemplarRecipe {
    pub fn validate(&self) -> Result<()> {
        let n = self.resolution;
        ensure!(
            n.is_power_of_two() && (16..=4096).contains(&n),
            "exemplar_synthesis resolution must be a power of two in 16..=4096"
        );
        ensure!(
            (1..=8).contains(&self.levels.len()),
            "exemplar_synthesis needs 1..=8 levels"
        );
        ensure!(
            self.alpha.is_finite()
                && (0.0..=100.0).contains(&self.alpha)
                && self.lambda.is_finite()
                && (0.0..=100.0).contains(&self.lambda),
            "alpha and lambda must be finite in 0..=100"
        );
        let depth = self.levels.len();
        let smallest = n >> (depth - 1);
        ensure!(smallest >= 8, "too many levels for resolution {n}");
        for (k, l) in self.levels.iter().enumerate() {
            let at = |m: &str| format!("level {k}: {m}");
            let side = n >> k;
            ensure!(
                (4..=MAX_PATCH_PX).contains(&l.patch_px) && l.patch_px <= side,
                at("patch_px must be in 4..=192 and fit the level")
            );
            ensure!(
                l.overlap_px >= 2 && l.overlap_px < l.patch_px,
                at("overlap_px must be in 2..patch_px")
            );
            ensure!(
                l.overlap_px > 2 * (l.feather_px + 1),
                at("overlap_px must exceed 2 * (feather_px + 1) so the seam can move")
            );
            let step = l.patch_px - l.overlap_px;
            ensure!(
                side.is_multiple_of(step),
                at("level side must be a multiple of patch_px - overlap_px")
            );
            ensure!(
                (1..=100_000).contains(&l.candidates),
                at("candidates must be in 1..=100000")
            );
            ensure!(l.feather_px <= 32, at("feather_px must be at most 32"));
        }
        ensure!(
            !self.sources.is_empty(),
            "exemplar_synthesis needs at least one source"
        );
        let mut count = 0usize;
        for s in &self.sources {
            ensure!(
                !s.product.is_empty()
                    && s.product
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
                "source product must be [A-Za-z0-9_]+"
            );
            ensure!(
                s.sha256.len() == 64
                    && s.sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "source {}: sha256 must be 64 lower-case hex digits",
                s.product
            );
            ensure!(!s.windows.is_empty(), "source {} has no windows", s.product);
            for w in &s.windows {
                let size = w[2];
                ensure!(
                    size.is_multiple_of(1 << (depth - 1)) && size >> (depth - 1) > 0,
                    "source {}: window size {size} must be a multiple of {}",
                    s.product,
                    1u32 << (depth - 1)
                );
                for (k, l) in self.levels.iter().enumerate() {
                    ensure!(
                        (size >> k) >= l.patch_px,
                        "source {}: window size {size} is smaller than the level {k} patch",
                        s.product
                    );
                }
            }
            count += s.windows.len();
        }
        ensure!(
            count <= MAX_EXEMPLARS,
            "at most {MAX_EXEMPLARS} exemplar windows"
        );
        Ok(())
    }

    /// One provenance line per source for `external_asset_dependencies`.
    pub fn dependencies(&self) -> Vec<String> {
        self.sources
            .iter()
            .map(|s| {
                format!(
                    "NASA LROC NAC DTM {} sha256:{} windows:{}",
                    s.product,
                    s.sha256,
                    s.windows.len()
                )
            })
            .collect()
    }
}

/// Worker threads for the candidate search; `ASTRUM_BAKE_THREADS` overrides.
/// The output does not depend on this.
pub fn thread_count() -> usize {
    std::env::var("ASTRUM_BAKE_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&t| t > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(2, |t| t.get()))
        .clamp(1, 64)
}

// ---------------------------------------------------------------- generator

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

// ---------------------------------------------------------------- resampling

/// Binomial 1-3-3-1 downsample by two (clamped edges).
fn down2(src: &[f32], n: usize) -> Vec<f32> {
    let m = n / 2;
    let cl = |i: i64| i.clamp(0, n as i64 - 1) as usize;
    let k = [1.0f32, 3.0, 3.0, 1.0];
    let mut tmp = vec![0f32; m * n];
    for y in 0..n {
        for i in 0..m {
            let mut a = 0.0;
            for (t, &kw) in k.iter().enumerate() {
                a += kw * src[y * n + cl(2 * i as i64 - 1 + t as i64)];
            }
            tmp[y * m + i] = a / 8.0;
        }
    }
    let mut out = vec![0f32; m * m];
    for j in 0..m {
        for i in 0..m {
            let mut a = 0.0;
            for (t, &kw) in k.iter().enumerate() {
                a += kw * tmp[cl(2 * j as i64 - 1 + t as i64) * m + i];
            }
            out[j * m + i] = a / 8.0;
        }
    }
    out
}

/// Catmull-Rom weights for samples at -1, 0, 1, 2.
fn catmull_rom(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        0.5 * (-t3 + 2.0 * t2 - t),
        0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
        0.5 * (-3.0 * t3 + 4.0 * t2 + t),
        0.5 * (t3 - t2),
    ]
}

/// Catmull-Rom upsample by two; periodic when `wrap`, else clamped.
fn up2(src: &[f32], n: usize, wrap: bool) -> Vec<f32> {
    let m = n * 2;
    let ix = |i: i64| -> usize {
        if wrap {
            i.rem_euclid(n as i64) as usize
        } else {
            i.clamp(0, n as i64 - 1) as usize
        }
    };
    let taps: Vec<(i64, [f32; 4])> = (0..m)
        .map(|j| {
            let u = (j as f32 + 0.5) / 2.0 - 0.5;
            let i0 = u.floor();
            (i0 as i64, catmull_rom(u - i0))
        })
        .collect();
    let mut tmp = vec![0f32; m * n];
    for y in 0..n {
        for (j, (i0, w)) in taps.iter().enumerate() {
            let mut a = 0.0;
            for (t, wt) in w.iter().enumerate() {
                a += wt * src[y * n + ix(i0 - 1 + t as i64)];
            }
            tmp[y * m + j] = a;
        }
    }
    let mut out = vec![0f32; m * m];
    for (j, (i0, w)) in taps.iter().enumerate() {
        for x in 0..m {
            let mut a = 0.0;
            for (t, wt) in w.iter().enumerate() {
                a += wt * tmp[ix(i0 - 1 + t as i64) * m + x];
            }
            out[j * m + x] = a;
        }
    }
    out
}

// ---------------------------------------------------------------- pyramid

/// One exemplar as per-level detail `d[k]` (absolute at the coarsest level) and
/// guide `g[k] = up(E_{k+1})`.
pub struct Pyramid {
    n: Vec<usize>,
    d: Vec<Vec<f32>>,
    g: Vec<Vec<f32>>,
}

/// Build the pyramid of an `size²` exemplar with `levels` levels.
pub fn build_pyramid(e0: Vec<f32>, size: usize, levels: usize) -> Pyramid {
    let mut es = vec![e0];
    let mut ns = vec![size];
    for k in 1..levels {
        let n = ns[k - 1];
        es.push(down2(&es[k - 1], n));
        ns.push(n / 2);
    }
    let mut d = vec![];
    let mut g = vec![];
    for k in 0..levels {
        if k + 1 < levels {
            let up = up2(&es[k + 1], ns[k + 1], false);
            d.push(es[k].iter().zip(&up).map(|(a, b)| a - b).collect());
            g.push(up);
        } else {
            d.push(es[k].clone());
            g.push(vec![]);
        }
    }
    Pyramid { n: ns, d, g }
}

/// Least-squares plane and mean removal on an `n²` grid.
fn detrend_plane(h: &mut [f64], n: usize) {
    let c = (n as f64 - 1.0) / 2.0;
    let (mut sx, mut sy, mut sz, mut sxx, mut syy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &z) in h.iter().enumerate() {
        let (x, y) = ((i % n) as f64 - c, (i / n) as f64 - c);
        sx += x * z;
        sy += y * z;
        sz += z;
        sxx += x * x;
        syy += y * y;
    }
    let (ax, ay, m) = (sx / sxx, sy / syy, sz / h.len() as f64);
    for (i, z) in h.iter_mut().enumerate() {
        let (x, y) = ((i % n) as f64 - c, (i / n) as f64 - c);
        *z -= m + ax * x + ay * y;
    }
}

fn resolve_path(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        return Ok(p);
    }
    let root = std::env::var_os("ASTRUM_DEM_ROOT")
        .with_context(|| format!("relative source path {path} needs ASTRUM_DEM_ROOT"))?;
    Ok(PathBuf::from(root).join(p))
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Read every source, verify its hash, cut and detrend its windows and build
/// the pyramids. Sources are processed one at a time to bound memory.
pub fn load_pyramids(recipe: &ExemplarRecipe) -> Result<Vec<Pyramid>> {
    let depth = recipe.levels.len();
    let mut out = Vec::new();
    for source in &recipe.sources {
        let path = resolve_path(&source.path)?;
        let bytes = std::fs::read(&path).with_context(|| {
            format!("source {}: cannot read {}", source.product, path.display())
        })?;
        let found = sha256_hex(&bytes);
        ensure!(
            found == source.sha256,
            "source {} ({}): sha256 mismatch (recipe {}, file {found})",
            source.product,
            path.display(),
            source.sha256
        );
        let layout = Layout::parse(&bytes)
            .with_context(|| format!("source {} ({})", source.product, path.display()))?;
        for &[x, y, size] in &source.windows {
            let (x, y, size) = (x as usize, y as usize, size as usize);
            let raw = layout
                .window(&bytes, x, y, size, size)
                .with_context(|| format!("source {} window {x},{y},{size}", source.product))?;
            ensure!(
                raw.iter().all(|&v| v.is_finite() && v > -1.0e30),
                "source {} window {x},{y},{size} contains no-data",
                source.product
            );
            let mut win: Vec<f64> = raw.iter().map(|&v| f64::from(v)).collect();
            drop(raw);
            detrend_plane(&mut win, size);
            let e0: Vec<f32> = win.iter().map(|&v| v as f32).collect();
            out.push(build_pyramid(e0, size, depth));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- quilting

#[derive(Clone, Copy)]
struct Cand {
    img: u32,
    sym: u8,
    x0: u32,
    y0: u32,
}

/// Source coordinates of patch pixel (u, v) under one of the 8 dihedral symmetries.
#[inline(always)]
fn map(sym: u8, p: usize, u: usize, v: usize) -> (usize, usize) {
    let (mut a, mut b) = (u, v);
    if sym & 1 != 0 {
        std::mem::swap(&mut a, &mut b);
    }
    if sym & 2 != 0 {
        a = p - 1 - a;
    }
    if sym & 4 != 0 {
        b = p - 1 - b;
    }
    (a, b)
}

/// DP seam through an error strip. `err[line * depth + t]`; returns the cut
/// `c(line)` in `[0, depth)`: pixels with `t >= c` come from the new patch.
fn min_cut(err: &[f32], lines: usize, depth: usize) -> Vec<usize> {
    let mut cost = vec![0f32; lines * depth];
    let mut from = vec![0u8; lines * depth];
    cost[..depth].copy_from_slice(&err[..depth]);
    for l in 1..lines {
        for t in 0..depth {
            let mut best = cost[(l - 1) * depth + t];
            let mut arg = 1u8;
            if t > 0 && cost[(l - 1) * depth + t - 1] < best {
                best = cost[(l - 1) * depth + t - 1];
                arg = 0;
            }
            if t + 1 < depth && cost[(l - 1) * depth + t + 1] < best {
                best = cost[(l - 1) * depth + t + 1];
                arg = 2;
            }
            cost[l * depth + t] = err[l * depth + t] + best;
            from[l * depth + t] = arg;
        }
    }
    let last = (lines - 1) * depth;
    let mut t = (0..depth)
        .min_by(|&a, &b| cost[last + a].total_cmp(&cost[last + b]))
        .unwrap_or(0);
    let mut cut = vec![0; lines];
    for l in (0..lines).rev() {
        cut[l] = t;
        t = (t as i64 + i64::from(from[l * depth + t]) - 1).clamp(0, depth as i64 - 1) as usize;
    }
    cut
}

/// `min_cut` that keeps the seam `m` samples away from both strip edges (room
/// for feathering).
fn cut_margin(err: &[f32], lines: usize, depth: usize, m: usize) -> Vec<usize> {
    let w = depth - 2 * m;
    let mut sub = vec![0f32; lines * w];
    for l in 0..lines {
        for t in 0..w {
            sub[l * w + t] = err[l * depth + m + t];
        }
    }
    min_cut(&sub, lines, w).into_iter().map(|c| c + m).collect()
}

struct LevelParams {
    fr: usize,
    n: usize,
    p: usize,
    o: usize,
    k_cands: usize,
    alpha: f32,
    lambda: f32,
}

struct Stats {
    patches: usize,
    sources: HashSet<(u32, u8, u32, u32)>,
}

/// Quilt one level on an `n²` torus. `guide` is the periodic upsample of the
/// coarser result (absent at the coarsest level).
fn quilt(
    lp: &LevelParams,
    level: usize,
    coarsest: bool,
    exs: &[Pyramid],
    guide: Option<&[f32]>,
    rng: &mut Rng,
    threads: usize,
) -> Result<(Vec<f32>, Stats)> {
    let (n, p, o) = (lp.n, lp.p, lp.o);
    let step = p - o;
    ensure!(
        n.is_multiple_of(step),
        "N {n} not a multiple of step {step}"
    );
    ensure!(p <= MAX_PATCH_PX as usize, "patch too large");
    let np = n / step;
    let mut out = vec![0f32; n * n];
    let mut written = vec![false; n * n];
    for ex in exs {
        ensure!(ex.n[level] >= p, "exemplar smaller than the level patch");
    }
    let mut stats = Stats {
        patches: 0,
        sources: HashSet::new(),
    };
    for i in 0..np {
        for j in 0..np {
            let (px, py) = (j * step, i * step);
            // Already-written pixels under this patch.
            let mut ov: Vec<(u16, u16, f32)> = vec![];
            for v in 0..p {
                let oy = (py + v) % n;
                for u in 0..p {
                    let ox = (px + u) % n;
                    if written[oy * n + ox] {
                        ov.push((u as u16, v as u16, out[oy * n + ox]));
                    }
                }
            }
            // Guide (stride-2 samples, mean removed).
            let mut sloc: Vec<f32> = vec![];
            if let Some(gd) = guide {
                for v in (0..p).step_by(2) {
                    for u in (0..p).step_by(2) {
                        sloc.push(gd[((py + v) % n) * n + (px + u) % n]);
                    }
                }
                let m = sloc.iter().sum::<f32>() / sloc.len() as f32;
                for s in &mut sloc {
                    *s -= m;
                }
            }
            let cands: Vec<Cand> = (0..lp.k_cands)
                .map(|_| {
                    let img = rng.below(exs.len());
                    let sym = rng.below(8) as u8;
                    let room = exs[img].n[level] - p + 1;
                    Cand {
                        img: img as u32,
                        sym,
                        x0: rng.below(room) as u32,
                        y0: rng.below(room) as u32,
                    }
                })
                .collect();
            let eval = |c: &Cand, tmp: &mut Vec<f32>| -> (f32, f32) {
                let ex = &exs[c.img as usize];
                let exn = ex.n[level];
                let d = &ex.d[level];
                let (mut cost, mut off) = (0f32, 0f32);
                if !ov.is_empty() {
                    let (mut s1, mut s2) = (0f32, 0f32);
                    for &(u, v, old) in &ov {
                        let (a, b) = map(c.sym, p, u as usize, v as usize);
                        let diff = old - d[(c.y0 as usize + b) * exn + c.x0 as usize + a];
                        s1 += diff;
                        s2 += diff * diff;
                    }
                    let nn = ov.len() as f32;
                    if coarsest {
                        off = s1 / nn;
                        cost = (s2 - s1 * s1 / nn) / nn + lp.lambda * off * off;
                    } else {
                        cost = s2 / nn;
                    }
                }
                if guide.is_some() {
                    let gl = &ex.g[level];
                    tmp.clear();
                    let mut sum = 0f32;
                    for v in (0..p).step_by(2) {
                        for u in (0..p).step_by(2) {
                            let (a, b) = map(c.sym, p, u, v);
                            let val = gl[(c.y0 as usize + b) * exn + c.x0 as usize + a];
                            tmp.push(val);
                            sum += val;
                        }
                    }
                    let m = sum / tmp.len() as f32;
                    let mut s2 = 0f32;
                    for (q, &t) in tmp.iter().enumerate() {
                        let diff = sloc[q] - (t - m);
                        s2 += diff * diff;
                    }
                    cost += lp.alpha * s2 / tmp.len() as f32;
                }
                (cost, off)
            };
            // Each candidate owns one result slot; the thread count only changes
            // who fills which slot.
            let mut results = vec![(0f32, 0f32); cands.len()];
            if threads > 1 && cands.len() > 1 {
                let chunk = cands.len().div_ceil(threads);
                std::thread::scope(|s| {
                    for (cs, rs) in cands.chunks(chunk).zip(results.chunks_mut(chunk)) {
                        let eval = &eval;
                        s.spawn(move || {
                            let mut tmp = Vec::new();
                            for (c, r) in cs.iter().zip(rs) {
                                *r = eval(c, &mut tmp);
                            }
                        });
                    }
                });
            } else {
                let mut tmp = Vec::new();
                for (c, r) in cands.iter().zip(&mut results) {
                    *r = eval(c, &mut tmp);
                }
            }
            let min = results.iter().map(|r| r.0).fold(f32::INFINITY, f32::min);
            ensure!(min.is_finite(), "non-finite quilting cost");
            let ok: Vec<usize> = (0..results.len())
                .filter(|&q| results[q].0 <= min * 1.1 + 1e-9)
                .collect();
            let pick = ok[rng.below(ok.len())];
            let (c, off) = (cands[pick], results[pick].1);
            stats.patches += 1;
            stats.sources.insert((c.img, c.sym, c.x0, c.y0));
            let ex = &exs[c.img as usize];
            let exn = ex.n[level];
            let d = &ex.d[level];
            let mut newp = vec![0f32; p * p];
            for v in 0..p {
                for u in 0..p {
                    let (a, b) = map(c.sym, p, u, v);
                    newp[v * p + u] = d[(c.y0 as usize + b) * exn + c.x0 as usize + a] + off;
                }
            }
            // Per-pixel "use new" mask via min-error cuts on the active overlap strips.
            let mut use_new = vec![true; p * p];
            let old_at = |u: usize, v: usize| out[((py + v) % n) * n + (px + u) % n];
            let (sl, st, sr, sb) = (j > 0, i > 0, j == np - 1, i == np - 1);
            let e = |u: usize, v: usize| {
                let dd = old_at(u, v) - newp[v * p + u];
                dd * dd
            };
            let m = lp.fr + 1;
            if sl {
                let mut err = vec![0f32; p * o];
                for v in 0..p {
                    for t in 0..o {
                        err[v * o + t] = e(t, v);
                    }
                }
                let cut = cut_margin(&err, p, o, m);
                for v in 0..p {
                    for t in 0..cut[v] {
                        use_new[v * p + t] = false;
                    }
                }
            }
            if sr {
                let mut err = vec![0f32; p * o];
                for v in 0..p {
                    for t in 0..o {
                        err[v * o + t] = e(p - 1 - t, v);
                    }
                }
                let cut = cut_margin(&err, p, o, m);
                for v in 0..p {
                    for t in 0..cut[v] {
                        use_new[v * p + p - 1 - t] = false;
                    }
                }
            }
            if st {
                let mut err = vec![0f32; p * o];
                for u in 0..p {
                    for t in 0..o {
                        err[u * o + t] = e(u, t);
                    }
                }
                let cut = cut_margin(&err, p, o, m);
                for u in 0..p {
                    for t in 0..cut[u] {
                        use_new[t * p + u] = false;
                    }
                }
            }
            if sb {
                let mut err = vec![0f32; p * o];
                for u in 0..p {
                    for t in 0..o {
                        err[u * o + t] = e(u, p - 1 - t);
                    }
                }
                let cut = cut_margin(&err, p, o, m);
                for u in 0..p {
                    for t in 0..cut[u] {
                        use_new[(p - 1 - t) * p + u] = false;
                    }
                }
            }
            // Feather: box-blur the binary mask so cuts become smooth transitions.
            let mut wgt: Vec<f32> = use_new.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect();
            let r = lp.fr as i64;
            if r > 0 {
                for pass in 0..2 {
                    let src = wgt.clone();
                    for v in 0..p as i64 {
                        for u in 0..p as i64 {
                            let mut a = 0.0;
                            for t in -r..=r {
                                let (uu, vv) = if pass == 0 {
                                    ((u + t).clamp(0, p as i64 - 1), v)
                                } else {
                                    (u, (v + t).clamp(0, p as i64 - 1))
                                };
                                a += src[(vv * p as i64 + uu) as usize];
                            }
                            wgt[(v * p as i64 + u) as usize] = a / (2 * r + 1) as f32;
                        }
                    }
                }
            }
            for v in 0..p {
                for u in 0..p {
                    let idx = ((py + v) % n) * n + (px + u) % n;
                    if written[idx] {
                        let w = wgt[v * p + u];
                        out[idx] = out[idx] * (1.0 - w) + newp[v * p + u] * w;
                    } else {
                        out[idx] = newp[v * p + u];
                        written[idx] = true;
                    }
                }
            }
        }
    }
    ensure!(written.iter().all(|&w| w), "quilting left unwritten cells");
    Ok((out, stats))
}

#[derive(Debug, Clone, Serialize)]
pub struct LevelReport {
    pub level: usize,
    pub resolution: usize,
    pub patch_px: usize,
    pub overlap_px: usize,
    pub candidates: usize,
    pub patches: usize,
    pub distinct_sources: usize,
    pub seconds: f64,
}

/// Synthesize a periodic `resolution²` height grid (f32 precision, zero mean)
/// from prepared exemplar pyramids.
pub fn synthesize_pyramids(
    exs: &[Pyramid],
    recipe: &ExemplarRecipe,
    threads: usize,
) -> Result<(Vec<f32>, Vec<LevelReport>)> {
    ensure!(!exs.is_empty(), "no exemplars");
    let depth = recipe.levels.len();
    let mut rng = Rng(recipe.seed.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0x00AB_CDEF);
    let mut cur: Vec<f32> = vec![];
    let mut reports = Vec::new();
    for level in (0..depth).rev() {
        let spec = &recipe.levels[level];
        let n = recipe.resolution as usize >> level;
        let lp = LevelParams {
            fr: spec.feather_px as usize,
            n,
            p: spec.patch_px as usize,
            o: spec.overlap_px as usize,
            k_cands: spec.candidates as usize,
            alpha: recipe.alpha as f32,
            lambda: recipe.lambda as f32,
        };
        let coarsest = level == depth - 1;
        let started = Instant::now();
        let guide = if coarsest {
            None
        } else {
            Some(up2(&cur, n / 2, true))
        };
        let (d, stats) = quilt(
            &lp,
            level,
            coarsest,
            exs,
            guide.as_deref(),
            &mut rng,
            threads,
        )?;
        cur = match guide {
            Some(g) => g.iter().zip(&d).map(|(a, b)| a + b).collect(),
            None => d,
        };
        reports.push(LevelReport {
            level,
            resolution: n,
            patch_px: lp.p,
            overlap_px: lp.o,
            candidates: lp.k_cands,
            patches: stats.patches,
            distinct_sources: stats.sources.len(),
            seconds: started.elapsed().as_secs_f64(),
        });
    }
    let mean = cur.iter().map(|&v| f64::from(v)).sum::<f64>() / cur.len() as f64;
    let mean = mean as f32;
    for v in &mut cur {
        *v -= mean;
    }
    ensure!(cur.iter().all(|v| v.is_finite()), "non-finite synthesis");
    Ok((cur, reports))
}

/// Load the recipe's sources (hash-checked) and synthesize the height grid in
/// metres, with a JSON report of the run.
pub fn synthesize(
    recipe: &ExemplarRecipe,
    threads: usize,
) -> Result<(Vec<f64>, serde_json::Value)> {
    recipe.validate()?;
    let started = Instant::now();
    let pyramids = load_pyramids(recipe)?;
    let load_seconds = started.elapsed().as_secs_f64();
    if pyramids.is_empty() {
        bail!("no exemplar windows");
    }
    let (height, levels) = synthesize_pyramids(&pyramids, recipe, threads)?;
    let rms =
        (height.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / height.len() as f64).sqrt();
    let report = serde_json::json!({
        "exemplars": pyramids.len(),
        "threads": threads,
        "load_seconds": load_seconds,
        "rms_m": rms,
        "levels": levels,
    });
    Ok((height.into_iter().map(f64::from).collect(), report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smooth band-limited surface plus rough noise, deterministic in (seed, i).
    fn synthetic_exemplar(size: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let waves: Vec<(f64, f64, f64, f64)> = (0..8)
            .map(|_| {
                let k = 1.0 + (rng.below(6)) as f64;
                let a = rng.below(1000) as f64 / 1000.0 * std::f64::consts::TAU;
                let ph = rng.below(1000) as f64 / 1000.0 * std::f64::consts::TAU;
                let amp = 4.0 / k;
                (k, a, ph, amp)
            })
            .collect();
        (0..size * size)
            .map(|i| {
                let (x, y) = ((i % size) as f64, (i / size) as f64);
                let t = std::f64::consts::TAU / size as f64;
                let mut h = 0.0;
                for &(k, a, ph, amp) in &waves {
                    h += amp * (k * t * (x * a.cos() + y * a.sin()) + ph).sin();
                }
                h += rng.below(1000) as f64 * 0.001;
                h as f32
            })
            .collect()
    }

    fn small_recipe() -> ExemplarRecipe {
        let level = |p: u32| ExemplarLevel {
            patch_px: p,
            overlap_px: 8,
            candidates: 40,
            feather_px: 1,
        };
        ExemplarRecipe {
            resolution: 128,
            sources: vec![],
            levels: vec![level(24), level(24)],
            alpha: 0.5,
            lambda: 0.05,
            seed: 3,
        }
    }

    fn pyramids() -> Vec<Pyramid> {
        (0..3)
            .map(|k| build_pyramid(synthetic_exemplar(256, 100 + k), 256, 2))
            .collect()
    }

    #[test]
    fn synthesis_is_identical_for_any_thread_count() {
        let recipe = small_recipe();
        let exs = pyramids();
        let (reference, _) = synthesize_pyramids(&exs, &recipe, 1).unwrap();
        for threads in [2, 3, 8] {
            let (other, _) = synthesize_pyramids(&exs, &recipe, threads).unwrap();
            let same = reference
                .iter()
                .zip(&other)
                .all(|(a, b)| a.to_bits() == b.to_bits());
            assert!(same, "output differs with {threads} threads");
        }
        // A different seed changes the result.
        let mut other = recipe.clone();
        other.seed = 4;
        let (changed, _) = synthesize_pyramids(&exs, &other, 2).unwrap();
        assert!(changed.iter().zip(&reference).any(|(a, b)| a != b));
    }

    #[test]
    fn synthesis_is_finite_zero_mean_and_periodic() {
        let recipe = small_recipe();
        let exs = pyramids();
        let (h, reports) = synthesize_pyramids(&exs, &recipe, 2).unwrap();
        let n = recipe.resolution as usize;
        assert_eq!(h.len(), n * n);
        assert_eq!(reports.len(), 2);
        assert!(h.iter().all(|v| v.is_finite()));
        let mean = h.iter().map(|&v| f64::from(v)).sum::<f64>() / h.len() as f64;
        assert!(mean.abs() < 1e-3, "mean {mean}");
        let rms = (h.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / h.len() as f64).sqrt();
        assert!(rms > 0.5, "flat output, rms {rms}");
        // Mean |step| across the wrap seam against the interior, both axes. The
        // quilt treats the torus uniformly, so they must be comparable.
        let (mut wrap, mut interior, mut wn, mut inn) = (0.0f64, 0.0f64, 0usize, 0usize);
        for y in 0..n {
            for x in 0..n {
                let a = f64::from(h[y * n + x]);
                let right = f64::from(h[y * n + (x + 1) % n]);
                let down = f64::from(h[((y + 1) % n) * n + x]);
                for (d, last) in [
                    ((a - right).abs(), x + 1 == n),
                    ((a - down).abs(), y + 1 == n),
                ] {
                    if last {
                        wrap += d;
                        wn += 1;
                    } else {
                        interior += d;
                        inn += 1;
                    }
                }
            }
        }
        let ratio = (wrap / wn as f64) / (interior / inn as f64);
        assert!(
            (0.7..1.4).contains(&ratio),
            "wrap/interior step ratio {ratio}"
        );
    }

    #[test]
    fn recipe_validation_rejects_bad_parameters() {
        let mut ok = small_recipe();
        ok.sources = vec![ExemplarSource {
            product: "TEST".into(),
            path: "x.tif".into(),
            sha256: "0".repeat(64),
            windows: vec![[0, 0, 256]],
        }];
        ok.validate().unwrap();
        type Mutation = Box<dyn Fn(&mut ExemplarRecipe)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("resolution", Box::new(|r| r.resolution = 100)),
            ("no levels", Box::new(|r| r.levels.clear())),
            ("overlap too big", Box::new(|r| r.levels[0].overlap_px = 24)),
            ("feather too wide", Box::new(|r| r.levels[0].feather_px = 4)),
            (
                "step does not divide",
                Box::new(|r| r.levels[0].patch_px = 25),
            ),
            ("no candidates", Box::new(|r| r.levels[1].candidates = 0)),
            ("nan alpha", Box::new(|r| r.alpha = f64::NAN)),
            ("no sources", Box::new(|r| r.sources.clear())),
            ("bad hash", Box::new(|r| r.sources[0].sha256 = "ABC".into())),
            (
                "window not divisible",
                Box::new(|r| r.sources[0].windows[0][2] = 255),
            ),
            (
                "window smaller than patch",
                Box::new(|r| r.sources[0].windows[0][2] = 32),
            ),
        ];
        for (name, mutate) in cases {
            let mut bad = ok.clone();
            mutate(&mut bad);
            assert!(bad.validate().is_err(), "{name} was accepted");
        }
    }

    #[test]
    fn missing_and_mismatched_sources_fail_clearly() {
        let mut recipe = small_recipe();
        recipe.sources = vec![ExemplarSource {
            product: "TEST".into(),
            path: std::env::temp_dir()
                .join("astrum-no-such-dem.tif")
                .to_string_lossy()
                .into_owned(),
            sha256: "0".repeat(64),
            windows: vec![[0, 0, 256]],
        }];
        let missing = format!("{:#}", load_pyramids(&recipe).err().unwrap());
        assert!(missing.contains("cannot read"), "{missing}");
        // A file that exists but hashes differently.
        let path =
            std::env::temp_dir().join(format!("astrum-exemplar-{}.tif", std::process::id()));
        std::fs::write(&path, b"not a tiff").unwrap();
        recipe.sources[0].path = path.to_string_lossy().into_owned();
        let mismatch = format!("{:#}", load_pyramids(&recipe).err().unwrap());
        std::fs::remove_file(&path).unwrap();
        assert!(mismatch.contains("sha256 mismatch"), "{mismatch}");
    }
}
