mod common;
use common::*;
use glam::DVec3;
use mundaris_simulation::*;
use mundaris_world::*;
use std::time::Duration;

fn runner(world: &CelestialSystem, work: u32, debt: u64, slots: usize) -> FixedStepRunner {
    FixedStepRunner::new(
        world,
        SimulationConfig::try_new(10.0)
            .unwrap()
            .with_limits(work, debt, slots, 16 * 1024 * 1024)
            .unwrap(),
    )
    .unwrap()
}
fn states(world: &CelestialSystem) -> Vec<BodyState> {
    world.bodies().map(|(_, b)| *b.state()).collect()
}
fn drain(r: &mut FixedStepRunner, w: &mut CelestialSystem) {
    loop {
        let report = r.pump(w, |_, _| {}).unwrap();
        if report.backlog_ticks == 0 && report.replay_remaining.is_none() {
            break;
        }
    }
}
#[test]
fn fps_equal_exact_duration_and_reverse_demand() {
    for rate in [0.1, 1.0, 10.0, 100.0, 1000.0, -10.0, -1000.0] {
        let mut expected = None;
        for fps in [30, 60, 144, 317] {
            let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
            let mut r = runner(&w, 512, 65536, 2048);
            if rate < 0.0 {
                r.seek_tick(1100).unwrap();
                drain(&mut r, &mut w);
            }
            r.set_rate(PlaybackRate::try_multiplier(rate).unwrap());
            r.set_paused(false);
            let total = 10_000_000_000u64;
            let partitions = fps * 10;
            let slice = total / partitions;
            for i in 0..partitions {
                let ns = if i + 1 == partitions {
                    total - slice * (partitions - 1)
                } else {
                    slice
                };
                r.admit_wall_elapsed(Duration::from_nanos(ns)).unwrap();
                r.pump(&mut w, |_, _| {}).unwrap();
                r.admit_wall_elapsed(Duration::ZERO).unwrap();
                r.pump(&mut w, |_, _| {}).unwrap();
            }
            drain(&mut r, &mut w);
            let actual = (r.tick(), states(&w), r.report().requested_time);
            if let Some(e) = &expected {
                assert_eq!(&actual, e, "rate {rate} fps {fps}");
            } else {
                expected = Some(actual);
            }
        }
    }
}
#[test]
fn boundaries_fractions_pause_rate_backlog_overload_and_lifecycle() {
    let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&w, 4, 16, 8);
    r.set_paused(false);
    r.admit_wall_elapsed(Duration::from_nanos(9_999_999_999))
        .unwrap();
    assert_eq!(r.report().target_tick, 0);
    r.admit_wall_elapsed(Duration::from_nanos(1)).unwrap();
    assert_eq!(r.report().target_tick, 1);
    r.pump(&mut w, |_, _| {}).unwrap();
    assert_eq!(r.tick(), 1);
    r.admit_wall_elapsed(Duration::from_secs(160)).unwrap();
    let requested = r.report().requested_time;
    let report = r.pump(&mut w, |_, _| {}).unwrap();
    assert_eq!(report.work_steps, 4);
    assert_eq!(report.backlog_ticks, 12);
    r.admit_wall_elapsed(Duration::from_secs(100)).unwrap();
    assert_eq!(r.report().requested_time, requested);
    assert_eq!(r.report().status, PlaybackStatus::DemandHaltedOverload);
    assert_eq!(r.report().rejected_simulation_seconds, 100.0);
    r.resume_admission();
    assert_eq!(r.report().backlog_ticks, 12);
    drain(&mut r, &mut w);
    assert_eq!(r.tick(), 17);
    r.admit_wall_elapsed(Duration::from_secs(5)).unwrap();
    r.set_rate(PlaybackRate::try_multiplier(10.0).unwrap());
    assert_eq!(r.report().fractional_seconds, 5.0);
    r.set_paused(true);
    assert_eq!(r.report().pending_simulation_seconds, 0.0);
    for _ in 0..100 {
        r.admit_wall_elapsed(Duration::from_secs(100)).unwrap();
        assert_eq!(r.pump(&mut w, |_, _| {}).unwrap().work_steps, 0);
    }
    r.single_step(true).unwrap();
    assert_eq!(r.pump(&mut w, |_, _| {}).unwrap().forward_steps, 1);
    assert!(r.paused());
    r.single_step(false).unwrap();
    assert_eq!(r.pump(&mut w, |_, _| {}).unwrap().restored_steps, 1);
    r.set_paused(false);
    r.set_lifecycle_suspended(true);
    r.admit_wall_elapsed(Duration::from_secs(1_000_000))
        .unwrap();
    assert_eq!(r.pump(&mut w, |_, _| {}).unwrap().work_steps, 0);
    r.set_lifecycle_suspended(false);
    assert_eq!(r.report().backlog_ticks, 0);
    assert_eq!(r.quantize_seek_seconds(25.0).unwrap().tick, 2);
    assert!(r.quantize_seek_seconds(-1.0).is_err());
    r.set_rate(PlaybackRate::try_multiplier(-10.0).unwrap());
    assert_eq!(r.report().pending_simulation_seconds, 0.0);
}
#[test]
fn exact_snapshot_reverse_private_replay_cancel_reset_and_stale() {
    let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&w, 4, 65536, 8);
    r.seek_tick(10000).unwrap();
    drain(&mut r, &mut w);
    let expected = states(&w);
    r.reset_branch(&mut w).unwrap();
    r.seek_tick(10000).unwrap();
    drain(&mut r, &mut w);
    assert_eq!(states(&w), expected);
    r.single_step(false).unwrap();
    drain(&mut r, &mut w);
    let previous = states(&w);
    r.single_step(true).unwrap();
    drain(&mut r, &mut w);
    assert_eq!(states(&w), expected);
    r.seek_tick(9999).unwrap();
    drain(&mut r, &mut w);
    assert_eq!(states(&w), previous);
    r.seek_tick(123).unwrap();
    let live = states(&w);
    let revision = w.revision();
    let report = r
        .pump(&mut w, |_, _| panic!("private replay must not publish"))
        .unwrap();
    assert_eq!(report.work_steps, 4);
    assert!(report.replay_remaining.is_some());
    assert_eq!(states(&w), live);
    assert_eq!(w.revision(), revision);
    r.cancel_seek();
    assert_eq!(states(&w), live);
    assert_eq!(r.report().backlog_ticks, 0);
    r.seek_tick(123).unwrap();
    drain(&mut r, &mut w);
    assert_eq!(r.tick(), 123);
    let ids: Vec<_> = w.bodies().map(|(id, _)| id).collect();
    r.reset_branch(&mut w).unwrap();
    assert_eq!(r.tick(), 0);
    assert_eq!(w.bodies().map(|(id, _)| id).collect::<Vec<_>>(), ids);
    let id = ids[0];
    w.edit_properties(id, BodyProperties::new(M, 2e6).unwrap())
        .unwrap();
    assert!(matches!(
        r.pump(&mut w, |_, _| {}).unwrap_err().source,
        SimulationError::StaleSession
    ));
    r.rebranch_after_edit(&w).unwrap();
    assert_eq!(r.branch_generation(), 1);
}
#[test]
fn edit_preflight_rejects_without_changing_session_and_name_retains_history() {
    let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&w, 512, 65536, 2048);
    r.seek_tick(100).unwrap();
    drain(&mut r, &mut w);
    r.set_paused(false);
    let ids: Vec<_> = w.bodies().map(|(id, _)| id).collect();
    let old = states(&w);
    let revision = w.revision();
    let report = r.report();
    let coincident = state(old[0].center_in_system().metres(), DVec3::ZERO);
    assert!(r.edit_state(&mut w, ids[1], coincident).is_err());
    let invalid_diagnostic = state(old[1].center_in_system().metres(), DVec3::X * 1e200);
    assert!(r.edit_state(&mut w, ids[1], invalid_diagnostic).is_err());
    assert_eq!(states(&w), old);
    assert_eq!(w.revision(), revision);
    assert!(!r.paused());
    assert_eq!(r.report().retained_ticks, report.retained_ticks);
    r.rename(&mut w, ids[1], "renamed").unwrap();
    assert_eq!(r.tick(), 100);
    assert!(!r.paused());
    assert_eq!(r.report().retained_ticks, report.retained_ticks);
    r.edit_properties(
        &mut w,
        ids[1],
        BodyProperties::new(SMALL * 2.0, 1e5).unwrap(),
    )
    .unwrap();
    assert_eq!(states(&w), old);
    assert_eq!(r.tick(), 0);
    assert!(r.paused());
    assert_eq!(r.branch_generation(), 1);
    assert_eq!(r.report().retained_ticks, Some((0, 0)));
    r.single_step(true).unwrap();
    drain(&mut r, &mut w);
    r.reset_branch(&mut w).unwrap();
    assert_eq!(states(&w), old);
    assert_eq!(w.body(ids[1]).unwrap().properties().mass_kg(), SMALL * 2.0);
    assert!(r.report().history_payload_bytes <= 16 * 1024 * 1024);
}
#[test]
fn configuration_epoch_overflow_and_numerical_failure_report() {
    for h in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(SimulationConfig::try_new(h).is_err());
    }
    let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    assert!(
        FixedStepRunner::new(
            &w,
            SimulationConfig::try_new(10.0)
                .unwrap()
                .with_limits(4, 16, 8, 100)
                .unwrap()
        )
        .is_err()
    );
    let updates: Vec<_> = w
        .bodies()
        .map(|(body, b)| BodyStateUpdate {
            body,
            state: *b.state(),
        })
        .collect();
    w.update_states(
        SimulationInstant::try_seconds_since_epoch(1e20).unwrap(),
        &updates,
    )
    .unwrap();
    assert!(matches!(
        FixedStepRunner::new(&w, SimulationConfig::try_new(10.0).unwrap()),
        Err(SimulationError::TimeResolution)
    ));
    let mut w = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&w, 4, 16, 8);
    assert!(r.seek_tick(u64::MAX).is_err());
    assert_eq!(r.tick(), 0);
    r.set_rate(PlaybackRate::try_multiplier(f64::MAX).unwrap());
    r.set_paused(false);
    assert!(r.admit_wall_elapsed(Duration::from_secs(2)).is_err());
    assert_eq!(r.report().target_tick, 0);
    let id = w.bodies().nth(1).unwrap().0;
    let s = *w.body(id).unwrap().state();
    r.edit_state(
        &mut w,
        id,
        state(s.center_in_system().metres(), -DVec3::X * 300000.0),
    )
    .unwrap();
    r.set_paused(false);
    r.admit_wall_elapsed(Duration::from_secs(40)).unwrap();
    let error = r.pump(&mut w, |_, _| {}).unwrap_err();
    assert!(error.report.forward_steps > 0);
    assert_eq!(error.report.tick, r.tick());
    assert_eq!(w.sample_time(), error.report.authoritative_time);
}

#[test]
fn hundred_retained_snapshots_restore_exactly_and_origin_pauses() {
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&world, 512, 65536, 2048);
    let mut snapshots = vec![states(&world)];
    for _ in 0..100 {
        r.single_step(true).unwrap();
        r.pump(&mut world, |_, _| {}).unwrap();
        snapshots.push(states(&world));
    }
    r.set_rate(PlaybackRate::try_multiplier(-1000.0).unwrap());
    r.set_paused(false);
    for tick in (0..100).rev() {
        r.admit_wall_elapsed(Duration::from_millis(10)).unwrap();
        let report = r.pump(&mut world, |_, _| {}).unwrap();
        assert_eq!(report.restored_steps, 1);
        assert_eq!(states(&world), snapshots[tick]);
    }
    assert!(r.paused());
    assert_eq!(r.report().status, PlaybackStatus::OriginBoundary);
}

#[test]
fn thousand_gravity_commits_projection_coherence_and_rebuild() {
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = runner(&world, 512, 65536, 2048);
    let mut projection =
        CelestialFrameProjection::build(&world, std::num::NonZeroU64::new(1).unwrap()).unwrap();
    for _ in 0..1000 {
        r.single_step(true).unwrap();
        r.pump(&mut world, |_, _| {}).unwrap();
        assert!(projection.coherent_view(&world).is_err());
        projection.publish(&world).unwrap();
        let paired = projection.coherent_view(&world).unwrap();
        let evaluation = paired.evaluation();
        for (id, body) in paired.system().bodies() {
            let frames = paired.projection().frames_for(id).unwrap();
            let anchor = evaluation.state(frames.translating).unwrap();
            assert_eq!(
                anchor.parent_from_local().translation().metres(),
                body.state().center_in_system().metres()
            );
            assert_eq!(
                anchor.motion().unwrap().origin_velocity_in_parent(),
                body.state().center_velocity_in_system()
            );
            assert_eq!(
                evaluation
                    .state(frames.body_fixed)
                    .unwrap()
                    .parent_from_local()
                    .translation(),
                mundaris_math::Displacement3::zero()
            );
        }
    }
    let states_before = states(&world);
    let revision = world.revision();
    let rebuilt =
        CelestialFrameProjection::build(&world, std::num::NonZeroU64::new(2).unwrap()).unwrap();
    for (id, _) in world.bodies() {
        let old = projection.frames_for(id).unwrap().body_fixed;
        let new = rebuilt.frames_for(id).unwrap().body_fixed;
        assert_ne!(old, new);
        assert_eq!(
            projection.tree().evaluate().transform_to_root(old).unwrap(),
            rebuilt.tree().evaluate().transform_to_root(new).unwrap()
        );
    }
    assert_eq!(world.revision(), revision);
    assert_eq!(states(&world), states_before);
}

#[test]
fn direct_headless_ticks_append_staleness_and_small_history_cap() {
    let mut world = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut r = FixedStepRunner::new(
        &world,
        SimulationConfig::try_new(10.0)
            .unwrap()
            .with_limits(4, 16, 2048, 1024)
            .unwrap(),
    )
    .unwrap();
    assert!(r.report().history_payload_bytes <= 1024);
    assert!(r.report().replay_history_payload_bytes <= 1024);
    r.request_forward_to_tick(12).unwrap();
    let report = r.pump(&mut world, |_, _| {}).unwrap();
    assert_eq!(report.work_steps, 4);
    assert_eq!(report.tick, 4);
    drain(&mut r, &mut world);
    assert_eq!(r.tick(), 12);
    assert!(r.request_forward_to_tick(11).is_err());
    world
        .insert_body(
            "appended",
            BodyProperties::new(1.0, 1.0).unwrap(),
            state(DVec3::Z * 1e8, DVec3::ZERO),
        )
        .unwrap();
    assert!(matches!(
        r.pump(&mut world, |_, _| {}).unwrap_err().source,
        SimulationError::StaleSession
    ));
    r.rebranch_after_edit(&world).unwrap();
    assert_eq!(r.tick(), 0);
    assert_eq!(r.branch_generation(), 1);
}

#[test]
fn orientation_and_spin_branch_do_not_change_gravity_or_translation() {
    let mut a = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut b = circular(0.0, DVec3::ZERO, DVec3::ZERO);
    let mut ar = runner(&a, 512, 65536, 2048);
    let mut br = runner(&b, 512, 65536, 2048);
    let id = b.bodies().nth(1).unwrap().0;
    let old = *b.body(id).unwrap().state();
    let changed = BodyState::new(
        old.center_in_system(),
        old.center_velocity_in_system(),
        mundaris_math::UnitRotation::from_axis_angle(
            mundaris_math::Direction3::try_new(DVec3::Y).unwrap(),
            0.7,
        )
        .unwrap(),
        mundaris_math::AngularVelocity3::try_radians_per_second(DVec3::new(0.0001, 0.0002, 0.0003))
            .unwrap(),
    );
    br.edit_state(&mut b, id, changed).unwrap();
    ar.request_forward_to_tick(1000).unwrap();
    br.request_forward_to_tick(1000).unwrap();
    drain(&mut ar, &mut a);
    drain(&mut br, &mut b);
    for (a, b) in states(&a).into_iter().zip(states(&b)) {
        assert_eq!(a.center_in_system(), b.center_in_system());
        assert_eq!(a.center_velocity_in_system(), b.center_velocity_in_system());
    }
}
