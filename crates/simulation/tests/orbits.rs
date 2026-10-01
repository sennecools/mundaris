mod common;
use common::*;
use glam::DVec3;
use mundaris_math::*;
use mundaris_simulation::*;

#[derive(Debug, Default)]
struct Maxima {
    radius: f64,
    energy: f64,
    p: f64,
    l: f64,
    com: f64,
    phase: f64,
    final_phase: f64,
    final_time_s: f64,
    period: f64,
    oracle_crossing_interpolation_s: f64,
}
fn run(e: f64, periods: f64, h: f64, boosted: bool) -> Maxima {
    let (offset, boost) = if boosted {
        (DVec3::new(1e9, -2e9, 3e9), DVec3::new(100.0, -200.0, 50.0))
    } else {
        (DVec3::ZERO, DVec3::ZERO)
    };
    let mut world = circular(e, offset, boost);
    let initial = system_diagnostics(&world).unwrap();
    let baseline = DiagnosticBaseline(initial);
    let expected_energy = -G * M * SMALL / (2.0 * R) + 0.5 * (M + SMALL) * boost.length_squared();
    assert!((initial.total_energy_j() - expected_energy).abs() / expected_energy.abs() <= 2e-14);
    let reduced = M * (SMALL / (M + SMALL));
    let expected_l = reduced * (G * (M + SMALL) * R * (1.0 - e * e)).sqrt();
    assert!((initial.angular_momentum_com_kg_m2_s.z - expected_l).abs() / expected_l <= 2e-14);
    let mut work = IntegrationWorkspace::new(&world, h).unwrap();
    let steps = (periods * period() / h).floor() as u64;
    let mut max = Maxima::default();
    let mut previous_angle = 0.0;
    let mut unwrapped = 0.0;
    let mut crossing = std::f64::consts::TAU;
    let mut last_return = 0.0;
    let mut returns = 0;
    for k in 1..=steps {
        work.prepare_step().unwrap();
        work.commit_candidate(
            &mut world,
            SimulationInstant::try_seconds_since_epoch(k as f64 * h).unwrap(),
        )
        .unwrap();
        let drift = baseline.drift(system_diagnostics(&world).unwrap());
        max.energy = max.energy.max(drift.relative_energy.abs());
        max.p = max.p.max(drift.normalized_momentum);
        max.l = max.l.max(drift.normalized_angular_momentum);
        max.com = max.com.max(drift.com_residual_m.length());
        let relative = work.states()[1].center_in_system().metres()
            - work.states()[0].center_in_system().metres();
        max.radius = max.radius.max((relative.length() / R - 1.0).abs());
        if e == 0.0 {
            let angle = relative.y.atan2(relative.x);
            let delta = (angle - previous_angle + std::f64::consts::PI)
                .rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            let old = unwrapped;
            unwrapped += delta;
            previous_angle = angle;
            max.final_phase = (unwrapped - std::f64::consts::TAU * (k as f64 * h / period())).abs();
            max.phase = max.phase.max(max.final_phase);
            if unwrapped >= crossing {
                let time = (k - 1) as f64 * h + h * (crossing - old) / delta;
                if returns < 20 {
                    max.period = max
                        .period
                        .max(((time - last_return) / period() - 1.0).abs());
                    let oracle_return = (returns + 1) as f64 * period();
                    let k0 = (oracle_return / h).floor();
                    let t0 = k0 * h;
                    let n = std::f64::consts::TAU / period();
                    let interpolated = t0
                        + h * ((returns + 1) as f64 * std::f64::consts::TAU - n * t0)
                            / (n * (t0 + h) - n * t0);
                    max.oracle_crossing_interpolation_s = max
                        .oracle_crossing_interpolation_s
                        .max((interpolated - oracle_return).abs());
                }
                last_return = time;
                crossing += std::f64::consts::TAU;
                returns += 1;
            }
        }
    }
    max.final_time_s = steps as f64 * h;
    eprintln!(
        "e={e} h={h} periods={periods} boosted={boosted} steps={steps} T={} maxima={max:?}",
        period()
    );
    assert!(max.p <= 1e-10 && max.l <= 1e-10);
    assert!(max.com <= 0.1);
    if e == 0.0 {
        assert!(max.radius <= 2e-5);
        assert!(max.energy <= 1e-7);
        assert!(max.period <= 1e-5);
        assert!(max.oracle_crossing_interpolation_s <= 1e-8);
        assert!(max.phase <= if periods > 100.0 { 0.03 } else { 0.003 });
    } else {
        assert!(max.energy <= 2e-5);
    }
    max
}
#[test]
fn circular_and_eccentric_100_periods() {
    run(0.0, 100.0, 10.0, false);
    run(0.3, 100.0, 10.0, false);
}
#[test]
fn boosted_straight_line_com_100_periods() {
    run(0.0, 100.0, 10.0, true);
}
#[test]
fn second_order_phase_convergence() {
    let coarse = run(0.0, 20.0, 10.0, false);
    // Common final instant, no shortened step: fine run has exactly twice coarse ticks.
    let periods = ((20.0 * period() / 10.0).floor() * 10.0) / period();
    let fine = run(0.0, periods, 5.0, false);
    assert_eq!(coarse.final_time_s, fine.final_time_s);
    let ratio = fine.final_phase / coarse.final_phase;
    eprintln!("common endpoint phase ratio={ratio}");
    assert!((0.20..=0.35).contains(&ratio), "ratio {}", ratio);
}
#[test]
#[ignore = "focused release: 1000 orbital periods"]
fn long_run_circular_eccentric() {
    run(0.0, 1000.0, 10.0, false);
    let short = run(0.3, 100.0, 10.0, false);
    let long = run(0.3, 1000.0, 10.0, false);
    assert!(long.energy <= 2.0 * short.energy + 1e-9);
}
#[test]
#[ignore = "focused release: two outer periods"]
fn long_run_hierarchy() {
    let mut world = hierarchy();
    let baseline = DiagnosticBaseline(system_diagnostics(&world).unwrap());
    let mut work = IntegrationWorkspace::new(&world, 60.0).unwrap();
    let t = std::f64::consts::TAU
        * (1.5e11_f64.powi(3) / (G * (1.98847e30 + 5.9722e24 + 7.342e20))).sqrt();
    let steps = (2.0 * t / 60.0).floor() as u64;
    let mut max = Maxima::default();
    let mut min_radius = f64::MAX;
    let mut max_radius: f64 = 0.0;
    for k in 1..=steps {
        work.prepare_step().unwrap();
        work.commit_candidate(
            &mut world,
            SimulationInstant::try_seconds_since_epoch(k as f64 * 60.0).unwrap(),
        )
        .unwrap();
        let drift = baseline.drift(system_diagnostics(&world).unwrap());
        max.energy = max.energy.max(drift.relative_energy.abs());
        max.p = max.p.max(drift.normalized_momentum);
        max.l = max.l.max(drift.normalized_angular_momentum);
        max.com = max.com.max(drift.com_residual_m.length());
        let r = (work.states()[2].center_in_system().metres()
            - work.states()[1].center_in_system().metres())
        .length();
        min_radius = min_radius.min(r);
        max_radius = max_radius.max(r);
    }
    eprintln!("hierarchy h60 steps={steps} T={t} maxima={max:?} moon=[{min_radius},{max_radius}]");
    assert!(max.energy <= 2e-6 && max.p <= 1e-9 && max.l <= 1e-9 && max.com <= 1500.0);
    assert!(min_radius >= 0.9e8 && max_radius <= 1.1e8);
}
#[test]
fn constant_system_axis_spin_100000_steps() {
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let id = world.bodies().next().unwrap().0;
    let original = *world.body(id).unwrap().state();
    let q = UnitRotation::from_axis_angle(Direction3::try_new(DVec3::Y).unwrap(), 0.7).unwrap();
    let omega = DVec3::new(0.0001, 0.0002, -0.0003);
    world
        .edit_state(
            id,
            mundaris_world::BodyState::new(
                original.center_in_system(),
                original.center_velocity_in_system(),
                q,
                AngularVelocity3::try_radians_per_second(omega).unwrap(),
            ),
        )
        .unwrap();
    let mut work = IntegrationWorkspace::new(&world, 10.0).unwrap();
    let mut max_norm: f64 = 0.0;
    let mut max_basis: f64 = 0.0;
    for k in 1..=100000 {
        work.prepare_step().unwrap();
        work.commit_candidate(
            &mut world,
            SimulationInstant::try_seconds_since_epoch(k as f64 * 10.0).unwrap(),
        )
        .unwrap();
        let actual = work.states()[0].body_to_system().quaternion();
        let expected =
            glam::DQuat::from_axis_angle(omega.normalize(), omega.length() * k as f64 * 10.0)
                * q.quaternion();
        max_norm = max_norm.max((actual.length_squared() - 1.0).abs());
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            max_basis = max_basis.max((actual * axis - expected * axis).length());
        }
    }
    eprintln!("spin maxima norm={max_norm} basis={max_basis}");
    assert!(max_norm <= 1e-12 && max_basis <= 1e-9);
}

#[test]
fn radius_independence_full_velocity_and_failing_candidate_rollback() {
    let mut a = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut b = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let ids: Vec<_> = b.bodies().map(|(id, _)| id).collect();
    for id in ids {
        let properties = *b.body(id).unwrap().properties();
        b.edit_properties(
            id,
            mundaris_world::BodyProperties::new(
                properties.mass_kg(),
                100.0 * properties.reference_radius_m(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut aw = IntegrationWorkspace::new(&a, 10.0).unwrap();
    let mut bw = IntegrationWorkspace::new(&b, 10.0).unwrap();
    for k in 1..=1000 {
        let old = aw.states().to_vec();
        aw.prepare_step().unwrap();
        bw.prepare_step().unwrap();
        let instant = SimulationInstant::try_seconds_since_epoch(k as f64 * 10.0).unwrap();
        aw.commit_candidate(&mut a, instant).unwrap();
        bw.commit_candidate(&mut b, instant).unwrap();
        assert_eq!(aw.states(), bw.states());
        if k == 1 {
            let mut old_accel = [DVec3::ZERO; 2];
            let mut next_accel = [DVec3::ZERO; 2];
            evaluate_accelerations(
                &[M, SMALL],
                &old.iter()
                    .map(|s| s.center_in_system().metres())
                    .collect::<Vec<_>>(),
                &mut old_accel,
            )
            .unwrap();
            evaluate_accelerations(
                &[M, SMALL],
                &aw.states()
                    .iter()
                    .map(|s| s.center_in_system().metres())
                    .collect::<Vec<_>>(),
                &mut next_accel,
            )
            .unwrap();
            for i in 0..2 {
                let expected = old[i].center_velocity_in_system().metres_per_second()
                    + 5.0 * old_accel[i]
                    + 5.0 * next_accel[i];
                assert!(
                    (expected
                        - aw.states()[i]
                            .center_velocity_in_system()
                            .metres_per_second())
                    .length()
                        <= 1e-12
                );
            }
        }
    }
    // Old position resolves, but this authored inward flyby fails at drifted positions.
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let id = world.bodies().nth(1).unwrap().0;
    let s = *world.body(id).unwrap().state();
    world
        .edit_state(
            id,
            state(s.center_in_system().metres(), -DVec3::X * 900000.0),
        )
        .unwrap();
    let mut work = IntegrationWorkspace::new(&world, 10.0).unwrap();
    let original = work.states().to_vec();
    let revision = world.revision();
    assert!(matches!(
        work.prepare_step(),
        Err(SimulationError::Gravity(
            GravityError::UnresolvedEncounter { .. }
        ))
    ));
    assert!(
        work.commit_candidate(
            &mut world,
            SimulationInstant::try_seconds_since_epoch(10.0).unwrap()
        )
        .is_err()
    );
    assert_eq!(work.states(), original);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.sample_time(), SimulationInstant::ZERO);
}
