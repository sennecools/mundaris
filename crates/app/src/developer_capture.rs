//! Small, deterministic production-path captures for developer inspection.
use crate::{
    celestial_camera::CameraMode,
    system_view::{OverviewScope, SystemViewBounds},
};
use anyhow::{Context, Result};
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_renderer::CelestialProjection;
use mundaris_world::CoherentCelestialView;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[cfg(feature = "terrain-capture")]
mod capture {
    use super::*;
    use crate::{
        developer_snapshot::{
            CaptureMetadata, DeveloperSnapshot, PerformanceSnapshot, RenderingSnapshot,
            SnapshotInput, render_mode_name,
        },
        motion_session::{AnalyticSession, MotionSnapshot},
        planet_surface::PlanetSurfaceSession,
        planet_terrain::MAX_TERRAIN_PATCHES,
        solar_system::{self, SOLAR_SYSTEM_CONTENT, SolarSystemPreset},
        terrain_population::TerrainPopulation,
    };
    use anyhow::Context;
    use anyhow::ensure;
    use mundaris_math::surface::SurfaceLocation;
    use mundaris_renderer::{
        CelestialFrame, CelestialRenderBody, CelestialStaging, Icosphere, PreparedView,
        RenderPrecisionBudget,
        planet_surface::{SurfaceStyle, TerrainLighting, TerrainRenderMode},
        terrain_capture::TerrainCaptureRenderer,
    };
    use mundaris_world::terrain::{
        TerrainFootprint, TerrainGenerator, TerrainIdentity, TerrainQuery, TerrainSeed,
    };
    use std::{
        fs,
        num::NonZeroU64,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    const UPDATES: usize = 64;
    const CRATER_DETAIL_UPDATES: usize = 4096;
    const VERTEX_BUDGET: usize = 64;
    const STEP: Duration = Duration::from_millis(16);
    const MORPH: Duration = Duration::from_millis(150);

    #[derive(Debug, Clone, Copy)]
    struct CaptureBudget {
        updates: usize,
        camera_mode: Option<CameraMode>,
        vertices_per_update: usize,
    }

    #[derive(Debug)]
    pub struct DeveloperCapture {
        pub rgba: Vec<u8>,
        pub snapshot: DeveloperSnapshot,
    }

    pub fn capture_scene(scene: DeveloperScene) -> Result<DeveloperCapture> {
        capture_scene_at(scene, 0.0)
    }
    pub fn capture_scene_at(scene: DeveloperScene, time_s: f64) -> Result<DeveloperCapture> {
        let namespace = NonZeroU64::new(512).context("invalid fixture namespace")?;
        let (mut world, definition) = SolarSystemPreset::gameplay().create_analytic(namespace)?;
        let mut motion = AnalyticSession::new(&mut world, definition)?;
        motion.seek_seconds(time_s, &mut world)?;
        let publication_started = Instant::now();
        let frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
        motion.frame_published(&world, publication_started.elapsed());
        let pair = frames.coherent_view(&world)?;
        let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60_f64.to_radians(), 0.1)?;
        let pose = fixture_pose(scene, &pair, projection)?;
        capture_pose(
            scene,
            &pair,
            projection,
            pose,
            None,
            Some(motion.snapshot(&world)),
        )
    }

    /// Captures deterministic diagnostic views of the gameplay Moon's seeded crater field.
    /// The fixed-step update counts are work budgets, not elapsed simulation time.
    pub fn capture_crater_reference(output: &Path, seed: u64) -> Result<()> {
        let namespace = NonZeroU64::new(514).context("invalid crater fixture namespace")?;
        let (mut world, definition) = SolarSystemPreset::gameplay().create_analytic(namespace)?;
        let (moon_id, moon) = world
            .bodies()
            .nth(4)
            .context("gameplay Moon fixture is missing")?;
        let radius_m = moon.properties().reference_radius_m();
        let terrain_definition = solar_system::cratered_terrain_definition(
            TerrainIdentity(seed),
            TerrainSeed(seed),
            radius_m,
        )?;
        let generator = TerrainGenerator::new(&terrain_definition, radius_m)?;
        ensure!(
            !generator.crater_features().is_empty(),
            "generated crater catalogue is empty"
        );
        world.edit_terrain(moon_id, Some(terrain_definition.clone()))?;
        ensure!(
            world.body(moon_id)?.terrain() == Some(&terrain_definition),
            "Moon terrain does not match the requested crater fixture"
        );

        let mut motion = AnalyticSession::new(&mut world, definition)?;
        motion.seek_seconds(0.0, &mut world)?;
        let publication_started = Instant::now();
        let frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
        motion.frame_published(&world, publication_started.elapsed());
        let pair = frames.coherent_view(&world)?;
        let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60_f64.to_radians(), 0.1)?;
        let body = pair.system().body(moon_id)?;
        ensure!(
            body.terrain() == Some(&terrain_definition),
            "published Moon terrain does not match the crater generator"
        );
        let moon_frame = pair.projection().frames_for(moon_id)?.body_fixed;
        let root = pair.projection().tree().root();
        let sun = pair
            .system()
            .bodies()
            .next()
            .context("Sun fixture is missing")?
            .1
            .state()
            .center_in_system()
            .metres();
        let to_sun = sun - body.state().center_in_system().metres();
        let sun_direction = pair
            .evaluation()
            .convert_direction(
                FrameDirection::new(root, Direction3::try_new(to_sun)?),
                moon_frame,
            )?
            .local()
            .unit();
        let features = generator.crater_features();
        let chosen = features
            .iter()
            .copied()
            .filter(|feature| {
                let dot = feature.center().unit().dot(sun_direction);
                (0.25..=0.85).contains(&dot)
            })
            .max_by(|left, right| left.radius_m().total_cmp(&right.radius_m()))
            .or_else(|| {
                features.iter().copied().max_by(|left, right| {
                    left.center()
                        .unit()
                        .dot(sun_direction)
                        .total_cmp(&right.center().unit().dot(sun_direction))
                })
            })
            .context("no generated crater feature is available")?;
        let center = chosen.center().unit();
        let radial_camera_pose = |direction: DVec3, clearance_m: f64| -> Result<FramePose> {
            ensure!(
                clearance_m.is_finite() && clearance_m > 0.0,
                "crater capture clearance must be finite and positive"
            );
            let location = SurfaceLocation::new(Direction3::try_new(direction)?);
            let sample = generator.evaluate_point(TerrainQuery {
                location,
                footprint: TerrainFootprint::COMPLETE,
            })?;
            let eye = direction * (radius_m + sample.height_m() + clearance_m);
            Ok(FramePose::new(
                FramePosition::new(moon_frame, LocalPosition::try_metres(eye)?),
                look_rotation(-direction)?,
            ))
        };
        let tangent_to_sun = (sun_direction - center * center.dot(sun_direction))
            .try_normalize()
            .or_else(|| (DVec3::X - center * center.x).try_normalize())
            .or_else(|| (DVec3::Z - center * center.z).try_normalize())
            .context("no tangent direction is available for crater rim pose")?;
        let rim_angle = chosen.radius_m() / radius_m;
        let rim_direction = (center * rim_angle.cos() + tangent_to_sun * rim_angle.sin())
            .try_normalize()
            .context("crater rim direction is degenerate")?;
        let rim_pose = radial_camera_pose(rim_direction, 200.0)?;
        let center_sample = generator.evaluate_point(TerrainQuery {
            location: SurfaceLocation::new(chosen.center()),
            footprint: TerrainFootprint::COMPLETE,
        })?;
        let floor = center * (radius_m + center_sample.height_m());
        let rim_pose = FramePose::new(rim_pose.position(), {
            let forward = (floor - rim_pose.position().local().metres()).normalize();
            let right = forward.cross(rim_direction).normalize();
            let up = right.cross(forward).normalize();
            UnitRotation::try_from_quaternion(DQuat::from_mat3(&glam::DMat3::from_cols(
                right, up, -forward,
            )))?
        });
        let views = [
            (
                "crater-orbit",
                radial_camera_pose(center, radius_m)?,
                512,
                CameraMode::BodyOrbit,
            ),
            (
                "crater-regional",
                radial_camera_pose(center, (2.0 * chosen.radius_m()).max(5_000.0))?,
                CRATER_DETAIL_UPDATES,
                CameraMode::SurfaceInspection,
            ),
            (
                "crater-rim",
                rim_pose,
                CRATER_DETAIL_UPDATES,
                CameraMode::SurfaceInspection,
            ),
        ];
        fs::create_dir_all(output)?;
        let manifest_path = output.join("crater-reference.json");
        ensure!(
            !manifest_path.exists(),
            "refusing to replace existing crater reference manifest"
        );
        for (name, _, _, _) in views {
            ensure!(
                !output.join(format!("{name}.png")).exists()
                    && !output.join(format!("{name}.json")).exists(),
                "refusing to replace existing crater capture {name}"
            );
        }
        for (name, pose, updates, camera_mode) in views {
            let mut capture = capture_pose_with_updates(
                DeveloperScene::MoonOrbit,
                &pair,
                projection,
                pose,
                None,
                Some(motion.snapshot(&world)),
                CaptureBudget {
                    updates,
                    camera_mode: Some(camera_mode),
                    vertices_per_update: 1156,
                },
            )?;
            ensure!(
                pair.system().body(moon_id)?.terrain() == Some(&terrain_definition)
                    && capture
                        .snapshot
                        .terrain
                        .active_body
                        .as_ref()
                        .is_some_and(|active| active.index == 4),
                "crater capture has inactive or mismatched Moon terrain"
            );
            let metadata = capture
                .snapshot
                .capture
                .as_mut()
                .context("capture metadata missing")?;
            metadata.scene = name.into();
            metadata.image = format!("{name}.png");
            write_pair(output, &capture)?;
        }

        let manifest = serde_json::json!({
            "fixture": "solar regression fixture using generic seeded crater terrain; not procedural-system generation",
            "seed": seed,
            "radius_m": radius_m,
            "generator_code": terrain_definition.version().code(),
            "generated_crater_count": features.len(),
            "chosen_feature": {
                "center_body_fixed": chosen.center().unit().to_array(),
                "radius_m": chosen.radius_m(),
                "depth_m": chosen.depth_m(),
                "rim_height_m": chosen.rim_height_m(),
                "sun_dot": chosen.center().unit().dot(sun_direction),
            },
            "captures": ["crater-orbit", "crater-regional", "crater-rim"],
            "timing_limitation": "Serial diagnostic terrain updates at fixed 16 ms steps; update counts are work budgets, not admitted elapsed simulation time or native control evidence.",
            "vertices_per_update": 1156,
        });
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .context("publishing crater reference manifest without replacement")?;
        file.write_all(&manifest_bytes)?;
        file.write_all(b"\n")?;
        Ok(())
    }

    /// Small same-session playback probe: epoch, forward, reverse, then epoch again.
    pub fn capture_analytic_playback(output: &Path) -> Result<()> {
        let namespace = NonZeroU64::new(513).context("playback fixture namespace")?;
        let (mut world, definition) = SolarSystemPreset::gameplay().create_analytic(namespace)?;
        let mut motion = AnalyticSession::new(&mut world, definition)?;
        let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60_f64.to_radians(), 0.1)?;
        let mut frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
        for (label, time_s) in [
            ("epoch", 0.0),
            ("forward", 86_400.0),
            ("reverse", -86_400.0),
            ("return", 0.0),
        ] {
            motion.seek_seconds(time_s, &mut world)?;
            let publication_started = Instant::now();
            frames.publish(&world)?;
            motion.frame_published(&world, publication_started.elapsed());
            let pair = frames.coherent_view(&world)?;
            let scene = DeveloperScene::EarthOrbit;
            let pose = fixture_pose(scene, &pair, projection)?;
            let mut capture = capture_pose(
                scene,
                &pair,
                projection,
                pose,
                None,
                Some(motion.snapshot(&world)),
            )?;
            let metadata = capture
                .snapshot
                .capture
                .as_mut()
                .context("capture metadata missing")?;
            metadata.scene = format!("analytic-playback-{label}");
            metadata.image = format!("{}.png", metadata.scene);
            write_pair(output, &capture)?;
        }
        Ok(())
    }
    fn capture_pose(
        scene: DeveloperScene,
        pair: &CoherentCelestialView<'_>,
        projection: CelestialProjection,
        pose: FramePose,
        camera: Option<&crate::celestial_camera::CelestialCamera>,
        motion: Option<MotionSnapshot>,
    ) -> Result<DeveloperCapture> {
        capture_pose_with_updates(
            scene,
            pair,
            projection,
            pose,
            camera,
            motion,
            CaptureBudget {
                updates: UPDATES,
                camera_mode: None,
                vertices_per_update: VERTEX_BUDGET,
            },
        )
    }

    fn capture_pose_with_updates(
        scene: DeveloperScene,
        pair: &CoherentCelestialView<'_>,
        projection: CelestialProjection,
        pose: FramePose,
        camera: Option<&crate::celestial_camera::CelestialCamera>,
        motion: Option<MotionSnapshot>,
        budget: CaptureBudget,
    ) -> Result<DeveloperCapture> {
        let evaluation = pair.evaluation();
        let requests: Vec<_> = pair
            .system()
            .bodies()
            .enumerate()
            .map(|(index, (id, body))| {
                Ok((
                    id,
                    CelestialRenderBody {
                        body_fixed_frame: pair.projection().frames_for(id)?.body_fixed,
                        reference_radius_m: body.properties().reference_radius_m(),
                        color: SOLAR_SYSTEM_CONTENT[index].color,
                        unlit: index == 0,
                        selected: false,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let render_bodies: Vec<_> = requests.iter().map(|(_, request)| *request).collect();
        let target = scene
            .target_body_index()
            .and_then(|index| requests.get(index).map(|(id, _)| *id));
        let selected = target.or_else(|| requests.get(3).map(|(id, _)| *id));
        let mut render_bodies = render_bodies;
        if let Some((index, _)) = selected.and_then(|selected| {
            requests
                .iter()
                .position(|(id, _)| *id == selected)
                .map(|index| (index, selected))
        }) {
            render_bodies[index].selected = true;
        }
        let view = PreparedView::new(&evaluation, pose, RenderPrecisionBudget::near_debug())?;
        let mut sessions = requests
            .iter()
            .filter(|(id, _)| pair.system().body(*id).is_ok_and(|b| b.has_surface()))
            .map(|(id, _)| PlanetSurfaceSession::new(*id, MAX_TERRAIN_PATCHES))
            .collect::<Result<Vec<_>>>()?;
        let mut population = TerrainPopulation::new()?;
        let mut owners = vec![false; render_bodies.len()];
        let sphere = Icosphere::new();
        let mut update_duration = Duration::ZERO;
        for _ in 0..budget.updates {
            let update_started = Instant::now();
            population.update(
                pair,
                &view,
                projection,
                &render_bodies,
                &mut sessions,
                &mut owners,
                &sphere,
                true,
                MORPH,
                budget.vertices_per_update,
                None,
                STEP,
            )?;
            update_duration = update_started.elapsed();
        }
        let mut gpu = TerrainCaptureRenderer::new(WIDTH, HEIGHT)?;
        let mut staging = CelestialStaging::default();
        let preparation_started = Instant::now();
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        let sky = crate::sky_definition::default_sky()?;
        let sky_settings = mundaris_renderer::sky::SkySettings::default();
        frame.set_distant_sky(
            pair.projection().tree().root(),
            std::sync::Arc::clone(&sky),
            sky_settings,
        )?;
        let light_body = population
            .active_body()
            .or(target)
            .or(selected)
            .context("fixture light body missing")?;
        let light_index = requests
            .iter()
            .position(|(id, _)| *id == light_body)
            .context("light body association missing")?;
        let light = natural_lighting(pair, &requests, light_index)?;
        frame.set_terrain_lighting(light);
        for (index, (_, request)) in requests.iter().enumerate() {
            if !owners[index] {
                continue;
            }
            let active_surface = population.active_body() == Some(requests[index].0)
                && owners[index]
                && population.cover.ready();
            let environment = solar_system::planetary_config(
                SOLAR_SYSTEM_CONTENT[index].identity,
                request.reference_radius_m,
            )?;
            if active_surface {
                if let Some(config) = environment {
                    frame.set_planetary_environment(*request, config)?;
                }
                frame.append_stitched_surface(
                    *request,
                    population.cover.visible(),
                    population
                        .cover
                        .surface()
                        .context("ready cover missing generated surface")?,
                    population
                        .cover
                        .topology()
                        .context("ready cover missing topology")?,
                    SurfaceStyle::default(),
                )?;
                if let Some((mesh, fraction)) = population.cover.transition() {
                    frame.append_surface_transition(
                        *request,
                        mesh,
                        fraction,
                        SurfaceStyle::default(),
                    )?;
                }
            } else {
                let session = sessions
                    .iter()
                    .find(|session| session.body() == requests[index].0)
                    .context("terrain body session missing")?;
                frame.append_surface(
                    *request,
                    session.lod().active_visible(),
                    session.lod().topology(),
                    SurfaceStyle::default(),
                )?;
            }
        }
        frame.append_body_observations(&render_bodies, &owners)?;
        if scene == DeveloperScene::SolarOverview {
            // Same production polyline overlays as existing Solar captures. Bodies
            // remain at physical radii; subpixel planets otherwise cannot be inspected.
            let root = pair.projection().tree().root();
            let observer = pose.position().local().metres();
            for (index, (_, body)) in pair.system().bodies().enumerate() {
                let center = body.state().center_in_system().metres();
                let half = (center - observer).length() * 5.0 / projection.focal_pixels();
                let color = SOLAR_SYSTEM_CONTENT[index].color;
                for axis in [DVec3::X, DVec3::Y] {
                    let points = [
                        FramePosition::new(root, LocalPosition::try_metres(center - axis * half)?),
                        FramePosition::new(root, LocalPosition::try_metres(center + axis * half)?),
                    ];
                    frame.append_polylines(&[mundaris_renderer::CelestialPolyline {
                        points: &points,
                        colors: &[color, color],
                        width_pixels: 1.5,
                        style: mundaris_renderer::CelestialLineStyle::Solid,
                    }])?;
                }
            }
        }
        let report = frame.report();
        if budget.camera_mode.is_some() {
            println!(
                "crater work: selector_level={} precision_floor={} splits={} merges={} deferred={} parent={:?} work={:?} cache={:?}",
                population.cover.report.max_level,
                population.cover.report.precision_floor,
                population.cover.report.splits,
                population.cover.report.merges,
                population.cover.report.deferred_transactions,
                population.cover.report.refinement_parent,
                population.work,
                population.cache.report(),
            );
            println!(
                "crater preparation: patches={} triangles={} fallback_triangles={} draws={} max_error_pixels={} convergence={:?}",
                report.surface.patches,
                report.surface.triangles,
                report.surface.fallback_triangles,
                report.surface.draws,
                population.cover.report.max_error_pixels,
                population.cover.convergence,
            );
        }
        let preparation_duration = preparation_started.elapsed();
        let rgba = gpu.render(&frame)?;
        let profile = gpu.last_gpu_profile();
        let upload = gpu.last_terrain_upload_profile();
        let clearance = target
            .map(|id| crate::terrain_inspection::terrain_clearance(pair, pose, id))
            .transpose()?
            .flatten();
        let drawn_mesh_clearance = if let Some(clearance) =
            clearance.filter(|c| population.active_body() == Some(c.body))
        {
            crate::surface_probe::ready_mesh_probe(&population.cover, clearance.location)?
                .map(|mesh| clearance.camera_radius_m - mesh.radius_m)
        } else {
            None
        };
        let rendering = RenderingSnapshot {
            terrain_render_mode: render_mode_name(light.mode()).into(),
            terrain_enabled: true,
            ocean_enabled: true,
            clouds_enabled: true,
            atmosphere_enabled: true,
            navigation_markers_enabled: scene == DeveloperScene::SolarOverview,
            ..Default::default()
        }
        .with_draw_report(report);
        let frame_cpu = update_duration + preparation_duration;
        #[cfg(feature = "surface-profile")]
        let terrain_update_ms = Some(population.profile.total.as_secs_f64() * 1000.0);
        #[cfg(not(feature = "surface-profile"))]
        let terrain_update_ms = None;
        #[cfg(feature = "surface-profile")]
        let terrain_preparation_ms = Some(report.surface.profile.total.as_secs_f64() * 1000.0);
        #[cfg(not(feature = "surface-profile"))]
        let terrain_preparation_ms = None;
        let mut snapshot = DeveloperSnapshot::collect(SnapshotInput {
            navigation: camera.map(|c| c.navigation_diagnostics()),
            paused: motion.as_ref().is_none_or(|m| m.paused),
            simulation_speed: motion.as_ref().map_or(1.0, |m| m.rate),
            motion,
            pair,
            pose,
            camera_mode: budget
                .camera_mode
                .unwrap_or_else(|| camera.map_or(scene.camera_mode(), |c| c.mode())),
            selected_body: selected,
            focused_body: target,
            reference_body: target,
            frame_number: budget.updates as u64,
            projection,
            terrain: &population,
            terrain_clearance_m: clearance.map(|c| c.clearance_m),
            drawn_mesh_clearance_m: drawn_mesh_clearance,
            rendering,
            performance: PerformanceSnapshot {
                frame_cpu_ms: Some(frame_cpu.as_secs_f64() * 1000.0),
                update_ms: Some(update_duration.as_secs_f64() * 1000.0),
                terrain_update_ms,
                preparation_ms: Some(preparation_duration.as_secs_f64() * 1000.0),
                terrain_preparation_ms,
                upload_bytes: Some(upload.bytes_uploaded),
                ..Default::default()
            }
            .with_gpu(profile, "same_frame"),
        })?;
        snapshot.capture = Some(CaptureMetadata {
            scene: scene.name().into(),
            image: format!("{}.png", scene.name()),
            width: WIDTH,
            height: HEIGHT,
            terrain_updates: budget.updates,
            worker_count: 0,
            step_ms: STEP.as_millis() as u64,
            morph_duration_ms: MORPH.as_millis() as u64,
            adapter: gpu.adapter_name().into(),
            backend: gpu.adapter_backend().into(),
        });
        if let Some(sky_report) = report.sky {
            snapshot.sky = Some(crate::developer_snapshot::SkySnapshot::collect(
                &sky,
                sky_settings,
                sky_report,
                Some(gpu.last_sky_resource_report()),
                profile,
                "same_frame upload API; excludes submission/readback",
                "same_frame sky draw timestamps; excludes readback",
            ));
        }
        Ok(DeveloperCapture { rgba, snapshot })
    }

    /// Ordinary-controller route; unlike static fixtures, every checkpoint follows
    /// production wheel/mode commands. No direct pose or clearance assignment.
    pub fn capture_navigation_route(output: &Path) -> Result<()> {
        use crate::celestial_camera::{CelestialCamera, NavigationInput};
        let namespace = NonZeroU64::new(512).context("route namespace")?;
        let (mut world, definition) = SolarSystemPreset::gameplay().create_analytic(namespace)?;
        let mut motion = AnalyticSession::new(&mut world, definition)?;
        motion.set_paused(true);
        let publication_started = Instant::now();
        let frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
        motion.frame_published(&world, publication_started.elapsed());
        let pair = frames.coherent_view(&world)?;
        let projection = CelestialProjection::try_new(WIDTH, HEIGHT, 60_f64.to_radians(), 0.1)?;
        let ids = world.bodies().map(|(id, _)| id).collect::<Vec<_>>();
        let bounds =
            SystemViewBounds::calculate(&world, &OverviewScope::WholeSystem, &[], &[], false)?;
        let fit = bounds.fit_distance_m(projection)?;
        let mut camera = CelestialCamera::overview(&pair, bounds.center_m(), fit / 2.5)?;
        camera.set_navigation_projection(projection, 1.0)?;
        let mut checkpoints = Vec::new();
        let mut elapsed = 0.0;
        let save = |name: &str,
                    scene: DeveloperScene,
                    camera: &CelestialCamera|
         -> Result<DeveloperSnapshot> {
            let mut capture = capture_pose(
                scene,
                &pair,
                projection,
                camera.pose(),
                Some(camera),
                Some(motion.snapshot(&world)),
            )?;
            if let Some(metadata) = &mut capture.snapshot.capture {
                metadata.scene = name.into();
                metadata.image = format!("{name}.png");
            }
            write_pair(output, &capture)?;
            Ok(capture.snapshot)
        };
        let settle = |camera: &mut CelestialCamera, seconds: f64| -> Result<()> {
            for _ in 0..(seconds * 60.0) as usize {
                camera.update_navigation(
                    &pair,
                    &NavigationInput::default(),
                    Duration::from_secs_f64(1.0 / 60.0),
                )?;
            }
            Ok(())
        };
        save(
            "route-overview-start",
            DeveloperScene::SolarOverview,
            &camera,
        )?;
        for (index, orbit_scene, surface_scene, name) in [
            (
                3,
                DeveloperScene::EarthOrbit,
                DeveloperScene::EarthClose,
                "earth",
            ),
            (
                4,
                DeveloperScene::MoonOrbit,
                DeveloperScene::MoonOrbit,
                "moon",
            ),
        ] {
            let body = ids[index];
            camera.fit_body(
                &pair,
                body,
                projection.vertical_fov_rad(),
                WIDTH as f64 / HEIGHT as f64,
            )?;
            settle(&mut camera, 1.0)?;
            elapsed += 1.0;
            let c = crate::terrain_inspection::terrain_clearance(&pair, camera.pose(), body)?
                .context("route requires complete terrain")?
                .clearance_m;
            let k = camera.navigation_diagnostics().wheel_log_per_notch;
            camera.update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: (c / 100_000.0).ln() / k,
                    ..Default::default()
                },
                Duration::ZERO,
            )?;
            settle(&mut camera, 2.0)?;
            elapsed += 2.0;
            save(&format!("route-{name}-orbit"), orbit_scene, &camera)?;
            camera.enter_surface_inspection(&pair, body)?;
            let c = crate::terrain_inspection::terrain_clearance(&pair, camera.pose(), body)?
                .context("route clearance")?
                .clearance_m;
            let notches = (c / 2.0).ln() / k;
            camera.update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: notches,
                    ..Default::default()
                },
                Duration::ZERO,
            )?;
            settle(&mut camera, 2.0)?;
            elapsed += 2.0;
            let snapshot = save(&format!("route-{name}-2m"), surface_scene, &camera)?;
            let actual = snapshot
                .camera
                .terrain_clearance_m
                .context("checkpoint terrain clearance")?;
            checkpoints.push(serde_json::json!({"body":name,"requested_complete_terrain_clearance_m":2.0,
                "actual_complete_terrain_clearance_m":actual,"surface_wheel_notches":notches,
                "admitted_navigation_time_s":elapsed,"quality_pending":snapshot.terrain.quality_pending,
                "drawn_mesh_clearance_m":snapshot.camera.drawn_mesh_clearance_m}));
            camera.update_navigation(
                &pair,
                &NavigationInput {
                    translation: DVec3::X,
                    ..Default::default()
                },
                Duration::from_millis(100),
            )?;
            camera.update_navigation(
                &pair,
                &NavigationInput {
                    translation: DVec3::Y,
                    ..Default::default()
                },
                Duration::from_millis(100),
            )?;
            elapsed += 0.2;
            ensure!(
                (actual - 2.0).abs() <= 0.05,
                "ordinary route missed 2 m target on {name}"
            );
        }
        camera.fit_overview(&pair, bounds.center_m(), fit)?;
        settle(&mut camera, 1.0)?;
        save(
            "route-overview-recovered",
            DeveloperScene::SolarOverview,
            &camera,
        )?;
        let route = output.join("navigation-route.json");
        ensure!(!route.exists(), "refusing to replace route record");
        fs::write(route, serde_json::to_vec_pretty(&checkpoints)?)?;
        Ok(())
    }

    fn natural_lighting(
        pair: &mundaris_world::CoherentCelestialView<'_>,
        requests: &[(mundaris_world::BodyId, CelestialRenderBody)],
        target_index: usize,
    ) -> Result<TerrainLighting> {
        let (sun_id, _) = requests.first().context("Sun missing")?;
        let sun = pair
            .system()
            .body(*sun_id)?
            .state()
            .center_in_system()
            .metres();
        let (id, request) = requests.get(target_index).context("light target missing")?;
        let center = pair.system().body(*id)?.state().center_in_system().metres();
        let root = pair.projection().tree().root();
        let direction = Direction3::try_new(sun - center)?;
        let light = pair
            .evaluation()
            .convert_direction(
                FrameDirection::new(root, direction),
                request.body_fixed_frame,
            )?
            .local()
            .unit();
        let lighting = TerrainLighting::try_new(light, 0.06, 0.94, TerrainRenderMode::Natural)?;
        Ok(
            if let Some(config) = solar_system::terrain_readability_config(
                SOLAR_SYSTEM_CONTENT[target_index].identity,
                request.reference_radius_m,
            )? {
                lighting.with_readability(config)
            } else {
                lighting
            },
        )
    }

    pub fn write_pair(output: &Path, capture: &DeveloperCapture) -> Result<()> {
        let metadata = capture
            .snapshot
            .capture
            .as_ref()
            .context("capture metadata missing")?;
        let image = output.join(&metadata.image);
        let json = output.join(format!("{}.json", metadata.scene));
        ensure!(
            !image.exists() && !json.exists(),
            "refusing to replace existing capture files for {}",
            metadata.scene
        );
        fs::create_dir_all(output)?;
        // Encode both before publishing either. Linking staged files refuses
        // existing destinations on Windows and Unix, unlike overwrite-prone rename.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let staging = output.join(format!(".capture-{}-{stamp}", std::process::id()));
        fs::create_dir(&staging)?;
        let staged_image = staging.join("frame.png");
        let staged_json = staging.join("frame.json");
        let mut image_published = false;
        let mut json_published = false;
        let result = (|| -> Result<()> {
            let file = fs::File::create(&staged_image)?;
            let mut encoder = png::Encoder::new(file, metadata.width, metadata.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&capture.rgba)?;
            writer.finish()?;
            capture.snapshot.write_json(&staged_json)?;
            fs::hard_link(&staged_image, &image)
                .context("publishing capture PNG without replacement")?;
            image_published = true;
            fs::hard_link(&staged_json, &json)
                .context("publishing capture JSON without replacement")?;
            json_published = true;
            Ok(())
        })();
        if result.is_err() {
            if image_published {
                fs::remove_file(&image).context("rolling back owned capture PNG")?;
            }
            if json_published {
                fs::remove_file(&json).context("rolling back owned capture JSON")?;
            }
        }
        for path in [&staged_image, &staged_json] {
            if path.exists() {
                fs::remove_file(path).context("cleaning capture staging file")?;
            }
        }
        fs::remove_dir(&staging).context("cleaning capture staging directory")?;
        result?;
        Ok(())
    }

    pub fn default_output_directory() -> PathBuf {
        PathBuf::from("target/mundaris-diagnostics")
    }
}

#[cfg(feature = "terrain-capture")]
pub use capture::{
    DeveloperCapture, capture_analytic_playback, capture_crater_reference,
    capture_navigation_route, capture_scene, capture_scene_at, default_output_directory,
    write_pair,
};

#[cfg(feature = "terrain-capture")]
pub const WIDTH: u32 = 960;
#[cfg(feature = "terrain-capture")]
pub const HEIGHT: u32 = 640;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeveloperScene {
    SolarOverview,
    EarthOrbit,
    EarthClose,
    MoonOrbit,
}
impl DeveloperScene {
    pub const ALL: [Self; 4] = [
        Self::SolarOverview,
        Self::EarthOrbit,
        Self::EarthClose,
        Self::MoonOrbit,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::SolarOverview => "solar-overview",
            Self::EarthOrbit => "earth-orbit",
            Self::EarthClose => "earth-close",
            Self::MoonOrbit => "moon-orbit",
        }
    }
    pub const fn camera_mode(self) -> CameraMode {
        match self {
            Self::SolarOverview => CameraMode::SystemOrbit,
            Self::EarthOrbit | Self::MoonOrbit => CameraMode::BodyOrbit,
            Self::EarthClose => CameraMode::SurfaceInspection,
        }
    }
    pub const fn target_body_index(self) -> Option<usize> {
        match self {
            Self::SolarOverview => None,
            Self::EarthOrbit | Self::EarthClose => Some(3),
            Self::MoonOrbit => Some(4),
        }
    }
}
impl FromStr for DeveloperScene {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|scene| scene.name() == s)
            .ok_or_else(|| anyhow::anyhow!("unknown developer scene {s:?}; expected solar-overview, earth-orbit, earth-close, or moon-orbit"))
    }
}

/// A fixed, source-centred observer for a coherent published celestial pair.
pub fn fixture_pose(
    scene: DeveloperScene,
    pair: &CoherentCelestialView<'_>,
    projection: CelestialProjection,
) -> Result<FramePose> {
    let root = pair.projection().tree().root();
    if scene == DeveloperScene::SolarOverview {
        let ids = pair.system().bodies().map(|(id, _)| id).collect::<Vec<_>>();
        let bounds = SystemViewBounds::calculate(
            pair.system(),
            &OverviewScope::ExplicitBodies(ids),
            &[],
            &[],
            false,
        )?;
        let center = bounds.center_m();
        let position = center + DVec3::Z * bounds.fit_distance_m(projection)?;
        return Ok(FramePose::new(
            FramePosition::new(root, LocalPosition::try_metres(position)?),
            look_rotation(-DVec3::Z)?,
        ));
    }
    let index = scene.target_body_index().context("scene has no target")?;
    let (id, body) = pair
        .system()
        .bodies()
        .nth(index)
        .context("target body unavailable")?;
    let fixed = pair.projection().frames_for(id)?.body_fixed;
    let radius = body.properties().reference_radius_m();
    let star = pair.system().bodies().next().context("Sun unavailable")?.1;
    let to_sun =
        star.state().center_in_system().metres() - body.state().center_in_system().metres();
    let sun_dir = pair
        .evaluation()
        .convert_direction(
            FrameDirection::new(root, Direction3::try_new(to_sun)?),
            fixed,
        )?
        .local()
        .unit();
    let (direction, distance) = match scene {
        DeveloperScene::EarthOrbit => (fixture_direction(sun_dir), radius + 800_000.0),
        DeveloperScene::EarthClose => (fixture_direction(sun_dir), radius + 10_000.0),
        DeveloperScene::MoonOrbit => (fixture_direction(sun_dir), radius * 2.0),
        DeveloperScene::SolarOverview => anyhow::bail!("overview pose was already handled"),
    };
    let eye = direction * distance;
    Ok(FramePose::new(
        FramePosition::new(fixed, LocalPosition::try_metres(eye)?),
        look_rotation(-direction)?,
    ))
}

fn fixture_direction(sun: DVec3) -> DVec3 {
    let tangent = sun.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
    (sun + DVec3::Z * 0.15 + tangent * 0.04).normalize()
}

fn look_rotation(forward: DVec3) -> Result<UnitRotation> {
    let forward = forward.normalize();
    let right = forward.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
    let up = right.cross(forward).normalize();
    Ok(UnitRotation::try_from_quaternion(DQuat::from_mat3(
        &glam::DMat3::from_cols(right, up, -forward),
    ))?)
}
