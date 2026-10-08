//! Native first-look sky presentation. Usage: sky_native_capture <output-dir> <scene> [width height]

use std::{fs, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use glam::{DQuat, DVec3};
use mundaris_app::{
    celestial_camera::CameraMode,
    developer_snapshot::{
        CaptureMetadata, DeveloperSnapshot, PerformanceSnapshot, RenderingSnapshot, SkySnapshot,
        SnapshotInput,
    },
    sky_definition::default_sky,
    solar_system::SolarSystemPreset,
    terrain_population::TerrainPopulation,
};
use mundaris_math::*;
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    RenderPrecisionBudget, Renderer, sky::SkySettings,
};
use std::num::NonZeroU64;
use winit::{
    application::ApplicationHandler,
    dpi::{PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};

const READY_FRAMES: u8 = 3;

struct Options {
    output: PathBuf,
    scene: String,
    width: u32,
    height: u32,
    direction: DVec3,
}

struct App {
    options: Options,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    world: mundaris_world::CelestialSystem,
    frames: mundaris_world::CelestialFrameProjection,
    sky: Arc<mundaris_renderer::sky::SkyDefinition>,
    terrain: TerrainPopulation,
    sphere: Icosphere,
    staging: CelestialStaging,
    ready_frames: u8,
    error: Option<anyhow::Error>,
}

impl App {
    fn draw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer)) = (&self.window, &mut self.renderer) else {
            return;
        };
        let actual_size = window.inner_size();
        if actual_size.width != self.options.width || actual_size.height != self.options.height {
            return self.fail(
                event_loop,
                anyhow::anyhow!("native client dimensions differ from requested fixture"),
            );
        }
        let projection = match CelestialProjection::try_new(
            self.options.width,
            self.options.height,
            60_f64.to_radians(),
            0.1,
        ) {
            Ok(projection) => projection,
            Err(error) => return self.fail(event_loop, error.into()),
        };
        let pair = match self.frames.coherent_view(&self.world) {
            Ok(pair) => pair,
            Err(error) => return self.fail(event_loop, error.into()),
        };
        let root = pair.projection().tree().root();
        let pose = match root_pose(root, self.options.direction) {
            Ok(pose) => pose,
            Err(error) => return self.fail(event_loop, error),
        };
        let view = match PreparedView::new(
            &pair.evaluation(),
            pose,
            RenderPrecisionBudget::near_debug(),
        ) {
            Ok(view) => view,
            Err(error) => return self.fail(event_loop, error.into()),
        };
        let settings = SkySettings::default();
        let mut frame = CelestialFrame::new(&view, &mut self.staging, projection, &self.sphere);
        if let Err(error) = frame.set_distant_sky(root, Arc::clone(&self.sky), settings) {
            return self.fail(event_loop, error.into());
        }
        let report = frame.report();
        if let Err(error) = renderer.render_celestial(&frame, |_, _| {}) {
            return self.fail(event_loop, error.into());
        }
        self.ready_frames = self.ready_frames.saturating_add(1);
        if self.ready_frames == READY_FRAMES {
            let profile = renderer.latest_gpu_profile();
            let snapshot_result = (|| -> Result<DeveloperSnapshot> {
                let mut snapshot = DeveloperSnapshot::collect(SnapshotInput {
                    navigation: None,
                    paused: true,
                    simulation_speed: 0.0,
                    motion: None,
                    pair: &pair,
                    pose,
                    camera_mode: CameraMode::FreeFlight,
                    selected_body: None,
                    focused_body: None,
                    reference_body: None,
                    frame_number: u64::from(self.ready_frames),
                    projection,
                    terrain: &self.terrain,
                    terrain_clearance_m: None,
                    drawn_mesh_clearance_m: None,
                    rendering: RenderingSnapshot {
                        terrain_render_mode: "natural".into(),
                        terrain_enabled: false,
                        ocean_enabled: false,
                        clouds_enabled: false,
                        atmosphere_enabled: false,
                        ..Default::default()
                    },
                    performance: PerformanceSnapshot::default()
                        .with_gpu(profile, "latest_completed native"),
                })?;
                snapshot.sky = report.sky.map(|sky_report| {
                    SkySnapshot::collect(
                        &self.sky,
                        settings,
                        sky_report,
                        renderer.last_sky_resource_report(),
                        profile,
                        "latest_submitted native sky fixture",
                        "latest_completed native",
                    )
                });
                snapshot.capture = Some(CaptureMetadata {
                    scene: self.options.scene.clone(),
                    image: format!("{}.png", self.options.scene),
                    width: self.options.width,
                    height: self.options.height,
                    terrain_updates: 0,
                    worker_count: 0,
                    step_ms: 0,
                    morph_duration_ms: 0,
                    adapter: "unreported".into(),
                    backend: "unreported".into(),
                });
                Ok(snapshot)
            })();
            if let Err(error) = snapshot_result.and_then(|snapshot| {
                let json = serde_json::to_vec_pretty(&snapshot)?;
                fs::write(
                    self.options
                        .output
                        .join(format!("{}.json", self.options.scene)),
                    json,
                )?;
                fs::write(
                    self.options
                        .output
                        .join(format!("{}.ready", self.options.scene)),
                    b"presented\n",
                )?;
                Ok(())
            }) {
                self.fail(event_loop, error);
            }
        } else {
            window.request_redraw();
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(format!("Mundaris native sky — {}", self.options.scene))
            .with_decorations(false)
            .with_inner_size(PhysicalSize::new(self.options.width, self.options.height))
            .with_position(PhysicalPosition::new(0, 0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => return self.fail(event_loop, error.into()),
        };
        let renderer = match Renderer::new(Arc::clone(&window)) {
            Ok(renderer) => renderer,
            Err(error) => return self.fail(event_loop, error.into()),
        };
        self.window = Some(window);
        self.renderer = Some(renderer);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.as_ref().filter(|window| window.id() == id) else {
            return;
        };
        if let Some(renderer) = &mut self.renderer {
            let repaint = renderer.on_window_event(&event);
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
                WindowEvent::RedrawRequested => self.draw(event_loop),
                _ if repaint && self.ready_frames < READY_FRAMES => window.request_redraw(),
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }
}

fn root_pose(root: FrameId, direction: DVec3) -> Result<FramePose> {
    let galactic_direction = DQuat::from_rotation_z(0.4) * DQuat::from_rotation_x(0.7) * direction;
    let forward = galactic_direction.normalize();
    let right = forward.cross(DVec3::Y).try_normalize().unwrap_or(DVec3::X);
    let up = right.cross(forward).normalize();
    let rotation = UnitRotation::try_from_quaternion(DQuat::from_mat3(&glam::DMat3::from_cols(
        right, up, -forward,
    )))?;
    Ok(FramePose::new(
        FramePosition::new(root, LocalPosition::try_metres(DVec3::ZERO)?),
        rotation,
    ))
}

fn parse() -> Result<Options> {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .context("usage: sky_native_capture <output-dir> <toward|along|away> [width height]")?;
    let scene = args
        .next()
        .context("scene required: toward, along or away")?
        .to_string_lossy()
        .into_owned();
    let width = args
        .next()
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(2560);
    let height = args
        .next()
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(1440);
    anyhow::ensure!(
        args.next().is_none(),
        "usage: sky_native_capture <output-dir> <toward|along|away> [width height]"
    );
    anyhow::ensure!(
        (1..=8192).contains(&width) && (1..=8192).contains(&height),
        "dimensions must be within 1..=8192"
    );
    let direction = match scene.as_str() {
        "toward" => DVec3::X,
        "along" => DVec3::Z,
        "away" => DVec3::Y,
        _ => anyhow::bail!("scene must be toward, along, or away"),
    };
    Ok(Options {
        output,
        scene,
        width,
        height,
        direction,
    })
}

fn main() -> Result<()> {
    let options = parse()?;
    fs::create_dir_all(&options.output)?;
    for extension in ["png", "json", "ready"] {
        anyhow::ensure!(
            !options
                .output
                .join(format!("{}.{}", options.scene, extension))
                .exists(),
            "native capture refuses to replace existing scene evidence"
        );
    }
    let namespace = NonZeroU64::new(513_013).context("invalid fixture namespace")?;
    let (world, _) = SolarSystemPreset::gameplay().create_analytic(namespace)?;
    let frames = mundaris_world::CelestialFrameProjection::build(&world, namespace)?;
    let sky = default_sky()?;
    let mut app = App {
        options,
        window: None,
        renderer: None,
        world,
        frames,
        sky,
        terrain: TerrainPopulation::new()?,
        sphere: Icosphere::new(),
        staging: CelestialStaging::default(),
        ready_frames: 0,
        error: None,
    };
    let event_loop = EventLoop::new()?;
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
