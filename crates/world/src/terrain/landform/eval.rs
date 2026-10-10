//! f64 CPU oracle of landform recipes with dual numbers (value and gradient
//! with respect to the body position in metres; M2 Shape design §2–§3).
//!
//! Generated WGSL (Tier B producer) mirrors this file. Conventions:
//!
//! - **Seeds.** Each stack draws `node_seed(body_seed, salt, lane)` (lane 0
//!   relief, 1–3 warp components x/y/z, 4 gullies) and per octave
//!   `octave_seed(node_seed, octave)`.
//! - **Band limit.** Octave `k` is weighted by `octave_weight(f_eff, texel)`;
//!   `f_eff` is [`StackOp::effective_frequency`] (warp and gully octaves use
//!   their own frequency). `texel = None` evaluates the complete function.
//! - **Zero mean.** Ridged and billow octaves subtract the measured mean of
//!   their shape ([`shape_mean`], 3D and 4D separately); gullies subtract
//!   [`gully_mean`]. Means are measured from the linked noise at compile time
//!   and travel in the constants buffer.
//! - **Warp.** `q = p + strength·(F_x, F_y, F_z)(p)`; the stack (relief,
//!   anisotropy field lookup and gullies) is evaluated at `q`, and the
//!   gradient is pulled back with the warp Jacobian.
//! - **Anisotropy.** Octave `k < aniso.octaves` evaluates
//!   `octave_noise4(q, f_k·kappa·δ_c, level_k + stretch_log2)` with
//!   `δ_c = clamp(boundary_coord(q), ±clamp_m)`; its gradient adds
//!   `∂N/∂w · f_k·kappa · ∇δ_c`.
//! - **Damping and gullies.** With `n̂ = q/|q|` and the slope `s = A·|G_t|`
//!   (`A` the landform amplitude, `G_t` the tangential part of a gradient
//!   `G` in output units), octave `k` of a damped stack is scaled by
//!   `1 / (1 + damp·s²)` with `G` summed over earlier octaves whose `f_eff`
//!   is at most half of octave `k`'s. Gully octave `g` (frequency `f_g`,
//!   wavelength `λ_g`) uses `G` summed over relief octaves with
//!   `f_eff ≤ f_g/2` plus earlier gully octaves: stripes run downhill
//!   (`t̂ = n̂ × Ĝ_t` is the contour direction) and the octave adds
//!   `strength·λ_g/(2π)·min(|G_t|, EROSION_SLOPE_CAP/A)·w_g·(1 − 0.6·hardness)·(s_g − μ_g)`.
//!   The damping factors, the stripe direction and the `min(..)` slope term
//!   are held locally constant in the gradient (as in IQ's derivative fBm and
//!   the gully filters of §9.6; exact derivatives would need the Hessian), so
//!   for damped or eroded stacks the returned gradient is the analytic normal
//!   of that convention, not the exact derivative.
use super::ir::{NoiseKind, Op, Program, StackOp};
use super::schema::RecipeField;
use crate::terrain::noise::{lattice_bits, octave_weight, pcg3d};
use crate::terrain::surface::ladder::{
    LadderOctave, octave_noise3, octave_noise4, octave_offset, octave_seed,
};
use glam::DVec3;
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::{Mutex, OnceLock};

/// Seed lanes of [`node_seed`].
pub const LANE_RELIEF: u32 = 0;
pub const LANE_WARP: u32 = 1;
pub const LANE_GULLY: u32 = 4;
/// Hillslope (rise over run) above which gully depth stops growing.
pub const EROSION_SLOPE_CAP: f64 = 1.0;
/// Share of rock hardness that fades gullies: `1 − GULLY_HARDNESS_FADE·hardness`.
/// The pipeline (§9.6) fades by `1 − hardness`; orogens are ~0.85 hard, which
/// left mountains almost without gullies (M2 tuning 2026-10-10).
pub const GULLY_HARDNESS_FADE: f64 = 0.6;
/// Gully kernel: jittered points `cell + 0.5 + GULLY_JITTER·(u − 0.5)` with
/// `u` from `lattice_bits(cell, seed)/2³²`, weights
/// `max(0, 1 − d²/GULLY_RADIUS²)²` over the 3×3×3 cells around `floor(x)`.
/// With jitter 0.4 the own cell's point is always within the radius and a
/// point two cells away never is, so the kernel is continuous.
pub const GULLY_JITTER: f64 = 0.4;
pub const GULLY_RADIUS: f64 = 1.25;

/// Value and gradient with respect to the body position (metres).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Dual {
    pub value: f64,
    pub gradient: DVec3,
}

impl Dual {
    pub const fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: DVec3::ZERO,
        }
    }

    pub const fn new(value: f64, gradient: DVec3) -> Self {
        Self { value, gradient }
    }

    fn scale(self, s: f64) -> Self {
        Self::new(self.value * s, self.gradient * s)
    }
}

/// Tier A fields at a body position, sampled the way Tier B samples them
/// (independent of the texel size). The evaluator clamps values to
/// [`RecipeField::range`].
pub trait FieldSource {
    /// Value and gradient (body space, per metre) of `field` at `p_m`.
    fn field(&self, field: RecipeField, p_m: DVec3) -> Dual;
}

/// Per-body landform parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LandformParams {
    /// Landform amplitude: metres per output unit.
    pub amplitude_m: f64,
    /// Body seed of the landform stage.
    pub seed: u32,
}

/// Seed of one noise source of a node.
pub fn node_seed(body_seed: u32, salt: u32, lane: u32) -> u32 {
    pcg3d([body_seed, salt, lane ^ 0x4c46_0000])[0]
}

impl Program {
    /// Landform height in metres at body position `p_m` (on the reference
    /// sphere), band-limited for texels of `texel_m` (`None`: complete).
    pub fn evaluate(
        &self,
        p_m: DVec3,
        params: &LandformParams,
        texel_m: Option<f64>,
        fields: &dyn FieldSource,
    ) -> Dual {
        let mut values: Vec<Dual> = Vec::with_capacity(self.ops().len());
        for op in self.ops() {
            let v = |i: &usize| values[*i];
            let value = match op {
                Op::Stack(stack) => evaluate_stack(stack, p_m, params, texel_m, fields),
                Op::Field(field) => clamped_field(fields, *field, p_m),
                Op::Const(c) => Dual::constant(*c),
                Op::Add(a, b) => Dual::new(v(a).value + v(b).value, v(a).gradient + v(b).gradient),
                Op::Multiply(a, b) => {
                    let (a, b) = (v(a), v(b));
                    Dual::new(
                        a.value * b.value,
                        a.gradient * b.value + b.gradient * a.value,
                    )
                }
                Op::Mix(a, b, t) => {
                    let (a, b, t) = (v(a), v(b), v(t));
                    Dual::new(
                        a.value + (b.value - a.value) * t.value,
                        a.gradient
                            + (b.gradient - a.gradient) * t.value
                            + t.gradient * (b.value - a.value),
                    )
                }
                Op::Min(a, b) => {
                    if v(b).value < v(a).value {
                        v(b)
                    } else {
                        v(a)
                    }
                }
                Op::Max(a, b) => {
                    if v(b).value > v(a).value {
                        v(b)
                    } else {
                        v(a)
                    }
                }
                Op::SmoothMin(a, b, k) => smooth_min(v(a), v(b), *k),
                Op::Clamp(x, lo, hi) => {
                    let x = v(x);
                    if x.value < *lo {
                        Dual::constant(*lo)
                    } else if x.value > *hi {
                        Dual::constant(*hi)
                    } else {
                        x
                    }
                }
                Op::Curve(x, curve) => {
                    let x = v(x);
                    let (y, dy) = curve.evaluate(x.value);
                    Dual::new(y, x.gradient * dy)
                }
                Op::Scale(x, f) => v(x).scale(*f),
            };
            values.push(value);
        }
        values
            .last()
            .copied()
            .unwrap_or_default()
            .scale(params.amplitude_m)
    }
}

/// Polynomial smooth minimum: `min(a, b) − h²k/4`, `h = max(k − |a − b|, 0)/k`.
fn smooth_min(a: Dual, b: Dual, k: f64) -> Dual {
    let h = (k - (a.value - b.value).abs()).max(0.0) / k;
    let (lo, hi) = if a.value <= b.value { (a, b) } else { (b, a) };
    Dual::new(
        lo.value - h * h * k * 0.25,
        lo.gradient * (1.0 - 0.5 * h) + hi.gradient * (0.5 * h),
    )
}

fn clamped_field(fields: &dyn FieldSource, field: RecipeField, p_m: DVec3) -> Dual {
    let (lo, hi) = field.range();
    let f = fields.field(field, p_m);
    if !f.value.is_finite() {
        Dual::constant(lo.max(0.0).min(hi))
    } else if f.value < lo {
        Dual::constant(lo)
    } else if f.value > hi {
        Dual::constant(hi)
    } else {
        f
    }
}

fn tangential(g: DVec3, normal: DVec3) -> DVec3 {
    g - normal * normal.dot(g)
}

fn band_weight(frequency_per_m: f64, texel_m: Option<f64>) -> f64 {
    texel_m.map_or(1.0, |t| octave_weight(frequency_per_m, t))
}

/// Rounding of the `|n|` crease of ridged and billow octaves, in noise units:
/// `|n|` becomes `√(n² + k²) − k`. A hard crease is a gradient jump at every
/// scale; close up a kilometre-wavelength crest showed as a long, perfectly
/// sharp line. The rounding width scales with each octave's wavelength.
pub const CREASE_ROUNDING: f64 = 0.08;

/// Smooth `|n|` ([`CREASE_ROUNDING`]) and its derivative.
fn soft_abs(n: f64) -> (f64, f64) {
    let s = (n * n + CREASE_ROUNDING * CREASE_ROUNDING).sqrt();
    (s - CREASE_ROUNDING, n / s)
}

/// Shape of one octave value `n` (gradient `g`) before the mean is removed.
fn shape(kind: NoiseKind, n: f64, g: DVec3) -> (f64, DVec3) {
    match kind {
        NoiseKind::Fbm => (n, g),
        NoiseKind::Billow => {
            let (a, da) = soft_abs(n);
            (a, g * da)
        }
        NoiseKind::Ridged { sharpness } => {
            let (a, da) = soft_abs(n);
            let r = 1.0 - a;
            if r <= 0.0 {
                (0.0, DVec3::ZERO)
            } else {
                let v = r.powf(sharpness);
                (v, g * (-da * sharpness * v / r))
            }
        }
    }
}

/// Unit-normalised gain-½ fBm of `octaves` octaves from `base` (warp
/// components).
fn warp_fbm(
    p: DVec3,
    base: LadderOctave,
    octaves: u32,
    seed: u32,
    texel_m: Option<f64>,
) -> (f64, DVec3) {
    let total: f64 = (0..octaves).map(|k| 0.5f64.powi(k as i32)).sum();
    let mut value = 0.0;
    let mut gradient = DVec3::ZERO;
    for k in 0..octaves {
        let octave = LadderOctave {
            ladder: base.ladder,
            level: base.level - k as i32,
        };
        let w = band_weight(octave.frequency_per_m(), texel_m);
        if w <= 0.0 {
            continue;
        }
        let a = 0.5f64.powi(k as i32) / total * w;
        let (n, g) = octave_noise3(p, octave, octave_seed(seed, octave));
        value += a * n;
        gradient += g * a;
    }
    (value, gradient)
}

fn evaluate_stack(
    s: &StackOp,
    p: DVec3,
    params: &LandformParams,
    texel_m: Option<f64>,
    fields: &dyn FieldSource,
) -> Dual {
    // Warp: q = p + d(p); rows[c] = ∇d_c.
    let mut q = p;
    let mut rows = [DVec3::ZERO; 3];
    if let Some(w) = s.warp {
        for (c, row) in rows.iter_mut().enumerate() {
            let seed = node_seed(params.seed, w.salt, LANE_WARP + c as u32);
            let (v, g) = warp_fbm(p, w.base, w.octaves, seed, texel_m);
            q[c] += w.strength_m * v;
            *row = g * w.strength_m;
        }
    }
    let normal = q.normalize_or_zero();
    let delta = s.anisotropy.map(|a| {
        let d = clamped_field(fields, RecipeField::BoundaryCoord, q);
        if d.value.abs() > a.clamp_m {
            Dual::constant(a.clamp_m.copysign(d.value))
        } else {
            d
        }
    });
    let relief_seed = node_seed(params.seed, s.salt, LANE_RELIEF);
    let amplitude = params.amplitude_m;
    // Per-octave effective frequency and gradient contribution (output
    // units, with respect to q) for damping and gully steering.
    let mut contributions: Vec<(f64, DVec3)> = Vec::with_capacity(s.octaves as usize);
    let mut value = 0.0;
    let mut gradient = DVec3::ZERO;
    for k in 0..s.octaves {
        let f_eff = s.effective_frequency(k);
        let w = band_weight(f_eff, texel_m);
        if w <= 0.0 {
            contributions.push((f_eff, DVec3::ZERO));
            continue;
        }
        let octave = s.octave(k);
        let seed = octave_seed(relief_seed, octave);
        let (n, g) = match (s.anisotropy, delta) {
            (Some(a), Some(delta)) if k < a.octaves => {
                let stretched = LadderOctave {
                    ladder: octave.ladder,
                    level: octave.level + a.stretch_log2 as i32,
                };
                let fk = octave.frequency_per_m() * a.kappa;
                let (n, g3, gw) = octave_noise4(q, fk * delta.value, stretched, seed);
                (n, g3 + delta.gradient * (fk * gw))
            }
            _ => octave_noise3(q, octave, seed),
        };
        let (shape_v, shape_g) = shape(s.kind, n, g);
        let damp = s.damp.map_or(1.0, |kd| {
            let steer: DVec3 = contributions
                .iter()
                .filter(|(f, _)| *f <= 0.5 * f_eff * (1.0 + 1e-9))
                .map(|(_, g)| *g)
                .sum();
            let slope = amplitude * tangential(steer, normal).length();
            1.0 / (1.0 + kd * slope * slope)
        });
        let scale = s.amplitude(k) * w * damp;
        value += scale * (shape_v - s.mean(k));
        let contribution = shape_g * scale;
        gradient += contribution;
        contributions.push((f_eff, contribution));
    }
    if let Some(e) = s.erosion {
        let gully_seed = node_seed(params.seed, e.salt, LANE_GULLY);
        let hardness = clamped_field(fields, RecipeField::Hardness, q);
        let fade = Dual::new(
            1.0 - GULLY_HARDNESS_FADE * hardness.value,
            -hardness.gradient * GULLY_HARDNESS_FADE,
        );
        let mut gullies = DVec3::ZERO;
        for k in 0..e.octaves {
            let octave = LadderOctave {
                ladder: e.base.ladder,
                level: e.base.level - k as i32,
            };
            let f = octave.frequency_per_m();
            let w = band_weight(f, texel_m);
            if w <= 0.0 {
                break;
            }
            let steer: DVec3 = contributions
                .iter()
                .filter(|(fr, _)| *fr <= 0.5 * f * (1.0 + 1e-9))
                .map(|(_, g)| *g)
                .sum::<DVec3>()
                + gullies;
            let g_t = tangential(steer, normal);
            let slope = g_t.length();
            if slope <= 0.0 || !slope.is_finite() {
                continue;
            }
            let contour = normal.cross(g_t / slope);
            let (s_v, s_g) = gully(q, octave, octave_seed(gully_seed, octave), contour);
            let cap = if amplitude > 0.0 {
                EROSION_SLOPE_CAP / amplitude
            } else {
                f64::INFINITY
            };
            let depth = e.strength * octave.wavelength_m() / TAU * slope.min(cap) * w;
            let centred = s_v - e.mean;
            value += depth * fade.value * centred;
            let contribution = (s_g * fade.value + fade.gradient * centred) * depth;
            gradient += contribution;
            gullies += contribution;
        }
    }
    // Pull back through the warp: ∇_p = ∇_q + Σ_c (∇_q)_c ∇d_c.
    let pulled = gradient + rows[0] * gradient.x + rows[1] * gradient.y + rows[2] * gradient.z;
    Dual::new(value, pulled)
}

/// Gully kernel of one octave at body position `q_m` with stripes varying
/// along `contour` (unit): `Σ K_i cos(2π (x − c_i)·t̂) / Σ K_i` over the
/// jittered points `c_i`, `x = q_m·f + octave_offset(seed)`. Returns the
/// value (in [−1, 1]) and its gradient per metre (direction held fixed).
pub fn gully(q_m: DVec3, octave: LadderOctave, seed: u32, contour: DVec3) -> (f64, DVec3) {
    let f = octave.frequency_per_m();
    let x = q_m * f + octave_offset(seed);
    let cell = x.floor();
    let r2 = GULLY_RADIUS * GULLY_RADIUS;
    let (mut num, mut den) = (0.0, 0.0);
    let (mut dnum, mut dden) = (DVec3::ZERO, DVec3::ZERO);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let c = cell + DVec3::new(f64::from(dx), f64::from(dy), f64::from(dz));
                let bits = lattice_bits([c.x as i32, c.y as i32, c.z as i32], seed);
                let u = DVec3::new(f64::from(bits[0]), f64::from(bits[1]), f64::from(bits[2]))
                    / 4_294_967_296.0;
                let d = x - (c + DVec3::splat(0.5) + (u - DVec3::splat(0.5)) * GULLY_JITTER);
                let d2 = d.length_squared();
                if d2 >= r2 {
                    continue;
                }
                let t = 1.0 - d2 / r2;
                let kernel = t * t;
                let dkernel = d * (-4.0 * t / r2);
                let phase = TAU * d.dot(contour);
                let (sin, cos) = phase.sin_cos();
                num += kernel * cos;
                den += kernel;
                dnum += dkernel * cos + contour * (-sin * TAU * kernel);
                dden += dkernel;
            }
        }
    }
    if den <= 0.0 {
        return (0.0, DVec3::ZERO);
    }
    let value = num / den;
    (value, (dnum * den - dden * num) / (den * den) * f)
}

// ------------------------------------------------------- measured means

const MEAN_SAMPLES: u32 = 32_768;

/// Measured means by (shape tag, sharpness bits, 4D).
type MeanKey = (u64, u64, bool);

fn mean_cache() -> &'static Mutex<HashMap<MeanKey, f64>> {
    static CACHE: OnceLock<Mutex<HashMap<MeanKey, f64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(key: MeanKey, measure: impl FnOnce() -> f64) -> f64 {
    if let Some(v) = mean_cache().lock().ok().and_then(|c| c.get(&key).copied()) {
        return v;
    }
    let v = measure();
    if let Ok(mut c) = mean_cache().lock() {
        c.insert(key, v);
    }
    v
}

/// Low-discrepancy (R_d) sample `i` in [0, 1)^4.
fn r_sequence(i: u32, four: bool) -> [f64; 4] {
    // Plastic-number generalisations: φ_3 ≈ 1.2207440846, φ_4 ≈ 1.1673039783.
    let phi: f64 = if four {
        1.167_303_978_261_418_7
    } else {
        1.220_744_084_605_759_5
    };
    let n = f64::from(i) + 0.5;
    std::array::from_fn(|d| (n / phi.powi(d as i32 + 1)).fract())
}

/// Mean of the per-octave shape (`max(1 − |n|, 0)^s` or `|n|`; 0 for fBm)
/// of the linked 3D or 4D octave noise, measured over a fixed
/// low-discrepancy point set and 16 seeds.
pub fn shape_mean(kind: NoiseKind, four_d: bool) -> f64 {
    let sharpness = match kind {
        NoiseKind::Fbm => return 0.0,
        NoiseKind::Ridged { sharpness } => sharpness,
        NoiseKind::Billow => 0.0,
    };
    cached((kind.tag(), sharpness.to_bits(), four_d), || {
        let octave = LadderOctave {
            ladder: 0,
            level: 0,
        };
        let mut sum = 0.0;
        for i in 0..MEAN_SAMPLES {
            let r = r_sequence(i, four_d);
            let p = DVec3::new(r[0], r[1], r[2]) * 4096.0 - DVec3::splat(2048.0);
            let seed = octave_seed(0x6d65_616e ^ (i & 15), octave);
            let n = if four_d {
                octave_noise4(p, r[3] * 512.0 - 256.0, octave, seed).0
            } else {
                octave_noise3(p, octave, seed).0
            };
            sum += shape(kind, n, DVec3::ZERO).0;
        }
        sum / f64::from(MEAN_SAMPLES)
    })
}

/// Mean of the gully kernel over positions and stripe directions.
pub fn gully_mean() -> f64 {
    cached((0x6775_6c6c, 0, false), || {
        let octave = LadderOctave {
            ladder: 0,
            level: 0,
        };
        let mut sum = 0.0;
        for i in 0..MEAN_SAMPLES {
            let r = r_sequence(i, true);
            let p = DVec3::new(r[0], r[1], r[2]) * 4096.0 - DVec3::splat(2048.0);
            // Direction from the 4th coordinate and a scrambled copy of the first.
            let z = 2.0 * r[3] - 1.0;
            let phi = TAU * (r[0] * 7919.0).fract();
            let rho = (1.0 - z * z).sqrt();
            let dir = DVec3::new(rho * phi.cos(), rho * phi.sin(), z);
            let seed = octave_seed(0x6775_6c79 ^ (i & 15), octave);
            sum += gully(p, octave, seed, dir).0;
        }
        sum / f64::from(MEAN_SAMPLES)
    })
}
