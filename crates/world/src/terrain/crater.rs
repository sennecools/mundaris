//! Deterministic, body-fixed compact crater profiles.
//!
//! This module supplies a bounded catalogue and analytic query helpers. It does
//! not depend on a celestial identity, renderer, or particular reference body.

use super::{TerrainError, TerrainFootprint};
use glam::DVec3;
use astrum_math::{Direction3, surface::DirectionalCap};

const MAX_FEATURES: u16 = 128;
const MIN_RADIUS_M: f64 = 64.0;
const MAX_DEPTH_FRACTION: f64 = 0.1;
const MAX_RIM_FRACTION: f64 = 0.05;
const BOWL_FLOOR_S_MAX: f64 = 0.36;
const BOWL_WALL_S_WIDTH: f64 = 0.64;
const RIM_START_S: f64 = 0.81;
const RIM_S_WIDTH: f64 = 0.40;
const SUPPORT_S_MAX: f64 = RIM_START_S + RIM_S_WIDTH;
const OUTWARD_EPS: f64 = 512.0 * f64::EPSILON;

/// Validated bounds and shape ratios for a deterministic crater catalogue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CraterFieldConfig {
    count: u16,
    min_radius_m: f64,
    max_radius_m: f64,
    depth_fraction: f64,
    rim_fraction: f64,
    absolute_height_bound_m: f64,
}

impl CraterFieldConfig {
    pub fn new(
        count: u16,
        min_radius_m: f64,
        max_radius_m: f64,
        depth_fraction: f64,
        rim_fraction: f64,
    ) -> Result<Self, TerrainError> {
        if !(1..=MAX_FEATURES).contains(&count)
            || !min_radius_m.is_finite()
            || min_radius_m < MIN_RADIUS_M
            || !max_radius_m.is_finite()
            || max_radius_m < min_radius_m
            || !depth_fraction.is_finite()
            || !(0.0..=MAX_DEPTH_FRACTION).contains(&depth_fraction)
            || depth_fraction == 0.0
            || !rim_fraction.is_finite()
            || !(0.0..=MAX_RIM_FRACTION).contains(&rim_fraction)
        {
            return Err(TerrainError::InvalidConfig);
        }

        let mut radius_sum_bound_m = 0.0;
        for index in 0..count {
            radius_sum_bound_m = outward_add(
                radius_sum_bound_m,
                radius_ceiling_m(min_radius_m, max_radius_m, index),
            );
        }
        let raw_bound = radius_sum_bound_m * (depth_fraction + rim_fraction);
        if !raw_bound.is_finite() {
            return Err(TerrainError::InvalidConfig);
        }
        let bound = (raw_bound + raw_bound * OUTWARD_EPS).next_up();
        if !bound.is_finite() {
            return Err(TerrainError::InvalidConfig);
        }

        Ok(Self {
            count,
            min_radius_m: canonical_zero(min_radius_m),
            max_radius_m: canonical_zero(max_radius_m),
            depth_fraction: canonical_zero(depth_fraction),
            rim_fraction: canonical_zero(rim_fraction),
            absolute_height_bound_m: bound,
        })
    }

    pub fn count(self) -> u16 {
        self.count
    }
    pub fn min_radius_m(self) -> f64 {
        self.min_radius_m
    }
    pub fn max_radius_m(self) -> f64 {
        self.max_radius_m
    }
    pub fn depth_fraction(self) -> f64 {
        self.depth_fraction
    }
    pub fn rim_fraction(self) -> f64 {
        self.rim_fraction
    }
    pub fn absolute_height_bound_m(self) -> f64 {
        self.absolute_height_bound_m
    }
}

/// One immutable diagnostic anchor in a compiled body-fixed crater catalogue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CraterFeature {
    center: Direction3,
    radius_m: f64,
    depth_m: f64,
    rim_height_m: f64,
    chord_squared: f64,
    support_half_angle_rad: f64,
    gradient_bound: f64,
    hessian_bound: f64,
}

impl CraterFeature {
    pub fn center(self) -> Direction3 {
        self.center
    }
    pub fn radius_m(self) -> f64 {
        self.radius_m
    }
    pub fn depth_m(self) -> f64 {
        self.depth_m
    }
    pub fn rim_height_m(self) -> f64 {
        self.rim_height_m
    }
}

/// Conservative regional certificate for a crater field query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CraterBounds {
    pub(crate) interval: [f64; 2],
    pub(crate) represented: f64,
    pub(crate) unresolved: f64,
    pub(crate) gradient: f64,
    pub(crate) hessian: f64,
}

/// Seed- and radius-specific immutable crater catalogue.
#[derive(Debug, Clone)]
pub(crate) struct CompiledCraterField {
    features: Vec<CraterFeature>,
}

impl CompiledCraterField {
    pub(crate) fn new(
        config: CraterFieldConfig,
        seed: u64,
        reference_radius_m: f64,
    ) -> Result<Self, TerrainError> {
        if !reference_radius_m.is_finite()
            || reference_radius_m <= 0.0
            || config.max_radius_m / reference_radius_m > 0.3
            || config.absolute_height_bound_m > 0.1 * reference_radius_m
        {
            return Err(TerrainError::InvalidRadius);
        }

        let mut features = Vec::with_capacity(usize::from(config.count));
        for index in 0..config.count {
            // Each attribute has an independent tagged stream, so adding or
            // changing one sampled value cannot perturb later features.
            let feature_key = splitmix64(seed ^ 0x4352_4154_4552_0001 ^ u64::from(index));
            let z = 2.0 * unit_interval(splitmix64(feature_key ^ 0x4345_4e54_4552_5a00)) - 1.0;
            let azimuth = std::f64::consts::TAU
                * unit_interval(splitmix64(feature_key ^ 0x4345_4e54_4552_5000));
            let radial = (1.0 - z * z).max(0.0).sqrt();
            let center = Direction3::try_new(DVec3::new(
                radial * azimuth.cos(),
                radial * azimuth.sin(),
                z,
            ))
            .map_err(|_| TerrainError::NonFiniteResult)?;

            let size_sample = unit_interval(splitmix64(feature_key ^ 0x5349_5a45_0000_0001));
            let ceiling_m = radius_ceiling_m(config.min_radius_m, config.max_radius_m, index);
            let radius_m = if index == 0 {
                config.max_radius_m
            } else {
                (ceiling_m * (0.65 + 0.35 * size_sample))
                    .max(config.min_radius_m)
                    .min(ceiling_m)
            };
            let depth_m = radius_m * config.depth_fraction;
            let rim_height_m = radius_m * config.rim_fraction;

            let half_chord = (radius_m / (2.0 * reference_radius_m)).sin();
            let chord_squared = (2.0 * half_chord).powi(2);
            let support_ratio = SUPPORT_S_MAX.sqrt() * half_chord;
            if !radius_m.is_finite()
                || !depth_m.is_finite()
                || !rim_height_m.is_finite()
                || !chord_squared.is_finite()
                || chord_squared <= 0.0
                || !support_ratio.is_finite()
                || support_ratio >= 1.0
            {
                return Err(TerrainError::InvalidRadius);
            }

            let support_half_angle_rad = 2.0 * support_ratio.asin();
            let profile_first_bound = 3.0 * depth_m + 12.0 * rim_height_m / RIM_S_WIDTH;
            let profile_second_bound = 15.0 * depth_m / BOWL_WALL_S_WIDTH.powi(2)
                + 96.0 * rim_height_m / RIM_S_WIDTH.powi(2);
            let ds_bound = 2.0 * SUPPORT_S_MAX.sqrt() / chord_squared.sqrt();
            let d2s_bound = 2.0 / chord_squared;
            let gradient_bound = (profile_first_bound * ds_bound * (1.0 + OUTWARD_EPS)).next_up();
            let hessian_bound = ((profile_second_bound * ds_bound * ds_bound
                + profile_first_bound * d2s_bound)
                * (1.0 + OUTWARD_EPS))
                .next_up();
            if !support_half_angle_rad.is_finite()
                || !profile_first_bound.is_finite()
                || !profile_second_bound.is_finite()
                || !gradient_bound.is_finite()
                || !hessian_bound.is_finite()
                || gradient_bound <= 0.0
            {
                return Err(TerrainError::InvalidRadius);
            }

            features.push(CraterFeature {
                center,
                radius_m,
                depth_m,
                rim_height_m,
                chord_squared,
                support_half_angle_rad,
                gradient_bound,
                hessian_bound,
            });
        }
        Ok(Self { features })
    }

    pub(crate) fn features(&self) -> &[CraterFeature] {
        &self.features
    }

    pub(crate) fn resident_heap_bytes(&self) -> usize {
        self.features.capacity() * std::mem::size_of::<CraterFeature>()
    }

    /// Landmarks are analytic terrain truth at every footprint. Their full
    /// curvature certificate, rather than an amplitude fade, drives refinement.
    pub(crate) fn weights(&self, _footprint: TerrainFootprint) -> Result<Vec<f64>, TerrainError> {
        Ok(vec![1.0; self.features.len()])
    }

    /// Evaluate the field and its ambient derivative for a unit direction.
    /// The terrain composer projects the combined gradient to the tangent plane.
    /// A short weights slice intentionally contributes only its matching prefix.
    pub(crate) fn sample(&self, n: DVec3, weights: &[f64]) -> (f64, DVec3) {
        let mut height_m = 0.0;
        let mut gradient = DVec3::ZERO;
        for (feature, &weight) in self.features.iter().zip(weights) {
            if weight == 0.0 {
                continue;
            }
            let delta = n - feature.center.unit();
            let s = delta.length_squared() / feature.chord_squared;
            if s >= SUPPORT_S_MAX {
                continue;
            }
            let (height, first) = profile(*feature, s);
            height_m += weight * height;
            let ds = delta * (2.0 / feature.chord_squared);
            gradient += ds * (weight * first);
        }
        (height_m, gradient)
    }

    /// Conservative cap certificate. The height interval encloses the complete
    /// unfiltered field; the other totals describe only intersecting supports.
    pub(crate) fn bounds(&self, cap: DirectionalCap, weights: &[f64]) -> CraterBounds {
        let mut interval_min = 0.0;
        let mut interval_max = 0.0;
        let mut represented = 0.0;
        let mut unresolved = 0.0;
        let mut gradient = 0.0;
        let mut hessian = 0.0;

        for (index, feature) in self.features.iter().enumerate() {
            if !cap_intersects(*feature, cap) {
                continue;
            }
            let global_min = -feature.depth_m;
            let global_max = feature.rim_height_m;
            let flat_floor = cap_s_max(*feature, cap) <= BOWL_FLOOR_S_MAX;
            let center_height = if flat_floor {
                -feature.depth_m
            } else {
                feature_height_at(*feature, cap.axis().unit())
            };
            // Account for rounded center/profile evaluation, then widen the
            // Lipschitz radius before intersecting with the global interval.
            let amplitude = feature.depth_m + feature.rim_height_m;
            let mut evaluation_error =
                outward_add(amplitude * OUTWARD_EPS, center_height.abs() * OUTWARD_EPS);
            if !flat_floor {
                evaluation_error =
                    outward_add(evaluation_error, feature.gradient_bound * OUTWARD_EPS);
            }
            let variation = outward_add(
                if flat_floor {
                    0.0
                } else {
                    feature.gradient_bound * cap.half_angle_rad()
                },
                evaluation_error,
            );
            let local_min = outward_subtract_lower(center_height, variation).max(global_min);
            let local_max = outward_add(center_height, variation).min(global_max);
            let (local_min, local_max) = if local_min <= local_max {
                (local_min, local_max)
            } else {
                (global_min, global_max)
            };
            let weight = weights.get(index).copied().unwrap_or(1.0).clamp(0.0, 1.0);
            // The extent encloses complete truth and the currently filtered
            // mesh, whose signed local interval may move toward zero.
            let (local_min, local_max) = if weight == 1.0 {
                (local_min, local_max)
            } else {
                (
                    local_min.min((local_min * weight).next_down()),
                    local_max.max((local_max * weight).next_up()),
                )
            };
            interval_min = outward_sum_lower(interval_min, local_min);
            interval_max = outward_add(interval_max, local_max);

            represented = outward_add(represented, amplitude * weight);
            unresolved = outward_add(unresolved, amplitude * (1.0 - weight));
            if !flat_floor {
                gradient = outward_add(gradient, feature.gradient_bound * weight);
                hessian = outward_add(hessian, feature.hessian_bound * weight);
            }
        }

        CraterBounds {
            interval: [interval_min, interval_max],
            represented,
            unresolved,
            gradient,
            hessian,
        }
    }
}

fn feature_height_at(feature: CraterFeature, n: DVec3) -> f64 {
    let delta = n - feature.center.unit();
    let s = delta.length_squared() / feature.chord_squared;
    if s >= SUPPORT_S_MAX {
        0.0
    } else {
        profile(feature, s).0
    }
}

fn profile(feature: CraterFeature, s: f64) -> (f64, f64) {
    if s >= SUPPORT_S_MAX {
        return (0.0, 0.0);
    }
    let mut height;
    let mut first = 0.0;
    if s <= BOWL_FLOOR_S_MAX {
        height = -feature.depth_m;
    } else if s < 1.0 {
        let t = (s - BOWL_FLOOR_S_MAX) / BOWL_WALL_S_WIDTH;
        let t2 = t * t;
        let t3 = t2 * t;
        let smooth_rise = 10.0 * t3 - 15.0 * t3 * t + 6.0 * t3 * t2;
        let remaining = 1.0 - t;
        height = -feature.depth_m * (1.0 - smooth_rise);
        first = 30.0 * feature.depth_m * t2 * remaining * remaining / BOWL_WALL_S_WIDTH;
    } else {
        height = 0.0;
    }
    let t = (s - RIM_START_S) / RIM_S_WIDTH;
    if (0.0..1.0).contains(&t) {
        let one_minus_t = 1.0 - t;
        let product = t * one_minus_t;
        height += 64.0 * feature.rim_height_m * product.powi(3);
        first += 192.0 * feature.rim_height_m * product.powi(2) * (1.0 - 2.0 * t) / RIM_S_WIDTH;
    }
    (height, first)
}

fn cap_s_max(feature: CraterFeature, cap: DirectionalCap) -> f64 {
    let axis = cap.axis().unit();
    let center = feature.center.unit();
    let center_angle = axis.cross(center).length().atan2(axis.dot(center));
    let angle_margin = OUTWARD_EPS * (1.0 + std::f64::consts::PI);
    let maximum_angle =
        (center_angle + cap.half_angle_rad() + angle_margin).min(std::f64::consts::PI);
    let chord = 2.0 * (maximum_angle * 0.5).sin();
    let value = chord * chord / feature.chord_squared;
    (value + value.abs() * OUTWARD_EPS).next_up()
}

fn radius_ceiling_m(min_radius_m: f64, max_radius_m: f64, index: u16) -> f64 {
    (max_radius_m / (1.0 + 0.25 * f64::from(index)).powf(1.7)).max(min_radius_m)
}

fn cap_intersects(feature: CraterFeature, cap: DirectionalCap) -> bool {
    let axis = cap.axis().unit();
    let center = feature.center.unit();
    let center_angle = axis.cross(center).length().atan2(axis.dot(center));
    let separation = (center_angle - cap.half_angle_rad()).max(0.0);
    separation
        <= feature.support_half_angle_rad + 512.0 * f64::EPSILON * (1.0 + std::f64::consts::PI)
}

fn outward_add(sum: f64, value: f64) -> f64 {
    if value == 0.0 {
        sum
    } else {
        (sum + value + (sum + value).abs() * OUTWARD_EPS).next_up()
    }
}

fn outward_subtract_lower(a: f64, b: f64) -> f64 {
    let difference = a - b;
    (difference - (a.abs() + b.abs()) * OUTWARD_EPS).next_down()
}

fn outward_sum_lower(sum: f64, value: f64) -> f64 {
    if value == 0.0 {
        sum
    } else {
        let total = sum + value;
        (total - (sum.abs() + value.abs()) * OUTWARD_EPS).next_down()
    }
}

fn canonical_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}

fn unit_interval(bits: u64) -> f64 {
    // Midpoint bins over 52 bits remain strictly inside (0, 1) after rounding.
    ((bits >> 12) as f64 + 0.5) * (1.0 / ((1u64 << 52) as f64))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CraterFieldConfig {
        CraterFieldConfig::new(4, 80.0, 640.0, 0.06, 0.02).unwrap()
    }

    fn field(seed: u64) -> CompiledCraterField {
        CompiledCraterField::new(config(), seed, 10_000.0).unwrap()
    }

    fn direction(value: DVec3) -> DVec3 {
        value.normalize()
    }

    fn tangent(center: DVec3) -> DVec3 {
        let helper = if center.x.abs() < 0.8 {
            DVec3::X
        } else {
            DVec3::Y
        };
        (helper - center * helper.dot(center)).normalize()
    }

    fn at_s(crater: CraterFeature, reference_radius_m: f64, s: f64) -> DVec3 {
        let angle = 2.0 * (s.sqrt() * (crater.radius_m / (2.0 * reference_radius_m)).sin()).asin();
        direction(crater.center.unit() * angle.cos() + tangent(crater.center.unit()) * angle.sin())
    }

    #[test]
    fn profile_has_bowl_center_rim_and_compact_exterior() {
        let field = field(7);
        let crater = field.features()[0];
        let center = crater.center.unit();
        let (center_height, _) = field.sample(center, &[1.0, 0.0, 0.0, 0.0]);
        assert!((center_height + crater.depth_m()).abs() < 1.0e-10);

        let rim_direction = at_s(crater, 10_000.0, 1.0);
        let (rim_height, _) = field.sample(rim_direction, &[1.0, 0.0, 0.0, 0.0]);
        assert!(rim_height > 0.0);

        let outside = direction(center * -1.0 + DVec3::Y * 1.0e-4);
        let (outside_height, outside_gradient) = field.sample(outside, &[1.0, 0.0, 0.0, 0.0]);
        assert_eq!(outside_height, 0.0);
        assert_eq!(outside_gradient, DVec3::ZERO);
    }

    #[test]
    fn tangent_gradient_matches_directional_finite_difference() {
        let field = field(11);
        let n = at_s(field.features()[0], 10_000.0, 0.85);
        let tangent = tangent(n);
        let (height, gradient) = field.sample(n, &[1.0; 4]);
        let step = 1.0e-6;
        let plus = direction(n + tangent * step);
        let minus = direction(n - tangent * step);
        let hp = field.sample(plus, &[1.0; 4]).0;
        let hm = field.sample(minus, &[1.0; 4]).0;
        let finite_difference = (hp - hm) / (2.0 * step);
        assert!(height.is_finite());
        assert!(
            (finite_difference - gradient.dot(tangent)).abs() < 1.0e-4,
            "finite difference {finite_difference}, analytic {}",
            gradient.dot(tangent)
        );
    }

    #[test]
    fn cap_certificates_enclose_sampled_height_gradient_and_hessian() {
        let field = field(17);
        let center = field.features()[0].center;
        let cap = DirectionalCap::new(center, 0.12).unwrap();
        let weights = [1.0; 4];
        let bounds = field.bounds(cap, &weights);
        let n = at_s(field.features()[0], 10_000.0, 0.85);
        let (height, gradient) = field.sample(n, &weights);
        assert!(bounds.interval[0] <= height && height <= bounds.interval[1]);
        assert!(gradient.length() <= bounds.gradient);

        let tangent = tangent(n);
        let step = 2.0e-5;
        let gp = field.sample(direction(n + tangent * step), &weights).1;
        let gm = field.sample(direction(n - tangent * step), &weights).1;
        let numeric_hessian = (gp - gm).length() / (2.0 * step);
        assert!(numeric_hessian <= bounds.hessian * 1.1 + 1.0e-7);
    }

    #[test]
    fn cap_height_interval_tightens_at_center_and_contains_center_rim_far_samples() {
        let field = CompiledCraterField::new(
            CraterFieldConfig::new(1, 640.0, 640.0, 0.085, 0.020).unwrap(),
            41,
            10_000.0,
        )
        .unwrap();
        let crater = field.features()[0];
        let weights = [1.0];
        let center = crater.center;
        let center_cap = DirectionalCap::new(center, 1.0e-3).unwrap();
        let wide_cap = DirectionalCap::new(center, 0.04).unwrap();
        let tight = field.bounds(center_cap, &weights).interval;
        let wide = field.bounds(wide_cap, &weights).interval;
        assert!(tight[1] - tight[0] < wide[1] - wide[0]);

        let rim = at_s(crater, 10_000.0, 1.0);
        let rim_cap = DirectionalCap::new(Direction3::try_new(rim).unwrap(), 1.0e-3).unwrap();
        let far = DirectionalCap::new(Direction3::try_new(-center.unit()).unwrap(), 0.01).unwrap();
        for (cap, directions) in [
            (center_cap, vec![center.unit()]),
            (rim_cap, vec![rim]),
            (far, vec![-center.unit()]),
        ] {
            let interval = field.bounds(cap, &weights).interval;
            for n in directions {
                let (height, _) = field.sample(n, &weights);
                assert!(interval[0] <= height && height <= interval[1]);
            }
        }
        assert_eq!(field.bounds(far, &weights).interval, [0.0; 2]);
    }

    #[test]
    fn flat_floor_caps_have_zero_derivative_bounds_but_wall_caps_do_not() {
        let mut field = CompiledCraterField::new(
            CraterFieldConfig::new(1, 640.0, 640.0, 0.085, 0.020).unwrap(),
            43,
            10_000.0,
        )
        .unwrap();
        field.features[0].center = Direction3::try_new(DVec3::Z).unwrap();
        let crater = field.features[0];
        let weights = [1.0];

        let floor_cap = DirectionalCap::new(crater.center, 0.005).unwrap();
        let floor_bounds = field.bounds(floor_cap, &weights);
        assert_eq!(floor_bounds.gradient, 0.0);
        assert_eq!(floor_bounds.hessian, 0.0);
        assert!(floor_bounds.interval[1] - floor_bounds.interval[0] < 1.0e-8);
        for radial_index in 0..=8 {
            let radial = floor_cap.half_angle_rad() * radial_index as f64 / 8.0;
            let n = DVec3::new(radial.sin(), 0.0, radial.cos());
            let (height, gradient) = field.sample(n, &weights);
            assert_eq!(height, -crater.depth_m);
            assert_eq!(gradient, DVec3::ZERO);
        }

        let wall_axis = at_s(crater, 10_000.0, BOWL_FLOOR_S_MAX);
        let wall_cap = DirectionalCap::new(Direction3::try_new(wall_axis).unwrap(), 0.001).unwrap();
        let wall_bounds = field.bounds(wall_cap, &weights);
        assert!(wall_bounds.gradient > 0.0);
        assert!(wall_bounds.hessian > 0.0);
    }

    #[test]
    fn pole_support_tangent_and_antipodal_caps_are_conservative() {
        let mut field = CompiledCraterField::new(
            CraterFieldConfig::new(1, 640.0, 640.0, 0.06, 0.02).unwrap(),
            11,
            10_000.0,
        )
        .unwrap();
        field.features[0].center = Direction3::try_new(DVec3::Z).unwrap();
        let crater = field.features[0];
        let cap_angle = 0.01;
        for axis_angle in [
            0.0,
            crater.support_half_angle_rad - cap_angle,
            crater.support_half_angle_rad + cap_angle,
            std::f64::consts::PI,
        ] {
            let axis =
                Direction3::try_new(DVec3::new(axis_angle.sin(), 0.0, axis_angle.cos())).unwrap();
            let cap = DirectionalCap::new(axis, cap_angle).unwrap();
            for footprint in [
                TerrainFootprint::COMPLETE,
                TerrainFootprint::new(50.0).unwrap(),
            ] {
                let weights = field.weights(footprint).unwrap();
                let bounds = field.bounds(cap, &weights);
                if axis_angle != 0.0 && axis_angle != std::f64::consts::PI {
                    assert!(bounds.gradient > 0.0);
                    assert!(bounds.hessian > 0.0);
                }
                let axis_unit = axis.unit();
                let tangent = tangent(axis_unit);
                let bitangent = axis_unit.cross(tangent).normalize();
                for radial_index in 0..=8 {
                    let radial = cap_angle * radial_index as f64 / 8.0;
                    for azimuth_index in 0..32 {
                        let azimuth = std::f64::consts::TAU * azimuth_index as f64 / 32.0;
                        let around = tangent * azimuth.cos() + bitangent * azimuth.sin();
                        let n = axis_unit * radial.cos() + around * radial.sin();
                        let (full, _) = field.sample(n, &[1.0]);
                        let (filtered, gradient) = field.sample(n, &weights);
                        assert!(full >= bounds.interval[0] && full <= bounds.interval[1]);
                        assert!((full - filtered).abs() <= bounds.unresolved);
                        assert!(gradient.length() <= bounds.gradient);
                    }
                }
                if axis_angle == std::f64::consts::PI {
                    assert_eq!(bounds.interval, [0.0; 2]);
                    assert_eq!(bounds.represented, 0.0);
                    assert_eq!(bounds.unresolved, 0.0);
                }
            }
        }
        assert!(unit_interval(0) > 0.0);
        assert!(unit_interval(u64::MAX) < 1.0);
    }

    #[test]
    fn profile_and_derivative_are_compact_at_support_joins() {
        let field = field(19);
        let crater = field.features()[0];
        for join in [BOWL_FLOOR_S_MAX, RIM_START_S, 1.0, SUPPORT_S_MAX] {
            let epsilon = 1.0e-7;
            let (at, first_at) = profile(crater, join);
            let (left, first_left) = profile(crater, join - epsilon);
            let (right, first_right) = profile(crater, join + epsilon);
            assert!((left - at).abs() < 1.0e-4);
            assert!((right - at).abs() < 1.0e-4);
            assert!((first_left - first_at).abs() < 1.0e-3);
            assert!((first_right - first_at).abs() < 1.0e-3);
            let second_left = (first_at - first_left) / epsilon;
            let second_right = (first_right - first_at) / epsilon;
            assert!((second_left - second_right).abs() < 1.0e-2);
        }
        let (outside_height, outside_first) = profile(crater, SUPPORT_S_MAX);
        assert_eq!(outside_height, 0.0);
        assert_eq!(outside_first, 0.0);
    }

    #[test]
    fn landmark_weights_remain_complete_at_coarse_footprints() {
        let field = field(23);
        let coarse = TerrainFootprint::new(100_000.0).unwrap();
        let complete = TerrainFootprint::COMPLETE;
        let coarse_weights = field.weights(coarse).unwrap();
        let complete_weights = field.weights(complete).unwrap();
        assert_eq!(coarse_weights, complete_weights);
        for crater in field.features() {
            let (a, _) = field.sample(crater.center.unit(), &coarse_weights);
            let (b, _) = field.sample(crater.center.unit(), &complete_weights);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn configuration_radius_and_seed_validation_are_deterministic() {
        assert!(CraterFieldConfig::new(0, 64.0, 64.0, 0.01, 0.0).is_err());
        assert!(CraterFieldConfig::new(129, 64.0, 64.0, 0.01, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 63.9, 64.0, 0.01, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 64.0, 63.0, 0.01, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 64.0, f64::INFINITY, 0.01, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 64.0, 64.0, 0.0, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 64.0, 64.0, 0.11, 0.0).is_err());
        assert!(CraterFieldConfig::new(1, 64.0, 64.0, 0.01, 0.051).is_err());

        let small_config = CraterFieldConfig::new(1, 64.0, 64.0, 0.01, 0.0).unwrap();
        assert!(CompiledCraterField::new(small_config, 1, 200.0).is_err());
        assert!(CompiledCraterField::new(small_config, 1, 0.0).is_err());

        let a = field(29);
        let same = field(29);
        let different = field(31);
        assert_eq!(a.features(), same.features());
        assert_ne!(a.features(), different.features());
        assert_eq!(a.features()[0].radius_m(), config().max_radius_m());
        let radius_sum_bound = (0..config().count())
            .map(|index| radius_ceiling_m(config().min_radius_m(), config().max_radius_m(), index))
            .sum::<f64>();
        assert!(config().absolute_height_bound_m() >= radius_sum_bound * 0.08);
    }

    #[test]
    fn hierarchy_with_128_features_respects_radius_ceilings_and_height_envelope() {
        let config = CraterFieldConfig::new(128, 64.0, 640.0, 0.085, 0.020).unwrap();
        let minimum_radius_m = config.absolute_height_bound_m() / 0.1;
        let field = CompiledCraterField::new(config, 47, minimum_radius_m * 1.01).unwrap();
        assert_eq!(field.features()[0].radius_m(), config.max_radius_m());

        let mut actual_height_sum = 0.0;
        for (index, crater) in field.features().iter().enumerate() {
            let ceiling =
                radius_ceiling_m(config.min_radius_m(), config.max_radius_m(), index as u16);
            assert!(crater.radius_m() >= config.min_radius_m());
            assert!(crater.radius_m() <= ceiling);
            actual_height_sum += crater.depth_m() + crater.rim_height_m();
        }
        assert!(actual_height_sum <= config.absolute_height_bound_m());
        assert!(config.absolute_height_bound_m() < 0.1 * minimum_radius_m * 1.01);
        assert!(CompiledCraterField::new(config, 47, minimum_radius_m * 0.99).is_err());
    }
}
