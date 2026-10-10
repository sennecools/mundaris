//! Interval bounds of landform programs (M2 Shape design §4).
//!
//! [`Program::interval_m`] propagates value intervals through the DAG: a
//! stack octave lies in `a_k·[−B, B]` (fBm, `B` the noise bound),
//! `a_k·[−μ, 1 − μ]` (ridged) or `a_k·[−μ, B − μ]` (billow); damping and
//! band-limit weights only shrink towards 0, which every interval contains.
//! Gully octave `g` adds `[−1 − μ_g, 1 − μ_g]·strength·λ_g/(2π)·cap/A`
//! (the slope term is capped, so this holds for any input gradient). Fields
//! use [`RecipeField::range`](super::schema::RecipeField::range).
//!
//! [`Program::unresolved_bound_m`] bounds `|h_full − h_texel|`. By the nesting
//! rule (`ir.rs`), a stack's band-limited value differs from the full one
//! only by its faded octaves: `Σ (1 − w_k)·|octave k|` for relief and gully
//! octaves. Combinators propagate the error with their Lipschitz constants
//! (`|Δ(ab)| ≤ |a|·Δb + |b|·Δa`; min, max, smooth min and clamp are
//! 1-Lipschitz; a curve by its largest slope).
use super::eval::{EROSION_SLOPE_CAP, LandformParams};
use super::ir::{NoiseKind, Op, Program, StackOp};
use crate::terrain::noise::{GRADIENT_NOISE_BOUND, octave_weight};
use crate::terrain::surface::ladder::LadderOctave;
use std::f64::consts::TAU;

/// Closed interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub const fn new(lo: f64, hi: f64) -> Self {
        Self { lo, hi }
    }

    pub fn magnitude(self) -> f64 {
        self.lo.abs().max(self.hi.abs())
    }

    fn add(self, o: Self) -> Self {
        Self::new(self.lo + o.lo, self.hi + o.hi)
    }

    fn mul(self, o: Self) -> Self {
        let p = [
            self.lo * o.lo,
            self.lo * o.hi,
            self.hi * o.lo,
            self.hi * o.hi,
        ];
        Self::new(
            p.iter().copied().fold(f64::INFINITY, f64::min),
            p.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    }

    fn scale(self, s: f64) -> Self {
        self.mul(Self::new(s, s))
    }
}

impl Program {
    /// Interval of the landform height in metres over all positions,
    /// fields and texel sizes.
    pub fn interval_m(&self, params: &LandformParams) -> Interval {
        let i = self.propagate(params, None).0.scale(params.amplitude_m);
        // Outward allowance for rounding in either evaluator's summation order.
        Interval::new(i.lo - 1e-9 * i.lo.abs(), i.hi + 1e-9 * i.hi.abs())
    }

    /// Largest |height| in metres.
    pub fn bound_m(&self, params: &LandformParams) -> f64 {
        self.interval_m(params).magnitude()
    }

    /// Bound of `|h_full − h_texel|` in metres for texels of `texel_m`: the
    /// relief a coarse tile does not evaluate (culling, far collider, camera
    /// clearance).
    pub fn unresolved_bound_m(&self, params: &LandformParams, texel_m: f64) -> f64 {
        self.propagate(params, Some(texel_m)).1 * params.amplitude_m.abs() * (1.0 + 1e-9)
    }

    /// Value intervals and unresolved errors (output units).
    fn propagate(&self, params: &LandformParams, texel_m: Option<f64>) -> (Interval, f64) {
        if params.amplitude_m == 0.0 {
            return (Interval::new(0.0, 0.0), 0.0);
        }
        let mut out: Vec<(Interval, f64)> = Vec::with_capacity(self.ops().len());
        for op in self.ops() {
            let v = |i: &usize| out[*i];
            let entry = match op {
                Op::Stack(s) => stack_bounds(s, params.amplitude_m, texel_m),
                Op::Field(f) => {
                    let (lo, hi) = f.range();
                    (Interval::new(lo, hi), 0.0)
                }
                Op::Const(c) => (Interval::new(*c, *c), 0.0),
                Op::Add(a, b) => (v(a).0.add(v(b).0), v(a).1 + v(b).1),
                Op::Multiply(a, b) => {
                    let ((ia, ea), (ib, eb)) = (v(a), v(b));
                    (ia.mul(ib), ia.magnitude() * eb + ib.magnitude() * ea)
                }
                Op::Mix(a, b, t) => {
                    let ((ia, ea), (ib, eb), (it, et)) = (v(a), v(b), v(t));
                    let one_minus = Interval::new(1.0 - it.hi, 1.0 - it.lo);
                    (
                        ia.mul(one_minus).add(ib.mul(it)),
                        one_minus.magnitude() * ea
                            + it.magnitude() * eb
                            + (ia.magnitude() + ib.magnitude()) * et,
                    )
                }
                Op::Min(a, b) => (
                    Interval::new(v(a).0.lo.min(v(b).0.lo), v(a).0.hi.min(v(b).0.hi)),
                    v(a).1.max(v(b).1),
                ),
                Op::Max(a, b) => (
                    Interval::new(v(a).0.lo.max(v(b).0.lo), v(a).0.hi.max(v(b).0.hi)),
                    v(a).1.max(v(b).1),
                ),
                Op::SmoothMin(a, b, k) => (
                    Interval::new(
                        v(a).0.lo.min(v(b).0.lo) - 0.25 * k,
                        v(a).0.hi.min(v(b).0.hi),
                    ),
                    v(a).1.max(v(b).1),
                ),
                Op::Clamp(x, lo, hi) => (
                    Interval::new(v(x).0.lo.clamp(*lo, *hi), v(x).0.hi.clamp(*lo, *hi)),
                    v(x).1.min(hi - lo),
                ),
                Op::Curve(x, c) => {
                    let lo = c.ys.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = c.ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    (Interval::new(lo, hi), (c.lipschitz * v(x).1).min(hi - lo))
                }
                Op::Scale(x, f) => (v(x).0.scale(*f), v(x).1 * f.abs()),
            };
            out.push(entry);
        }
        out.last()
            .copied()
            .unwrap_or((Interval::new(0.0, 0.0), 0.0))
    }
}

/// Interval of one stack and its unresolved error at `texel_m`.
fn stack_bounds(s: &StackOp, amplitude_m: f64, texel_m: Option<f64>) -> (Interval, f64) {
    let weight = |f: f64| texel_m.map_or(1.0, |t| octave_weight(f, t));
    let mut interval = Interval::new(0.0, 0.0);
    let mut error = 0.0;
    for k in 0..s.octaves {
        let mu = s.mean(k);
        let shape = match s.kind {
            NoiseKind::Fbm => Interval::new(-GRADIENT_NOISE_BOUND, GRADIENT_NOISE_BOUND),
            NoiseKind::Ridged { .. } => Interval::new(-mu, 1.0 - mu),
            NoiseKind::Billow => Interval::new(-mu, GRADIENT_NOISE_BOUND - mu),
        };
        let octave = shape.scale(s.amplitude(k));
        interval = interval.add(octave);
        error += (1.0 - weight(s.effective_frequency(k))) * octave.magnitude();
    }
    if let Some(e) = s.erosion {
        let cap = EROSION_SLOPE_CAP / amplitude_m.abs();
        for k in 0..e.octaves {
            let octave = LadderOctave {
                ladder: e.base.ladder,
                level: e.base.level - k as i32,
            };
            let depth = e.strength * octave.wavelength_m() / TAU * cap;
            let gully = Interval::new(-1.0 - e.mean, 1.0 - e.mean).mul(Interval::new(0.0, depth));
            interval = interval.add(gully);
            error += (1.0 - weight(octave.frequency_per_m())) * gully.magnitude();
        }
    }
    (interval, error)
}
