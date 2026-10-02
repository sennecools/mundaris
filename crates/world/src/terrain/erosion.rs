//! A bounded, stateless erosion-inspired feature octave.
//!
//! Each lattice feature owns a fixed body-space anchor. Its orientation is
//! sampled there (never at the query), so the returned derivative does not
//! differentiate through a recursively varying downhill direction.
use super::TerrainError;
use glam::DVec3;

const SLOPE_EPSILON: f64 = 0.0002;
const GULLY_WIDTH_FRACTION: f64 = 0.12;
/// With callback values in `[0,1]`, the partitioned field
/// is bounded by these global operator envelopes. The loose constants include
/// the quintic partition's first/second derivatives and all product terms.
// Sum |Dw| <= 2*(15/8)*sqrt(3)/cell; feature |Df| <=
// exp(-1/2)/0.12/cell. Their sum is <16/cell.
// For D²w the diagonal sum <=12 and
// mixed sum <=4*(15/8)^2; row-sum <=40.125. Product terms add
// 2*(2*15/8*sqrt(3))*(exp(-1/2)/0.12) + 1/0.12² <136,
// so 192 is conservative.
pub(super) const GRADIENT_BOUND_FACTOR: f64 = 16.0;
pub(super) const HESSIAN_BOUND_FACTOR: f64 = 192.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ErosionValue {
    pub value: f64,
    /// The callback's `value` is its fixed-anchor stack fade in `[0,1]`;
    /// `gradient` is the preceding terrain's physical gradient, used only to
    /// orient this feature and determine its slope attenuation.
    pub gradient: DVec3,
}

fn hash(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

fn feature_hash(seed: u64, lattice: [i64; 3]) -> u64 {
    let mut h = hash(seed ^ 0x9e3779b97f4a7c15);
    for (i, coordinate) in lattice.into_iter().enumerate() {
        h = hash(
            h ^ (coordinate as u64).wrapping_mul(0xd6e8feb86659fd93)
                ^ (i as u64).wrapping_mul(0xa5a3564e27f8862f),
        );
    }
    h
}

fn quintic(t: f64) -> (f64, f64) {
    let t2 = t * t;
    let t3 = t2 * t;
    (
        t3 * (t * (t * 6.0 - 15.0) + 10.0),
        30.0 * t2 * (t - 1.0) * (t - 1.0),
    )
}

/// Evaluate one deterministic rounded-gully octave in a body-fixed metre grid.
pub(super) fn octave(
    seed: u64,
    position: DVec3,
    cell_size_m: f64,
    mut orientation: impl FnMut(DVec3) -> Result<ErosionValue, TerrainError>,
) -> Result<ErosionValue, TerrainError> {
    if !position.is_finite() || !cell_size_m.is_finite() || cell_size_m <= 0.0 {
        return Err(TerrainError::InvalidConfig);
    }
    let cell = position / cell_size_m;
    if !cell.is_finite()
        || cell.min_element() < i64::MIN as f64 + 2.0
        || cell.max_element() > i64::MAX as f64 - 2.0
    {
        return Err(TerrainError::InvalidConfig);
    }
    let base = [
        cell.x.floor() as i64,
        cell.y.floor() as i64,
        cell.z.floor() as i64,
    ];
    let t = cell - DVec3::new(base[0] as f64, base[1] as f64, base[2] as f64);
    let (fade_x, dx) = quintic(t.x);
    let (fade_y, dy) = quintic(t.y);
    let (fade_z, dz) = quintic(t.z);
    let fades = [fade_x, fade_y, fade_z];
    let derivs = [dx, dy, dz];
    let mut result = ErosionValue {
        value: 0.0,
        gradient: DVec3::ZERO,
    };

    for corner in 0..8 {
        let bits = [corner & 1, (corner >> 1) & 1, (corner >> 2) & 1];
        let mut weight = 1.0;
        let mut weight_gradient = DVec3::ZERO;
        for axis in 0..3 {
            let factor = if bits[axis] == 1 {
                fades[axis]
            } else {
                1.0 - fades[axis]
            };
            weight *= factor;
        }
        for axis in 0..3 {
            let sign = if bits[axis] == 1 { 1.0 } else { -1.0 };
            let mut component = sign * derivs[axis] / cell_size_m;
            for other in 0..3 {
                if other != axis {
                    component *= if bits[other] == 1 {
                        fades[other]
                    } else {
                        1.0 - fades[other]
                    };
                }
            }
            weight_gradient[axis] = component;
        }
        if weight == 0.0 && weight_gradient == DVec3::ZERO {
            continue;
        }
        let lattice = [
            base[0] + bits[0] as i64,
            base[1] + bits[1] as i64,
            base[2] + bits[2] as i64,
        ];
        let anchor =
            DVec3::new(lattice[0] as f64, lattice[1] as f64, lattice[2] as f64) * cell_size_m;
        let hashed = feature_hash(seed, lattice);
        let offset = (hashed >> 11) as f64 / ((1u64 << 53) as f64) - 0.5;
        let downhill = orientation(anchor)?;
        if !downhill.value.is_finite()
            || !(0.0..=1.0).contains(&downhill.value)
            || !downhill.gradient.is_finite()
        {
            return Err(TerrainError::NonFiniteResult);
        }
        let radial = anchor.try_normalize();
        let slope = downhill.gradient;
        let slope_sq = slope.length_squared();
        let amplitude = if let Some(radial) = radial.filter(|_| slope_sq.is_finite()) {
            let denom = slope_sq + SLOPE_EPSILON * SLOPE_EPSILON;
            let amp = slope_sq / denom;
            let transverse = radial.cross(slope);
            if transverse.length_squared() > 0.0 {
                // Regularization makes this direction well behaved as slope tends to zero.
                amp
            } else {
                0.0
            }
        } else {
            0.0
        };
        let feature = if let Some(radial) = radial {
            // No normalization of a near-zero vector: direction and frequency
            // both fade smoothly through a strictly positive denominator.
            let tangent = radial.cross(slope) / slope.length().hypot(SLOPE_EPSILON);
            // One non-periodic line per feature, rather than infinite sinusoidal
            // stripes. Gaussian width rounds the bottom and flat ridge shoulders.
            let u = tangent.dot(position - anchor) / cell_size_m + offset;
            let width_sq = GULLY_WIDTH_FRACTION * GULLY_WIDTH_FRACTION;
            let wave = (-0.5 * u * u / width_sq).exp() * -downhill.value;
            let wave_gradient = tangent * (-wave * u / (width_sq * cell_size_m));
            ErosionValue {
                value: amplitude * wave,
                gradient: wave_gradient * amplitude,
            }
        } else {
            ErosionValue {
                value: 0.0,
                gradient: DVec3::ZERO,
            }
        };
        result.value += weight * feature.value;
        result.gradient += weight_gradient * feature.value + weight * feature.gradient;
    }
    if !result.value.is_finite() || !result.gradient.is_finite() {
        return Err(TerrainError::NonFiniteResult);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(p: DVec3, slope: DVec3) -> ErosionValue {
        octave(0x1234_5678, p, 8.0, |_| {
            Ok(ErosionValue {
                value: 0.7,
                gradient: slope,
            })
        })
        .unwrap()
    }

    #[test]
    fn analytic_gradient_matches_finite_differences_across_cells_and_axes() {
        let slope = DVec3::new(0.4, -0.7, 0.2);
        let points = [
            DVec3::new(0.1, 0.2, 0.3),
            DVec3::new(7.999, 3.0, 4.0),
            DVec3::new(8.001, 3.0, 4.0),
            DVec3::X * 12.0,
            DVec3::Y * 12.0,
            DVec3::Z * 12.0,
            DVec3::new(-8.0, -16.0, 24.0),
        ];
        let step = 1.0e-5;
        for point in points {
            let value = sample(point, slope);
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                let fd = (sample(point + axis * step, slope).value
                    - sample(point - axis * step, slope).value)
                    / (2.0 * step);
                let analytic = value.gradient.dot(axis);
                assert!(
                    (fd - analytic).abs() < 2.0e-6,
                    "{point:?}: {fd} != {analytic}"
                );
            }
        }
    }

    #[test]
    fn zero_slope_and_origin_have_no_feature_influence() {
        for p in [DVec3::ZERO, DVec3::new(4.0, 9.0, -2.0)] {
            let v = sample(p, DVec3::ZERO);
            assert_eq!(v.value, 0.0);
            assert_eq!(v.gradient, DVec3::ZERO);
        }
    }

    #[test]
    fn output_range_and_cell_boundary_continuity() {
        let slope = DVec3::new(0.1, 0.3, 0.2);
        for i in -20..=20 {
            let v = sample(DVec3::new(i as f64 * 0.7, 2.1, -3.3), slope);
            assert!((-1.0..=0.0).contains(&v.value));
            assert!(v.gradient.is_finite());
        }
        let left = sample(DVec3::new(8.0 - 1.0e-7, 3.1, 2.7), slope);
        let edge = sample(DVec3::new(8.0, 3.1, 2.7), slope);
        let right = sample(DVec3::new(8.0 + 1.0e-7, 3.1, 2.7), slope);
        assert!((left.value - right.value).abs() < 2.0e-6);
        assert!((left.gradient - right.gradient).length() < 2.0e-5);
        assert!((left.value - edge.value).abs() < 2.0e-6);
    }

    #[test]
    fn callback_fade_is_validated() {
        let bad = octave(1, DVec3::new(1.0, 2.0, 3.0), 4.0, |_| {
            Ok(ErosionValue {
                value: 1.1,
                gradient: DVec3::ZERO,
            })
        });
        assert_eq!(bad, Err(TerrainError::NonFiniteResult));
    }
}
