use glam::DVec3;
use mundaris_app::{
    solar_system::{SolarBody, SolarSystemPreset},
    terrain_inspection::{clearance_at_position, terrain_clearance},
};
use mundaris_math::*;
use mundaris_world::*;
use std::{num::NonZeroU64, time::Duration};

#[test]
fn terrain_clearance_is_complete_read_only_and_absent_for_gas_giants() {
    let world = SolarSystemPreset::gameplay()
        .create(NonZeroU64::new(581).unwrap())
        .unwrap();
    let projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(581).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    let revision = world.revision();
    for body_kind in [SolarBody::Earth, SolarBody::Moon, SolarBody::Mars] {
        let (id, body) = world.bodies().nth(body_kind as usize).unwrap();
        let terrain = body.terrain().unwrap();
        let radius = body.properties().reference_radius_m();
        for direction in [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            -DVec3::X,
            -DVec3::Y,
            -DVec3::Z,
        ] {
            let measured =
                clearance_at_position(terrain, radius, direction * (radius + 100.0), id).unwrap();
            assert!((measured.clearance_m - (100.0 - measured.terrain_elevation_m)).abs() < 1e-7);
            assert_eq!(measured.sphere_altitude_m, 100.0);
            assert!(measured.slope_angle_rad.is_finite());
        }
        let fixed = projection.frames_for(id).unwrap().body_fixed;
        let pose = FramePose::new(
            FramePosition::new(
                fixed,
                LocalPosition::try_metres(DVec3::Z * (radius + 100.0)).unwrap(),
            ),
            UnitRotation::identity(),
        );
        assert!(terrain_clearance(&pair, pose, id).unwrap().is_some());
    }
    let (neptune, body) = world.bodies().nth(SolarBody::Neptune as usize).unwrap();
    let fixed = projection.frames_for(neptune).unwrap().body_fixed;
    let pose = FramePose::new(
        FramePosition::new(
            fixed,
            LocalPosition::try_metres(DVec3::Z * (body.properties().reference_radius_m() + 100.0))
                .unwrap(),
        ),
        UnitRotation::identity(),
    );
    assert!(terrain_clearance(&pair, pose, neptune).unwrap().is_none());
    assert_eq!(world.revision(), revision);
}

#[test]
fn rocky_body_orbit_approach_smooths_clearance_above_complete_terrain() {
    use mundaris_app::celestial_camera::{CelestialCamera, NavigationInput};
    let world = SolarSystemPreset::gameplay()
        .create(NonZeroU64::new(582).unwrap())
        .unwrap();
    let projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(582).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    for body_kind in [SolarBody::Earth, SolarBody::Moon, SolarBody::Mars] {
        let (body, celestial) = world.bodies().nth(body_kind as usize).unwrap();
        let radius = celestial.properties().reference_radius_m();
        for clearance in [2.0, 10.0, 100.0, 1_000.0, 10_000.0, 100_000.0] {
            let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.0e11).unwrap();
            camera.focus(&pair, body, true, true).unwrap();
            camera.target_clearance(&pair, clearance).unwrap();
            for _ in 0..12 {
                camera
                    .update_navigation(
                        &pair,
                        &NavigationInput::default(),
                        Duration::from_millis(500),
                    )
                    .unwrap();
            }
            let diagnostic = terrain_clearance(&pair, camera.pose(), body)
                .unwrap()
                .unwrap();
            assert!(
                (diagnostic.clearance_m - clearance).abs() < 0.1,
                "{body_kind:?} target={clearance} actual={}",
                diagnostic.clearance_m
            );
            assert!(
                (camera.measured_clearance(&pair, body).unwrap()
                    - (clearance + diagnostic.terrain_elevation_m))
                    .abs()
                    < 0.1
            );
            assert!(radius > 0.0);
        }
    }
}

#[test]
fn translating_orbit_queries_fixed_direction_and_guard_preserves_orientation() {
    use mundaris_app::celestial_camera::{CelestialCamera, NavigationInput};
    let world = SolarSystemPreset::gameplay()
        .create(NonZeroU64::new(583).unwrap())
        .unwrap();
    let projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(583).unwrap()).unwrap();
    let pair = projection.coherent_view(&world).unwrap();
    let (body, _) = world.bodies().nth(SolarBody::Earth as usize).unwrap();
    let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1e11).unwrap();
    camera.focus(&pair, body, false, true).unwrap();
    camera.target_clearance(&pair, 2.0).unwrap();
    for _ in 0..12 {
        camera
            .update_navigation(
                &pair,
                &NavigationInput::default(),
                Duration::from_millis(500),
            )
            .unwrap();
    }
    let c = terrain_clearance(&pair, camera.pose(), body)
        .unwrap()
        .unwrap();
    assert!((c.clearance_m - 2.0).abs() < 1e-5);
    let before = camera.pose().orientation();
    assert!(
        camera
            .enforce_radial_clearance(&pair, body, c.surface_radius_m + 50.0, 10.0)
            .unwrap()
    );
    assert!(
        (before
            .quaternion()
            .dot(camera.pose().orientation().quaternion())
            .abs()
            - 1.0)
            .abs()
            < 1e-12
    );
    let guarded = terrain_clearance(&pair, camera.pose(), body)
        .unwrap()
        .unwrap();
    assert!((guarded.clearance_m - 60.0).abs() < 1e-7);
    camera
        .update_navigation(
            &pair,
            &NavigationInput::default(),
            Duration::from_millis(500),
        )
        .unwrap();
    assert!(
        (terrain_clearance(&pair, camera.pose(), body)
            .unwrap()
            .unwrap()
            .clearance_m
            - 60.0)
            .abs()
            < 1e-5
    );
    camera.enter_surface_inspection(&pair, body).unwrap();
    camera.target_clearance(&pair, 2.0).unwrap();
    camera.set_terrain_clearance_guard(Some(10.0)).unwrap();
    camera
        .update_navigation(
            &pair,
            &NavigationInput::default(),
            Duration::from_millis(16),
        )
        .unwrap();
    assert!(
        (terrain_clearance(&pair, camera.pose(), body)
            .unwrap()
            .unwrap()
            .clearance_m
            - 10.0)
            .abs()
            < 1e-6
    );
    assert!(camera.set_terrain_clearance_guard(Some(f64::NAN)).is_err());
}
