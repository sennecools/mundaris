//! Focused first-look fixtures for the distant sky and its depth interaction.

use anyhow::{Context, Result};
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialRenderBody, CelestialStaging, Icosphere,
    PreparedView, RenderPrecisionBudget,
    planet_surface::{TerrainLighting, TerrainRenderMode},
    sky::SkySettings,
    terrain_capture::TerrainCaptureRenderer,
};
use mundaris_world::BodyId;
use std::{num::NonZeroU64, path::Path, sync::Arc, time::Instant};

use crate::{
    celestial_camera::CameraMode,
    developer_capture::{DeveloperCapture, HEIGHT, WIDTH, write_pair},
    developer_snapshot::{
        CaptureMetadata, DeveloperSnapshot, PerformanceSnapshot, RenderingSnapshot, SnapshotInput,
    },
    motion_session::{AnalyticSession, MotionSnapshot},
    solar_system::{self, SOLAR_SYSTEM_CONTENT, SolarBody, SolarSystemPreset},
    terrain_population::TerrainPopulation,
};

const SKY_RESOURCE_SCOPE: &str = "same_frame upload API; excludes submission/readback";
const SKY_GPU_SCOPE: &str = "same_frame sky draw timestamps; excludes readback";

/// Writes deterministic first-look sky/depth PNG and canonical schema-4 JSON pairs.
pub fn capture_first_look(output: &Path) -> Result<()> {
    capture_first_look_with_options(output, SkyCaptureOptions::default())
}

/// Dimensions and optional temporal preview settings for sky diagnostic captures.
#[derive(Clone, Copy, Debug)]
pub struct SkyCaptureOptions {
    pub width: u32,
    pub height: u32,
    pub sequence_frames: usize,
}

impl Default for SkyCaptureOptions {
    fn default() -> Self {
        Self {
            width: WIDTH,
            height: HEIGHT,
            sequence_frames: 0,
        }
    }
}

/// Writes the legacy stills at requested dimensions and optionally a slow observer
/// rotation/translation preview. Each sequence frame's paired snapshot records the
/// exact rendered pose and its sample instant.
pub fn capture_first_look_with_options(output: &Path, options: SkyCaptureOptions) -> Result<()> {
    anyhow::ensure!(
        options.width > 0 && options.height > 0 && options.width <= 8192 && options.height <= 8192,
        "capture dimensions must be in 1..=8192"
    );
    anyhow::ensure!(
        options.width as u64 * options.height as u64 <= 33_554_432,
        "capture exceeds 33,554,432 pixel safety limit"
    );
    anyhow::ensure!(
        options.sequence_frames <= 101,
        "sequence frame count exceeds 101-frame (1e14 m) preview envelope"
    );
    let namespace = NonZeroU64::new(518).context("fixture namespace")?;
    let (mut world, motion_definition) =
        SolarSystemPreset::gameplay().create_analytic(namespace)?;
    let mut motion = AnalyticSession::new(&mut world, motion_definition)?;
    let mut frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
    let sky = crate::sky_definition::default_sky()?;
    let settings = SkySettings::default();
    let mut renderer = TerrainCaptureRenderer::new(options.width, options.height)?;
    let sphere = Icosphere::new();

    for (name, time, view) in [
        ("sky-toward", 0.0, View::Galactic(DVec3::X)),
        ("sky-along", 86_400.0, View::Galactic(DVec3::Z)),
        ("sky-away", 0.0, View::Galactic(DVec3::Y)),
        ("sky-rotated", 0.0, View::Rotated),
        ("sky-viewport-origin", 0.0, View::Viewport),
        ("sky-return", 0.0, View::Galactic(DVec3::X)),
        ("sky-stars-only", 0.0, View::StarsOnly),
        ("sky-background-only", 0.0, View::BackgroundOnly),
        ("sky-seam", 0.0, View::Galactic(-DVec3::X)),
        ("sky-pole", 0.0, View::Galactic(DVec3::Y)),
        (
            "sky-nebula",
            0.0,
            View::Galactic(DVec3::new(-0.24, 0.035, 1.0)),
        ),
        (
            "sky-nebula-focus",
            0.0,
            View::Galactic(galactic_bearing(-0.24, 0.035)),
        ),
        (
            "sky-offband-upper",
            0.0,
            View::Galactic(galactic_bearing(0.28, 0.44)),
        ),
        (
            "sky-offband-lower",
            0.0,
            View::Galactic(galactic_bearing(1.50, -0.50)),
        ),
        (
            "sky-offband-warm",
            0.0,
            View::Galactic(galactic_bearing(-1.75, 0.32)),
        ),
        (
            "sky-dark-gap",
            0.0,
            View::Galactic(galactic_bearing(2.65, 0.60)),
        ),
        ("earth-day-on", 0.0, View::Earth(true, true)),
        ("earth-day-off", 0.0, View::Earth(true, false)),
        ("earth-night-on", 0.0, View::Earth(false, true)),
        ("earth-night-off", 0.0, View::Earth(false, false)),
        ("earth-silhouette", 0.0, View::Body(SolarBody::Earth)),
        ("moon-silhouette", 0.0, View::Body(SolarBody::Moon)),
        ("moon-airless", 0.0, View::MoonAirless),
    ] {
        motion.seek_seconds(time, &mut world)?;
        let started = Instant::now();
        frames.publish(&world)?;
        motion.frame_published(&world, started.elapsed());
        let pair = frames.coherent_view(&world)?;
        let projection = match view {
            View::Viewport => CelestialProjection::try_new(800, 540, 60_f64.to_radians(), 0.1)?
                .with_origin([71, 39])?,
            _ => CelestialProjection::try_new(
                options.width,
                options.height,
                60_f64.to_radians(),
                0.1,
            )?,
        };
        let fixture = make_pose(&pair, projection, view, settings)?;
        let capture = render_fixture(
            &mut renderer,
            &sphere,
            &pair,
            fixture,
            motion.snapshot(&world),
            Arc::clone(&sky),
            CaptureOutput {
                name,
                width: options.width,
                height: options.height,
                frame_number: 0,
            },
        )?;
        write_pair(output, &capture)?;
    }
    if options.sequence_frames > 0 {
        motion.seek_seconds(0.0, &mut world)?;
        let started = Instant::now();
        frames.publish(&world)?;
        motion.frame_published(&world, started.elapsed());
        for index in 0..options.sequence_frames {
            let timestamp_seconds = index as f64 / 30.0;
            motion.seek_seconds(timestamp_seconds, &mut world)?;
            let started = Instant::now();
            frames.publish(&world)?;
            motion.frame_published(&world, started.elapsed());
            let pair = frames.coherent_view(&world)?;
            let projection = CelestialProjection::try_new(
                options.width,
                options.height,
                60_f64.to_radians(),
                0.1,
            )?;
            let angle = (index as f64 * 0.08).to_radians();
            let root = pair.projection().tree().root();
            let pose = FramePose::new(
                FramePosition::new(
                    root,
                    LocalPosition::try_metres(DVec3::new(index as f64 * 1.0e12, 0.0, 0.0))?,
                ),
                look_rotation(
                    DQuat::from_rotation_y(angle)
                        * DQuat::from_rotation_z(0.4)
                        * DQuat::from_rotation_x(0.7)
                        * DVec3::X,
                )?,
            );
            let fixture = FixturePose {
                projection,
                pose,
                mode: CameraMode::FreeFlight,
                focused: None,
                selected: None,
                settings,
                body: None,
            };
            let name = format!("sky-motion-{index:04}");
            let capture = render_fixture(
                &mut renderer,
                &sphere,
                &pair,
                fixture,
                motion.snapshot(&world),
                Arc::clone(&sky),
                CaptureOutput {
                    name: &name,
                    width: options.width,
                    height: options.height,
                    frame_number: index as u64,
                },
            )?;
            write_pair(output, &capture)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum View {
    Galactic(DVec3),
    StarsOnly,
    BackgroundOnly,
    Rotated,
    Viewport,
    Earth(bool, bool),
    Body(SolarBody),
    MoonAirless,
}

struct FixturePose {
    projection: CelestialProjection,
    pose: FramePose,
    mode: CameraMode,
    focused: Option<BodyId>,
    selected: Option<BodyId>,
    settings: SkySettings,
    body: Option<SolarBody>,
}

fn make_pose(
    pair: &mundaris_world::CoherentCelestialView<'_>,
    projection: CelestialProjection,
    view: View,
    settings: SkySettings,
) -> Result<FixturePose> {
    let root = pair.projection().tree().root();
    let mut sky_settings = settings;
    let body = match view {
        View::Galactic(direction) => {
            return Ok(FixturePose {
                projection,
                pose: root_pose(root, direction, DQuat::IDENTITY)?,
                mode: CameraMode::FreeFlight,
                focused: None,
                selected: None,
                settings: sky_settings,
                body: None,
            });
        }
        View::StarsOnly | View::BackgroundOnly => {
            if matches!(view, View::StarsOnly) {
                sky_settings.background_intensity = 0.0;
            } else {
                sky_settings.star_intensity = 0.0;
            }
            return Ok(FixturePose {
                projection,
                pose: root_pose(root, DVec3::X, DQuat::IDENTITY)?,
                mode: CameraMode::FreeFlight,
                focused: None,
                selected: None,
                settings: sky_settings,
                body: None,
            });
        }
        View::Rotated => {
            return Ok(FixturePose {
                projection,
                pose: root_pose(root, DVec3::X, DQuat::from_rotation_y(0.15))?,
                mode: CameraMode::FreeFlight,
                focused: None,
                selected: None,
                settings: sky_settings,
                body: None,
            });
        }
        View::Viewport => {
            return Ok(FixturePose {
                projection,
                pose: root_pose(root, DVec3::X, DQuat::IDENTITY)?,
                mode: CameraMode::FreeFlight,
                focused: None,
                selected: None,
                settings: sky_settings,
                body: None,
            });
        }
        View::Earth(day, enabled) => {
            sky_settings.enabled = enabled;
            let id = body_id(pair, SolarBody::Earth)?;
            let fixed = pair.projection().frames_for(id)?.body_fixed;
            let radial = sun_direction(pair, id)? * if day { 1.0 } else { -1.0 };
            let rotation = look_outward(radial)?;
            let radius = pair.system().body(id)?.properties().reference_radius_m();
            let pose = FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(radial * (radius + 1000.0))?,
                ),
                rotation,
            );
            return Ok(FixturePose {
                projection,
                pose,
                mode: CameraMode::SurfaceInspection,
                focused: Some(id),
                selected: Some(id),
                settings: sky_settings,
                body: Some(SolarBody::Earth),
            });
        }
        View::Body(target) => target,
        View::MoonAirless => {
            let id = body_id(pair, SolarBody::Moon)?;
            let fixed = pair.projection().frames_for(id)?.body_fixed;
            let radial = sun_direction(pair, id)?;
            let radius = pair.system().body(id)?.properties().reference_radius_m();
            let pose = FramePose::new(
                FramePosition::new(
                    fixed,
                    LocalPosition::try_metres(radial * (radius + 1000.0))?,
                ),
                look_rotation(radial)?,
            );
            return Ok(FixturePose {
                projection,
                pose,
                mode: CameraMode::SurfaceInspection,
                focused: Some(id),
                selected: Some(id),
                settings: sky_settings,
                body: Some(SolarBody::Moon),
            });
        }
    };
    let id = body_id(pair, body)?;
    let fixed = pair.projection().frames_for(id)?.body_fixed;
    let radius = pair.system().body(id)?.properties().reference_radius_m();
    let radial = if body == SolarBody::Moon {
        sun_direction(pair, id)?
    } else {
        -sun_direction(pair, id)?
    };
    let pose = FramePose::new(
        FramePosition::new(fixed, LocalPosition::try_metres(radial * (radius * 3.0))?),
        look_rotation(-radial)?,
    );
    Ok(FixturePose {
        projection,
        pose,
        mode: CameraMode::SurfaceInspection,
        focused: Some(id),
        selected: Some(id),
        settings: sky_settings,
        body: Some(body),
    })
}

fn render_fixture(
    renderer: &mut TerrainCaptureRenderer,
    sphere: &Icosphere,
    pair: &mundaris_world::CoherentCelestialView<'_>,
    fixture: FixturePose,
    motion: MotionSnapshot,
    sky: Arc<mundaris_renderer::sky::SkyDefinition>,
    output: CaptureOutput<'_>,
) -> Result<DeveloperCapture> {
    let CaptureOutput {
        name,
        width,
        height,
        frame_number,
    } = output;
    let FixturePose {
        projection,
        pose,
        mode,
        focused,
        selected,
        settings,
        body,
    } = fixture;
    let root = pair.projection().tree().root();
    let bodies = if let Some(target) = body {
        let id = body_id(pair, target)?;
        let index = SOLAR_SYSTEM_CONTENT
            .iter()
            .position(|entry| entry.identity == target)
            .context("body content missing")?;
        let request = CelestialRenderBody {
            body_fixed_frame: pair.projection().frames_for(id)?.body_fixed,
            reference_radius_m: pair.system().body(id)?.properties().reference_radius_m(),
            color: SOLAR_SYSTEM_CONTENT[index].color,
            unlit: false,
            selected: false,
        };
        vec![(id, request)]
    } else {
        vec![]
    };
    let requests: Vec<_> = bodies.iter().map(|(_, request)| *request).collect();
    let view = PreparedView::new(
        &pair.evaluation(),
        pose,
        RenderPrecisionBudget::near_debug(),
    )?;
    let mut staging = CelestialStaging::default();
    let mut frame = CelestialFrame::new(&view, &mut staging, projection, sphere);
    frame.set_distant_sky(root, Arc::clone(&sky), settings)?;
    if let Some((id, request)) = bodies.first() {
        frame.append_bodies(&requests)?;
        let target = body.context("body rendering target missing")?;
        frame.set_terrain_lighting(TerrainLighting::try_new(
            sun_direction(pair, *id)?,
            0.06,
            0.94,
            TerrainRenderMode::Natural,
        )?);
        if let Some(mut config) =
            solar_system::planetary_config(target, request.reference_radius_m)?
        {
            // These low-level depth/atmosphere fixtures intentionally omit terrain,
            // ocean and clouds. Production terrain captures are retained separately.
            config.ocean_enabled = false;
            config.clouds_enabled = false;
            frame.set_planetary_environment(*request, config)?;
        }
    }
    let draw_report = frame.report();
    let rgba = renderer.render(&frame)?;
    let profile = renderer.last_gpu_profile();
    let population = TerrainPopulation::new()?;
    let snapshot_rendering = RenderingSnapshot {
        terrain_render_mode: "natural".into(),
        terrain_enabled: false,
        ocean_enabled: false,
        clouds_enabled: false,
        atmosphere_enabled: body == Some(SolarBody::Earth),
        ..Default::default()
    }
    .with_draw_report(draw_report);
    let mut snapshot = DeveloperSnapshot::collect(SnapshotInput {
        navigation: None,
        paused: motion.paused,
        simulation_speed: motion.rate,
        motion: Some(motion),
        pair,
        pose,
        camera_mode: mode,
        selected_body: selected,
        focused_body: focused,
        reference_body: focused,
        frame_number,
        projection,
        terrain: &population,
        terrain_clearance_m: None,
        drawn_mesh_clearance_m: None,
        rendering: snapshot_rendering,
        performance: PerformanceSnapshot::default().with_gpu(profile, "same_frame"),
    })?;
    snapshot.capture = Some(CaptureMetadata {
        scene: name.into(),
        image: format!("{name}.png"),
        width,
        height,
        terrain_updates: 0,
        worker_count: 0,
        step_ms: 0,
        morph_duration_ms: 0,
        adapter: renderer.adapter_name().into(),
        backend: renderer.adapter_backend().into(),
    });
    if let Some(report) = draw_report.sky {
        snapshot.sky = Some(crate::developer_snapshot::SkySnapshot::collect(
            &sky,
            settings,
            report,
            Some(renderer.last_sky_resource_report()),
            profile,
            SKY_RESOURCE_SCOPE,
            SKY_GPU_SCOPE,
        ));
    }
    Ok(DeveloperCapture { rgba, snapshot })
}

struct CaptureOutput<'a> {
    name: &'a str,
    width: u32,
    height: u32,
    frame_number: u64,
}

fn root_pose(root: FrameId, direction: DVec3, rotation: DQuat) -> Result<FramePose> {
    let galactic_direction = DQuat::from_rotation_z(0.4) * DQuat::from_rotation_x(0.7) * direction;
    let forward = rotation * galactic_direction;
    Ok(FramePose::new(
        FramePosition::new(root, LocalPosition::try_metres(DVec3::ZERO)?),
        look_rotation(forward)?,
    ))
}

fn galactic_bearing(longitude: f64, latitude: f64) -> DVec3 {
    let cos_latitude = latitude.cos();
    DVec3::new(
        cos_latitude * longitude.cos(),
        latitude.sin(),
        cos_latitude * longitude.sin(),
    )
}

fn body_id(pair: &mundaris_world::CoherentCelestialView<'_>, target: SolarBody) -> Result<BodyId> {
    let index = SOLAR_SYSTEM_CONTENT
        .iter()
        .position(|item| item.identity == target)
        .context("body identity missing")?;
    pair.system()
        .bodies()
        .nth(index)
        .map(|(id, _)| id)
        .context("body missing")
}
fn sun_direction(pair: &mundaris_world::CoherentCelestialView<'_>, id: BodyId) -> Result<DVec3> {
    let sun = pair
        .system()
        .bodies()
        .next()
        .context("Sun missing")?
        .1
        .state()
        .center_in_system()
        .metres();
    let center = pair.system().body(id)?.state().center_in_system().metres();
    Ok(pair
        .evaluation()
        .convert_direction(
            FrameDirection::new(
                pair.projection().tree().root(),
                Direction3::try_new(sun - center)?,
            ),
            pair.projection().frames_for(id)?.body_fixed,
        )?
        .local()
        .unit())
}
fn look_outward(radial: DVec3) -> Result<UnitRotation> {
    look_rotation(radial)
}
fn look_rotation(forward: DVec3) -> Result<UnitRotation> {
    let forward = forward.normalize();
    let right = forward.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
    let up = right.cross(forward).normalize();
    Ok(UnitRotation::try_from_quaternion(DQuat::from_mat3(
        &glam::DMat3::from_cols(right, up, -forward),
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn galactic_bearing_is_unit_direction_with_requested_latitude() {
        for (longitude, latitude) in [(0.28, 0.44), (1.50, -0.50), (-1.75, 0.32), (2.65, 0.60)] {
            let direction = galactic_bearing(longitude, latitude);
            assert!((direction.length() - 1.0).abs() < 1.0e-14);
            assert!((direction.y.asin() - latitude).abs() < 1.0e-14);
        }
    }

    #[test]
    #[ignore = "requires a real adapter; explicit daytime decorative-sky composition coverage"]
    fn daytime_suppression_and_night_visibility_use_identical_foreground_state() {
        let namespace = NonZeroU64::new(513131).unwrap();
        let (mut world, definition) = SolarSystemPreset::gameplay()
            .create_analytic(namespace)
            .unwrap();
        let mut motion = AnalyticSession::new(&mut world, definition).unwrap();
        motion.seek_seconds(0.0, &mut world).unwrap();
        let frames = mundaris_world::CelestialFrameProjection::build(&world, namespace).unwrap();
        motion.frame_published(&world, std::time::Duration::ZERO);
        let pair = frames.coherent_view(&world).unwrap();
        let sky = crate::sky_definition::default_sky().unwrap();
        let sphere = Icosphere::new();
        let projection =
            CelestialProjection::try_new(WIDTH, HEIGHT, 60_f64.to_radians(), 0.1).unwrap();
        let mut renderer = TerrainCaptureRenderer::new(WIDTH, HEIGHT)
            .expect("adapter required, never silently skipped");
        for day in [true, false] {
            let capture = |renderer: &mut TerrainCaptureRenderer, enabled| {
                let fixture = make_pose(
                    &pair,
                    projection,
                    View::Earth(day, enabled),
                    SkySettings::default(),
                )
                .unwrap();
                render_fixture(
                    renderer,
                    &sphere,
                    &pair,
                    fixture,
                    motion.snapshot(&world),
                    Arc::clone(&sky),
                    CaptureOutput {
                        name: "day-night-regression",
                        width: WIDTH,
                        height: HEIGHT,
                        frame_number: 0,
                    },
                )
                .unwrap()
            };
            let on = capture(&mut renderer, true);
            let off = capture(&mut renderer, false);
            assert!(
                on.snapshot.rendering.atmosphere_drawn && off.snapshot.rendering.atmosphere_drawn
            );
            assert_eq!(on.snapshot.general, off.snapshot.general);
            assert_eq!(on.snapshot.camera, off.snapshot.camera);
            assert_eq!(on.snapshot.terrain, off.snapshot.terrain);
            assert_eq!(on.snapshot.work, off.snapshot.work);
            assert_eq!(on.snapshot.memory, off.snapshot.memory);
            let maximum_difference = on
                .rgba
                .iter()
                .zip(&off.rgba)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            println!(
                "{} sky on/off max channel difference: {maximum_difference}/255",
                if day { "day" } else { "night" }
            );
            if day {
                assert!(
                    maximum_difference <= 1,
                    "daytime sky leaks through scattering"
                );
            } else {
                assert!(maximum_difference > 20, "night sky must remain visible");
            }
        }
    }
}
