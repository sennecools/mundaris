//! Continuous, body-fixed geological province controls.
use super::{GeologicalControls, GeologicalParameters, SurfaceAlgorithm, TerrainError, mix, unit};
use glam::{DMat3, DVec3};
use mundaris_math::noise::gradient_noise;

#[derive(Debug, Clone, Copy)]
pub(super) struct DirectedSample {
    pub controls: GeologicalControls,
    pub province_gradients: [DVec3; 4],
    pub control_gradients: [DVec3; 5],
}

#[derive(Debug, Clone)]
pub(super) struct DirectorField {
    algorithm: SurfaceAlgorithm,
    parameters: GeologicalParameters,
    seed: u64,
    rotation: DMat3,
}

impl DirectorField {
    pub fn new(algorithm: SurfaceAlgorithm, parameters: GeologicalParameters, seed: u64) -> Self {
        let salt = mix(seed ^ algorithm.code() ^ 0x4449_5245_4354_0001);
        let angle = |tag| unit(salt ^ tag) * std::f64::consts::TAU;
        let rotation = DMat3::from_rotation_z(angle(1))
            * DMat3::from_rotation_y(angle(2))
            * DMat3::from_rotation_x(angle(3))
            * DMat3::from_rotation_z(parameters.orientation_radians);
        Self {
            algorithm,
            parameters,
            seed: salt,
            rotation,
        }
    }

    pub fn evaluate(&self, direction: DVec3) -> Result<DirectedSample, TerrainError> {
        if !direction.is_finite() || direction.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        let n = direction.normalize();
        let p = self.rotation * n;
        let age0 = self.parameters.age;
        let activity0 = self.parameters.activity;
        let resurfacing0 = self.parameters.resurfacing_fraction;
        let retention0 = self.parameters.impact_retention;
        let (bias, freq) = match self.algorithm {
            SurfaceAlgorithm::RockyV4 => (
                [0.75 * age0, 0.6 * activity0, 0.75 * resurfacing0, 0.25],
                3.5,
            ),
            SurfaceAlgorithm::IcyV2 => (
                [0.45 * age0, 0.75 * activity0, 0.85 * resurfacing0, 0.1],
                4.2,
            ),
            SurfaceAlgorithm::VolcanicV2 => (
                [
                    0.8 * age0,
                    0.7 * activity0,
                    0.6 * activity0,
                    0.85 * resurfacing0,
                ],
                3.8,
            ),
            _ => return Err(TerrainError::InvalidConfig),
        };
        let mut logits = [0.0; 4];
        let mut gradients = [DVec3::ZERO; 4];
        for i in 0..4 {
            let sample = gradient_noise(
                self.seed ^ (0x5052_4f56_0000_0001_u64.wrapping_mul(i as u64 + 1)),
                p * freq,
            )
            .map_err(|_| TerrainError::NonFiniteResult)?;
            logits[i] = sample.value * 2.6 + bias[i];
            gradients[i] = self.rotation.transpose() * (sample.gradient * (freq * 2.6));
        }
        let maximum = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let exponentials = logits.map(|v| ((v - maximum) * 2.8).exp());
        let total = exponentials.iter().sum::<f64>();
        let weights = exponentials.map(|v| v / total);
        let weighted_gradient = (0..4).map(|i| gradients[i] * weights[i]).sum::<DVec3>();
        let province_gradients =
            std::array::from_fn(|i| (gradients[i] - weighted_gradient) * (2.8 * weights[i]))
                .map(|g| tangent(n, g));

        let (young_index, active_index, structure_index) = match self.algorithm {
            SurfaceAlgorithm::RockyV4 => (2, 3, 1),
            SurfaceAlgorithm::IcyV2 => (2, 1, 3),
            SurfaceAlgorithm::VolcanicV2 => (3, 1, 2),
            _ => unreachable!(),
        };
        let resurfacing_raw = resurfacing0 * 0.55 + weights[young_index] * 0.62;
        let resurfacing = resurfacing_raw.clamp(0.0, 1.0);
        let resurfacing_gradient = if resurfacing_raw == resurfacing {
            province_gradients[young_index] * 0.62
        } else {
            DVec3::ZERO
        };
        let activity_raw =
            activity0 * (0.55 + 0.9 * weights[active_index]) + 0.12 * weights[structure_index];
        let activity = activity_raw.clamp(0.0, 1.0);
        let activity_gradient = if activity_raw == activity {
            province_gradients[active_index] * (activity0 * 0.9)
                + province_gradients[structure_index] * 0.12
        } else {
            DVec3::ZERO
        };
        let age_raw = age0 * (0.72 + 0.38 * weights[0]) * (1.0 - 0.20 * resurfacing);
        let age = age_raw.clamp(0.0, 1.0);
        let age_gradient = if age_raw == age {
            province_gradients[0] * (age0 * 0.38 * (1.0 - 0.20 * resurfacing))
                - resurfacing_gradient * (age0 * (0.72 + 0.38 * weights[0]) * 0.20)
        } else {
            DVec3::ZERO
        };
        let retention_raw = retention0 * (1.0 - 0.58 * resurfacing) * (0.72 + 0.28 * weights[0]);
        let impact_retention = retention_raw.clamp(0.0, 1.0);
        let retention_gradient = if retention_raw == impact_retention {
            province_gradients[0] * (retention0 * (1.0 - 0.58 * resurfacing) * 0.28)
                - resurfacing_gradient * (retention0 * 0.58 * (0.72 + 0.28 * weights[0]))
        } else {
            DVec3::ZERO
        };
        let relief_raw = self.parameters.relief_fraction
            * (0.65 + 0.55 * weights[structure_index] + 0.2 * weights[active_index]);
        let relief_potential = relief_raw.clamp(0.0, 0.01);
        let relief_gradient = if relief_raw == relief_potential {
            province_gradients[structure_index] * (self.parameters.relief_fraction * 0.55)
                + province_gradients[active_index] * (self.parameters.relief_fraction * 0.2)
        } else {
            DVec3::ZERO
        };

        let stress = gradient_noise(self.seed ^ 0x5354_5255_4354_0001, p * 1.35)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let mut structural_direction = self.rotation.transpose() * stress.gradient;
        structural_direction = tangent(n, structural_direction);
        if structural_direction.length_squared() < 1.0e-18 {
            structural_direction = n.cross(if n.y.abs() < 0.8 { DVec3::Y } else { DVec3::X });
        }
        structural_direction = structural_direction.normalize();
        if ![
            age,
            activity,
            resurfacing,
            impact_retention,
            relief_potential,
        ]
        .iter()
        .all(|v| v.is_finite())
        {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(DirectedSample {
            controls: GeologicalControls {
                age,
                activity,
                resurfacing,
                impact_retention,
                relief_potential,
                province_weights: weights,
                process_strengths: match self.algorithm {
                    SurfaceAlgorithm::RockyV4 => [
                        impact_retention,
                        0.15 + 0.85 * weights[1],
                        resurfacing,
                        0.12 + 0.88 * weights[3],
                    ],
                    SurfaceAlgorithm::IcyV2 => [
                        impact_retention,
                        (0.18 + 0.7 * activity + 0.65 * weights[1]) / 1.53,
                        resurfacing,
                        weights[3],
                    ],
                    SurfaceAlgorithm::VolcanicV2 => [
                        impact_retention,
                        (0.08 + 0.85 * weights[1] + 0.40 * activity) / 1.33,
                        (0.12 + 0.65 * weights[2] + 0.70 * resurfacing) / 1.47,
                        resurfacing,
                    ],
                    _ => unreachable!(),
                },
                structural_direction,
            },
            province_gradients,
            control_gradients: [
                age_gradient,
                activity_gradient,
                resurfacing_gradient,
                retention_gradient,
                relief_gradient,
            ],
        })
    }
}

fn tangent(n: DVec3, v: DVec3) -> DVec3 {
    v - n * n.dot(v)
}
