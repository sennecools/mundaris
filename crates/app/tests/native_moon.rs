use mundaris_app::planet_terrain::{
    NativeTerrainDefinition, TERRAIN_CPU_CAP_BYTES, TerrainGeometryIdentity, TerrainPatchCache,
};
use mundaris_math::{surface::*, *};
use mundaris_renderer::planet_surface::GRID_SAMPLES;
use mundaris_world::{
    BodyId, BodyProperties, BodyState, CelestialSystem, SimulationInstant,
    terrain::{
        SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
    },
};
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

const MOON_RADIUS_M: f64 = 109_081.776_8;

fn body_state() -> BodyState {
    BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}

fn moon_world() -> (CelestialSystem, BodyId) {
    let mut world = CelestialSystem::new(NonZeroU64::new(91_514).unwrap(), SimulationInstant::ZERO);
    let moon = world
        .insert_body(
            "Moon",
            BodyProperties::new(7.35e22, MOON_RADIUS_M).unwrap(),
            body_state(),
        )
        .unwrap();
    (world, moon)
}

fn rocky_surface(seed: u64) -> SurfaceDefinition {
    SurfaceDefinition::generated(
        TerrainIdentity(0x4d4f_4f4e_0001),
        TerrainSeed(seed),
        SurfaceAlgorithm::RockyV5,
    )
}

fn finish_roots(
    cache: &mut TerrainPatchCache,
    identity: &TerrainGeometryIdentity,
) -> Vec<CubePatchAddress> {
    let roots: Vec<_> = CubeFace::ALL
        .into_iter()
        .map(CubePatchAddress::root)
        .collect();
    for &address in &roots {
        assert!(cache.request(identity, address));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if roots
            .iter()
            .all(|&address| cache.peek(identity, address).is_some())
        {
            return roots;
        }
        assert!(Instant::now() < deadline, "root-patch generation timed out");
        cache.generate(roots.len() * GRID_SAMPLES, 8, None).unwrap();
        if cache.worker_count() != 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn assert_matches_complete_oracle(
    cache: &TerrainPatchCache,
    identity: &TerrainGeometryIdentity,
    roots: &[CubePatchAddress],
    oracle: &SurfaceGenerator,
) {
    let global_envelope = oracle.conservative_radius_envelope_m();
    for &address in roots {
        let patch = cache.peek(identity, address).unwrap();
        let extent = patch.extent();
        assert!(MOON_RADIUS_M + extent.min_height_m <= global_envelope[0]);
        assert!(MOON_RADIUS_M + extent.max_height_m >= global_envelope[1]);

        for (index, actual) in patch.samples().iter().enumerate() {
            let location = SurfaceLocation::new(
                address
                    .sample_direction(index as u32 % 17, index as u32 / 17, 16)
                    .unwrap(),
            );
            let expected = oracle.evaluate_point(location).unwrap();
            let expected_position = expected.position(location);
            let radius = actual.position_body_m.length();
            assert_eq!(
                radius.to_bits(),
                expected_position.length().to_bits(),
                "radius mismatch at {address:?}, sample {index}"
            );
            assert_eq!(
                actual.position_body_m.to_array().map(f64::to_bits),
                expected_position.to_array().map(f64::to_bits),
                "position mismatch at {address:?}, sample {index}"
            );
            assert!(actual.normal_body.is_finite());
            assert!((actual.normal_body.length() - 1.0).abs() < 1e-12);
            assert!(actual.normal_body.dot(location.direction().unit()) > 0.0);
            assert!(radius >= global_envelope[0] && radius <= global_envelope[1]);
            assert!(radius >= MOON_RADIUS_M + extent.min_height_m);
            assert!(radius <= MOON_RADIUS_M + extent.max_height_m);
        }

        // A root patch is a coarse representation; its certificate must retain
        // a material error bound instead of implying settled/converged geometry.
        assert!(patch.error().filtered_interpolation_m > 1.0);
        assert!(patch.error().total_m().unwrap() > 1.0);
    }
}

#[test]
fn rocky_v5_moon_roots_match_complete_serial_and_worker_queries() {
    let (mut world, moon) = moon_world();
    let definition = rocky_surface(71);
    world
        .edit_surface_definition(moon, Some(definition.clone()))
        .unwrap();
    let body = world.body(moon).unwrap();
    let identity = TerrainGeometryIdentity::from_body(moon, body).unwrap();
    assert!(matches!(
        &identity.definition,
        NativeTerrainDefinition::Surface(_)
    ));
    let oracle = SurfaceGenerator::new(&definition, MOON_RADIUS_M).unwrap();

    let mut serial = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 16).unwrap();
    let serial_roots = finish_roots(&mut serial, &identity);
    assert_matches_complete_oracle(&serial, &identity, &serial_roots, &oracle);

    let mut worker = TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 6, 1).unwrap();
    assert_eq!(worker.worker_count(), 1);
    let worker_roots = finish_roots(&mut worker, &identity);
    assert_eq!(worker_roots, serial_roots);
    assert_matches_complete_oracle(&worker, &identity, &worker_roots, &oracle);
    for &address in &serial_roots {
        assert_eq!(
            serial.peek(&identity, address).unwrap().samples(),
            worker.peek(&identity, address).unwrap().samples(),
            "serial/worker patch differs at {address:?}"
        );
    }

    // Coarse shading must resolve the mesh footprint, rather than folding all
    // of the complete field's metre-scale slopes into an orbital vertex.
    let mut complete_slope_energy = 0.0;
    let mut represented_slope_energy = 0.0;
    let mut shared = Vec::<(glam::DVec3, glam::DVec3)>::new();
    for &address in &serial_roots {
        let patch = serial.peek(&identity, address).unwrap();
        for (index, sample) in patch.samples().iter().enumerate() {
            let n = address
                .sample_key(index as u32 % 17, index as u32 / 17, 16)
                .unwrap()
                .direction();
            let complete = oracle.evaluate_point(SurfaceLocation::new(n)).unwrap();
            complete_slope_energy += (complete.normal() - n.unit()).length_squared();
            represented_slope_energy += (sample.normal_body - n.unit()).length_squared();
            if index % 17 == 0 || index % 17 == 16 || !(17..272).contains(&index) {
                if let Some((_, normal)) =
                    shared.iter().find(|(direction, _)| *direction == n.unit())
                {
                    assert_eq!(*normal, sample.normal_body, "same-footprint face seam");
                } else {
                    shared.push((n.unit(), sample.normal_body));
                }
            }
        }
    }
    assert!(
        represented_slope_energy < complete_slope_energy * 0.5,
        "orbital normals retain unresolved fine slopes: represented={represented_slope_energy} complete={complete_slope_energy}"
    );

    // Refinement changes shading resolution, while canonical boundary positions
    // retain the same complete query. Actual stitching must introduce no height
    // correction when coarse and fine normals differ.
    use mundaris_renderer::planet_surface::{
        StitchedSurface, SurfaceTopology, active_surface_cover,
    };
    let refined = CubePatchAddress::root(CubeFace::PositiveZ);
    let children = refined.children().unwrap();
    for address in children {
        assert!(serial.request(&identity, address));
    }
    while children
        .iter()
        .any(|address| serial.peek(&identity, *address).is_none())
    {
        serial.generate(4 * GRID_SAMPLES, 32, None).unwrap();
    }
    let addresses: Vec<_> = serial_roots
        .iter()
        .copied()
        .filter(|address| *address != refined)
        .chain(children)
        .collect();
    let topology = SurfaceTopology::new();
    let active = active_surface_cover(&addresses, &topology).unwrap();
    let raw: Vec<_> = active
        .iter()
        .map(|patch| serial.peek(&identity, patch.address).unwrap())
        .collect();
    let stitched = StitchedSurface::build(&active, &raw, &topology).unwrap();
    for (drawn, original) in stitched.patches().iter().zip(raw) {
        for (actual, expected) in drawn.samples().iter().zip(original.samples()) {
            assert_eq!(
                actual.position_body_m, expected.position_body_m,
                "mixed-LOD stitch changed complete geometry"
            );
        }
        assert!(drawn.error().boundary_constraint_m < 1e-9);
    }
}

#[test]
fn cache_identity_switches_surface_seed_revision_and_keeps_legacy_dispatch() {
    let (mut world, moon) = moon_world();
    let legacy_definition =
        mundaris_app::planet_terrain::checkpoint_terrain_definition(MOON_RADIUS_M).unwrap();
    world.edit_terrain(moon, Some(legacy_definition)).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 6).unwrap();
    let root = CubePatchAddress::root(CubeFace::PositiveZ);

    let legacy = TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    assert!(matches!(
        &legacy.definition,
        NativeTerrainDefinition::Legacy(_)
    ));
    assert!(cache.request(&legacy, root));
    while cache.peek(&legacy, root).is_none() {
        cache.generate(GRID_SAMPLES, 8, None).unwrap();
    }
    let legacy_samples = cache.peek(&legacy, root).unwrap().samples().to_vec();

    let first_definition = rocky_surface(71);
    world
        .edit_surface_definition(moon, Some(first_definition))
        .unwrap();
    let first = TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    cache.invalidate_body(&first);
    assert!(cache.peek(&legacy, root).is_none());
    let roots = finish_roots(&mut cache, &first);
    let first_samples = cache.peek(&first, root).unwrap().samples().to_vec();
    assert_ne!(first_samples, legacy_samples);
    assert_matches_complete_oracle(
        &cache,
        &first,
        &roots,
        &SurfaceGenerator::new(&rocky_surface(71), MOON_RADIUS_M).unwrap(),
    );

    let second_definition = rocky_surface(72);
    world
        .edit_surface_definition(moon, Some(second_definition.clone()))
        .unwrap();
    let second = TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    assert_ne!(first.revision, second.revision);
    cache.invalidate_body(&second);
    assert!(cache.peek(&first, root).is_none());
    let second_roots = finish_roots(&mut cache, &second);
    let second_samples = cache.peek(&second, root).unwrap().samples().to_vec();
    assert_ne!(first_samples, second_samples);
    assert_matches_complete_oracle(
        &cache,
        &second,
        &second_roots,
        &SurfaceGenerator::new(&second_definition, MOON_RADIUS_M).unwrap(),
    );

    world
        .edit_terrain(
            moon,
            Some(
                mundaris_app::planet_terrain::checkpoint_terrain_definition(MOON_RADIUS_M).unwrap(),
            ),
        )
        .unwrap();
    let restored_legacy =
        TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    cache.invalidate_body(&restored_legacy);
    assert!(matches!(
        &restored_legacy.definition,
        NativeTerrainDefinition::Legacy(_)
    ));
    assert!(cache.peek(&second, root).is_none());
    assert!(cache.request(&restored_legacy, root));
    while cache.peek(&restored_legacy, root).is_none() {
        cache.generate(GRID_SAMPLES, 8, None).unwrap();
    }
    assert!(
        !cache
            .peek(&restored_legacy, root)
            .unwrap()
            .samples()
            .is_empty()
    );
}

#[test]
fn compositional_mesh_demand_stops_at_screen_resolution_without_claiming_certification() {
    use mundaris_app::planet_terrain::AdaptiveTerrainCover;
    use mundaris_renderer::{planet_surface::*, *};
    use mundaris_world::CelestialFrameProjection;
    let (mut world, moon) = moon_world();
    world
        .edit_surface_definition(moon, Some(rocky_surface(71)))
        .unwrap();
    let identity = TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    let oracle = SurfaceGenerator::new(&rocky_surface(71), MOON_RADIUS_M).unwrap();
    let surface_radius = oracle
        .evaluate_point(SurfaceLocation::new(
            Direction3::try_new(glam::DVec3::Z).unwrap(),
        ))
        .unwrap()
        .radius_m();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(91_514).unwrap()).unwrap();
    let pair = frames.coherent_view(&world).unwrap();
    let fixed = frames.frames_for(moon).unwrap().body_fixed;
    let mut levels = Vec::new();
    for clearance in [327_278.0, 1000.0] {
        let view = PreparedView::new(
            &pair.evaluation(),
            FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(glam::DVec3::Z * (surface_radius + clearance))
                        .unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: fixed,
            reference_radius_m: MOON_RADIUS_M,
            projection: CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap(),
        };
        let mut cover = AdaptiveTerrainCover::default();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 64).unwrap();
        cover
            .update(
                &mut cache,
                &identity,
                &input,
                &LodSettings::default(),
                0,
                None,
            )
            .unwrap();
        let diagnostic = cover.convergence;
        let level = diagnostic.desired_local_lod.unwrap();
        assert!(
            level < 20,
            "constant certificate drove excessive resolution: {level}"
        );
        assert!(!diagnostic.target_certifiable);
        assert_eq!(diagnostic.certificate_limited_target_lod, None);
        assert!(diagnostic.desired_total_pixels > LodSettings::default().split_pixels());
        assert!(cover.report.quality_pending);
        assert!(!cover.report.settled);
        levels.push(level);
    }
    assert!(
        levels[1] > levels[0],
        "approach must still demand finer geometry"
    );
}

#[test]
fn compositional_moon_close_far_coarsens_and_releases_fine_pins() {
    use mundaris_app::planet_terrain::AdaptiveTerrainCover;
    use mundaris_renderer::{planet_surface::*, *};
    use mundaris_world::CelestialFrameProjection;

    let (mut world, moon) = moon_world();
    world
        .edit_surface_definition(moon, Some(rocky_surface(71)))
        .unwrap();
    let identity = TerrainGeometryIdentity::from_body(moon, world.body(moon).unwrap()).unwrap();
    let oracle = SurfaceGenerator::new(&rocky_surface(71), MOON_RADIUS_M).unwrap();
    let surface_radius = oracle
        .evaluate_point(SurfaceLocation::new(
            Direction3::try_new(glam::DVec3::Z).unwrap(),
        ))
        .unwrap()
        .radius_m();
    let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(91_514).unwrap()).unwrap();
    let pair = frames.coherent_view(&world).unwrap();
    let evaluation = pair.evaluation();
    let fixed = frames.frames_for(moon).unwrap().body_fixed;
    let prepare_view = |height_m| {
        PreparedView::new(
            &evaluation,
            FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(glam::DVec3::Z * height_m).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap()
    };
    let near_view = prepare_view(surface_radius + 1000.0);
    let far_view = prepare_view(MOON_RADIUS_M * 1000.0);
    let projection = CelestialProjection::try_new(640, 480, 60.0_f64.to_radians(), 0.1).unwrap();
    let near = SurfaceViewInput {
        view: &near_view,
        body_fixed_frame: fixed,
        reference_radius_m: MOON_RADIUS_M,
        projection,
    };
    let far = SurfaceViewInput {
        view: &far_view,
        body_fixed_frame: fixed,
        reference_radius_m: MOON_RADIUS_M,
        projection,
    };
    let settings = LodSettings::default().with_limits(24, 24, 1).unwrap();
    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 64).unwrap();
    assert_eq!(
        cache.worker_count(),
        0,
        "the regression uses serial generation"
    );
    let mut cover = AdaptiveTerrainCover::default();
    cover.set_morph_duration(Duration::ZERO).unwrap();

    for _ in 0..120 {
        let work = cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &near,
                &settings,
                4096,
                None,
                Duration::ZERO,
            )
            .unwrap();
        assert!(work.vertices_generated <= 4096);
        if cover.ready()
            && cover.active().len() == 24
            && cover
                .active()
                .iter()
                .all(|patch| patch.address.level() == 1)
        {
            break;
        }
    }
    assert!(cover.ready());
    assert_eq!(
        cover.active().len(),
        24,
        "near observer must publish level 1"
    );
    assert!(
        cover
            .active()
            .iter()
            .all(|patch| patch.address.level() == 1)
    );
    assert!(
        cover.transition().is_none(),
        "zero morph duration publishes statically"
    );
    let fine_pins = cache.report().pinned_patches;

    let mut largest_merge_batch = 0;
    for _ in 0..120 {
        let work = cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &far,
                &settings,
                4096,
                None,
                Duration::ZERO,
            )
            .unwrap();
        largest_merge_batch = largest_merge_batch.max(cover.report.merges);
        assert!(work.vertices_generated <= 4096);
        if cover.ready()
            && cover.active().len() == 6
            && cover
                .active()
                .iter()
                .all(|patch| patch.address.level() == 0)
        {
            break;
        }
    }
    assert!(cover.ready());
    assert!(
        largest_merge_batch > 1,
        "zoom-out must batch eligible merges"
    );
    assert_eq!(cover.active().len(), 6, "far observer must publish roots");
    assert!(
        cover
            .active()
            .iter()
            .all(|patch| patch.address.level() == 0)
    );
    assert!(cover.transition().is_none());
    // Publication retires the old cover; the next update refreshes cache pins
    // from that published endpoint rather than the previous frame's source.
    cover
        .update_with_elapsed(
            &mut cache,
            &identity,
            &far,
            &settings,
            4096,
            None,
            Duration::ZERO,
        )
        .unwrap();
    assert!(
        cache.report().pinned_patches < fine_pins,
        "coarsening must release pins on the fine cover"
    );
    assert!(cache.report().pinned_patches <= cover.active().len() + 6);
    let report = cache.report();
    assert!(
        report.resident_bytes + cover.resident_bytes() + report.external_bytes
            <= TERRAIN_CPU_CAP_BYTES
    );

    for _ in 0..10 {
        let work = cover
            .update_with_elapsed(
                &mut cache,
                &identity,
                &far,
                &settings,
                4096,
                None,
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(
            work.vertices_generated, 0,
            "settled far cover does no generation"
        );
    }
}
