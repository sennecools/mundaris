use mundaris_math::{AngularVelocity3, LinearVelocity3, LocalPosition, UnitRotation};
use mundaris_world::{
    BodyProperties, BodyState, CelestialFrameProjection, CelestialSystem, SimulationInstant,
    terrain::*,
};
use std::num::NonZeroU64;

fn body_state() -> BodyState {
    BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}

fn legacy_definition() -> TerrainDefinition {
    let band = TerrainBandConfig::new(
        1.0,
        TerrainScale::Metres {
            longest_wavelength_m: 128.0,
        },
        2,
    )
    .unwrap();
    TerrainDefinition::new(
        TerrainIdentity(11),
        TerrainSeed(12),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(
            [band; 5],
            TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap(),
        )
        .unwrap(),
    )
}

#[test]
fn surface_publication_is_exclusive_transactional_and_preserves_frame_authority() {
    let namespace = NonZeroU64::new(7001).unwrap();
    let mut world = CelestialSystem::new(namespace, SimulationInstant::ZERO);
    let state = body_state();
    let body = world
        .insert_body(
            "test body",
            BodyProperties::new(1.0e20, 10_000.0).unwrap(),
            state,
        )
        .unwrap();
    let original_world_revision = world.revision();
    let projection =
        CelestialFrameProjection::build(&world, NonZeroU64::new(7002).unwrap()).unwrap();

    let legacy = legacy_definition();
    world.edit_terrain(body, Some(legacy.clone())).unwrap();
    assert_eq!(world.body(body).unwrap().terrain(), Some(&legacy));
    assert!(world.body(body).unwrap().has_surface());
    assert_eq!(world.body(body).unwrap().terrain_revision().value(), 1);

    let surface = SurfaceDefinition::generated(
        TerrainIdentity(21),
        TerrainSeed(22),
        SurfaceAlgorithm::RockyV3,
    );
    world
        .edit_surface_definition(body, Some(surface.clone()))
        .unwrap();
    let published = world.body(body).unwrap();
    assert_eq!(published.terrain(), None);
    assert_eq!(published.surface_definition(), Some(&surface));
    assert!(published.has_surface());
    assert_eq!(published.terrain_revision().value(), 2);
    assert_eq!(world.revision(), original_world_revision);
    assert_eq!(*published.state(), state);
    assert!(projection.coherent_view(&world).is_ok());

    let before_failed_edit = world.body(body).unwrap().clone();
    assert!(
        world
            .edit_body(
                body,
                "rejected rename",
                BodyProperties::new(2.0e20, 1.0e-8).unwrap(),
            )
            .is_err()
    );
    assert!(
        world
            .edit_properties(body, BodyProperties::new(2.0e20, 1.0e-8).unwrap())
            .is_err()
    );
    assert_eq!(world.body(body).unwrap(), &before_failed_edit);
    assert_eq!(world.revision(), original_world_revision);

    // The old API remains usable and replaces the newer authority atomically.
    world.edit_terrain(body, Some(legacy.clone())).unwrap();
    let replaced = world.body(body).unwrap();
    assert_eq!(replaced.terrain(), Some(&legacy));
    assert_eq!(replaced.surface_definition(), None);
    assert_eq!(replaced.terrain_revision().value(), 3);
    assert_eq!(world.revision(), original_world_revision);
}
