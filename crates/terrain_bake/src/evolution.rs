//! Impact-history simulation for airless bodies (pipeline stage
//! `impact_evolution`), in the spirit of CTEM (Richardson 2009; Minton et al.
//! 2019).
//!
//! Instead of stamping final-state craters, the surface is built by its
//! history:
//! - **Bombardment.** Craters from a power-law size-frequency distribution
//!   arrive in random chronological order, mixing all sizes in time.
//! - **Emplacement into the current surface.** Each crater measures the current
//!   ground around it, excavates a bowl (or a flat floor plus central peak above
//!   the complex transition) relative to that level, and destroys most of the
//!   older relief inside. It then drapes a raised rim and ejecta blanket
//!   thinning as (r/R)^−e over its surroundings, partly burying neighbours.
//!   Rims and ejecta are irregular and broken.
//! - **Unresolved impacts and micrometeorite gardening** act as topographic
//!   diffusion (κt in m², Fassett & Thomson 2014), applied between batches, so
//!   older forms are progressively softened and interwoven by younger ones.
//!   With `diffusion_scaling: crater_size` the diffusivity grows with feature
//!   size, κ_eff(D) = κ_1km·(D/1 km)^0.9 (Fassett et al. 2018, 2021; Minton et al.
//!   2019): one constant κ either erases every small crater or leaves the large
//!   ones fresh (2026-10-09 diagnosis, experiment E3).
//! - **Mass wasting.** Slopes above the angle of repose are lowered after every
//!   batch.
//!
//! Everything is periodic and deterministic for a seed. Craters smaller than
//! about 1.5 cells are not emplaced (they are part of the diffusion).

use crate::noise::pcg;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvolutionRecipe {
    pub d_min_m: f64,
    pub d_max_m: f64,
    /// Cumulative size-frequency slope b (N(>D) ∝ D^−b).
    pub sfd_exponent: f64,
    /// Area covered by crater interiors in the smallest octave, in units of the
    /// tile area (1 = every point hit once on average; > 1 approaches
    /// saturation). Larger octaves scale as (D/d_min)^(2−b).
    pub coverage: f64,
    #[serde(default = "depth_ratio")]
    pub depth_ratio: f64,
    #[serde(default = "rim_ratio")]
    pub rim_ratio: f64,
    #[serde(default = "ejecta_extent")]
    pub ejecta_extent: f64,
    #[serde(default = "ejecta_exponent")]
    pub ejecta_exponent: f64,
    #[serde(default = "complex_d")]
    pub complex_d_m: f64,
    #[serde(default = "irregularity")]
    pub irregularity: f64,
    #[serde(default = "rim_breakup")]
    pub rim_breakup: f64,
    /// Fraction of older relief surviving inside a new crater.
    #[serde(default = "inheritance")]
    pub inheritance: f64,
    /// Total topographic diffusion over the whole bombardment, in m²: κt for
    /// `constant` scaling, κ_1km·t for `crater_size` scaling.
    pub diffusion_m2: f64,
    /// How degradation scales with feature size (see the module docs).
    #[serde(default)]
    pub diffusion_scaling: DiffusionScaling,
    /// Scale-independent degradation: relief relaxes by this many e-folds over
    /// the bombardment at every scale alike (degradation by distal ejecta and
    /// unresolved impacts is roughly proportional to feature size, so old large
    /// and old small craters are equally worn). 0 disables.
    #[serde(default)]
    pub fading: f64,
    /// Angle of repose as a slope (rise over run); 0 disables mass wasting.
    #[serde(default = "repose")]
    pub repose_slope: f64,
    pub batches: u32,
    pub seed: u32,
}

/// Size scaling of the topographic diffusion.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiffusionScaling {
    /// One κ for every size (classic linear diffusion, box-blur approximation).
    #[default]
    Constant,
    /// κ_eff(D) = κ_1km·(D/1 km)^0.9, applied exactly in the frequency domain.
    CraterSize,
}

/// Spectral gain of `crater_size` diffusion for κ_1km·Δt (m²): mode |k| decays
/// as exp(−κ_1km·Δt·c·|k|^1.1). With D = λ/2 = π/|k|, κ_eff(D)·|k|² equals
/// κ_1km·(π/1000)^0.9·|k|^1.1; the factor 1.54 calibrates that wavelength–size
/// mapping so a crater of this module's shape loses the same rim-to-floor relief
/// as under constant κ_eff(D), for κ_eff·t/D² from 0.01 to 0.03
/// (`ai/experiments/heightmap-diagnosis/e3-calibrated-evolution/scripts/calib_frac.js`).
fn crater_size_gain(n: usize, cell_m: f64, kappa_1km_t: f64) -> Vec<f64> {
    let c = 1.54 * (std::f64::consts::PI / 1000.0).powf(0.9);
    crate::fft::wavenumbers(n, cell_m)
        .iter()
        .map(|k| (-kappa_1km_t * c * k.powf(1.1)).exp())
        .collect()
}

fn depth_ratio() -> f64 {
    0.2
}
fn rim_ratio() -> f64 {
    0.04
}
fn ejecta_extent() -> f64 {
    2.5
}
fn ejecta_exponent() -> f64 {
    3.0
}
fn complex_d() -> f64 {
    17_000.0
}
fn irregularity() -> f64 {
    0.08
}
fn rim_breakup() -> f64 {
    0.4
}
fn inheritance() -> f64 {
    0.1
}
fn repose() -> f64 {
    0.65
}

fn finite_in(v: f64, lo: f64, hi: f64) -> bool {
    v.is_finite() && (lo..=hi).contains(&v)
}

impl EvolutionRecipe {
    pub fn validate(&self, footprint_m: f64) -> Result<()> {
        ensure!(
            finite_in(self.d_min_m, 0.5, footprint_m)
                && finite_in(self.d_max_m, self.d_min_m, footprint_m),
            "impact_evolution needs 0.5 <= d_min_m <= d_max_m <= footprint"
        );
        ensure!(
            self.d_max_m * self.ejecta_extent <= footprint_m,
            "impact_evolution: d_max_m × ejecta_extent must fit in the tile"
        );
        ensure!(
            finite_in(self.sfd_exponent, 0.5, 5.0)
                && finite_in(self.coverage, 0.0, 50.0)
                && finite_in(self.depth_ratio, 0.0, 0.5)
                && finite_in(self.rim_ratio, 0.0, 0.2)
                && finite_in(self.ejecta_extent, 1.2, 6.0)
                && finite_in(self.ejecta_exponent, 1.0, 6.0)
                && finite_in(self.complex_d_m, 0.0, 1.0e7)
                && finite_in(self.irregularity, 0.0, 0.4)
                && finite_in(self.rim_breakup, 0.0, 2.0)
                && finite_in(self.inheritance, 0.0, 1.0)
                && finite_in(self.diffusion_m2, 0.0, 1.0e9)
                && finite_in(self.fading, 0.0, 20.0)
                && finite_in(self.repose_slope, 0.0, 10.0)
                && (1..=10_000).contains(&self.batches),
            "impact_evolution parameters out of range"
        );
        Ok(())
    }

    /// Expected number of emplaced craters (cost estimate).
    pub fn crater_count(&self, footprint_m: f64) -> f64 {
        let mut total = 0.0;
        let mut d = self.d_min_m;
        while d < self.d_max_m {
            let hi = (2.0 * d).min(self.d_max_m);
            let cov = self.coverage * (d / self.d_min_m).powf(2.0 - self.sfd_exponent);
            let r = 0.25 * (d + hi);
            total += cov * footprint_m * footprint_m / (std::f64::consts::PI * r * r);
            d = hi;
        }
        total
    }
}

fn unit(h: u32) -> f64 {
    f64::from(h) / f64::from(u32::MAX)
}

struct Impact {
    time: f64,
    x: f64,
    y: f64,
    radius: f64,
    harm: [(f64, f64); 6],
}

fn diffuse(h: &mut [f64], n: usize, cell_m: f64, kappa_t: f64) {
    let total = kappa_t / (cell_m * cell_m);
    if total <= 0.0 {
        return;
    }
    if total > 2.0 {
        // Gaussian of σ² = 2κt (cells²) via three periodic box blurs.
        let sigma2 = 2.0 * total;
        let r = (((4.0 * sigma2 + 1.0).sqrt() - 1.0) / 2.0).round().max(1.0) as usize;
        let r = r.min(n / 2 - 1);
        let blurred = crate::derive::box_blur(
            &crate::derive::box_blur(&crate::derive::box_blur(h, n, r), n, r),
            n,
            r,
        );
        h.copy_from_slice(&blurred);
        return;
    }
    let steps = (total / 0.2).ceil() as usize;
    let k = total / steps as f64;
    let mut next = h.to_vec();
    for _ in 0..steps {
        for y in 0..n {
            let (up, down) = (((y + n - 1) % n) * n, ((y + 1) % n) * n);
            for x in 0..n {
                let (l, r) = ((x + n - 1) % n, (x + 1) % n);
                let i = y * n + x;
                next[i] =
                    h[i] + k * (h[y * n + l] + h[y * n + r] + h[up + x] + h[down + x] - 4.0 * h[i]);
            }
        }
        h.copy_from_slice(&next);
    }
}

/// Run the bombardment on `h` (an n² periodic grid in metres).
/// `craters` holds crater relief from earlier stages (zeros at first); it is
/// degraded together with the new craters and returned updated.
pub fn run(
    h: &mut [f64],
    craters: &mut [f64],
    n: usize,
    cell_m: f64,
    recipe: &EvolutionRecipe,
    seed: u32,
) {
    let footprint = n as f64 * cell_m;
    let min_radius = 0.75 * cell_m;
    // Population: per octave, Poisson-free deterministic counts.
    let mut impacts = Vec::new();
    let mut d_lo = recipe.d_min_m;
    let mut band = 0u32;
    loop {
        let d_hi = (2.0 * d_lo).min(recipe.d_max_m);
        let cov = recipe.coverage * (d_lo / recipe.d_min_m).powf(2.0 - recipe.sfd_exponent);
        let r_mean = 0.25 * (d_lo + d_hi);
        let expected = cov * footprint * footprint / (std::f64::consts::PI * r_mean * r_mean);
        let frac = expected.fract();
        let extra = unit(pcg(seed ^ band.wrapping_mul(0x27d4_eb2d))) < frac;
        let count = expected.floor() as u64 + u64::from(extra);
        let b = recipe.sfd_exponent;
        let (lo, hi) = (d_lo.powf(-b), d_hi.powf(-b));
        for k in 0..count {
            let hsh = |role: u32| {
                unit(pcg((k as u32)
                    ^ pcg(seed
                        ^ band.wrapping_mul(0x9e37_79b9)
                        ^ pcg(role ^ ((k >> 32) as u32)))))
            };
            let diameter = if lo > hi {
                (lo - hsh(1) * (lo - hi)).powf(-1.0 / b)
            } else {
                d_lo
            };
            if 0.5 * diameter < min_radius {
                continue;
            }
            let mut harm = [(0.0, 0.0); 6];
            for (j, hm) in harm.iter_mut().enumerate() {
                *hm = (
                    (hsh(10 + j as u32) - 0.5) * 2.0 / (j as f64 + 1.5),
                    hsh(20 + j as u32) * std::f64::consts::TAU,
                );
            }
            impacts.push(Impact {
                time: hsh(2),
                x: hsh(3) * footprint,
                y: hsh(4) * footprint,
                radius: 0.5 * diameter,
                harm,
            });
        }
        band += 1;
        if d_hi >= recipe.d_max_m * (1.0 - 1e-9) {
            break;
        }
        d_lo = d_hi;
    }
    impacts.sort_by(|a, b| a.time.total_cmp(&b.time));

    let batches = recipe.batches as usize;
    let per_batch = impacts.len().div_ceil(batches).max(1);
    let kappa = recipe.diffusion_m2 / batches as f64;
    let limit = vec![recipe.repose_slope * cell_m; n * n];
    let degrade = |c: &mut [f64], batches_worth: f64| match recipe.diffusion_scaling {
        DiffusionScaling::Constant => diffuse(c, n, cell_m, kappa * batches_worth),
        DiffusionScaling::CraterSize => {
            crate::fft::filter(c, n, &crater_size_gain(n, cell_m, kappa * batches_worth));
        }
    };
    let batch_gain = (recipe.diffusion_scaling == DiffusionScaling::CraterSize)
        .then(|| crater_size_gain(n, cell_m, kappa));
    let wrap = |v: i64| v.rem_euclid(n as i64) as usize;
    let chunks = impacts.len().div_ceil(per_batch);
    // Two layers: the input terrain (kept) and the crater layer c. Impacts
    // measure the combined surface; degradation acts on c only, so worn craters
    // relax back onto the underlying slopes (and open up on them) while the
    // large-scale terrain keeps its relief.
    let base: Vec<f64> = h.iter().zip(craters.iter()).map(|(t, c)| t - c).collect();
    let mut c = craters.to_vec();
    for (bi, chunk) in impacts.chunks(per_batch).enumerate() {
        for imp in chunk {
            emplace(&base, &mut c, n, cell_m, imp, recipe, &wrap);
        }
        match &batch_gain {
            Some(gain) => crate::fft::filter(&mut c, n, gain),
            None => diffuse(&mut c, n, cell_m, kappa),
        }
        if recipe.fading > 0.0 {
            let keep = (-recipe.fading / batches as f64).exp();
            for v in c.iter_mut() {
                *v *= keep;
            }
        }
        // Mass wasting every fourth batch and at the end: it is costly and
        // diffusion keeps most slopes below repose in between.
        if recipe.repose_slope > 0.0 && (bi % 4 == 3 || bi + 1 == chunks) {
            let mut total: Vec<f64> = base.iter().zip(&c).map(|(b, v)| b + v).collect();
            crate::pipeline::slope_limit(&mut total, n, &limit);
            for ((v, t), b) in c.iter_mut().zip(&total).zip(&base) {
                *v = t - b;
            }
        }
    }
    // Any remaining diffusion budget if there were fewer chunks than batches.
    let used = impacts.len().div_ceil(per_batch);
    if used < batches {
        degrade(&mut c, (batches - used) as f64);
    }
    for ((v, b), d) in h.iter_mut().zip(&base).zip(&c) {
        *v = b + d;
    }
    craters.copy_from_slice(&c);
}

// Grid, both layers, the impact and the recipe stay explicit at this call site.
#[allow(clippy::too_many_arguments)]
fn emplace(
    base: &[f64],
    c: &mut [f64],
    n: usize,
    cell_m: f64,
    imp: &Impact,
    recipe: &EvolutionRecipe,
    wrap: &impl Fn(i64) -> usize,
) {
    let r0 = imp.radius;
    let diameter = 2.0 * r0;
    let complex = diameter > recipe.complex_d_m;
    let depth_ratio = if complex {
        recipe.depth_ratio * (recipe.complex_d_m / diameter).sqrt()
    } else {
        recipe.depth_ratio
    };
    let depth = depth_ratio * diameter;
    let rim = recipe.rim_ratio * diameter;
    let floor = if complex { 0.45 } else { 0.0 };
    let peak = if complex { 0.3 * depth } else { 0.0 };
    let (cx, cy) = (
        (imp.x / cell_m).floor() as i64,
        (imp.y / cell_m).floor() as i64,
    );
    // Reference level: mean current height over the interior.
    let ri = (r0 / cell_m).ceil() as i64;
    let (mut sum, mut cnt) = (0.0, 0.0);
    let step = (ri / 6).max(1);
    let mut dy = -ri;
    while dy <= ri {
        let mut dx = -ri;
        while dx <= ri {
            if (dx * dx + dy * dy) as f64 * cell_m * cell_m <= r0 * r0 {
                let j = wrap(cy + dy) * n + wrap(cx + dx);
                sum += base[j] + c[j];
                cnt += 1.0;
            }
            dx += step;
        }
        dy += step;
    }
    let reference = if cnt > 0.0 {
        sum / cnt
    } else {
        let j = wrap(cy) * n + wrap(cx);
        base[j] + c[j]
    };
    let reach = r0 * recipe.ejecta_extent * (1.0 + recipe.irregularity);
    let half = ((reach / cell_m).ceil() as i64).min(n as i64 / 2 - 1);
    for oy in -half..=half {
        let py = (cy + oy) as f64 * cell_m + 0.5 * cell_m - imp.y;
        for ox in -half..=half {
            let px = (cx + ox) as f64 * cell_m + 0.5 * cell_m - imp.x;
            let dist = px.hypot(py);
            if dist > reach {
                continue;
            }
            let theta = py.atan2(px);
            let (mut f, mut g) = (0.0, 0.0);
            for (j, &(a, phase)) in imp.harm.iter().enumerate() {
                f += a * ((j as f64 + 2.0) * theta + phase).cos();
                g += 1.5 * a * ((j as f64 + 7.0) * theta + phase * 1.7 + 0.9).cos();
            }
            let r = dist / (r0 * (1.0 + recipe.irregularity * f));
            let breakup = (1.0 + recipe.rim_breakup * g).max(0.0);
            let i = wrap(cy + oy) * n + wrap(cx + ox);
            let old = base[i] + c[i];
            let new = if r < 1.0 {
                let t = if r <= floor {
                    0.0
                } else {
                    (r - floor) / (1.0 - floor)
                };
                let bowl = -depth * (1.0 - t * t) + rim * breakup * t * t;
                let pk = if peak > 0.0 {
                    peak * (-(r / 0.12).powi(2)).exp()
                } else {
                    0.0
                };
                let carved = reference + bowl + pk + recipe.inheritance * (old - reference);
                // Blend toward the draped rim over the outer 15%.
                let w = ((1.0 - r) / 0.15).clamp(0.0, 1.0);
                let w = w * w * (3.0 - 2.0 * w);
                let draped = old + rim * breakup;
                draped + w * (carved - draped)
            } else {
                let extent = recipe.ejecta_extent;
                if r >= extent {
                    old
                } else {
                    let t = ((r - 1.0) / (extent - 1.0)).clamp(0.0, 1.0);
                    let taper = 1.0 - t * t * (3.0 - 2.0 * t);
                    old + rim * breakup * r.powf(-recipe.ejecta_exponent) * taper
                }
            };
            c[i] = new - base[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipe() -> EvolutionRecipe {
        EvolutionRecipe {
            d_min_m: 8.0,
            d_max_m: 200.0,
            sfd_exponent: 2.0,
            coverage: 1.0,
            depth_ratio: 0.2,
            rim_ratio: 0.04,
            ejecta_extent: 2.5,
            ejecta_exponent: 3.0,
            complex_d_m: 1.0e6,
            irregularity: 0.08,
            rim_breakup: 0.4,
            inheritance: 0.1,
            diffusion_m2: 20.0,
            diffusion_scaling: DiffusionScaling::Constant,
            fading: 1.0,
            repose_slope: 0.65,
            batches: 8,
            seed: 3,
        }
    }

    #[test]
    fn bombardment_is_deterministic_periodic_and_bounded_in_slope() {
        let (n, cell) = (128, 4.0);
        let mut a = vec![0.0; n * n];
        run(&mut a, &mut vec![0.0; n * n], n, cell, &recipe(), 9);
        let mut b = vec![0.0; n * n];
        run(&mut b, &mut vec![0.0; n * n], n, cell, &recipe(), 9);
        assert_eq!(a, b);
        let step = |i: usize, j: usize| (a[i] - a[j]).abs();
        let wrap_max = (0..n)
            .map(|y| step(y * n + n - 1, y * n))
            .fold(0.0, f64::max);
        let interior_max = (0..n)
            .flat_map(|y| (0..n - 1).map(move |x| (y, x)))
            .map(|(y, x)| step(y * n + x, y * n + x + 1))
            .fold(0.0, f64::max);
        assert!(
            wrap_max <= interior_max + 1e-9,
            "{wrap_max} vs {interior_max}"
        );
        // Mass wasting caps every neighbour step at the repose slope.
        assert!(
            interior_max <= 0.65 * cell * std::f64::consts::SQRT_2 + 1e-6,
            "{interior_max}"
        );
        assert!(a.iter().any(|&v| v < -1.0), "no craters were emplaced");
    }

    #[test]
    fn crater_size_scaling_keeps_small_craters_that_constant_kappa_erases() {
        // κ_1km·t of a ~3 Ga lunar surface (Fassett & Thomson 2014).
        let kt = 16_500.0;
        // E-folds of relief decay over the whole bombardment for features of
        // size D (D = λ/2 = π/|k|), size scaling vs one constant κ = κ_1km. The
        // grid must resolve D: 2 m cells for 10 m, a 4 km tile for 1 km.
        let efolds = |n: usize, cell: f64, d: f64| {
            let gain = crater_size_gain(n, cell, kt);
            let k = crate::fft::wavenumbers(n, cell);
            let target = std::f64::consts::PI / d;
            let i = (0..n * n)
                .min_by(|&a, &b| (k[a] - target).abs().total_cmp(&(k[b] - target).abs()))
                .unwrap();
            (-gain[i].ln(), kt * k[i] * k[i])
        };
        // About equal at the 1 km anchor (×1.54 shape calibration), tens of
        // times fewer at 10 m, so late small craters survive instead of
        // vanishing within a batch.
        let (scaled, constant) = efolds(256, 16.0, 1000.0);
        assert!(
            (scaled / constant - 1.54).abs() < 0.2,
            "{scaled} {constant}"
        );
        let (scaled, constant) = efolds(256, 2.0, 10.0);
        assert!(constant / scaled > 30.0, "{scaled} {constant}");

        let mut r = recipe();
        r.diffusion_scaling = DiffusionScaling::CraterSize;
        let (n, cell) = (128, 4.0);
        let mut a = vec![0.0; n * n];
        run(&mut a, &mut vec![0.0; n * n], n, cell, &r, 9);
        let mut b = vec![0.0; n * n];
        run(&mut b, &mut vec![0.0; n * n], n, cell, &r, 9);
        assert_eq!(a, b);
        assert!(a.iter().all(|v| v.is_finite()) && a.iter().any(|&v| v < -1.0));
    }
}
