use glam::{DQuat, DVec3};
use astrum_math::*;
use astrum_world::*;
use std::num::NonZeroU64;

fn system(namespace: u64) -> CelestialSystem {
    CelestialSystem::new(NonZeroU64::new(namespace).unwrap(), SimulationInstant::ZERO)
}
fn state(x: f64) -> BodyState {
    BodyState::new(
        LocalPosition::try_metres(DVec3::new(x, 0.0, 0.0)).unwrap(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}
fn insert(system: &mut CelestialSystem, name: &str, x: f64) -> BodyId {
    system
        .insert_body(name, BodyProperties::new(1.0, 2.0).unwrap(), state(x))
        .unwrap()
}

#[test]
fn positive_finite_properties_and_utf8_byte_limit() {
    assert!(BodyProperties::new(1.98847e30, 6.957e8).is_ok());
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(BodyProperties::new(invalid, 1.0).is_err());
        assert!(BodyProperties::new(1.0, invalid).is_err());
    }
    let mut world = system(1);
    for valid in ["☀ Solace / A", " ", &"é".repeat(64)] {
        insert(&mut world, valid, 0.0);
    }
    let revision = world.revision();
    for invalid in ["", &"é".repeat(65)] {
        assert!(
            world
                .insert_body(invalid, BodyProperties::new(1.0, 1.0).unwrap(), state(0.0))
                .is_err()
        );
    }
    assert_eq!(world.revision(), revision);
    assert_eq!(world.body_count(), 3);
}

#[test]
fn identity_and_atomic_editor_mutation() {
    let mut world = system(1);
    let first = insert(&mut world, "same", 0.0);
    let second = insert(&mut world, "same", 1.5e11);
    assert_ne!(first, second);
    assert_eq!(world.body(first).unwrap().state(), &state(0.0));
    let foreign = insert(&mut system(2), "same", 0.0);
    assert!(matches!(
        world.body(foreign),
        Err(CelestialSystemError::WrongSystem(_))
    ));
    let before = world.body(first).unwrap().clone();
    let revision = world.revision();
    assert!(
        world
            .edit_body(
                first,
                "x".repeat(129),
                BodyProperties::new(2.0, 3.0).unwrap()
            )
            .is_err()
    );
    assert_eq!(world.body(first).unwrap(), &before);
    assert_eq!(world.revision(), revision);
    world
        .edit_body(first, "renamed", BodyProperties::new(3.0, 4.0).unwrap())
        .unwrap();
    assert_eq!(world.revision(), revision + 1);
    assert_eq!(world.body(first).unwrap().state(), before.state());
    assert_eq!(world.body(first).unwrap().name(), "renamed");
    assert_eq!(
        world.bodies().map(|(id, _)| id).collect::<Vec<_>>(),
        vec![first, second]
    );
}

#[test]
fn arbitrary_order_batch_is_atomic_and_prevents_mixed_time() {
    let mut world = system(1);
    let a = insert(&mut world, "a", 0.0);
    let b = insert(&mut world, "b", 10.0);
    let foreign = insert(&mut system(2), "foreign", 0.0);
    let time = SimulationInstant::try_seconds_since_epoch(-10.0).unwrap();
    let revision = world.revision();
    for updates in [
        vec![
            BodyStateUpdate {
                body: a,
                state: state(1.0),
            },
            BodyStateUpdate {
                body: foreign,
                state: state(2.0),
            },
        ],
        vec![
            BodyStateUpdate {
                body: a,
                state: state(1.0),
            },
            BodyStateUpdate {
                body: a,
                state: state(2.0),
            },
        ],
        vec![BodyStateUpdate {
            body: a,
            state: state(1.0),
        }],
    ] {
        assert!(world.update_states(time, &updates).is_err());
        assert_eq!(world.revision(), revision);
        assert_eq!(world.sample_time(), SimulationInstant::ZERO);
        assert_eq!(world.body(a).unwrap().state(), &state(0.0));
        assert_eq!(world.body(b).unwrap().state(), &state(10.0));
    }
    world
        .update_states(
            time,
            &[
                BodyStateUpdate {
                    body: b,
                    state: state(20.0),
                },
                BodyStateUpdate {
                    body: a,
                    state: state(2.0),
                },
            ],
        )
        .unwrap();
    assert_eq!(world.revision(), revision + 1);
    assert_eq!(world.sample_time(), time);
    let properties = *world.body(a).unwrap().properties();
    world.edit_state(a, state(3.0)).unwrap();
    assert_eq!(world.revision(), revision + 2);
    assert_eq!(*world.body(a).unwrap().properties(), properties);
    assert!(world.edit_state(foreign, state(4.0)).is_err());
    assert_eq!(world.body(a).unwrap().state(), &state(3.0));
}

#[test]
fn invalid_state_components_are_rejected_before_world_mutation() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(LocalPosition::try_metres(DVec3::splat(invalid)).is_err());
        assert!(LinearVelocity3::try_metres_per_second(DVec3::splat(invalid)).is_err());
        assert!(AngularVelocity3::try_radians_per_second(DVec3::splat(invalid)).is_err());
        assert!(
            UnitRotation::try_from_quaternion(DQuat::from_xyzw(invalid, 0.0, 0.0, 1.0)).is_err()
        );
    }
    assert!(UnitRotation::try_from_quaternion(DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0)).is_err());
    let mut empty = system(1);
    let time = SimulationInstant::try_seconds_since_epoch(1.0).unwrap();
    empty.update_states(time, &[]).unwrap();
    assert_eq!(empty.sample_time(), time);
}
