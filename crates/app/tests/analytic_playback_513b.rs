use glam::DVec3;
use mundaris_app::{GravityOrbitsDemo, orbit_guides::OrbitGuideReference};
use mundaris_math::{FramePosition, LocalPosition};
use mundaris_simulation::ANALYTIC_TIME_LIMIT_SECONDS;
use mundaris_world::{BodyId, CelestialSystem};
use std::time::Duration;

fn bits(world: &CelestialSystem) -> Vec<(BodyId, [u64; 13])> {
    world
        .bodies()
        .map(|(id, body)| {
            let state = *body.state();
            let p = state.center_in_system().metres().to_array();
            let v = state
                .center_velocity_in_system()
                .metres_per_second()
                .to_array();
            let q = state.body_to_system().quaternion().to_array();
            let w = state
                .angular_velocity_in_system()
                .radians_per_second()
                .to_array();
            (
                id,
                [
                    p[0].to_bits(),
                    p[1].to_bits(),
                    p[2].to_bits(),
                    v[0].to_bits(),
                    v[1].to_bits(),
                    v[2].to_bits(),
                    q[0].to_bits(),
                    q[1].to_bits(),
                    q[2].to_bits(),
                    q[3].to_bits(),
                    w[0].to_bits(),
                    w[1].to_bits(),
                    w[2].to_bits(),
                ],
            )
        })
        .collect()
}

fn body_id(world: &CelestialSystem, authored_name: &str) -> BodyId {
    world
        .bodies()
        .find_map(|(id, body)| (body.name() == authored_name).then_some(id))
        .unwrap_or_else(|| panic!("shared test system is missing body {authored_name}"))
}

fn assert_pose_near(actual: mundaris_math::FramePose, expected: mundaris_math::FramePose) {
    assert_eq!(actual.position().frame(), expected.position().frame());
    assert!(
        (actual.position().local().metres() - expected.position().local().metres()).length() < 1e-6
    );
    let a = actual.orientation().quaternion();
    let b = expected.orientation().quaternion();
    assert!(a.dot(b).abs() > 1.0 - 1e-14);
}

#[test]
fn analytic_shared_system_seeks_history_independently_and_rejects_bad_targets_transactionally() {
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    assert_eq!(demo.motion_snapshot().mode, "prescribed_analytic");
    let moon = body_id(demo.world(), "Moon");
    let epoch = demo.motion_snapshot().published_time_s;
    let expected = [
        0.0,
        0.375,
        -0.625,
        31_557_600.0,
        -31_557_600.0,
        31_557_600_000.0,
        -31_557_600_000.0,
    ];
    for &time in &expected {
        demo.seek_seconds(time).unwrap();
        let state = bits(demo.world());
        for other in [time + 123.0, time - 987.0, epoch] {
            demo.seek_seconds(other).unwrap();
        }
        demo.seek_seconds(time).unwrap();
        assert_eq!(bits(demo.world()), state, "time={time}");
    }
    demo.seek_seconds(0.0).unwrap();
    let moon_at_epoch = demo
        .world()
        .body(moon)
        .unwrap()
        .state()
        .center_in_system()
        .metres()
        .to_array();
    demo.seek_seconds(0.375).unwrap();
    assert_ne!(
        demo.world()
            .body(moon)
            .unwrap()
            .state()
            .center_in_system()
            .metres()
            .to_array(),
        moon_at_epoch,
        "the authored Moon orbit must move under analytic playback"
    );
    let revision = demo.world().revision();
    let time = demo.motion_snapshot().published_time_s;
    demo.set_paused(true).unwrap();
    demo.update(Duration::from_secs(1));
    assert_eq!(demo.world().revision(), revision);
    for rate in [-1000.0, -1.0, 0.0, 1.0, 1000.0] {
        demo.set_playback_rate(rate).unwrap();
        assert_eq!(demo.motion_snapshot().rate, rate);
    }
    demo.set_playback_rate(1.0).unwrap();
    demo.single_step(true).unwrap();
    assert_eq!(demo.motion_snapshot().published_time_s, time + 60.0);
    demo.reset_motion().unwrap();
    assert_eq!(demo.motion_snapshot().published_time_s, epoch);

    let before = bits(demo.world());
    let revision = demo.world().revision();
    let published = demo.motion_snapshot().published_time_s;
    assert!(demo.seek_seconds(f64::NAN).is_err());
    assert_eq!(bits(demo.world()), before);
    assert_eq!(demo.world().revision(), revision);
    assert_eq!(demo.motion_snapshot().published_time_s, published);
    assert!(demo.motion_snapshot().latest_failure.is_some());
    assert!(
        demo.seek_seconds(ANALYTIC_TIME_LIMIT_SECONDS + 1.0)
            .is_err()
    );
    assert_eq!(
        demo.motion_snapshot().requested_time_s,
        ANALYTIC_TIME_LIMIT_SECONDS + 1.0
    );
    assert!(demo.motion_snapshot().latest_failure.is_some());
    assert!(demo.seek_seconds(epoch + 0.25).is_ok());
    assert_eq!(demo.motion_snapshot().published_time_s, epoch + 0.25);
    assert!(demo.motion_snapshot().latest_failure.is_none());
    let recovered = demo.motion_snapshot().published_time_s;
    let revision = demo.world().revision();
    assert!(demo.set_playback_rate(f64::INFINITY).is_err());
    assert_eq!(demo.world().revision(), revision);
    assert_eq!(demo.motion_snapshot().published_time_s, recovered);
}

#[test]
fn analytic_playback_trails_guides_and_selection_remain_separate_from_authority() {
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    let selected = body_id(demo.world(), "Moon");
    let rust = body_id(demo.world(), "Rust");
    let sol = body_id(demo.world(), "Sol");
    assert!(!demo.guides().is_empty());
    demo.select_body(selected).unwrap();
    demo.focus_selected(false).unwrap();
    assert_eq!(demo.camera().focused_body(), Some(selected));
    demo.enter_surface_navigation().unwrap();
    assert_eq!(demo.camera().focused_body(), Some(selected));

    let guide = demo.guides().iter().find(|g| g.body == selected).unwrap();
    let period = guide.authored_orbit.unwrap().period_seconds();
    let radius = demo
        .world()
        .body(selected)
        .unwrap()
        .properties()
        .reference_radius_m();
    let mass = demo.world().body(selected).unwrap().properties().mass_kg();
    demo.edit_selected_mass(mass * 1.1).unwrap();
    assert_eq!(
        demo.guides()
            .iter()
            .find(|g| g.body == selected)
            .unwrap()
            .authored_orbit
            .unwrap()
            .period_seconds(),
        period
    );
    demo.edit_selected_radius(radius * 1.1).unwrap();
    assert_eq!(
        demo.guides()
            .iter()
            .find(|g| g.body == selected)
            .unwrap()
            .authored_orbit
            .unwrap()
            .period_seconds(),
        period
    );
    let edited = *demo.world().body(selected).unwrap().properties();
    let name = demo.world().body(selected).unwrap().name().to_owned();
    let revision = demo.world().revision();
    assert!(demo.edit_selected_radius(0.01).is_err());
    assert_eq!(demo.world().revision(), revision);
    assert_eq!(*demo.world().body(selected).unwrap().properties(), edited);
    assert_eq!(demo.world().body(selected).unwrap().name(), name);
    assert!(demo.edit_selected_velocity(DVec3::X).is_err());
    assert_eq!(demo.world().revision(), revision);
    demo.rename_selected("Moon renamed").unwrap();
    assert_eq!(demo.world().body(selected).unwrap().name(), "Moon renamed");
    demo.seek_seconds(1234.5).unwrap();
    assert_eq!(
        demo.guides()
            .iter()
            .find(|g| g.body == selected)
            .unwrap()
            .authored_orbit
            .unwrap()
            .period_seconds(),
        period
    );

    let initial_times = demo.trail_times_s();
    assert!(!initial_times.is_empty());
    demo.set_paused(false).unwrap();
    demo.set_playback_rate(0.5).unwrap();
    for _ in 0..8 {
        demo.update(Duration::from_millis(125));
    }
    assert_eq!(demo.motion_snapshot().published_time_s, 1235.0);
    assert_eq!(demo.motion_snapshot().requested_time_s, 1235.0);
    let times = demo.trail_times_s();
    assert_eq!(times.last().copied(), Some(1235.0));
    demo.seek_seconds(500.0).unwrap();
    assert_eq!(demo.trail_times_s(), [500.0]);
    demo.seek_seconds(600.0).unwrap();
    assert_eq!(demo.trail_times_s(), [600.0]);
    demo.set_playback_rate(-0.5).unwrap();
    demo.set_paused(false).unwrap();
    for _ in 0..8 {
        demo.update(Duration::from_millis(125));
    }
    assert_eq!(demo.motion_snapshot().published_time_s, 599.5);
    assert_eq!(demo.trail_times_s().last().copied(), Some(599.5));
    demo.set_playback_rate(0.0).unwrap();
    let paused_revision = demo.world().revision();
    for _ in 0..8 {
        demo.update(Duration::from_millis(125));
    }
    assert_eq!(demo.world().revision(), paused_revision);
    assert_eq!(demo.motion_snapshot().published_time_s, 599.5);
    demo.single_step(false).unwrap();
    assert_eq!(demo.motion_snapshot().published_time_s, 539.5);
    assert_eq!(demo.trail_times_s(), [599.5, 539.5]);
    demo.reset_motion().unwrap();
    assert_eq!(demo.trail_times_s(), [0.0]);
    demo.set_paused(true).unwrap();
    let unchanged_revision = demo.world().revision();
    demo.update(Duration::from_millis(200));
    assert_eq!(demo.world().revision(), unchanged_revision);

    // Long wall gaps are discarded instead of becoming a navigation/simulation jump.
    demo.set_paused(false).unwrap();
    let before_gap = demo.world().sample_time().seconds_since_epoch();
    let pose_before_gap = demo.camera().pose();
    demo.update(Duration::from_millis(251));
    assert_eq!(demo.world().sample_time().seconds_since_epoch(), before_gap);
    assert_pose_near(demo.camera().pose(), pose_before_gap);
    assert!(demo.motion_snapshot().paused);

    demo.set_paused(false).unwrap();
    demo.set_lifecycle_drawable(false);
    let before_hidden = demo.world().sample_time().seconds_since_epoch();
    let pose_before_hidden = demo.camera().pose();
    demo.update(Duration::from_secs(10));
    assert_eq!(
        demo.world().sample_time().seconds_since_epoch(),
        before_hidden
    );
    assert_pose_near(demo.camera().pose(), pose_before_hidden);
    demo.set_lifecycle_drawable(true);
    demo.update(Duration::ZERO);
    demo.set_playback_rate(1.0).unwrap();
    demo.update(Duration::from_millis(100));
    assert_eq!(
        demo.world().sample_time().seconds_since_epoch(),
        before_hidden + 0.1
    );

    // The Moon's authored orbit is around Rust (1/10 world-scale test system,
    // GAME_AND_SYSTEM_DIRECTION.md); a guide relative to Sol is a non-authored
    // alternate reference with a diagnostic instead of elements.
    demo.set_guide_reference(OrbitGuideReference::Explicit(sol))
        .unwrap();
    let alternate = demo.guides().iter().find(|g| g.body == selected).unwrap();
    assert_eq!(alternate.reference, Some(sol));
    assert!(alternate.authored_orbit.is_none());
    assert!(alternate.elements.is_none());
    assert!(alternate.diagnostic.is_some());
    demo.set_guide_reference(OrbitGuideReference::Explicit(rust))
        .unwrap();
    assert!(
        demo.guides()
            .iter()
            .any(|g| g.body == selected && g.reference == Some(rust))
    );
    let guide = demo
        .guides()
        .iter()
        .find(|g| g.body == selected && g.reference == Some(rust))
        .unwrap();
    let orbit = guide.authored_orbit.unwrap();
    let mut vertices = Vec::new();
    guide.vertices(64, &mut vertices).unwrap();
    let expected_periapsis = orbit.plane_to_system().quaternion()
        * DVec3::new(
            orbit.semi_major_axis_m() * (1.0 - orbit.eccentricity()),
            0.0,
            0.0,
        );
    assert!((vertices[0] - expected_periapsis).length() < 1e-6);
    assert!(
        demo.set_guide_reference(OrbitGuideReference::Explicit(selected))
            .is_err()
    );
}

#[test]
fn translating_and_body_fixed_frames_preserve_local_attachments_across_extreme_seeks() {
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    let moon = body_id(demo.world(), "Moon");
    let rust = body_id(demo.world(), "Rust");
    for time in [-31_557_600_000.0, 0.0, 31_557_600_000.0] {
        demo.seek_seconds(time).unwrap();
        let evaluation = demo.frames().tree().evaluate();
        let moon_frames = demo.frames().frames_for(moon).unwrap();
        let rust_frames = demo.frames().frames_for(rust).unwrap();
        assert_eq!(
            evaluation.parent(moon_frames.body_fixed).unwrap(),
            Some(moon_frames.translating)
        );
        assert_eq!(
            evaluation.parent(rust_frames.body_fixed).unwrap(),
            Some(rust_frames.translating)
        );
        let offset = DVec3::new(0.001, -0.002, 0.003);
        for frames in [moon_frames, rust_frames] {
            let translating = FramePosition::new(
                frames.translating,
                LocalPosition::try_metres(offset).unwrap(),
            );
            let body_fixed = FramePosition::new(
                frames.body_fixed,
                LocalPosition::try_metres(offset).unwrap(),
            );
            let translated_in_fixed = evaluation
                .convert_position(translating, frames.body_fixed)
                .unwrap();
            let fixed_in_translating = evaluation
                .convert_position(body_fixed, frames.translating)
                .unwrap();
            let body_to_system = demo
                .world()
                .body(if frames == moon_frames { moon } else { rust })
                .unwrap()
                .state()
                .body_to_system()
                .quaternion();
            assert!(
                (translated_in_fixed.local().metres() - body_to_system.inverse() * offset).length()
                    < 1e-15
            );
            assert!(
                (fixed_in_translating.local().metres() - body_to_system * offset).length() < 1e-15
            );
            assert!(translated_in_fixed.local().metres().is_finite());
            assert!(fixed_in_translating.local().metres().is_finite());
        }
        // The moving Moon and stationary Rust translating anchors share the system root.
        let mut ancestors = Vec::new();
        let mut cursor = Some(moon_frames.translating);
        while let Some(frame) = cursor {
            ancestors.push(frame);
            cursor = evaluation.parent(frame).unwrap();
        }
        let mut common_ancestor = None;
        let mut cursor = Some(rust_frames.translating);
        while let Some(frame) = cursor {
            if ancestors.contains(&frame) {
                common_ancestor = Some(frame);
                break;
            }
            cursor = evaluation.parent(frame).unwrap();
        }
        assert_eq!(common_ancestor, Some(evaluation.root()));
    }
    demo.select_body(moon).unwrap();
    demo.focus_selected(true).unwrap();
    assert_eq!(demo.camera().focused_body(), Some(moon));
    demo.enter_surface_navigation().unwrap();
    assert_eq!(demo.camera().focused_body(), Some(moon));
    let attached = demo.camera().pose();
    demo.seek_seconds(-31_557_600_000.0).unwrap();
    assert_eq!(demo.camera().focused_body(), Some(moon));
    assert_eq!(demo.camera().pose(), attached);
}
