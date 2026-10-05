use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{
    f64::consts::{PI, TAU},
    num::NonZeroU64,
};

const YEAR: f64 = 31_557_600.0;
fn ns(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).unwrap()
}
fn time(t: f64) -> SimulationInstant {
    SimulationInstant::try_seconds_since_epoch(t).unwrap()
}
fn initial() -> BodyState {
    BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}
fn insert(world: &mut CelestialSystem, name: &str) -> BodyId {
    world
        .insert_body(name, BodyProperties::new(1.0, 1.0).unwrap(), initial())
        .unwrap()
}
fn spin(rate: f64) -> AxialSpin {
    AxialSpin::new(
        UnitRotation::identity(),
        Direction3::try_new(DVec3::Z).unwrap(),
        rate,
        time(0.0),
    )
    .unwrap()
}
fn fixed(body: BodyId, center: DVec3, rate: f64) -> BodyMotion {
    BodyMotion {
        body,
        translation: CelestialTranslation::Stationary(LocalPosition::try_metres(center).unwrap()),
        spin: spin(rate),
    }
}
fn orbit(body: BodyId, parent: BodyId, a: f64, e: f64, period: f64) -> BodyMotion {
    BodyMotion {
        body,
        translation: CelestialTranslation::Elliptic(
            EllipticOrbit::new(
                parent,
                a,
                e,
                UnitRotation::identity(),
                period,
                0.0,
                time(0.0),
            )
            .unwrap(),
        ),
        spin: spin(0.0),
    }
}
fn fixture(a: f64, e: f64, period: f64) -> (CelestialSystem, AnalyticMotionProducer, BodyId) {
    let mut world = CelestialSystem::new(ns(1), time(0.0));
    let star = insert(&mut world, "star");
    let planet = insert(&mut world, "planet");
    let definition = CelestialMotionDefinition::new(
        &world,
        &[
            orbit(planet, star, a, e, period),
            fixed(star, DVec3::ZERO, 0.0),
        ],
    )
    .unwrap();
    let producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    (world, producer, planet)
}
fn vector_check(label: &str, actual: DVec3, expected: DVec3, scale: f64, bound: f64) {
    let absolute = (actual - expected).length();
    let normalized = absolute / scale;
    println!(
        "oracle={label} scale={scale:.16e} absolute_error={absolute:.16e} normalized_error={normalized:.16e} bound={bound:.1e}"
    );
    assert!(normalized <= bound, "{label}: {normalized:e} > {bound:e}");
}
fn vectors(world: &CelestialSystem, body: BodyId) -> (DVec3, DVec3) {
    let state = world.body(body).unwrap().state();
    (
        state.center_in_system().metres(),
        state.center_velocity_in_system().metres_per_second(),
    )
}

#[test]
fn circular_closed_form_oracles_and_direct_millennium_seeks() {
    for a in [1.0, 1.5e11] {
        let (mut world, mut producer, planet) = fixture(a, 0.0, YEAR);
        let speed = TAU * a / YEAR;
        for years in [-1000.0, -1.0, 0.0, 1.0, 1000.0] {
            for (fraction, unit_position, unit_velocity) in [
                (0.0, DVec3::X, DVec3::Y),
                (0.25, DVec3::Y, -DVec3::X),
                (0.5, -DVec3::X, -DVec3::Y),
                (0.75, -DVec3::Y, DVec3::X),
            ] {
                let requested = (years + fraction) * YEAR;
                let stats = producer.sample(&mut world, time(requested)).unwrap();
                println!(
                    "time_s={requested} bodies={} iterations={}",
                    stats.body_count, stats.solver_iterations
                );
                assert_eq!(stats.solver_iterations, 0);
                assert_eq!(world.sample_time(), time(requested));
                let (p, v) = vectors(&world, planet);
                vector_check("circular position", p, unit_position * a, a, 1e-11);
                vector_check("circular velocity", v, unit_velocity * speed, speed, 1e-10);
            }
        }
    }
}

#[test]
fn eccentric_periapsis_apoapsis_and_independent_decimal_quarter_oracles() {
    // 80-decimal independent bisection/series oracle: docs/evidence/phase513a/oracle.py.
    for (e, quarter_p, quarter_v) in [
        (
            0.6,
            DVec3::new(-1.097_342_301_884_903_5, 0.694_043_518_984_024_7, 0.0),
            DVec3::new(-0.668_169_133_721_835_3, -0.306_432_680_648_138_7, 0.0),
        ),
        (
            0.95,
            DVec3::new(-1.606_704_512_548_465, 0.235_482_632_824_274_5, 0.0),
            DVec3::new(-0.464_414_217_954_794_65, -0.126_276_123_313_482_28, 0.0),
        ),
    ] {
        for a in [1000.0, 1.5e11] {
            let (mut world, mut producer, planet) = fixture(a, e, YEAR);
            let speed = TAU * a / YEAR;
            for years in [-1000.0, -1.0, 0.0, 1.0, 1000.0] {
                for (fraction, p, v) in [
                    (
                        0.0,
                        DVec3::X * (1.0 - e),
                        DVec3::Y * ((1.0 + e) / (1.0 - e)).sqrt(),
                    ),
                    (
                        0.5,
                        -DVec3::X * (1.0 + e),
                        -DVec3::Y * ((1.0 - e) / (1.0 + e)).sqrt(),
                    ),
                    (0.25, quarter_p, quarter_v),
                ] {
                    let requested = (years + fraction) * YEAR;
                    let stats = producer.sample(&mut world, time(requested)).unwrap();
                    println!(
                        "eccentricity={e} time_s={requested} iterations={}",
                        stats.solver_iterations
                    );
                    assert!(stats.maximum_solver_iterations <= 64);
                    let (actual_p, actual_v) = vectors(&world, planet);
                    vector_check("eccentric position", actual_p, p * a, a, 1e-11);
                    vector_check("eccentric velocity", actual_v, v * speed, speed, 1e-10);
                }
            }
        }
    }
}

#[test]
fn inclined_orbit_epoch_and_authored_phase_are_system_axis_inputs() {
    let a = 12345.0;
    let period = 1000.0;
    let mut world = CelestialSystem::new(ns(1), time(42.0));
    let star = insert(&mut world, "star");
    let planet = insert(&mut world, "planet");
    let tilt = UnitRotation::try_from_quaternion(DQuat::from_rotation_x(PI / 2.0)).unwrap();
    let motion = BodyMotion {
        body: planet,
        translation: CelestialTranslation::Elliptic(
            EllipticOrbit::new(star, a, 0.0, tilt, period, PI / 2.0, time(42.0)).unwrap(),
        ),
        spin: spin(0.0),
    };
    let definition =
        CelestialMotionDefinition::new(&world, &[motion, fixed(star, DVec3::ZERO, 0.0)]).unwrap();
    let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    producer.sample(&mut world, time(42.0)).unwrap();
    let (p, v) = vectors(&world, planet);
    vector_check("tilted phase position", p, DVec3::Z * a, a, 1e-11);
    vector_check(
        "tilted phase velocity",
        v,
        -DVec3::X * TAU * a / period,
        TAU * a / period,
        1e-10,
    );
}

#[test]
fn analytic_velocity_matches_independent_five_point_derivative() {
    let a = 1.5e11;
    let period = YEAR;
    let h = period * 1e-5;
    let (mut world, mut producer, planet) = fixture(a, 0.6, period);
    for t in [period * 0.03, period * 0.21, -period * 0.36] {
        let mut points = [DVec3::ZERO; 4];
        for (slot, delta) in points.iter_mut().zip([-2.0, -1.0, 1.0, 2.0]) {
            producer.sample(&mut world, time(t + delta * h)).unwrap();
            *slot = vectors(&world, planet).0;
        }
        producer.sample(&mut world, time(t)).unwrap();
        let derivative = (points[0] - 8.0 * points[1] + 8.0 * points[2] - points[3]) / (12.0 * h);
        vector_check(
            "five point derivative",
            vectors(&world, planet).1,
            derivative,
            TAU * a / period,
            1e-10,
        );
    }
}

fn hierarchy(
    namespace: u64,
    reordered: bool,
    parent_rate: f64,
) -> (CelestialSystem, AnalyticMotionProducer, [BodyId; 3]) {
    let mut world = CelestialSystem::new(ns(namespace), time(0.0));
    let ids = if reordered {
        let moon = insert(&mut world, "moon");
        let planet = insert(&mut world, "planet");
        let star = insert(&mut world, "star");
        [star, planet, moon]
    } else {
        [
            insert(&mut world, "star"),
            insert(&mut world, "planet"),
            insert(&mut world, "moon"),
        ]
    };
    let mut planet = orbit(ids[1], ids[0], 1000.0, 0.0, 100.0);
    planet.spin = spin(parent_rate);
    let definition = CelestialMotionDefinition::new(
        &world,
        &[
            orbit(ids[2], ids[1], 10.0, 0.0, 20.0),
            planet,
            fixed(ids[0], DVec3::new(1.0, 2.0, 3.0), 0.0),
        ],
    )
    .unwrap();
    let producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    (world, producer, ids)
}

#[test]
fn reordered_hierarchy_adds_parent_velocity_not_parent_spin_and_systems_are_independent() {
    let (mut a, mut pa, ids_a) = hierarchy(1, false, 0.0);
    let (mut b, mut pb, ids_b) = hierarchy(2, true, -123.0);
    let untouched = b.revision();
    pa.sample(&mut a, time(5.0)).unwrap();
    assert_eq!(b.revision(), untouched);
    pb.sample(&mut b, time(5.0)).unwrap();
    for i in 0..3 {
        assert_eq!(vectors(&a, ids_a[i]), vectors(&b, ids_b[i]));
    }
    let angle = TAU * 5.0 / 100.0;
    let expected_p = DVec3::new(
        1.0 + 1000.0 * angle.cos(),
        2.0 + 1000.0 * angle.sin() + 10.0,
        3.0,
    );
    let expected_v = DVec3::new(
        -1000.0 * TAU / 100.0 * angle.sin() - 10.0 * TAU / 20.0,
        1000.0 * TAU / 100.0 * angle.cos(),
        0.0,
    );
    vector_check(
        "hierarchy position",
        vectors(&a, ids_a[2]).0,
        expected_p,
        1000.0,
        1e-11,
    );
    vector_check(
        "hierarchy velocity",
        vectors(&a, ids_a[2]).1,
        expected_v,
        TAU * 1000.0 / 100.0,
        1e-10,
    );
    // Foreign-system publication is rejected even when count/revision match.
    let before = b.revision();
    assert_eq!(
        pa.sample(&mut b, time(10.0)),
        Err(AnalyticMotionError::StaleBinding)
    );
    assert_eq!(b.revision(), before);
}

#[test]
fn tilted_stationary_and_retrograde_spin_derivatives() {
    let axis = Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap();
    let initial_rotation = UnitRotation::try_from_quaternion(
        DQuat::from_rotation_x(0.7) * DQuat::from_rotation_y(-0.4),
    )
    .unwrap();
    for rate in [0.0, 1e-4, -1e-4] {
        let mut world = CelestialSystem::new(ns(1), time(0.0));
        let body = insert(&mut world, "spinner");
        let motion = BodyMotion {
            body,
            translation: CelestialTranslation::Stationary(LocalPosition::origin()),
            spin: AxialSpin::new(initial_rotation, axis, rate, time(100.0)).unwrap(),
        };
        let definition = CelestialMotionDefinition::new(&world, &[motion]).unwrap();
        let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
        let t = 321.0;
        let h = 10.0;
        let point = DVec3::new(2.0, -1.0, 0.5);
        let mut positions = [DVec3::ZERO; 4];
        for (slot, delta) in positions.iter_mut().zip([-2.0, -1.0, 1.0, 2.0]) {
            producer.sample(&mut world, time(t + h * delta)).unwrap();
            *slot = world
                .body(body)
                .unwrap()
                .state()
                .body_to_system()
                .quaternion()
                * point;
        }
        producer.sample(&mut world, time(t)).unwrap();
        let state = world.body(body).unwrap().state();
        let omega = initial_rotation.quaternion() * (axis.unit() * rate);
        assert_eq!(
            state.angular_velocity_in_system().radians_per_second(),
            omega
        );
        let angle = rate * (t - 100.0);
        // Independent Rodrigues vector oracle, not quaternion construction.
        let unit = axis.unit();
        let expected = initial_rotation.quaternion()
            * (point * angle.cos()
                + unit.cross(point) * angle.sin()
                + unit * unit.dot(point) * (1.0 - angle.cos()));
        let actual = state.body_to_system().quaternion() * point;
        vector_check(
            "spin Rodrigues orientation",
            actual,
            expected,
            point.length(),
            1e-11,
        );
        let derivative =
            (positions[0] - 8.0 * positions[1] + 8.0 * positions[2] - positions[3]) / (12.0 * h);
        vector_check(
            "spin angular derivative",
            omega.cross(actual),
            derivative,
            (rate.abs() * point.length()).max(1e-4),
            1e-10,
        );
        if rate == 0.0 {
            assert_eq!(state.body_to_system(), initial_rotation);
        }
    }
}

#[test]
fn samples_are_bitwise_history_independent_even_after_failed_samples() {
    let (mut world, mut producer, planet) = fixture(1.5e11, 0.6, YEAR);
    let t = time(0.3125 * YEAR);
    producer.sample(&mut world, t).unwrap();
    let expected = state_bits(*world.body(planet).unwrap().state());
    for other in [-1000.0 * YEAR, 1000.0 * YEAR, 0.0, -0.125 * YEAR] {
        producer.sample(&mut world, time(other)).unwrap();
        producer.sample(&mut world, t).unwrap();
        assert_eq!(state_bits(*world.body(planet).unwrap().state()), expected);
    }
    assert_eq!(
        producer.sample(&mut world, time(ANALYTIC_TIME_LIMIT_SECONDS * 2.0)),
        Err(AnalyticMotionError::UnsupportedTime)
    );
    producer.sample(&mut world, t).unwrap();
    assert_eq!(state_bits(*world.body(planet).unwrap().state()), expected);
}

fn state_bits(state: BodyState) -> Vec<u64> {
    state
        .center_in_system()
        .metres()
        .to_array()
        .into_iter()
        .chain(
            state
                .center_velocity_in_system()
                .metres_per_second()
                .to_array(),
        )
        .chain(state.body_to_system().quaternion().to_array())
        .chain(
            state
                .angular_velocity_in_system()
                .radians_per_second()
                .to_array(),
        )
        .map(f64::to_bits)
        .collect()
}

#[test]
fn noncommensurate_period_long_seeks_match_exact_input_decimal_oracle() {
    let a = 1.5e11;
    let period = 1_234_567.89;
    let mut world = CelestialSystem::new(ns(1), time(0.0));
    let star = insert(&mut world, "star");
    let planet = insert(&mut world, "planet");
    let motion = BodyMotion {
        body: planet,
        spin: spin(0.0),
        translation: CelestialTranslation::Elliptic(
            EllipticOrbit::new(
                star,
                a,
                0.6,
                UnitRotation::identity(),
                period,
                0.37,
                time(1234.56789),
            )
            .unwrap(),
        ),
    };
    let definition =
        CelestialMotionDefinition::new(&world, &[fixed(star, DVec3::ZERO, 0.0), motion]).unwrap();
    let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    for (seconds, expected_p, expected_v) in [
        (
            -31_557_600_000.123_45,
            DVec3::new(-1.524_955_700_926_398, 0.304_059_942_852_069_6, 0.0),
            DVec3::new(-0.244_425_353_861_820_94, -0.475_869_587_839_393_16, 0.0),
        ),
        (
            31_557_600_000.123_45,
            DVec3::new(-1.235_525_167_955_462, -0.617_664_121_487_493, 0.0),
            DVec3::new(0.558_945_711_537_610_9, -0.368_069_627_328_152_57, 0.0),
        ),
    ] {
        producer.sample(&mut world, time(seconds)).unwrap();
        let (p, v) = vectors(&world, planet);
        vector_check(
            "decimal noncommensurate millennium position",
            p,
            expected_p * a,
            a,
            1e-11,
        );
        vector_check(
            "decimal noncommensurate millennium velocity",
            v,
            expected_v * TAU * a / period,
            TAU * a / period,
            1e-10,
        );
    }
}

#[test]
fn signed_millennium_spin_matches_decimal_rate_product_not_rounded_period() {
    let tilt = UnitRotation::try_from_quaternion(DQuat::from_rotation_x(0.7)).unwrap();
    for rate in [1e-4, -1e-4] {
        let mut world = CelestialSystem::new(ns(1), time(0.0));
        let body = insert(&mut world, "spinner");
        let motion = BodyMotion {
            body,
            translation: CelestialTranslation::Stationary(LocalPosition::origin()),
            spin: AxialSpin::new(
                tilt,
                Direction3::try_new(DVec3::Z).unwrap(),
                rate,
                time(0.0),
            )
            .unwrap(),
        };
        let definition = CelestialMotionDefinition::new(&world, &[motion]).unwrap();
        let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
        for seconds in [-1000.0 * YEAR, 1000.0 * YEAR] {
            producer.sample(&mut world, time(seconds)).unwrap();
            let sign = -(rate * seconds).signum();
            let expected = tilt.quaternion()
                * DVec3::new(0.328_144_695_274_284_3, sign * 0.944_627_470_997_613_5, 0.0);
            let state = world.body(body).unwrap().state();
            vector_check(
                "decimal millennium spin",
                state.body_to_system().quaternion() * DVec3::X,
                expected,
                1.0,
                1e-11,
            );
            assert_eq!(
                state.angular_velocity_in_system().radians_per_second(),
                tilt.quaternion() * (DVec3::Z * rate)
            );
        }
    }
}

fn authority(world: &CelestialSystem) -> (u64, SimulationInstant, Vec<BodyState>) {
    (
        world.revision(),
        world.sample_time(),
        world.bodies().map(|(_, b)| *b.state()).collect(),
    )
}

#[test]
fn convergence_arithmetic_and_stale_binding_failures_are_transactional() {
    let (mut world, _, planet) = fixture(1.5e11, 0.6, YEAR);
    let star = world.bodies().next().unwrap().0;
    let definition = CelestialMotionDefinition::new(
        &world,
        &[
            fixed(star, DVec3::ZERO, 0.0),
            orbit(planet, star, 1.5e11, 0.6, YEAR),
        ],
    )
    .unwrap();
    let mut producer = AnalyticMotionProducer::with_iteration_limit(&world, definition, 1).unwrap();
    let before = authority(&world);
    assert_eq!(
        producer.sample(&mut world, time(0.25 * YEAR)),
        Err(AnalyticMotionError::Convergence { iterations: 1 })
    );
    assert_eq!(authority(&world), before);
    // Scratch was partially evaluated; a subsequent successful sample still replaces all states.
    producer.sample(&mut world, time(0.0)).unwrap();
    world
        .edit_properties(planet, BodyProperties::new(2.0, 2.0).unwrap())
        .unwrap();
    let before = authority(&world);
    assert_eq!(
        producer.sample(&mut world, time(0.0)),
        Err(AnalyticMotionError::StaleBinding)
    );
    assert_eq!(authority(&world), before);
    let definition = CelestialMotionDefinition::new(
        &world,
        &[
            fixed(star, DVec3::ZERO, 0.0),
            orbit(planet, star, f64::MAX, 0.5, 1.0),
        ],
    )
    .unwrap();
    let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    let before = authority(&world);
    assert_eq!(
        producer.sample(&mut world, time(0.25)),
        Err(AnalyticMotionError::UnrepresentableArithmetic)
    );
    assert_eq!(authority(&world), before);
    insert(&mut world, "appended");
    let before = authority(&world);
    assert_eq!(
        producer.sample(&mut world, time(0.0)),
        Err(AnalyticMotionError::StaleBinding)
    );
    assert_eq!(authority(&world), before);
}

#[test]
fn bounded_time_cycles_underflow_and_empty_namespace_checks() {
    for period in [1e-300, f64::MAX] {
        let (mut world, mut producer, _) = fixture(f64::MIN_POSITIVE, 0.9, period);
        let before = authority(&world);
        assert!(producer.sample(&mut world, time(1000.0)).is_err());
        assert_eq!(authority(&world), before);
    }
    let mut a = CelestialSystem::new(ns(1), time(0.0));
    let mut b = CelestialSystem::new(ns(2), time(0.0));
    let definition = CelestialMotionDefinition::new(&a, &[]).unwrap();
    let mut producer = AnalyticMotionProducer::new(&a, definition).unwrap();
    assert_eq!(
        producer.sample(&mut b, time(0.0)),
        Err(AnalyticMotionError::StaleBinding)
    );
    assert_eq!(producer.sample(&mut a, time(0.0)).unwrap().body_count, 0);
}

#[test]
fn vanished_tangential_velocity_intermediate_is_rejected_before_publication() {
    // All authored inputs and the final mathematical periapsis velocity are finite.
    // This subnormal fixture is outside the producer's representable-intermediate
    // envelope: multiplying speed by beta would silently erase the numerator.
    let (mut world, mut producer, _) = fixture(1e-300, 1.0 - f64::EPSILON, 1e20);
    let before = authority(&world);
    assert_eq!(
        producer.sample(&mut world, time(0.0)),
        Err(AnalyticMotionError::UnrepresentableArithmetic)
    );
    assert_eq!(authority(&world), before);
}

#[test]
fn sampled_projection_preserves_millimetre_attachments_below_large_shared_translation() {
    let mut world = CelestialSystem::new(ns(1), time(0.0));
    let star = insert(&mut world, "star");
    let planet = insert(&mut world, "planet");
    let mut motion = orbit(planet, star, 1.5e11, 0.6, YEAR);
    motion.spin = spin(1e-4);
    let definition =
        CelestialMotionDefinition::new(&world, &[motion, fixed(star, DVec3::splat(1e16), 0.0)])
            .unwrap();
    let mut producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    let mut projection = CelestialFrameProjection::build(&world, ns(10)).unwrap();
    for requested in [0.0, YEAR * 0.25, -1000.0 * YEAR, 1000.0 * YEAR] {
        producer.sample(&mut world, time(requested)).unwrap();
        projection.publish(&world).unwrap();
        let frames = projection.frames_for(planet).unwrap();
        let view = projection.coherent_view(&world).unwrap();
        let evaluation = view.projection().tree().evaluate();
        let source = FramePosition::new(
            frames.body_fixed,
            LocalPosition::try_metres(DVec3::new(0.001, -0.002, 0.003)).unwrap(),
        );
        let converted = evaluation
            .convert_position(source, frames.translating)
            .unwrap();
        let expected = world
            .body(planet)
            .unwrap()
            .state()
            .body_to_system()
            .quaternion()
            * source.local().metres();
        vector_check(
            "sampled shared LCA millimetres",
            converted.local().metres(),
            expected,
            0.001,
            1e-11,
        );
        assert!(
            (converted.local().metres().length() - source.local().metres().length()).abs() <= 1e-15
        );
    }
}

#[test]
fn nearly_parabolic_periapsis_remains_finite_without_eccentricity_clamping() {
    let e = 1.0 - f64::EPSILON;
    let (mut world, mut producer, planet) = fixture(1.0, e, 1.0);
    for t in [0.0, 1e-20, -1e-20, 0.25, -0.25] {
        let stats = producer.sample(&mut world, time(t)).unwrap();
        assert!(stats.maximum_solver_iterations <= 64);
        let (p, v) = vectors(&world, planet);
        assert!(p.is_finite() && v.is_finite());
    }
    producer.sample(&mut world, time(0.0)).unwrap();
    let (p, v) = vectors(&world, planet);
    vector_check(
        "near parabolic periapsis position",
        p,
        DVec3::X * (1.0 - e),
        1.0,
        1e-11,
    );
    vector_check(
        "near parabolic periapsis velocity",
        v,
        DVec3::Y * TAU * ((1.0 + e) / (1.0 - e)).sqrt(),
        v.length(),
        1e-10,
    );
}
