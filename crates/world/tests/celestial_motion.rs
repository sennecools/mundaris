use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_world::*;
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
fn insert(world: &mut CelestialSystem, name: &str, mass: f64, radius: f64) -> BodyId {
    world
        .insert_body(name, BodyProperties::new(mass, radius).unwrap(), state(0.0))
        .unwrap()
}
fn spin() -> AxialSpin {
    AxialSpin::new(
        UnitRotation::identity(),
        Direction3::try_new(DVec3::Z).unwrap(),
        0.25,
        SimulationInstant::ZERO,
    )
    .unwrap()
}
fn stationary(body: BodyId) -> BodyMotion {
    BodyMotion {
        body,
        translation: CelestialTranslation::Stationary(LocalPosition::origin()),
        spin: spin(),
    }
}
fn orbit(
    reference: BodyId,
    axis: f64,
    eccentricity: f64,
    period: f64,
    phase: f64,
) -> EllipticOrbit {
    EllipticOrbit::new(
        reference,
        axis,
        eccentricity,
        UnitRotation::identity(),
        period,
        phase,
        SimulationInstant::ZERO,
    )
    .unwrap()
}
fn orbiting(body: BodyId, orbit: EllipticOrbit) -> BodyMotion {
    BodyMotion {
        body,
        translation: CelestialTranslation::Elliptic(orbit),
        spin: spin(),
    }
}

#[test]
fn definitions_resolve_star_planet_moon_independent_of_insertion_order() {
    let mut world = system(1);
    // Deliberately neither semantic nor definition order.
    let moon = insert(&mut world, "moon", 1.0, 2.0);
    let star = insert(&mut world, "star", 1e30, 1e8);
    let planet = insert(&mut world, "planet", 1e24, 6e6);
    let definitions = [
        orbiting(moon, orbit(planet, 4e8, 0.05, 20.0, -0.3)),
        stationary(star),
        orbiting(planet, orbit(star, 1.5e11, 0.02, 365.0 * 86400.0, 0.7)),
    ];
    let definition = CelestialMotionDefinition::new(&world, &definitions).unwrap();
    assert_eq!(
        definition
            .definitions()
            .iter()
            .map(|d| d.body)
            .collect::<Vec<_>>(),
        vec![moon, star, planet]
    );
    assert_eq!(definition.reference_indices(), &[Some(2), None, Some(1)]);
    assert_eq!(definition.evaluation_order(), &[1, 2, 0]);
    let reversed = [definitions[1], definitions[2], definitions[0]];
    let reordered = CelestialMotionDefinition::new(&world, &reversed).unwrap();
    assert_eq!(definition.definitions(), reordered.definitions());
    assert_eq!(definition.evaluation_order(), reordered.evaluation_order());
}

#[test]
fn definition_completeness_duplicates_and_reference_identity_are_checked() {
    let mut world = system(1);
    let a = insert(&mut world, "a", 1.0, 1.0);
    let b = insert(&mut world, "b", 1.0, 1.0);
    assert!(matches!(
        CelestialMotionDefinition::new(&world, &[stationary(a)]),
        Err(CelestialMotionError::IncompleteDefinitions)
    ));
    assert!(
        matches!(CelestialMotionDefinition::new(&world, &[stationary(a), stationary(a)]), Err(CelestialMotionError::DuplicateBody(id)) if id == a)
    );

    let mut foreign_world = system(2);
    let foreign = insert(&mut foreign_world, "foreign", 1.0, 1.0);
    assert!(
        matches!(CelestialMotionDefinition::new(&world, &[stationary(a), stationary(foreign)]), Err(CelestialMotionError::System(CelestialSystemError::WrongSystem(id))) if id == foreign)
    );
    let wrong_reference = orbiting(b, orbit(foreign, 2.0, 0.0, 10.0, 0.0));
    assert!(
        matches!(CelestialMotionDefinition::new(&world, &[stationary(a), wrong_reference]), Err(CelestialMotionError::System(CelestialSystemError::WrongSystem(id))) if id == foreign)
    );

    // Same namespace, but an index not present in this system.
    let mut longer = system(3);
    let _ = insert(&mut longer, "a", 1.0, 1.0);
    let unknown = insert(&mut longer, "unknown", 1.0, 1.0);
    let mut shorter = system(3);
    let only = insert(&mut shorter, "only", 1.0, 1.0);
    assert!(
        matches!(CelestialMotionDefinition::new(&shorter, &[orbiting(only, orbit(unknown, 1.0, 0.0, 1.0, 0.0))]), Err(CelestialMotionError::System(CelestialSystemError::UnknownBody(id))) if id == unknown)
    );
}

#[test]
fn cycles_and_self_references_are_rejected() {
    let mut world = system(1);
    let a = insert(&mut world, "a", 1.0, 1.0);
    let b = insert(&mut world, "b", 1.0, 1.0);
    assert!(matches!(
        CelestialMotionDefinition::new(
            &world,
            &[
                orbiting(a, orbit(b, 1.0, 0.0, 1.0, 0.0)),
                orbiting(b, orbit(a, 1.0, 0.0, 1.0, 0.0))
            ]
        ),
        Err(CelestialMotionError::ReferenceCycle)
    ));
    assert!(matches!(
        CelestialMotionDefinition::new(
            &world,
            &[orbiting(a, orbit(a, 1.0, 0.0, 1.0, 0.0)), stationary(b)]
        ),
        Err(CelestialMotionError::ReferenceCycle)
    ));
}

#[test]
fn orbit_and_spin_numeric_inputs_are_checked() {
    let mut world = system(1);
    let a = insert(&mut world, "a", 1.0, 1.0);
    for eccentricity in [-f64::EPSILON, 1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            EllipticOrbit::new(
                a,
                1.0,
                eccentricity,
                UnitRotation::identity(),
                1.0,
                0.0,
                SimulationInstant::ZERO
            ),
            Err(CelestialMotionError::InvalidOrbit)
        ));
    }
    for axis in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            EllipticOrbit::new(
                a,
                axis,
                0.0,
                UnitRotation::identity(),
                1.0,
                0.0,
                SimulationInstant::ZERO
            ),
            Err(CelestialMotionError::InvalidOrbit)
        ));
    }
    for period in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            EllipticOrbit::new(
                a,
                1.0,
                0.0,
                UnitRotation::identity(),
                period,
                0.0,
                SimulationInstant::ZERO
            ),
            Err(CelestialMotionError::InvalidOrbit)
        ));
    }
    for phase in [
        f64::NAN,
        f64::INFINITY,
        std::f64::consts::TAU + 0.01,
        -std::f64::consts::TAU - 0.01,
    ] {
        assert!(matches!(
            EllipticOrbit::new(
                a,
                1.0,
                0.0,
                UnitRotation::identity(),
                1.0,
                phase,
                SimulationInstant::ZERO
            ),
            Err(CelestialMotionError::InvalidOrbit)
        ));
    }
    for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(matches!(
            AxialSpin::new(
                UnitRotation::identity(),
                Direction3::try_new(DVec3::Z).unwrap(),
                rate,
                SimulationInstant::ZERO
            ),
            Err(CelestialMotionError::InvalidSpin)
        ));
    }
    assert!(UnitRotation::try_from_quaternion(DQuat::from_xyzw(f64::NAN, 0.0, 0.0, 1.0)).is_err());
    assert!(UnitRotation::try_from_quaternion(DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0)).is_err());
    assert!(Direction3::try_new(DVec3::splat(f64::INFINITY)).is_err());
    assert!(Direction3::try_new(DVec3::ZERO).is_err());
}

#[test]
fn rejected_definition_does_not_mutate_authoritative_system() {
    let mut world = system(1);
    let a = insert(&mut world, "a", 1.0, 2.0);
    let b = insert(&mut world, "b", 3.0, 4.0);
    let before_time = world.sample_time();
    let before_revision = world.revision();
    let before_states = world
        .bodies()
        .map(|(_, body)| *body.state())
        .collect::<Vec<_>>();
    assert!(CelestialMotionDefinition::new(&world, &[stationary(a)]).is_err());
    assert_eq!(world.sample_time(), before_time);
    assert_eq!(world.revision(), before_revision);
    assert_eq!(
        world
            .bodies()
            .map(|(_, body)| *body.state())
            .collect::<Vec<_>>(),
        before_states
    );
    assert_eq!(world.body_count(), 2);
    assert_ne!(a, b);
}

#[test]
fn empty_system_has_empty_definition_and_authored_period_ignores_properties() {
    let empty = system(1);
    let definition = CelestialMotionDefinition::new(&empty, &[]).unwrap();
    assert!(definition.definitions().is_empty());
    assert!(definition.reference_indices().is_empty());
    assert!(definition.evaluation_order().is_empty());

    let mut world = system(2);
    let star = insert(&mut world, "star", 1.0, 1.0);
    let planet = insert(&mut world, "planet", 2.0, 3.0);
    let authored = orbit(star, 42.0, 0.1, 1234.5, 0.0);
    let motions = [stationary(star), orbiting(planet, authored)];
    let initial = CelestialMotionDefinition::new(&world, &motions).unwrap();
    world
        .edit_properties(star, BodyProperties::new(9e30, 9e9).unwrap())
        .unwrap();
    world
        .edit_properties(planet, BodyProperties::new(7e25, 7e7).unwrap())
        .unwrap();
    let later = CelestialMotionDefinition::new(&world, &motions).unwrap();
    assert_eq!(initial.definitions(), later.definitions());
    match later.definitions()[1].translation {
        CelestialTranslation::Elliptic(o) => assert_eq!(o.period_seconds(), 1234.5),
        CelestialTranslation::Stationary(_) => panic!("expected authored orbit"),
    }
}
