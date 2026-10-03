use glam::DVec3;
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::surface::PatchEdge;
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::terrain::*;
use mundaris_world::*;
use std::num::NonZeroU64;

fn fixture() -> (
    CelestialSystem,
    CelestialFrameProjection,
    TerrainGeometryIdentity,
    FrameId,
    f64,
) {
    let mut world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(73).unwrap())
        .unwrap();
    let body = world.bodies().nth(1).unwrap().0;
    let radius = world.body(body).unwrap().properties().reference_radius_m();
    let band = TerrainBandConfig::new(
        8.0,
        TerrainScale::Metres {
            longest_wavelength_m: 100_000.0,
        },
        2,
    )
    .unwrap();
    let definition = TerrainDefinition::new(
        TerrainIdentity(700),
        TerrainSeed(19),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(
            [band; 5],
            TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap(),
        )
        .unwrap(),
    );
    world.edit_terrain(body, Some(definition)).unwrap();
    let body_state = world.body(body).unwrap();
    let identity = TerrainGeometryIdentity::new(
        body,
        body_state.terrain().unwrap().clone(),
        body_state.terrain_revision(),
        radius,
    )
    .unwrap();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(73).unwrap()).unwrap();
    let frame = frames.frames_for(body).unwrap().body_fixed;
    (world, frames, identity, frame, radius)
}

fn view<'tree>(
    pair: &CoherentCelestialView<'tree>,
    frame: FrameId,
    radius: f64,
    height: f64,
) -> PreparedView<'tree> {
    let observer = FramePose::new(
        FramePosition::new(
            frame,
            LocalPosition::try_metres(DVec3::Z * (radius + height)).unwrap(),
        ),
        UnitRotation::identity(),
    );
    PreparedView::new(
        &pair.evaluation(),
        observer,
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap()
}

fn assert_cover(cover: &AdaptiveTerrainCover) {
    let active = cover.active();
    assert!(!active.is_empty());
    let area: u128 = active
        .iter()
        .map(|p| 1u128 << (2 * (30 - p.address.level())))
        .sum();
    assert_eq!(
        area,
        6u128 << 60,
        "active patches must cover the sphere exactly"
    );
    for patch in active {
        for edge in PatchEdge::ALL {
            let mut neighbor = Some(patch.address.neighbor(edge).address);
            let other = loop {
                match neighbor {
                    Some(address) if active.iter().any(|p| p.address == address) => {
                        break active.iter().find(|p| p.address == address);
                    }
                    Some(address) => neighbor = address.parent(),
                    None => break None,
                }
            };
            if let Some(other) = other {
                assert!(
                    patch.address.level().abs_diff(other.address.level()) <= 1,
                    "unbalanced edge {:?} / {:?}",
                    patch.address,
                    other.address
                );
            }
        }
    }
}

#[test]
fn delayed_roots_and_siblings_publish_only_complete_balanced_covers() {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let far_view = view(&pair, frame, radius, radius * 3.0);
    let far = SurfaceViewInput {
        view: &far_view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection: CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap(),
    };
    let settings = LodSettings::default().with_limits(256, 256, 2).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    cover
        .update(&mut cache, &identity, &far, &settings, 0, None)
        .unwrap();
    assert!(!cover.ready());
    assert!(cover.active().is_empty());
    for _ in 0..300 {
        cover
            .update(&mut cache, &identity, &far, &settings, 256, None)
            .unwrap();
        if !cover.active().is_empty() {
            break;
        }
    }
    assert_cover(&cover);
    assert!(cover.ready());
    for _ in 0..500 {
        cover
            .update(&mut cache, &identity, &far, &settings, 256, None)
            .unwrap();
        assert_cover(&cover);
        let report = cache.report();
        assert!(cache.pending() <= MAX_PENDING_PATCHES);
        assert!(
            report.resident_bytes + cover.resident_bytes() + report.external_bytes
                <= TERRAIN_CPU_CAP_BYTES
        );
    }
    assert!(cache.report().pinned_patches >= 6);
}

#[test]
fn oscillating_observer_is_bounded_and_settled_updates_do_no_generation() {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let near_view = view(&pair, frame, radius, 20.0);
    let far_view = view(&pair, frame, radius, radius * 4.0);
    let projection = CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap();
    let near = SurfaceViewInput {
        view: &near_view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection,
    };
    let far = SurfaceViewInput {
        view: &far_view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection,
    };
    let settings = LodSettings::default().with_limits(256, 256, 2).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    for i in 0..160 {
        let view = if i % 2 == 0 { &near } else { &far };
        cover
            .update(&mut cache, &identity, view, &settings, 128, None)
            .unwrap();
        if cover.ready() {
            assert_cover(&cover);
        }
        assert!(cache.pending() <= MAX_PENDING_PATCHES);
        let report = cache.report();
        assert!(
            report.resident_bytes + cover.resident_bytes() + report.external_bytes
                <= TERRAIN_CPU_CAP_BYTES
        );
    }
    // Settle the coarse selection, then prove updates do not regenerate samples.
    for _ in 0..300 {
        cover
            .update(&mut cache, &identity, &far, &settings, 512, None)
            .unwrap();
    }
    assert!(
        cache.report().pinned_patches <= cover.active().len() + 6,
        "coarsening must release obsolete fine-cover pins while retaining root fallback"
    );
    let before = cache.report().hits;
    for _ in 0..100 {
        let work = cover
            .update(&mut cache, &identity, &far, &settings, 512, None)
            .unwrap();
        assert_eq!(work.vertices_generated, 0);
        assert_eq!(work.patches_completed, 0);
        assert_cover(&cover);
        assert!(
            cache.report().resident_bytes + cover.resident_bytes() + cache.report().external_bytes
                <= TERRAIN_CPU_CAP_BYTES
        );
    }
    assert_eq!(cache.report().hits, before);
}

#[test]
fn checkpoint_v2_cover_has_finite_outward_surface_normals() {
    let (mut world, frames, _, frame, radius) = fixture();
    let body = world.bodies().nth(1).unwrap().0;
    world
        .edit_terrain(body, Some(checkpoint_terrain_definition(radius).unwrap()))
        .unwrap();
    let body_state = world.body(body).unwrap();
    let identity = TerrainGeometryIdentity::new(
        body,
        body_state.terrain().unwrap().clone(),
        body_state.terrain_revision(),
        radius,
    )
    .unwrap();
    let pair = frames.coherent_view(&world).unwrap();
    let prepared_view = view(&pair, frame, radius, radius * 3.0);
    let view = SurfaceViewInput {
        view: &prepared_view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection: CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap(),
    };
    let settings = LodSettings::default().with_limits(256, 256, 2).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    for _ in 0..300 {
        cover
            .update(&mut cache, &identity, &view, &settings, 1024, None)
            .unwrap();
        if cover.ready() && cover.active().iter().all(|p| p.address.level() <= 2) {
            break;
        }
    }
    assert!(cover.ready());
    for patch in cover.surface().unwrap().patches() {
        for sample in patch.samples() {
            assert!(sample.position_body_m.is_finite() && sample.normal_body.is_finite());
            assert!(
                sample.position_body_m.dot(sample.normal_body) > 0.0,
                "normal must face outward"
            );
        }
    }
}
