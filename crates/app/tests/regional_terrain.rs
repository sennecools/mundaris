use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use glam::DVec3;
use mundaris_app::regional_terrain::{
    RegionalConfig, RegionalError, RegionalTerrain, RegionalView,
};
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity, TileData};
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};

fn generator() -> SurfaceGenerator {
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x2c00_0001),
        TerrainSeed(0x2c00_0002),
        SurfaceAlgorithm::RockyV5,
    );
    SurfaceGenerator::new(&definition, 80_000.0).unwrap()
}

fn capture_generator() -> SurfaceGenerator {
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(5_931_033_225_171_238_913),
        TerrainSeed(0),
        SurfaceAlgorithm::RockyV5,
    );
    SurfaceGenerator::new(&definition, 80_000.0).unwrap()
}

fn identity() -> TileBuildIdentity {
    TileBuildIdentity {
        body_identity: 0x2c00_0003,
        surface_revision: 1,
        material_revision: 1,
    }
}

fn config(roots: Vec<CubePatchAddress>) -> RegionalConfig {
    RegionalConfig {
        roots,
        max_level: 3,
        cells: 4,
        cpu_tile_cap: 48,
        cpu_byte_cap: 2 * 1024 * 1024,
        worker_count: 1,
        worker_delay: Duration::ZERO,
        queue_cap: 2,
        completion_cap: 2,
        admission_cap_per_tick: 2,
        max_desired_patches: 40,
        upload_tile_cap: 2,
        upload_byte_cap: 64 * 1024,
        publication_cap_per_tick: 4,
        transition_cap: 4,
        split_threshold_px: 0.02,
        merge_threshold_px: 0.01,
        prediction_seconds: 0.5,
        high_speed_mps: 500.0,
    }
}

fn view(radius_m: f64, offset_m: f64, projection_scale_px: f64) -> RegionalView {
    RegionalView {
        body_position_m: DVec3::Z * (radius_m + offset_m),
        body_velocity_mps: DVec3::ZERO,
        projection_scale_px,
    }
}

fn build_tile(address: CubePatchAddress) -> Arc<TileData> {
    let tile = ResidentTileBuilder::build(&generator(), identity(), address, 4)
        .unwrap()
        .0;
    Arc::new(tile)
}

#[test]
fn projected_selection_is_deterministic_bounded_and_cross_face_balanced() {
    let roots = [
        CubePatchAddress::root(CubeFace::PositiveZ),
        CubePatchAddress::root(CubeFace::PositiveX),
    ];
    let mut first = RegionalTerrain::new(generator(), identity(), config(roots.to_vec())).unwrap();
    let mut second = RegionalTerrain::new(generator(), identity(), config(roots.to_vec())).unwrap();
    let observer = view(80_000.0, 220.0, 900.0);

    first.tick(observer, Duration::from_millis(16)).unwrap();
    second.tick(observer, Duration::from_millis(16)).unwrap();

    assert_eq!(first.desired(), second.desired());
    assert!(first.desired().len() > roots.len());
    assert!(first.desired().len() <= first.config().max_desired_patches);
    assert_cover_balanced(first.desired());
    assert!(first.snapshot().desired_projected_error_px > 0.0);
    assert!(first.snapshot().queue_pressure);
}

#[test]
fn exact_unchanged_view_reuses_only_a_stable_selector_cover_and_keeps_pressure() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cfg = config(vec![root]);
    cfg.max_level = 4;
    cfg.max_desired_patches = 8;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    let observer = view(80_000.0, 220.0, 30_000.0);

    let mut previous = Vec::new();
    for _ in 0..6 {
        regional.tick(observer, Duration::from_millis(16)).unwrap();
        let snapshot = regional.snapshot();
        if !previous.is_empty()
            && regional.desired() == previous
            && snapshot.selector_fixed_point_reused
        {
            assert_eq!(regional.desired(), previous.as_slice());
            assert!(snapshot.desired_capacity_pressure);
            return;
        }
        previous = regional.desired().to_vec();
    }
    panic!("stable unchanged view did not reach the exact selector fixed point");
}

#[test]
fn capture_a_to_b_approach_preserves_coarse_start_and_refines_the_near_region() {
    // The regional root matches the real resident parent used by the route.
    let root = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap();
    let mut cfg = config(vec![root]);
    cfg.max_level = 13;
    cfg.cells = 32;
    cfg.max_desired_patches = 256;
    cfg.split_threshold_px = 0.15;
    cfg.merge_threshold_px = 0.075;
    cfg.worker_delay = Duration::from_secs(1);
    cfg.queue_cap = 1;
    cfg.completion_cap = 1;
    cfg.admission_cap_per_tick = 1;
    let mut regional = RegionalTerrain::new(capture_generator(), identity(), cfg).unwrap();
    // Match candidate7's 640x402 logical viewport, not the capture window's
    // 960x540 outer size.
    let projection_scale = 402.0 / (2.0 * (30.0f64.to_radians()).tan());

    let position_a = DVec3::new(-23_479.875025493548, -51_608.0501829376, 61_023.83762970912);
    regional
        .tick(
            RegionalView {
                body_position_m: position_a,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: projection_scale,
            },
            Duration::from_millis(16),
        )
        .unwrap();
    let coarse_count = regional.desired().len();
    let coarse_pressure = regional.snapshot().desired_capacity_pressure;

    let position_b = DVec3::new(-22_511.66087109423, -49_644.05391521016, 58_730.2421964268);
    regional
        .tick(
            RegionalView {
                body_position_m: position_b,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: projection_scale,
            },
            Duration::from_millis(16),
        )
        .unwrap();
    let refined_count = regional.desired().len();
    let refined_pressure = regional.snapshot().desired_capacity_pressure;
    let refined_levels = level_counts(regional.desired());
    let refined_time_us = regional.snapshot().selection_time_micros;
    let position_c = DVec3::new(-22_660.98864990531, -49_644.05391521016, 58_672.78600028269);
    regional
        .tick(
            RegionalView {
                body_position_m: position_c,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: projection_scale,
            },
            Duration::from_millis(16),
        )
        .unwrap();
    let final_count = regional.desired().len();
    let final_levels = level_counts(regional.desired());
    let final_pressure = regional.snapshot().desired_capacity_pressure;
    let final_time_us = regional.snapshot().selection_time_micros;
    regional
        .tick(
            RegionalView {
                body_position_m: position_b,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: projection_scale,
            },
            Duration::from_millis(16),
        )
        .unwrap();
    let return_count = regional.desired().len();
    let return_time_us = regional.snapshot().selection_time_micros;
    let return_pressure = regional.snapshot().desired_capacity_pressure;
    eprintln!(
        "Slice 2C selector fixture: A={coarse_count} pressure={coarse_pressure}, B={refined_count} levels={refined_levels:?} selection={refined_time_us}us pressure={refined_pressure}, C={final_count} levels={final_levels:?} selection={final_time_us}us pressure={final_pressure}, return-B={return_count} selection={return_time_us}us pressure={return_pressure}"
    );
    assert_eq!(coarse_count, 1);
    assert!(refined_count > 1 && refined_count <= regional.config().max_desired_patches);
    assert!(final_count > 1 && final_count <= regional.config().max_desired_patches);
    assert!(
        !refined_pressure,
        "fixture must not hide selector behavior behind the patch cap"
    );
    assert!(
        refined_levels.len() > 1,
        "near route position should produce mixed LOD from projected error"
    );
    assert_eq!(refined_count, 133);
    assert!(!final_pressure);
    assert!(!return_pressure);
    assert_cover_balanced(regional.desired());
    regional.request_cancel_all();
    regional.drain_cancelled();
}

fn level_counts(addresses: &[CubePatchAddress]) -> BTreeMap<u8, usize> {
    let mut counts = BTreeMap::new();
    for address in addresses {
        *counts.entry(address.level()).or_insert(0) += 1;
    }
    counts
}

#[test]
fn seeded_root_stays_drawable_until_a_resident_balanced_split_is_acknowledged() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut regional = RegionalTerrain::new(generator(), identity(), config(vec![root])).unwrap();

    regional.seed_tile(build_tile(root)).unwrap();
    assert_eq!(regional.queued_uploads().len(), 1);
    regional.ack_resident(root);
    regional.ack_resident(root);
    assert_eq!(regional.snapshot().reuploads, 0);
    assert_eq!(regional.drawable(), &[root]);

    let children = root.children().unwrap();
    regional
        .tick(view(80_000.0, 100.0, 30_000.0), Duration::from_millis(16))
        .unwrap();
    assert!(regional.snapshot().refinement_debt > 0.0);
    for child in children {
        regional.seed_tile(build_tile(child)).unwrap();
        regional.ack_resident(child);
    }
    regional.drain_cancelled();

    let split = regional
        .publication_candidates()
        .into_iter()
        .find(|candidate| matches!(candidate, mundaris_app::regional_terrain::RegionalPublication::Split { parent, .. } if *parent == root))
        .expect("resident children allow a local split");
    assert_eq!(regional.drawable(), &[root]);
    regional.ack_drawable(split.cover()).unwrap();
    assert_eq!(regional.drawable().len(), 4);
    assert!(
        regional
            .drawable()
            .iter()
            .all(|address| children.contains(address))
    );
    assert_cover_balanced(regional.drawable());
    assert_eq!(
        regional.ack_drawable(&[children[0]]),
        Err(RegionalError::InvalidCover)
    );
}

#[test]
fn retreat_requeues_evicted_cached_merge_parents_and_coarsens_without_holes() {
    use mundaris_app::regional_terrain::RegionalPublication;

    let root = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap();
    let parent_a = CubePatchAddress::try_new(CubeFace::PositiveZ, 10, 314, 78).unwrap();
    let parent_b = CubePatchAddress::try_new(CubeFace::PositiveZ, 10, 314, 79).unwrap();
    let mut cfg = config(vec![root]);
    cfg.max_level = 13;
    cfg.upload_tile_cap = 1;
    cfg.upload_byte_cap = 64 * 1024;
    cfg.publication_cap_per_tick = 1;
    cfg.transition_cap = 8;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();

    let drawable = vec![
        CubePatchAddress::try_new(CubeFace::PositiveZ, 10, 315, 78).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 10, 315, 79).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 628, 156).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 629, 156).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 628, 157).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 629, 157).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 628, 158).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 629, 158).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 628, 159).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 11, 629, 159).unwrap(),
    ];
    for address in std::iter::once(root)
        .chain(drawable.iter().copied())
        .chain([parent_a, parent_b])
    {
        regional.seed_tile(build_tile(address)).unwrap();
        regional.ack_resident(address);
    }
    regional.ack_drawable(&drawable).unwrap();
    regional.mark_not_resident(parent_a);
    regional.mark_not_resident(parent_b);
    assert_cover_balanced(regional.drawable());

    let retreat = view(80_000.0, 5_000.0, 0.0);
    regional.tick(retreat, Duration::from_millis(16)).unwrap();
    assert_eq!(regional.desired(), &[root]);
    assert!(regional.publication_candidates().iter().all(|candidate| {
        !matches!(candidate, RegionalPublication::Merge { parent, .. } if *parent == parent_a || *parent == parent_b)
    }));
    assert_eq!(regional.queued_uploads().len(), 1);
    let first_parent = regional.queued_uploads()[0].address;
    assert!([parent_a, parent_b].contains(&first_parent));
    assert_eq!(regional.snapshot().requests_issued, 0);

    regional.ack_resident(first_parent);
    let first_merge = regional
        .publication_candidates()
        .into_iter()
        .find(|candidate| {
            matches!(candidate, RegionalPublication::Merge { parent, .. } if *parent == first_parent)
        })
        .expect("GPU-resident parent enables its local merge");
    regional.ack_drawable(first_merge.cover()).unwrap();
    assert!(regional.drawable().contains(&first_parent));
    assert_cover_balanced(regional.drawable());

    regional.tick(retreat, Duration::from_millis(16)).unwrap();
    assert_eq!(regional.queued_uploads().len(), 1);
    let second_parent = regional.queued_uploads()[0].address;
    assert_ne!(first_parent, second_parent);
    regional.ack_resident(second_parent);
    let second_merge = regional
        .publication_candidates()
        .into_iter()
        .find(|candidate| {
            matches!(candidate, RegionalPublication::Merge { parent, .. } if *parent == second_parent)
        })
        .expect("the next one-upload merge can proceed after the first publishes");
    regional.ack_drawable(second_merge.cover()).unwrap();
    assert_cover_balanced(regional.drawable());

    regional.tick(retreat, Duration::from_millis(16)).unwrap();
    let root_merge = regional
        .publication_candidates()
        .into_iter()
        .find(|candidate| {
            matches!(candidate, RegionalPublication::Merge { parent, .. } if *parent == root)
        })
        .expect("completed sibling coarsening exposes the root merge");
    regional.ack_drawable(root_merge.cover()).unwrap();
    assert_eq!(regional.drawable(), &[root]);
    assert_cover_balanced(regional.drawable());
}

#[test]
fn pinned_cpu_cache_pressure_stops_rebuilds_but_keeps_root_retreat_merge_ready() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let children = root.children().unwrap();
    let mut cfg = config(vec![root]);
    cfg.max_level = 2;
    cfg.cpu_tile_cap = 5;
    cfg.cpu_byte_cap = 2 * 1024 * 1024;
    cfg.max_desired_patches = 32;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    regional.seed_tile(build_tile(root)).unwrap();
    regional.ack_resident(root);
    for child in children {
        regional.seed_tile(build_tile(child)).unwrap();
        regional.ack_resident(child);
    }
    regional.ack_drawable(&children).unwrap();
    assert_eq!(regional.snapshot().cpu_cached_tiles, 5);

    let near = view(80_000.0, 220.0, 30_000.0);
    for _ in 0..6 {
        regional.tick(near, Duration::from_millis(16)).unwrap();
        let snapshot = regional.snapshot();
        assert!(snapshot.cpu_cache_pressure);
        assert!(snapshot.worker_queued + snapshot.worker_running == 0);
        assert_eq!(snapshot.requests_issued, 0);
    }
    assert!(
        regional
            .desired()
            .iter()
            .any(|address| address.level() == 2)
    );

    let retreat = view(80_000.0, 5_000.0, 0.0);
    regional.tick(retreat, Duration::from_millis(16)).unwrap();
    assert_eq!(regional.desired(), &[root]);
    let merge = regional
        .publication_candidates()
        .into_iter()
        .find(|candidate| {
            matches!(candidate, mundaris_app::regional_terrain::RegionalPublication::Merge { parent, .. } if *parent == root)
        })
        .expect("the already-resident root remains available for retreat");
    regional.ack_drawable(merge.cover()).unwrap();
    assert_eq!(regional.drawable(), &[root]);
}

#[test]
fn constrained_cache_reserves_one_complete_sibling_frontier_at_a_time() {
    let roots = [
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap(),
    ];
    let mut cfg = config(roots.to_vec());
    cfg.max_level = 2;
    cfg.max_desired_patches = 8;
    cfg.cpu_tile_cap = 6;
    cfg.cpu_byte_cap = 2 * 1024 * 1024;
    cfg.worker_count = 1;
    cfg.queue_cap = 8;
    cfg.completion_cap = 8;
    cfg.admission_cap_per_tick = 4;
    cfg.upload_tile_cap = 2;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    for root in roots {
        regional.seed_tile(build_tile(root)).unwrap();
        regional.ack_resident(root);
    }
    let observer = RegionalView {
        body_position_m: DVec3::Z * 80_100.0,
        body_velocity_mps: DVec3::ZERO,
        projection_scale_px: 30_000.0,
    };
    regional.tick(observer, Duration::from_millis(16)).unwrap();
    assert_eq!(regional.desired().len(), 8);
    let frontiers = regional.split_frontiers();
    assert_eq!(frontiers.len(), 2);
    assert!(frontiers.iter().all(|frontier| {
        frontier.parent_resident
            && frontier.ready_children <= 4
            && frontier.aggregate_priority > 0.0
            && frontier
                .children
                .iter()
                .all(|child| regional.desired().contains(child))
    }));
    for frontier in frontiers {
        let mut trial = regional.drawable().to_vec();
        trial.retain(|address| *address != frontier.parent);
        trial.extend(frontier.children);
        assert_cover_balanced(&trial);
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        regional.tick(observer, Duration::from_millis(16)).unwrap();
        for upload in regional.queued_uploads().to_vec() {
            regional.ack_resident(upload.address);
        }
        for candidate in regional.publication_candidates() {
            regional.ack_drawable(candidate.cover()).unwrap();
        }
        let split_group = roots.iter().find_map(|root| {
            let children = root.children().ok()?;
            children
                .iter()
                .all(|child| regional.drawable().contains(child))
                .then_some((root, children))
        });
        if let Some((split_root, children)) = split_group {
            assert_eq!(regional.drawable().len(), 5);
            let other_root = roots.iter().find(|root| *root != split_root).unwrap();
            assert!(regional.drawable().contains(other_root));
            assert!(
                children
                    .iter()
                    .all(|child| regional.drawable().contains(child))
            );
            let requests_after_complete_group = regional.snapshot().requests_issued;
            for _ in 0..16 {
                regional.tick(observer, Duration::from_millis(16)).unwrap();
                for upload in regional.queued_uploads().to_vec() {
                    regional.ack_resident(upload.address);
                }
                for candidate in regional.publication_candidates() {
                    regional.ack_drawable(candidate.cover()).unwrap();
                }
                thread::sleep(Duration::from_millis(1));
            }
            let snapshot = regional.snapshot();
            assert!(snapshot.cpu_cache_pressure);
            assert_eq!(snapshot.requests_issued, requests_after_complete_group);
            assert_eq!(regional.drawable().len(), 5);
            assert!(!regional.drawable().contains(split_root));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let snapshot = regional.snapshot();
    panic!(
        "no complete sibling frontier under bounded CPU cache: desired={} drawable={} cached={} requests={} pressure={} uploads={}",
        snapshot.desired_count,
        snapshot.drawable_count,
        snapshot.cpu_cached_tiles,
        snapshot.requests_issued,
        snapshot.cpu_cache_pressure,
        snapshot.upload_backlog_tiles,
    );
}

#[test]
fn cpu_eviction_invalidates_residency_and_backtrack_reuploads_the_exact_tile() {
    use mundaris_app::regional_terrain::RegionalPublication;

    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let children = root.children().unwrap();
    let target = children[0];
    let mut cfg = config(vec![root]);
    cfg.max_level = 2;
    cfg.max_desired_patches = 4;
    cfg.cpu_tile_cap = 5;
    cfg.upload_tile_cap = 4;
    cfg.admission_cap_per_tick = 4;
    cfg.worker_count = 1;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();

    regional.seed_tile(build_tile(root)).unwrap();
    regional.ack_resident(root);
    for child in children {
        regional.seed_tile(build_tile(child)).unwrap();
        regional.ack_resident(child);
    }
    let unrelated = children[1].children().unwrap()[0];
    regional.seed_tile(build_tile(unrelated)).unwrap();
    assert!(
        !regional
            .snapshot()
            .resident
            .contains(&format!("{target:?}"))
    );
    assert!(regional.tile(target).is_none());

    let observer = view(80_000.0, 100.0, 30_000.0);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        regional.tick(observer, Duration::from_millis(16)).unwrap();
        let restored = regional
            .queued_uploads()
            .iter()
            .any(|upload| upload.address == target);
        for upload in regional.queued_uploads().to_vec() {
            regional.ack_resident(upload.address);
        }
        if restored {
            assert!(regional.tile(target).is_some());
            assert!(
                regional
                    .snapshot()
                    .resident
                    .contains(&format!("{target:?}"))
            );
            assert!(regional.snapshot().reuploads > 0);
            assert!(regional.publication_candidates().iter().any(|candidate| {
                matches!(candidate, RegionalPublication::Split { parent, .. } if *parent == root)
            }));
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("the evicted resident tile was not rebuilt and re-uploaded");
}

#[test]
fn regional_error_means_are_normalized_by_the_configured_root_area() {
    let root = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap();
    let children = root.children().unwrap();
    let observer = RegionalView {
        body_position_m: DVec3::Z * 180_000.0,
        body_velocity_mps: DVec3::ZERO,
        projection_scale_px: 900.0,
    };
    let mut coarse_config = config(vec![root]);
    coarse_config.max_level = 13;
    let mut coarse = RegionalTerrain::new(generator(), identity(), coarse_config).unwrap();
    coarse.seed_tile(build_tile(root)).unwrap();
    coarse.ack_resident(root);
    for child in children {
        coarse.seed_tile(build_tile(child)).unwrap();
        coarse.ack_resident(child);
    }
    coarse.ack_drawable(&children).unwrap();
    coarse.tick(observer, Duration::from_millis(16)).unwrap();
    assert_eq!(coarse.desired(), &[root]);
    let coarse_snapshot = coarse.snapshot();
    let root_error = coarse_snapshot.desired[0].projected_error_px;
    assert!(root_error > 0.0);
    assert!((coarse_snapshot.desired_projected_error_px - root_error).abs() < 1.0e-12);

    let mut fine_config = config(children.to_vec());
    fine_config.max_level = 13;
    let mut fine = RegionalTerrain::new(generator(), identity(), fine_config).unwrap();
    fine.tick(observer, Duration::from_millis(16)).unwrap();
    let fine_snapshot = fine.snapshot();
    assert!(
        fine.desired() == children,
        "the high-altitude fine fixture should remain at the four configured roots"
    );
    let fine_min = fine_snapshot
        .desired
        .iter()
        .map(|patch| patch.projected_error_px)
        .fold(f64::INFINITY, f64::min);
    let fine_max = fine_snapshot
        .desired
        .iter()
        .map(|patch| patch.projected_error_px)
        .fold(0.0, f64::max);
    assert!(
        coarse_snapshot.drawable_projected_error_px >= fine_min
            && coarse_snapshot.drawable_projected_error_px <= fine_max,
        "the drawable error mean must remain in projected-pixel units"
    );
    assert!(
        (fine_snapshot.desired_projected_error_px - coarse_snapshot.drawable_projected_error_px)
            .abs()
            < 1.0e-12
    );
    assert!(coarse_snapshot.refinement_debt >= 0.0);
}

#[test]
fn desired_descendants_build_ancestors_and_converge_through_local_publications() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cfg = config(vec![root]);
    cfg.max_level = 3;
    cfg.max_desired_patches = 256;
    cfg.cpu_tile_cap = 256;
    cfg.cpu_byte_cap = 16 * 1024 * 1024;
    cfg.worker_count = 4;
    cfg.queue_cap = 64;
    cfg.completion_cap = 64;
    cfg.admission_cap_per_tick = 16;
    cfg.upload_tile_cap = 32;
    cfg.upload_byte_cap = 1024 * 1024;
    cfg.publication_cap_per_tick = 8;
    cfg.transition_cap = 8;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();

    regional.seed_tile(build_tile(root)).unwrap();
    regional.ack_resident(root);
    regional
        .tick(view(80_000.0, 220.0, 900.0), Duration::from_millis(16))
        .unwrap();
    assert!(
        regional
            .desired()
            .iter()
            .any(|address| address.level() == 3)
    );
    assert!(
        regional
            .snapshot()
            .build_requests
            .iter()
            .all(|request| request.address.contains("level: 1")),
        "the first requests must be the current drawable root's children"
    );

    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        regional
            .tick(view(80_000.0, 220.0, 900.0), Duration::from_millis(16))
            .unwrap();
        for upload in regional.queued_uploads().to_vec() {
            regional.ack_resident(upload.address);
        }
        for candidate in regional.publication_candidates() {
            regional.ack_drawable(candidate.cover()).unwrap();
        }
        if regional.drawable() == regional.desired() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(regional.drawable(), regional.desired());
    assert_cover_balanced(regional.drawable());
    assert!(regional.publication_candidates().is_empty());
}

#[test]
fn cached_tiles_are_reused_and_upload_admission_stays_within_count_and_bytes() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cfg = config(vec![root]);
    cfg.upload_tile_cap = 1;
    cfg.upload_byte_cap = 2_000;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    regional.seed_tile(build_tile(root)).unwrap();

    regional
        .tick(view(80_000.0, 5_000.0, 0.0), Duration::from_millis(16))
        .unwrap();
    assert_eq!(regional.snapshot().cache_hits, 1);
    let queued_snapshot = regional.snapshot();
    assert_eq!(queued_snapshot.desired.len(), 1);
    assert_eq!(queued_snapshot.desired[0].state, "queued_upload");
    assert_eq!(queued_snapshot.upload_queue.len(), 1);
    assert_eq!(
        queued_snapshot.upload_queue[0].bytes,
        regional.queued_uploads()[0].bytes
    );
    assert!(queued_snapshot.resident.is_empty());
    assert_eq!(queued_snapshot.estimated_completed_unpublished_bytes, 0);
    assert!(regional.queued_uploads().len() <= 1);
    assert!(
        regional
            .queued_uploads()
            .iter()
            .map(|upload| upload.bytes)
            .sum::<usize>()
            <= 2_000
    );

    regional.ack_resident(root);
    let resident_snapshot = regional.snapshot();
    assert_eq!(resident_snapshot.desired[0].state, "drawable");
    assert_eq!(resident_snapshot.resident, vec![format!("{root:?}")]);
    assert_eq!(resident_snapshot.drawable, vec![format!("{root:?}")]);
    regional.mark_not_resident(root);
    regional
        .tick(view(80_000.0, 5_000.0, 0.0), Duration::from_millis(16))
        .unwrap();
    assert_eq!(regional.queued_uploads().len(), 1);
    assert_eq!(regional.queued_uploads()[0].address, root);
}

#[test]
fn upload_allowlist_trims_queue_without_losing_cpu_keys_and_none_restores_admission() {
    let roots = [
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap(),
    ];
    let mut cfg = config(roots.to_vec());
    cfg.upload_tile_cap = 1;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    regional.seed_tile(build_tile(roots[0])).unwrap();
    regional.seed_tile(build_tile(roots[1])).unwrap();
    let cached_b = regional.tile(roots[1]).unwrap();
    assert_eq!(regional.queued_uploads()[0].address, roots[0]);

    regional.set_upload_allowlist(Some(&[roots[1]]));
    assert_eq!(regional.queued_uploads().len(), 1);
    assert_eq!(regional.queued_uploads()[0].address, roots[1]);
    assert!(Arc::ptr_eq(&cached_b, &regional.queued_uploads()[0].tile));
    regional.ack_resident(roots[1]);
    assert!(regional.queued_uploads().is_empty());

    regional.set_upload_allowlist(None);
    assert_eq!(regional.queued_uploads().len(), 1);
    assert_eq!(regional.queued_uploads()[0].address, roots[0]);
    assert_eq!(regional.snapshot().requests_issued, 0);
    assert!(regional.tile(roots[1]).is_some());
}

#[test]
fn cover_validation_is_pure_and_rejects_unbalanced_trial_cover() {
    let roots = [
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap(),
        CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap(),
    ];
    let mut cfg = config(roots.to_vec());
    cfg.max_level = 3;
    let regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    assert!(regional.snapshot().resident.is_empty());
    assert!(regional.cover_is_valid(&roots));

    let mut balanced_split = roots[0].children().unwrap().to_vec();
    balanced_split.push(roots[1]);
    assert!(regional.cover_is_valid(&balanced_split));

    let boundary_child = roots[0].children().unwrap()[1];
    balanced_split.retain(|address| *address != boundary_child);
    balanced_split.extend(boundary_child.children().unwrap());
    assert!(!regional.cover_is_valid(&balanced_split));
    assert!(!regional.cover_is_valid(&roots[..1]));
}

#[test]
fn rejected_cpu_build_reports_discarded_bytes_and_builder_time() {
    let root = CubePatchAddress::root(CubeFace::PositiveZ);
    let mut cfg = config(vec![root]);
    cfg.max_level = 1;
    cfg.cells = 32;
    cfg.cpu_tile_cap = 2;
    cfg.cpu_byte_cap = 4 * 1024 * 1024;
    cfg.worker_count = 1;
    cfg.worker_delay = Duration::from_millis(120);
    cfg.queue_cap = 1;
    cfg.completion_cap = 1;
    cfg.admission_cap_per_tick = 1;
    cfg.max_desired_patches = 8;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    let root_tile = ResidentTileBuilder::build(&generator(), identity(), root, 32)
        .unwrap()
        .0;
    regional.seed_tile(Arc::new(root_tile)).unwrap();
    regional.ack_resident(root);
    let observer = view(80_000.0, 220.0, 30_000.0);
    regional.tick(observer, Duration::from_millis(16)).unwrap();
    let children = root.children().unwrap();
    let requested_child = regional
        .snapshot()
        .build_requests
        .iter()
        .find_map(|request| {
            children
                .iter()
                .copied()
                .find(|child| request.address == format!("{child:?}"))
        })
        .expect("first frontier child build starts with one cache slot available");
    let filler = children
        .into_iter()
        .find(|child| *child != requested_child)
        .unwrap();
    let filler_tile = ResidentTileBuilder::build(&generator(), identity(), filler, 32)
        .unwrap()
        .0;
    regional.seed_tile(Arc::new(filler_tile)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        regional.tick(observer, Duration::from_millis(16)).unwrap();
        let snapshot = regional.snapshot();
        if snapshot.bytes_built_but_unused > 0 {
            assert!(snapshot.build_time_discarded_micros > 0);
            assert!(snapshot.build_time_micros >= snapshot.build_time_discarded_micros);
            regional.request_cancel_all();
            regional.drain_cancelled();
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    panic!("CPU cache rejection did not account for the completed build");
}

#[test]
fn priority_and_delayed_cancellation_keep_frame_calls_nonblocking() {
    let roots = [
        CubePatchAddress::root(CubeFace::PositiveZ),
        CubePatchAddress::root(CubeFace::PositiveX),
    ];
    let mut cfg = config(roots.to_vec());
    cfg.worker_delay = Duration::from_millis(120);
    cfg.queue_cap = 1;
    cfg.completion_cap = 1;
    cfg.admission_cap_per_tick = 1;
    cfg.max_desired_patches = 8;
    let mut regional = RegionalTerrain::new(generator(), identity(), cfg).unwrap();
    for root in roots {
        regional.seed_tile(build_tile(root)).unwrap();
        regional.ack_resident(root);
    }

    let started = Instant::now();
    regional
        .tick(view(80_000.0, 220.0, 900.0), Duration::from_millis(16))
        .unwrap();
    let frame_call = started.elapsed();
    assert!(
        frame_call < Duration::from_millis(80),
        "tick waited {frame_call:?}"
    );
    let initial_snapshot = regional.snapshot();
    assert_eq!(initial_snapshot.configuration.worker_count, 1);
    assert_eq!(initial_snapshot.configuration.worker_delay_millis, 120);
    let priorities = initial_snapshot.last_admitted_priorities;
    assert_eq!(priorities.len(), 1);
    assert!(priorities[0].address.contains("PositiveZ"));

    regional
        .tick(view(80_000.0, 220.0, 0.0), Duration::from_millis(16))
        .unwrap();
    assert!(
        regional.snapshot().cancelled_before_start + regional.snapshot().cancelled_during_work > 0
    );
    regional.request_cancel_all();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !regional.is_idle() && Instant::now() < deadline {
        regional.drain_cancelled();
        thread::sleep(Duration::from_millis(2));
    }
    regional.drain_cancelled();
    assert!(regional.is_idle());
    let snapshot = regional.snapshot();
    assert!(snapshot.recent_build_elapsed_micros.len() <= 256);
    assert!(snapshot.jobs_started <= snapshot.requests_issued);
    assert!(
        snapshot.worker_queued <= regional.config().queue_cap + regional.config().completion_cap
    );
}

fn assert_cover_balanced(cover: &[CubePatchAddress]) {
    for &patch in cover {
        for edge in PatchEdge::ALL {
            for &neighbor in cover {
                if patch == neighbor || !touches_edge(patch, edge, neighbor) {
                    continue;
                }
                assert!(
                    patch.level().abs_diff(neighbor.level()) <= 1,
                    "{patch:?} vs {neighbor:?}"
                );
            }
        }
    }
}

fn touches_edge(patch: CubePatchAddress, edge: PatchEdge, neighbor: CubePatchAddress) -> bool {
    let relation = patch.neighbor(edge);
    if neighbor.level() >= patch.level() {
        let mut ancestor = neighbor;
        while ancestor.level() > patch.level() {
            ancestor = match ancestor.parent() {
                Some(parent) => parent,
                None => return false,
            };
        }
        if ancestor != relation.address {
            return false;
        }
        let depth = neighbor.level() - patch.level();
        let [x, y] = neighbor.coordinates();
        let side = 1u32 << depth;
        match relation.edge {
            PatchEdge::UMin => x == 0,
            PatchEdge::UMax => x + 1 == side,
            PatchEdge::VMin => y == 0,
            PatchEdge::VMax => y + 1 == side,
        }
    } else {
        let mut ancestor = patch;
        while ancestor.level() > neighbor.level() {
            ancestor = match ancestor.parent() {
                Some(parent) => parent,
                None => return false,
            };
        }
        let depth = patch.level() - neighbor.level();
        let [x, y] = patch.coordinates();
        let side = 1u32 << depth;
        let on_edge = match edge {
            PatchEdge::UMin => x % side == 0,
            PatchEdge::UMax => x % side + 1 == side,
            PatchEdge::VMin => y % side == 0,
            PatchEdge::VMax => y % side + 1 == side,
        };
        on_edge && ancestor.neighbor(edge).address == neighbor
    }
}
