use glam::DVec3;
use mundaris_math::*;
use mundaris_world::*;
use std::num::NonZeroU64;
pub fn fixture(count: usize) -> CelestialSystem {
    let mut system = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    for i in 0..count {
        let position = DVec3::new((i % 16) as f64, (i / 16 % 16) as f64, (i / 256) as f64) * 1e9;
        let velocity = DVec3::new((i % 3) as f64 * 0.1, (i % 5) as f64 * 0.1, 0.0);
        system
            .insert_body(
                "scale probe",
                BodyProperties::new(1e18 + (i % 7) as f64 * 1e16, 1e3).unwrap(),
                BodyState::new(
                    LocalPosition::try_metres(position).unwrap(),
                    LinearVelocity3::try_metres_per_second(velocity).unwrap(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
    }
    system
}
