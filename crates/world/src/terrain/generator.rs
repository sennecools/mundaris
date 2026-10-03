//! Landform composition and feature-anchored V2 erosion. Bounds are global fallbacks,
//! not sampled extrema or a claim of tight regional interval subdivision.
use super::*;
use glam::{DMat3, DQuat, DVec3};
use mundaris_math::{
    noise::{GLOBAL_BOUNDS, gradient_noise},
    surface::{DirectionalCap, SurfaceLocation},
};

const G: f64 = GLOBAL_BOUNDS.gradient_component_abs * 1.7320508075688772;
const H: f64 = GLOBAL_BOUNDS.hessian_component_abs * 3.0;
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
/// Ordered tagged salts: seed, identity, frozen V1 base salt, field, octave. Constants and
/// operation order are algorithm identity, never dependent on request order.
fn salt(d: &TerrainDefinition, tag: u64, octave: u64) -> u64 {
    let s = mix(d.seed().0 ^ 0x9e3779b97f4a7c15);
    let s = mix(s ^ d.identity().0.wrapping_mul(0xd6e8feb86659fd93));
    // V2 intentionally retains V1 macro/range seeds for matched A/B landforms.
    let s = mix(s ^ 1u64.wrapping_mul(0xa5a3564e27f8862f));
    mix(mix(s ^ tag) ^ octave.wrapping_mul(0x9e3779b185ebca87))
}
#[derive(Debug, Clone, Copy)]
struct Domain {
    seed: u64,
    rotation: DMat3,
    offset: DVec3,
    frequency: f64,
    warp: f64,
    anisotropic: bool,
}
impl Domain {
    fn new(
        d: &TerrainDefinition,
        tag: u64,
        octave: u64,
        frequency: f64,
        warp: f64,
        anisotropic: bool,
    ) -> Self {
        let seed = salt(d, tag, octave);
        let unit = |s: u64| (mix(s) >> 11) as f64 / ((1u64 << 53) as f64);
        let angles =
            DVec3::new(unit(seed), unit(seed ^ 17), unit(seed ^ 29)) * std::f64::consts::TAU;
        Self {
            seed,
            rotation: DMat3::from_quat(DQuat::from_euler(
                glam::EulerRot::XYZ,
                angles.x,
                angles.y,
                angles.z,
            )),
            offset: DVec3::new(unit(seed ^ 41), unit(seed ^ 53), unit(seed ^ 67)) * 32.0,
            frequency,
            warp,
            anisotropic,
        }
    }
    fn evaluate(self, n: DVec3, calls: &mut usize) -> Result<Value, TerrainError> {
        let stretch = if self.anisotropic {
            DMat3::from_diagonal(DVec3::new(1.0, 0.45, 0.2))
        } else {
            DMat3::IDENTITY
        };
        let jacobian = stretch * self.rotation * self.frequency;
        let p = jacobian * n + self.offset;
        let mut warped = p;
        let mut j = jacobian;
        if self.warp != 0.0 {
            let mut values = DVec3::ZERO;
            let mut rows = [DVec3::ZERO; 3];
            for axis in 0..3 {
                let sample = gradient_noise(
                    mix(self.seed ^ (axis as u64 + 1).wrapping_mul(0x57415250)),
                    p * 0.25 + DVec3::splat(7.0),
                )
                .map_err(|_| TerrainError::InvalidConfig)?;
                *calls += 1;
                values[axis] = sample.value;
                rows[axis] = sample.gradient * 0.25;
            }
            warped += self.warp * values;
            let warp_j = DMat3::from_cols(rows[0], rows[1], rows[2]).transpose();
            j = (DMat3::IDENTITY + self.warp * warp_j) * jacobian;
        }
        let sample = gradient_noise(self.seed, warped).map_err(|_| TerrainError::InvalidConfig)?;
        *calls += 1;
        Ok(Value {
            v: sample.value,
            g: j.transpose() * sample.gradient,
        })
    }
    fn bound(self) -> Bound {
        // Vector warp Jacobian <= sqrt(3)*G/4; Hessian <= sqrt(3)*H/16.
        // Anisotropic map norm <=1. Rotation error absorbed by outward margin.
        let stretch = 1.0 + self.warp * 1.7320508075688772 * G * 0.25;
        Bound {
            a: 1.0,
            g: G * self.frequency * stretch,
            h: self.frequency.powi(2)
                * (H * stretch.powi(2) + G * self.warp * 1.7320508075688772 * H / 16.0),
        }
    }
}
#[derive(Clone, Copy)]
struct Value {
    v: f64,
    g: DVec3,
}
impl Value {
    fn constant(v: f64) -> Self {
        Self { v, g: DVec3::ZERO }
    }
    fn scale(self, k: f64) -> Self {
        Self {
            v: self.v * k,
            g: self.g * k,
        }
    }
    fn add(self, b: Self) -> Self {
        Self {
            v: self.v + b.v,
            g: self.g + b.g,
        }
    }
    fn mul(self, b: Self) -> Self {
        Self {
            v: self.v * b.v,
            g: self.g * b.v + b.g * self.v,
        }
    }
    fn tanh(self) -> Self {
        let v = self.v.tanh();
        Self {
            v,
            g: self.g * (1.0 - v * v),
        }
    }
    fn ridge(self, softness: f64) -> Self {
        let denominator = 1.0 / (1.0_f64.hypot(softness) + softness);
        let magnitude = self.v.hypot(softness);
        Self {
            v: (1.0_f64.hypot(softness) - magnitude) / denominator,
            g: self.g * (-self.v / magnitude / denominator),
        }
    }
}
#[derive(Debug, Clone, Copy)]
struct Bound {
    a: f64,
    g: f64,
    h: f64,
}
impl Bound {
    fn constant(a: f64) -> Self {
        Self {
            a: a.abs(),
            g: 0.0,
            h: 0.0,
        }
    }
    fn scale(self, k: f64) -> Self {
        Self {
            a: self.a * k.abs(),
            g: self.g * k.abs(),
            h: self.h * k.abs(),
        }
    }
    fn add(self, b: Self) -> Self {
        Self {
            a: self.a + b.a,
            g: self.g + b.g,
            h: self.h + b.h,
        }
    }
    fn mul(self, b: Self) -> Self {
        Self {
            a: self.a * b.a,
            g: self.g * b.a + b.g * self.a,
            h: self.h * b.a + b.h * self.a + 2.0 * self.g * b.g,
        }
    }
    fn tanh(self) -> Self {
        Self {
            a: 1.0,
            g: self.g,
            h: self.h + 2.0 * self.g * self.g,
        }
    }
    fn ridge(self, e: f64) -> Self {
        let d = 1.0 / (1.0_f64.hypot(e) + e);
        Self {
            a: 1.0,
            g: self.g / d,
            h: self.h / d + self.g * self.g / (e * d),
        }
    }
}
#[derive(Debug, Clone, Copy)]
struct Octave {
    domain: Domain,
    amplitude: f64,
    effective_wavelength_m: f64,
    bound: Bound,
}
#[derive(Debug, Clone, Copy)]
struct ErosionOctave {
    seed: u64,
    rotation: DMat3,
    cell_m: f64,
    amplitude_m: f64,
    bound: Bound,
    effective_m: f64,
}
const MEMO_SLOTS: usize = 256;
#[derive(Clone, Copy)]
struct ErosionState {
    field: Value,
    stack: f64,
}
#[derive(Clone, Copy)]
struct MemoEntry {
    key: [u64; 3],
    count: usize,
    state: ErosionState,
}
struct ErosionContext {
    memo: [Option<MemoEntry>; MEMO_SLOTS],
    noise_calls: usize,
    features: usize,
    feedback: bool,
}
impl ErosionContext {
    fn new(feedback: bool) -> Self {
        Self {
            memo: [None; MEMO_SLOTS],
            noise_calls: 0,
            features: 0,
            feedback,
        }
    }
}
#[derive(Debug, Clone)]
pub struct TerrainGenerator {
    radius_m: f64,
    controls: TerrainControls,
    octaves: [[Option<Octave>; 4]; 5],
    continent: [Domain; 3],
    mountains: [Domain; 2],
    global_height_m: f64,
    erosion: [Option<ErosionOctave>; 5],
    orientation_weights: [[f64; 4]; 5],
}
impl TerrainGenerator {
    pub fn new(definition: &TerrainDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        definition.validate_radius_envelope(radius_m)?;
        Self::compile(definition, radius_m)
    }
    pub(super) fn validate_definition(
        definition: &TerrainDefinition,
        radius_m: f64,
    ) -> Result<(), TerrainError> {
        Self::compile(definition, radius_m).map(|_| ())
    }
    fn compile(d: &TerrainDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        let c = d.config().controls();
        let frequency = |b: TerrainBandConfig| match b.scale() {
            TerrainScale::Metres {
                longest_wavelength_m,
            } => radius_m / longest_wavelength_m,
            TerrainScale::Angular {
                lowest_cycles_per_body,
            } => lowest_cycles_per_body / std::f64::consts::TAU,
        };
        let macro_f = frequency(d.config().band(TerrainBand::Macro));
        let range_f = frequency(d.config().band(TerrainBand::Range));
        let continent = [
            Domain::new(d, 0x424153494e, 0, macro_f, c.warp_fraction(), false),
            Domain::new(d, 0x424153494e, 1, macro_f * 0.63, c.warp_fraction(), false),
            Domain::new(
                d,
                0x55504c494654,
                0,
                macro_f * 0.37,
                c.warp_fraction(),
                false,
            ),
        ];
        let mountains = [
            Domain::new(d, 0x4d41534b, 0, range_f * 0.25, c.warp_fraction(), false),
            Domain::new(d, 0x4d41534b, 1, range_f * 0.17, c.warp_fraction(), false),
        ];
        let uplift = continent[2].bound();
        let basin = continent[0]
            .bound()
            .scale(0.7)
            .add(continent[1].bound().scale(0.3))
            .add(Bound::constant(c.continent_bias()))
            .scale(c.continent_contrast())
            .tanh();
        let mask = if c.mountain_coverage() == 0.0 {
            Bound::constant(0.0)
        } else if c.mountain_coverage() == 1.0 {
            Bound::constant(1.0)
        } else {
            let mut b = mountains[0]
                .bound()
                .scale(0.6)
                .add(mountains[1].bound().scale(0.25))
                .add(uplift.scale(0.15))
                .scale(3.0)
                .tanh()
                .scale(0.5)
                .add(Bound::constant(0.5));
            b.a = 1.0;
            b
        };
        let mut octaves = [[None; 4]; 5];
        for (i, band) in TerrainBand::ALL.into_iter().enumerate() {
            let cfg = d.config().band(band);
            if band == TerrainBand::Regional && d.version() == TerrainGeneratorVersion::V2 {
                continue;
            }
            for o in 0..cfg.octaves() {
                let f = frequency(cfg) * 2f64.powi(i32::from(o));
                let domain = Domain::new(
                    d,
                    0x42414e44 + (i as u64) * 0x100,
                    o as u64,
                    f,
                    if i <= 1 { c.warp_fraction() } else { 0.0 },
                    i == 1 || i == 2,
                );
                let noise = domain.bound();
                let ridge = noise.ridge(c.ridge_softness());
                let mut b = match band {
                    TerrainBand::Macro => basin
                        .scale(0.65)
                        .add(uplift.scale(0.35))
                        .add(noise.scale(0.1))
                        .scale(1.0 / 1.1),
                    TerrainBand::Range => mask.mul(ridge.mul(ridge)).scale(c.mountain_strength()),
                    TerrainBand::Regional => mask.mul(
                        ridge.mul(ridge).scale(0.75).add(
                            Bound::constant(1.0)
                                .add(ridge.scale(-1.0))
                                .mul(Bound::constant(1.0).add(ridge.scale(-1.0)))
                                .scale(-0.25),
                        ),
                    ),
                    TerrainBand::Local => Bound::constant(0.25)
                        .add(mask.scale(0.75))
                        .mul(noise.scale(0.7).add(ridge.mul(ridge).scale(0.3))),
                    TerrainBand::Fine => Bound::constant(0.4).add(mask.scale(0.6)).mul(noise),
                };
                // Range arithmetic is tighter than triangle-inequality bounds:
                // all masks/ridges in [0,1], each signed output in [-1,1].
                b.a = if cfg.amplitude_m() == 0.0 { 0.0 } else { 1.0 };
                let effective = radius_m / (f * (b.g / (G * f)).max(1.0));
                if !f.is_finite()
                    || f <= 0.0
                    || f * 2.0 + 64.0 >= 2_147_483_000.0
                    || !b.g.is_finite()
                    || !b.h.is_finite()
                    || !effective.is_finite()
                    || effective <= 0.0
                {
                    return Err(TerrainError::InvalidConfig);
                }
                octaves[i][o as usize] = Some(Octave {
                    domain,
                    amplitude: cfg.amplitude_m() * 0.5f64.powi(i32::from(o)),
                    effective_wavelength_m: effective,
                    bound: b,
                });
            }
        }
        let mut erosion = [None; 5];
        if d.version() == TerrainGeneratorVersion::V2 {
            let cfg = d.config().band(TerrainBand::Regional);
            let TerrainScale::Metres {
                longest_wavelength_m,
            } = cfg.scale()
            else {
                return Err(TerrainError::InvalidConfig);
            };
            let e = d.config().erosion();
            // Fixed sum < 2. Every octave consumes at most half the remaining
            // regional budget, independent of selected octave count.
            let budget = cfg.absolute_height_bound_m() * e.strength();
            for o in 0..e.octaves() {
                let cell_m = longest_wavelength_m / 4f64.powi(i32::from(o));
                if cell_m < 8.0 || radius_m / cell_m >= 2_147_483_000.0 {
                    return Err(TerrainError::InvalidConfig);
                }
                let pattern = Bound {
                    a: 1.0,
                    g: erosion::GRADIENT_BOUND_FACTOR * radius_m / cell_m,
                    h: erosion::HESSIAN_BOUND_FACTOR * (radius_m / cell_m).powi(2),
                };
                let bound = mask.mul(pattern);
                let effective_m = radius_m / bound.g.max(radius_m / cell_m);
                if !bound.g.is_finite() || !bound.h.is_finite() || !effective_m.is_finite() {
                    return Err(TerrainError::InvalidConfig);
                }
                erosion[o as usize] = Some(ErosionOctave {
                    seed: salt(d, 0x45524f53494f4e, u64::from(o)),
                    rotation: Domain::new(d, 0x45524f53494f4e, u64::from(o), 1.0, 0.0, false)
                        .rotation,
                    cell_m,
                    amplitude_m: budget * 0.5f64.powi(i32::from(o) + 1),
                    bound,
                    effective_m,
                });
            }
        }
        let mut orientation_weights = [[0.0; 4]; 5];
        for i in 0..2 {
            for o in 0..4 {
                if octaves[i][o].is_some_and(|x| x.amplitude != 0.0) {
                    orientation_weights[i][o] = 1.0;
                }
            }
        }
        if erosion.iter().flatten().any(|x| x.amplitude_m != 0.0) {
            orientation_weights[2][0] = 1.0; // request mask even with zero range amplitude
        }
        let mut gradient_bound = 0.0;
        let mut hessian_bound = 0.0;
        for octave in octaves.iter().flatten().flatten() {
            gradient_bound = (gradient_bound + octave.amplitude * octave.bound.g).next_up();
            hessian_bound = (hessian_bound + octave.amplitude * octave.bound.h).next_up();
        }
        for x in erosion.iter().flatten() {
            gradient_bound += x.amplitude_m * x.bound.g;
            hessian_bound += x.amplitude_m * x.bound.h;
        }
        if !(gradient_bound * 1.0000000001).is_finite()
            || !(hessian_bound * 1.0000000001).is_finite()
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            radius_m,
            controls: c,
            octaves,
            continent,
            mountains,
            global_height_m: d.config().absolute_height_bound_m(),
            erosion,
            orientation_weights,
        })
    }
    fn weights(&self, footprint: TerrainFootprint) -> Result<[[f64; 4]; 5], TerrainError> {
        let mut weights = [[0.0; 4]; 5];
        for (i, band) in self.octaves.iter().enumerate() {
            for (o, octave) in band.iter().enumerate() {
                if let Some(x) = octave
                    && x.amplitude != 0.0
                {
                    weights[i][o] = footprint.octave_weight(x.effective_wavelength_m)?;
                }
            }
        }
        Ok(weights)
    }
    fn base(
        &self,
        n: DVec3,
        weights: &[[f64; 4]; 5],
        calls: &mut usize,
    ) -> Result<(Value, Value), TerrainError> {
        let c = self.controls;
        let macro_active = weights[0].iter().any(|w| *w != 0.0);
        let mountain_active = weights[1..].iter().flatten().any(|w| *w != 0.0);
        let uplift = if macro_active
            || (mountain_active && c.mountain_coverage() > 0.0 && c.mountain_coverage() < 1.0)
        {
            self.continent[2].evaluate(n, calls)?
        } else {
            Value::constant(0.0)
        };
        let basin = if macro_active {
            self.continent[0]
                .evaluate(n, calls)?
                .scale(0.7)
                .add(self.continent[1].evaluate(n, calls)?.scale(0.3))
                .add(Value::constant(c.continent_bias()))
                .scale(c.continent_contrast())
                .tanh()
        } else {
            Value::constant(0.0)
        };
        let mask = if !mountain_active || c.mountain_coverage() == 0.0 {
            Value::constant(0.0)
        } else if c.mountain_coverage() == 1.0 {
            Value::constant(1.0)
        } else {
            self.mountains[0]
                .evaluate(n, calls)?
                .scale(0.6)
                .add(self.mountains[1].evaluate(n, calls)?.scale(0.25))
                .add(uplift.scale(0.15))
                .add(Value::constant(2.0 * c.mountain_coverage() - 1.0))
                .scale(3.0)
                .tanh()
                .scale(0.5)
                .add(Value::constant(0.5))
        };
        let mut sum = Value::constant(0.0);
        for (i, band) in self.octaves.iter().enumerate() {
            for (o, octave) in band.iter().enumerate() {
                if let Some(x) = octave {
                    let w = weights[i][o];
                    if w == 0.0 {
                        continue;
                    }
                    let noise = x.domain.evaluate(n, calls)?;
                    let ridge = if i == 1 || i == 2 || i == 3 {
                        noise.ridge(c.ridge_softness())
                    } else {
                        Value::constant(0.0)
                    };
                    let term = match i {
                        0 => basin
                            .scale(0.65)
                            .add(uplift.scale(0.35))
                            .add(noise.scale(0.1))
                            .scale(1.0 / 1.1),
                        1 => mask.mul(ridge.mul(ridge)).scale(c.mountain_strength()),
                        2 => {
                            let valley = Value::constant(1.0).add(ridge.scale(-1.0));
                            mask.mul(
                                ridge
                                    .mul(ridge)
                                    .scale(0.75)
                                    .add(valley.mul(valley).scale(-0.25)),
                            )
                        }
                        3 => Value::constant(0.25)
                            .add(mask.scale(0.75))
                            .mul(noise.scale(0.7).add(ridge.mul(ridge).scale(0.3))),
                        _ => Value::constant(0.4).add(mask.scale(0.6)).mul(noise),
                    };
                    sum = sum.add(term.scale(x.amplitude * w));
                }
            }
        }
        Ok((sum, mask))
    }
    fn erosion_weights(&self, footprint: TerrainFootprint) -> Result<[f64; 5], TerrainError> {
        let mut weights = [0.0; 5];
        for (i, x) in self.erosion.iter().enumerate() {
            if let Some(x) = x
                && x.amplitude_m != 0.0
                && self.controls.mountain_coverage() != 0.0
            {
                weights[i] = footprint.octave_weight(x.effective_m)?;
            }
        }
        Ok(weights)
    }
    fn erosion_term(
        &self,
        n: DVec3,
        mask: Value,
        level: usize,
        context: &mut ErosionContext,
    ) -> Result<Value, TerrainError> {
        let Some(x) = self.erosion[level] else {
            return Ok(Value::constant(0.0));
        };
        if x.amplitude_m == 0.0 || mask.v == 0.0 {
            return Ok(Value::constant(0.0));
        }
        let pattern =
            erosion::octave(x.seed, x.rotation * n * self.radius_m, x.cell_m, |anchor| {
                context.features += 1;
                let length = anchor.length();
                if length == 0.0 {
                    return Ok(erosion::ErosionValue {
                        value: 0.0,
                        gradient: DVec3::ZERO,
                    });
                }
                let a = x.rotation.transpose() * (anchor / length);
                let previous =
                    self.orientation(a, if context.feedback { level } else { 0 }, context)?;
                let tangent = previous.field.g - a * previous.field.g.dot(a);
                Ok(erosion::ErosionValue {
                    value: previous.stack,
                    gradient: x.rotation * tangent / self.radius_m,
                })
            })?;
        Ok(mask
            .mul(Value {
                v: pattern.value,
                g: x.rotation.transpose() * pattern.gradient * self.radius_m,
            })
            .scale(x.amplitude_m))
    }
    fn orientation(
        &self,
        n: DVec3,
        count: usize,
        context: &mut ErosionContext,
    ) -> Result<ErosionState, TerrainError> {
        let key = [n.x.to_bits(), n.y.to_bits(), n.z.to_bits()];
        let index = (mix(key[0] ^ key[1].rotate_left(21) ^ key[2].rotate_left(42) ^ count as u64)
            as usize)
            % MEMO_SLOTS;
        if let Some(entry) = context.memo[index]
            && entry.key == key
            && entry.count == count
        {
            return Ok(entry.state);
        }
        let (base, mask) = self.base(n, &self.orientation_weights, &mut context.noise_calls)?;
        let mut state = ErosionState {
            field: base,
            stack: 1.0,
        };
        for level in 0..count {
            let term = self.erosion_term(n, mask, level, context)?;
            state.field = state.field.add(term);
            if let Some(x) = self.erosion[level]
                && x.amplitude_m != 0.0
            {
                // Negative normalized incision preserves large ridges: subsequent
                // features receive less amplitude inside existing deep creases.
                state.stack *= 1.0 + 0.5 * (term.v / x.amplitude_m).clamp(-1.0, 0.0);
            }
        }
        context.memo[index] = Some(MemoEntry { key, count, state });
        Ok(state)
    }
    fn evaluate(
        &self,
        location: SurfaceLocation,
        weights: &[[f64; 4]; 5],
        erosion_weights: &[f64; 5],
        calls: &mut usize,
        features: &mut usize,
        context: &mut ErosionContext,
    ) -> Result<TerrainSample, TerrainError> {
        let n = location.direction().unit();
        let (mut sum, base_mask) = self.base(n, weights, calls)?;
        if erosion_weights.iter().any(|w| *w != 0.0) {
            let mask = if weights[1..].iter().flatten().any(|w| *w != 0.0) {
                base_mask
            } else {
                self.base(n, &self.orientation_weights, calls)?.1
            };
            context.noise_calls = 0;
            context.features = 0;
            for (level, w) in erosion_weights.iter().copied().enumerate() {
                if w == 0.0 {
                    continue;
                }
                sum = sum.add(self.erosion_term(n, mask, level, context)?.scale(w));
            }
            *calls += context.noise_calls;
            *features += context.features;
        }
        let tangent = sum.g - n * sum.g.dot(n);
        if !sum.v.is_finite() || !tangent.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(TerrainSample::from_parts(sum.v, tangent))
    }
    pub fn evaluate_point(&self, query: TerrainQuery) -> Result<TerrainSample, TerrainError> {
        self.evaluate(
            query.location,
            &self.weights(query.footprint)?,
            &self.erosion_weights(query.footprint)?,
            &mut 0,
            &mut 0,
            &mut ErosionContext::new(true),
        )
    }
    /// Diagnostic only: freeze later feature orientations to the broad field.
    /// Production geometry always uses `evaluate_point`/`evaluate_batch` feedback.
    pub fn evaluate_without_erosion_feedback(
        &self,
        query: TerrainQuery,
    ) -> Result<TerrainSample, TerrainError> {
        self.evaluate(
            query.location,
            &self.weights(query.footprint)?,
            &self.erosion_weights(query.footprint)?,
            &mut 0,
            &mut 0,
            &mut ErosionContext::new(false),
        )
    }
    /// Optional inspection output; not stored in geometry or used by rendering.
    pub fn erosion_diagnostics(
        &self,
        query: TerrainQuery,
    ) -> Result<ErosionDiagnostics, TerrainError> {
        let n = query.location.direction().unit();
        let (base, _) = self.base(n, &self.weights(query.footprint)?, &mut 0)?;
        let (_, mask) = self.base(n, &self.orientation_weights, &mut 0)?;
        let mut output = [TerrainSample::default(); 1];
        let report = self.evaluate_batch(&[query.location], query.footprint, &mut output)?;
        let contribution_m = output[0].height_m() - base.v;
        let budget: f64 = self.erosion.iter().flatten().map(|x| x.amplitude_m).sum();
        Ok(ErosionDiagnostics {
            contribution_m,
            mountain_mask: mask.v,
            crease: if budget == 0.0 {
                0.0
            } else {
                (-contribution_m / budget).clamp(0.0, 1.0)
            },
            tangent_gradient_m: output[0].tangent_gradient_m_per_unit_direction(),
            report,
        })
    }
    pub fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        footprint: TerrainFootprint,
        output: &mut [TerrainSample],
    ) -> Result<TerrainEvaluationReport, TerrainError> {
        if locations.len() != output.len() {
            return Err(TerrainError::LengthMismatch);
        }
        let weights = self.weights(footprint)?;
        let erosion_weights = self.erosion_weights(footprint)?;
        let mut primitive_calls = 0;
        let mut erosion_features = 0;
        // Fixed-anchor coefficients may be reused within one caller-owned batch.
        // This temporary memo never survives the call or enters geometry/cache
        // state; hits change work counts, not sample bits or generator truth.
        let mut context = ErosionContext::new(true);
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate(
                *location,
                &weights,
                &erosion_weights,
                &mut primitive_calls,
                &mut erosion_features,
                &mut context,
            )?;
        }
        let cap = DirectionalCap::new(
            mundaris_math::Direction3::try_new(DVec3::X)
                .map_err(|_| TerrainError::InvalidConfig)?,
            0.0,
        )
        .map_err(|_| TerrainError::InvalidConfig)?;
        Ok(TerrainEvaluationReport {
            sample_count: locations.len(),
            primitive_calls,
            erosion_features,
            active_erosion_octaves: erosion_weights.iter().filter(|w| **w == 1.0).count(),
            faded_erosion_octaves: erosion_weights
                .iter()
                .filter(|w| **w > 0.0 && **w < 1.0)
                .count(),
            skipped_erosion_octaves: self.erosion.iter().flatten().count()
                - erosion_weights.iter().filter(|w| **w != 0.0).count(),
            cartesian_height_gradient_bound_m: self.bounds_for_region(cap, footprint)?.gradient,
        })
    }
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }
    pub fn bounds_for_region(
        &self,
        _cap: DirectionalCap,
        footprint: TerrainFootprint,
    ) -> Result<TerrainRegionCertificate, TerrainError> {
        let weights = self.weights(footprint)?;
        let mut represented = 0.0;
        let mut unresolved = 0.0;
        let mut gradient = 0.0;
        let mut hessian = 0.0;
        let widen = |x: f64| {
            if x == 0.0 {
                0.0
            } else {
                (x + x * 1024.0 * f64::EPSILON).next_up()
            }
        };
        let product = |a: f64, b: f64| {
            if a == 0.0 || b == 0.0 {
                0.0
            } else {
                (a * b).next_up()
            }
        };
        for (i, band) in self.octaves.iter().enumerate() {
            for (o, octave) in band.iter().enumerate() {
                if let Some(x) = octave {
                    let w = weights[i][o];
                    let active = product(x.amplitude, w);
                    let omitted = if w == 1.0 {
                        0.0
                    } else {
                        product(x.amplitude, (1.0 - w).next_up())
                    };
                    represented = widen(represented + active);
                    unresolved = widen(unresolved + omitted);
                    gradient = widen(gradient + product(active, x.bound.g));
                    hessian = widen(hessian + product(active, x.bound.h));
                }
            }
        }
        let ew = self.erosion_weights(footprint)?;
        for (level, x) in self.erosion.iter().enumerate() {
            if let Some(x) = x {
                let w = ew[level];
                represented = widen(represented + product(x.amplitude_m, w));
                unresolved = widen(
                    unresolved
                        + if w == 1.0 {
                            0.0
                        } else {
                            product(x.amplitude_m, (1.0 - w).next_up())
                        },
                );
                gradient = widen(gradient + product(product(x.amplitude_m, w), x.bound.g));
                hessian = widen(hessian + product(product(x.amplitude_m, w), x.bound.h));
            }
        }
        if ![represented, unresolved, gradient, hessian]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(TerrainRegionCertificate {
            interval: [-self.global_height_m, self.global_height_m],
            represented,
            unresolved,
            gradient,
            hessian,
        })
    }

    /// Outward bound on the height difference between two footprint-filtered
    /// profiles at the same direction.
    pub fn profile_difference_bound_m(
        &self,
        a: TerrainFootprint,
        b: TerrainFootprint,
    ) -> Result<f64, TerrainError> {
        let wa = self.weights(a)?;
        let wb = self.weights(b)?;
        let ea = self.erosion_weights(a)?;
        let eb = self.erosion_weights(b)?;
        let mut total = 0.0;
        let product = |x: f64, y: f64| {
            if x == 0.0 || y == 0.0 {
                0.0
            } else {
                (x * y).next_up()
            }
        };
        let widen = |x: f64| {
            if x == 0.0 {
                0.0
            } else {
                (x + x * 1024.0 * f64::EPSILON).next_up()
            }
        };
        for (i, band) in self.octaves.iter().enumerate() {
            for (o, octave) in band.iter().enumerate() {
                if let Some(x) = octave {
                    let delta = (wa[i][o] - wb[i][o]).abs();
                    let delta = if delta == 0.0 { 0.0 } else { delta.next_up() };
                    total = widen(total + product(x.amplitude, delta));
                }
            }
        }
        for (i, octave) in self.erosion.iter().enumerate() {
            if let Some(x) = octave {
                let delta = (ea[i] - eb[i]).abs();
                let delta = if delta == 0.0 { 0.0 } else { delta.next_up() };
                total = widen(total + product(x.amplitude_m, delta));
            }
        }
        if total.is_finite() {
            Ok(total)
        } else {
            Err(TerrainError::InvalidConfig)
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainEvaluationReport {
    sample_count: usize,
    primitive_calls: usize,
    erosion_features: usize,
    active_erosion_octaves: usize,
    faded_erosion_octaves: usize,
    skipped_erosion_octaves: usize,
    cartesian_height_gradient_bound_m: f64,
}
/// Transient query diagnostics, deliberately independent of renderer resources.
#[derive(Debug, Clone, Copy)]
pub struct ErosionDiagnostics {
    pub contribution_m: f64,
    pub mountain_mask: f64,
    pub crease: f64,
    pub tangent_gradient_m: DVec3,
    pub report: TerrainEvaluationReport,
}
impl TerrainEvaluationReport {
    pub fn erosion_feature_evaluations(self) -> usize {
        self.erosion_features
    }
    pub fn active_erosion_octaves(self) -> usize {
        self.active_erosion_octaves
    }
    pub fn faded_erosion_octaves(self) -> usize {
        self.faded_erosion_octaves
    }
    pub fn skipped_erosion_octaves(self) -> usize {
        self.skipped_erosion_octaves
    }
    pub fn sample_count(self) -> usize {
        self.sample_count
    }
    pub fn primitive_calls(self) -> usize {
        self.primitive_calls
    }
    pub fn cartesian_height_gradient_bound_m(self) -> f64 {
        self.cartesian_height_gradient_bound_m
    }
}
#[derive(Debug, Clone, Copy)]
pub struct TerrainRegionCertificate {
    interval: [f64; 2],
    represented: f64,
    unresolved: f64,
    gradient: f64,
    hessian: f64,
}
impl TerrainRegionCertificate {
    pub fn height_interval_m(self) -> [f64; 2] {
        self.interval
    }
    pub fn represented_height_bound_m(self) -> f64 {
        self.represented
    }
    pub fn unresolved_height_bound_m(self) -> f64 {
        self.unresolved
    }
    pub fn cartesian_height_gradient_bound_m(self) -> f64 {
        self.gradient
    }
    pub fn cartesian_height_hessian_bound_m(self) -> f64 {
        self.hessian
    }
}
