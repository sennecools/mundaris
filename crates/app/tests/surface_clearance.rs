use glam::DVec3;
use astrum_app::celestial_camera::{CelestialCamera, NavigationInput};
use astrum_app::terrain_inspection::{clearance_at_body_position, terrain_clearance};
use astrum_math::surface::SurfaceLocation;
use astrum_math::*;
use astrum_world::{
    BodyProperties, BodyState, CelestialFrameProjection, CelestialSystem, SimulationInstant,
    terrain::*,
};
use std::num::NonZeroU64;
use std::time::Duration;

#[test]
fn clearance_uses_composed_shape_relief_and_combined_surface_normal() {
    let namespace = NonZeroU64::new(7101).unwrap();
    let mut world = CelestialSystem::new(namespace, SimulationInstant::ZERO);
    let radius = 100_000.0;
    let body = world
        .insert_body(
            "irregular test body",
            BodyProperties::new(1.0e20, radius).unwrap(),
            BodyState::new(
                LocalPosition::origin(),
                LinearVelocity3::zero(),
                UnitRotation::identity(),
                AngularVelocity3::zero(),
            ),
        )
        .unwrap();
    let surface = SurfaceDefinition::new(
        TerrainIdentity(31),
        TerrainSeed(32),
        ShapeDefinition::ellipsoid([1.12, 0.91, 1.0]).unwrap(),
        SurfaceTerrainDefinition::generated(SurfaceAlgorithm::RockyV3, TerrainSeed(32)),
        SurfaceMaterialDefinition::new(SurfaceMaterialVersion::RockyV1, 0.6, 0.4).unwrap(),
        SurfaceAtmosphere::Airless,
    )
    .unwrap();
    world
        .edit_surface_definition(body, Some(surface.clone()))
        .unwrap();

    let direction = DVec3::X;
    let location = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
    let expected = SurfaceGenerator::new(&surface, radius)
        .unwrap()
        .evaluate_point(location)
        .unwrap();
    let camera_position = direction * (expected.radius_m() + 50.0);
    let clearance = clearance_at_body_position(world.body(body).unwrap(), camera_position, body)
        .unwrap()
        .unwrap();
    assert!((clearance.surface_radius_m - expected.radius_m()).abs() < 1e-8);
    assert!((clearance.terrain_elevation_m - expected.terrain().height_m()).abs() < 1e-8);
    assert!((clearance.clearance_m - 50.0).abs() < 1e-8);
    let expected_slope = expected.normal().dot(direction).clamp(-1.0, 1.0).acos();
    assert!((clearance.slope_angle_rad - expected_slope).abs() < 1e-12);
    assert!((expected.shape().radius_m() - radius * 1.12).abs() < 1e-8);

    let projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(7102).unwrap()).unwrap();
    let fixed = projection.frames_for(body).unwrap().body_fixed;
    let pose = FramePose::new(
        FramePosition::new(fixed, LocalPosition::try_metres(camera_position).unwrap()),
        UnitRotation::identity(),
    );
    let queried = terrain_clearance(&projection.coherent_view(&world).unwrap(), pose, body)
        .unwrap()
        .unwrap();
    assert_eq!(queried.surface_radius_m, clearance.surface_radius_m);
    assert_eq!(queried.slope_angle_rad, clearance.slope_angle_rad);
}

fn zero_relief_ellipsoid(identity: u64, seed: u64, z_axis_fraction: f64) -> SurfaceDefinition {
    let parameters = GeologicalParameters {
        age: 0.8,
        activity: 0.2,
        resurfacing_fraction: 0.4,
        impact_retention: 0.7,
        relief_fraction: 0.0,
        feature_scale_fraction: 0.2,
        orientation_radians: 0.0,
    };
    SurfaceDefinition::new(
        TerrainIdentity(identity),
        TerrainSeed(seed),
        ShapeDefinition::ellipsoid([1.0, 0.9, z_axis_fraction]).unwrap(),
        SurfaceTerrainDefinition::new(SurfaceAlgorithm::IcyV1, parameters).unwrap(),
        SurfaceMaterialDefinition::new(SurfaceMaterialVersion::IcyV1, 0.4, 0.3).unwrap(),
        SurfaceAtmosphere::Airless,
    )
    .unwrap()
}

#[test]
fn camera_guard_uses_composed_envelope_and_invalidates_clearance_cache() {
    let namespace = NonZeroU64::new(7201).unwrap();
    let mut world = CelestialSystem::new(namespace, SimulationInstant::ZERO);
    let radius = 100_000.0;
    let body = world
        .insert_body(
            "extended ellipsoid",
            BodyProperties::new(1.0e20, radius).unwrap(),
            BodyState::new(
                LocalPosition::origin(),
                LinearVelocity3::zero(),
                UnitRotation::identity(),
                AngularVelocity3::zero(),
            ),
        )
        .unwrap();
    let mut projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(7202).unwrap()).unwrap();
    let first = zero_relief_ellipsoid(41, 42, 1.35);
    world
        .edit_surface_definition(body, Some(first.clone()))
        .unwrap();

    let mut camera;
    let guarded;
    {
        let pair = projection.coherent_view(&world).unwrap();
        let point = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
        let surface_radius = SurfaceGenerator::new(&first, radius)
            .unwrap()
            .evaluate_point(point)
            .unwrap()
            .radius_m();
        assert!(surface_radius > 1.1 * radius);
        let initial_position = DVec3::Z * (surface_radius + 0.5);
        let extent = radius;
        let center = initial_position - DVec3::Z * (2.5 * extent);
        camera = CelestialCamera::overview(&pair, center, extent).unwrap();
        camera.enter_free_flight(&pair).unwrap();
        let before = terrain_clearance(&pair, camera.pose(), body)
            .unwrap()
            .unwrap();
        assert!((before.clearance_m - 0.5).abs() < 1e-6);

        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::ZERO)
            .unwrap();
        guarded = camera.recorded_terrain_clearance().unwrap();
        assert!(guarded.clearance_m >= 1.0 - 1e-6);
        assert!(guarded.surface_radius_m > 1.1 * radius);
    }

    // The terrain-only edit keeps frame projection coherent but changes the
    // complete surface identity cached by the camera.
    let second = zero_relief_ellipsoid(43, 42, 1.45);
    world
        .edit_surface_definition(body, Some(second.clone()))
        .unwrap();
    let changed_definition;
    {
        let pair = projection.coherent_view(&world).unwrap();
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::ZERO)
            .unwrap();
        changed_definition = camera.recorded_terrain_clearance().unwrap();
    }
    assert!(changed_definition.surface_radius_m > guarded.surface_radius_m);
    assert!(changed_definition.clearance_m >= 1.0 - 1e-6);

    // A property edit invalidates the cached reference radius. Re-publish into
    // the same projection so existing frame IDs and camera pose survive.
    world
        .edit_properties(body, BodyProperties::new(1.0e20, 120_000.0).unwrap())
        .unwrap();
    projection.publish(&world).unwrap();
    {
        let pair = projection.coherent_view(&world).unwrap();
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::ZERO)
            .unwrap();
    }
    let changed_radius = camera.recorded_terrain_clearance().unwrap();
    assert!(changed_radius.surface_radius_m > changed_definition.surface_radius_m);
    assert!(changed_radius.clearance_m >= 1.0 - 1e-6);
}
