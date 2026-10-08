//! Deterministic scalar terrain and climate fields sampled by body-local direction.
//!
//! Recipe identities and seed namespaces separate physical height, climate,
//! material weights, and palette color. Export resolution never enters queries.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

pub mod graph;
pub mod graph_bundle;
pub mod server;

pub const SCHEMA_VERSION: u32 = 1;
pub const ALGORITHM_VERSION: &str = "smooth-gradient-noise-1";
pub const MAX_OCTAVES: u8 = 8;
pub const MAX_RESOLUTION: u32 = 256;
pub const MAX_SAMPLES: usize = 6 * MAX_RESOLUTION as usize * MAX_RESOLUTION as usize;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Io(std::io::Error),
    Json(serde_json::Error),
    Png(png::EncodingError),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid recipe or request: {s}"),
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::Png(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<png::EncodingError> for Error {
    fn from(e: png::EncodingError) -> Self {
        Self::Png(e)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub recipe_id: String,
    pub reference_radius_m: f64,
    pub seeds: Seeds,
    pub height: HeightRecipe,
    pub climate: ClimateRecipe,
    pub materials: MaterialRecipe,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Seeds {
    pub geometry: u64,
    pub climate: u64,
    pub material: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeightRecipe {
    pub base_m: f64,
    pub amplitude_m: f64,
    pub wavelength_m: f64,
    pub octaves: u8,
    pub persistence: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClimateRecipe {
    pub mean_temperature_k: f64,
    pub temperature_variation_k: f64,
    pub temperature_wavelength_m: f64,
    pub humidity_mean: f64,
    pub humidity_variation: f64,
    pub humidity_wavelength_m: f64,
    pub octaves: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialRecipe {
    pub names: [String; 3],
    pub height_transition_m: f64,
    pub temperature_transition_k: f64,
    pub variation_wavelength_m: f64,
    pub variation_strength: f64,
    pub humidity_bias: [f64; 3],
    pub linear_palette: [[f64; 3]; 3],
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub height_m: f64,
    pub temperature_k: f64,
    pub humidity: f64,
    pub material_weights: [f64; 3],
    pub color_linear: [f64; 3],
}

impl Recipe {
    pub fn validate(&self) -> Result<(), Error> {
        let fail = |s: &str| Err(Error::Invalid(s.to_owned()));
        if self.schema_version != SCHEMA_VERSION {
            return fail("unsupported schema_version");
        }
        if self.recipe_id.is_empty() || self.recipe_id.len() > 128 {
            return fail("recipe_id must contain 1..=128 bytes");
        }
        if !finite(self.reference_radius_m) || !(1.0..=1.0e12).contains(&self.reference_radius_m) {
            return fail("reference_radius_m must be finite and within [1, 1e12]");
        }
        let h = &self.height;
        if !finite(h.base_m)
            || !finite(h.amplitude_m)
            || h.amplitude_m < 0.0
            || h.base_m.abs() + h.amplitude_m > self.reference_radius_m * 0.1
        {
            return fail("height envelope is invalid or exceeds 10% of radius");
        }
        if !valid_wavelength(h.wavelength_m, self.reference_radius_m, h.octaves) {
            return fail("height wavelength or octave frequency is out of range");
        }
        if h.octaves == 0 || h.octaves > MAX_OCTAVES {
            return fail("height octaves must be 1..=8");
        }
        if !finite(h.persistence) || !(0.0..=1.0).contains(&h.persistence) {
            return fail("persistence must be in [0,1]");
        }
        let c = &self.climate;
        if !finite(c.mean_temperature_k)
            || !(0.0..=3000.0).contains(&c.mean_temperature_k)
            || !finite(c.temperature_variation_k)
            || c.temperature_variation_k < 0.0
            || c.temperature_variation_k > 2000.0
        {
            return fail("temperature controls are out of range");
        }
        if !finite(c.humidity_mean)
            || !finite(c.humidity_variation)
            || !(0.0..=1.0).contains(&c.humidity_mean)
            || !(0.0..=1.0).contains(&c.humidity_variation)
        {
            return fail("humidity controls must be in [0,1]");
        }
        if !valid_wavelength(
            c.temperature_wavelength_m,
            self.reference_radius_m,
            c.octaves,
        ) || !valid_wavelength(c.humidity_wavelength_m, self.reference_radius_m, c.octaves)
        {
            return fail("climate wavelengths or octave frequencies are out of range");
        }
        if c.octaves == 0 || c.octaves > MAX_OCTAVES {
            return fail("climate octaves must be 1..=8");
        }
        let m = &self.materials;
        if m.names.iter().any(|n| n.is_empty() || n.len() > 64)
            || m.names[0] == m.names[1]
            || m.names[0] == m.names[2]
            || m.names[1] == m.names[2]
        {
            return fail("material names must be unique and contain 1..=64 bytes");
        }
        if !positive(m.height_transition_m)
            || !positive(m.temperature_transition_k)
            || !valid_wavelength(m.variation_wavelength_m, self.reference_radius_m, 2)
            || !finite(m.variation_strength)
            || !(0.0..=2.0).contains(&m.variation_strength)
        {
            return fail("material transition or variation controls are out of range");
        }
        if m.humidity_bias
            .iter()
            .any(|v| !finite(*v) || !(-8.0..=8.0).contains(v))
            || m.linear_palette
                .iter()
                .flatten()
                .any(|v| !finite(*v) || !(0.0..=1.0).contains(v))
        {
            return fail("material biases or linear palette values are out of range");
        }
        Ok(())
    }
    pub fn identity(&self) -> Result<String, Error> {
        let bytes = serde_json::to_vec(self)?;
        Ok(hex(&Sha256::digest(bytes)))
    }
    pub fn geometry_identity(&self) -> Result<String, Error> {
        hash_json(&(
            ALGORITHM_VERSION,
            self.reference_radius_m.to_bits(),
            self.seeds.geometry,
            &self.height,
        ))
    }
    pub fn climate_identity(&self) -> Result<String, Error> {
        hash_json(&(
            ALGORITHM_VERSION,
            self.reference_radius_m.to_bits(),
            self.seeds.climate,
            &self.climate,
        ))
    }
    pub fn material_response_identity(&self) -> Result<String, Error> {
        let mut response = self.materials.clone();
        response.linear_palette = [[0.0; 3]; 3];
        hash_json(&(ALGORITHM_VERSION, self.seeds.material, response))
    }
    pub fn palette_identity(&self) -> Result<String, Error> {
        hash_json(&(self.materials.linear_palette,))
    }
    /// Identity of evaluated material weights, including all field dependencies.
    pub fn material_identity(&self) -> Result<String, Error> {
        hash_json(&(
            self.geometry_identity()?,
            self.climate_identity()?,
            self.material_response_identity()?,
        ))
    }
    pub fn evaluate(&self, direction: [f64; 3]) -> Result<Sample, Error> {
        self.validate()?;
        if direction.iter().any(|v| !finite(*v)) {
            return Err(Error::Invalid("direction components must be finite".into()));
        }
        let len = (direction[0] * direction[0]
            + direction[1] * direction[1]
            + direction[2] * direction[2])
            .sqrt();
        if !finite(len) || len <= f64::MIN_POSITIVE {
            return Err(Error::Invalid(
                "direction must be nonzero and normalizable".into(),
            ));
        }
        let n = [direction[0] / len, direction[1] / len, direction[2] / len];
        let h = self.height.base_m
            + self.height.amplitude_m
                * fbm(
                    n,
                    self.reference_radius_m / self.height.wavelength_m,
                    self.height.octaves,
                    self.height.persistence,
                    self.seeds.geometry,
                );
        let t_noise = fbm(
            n,
            self.reference_radius_m / self.climate.temperature_wavelength_m,
            self.climate.octaves,
            0.5,
            self.seeds.climate,
        );
        let q_noise = fbm(
            n,
            self.reference_radius_m / self.climate.humidity_wavelength_m,
            self.climate.octaves,
            0.5,
            self.seeds.climate ^ 0x9e3779b97f4a7c15,
        );
        let temperature_k = (self.climate.mean_temperature_k
            + self.climate.temperature_variation_k * t_noise)
            .max(0.0);
        let humidity = (self.climate.humidity_mean + self.climate.humidity_variation * q_noise)
            .clamp(0.0, 1.0);
        let m = &self.materials;
        let altitude = ((h - self.height.base_m) / m.height_transition_m).clamp(-1.0, 1.0);
        let thermal = ((temperature_k - self.climate.mean_temperature_k)
            / m.temperature_transition_k)
            .clamp(-1.0, 1.0);
        let material_noise = fbm(
            n,
            self.reference_radius_m / m.variation_wavelength_m,
            2,
            0.5,
            self.seeds.material,
        );
        let logits = [
            m.humidity_bias[0] + humidity * 2.0 - altitude * 0.4
                + material_noise * m.variation_strength,
            m.humidity_bias[1] + altitude * 0.8
                - thermal * 0.2
                - material_noise * m.variation_strength * 0.5,
            m.humidity_bias[2] - humidity + thermal * 0.6 + altitude * 0.2
                - material_noise * m.variation_strength * 0.5,
        ];
        let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let mut weights = logits.map(|v| (v - max).exp());
        let sum: f64 = weights.iter().sum();
        weights.iter_mut().for_each(|v| *v /= sum);
        let color = std::array::from_fn(|channel| {
            (0..3)
                .map(|i| weights[i] * m.linear_palette[i][channel])
                .sum()
        });
        Ok(Sample {
            height_m: h,
            temperature_k,
            humidity,
            material_weights: weights,
            color_linear: color,
        })
    }
}

fn finite(v: f64) -> bool {
    v.is_finite()
}
fn positive(v: f64) -> bool {
    v.is_finite() && v > 0.0
}
fn valid_wavelength(v: f64, radius: f64, octaves: u8) -> bool {
    if !v.is_finite() || v < 0.01 || v > radius * 1.0e6 || octaves == 0 {
        return false;
    }
    let max_lattice_coordinate = (radius / v) * 2f64.powi((octaves - 1) as i32);
    max_lattice_coordinate.is_finite() && max_lattice_coordinate <= 1.0e12
}
fn hash_json<T: Serialize>(v: &T) -> Result<String, Error> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(v)?)))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58476d1ce4e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn lattice(ix: i64, iy: i64, iz: i64, seed: u64) -> [f64; 3] {
    let h = mix64(
        seed ^ (ix as u64).wrapping_mul(0x9e3779b185ebca87)
            ^ (iy as u64).rotate_left(21).wrapping_mul(0xc2b2ae3d27d4eb4f)
            ^ (iz as u64).rotate_left(43).wrapping_mul(0x165667b19e3779f9),
    );
    let z = ((h >> 11) as f64) / ((1u64 << 53) as f64) * 2.0 - 1.0;
    let a = (h.rotate_left(17) >> 11) as f64 / ((1u64 << 53) as f64) * std::f64::consts::TAU;
    let r = (1.0 - z * z).max(0.0).sqrt();
    [r * a.cos(), r * a.sin(), z]
}
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn noise(p: [f64; 3], seed: u64) -> f64 {
    let base = p.map(f64::floor);
    let f = [p[0] - base[0], p[1] - base[1], p[2] - base[2]];
    let b = [base[0] as i64, base[1] as i64, base[2] as i64];
    let w = f.map(fade);
    let mut value = 0.0;
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let d = [f[0] - x as f64, f[1] - y as f64, f[2] - z as f64];
                let g = lattice(b[0] + x, b[1] + y, b[2] + z, seed);
                let wx = if x == 0 { 1.0 - w[0] } else { w[0] };
                let wy = if y == 0 { 1.0 - w[1] } else { w[1] };
                let wz = if z == 0 { 1.0 - w[2] } else { w[2] };
                value += (g[0] * d[0] + g[1] * d[1] + g[2] * d[2]) * wx * wy * wz;
            }
        }
    }
    (value * 1.8).clamp(-1.0, 1.0)
}
fn fbm(n: [f64; 3], frequency: f64, octaves: u8, persistence: f64, seed: u64) -> f64 {
    let mut total = 0.0;
    let mut norm = 0.0;
    let mut amp = 1.0;
    for octave in 0..octaves {
        let f = frequency * 2f64.powi(octave as i32);
        let s = seed.wrapping_add((octave as u64).wrapping_mul(0x9e3779b97f4a7c15));
        total += noise([n[0] * f, n[1] * f, n[2] * f], s) * amp;
        norm += amp;
        amp *= persistence;
    }
    if norm > 0.0 { total / norm } else { 0.0 }
}

/// Map face grid coordinates to one canonical unit direction. Axes are documented in README.
pub fn cube_direction(face: usize, u: f64, v: f64) -> Result<[f64; 3], Error> {
    if face >= 6
        || !finite(u)
        || !finite(v)
        || !(-1.0..=1.0).contains(&u)
        || !(-1.0..=1.0).contains(&v)
    {
        return Err(Error::Invalid(
            "cube face or coordinates out of range".into(),
        ));
    }
    let p = match face {
        0 => [1.0, v, -u],
        1 => [-1.0, v, u],
        2 => [u, 1.0, -v],
        3 => [u, -1.0, v],
        4 => [u, v, 1.0],
        _ => [-u, v, -1.0],
    };
    let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    Ok([p[0] / len, p[1] / len, p[2] / len])
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe() -> Recipe {
        serde_json::from_str(include_str!("../recipes/airless-rocky.json"))
            .expect("fixture recipe parses")
    }
    #[test]
    fn deterministic_seeded_samples() {
        let r = recipe();
        let n = r.evaluate([0.2, 0.3, 0.9]).unwrap();
        assert_eq!(n, r.evaluate([0.2, 0.3, 0.9]).unwrap());
        let mut changed = r.clone();
        changed.seeds.geometry += 1;
        assert_ne!(
            n.height_m,
            changed.evaluate([0.2, 0.3, 0.9]).unwrap().height_m
        );
    }
    #[test]
    fn channel_namespaces_are_isolated() {
        let r = recipe();
        let a = r.evaluate([0.2, 0.3, 0.9]).unwrap();
        let mut p = r.clone();
        p.materials.linear_palette[0] = [1.0, 0.0, 1.0];
        let b = p.evaluate([0.2, 0.3, 0.9]).unwrap();
        assert_eq!(a.height_m, b.height_m);
        assert_eq!(a.temperature_k, b.temperature_k);
        assert_eq!(a.humidity, b.humidity);
        assert_eq!(a.material_weights, b.material_weights);
        assert_ne!(a.color_linear, b.color_linear);
        assert_ne!(r.palette_identity().unwrap(), p.palette_identity().unwrap());
        assert_eq!(
            r.material_identity().unwrap(),
            p.material_identity().unwrap()
        );
        let mut c = r.clone();
        c.seeds.climate += 1;
        let d = c.evaluate([0.2, 0.3, 0.9]).unwrap();
        assert_eq!(a.height_m, d.height_m);
        assert_ne!(a.temperature_k, d.temperature_k);
        assert_ne!(
            r.material_identity().unwrap(),
            c.material_identity().unwrap()
        );
        let mut m = r.clone();
        m.seeds.material += 1;
        assert_eq!(a.height_m, m.evaluate([0.2, 0.3, 0.9]).unwrap().height_m);
        assert_ne!(
            a.material_weights,
            m.evaluate([0.2, 0.3, 0.9]).unwrap().material_weights
        );
    }
    #[test]
    fn outputs_are_bounded_and_weights_normalized() {
        let r = recipe();
        for d in [[0., 0., 1.], [1., 1., 1.], [-1., 0., -1.]] {
            let s = r.evaluate(d).unwrap();
            assert!(s.height_m.is_finite());
            assert!((0.0..=5000.0).contains(&s.temperature_k));
            assert!((0.0..=1.0).contains(&s.humidity));
            assert!(s.material_weights.iter().all(|x| *x >= 0.0));
            assert!((s.material_weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        }
    }
    #[test]
    fn query_has_no_grid_or_order_dependency() {
        let r = recipe();
        let direction = [0.123, 0.456, 0.789];
        let expected = r.evaluate(direction).unwrap();
        for d in [[0., 0., 1.], [1., 1., 1.], [-0.1, 0.2, 0.3]] {
            let _ = r.evaluate(d).unwrap();
        }
        assert_eq!(expected, r.evaluate(direction).unwrap());
        let same_direction_at_another_scale = direction.map(|v| v * 2.0);
        assert_eq!(
            expected,
            r.evaluate(same_direction_at_another_scale).unwrap()
        );
        for resolution in [1u32, 3, 5] {
            let center = (resolution - 1) / 2;
            let u = 2.0 * (center as f64 + 0.5) / resolution as f64 - 1.0;
            let v = 2.0 * (center as f64 + 0.5) / resolution as f64 - 1.0;
            assert_eq!(
                r.evaluate(cube_direction(4, u, v).unwrap()).unwrap(),
                expected_for_axis(&r)
            );
        }
    }
    fn expected_for_axis(r: &Recipe) -> Sample {
        r.evaluate([0.0, 0.0, 1.0]).unwrap()
    }
    #[test]
    fn invalid_inputs_and_limits_rejected() {
        let mut r = recipe();
        r.height.octaves = 9;
        assert!(r.validate().is_err());
        r = recipe();
        r.height.wavelength_m = 0.000001;
        assert!(r.validate().is_err());
        r = recipe();
        r.reference_radius_m = 1.0e12;
        r.climate.humidity_wavelength_m = 0.01;
        assert!(r.validate().is_err());
        r = recipe();
        r.reference_radius_m = f64::NAN;
        assert!(r.validate().is_err());
        r = recipe();
        r.height.base_m = r.reference_radius_m;
        assert!(r.validate().is_err());
        assert!(r.evaluate([0.0, 0.0, 0.0]).is_err());
        assert!(r.evaluate([f64::INFINITY, 0.0, 1.0]).is_err());
    }
    #[test]
    fn cube_shared_edge_and_corner_directions_match() {
        let corner = cube_direction(0, 1.0, 1.0).unwrap();
        assert_eq!(corner, cube_direction(2, 1.0, 1.0).unwrap());
        assert_eq!(corner, cube_direction(5, -1.0, 1.0).unwrap());
        for v in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let a = cube_direction(0, 1.0, v).unwrap();
            let b = cube_direction(5, -1.0, v).unwrap();
            assert_eq!(a, b);
        }
    }
}
