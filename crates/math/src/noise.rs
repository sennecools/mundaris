//! Deterministic, non-periodic 3D gradient noise and conservative global bounds.
use glam::DVec3;

/// V1 full-width SplitMix64-finalizer lattice hash. Start with seed XOR the
/// golden-ratio constant, then mix each signed coordinate reinterpreted as
/// two's-complement u64 after multiplying by its fixed axis tag, in x/y/z order.
pub fn lattice_hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    fn mix(mut v: u64) -> u64 {
        v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
        v ^ (v >> 31)
    }
    let mut v = seed ^ 0x9e3779b97f4a7c15;
    v = mix(v ^ (x as u64).wrapping_mul(0xd6e8feb86659fd93));
    v = mix(v ^ (y as u64).wrapping_mul(0xa5a3564e27f8862f));
    mix(v ^ (z as u64).wrapping_mul(0x9e3779b185ebca87))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseSample {
    pub value: f64,
    pub gradient: DVec3,
}

/// Proven componentwise envelopes for the scaled primitive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoiseBounds {
    pub value_abs: f64,
    pub gradient_component_abs: f64,
    pub hessian_component_abs: f64,
}

pub const GLOBAL_BOUNDS: NoiseBounds = NoiseBounds {
    value_abs: 1.0,
    // In a coordinate derivative, the partition-of-unity gradient terms sum
    // to at most |g_i| <= 1. The differentiated weights have total absolute
    // sum <= 2 F', with F' <= 15/8 and |dot(g, t-c)| <= sqrt(3). Thus the
    // scaled bound is (1 + 2*(15/8)*sqrt(3))/sqrt(3) < 4.5.
    gradient_component_abs: 4.5,
    // For a diagonal second partial, |F''| <= 6 and the two dot-gradient
    // terms total <= 4 F'; scaled total <= 12 + 7.5/sqrt(3) < 17. For mixed
    // partials, the product-weight term is <= 4*(15/8)^2, with the two
    // dot-gradient terms <= 7.5/sqrt(3); total < 19. Publish the rounded-up
    // component envelope 20 (these are bounds on the 1/sqrt(3)-scaled field).
    hessian_component_abs: 20.0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NoiseError {
    #[error("noise domain must be finite and have a safely representable lattice cell")]
    InvalidDomain,
}

// After multiplying the table by 1/sqrt(2), each corner dot has |dot|<=sqrt(2).
// Interpolation is a convex sum, so scaling by 1/sqrt(3) leaves a strict
// sqrt(2/3)<1 envelope with ample roundoff slack, without sample clamping.
const INV_SQRT_3: f64 = 0.5773502691896258;
const GRADIENTS: [DVec3; 12] = [
    DVec3::new(1., 1., 0.),
    DVec3::new(-1., 1., 0.),
    DVec3::new(1., -1., 0.),
    DVec3::new(-1., -1., 0.),
    DVec3::new(1., 0., 1.),
    DVec3::new(-1., 0., 1.),
    DVec3::new(1., 0., -1.),
    DVec3::new(-1., 0., -1.),
    DVec3::new(0., 1., 1.),
    DVec3::new(0., -1., 1.),
    DVec3::new(0., 1., -1.),
    DVec3::new(0., -1., -1.),
];

fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn fade_d(t: f64) -> f64 {
    30.0 * t * t * (t - 1.0) * (t - 1.0)
}
/// Evaluate the fixed-order quintic gradient primitive. Coordinates are accepted
/// only when floor conversion and the adjacent corner indices remain exact-safe.
pub fn gradient_noise(seed: u64, p: DVec3) -> Result<NoiseSample, NoiseError> {
    evaluate::<true>(seed, p)
}

/// Same fixed-order value expression, omitting unneeded gradient arithmetic.
pub fn gradient_noise_value(seed: u64, p: DVec3) -> Result<f64, NoiseError> {
    Ok(evaluate::<false>(seed, p)?.value)
}

fn evaluate<const DERIVATIVES: bool>(seed: u64, p: DVec3) -> Result<NoiseSample, NoiseError> {
    if !p.is_finite() || p.abs().max_element() >= 2_147_483_646.0 {
        return Err(NoiseError::InvalidDomain);
    }
    let cell = p.floor();
    let base = cell.to_array().map(|v| v as i64);
    let t = p - cell;
    let f = t.to_array().map(fade);
    let fd = if DERIVATIVES {
        t.to_array().map(fade_d)
    } else {
        [0.0; 3]
    };
    let mut value = 0.;
    let mut gradient = DVec3::ZERO;
    // Corner order is binary xyz: x is bit 0, y bit 1, z bit 2. The
    // accumulation order is value then x/y/z gradient for each corner.
    for corner in 0..8 {
        let c = DVec3::new(
            (corner & 1) as f64,
            ((corner >> 1) & 1) as f64,
            ((corner >> 2) & 1) as f64,
        );
        let lattice = [
            base[0] + c.x as i64,
            base[1] + c.y as i64,
            base[2] + c.z as i64,
        ];
        let g = GRADIENTS[(lattice_hash(seed, lattice[0], lattice[1], lattice[2]) % 12) as usize]
            * std::f64::consts::FRAC_1_SQRT_2;
        let d = t - c;
        let dot = g.dot(d);
        let w = DVec3::new(
            if corner & 1 != 0 { f[0] } else { 1. - f[0] },
            if corner & 2 != 0 { f[1] } else { 1. - f[1] },
            if corner & 4 != 0 { f[2] } else { 1. - f[2] },
        );
        let dw = DVec3::new(
            if corner & 1 != 0 { fd[0] } else { -fd[0] },
            if corner & 2 != 0 { fd[1] } else { -fd[1] },
            if corner & 4 != 0 { fd[2] } else { -fd[2] },
        );
        let weight = w.x * w.y * w.z;
        let wp = DVec3::new(dw.x * w.y * w.z, w.x * dw.y * w.z, w.x * w.y * dw.z);
        value += weight * dot;
        if DERIVATIVES {
            gradient += wp * dot + g * weight;
        }
    }
    Ok(NoiseSample {
        value: value * INV_SQRT_3,
        gradient: gradient * INV_SQRT_3,
    })
}
