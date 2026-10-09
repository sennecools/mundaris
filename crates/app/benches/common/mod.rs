use glam::DVec3;
use astrum_math::*;
use astrum_simulation::GRAVITATIONAL_CONSTANT_M3_KG_S2 as G;
use astrum_world::*;
use std::num::NonZeroU64;
/// Deterministic plausible planet/moon pairs. A scale fixture, not an accuracy oracle.
pub fn fixture(count: usize) -> CelestialSystem {
    if count == 3 {
        return astrum_app::gravity_fixtures::GravityFixture::Hierarchy
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
    }
    let mut system = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    for i in 0..count {
        let (mass, radius, p, v) = if i == 0 {
            (1e30, 6e8, DVec3::ZERO, DVec3::ZERO)
        } else {
            let pair = (i - 1) / 2;
            let angle = pair as f64 * 0.71;
            let a = (pair + 1) as f64 * 5e10;
            let radial = DVec3::new(angle.cos(), angle.sin(), 0.0);
            let tangent = DVec3::new(-angle.sin(), angle.cos(), 0.0);
            let planet = radial * a;
            let velocity = tangent * (G * 1e30 / a).sqrt();
            if i % 2 == 1 {
                (1e24, 6e6, planet, velocity)
            } else {
                (
                    1e18,
                    3e5,
                    planet + radial * 1e8,
                    velocity + tangent * (G * 1e24 / 1e8).sqrt(),
                )
            }
        };
        system
            .insert_body(
                format!("Body {i}"),
                BodyProperties::new(mass, radius).unwrap(),
                BodyState::new(
                    LocalPosition::try_metres(p).unwrap(),
                    LinearVelocity3::try_metres_per_second(v).unwrap(),
                    UnitRotation::identity(),
                    AngularVelocity3::try_radians_per_second(DVec3::Y * 1e-5).unwrap(),
                ),
            )
            .unwrap();
    }
    system
}
