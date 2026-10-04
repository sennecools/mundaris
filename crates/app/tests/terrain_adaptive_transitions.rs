use glam::DVec3;
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use mundaris_world::terrain::*;
use mundaris_world::*;
use std::{num::NonZeroU64, time::Duration};

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
    let state = world.body(body).unwrap();
    let identity = TerrainGeometryIdentity::new(
        body,
        state.terrain().unwrap().clone(),
        state.terrain_revision(),
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

fn input<'a, 'tree>(
    view: &'a PreparedView<'tree>,
    frame: FrameId,
    radius: f64,
) -> SurfaceViewInput<'a, 'tree> {
    SurfaceViewInput {
        view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection: CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap(),
    }
}

#[test]
fn refinement_waits_for_complete_destination_then_morphs_and_finishes_on_reversal() {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let far_view = view(&pair, frame, radius, radius * 3.0);
    let near_view = view(&pair, frame, radius, 20.0);
    let far = input(&far_view, frame, radius);
    let near = input(&near_view, frame, radius);
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    cover
        .set_morph_duration(Duration::from_millis(150))
        .unwrap();
    let roots = LodSettings::default().with_limits(256, 256, 0).unwrap();
    for _ in 0..100 {
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &far,
                &roots,
                64,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
        if cover.active().len() == 6 {
            break;
        }
    }
    assert_eq!(
        cover.active().len(),
        6,
        "roots must be the complete source cover"
    );
    let source: Vec<_> = cover.active().iter().map(|p| p.address).collect();
    let refined = LodSettings::default().with_limits(256, 256, 2).unwrap();
    let mut started = false;
    for _ in 0..300 {
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &near,
                &refined,
                64,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
        if cover.transition().is_some() {
            started = true;
            assert_eq!(
                cover.transition().unwrap().1,
                0.0,
                "new transitions begin at t=0"
            );
            assert_eq!(
                cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
                source,
                "logical source coverage stays active until transition completion"
            );
            break;
        }
        assert_eq!(
            cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
            source,
            "incomplete child geometry must not replace parent coverage"
        );
    }
    assert!(started, "refinement should eventually produce a transition");
    let mut previous = 0.0;
    let mut completed = false;
    for _ in 0..20 {
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &far,
                &refined,
                64,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
        if let Some((_, fraction)) = cover.transition() {
            assert!(
                fraction >= previous,
                "active transition fraction must be monotonic"
            );
            previous = fraction;
            assert!(fraction <= 1.0);
        } else {
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "camera reversal must finish the bounded transition, not swap its topology"
    );
    assert!(
        cover.active().iter().any(|p| p.address.level() > 0),
        "completed transition publishes destination cover"
    );
}

#[test]
fn exact_successor_constructs_during_frozen_morph_without_replacing_displayed_source() {
    check_frozen_successor(0, false);
}

#[test]
fn worker_successor_constructs_during_morph_and_retains_charged_endpoints() {
    check_frozen_successor(4, false);
}

#[test]
fn ready_successor_cancels_on_view_reversal_without_mutating_active_morph() {
    check_frozen_successor(4, true);
}

fn check_frozen_successor(workers: usize, reverse: bool) {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let near_view = view(&pair, frame, radius, 20.0);
    let near = input(&near_view, frame, radius);
    let mut cache =
        TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 256, workers).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    let roots = LodSettings::default().with_limits(256, 256, 0).unwrap();
    for _ in 0..2000 {
        if workers != 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        cover
            .update(&mut cache, &identity, &near, &roots, 64, None)
            .unwrap();
        if cover.ready() {
            break;
        }
    }
    cover
        .set_morph_duration(Duration::from_millis(150))
        .unwrap();
    let refined = LodSettings::default().with_limits(256, 256, 3).unwrap();
    for _ in 0..2000 {
        if workers != 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        cover
            .update(&mut cache, &identity, &near, &refined, 64, None)
            .unwrap();
        if cover.transition().is_some() {
            break;
        }
    }
    let source: Vec<_> = cover.active().iter().map(|p| p.address).collect();
    let first = cover.transition().expect("first complete transition");
    let first_new = first.0.triangles()[0][0].new.position_body_m;
    assert_eq!(first.1, 0.0);
    for _ in 0..2000 {
        if workers != 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        // No elapsed visual time: a serial morph gate would never make progress.
        cover
            .update(&mut cache, &identity, &near, &refined, 64, None)
            .unwrap();
        assert_eq!(
            cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
            source
        );
        let first = cover.transition().unwrap();
        assert_eq!(first.1, 0.0);
        assert_eq!(first.0.triangles()[0][0].new.position_body_m, first_new);
        assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
        if cover.successor_ready() {
            break;
        }
    }
    assert!(
        cover.successor_ready(),
        "one deeper complete endpoint should be constructed while the displayed morph is frozen"
    );
    if reverse {
        let away_view = PreparedView::new(
            &pair.evaluation(),
            FramePose::new(
                FramePosition::new(
                    frame,
                    LocalPosition::try_metres(DVec3::Z * (radius * 4.0)).unwrap(),
                ),
                UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_y(
                    std::f64::consts::PI,
                ))
                .unwrap(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let away = input(&away_view, frame, radius);
        cover
            .update(&mut cache, &identity, &away, &refined, 0, None)
            .unwrap();
        assert!(
            !cover.successor_ready(),
            "unshown obsolete successor must be discarded"
        );
        assert_eq!(
            cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
            source
        );
        assert_eq!(cover.transition().unwrap().1, 0.0);
        assert_eq!(
            cover.transition().unwrap().0.triangles()[0][0]
                .new
                .position_body_m,
            first_new
        );
        assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
        return;
    }
    cover
        .update_with_elapsed(
            &mut cache,
            &identity,
            &near,
            &refined,
            0,
            None,
            Duration::from_millis(150),
        )
        .unwrap();
    assert_ne!(
        cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
        source
    );
    let (successor, fraction) = cover
        .transition()
        .expect("successor starts only at promoted endpoint");
    assert_eq!(fraction, 0.0);
    let surface = cover.surface().unwrap();
    for vertex in successor.triangles().iter().flatten() {
        let reference = vertex.old_reference;
        let patch = surface
            .patches()
            .iter()
            .find(|p| p.address() == reference.address)
            .unwrap();
        let old = reference
            .indices
            .iter()
            .zip(reference.weights)
            .fold(DVec3::ZERO, |sum, (&i, w)| {
                sum + patch.samples()[usize::from(i)].position_body_m * w
            });
        assert!(
            (old - vertex.old.position_body_m).length() < 1e-8,
            "successor must capture the exact promoted surface, not a mid-morph source"
        );
    }
    assert!(cache.report().peak_aggregate_bytes <= TERRAIN_CPU_CAP_BYTES);
}

#[test]
fn alternating_views_keep_generation_and_transition_resources_bounded() {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let near_view = view(&pair, frame, radius, 20.0);
    let far_view = view(&pair, frame, radius, radius * 4.0);
    let near = input(&near_view, frame, radius);
    let far = input(&far_view, frame, radius);
    let settings = LodSettings::default().with_limits(32, 64, 2).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 128).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    cover
        .set_morph_duration(Duration::from_millis(150))
        .unwrap();
    for i in 0..300 {
        let input = if i % 2 == 0 { &near } else { &far };
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                input,
                &settings,
                32,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
        assert!(cache.pending() <= MAX_PENDING_PATCHES);
        let report = cache.report();
        assert!(report.resident_bytes + report.external_bytes <= TERRAIN_CPU_CAP_BYTES);
        if let Some((_, fraction)) = cover.transition() {
            assert!(fraction.is_finite() && (0.0..=1.0).contains(&fraction));
        }
    }
    for _ in 0..400 {
        cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &far,
                &settings,
                64,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
        if cover.transition().is_none() && cover.ready() {
            break;
        }
    }
    assert!(
        cover.transition().is_none(),
        "settled far view must retire transitions"
    );
    assert!(
        cache.report().pinned_patches <= cover.active().len() + 6,
        "obsolete transition pins must be released"
    );
    assert!(cache.pending() <= MAX_PENDING_PATCHES);
    let report = cache.report();
    assert!(report.resident_bytes + report.external_bytes <= TERRAIN_CPU_CAP_BYTES);
}

#[test]
fn zero_work_keeps_readiness_and_source_pins_until_replacement_is_complete() {
    let (world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let far_view = view(&pair, frame, radius, radius * 3.0);
    let far = input(&far_view, frame, radius);
    let roots = LodSettings::default().with_limits(256, 256, 0).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    cover
        .update(&mut cache, &identity, &far, &roots, 0, None)
        .unwrap();
    assert!(!cover.ready());
    assert!(cover.active().is_empty());
    for _ in 0..100 {
        cover
            .update(&mut cache, &identity, &far, &roots, 64, None)
            .unwrap();
        if cover.ready() {
            break;
        }
    }
    assert!(cover.ready());
    let source: Vec<_> = cover.active().iter().map(|p| p.address).collect();

    let refined = LodSettings::default().with_limits(256, 256, 2).unwrap();
    cover
        .update(&mut cache, &identity, &far, &refined, 0, None)
        .unwrap();
    assert!(cover.ready());
    assert_eq!(
        cover.active().iter().map(|p| p.address).collect::<Vec<_>>(),
        source
    );
    assert!(cache.report().pinned_patches >= source.len());
}

#[test]
fn replacing_geometry_identity_drops_active_morph_and_old_pins() {
    let (mut world, frames, identity, frame, radius) = fixture();
    let pair = frames.coherent_view(&world).unwrap();
    let far_view = view(&pair, frame, radius, radius * 3.0);
    let near_view = view(&pair, frame, radius, 20.0);
    let far = input(&far_view, frame, radius);
    let near = input(&near_view, frame, radius);
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 256).unwrap();
    let mut cover = AdaptiveTerrainCover::default();
    cover
        .set_morph_duration(Duration::from_millis(500))
        .unwrap();
    let roots = LodSettings::default().with_limits(256, 256, 0).unwrap();
    for _ in 0..100 {
        cover
            .update(&mut cache, &identity, &far, &roots, 64, None)
            .unwrap();
        if cover.ready() {
            break;
        }
    }
    assert!(cover.ready());
    let refined = LodSettings::default().with_limits(256, 256, 2).unwrap();
    for _ in 0..200 {
        cover
            .update(&mut cache, &identity, &near, &refined, 64, None)
            .unwrap();
        if cover.transition().is_some() {
            break;
        }
    }
    assert!(
        cover.transition().is_some(),
        "expected an active morph before identity replacement"
    );

    let body = identity.body;
    let changed = TerrainDefinition::new(
        TerrainIdentity(701),
        TerrainSeed(20),
        TerrainGeneratorVersion::V2,
        identity.definition.config().clone(),
    );
    world.edit_terrain(body, Some(changed)).unwrap();
    let state = world.body(body).unwrap();
    let replacement = TerrainGeometryIdentity::new(
        body,
        state.terrain().unwrap().clone(),
        state.terrain_revision(),
        radius,
    )
    .unwrap();
    let replacement_frames =
        CelestialFrameProjection::build(&world, NonZeroU64::new(73).unwrap()).unwrap();
    let replacement_frame = replacement_frames.frames_for(body).unwrap().body_fixed;
    let replacement_pair = replacement_frames.coherent_view(&world).unwrap();
    let replacement_view = view(&replacement_pair, replacement_frame, radius, radius * 3.0);
    let replacement_input = input(&replacement_view, replacement_frame, radius);
    cover
        .update(
            &mut cache,
            &replacement,
            &replacement_input,
            &roots,
            0,
            None,
        )
        .unwrap();
    assert!(cover.transition().is_none());
    assert!(
        cover.active().is_empty(),
        "obsolete coverage must not survive an identity change"
    );
    assert!(!cover.ready());
    assert!(
        cache.report().pinned_patches <= 6,
        "only replacement roots may remain pinned"
    );
}
