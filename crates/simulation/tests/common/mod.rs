#![allow(dead_code)]
use glam::DVec3;
use mundaris_math::*;
use mundaris_world::*;
use std::num::NonZeroU64;
pub const G: f64 = 6.67430e-11;
pub const R: f64 = 1e7;
pub const M: f64 = 1e24;
pub const SMALL: f64 = 1e20;
pub fn period() -> f64 {
    std::f64::consts::TAU * (R.powi(3) / (G * (M + SMALL))).sqrt()
}
pub fn state(x: DVec3, v: DVec3) -> BodyState {
    BodyState::new(
        LocalPosition::try_metres(x).unwrap(),
        LinearVelocity3::try_metres_per_second(v).unwrap(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}
pub fn circular(eccentricity: f64, offset: DVec3, boost: DVec3) -> CelestialSystem {
    let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    let separation = R * (1.0 - eccentricity);
    let speed = (G * (M + SMALL) * (1.0 + eccentricity) / separation).sqrt();
    for (i, (mass, radius, fraction)) in [
        (M, 1e6, -SMALL / (M + SMALL)),
        (SMALL, 1e5, M / (M + SMALL)),
    ]
    .into_iter()
    .enumerate()
    {
        world
            .insert_body(
                format!("body {i}"),
                BodyProperties::new(mass, radius).unwrap(),
                state(
                    offset + DVec3::X * (fraction * separation),
                    boost + DVec3::Y * (fraction * speed),
                ),
            )
            .unwrap();
    }
    world
}
pub fn hierarchy() -> CelestialSystem {
    let masses = [1.98847e30, 5.9722e24, 7.342e20];
    let radii = [6.957e8, 6.371e6, 3.74e5];
    let inner_mass = masses[1] + masses[2];
    let total = masses[0] + inner_mass;
    let outer_speed = (G * total / 1.5e11_f64).sqrt();
    let inner_speed = (G * inner_mass / 1e8_f64).sqrt();
    let tangent = DVec3::new(0.0, 5.0_f64.to_radians().cos(), 5.0_f64.to_radians().sin());
    let bary_x = DVec3::X * (masses[0] / total * 1.5e11);
    let bary_v = DVec3::Y * (masses[0] / total * outer_speed);
    let states = [
        state(
            -DVec3::X * (inner_mass / total * 1.5e11),
            -DVec3::Y * (inner_mass / total * outer_speed),
        ),
        state(
            bary_x - DVec3::X * (masses[2] / inner_mass * 1e8),
            bary_v - tangent * (masses[2] / inner_mass * inner_speed),
        ),
        state(
            bary_x + DVec3::X * (masses[1] / inner_mass * 1e8),
            bary_v + tangent * (masses[1] / inner_mass * inner_speed),
        ),
    ];
    let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    for i in 0..3 {
        world
            .insert_body(
                format!("body {i}"),
                BodyProperties::new(masses[i], radii[i]).unwrap(),
                states[i],
            )
            .unwrap();
    }
    world
}
