use std::{num::NonZeroU64, time::Duration};

use mundaris_app::{
    developer_capture::{DeveloperScene, fixture_pose},
    developer_snapshot::*,
    motion_session::{AnalyticSession, MotionSnapshot},
    planet_surface::PlanetSurfaceSession,
    planet_terrain::MAX_TERRAIN_PATCHES,
    solar_system::{SOLAR_SYSTEM_CONTENT, SolarSystemPreset},
    terrain_population::TerrainPopulation,
};
use mundaris_renderer::*;
use mundaris_world::{CelestialFrameProjection, CelestialSystem};

struct Fixture {
    world: CelestialSystem,
    frames: CelestialFrameProjection,
    terrain: TerrainPopulation,
    scene: DeveloperScene,
    projection: CelestialProjection,
    motion: MotionSnapshot,
}
impl Fixture {
    fn new(scene: DeveloperScene) -> Self {
        let namespace = NonZeroU64::new(51299).unwrap();
        let (mut world, definition) = SolarSystemPreset::gameplay()
            .create_analytic(namespace)
            .unwrap();
        let mut session = AnalyticSession::new(&mut world, definition).unwrap();
        session.set_paused(true);
        let frames = CelestialFrameProjection::build(&world, namespace).unwrap();
        // This test is not a timed publication fixture.
        let motion = session.snapshot(&world);
        Self {
            world,
            frames,
            terrain: TerrainPopulation::new().unwrap(),
            scene,
            projection: CelestialProjection::try_new(960, 640, 60_f64.to_radians(), 0.1).unwrap(),
            motion,
        }
    }
    fn snapshot(&self) -> DeveloperSnapshot {
        let pair = self.frames.coherent_view(&self.world).unwrap();
        let pose = fixture_pose(self.scene, &pair, self.projection).unwrap();
        let id = self
            .scene
            .target_body_index()
            .map(|index| self.world.bodies().nth(index).unwrap().0);
        let clearance = id.and_then(|id| {
            mundaris_app::terrain_inspection::terrain_clearance(&pair, pose, id).unwrap()
        });
        DeveloperSnapshot::collect(SnapshotInput {
            pair: &pair,
            pose,
            camera_mode: self.scene.camera_mode(),
            selected_body: id,
            focused_body: id,
            reference_body: id,
            frame_number: 42,
            paused: self.motion.paused,
            simulation_speed: self.motion.rate,
            projection: self.projection,
            terrain: &self.terrain,
            terrain_clearance_m: clearance.map(|c| c.clearance_m),
            drawn_mesh_clearance_m: None,
            rendering: RenderingSnapshot {
                terrain_render_mode: "natural".into(),
                terrain_enabled: true,
                ..Default::default()
            },
            performance: PerformanceSnapshot::default(),
            navigation: None,
            motion: Some(self.motion.clone()),
        })
        .unwrap()
    }
    fn admit(&mut self) {
        let pair = self.frames.coherent_view(&self.world).unwrap();
        let pose = fixture_pose(self.scene, &pair, self.projection).unwrap();
        let view = PreparedView::new(
            &pair.evaluation(),
            pose,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let requests = self
            .world
            .bodies()
            .enumerate()
            .map(|(i, (id, b))| CelestialRenderBody {
                body_fixed_frame: self.frames.frames_for(id).unwrap().body_fixed,
                reference_radius_m: b.properties().reference_radius_m(),
                color: SOLAR_SYSTEM_CONTENT[i].color,
                selected: false,
                unlit: i == 0,
            })
            .collect::<Vec<_>>();
        let mut sessions = self
            .world
            .bodies()
            .filter(|(_, b)| b.terrain().is_some())
            .map(|(id, _)| PlanetSurfaceSession::new(id, MAX_TERRAIN_PATCHES).unwrap())
            .collect::<Vec<_>>();
        self.terrain
            .update(
                &pair,
                &view,
                self.projection,
                &requests,
                &mut sessions,
                &mut vec![false; requests.len()],
                &Icosphere::new(),
                true,
                Duration::from_millis(150),
                64,
                None,
                Duration::from_millis(16),
            )
            .unwrap();
    }
}

#[test]
fn developer_snapshot_serializes_stable_names_units_and_null_measurements() {
    let snapshot = Fixture::new(DeveloperScene::EarthClose).snapshot();
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["schema_version"], 4);
    assert_eq!(json["general"]["camera_mode"], "surface_inspection");
    assert_eq!(json["camera"]["reference_frame"], "body_fixed");
    assert!(json["performance"]["gpu_terrain_ms"].is_null());
    assert!(json["terrain"]["settled"].is_null());
    assert!(json["camera"]["navigation"].is_null());
    assert_eq!(json["memory"]["cap_bytes"], 128 * 1024 * 1024);
    assert_eq!(
        serde_json::from_value::<DeveloperSnapshot>(json).unwrap(),
        snapshot
    );
    let mut schema_one = serde_json::to_value(&snapshot).unwrap();
    schema_one["schema_version"] = serde_json::json!(1);
    schema_one["camera"]
        .as_object_mut()
        .unwrap()
        .remove("navigation");
    let compatible: DeveloperSnapshot = serde_json::from_value(schema_one).unwrap();
    assert_eq!(compatible.schema_version, 1);
    assert!(compatible.camera.navigation.is_none());
    let rendering = RenderingSnapshot::default().with_draw_report(CelestialPreparationReport {
        planetary_ocean_draws: 1,
        planetary_cloud_draws: 0,
        planetary_atmosphere_draws: 1,
        ..Default::default()
    });
    assert!(rendering.ocean_drawn && rendering.atmosphere_drawn && !rendering.clouds_drawn);
    assert!(
        !rendering.ocean_enabled,
        "actual draw flags and developer enable settings remain distinct"
    );
    let performance = PerformanceSnapshot::default().with_gpu(
        GpuProfile {
            terrain: Some(Duration::from_micros(125)),
            transition_fallback: Some(Duration::from_micros(50)),
            ..Default::default()
        },
        "latest_completed",
    );
    assert_eq!(performance.gpu_terrain_ms, Some(0.125));
    assert_eq!(performance.gpu_transition_fallback_ms, Some(0.05));
    assert_eq!(performance.gpu_timing_scope, "latest_completed");
}

#[test]
fn navigation_diagnostics_are_optional_and_round_trip_as_snapshot_data() {
    let mut snapshot = Fixture::new(DeveloperScene::EarthClose).snapshot();
    let diagnostics = mundaris_app::celestial_camera::NavigationDiagnostics {
        safeguard_minimum_clearance_m: Some(1.0),
        last_wheel_notches: None,
        window_focused: None,
        viewport_keyboard_owned: None,
        viewport_gesture_owned: None,
        requested_distance_m: None,
        attachment_policy: "body_fixed".into(),
        transitioning: false,
        base_speed_m_s: 12.0,
        base_source: "reference_radius".into(),
        user_multiplier: 2.0,
        boost_multiplier: 3.0,
        effective_speed_m_s: 72.0,
        requested_clearance_m: Some(10.0),
        zoom_target_meaning: "clearance".into(),
        pending_forward_m: 4.0,
        safeguard: "terrain_clearance".into(),
        local_radians_per_logical_pixel: 0.01,
        orbit_radians_per_logical_pixel: 0.02,
        wheel_log_per_notch: 0.1,
        logical_viewport_height: 640.0,
        terrain_query_count: 0,
        terrain_query_us: 0.0,
    };
    snapshot.camera.navigation = Some(diagnostics.clone());
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["camera"]["navigation"]["effective_speed_m_s"], 72.0);
    let decoded: DeveloperSnapshot = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.camera.navigation, Some(diagnostics));
    assert_eq!(decoded, snapshot);
}

#[test]
fn compact_units_are_signed_readable_and_handle_missing_values() {
    assert_eq!(format_distance(Some(12_400.0), false), "12.4 km");
    assert_eq!(format_distance(Some(11_800.0), true), "+11.8 km");
    assert_eq!(format_distance(Some(-2.0), true), "-2.00 m");
    assert_eq!(format_distance(Some(999.0), false), "999 m");
    assert_eq!(format_distance(Some(1_000_000.0), false), "1.00 Mm");
    assert_eq!(format_distance(Some(149_597_870_700.0), false), "1.00 AU");
    for value in [None, Some(f64::NAN), Some(f64::INFINITY)] {
        assert_eq!(format_distance(value, false), "Unavailable");
    }
    assert_eq!(format_milliseconds(Some(12.789)), "12.79 ms");
    assert_eq!(format_milliseconds(None), "Unavailable");
    assert_eq!(format_bytes(128 * 1024 * 1024), "128.0 MiB");
}

#[test]
fn snapshot_matches_current_coherent_engine_reports_not_idle_inference() {
    let mut fixture = Fixture::new(DeveloperScene::EarthOrbit);
    fixture.admit();
    fixture.terrain.cover.report.quality_pending = true;
    fixture.terrain.cover.report.settled = false;
    let snapshot = fixture.snapshot();
    let cache = fixture.terrain.cache.report();
    assert_eq!(snapshot.general.frame_number, 42);
    assert_eq!(
        snapshot.general.simulation_time_s,
        fixture.world.sample_time().seconds_since_epoch()
    );
    assert_eq!(snapshot.general.world_revision, fixture.world.revision());
    assert_eq!(
        snapshot.general.selected_body.as_ref().unwrap().name,
        "Earth"
    );
    assert_eq!(snapshot.terrain.active_body.as_ref().unwrap().name, "Earth");
    assert_eq!(
        snapshot.terrain.source_radial_lod,
        fixture.terrain.cover.convergence.rendered_local_lod
    );
    assert_eq!(
        snapshot.terrain.ready_radial_lod,
        fixture.terrain.cover.convergence.ready_local_lod
    );
    assert_eq!(
        snapshot.terrain.desired_radial_lod,
        fixture.terrain.cover.convergence.desired_local_lod
    );
    assert_eq!(
        snapshot.terrain.source_leaf_count,
        fixture.terrain.cover.active().len()
    );
    assert_eq!(
        snapshot.terrain.visible_leaf_count,
        fixture.terrain.cover.visible().len()
    );
    assert_eq!(snapshot.terrain.quality_pending, Some(true));
    assert_eq!(snapshot.terrain.settled, Some(false));
    assert_eq!(
        snapshot.work.pending_requests,
        fixture.terrain.cache.pending()
    );
    assert_eq!(snapshot.work.raw_resident_patches, cache.resident_patches);
    assert_eq!(
        snapshot.memory.used_bytes,
        (cache.resident_bytes + cache.external_bytes) as u64
    );
    assert_eq!(
        snapshot.memory.used_bytes + snapshot.memory.headroom_bytes,
        snapshot.memory.cap_bytes
    );
}

#[test]
fn objective_warning_rules_have_documented_boundaries_and_no_speculation() {
    let mut snapshot = Fixture::new(DeveloperScene::EarthClose).snapshot();
    snapshot.memory = MemorySnapshot {
        used_bytes: 95,
        cap_bytes: 100,
        headroom_bytes: 5,
    };
    snapshot.refresh_warnings();
    assert!(
        !snapshot
            .warnings
            .iter()
            .any(|w| w == "terrain_memory_near_cap")
    );
    snapshot.memory.used_bytes = 96;
    snapshot.terrain.quality_pending = Some(true);
    snapshot.terrain.active_morph = true;
    snapshot.terrain.source_radial_lod = Some(4);
    snapshot.terrain.desired_radial_lod = Some(18);
    snapshot.refresh_warnings();
    assert_eq!(
        snapshot.warnings,
        [
            "terrain_memory_near_cap",
            "terrain_quality_pending",
            "active_transition",
            "source_below_desired",
            "gpu_timing_unavailable"
        ]
    );
    snapshot.performance.gpu_terrain_ms = Some(0.1);
    snapshot.refresh_warnings();
    assert!(
        !snapshot
            .warnings
            .iter()
            .any(|w| w == "gpu_timing_unavailable")
    );
}

#[test]
fn terrain_summary_never_promotes_quality_pending_to_settled() {
    let mut terrain = TerrainSnapshot::default();
    assert_eq!(terrain.status(), "Not active");
    terrain.active_body = Some(BodySnapshot {
        index: 3,
        name: "Earth".into(),
    });
    assert_eq!(terrain.status(), "Preparing");
    terrain.ready = true;
    terrain.settled = Some(true);
    terrain.quality_pending = Some(true);
    assert_eq!(terrain.status(), "Refining");
    terrain.quality_pending = Some(false);
    assert_eq!(terrain.status(), "Settled");
    terrain.active_morph = true;
    assert_eq!(terrain.status(), "Transitioning");
    terrain.budget_constrained = true;
    assert_eq!(terrain.status(), "Resource constrained");
}

#[test]
fn named_scene_pose_and_snapshot_body_mode_are_deterministic() {
    for scene in DeveloperScene::ALL {
        assert_eq!(scene.name().parse::<DeveloperScene>().unwrap(), scene);
        let fixture = Fixture::new(scene);
        let first = fixture.snapshot();
        assert_eq!(first, fixture.snapshot());
        assert_eq!(
            first.general.camera_mode,
            SnapshotCameraMode::from(scene.camera_mode())
        );
        assert_eq!(
            first.general.focused_body.as_ref().map(|b| b.index),
            scene.target_body_index()
        );
        assert_eq!(first.camera.viewport_size_pixels, [960, 640]);
        assert!((first.camera.fov_y_degrees - 60.0).abs() < 1e-12);
        if scene == DeveloperScene::EarthClose {
            assert!((first.camera.reference_altitude_m.unwrap() - 10_000.0).abs() < 1e-8);
        }
        if scene == DeveloperScene::SolarOverview {
            let pair = fixture.frames.coherent_view(&fixture.world).unwrap();
            let pose = fixture_pose(scene, &pair, fixture.projection).unwrap();
            let forward = pose.orientation().quaternion() * -glam::DVec3::Z;
            assert!(
                forward.dot(-glam::DVec3::Z) > 0.99,
                "overview must look toward system, not away"
            );
        }
    }
    assert!("unknown".parse::<DeveloperScene>().is_err());
}

#[cfg(feature = "terrain-capture")]
#[test]
fn failed_pair_encoding_does_not_publish_orphan_evidence() {
    use mundaris_app::developer_capture::{DeveloperCapture, write_pair};
    let mut snapshot = Fixture::new(DeveloperScene::EarthOrbit).snapshot();
    snapshot.capture = Some(CaptureMetadata {
        scene: "earth-orbit".into(),
        image: "earth-orbit.png".into(),
        width: 1,
        height: 1,
        terrain_updates: 0,
        worker_count: 0,
        step_ms: 16,
        morph_duration_ms: 150,
        adapter: "fixture".into(),
        backend: "fixture".into(),
    });
    let capture = DeveloperCapture {
        rgba: vec![0],
        snapshot,
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/developer-interface-test/failed-{stamp}"
    ));
    assert!(write_pair(&directory, &capture).is_err());
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
}

#[cfg(feature = "terrain-capture")]
#[test]
#[ignore = "requires a graphics adapter; production PNG/readback validation"]
fn capture_pair_json_matches_rendered_body_mode_and_pixels() {
    use mundaris_app::developer_capture::{capture_scene, write_pair};
    for scene in DeveloperScene::ALL {
        let capture = capture_scene(scene).unwrap();
        let fixture = Fixture::new(scene);
        let expected_camera = fixture.snapshot().camera;
        assert_eq!(
            capture.snapshot.camera.position_m,
            expected_camera.position_m
        );
        assert_eq!(
            capture.snapshot.camera.orientation_xyzw,
            expected_camera.orientation_xyzw
        );
        assert_eq!(
            capture.snapshot.camera.body_distance_m,
            expected_camera.body_distance_m
        );
        assert_eq!(
            capture.snapshot.camera.terrain_clearance_m,
            expected_camera.terrain_clearance_m
        );
        assert_eq!(capture.snapshot.performance.gpu_timing_scope, "same_frame");
        assert_eq!(
            capture.snapshot.general.camera_mode,
            scene.camera_mode().into()
        );
        assert_eq!(
            capture
                .snapshot
                .general
                .focused_body
                .as_ref()
                .map(|b| b.index),
            scene.target_body_index()
        );
        assert_eq!(capture.snapshot.camera.position_m.len(), 3);
        let meta = capture.snapshot.capture.as_ref().unwrap();
        assert_eq!(meta.scene, scene.name());
        assert_eq!(
            capture.rgba.len(),
            meta.width as usize * meta.height as usize * 4
        );
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/developer-interface-test/{stamp}/{}",
            scene.name()
        ));
        write_pair(&directory, &capture).unwrap();
        let json: DeveloperSnapshot = serde_json::from_slice(
            &std::fs::read(directory.join(format!("{}.json", scene.name()))).unwrap(),
        )
        .unwrap();
        assert_eq!(json, capture.snapshot);
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(directory.join(&meta.image)).unwrap(),
        ));
        let mut reader = decoder.read_info().unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let output = reader.next_frame(&mut decoded).unwrap();
        assert_eq!(&decoded[..output.buffer_size()], capture.rgba.as_slice());
        assert!(
            write_pair(&directory, &capture).is_err(),
            "evidence must not be overwritten"
        );
        let repeated = capture_scene(scene).unwrap();
        assert_eq!(
            capture.rgba, repeated.rgba,
            "fixed serial scene must be pixel repeatable"
        );
        assert_eq!(capture.snapshot.terrain, repeated.snapshot.terrain);
    }
}
