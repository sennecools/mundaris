//! Versioned authored inputs for deterministic surface generation.

use super::{GeologicalParameters, SurfaceAlgorithm, TerrainError, TerrainSeed, mix, unit};
use serde::{Deserialize, Serialize};

/// A deterministic affine range sampled as `start + span * unit(seed)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffineRandomRange {
    pub start: f64,
    pub span: f64,
}

/// An affine geological control driven by normalized body age and activity,
/// with an optional independently salted random contribution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologicalAffineControl {
    pub base: f64,
    /// When set, the age coefficient multiplies `(1 - age)`.
    pub age_complement: bool,
    pub age: f64,
    pub activity: f64,
    pub random: f64,
}

/// Versioned authored distribution for one algorithm's body history.
///
/// Generation salts are deterministic and stable for this version. Ranges and
/// affine controls are content; the sampling and hash algorithms remain code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologicalDistribution {
    pub version: u32,
    pub age: AffineRandomRange,
    /// Random multiplier for `(1 - age) * activity_factor`.
    pub activity_factor: AffineRandomRange,
    pub resurfacing: GeologicalAffineControl,
    pub impact_retention: GeologicalAffineControl,
    pub relief_fraction: GeologicalAffineControl,
    pub feature_scale_fraction: AffineRandomRange,
    /// Turns in `[0, 1]`, converted to radians after sampling.
    pub orientation_turns: AffineRandomRange,
}

impl GeologicalDistribution {
    pub const VERSION: u32 = 1;

    /// The existing per-algorithm ranges expressed as authored configuration.
    pub const fn default_for(algorithm: SurfaceAlgorithm) -> Self {
        let common = Self {
            version: Self::VERSION,
            age: AffineRandomRange {
                start: 0.12,
                span: 0.86,
            },
            activity_factor: AffineRandomRange {
                start: 0.55,
                span: 0.45,
            },
            resurfacing: GeologicalAffineControl {
                base: 0.0,
                age_complement: false,
                age: 0.0,
                activity: 0.0,
                random: 0.0,
            },
            impact_retention: GeologicalAffineControl {
                base: 0.0,
                age_complement: false,
                age: 0.0,
                activity: 0.0,
                random: 0.0,
            },
            relief_fraction: GeologicalAffineControl {
                base: 0.0,
                age_complement: false,
                age: 0.0,
                activity: 0.0,
                random: 0.0,
            },
            feature_scale_fraction: AffineRandomRange {
                start: 0.0,
                span: 0.0,
            },
            orientation_turns: AffineRandomRange {
                start: 0.0,
                span: 1.0,
            },
        };
        match algorithm {
            SurfaceAlgorithm::RockyV3 | SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::RockyV5 => {
                Self {
                    resurfacing: GeologicalAffineControl {
                        base: 0.05,
                        age_complement: false,
                        age: 0.0,
                        activity: 0.78,
                        random: 0.0,
                    },
                    impact_retention: GeologicalAffineControl {
                        base: 0.25,
                        age_complement: false,
                        age: 0.75,
                        activity: 0.0,
                        random: 0.0,
                    },
                    relief_fraction: GeologicalAffineControl {
                        base: 0.004,
                        age_complement: false,
                        age: 0.005,
                        activity: 0.0,
                        random: 0.0,
                    },
                    feature_scale_fraction: AffineRandomRange {
                        start: 0.17,
                        span: 0.18,
                    },
                    ..common
                }
            }
            SurfaceAlgorithm::MoonFieldsV1 => Self {
                resurfacing: GeologicalAffineControl {
                    base: 0.08,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.34,
                    random: 0.0,
                },
                impact_retention: GeologicalAffineControl {
                    base: 0.52,
                    age_complement: false,
                    age: 0.46,
                    activity: 0.0,
                    random: 0.0,
                },
                relief_fraction: GeologicalAffineControl {
                    base: 0.005,
                    age_complement: false,
                    age: 0.003,
                    activity: 0.0,
                    random: 0.0,
                },
                feature_scale_fraction: AffineRandomRange {
                    start: 0.20,
                    span: 0.16,
                },
                ..common
            },
            SurfaceAlgorithm::MoonProfileV1 => Self {
                resurfacing: GeologicalAffineControl {
                    base: 0.0,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.0,
                    random: 0.0,
                },
                impact_retention: GeologicalAffineControl {
                    base: 1.0,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.0,
                    random: 0.0,
                },
                relief_fraction: GeologicalAffineControl {
                    base: 0.00685,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.0,
                    random: 0.0,
                },
                feature_scale_fraction: AffineRandomRange {
                    start: 0.20,
                    span: 0.16,
                },
                ..common
            },
            SurfaceAlgorithm::IcyV1 | SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::IcyV3 => Self {
                resurfacing: GeologicalAffineControl {
                    base: 0.15,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.75,
                    random: 0.0,
                },
                impact_retention: GeologicalAffineControl {
                    base: 0.10,
                    age_complement: false,
                    age: 0.65,
                    activity: 0.0,
                    random: 0.0,
                },
                relief_fraction: GeologicalAffineControl {
                    base: 0.002,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.004,
                    random: 0.0,
                },
                feature_scale_fraction: AffineRandomRange {
                    start: 0.16,
                    span: 0.18,
                },
                ..common
            },
            SurfaceAlgorithm::VolcanicV1
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3 => Self {
                resurfacing: GeologicalAffineControl {
                    base: 0.45,
                    age_complement: true,
                    age: 0.48,
                    activity: 0.0,
                    random: 0.0,
                },
                impact_retention: GeologicalAffineControl {
                    base: 0.05,
                    age_complement: false,
                    age: 0.65,
                    activity: 0.0,
                    random: 0.0,
                },
                relief_fraction: GeologicalAffineControl {
                    base: 0.003,
                    age_complement: false,
                    age: 0.0,
                    activity: 0.006,
                    random: 0.0,
                },
                feature_scale_fraction: AffineRandomRange {
                    start: 0.18,
                    span: 0.22,
                },
                ..common
            },
        }
    }

    /// Generate validated world-owned parameters from this authored content.
    pub fn generate(
        self,
        algorithm: SurfaceAlgorithm,
        seed: TerrainSeed,
    ) -> Result<GeologicalParameters, TerrainError> {
        self.validate()?;
        let salt = mix(seed.0 ^ algorithm.code() ^ 0x5048_454e_4f54_0001);
        let age = self.age.sample(unit(salt ^ 1));
        let activity = (1.0 - age) * self.activity_factor.sample(unit(salt ^ 2));
        let parameters = GeologicalParameters {
            age,
            activity,
            resurfacing_fraction: self.resurfacing.evaluate(age, activity, unit(salt ^ 5)),
            impact_retention: self
                .impact_retention
                .evaluate(age, activity, unit(salt ^ 6)),
            relief_fraction: self.relief_fraction.evaluate(age, activity, unit(salt ^ 7)),
            feature_scale_fraction: self.feature_scale_fraction.sample(unit(salt ^ 3)),
            orientation_radians: self.orientation_turns.sample(unit(salt ^ 4))
                * std::f64::consts::TAU,
        };
        parameters.validate()?;
        Ok(parameters)
    }

    pub(super) fn generate_default(
        algorithm: SurfaceAlgorithm,
        seed: TerrainSeed,
    ) -> GeologicalParameters {
        // These compile-time defaults are validated by the focused default
        // regression tests; keeping this infallible preserves the historic API.
        let distribution = Self::default_for(algorithm);
        let salt = mix(seed.0 ^ algorithm.code() ^ 0x5048_454e_4f54_0001);
        let age = distribution.age.sample(unit(salt ^ 1));
        let activity = (1.0 - age) * distribution.activity_factor.sample(unit(salt ^ 2));
        GeologicalParameters {
            age,
            activity,
            resurfacing_fraction: distribution
                .resurfacing
                .evaluate(age, activity, unit(salt ^ 5)),
            impact_retention: distribution
                .impact_retention
                .evaluate(age, activity, unit(salt ^ 6)),
            relief_fraction: distribution
                .relief_fraction
                .evaluate(age, activity, unit(salt ^ 7)),
            feature_scale_fraction: distribution.feature_scale_fraction.sample(unit(salt ^ 3)),
            orientation_radians: distribution.orientation_turns.sample(unit(salt ^ 4))
                * std::f64::consts::TAU,
        }
    }

    pub fn validate(self) -> Result<(), TerrainError> {
        if self.version != Self::VERSION
            || !self.age.valid_in(0.0, 1.0)
            || !self.activity_factor.valid_in(0.0, 1.0)
            || !self.feature_scale_fraction.valid_in(0.05, 0.8)
            || !self.orientation_turns.valid_in(0.0, 1.0)
            || !self.resurfacing.finite()
            || !self.impact_retention.finite()
            || !self.relief_fraction.finite()
            || !self.resurfacing.envelope_in(0.0, 1.0)
            || !self.impact_retention.envelope_in(0.0, 1.0)
            || !self.relief_fraction.envelope_in(0.0, 0.01)
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(())
    }

    pub fn configuration_identity(self) -> u64 {
        let mut hash = mix(0x4745_4f4c_4453_0000 ^ u64::from(self.version));
        for value in [
            self.age.start,
            self.age.span,
            self.activity_factor.start,
            self.activity_factor.span,
            self.resurfacing.base,
            if self.resurfacing.age_complement {
                1.0
            } else {
                0.0
            },
            self.resurfacing.age,
            self.resurfacing.activity,
            self.resurfacing.random,
            self.impact_retention.base,
            if self.impact_retention.age_complement {
                1.0
            } else {
                0.0
            },
            self.impact_retention.age,
            self.impact_retention.activity,
            self.impact_retention.random,
            self.relief_fraction.base,
            if self.relief_fraction.age_complement {
                1.0
            } else {
                0.0
            },
            self.relief_fraction.age,
            self.relief_fraction.activity,
            self.relief_fraction.random,
            self.feature_scale_fraction.start,
            self.feature_scale_fraction.span,
            self.orientation_turns.start,
            self.orientation_turns.span,
        ] {
            hash = mix(hash ^ identity_value_bits(value));
        }
        hash
    }

    pub(super) fn same_as_default(self, algorithm: SurfaceAlgorithm) -> bool {
        self == Self::default_for(algorithm)
    }
}

impl AffineRandomRange {
    pub(super) fn sample(self, random: f64) -> f64 {
        self.start + self.span * random
    }
    fn valid_in(self, low: f64, high: f64) -> bool {
        self.start.is_finite()
            && self.span.is_finite()
            && self.span >= 0.0
            && self.start >= low
            && self.start + self.span <= high
    }
}

impl GeologicalAffineControl {
    fn evaluate(self, age: f64, activity: f64, random: f64) -> f64 {
        let age_input = if self.age_complement { 1.0 - age } else { age };
        self.base + self.age * age_input + self.activity * activity + self.random * random
    }
    fn finite(self) -> bool {
        [self.base, self.age, self.activity, self.random]
            .iter()
            .all(|value| value.is_finite())
    }
    fn envelope_in(self, low: f64, high: f64) -> bool {
        // Conservative independent unit ranges also validate settings without
        // relying on the correlation between age and activity.
        let min = self.base + self.age.min(0.0) + self.activity.min(0.0) + self.random.min(0.0);
        let max = self.base + self.age.max(0.0) + self.activity.max(0.0) + self.random.max(0.0);
        min.is_finite() && max.is_finite() && min >= low && max <= high
    }
}

/// Physical feature scale and displacement budget for one MoonFields band.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoonFieldBandDefinition {
    pub edge_m: f64,
    pub height_budget_m: f64,
}

/// Authored process strengths and scales consumed by the fixed MoonFieldsV1
/// algorithm. Candidate enumeration and polynomial forms remain versioned code.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoonCraterProfileDefinition {
    pub bowl_base: f64,
    pub bowl_freshness: f64,
    pub rim_base: f64,
    pub rim_freshness: f64,
    pub ejecta_base: f64,
    pub ejecta_freshness: f64,
    pub degradation_offset: f64,
    pub degradation_low: f64,
    pub degradation_high: f64,
    pub degradation_strength: f64,
}

/// Optional MoonFieldsV1 authored settings. `None` on a `SurfaceDefinition`
/// means these exact V1 defaults and preserves the historical definition ID.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoonFieldDefinition {
    pub version: u32,
    pub bands: [MoonFieldBandDefinition; 3],
    pub structure_weights: [f64; 4],
    pub basin_scale: AffineRandomRange,
    pub basin_bowl_depth: AffineRandomRange,
    pub basin_rim_strength: f64,
    pub plains_offset: f64,
    pub plains_low: f64,
    pub plains_high: f64,
    pub global_relief_weights: [f64; 3],
    pub crater_regional_strength: AffineRandomRange,
    pub crater_profile: MoonCraterProfileDefinition,
}

impl MoonFieldDefinition {
    pub const VERSION: u32 = 1;

    pub const fn default_v1() -> Self {
        Self {
            version: Self::VERSION,
            bands: [
                MoonFieldBandDefinition {
                    edge_m: 12_000.0,
                    height_budget_m: 48.0,
                },
                MoonFieldBandDefinition {
                    edge_m: 256.0,
                    height_budget_m: 8.0,
                },
                MoonFieldBandDefinition {
                    edge_m: 8.0,
                    height_budget_m: 1.5,
                },
            ],
            structure_weights: [0.46, 0.18, 0.12, -0.20],
            basin_scale: AffineRandomRange {
                start: 0.36,
                span: 0.16,
            },
            basin_bowl_depth: AffineRandomRange {
                start: 0.16,
                span: 0.10,
            },
            basin_rim_strength: 0.035,
            plains_offset: 0.12,
            plains_low: -0.10,
            plains_high: 0.30,
            global_relief_weights: [0.30, 0.82, 0.12],
            crater_regional_strength: AffineRandomRange {
                start: 0.55,
                span: 0.45,
            },
            crater_profile: MoonCraterProfileDefinition {
                bowl_base: 0.18,
                bowl_freshness: 0.30,
                rim_base: 0.10,
                rim_freshness: 0.34,
                ejecta_base: 0.10,
                ejecta_freshness: 0.22,
                degradation_offset: 0.15,
                degradation_low: -0.25,
                degradation_high: 0.30,
                degradation_strength: 0.28,
            },
        }
    }

    pub fn validate(self) -> Result<(), TerrainError> {
        let profile = self.crater_profile;
        let strengths = [
            profile.bowl_base,
            profile.bowl_freshness,
            profile.rim_base,
            profile.rim_freshness,
            profile.ejecta_base,
            profile.ejecta_freshness,
            profile.degradation_strength,
            self.basin_rim_strength,
        ];
        if self.version != Self::VERSION
            || self.bands.iter().any(|band| {
                !band.edge_m.is_finite()
                    || !(1.0..=1.0e7).contains(&band.edge_m)
                    || !band.height_budget_m.is_finite()
                    || !(0.0..=10_000.0).contains(&band.height_budget_m)
            })
            || !(self.bands[0].edge_m > self.bands[1].edge_m
                && self.bands[1].edge_m > self.bands[2].edge_m)
            || self.structure_weights.iter().any(|v| !v.is_finite())
            || self
                .structure_weights
                .iter()
                .map(|value| value.abs())
                .sum::<f64>()
                > 0.96
            || !self.basin_scale.valid_in(0.36, 0.52)
            || !self.basin_bowl_depth.valid_in(0.0, 0.26)
            || !self.crater_regional_strength.valid_in(0.0, 1.0)
            || !strengths
                .iter()
                .all(|v| v.is_finite() && (0.0..=0.48).contains(v))
            || profile.bowl_base > 0.18
            || profile.bowl_freshness > 0.30
            || profile.rim_base > 0.10
            || profile.rim_freshness > 0.34
            || profile.ejecta_base > 0.10
            || profile.ejecta_freshness > 0.22
            || profile.degradation_strength > 0.28
            || self.basin_rim_strength > 0.035
            || !self.plains_offset.is_finite()
            || self.plains_offset.abs() > 0.12
            || !self.plains_low.is_finite()
            || !self.plains_high.is_finite()
            || self.plains_low >= self.plains_high
            || self.plains_low < -1.0
            || self.plains_high > 1.0
            || self
                .global_relief_weights
                .iter()
                .zip([0.30, 0.82, 0.12])
                .any(|(v, max)| !v.is_finite() || !(0.0..=max).contains(v))
            || !profile.degradation_offset.is_finite()
            || !profile.degradation_low.is_finite()
            || !profile.degradation_high.is_finite()
            || profile.degradation_low >= profile.degradation_high
            || profile.degradation_low < -1.0
            || profile.degradation_high > 1.0
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(())
    }

    pub fn configuration_identity(self) -> u64 {
        let mut hash = mix(0x4d4f_4f4e_4644_0000 ^ u64::from(self.version));
        let values = [
            self.bands[0].edge_m,
            self.bands[0].height_budget_m,
            self.bands[1].edge_m,
            self.bands[1].height_budget_m,
            self.bands[2].edge_m,
            self.bands[2].height_budget_m,
            self.structure_weights[0],
            self.structure_weights[1],
            self.structure_weights[2],
            self.structure_weights[3],
            self.basin_scale.start,
            self.basin_scale.span,
            self.basin_bowl_depth.start,
            self.basin_bowl_depth.span,
            self.basin_rim_strength,
            self.plains_offset,
            self.plains_low,
            self.plains_high,
            self.global_relief_weights[0],
            self.global_relief_weights[1],
            self.global_relief_weights[2],
            self.crater_regional_strength.start,
            self.crater_regional_strength.span,
            self.crater_profile.bowl_base,
            self.crater_profile.bowl_freshness,
            self.crater_profile.rim_base,
            self.crater_profile.rim_freshness,
            self.crater_profile.ejecta_base,
            self.crater_profile.ejecta_freshness,
            self.crater_profile.degradation_offset,
            self.crater_profile.degradation_low,
            self.crater_profile.degradation_high,
            self.crater_profile.degradation_strength,
        ];
        for value in values {
            hash = mix(hash ^ identity_value_bits(value));
        }
        hash
    }

    pub(super) fn same_as_default(self) -> bool {
        self == Self::default_v1()
    }
}

impl Default for MoonFieldDefinition {
    fn default() -> Self {
        Self::default_v1()
    }
}

fn identity_value_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn historical_parameters(
        algorithm: SurfaceAlgorithm,
        seed: TerrainSeed,
    ) -> GeologicalParameters {
        let salt = mix(seed.0 ^ algorithm.code() ^ 0x5048_454e_4f54_0001);
        let age = 0.12 + 0.86 * unit(salt ^ 1);
        let activity = (1.0 - age) * (0.55 + 0.45 * unit(salt ^ 2));
        let (resurfacing, retention, relief, scale) = match algorithm {
            SurfaceAlgorithm::RockyV3 | SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::RockyV5 => (
                0.05 + 0.78 * activity,
                0.25 + 0.75 * age,
                0.004 + 0.005 * age,
                0.17 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::MoonFieldsV1 => (
                0.08 + 0.34 * activity,
                0.52 + 0.46 * age,
                0.005 + 0.003 * age,
                0.20 + 0.16 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::MoonProfileV1 => (0.0, 1.0, 0.00685, 0.20 + 0.16 * unit(salt ^ 3)),
            SurfaceAlgorithm::IcyV1 | SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::IcyV3 => (
                0.15 + 0.75 * activity,
                0.10 + 0.65 * age,
                0.002 + 0.004 * activity,
                0.16 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::VolcanicV1
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3 => (
                0.45 + 0.48 * (1.0 - age),
                0.05 + 0.65 * age,
                0.003 + 0.006 * activity,
                0.18 + 0.22 * unit(salt ^ 3),
            ),
        };
        GeologicalParameters {
            age,
            activity,
            resurfacing_fraction: resurfacing,
            impact_retention: retention,
            relief_fraction: relief,
            feature_scale_fraction: scale,
            orientation_radians: unit(salt ^ 4) * std::f64::consts::TAU,
        }
    }

    fn assert_parameter_bits_equal(actual: GeologicalParameters, expected: GeologicalParameters) {
        let actual = [
            actual.age,
            actual.activity,
            actual.resurfacing_fraction,
            actual.impact_retention,
            actual.relief_fraction,
            actual.feature_scale_fraction,
            actual.orientation_radians,
        ];
        let expected = [
            expected.age,
            expected.activity,
            expected.resurfacing_fraction,
            expected.impact_retention,
            expected.relief_fraction,
            expected.feature_scale_fraction,
            expected.orientation_radians,
        ];
        for (field, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
            assert_eq!(actual.to_bits(), expected.to_bits(), "field={field}");
        }
    }

    #[test]
    fn default_distributions_preserve_historical_parameter_bits() {
        let algorithms = [
            SurfaceAlgorithm::RockyV3,
            SurfaceAlgorithm::RockyV4,
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::MoonFieldsV1,
            SurfaceAlgorithm::MoonProfileV1,
            SurfaceAlgorithm::IcyV1,
            SurfaceAlgorithm::IcyV2,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV1,
            SurfaceAlgorithm::VolcanicV2,
            SurfaceAlgorithm::VolcanicV3,
        ];
        for algorithm in algorithms {
            for seed in 0..128 {
                let seed = TerrainSeed(seed);
                let expected = historical_parameters(algorithm, seed);
                assert_parameter_bits_equal(
                    GeologicalDistribution::default_for(algorithm)
                        .generate(algorithm, seed)
                        .unwrap(),
                    expected,
                );
                assert_parameter_bits_equal(
                    GeologicalParameters::generated(algorithm, seed),
                    expected,
                );
            }
        }
    }

    #[test]
    fn authored_distribution_changes_generated_parameters_and_identity() {
        use super::super::{
            ShapeDefinition, SurfaceDefinition, SurfaceMaterialDefinition, SurfaceMaterialVersion,
            SurfaceTerrainDefinition,
        };

        let algorithm = SurfaceAlgorithm::MoonFieldsV1;
        let seed = TerrainSeed(77);
        let identity = super::super::super::TerrainIdentity(55);
        let mut distribution = GeologicalDistribution::default_for(algorithm);
        distribution.relief_fraction.base += 0.0001;
        let default = SurfaceDefinition::generated(identity, seed, algorithm);
        let authored = default
            .clone()
            .with_geological_distribution(distribution)
            .unwrap();
        assert_ne!(
            default.terrain().parameters(),
            authored.terrain().parameters()
        );
        assert_ne!(default.terrain_identity(), authored.terrain_identity());

        let explicit_default = SurfaceTerrainDefinition::generated_with_distribution(
            algorithm,
            seed,
            GeologicalDistribution::default_for(algorithm),
        )
        .unwrap();
        assert_eq!(explicit_default.distribution(), None);
        let material = SurfaceMaterialDefinition::new(
            SurfaceMaterialVersion::RockyV2,
            default.material().composition(),
            default.material().regional_contrast(),
        )
        .unwrap();
        let explicit_default = SurfaceDefinition::new(
            identity,
            seed,
            ShapeDefinition::sphere(),
            explicit_default,
            material,
            default.atmosphere(),
        )
        .unwrap();
        assert_eq!(
            default.terrain_identity(),
            explicit_default.terrain_identity()
        );
    }

    #[test]
    fn malformed_distribution_ranges_and_versions_are_rejected() {
        let algorithm = SurfaceAlgorithm::MoonFieldsV1;
        let mut invalid = GeologicalDistribution::default_for(algorithm);
        invalid.age.span = f64::NAN;
        assert!(invalid.generate(algorithm, TerrainSeed(1)).is_err());
        let mut invalid = GeologicalDistribution::default_for(algorithm);
        invalid.version += 1;
        assert!(invalid.generate(algorithm, TerrainSeed(1)).is_err());
        let mut invalid = GeologicalDistribution::default_for(algorithm);
        invalid.relief_fraction.base = 0.02;
        assert!(invalid.generate(algorithm, TerrainSeed(1)).is_err());
    }

    #[test]
    fn moon_field_defaults_are_identity_neutral_and_authored_values_are_validated() {
        use super::super::super::{TerrainIdentity, TerrainSeed};
        use super::super::SurfaceGenerator;
        use super::super::{SurfaceAlgorithm, SurfaceDefinition};
        use glam::DVec3;
        use mundaris_math::{Direction3, surface::SurfaceLocation};

        let definition = SurfaceDefinition::generated(
            TerrainIdentity(55),
            TerrainSeed(77),
            SurfaceAlgorithm::MoonFieldsV1,
        );
        let default_identity = definition.terrain_identity();
        let explicit_default = definition
            .clone()
            .with_moon_fields(MoonFieldDefinition::default_v1())
            .unwrap();
        assert_eq!(definition, explicit_default);
        assert_eq!(default_identity, explicit_default.terrain_identity());
        let mut changed = MoonFieldDefinition::default_v1();
        changed.global_relief_weights[0] -= 0.05;
        let changed = definition.clone().with_moon_fields(changed).unwrap();
        assert_ne!(default_identity, changed.terrain_identity());
        let location =
            SurfaceLocation::new(Direction3::try_new(DVec3::new(0.3, 0.5, 0.8)).unwrap());
        let baseline_sample = SurfaceGenerator::new(&definition, 1_737_400.0)
            .unwrap()
            .evaluate_point(location)
            .unwrap();
        let changed_sample = SurfaceGenerator::new(&changed, 1_737_400.0)
            .unwrap()
            .evaluate_point(location)
            .unwrap();
        assert_ne!(baseline_sample.terrain(), changed_sample.terrain());
        let mut malformed = MoonFieldDefinition::default_v1();
        malformed.bands[1].edge_m = f64::NAN;
        assert!(definition.clone().with_moon_fields(malformed).is_err());
        let mut malformed = MoonFieldDefinition::default_v1();
        malformed.bands[0].edge_m = 8.0;
        assert!(definition.clone().with_moon_fields(malformed).is_err());
        let wrong_algorithm = SurfaceDefinition::generated(
            TerrainIdentity(55),
            TerrainSeed(77),
            SurfaceAlgorithm::RockyV5,
        );
        assert!(
            wrong_algorithm
                .with_moon_fields(MoonFieldDefinition::default_v1())
                .is_err()
        );
    }
}
