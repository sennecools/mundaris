//! App-session failure safety beyond the default circular content.
use glam::DVec3;
use mundaris_app::{motion_session::AnalyticSession, solar_system::SolarSystemPreset};
use mundaris_math::*;
use mundaris_world::*;
use std::num::NonZeroU64;

fn authority(world: &CelestialSystem) -> (u64, SimulationInstant, Vec<CelestialBody>) {
    (
        world.revision(),
        world.sample_time(),
        world.bodies().map(|(_, b)| b.clone()).collect(),
    )
}

#[test]
fn stale_binding_is_diagnosed_without_resampling_or_mutating_authority() {
    let (mut world, definition) = SolarSystemPreset::gameplay()
        .create_analytic(NonZeroU64::new(1).unwrap())
        .unwrap();
    let mut session = AnalyticSession::new(&mut world, definition).unwrap();
    let id = world.bodies().nth(3).unwrap().0;
    let properties = *world.body(id).unwrap().properties();
    world.edit_properties(id, properties).unwrap();
    let before = authority(&world);
    assert!(session.seek_seconds(12.5, &mut world).is_err());
    assert_eq!(authority(&world), before);
    let snapshot = session.snapshot(&world);
    assert_eq!(snapshot.requested_time_s, 12.5);
    assert_eq!(snapshot.published_time_s, 0.0);
    assert!(snapshot.paused);
    assert!(snapshot.latest_failure.unwrap().contains("revision"));
    // A stale session does not silently bind itself to arbitrary external edits.
    assert!(session.edit_properties(&mut world, id, properties).is_err());
    assert_eq!(authority(&world), before);
}

#[test]
fn solver_failure_pauses_and_preserves_all_states_time_revision_and_history_independence() {
    let mut world = CelestialSystem::new(NonZeroU64::new(2).unwrap(), SimulationInstant::ZERO);
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    let star = world
        .insert_body("star", BodyProperties::new(1.0, 1.0).unwrap(), state)
        .unwrap();
    let planet = world
        .insert_body("planet", BodyProperties::new(1.0, 1.0).unwrap(), state)
        .unwrap();
    let spin = AxialSpin::new(
        UnitRotation::identity(),
        Direction3::try_new(DVec3::Z).unwrap(),
        0.0,
        SimulationInstant::ZERO,
    )
    .unwrap();
    let definition = CelestialMotionDefinition::new(
        &world,
        &[
            BodyMotion {
                body: star,
                translation: CelestialTranslation::Stationary(LocalPosition::origin()),
                spin,
            },
            BodyMotion {
                body: planet,
                translation: CelestialTranslation::Elliptic(
                    EllipticOrbit::new(
                        star,
                        1000.0,
                        0.6,
                        UnitRotation::identity(),
                        100.0,
                        0.0,
                        SimulationInstant::ZERO,
                    )
                    .unwrap(),
                ),
                spin,
            },
        ],
    )
    .unwrap();
    let mut session = AnalyticSession::with_iteration_limit(&mut world, definition, 1).unwrap();
    let before = authority(&world);
    assert!(session.seek_seconds(25.0, &mut world).is_err());
    assert_eq!(authority(&world), before);
    let snapshot = session.snapshot(&world);
    assert!(snapshot.paused);
    assert_eq!(snapshot.requested_time_s, 25.0);
    assert_eq!(snapshot.published_time_s, 0.0);
    assert!(snapshot.sampling_ms.is_none());
    assert!(snapshot.latest_failure.unwrap().contains("Kepler solver"));
    session.seek_seconds(0.0, &mut world).unwrap();
    assert_eq!(authority(&world), before);
    assert!(session.snapshot(&world).latest_failure.is_none());
}
