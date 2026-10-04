use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_renderer::planet_surface::GRID_SAMPLES;
use mundaris_world::{terrain::*, *};
use std::num::NonZeroU64;

fn fixture() -> TerrainGeometryIdentity {
    let mut world = CelestialSystem::new(NonZeroU64::new(711).unwrap(), SimulationInstant::ZERO);
    let body = world
        .insert_body(
            "headroom",
            BodyProperties::new(1.0, 10_000.0).unwrap(),
            BodyState::new(
                LocalPosition::origin(),
                LinearVelocity3::zero(),
                UnitRotation::identity(),
                AngularVelocity3::zero(),
            ),
        )
        .unwrap();
    let band = TerrainBandConfig::new(
        1.0,
        TerrainScale::Metres {
            longest_wavelength_m: 128.0,
        },
        1,
    )
    .unwrap();
    let definition = TerrainDefinition::new(
        TerrainIdentity(1),
        TerrainSeed(2),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(
            [band; 5],
            TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap(),
        )
        .unwrap(),
    );
    TerrainGeometryIdentity::new(body, definition, TerrainRevision::default(), 10_000.0).unwrap()
}

#[test]
fn headroom_setter_is_opt_in_and_blocked_work_remains_pending() {
    let identity = fixture();
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cache = TerrainPatchCache::new(2 * 1024 * 1024, 4).unwrap();
    assert!(cache.request(&identity, address));
    assert_eq!(cache.report().reservation_rejected, 0);

    // 16 MiB is a useful caller-selected measurement point; small test quota
    // clamps it to the cap and demonstrates a soft admission rejection.
    cache.set_soft_residency_headroom(16 * 1024 * 1024);
    let blocked = cache
        .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
        .unwrap();
    assert_eq!(blocked.vertices_generated, 0);
    assert_eq!(cache.pending(), 1);
    assert_eq!(cache.report().reservation_rejected, 1);
    assert!(cache.report().eviction_attempts > 0);

    cache.set_soft_residency_headroom(0);
    let completed = cache
        .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
        .unwrap();
    assert_eq!(completed.patches_completed, 1);
    assert!(cache.report().peak_aggregate_bytes <= 2 * 1024 * 1024);
}
