use glam::DVec3;
use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_world::{terrain::*, *};
use std::num::NonZeroU64;

#[test]
fn checkpoint_generator_and_geometry_support_arbitrary_body_radii() {
    for radius_m in [
        50_000.0,
        100_000.0,
        400_000.0,
        1_000_000.0,
        6_371_000.0,
        12_742_000.0,
    ] {
        let definition = checkpoint_terrain_definition(radius_m).unwrap();
        let generator = TerrainGenerator::new(&definition, radius_m).unwrap();
        let directions = [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            DVec3::new(1.0, 2.0, 3.0).normalize(),
        ];
        let locations = directions
            .map(|direction| SurfaceLocation::new(Direction3::try_new(direction).unwrap()));
        let mut complete = [TerrainSample::default(); 4];
        generator
            .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut complete)
            .unwrap();
        let mut repeated = [TerrainSample::default(); 4];
        generator
            .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut repeated)
            .unwrap();
        assert_eq!(complete, repeated);
        for (direction, location, sample) in directions
            .into_iter()
            .zip(locations)
            .zip(complete)
            .map(|((d, l), s)| (d, l, s))
        {
            assert!(sample.height_m().is_finite());
            let point = direction * (radius_m + sample.height_m());
            assert!(point.is_finite());
            let normal = sample.normal_body(location, radius_m).unwrap();
            assert!(normal.unit().is_finite());
            assert!(point.dot(normal.unit()) > 0.0);
        }
        let footprint = TerrainFootprint::new(100.0).unwrap();
        let filtered = generator.evaluate_batch(&locations, footprint, &mut repeated);
        assert!(filtered.is_ok(), "radius={radius_m}");

        let mut world = CelestialSystem::new(NonZeroU64::new(77).unwrap(), SimulationInstant::ZERO);
        let body = world
            .insert_body(
                "scale fixture",
                BodyProperties::new(1.0, radius_m).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
        world.edit_terrain(body, Some(definition.clone())).unwrap();
        let body_state = world.body(body).unwrap();
        let identity =
            TerrainGeometryIdentity::new(body, definition, body_state.terrain_revision(), radius_m)
                .unwrap();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap();
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        assert!(cache.request(&identity, address));
        while cache.peek(&identity, address).is_none() {
            assert!(
                cache
                    .generate(17 * 17, GENERATION_MICROBATCH, None)
                    .unwrap()
                    .vertices_generated
                    > 0
            );
        }
        for sample in cache.peek(&identity, address).unwrap().samples() {
            assert!(sample.position_body_m.is_finite() && sample.normal_body.is_finite());
            assert!(sample.position_body_m.dot(sample.normal_body) > 0.0);
        }
    }
}
