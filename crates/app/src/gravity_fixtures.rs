//! Initial conditions only: all subsequent translation comes from mutual gravity.
use anyhow::Result;
use glam::DVec3;
use mundaris_math::*;
use mundaris_simulation::GRAVITATIONAL_CONSTANT_M3_KG_S2 as G;
use mundaris_world::*;
use std::num::NonZeroU64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GravityFixture {
    Circular,
    Hierarchy,
    GameplaySolarSystem,
    RealSolarSystem,
}
impl GravityFixture {
    pub fn fixed_step_s(self) -> f64 {
        match self {
            Self::Circular => 10.0,
            Self::Hierarchy => 60.0,
            Self::GameplaySolarSystem | Self::RealSolarSystem => 60.0,
        }
    }
    pub fn trail_stride(self) -> u64 {
        match self {
            Self::Circular => 8,
            Self::Hierarchy => 64,
            Self::GameplaySolarSystem | Self::RealSolarSystem => 64,
        }
    }
    pub fn create(self, namespace: NonZeroU64) -> Result<CelestialSystem> {
        match self {
            Self::GameplaySolarSystem => {
                return crate::solar_system::SolarSystemPreset::gameplay().create(namespace);
            }
            Self::RealSolarSystem => {
                return crate::solar_system::SolarSystemPreset::real_scale().create(namespace);
            }
            _ => {}
        }
        let mut system = CelestialSystem::new(namespace, SimulationInstant::ZERO);
        let (names, masses, radii, positions, velocities) = match self {
            Self::Circular => {
                let mass = 1e24;
                let small = 1e20;
                let r = 1e7;
                let total = mass + small;
                let v = (G * total / r).sqrt();
                (
                    vec!["Primary", "Companion"],
                    vec![mass, small],
                    vec![1e6, 1e5],
                    vec![
                        -DVec3::X * (small / total * r),
                        DVec3::X * (mass / total * r),
                    ],
                    vec![
                        -DVec3::Y * (small / total * v),
                        DVec3::Y * (mass / total * v),
                    ],
                )
            }
            Self::Hierarchy => {
                let ms = 1.98847e30;
                let mp = 5.9722e24;
                let mm = 7.342e20;
                let inner = mp + mm;
                let total = ms + inner;
                let outer_v = (G * total / 1.5e11).sqrt();
                let inner_v = (G * inner / 1e8).sqrt();
                let tangent =
                    DVec3::new(0.0, 5.0_f64.to_radians().cos(), 5.0_f64.to_radians().sin());
                let bary_x = DVec3::X * (ms / total * 1.5e11);
                let bary_v = DVec3::Y * (ms / total * outer_v);
                (
                    vec!["Solace", "Aurelia", "Luma"],
                    vec![ms, mp, mm],
                    vec![6.957e8, 6.371e6, 3.74e5],
                    vec![
                        -DVec3::X * (inner / total * 1.5e11),
                        bary_x - DVec3::X * (mm / inner * 1e8),
                        bary_x + DVec3::X * (mp / inner * 1e8),
                    ],
                    vec![
                        -DVec3::Y * (inner / total * outer_v),
                        bary_v - tangent * (mm / inner * inner_v),
                        bary_v + tangent * (mp / inner * inner_v),
                    ],
                )
            }
            Self::GameplaySolarSystem | Self::RealSolarSystem => {
                unreachable!("solar presets dispatched above")
            }
        };
        for i in 0..masses.len() {
            let (orientation, omega) = if self == Self::Circular {
                (UnitRotation::identity(), DVec3::ZERO)
            } else {
                let q = UnitRotation::from_axis_angle(
                    Direction3::try_new(if i == 2 { DVec3::X } else { DVec3::Z })?,
                    if i == 1 {
                        23.4_f64.to_radians()
                    } else if i == 2 {
                        -0.3
                    } else {
                        0.0
                    },
                )?;
                let period_days = [25.0, 1.0, 4.0][i];
                (
                    q,
                    q.rotate_direction(Direction3::try_new(DVec3::Y)?)?.unit()
                        * (std::f64::consts::TAU / (period_days * 86400.0)),
                )
            };
            system.insert_body(
                names[i],
                BodyProperties::new(masses[i], radii[i])?,
                BodyState::new(
                    LocalPosition::try_metres(positions[i])?,
                    LinearVelocity3::try_metres_per_second(velocities[i])?,
                    orientation,
                    AngularVelocity3::try_radians_per_second(omega)?,
                ),
            )?;
        }
        Ok(system)
    }
}
impl GravityFixture {
    pub fn color(self, index: usize) -> [f32; 4] {
        match self {
            Self::GameplaySolarSystem | Self::RealSolarSystem => {
                crate::solar_system::SOLAR_SYSTEM_CONTENT[index].color
            }
            _ => body_color(index),
        }
    }
}
pub fn body_color(index: usize) -> [f32; 4] {
    [
        [1.0, 0.7, 0.25, 1.0],
        [0.2, 0.5, 1.0, 1.0],
        [0.65, 0.65, 0.7, 1.0],
    ][index % 3]
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_simulation::*;
    #[test]
    fn independent_initial_condition_answers_and_hill_sanity() {
        let world = GravityFixture::Circular
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let diagnostics = system_diagnostics(&world).unwrap();
        let expected = -6.67430e-11 * 1e24 * 1e20 / (2.0 * 1e7);
        assert!((diagnostics.total_energy_j() - expected).abs() / expected.abs() <= 2e-14);
        let world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(2).unwrap())
            .unwrap();
        let diagnostics = system_diagnostics(&world).unwrap();
        assert!(diagnostics.center_of_mass_m.length() < 1e-6);
        assert!(diagnostics.linear_momentum_kg_m_s.length() / diagnostics.momentum_scale < 1e-14);
        let hill = 1.5e11 * ((5.9722e24 + 7.342e20) / (3.0 * 1.98847e30_f64)).cbrt();
        assert!(1e8 / hill < 0.07);
        let bodies: Vec<_> = world.bodies().map(|(_, b)| b).collect();
        assert!(
            ((bodies[2].state().center_in_system().metres()
                - bodies[1].state().center_in_system().metres())
            .length()
                - 1e8)
                .abs()
                < 1e-4
        );
        assert!(IntegrationWorkspace::new(&world, 60.0).is_ok());
    }
}
