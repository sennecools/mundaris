//! Periodic gradient noise. This f64 implementation is the reference for the
//! f32 WGSL in `shaders/base.wgsl`; both use the same integer hash, gradient
//! table and fade so they agree within f32 rounding.

use crate::recipe::{BaseRecipe, NoiseKind, NoiseLayer};

/// Seed roles keep warp, hardness and height layers decorrelated.
pub const ROLE_HEIGHT: u32 = 0;
pub const ROLE_WARP_X: u32 = 16;
pub const ROLE_WARP_Y: u32 = 17;
pub const ROLE_HARDNESS: u32 = 18;

const DIAGONAL: f64 = std::f64::consts::FRAC_1_SQRT_2;
const GRADIENTS: [[f64; 2]; 8] = [
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [DIAGONAL, DIAGONAL],
    [-DIAGONAL, DIAGONAL],
    [DIAGONAL, -DIAGONAL],
    [-DIAGONAL, -DIAGONAL],
];

pub fn pcg(value: u32) -> u32 {
    let state = value.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    (word >> 22) ^ word
}

/// 32-bit seed for one (role, octave) pair; matches `layer_seed` in WGSL inputs.
pub fn layer_seed(recipe_seed: u64, role: u32, octave: u32) -> u32 {
    let low = recipe_seed as u32;
    let high = (recipe_seed >> 32) as u32;
    pcg(pcg(low ^ pcg(high)) ^ role.wrapping_mul(0x9e37_79b9) ^ octave.wrapping_mul(0x85eb_ca6b))
}

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn corner(ix: u32, iy: u32, seed: u32, dx: f64, dy: f64) -> f64 {
    let g = GRADIENTS[(pcg(ix ^ pcg(iy ^ pcg(seed))) & 7) as usize];
    g[0] * dx + g[1] * dy
}

/// Gradient noise of period `frequency` cells over the unit tile.
pub fn periodic_noise(u: f64, v: f64, frequency: u32, seed: u32) -> f64 {
    let f = f64::from(frequency);
    let x = u * f;
    let y = v * f;
    let fx = x.floor();
    let fy = y.floor();
    let tx = x - fx;
    let ty = y - fy;
    let ix0 = (fx as i64).rem_euclid(i64::from(frequency)) as u32;
    let iy0 = (fy as i64).rem_euclid(i64::from(frequency)) as u32;
    let ix1 = (ix0 + 1) % frequency;
    let iy1 = (iy0 + 1) % frequency;
    let n00 = corner(ix0, iy0, seed, tx, ty);
    let n10 = corner(ix1, iy0, seed, tx - 1.0, ty);
    let n01 = corner(ix0, iy1, seed, tx, ty - 1.0);
    let n11 = corner(ix1, iy1, seed, tx - 1.0, ty - 1.0);
    let sx = fade(tx);
    let sy = fade(ty);
    let a = n00 + (n10 - n00) * sx;
    let b = n01 + (n11 - n01) * sx;
    a + (b - a) * sy
}

/// Normalized octave sum: fBm in about [-0.7, 0.7], ridged in [0, 1].
pub fn layer_value(layer: &NoiseLayer, u: f64, v: f64, recipe_seed: u64, role: u32) -> f64 {
    let mut frequency = layer.frequency;
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let mut norm = 0.0;
    let mut weight = 1.0;
    for octave in 0..layer.octaves {
        let seed = layer_seed(recipe_seed, role, octave);
        let n = periodic_noise(u, v, frequency, seed);
        let contribution = match layer.kind {
            NoiseKind::Fbm => n,
            NoiseKind::Ridged => {
                let ridge = (1.0 - n.abs() * std::f64::consts::SQRT_2)
                    .clamp(0.0, 1.0)
                    .powf(layer.sharpness);
                let signal = ridge * weight;
                weight = (signal * 2.0).clamp(0.0, 1.0);
                signal
            }
        };
        total += contribution * amplitude;
        norm += amplitude;
        amplitude *= layer.gain;
        frequency *= layer.lacunarity;
    }
    total / norm
}

/// Height in metres and hardness at tile coordinates `(u, v)` in [0, 1).
pub fn base_sample(base: &BaseRecipe, recipe_seed: u64, u: f64, v: f64) -> (f64, f64) {
    let (mut wu, mut wv) = (u, v);
    if let Some(warp) = &base.warp {
        wu += warp.amplitude * layer_value(&warp.layer, u, v, recipe_seed, ROLE_WARP_X);
        wv += warp.amplitude * layer_value(&warp.layer, u, v, recipe_seed, ROLE_WARP_Y);
    }
    let mut height = 0.0;
    for (index, layer) in base.layers.iter().enumerate() {
        height +=
            layer.weight * layer_value(layer, wu, wv, recipe_seed, ROLE_HEIGHT + index as u32);
    }
    let h = &base.hardness;
    let hardness = (h.base + h.variation * layer_value(&h.layer, u, v, recipe_seed, ROLE_HARDNESS))
        .clamp(0.0, 0.95);
    (height * base.amplitude_m, hardness)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_periodic_and_zero_on_lattice() {
        for frequency in [1, 3, 8] {
            for k in 0..20 {
                let t = f64::from(k) / 20.0;
                let a = periodic_noise(t, 0.3, frequency, 7);
                let b = periodic_noise(t + 1.0, 1.3, frequency, 7);
                assert!((a - b).abs() < 1e-12);
            }
            assert!(periodic_noise(0.0, 0.0, frequency, 7).abs() < 1e-15);
        }
    }

    #[test]
    fn seeds_decorrelate_roles() {
        assert_ne!(layer_seed(1, ROLE_WARP_X, 0), layer_seed(1, ROLE_WARP_Y, 0));
        assert_ne!(layer_seed(1, ROLE_HEIGHT, 0), layer_seed(2, ROLE_HEIGHT, 0));
    }
}
