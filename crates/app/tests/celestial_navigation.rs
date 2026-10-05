use glam::DVec3;
use mundaris_app::{celestial_camera::*, gravity_fixtures::*};
use mundaris_simulation::*;
use mundaris_world::*;
use std::{num::NonZeroU64, time::Duration};
#[test]
fn every_body_focus_interrupt_fps_precision_and_read_only_world() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let projection = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    let revision = world.revision();
    let before: Vec<_> = world.bodies().map(|(id, b)| (id, b.clone())).collect();
    for &(id, _) in &before {
        let mut endpoint: Option<DVec3> = None;
        for fps in [30, 60, 144] {
            let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
            let initial = camera.pose();
            camera.transition_to(&pair, FocusTarget::Body(id)).unwrap();
            assert_eq!(camera.pose(), initial);
            for i in 0..fps {
                let ns = if i + 1 == fps {
                    1_000_000_000 - (1_000_000_000 / fps) * (fps - 1)
                } else {
                    1_000_000_000 / fps
                };
                camera
                    .update_navigation(&pair, &NavigationInput::default(), Duration::from_nanos(ns))
                    .unwrap();
            }
            assert_eq!(camera.mode(), CameraMode::BodyOrbit);
            assert_eq!(camera.focused_body(), Some(id));
            let p = camera.pose().position().local().metres();
            assert!(
                (p.length() / world.body(id).unwrap().properties().reference_radius_m() - 4.0)
                    .abs()
                    < 1e-12
            );
            if let Some(e) = endpoint {
                assert!((p - e).length() < 1e-7);
            } else {
                endpoint = Some(p);
            }
            camera.enter_free_flight(&pair).unwrap();
            let current = camera.pose();
            camera
                .transition_to(&pair, FocusTarget::Body(before[2].0))
                .unwrap();
            assert_eq!(current, camera.pose());
            camera
                .update_navigation(
                    &pair,
                    &NavigationInput::default(),
                    Duration::from_millis(100),
                )
                .unwrap();
            let current = camera.pose();
            camera
                .transition_to(&pair, FocusTarget::Body(before[0].0))
                .unwrap();
            assert_eq!(current, camera.pose());
        }
    }
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world
            .bodies()
            .map(|(id, b)| (id, b.clone()))
            .collect::<Vec<_>>(),
        before
    );
}
#[test]
fn clearance_zoom_reaches_metres_and_free_flight_has_no_inertia() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let projection = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    let id = world.bodies().nth(1).unwrap().0;
    let radius = world.body(id).unwrap().properties().reference_radius_m();
    let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
    camera.focus(&pair, id, false, true).unwrap();
    camera.orbit_zoom([0.0; 2], 1e5).unwrap();
    assert_eq!(camera.clearance_m(), 1.0);
    // Wheel units are now normalized notches, not the superseded raw-pixel gain.
    camera.orbit_zoom([0.0; 2], -1.0).unwrap();
    assert!((camera.clearance_m() - 1.25).abs() < 1e-7);
    assert!(camera.distance_m() > radius);
    camera.enter_free_flight(&pair).unwrap();
    assert_eq!(camera.focused_body(), None);
    let before = camera.pose();
    camera
        .update_navigation(
            &pair,
            &NavigationInput {
                translation: DVec3::X,
                ..Default::default()
            },
            Duration::from_secs(1),
        )
        .unwrap();
    assert!(
        (camera.pose().position().local().metres() - before.position().local().metres() - DVec3::X)
            .length()
            < 1e-7
    );
    let after = camera.pose();
    camera
        .update_navigation(&pair, &NavigationInput::default(), Duration::from_secs(1))
        .unwrap();
    assert_eq!(after, camera.pose());
}

#[test]
fn free_flight_carrier_is_system_stationary_across_real_commits() {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let mut projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let id = world.bodies().nth(1).unwrap().0;
    let mut camera = CelestialCamera::overview(
        &projection.coherent_view(&world).unwrap(),
        DVec3::ZERO,
        1.6e11,
    )
    .unwrap();
    camera
        .focus(&projection.coherent_view(&world).unwrap(), id, false, true)
        .unwrap();
    camera
        .enter_free_flight(&projection.coherent_view(&world).unwrap())
        .unwrap();
    let before = projection
        .tree()
        .evaluate()
        .convert_position(camera.pose().position(), projection.tree().root())
        .unwrap();
    let mut runner =
        FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
    runner.request_forward_to_tick(10).unwrap();
    runner.pump(&mut world, |_, _| {}).unwrap();
    projection.publish(&world).unwrap();
    camera
        .update_navigation(
            &projection.coherent_view(&world).unwrap(),
            &NavigationInput::default(),
            Duration::from_millis(100),
        )
        .unwrap();
    let after = projection
        .tree()
        .evaluate()
        .convert_position(camera.pose().position(), projection.tree().root())
        .unwrap();
    assert!((after.local().metres() - before.local().metres()).length() <= 1e-3);
    assert_eq!(camera.mode(), CameraMode::FreeFlight);
    let rebuilt = CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
    camera.remap_projection(&rebuilt).unwrap();
    assert!(
        (camera.pose().position().local().metres()
            - projection
                .tree()
                .evaluate()
                .convert_position(after, projection.frames_for(id).unwrap().translating)
                .unwrap()
                .local()
                .metres())
        .length()
            <= 1e-3
    );
}

#[test]
fn smooth_zoom_endpoint_independent_of_navigation_fps() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let projection = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    let id = world.bodies().nth(1).unwrap().0;
    let mut endpoint = None::<f64>;
    for fps in [30, 60, 144] {
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
        camera.focus(&pair, id, false, true).unwrap();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: 5.0,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        for i in 0..fps {
            let ns = if i + 1 == fps {
                1_000_000_000 - (1_000_000_000 / fps) * (fps - 1)
            } else {
                1_000_000_000 / fps
            };
            camera
                .update_navigation(&pair, &NavigationInput::default(), Duration::from_nanos(ns))
                .unwrap();
        }
        if let Some(previous) = endpoint {
            assert!((camera.clearance_m() - previous).abs() < 1e-7);
        } else {
            endpoint = Some(camera.clearance_m());
        }
    }
}

#[test]
fn refocus_from_free_flight_looking_away_avoids_old_body() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let pair = frames.coherent_view(&world).unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
    camera.focus(&pair, ids[1], false, true).unwrap();
    camera.enter_free_flight(&pair).unwrap();
    camera
        .update_navigation(
            &pair,
            &NavigationInput {
                drag: [
                    std::f64::consts::PI
                        / camera
                            .navigation_diagnostics()
                            .local_radians_per_logical_pixel,
                    0.0,
                ],
                ..Default::default()
            },
            Duration::ZERO,
        )
        .unwrap();
    let before = camera.pose();
    camera
        .transition_to(&pair, FocusTarget::Body(ids[2]))
        .unwrap();
    assert_eq!(camera.pose(), before);
    for _ in 0..60 {
        camera
            .update_navigation(
                &pair,
                &NavigationInput::default(),
                Duration::from_millis(16),
            )
            .unwrap();
    }
    assert_eq!(camera.focused_body(), Some(ids[2]));
    assert_eq!(camera.mode(), CameraMode::BodyOrbit);
}
