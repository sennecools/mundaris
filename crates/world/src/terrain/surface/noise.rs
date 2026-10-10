//! Integer-hashed gradient noise and band-limited fBm detail on dyadic octave
//! ladders (`ASTRUM_TERRAIN_PIPELINE.md` §4.4, §9.3, §17.1, App. A.1–A.3; M2
//! Shape design §3).
//!
//! The GPU producer (`terrain_noise.wgsl`) evaluates the same functions in f32.
//! It never sees an absolute lattice coordinate: each tile carries fixed-point
//! anchors of its chart centre ([`super::ladder::tile_anchors`]), from which
//! the GPU derives every octave's integer cell and fraction exactly, then adds
//! the f32 offset of each sample from the centre. This module is the f64 test
//! oracle for that evaluation; hashing is integer-only so both sides pick
//! identical lattice gradients, octave seeds and sub-cell offsets.
use super::ladder::{LadderOctave, octave_noise3, octave_seed, snap_wavelength};
use glam::DVec3;
use serde::{Deserialize, Serialize};

/// Largest number of octaves a definition may produce (a content sanity
/// limit; the GPU producer derives octaves from the tile anchors and keeps no
/// per-octave table).
pub const MAX_DETAIL_OCTAVES: usize = 16;
/// Conservative bound of `|gradient_noise|` for unit lattice gradients
/// (the attained maximum of 3D Perlin noise is about `sqrt(3)/2`).
pub const GRADIENT_NOISE_BOUND: f64 = 1.0;

/// Authored band-limited fBm detail layer added to a body's surface height.
/// The base wavelength snaps to the nearest dyadic ladder rung
/// ([`snap_wavelength`]); octave `k` has half the wavelength of octave `k - 1`
/// on the same ladder and amplitude `amplitude_m * gain^k`; octaves stop
/// before the wavelength drops below `min_wavelength_m`. The lacunarity must
/// be 2 (the ladders' fixed ratio).
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
            || self.lacunarity != 2.0
            || !(self.gain > 0.0 && self.gain < 1.0)
            || !(0.0..=1000.0).contains(&self.amplitude_m)
            || self.octave_count() > MAX_DETAIL_OCTAVES
        {
            return Err(super::TerrainError::InvalidConfig);
        }
        Ok(())
    }

    /// Coarsest octave: the base wavelength snapped to its ladder rung.
    pub fn first_octave(&self) -> LadderOctave {
        snap_wavelength(self.base_wavelength_m)
    }

    /// Number of ladder octaves, from the snapped base down, whose wavelength
    /// is at least `min_wavelength_m`.
    pub fn octave_count(&self) -> usize {
        let mut count = 0;
        let mut octave = self.first_octave();
        // A small relative slack keeps exact authored ratios inclusive.
        while octave.wavelength_m() >= self.min_wavelength_m * (1.0 - 1.0e-9)
            && count <= MAX_DETAIL_OCTAVES
        {
            count += 1;
            octave = octave.finer();
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
    pub octave: LadderOctave,
    pub frequency_per_m: f64,
    pub amplitude_m: f64,
    /// `octave_seed(salt, octave)`.
    pub seed: u32,
}

/// Compiled detail layer of one body: consecutive octaves of one ladder.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailNoise {
    salt: u32,
    amplitude_m: f64,
    gain: f64,
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
        let salt =
            splitmix(body_seed ^ (u64::from(definition.seed_salt) << 32) ^ 0x4e4f_4953_4500_0001)
                as u32;
        let mut octave = definition.first_octave();
        let mut octaves = Vec::with_capacity(definition.octave_count());
        for k in 0..definition.octave_count() {
            if !octave.gpu_supported() {
                return Err(super::TerrainError::InvalidConfig);
            }
            octaves.push(DetailOctave {
                octave,
                frequency_per_m: octave.frequency_per_m(),
                amplitude_m: definition.amplitude_m * definition.gain.powi(k as i32),
                seed: octave_seed(salt, octave),
            });
            octave = octave.finer();
        }
        Ok(Self {
            salt,
            amplitude_m: definition.amplitude_m,
            gain: definition.gain,
            octaves,
        })
    }

    pub fn octaves(&self) -> &[DetailOctave] {
        &self.octaves
    }

    /// Seed salt of the octave seeds (`octave_seed(salt, octave)`).
    pub fn salt(&self) -> u32 {
        self.salt
    }

    /// Amplitude of the coarsest octave; octave `k` has `amplitude · gain^k`.
    pub fn amplitude_m(&self) -> f64 {
        self.amplitude_m
    }

    pub fn gain(&self) -> f64 {
        self.gain
    }

    /// Conservative bound of the absolute height contribution.
    pub fn bound_m(&self) -> f64 {
        self.octaves.iter().map(|o| o.amplitude_m).sum::<f64>() * GRADIENT_NOISE_BOUND
    }

    /// Bound of what texels of `texel_m` leave out: `Σ amplitude·(1 − w)`
    /// over the octaves' band-limit weights `w`, times the noise bound.
    pub fn unresolved_bound_m(&self, texel_m: f64) -> f64 {
        self.octaves
            .iter()
            .map(|o| o.amplitude_m * (1.0 - octave_weight(o.frequency_per_m, texel_m)))
            .sum::<f64>()
            * GRADIENT_NOISE_BOUND
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
            let (v, g) = octave_noise3(p_m, octave.octave, octave.seed);
            value += amplitude * v;
            gradient += g * amplitude;
        }
        (value, gradient)
    }

    /// Band-limited fBm value with every octave seed xor `salt`: an
    /// independent field on the same ladder (the GPU producer mirrors it with
    /// the seed xor `salt`, including that seed's sub-cell offset).
    pub fn value_salted(&self, p_m: DVec3, texel_m: Option<f64>, salt: u32) -> f64 {
        self.octaves
            .iter()
            .map(|octave| {
                let weight = texel_m.map_or(1.0, |t| octave_weight(octave.frequency_per_m, t));
                if weight <= 0.0 {
                    return 0.0;
                }
                let (v, _) = octave_noise3(p_m, octave.octave, octave.seed ^ salt);
                octave.amplitude_m * weight * v
            })
            .sum()
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
/// gradient with respect to `q`. Cells wrap to i32 like the GPU's (fine
/// ladder octaves reach coordinates near 2^31).
pub fn gradient_noise(q: DVec3, seed: u32) -> (f64, DVec3) {
    let base = q.floor();
    let cell = base.to_array().map(|c| c as i64 as i32);
    noise_from_split(cell, q - base, seed)
}

/// Gradient noise given the integer cell and the in-cell position `t`.
pub(crate) fn noise_from_split(cell: [i32; 3], t: DVec3, seed: u32) -> (f64, DVec3) {
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
    use crate::terrain::ladder::{TileAnchors, tests::gpu_style_noise3, tile_anchors};

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

    /// f32 replica of the producer's `detail_fbm`: per octave the GPU split
    /// from the tile anchors plus the local offset.
    fn gpu_style(noise: &DetailNoise, anchors: &TileAnchors, texel: f64, local: [f32; 3]) -> f64 {
        noise
            .octaves()
            .iter()
            .map(|o| {
                let weight = octave_weight(o.frequency_per_m, texel);
                o.amplitude_m
                    * weight
                    * gpu_style_noise3(anchors, o.octave, o.seed, glam::Vec3::from_array(local))
            })
            .sum()
    }

    #[test]
    fn unresolved_bound_covers_what_a_texel_leaves_out() {
        let noise = DetailNoise::new(&moon_like(), 7).unwrap();
        assert_eq!(noise.unresolved_bound_m(1.0e-3), 0.0);
        let mut previous = 0.0;
        for texel in [0.1, 0.5, 2.0, 8.0, 64.0] {
            let bound = noise.unresolved_bound_m(texel);
            assert!(bound >= previous, "grows with the texel");
            previous = bound;
            for k in 0..200 {
                let a = f64::from(k) * 0.37;
                let p = DVec3::new(a.cos(), a.sin(), (0.1 * a).cos()).normalize() * 1.7e6;
                let full = noise.evaluate(p, None).0;
                let coarse = noise.evaluate(p, Some(texel)).0;
                assert!(
                    (full - coarse).abs() <= bound,
                    "{texel}: {} > {bound}",
                    (full - coarse).abs()
                );
            }
        }
        assert!(noise.unresolved_bound_m(1.0e4) <= noise.bound_m() * (1.0 + 1e-12));
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
        // Cells beyond i32 wrap as on the GPU instead of saturating.
        let far = DVec3::new(2.0f64.powi(31) + 0.25, 0.5, 0.5);
        let wrapped = DVec3::new(-(2.0f64.powi(31)) + 0.25, 0.5, 0.5);
        assert_eq!(gradient_noise(far, 4).0, gradient_noise(wrapped, 4).0);
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
                lacunarity: 2.5,
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
            // 98,304 m down to 1.5 · 2^-7 m: 24 ladder octaves.
            DetailNoiseDefinition {
                min_wavelength_m: 0.01,
                base_wavelength_m: 1.0e5,
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
        // Off-ladder bases snap: 20 km is 1.25 · 2^14 m, 9 octaves to 64 m.
        let climate = DetailNoiseDefinition {
            base_wavelength_m: 20_000.0,
            min_wavelength_m: 64.0,
            ..d
        };
        let noise = DetailNoise::new(&climate, 3).unwrap();
        assert_eq!(noise.octaves().len(), 9);
        assert_eq!(noise.octaves()[0].octave.wavelength_m(), 20_480.0);
        assert_eq!(noise.octaves()[8].octave.wavelength_m(), 80.0);
        for o in noise.octaves() {
            assert_eq!(o.octave.ladder, 1);
            assert_eq!(o.seed, octave_seed(noise.salt(), o.octave));
        }
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
        let anchors = tile_anchors(centre).unwrap();
        let mut worst = 0.0f64;
        for i in 0..400 {
            let local = DVec3::new(
                f64::from(i % 20) * 0.731 - 7.0,
                f64::from(i / 20) * 0.517 - 5.0,
                f64::from(i % 7) * 0.013,
            );
            let reference = noise.evaluate(centre + local, Some(texel)).0;
            let split = gpu_style(&noise, &anchors, texel, local.as_vec3().to_array());
            worst = worst.max((reference - split).abs());
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
                per_octave += o.amplitude_m * w * octave_noise3(p, o.octave, o.seed).0;
            }
            assert!((blended - per_octave).abs() < 1e-12);
            if t == 1.0 {
                assert_eq!(blended, parent);
            }
        }
    }
}
