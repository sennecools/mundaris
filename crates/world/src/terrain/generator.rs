//! Concrete V1 landform composition. Bounds below are analytic global fallbacks,
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
/// V1 ordered tagged salts: seed, identity, version, field, octave. Constants and
/// operation order are algorithm identity, never dependent on request order.
fn salt(d: &TerrainDefinition, tag: u64, octave: u64) -> u64 {
    let s = mix(d.seed().0 ^ 0x9e3779b97f4a7c15);
    let s = mix(s ^ d.identity().0.wrapping_mul(0xd6e8feb86659fd93));
    let s = mix(s ^ u64::from(d.version().code()).wrapping_mul(0xa5a3564e27f8862f));
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
#[derive(Debug, Clone)]
pub struct TerrainGenerator {
    radius_m: f64,
    controls: TerrainControls,
    octaves: [[Option<Octave>; 4]; 5],
    continent: [Domain; 3],
    mountains: [Domain; 2],
    global_height_m: f64,
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
        let mut gradient_bound = 0.0;
        let mut hessian_bound = 0.0;
        for octave in octaves.iter().flatten().flatten() {
            gradient_bound = (gradient_bound + octave.amplitude * octave.bound.g).next_up();
            hessian_bound = (hessian_bound + octave.amplitude * octave.bound.h).next_up();
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
    fn evaluate(
        &self,
        location: SurfaceLocation,
        weights: &[[f64; 4]; 5],
        calls: &mut usize,
    ) -> Result<TerrainSample, TerrainError> {
        let n = location.direction().unit();
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
        let tangent = sum.g - n * sum.g.dot(n);
        if !sum.v.is_finite() || !tangent.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(TerrainSample::from_parts(sum.v, tangent))
    }
    pub fn evaluate_point(&self, query: TerrainQuery) -> Result<TerrainSample, TerrainError> {
        self.evaluate(query.location, &self.weights(query.footprint)?, &mut 0)
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
        let mut primitive_calls = 0;
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate(*location, &weights, &mut primitive_calls)?;
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
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainEvaluationReport {
    sample_count: usize,
    primitive_calls: usize,
    cartesian_height_gradient_bound_m: f64,
}
impl TerrainEvaluationReport {
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
