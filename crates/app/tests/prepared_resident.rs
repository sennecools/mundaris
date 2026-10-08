use glam::DVec3;
use mundaris_app::{
    resident_terrain::{ResidentTileBuilder, SharedDerivedField, TileBuildIdentity},
    solar_system::{SolarBody, SolarSystemPreset},
    terrain_inspection::clearance_at_body_position,
};
use mundaris_math::Direction3;
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge, SurfaceLocation};
use mundaris_world::terrain::{
    PreparedSurface, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use std::{num::NonZeroU64, path::PathBuf, sync::Arc};
fn source() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/prepared/composition.native.json")
}
fn generator() -> SurfaceGenerator {
    SurfaceGenerator::new(
        &SurfaceDefinition::from_prepared(
            TerrainIdentity(81273),
            TerrainSeed(81273),
            Arc::new(PreparedSurface::load(source()).unwrap()),
        ),
        100000.,
    )
    .unwrap()
}
fn identity() -> TileBuildIdentity {
    TileBuildIdentity {
        body_identity: 81273,
        surface_revision: 1,
        material_revision: 1,
    }
}

#[test]
fn prepared_resident_is_repeatable_and_edges_match() {
    let generator = generator();
    let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 256, 256).unwrap();
    let (a, diagnostics) = ResidentTileBuilder::build(&generator, identity(), address, 8).unwrap();
    let (b, _) = ResidentTileBuilder::build(&generator, identity(), address, 8).unwrap();
    assert_eq!(a.key, b.key);
    assert_eq!(a.texels, b.texels);
    assert!(
        diagnostics.surface_generator_retained_heap_bytes
            <= diagnostics.surface_generator_heap_bound_bytes
    );
    assert!(
        a.key
            .definition_words
            .windows(
                generator
                    .definition()
                    .prepared()
                    .unwrap()
                    .exact_definition_words()
                    .len()
            )
            .any(|w| w
                == generator
                    .definition()
                    .prepared()
                    .unwrap()
                    .exact_definition_words())
    );
    // Existing global-direction filter/stitch tests continue to own cross-face
    // geometry agreement; this additionally exercises prepared neighbours.
    for edge in PatchEdge::ALL {
        let neighbor = address.neighbor(edge);
        let (tile, _) =
            ResidentTileBuilder::build(&generator, identity(), neighbor.address, 8).unwrap();
        for along in 0..=8 {
            let source = edge.grid(along, 8);
            let target = neighbor
                .edge
                .grid(if neighbor.reversed { 8 - along } else { along }, 8);
            let at = a.texels[((source[1] + 1) * 11 + source[0] + 1) as usize];
            let bt = tile.texels[((target[1] + 1) * 11 + target[0] + 1) as usize];
            assert!(
                ((a.anchor_radius_m + at.radial_offset_m as f64)
                    - (tile.anchor_radius_m + bt.radial_offset_m as f64))
                    .abs()
                    < 1e-3
            );
            assert_eq!(at.material, bt.material);
        }
    }
}
#[test]
fn prepared_identity_invalidates_tiles_and_derived_bindings() {
    let g = generator();
    let shared = SharedDerivedField::new(&g).unwrap();
    assert!(shared.is_bound_to(&g));
    let a = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 256, 256).unwrap();
    let before = ResidentTileBuilder::tile_key(&g, identity(), a, 8).unwrap();
    let mut changed = identity();
    changed.surface_revision += 1;
    assert_ne!(
        before,
        ResidentTileBuilder::tile_key(&g, changed, a, 8).unwrap()
    );
    let prepared = Arc::clone(g.definition().prepared().unwrap());
    let different = SurfaceGenerator::new(
        &SurfaceDefinition::from_prepared(TerrainIdentity(81274), TerrainSeed(81273), prepared),
        100000.,
    )
    .unwrap();
    assert!(!shared.is_bound_to(&different));
    assert_ne!(
        before.definition_words,
        ResidentTileBuilder::tile_key(&different, identity(), a, 8)
            .unwrap()
            .definition_words
    );
}
#[test]
fn shared_scene_uses_same_world_and_clearance_authority_after_motion() {
    let (mut system, motion) = SolarSystemPreset::gameplay()
        .create_analytic(NonZeroU64::new(734).unwrap())
        .unwrap();
    let body = system.bodies().find(|(_, b)| b.name() == "Moon").unwrap().0;
    let definition = system
        .body(body)
        .unwrap()
        .surface_definition()
        .unwrap()
        .clone();
    assert!(definition.prepared().is_some());
    let radius = system.body(body).unwrap().properties().reference_radius_m();
    let g = SurfaceGenerator::new(&definition, radius).unwrap();
    let n = DVec3::new(0.0018, 0.0005, 1.).normalize();
    let sample = g
        .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
        .unwrap();
    let clearance = clearance_at_body_position(
        system.body(body).unwrap(),
        n * (sample.radius_m() + 1200.),
        body,
    )
    .unwrap()
    .unwrap();
    assert!((clearance.clearance_m - 1200.).abs() < 1e-9);
    assert_eq!(clearance.terrain_elevation_m, sample.terrain().height_m());
    let mut producer = mundaris_simulation::AnalyticMotionProducer::new(&system, motion).unwrap();
    producer
        .sample(
            &mut system,
            mundaris_math::SimulationInstant::try_seconds_since_epoch(1000.).unwrap(),
        )
        .unwrap();
    assert_eq!(
        system.body(body).unwrap().surface_definition().unwrap(),
        &definition
    );
    assert_eq!(
        g.evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
            .unwrap(),
        sample
    );
    assert_eq!(
        mundaris_app::solar_system::content(SolarBody::Moon).name,
        "Moon"
    );
}
