use mundaris_app::planet_terrain::*;
use mundaris_math::{surface::*, *};
use mundaris_renderer::planet_surface::GRID_SAMPLES;
use mundaris_world::{terrain::*, *};
use std::num::NonZeroU64;

const RADIUS: f64 = 10_000.0;

fn definition(identity: u64, seed: u64, amplitude: f64) -> TerrainDefinition {
    let band = TerrainBandConfig::new(
        amplitude,
        TerrainScale::Metres {
            longest_wavelength_m: 128.0,
        },
        4,
    )
    .unwrap();
    let controls = TerrainControls::new(0.0, 1.0, 0.5, 1.0, 0.1, 0.1).unwrap();
    TerrainDefinition::new(
        TerrainIdentity(identity),
        TerrainSeed(seed),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new([band; 5], controls).unwrap(),
    )
}

fn identity(seed: u64, amplitude: f64) -> TerrainGeometryIdentity {
    TerrainGeometryIdentity::new(
        body_id(),
        definition(7, seed, amplitude),
        TerrainRevision::default(),
        RADIUS,
    )
    .unwrap()
}

fn body_id() -> BodyId {
    let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    world
        .insert_body(
            "cache-key",
            BodyProperties::new(1e20, RADIUS).unwrap(),
            state,
        )
        .unwrap()
}

fn cache(max: usize) -> TerrainPatchCache {
    TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, max).unwrap()
}

fn complete(
    cache: &mut TerrainPatchCache,
    key: &TerrainGeometryIdentity,
    address: CubePatchAddress,
) {
    assert!(cache.request(key, address));
    let mut total = 0;
    while cache.peek(key, address).is_none() {
        let report = cache.generate(37, GENERATION_MICROBATCH, None).unwrap();
        total += report.vertices_generated;
        assert!(
            report.vertices_generated > 0 || report.patches_completed > 0,
            "generation stalled"
        );
    }
    assert_eq!(total, GRID_SAMPLES);
    let report = cache.report();
    assert!(report.resident_bytes <= TERRAIN_CPU_CAP_BYTES);
    assert!(report.peak_bytes <= TERRAIN_CPU_CAP_BYTES);
}

fn assert_uniform_complete_cover(cover: &TerrainReadyCover, level: u8) {
    assert!(cover.ready());
    assert_eq!(cover.active().len(), 6 * 4usize.pow(u32::from(level)));
    let total_area: u128 = cover
        .active()
        .iter()
        .map(|patch| 1u128 << (2 * (30 - patch.address.level())))
        .sum();
    assert_eq!(total_area, 6u128 << 60);
    assert!(
        cover
            .active()
            .iter()
            .all(|patch| patch.address.level() == level)
    );
}

fn assert_within_budget(cache: &TerrainPatchCache, cover: &TerrainReadyCover) {
    let report = cache.report();
    assert!(report.resident_patches <= MAX_TERRAIN_PATCHES);
    assert!(report.resident_bytes + cover.bookkeeping_bytes() <= TERRAIN_CPU_CAP_BYTES);
    assert!(report.peak_bytes <= TERRAIN_CPU_CAP_BYTES);
}

#[test]
fn ready_cover_keeps_complete_roots_until_all_target_children_are_ready() {
    let key = identity(10, 8.0);
    let mut cache = cache(30);
    let mut cover = TerrainReadyCover::default();
    let root_work = cover
        .update(&mut cache, &key, 0, 6 * GRID_SAMPLES, None)
        .unwrap();
    assert_eq!(root_work.patches_completed, 6);
    assert_uniform_complete_cover(&cover, 0);
    let old_cover: Vec<_> = cover.active().iter().map(|patch| patch.address).collect();
    assert_within_budget(&cache, &cover);

    let no_work = cover.update(&mut cache, &key, 1, 0, None).unwrap();
    assert_eq!(no_work.vertices_generated, 0);
    assert_eq!(
        cover
            .active()
            .iter()
            .map(|patch| patch.address)
            .collect::<Vec<_>>(),
        old_cover
    );
    assert_uniform_complete_cover(&cover, 0);

    let mut replacement_published = false;
    for _ in 0..300 {
        let work = cover.update(&mut cache, &key, 1, 32, None).unwrap();
        assert_within_budget(&cache, &cover);
        if cover
            .active()
            .first()
            .is_some_and(|patch| patch.address.level() == 1)
        {
            assert_eq!(work.patches_completed, 1);
            replacement_published = true;
            break;
        }
        assert_eq!(
            cover
                .active()
                .iter()
                .map(|patch| patch.address)
                .collect::<Vec<_>>(),
            old_cover
        );
        assert_uniform_complete_cover(&cover, 0);
        assert!(cache.report().resident_patches >= 6);
        for face in CubeFace::ALL {
            assert!(
                cache.peek(&key, CubePatchAddress::root(face)).is_some(),
                "old covering root was evicted"
            );
        }
    }
    assert!(
        replacement_published,
        "all 24 target children should eventually become ready"
    );
    assert_uniform_complete_cover(&cover, 1);
    for face in CubeFace::ALL {
        assert!(
            cache.peek(&key, CubePatchAddress::root(face)).is_some(),
            "pinned old cover was evicted"
        );
    }
}

#[test]
fn settled_level_one_cover_generates_no_samples_for_one_thousand_updates() {
    let key = identity(11, 8.0);
    let mut cache = cache(30);
    let mut cover = TerrainReadyCover::default();
    cover
        .update(&mut cache, &key, 1, 8 * GRID_SAMPLES, None)
        .unwrap();
    for _ in 0..300 {
        cover.update(&mut cache, &key, 1, 32, None).unwrap();
        if cover
            .active()
            .first()
            .is_some_and(|patch| patch.address.level() == 1)
        {
            break;
        }
    }
    assert_uniform_complete_cover(&cover, 1);
    let before = cache.report().hits;
    for _ in 0..1000 {
        let report = cover.update(&mut cache, &key, 1, 32, None).unwrap();
        assert_eq!(report.vertices_generated, 0);
        assert_eq!(report.patches_completed, 0);
        assert_within_budget(&cache, &cover);
    }
    assert_eq!(cache.report().hits, before);
}

#[test]
fn revision_change_clears_ready_cover_until_new_root_coverage_is_generated() {
    let old = identity(12, 8.0);
    let new = TerrainGeometryIdentity::new(
        old.body,
        definition(7, 13, 8.0),
        TerrainRevision::default(),
        RADIUS,
    )
    .unwrap();
    let mut cache = cache(12);
    let mut cover = TerrainReadyCover::default();
    cover
        .update(&mut cache, &old, 0, 6 * GRID_SAMPLES, None)
        .unwrap();
    assert_uniform_complete_cover(&cover, 0);
    let report = cover.update(&mut cache, &new, 0, 0, None).unwrap();
    assert_eq!(report.vertices_generated, 0);
    assert!(!cover.ready());
    assert!(
        cover.active().is_empty(),
        "stale terrain cannot remain a drawable owner"
    );
    assert_eq!(cache.pending(), 6);
    for face in CubeFace::ALL {
        assert!(cache.peek(&new, CubePatchAddress::root(face)).is_none());
    }
}

#[test]
fn complete_profile_parent_child_common_directions_are_identical() {
    let definition = definition(7, 14, 8.0);
    let generator = TerrainGenerator::new(&definition, RADIUS).unwrap();
    let parent = CubePatchAddress::root(CubeFace::PositiveX);
    let child = CubePatchAddress::try_new(CubeFace::PositiveX, 1, 0, 0).unwrap();
    let mut shared = 0;
    for i in 0..=8 {
        for j in 0..=8 {
            let key = parent.sample_key(i, j, 16).unwrap();
            let (child_i, child_j) = (i * 2, j * 2);
            let child_key = child.sample_key(child_i, child_j, 16).unwrap();
            if key.direction() == child_key.direction() {
                let parent_location = SurfaceLocation::new(key.direction());
                let child_location = SurfaceLocation::new(child_key.direction());
                let mut output = [TerrainSample::default(); 2];
                generator
                    .evaluate_batch(
                        &[parent_location, child_location],
                        TerrainFootprint::COMPLETE,
                        &mut output,
                    )
                    .unwrap();
                assert_eq!(output[0], output[1]);
                shared += 1;
            }
        }
    }
    assert!(shared > 0);
}

#[test]
fn cold_request_resumes_hits_and_eviction_regenerates_deterministically() {
    let key = identity(1, 8.0);
    let a = CubePatchAddress::root(CubeFace::PositiveX);
    let b = CubePatchAddress::root(CubeFace::NegativeX);
    let mut cache = cache(1);
    assert!(cache.request(&key, a));
    let first = cache.generate(23, GENERATION_MICROBATCH, None).unwrap();
    assert_eq!(first.vertices_generated, 23);
    assert_eq!(cache.pending(), 1);
    let resumed = cache
        .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
        .unwrap();
    assert_eq!(resumed.patches_completed, 1);
    let expected = cache.peek(&key, a).unwrap().samples().to_vec();
    assert!(cache.get(&key, a).unwrap().is_some());
    assert!(cache.request(&key, b));
    complete(&mut cache, &key, b);
    assert_eq!(cache.report().evictions, 1);
    complete(&mut cache, &key, a);
    assert_eq!(cache.peek(&key, a).unwrap().samples(), expected);
    assert_eq!(cache.report().resident_patches, 1);
}

#[test]
fn get_refreshes_lru_and_pins_preserve_ready_entries_under_pressure() {
    let key = identity(2, 8.0);
    let a = CubePatchAddress::root(CubeFace::PositiveX);
    let b = CubePatchAddress::root(CubeFace::NegativeX);
    let c = CubePatchAddress::root(CubeFace::PositiveY);
    let mut cache = cache(2);
    complete(&mut cache, &key, a);
    complete(&mut cache, &key, b);
    assert!(cache.get(&key, a).unwrap().is_some());
    complete(&mut cache, &key, c);
    assert!(cache.peek(&key, a).is_some());
    assert!(cache.peek(&key, c).is_some());
    assert!(cache.peek(&key, b).is_none());
    assert!(cache.pin(&key, a));
    assert!(cache.pin(&key, c));
    assert!(cache.request(&key, b));
    let blocked = cache
        .generate(GRID_SAMPLES, GENERATION_MICROBATCH, None)
        .unwrap();
    assert_eq!(blocked.vertices_generated, 0);
    assert!(cache.peek(&key, a).is_some() && cache.peek(&key, c).is_some());
    assert_eq!(cache.pending(), 1);
}

#[test]
fn geometry_identity_separates_definition_seed_radius_and_address_profile() {
    let a = identity(3, 8.0);
    let different_seed = identity(4, 8.0);
    let different_definition = identity(3, 12.0);
    let different_radius =
        TerrainGeometryIdentity::new(a.body, a.definition.clone(), a.revision, RADIUS + 1.0)
            .unwrap();
    let root = CubePatchAddress::root(CubeFace::PositiveX);
    let child = CubePatchAddress::try_new(CubeFace::PositiveX, 1, 0, 0).unwrap();
    let mut cache = cache(8);
    for key in [
        &a,
        &different_seed,
        &different_definition,
        &different_radius,
    ] {
        complete(&mut cache, key, root);
    }
    for key in [
        &a,
        &different_seed,
        &different_definition,
        &different_radius,
    ] {
        assert!(cache.get(key, root).unwrap().is_some());
    }
    complete(&mut cache, &a, child);
    assert!(cache.get(&a, child).unwrap().is_some());
    assert_eq!(cache.report().resident_patches, 5);
}

#[test]
fn canonical_root_geometry_matches_on_all_face_edges_and_corners() {
    let key = identity(5, 8.0);
    let mut cache = cache(6);
    for face in CubeFace::ALL {
        complete(&mut cache, &key, CubePatchAddress::root(face));
    }
    let mut boundaries = Vec::new();
    for face in CubeFace::ALL {
        for t in 0..=16 {
            for (i, j) in [(0, t), (16, t), (t, 0), (t, 16)] {
                let address = CubePatchAddress::root(face);
                let direction = address.sample_key(i, j, 16).unwrap().direction();
                let sample =
                    cache.peek(&key, address).unwrap().samples()[j as usize * 17 + i as usize];
                boundaries.push((direction, sample));
            }
        }
    }
    for (index, (direction, sample)) in boundaries.iter().enumerate() {
        for (other_direction, other_sample) in boundaries.iter().skip(index + 1) {
            if direction == other_direction {
                assert_eq!(
                    sample, other_sample,
                    "canonical shared boundary {direction:?}"
                );
            }
        }
    }
}

#[test]
fn terrain_revision_invalidation_cancels_partial_work_before_publication() {
    let mut world = CelestialSystem::new(NonZeroU64::new(91).unwrap(), SimulationInstant::ZERO);
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    let body = world
        .insert_body("terrain", BodyProperties::new(1e20, RADIUS).unwrap(), state)
        .unwrap();
    let old = definition(9, 1, 8.0);
    world.edit_terrain(body, Some(old.clone())).unwrap();
    let first = world.body(body).unwrap();
    let old_key = TerrainGeometryIdentity::new(
        body,
        first.terrain().unwrap().clone(),
        first.terrain_revision(),
        RADIUS,
    )
    .unwrap();
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cache = cache(2);
    assert!(cache.request(&old_key, address));
    assert_eq!(
        cache
            .generate(11, GENERATION_MICROBATCH, None)
            .unwrap()
            .vertices_generated,
        11
    );
    world
        .edit_terrain(body, Some(definition(9, 2, 8.0)))
        .unwrap();
    let current = world.body(body).unwrap();
    let new_key = TerrainGeometryIdentity::new(
        body,
        current.terrain().unwrap().clone(),
        current.terrain_revision(),
        RADIUS,
    )
    .unwrap();
    cache.invalidate_body(&new_key);
    assert_eq!(cache.pending(), 0);
    assert!(cache.peek(&old_key, address).is_none());
    assert!(cache.request(&new_key, address));
    complete(&mut cache, &new_key, address);
    assert!(cache.peek(&old_key, address).is_none());
}

#[test]
fn erosion_version_and_configuration_separate_cached_geometry_and_regenerate() {
    let old = identity(5, 8.0);
    let eroded = |octaves| {
        TerrainGeometryIdentity::new(
            old.body,
            TerrainDefinition::new(
                old.definition.identity(),
                old.definition.seed(),
                TerrainGeneratorVersion::V2,
                old.definition
                    .config()
                    .clone()
                    .with_erosion(ErosionConfig::new(octaves, 1.0).unwrap()),
            ),
            old.revision,
            old.radius_m,
        )
        .unwrap()
    };
    let a = eroded(1);
    let b = eroded(3);
    let address = CubePatchAddress::try_new(CubeFace::PositiveX, 18, 10, 10).unwrap();
    let mut cache = cache(3);
    for key in [&old, &a, &b] {
        complete(&mut cache, key, address);
    }
    let expected = cache.peek(&b, address).unwrap().samples().to_vec();
    assert_ne!(cache.peek(&old, address).unwrap().samples(), expected);
    assert_ne!(cache.peek(&a, address).unwrap().samples(), expected);
    cache.invalidate_body(&b);
    assert!(cache.peek(&old, address).is_none());
    assert!(cache.peek(&a, address).is_none());
    let mut small = self::cache(1);
    complete(&mut small, &b, address);
    complete(&mut small, &b, CubePatchAddress::root(CubeFace::NegativeX));
    complete(&mut small, &b, address);
    assert_eq!(small.peek(&b, address).unwrap().samples(), expected);
}

#[test]
fn eroded_height_gradient_and_normal_match_all_canonical_boundary_owners() {
    let old = definition(7, 14, 8.0);
    let definition = TerrainDefinition::new(
        old.identity(),
        old.seed(),
        TerrainGeneratorVersion::V2,
        old.config().clone(),
    );
    let generator = TerrainGenerator::new(&definition, RADIUS).unwrap();
    let mut samples = Vec::new();
    // Face edges/corners and same-face child edges use one complete profile.
    for face in CubeFace::ALL {
        let root = CubePatchAddress::root(face);
        for address in [
            root,
            CubePatchAddress::try_new(face, 1, 0, 0).unwrap(),
            CubePatchAddress::try_new(face, 1, 1, 0).unwrap(),
        ] {
            for t in 0..=16 {
                for (i, j) in [(0, t), (16, t), (t, 0), (t, 16)] {
                    let location =
                        SurfaceLocation::new(address.sample_key(i, j, 16).unwrap().direction());
                    let sample = generator
                        .evaluate_point(TerrainQuery {
                            location,
                            footprint: TerrainFootprint::COMPLETE,
                        })
                        .unwrap();
                    samples.push((
                        location,
                        sample,
                        sample.normal_body(location, RADIUS).unwrap(),
                    ));
                }
            }
        }
    }
    let mut shared = 0;
    for (index, (location, sample, normal)) in samples.iter().enumerate() {
        for (other, other_sample, other_normal) in samples.iter().skip(index + 1) {
            if location == other {
                assert_eq!(sample, other_sample);
                assert_eq!(normal, other_normal);
                shared += 1;
            }
        }
    }
    assert!(shared > 100);
}
