//! Dyadic octave ladders and per-tile fixed-point anchors (M2 Shape design §3).
//!
//! Every noise octave lives on one of four ladders with wavelengths
//! `LADDER_SCALES[m] · 2^level` metres (fixed lacunarity 2). A tile carries one
//! fixed-point anchor per ladder, `A_m = round(c / s_m · 2^11)` as i32 with the
//! f32 residual `r_m = c / s_m − A_m · 2^-11`, where `c` is the tile's chart
//! centre in body metres. The GPU producer (`terrain_noise.wgsl`
//! `ladder_split`) then derives each octave's integer lattice cell and fraction
//! exactly by arithmetic shift, so the number of octaves is unlimited and f64
//! never reaches the GPU. This module is the f64 oracle: octave seeds and
//! sub-cell offsets are integer hashes shared bit for bit with the WGSL port.
//!
//! Precision: the integer part of every octave's lattice coordinate is exact;
//! the centre's fraction carries at most about 2^-23 of a cell, and the tile's
//! local offset adds the usual f32 relative error of `local · f`. i32 anchors
//! cover chart centres within about 1,048 km of the body centre; larger bodies
//! would need a two-word anchor.
use super::TerrainError;
use super::noise::{gradient_noise, pcg3d};
use glam::{DVec3, DVec4, Vec3};

/// Wavelength scales of the four ladders: octave `(m, j)` has wavelength
/// `LADDER_SCALES[m] · 2^j` metres.
pub const LADDER_SCALES: [f64; 4] = [1.0, 1.25, 1.5, 1.75];
/// Reciprocals of `LADDER_SCALES` rounded to f32: the GPU's
/// `LADDER_INVERSE_SCALES` (multiplication is correctly rounded in WGSL,
/// division is not).
pub const LADDER_INVERSE_SCALES_F32: [f32; 4] = [1.0, 0.8, 0.666_666_7, 0.571_428_6];
/// Fraction bits of the fixed-point anchors (units of 2^-11 m · s_m).
pub const ANCHOR_FRACTION_BITS: i32 = 11;
/// Finest and coarsest levels the GPU split supports: the shift `11 + level`
/// stays within `0..=30` (wavelengths about 0.5 mm to 917 km).
pub const MIN_LADDER_LEVEL: i32 = -ANCHOR_FRACTION_BITS;
pub const MAX_LADDER_LEVEL: i32 = 19;

/// One octave: wavelength `LADDER_SCALES[ladder] · 2^level` metres.
/// `ladder` must be below 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LadderOctave {
    pub ladder: u8,
    pub level: i32,
}

impl LadderOctave {
    pub fn wavelength_m(self) -> f64 {
        LADDER_SCALES[usize::from(self.ladder)] * 2f64.powi(self.level)
    }

    /// `2^-level / s`: the multiplier from metres to lattice units.
    pub fn frequency_per_m(self) -> f64 {
        2f64.powi(-self.level) / LADDER_SCALES[usize::from(self.ladder)]
    }

    /// The next finer octave on the same ladder (half the wavelength).
    pub fn finer(self) -> Self {
        Self {
            ladder: self.ladder,
            level: self.level - 1,
        }
    }

    /// Whether the GPU split (`ladder_split`) supports this octave.
    pub fn gpu_supported(self) -> bool {
        self.ladder < 4 && (MIN_LADDER_LEVEL..=MAX_LADDER_LEVEL).contains(&self.level)
    }

    /// Lattice coordinate of body point `p_m` without the seed offset:
    /// `p / s · 2^-level`, the same rounding as the anchors (a division by
    /// the scale, then an exact power-of-two scaling).
    pub fn lattice(self, p_m: DVec3) -> DVec3 {
        p_m / LADDER_SCALES[usize::from(self.ladder)] * 2f64.powi(-self.level)
    }
}

/// Nearest ladder rung to `wavelength_m` in log distance; ties go to the
/// lower ladder index. `wavelength_m` must be positive and finite.
pub fn snap_wavelength(wavelength_m: f64) -> LadderOctave {
    let target = wavelength_m.log2();
    let mut best = LadderOctave {
        ladder: 0,
        level: 0,
    };
    let mut best_distance = f64::INFINITY;
    for (ladder, scale) in LADDER_SCALES.iter().enumerate() {
        let level = (target - scale.log2()).round() as i32;
        let octave = LadderOctave {
            ladder: ladder as u8,
            level,
        };
        let distance = (octave.wavelength_m().log2() - target).abs();
        if distance < best_distance {
            best = octave;
            best_distance = distance;
        }
    }
    best
}

/// Seed of one octave of a noise layer with seed salt `salt` (integer hash,
/// mirrored by `ladder_octave_seed` in `terrain_noise.wgsl`).
pub fn octave_seed(salt: u32, octave: LadderOctave) -> u32 {
    pcg3d([
        salt,
        0x4c41_4444 ^ u32::from(octave.ladder),
        octave.level as u32,
    ])[0]
}

/// Sub-cell lattice offset `o_seed` in [0, 1)^3 of an octave seed: 24-bit
/// fractions, exact in f32, so octaves of nested dyadic lattices never share
/// their zero points (mirrored by `ladder_octave_offset`).
pub fn octave_offset(seed: u32) -> DVec3 {
    let h = pcg3d([seed, seed ^ 0x9e37_79b9, seed ^ 0x7f4a_7c15]);
    DVec3::new(
        f64::from(h[0] >> 8),
        f64::from(h[1] >> 8),
        f64::from(h[2] >> 8),
    ) / 16_777_216.0
}

/// One octave of gradient noise at body point `p_m` (metres): value and
/// gradient with respect to `p_m`. The lattice coordinate is
/// `octave.lattice(p_m) + octave_offset(seed)`.
pub fn octave_noise3(p_m: DVec3, octave: LadderOctave, seed: u32) -> (f64, DVec3) {
    let q = octave.lattice(p_m) + octave_offset(seed);
    let (value, gradient) = gradient_noise(q, seed);
    (value, gradient * octave.frequency_per_m())
}

/// 4D variant: w is the 4th lattice coordinate (already in lattice units);
/// returns (value, d/dp_m, d/dw). The seed offset applies to the three
/// spatial coordinates only.
pub fn octave_noise4(p_m: DVec3, w: f64, octave: LadderOctave, seed: u32) -> (f64, DVec3, f64) {
    let q = octave.lattice(p_m) + octave_offset(seed);
    let (value, gradient) = gradient_noise4(q.extend(w), seed);
    (
        value,
        gradient.truncate() * octave.frequency_per_m(),
        gradient.w,
    )
}

/// Per-tile fixed-point anchors (DESIGN §3): A_m = round(c/s_m · 2^11) as
/// i32, residual r_m = c/s_m − A_m·2^-11.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileAnchors {
    pub cells: [[i32; 3]; 4],
    pub residual: [[f32; 3]; 4],
}

/// Anchor of one coordinate on ladder `ladder`: the fixed-point cell and the
/// exact f64 residual. `c / s` is rounded once; `A · 2^-11` is exact and
/// within 2^-12 of it, so the residual's subtraction is exact (Sterbenz).
fn anchor(c: f64, ladder: usize) -> Option<(i32, f64)> {
    let x = c / LADDER_SCALES[ladder];
    let fixed = (x * f64::from(1u32 << ANCHOR_FRACTION_BITS)).round();
    if !fixed.is_finite() || fixed.abs() > f64::from(i32::MAX - 1) {
        return None;
    }
    let cell = fixed as i32;
    Some((cell, x - f64::from(cell) * 2f64.powi(-ANCHOR_FRACTION_BITS)))
}

/// Anchors of a tile whose chart centre is `centre_m` (body metres, f64).
/// Fails when a fixed-point coordinate leaves i32 (|c| beyond about
/// 1,048 km on ladder 0) or the centre is not finite.
pub fn tile_anchors(centre_m: DVec3) -> Result<TileAnchors, TerrainError> {
    let mut anchors = TileAnchors {
        cells: [[0; 3]; 4],
        residual: [[0.0; 3]; 4],
    };
    for ladder in 0..4 {
        for (axis, c) in centre_m.to_array().into_iter().enumerate() {
            let (cell, residual) = anchor(c, ladder).ok_or(TerrainError::InvalidConfig)?;
            anchors.cells[ladder][axis] = cell;
            anchors.residual[ladder][axis] = residual as f32;
        }
    }
    Ok(anchors)
}

/// Integer cell `A >> (11 + level)` (arithmetic shift: an exact floor) and
/// the remainder `A − cell · 2^(11 + level)` of one anchor coordinate.
fn shift_split(anchor: i32, level: i32) -> (i32, u32) {
    let shift = (ANCHOR_FRACTION_BITS + level) as u32;
    let cell = anchor >> shift;
    (cell, (anchor as u32) & ((1u32 << shift) - 1))
}

/// Exact GPU-equivalent split of `centre·f + local·f` for one octave, for
/// tests (f64 reference): the integer cell `A >> (11 + level)` and, in f64,
/// the fraction `rem · 2^-(11 + level) + (local / s + r) · 2^-level` from the
/// stored anchors. `cell + fraction` is the octave's lattice coordinate
/// without the seed offset; the fraction may leave [0, 1) by the local part.
/// The octave must be `gpu_supported`.
pub fn ladder_split(
    anchors: &TileAnchors,
    octave: LadderOctave,
    local_m: DVec3,
) -> ([i32; 3], DVec3) {
    debug_assert!(octave.gpu_supported());
    let m = usize::from(octave.ladder);
    let shift = ANCHOR_FRACTION_BITS + octave.level;
    let dyadic = 2f64.powi(-octave.level);
    let mut cell = [0i32; 3];
    let mut fraction = DVec3::ZERO;
    for axis in 0..3 {
        let (c, rem) = shift_split(anchors.cells[m][axis], octave.level);
        cell[axis] = c;
        fraction[axis] = f64::from(rem) * 2f64.powi(-shift)
            + (local_m[axis] / LADDER_SCALES[m] + f64::from(anchors.residual[m][axis])) * dyadic;
    }
    (cell, fraction)
}

/// f32 replica of the WGSL `ladder_split` (same operation order): the cell
/// and the f32 fraction including `local`, without the seed offset.
pub fn ladder_split_f32(
    anchors: &TileAnchors,
    octave: LadderOctave,
    local_m: Vec3,
) -> ([i32; 3], Vec3) {
    debug_assert!(octave.gpu_supported());
    let m = usize::from(octave.ladder);
    let shift = ANCHOR_FRACTION_BITS + octave.level;
    let dyadic = 2f32.powi(-octave.level);
    let inverse = LADDER_INVERSE_SCALES_F32[m];
    let mut cell = [0i32; 3];
    let mut fraction = Vec3::ZERO;
    for axis in 0..3 {
        let (c, rem) = shift_split(anchors.cells[m][axis], octave.level);
        cell[axis] = c;
        // Two exact conversions (each below 2^24), one rounded sum.
        let high = (rem >> 12) as f32 * 2f32.powi(12 - shift);
        let low = (rem & 0xfff) as f32 * 2f32.powi(-shift);
        fraction[axis] =
            (high + low) + (local_m[axis] * inverse + anchors.residual[m][axis]) * dyadic;
    }
    (cell, fraction)
}

/// PCG4D integer hash (Jarzynski & Olano 2020), mirrored by `pcg4d` in
/// `terrain_noise.wgsl`.
pub fn pcg4d(input: [u32; 4]) -> [u32; 4] {
    let mut v = input.map(|x| x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    let round = |v: &mut [u32; 4]| {
        v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[3]));
        v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
        v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
        v[3] = v[3].wrapping_add(v[1].wrapping_mul(v[2]));
    };
    round(&mut v);
    v = v.map(|x| x ^ (x >> 16));
    round(&mut v);
    v
}

/// Raw 4D lattice hash of a signed cell under `seed`.
pub fn lattice_bits4(cell: [i32; 4], seed: u32) -> [u32; 4] {
    pcg4d([
        (cell[0] as u32) ^ seed,
        (cell[1] as u32) ^ seed.wrapping_mul(747_796_405),
        (cell[2] as u32) ^ seed.wrapping_mul(2_891_336_453),
        (cell[3] as u32) ^ seed.wrapping_mul(277_803_737),
    ])
}

/// Unit 4D lattice gradient of a cell; near-zero raw vectors fall back to +X
/// on both CPU and GPU.
pub fn lattice_gradient4(cell: [i32; 4], seed: u32) -> DVec4 {
    let h = lattice_bits4(cell, seed);
    let raw = DVec4::from_array(h.map(|x| f64::from(x) / 4_294_967_295.0)) * 2.0 - DVec4::ONE;
    let length_squared = raw.length_squared();
    if length_squared < 1.0e-6 {
        DVec4::X
    } else {
        raw / length_squared.sqrt()
    }
}

/// Quintic-interpolated 4D gradient noise at lattice coordinate `q`, with its
/// gradient with respect to `q`. Cells wrap to i32 like the GPU's.
pub fn gradient_noise4(q: DVec4, seed: u32) -> (f64, DVec4) {
    let base = q.floor();
    let cell = base.to_array().map(|c| c as i64 as i32);
    noise4_from_split(cell, q - base, seed)
}

/// 4D gradient noise given the integer cell and the in-cell position `t`.
pub fn noise4_from_split(cell: [i32; 4], t: DVec4, seed: u32) -> (f64, DVec4) {
    let u = t * t * t * (t * (t * 6.0 - DVec4::splat(15.0)) + DVec4::splat(10.0));
    let du = t * t * (t * (t - DVec4::splat(2.0)) + DVec4::ONE) * 30.0;
    let mut value = 0.0;
    let mut gradient = DVec4::ZERO;
    for corner in 0..16u32 {
        let c: [u32; 4] = std::array::from_fn(|axis| (corner >> axis) & 1);
        let offset = DVec4::from_array(c.map(f64::from));
        let g = lattice_gradient4(
            std::array::from_fn(|axis| cell[axis].wrapping_add(c[axis] as i32)),
            seed,
        );
        let d = g.dot(t - offset);
        let pick: [f64; 4] =
            std::array::from_fn(|axis| if c[axis] == 1 { u[axis] } else { 1.0 - u[axis] });
        let slope: [f64; 4] =
            std::array::from_fn(|axis| if c[axis] == 1 { du[axis] } else { -du[axis] });
        let w = pick[0] * pick[1] * pick[2] * pick[3];
        value += w * d;
        let dw = DVec4::new(
            slope[0] * pick[1] * pick[2] * pick[3],
            pick[0] * slope[1] * pick[2] * pick[3],
            pick[0] * pick[1] * slope[2] * pick[3],
            pick[0] * pick[1] * pick[2] * slope[3],
        );
        gradient += g * w + dw * d;
    }
    (value, gradient)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain::noise::noise_from_split;

    /// f32 replica of `ladder_noise3` in `terrain_noise.wgsl`: split, seed
    /// offset, floor, then the lattice evaluation (in f64, as the hash and
    /// gradients are what the GPU must match; value rounding is tested on a
    /// GPU in `crates/app/tests/gpu_terrain_oracle.rs`).
    pub(crate) fn gpu_style_noise3(
        anchors: &TileAnchors,
        octave: LadderOctave,
        seed: u32,
        local: Vec3,
    ) -> f64 {
        let (cell, fraction) = ladder_split_f32(anchors, octave, local);
        let q = fraction + octave_offset(seed).as_vec3();
        let fl = q.floor();
        let cell: [i32; 3] = std::array::from_fn(|i| cell[i].wrapping_add(fl[i] as i32));
        noise_from_split(cell, (q - fl).as_dvec3(), seed).0
    }

    /// Deterministic directions spread over the sphere.
    fn directions(count: usize) -> Vec<DVec3> {
        let mut state = 0x1add_e125_u64;
        let mut unit = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        (0..count)
            .map(|_| {
                let z = 2.0 * unit() - 1.0;
                let a = std::f64::consts::TAU * unit();
                let r = (1.0 - z * z).sqrt();
                DVec3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect()
    }

    #[test]
    fn snap_picks_the_nearest_rung_and_octaves_round_trip() {
        assert_eq!(
            snap_wavelength(2048.0),
            LadderOctave {
                ladder: 0,
                level: 11
            }
        );
        // Climate detail: 20 km snaps to 1.25 · 2^14 = 20,480 m.
        let climate = snap_wavelength(20_000.0);
        assert_eq!(
            climate,
            LadderOctave {
                ladder: 1,
                level: 14
            }
        );
        assert_eq!(climate.wavelength_m(), 20_480.0);
        assert_eq!(snap_wavelength(0.5).level, -1);
        assert_eq!(snap_wavelength(3.0 * 1024.0).ladder, 2);
        assert_eq!(snap_wavelength(1.75 / 64.0).ladder, 3);
        for ladder in 0..4u8 {
            for level in MIN_LADDER_LEVEL..=MAX_LADDER_LEVEL {
                let octave = LadderOctave { ladder, level };
                assert_eq!(snap_wavelength(octave.wavelength_m()), octave);
                assert_eq!(snap_wavelength(octave.wavelength_m() * 1.05), octave);
                assert!((octave.wavelength_m() * octave.frequency_per_m() - 1.0).abs() < 1e-15);
                assert_eq!(octave.finer().wavelength_m(), 0.5 * octave.wavelength_m());
            }
        }
    }

    #[test]
    fn seeds_and_offsets_decorrelate_octaves() {
        let mut seeds = std::collections::HashSet::new();
        for ladder in 0..4u8 {
            for level in MIN_LADDER_LEVEL..=MAX_LADDER_LEVEL {
                for salt in [0u32, 1, 0xc11a_7e00] {
                    let seed = octave_seed(salt, LadderOctave { ladder, level });
                    assert!(seeds.insert(seed));
                    let o = octave_offset(seed);
                    assert!(o.min_element() >= 0.0 && o.max_element() < 1.0);
                    // Exact in f32.
                    assert_eq!(o.as_vec3().as_dvec3(), o);
                }
            }
        }
    }

    #[test]
    fn anchors_reject_bodies_beyond_the_i32_range() {
        assert!(tile_anchors(DVec3::new(1.0e6, -1.0e6, 3.0)).is_ok());
        assert!(tile_anchors(DVec3::new(1.05e6, 0.0, 0.0)).is_err());
        assert!(tile_anchors(DVec3::new(f64::NAN, 0.0, 0.0)).is_err());
        let anchors = tile_anchors(DVec3::new(338_950.0, -1.0, 0.25)).unwrap();
        for m in 0..4 {
            for r in anchors.residual[m] {
                assert!(r.abs() <= 2f32.powi(-12));
            }
        }
    }

    /// DESIGN §3 / PLAN verification: the shift split equals the f64 split
    /// exactly. For many tile centres (Rust's radius and 1,000 km), every
    /// ladder and every level −11..=19: the integer cell is the exact floor
    /// of `A · 2^-(11 + level)`, and cell + remainder + residual recombine in
    /// f64 to `c / s · 2^-level` bit for bit. The GPU's f32 fraction of the
    /// centre stays within 2^-23 of a cell.
    #[test]
    fn ladder_split_equals_the_f64_split_exactly() {
        let mut checked = 0u64;
        let mut worst_f32 = 0.0f64;
        for radius in [338_950.0, 1_000_000.0] {
            for n in directions(400) {
                let centre = n * radius;
                let anchors = tile_anchors(centre).unwrap();
                for ladder in 0..4u8 {
                    let m = usize::from(ladder);
                    for level in MIN_LADDER_LEVEL..=MAX_LADDER_LEVEL {
                        let octave = LadderOctave { ladder, level };
                        let (cell, fraction) = ladder_split(&anchors, octave, DVec3::ZERO);
                        let (cell32, fraction32) = ladder_split_f32(&anchors, octave, Vec3::ZERO);
                        assert_eq!(cell, cell32);
                        let shift = ANCHOR_FRACTION_BITS + level;
                        for axis in 0..3 {
                            let a = anchors.cells[m][axis];
                            // Exact floor by integer division.
                            let floor = i64::from(a).div_euclid(1i64 << shift);
                            assert_eq!(i64::from(cell[axis]), floor);
                            let (_, rem) = shift_split(a, level);
                            assert_eq!(i64::from(rem), i64::from(a).rem_euclid(1i64 << shift));
                            // Recombination with the unrounded residual.
                            let (fixed, residual) = anchor(centre[axis], m).unwrap();
                            assert_eq!(fixed, a);
                            let reference = centre[axis] / LADDER_SCALES[m] * 2f64.powi(-level);
                            let recombined = (f64::from(cell[axis])
                                + f64::from(rem) * 2f64.powi(-shift))
                                + residual * 2f64.powi(-level);
                            assert_eq!(
                                recombined.to_bits(),
                                reference.to_bits(),
                                "{octave:?} axis {axis}: {recombined} vs {reference}"
                            );
                            // f32 centre fraction (stored residual) vs exact.
                            let exact = reference - f64::from(cell[axis]);
                            let error = (f64::from(fraction32[axis]) - exact).abs();
                            worst_f32 = worst_f32.max(error);
                            assert!((fraction[axis] - exact).abs() <= 2f64.powi(-36 + 11));
                            checked += 1;
                        }
                    }
                }
            }
        }
        println!("{checked} splits; worst f32 centre fraction error {worst_f32:.3e} cell");
        // Two roundings of a value below 2 plus the residual's f32 rounding.
        assert!(worst_f32 <= 2f64.powi(-23), "{worst_f32}");
    }

    #[test]
    fn f32_split_noise_matches_the_f64_octave_at_every_level() {
        // Tile-sized offsets: band limiting keeps f · L_tile below ~50.
        let mut worst = 0.0f64;
        for radius in [338_950.0, 1_000_000.0] {
            for n in directions(24) {
                let centre = n * radius;
                let anchors = tile_anchors(centre).unwrap();
                for ladder in 0..4u8 {
                    for level in MIN_LADDER_LEVEL..=MAX_LADDER_LEVEL {
                        let octave = LadderOctave { ladder, level };
                        let seed = octave_seed(7, octave);
                        let extent = 40.0 * octave.wavelength_m();
                        for k in 0..6 {
                            let local = DVec3::new(
                                (f64::from(k) * 0.37 - 1.0) * extent,
                                (f64::from(k) * 0.21 - 0.5) * extent,
                                f64::from(k % 3) * 0.013 * extent,
                            )
                            .as_vec3();
                            let reference =
                                octave_noise3(centre + local.as_dvec3(), octave, seed).0;
                            let split = gpu_style_noise3(&anchors, octave, seed, local);
                            worst = worst.max((reference - split).abs());
                        }
                    }
                }
            }
        }
        println!("worst f32 split vs f64 octave value: {worst:.3e}");
        // f32 lattice offsets of ~50 cells resolve to ~4e-6 of a cell.
        assert!(worst < 5.0e-5, "{worst}");
    }

    #[test]
    fn noise4_is_bounded_continuous_and_differentiable() {
        let mut max_abs = 0.0f64;
        for i in 0..4000 {
            let q = DVec4::new(
                f64::from(i) * 0.37,
                f64::from(i % 17) * 1.13 - 4.0,
                1.0e5 + f64::from(i % 29) * 0.71,
                f64::from(i % 13) * -0.53,
            );
            max_abs = max_abs.max(gradient_noise4(q, 3).0.abs());
            assert!((lattice_gradient4([i, -i, 3 * i, 7], 9).length() - 1.0).abs() < 1e-12);
        }
        assert!(max_abs <= 1.0 && max_abs > 0.3, "{max_abs}");
        // Continuity across a lattice face in w.
        let a = DVec4::new(0.3, 4.3, -7.2, 2.0 - 1e-9);
        let b = DVec4::new(0.3, 4.3, -7.2, 2.0 + 1e-9);
        assert!((gradient_noise4(a, 1).0 - gradient_noise4(b, 1).0).abs() < 1e-7);
        // Analytic derivatives against central differences, through the
        // octave wrapper (metres and the w lattice coordinate).
        let octave = LadderOctave {
            ladder: 2,
            level: 3,
        };
        let seed = octave_seed(5, octave);
        let p = DVec3::new(338_000.3, -1_234.5, 777.7);
        let w = -3.21;
        let (_, g, gw) = octave_noise4(p, w, octave, seed);
        let h = 1e-4;
        let at = |p: DVec3, w: f64| octave_noise4(p, w, octave, seed).0;
        let fd = DVec3::new(
            at(p + DVec3::X * h, w) - at(p - DVec3::X * h, w),
            at(p + DVec3::Y * h, w) - at(p - DVec3::Y * h, w),
            at(p + DVec3::Z * h, w) - at(p - DVec3::Z * h, w),
        ) / (2.0 * h);
        let fdw = (at(p, w + h) - at(p, w - h)) / (2.0 * h);
        assert!((g - fd).length() < 1e-6, "{g} vs {fd}");
        assert!((gw - fdw).abs() < 1e-6, "{gw} vs {fdw}");
        // The 3D octave's gradient too.
        let (_, g3) = octave_noise3(p, octave, seed);
        let at3 = |p: DVec3| octave_noise3(p, octave, seed).0;
        let fd3 = DVec3::new(
            at3(p + DVec3::X * h) - at3(p - DVec3::X * h),
            at3(p + DVec3::Y * h) - at3(p - DVec3::Y * h),
            at3(p + DVec3::Z * h) - at3(p - DVec3::Z * h),
        ) / (2.0 * h);
        assert!((g3 - fd3).length() < 1e-6, "{g3} vs {fd3}");
    }

    #[test]
    fn pcg4d_mixes_single_bit_changes() {
        // Bit-exact agreement with the WGSL port is checked on a GPU in
        // `crates/app/tests/gpu_terrain_oracle.rs`.
        let a = pcg4d([1, 2, 3, 4]);
        let b = pcg4d([1, 2, 3, 5]);
        let flipped: u32 = a.iter().zip(&b).map(|(x, y)| (x ^ y).count_ones()).sum();
        assert!((32..=96).contains(&flipped), "{flipped}");
        assert_ne!(
            lattice_bits4([-1, 0, 0, 0], 1),
            lattice_bits4([1, 0, 0, 0], 1)
        );
    }
}
