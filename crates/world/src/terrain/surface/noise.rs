//! Integer-hashed gradient noise on a split lattice and band-limited fBm
//! detail (`ASTRUM_TERRAIN_PIPELINE.md` §4.4, §9.3, §17.1, App. A.1–A.3).
//!
//! The GPU producer (`terrain_noise.wgsl`) evaluates the same functions in f32.
//! It never sees an absolute lattice coordinate: per node and octave, the CPU
//! splits `node_centre * frequency` in f64 into an integer cell and an f32
//! fraction ([`DetailNoise::split`]), and the GPU adds the f32 offset of each
//! sample from the node centre. This module is the f64 test oracle for that
//! evaluation; hashing is integer-only so both sides pick identical lattice
//! gradients.
use glam::DVec3;
use serde::{Deserialize, Serialize};

/// Largest number of octaves a definition may produce; also the per-node
/// octave capacity of the GPU producer.
pub const MAX_DETAIL_OCTAVES: usize = 16;
/// Conservative bound of `|gradient_noise|` for unit lattice gradients
/// (the attained maximum of 3D Perlin noise is about `sqrt(3)/2`).
pub const GRADIENT_NOISE_BOUND: f64 = 1.0;
/// Largest absolute lattice coordinate accepted, keeping every cell inside i32
/// with headroom for the GPU's local cell offsets.
const MAX_LATTICE_COORDINATE: f64 = (1u32 << 30) as f64;

/// Authored band-limited fBm detail layer added to a body's surface height.
/// Octave `k` has wavelength `base_wavelength_m / lacunarity^k` and amplitude
/// `amplitude_m * gain^k`; octaves stop before the wavelength drops below
/// `min_wavelength_m`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetailNoiseDefinition {
    pub version: u32,
    pub seed_salt: u32,
    pub base_wavelength_m: f64,
    pub min_wavelength_m: f64,
    pub lacunarity: f64,
    pub gain: f64,
    pub amplitude_m: f64,
}

impl DetailNoiseDefinition {
    pub fn validate(&self) -> Result<(), super::TerrainError> {
        let finite = [
            self.base_wavelength_m,
            self.min_wavelength_m,
            self.lacunarity,
            self.gain,
            self.amplitude_m,
        ]
        .iter()
        .all(|v| v.is_finite());
        if self.version != 1
            || !finite
            || !(0.01..=100_000.0).contains(&self.min_wavelength_m)
            || !(self.min_wavelength_m..=100_000.0).contains(&self.base_wavelength_m)
            || !(1.5..=4.0).contains(&self.lacunarity)
            || !(self.gain > 0.0 && self.gain < 1.0)
            || !(0.0..=1000.0).contains(&self.amplitude_m)
            || self.octave_count() > MAX_DETAIL_OCTAVES
        {
            return Err(super::TerrainError::InvalidConfig);
        }
        Ok(())
    }

    /// Number of octaves whose wavelength is at least `min_wavelength_m`.
    pub fn octave_count(&self) -> usize {
        let mut count = 0;
        let mut wavelength = self.base_wavelength_m;
        // A small relative slack keeps exact authored ratios inclusive.
        while wavelength >= self.min_wavelength_m * (1.0 - 1.0e-9) && count <= MAX_DETAIL_OCTAVES {
            count += 1;
            wavelength /= self.lacunarity;
        }
        count
    }

    /// Words participating in the owning definition's terrain identity.
    pub fn configuration_identity(&self) -> u64 {
        [
            0x4445_5441_494c_0001 ^ u64::from(self.version),
            u64::from(self.seed_salt),
            self.base_wavelength_m.to_bits(),
            self.min_wavelength_m.to_bits(),
            self.lacunarity.to_bits(),
            self.gain.to_bits(),
            self.amplitude_m.to_bits(),
        ]
        .into_iter()
        .fold(0, |hash, word| splitmix(hash ^ word))
    }
}

/// One compiled octave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailOctave {
    pub frequency_per_m: f64,
    pub amplitude_m: f64,
    pub seed: u32,
}

/// Lattice origin of one octave for one node, split in f64 (§4.4). `cell` is
/// `floor(centre * frequency)`, `fraction` the f32 remainder. `amplitude_m`
/// already includes the node's band-limit weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OctaveOrigin {
    pub cell: [i32; 3],
    pub seed: u32,
    pub fraction: [f32; 3],
    pub frequency_per_m: f32,
    pub amplitude_m: f32,
}

/// Compiled detail layer of one body.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailNoise {
    octaves: Vec<DetailOctave>,
}

impl DetailNoise {
    /// Compile a validated definition. `body_seed` is the owning surface's
    /// seed; the definition's salt decorrelates layers sharing a body.
    pub fn new(
        definition: &DetailNoiseDefinition,
        body_seed: u64,
    ) -> Result<Self, super::TerrainError> {
        definition.validate()?;
        let base_seed =
            splitmix(body_seed ^ (u64::from(definition.seed_salt) << 32) ^ 0x4e4f_4953_4500_0001)
                as u32;
        let octaves = (0..definition.octave_count())
            .map(|k| {
                let k32 = k as i32;
                DetailOctave {
                    frequency_per_m: definition.lacunarity.powi(k32) / definition.base_wavelength_m,
                    amplitude_m: definition.amplitude_m * definition.gain.powi(k32),
                    seed: base_seed.wrapping_add(k as u32),
                }
            })
            .collect();
        Ok(Self { octaves })
    }

    pub fn octaves(&self) -> &[DetailOctave] {
        &self.octaves
    }

    /// Conservative bound of the absolute height contribution.
    pub fn bound_m(&self) -> f64 {
        self.octaves.iter().map(|o| o.amplitude_m).sum::<f64>() * GRADIENT_NOISE_BOUND
    }

    /// Band-limited fBm at body-space point `p_m` (metres from the body
    /// centre, on the reference sphere). `texel_m = None` evaluates every
    /// octave (the complete function). Returns the height and its gradient
    /// with respect to `p_m`.
    pub fn evaluate(&self, p_m: DVec3, texel_m: Option<f64>) -> (f64, DVec3) {
        let mut value = 0.0;
        let mut gradient = DVec3::ZERO;
        for octave in &self.octaves {
            let weight = texel_m.map_or(1.0, |t| octave_weight(octave.frequency_per_m, t));
            if weight <= 0.0 {
                continue;
            }
            let amplitude = octave.amplitude_m * weight;
            let (v, g) = gradient_noise(p_m * octave.frequency_per_m, octave.seed);
            value += amplitude * v;
            gradient += g * (amplitude * octave.frequency_per_m);
        }
        (value, gradient)
    }

    /// Per-octave lattice split for a node centred at `centre_m` (body space,
    /// metres) whose texels are `texel_m`. Octaves with zero band-limit weight
    /// are omitted. Fails when a lattice coordinate leaves the integer range.
    pub fn split(
        &self,
        centre_m: DVec3,
        texel_m: f64,
    ) -> Result<Vec<OctaveOrigin>, super::TerrainError> {
        let mut origins = Vec::with_capacity(self.octaves.len());
        for octave in &self.octaves {
            let weight = octave_weight(octave.frequency_per_m, texel_m);
            if weight <= 0.0 {
                continue;
            }
            let scaled = centre_m * octave.frequency_per_m;
            if !scaled.is_finite() || scaled.abs().max_element() >= MAX_LATTICE_COORDINATE {
                return Err(super::TerrainError::InvalidConfig);
            }
            let base = scaled.floor();
            let fraction = scaled - base;
            origins.push(OctaveOrigin {
                cell: base.to_array().map(|c| c as i32),
                seed: octave.seed,
                fraction: fraction.to_array().map(|f| f as f32),
                frequency_per_m: octave.frequency_per_m as f32,
                amplitude_m: (octave.amplitude_m * weight) as f32,
            });
        }
        Ok(origins)
    }
}

/// Band-limit weight of an octave at `frequency_per_m` for texels of `texel_m`
/// (§9.3): full below half the sampling limit `f_max = 0.5 / texel`, zero at
/// and above it, smoothstep in between. A child node (half the texel) keeps
/// every octave its parent shows and fades in the next band.
pub fn octave_weight(frequency_per_m: f64, texel_m: f64) -> f64 {
    if texel_m <= 0.0 {
        return 1.0;
    }
    let f_max = 0.5 / texel_m;
    let t = ((frequency_per_m - 0.5 * f_max) / (0.5 * f_max)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// PCG3D integer hash (Jarzynski & Olano 2020; pipeline App. A.1).
pub fn pcg3d(input: [u32; 3]) -> [u32; 3] {
    let mut v = input.map(|x| x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v = v.map(|x| x ^ (x >> 16));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v
}

/// Raw lattice hash of a signed cell under `seed`.
pub fn lattice_bits(cell: [i32; 3], seed: u32) -> [u32; 3] {
    pcg3d([
        (cell[0] as u32) ^ seed,
        (cell[1] as u32) ^ seed.wrapping_mul(747_796_405),
        (cell[2] as u32) ^ seed.wrapping_mul(2_891_336_453),
    ])
}

/// Unit lattice gradient of a cell. Near-zero raw vectors (probability about
/// 1e-9) fall back to +X on both CPU and GPU.
pub fn lattice_gradient(cell: [i32; 3], seed: u32) -> DVec3 {
    let h = lattice_bits(cell, seed);
    let raw = DVec3::new(
        f64::from(h[0]) / 4_294_967_295.0,
        f64::from(h[1]) / 4_294_967_295.0,
        f64::from(h[2]) / 4_294_967_295.0,
    ) * 2.0
        - DVec3::ONE;
    let length_squared = raw.length_squared();
    if length_squared < 1.0e-6 {
        DVec3::X
    } else {
        raw / length_squared.sqrt()
    }
}

/// Quintic-interpolated gradient noise at lattice coordinate `q`, with its
/// gradient with respect to `q`.
pub fn gradient_noise(q: DVec3, seed: u32) -> (f64, DVec3) {
    let base = q.floor();
    let cell = base.to_array().map(|c| c as i32);
    noise_from_split(cell, q - base, seed)
}

/// Gradient noise given the integer cell and the in-cell position `t`.
fn noise_from_split(cell: [i32; 3], t: DVec3, seed: u32) -> (f64, DVec3) {
    let u = t * t * t * (t * (t * 6.0 - DVec3::splat(15.0)) + DVec3::splat(10.0));
    let du = t * t * (t * (t - DVec3::splat(2.0)) + DVec3::ONE) * 30.0;
    let mut value = 0.0;
    let mut gradient = DVec3::ZERO;
    for corner in 0..8u32 {
        let c = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let offset = DVec3::new(f64::from(c[0]), f64::from(c[1]), f64::from(c[2]));
        let g = lattice_gradient(
            [
                cell[0].wrapping_add(c[0] as i32),
                cell[1].wrapping_add(c[1] as i32),
                cell[2].wrapping_add(c[2] as i32),
            ],
            seed,
        );
        let d = g.dot(t - offset);
        let pick = |axis: usize| if c[axis] == 1 { u[axis] } else { 1.0 - u[axis] };
        let slope = |axis: usize| if c[axis] == 1 { du[axis] } else { -du[axis] };
        let (wx, wy, wz) = (pick(0), pick(1), pick(2));
        let w = wx * wy * wz;
        value += w * d;
        gradient +=
            g * w + DVec3::new(slope(0) * wy * wz, wx * slope(1) * wz, wx * wy * slope(2)) * d;
    }
    (value, gradient)
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn moon_like() -> DetailNoiseDefinition {
        DetailNoiseDefinition {
            version: 1,
            seed_salt: 1,
            base_wavelength_m: 32.0,
            min_wavelength_m: 0.5,
            lacunarity: 2.0,
            gain: 0.5,
            amplitude_m: 1.0,
        }
    }

    /// f32 replica of `terrain_noise.wgsl`: split origin plus local offset.
    fn gpu_style(origins: &[OctaveOrigin], local: [f32; 3]) -> f32 {
        let mut sum = 0.0f32;
        for o in origins {
            let q: [f32; 3] = std::array::from_fn(|i| o.fraction[i] + local[i] * o.frequency_per_m);
            let fl = q.map(f32::floor);
            let cell: [i32; 3] = std::array::from_fn(|i| o.cell[i] + fl[i] as i32);
            let t = DVec3::new(
                f64::from(q[0] - fl[0]),
                f64::from(q[1] - fl[1]),
                f64::from(q[2] - fl[2]),
            );
            sum += o.amplitude_m * noise_from_split(cell, t, o.seed).0 as f32;
        }
        sum
    }

    #[test]
    fn pcg3d_mixes_single_bit_changes() {
        // Bit-exact agreement with the WGSL port is checked on a GPU in
        // `crates/app/tests/gpu_terrain_oracle.rs`.
        let a = pcg3d([1, 2, 3]);
        let b = pcg3d([1, 2, 4]);
        let flipped: u32 = a.iter().zip(&b).map(|(x, y)| (x ^ y).count_ones()).sum();
        assert!((24..=72).contains(&flipped), "{flipped}");
        assert_ne!(lattice_bits([-1, 0, 0], 1), lattice_bits([1, 0, 0], 1));
    }

    #[test]
    fn gradients_are_unit_and_noise_is_bounded_and_continuous() {
        let noise = DetailNoise::new(&moon_like(), 7).unwrap();
        let mut max_abs = 0.0f64;
        for i in 0..2000 {
            let p = DVec3::new(
                f64::from(i) * 0.37,
                f64::from(i % 17) * 1.13,
                1.0e5 + f64::from(i % 29) * 0.71,
            );
            let (v, _) = gradient_noise(p, 3);
            max_abs = max_abs.max(v.abs());
            assert!((lattice_gradient([i, -i, 3 * i], 9).length() - 1.0).abs() < 1e-12);
        }
        assert!(max_abs <= GRADIENT_NOISE_BOUND, "{max_abs}");
        assert!(max_abs > 0.3, "{max_abs}");
        // Continuity across a lattice face and analytic gradient vs differences.
        let p = DVec3::new(10.0 - 1e-9, 4.3, -7.2);
        let q = DVec3::new(10.0 + 1e-9, 4.3, -7.2);
        assert!((gradient_noise(p, 1).0 - gradient_noise(q, 1).0).abs() < 1e-7);
        let x = DVec3::new(100_000.31, 2.7, -55.9);
        let (_, g) = noise.evaluate(x, None);
        let h = 1.0e-5;
        let fd = DVec3::new(
            noise.evaluate(x + DVec3::X * h, None).0 - noise.evaluate(x - DVec3::X * h, None).0,
            noise.evaluate(x + DVec3::Y * h, None).0 - noise.evaluate(x - DVec3::Y * h, None).0,
            noise.evaluate(x + DVec3::Z * h, None).0 - noise.evaluate(x - DVec3::Z * h, None).0,
        ) / (2.0 * h);
        assert!((g - fd).length() < 1e-5 * (1.0 + g.length()), "{g} vs {fd}");
    }

    #[test]
    fn definition_validates_and_counts_octaves() {
        let d = moon_like();
        assert_eq!(d.octave_count(), 7); // 32, 16, 8, 4, 2, 1, 0.5 m
        assert!(d.validate().is_ok());
        for bad in [
            DetailNoiseDefinition { version: 2, ..d },
            DetailNoiseDefinition { gain: 1.0, ..d },
            DetailNoiseDefinition {
                lacunarity: 1.0,
                ..d
            },
            DetailNoiseDefinition {
                min_wavelength_m: 64.0,
                ..d
            },
            DetailNoiseDefinition {
                amplitude_m: f64::NAN,
                ..d
            },
            DetailNoiseDefinition {
                min_wavelength_m: 0.01,
                base_wavelength_m: 1.0e5,
                lacunarity: 1.5,
                ..d
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
        assert_ne!(
            d.configuration_identity(),
            DetailNoiseDefinition {
                amplitude_m: 1.5,
                ..d
            }
            .configuration_identity()
        );
    }

    #[test]
    fn split_lattice_f32_matches_f64_at_half_metre_wavelengths() {
        // Deep node on the larger test body: the absolute lattice coordinate at
        // 0.5 m wavelength is ~7e5, far beyond f32's sub-cell resolution.
        let noise = DetailNoise::new(&moon_like(), 11).unwrap();
        let radius = 338_950.0;
        let n0 = DVec3::new(0.018, 0.011, 0.9997).normalize();
        let centre = n0 * radius;
        let texel = 0.05; // every octave fully weighted
        let origins = noise.split(centre, texel).unwrap();
        assert_eq!(origins.len(), 7);
        let mut worst = 0.0f64;
        for i in 0..400 {
            let local = DVec3::new(
                f64::from(i % 20) * 0.731 - 7.0,
                f64::from(i / 20) * 0.517 - 5.0,
                f64::from(i % 7) * 0.013,
            );
            let reference = noise.evaluate(centre + local, Some(texel)).0;
            let split = gpu_style(&origins, local.as_vec3().to_array());
            worst = worst.max((reference - f64::from(split)).abs());
        }
        assert!(worst < 1.0e-4, "split error {worst} m");
    }

    #[test]
    fn band_limit_keeps_parent_octaves_and_fades_new_ones() {
        let noise = DetailNoise::new(&moon_like(), 5).unwrap();
        for texel in [0.03, 0.2, 1.0, 3.0, 9.0] {
            for octave in noise.octaves() {
                let child = octave_weight(octave.frequency_per_m, texel);
                let parent = octave_weight(octave.frequency_per_m, 2.0 * texel);
                // Refinement only adds detail.
                assert!(child >= parent);
                // Octaves the parent fully shows are untouched by the child.
                if parent == 1.0 {
                    assert_eq!(child, 1.0);
                }
                // The child only differs on octaves above the parent's fade
                // start, half the parent's limit 0.5 / (2 texel).
                if child != parent {
                    assert!(octave.frequency_per_m > 0.5 * (0.5 / (2.0 * texel)) - 1e-12);
                }
                // Nothing at or above the child's sampling limit.
                if octave.frequency_per_m >= 0.5 / texel {
                    assert_eq!(child, 0.0);
                }
            }
        }
    }

    #[test]
    fn page_blend_equals_per_octave_morph_fade() {
        // App. A.3 fades octaves that only exist at the child level by
        // (1 - t); the draw shader instead blends whole child and parent pages
        // by t. Both are the same linear combination of octaves: per octave,
        // weight(t) = (1 - t) * w_child + t * w_parent.
        let noise = DetailNoise::new(&moon_like(), 5).unwrap();
        let p = DVec3::new(3.0, -2.0, 109_000.0);
        let texel = 0.5;
        let child = noise.evaluate(p, Some(texel)).0;
        let parent = noise.evaluate(p, Some(2.0 * texel)).0;
        for t in [0.0, 0.25, 0.5, 1.0] {
            let blended = (1.0 - t) * child + t * parent;
            let mut per_octave = 0.0;
            for o in noise.octaves() {
                let wc = octave_weight(o.frequency_per_m, texel);
                let wp = octave_weight(o.frequency_per_m, 2.0 * texel);
                let w = (1.0 - t) * wc + t * wp;
                per_octave += o.amplitude_m * w * gradient_noise(p * o.frequency_per_m, o.seed).0;
            }
            assert!((blended - per_octave).abs() < 1e-12);
            if t == 1.0 {
                assert_eq!(blended, parent);
            }
        }
    }
}
