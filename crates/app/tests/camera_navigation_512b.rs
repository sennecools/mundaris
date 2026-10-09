use glam::DVec3;
use astrum_app::{celestial_camera::*, gravity_fixtures::*, terrain_inspection};
use astrum_math::*;
use astrum_renderer::CelestialProjection;
use astrum_world::*;
use std::{num::NonZeroU64, time::Duration};

fn fixture() -> (CelestialSystem, CelestialFrameProjection) {
    let world = GravityFixture::GameplaySolarSystem
        .create(NonZeroU64::new(81).unwrap())
        .unwrap();
    let projection = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    (world, projection)
}

fn settle(camera: &mut CelestialCamera, pair: &CoherentCelestialView<'_>, seconds: f64) {
    let n = (seconds / 0.02).ceil() as usize;
    for i in 0..n {
        let dt = if i + 1 == n {
            seconds - 0.02 * (n - 1) as f64
        } else {
            0.02
        };
        camera
            .update_navigation(
                pair,
                &NavigationInput::default(),
                Duration::from_secs_f64(dt),
            )
            .unwrap();
    }
}

fn clearance(pair: &CoherentCelestialView<'_>, camera: &CelestialCamera, body: BodyId) -> f64 {
    terrain_inspection::terrain_clearance(pair, camera.pose(), body)
        .unwrap()
        .expect("fixture body has complete terrain")
        .clearance_m
}

fn pose_error(
    pair: &CoherentCelestialView<'_>,
    a: FramePose,
    b: FramePose,
    frame: FrameId,
) -> (f64, f64) {
    let a = pair.evaluation().reexpress_pose(a, frame).unwrap();
    let b = pair.evaluation().reexpress_pose(b, frame).unwrap();
    let position = (a.position().local().metres() - b.position().local().metres()).length();
    let angle = 2.0
        * a.orientation()
            .quaternion()
            .dot(b.orientation().quaternion())
            .abs()
            .clamp(-1.0, 1.0)
            .acos();
    (position, angle)
}

fn projection(fov: f64, height: u32) -> CelestialProjection {
    CelestialProjection::try_new(1280, height, fov.to_radians(), 0.1).unwrap()
}

fn body_orbit(pair: &CoherentCelestialView<'_>, body: BodyId) -> CelestialCamera {
    let mut camera = CelestialCamera::overview(pair, DVec3::ZERO, 1.6e11).unwrap();
    camera.transition_to(pair, FocusTarget::Body(body)).unwrap();
    settle(&mut camera, pair, 1.0);
    camera.enter_body_orbit(pair, body).unwrap();
    camera
}

#[test]
fn response_matrix_actual_angles_and_wheel_displacements() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let earth = world.bodies().nth(3).unwrap().0;
    let clearances = [2.0, 10.0, 100.0, 10_000.0, 100_000.0];
    for clearance_m in clearances {
        for fov in [30.0, 60.0, 90.0] {
            for (height, dpi) in [(720, 1.0), (1080, 1.0), (1620, 1.5), (2160, 2.0)] {
                let screen = projection(fov, height);
                let mut orbit = body_orbit(&pair, earth);
                orbit.target_clearance(&pair, clearance_m).unwrap();
                settle(&mut orbit, &pair, 1.0);
                orbit.set_navigation_projection(screen, dpi).unwrap();
                let orbit_before = orbit.pose().position().local().metres();
                orbit
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            drag: [200.0, 0.0],
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                let orbit_after = orbit.pose().position().local().metres();
                let orbit_angle = orbit_before
                    .normalize()
                    .angle_between(orbit_after.normalize())
                    .to_degrees();
                let orbit_distance = orbit.distance_m();
                let orbit_diag = orbit.navigation_diagnostics();
                let k = orbit_diag.wheel_log_per_notch;
                orbit
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            scroll_notches: 0.1,
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                settle(&mut orbit, &pair, 0.5);
                let orbit_wheel_m = orbit.distance_m() - orbit_distance;

                let mut local = body_orbit(&pair, earth);
                local.target_clearance(&pair, clearance_m).unwrap();
                settle(&mut local, &pair, 1.0);
                local.enter_surface_inspection(&pair, earth).unwrap();
                local.set_navigation_projection(screen, dpi).unwrap();
                let orientation_before = local.pose().orientation().quaternion();
                local
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            drag: [200.0, 0.0],
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                let orientation_after = local.pose().orientation().quaternion();
                let local_angle = (2.0
                    * orientation_before
                        .dot(orientation_after)
                        .abs()
                        .clamp(-1.0, 1.0)
                        .acos())
                .to_degrees();
                let local_diag = local.navigation_diagnostics();
                let local_before_wheel = local.pose().position().local().metres();
                local
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            scroll_notches: 0.1,
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                let pending = local.navigation_diagnostics().pending_forward_m;
                settle(&mut local, &pair, 0.5);
                let local_wheel_m = local
                    .pose()
                    .position()
                    .local()
                    .metres()
                    .distance(local_before_wheel);
                println!(
                    "matrix,c={clearance_m},fov={fov},logical_h={},dpi={dpi},orbit_rad_px={},orbit_swipe_deg={orbit_angle},orbit_wheel_m={orbit_wheel_m},local_rad_px={},local_swipe_deg={local_angle},local_wheel_pending_m={pending},local_wheel_m={local_wheel_m},k={k}",
                    orbit_diag.logical_viewport_height,
                    orbit_diag.orbit_radians_per_logical_pixel,
                    local_diag.local_radians_per_logical_pixel
                );
                if fov == 60.0 && height == 1080 && dpi == 1.0 && [2.0, 10.0].contains(&clearance_m)
                {
                    assert!(
                        local_angle > 0.0 && local_angle < 1.0,
                        "local swipe {local_angle}° at {clearance_m}m"
                    );
                    assert!(
                        orbit_angle > 0.0 && orbit_angle < 1.0,
                        "orbit swipe {orbit_angle}° at {clearance_m}m"
                    );
                }
                assert!(local_angle.is_finite() && local_wheel_m.is_finite());
                assert!(orbit_angle.is_finite() && orbit_wheel_m.is_finite());
            }
        }
    }
}

#[test]
fn ordinary_controls_focus_orbit_surface_wheel_and_reach_two_metres() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let earth = ids[3];
    let moon = ids[4];
    let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
    camera
        .transition_to(&pair, FocusTarget::Body(earth))
        .unwrap();
    settle(&mut camera, &pair, 1.0);
    camera.enter_body_orbit(&pair, earth).unwrap();

    for body in [earth, moon] {
        if camera.focused_body() != Some(body) {
            camera
                .transition_to(&pair, FocusTarget::Body(body))
                .unwrap();
            settle(&mut camera, &pair, 1.0);
            camera.enter_body_orbit(&pair, body).unwrap();
        }
        // Ordinary orbit wheel input first establishes a 100 km approach distance.
        let c = clearance(&pair, &camera, body);
        let k = camera.navigation_diagnostics().wheel_log_per_notch;
        let notches = (c / 100_000.0).ln() / k;
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: notches,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        settle(&mut camera, &pair, 2.0);
        let orbit_c = clearance(&pair, &camera, body);
        let before_surface = camera.pose();
        camera.enter_surface_inspection(&pair, body).unwrap();
        let surface_entry = pose_error(
            &pair,
            before_surface,
            camera.pose(),
            frames.frames_for(body).unwrap().body_fixed,
        );
        assert!(surface_entry.0 <= 1e-3 && surface_entry.1 <= 1e-6);

        // Camera arrives looking inward from the focused orbit pose. Surface wheel is
        // view-forward, so positive notches drive along the terrain-directed view ray.
        let current_c = clearance(&pair, &camera, body);
        let k = camera.navigation_diagnostics().wheel_log_per_notch;
        let notches = (current_c / 2.0).ln() / k;
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: notches,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        settle(&mut camera, &pair, 2.0);
        let actual = clearance(&pair, &camera, body);
        println!(
            "ordinary_route,body={body:?},orbit_clearance={orbit_c},surface_notches={notches},complete_terrain_clearance={actual},target_kind={}",
            camera.navigation_diagnostics().zoom_target_meaning
        );
        assert!(
            (actual - 2.0).abs() <= 0.05,
            "{body:?} complete terrain clearance {actual}"
        );

        // Exercise the production tangent and local-up axes at near-ground scale.
        let before_move = camera.pose().position().local().metres();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    translation: DVec3::X,
                    ..Default::default()
                },
                Duration::from_millis(100),
            )
            .unwrap();
        let after_tangent = camera.pose().position().local().metres();
        assert!((after_tangent - before_move).length() > 0.0);
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    translation: DVec3::Y,
                    ..Default::default()
                },
                Duration::from_millis(100),
            )
            .unwrap();
        let after_vertical = camera.pose().position().local().metres();
        assert!((after_vertical - after_tangent).length() > 0.0);
        let moved_clearance = clearance(&pair, &camera, body);
        assert!(moved_clearance >= 1.0 - 1e-6);

        if body == earth {
            camera
                .transition_to(&pair, FocusTarget::Body(moon))
                .unwrap();
            settle(&mut camera, &pair, 1.0);
            camera.enter_body_orbit(&pair, moon).unwrap();
        }
    }
    camera
        .transition_to(
            &pair,
            FocusTarget::Overview {
                center_m: DVec3::ZERO,
                distance_m: 1.6e11,
            },
        )
        .unwrap();
    settle(&mut camera, &pair, 1.0);
    assert_eq!(camera.mode(), CameraMode::SystemOrbit);
    println!("ordinary_route,return=overview,mode={:?}", camera.mode());
}

#[test]
fn event_stream_boundaries_and_mixed_surface_trajectory_are_timestep_equivalent() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let mut endpoints = Vec::new();
    for fps in [30_u32, 60, 144] {
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
        camera
            .set_navigation_projection(projection(60.0, 1080), 1.0)
            .unwrap();
        let events = [
            (0.17, [120.0, 0.0], 0.5),
            (0.43, [-30.0, 0.0], -0.2),
            (0.79, [80.0, 0.0], 0.4),
        ];
        let mut t = 0.0;
        let mut e = 0;
        while t < 1.0 - 1e-12 {
            let frame_end = (t + 1.0 / fps as f64).min(1.0);
            while e < events.len() && events[e].0 <= frame_end + 1e-12 {
                let (at, drag, wheel) = events[e];
                if at > t {
                    camera
                        .update_navigation(
                            &pair,
                            &NavigationInput::default(),
                            Duration::from_secs_f64(at - t),
                        )
                        .unwrap();
                    t = at;
                }
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            drag,
                            scroll_notches: wheel,
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                e += 1;
            }
            if frame_end > t {
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput::default(),
                        Duration::from_secs_f64(frame_end - t),
                    )
                    .unwrap();
                t = frame_end;
            }
        }
        let d = camera.navigation_diagnostics();
        println!(
            "event_timing,fps={fps},distance={},orbit_target={:?},angle_per_pixel={}",
            camera.distance_m(),
            d.requested_distance_m,
            d.orbit_radians_per_logical_pixel
        );
        endpoints.push((
            camera.distance_m(),
            camera.pose().orientation().quaternion(),
            d.requested_distance_m.unwrap(),
        ));
    }
    for &(distance, orientation, target) in &endpoints[1..] {
        assert!((distance - endpoints[0].0).abs() / endpoints[0].0 <= 1e-6);
        assert!((target - endpoints[0].2).abs() <= (1e-6 * endpoints[0].2).max(1e-3));
        assert!(
            2.0 * orientation
                .dot(endpoints[0].1)
                .abs()
                .clamp(-1.0, 1.0)
                .acos()
                <= 1e-6
        );
    }

    let earth = world.bodies().nth(3).unwrap().0;
    let mut mixed = Vec::new();
    for fps in [30_u32, 60, 144] {
        let mut camera = body_orbit(&pair, earth);
        camera.target_clearance(&pair, 100.0).unwrap();
        settle(&mut camera, &pair, 1.0);
        camera.enter_surface_inspection(&pair, earth).unwrap();
        camera
            .set_navigation_projection(projection(60.0, 1080), 1.0)
            .unwrap();
        let initial_position = camera.pose().position().local().metres();
        let initial_q = camera.pose().orientation().quaternion();
        let mut elapsed: f64 = 0.0;
        let mut next_event = 0;
        let events = [
            (0.17, [1.0, 0.1], 0.1),
            (0.43, [-0.5, 0.0], -0.05),
            (0.79, [0.8, -0.1], 0.1),
        ];
        let held = NavigationInput {
            translation: DVec3::new(0.15, 0.05, -0.1),
            ..Default::default()
        };
        while elapsed < 1.0 - 1e-12 {
            let end = (elapsed + 1.0 / fps as f64).min(1.0);
            while next_event < events.len() && events[next_event].0 <= end + 1e-12 {
                let (at, drag, wheel) = events[next_event];
                if at > elapsed {
                    camera
                        .update_navigation(&pair, &held, Duration::from_secs_f64(at - elapsed))
                        .unwrap();
                    elapsed = at;
                }
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            drag,
                            scroll_notches: wheel,
                            ..Default::default()
                        },
                        Duration::ZERO,
                    )
                    .unwrap();
                next_event += 1;
            }
            if end > elapsed {
                camera
                    .update_navigation(&pair, &held, Duration::from_secs_f64(end - elapsed))
                    .unwrap();
                elapsed = end;
            }
        }
        let displacement = camera
            .pose()
            .position()
            .local()
            .metres()
            .distance(initial_position);
        let orientation_error = 2.0
            * initial_q
                .dot(camera.pose().orientation().quaternion())
                .abs()
                .clamp(-1.0, 1.0)
                .acos();
        println!(
            "mixed_timing,fps={fps},elapsed={elapsed},displacement={displacement},complete_clearance={},orientation_delta={orientation_error},target_kind={}",
            clearance(&pair, &camera, earth),
            camera.navigation_diagnostics().zoom_target_meaning
        );
        mixed.push((
            displacement,
            camera.pose().position().local().metres(),
            camera.pose().orientation().quaternion(),
            clearance(&pair, &camera, earth),
        ));
    }
    for &(distance, position, orientation, actual_clearance) in &mixed[1..] {
        let intended = mixed[0].0.max(1e-3);
        assert!((distance - mixed[0].0).abs() <= (0.01 * intended).max(1e-3));
        assert!(position.distance(mixed[0].1) <= (0.01 * intended).max(1e-3));
        let angle = 2.0 * orientation.dot(mixed[0].2).abs().clamp(-1.0, 1.0).acos();
        assert!(angle <= 1e-4);
        assert!((actual_clearance - mixed[0].3).abs() <= (0.01 * intended).max(1e-3));
        println!(
            "mixed_residual,endpoint_m={},orientation_rad={angle},clearance_m={}",
            position.distance(mixed[0].1),
            (actual_clearance - mixed[0].3).abs()
        );
    }
}

#[test]
fn overview_response_matrix_and_repeated_surface_turns_remain_usable() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    for fov in [30.0, 60.0, 90.0] {
        for (height, dpi) in [(720, 1.0), (1080, 1.0), (1620, 1.5), (2160, 2.0)] {
            let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
            camera
                .set_navigation_projection(projection(fov, height), dpi)
                .unwrap();
            let before = camera.pose().orientation().quaternion();
            camera
                .update_navigation(
                    &pair,
                    &NavigationInput {
                        drag: [200.0, 0.0],
                        scroll_notches: 0.25,
                        ..Default::default()
                    },
                    Duration::ZERO,
                )
                .unwrap();
            let n = camera.navigation_diagnostics();
            let angle = 2.0
                * before
                    .dot(camera.pose().orientation().quaternion())
                    .abs()
                    .clamp(-1.0, 1.0)
                    .acos();
            let expected = 200.0 / n.logical_viewport_height * 2.0 * (fov.to_radians() * 0.5).tan();
            assert!((angle - expected).abs() <= 1e-6);
            let target = n.requested_distance_m.unwrap();
            assert!((target / 4e11 - (-0.25 * n.wheel_log_per_notch).exp()).abs() <= 1e-12);
            settle(&mut camera, &pair, 1.0);
            println!(
                "overview_matrix,fov={fov},logical_h={},dpi={dpi},swipe_deg={},wheel_fraction=0.25,target_m={target},smoothed_m={}",
                n.logical_viewport_height,
                angle.to_degrees(),
                camera.distance_m()
            );
        }
    }
    let earth = world.bodies().nth(3).unwrap().0;
    let mut camera = body_orbit(&pair, earth);
    camera.target_clearance(&pair, 2.0).unwrap();
    settle(&mut camera, &pair, 1.8);
    camera.enter_surface_inspection(&pair, earth).unwrap();
    camera.look_surface_horizon().unwrap();
    camera
        .set_navigation_projection(projection(60.0, 1080), 1.0)
        .unwrap();
    let before = camera.pose().orientation().quaternion();
    let gesture_deg = (200.0
        * camera
            .navigation_diagnostics()
            .local_radians_per_logical_pixel)
        .to_degrees();
    let gestures = (180.0 / gesture_deg).ceil() as usize;
    let mut summed_angle = 0.0;
    for _ in 0..gestures {
        let previous = camera.pose().orientation().quaternion();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    drag: [200.0, 0.0],
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        summed_angle += 2.0
            * previous
                .dot(camera.pose().orientation().quaternion())
                .abs()
                .clamp(-1.0, 1.0)
                .acos();
    }
    let endpoint_angle = 2.0
        * before
            .dot(camera.pose().orientation().quaternion())
            .abs()
            .clamp(-1.0, 1.0)
            .acos();
    assert!(
        summed_angle >= std::f64::consts::PI
            && summed_angle < std::f64::consts::PI + gesture_deg.to_radians() + 1e-6
    );
    assert!(endpoint_angle.to_degrees() > 179.0);
    println!(
        "repeated_turn,clearance_m=2,logical_pixels_per_gesture=200,gestures={gestures},total_swipe_pixels={},summed_turn_deg={},endpoint_turn_deg={}; mechanical reachability only, not human usability approval",
        gestures * 200,
        summed_angle.to_degrees(),
        endpoint_angle.to_degrees()
    );
}

#[test]
fn surface_entry_exit_preserves_same_body_pose_at_100km_10km_and_2m() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let earth = world.bodies().nth(3).unwrap().0;
    let body_frame = frames.frames_for(earth).unwrap().body_fixed;
    let mut camera = body_orbit(&pair, earth);
    for distance in [100_000.0, 10_000.0, 2.0] {
        camera.target_clearance(&pair, distance).unwrap();
        settle(&mut camera, &pair, 1.5);
        let incoming = camera.pose();
        camera.enter_surface_inspection(&pair, earth).unwrap();
        let entry = pose_error(&pair, incoming, camera.pose(), body_frame);
        let entered = camera.pose();
        camera.enter_body_orbit(&pair, earth).unwrap();
        let exit = pose_error(&pair, entered, camera.pose(), body_frame);
        println!(
            "pose_residual,clearance={distance},entry_pos={},entry_angle={},exit_pos={},exit_angle={}",
            entry.0, entry.1, exit.0, exit.1
        );
        assert!(entry.0 <= 1e-3 && entry.1 <= 1e-6);
        assert!(exit.0 <= 1e-3 && exit.1 <= 1e-6);
    }
}

#[test]
fn fractional_wheel_continuity_and_speed_components_remain_explicit() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let earth = world.bodies().nth(3).unwrap().0;
    let mut camera = body_orbit(&pair, earth);
    camera.target_clearance(&pair, 10.0).unwrap();
    settle(&mut camera, &pair, 1.8);
    camera
        .set_navigation_projection(projection(60.0, 1080), 1.0)
        .unwrap();
    for mode in [
        CameraMode::BodyOrbit,
        CameraMode::SurfaceInspection,
        CameraMode::BodyOrbit,
        CameraMode::FreeFlight,
    ] {
        match mode {
            CameraMode::BodyOrbit => camera.enter_body_orbit(&pair, earth).unwrap(),
            CameraMode::SurfaceInspection => camera.enter_surface_inspection(&pair, earth).unwrap(),
            CameraMode::FreeFlight => camera.enter_free_flight(&pair).unwrap(),
            _ => unreachable!(),
        }
        for wheel in [0.25, -0.125] {
            let before = clearance(&pair, &camera, earth);
            let input = NavigationInput {
                scroll_notches: wheel,
                speed_multiplier: 0.5,
                boost_multiplier: 4.0,
                ..Default::default()
            };
            camera
                .update_navigation(&pair, &input, Duration::ZERO)
                .unwrap();
            for _ in 0..60 {
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput {
                            scroll_notches: 0.0,
                            ..input
                        },
                        Duration::from_secs_f64(1.0 / 60.0),
                    )
                    .unwrap();
            }
            let after = clearance(&pair, &camera, earth);
            let n = camera.navigation_diagnostics();
            assert_eq!(camera.mode(), mode);
            assert_eq!(n.user_multiplier, 0.5);
            assert_eq!(n.boost_multiplier, 4.0);
            assert_eq!(n.effective_speed_m_s, n.base_speed_m_s * 0.5 * 4.0);
            assert!((n.wheel_log_per_notch - 1.25_f64.ln()).abs() < 1e-12);
            assert!(
                (after - before) * wheel < 0.0,
                "wheel direction reversed: {mode:?} {before} -> {after}"
            );
            println!(
                "wheel_continuity,mode={mode:?},fractional_notches={wheel},before_complete_m={before},after_complete_m={after},base_m_s={},user={},boost={},effective_m_s={},source={}",
                n.base_speed_m_s,
                n.user_multiplier,
                n.boost_multiplier,
                n.effective_speed_m_s,
                n.base_source
            );
        }
    }
}

#[test]
fn cancelling_terrain_orbit_and_unstarted_focus_freezes_complete_clearance() {
    let (world, frames) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let earth = world.bodies().nth(3).unwrap().0;
    for unstarted_focus in [false, true] {
        let mut camera = body_orbit(&pair, earth);
        camera.target_clearance(&pair, 2.0).unwrap();
        settle(&mut camera, &pair, 2.0);
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: -0.5,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        let before = camera.pose();
        let before_clearance = clearance(&pair, &camera, earth);
        if unstarted_focus {
            camera
                .transition_to(
                    &pair,
                    FocusTarget::Overview {
                        center_m: DVec3::ZERO,
                        distance_m: 1.6e11,
                    },
                )
                .unwrap();
        }
        camera.cancel_transition();
        let target = camera
            .navigation_diagnostics()
            .requested_clearance_m
            .unwrap();
        assert!((target - before_clearance).abs() <= 1e-6);
        settle(&mut camera, &pair, 1.0);
        let residual = pose_error(
            &pair,
            before,
            camera.pose(),
            frames.frames_for(earth).unwrap().body_fixed,
        );
        assert!(residual.0 <= 1e-3 && residual.1 <= 1e-6);
        assert!(!camera.transitioning());
        println!(
            "cancel_terrain_orbit,unstarted_focus={unstarted_focus},before_complete_m={before_clearance},frozen_target_m={target},after_complete_m={},position_residual_m={},orientation_residual_rad={}",
            clearance(&pair, &camera, earth),
            residual.0,
            residual.1
        );
    }
}
