use std::collections::BTreeSet;
use std::time::Duration;

use glam::{DMat3, DQuat, DVec3};
use mundaris_app::regional_terrain::{RegionalConfig, RegionalTerrain, RegionalView};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::CelestialProjection;
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};

fn generator() -> SurfaceGenerator {
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x2d00_0001),
        TerrainSeed(0x2d00_0002),
        SurfaceAlgorithm::RockyV5,
    );
    SurfaceGenerator::new(&definition, 80_000.0).unwrap()
}

fn six_face_config() -> RegionalConfig {
    RegionalConfig {
        roots: CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect(),
        max_level: 24,
        cells: 4,
        cpu_tile_cap: 1536,
        cpu_byte_cap: 64 * 1024 * 1024,
        worker_delay: Duration::from_secs(30),
        queue_cap: 16,
        completion_cap: 16,
        admission_cap_per_tick: 8,
        max_desired_patches: 512,
        ..Default::default()
    }
}

fn projection() -> CelestialProjection {
    CelestialProjection::try_new(800, 600, 70.0f64.to_radians(), 0.1).unwrap()
}

fn view(position: DVec3, projection_scale_px: f64) -> RegionalView {
    RegionalView {
        body_position_m: position,
        body_velocity_mps: DVec3::ZERO,
        projection_scale_px,
    }
}

#[test]
fn six_face_planetary_selector_culls_demand_and_moves_balanced_lod_incrementally() {
    let mut terrain = RegionalTerrain::new(generator(), identity(), six_face_config()).unwrap();
    let projection = projection();
    let radius = 80_000.0;
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();

    let mut max_selection_micros = 0;
    for _ in 0..60 {
        terrain
            .tick(
                view(DVec3::Z * (radius + 250.0), projection.focal_pixels()),
                Duration::from_millis(16),
            )
            .unwrap();
        let snapshot = terrain.snapshot();
        max_selection_micros = max_selection_micros.max(snapshot.selection_time_micros);
        assert!(snapshot.planetary_topology_operations <= 8);
        assert!(terrain.desired().len() <= terrain.config().max_desired_patches);
        assert!(terrain.cover_is_valid(terrain.desired()));
    }

    let back_root = CubePatchAddress::root(CubeFace::NegativeZ);
    assert!(!terrain.patch_visible(back_root));
    let snapshot = terrain.snapshot();
    assert!(
        snapshot
            .desired
            .iter()
            .find(|patch| patch.address == format!("{back_root:?}"))
            .is_some_and(|patch| patch.projected_error_px == 0.0)
    );
    assert!(terrain.desired().iter().any(|patch| patch.level() > 0));
    assert!(terrain.desired().iter().any(|patch| patch.level() == 0));
    assert_eq!(snapshot.configuration.max_level, 24);
    let old_positive_z_detail = terrain
        .desired()
        .iter()
        .filter(|patch| patch.face() == CubeFace::PositiveZ && patch.level() > 0)
        .count();
    eprintln!(
        "planetary selector: desired={} max selection={}us quality_reached={} cap_pressure={}",
        snapshot.desired_count,
        max_selection_micros,
        snapshot.target_quality_reached,
        snapshot.desired_capacity_pressure,
    );

    terrain
        .set_planetary_view(
            DMat3::from_quat(DQuat::from_rotation_y(-std::f64::consts::FRAC_PI_2)),
            projection,
        )
        .unwrap();
    for _ in 0..20 {
        terrain
            .tick(
                view(DVec3::X * (radius + 250.0), projection.focal_pixels()),
                Duration::from_millis(16),
            )
            .unwrap();
        assert!(terrain.cover_is_valid(terrain.desired()));
        assert!(terrain.snapshot().planetary_topology_operations <= 8);
    }
    assert!(terrain.patch_visible(CubePatchAddress::root(CubeFace::PositiveX)));
    assert!(!terrain.patch_visible(CubePatchAddress::root(CubeFace::NegativeX)));
    assert!(
        terrain
            .desired()
            .iter()
            .any(|patch| patch.face() == CubeFace::PositiveX && patch.level() > 0)
    );
    let new_positive_z_detail = terrain
        .desired()
        .iter()
        .filter(|patch| patch.face() == CubeFace::PositiveZ && patch.level() > 0)
        .count();
    assert!(new_positive_z_detail < old_positive_z_detail);
}

#[test]
fn capacity_limited_quality_is_reported_and_stable_views_reuse_selection() {
    let mut config = six_face_config();
    config.max_desired_patches = 12;
    let mut terrain = RegionalTerrain::new(generator(), identity(), config).unwrap();
    let projection = projection();
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    let observer = view(DVec3::Z * 80_250.0, projection.focal_pixels());

    for _ in 0..8 {
        terrain.tick(observer, Duration::ZERO).unwrap();
        if terrain.snapshot().desired_capacity_pressure {
            break;
        }
    }
    let limited = terrain.snapshot();
    assert!(limited.desired_capacity_pressure);
    assert!(!limited.target_quality_reached);
    assert!(limited.desired_count <= limited.max_desired_patches);
    assert!(terrain.cover_is_valid(terrain.desired()));

    terrain.tick(observer, Duration::ZERO).unwrap();
    let stable = terrain.snapshot();
    assert!(stable.selector_fixed_point_reused);
    assert!(!stable.target_quality_reached);
    assert!(stable.desired_capacity_pressure);
}

#[test]
fn six_root_cover_is_warm_across_camera_motion_without_new_tile_requests() {
    let mut config = six_face_config();
    config.worker_delay = Duration::from_secs(30);
    let mut terrain = RegionalTerrain::new(generator(), identity(), config).unwrap();
    let projection = projection();
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    let far = 80_000.0 * 1_000_000.0;
    for _ in 0..3 {
        terrain
            .tick(
                view(DVec3::Z * far, projection.focal_pixels()),
                Duration::ZERO,
            )
            .unwrap();
    }
    assert_eq!(terrain.desired().len(), 6);
    let admitted = terrain.snapshot().requests_issued;
    assert!(admitted > 0);

    terrain
        .set_planetary_view(
            DMat3::from_quat(DQuat::from_rotation_y(-std::f64::consts::FRAC_PI_2)),
            projection,
        )
        .unwrap();
    for _ in 0..12 {
        terrain
            .tick(
                view(DVec3::X * far, projection.focal_pixels()),
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(terrain.desired().len(), 6);
    }
    assert_eq!(terrain.snapshot().requests_issued, admitted);
    assert_eq!(
        terrain
            .desired()
            .iter()
            .map(|address| address.face())
            .collect::<BTreeSet<_>>()
            .len(),
        6
    );
}

fn identity() -> mundaris_app::resident_terrain::TileBuildIdentity {
    mundaris_app::resident_terrain::TileBuildIdentity {
        body_identity: 0x2d00_0003,
        surface_revision: 1,
        material_revision: 1,
    }
}

#[test]
fn stationary_roundoff_keeps_a_converged_planetary_selection_fixed() {
    let mut terrain = RegionalTerrain::new(generator(), identity(), six_face_config()).unwrap();
    let projection = projection();
    let observer = view(DVec3::Z * 1_080_000.0, projection.focal_pixels());
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    for _ in 0..1200 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        if terrain.snapshot().selector_fixed_point_reused {
            break;
        }
    }
    assert!(terrain.snapshot().target_quality_reached);
    let stable = terrain.desired().to_vec();
    let requests = terrain.snapshot().requests_issued;
    for iteration in 0..120 {
        let angle = if iteration % 2 == 0 {
            1.0e-16
        } else {
            -1.0e-16
        };
        terrain
            .set_planetary_view(DMat3::from_rotation_x(angle), projection)
            .unwrap();
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        assert_eq!(terrain.desired(), stable);
        assert!(terrain.snapshot().selector_fixed_point_reused);
        assert!(terrain.snapshot().target_quality_reached);
    }
    assert_eq!(terrain.snapshot().requests_issued, requests);
}

#[test]
fn planetary_settling_waits_for_the_complete_merge_scan() {
    let mut terrain = RegionalTerrain::new(generator(), identity(), six_face_config()).unwrap();
    let projection = projection();
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    for _ in 0..80 {
        terrain
            .tick(
                view(DVec3::Z * 80_250.0, projection.focal_pixels()),
                Duration::from_millis(16),
            )
            .unwrap();
    }
    let observer = view(DVec3::Z * 1_080_000.0, projection.focal_pixels());
    let mut settled = false;
    for _ in 0..2400 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        assert!(terrain.cover_is_valid(terrain.desired()));
        if terrain.snapshot().target_quality_reached {
            settled = true;
            break;
        }
    }
    assert!(settled);
    let stable = terrain.desired().to_vec();
    for _ in 0..120 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        assert_eq!(terrain.desired(), stable);
        assert!(terrain.snapshot().target_quality_reached);
    }
}

#[test]
fn culled_children_cannot_oscillate_through_a_visible_coarse_parent() {
    let mut config = six_face_config();
    config.cells = 32;
    config.max_desired_patches = 2048;
    let world = mundaris_app::solar_system::SolarSystemPreset::gameplay()
        .create(std::num::NonZeroU64::new(0x2d015).unwrap())
        .unwrap();
    let (_, moon) = world
        .bodies()
        .find(|(_, state)| state.name() == "Moon")
        .unwrap();
    let radius = moon.properties().reference_radius_m();
    let generator = SurfaceGenerator::new(moon.surface_definition().unwrap(), radius).unwrap();
    let surface_radius = generator
        .evaluate_point(mundaris_math::surface::SurfaceLocation::new(
            mundaris_math::Direction3::try_new(DVec3::Z).unwrap(),
        ))
        .unwrap()
        .radius_m();
    let mut terrain = RegionalTerrain::new(generator, identity(), config).unwrap();
    let projection = CelestialProjection::try_new(660, 726, 60.0f64.to_radians(), 5000.0).unwrap();
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    let observer = view(
        DVec3::Z * (surface_radius + 500_000.0),
        projection.focal_pixels(),
    );
    let mut fixed = false;
    for _ in 0..1200 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        if terrain.snapshot().selector_fixed_point_reused {
            fixed = true;
            break;
        }
    }
    assert!(fixed, "stationary frustum demand never converged");
    assert!(terrain.snapshot().target_quality_reached);
    let stable = terrain.desired().to_vec();
    for _ in 0..120 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        assert_eq!(terrain.desired(), stable);
        assert!(terrain.snapshot().selector_fixed_point_reused);
    }
}

#[test]
fn large_window_moon_selection_converges_without_hiding_capacity_pressure() {
    let mut config = six_face_config();
    config.cells = 32;
    config.max_desired_patches = 16384;
    config.cpu_tile_cap = 49152;
    config.cpu_byte_cap = 2048 * 1024 * 1024;
    let world = mundaris_app::solar_system::SolarSystemPreset::gameplay()
        .create(std::num::NonZeroU64::new(0x2d015).unwrap())
        .unwrap();
    let (_, moon) = world
        .bodies()
        .find(|(_, state)| state.name() == "Moon")
        .unwrap();
    let generator = SurfaceGenerator::new(
        moon.surface_definition().unwrap(),
        moon.properties().reference_radius_m(),
    )
    .unwrap();
    let surface_radius = generator
        .evaluate_point(mundaris_math::surface::SurfaceLocation::new(
            mundaris_math::Direction3::try_new(DVec3::Z).unwrap(),
        ))
        .unwrap()
        .radius_m();
    let mut terrain = RegionalTerrain::new(generator, identity(), config).unwrap();
    let projection = CelestialProjection::try_new(1938, 1334, 60.0f64.to_radians(), 250.0).unwrap();
    terrain
        .set_planetary_view(DMat3::IDENTITY, projection)
        .unwrap();
    let observer = view(
        DVec3::Z * (surface_radius + 25_000.0),
        projection.focal_pixels(),
    );
    let mut converged = false;
    for iteration in 0..5000 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        if iteration % 8 != 0 && iteration != 4999 {
            continue;
        }
        let progress = terrain.snapshot();
        if progress.target_quality_reached {
            converged = true;
            break;
        }
        if progress.desired_capacity_pressure {
            break;
        }
    }
    let snapshot = terrain.snapshot();
    assert!(
        converged,
        "large Moon selection stopped at {} leaves: pressure={}, budget_exhausted={}",
        snapshot.desired_count,
        snapshot.desired_capacity_pressure,
        snapshot.selection_budget_exhausted
    );
    assert!(terrain.cover_is_valid(terrain.desired()));
    assert!(!snapshot.desired_capacity_pressure);
    assert!(snapshot.desired_count > 2048);
    let fixed = terrain.desired().to_vec();
    for _ in 0..120 {
        terrain.tick(observer, Duration::from_millis(16)).unwrap();
        assert_eq!(terrain.desired(), fixed);
        assert!(terrain.snapshot().target_quality_reached);
    }
    for clearance in [5000.0, 1000.0, 25.0] {
        let projection = CelestialProjection::try_new(
            1938,
            1334,
            60.0f64.to_radians(),
            (clearance * 0.01_f64).max(0.1),
        )
        .unwrap();
        terrain
            .set_planetary_view(DMat3::IDENTITY, projection)
            .unwrap();
        let observer = view(
            DVec3::Z * (surface_radius + clearance),
            projection.focal_pixels(),
        );
        let mut converged = false;
        for iteration in 0..6000 {
            terrain.tick(observer, Duration::from_millis(16)).unwrap();
            if iteration % 8 != 0 && iteration != 5999 {
                continue;
            }
            let progress = terrain.snapshot();
            if iteration > 0 && iteration % 1000 == 0 {
                eprintln!(
                    "Moon {clearance}m tick {}: leaves={}, selection={}us, operations={}, exhausted={}",
                    iteration + 1,
                    progress.desired_count,
                    progress.selection_time_micros,
                    progress.planetary_topology_operations,
                    progress.selection_budget_exhausted
                );
            }
            if progress.target_quality_reached {
                converged = true;
                break;
            }
            if progress.desired_capacity_pressure {
                break;
            }
        }
        let snapshot = terrain.snapshot();
        eprintln!(
            "Moon clearance {clearance}m: {} desired leaves, target={}, pressure={}, exhausted={}, operations={}, scanned={}, max_error={}",
            snapshot.desired_count,
            snapshot.target_quality_reached,
            snapshot.desired_capacity_pressure,
            snapshot.selection_budget_exhausted,
            snapshot.planetary_topology_operations,
            snapshot.planetary_split_candidates_scanned,
            snapshot
                .desired
                .iter()
                .map(|patch| patch.projected_error_px)
                .fold(0.0_f64, f64::max)
        );
        assert!(
            converged,
            "Moon clearance {clearance}m failed to converge at {} leaves",
            snapshot.desired_count
        );
        assert!(terrain.cover_is_valid(terrain.desired()));
        let stable = terrain.desired().to_vec();
        for _ in 0..120 {
            terrain.tick(observer, Duration::from_millis(16)).unwrap();
            assert_eq!(terrain.desired(), stable);
            assert!(terrain.snapshot().target_quality_reached);
        }
    }
}
