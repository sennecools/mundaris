//! Point-evaluated impact crater field (graph op `crater_field`).
//!
//! Designed for the node graph, and later the runtime GPU producer: every value
//! is a pure function of the sample position, with an analytic gradient, and no
//! neighbour reads.
//!
//! **Population.** Diameters are split into octave bands from `d_min_m` to
//! `d_max_m`.
//! - Each band has a periodic lattice whose cells are at least one ejecta reach
//!   wide, so a point only needs its 3×3 neighbour cells.
//! - Each cell holds a hashed number of craters.
//! - Areal coverage per band scales as (D/d_min)^(2−b) for a cumulative
//!   size-frequency slope b, the standard N(>D) ∝ D^−b power law.
//!
//! **Morphology.** For a fresh simple crater:
//! - parabolic bowl with depth = `depth_ratio`·D
//! - sharp raised rim `rim_ratio`·D
//! - ejecta thinning as (r/R)^−`ejecta_exponent`, tapered to zero at
//!   `ejecta_extent` radii
//!
//! Craters above `complex_d_m` get a flat floor and central peak, with depth
//! falling as √(complex/D). Rims are irregular (per-crater angular harmonics) and
//! interiors carry faint radial slump texture.
//!
//! **Age and fading.** Each crater has a hashed age in [0, 1] and a relative
//! degradation s = `degradation`·age, roughly the diffusion length over R.
//! Degraded craters are shallower, rims drop and broaden faster than bowls, and
//! ejecta fades. This approximates topographic diffusion (Fassett & Thomson 2014)
//! and gives a continuous range from crisp to barely visible.
//!
//! **Overprint.** Craters are composed oldest first. A crater's interior replaces
//! the older crater relief beneath it by a factor of 1 − `inheritance`
//! ("cookie cutting"), so young craters cut cleanly through old ones. The input
//! terrain is added unchanged.

use crate::noise::pcg;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

const MAX_PER_CELL: u32 = 12;
const HARMONICS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CraterFieldNode {
    /// Terrain the craters are added to.
    pub input: String,
    pub d_min_m: f64,
    pub d_max_m: f64,
    /// Cumulative size-frequency slope b (N(>D) ∝ D^−b). 2 keeps coverage per
    /// octave constant; larger values favour small craters.
    pub sfd_exponent: f64,
    /// Fraction of the area covered by crater interiors in the smallest octave.
    pub coverage: f64,
    #[serde(default = "depth_ratio")]
    pub depth_ratio: f64,
    #[serde(default = "rim_ratio")]
    pub rim_ratio: f64,
    #[serde(default = "ejecta_extent")]
    pub ejecta_extent: f64,
    #[serde(default = "ejecta_exponent")]
    pub ejecta_exponent: f64,
    /// Simple-to-complex transition diameter (lunar: about 15–20 km).
    #[serde(default = "complex_d")]
    pub complex_d_m: f64,
    /// Rim radius irregularity (fraction of the radius).
    #[serde(default = "irregularity")]
    pub irregularity: f64,
    /// Fraction of older relief that survives inside a new crater.
    #[serde(default = "inheritance")]
    pub inheritance: f64,
    /// Degradation of the oldest crater, as diffusion length over radius.
    /// 0 = all fresh; about 1 makes the oldest nearly invisible.
    #[serde(default = "degradation")]
    pub degradation: f64,
    /// Amplitude of radial slump texture on crater walls (fraction of depth).
    #[serde(default = "slump")]
    pub slump: f64,
    /// Breaks rims and ejecta into segments (0 = perfect rings, about 0.5 strong).
    #[serde(default)]
    pub rim_breakup: f64,
    pub seed: u32,
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
    0.06
}
fn inheritance() -> f64 {
    0.15
}
fn degradation() -> f64 {
    0.8
}
fn slump() -> f64 {
    0.01
}

fn finite_in(value: f64, lo: f64, hi: f64) -> bool {
    value.is_finite() && (lo..=hi).contains(&value)
}

#[derive(Debug, Clone)]
struct Band {
    d_lo: f64,
    d_hi: f64,
    /// Lattice cells per tile edge.
    cells: u32,
    /// Expected craters per cell.
    lambda: f64,
}

/// A validated crater field ready for evaluation (tile coordinates in [0, 1)).
#[derive(Debug, Clone)]
pub struct CraterField {
    node: CraterFieldNode,
    footprint_m: f64,
    bands: Vec<Band>,
}

/// One crater influencing the current sample.
struct Hit {
    age: f64,
    /// Offset from the crater centre to the sample, metres.
    dx: f64,
    dy: f64,
    radius: f64,
    depth: f64,
    rim: f64,
    floor: f64,
    peak: f64,
    harmonics: [(f64, f64); HARMONICS],
    slump_phase: f64,
    /// 1 for a fresh crater, toward 0 as it degrades; scales cookie cutting
    /// so old craters do not leave a sharp erase edge.
    freshness: f64,
}

fn unit(hash: u32) -> f64 {
    f64::from(hash) / f64::from(u32::MAX)
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> (f64, f64) {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    let d = if t > 0.0 && t < 1.0 {
        6.0 * t * (1.0 - t) / (e1 - e0)
    } else {
        0.0
    };
    (t * t * (3.0 - 2.0 * t), d)
}

impl CraterField {
    pub fn new(node: &CraterFieldNode, footprint_m: f64) -> Result<Self> {
        let n = node;
        ensure!(
            finite_in(n.d_min_m, 0.5, footprint_m) && finite_in(n.d_max_m, n.d_min_m, footprint_m),
            "crater_field needs 0.5 <= d_min_m <= d_max_m <= footprint"
        );
        ensure!(
            n.d_max_m * n.ejecta_extent <= footprint_m,
            "crater_field: d_max_m × ejecta_extent must fit in the tile"
        );
        ensure!(
            finite_in(n.sfd_exponent, 0.5, 5.0)
                && finite_in(n.coverage, 0.0, 2.0)
                && finite_in(n.depth_ratio, 0.0, 0.5)
                && finite_in(n.rim_ratio, 0.0, 0.2)
                && finite_in(n.ejecta_extent, 1.2, 6.0)
                && finite_in(n.ejecta_exponent, 1.0, 6.0)
                && finite_in(n.complex_d_m, 0.0, 1.0e7)
                && finite_in(n.irregularity, 0.0, 0.4)
                && finite_in(n.inheritance, 0.0, 1.0)
                && finite_in(n.degradation, 0.0, 4.0)
                && finite_in(n.slump, 0.0, 0.5)
                && finite_in(n.rim_breakup, 0.0, 2.0),
            "crater_field parameters out of range"
        );
        let mut bands = Vec::new();
        let mut d_lo = n.d_min_m;
        loop {
            let d_hi = (2.0 * d_lo).min(n.d_max_m);
            let reach = 0.5 * d_hi * n.ejecta_extent * (1.0 + n.irregularity);
            // Fewest cells whose size still covers one reach; more cells if the
            // per-cell count would exceed the cap.
            let mut cells = ((footprint_m / reach).floor() as u32).max(1);
            let coverage = n.coverage * (d_lo / n.d_min_m).powf(2.0 - n.sfd_exponent);
            let r_mean = 0.25 * (d_lo + d_hi);
            let lambda_for = |cells: u32| {
                let cell_area = (footprint_m / f64::from(cells)).powi(2);
                coverage * cell_area / (std::f64::consts::PI * r_mean * r_mean)
            };
            while lambda_for(cells) > f64::from(MAX_PER_CELL) {
                cells += 1;
            }
            ensure!(
                footprint_m / f64::from(cells) >= reach * 0.999,
                "crater_field: coverage too high for the band at D={d_lo:.1} m"
            );
            bands.push(Band {
                d_lo,
                d_hi,
                cells,
                lambda: lambda_for(cells),
            });
            if d_hi >= n.d_max_m * (1.0 - 1.0e-9) {
                break;
            }
            d_lo = d_hi;
        }
        Ok(Self {
            node: node.clone(),
            footprint_m,
            bands,
        })
    }

    /// Worst-case craters inspected per sample (cost estimate).
    pub fn work_per_sample(&self) -> u32 {
        self.bands
            .iter()
            .map(|b| 9 * (b.lambda.ceil() as u32 + 1).min(MAX_PER_CELL))
            .sum()
    }

    fn crater_hash(&self, band: usize, cx: u32, cy: u32, k: u32, role: u32) -> u32 {
        pcg(self.node.seed
            ^ pcg((band as u32).wrapping_mul(0x9e37_79b9)
                ^ pcg(
                    cx ^ pcg(cy.wrapping_mul(0x85eb_ca6b) ^ pcg(k ^ role.wrapping_mul(0xc2b2_ae35)))
                )))
    }

    fn collect(&self, u: f64, v: f64, hits: &mut Vec<Hit>) {
        let n = &self.node;
        let (px, py) = (u * self.footprint_m, v * self.footprint_m);
        for (bi, band) in self.bands.iter().enumerate() {
            let cells = band.cells as i64;
            let cell_m = self.footprint_m / band.cells as f64;
            let (gx, gy) = ((px / cell_m).floor() as i64, (py / cell_m).floor() as i64);
            for oy in -1..=1 {
                for ox in -1..=1 {
                    let (ix, iy) = (gx + ox, gy + oy);
                    let (hx, hy) = (ix.rem_euclid(cells) as u32, iy.rem_euclid(cells) as u32);
                    let base = band.lambda.floor();
                    let extra = unit(self.crater_hash(bi, hx, hy, 0, 99)) < band.lambda - base;
                    let count = (base as u32 + u32::from(extra)).min(MAX_PER_CELL);
                    for k in 0..count {
                        let h = |role: u32| unit(self.crater_hash(bi, hx, hy, k, role));
                        let cx = (ix as f64 + h(1)) * cell_m;
                        let cy = (iy as f64 + h(2)) * cell_m;
                        // Truncated power law within the band.
                        let b = n.sfd_exponent;
                        let (lo, hi) = (band.d_lo.powf(-b), band.d_hi.powf(-b));
                        let diameter = (lo - h(3) * (lo - hi)).powf(-1.0 / b);
                        let radius = 0.5 * diameter;
                        let (dx, dy) = (px - cx, py - cy);
                        let reach = radius * n.ejecta_extent * (1.0 + n.irregularity);
                        if dx * dx + dy * dy >= reach * reach {
                            continue;
                        }
                        let age = h(4);
                        let s = n.degradation * age;
                        // Diffusion: rims (short wavelength) decay faster than bowls.
                        let depth_fade = 1.0 / (1.0 + (s / 0.45).powi(2));
                        let rim_fade = 1.0 / (1.0 + (s / 0.18).powi(2));
                        let complex = diameter > n.complex_d_m;
                        let depth_ratio = if complex {
                            n.depth_ratio * (n.complex_d_m / diameter).sqrt()
                        } else {
                            n.depth_ratio
                        };
                        let mut harmonics = [(0.0, 0.0); HARMONICS];
                        for (j, hm) in harmonics.iter_mut().enumerate() {
                            *hm = (
                                (h(10 + j as u32) - 0.5) * 2.0 / (j as f64 + 1.5),
                                h(20 + j as u32) * std::f64::consts::TAU,
                            );
                        }
                        hits.push(Hit {
                            age,
                            dx,
                            dy,
                            // Degraded craters broaden slightly.
                            radius: radius * (1.0 + 0.15 * s),
                            depth: depth_ratio * diameter * depth_fade,
                            rim: n.rim_ratio * diameter * rim_fade,
                            floor: if complex { 0.45 } else { 0.0 },
                            peak: if complex {
                                0.3 * depth_ratio * diameter * depth_fade
                            } else {
                                0.0
                            },
                            harmonics,
                            slump_phase: h(30) * std::f64::consts::TAU,
                            freshness: depth_fade,
                        });
                    }
                }
            }
        }
        // Oldest first; ties broken deterministically by size.
        hits.sort_by(|a, b| b.age.total_cmp(&a.age).then(a.radius.total_cmp(&b.radius)));
    }

    /// Crater relief (metres) and its gradient with respect to tile coordinates.
    pub fn sample(&self, u: f64, v: f64) -> (f64, f64, f64) {
        let n = &self.node;
        let mut hits = Vec::new();
        self.collect(u, v, &mut hits);
        // Accumulated crater relief c and its gradient (metres per metre).
        let (mut c, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
        for hit in &hits {
            let dist = hit.dx.hypot(hit.dy).max(1e-9);
            let (ux, uy) = (hit.dx / dist, hit.dy / dist);
            let theta = hit.dy.atan2(hit.dx);
            // Angular radius factor g(θ) = 1 + ε f(θ) and its derivative.
            let (mut f, mut df) = (0.0, 0.0);
            for (j, &(a, phase)) in hit.harmonics.iter().enumerate() {
                let k = j as f64 + 2.0;
                f += a * (k * theta + phase).cos();
                df -= a * k * (k * theta + phase).sin();
            }
            let g = 1.0 + n.irregularity * f;
            let dg = n.irregularity * df;
            let r = dist / (hit.radius * g);
            // ∇r = (û − r·g′/g · θ̂)/(R g), θ̂ = (−uy, ux), ∇θ = θ̂ / dist.
            let (rx, ry) = (
                (ux - r * hit.radius * dg / dist * -uy) / (hit.radius * g),
                (uy - r * hit.radius * dg / dist * ux) / (hit.radius * g),
            );
            let (bowl, dbowl, rim_part, drim_part) = profile_parts(hit, r, n);
            let (m, dm) = rim_factor(hit, theta, n.rim_breakup);
            let profile = bowl + hit.rim * m * rim_part;
            let dprofile_dr = dbowl + hit.rim * m * drim_part;
            // Angular term: ∂/∂θ · ∇θ, with ∇θ = θ̂ / dist.
            let dtheta = hit.rim * rim_part * dm;
            let (ax, ay) = (dtheta * -uy / dist, dtheta * ux / dist);
            // Radial slump texture on the wall, strongest mid-wall.
            let (wall, dwall) = smoothstep(0.35, 0.75, r);
            let (wall_out, dwall_out) = smoothstep(0.75, 0.98, r);
            let wall_w = wall * (1.0 - wall_out);
            let dwall_w = dwall * (1.0 - wall_out) - wall * dwall_out;
            let ripple_k = 9.0 + (hit.slump_phase * 3.0).floor();
            let ripple = (ripple_k * theta + hit.slump_phase).sin();
            let dripple = ripple_k * (ripple_k * theta + hit.slump_phase).cos();
            let slump_amp = n.slump * hit.depth;
            let slump = slump_amp * wall_w * ripple;
            let (tx, ty) = (-uy / dist, ux / dist);
            let slump_x = slump_amp * (dwall_w * rx * ripple + wall_w * dripple * tx);
            let slump_y = slump_amp * (dwall_w * ry * ripple + wall_w * dripple * ty);
            let p = profile + slump;
            let (px, py) = (
                dprofile_dr * rx + slump_x + ax,
                dprofile_dr * ry + slump_y + ay,
            );
            // Cookie cutting of older relief inside the rim.
            let (outside, doutside) = smoothstep(0.85, 1.05, r);
            let cut = (1.0 - n.inheritance) * hit.freshness;
            let erase = cut * (1.0 - outside);
            let (ex, ey) = (-cut * doutside * rx, -cut * doutside * ry);
            let keep = 1.0 - erase;
            let (ncx, ncy) = (cx * keep - c * ex + px, cy * keep - c * ey + py);
            c = c * keep + p;
            cx = ncx;
            cy = ncy;
        }
        (c, cx * self.footprint_m, cy * self.footprint_m)
    }
}

/// Radial profile split into a bowl part and a unit rim part, each with d/dr:
/// height = bowl + rim · m(θ) · rim_part, where m(θ) is the rim breakup factor.
fn profile_parts(hit: &Hit, r: f64, n: &CraterFieldNode) -> (f64, f64, f64, f64) {
    if r < 1.0 {
        let f = hit.floor;
        let (t, dt) = if r <= f {
            (0.0, 0.0)
        } else {
            ((r - f) / (1.0 - f), 1.0 / (1.0 - f))
        };
        // −depth·(1 − t²) + rim·t²: the rim term rises to the crest at r = 1.
        let bowl = -hit.depth * (1.0 - t * t);
        let dbowl = hit.depth * 2.0 * t * dt;
        let (peak, dpeak) = if hit.peak > 0.0 {
            let e = (-(r / 0.12).powi(2)).exp();
            (hit.peak * e, hit.peak * e * (-2.0 * r / (0.12 * 0.12)))
        } else {
            (0.0, 0.0)
        };
        (bowl + peak, dbowl + dpeak, t * t, 2.0 * t * dt)
    } else {
        let extent = n.ejecta_extent;
        if r >= extent {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let (t, dt) = smoothstep(1.0, extent, r);
        let taper = 1.0 - t;
        let power = r.powf(-n.ejecta_exponent);
        let dpower = -n.ejecta_exponent * power / r;
        (0.0, 0.0, power * taper, dpower * taper - power * dt)
    }
}

/// Rim breakup factor m(θ) ≥ 0 and dm/dθ: higher angular harmonics (orders
/// 7–11) reuse the crater's hashed amplitudes with shifted phases, so rims and
/// ejecta break into segments instead of perfect rings.
fn rim_factor(hit: &Hit, theta: f64, breakup: f64) -> (f64, f64) {
    if breakup <= 0.0 {
        return (1.0, 0.0);
    }
    let (mut f, mut df) = (0.0, 0.0);
    for (j, &(a, phase)) in hit.harmonics.iter().enumerate() {
        let k = j as f64 + 7.0;
        let p = phase * 1.7 + 0.9;
        f += 1.5 * a * (k * theta + p).cos();
        df -= 1.5 * a * k * (k * theta + p).sin();
    }
    // Softplus keeps m positive and smooth.
    let x = breakup * f;
    let soft = (1.0 + (4.0 * x).exp()).ln() / 4.0;
    let dsoft = 1.0 / (1.0 + (-4.0 * x).exp());
    let base = (1.0f64 + 1.0).ln() / 4.0;
    // Normalised so m = 1 where f = 0.
    ((1.0 + soft - base).max(0.0), dsoft * breakup * df)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> CraterFieldNode {
        CraterFieldNode {
            input: "x".into(),
            d_min_m: 20.0,
            d_max_m: 400.0,
            sfd_exponent: 2.0,
            coverage: 0.3,
            depth_ratio: 0.2,
            rim_ratio: 0.04,
            ejecta_extent: 2.5,
            ejecta_exponent: 3.0,
            complex_d_m: 300.0,
            irregularity: 0.06,
            inheritance: 0.15,
            degradation: 0.8,
            slump: 0.04,
            rim_breakup: 0.5,
            seed: 5,
        }
    }

    #[test]
    fn field_is_periodic_and_gradient_matches_finite_differences() {
        let field = CraterField::new(&node(), 4000.0).unwrap();
        let h = 1.0e-7;
        let mut agree = 0;
        let mut nonzero = 0;
        for k in 0..400 {
            let u = (f64::from(k) * 0.1373).fract();
            let v = (f64::from(k) * 0.2917 + 0.03).fract();
            let (a, gu, gv) = field.sample(u, v);
            let (b, _, _) = field.sample(u + 1.0, v - 1.0);
            assert!((a - b).abs() < 1e-6, "not periodic: {a} vs {b}");
            if gu.abs() + gv.abs() > 0.0 {
                nonzero += 1;
            }
            let fu = (field.sample(u + h, v).0 - field.sample(u - h, v).0) / (2.0 * h);
            let fv = (field.sample(u, v + h).0 - field.sample(u, v - h).0) / (2.0 * h);
            let scale = 1.0 + fu.hypot(fv);
            if (gu - fu).abs() / scale < 1e-3 && (gv - fv).abs() / scale < 1e-3 {
                agree += 1;
            }
        }
        assert!(nonzero > 100, "field too sparse: {nonzero}");
        assert!(agree >= 390, "gradient agreement {agree}/400");
    }

    #[test]
    fn a_lone_fresh_crater_has_authored_depth_and_rim() {
        let mut n = node();
        n.d_min_m = 1000.0;
        n.d_max_m = 1000.0 * 1.000_001;
        n.coverage = 0.45;
        n.inheritance = 0.0;
        n.degradation = 0.0;
        n.irregularity = 0.0;
        n.slump = 0.0;
        n.rim_breakup = 0.0;
        n.complex_d_m = 1.0e6;
        let field = CraterField::new(&n, 4000.0).unwrap();
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for y in 0..400 {
            for x in 0..400 {
                let (h, _, _) = field.sample(f64::from(x) / 400.0, f64::from(y) / 400.0);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        assert!(lo < -150.0 && lo > -210.0, "floor {lo}");
        assert!(hi > 30.0 && hi < 90.0, "rim {hi}");
    }
}
