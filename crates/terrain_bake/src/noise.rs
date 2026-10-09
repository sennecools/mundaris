//! Periodic gradient noise. This f64 implementation is the reference for the
//! f32 WGSL in `shaders/base.wgsl`; both use the same integer hash, gradient
//! table and fade so they agree within f32 rounding.

use crate::recipe::{BaseRecipe, NoiseKind, NoiseLayer};

/// Seed roles keep warp, hardness and height layers decorrelated.
pub const ROLE_HEIGHT: u32 = 0;
pub const ROLE_WARP_X: u32 = 16;
pub const ROLE_WARP_Y: u32 = 17;
pub const ROLE_HARDNESS: u32 = 18;

/// Unit gradients at odd multiples of 11.25°. None is perpendicular to a lattice
/// line, so the noise never vanishes along a whole lattice edge (axis-aligned
/// gradients make straight zero lines that ridged noise turns into straight crests).
const GRADIENTS: [[f64; 2]; 16] = [
    [0.9807852804032304, 0.19509032201612825],
    [0.8314696123025452, 0.5555702330196022],
    [0.5555702330196023, 0.8314696123025452],
    [0.19509032201612833, 0.9807852804032304],
    [-0.1950903220161282, 0.9807852804032304],
    [-0.555570233019602, 0.8314696123025453],
    [-0.8314696123025453, 0.5555702330196022],
    [-0.9807852804032304, 0.1950903220161286],
    [-0.9807852804032304, -0.19509032201612836],
    [-0.8314696123025455, -0.555570233019602],
    [-0.5555702330196022, -0.8314696123025452],
    [-0.19509032201612866, -0.9807852804032303],
    [0.1950903220161283, -0.9807852804032304],
    [0.5555702330196018, -0.8314696123025455],
    [0.8314696123025452, -0.5555702330196022],
    [0.9807852804032303, -0.19509032201612872],
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

fn fade_derivative(t: f64) -> f64 {
    30.0 * t * t * (t - 1.0) * (t - 1.0)
}

fn gradient(ix: u32, iy: u32, seed: u32) -> [f64; 2] {
    GRADIENTS[(pcg(ix ^ pcg(iy ^ pcg(seed))) & 15) as usize]
}

/// Gradient noise of period `frequency` cells over the unit tile.
pub fn periodic_noise(u: f64, v: f64, frequency: u32, seed: u32) -> f64 {
    periodic_noise_d(u, v, frequency, seed).0
}

/// Gradient noise and its analytic gradient with respect to the lattice
/// coordinates `(u·frequency, v·frequency)`.
pub fn periodic_noise_d(u: f64, v: f64, frequency: u32, seed: u32) -> (f64, f64, f64) {
    let f = i64::from(frequency);
    lattice_noise_d(
        u * f64::from(frequency),
        v * f64::from(frequency),
        [[f, 0], [0, f]],
        seed,
    )
}

/// Canonical representative of lattice cell (i, j) modulo the period lattice
/// whose basis vectors are the columns of `l` (exact integer arithmetic).
fn reduce(i: i64, j: i64, l: [[i64; 2]; 2]) -> (u32, u32) {
    let det = l[0][0] * l[1][1] - l[0][1] * l[1][0];
    let floor_div = |a: i64, b: i64| {
        if b > 0 {
            a.div_euclid(b)
        } else {
            (-a).div_euclid(-b)
        }
    };
    let k0 = floor_div(l[1][1] * i - l[0][1] * j, det);
    let k1 = floor_div(-l[1][0] * i + l[0][0] * j, det);
    let ri = i - (l[0][0] * k0 + l[0][1] * k1);
    let rj = j - (l[1][0] * k0 + l[1][1] * k1);
    (ri as u32, rj as u32)
}

/// Gradient noise at lattice coordinates (x, y), periodic modulo the integer
/// lattice spanned by the columns of `l`, with its gradient in lattice units.
/// Every cell of the period parallelogram gets its own random gradient, so an
/// integer-rotated octave (|det l| = d·F²) has d·F² independent cells instead of
/// repeating F² cells d times.
pub fn lattice_noise_d(x: f64, y: f64, l: [[i64; 2]; 2], seed: u32) -> (f64, f64, f64) {
    let fx = x.floor();
    let fy = y.floor();
    let tx = x - fx;
    let ty = y - fy;
    let (cx, cy) = (fx as i64, fy as i64);
    let cell = |dx: i64, dy: i64| {
        let (i, j) = reduce(cx + dx, cy + dy, l);
        gradient(i, j, seed)
    };
    let g00 = cell(0, 0);
    let g10 = cell(1, 0);
    let g01 = cell(0, 1);
    let g11 = cell(1, 1);
    let n00 = g00[0] * tx + g00[1] * ty;
    let n10 = g10[0] * (tx - 1.0) + g10[1] * ty;
    let n01 = g01[0] * tx + g01[1] * (ty - 1.0);
    let n11 = g11[0] * (tx - 1.0) + g11[1] * (ty - 1.0);
    let sx = fade(tx);
    let sy = fade(ty);
    let a = n00 + (n10 - n00) * sx;
    let b = n01 + (n11 - n01) * sx;
    let value = a + (b - a) * sy;
    let (dsx, dsy) = (fade_derivative(tx), fade_derivative(ty));
    let da_x = g00[0] + (g10[0] - g00[0]) * sx + (n10 - n00) * dsx;
    let db_x = g01[0] + (g11[0] - g01[0]) * sx + (n11 - n01) * dsx;
    let da_y = g00[1] + (g10[1] - g00[1]) * sx;
    let db_y = g01[1] + (g11[1] - g01[1]) * sx;
    let gx = da_x + (db_x - da_x) * sy;
    let gy = da_y + (db_y - da_y) * sy + (b - a) * dsy;
    (value, gx, gy)
}

/// Normalized octave sum: fBm in about [-0.7, 0.7], ridged in [0, 1], billow in
/// about [-1, 1].
pub fn layer_value(layer: &NoiseLayer, u: f64, v: f64, recipe_seed: u64, role: u32) -> f64 {
    layer_dual(layer, u, v, recipe_seed, role).0
}

/// [`layer_value`] with its gradient with respect to the tile coordinates
/// `(u, v)`. Exact except that the slope-damping factor is treated as constant
/// (its derivative needs second derivatives of the noise).
pub fn layer_dual(
    layer: &NoiseLayer,
    u: f64,
    v: f64,
    recipe_seed: u64,
    role: u32,
) -> (f64, f64, f64) {
    let mut frequency = layer.frequency;
    // Octave domain: q = D·p, then rotated octaves map q ← (2 + i)·q, all wrapped
    // to the unit torus (exact: integer maps of a one-tile-periodic lattice).
    // `jacobian` is d(q)/d(p); lattice coordinates are frequency · q.
    let [[d00, d01], [d10, d11]] = layer
        .domain
        .unwrap_or([[1, 0], [0, 1]])
        .map(|row| row.map(f64::from));
    let (mut qu, mut qv) = (
        (d00 * u + d01 * v).rem_euclid(1.0),
        (d10 * u + d11 * v).rem_euclid(1.0),
    );
    let mut jacobian = [[d00, d01], [d10, d11]];
    let (mut du, mut dv) = (0.0f64, 0.0f64);
    let mut amplitude = 1.0;
    let mut total = 0.0;
    let (mut total_u, mut total_v) = (0.0, 0.0);
    let mut norm = 0.0;
    let mut weight = 1.0;
    let (mut weight_u, mut weight_v) = (0.0f64, 0.0f64);
    for octave in 0..layer.octaves {
        let seed = layer_seed(recipe_seed, role, octave);
        let (n, gx, gy) = periodic_noise_d(qu, qv, frequency, seed);
        let [[j00, j01], [j10, j11]] = jacobian;
        let f = f64::from(frequency);
        let (pu, pv) = (f * (j00 * gx + j10 * gy), f * (j01 * gx + j11 * gy));
        let (mut contribution, mut cu, mut cv) = match layer.kind {
            NoiseKind::Fbm => (n, pu, pv),
            NoiseKind::Ridged => {
                let base = 1.0 - n.abs() * std::f64::consts::SQRT_2;
                let ridge = base.clamp(0.0, 1.0).powf(layer.sharpness);
                let ridge_slope = if base > 0.0 && base < 1.0 {
                    -layer.sharpness
                        * base.powf(layer.sharpness - 1.0)
                        * std::f64::consts::SQRT_2
                        * n.signum()
                } else {
                    0.0
                };
                // signal = ridge · weight, with weight = clamp(2 · previous signal).
                let signal = ridge * weight;
                let su = ridge_slope * pu * weight + ridge * weight_u;
                let sv = ridge_slope * pv * weight + ridge * weight_v;
                let doubled = signal * 2.0;
                weight = doubled.clamp(0.0, 1.0);
                (weight_u, weight_v) = if doubled > 0.0 && doubled < 1.0 {
                    (2.0 * su, 2.0 * sv)
                } else {
                    (0.0, 0.0)
                };
                (signal, su, sv)
            }
            NoiseKind::Billow => {
                let k = 2.0 * std::f64::consts::SQRT_2;
                (k * n.abs() - 1.0, k * n.signum() * pu, k * n.signum() * pv)
            }
        };
        if layer.slope_damping > 0.0 {
            let s = (j00 * j11 - j01 * j10).abs().sqrt();
            du += (j00 * gx + j10 * gy) / s;
            dv += (j01 * gx + j11 * gy) / s;
            let damping = 1.0 + layer.slope_damping * (du * du + dv * dv);
            contribution /= damping;
            cu /= damping;
            cv /= damping;
        }
        total += contribution * amplitude;
        total_u += cu * amplitude;
        total_v += cv * amplitude;
        norm += amplitude;
        amplitude *= layer.gain;
        if layer.rotate_octaves {
            (qu, qv) = (
                (2.0 * qu - qv).rem_euclid(1.0),
                (qu + 2.0 * qv).rem_euclid(1.0),
            );
            let [r0, r1] = jacobian;
            jacobian = [
                [2.0 * r0[0] - r1[0], 2.0 * r0[1] - r1[1]],
                [r0[0] + 2.0 * r1[0], r0[1] + 2.0 * r1[1]],
            ];
        } else {
            frequency *= layer.lacunarity;
        }
    }
    (total / norm, total_u / norm, total_v / norm)
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
    fn analytic_gradient_matches_finite_differences() {
        let (frequency, seed, h) = (5u32, 11u32, 1.0e-6);
        for k in 0..50 {
            let u = (f64::from(k) * 0.137).fract();
            let v = (f64::from(k) * 0.291 + 0.05).fract();
            let (_, gx, gy) = periodic_noise_d(u, v, frequency, seed);
            let f = f64::from(frequency);
            let fd_x = (periodic_noise(u + h, v, frequency, seed)
                - periodic_noise(u - h, v, frequency, seed))
                / (2.0 * h * f);
            let fd_y = (periodic_noise(u, v + h, frequency, seed)
                - periodic_noise(u, v - h, frequency, seed))
                / (2.0 * h * f);
            assert!((gx - fd_x).abs() < 1e-6 && (gy - fd_y).abs() < 1e-6);
        }
    }

    #[test]
    fn rotated_octaves_stay_periodic() {
        let layer = NoiseLayer {
            kind: NoiseKind::Ridged,
            frequency: 3,
            octaves: 6,
            lacunarity: 2,
            gain: 0.5,
            weight: 1.0,
            sharpness: 2.0,
            rotate_octaves: true,
            slope_damping: 0.7,
            domain: Some([[1, 2], [-1, 1]]),
        };
        for k in 0..40 {
            let u = (f64::from(k) * 0.173).fract();
            let v = (f64::from(k) * 0.311).fract();
            let a = layer_value(&layer, u, v, 9, ROLE_HEIGHT);
            let b = layer_value(&layer, u + 1.0, v - 1.0, 9, ROLE_HEIGHT);
            assert!((a - b).abs() < 1e-9, "{a} vs {b}");
        }
    }

    #[test]
    fn seeds_decorrelate_roles() {
        assert_ne!(layer_seed(1, ROLE_WARP_X, 0), layer_seed(1, ROLE_WARP_Y, 0));
        assert_ne!(layer_seed(1, ROLE_HEIGHT, 0), layer_seed(2, ROLE_HEIGHT, 0));
    }
}
