use glam::DVec3;
use mundaris_app::{celestial_camera::*, gravity_fixtures::*, planet_surface::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_simulation::*;
use mundaris_world::*;
use std::{num::NonZeroU64, time::Duration};

#[test]
fn approach_inspection_look_real_gravity_spin_remap_and_read_only_preparation() {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let mut projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let pair = projection.coherent_view(&world).unwrap();
    let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
    camera.focus(&pair, ids[1], false, true).unwrap();
    for clearance in [1e11, 1e5, 1e4, 1e3, 100.0, 10.0, 2.0] {
        camera.target_clearance(&pair, clearance).unwrap();
        for _ in 0..320 {
            camera
                .update_navigation(
                    &pair,
                    &NavigationInput::default(),
                    Duration::from_millis(16),
                )
                .unwrap();
        }
        assert!((camera.measured_clearance(&pair, ids[1]).unwrap() - clearance).abs() <= 1e-3);
    }
    let incoming = camera.pose();
    camera.enter_surface_inspection(&pair, ids[1]).unwrap();
    let returned = pair
        .evaluation()
        .reexpress_pose(camera.pose(), incoming.position().frame())
        .unwrap();
    assert!(
        (returned.position().local().metres() - incoming.position().local().metres()).length()
            <= 1e-7
    );
    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
        assert!(
            (returned.orientation().quaternion() * axis
                - incoming.orientation().quaternion() * axis)
                .length()
                <= 1e-12
        );
    }
    let local = camera.pose().position().local();
    camera.look_surface_horizon().unwrap();
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
    assert!((camera.pose().position().local().metres() - local.metres()).length() <= 1e-7);
    let before: Vec<_> = world.bodies().map(|(id, b)| (id, b.clone())).collect();
    let revision = world.revision();
    let mut surface = PlanetSurfaceSession::new(ids[1], 2048).unwrap();
    let screen = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let view = PreparedView::new(
        &pair.evaluation(),
        camera.pose(),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let source = pair.projection().frames_for(ids[1]).unwrap().body_fixed;
    for _ in 0..100 {
        surface
            .update(
                &SurfaceViewInput {
                    view: &view,
                    body_fixed_frame: source,
                    reference_radius_m: 6.371e6,
                    projection: screen,
                },
                &LodSettings::default(),
            )
            .unwrap();
    }
    assert_eq!(surface.state(), SurfaceRepresentationState::Surface);
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world
            .bodies()
            .map(|(id, b)| (id, b.clone()))
            .collect::<Vec<_>>(),
        before
    );
    let observed = camera.pose();
    let body_before = *world.body(ids[1]).unwrap().state();
    let moon_before = *world.body(ids[2]).unwrap().state();
    let mut runner =
        FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
    runner.request_forward_to_tick(10).unwrap();
    runner.pump(&mut world, |_, _| {}).unwrap();
    projection.publish(&world).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    camera
        .update_navigation(
            &pair,
            &NavigationInput::default(),
            Duration::from_millis(16),
        )
        .unwrap();
    assert!(
        (camera.pose().position().local().metres() - observed.position().local().metres()).length()
            <= 1e-7
    );
    assert!(
        (world
            .body(ids[1])
            .unwrap()
            .state()
            .center_in_system()
            .metres()
            - body_before.center_in_system().metres())
        .length()
            > 1e6
    );
    assert_ne!(
        world.body(ids[1]).unwrap().state().body_to_system(),
        body_before.body_to_system()
    );
    assert_ne!(*world.body(ids[2]).unwrap().state(), moon_before);
    assert_eq!(camera.velocity().relative(), LinearVelocity3::zero());
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
    assert!(camera.measured_clearance(&pair, ids[1]).unwrap() >= 1.0 - 1e-7);
    let old = camera.pose();
    let rebuilt = CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
    camera.remap_projection(&rebuilt).unwrap();
    assert_ne!(camera.pose().position().frame(), old.position().frame());
    assert_eq!(camera.pose().position().local(), old.position().local());
    assert_eq!(camera.focused_body(), Some(ids[1]));
}

#[test]
fn handoff_overlap_script_and_two_session_cache_accounting() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let mut sessions = [
        PlanetSurfaceSession::new(ids[1], 2048).unwrap(),
        PlanetSurfaceSession::new(ids[2], 2048).unwrap(),
    ];
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    let mut saw_far = false;
    let mut saw_surface = false;
    for clearance in [1e11, 1e9, 4e8, 2e8, 1e8, 5e7, 1e7, 1e5, 10.0, 2.0, 1e11] {
        let source = frames.frames_for(ids[1]).unwrap().body_fixed;
        let view = PreparedView::new(
            &frames.tree().evaluate(),
            FramePose::new(
                FramePosition::new(
                    source,
                    LocalPosition::try_metres(DVec3::Z * (6.371e6 + clearance)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let screen = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        for _ in 0..100 {
            for session in &mut sessions {
                let id = session.body();
                session
                    .update(
                        &SurfaceViewInput {
                            view: &view,
                            body_fixed_frame: frames.frames_for(id).unwrap().body_fixed,
                            reference_radius_m: world
                                .body(id)
                                .unwrap()
                                .properties()
                                .reference_radius_m(),
                            projection: screen,
                        },
                        &LodSettings::default(),
                    )
                    .unwrap();
            }
        }
        assert!(sessions.iter().map(|s| s.report.cache_bytes).sum::<usize>() <= 1024 * 1024);
        let session = &mut sessions[0];
        let body = CelestialRenderBody {
            body_fixed_frame: source,
            reference_radius_m: 6.371e6,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        if session.far_error_pixels < 0.05 {
            let mut probe = CelestialFrame::new(&view, &mut staging, screen, &sphere);
            probe.append_bodies(&[body]).unwrap();
            session.return_to_far_if_ready(
                probe.markers()[0].representation == SphereRepresentation::PhysicalSphere
                    || probe.markers()[0].representation == SphereRepresentation::SubpixelMarker,
            );
        }
        let active = session.state() == SurfaceRepresentationState::Surface;
        saw_surface |= active;
        saw_far |= !active;
        let mut frame = CelestialFrame::new(&view, &mut staging, screen, &sphere);
        if active {
            frame
                .append_surface(
                    body,
                    session.lod().active_visible(),
                    session.lod().topology(),
                    SurfaceStyle::default(),
                )
                .unwrap();
        }
        frame.append_body_observations(&[body], &[active]).unwrap();
        assert_eq!(frame.markers().len(), 1);
        assert_eq!(
            frame.markers()[0].representation == SphereRepresentation::Surface,
            active
        );
        if active {
            assert_eq!(frame.report().triangles, 0);
        }
        eprintln!(
            "handoff h={clearance} owner={:?} far_error={} surface_error={} visible={} upload={}",
            session.state(),
            session.far_error_pixels,
            session.report.max_error_pixels,
            session.report.visible_patches,
            frame.report().surface.uploaded_bytes
        );
    }
    assert!(saw_far && saw_surface);
    assert_eq!(sessions[0].state(), SurfaceRepresentationState::Far);
}

#[test]
fn starved_rapid_approach_keeps_complete_surface_owner_and_returns_when_far_ready() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let body_id = ids[1];
    let source = frames.frames_for(body_id).unwrap().body_fixed;
    let radius = world
        .body(body_id)
        .unwrap()
        .properties()
        .reference_radius_m();
    let revision = world.revision();
    let before: Vec<_> = world
        .bodies()
        .map(|(id, body)| (id, body.clone()))
        .collect();
    let instant = world.sample_time();
    let body = CelestialRenderBody {
        body_fixed_frame: source,
        reference_radius_m: radius,
        color: [0.2, 0.5, 1.0, 1.0],
        unlit: false,
        selected: true,
    };
    let screen = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let settings = LodSettings::default().with_work_limit(0).unwrap();
    let sphere = Icosphere::new();
    let mut staging = CelestialStaging::default();
    let mut session = PlanetSurfaceSession::new(body_id, 2048).unwrap();

    let close_view = PreparedView::new(
        &frames.tree().evaluate(),
        FramePose::new(
            FramePosition::new(
                source,
                LocalPosition::try_metres(DVec3::Z * (radius + 2.0)).unwrap(),
            ),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    for _ in 0..8 {
        session
            .update(
                &SurfaceViewInput {
                    view: &close_view,
                    body_fixed_frame: source,
                    reference_radius_m: radius,
                    projection: screen,
                },
                &settings,
            )
            .unwrap();
    }
    assert_eq!(session.state(), SurfaceRepresentationState::Surface);
    assert_eq!(
        session.lod().covering_leaves().collect::<Vec<_>>(),
        mundaris_math::surface::CubeFace::ALL
            .map(mundaris_math::surface::CubePatchAddress::root)
            .to_vec()
    );
    assert!(session.report.quality_pending);
    assert!(session.report.max_error_pixels > 0.125);

    let mut frame = CelestialFrame::new(&close_view, &mut staging, screen, &sphere);
    frame
        .append_surface(
            body,
            session.lod().active_visible(),
            session.lod().topology(),
            SurfaceStyle::default(),
        )
        .unwrap();
    frame.append_body_observations(&[body], &[true]).unwrap();
    assert_eq!(frame.markers().len(), 1);
    assert_eq!(
        frame.markers()[0].representation,
        SphereRepresentation::Surface
    );
    assert_eq!(frame.report().triangles, 0);
    let surface = frame.report().surface;
    assert!(surface.samples > 0 || surface.fallback_triangles > 0);
    assert!(surface.samples <= 4096 * 289);
    assert!(surface.fallback_triangles <= 4096 * 512);
    assert!(surface.max_component_error_m.is_finite());
    assert!(surface.max_projected_error_pixels.is_finite());
    assert!(surface.max_projected_error_pixels <= 0.05);
    session.return_to_far_if_ready(false);
    assert_eq!(session.state(), SurfaceRepresentationState::Surface);
    let far_view = PreparedView::new(
        &frames.tree().evaluate(),
        FramePose::new(
            FramePosition::new(
                source,
                LocalPosition::try_metres(DVec3::Z * (radius + 1e11)).unwrap(),
            ),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    session
        .update(
            &SurfaceViewInput {
                view: &far_view,
                body_fixed_frame: source,
                reference_radius_m: radius,
                projection: screen,
            },
            &settings,
        )
        .unwrap();
    assert!(session.far_error_pixels < 0.05);
    session.return_to_far_if_ready(false);
    assert_eq!(session.state(), SurfaceRepresentationState::Surface);
    let mut far_probe = CelestialFrame::new(&far_view, &mut staging, screen, &sphere);
    far_probe.append_bodies(&[body]).unwrap();
    assert!(matches!(
        far_probe.markers()[0].representation,
        SphereRepresentation::PhysicalSphere | SphereRepresentation::SubpixelMarker
    ));
    session.return_to_far_if_ready(true);
    assert_eq!(session.state(), SurfaceRepresentationState::Far);
    assert_eq!(world.revision(), revision);
    assert_eq!(world.sample_time(), instant);
    assert_eq!(
        world
            .bodies()
            .map(|(id, body)| (id, body.clone()))
            .collect::<Vec<_>>(),
        before
    );
}
