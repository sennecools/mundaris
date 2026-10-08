//! Composition of authoritative physics, coherent projection and disposable debug views.
use crate::developer_snapshot::{
    DeveloperSnapshot, PerformanceSnapshot, RenderingSnapshot, SnapshotInput, format_bytes,
    format_distance, format_milliseconds,
};
use crate::motion_session::{AnalyticSession, MotionSession, MotionSnapshot};
use crate::{celestial_camera::*, trails::*};
use crate::{
    celestial_labels::*, celestial_selection::*, interactive_clock::*, orbit_guides::*,
    playback_metrics::*, system_view::*,
};
use anyhow::{Context, Result};
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{
    collections::VecDeque,
    num::NonZeroU64,
    time::{Duration, Instant},
};

enum Command {
    Visual(visual_controls::VisualCommand),
    LookBody(BodyId),
    SurfaceInspection,
    BodyOrbit,
    SurfaceHorizon,
    Clearance(f64),
    TerrainGuard(Option<f64>),
    Approach,
    Pause(bool),
    Rate(f64),
    ResumeAdmission,
    Single(bool),
    Seek(u64),
    SeekSeconds(f64),
    CancelSeek,
    Reset,
    Select(BodyId),
    Focus { fixed: bool, fit: bool },
    Overview,
    Subsystem,
    ReferenceOverview,
    FreeFlight,
    GuideReference(OrbitGuideReference),
    Navigation(NavigationInput),
    TimedNavigation(Instant, NavigationInput),
    CancelNavigation,
    GapThreshold(Duration),
    FilteredOverview(Vec<BodyId>),
    Reexpress(bool),
    Rebuild,
    Mass(f64),
    Radius(f64),
    Velocity(DVec3),
    Rename(String),
    TrailMode(bool),
    TrailReference(BodyId),
}
#[cfg(feature = "developer-tools")]
mod developer;
mod developer_ui;
mod frame_host;
mod navigation_input;
mod visual_controls;
struct Controls {
    performance_lab: crate::performance_lab::PerformanceLab,
    #[cfg(feature = "developer-tools")]
    automation_owner: Option<String>,
    #[cfg(feature = "developer-tools")]
    automation_stop: bool,
    snapshot_export_status: Option<String>,
    terrain_preview: bool,
    sun_from_star: bool,
    terrain_view: TerrainViewMode,
    approach: Option<(f64, f64, Duration)>,
    clearance_target: f64,
    terrain_guard_m: Option<f64>,
    pending: VecDeque<Command>,
    seek_seconds: f64,
    name: String,
    mass: String,
    radius: String,
    velocity: [String; 3],
    markers: bool,
    labels: bool,
    trails: bool,
    relative_trails: bool,
    guide_visible: bool,
    custom_rate: f64,
    all_history: bool,
    full_trails: bool,
    manual_speed: f64,
    navigation: NavigationInput,
    viewport_input: navigation_input::ViewportInput,
    input_focused: bool,
    pixels_per_point: f64,
    native_events: bool,
    keyboard_blocked: bool,
    ui_context: Option<egui::Context>,
    scene_layer: Option<egui::LayerId>,
    viewport_ready: bool,
    viewport: Option<([u32; 2], [u32; 2])>,
    layout: LabelLayout,
    placed: Vec<PlacedLabel>,
    pick_cycle: PickCycle,
    gesture_start: Option<egui::Pos2>,
    gesture_dragged: bool,
    gap_threshold_ms: f64,
    filter_ids: Vec<BodyId>,
}
impl Controls {
    fn new(body: &CelestialBody) -> Self {
        Self {
            performance_lab: {
                let mut lab = crate::performance_lab::PerformanceLab::default();
                lab.open = std::env::var("MUNDARIS_PERFORMANCE_LAB").is_ok_and(|s| s == "1");
                lab.enabled = lab.open || std::env::var("MUNDARIS_PROFILE").is_ok_and(|s| s == "1");
                crate::engine_profile::set_enabled(lab.enabled);
                crate::engine_profile::set_budget_ns("Frame", Some(100_000_000));
                crate::engine_profile::set_budget_ns("Adoption", Some(2_000_000));
                lab
            },
            #[cfg(feature = "developer-tools")]
            automation_owner: None,
            #[cfg(feature = "developer-tools")]
            automation_stop: false,
            snapshot_export_status: None,
            terrain_preview: false,
            sun_from_star: false,
            terrain_view: terrain_view_from_environment(),
            approach: None,
            clearance_target: 1e11,
            terrain_guard_m: None,
            pending: VecDeque::new(),
            seek_seconds: 0.0,
            name: body.name().into(),
            mass: body.properties().mass_kg().to_string(),
            radius: body.properties().reference_radius_m().to_string(),
            velocity: body
                .state()
                .center_velocity_in_system()
                .metres_per_second()
                .to_array()
                .map(|v| v.to_string()),
            markers: true,
            labels: true,
            trails: true,
            relative_trails: false,
            guide_visible: true,
            custom_rate: 10000.0,
            all_history: false,
            full_trails: false,
            manual_speed: 1.0,
            navigation: NavigationInput::default(),
            viewport_input: navigation_input::ViewportInput::default(),
            input_focused: true,
            pixels_per_point: 1.0,
            native_events: false,
            keyboard_blocked: false,
            ui_context: None,
            scene_layer: None,
            viewport_ready: false,
            viewport: None,
            layout: LabelLayout::default(),
            placed: Vec::new(),
            pick_cycle: PickCycle::default(),
            gesture_start: None,
            gesture_dragged: false,
            gap_threshold_ms: 250.0,
            filter_ids: Vec::new(),
        }
    }
    fn refresh_draft(&mut self, body: &CelestialBody) {
        self.name = body.name().into();
        self.mass = body.properties().mass_kg().to_string();
        self.radius = body.properties().reference_radius_m().to_string();
        self.velocity = body
            .state()
            .center_velocity_in_system()
            .metres_per_second()
            .to_array()
            .map(|v| v.to_string());
    }
}

pub struct GravityOrbitsDemo {
    profile_sampler: crate::performance_capture::ProfileSampler,
    performance_capture: crate::performance_capture::PerformanceCapture,
    profile_sample_at: Option<Instant>,
    profile_snapshot: Option<std::sync::Arc<serde_json::Value>>,
    diagnostic_capture_request: Option<String>,
    #[cfg(feature = "developer-tools")]
    developer_session: Option<String>,
    #[cfg(feature = "developer-tools")]
    developer_snapshot: Option<DeveloperSnapshot>,
    #[cfg(feature = "developer-tools")]
    #[cfg(feature = "developer-tools")]
    developer_navigation: Option<(Duration, NavigationInput)>,
    navigation_snapshot_path: Option<std::path::PathBuf>,
    navigation_wall_at: Option<Instant>,
    developer_frame_number: u64,
    terrain_clearance: Option<crate::terrain_inspection::TerrainClearance>,
    clearance_query_us: f64,
    atlas: crate::planet_lod::PlanetLod,
    presentation: Vec<crate::shared_system::BodyPresentation>,
    scene_sha256: String,
    camera_sha256: String,
    surface_owners: Vec<bool>,
    system: CelestialSystem,
    motion: MotionSession,
    projection: CelestialFrameProjection,
    camera: CelestialCamera,
    ids: Vec<BodyId>,
    selection: BodySelection,
    tree_namespace: u64,
    #[cfg(feature = "developer-tools")]
    system_namespace: u64,
    overview_center_m: DVec3,
    overview_extent_m: f64,
    trails: TrailHistory,
    requests: Vec<CelestialRenderBody>,
    staging: CelestialStaging,
    sphere: Icosphere,
    controls: Controls,
    diagnostic: Option<String>,
    coherent: bool,
    seeking: bool,
    last_wall: Option<Instant>,
    hidden: bool,
    diagnostics: Option<SystemDiagnostics>,
    baseline: Option<DiagnosticBaseline>,
    sampled_tick: u64,
    diagnostic_elapsed: Duration,
    advance: Option<SimulationAdvanceReport>,
    achieved_rate: Option<f64>,
    achieved_elapsed: Duration,
    achieved_anchor_s: f64,
    throughput_steps_s: Option<f64>,
    pump_ms: f64,
    clock: InteractiveClock,
    metrics: PlaybackMetrics,
    guides: OrbitGuides,
    scope: OverviewScope,
    bounds: SystemViewBounds,
    curves: Vec<VisualCurve>,
    history_fit_points: Vec<DVec3>,
    viewport: Option<([u32; 2], [u32; 2])>,
    cpu_limited: bool,
    count_limited: bool,
    auto_fit: bool,
    gap_diagnostic: Option<String>,
    trail_scratch: TrailDisplayScratch,
    coarse_curves: usize,
}
struct VisualCurve {
    points: Vec<FramePosition>,
    colors: Vec<[f32; 4]>,
    width: f32,
    style: CelestialLineStyle,
    relative: Vec<DVec3>,
}

/// Retain the 0.1 m floor and existing infinite reverse-Z depth path. Relief can
/// put drawn terrain much closer than reference-sphere altitude suggests.
fn inspection_near_plane(clearance_m: f64) -> f64 {
    if !clearance_m.is_finite() || clearance_m <= 0.0 || clearance_m == f64::MAX {
        0.1
    } else {
        (0.01 * clearance_m).max(0.1)
    }
}

/// Terrain view selected at launch; `MUNDARIS_TERRAIN_MODE` takes a view-mode name.
fn terrain_view_from_environment() -> TerrainViewMode {
    std::env::var("MUNDARIS_TERRAIN_MODE")
        .ok()
        .and_then(|name| TerrainViewMode::from_name(&name))
        .unwrap_or_default()
}

impl GravityOrbitsDemo {
    #[cfg(feature = "developer-tools")]
    pub(crate) fn take_diagnostic_capture_request(&mut self) -> Option<String> {
        self.diagnostic_capture_request.take()
    }

    /// Read-only authority and publication access for diagnostics/regression fixtures.
    pub fn world(&self) -> &CelestialSystem {
        &self.system
    }
    pub fn frames(&self) -> &CelestialFrameProjection {
        &self.projection
    }
    pub fn camera(&self) -> &CelestialCamera {
        &self.camera
    }
    pub fn motion_snapshot(&self) -> MotionSnapshot {
        self.motion.snapshot(&self.system)
    }
    pub fn trail_times_s(&self) -> Vec<f64> {
        self.trails.retained_times_s()
    }
    pub fn guides(&self) -> &[OrbitGuide] {
        self.guides.guides()
    }
    pub fn seek_seconds(&mut self, seconds: f64) -> Result<()> {
        self.command(Command::SeekSeconds(seconds))
    }
    pub fn set_playback_rate(&mut self, rate: f64) -> Result<()> {
        self.command(Command::Rate(rate))
    }
    pub fn set_paused(&mut self, paused: bool) -> Result<()> {
        self.command(Command::Pause(paused))
    }
    pub fn single_step(&mut self, forward: bool) -> Result<()> {
        self.command(Command::Single(forward))
    }
    pub fn reset_motion(&mut self) -> Result<()> {
        self.command(Command::Reset)
    }
    pub fn select_body(&mut self, body: BodyId) -> Result<()> {
        self.command(Command::Select(body))
    }
    pub fn focus_selected(&mut self, body_fixed: bool) -> Result<()> {
        self.command(Command::Focus {
            fixed: body_fixed,
            fit: true,
        })
    }
    pub fn enter_surface_navigation(&mut self) -> Result<()> {
        self.command(Command::SurfaceInspection)
    }
    pub fn edit_selected_mass(&mut self, mass_kg: f64) -> Result<()> {
        self.command(Command::Mass(mass_kg))
    }
    pub fn edit_selected_radius(&mut self, radius_m: f64) -> Result<()> {
        self.command(Command::Radius(radius_m))
    }
    pub fn edit_selected_velocity(&mut self, velocity: DVec3) -> Result<()> {
        self.command(Command::Velocity(velocity))
    }
    pub fn rename_selected(&mut self, name: &str) -> Result<()> {
        self.command(Command::Rename(name.into()))
    }
    pub fn set_guide_reference(&mut self, reference: OrbitGuideReference) -> Result<()> {
        self.command(Command::GuideReference(reference))
    }
    pub fn set_relative_trails(&mut self, relative: bool) -> Result<()> {
        self.command(Command::TrailMode(relative))
    }
    pub fn shared_test_system() -> Result<Self> {
        let system_namespace = 1;
        let tree_namespace = 1;
        let loaded = crate::shared_system::SharedTestSystem::load_canonical(
            NonZeroU64::new(system_namespace).context("zero system namespace")?,
        )?;
        let crate::shared_system::SharedTestSystem {
            mut system,
            lod,
            motion: definition,
            presentation,
            initial_body_index: selected,
            camera: camera_state,
            scene_sha256,
            camera_sha256,
        } = loaded;
        let mut analytic = AnalyticSession::new(&mut system, definition)?;
        analytic.set_rate(PlaybackRate::try_multiplier(camera_state.rate)?);
        analytic.set_paused(camera_state.paused);
        let motion = MotionSession::Analytic(Box::new(analytic));
        let projection = CelestialFrameProjection::build(
            &system,
            NonZeroU64::new(tree_namespace)
                .ok_or_else(|| anyhow::anyhow!("zero tree namespace"))?,
        )?;
        let diagnostics = if motion.is_analytic() {
            None
        } else {
            Some(system_diagnostics(&system)?)
        };
        let mut guides = OrbitGuides::default();
        match &motion {
            MotionSession::Analytic(a) => guides.update_analytic(&system, a.definition()),
            MotionSession::Newtonian(_) => guides.update(&system),
        }
        let bounds = SystemViewBounds::calculate(
            &system,
            &OverviewScope::WholeSystem,
            guides.guides(),
            &[],
            false,
        )?;
        let center = bounds.center_m();
        let extent = bounds.radius_m();
        let mut camera =
            CelestialCamera::overview(&projection.coherent_view(&system)?, center, extent)?;
        let ids: Vec<_> = system.bodies().map(|(id, _)| id).collect();
        let mut selection = BodySelection::default();
        selection.select(&system, ids[selected])?;
        let mut controls = Controls::new(system.body(ids[selected])?);
        controls.terrain_preview = true;
        controls.sun_from_star = true;
        controls.trails = false;
        controls.guide_visible = false;
        let advance = motion.newtonian().map(FixedStepRunner::report);
        let trails = TrailHistory::new(&system, 64)?;
        let pair = projection.coherent_view(&system)?;
        let body = ids[selected];
        let pose = FramePose::new(
            FramePosition::new(
                pair.projection().frames_for(body)?.body_fixed,
                LocalPosition::try_metres(DVec3::from_array(camera_state.position_body_m))?,
            ),
            UnitRotation::try_from_quaternion(glam::DQuat::from_array(
                camera_state.orientation_xyzw,
            ))?,
        );
        let clearance = crate::terrain_inspection::clearance_at_body_position(
            pair.system().body(body)?,
            pose.position().local().metres(),
            body,
        )?
        .context("canonical camera needs complete surface authority")?
        .clearance_m;
        anyhow::ensure!(
            clearance >= 1.0,
            "canonical camera requires at least 1 m clearance"
        );
        camera.developer_set_surface_pose(&pair, body, pose)?;
        camera.target_clearance(&pair, clearance)?;
        camera.enter_surface_inspection(&pair, body)?;
        Ok(Self {
            #[cfg(feature = "developer-tools")]
            developer_session: None,
            #[cfg(feature = "developer-tools")]
            developer_snapshot: None,
            #[cfg(feature = "developer-tools")]
            #[cfg(feature = "developer-tools")]
            developer_navigation: None,
            navigation_snapshot_path: std::env::var_os("MUNDARIS_NAVIGATION_SNAPSHOT")
                .map(std::path::PathBuf::from),
            navigation_wall_at: None,
            developer_frame_number: 0,
            performance_capture: crate::performance_capture::PerformanceCapture::from_environment(),
            profile_sample_at: None,
            profile_sampler: crate::performance_capture::ProfileSampler::default(),
            profile_snapshot: None,
            diagnostic_capture_request: None,
            terrain_clearance: None,
            clearance_query_us: 0.0,
            atlas: crate::planet_lod::PlanetLod::new(Some(lod)),
            presentation,
            scene_sha256,
            camera_sha256,
            surface_owners: Vec::new(),
            system,
            motion,
            projection,
            camera,
            ids,
            selection,
            tree_namespace,
            #[cfg(feature = "developer-tools")]
            system_namespace,
            overview_center_m: center,
            overview_extent_m: extent,
            trails,
            requests: Vec::new(),
            staging: CelestialStaging::default(),
            sphere: Icosphere::new(),
            controls,
            diagnostic: None,
            coherent: true,
            seeking: false,
            last_wall: None,
            hidden: false,
            diagnostics,
            baseline: diagnostics.map(DiagnosticBaseline),
            sampled_tick: 0,
            diagnostic_elapsed: Duration::ZERO,
            advance,
            achieved_rate: None,
            achieved_elapsed: Duration::ZERO,
            achieved_anchor_s: 0.0,
            throughput_steps_s: None,
            pump_ms: 0.0,
            clock: InteractiveClock::default(),
            metrics: PlaybackMetrics::default(),
            guides,
            scope: OverviewScope::WholeSystem,
            bounds,
            curves: Vec::new(),
            history_fit_points: Vec::new(),
            viewport: None,
            cpu_limited: false,
            count_limited: false,
            auto_fit: true,
            gap_diagnostic: None,
            trail_scratch: TrailDisplayScratch::default(),
            coarse_curves: 0,
        })
    }
    pub fn set_lifecycle_drawable(&mut self, drawable: bool) {
        self.clock.set_drawable(drawable);
        if !drawable || self.hidden {
            self.motion.set_lifecycle_suspended(!drawable);
            self.last_wall = None;
            self.hidden = !drawable;
            self.achieved_elapsed = Duration::ZERO;
            self.achieved_rate = None;
            self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
            self.advance = self.motion.newtonian().map(FixedStepRunner::report);
            self.metrics.reset();
            self.camera.cancel_transition();
            self.cancel_navigation_input();
        }
    }
    pub fn reset_wall_capture(&mut self) {
        self.last_wall = None;
        self.clock.reset_capture();
    }
    fn cancel_navigation_input(&mut self) {
        self.controls.viewport_input.cancel();
        self.controls.navigation = NavigationInput::default();
        self.controls.gesture_start = None;
        self.controls.gesture_dragged = false;
        self.controls.approach = None;
        self.controls
            .pending
            .retain(|c| !matches!(c, Command::Navigation(_) | Command::TimedNavigation(..)));
        self.navigation_wall_at = None;
        self.camera.cancel_transition();
    }
    fn automation_active(&self) -> bool {
        #[cfg(feature = "developer-tools")]
        {
            self.controls.automation_owner.is_some()
        }
        #[cfg(not(feature = "developer-tools"))]
        {
            false
        }
    }
    /// Native lifecycle hook; camera policy and cancellation remain app-owned.
    pub fn on_window_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        use winit::{
            event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent},
            keyboard::{KeyCode, PhysicalKey},
        };
        self.controls.native_events = true;
        match event {
            winit::event::WindowEvent::Focused(focused) => {
                self.controls.input_focused = *focused;
                self.cancel_navigation_input();
                self.reset_wall_capture();
            }
            winit::event::WindowEvent::Resized(_)
            | winit::event::WindowEvent::ScaleFactorChanged { .. } => {
                self.controls.viewport_ready = false;
                self.cancel_navigation_input();
                self.reset_wall_capture();
            }
            _ => {}
        }
        if !self.controls.input_focused || self.hidden || !self.controls.viewport_ready {
            return false;
        }
        if let WindowEvent::KeyboardInput { event, .. } = event
            && event.state == ElementState::Pressed
            && !event.repeat
            && self.controls.viewport_input.owns_keyboard()
            && !self
                .controls
                .ui_context
                .as_ref()
                .is_some_and(|c| c.wants_keyboard_input() || c.memory(|m| m.focused().is_some()))
        {
            let command = match event.physical_key {
                PhysicalKey::Code(KeyCode::KeyF) => Some(Command::Focus {
                    fixed: false,
                    fit: false,
                }),
                PhysicalKey::Code(KeyCode::KeyI) => Some(Command::SurfaceInspection),
                PhysicalKey::Code(KeyCode::KeyH) => Some(Command::SurfaceHorizon),
                PhysicalKey::Code(KeyCode::Home) => Some(Command::Overview),
                PhysicalKey::Code(KeyCode::Escape) => Some(Command::FreeFlight),
                PhysicalKey::Code(KeyCode::Tab) => {
                    let at = self.selected_index();
                    let next = if self.controls.viewport_input.boost_active() {
                        (at + self.ids.len() - 1) % self.ids.len()
                    } else {
                        (at + 1) % self.ids.len()
                    };
                    Some(Command::Select(self.ids[next]))
                }
                _ => None,
            };
            if let Some(command) = command {
                self.controls.pending.push_back(command);
                return true;
            }
        }
        let scale = self.controls.pixels_per_point;
        let point = |p: winit::dpi::PhysicalPosition<f64>| {
            egui::pos2((p.x / scale) as f32, (p.y / scale) as f32)
        };
        let translated = match event {
            WindowEvent::CursorMoved { position, .. } => {
                Some(egui::Event::PointerMoved(point(*position)))
            }
            WindowEvent::CursorLeft { .. } => Some(egui::Event::PointerGone),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Some(egui::PointerButton::Primary),
                    MouseButton::Right => Some(egui::PointerButton::Secondary),
                    _ => None,
                };
                button.and_then(|button| {
                    self.controls.viewport_input.pointer_position().map(|pos| {
                        egui::Event::PointerButton {
                            pos,
                            button,
                            pressed: *state == ElementState::Pressed,
                            modifiers: egui::Modifiers::default(),
                        }
                    })
                })
            }
            WindowEvent::MouseWheel { delta, .. } => Some(match delta {
                MouseScrollDelta::LineDelta(x, y) => egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(*x, *y),
                    modifiers: egui::Modifiers::default(),
                },
                MouseScrollDelta::PixelDelta(p) => egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2((p.x / scale) as f32, (p.y / scale) as f32),
                    modifiers: egui::Modifiers::default(),
                },
            }),
            WindowEvent::KeyboardInput { event, .. } => {
                let key = match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => Some(egui::Key::W),
                    PhysicalKey::Code(KeyCode::KeyS) => Some(egui::Key::S),
                    PhysicalKey::Code(KeyCode::KeyA) => Some(egui::Key::A),
                    PhysicalKey::Code(KeyCode::KeyD) => Some(egui::Key::D),
                    PhysicalKey::Code(KeyCode::KeyQ) => Some(egui::Key::Q),
                    PhysicalKey::Code(KeyCode::KeyE) => Some(egui::Key::E),
                    _ => None,
                };
                key.map(|key| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: event.state == ElementState::Pressed,
                    repeat: event.repeat,
                    modifiers: egui::Modifiers::default(),
                })
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.controls
                    .viewport_input
                    .set_boost(modifiers.state().shift_key());
                Some(egui::Event::Key {
                    key: egui::Key::F12,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers {
                        shift: modifiers.state().shift_key(),
                        ..Default::default()
                    },
                })
            }
            _ => None,
        };
        if let Some(event) = translated {
            let (origin, size) = self.controls.viewport.unwrap_or(([320, 110], [960, 662]));
            let rect = egui::Rect::from_min_size(
                egui::pos2(
                    (origin[0] as f64 / scale) as f32,
                    (origin[1] as f64 / scale) as f32,
                ),
                egui::vec2(
                    (size[0] as f64 / scale) as f32,
                    (size[1] as f64 / scale) as f32,
                ),
            );
            let context = self.controls.ui_context.clone();
            let layer = self.controls.scene_layer;
            let pointer_blocked = |p: egui::Pos2| {
                context.as_ref().is_some_and(|c| c.layer_id_at(p) != layer)
                    || self.controls.placed.iter().any(|label| {
                        label
                            .rect
                            .contains([(p.x as f64) * scale, (p.y as f64) * scale])
                    })
            };
            if let egui::Event::PointerButton {
                pos, pressed: true, ..
            } = event
                && rect.contains(pos)
                && !pointer_blocked(pos)
                && let Some(context) = &context
            {
                context.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            let blocked = context
                .as_ref()
                .map_or(self.controls.keyboard_blocked, |c| {
                    c.wants_keyboard_input() || c.memory(|m| m.focused().is_some())
                });
            let inputs = self.controls.viewport_input.events(
                &[event],
                rect,
                matches!(
                    self.camera.mode(),
                    CameraMode::SurfaceInspection | CameraMode::FreeFlight
                ),
                blocked,
                pointer_blocked,
                self.controls.manual_speed,
            );
            let at = Instant::now();
            for input in inputs {
                self.controls
                    .pending
                    .push_back(Command::TimedNavigation(at, input));
            }
        }
        false
    }
    fn advance_navigation_wall_to(&mut self, at: Instant) -> Result<()> {
        let elapsed = self.navigation_wall_at.map_or(Duration::ZERO, |previous| {
            at.saturating_duration_since(previous)
        });
        self.navigation_wall_at = Some(at);
        if !self.automation_active() {
            self.controls.navigation.speed_multiplier = self.controls.manual_speed;
        }
        if elapsed > self.clock.threshold() {
            self.cancel_navigation_input();
            return Ok(());
        }
        self.camera.update_navigation(
            &self.projection.coherent_view(&self.system)?,
            &self.controls.navigation,
            elapsed,
        )
    }
    fn seed_trails(&mut self) {
        if let Some(runner) = self.motion.newtonian() {
            self.trails
                .clear_and_seed(runner.branch_generation(), runner.tick(), &self.system);
        } else {
            self.trails.clear_and_seed_analytic(&self.system);
        }
    }
    fn reseed_diagnostics(&mut self) -> Result<()> {
        self.metrics.reset();
        if self.motion.is_analytic() {
            return Ok(());
        }
        self.diagnostics = Some(system_diagnostics(&self.system)?);
        self.baseline = self.diagnostics.map(DiagnosticBaseline);
        self.sampled_tick = self
            .motion
            .newtonian()
            .ok_or_else(|| anyhow::anyhow!("Newtonian diagnostics unavailable"))?
            .tick();
        self.diagnostic_elapsed = Duration::ZERO;
        self.achieved_elapsed = Duration::ZERO;
        self.achieved_rate = None;
        self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
        Ok(())
    }
    fn sample_diagnostics(&mut self) -> Result<()> {
        if self.motion.is_analytic() {
            return Ok(());
        }
        self.diagnostics = Some(system_diagnostics(&self.system)?);
        self.sampled_tick = self
            .motion
            .newtonian()
            .ok_or_else(|| anyhow::anyhow!("Newtonian diagnostics unavailable"))?
            .tick();
        self.diagnostic_elapsed = Duration::ZERO;
        Ok(())
    }
    fn refresh_report_metadata(&mut self) {
        if let (Some(runner), Some(previous)) = (self.motion.newtonian(), self.advance) {
            self.advance = Some(SimulationAdvanceReport {
                work_steps: previous.work_steps,
                forward_steps: previous.forward_steps,
                restored_steps: previous.restored_steps,
                force_passes: previous.force_passes,
                pair_evaluations: previous.pair_evaluations,
                ..runner.report()
            });
        }
    }
    fn update_guides(&mut self) {
        match &self.motion {
            MotionSession::Newtonian(_) => self.guides.update(&self.system),
            MotionSession::Analytic(a) => self.guides.update_analytic(&self.system, a.definition()),
        }
    }
    fn command(&mut self, command: Command) -> Result<()> {
        let id = self
            .selection
            .selected()
            .ok_or_else(|| anyhow::anyhow!("no selected body"))?;
        match command {
            Command::LookBody(target) => self
                .camera
                .look_at_body(&self.projection.coherent_view(&self.system)?, target)?,
            Command::SurfaceInspection => {
                anyhow::ensure!(
                    self.system.body(id)?.has_surface(),
                    "selected body has no surface capability"
                );
                self.camera
                    .enter_surface_inspection(&self.projection.coherent_view(&self.system)?, id)?;
                self.controls.approach = None;
            }
            Command::BodyOrbit => {
                self.camera
                    .enter_body_orbit(&self.projection.coherent_view(&self.system)?, id)?;
                self.controls.approach = None;
            }
            Command::SurfaceHorizon => self.camera.look_surface_horizon()?,
            Command::Clearance(clearance) => {
                self.camera
                    .target_clearance(&self.projection.coherent_view(&self.system)?, clearance)?;
                self.controls.approach = None;
            }
            Command::TerrainGuard(clearance) => {
                self.camera.set_terrain_clearance_guard(clearance)?;
                self.controls.terrain_guard_m = clearance;
            }
            Command::Approach => {
                anyhow::ensure!(
                    self.camera.mode() == CameraMode::BodyOrbit && !self.camera.transitioning(),
                    "complete focused body orbit before approach"
                );
                let pair = self.projection.coherent_view(&self.system)?;
                let body = self.camera.focused_body().expect("body orbit");
                let start =
                    crate::terrain_inspection::terrain_clearance(&pair, self.camera.pose(), body)?
                        .map_or(self.camera.measured_clearance(&pair, body)?, |c| {
                            c.clearance_m
                        });
                self.controls.approach = Some((start.max(2.0).ln(), 2.0_f64.ln(), Duration::ZERO));
            }
            Command::Pause(paused) => {
                self.motion.set_paused(paused);
                self.seeking = false;
                self.sample_diagnostics()?;
                self.metrics.reset();
                self.gap_diagnostic = None;
                self.last_wall = None;
            }
            Command::Rate(rate) => {
                let rate = PlaybackRate::try_multiplier(rate)?;
                if self.motion.rate().multiplier().signum() != rate.multiplier().signum() {
                    self.seed_trails();
                }
                self.motion.set_rate(rate);
                self.metrics.reset();
            }
            Command::ResumeAdmission => self.motion.newtonian_mut()?.resume_admission(),
            Command::Single(forward) => {
                self.metrics.reset();
                match &mut self.motion {
                    MotionSession::Newtonian(runner) => runner.single_step(forward)?,
                    MotionSession::Analytic(a) => {
                        let changed = a.single(forward, &mut self.system)?;
                        self.publish();
                        if changed && self.coherent {
                            self.trails.record_analytic_publication(&self.system);
                        }
                        self.update_guides();
                        self.reset_wall_capture();
                    }
                }
                self.seeking = false;
            }
            Command::Seek(tick) => {
                self.metrics.reset();
                self.motion.newtonian_mut()?.seek_tick(tick)?;
                self.seed_trails();
                self.seeking = tick != self.motion.newtonian_mut()?.tick();
                if !self.seeking {
                    self.sample_diagnostics()?;
                }
            }
            Command::SeekSeconds(seconds) => {
                match &mut self.motion {
                    MotionSession::Analytic(a) => {
                        a.seek_seconds(seconds, &mut self.system)?;
                    }
                    MotionSession::Newtonian(runner) => {
                        let preview = runner.quantize_seek_seconds(seconds)?;
                        return self.command(Command::Seek(preview.tick));
                    }
                }
                self.publish();
                if self.coherent {
                    self.seed_trails();
                }
                self.cancel_navigation_input();
                self.reset_wall_capture();
                self.seeking = false;
                self.update_guides();
            }
            Command::CancelSeek => {
                self.metrics.reset();
                self.motion.newtonian_mut()?.cancel_seek();
                self.seeking = false;
                self.sample_diagnostics()?;
            }
            Command::Reset => {
                self.metrics.reset();
                match &mut self.motion {
                    MotionSession::Newtonian(runner) => runner.reset_branch(&mut self.system)?,
                    MotionSession::Analytic(a) => {
                        a.reset(&mut self.system)?;
                        self.publish();
                        self.cancel_navigation_input();
                        self.reset_wall_capture();
                    }
                }
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }

            Command::Select(body) => {
                self.selection.select(&self.system, body)?;
                self.controls.refresh_draft(self.system.body(body)?);
            }
            Command::Focus { fixed, fit } => {
                let pair = self.projection.coherent_view(&self.system)?;
                if fixed {
                    self.camera.focus(&pair, id, true, fit)?;
                } else if fit {
                    let projection = self.content_projection(0.1)?;
                    let [w, h] = projection.viewport();
                    self.camera.fit_body(
                        &pair,
                        id,
                        projection.vertical_fov_rad(),
                        w as f64 / h as f64,
                    )?;
                } else {
                    self.camera.transition_to(&pair, FocusTarget::Body(id))?;
                }
                self.auto_fit = false;
            }
            Command::Overview => {
                self.scope = OverviewScope::WholeSystem;
                self.refit(true)?;
            }
            Command::Subsystem => {
                self.scope = OverviewScope::SelectedSubsystem(id);
                self.refit(true)?;
            }
            Command::ReferenceOverview => {
                let reference = self
                    .guides
                    .guides()
                    .iter()
                    .find(|g| g.body == id)
                    .and_then(|g| g.reference)
                    .unwrap_or(id);
                self.scope = OverviewScope::SelectedSubsystem(reference);
                self.refit(true)?;
            }
            Command::FreeFlight => {
                self.camera
                    .enter_free_flight(&self.projection.coherent_view(&self.system)?)?;
                self.auto_fit = false;
            }
            Command::GuideReference(reference) => {
                self.guides.set_reference(&self.system, id, reference)?;
                self.update_guides();
            }
            Command::Navigation(input) => {
                if input.drag != [0.0; 2]
                    || input.scroll_notches != 0.0
                    || input.translation != DVec3::ZERO
                {
                    self.auto_fit = false;
                    self.controls.approach = None;
                }
                let projection = self.content_projection(0.1)?;
                self.camera
                    .set_navigation_projection(projection, self.controls.pixels_per_point)?;
                if input.drag != [0.0; 2] || input.scroll_notches != 0.0 {
                    self.camera.update_navigation(
                        &self.projection.coherent_view(&self.system)?,
                        &NavigationInput {
                            translation: DVec3::ZERO,
                            ..input
                        },
                        Duration::ZERO,
                    )?;
                }
                self.controls.navigation = NavigationInput {
                    drag: [0.0; 2],
                    scroll_notches: 0.0,
                    ..input
                };
            }
            Command::TimedNavigation(at, input) => {
                self.advance_navigation_wall_to(at)?;
                self.command(Command::Navigation(input))?;
            }
            Command::CancelNavigation => self.cancel_navigation_input(),
            Command::GapThreshold(threshold) => {
                anyhow::ensure!(
                    self.clock.set_threshold(threshold),
                    "clock threshold must be positive"
                );
                self.metrics.reset();
                self.last_wall = None;
                self.clock.reset_capture();
            }
            Command::FilteredOverview(mut ids) => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
                self.scope = OverviewScope::ExplicitBodies(ids);
                self.refit(true)?;
            }
            Command::Reexpress(fixed) => self.camera.reexpress(
                &self.projection.coherent_view(&self.system)?,
                if fixed {
                    CameraAttachment::BodyFixed(id)
                } else {
                    CameraAttachment::Translating(id)
                },
            )?,
            Command::Rebuild => {
                let namespace = self
                    .tree_namespace
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("tree namespace overflow"))?;
                let projection = CelestialFrameProjection::build(
                    &self.system,
                    NonZeroU64::new(namespace).expect("checked nonzero namespace"),
                )?;
                self.camera.remap_projection(&projection)?;
                self.projection = projection;
                self.tree_namespace = namespace;
            }
            Command::Mass(mass) => {
                let radius = self.system.body(id)?.properties().reference_radius_m();
                let properties = BodyProperties::new(mass, radius)?;
                match &mut self.motion {
                    MotionSession::Newtonian(runner) => {
                        runner.edit_properties(&mut self.system, id, properties)?
                    }
                    MotionSession::Analytic(a) => {
                        a.edit_properties(&mut self.system, id, properties)?
                    }
                }
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Radius(radius) => {
                let mass = self.system.body(id)?.properties().mass_kg();
                let properties = BodyProperties::new(mass, radius)?;
                match &mut self.motion {
                    MotionSession::Newtonian(runner) => {
                        runner.edit_properties(&mut self.system, id, properties)?
                    }
                    MotionSession::Analytic(a) => {
                        a.edit_properties(&mut self.system, id, properties)?
                    }
                }
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Velocity(velocity) => {
                anyhow::ensure!(
                    !self.motion.is_analytic(),
                    "Velocity edits are unavailable in prescribed motion: velocity is the authored trajectory derivative. "
                );
                let old = *self.system.body(id)?.state();
                let state = BodyState::new(
                    old.center_in_system(),
                    LinearVelocity3::try_metres_per_second(velocity)?,
                    old.body_to_system(),
                    old.angular_velocity_in_system(),
                );
                self.motion
                    .newtonian_mut()?
                    .edit_state(&mut self.system, id, state)?;
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Rename(name) => match &mut self.motion {
                MotionSession::Newtonian(runner) => runner.rename(&mut self.system, id, &name)?,
                MotionSession::Analytic(a) => a.rename(&mut self.system, id, &name)?,
            },
            Command::Visual(command) => command.apply(&mut self.controls)?,
            Command::TrailMode(relative) => {
                let mode = if relative {
                    TrailMode::SimultaneousBodyRelative(id)
                } else {
                    TrailMode::Inertial
                };
                self.trails.set_mode(mode, 0, 0, &self.system)?;
                self.controls.relative_trails = relative;
            }
            Command::TrailReference(reference) => {
                self.trails.set_mode(
                    TrailMode::SimultaneousBodyRelative(reference),
                    0,
                    0,
                    &self.system,
                )?;
                self.controls.relative_trails = true;
            }
        }
        self.advance = self.motion.newtonian().map(FixedStepRunner::report);
        Ok(())
    }
    fn publish(&mut self) {
        let started = Instant::now();
        let changed = self.projection.represented_revision() != self.system.revision();
        let result = (|| -> Result<()> {
            if self.projection.represented_revision() != self.system.revision() {
                self.projection.publish(&self.system)?;
            }
            self.projection.coherent_view(&self.system)?;
            Ok(())
        })();
        self.coherent = result.is_ok();
        if let Err(error) = result {
            self.motion.set_paused(true);
            if let MotionSession::Analytic(a) = &mut self.motion {
                a.frame_failed(&error);
            }
            self.diagnostic = Some(format!(
                "Projection failed; celestial drawing suppressed: {error}. Rebuild projection to retry."
            ));
        } else if let MotionSession::Analytic(a) = &mut self.motion
            && (changed || a.published_time() != self.system.sample_time())
        {
            a.frame_published(&self.system, started.elapsed());
        }
    }
    /// Host duration is explicit and captured once. Surface retries never reuse it.
    pub fn update(&mut self, elapsed: Duration) {
        #[cfg(feature = "developer-tools")]
        if let Some((remaining, input)) = self.developer_navigation.take() {
            self.controls.navigation = if remaining.is_zero() {
                NavigationInput::default()
            } else {
                input
            };
            if remaining > elapsed {
                self.developer_navigation = Some((remaining - elapsed, input));
            } else if !remaining.is_zero() {
                self.developer_navigation = Some((Duration::ZERO, input));
            }
        }
        while let Some(command) = self.controls.pending.pop_front() {
            match self.command(command) {
                Ok(()) => self.diagnostic = None,
                Err(error) => self.diagnostic = Some(error.to_string()),
            }
        }
        let elapsed = if self.hidden { Duration::ZERO } else { elapsed };
        let elapsed = if elapsed > self.clock.threshold() {
            self.motion.set_paused(true);
            self.metrics.reset();
            self.camera.cancel_transition();
            self.controls.navigation = NavigationInput::default();
            self.controls.approach = None;
            self.cancel_navigation_input();
            self.gap_diagnostic = Some(format!(
                "Interactive clock gap {:.3} wall s; no catch-up requested. Pending demand cancelled. Resume explicitly.",
                elapsed.as_secs_f64()
            ));
            self.last_wall = None;
            Duration::ZERO
        } else {
            elapsed
        };
        if !self.coherent {
            self.publish();
            if !self.coherent {
                self.advance = self.motion.newtonian().map(FixedStepRunner::report);
                return;
            }
        }
        if self.motion.is_analytic() {
            let sampled = match &mut self.motion {
                MotionSession::Analytic(a) => a.advance(elapsed, &mut self.system),
                MotionSession::Newtonian(_) => unreachable!("mode checked"),
            };
            match sampled {
                Ok(changed) => {
                    self.publish();
                    if changed && self.coherent {
                        self.trails.record_analytic_publication(&self.system);
                    }
                }
                Err(error) => {
                    self.diagnostic = Some(error.to_string());
                    self.cancel_navigation_input();
                }
            }
        } else {
            let runner = match &mut self.motion {
                MotionSession::Newtonian(r) => r,
                _ => unreachable!("mode checked"),
            };
            if let Err(error) = runner.admit_wall_elapsed(elapsed) {
                runner.set_paused(true);
                self.diagnostic = Some(error.to_string());
            }
            let branch = runner.branch_generation();
            let seeking = self.seeking;
            let measuring = !runner.paused() && !self.hidden && !seeking;
            let authority_before = self.system.sample_time().seconds_since_epoch();
            let replay_before = runner.report().replay_remaining.is_some();
            let trails = &mut self.trails;
            let started = Instant::now();
            let mut total = [0u64; 5];
            let mut limit = 1;
            let cap = runner.config().work_limit().min(512);
            self.cpu_limited = false;
            self.count_limited = false;
            let result = loop {
                let result = runner.pump_with_work_limit(&mut self.system, limit, |tick, world| {
                    if !seeking {
                        trails.record_committed(branch, tick, world);
                    }
                });
                let report = match &result {
                    Ok(report) => *report,
                    Err(error) => *error.report,
                };
                total[0] += u64::from(report.work_steps);
                total[1] += u64::from(report.forward_steps);
                total[2] += u64::from(report.restored_steps);
                total[3] += u64::from(report.force_passes);
                total[4] += report.pair_evaluations;
                if result.is_err()
                    || report.work_steps == 0
                    || (report.backlog_ticks == 0 && report.replay_remaining.is_none())
                {
                    break result;
                }
                if total[0] >= u64::from(cap) {
                    self.count_limited = true;
                    break result;
                }
                if started.elapsed() >= Duration::from_millis(4) {
                    self.cpu_limited = true;
                    break result;
                }
                let remaining = cap - total[0] as u32;
                let per = started.elapsed().as_secs_f64() / total[0] as f64;
                let affordable = ((0.004 - started.elapsed().as_secs_f64()) / per.max(1e-9))
                    .floor()
                    .max(1.0) as u32;
                limit = remaining.min(32).min(affordable);
            };
            self.pump_ms = started.elapsed().as_secs_f64() * 1000.0;
            let mut advance = match result {
                Ok(report) => report,
                Err(error) => {
                    self.diagnostic = Some(error.to_string());
                    self.seeking = false;
                    *error.report
                }
            };
            advance.work_steps = total[0] as u32;
            advance.forward_steps = total[1] as u32;
            advance.restored_steps = total[2] as u32;
            advance.force_passes = total[3] as u32;
            advance.pair_evaluations = total[4];
            self.advance = Some(advance);
            if advance.work_steps > 0 && self.pump_ms > 0.0 {
                self.throughput_steps_s = Some(advance.work_steps as f64 * 1000.0 / self.pump_ms);
            }
            if self.seeking && advance.backlog_ticks == 0 && advance.replay_remaining.is_none() {
                self.seed_trails();
                self.seeking = false;
                if let Err(error) = self.sample_diagnostics() {
                    self.diagnostic = Some(error.to_string());
                }
                self.achieved_elapsed = Duration::ZERO;
                self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
                self.achieved_rate = None;
            }
            self.publish();
            let private_work = advance.work_steps > advance.forward_steps + advance.restored_steps;
            if measuring && !replay_before && !private_work && advance.replay_remaining.is_none() {
                self.metrics.record(
                    elapsed,
                    self.system.sample_time().seconds_since_epoch() - authority_before,
                );
            } else if replay_before || private_work || advance.replay_remaining.is_some() {
                self.metrics.reset();
            }
            self.achieved_rate = self
                .metrics
                .measurement(60.0, self.motion.rate().multiplier())
                .map(|m| m.achieved_rate);
        }
        self.update_guides();
        if self.coherent {
            if self.controls.native_events
                && let Err(error) = self.advance_navigation_wall_to(Instant::now())
            {
                self.diagnostic = Some(error.to_string());
            }
            if let Some((start, end, at)) = &mut self.controls.approach {
                *at = at.saturating_add(elapsed);
                let t = (at.as_secs_f64() / 30.0).clamp(0.0, 1.0);
                let target = (*start * (1.0 - t) + *end * t).exp();
                if let Err(error) = self.camera.target_clearance(
                    &self
                        .projection
                        .coherent_view(&self.system)
                        .expect("coherent"),
                    target,
                ) {
                    self.diagnostic = Some(error.to_string());
                    self.controls.approach = None;
                }
            }
            let input = self.controls.navigation;
            let envelope_adjustment = self.camera.mode() == CameraMode::BodyOrbit
                && !self.camera.transitioning()
                && self
                    .camera
                    .focused_body()
                    .and_then(|id| self.system.body(id).ok())
                    .is_some_and(|body| {
                        let radius = body.properties().reference_radius_m();
                        minimum_clearance(radius)
                            .is_ok_and(|clearance| self.camera.distance_m() < radius + clearance)
                    });
            if let Err(error) = self.camera.update_navigation(
                &self
                    .projection
                    .coherent_view(&self.system)
                    .expect("coherence checked"),
                &input,
                if self.controls.native_events {
                    Duration::ZERO
                } else {
                    elapsed
                },
            ) {
                self.camera.cancel_transition();
                self.diagnostic = Some(error.to_string());
            } else if envelope_adjustment {
                self.diagnostic=Some("Reference radius grew around the observer; only the observer was moved outward to the reference-sphere clearance envelope.".into());
            }
            self.controls.navigation.drag = [0.0; 2];
            self.controls.navigation.scroll_notches = 0.0;
            if self.camera.mode() == CameraMode::SystemOrbit && !self.camera.transitioning() {
                let result = (|| -> Result<()> {
                    self.bounds = SystemViewBounds::calculate(
                        &self.system,
                        &self.scope,
                        if self.controls.guide_visible {
                            self.guides.guides()
                        } else {
                            &[]
                        },
                        &self.history_fit_points,
                        self.controls.all_history,
                    )?;
                    let distance = self.bounds.fit_distance_m(self.content_projection(0.1)?)?;
                    self.camera.track_overview(
                        self.bounds.center_m(),
                        distance,
                        self.auto_fit,
                        elapsed,
                    )
                })();
                if let Err(error) = result {
                    self.diagnostic = Some(error.to_string());
                }
            }
        }
        if self.coherent
            && let Err(error) = self
                .projection
                .coherent_view(&self.system)
                .map_err(anyhow::Error::new)
                .and_then(|pair| self.camera.refresh_navigation_constraint(&pair))
        {
            self.motion.set_paused(true);
            self.diagnostic = Some(error.to_string());
        }
        self.diagnostic_elapsed = self.diagnostic_elapsed.saturating_add(elapsed);
        if (self.diagnostic_elapsed >= Duration::from_millis(250)
            || (self.motion.paused() && self.advance.is_some_and(|a| a.work_steps > 0)))
            && let Err(error) = self.sample_diagnostics()
        {
            self.motion.set_paused(true);
            self.diagnostic = Some(error.to_string());
        }
        self.refresh_report_metadata();
    }
    // Opt-in native exercise of the ordinary Solar controls, not a second
    // renderer. Default launches remain unmodified; hidden/gap time is excluded.

    fn selected_index(&self) -> usize {
        self.ids
            .iter()
            .position(|id| Some(*id) == self.selection.selected())
            .expect("validated selected identity")
    }
    fn content_projection(&self, near: f64) -> Result<CelestialProjection> {
        let (origin, size) = self.viewport.unwrap_or(([320, 110], [960, 662]));
        Ok(
            CelestialProjection::try_new(size[0], size[1], 60.0_f64.to_radians(), near)?
                .with_origin(origin)?,
        )
    }
    fn refit(&mut self, transition: bool) -> Result<()> {
        self.bounds = SystemViewBounds::calculate(
            &self.system,
            &self.scope,
            if self.controls.guide_visible {
                self.guides.guides()
            } else {
                &[]
            },
            &self.history_fit_points,
            self.controls.all_history,
        )?;
        let distance = self.bounds.fit_distance_m(self.content_projection(0.1)?)?;
        self.overview_center_m = self.bounds.center_m();
        self.overview_extent_m = self.bounds.radius_m();
        let pair = self.projection.coherent_view(&self.system)?;
        if transition {
            self.camera
                .fit_overview(&pair, self.bounds.center_m(), distance)?;
        } else {
            self.camera = CelestialCamera::overview(&pair, self.bounds.center_m(), distance / 2.5)?;
            let mut normal = DVec3::ZERO;
            let maximum = self
                .guides
                .guides()
                .iter()
                .filter_map(|g| g.elements.and_then(|e| e.semi_major_axis_m()))
                .fold(1.0, f64::max);
            for guide in self.guides.guides() {
                if let Some(e) = guide.elements
                    && let Some(a) = e.semi_major_axis_m()
                {
                    let n = if e.normal().z < 0.0 {
                        -e.normal()
                    } else {
                        e.normal()
                    };
                    normal += n * (a / maximum).powi(2);
                }
            }
            self.camera
                .set_overview_direction(if normal.length() > 1e-12 {
                    normal
                } else {
                    DVec3::Z
                })?;
        }
        self.auto_fit = true;
        Ok(())
    }
    pub fn render(&mut self, renderer: &mut Renderer, width: u32, height: u32) -> Result<()> {
        self.render_host(
            &mut frame_host::FrameHost::Native(renderer),
            width,
            height,
            None,
        )
    }
    fn render_host(
        &mut self,
        renderer: &mut frame_host::FrameHost<'_>,
        width: u32,
        height: u32,
        fixed_elapsed: Option<Duration>,
    ) -> Result<()> {
        #[cfg(feature = "developer-tools")]
        if self.developer_session.is_some() {
            self.developer_snapshot = None;
        }
        if width == 0 || height == 0 {
            self.set_lifecycle_drawable(false);
            return Ok(());
        }
        self.set_lifecycle_drawable(true);
        self.developer_frame_number = self.developer_frame_number.saturating_add(1);
        crate::engine_profile::set_enabled(self.controls.performance_lab.enabled);
        if crate::engine_profile::is_enabled() {
            renderer.request_profile_timing();
        }
        crate::engine_profile::begin_frame(self.developer_frame_number);
        let frame_span = crate::engine_profile::span("Frame");
        let now = Instant::now();
        let elapsed = self
            .last_wall
            .map_or(Duration::ZERO, |previous| now.duration_since(previous));
        self.last_wall = Some(now);
        let native_interval_ms = elapsed.as_secs_f64() * 1000.0;
        let host_frame_started = Instant::now();
        let elapsed = fixed_elapsed.unwrap_or_else(|| match self.clock.classify(elapsed) {
            ClockInterval::Accepted(elapsed) => elapsed,
            ClockInterval::Hidden => Duration::ZERO,
            ClockInterval::Discontinuity(gap) => gap,
        });
        #[cfg(feature = "surface-profile")]
        let update_started = Instant::now();
        {
            let _span = crate::engine_profile::span("Simulation");
            self.update(elapsed);
        }
        #[cfg(feature = "surface-profile")]
        let update_ms = update_started.elapsed().as_secs_f64() * 1000.0;
        if !self.coherent {
            let (info, controls) = self.ui_info(CelestialPreparationReport::default(), 0.0, 0.0);
            renderer.render(|ctx| draw_ui(ctx, controls, &info, &[]))?;
            return Ok(());
        }
        let scale = renderer.pixels_per_point() as f64;
        let default_origin = [
            (320.0 * scale).round() as u32,
            (110.0 * scale).round() as u32,
        ];
        let default_size = [
            width.saturating_sub(default_origin[0]).max(1),
            height
                .saturating_sub(default_origin[1] + (28.0 * scale).round() as u32)
                .max(1),
        ];
        let (mut origin, mut size) = self
            .controls
            .viewport
            .unwrap_or((default_origin, default_size));
        origin[0] = origin[0].min(width - 1);
        origin[1] = origin[1].min(height - 1);
        size[0] = size[0].min(width - origin[0]).max(1);
        size[1] = size[1].min(height - origin[1]).max(1);
        let new_viewport = Some((origin, size));
        if self.viewport != new_viewport {
            let first = self.viewport.is_none();
            self.viewport = new_viewport;
            if self.camera.mode() == CameraMode::SystemOrbit
                && let Err(error) = self.refit(!first)
            {
                self.diagnostic = Some(error.to_string());
            }
        }
        let started = Instant::now();
        let preparation_span = crate::engine_profile::span("Renderer preparation");
        self.prepare_visuals()
            .context("orbit visual preparation failed")?;
        let selected_index = self.selected_index();
        let pair = self.projection.coherent_view(&self.system)?;
        self.camera
            .set_navigation_projection(self.content_projection(0.1)?, scale)?;
        self.controls.pixels_per_point = scale;
        self.terrain_clearance = self.camera.recorded_terrain_clearance();
        self.clearance_query_us = self.camera.navigation_diagnostics().terrain_query_us;
        let view = PreparedView::new(
            &pair.evaluation(),
            self.camera.pose(),
            RenderPrecisionBudget::near_debug(),
        )
        .context("preparing source-centered render view after fixture camera placement")?;
        self.requests.clear();
        let mut clearance = f64::MAX;
        for (index, (id, body)) in pair.system().bodies().enumerate() {
            let frames = pair.projection().frames_for(id)?;
            let radius = body.properties().reference_radius_m();
            let center = view
                .prepare_source(frames.body_fixed)?
                .view_displacement(FramePosition::new(
                    frames.body_fixed,
                    LocalPosition::origin(),
                ))?
                .metres();
            let surface = center.x.hypot(center.y).hypot(center.z) - radius;
            clearance = clearance.min(surface);
            self.requests.push(CelestialRenderBody {
                body_fixed_frame: frames.body_fixed,
                reference_radius_m: radius,
                color: self.presentation[index].color,
                unlit: self.presentation[index].unlit,
                selected: index == selected_index,
            });
        }
        if let Some(terrain) = self.terrain_clearance {
            clearance = clearance.min(terrain.clearance_m);
        }
        let near = inspection_near_plane(clearance);
        let projection = self.content_projection(near)?;
        self.surface_owners.clear();
        self.surface_owners.resize(self.requests.len(), false);
        let mut frame = CelestialFrame::new(&view, &mut self.staging, projection, &self.sphere);
        if self.controls.terrain_preview {
            let _span = crate::engine_profile::span("Atlas terrain preparation");
            self.atlas
                .receive_bounds(renderer.take_terrain_atlas_bounds());
            let star = pair.system().body(self.ids[0])?;
            let mut inputs = Vec::new();
            for &id in &self.ids {
                let body = pair.system().body(id)?;
                let Some(definition) = body.surface_definition() else {
                    continue;
                };
                let sun_body = body
                    .state()
                    .body_to_system()
                    .inverse()
                    .rotate_direction(Direction3::try_new(
                        star.state().center_in_system().metres()
                            - body.state().center_in_system().metres(),
                    )?)?
                    .unit();
                inputs.push(crate::planet_lod::AtlasBodyInput {
                    body: id,
                    body_fixed_frame: pair.projection().frames_for(id)?.body_fixed,
                    definition,
                    radius_m: body.properties().reference_radius_m(),
                    revision: body.terrain_revision().value(),
                    sun_body,
                });
            }
            let atlas_frame = self.atlas.prepare(
                &view,
                projection,
                &inputs,
                self.controls.terrain_view.shader_mode(),
                renderer.terrain_atlas_layer_limit(),
            )?;
            for body in self.atlas.drawn_bodies() {
                if let Some(index) = self.ids.iter().position(|id| id == body) {
                    self.surface_owners[index] = true;
                }
            }
            frame.set_terrain_atlas(atlas_frame);
        }
        let prepared = (|| -> Result<()> {
            frame.append_body_observations(&self.requests, &self.surface_owners)?;
            for curve in &self.curves {
                frame.append_polylines(&[CelestialPolyline {
                    points: &curve.points,
                    colors: &curve.colors,
                    width_pixels: curve.width,
                    style: curve.style,
                }])?;
            }
            let selected = self.requests[selected_index];
            let source = selected.body_fixed_frame;
            let line = |i: usize| {
                let endpoint =
                    [DVec3::X, DVec3::Y, DVec3::Z][i] * (1.2 * selected.reference_radius_m);
                Ok::<_, anyhow::Error>(DebugLine {
                    endpoints: [
                        FramePosition::new(source, LocalPosition::origin()),
                        FramePosition::new(source, LocalPosition::try_metres(endpoint)?),
                    ],
                    color: [
                        [1.0, 0.1, 0.1, 1.0],
                        [0.1, 1.0, 0.1, 1.0],
                        [0.1, 0.3, 1.0, 1.0],
                    ][i],
                })
            };
            if self.camera.mode() != CameraMode::SurfaceInspection {
                let lines = [line(0)?, line(1)?, line(2)?];
                frame.append_historical_lines(source, &lines)?;
            } else {
                let observer = view.prepare_source(source)?.observer_in_source().metres();
                let up = Direction3::try_new(observer)?;
                let tangent = self
                    .camera
                    .inspection_tangent()
                    .ok_or_else(|| anyhow::anyhow!("surface inspection has no tangent anchor"))?;
                let origin = observer
                    - up.unit()
                        * self
                            .camera
                            .measured_clearance(&pair, self.ids[selected_index])?;
                let lines = [
                    tangent.east().unit(),
                    tangent.up().unit(),
                    tangent.north().unit(),
                ]
                .into_iter()
                .enumerate()
                .map(|(i, d)| {
                    Ok(DebugLine {
                        endpoints: [
                            FramePosition::new(source, LocalPosition::try_metres(origin)?),
                            FramePosition::new(source, LocalPosition::try_metres(origin + d)?),
                        ],
                        color: [
                            [1.0, 0.1, 0.1, 1.0],
                            [0.1, 1.0, 0.1, 1.0],
                            [0.1, 0.3, 1.0, 1.0],
                        ][i],
                    })
                })
                .collect::<Result<Vec<_>>>()?;
                frame.append_historical_lines(source, &lines)?;
            }
            Ok(())
        })();
        let preparation_ms = started.elapsed().as_secs_f64() * 1000.0;
        if let Err(error) = prepared {
            self.motion.set_paused(true);
            self.refresh_report_metadata();
            self.diagnostic = Some(format!(
                "Render preparation failed; no partial celestial frame uploaded: {error}"
            ));
            let (info, controls) =
                self.ui_info(CelestialPreparationReport::default(), near, preparation_ms);
            renderer.render(|ctx| draw_ui(ctx, controls, &info, &[]))?;
            return Ok(());
        }
        let report = frame.report();
        drop(preparation_span);
        let performance = PerformanceSnapshot {
            gpu_timestamp_capability: format!("{:?}", renderer.timestamp_availability()),
            preparation_ms: Some(preparation_ms),
            gpu_source_frame: renderer.gpu_source_frame(),
            ..Default::default()
        }
        .with_gpu(renderer.latest_gpu_profile(), "latest_completed");
        #[cfg(feature = "surface-profile")]
        let performance = PerformanceSnapshot {
            update_ms: Some(update_ms),
            frame_cpu_ms: Some(update_ms + preparation_ms),
            ..performance
        };
        let mut snapshot = DeveloperSnapshot::collect(SnapshotInput {
            navigation: Some(NavigationDiagnostics {
                window_focused: Some(
                    self.controls.input_focused
                        && self
                            .controls
                            .ui_context
                            .as_ref()
                            .is_none_or(|c| c.input(|i| i.focused)),
                ),
                viewport_keyboard_owned: Some(self.controls.viewport_input.owns_keyboard()),
                viewport_gesture_owned: Some(self.controls.viewport_input.owns_gesture()),
                ..self.camera.navigation_diagnostics()
            }),
            pair: &pair,
            pose: self.camera.pose(),
            camera_mode: self.camera.mode(),
            selected_body: self.selection.selected(),
            focused_body: self.camera.focused_body(),
            reference_body: self
                .camera
                .focused_body()
                .or(self.terrain_clearance.map(|c| c.body))
                .or(self.selection.selected()),
            frame_number: self.developer_frame_number,
            paused: self.motion.paused(),
            simulation_speed: self.motion.rate().multiplier(),
            motion: Some(self.motion.snapshot(&self.system)),
            projection,
            terrain_clearance_m: self.terrain_clearance.map(|c| c.clearance_m),
            rendering: RenderingSnapshot {
                terrain_render_mode: self.controls.terrain_view.name().into(),
                terrain_enabled: true,
                patch_borders_enabled: self.controls.terrain_view == TerrainViewMode::Grid,
                lod_colors_enabled: self.controls.terrain_view == TerrainViewMode::Level,
                navigation_markers_enabled: self.controls.markers,
            },
            performance,
        })?;
        snapshot.engine_profile = self.profile_snapshot.clone();
        snapshot.shared_scene = Some(serde_json::json!({
            "id": crate::shared_system::SCENE_NAME,
            "scene_sha256": self.scene_sha256,
            "camera_sha256": self.camera_sha256,
            "bodies": self.presentation.iter().map(|body| serde_json::json!({
                "id": body.semantic_id,
                "identity": body.identity,
                "definition_sha256": body.definition_sha256,
                "definition_revision": body.definition_revision,
            })).collect::<Vec<_>>(),
        }));
        self.atlas
            .annotate_terrain(&mut snapshot.terrain, &self.ids, &self.system);
        // Opt-in native evidence scratch export of this exact prepared state.
        // Disabled for ordinary launches; collection remains observational.
        if let Some(path) = &self.navigation_snapshot_path {
            snapshot.write_json(path)?;
        }
        // Split borrows retain the coherent world/projection and prepared tree view
        // until submission. UI queues commands; it never mutates that borrowed state.
        let info = UiInfo {
            snapshot: Some(&snapshot),
            terrain_clearance: self.terrain_clearance,
            clearance_query_us: self.clearance_query_us,
            owned_surface_count: self.surface_owners.iter().filter(|&&owned| owned).count(),
            system: &self.system,
            projection: &self.projection,
            motion: &self.motion,
            runner: self.motion.newtonian(),
            camera: &self.camera,
            ids: &self.ids,
            selected: selected_index,
            diagnostic: self.diagnostic.as_deref(),
            diagnostics: self.diagnostics,
            drift: self
                .baseline
                .zip(self.diagnostics)
                .map(|(baseline, diagnostic)| baseline.drift(diagnostic)),
            sampled_tick: self.sampled_tick,
            advance: self.advance,
            achieved_rate: self.achieved_rate,
            throughput: self.throughput_steps_s,
            pump_ms: self.pump_ms,
            preparation_ms,
            report,
            near,
            trail_mode: self.trails.mode(),
            trail_stride: self.trails.stride(),
            trail_count: self.trails.sample_count(),
            trail_capacity: self.trails.capacity(),
            trail_bytes: self.trails.payload_bytes(),
            trail_times: self.trails.retained_time_s(),
            celestial_projection: projection,
            guides: self.guides.guides(),
            bounds: &self.bounds,
            measurement: self.motion.newtonian().and_then(|r| {
                self.metrics
                    .measurement(r.config().fixed_step_s(), r.rate().multiplier())
            }),
            cpu_limited: self.cpu_limited,
            count_limited: self.count_limited,
            gap_diagnostic: self.gap_diagnostic.as_deref(),
            coarse_curves: self.coarse_curves,
        };
        let controls = &mut self.controls;
        let render_started = Instant::now();
        let submission_span = crate::engine_profile::span("GPU submission / presentation");
        let mut ui_build_cpu_ms = 0.0;
        renderer.render_celestial(&frame, |context| {
            let _span = crate::engine_profile::span("UI");
            let ui_started = Instant::now();
            draw_ui(context, controls, &info, frame.markers());
            ui_build_cpu_ms += ui_started.elapsed().as_secs_f64() * 1000.0;
        })?;
        drop(submission_span);
        snapshot.performance.render_present_ms =
            Some(render_started.elapsed().as_secs_f64() * 1000.0);
        snapshot.performance.ui_build_cpu_ms = Some(ui_build_cpu_ms);
        let native_cpu = renderer.native_render_timings();
        snapshot.performance.native_render_cpu_ms = Some([
            native_cpu.poll_ms,
            native_cpu.acquire_ms,
            native_cpu.ui_prepare_ms,
            native_cpu.scene_encode_ms,
            native_cpu.ui_encode_ms,
            native_cpu.submit_ms,
            native_cpu.present_ms,
            native_cpu.total_ms,
        ]);
        snapshot.performance.native_submission_id = renderer.native_submission_id();
        snapshot.performance.native_gpu_timestamp_sampling = renderer.native_timestamp_sampling();
        snapshot.performance.native_presentation_mode = Some(renderer.presentation_mode_label());
        snapshot.performance.native_redraw_uncapped = cfg!(feature = "developer-tools")
            && std::env::var("MUNDARIS_UNCAPPED").is_ok_and(|value| value == "1");
        #[cfg(feature = "developer-tools")]
        if renderer.deterministic() {
            snapshot.performance = snapshot
                .performance
                .with_gpu(renderer.latest_gpu_profile(), "same_frame_offscreen");
            snapshot.performance.gpu_source_frame = Some(self.developer_frame_number);
        }
        {
            let ids = &self.ids;
            let system = &self.system;
            snapshot.terrain_atlas = Some(self.atlas.snapshot(|body| {
                let index = ids.iter().position(|id| *id == body);
                index
                    .and_then(|index| self.presentation.get(index))
                    .map(|p| p.semantic_id.clone())
                    .or_else(|| system.body(body).ok().map(|b| b.name().to_string()))
                    .unwrap_or_default()
            }));
        }
        snapshot.performance.host_frame_ms =
            Some(host_frame_started.elapsed().as_secs_f64() * 1000.0);
        self.atlas.record_frame(
            native_interval_ms,
            snapshot.performance.host_frame_ms,
            renderer.terrain_atlas_report().jobs as usize,
        );
        drop(frame_span);
        let profiler_started = Instant::now();
        if let Some(profile) = self.profile_sampler.poll_value() {
            self.profile_snapshot = Some(std::sync::Arc::new(profile));
        }
        let profile_sample_due = self
            .profile_sample_at
            .is_none_or(|at| at.elapsed() >= Duration::from_millis(250));
        if profile_sample_due {
            self.profile_sample_at = Some(Instant::now());
            if crate::engine_profile::is_enabled() {
                let _ = self.profile_sampler.request();
            } else {
                self.profile_snapshot = None;
            }
        }
        snapshot.engine_profile = self.profile_snapshot.clone();
        if let Some(name) = self.performance_capture.observe(&snapshot) {
            self.diagnostic_capture_request = Some(name);
        }
        if std::mem::take(&mut self.controls.performance_lab.capture_requested) {
            self.performance_capture.request();
        }
        if std::mem::take(&mut self.controls.performance_lab.capture_stop_requested) {
            self.performance_capture.stop();
        }
        snapshot.performance.profiler_publication_ms =
            Some(profiler_started.elapsed().as_secs_f64() * 1000.0);
        snapshot.performance.host_frame_ms =
            Some(host_frame_started.elapsed().as_secs_f64() * 1000.0);
        self.controls.performance_lab.observe_frame(&snapshot);
        if profile_sample_due {
            self.controls.performance_lab.ingest(&snapshot, None);
        }
        #[cfg(feature = "developer-tools")]
        if self.developer_session.is_some() || renderer.deterministic() {
            self.developer_snapshot = Some(snapshot);
        }
        #[cfg(feature = "surface-profile")]
        tracing::debug!(
            interval_ms = elapsed.as_secs_f64() * 1000.0,
            update_ms,
            pump_ms = self.pump_ms,
            preparation_ms,
            clearance_query_us = self.clearance_query_us,
            latest_completed_gpu_query = ?renderer.latest_gpu_profile(),
            render_present_ms = render_started.elapsed().as_secs_f64() * 1000.0,
            dpi = scale,
            "frame CPU/cadence probe (render includes UI/acquire/upload/submit/present, not GPU duration)"
        );
        Ok(())
    }
    fn ui_info(
        &mut self,
        report: CelestialPreparationReport,
        near: f64,
        preparation_ms: f64,
    ) -> (UiInfo<'_>, &mut Controls) {
        let selected = self.selected_index();
        let celestial_projection = self
            .content_projection(near.max(0.1))
            .expect("valid content projection");
        (
            UiInfo {
                snapshot: None,
                terrain_clearance: self.terrain_clearance,
                clearance_query_us: self.clearance_query_us,
                owned_surface_count: self.surface_owners.iter().filter(|&&owned| owned).count(),
                system: &self.system,
                projection: &self.projection,
                motion: &self.motion,
                runner: self.motion.newtonian(),
                camera: &self.camera,
                ids: &self.ids,
                selected,
                diagnostic: self.diagnostic.as_deref(),
                diagnostics: self.diagnostics,
                drift: self
                    .baseline
                    .zip(self.diagnostics)
                    .map(|(baseline, diagnostic)| baseline.drift(diagnostic)),
                sampled_tick: self.sampled_tick,
                advance: self.advance,
                achieved_rate: self.achieved_rate,
                throughput: self.throughput_steps_s,
                pump_ms: self.pump_ms,
                preparation_ms,
                report,
                near,
                trail_mode: self.trails.mode(),
                trail_stride: self.trails.stride(),
                trail_count: self.trails.sample_count(),
                trail_capacity: self.trails.capacity(),
                trail_bytes: self.trails.payload_bytes(),
                trail_times: self.trails.retained_time_s(),
                celestial_projection,
                guides: self.guides.guides(),
                bounds: &self.bounds,
                measurement: self.motion.newtonian().and_then(|r| {
                    self.metrics
                        .measurement(r.config().fixed_step_s(), r.rate().multiplier())
                }),
                cpu_limited: self.cpu_limited,
                count_limited: self.count_limited,
                gap_diagnostic: self.gap_diagnostic.as_deref(),
                coarse_curves: self.coarse_curves,
            },
            &mut self.controls,
        )
    }
    fn prepare_visuals(&mut self) -> Result<()> {
        self.coarse_curves = 0;
        let projection = self.content_projection(0.1)?;
        let pair = self.projection.coherent_view(&self.system)?;
        let view = PreparedView::new(
            &pair.evaluation(),
            self.camera.pose(),
            RenderPrecisionBudget::near_debug(),
        )?;
        let count = self.ids.len() * 2;
        self.curves.resize_with(count, || VisualCurve {
            points: Vec::new(),
            colors: Vec::new(),
            width: 1.0,
            style: CelestialLineStyle::Solid,
            relative: Vec::new(),
        });
        self.history_fit_points.clear();
        let selected = self.selection.selected();
        for (i, &id) in self.ids.iter().enumerate() {
            let curve = &mut self.curves[i];
            curve.points.clear();
            curve.colors.clear();
            curve.style = CelestialLineStyle::Solid;
            curve.width = if selected == Some(id) { 2.5 } else { 1.5 };
            let included = matches!(self.scope, OverviewScope::WholeSystem)
                || self.bounds.included().contains(&id);
            if self.controls.trails && included {
                self.trails.display_points(
                    &self.system,
                    &self.projection,
                    id,
                    8193,
                    &mut curve.points,
                    &mut curve.colors,
                )?;
                if !self.controls.full_trails {
                    self.coarse_curves += usize::from(self.trail_scratch.simplify(
                        &view,
                        projection,
                        if selected == Some(id) { 2048 } else { 1024 },
                        &mut curve.points,
                        &mut curve.colors,
                    )?);
                }
                if self.bounds.included().contains(&id) {
                    for &point in &curve.points {
                        self.history_fit_points.push(
                            self.projection
                                .tree()
                                .evaluate()
                                .convert_position(point, self.projection.tree().root())?
                                .local()
                                .metres(),
                        );
                    }
                }
            }
            let curve = &mut self.curves[self.ids.len() + i];
            curve.points.clear();
            curve.colors.clear();
            curve.style = CelestialLineStyle::Dashed;
            curve.width = if selected == Some(id) { 1.5 } else { 1.0 };
            if self.controls.guide_visible
                && included
                && let Some(g) = self.guides.guides().iter().find(|g| g.body == id)
                && let Some(reference) = g.reference
                && g.has_geometry()
            {
                if !matches!(self.scope, OverviewScope::WholeSystem)
                    && !self.bounds.included().contains(&reference)
                {
                    continue;
                }
                let source = self.projection.frames_for(reference)?.translating;
                let cap = if selected == Some(id) { 1024 } else { 512 };
                let prepared = view.prepare_source(source)?;
                self.coarse_curves += usize::from(g.tessellate(
                    cap,
                    |p| {
                        Ok(projection.project_pixels(
                            prepared
                                .view_displacement(FramePosition::new(
                                    source,
                                    LocalPosition::try_metres(p)?,
                                ))?
                                .metres(),
                        )?)
                    },
                    &mut curve.relative,
                )?);
                for &p in &curve.relative {
                    curve
                        .points
                        .push(FramePosition::new(source, LocalPosition::try_metres(p)?));
                    let mut color = self.presentation[i].color;
                    color[3] = if selected == Some(id) { 0.85 } else { 0.45 };
                    curve.colors.push(color);
                }
            }
        }
        Ok(())
    }
}

struct UiInfo<'a> {
    snapshot: Option<&'a crate::developer_snapshot::DeveloperSnapshot>,
    terrain_clearance: Option<crate::terrain_inspection::TerrainClearance>,
    clearance_query_us: f64,
    owned_surface_count: usize,
    system: &'a CelestialSystem,
    projection: &'a CelestialFrameProjection,
    motion: &'a MotionSession,
    runner: Option<&'a FixedStepRunner>,
    camera: &'a CelestialCamera,
    ids: &'a [BodyId],
    selected: usize,
    diagnostic: Option<&'a str>,
    diagnostics: Option<SystemDiagnostics>,
    drift: Option<DiagnosticDrift>,
    sampled_tick: u64,
    advance: Option<SimulationAdvanceReport>,
    achieved_rate: Option<f64>,
    throughput: Option<f64>,
    pump_ms: f64,
    preparation_ms: f64,
    report: CelestialPreparationReport,
    near: f64,
    trail_mode: TrailMode,
    trail_stride: u64,
    trail_count: usize,
    trail_capacity: usize,
    trail_bytes: usize,
    trail_times: Option<(f64, f64)>,
    celestial_projection: CelestialProjection,
    guides: &'a [OrbitGuide],
    bounds: &'a SystemViewBounds,
    measurement: Option<PlaybackMeasurement>,
    cpu_limited: bool,
    count_limited: bool,
    gap_diagnostic: Option<&'a str>,
    coarse_curves: usize,
}
fn draw_engineering_ui(
    context: &egui::Context,
    controls: &mut Controls,
    info: &UiInfo<'_>,
    markers: &[CelestialMarker],
) {
    egui::SidePanel::left("celestial body inspector").exact_width(320.0).resizable(false).show(context,|ui| {
        egui::ScrollArea::vertical().show(ui,|ui| {
        developer_ui::left(ui, controls, info);
        ui.separator();
        ui.collapsing("Advanced engine diagnostics", |ui| {
        ui.heading("Navigation / guide internals");
        for (i,&id) in info.ids.iter().enumerate() {
            let body=info.system.body(id).expect("mapped body");
            let focused=info.camera.focused_body()==Some(id);
            let reference=info.guides.iter().find(|g|g.body==id).and_then(|g|g.reference).map_or("no automatic reference",|id|info.system.body(id).expect("guide reference").name());
            if ui.selectable_label(i==info.selected,format!("{}{} · #{}",body.name(),if focused{" [focused]"}else{""},i)).clicked(){controls.pending.push_back(Command::Select(id));}
            let distance=markers.iter().find(|m|m.request_index==i).map(|m|compact_distance(m.distance_m)).unwrap_or_else(||"offscreen".into());
            ui.small(format!("{distance} · guide reference: {reference}"));
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Focus (F)").clicked() {
                controls.pending.push_back(Command::Focus { fixed:false, fit:false });
            }
            if ui.button("Selected subsystem").clicked() {
                controls.pending.push_back(Command::Subsystem);
            }
        });
        if ui.button("Overview around reference + companions").clicked(){controls.pending.push_back(Command::ReferenceOverview);}
        ui.collapsing("Explicit overview membership",|ui|{
            for &id in info.ids {
                let mut included=controls.filter_ids.contains(&id);
                if ui.checkbox(&mut included,info.system.body(id).expect("body").name()).changed(){if included{controls.filter_ids.push(id);}else{controls.filter_ids.retain(|&b|b!=id);}}
            }
            if ui.button("Fit checked bodies + selection").clicked(){controls.pending.push_back(Command::FilteredOverview(controls.filter_ids.clone()));}
        });
        if ui.button("Advanced Free Flight / unfocus (Esc)").clicked(){controls.pending.push_back(Command::FreeFlight);}
        ui.label(format!("Control: {:?}",info.camera.mode()));
        ui.small("Orbit: left drag / wheel. Flight: WASD, Q/E, right drag, Shift boost.");
        ui.add(egui::Slider::new(&mut controls.manual_speed,1e-3..=1e3).logarithmic(true).text("User speed multiplier (×)"));
        ui.small(format!("Navigation speed {} / wall s",compact_distance(info.camera.flight_speed_m_s())));
        visual_controls::checkbox(ui,controls,visual_controls::Layer::Markers,"Navigation markers");visual_controls::checkbox(ui,controls,visual_controls::Layer::Labels,"Labels");visual_controls::checkbox(ui,controls,visual_controls::Layer::Guides,"Instantaneous orbit guides");visual_controls::checkbox(ui,controls,visual_controls::Layer::Trails,"Committed historical trails");
        let guide_count=if controls.guide_visible{info.guides.iter().filter(|g|(g.authored_orbit.is_some() || g.elements.is_some_and(|e|e.class()==ConicClass::Elliptic))&&g.reference.is_some()).count()}else{0};
        ui.small(format!("{guide_count} available guides · sampled {:.3} simulation s",info.system.sample_time().seconds_since_epoch()));
        ui.small("Coincident markers: repeat click to cycle every candidate.");
        let selected_id=info.ids[info.selected];
        let guide=info.guides.iter().find(|g|g.body==selected_id);
        if let Some(g)=guide {ui.small(format!("Guide {:?}; eta {:?}",g.elements.map(|e|e.class()),g.perturbation_ratio));if let Some(reason)=&g.diagnostic{ui.small(reason);}}
        egui::ComboBox::from_label("Guide pair reference").selected_text(guide.and_then(|g|g.reference).map_or("Automatic / unavailable",|id|info.system.body(id).expect("reference").name())).show_ui(ui,|ui|{
            if ui.button("Automatic").clicked(){controls.pending.push_back(Command::GuideReference(OrbitGuideReference::Automatic));}
            if ui.button("No guide").clicked(){controls.pending.push_back(Command::GuideReference(OrbitGuideReference::None));}
            for &id in info.ids {if id!=selected_id && ui.button(info.system.body(id).expect("body").name()).clicked(){controls.pending.push_back(Command::GuideReference(OrbitGuideReference::Explicit(id)));}}
        });
        let mut relative=controls.relative_trails;if ui.checkbox(&mut relative,"History relative to selected body").changed(){controls.pending.push_back(Command::TrailMode(relative));}
        egui::ComboBox::from_label("Historical reference").selected_text(match info.trail_mode{TrailMode::Inertial=>"System",TrailMode::SimultaneousBodyRelative(id)=>info.system.body(id).expect("recorded reference").name()}).show_ui(ui,|ui|{
            if ui.button("Inertial system history").clicked(){controls.pending.push_back(Command::TrailMode(false));}
            for &id in info.ids{if ui.button(info.system.body(id).expect("recorded body").name()).clicked(){controls.pending.push_back(Command::TrailReference(id));}}
        });
        let mode=match info.trail_mode {TrailMode::Inertial=>"Inertial system history".into(),TrailMode::SimultaneousBodyRelative(id)=>format!("History relative to {} at each sample, displayed there now",info.system.body(id).expect("reference").name())};ui.small(mode);
        ui.small(format!("History: {} samples, span {:?} s{}",info.trail_count,info.trail_times,if info.trail_count<2{" · accumulating"}else{""}));
        if info.bounds.history_outside_fit {ui.colored_label(egui::Color32::YELLOW,"History extends outside fit (2× core cap)");}
        ui.checkbox(&mut controls.all_history,"Fit including all displayed history");ui.checkbox(&mut controls.full_trails,"Full retained trail display (validation)");
        ui.small("Reference-sphere clearance; debug mesh is not metre-accurate terrain.");
        if info.coarse_curves>0 {ui.colored_label(egui::Color32::YELLOW,format!("{} coarse curves: display cap prevents 0.5 px tolerance",info.coarse_curves));}
        if info.camera.mode()==CameraMode::BodyOrbit {ui.label(format!("Reference-sphere clearance {}",compact_distance(info.camera.clearance_m())));}
        egui::ScrollArea::vertical().show(ui,|ui|{ui.collapsing("Engineering / authoring",|ui| {
        if let (Some(runner), Some(advance)) = (info.runner, info.advance) {
        ui.label("Newtonian point masses / fixed KDK / debug celestial shading");
        ui.label(format!("Branch {} / tick {} → target {} / h {} s",advance.branch_generation,advance.tick,advance.target_tick,runner.config().fixed_step_s()));
        ui.label(format!("Requested {:.3} s / authoritative {:.3} s",advance.requested_time.seconds_since_epoch(),info.system.sample_time().seconds_since_epoch()));
        ui.label(format!("Requested {}x / achieved {} / {:?}",runner.rate().multiplier(),info.achieved_rate.map_or_else(||"not yet measured".into(),|r|format!("{r:.3}x (wall sample)")),advance.status));
        ui.label(format!("Pending {} ticks / {:.3} s (fraction {:.3} s)",advance.backlog_ticks,advance.pending_simulation_seconds,advance.fractional_seconds));
        ui.label(format!("Latest update: {} work / {} forward / {} restore; {:.3} ms",advance.work_steps,advance.forward_steps,advance.restored_steps,info.pump_ms));
        ui.label(format!("Latest rejected demand {:.3} s / latest explicit cancellation {:.3} s",advance.rejected_simulation_seconds,advance.cancelled_simulation_seconds));
        if let Some(remaining)=advance.replay_remaining {
            ui.colored_label(egui::Color32::YELLOW,format!("Private replay: {remaining} work remaining, estimate {}",info.throughput.map_or_else(||"unmeasured".into(),|rate|format!("{:.2} wall s",remaining as f64/rate))));
            if ui.button("Cancel seek/replay (retain live world)").clicked() {controls.pending.push_back(Command::CancelSeek);}
        }
        ui.label(format!("History ticks {:?}, {} live + {} private replay bytes; snapshots restore / positive-step replay",advance.retained_ticks,advance.history_payload_bytes,advance.replay_history_payload_bytes));
        ui.horizontal(|ui| {
            if ui.button(if runner.paused() {"Resume"} else {"Pause (cancel debt)"}).clicked() {controls.pending.push_back(Command::Pause(!runner.paused()));}
            if ui.button("Single +").clicked() {controls.pending.push_back(Command::Single(true));}
            if ui.button("Single −").clicked() {controls.pending.push_back(Command::Single(false));}
        });
        ui.horizontal_wrapped(|ui| {for rate in [-1000.0,-10.0,-1.0,0.1,1.0,10.0,100.0,1000.0,10000.0,100000.0,1000000.0,1000000000.0] {
            if ui.button(format!("{rate}x")).clicked() {controls.pending.push_back(Command::Rate(rate));}
        }});
        if advance.status==PlaybackStatus::DemandHaltedOverload {
            ui.colored_label(egui::Color32::LIGHT_RED,"Demand halted: overload. Admitted debt drains at fixed h.");
            if ui.button("Resume demand admission (retains debt)").clicked() {controls.pending.push_back(Command::ResumeAdmission);}
        }
        ui.add(egui::DragValue::new(&mut controls.seek_seconds).speed(runner.config().fixed_step_s()).suffix(" seek s"));
        match runner.quantize_seek_seconds(controls.seek_seconds) {
            Ok(preview)=>{
                ui.label(format!("Nearest tick {} / {:.3} s / delta {:+.3} s",preview.tick,preview.resulting_seconds,preview.quantization_delta_s));
                if ui.button("Confirm seek (paused)").clicked() {controls.pending.push_back(Command::Seek(preview.tick));}
            }
            Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}
        }
        }
        if ui.button("Reset branch baseline (IDs / focus retained)").clicked() {controls.pending.push_back(Command::Reset);}
        ui.separator();
        if let (Some(advance), Some(diagnostics), Some(drift)) = (info.advance, info.diagnostics, info.drift) {
        ui.label(format!("{} bodies / {} pairs / {} new force passes this update",info.system.body_count(),pair_count(info.system.body_count()).expect("valid count"),advance.force_passes));
        ui.label(format!("{} far / {} surface owners",info.system.body_count()-info.owned_surface_count,info.owned_surface_count));
        ui.label(format!("Diagnostic sample tick {} / {:.3} s",info.sampled_tick,diagnostics.sampled_time.seconds_since_epoch()));
        ui.label(format!("E {:.8e} J / drift {:.3e} J / normalized {:.3e}{}",diagnostics.total_energy_j(),drift.energy_j,drift.relative_energy,if drift.uses_near_zero_energy_scale {" (K0+|U0| scale)"} else {" (|E0| scale)"}));
        ui.label(format!("P drift {:.3e} kg m/s / P/Qp {:.3e}",drift.momentum_kg_m_s.length(),drift.normalized_momentum));
        ui.label(format!("Lcom drift {:.3e} kg m²/s / L/Ql {:.3e}",drift.angular_momentum_kg_m2_s.length(),drift.normalized_angular_momentum));
        ui.label(format!("COM residual {:.3e} m / COM {:?} m",drift.com_residual_m.length(),diagnostics.center_of_mass_m));
        if info.ids.len()==3 {
            let planet=info.system.body(info.ids[1]).expect("fixture planet");let moon=info.system.body(info.ids[2]).expect("fixture moon");
            let distance=(moon.state().center_in_system().metres()-planet.state().center_in_system().metres()).length();
            let speed=(moon.state().center_velocity_in_system().metres_per_second()-planet.state().center_velocity_in_system().metres_per_second()).length();
            let energy=0.5*speed*speed-GRAVITATIONAL_CONSTANT_M3_KG_S2*(planet.properties().mass_kg()+moon.properties().mass_kg())/distance;
            ui.label(format!("Moon relative: {distance:.6e} m / {speed:.6e} m/s / local Kepler {energy:.6e} J/kg (not conserved with third body)"));
        }
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {for (i,&id) in info.ids.iter().enumerate() {if ui.selectable_label(i==info.selected,info.system.body(id).expect("fixture body").name()).clicked() {controls.pending.push_back(Command::Select(id));}}});
        ui.horizontal_wrapped(|ui| {for (label,command) in [("Focus translating",Command::Focus {fixed:false,fit:false}),("Fit physical body",Command::Focus {fixed:false,fit:true}),("Inspect fixed spin",Command::Focus {fixed:true,fit:true}),("Overview",Command::Overview)] {if ui.button(label).clicked() {controls.pending.push_back(command);}}});
        ui.horizontal(|ui| {
            if ui.button("Re-express translating").clicked() {controls.pending.push_back(Command::Reexpress(false));}
            if ui.button("Re-express fixed").clicked() {controls.pending.push_back(Command::Reexpress(true));}
        });
        ui.label("Drag outside panel: orbit / wheel: exponential zoom / Tab: next body");
        let body=info.system.body(info.ids[info.selected]).expect("selected fixture body");
        ui.label(format!("ID {:?} / {}",info.ids[info.selected],body.name()));
        ui.label(format!("Mass {:.6e} kg / physical reference radius {:.6e} m",body.properties().mass_kg(),body.properties().reference_radius_m()));
        ui.label(format!("System position {:?} m",body.state().center_in_system().metres()));
        ui.label(format!("System velocity {:?} m/s / speed {:.6e} m/s",body.state().center_velocity_in_system().metres_per_second(),body.state().center_velocity_in_system().metres_per_second().length()));
        ui.label(format!("Orientation {:?} / spin {:?} rad/s",body.state().body_to_system().quaternion().to_array(),body.state().angular_velocity_in_system().radians_per_second()));
        ui.collapsing("Authoring (physical edits start a paused new branch)",|ui| {
            ui.text_edit_singleline(&mut controls.name);if ui.button("Apply name (retain history)").clicked() {controls.pending.push_back(Command::Rename(controls.name.clone()));}
            ui.text_edit_singleline(&mut controls.mass);if ui.button("Apply mass kg").clicked() {match controls.mass.parse() {Ok(value)=>controls.pending.push_back(Command::Mass(value)),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,format!("{error}"));}}}
            ui.text_edit_singleline(&mut controls.radius);if ui.button("Apply radius m (geometry only)").clicked() {match controls.radius.parse() {Ok(value)=>controls.pending.push_back(Command::Radius(value)),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,format!("{error}"));}}}
            if info.motion.is_analytic() { ui.small("Velocity edits unavailable: prescribed trajectories determine velocity.  Periods are independent of mass/radius edits."); }
            ui.add_enabled_ui(!info.motion.is_analytic(), |ui| {
            for value in &mut controls.velocity {ui.text_edit_singleline(value);}
            if ui.button("Apply system velocity m/s").clicked() {
                let values=controls.velocity.iter().map(|v|v.parse::<f64>()).collect::<std::result::Result<Vec<_>,_>>();
                match values {Ok(v)=>controls.pending.push_back(Command::Velocity(DVec3::new(v[0],v[1],v[2]))),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}}
            }
            });
        });
        visual_controls::checkbox(ui,controls,visual_controls::Layer::Markers,"Navigation markers (no physical size change)");visual_controls::checkbox(ui,controls,visual_controls::Layer::Labels,"Labels");visual_controls::checkbox(ui,controls,visual_controls::Layer::Trails,"Actual committed-history trails");
        let mut relative=controls.relative_trails;if ui.checkbox(&mut relative,"Simultaneous history relative to selected body").changed() {controls.pending.push_back(Command::TrailMode(relative));}
        let mode_label=match info.trail_mode {TrailMode::Inertial=>"Inertial system-space history".to_string(),TrailMode::SimultaneousBodyRelative(id)=>format!("History relative to {} at each sample",info.system.body(id).expect("trail reference").name())};
        ui.label(mode_label);ui.label(format!("Trail stride {} ticks / {}/{} complete samples / {} bytes / times {:?} s",info.trail_stride,info.trail_count,info.trail_capacity,info.trail_bytes,info.trail_times));
        ui.collapsing("Renderer / frame diagnostics",|ui| {
            ui.label(format!("World revision {} / projected {}",info.system.revision(),info.projection.represented_revision()));
            ui.label(format!("Observer {:?} / frame {:?} / depth {}",info.camera.attachment(),info.camera.pose().position().frame(),info.projection.tree().evaluate().depth(info.camera.pose().position().frame()).unwrap_or(0)));
            ui.label(format!("{} triangles / {} markers / {} styled guide+history segments / {} debug-axis segments",info.report.triangles,info.report.markers,info.report.polyline_segments,info.report.trail_segments));
            ui.label(format!("{} subpixel / {} culled / {} precision fallbacks",info.report.subpixel_bodies,info.report.culled_bodies,info.report.precision_fallbacks));
            ui.label(format!("Narrowing {:.3e} m / projected {:.3e} px / prepare {:.3} ms",info.report.max_narrowing_error_m,info.report.max_projected_error_pixels,info.preparation_ms));
            ui.label(format!("Reverse-Z near {:.3e} m / observer distance {:.3e} m",info.near,info.camera.distance_m()));
            if ui.button("Rebuild disposable projection").clicked() {controls.pending.push_back(Command::Rebuild);}
        });
        if let Some(error)=info.diagnostic {ui.colored_label(egui::Color32::LIGHT_RED,error);}
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut controls.gap_threshold_ms).range(1.0..=60000.0).suffix(" ms clock gap threshold"));
            if ui.button("Apply session threshold").clicked() {
                if controls.gap_threshold_ms.is_finite()&&(1.0..=60000.0).contains(&controls.gap_threshold_ms) {controls.pending.push_back(Command::GapThreshold(Duration::from_secs_f64(controls.gap_threshold_ms/1000.0)));}
                else {ui.colored_label(egui::Color32::LIGHT_RED,"Clock threshold must be finite and within 1..60000 ms");}
            }
        });
        });
        });
        });
    });
    });
}

fn draw_ui(
    context: &egui::Context,
    controls: &mut Controls,
    info: &UiInfo<'_>,
    markers: &[CelestialMarker],
) {
    if let Some(snapshot) = info.snapshot {
        controls
            .performance_lab
            .draw(context, Some(snapshot), snapshot.engine_profile.as_deref());
    } else {
        controls.performance_lab.draw(context, None, None);
    }
    egui::SidePanel::right("planet surface / inspection").exact_width(300.0).resizable(false).show(context,|ui| {
            egui::ScrollArea::vertical().show(ui,|ui| {
            developer_ui::right(ui, controls, info);
            ui.separator();
            ui.collapsing("Advanced terrain diagnostics", |ui| {
            if let Some(id)=info.camera.focused_body() {
            let pair=info.projection.coherent_view(info.system).expect("coherent UI");
            ui.collapsing("Camera precision / clearance / navigation", |ui| {
            if let Some(c)=info.terrain_clearance {
                ui.strong(format!("Terrain clearance: {:+.2} m",c.clearance_m));
                if c.clearance_m<0.0 {ui.colored_label(egui::Color32::RED,"INSIDE TERRAIN (complete field)");}
                else {ui.label("Above complete displaced terrain");}
                ui.monospace(format!("Centre distance {:.3} m\nSphere altitude {:+.3} m\nTerrain elevation {:+.3} m\nDisplaced radius {:.3} m\nAnalytic slope {:.2}°",c.camera_radius_m,c.sphere_altitude_m,c.terrain_elevation_m,c.surface_radius_m,c.slope_angle_rad.to_degrees()));
                ui.small(format!("Body {} · direction {:?} · cumulative controller complete-query time {:.1} µs",info.system.body(id).expect("body").name(),c.location.direction().unit(),info.clearance_query_us));
                egui::ComboBox::from_label("Debug terrain guard").selected_text(controls.terrain_guard_m.map_or("Disabled".into(),|m|format!("{m} m"))).show_ui(ui,|ui| {
                    for minimum in [None,Some(2.0),Some(10.0),Some(100.0)] {
                        let label=minimum.map_or("Disabled".into(),|m|format!("{m} m"));
                        if ui.selectable_label(controls.terrain_guard_m==minimum,label).clicked() {controls.pending.push_back(Command::TerrainGuard(minimum));}
                    }
                });
                ui.small("Guard protects sampled complete-terrain radial clearance (reference-sphere fallback when unavailable), not collision.");
            } else if info.system.body(id).is_ok_and(|body|!body.has_surface()) {
                ui.label("Non-terrain / far-only body: no rocky terrain query");
                if let Ok(clearance)=info.camera.measured_clearance(&pair,id) {ui.label(format!("Reference-sphere altitude {}",compact_distance(clearance)));}
            } else {ui.label("Terrain clearance unavailable");}
            ui.small(format!("Near plane {:.3} m · infinite reverse-Z",info.near));
            if matches!(info.camera.mode(),CameraMode::BodyOrbit|CameraMode::SurfaceInspection)&&!info.camera.transitioning() {
                ui.add(egui::DragValue::new(&mut controls.clearance_target).speed(1.0).suffix(" m target clearance"));
                if ui.button("Approach clearance target").clicked() {controls.pending.push_back(Command::Clearance(controls.clearance_target));}
                if info.camera.mode()==CameraMode::BodyOrbit && ui.button("Continuous 30 s approach to 2 m").clicked() {controls.pending.push_back(Command::Approach);}
                ui.horizontal_wrapped(|ui| {for clearance in [1e11,1e5,1e4,1e3,100.0,10.0,2.0] {if ui.button(compact_distance(clearance)).clicked() {controls.pending.push_back(Command::Clearance(clearance));}}});
                if info.camera.mode()==CameraMode::BodyOrbit && ui.button("Surface Navigation (I): co-rotating attachment").clicked() {controls.pending.push_back(Command::SurfaceInspection);}
            }
            if info.camera.mode()==CameraMode::SurfaceInspection {
                ui.small("Co-rotating; zero relative simulation derivative. WASD/QE editor offsets, right-drag look; optional displaced-terrain guard.");
                if ui.button("Look tangent / horizon (H)").clicked() {controls.pending.push_back(Command::SurfaceHorizon);}
                if ui.button("Depart to centre-look Body Orbit").clicked() {controls.pending.push_back(Command::Focus {fixed:false,fit:true});}
                if ui.button("Single physical step +h").clicked() {controls.pending.push_back(Command::Single(true));}
                ui.horizontal_wrapped(|ui| {for &target in info.ids {if target!=id&&ui.button(format!("Look at {}",info.system.body(target).expect("body").name())).clicked() {controls.pending.push_back(Command::LookBody(target));}}});
            }
            });
            }
            });
            });
        });
    egui::TopBottomPanel::top("exact playback and navigation").show(context,|ui| {
        ui.horizontal(|ui|{
            ui.heading("Mundaris");
            if ui.button(if info.motion.paused(){"▶ Resume"}else{"⏸ Pause"}).clicked(){controls.pending.push_back(Command::Pause(!info.motion.paused()));}
            ui.small(if info.motion.is_analytic(){"Prescribed analytic motion"}else{"Newtonian motion"});
            if ui.button("Whole system (Home)").clicked(){controls.pending.push_back(Command::Overview);}
            if ui.button("Previous focus").clicked(){controls.pending.push_back(Command::Select(info.ids[(info.selected+info.ids.len()-1)%info.ids.len()]));controls.pending.push_back(Command::Focus{fixed:false,fit:false});}
            if ui.button("Next focus").clicked(){controls.pending.push_back(Command::Select(info.ids[(info.selected+1)%info.ids.len()]));controls.pending.push_back(Command::Focus{fixed:false,fit:false});}
        });
        ui.collapsing("Advanced playback status", |ui| {
        if let (Some(runner), Some(advance)) = (info.runner, info.advance) {
        ui.horizontal(|ui|{
            ui.strong(format!("Requested {}×",runner.rate().multiplier()));
                ui.strong(info.measurement.map_or_else(||"Achieved: warming / paused / replaying".into(),|m|if m.tick_limited{format!("Achieved {:.1}× ({:.1}s; tick-limited), segment {:.2}× / {:.1}s",m.achieved_rate,m.window_s,m.segment_rate,m.segment_wall_s)}else{format!("Achieved {:.1}× ({:.1}s)",m.achieved_rate,m.window_s)}));
            ui.label(format!("{:?} · {:?}{}",advance.status,info.camera.mode(),if info.camera.transitioning(){" · transitioning"}else{""}));
        });
        ui.horizontal(|ui|{
            ui.label(format!("Baseline exact · full N-body KDK · h={}s · authority {:.2} days · requested {:.2} days",runner.config().fixed_step_s(),advance.authoritative_time.seconds_since_epoch()/86400.0,advance.requested_time.seconds_since_epoch()/86400.0));
            ui.label(format!("Pending {} ticks · last {} work{}{}",advance.backlog_ticks,advance.work_steps,if info.cpu_limited{" · CPU budget limited"}else{""},if info.count_limited{" · opportunity count limited"}else{""}));
            if advance.backlog_ticks==0 && !runner.paused() && runner.rate().multiplier()>0.0 {
                let wait=(runner.config().fixed_step_s()-advance.fractional_seconds)/runner.rate().multiplier();
                if wait>0.5 {ui.small(format!("Next committed step in {wait:.1} wall s"));}
            }
        });
        ui.horizontal(|ui|{
            for rate in [1.0,100.0,1000.0,10000.0,100000.0,1000000.0] {if ui.button(format!("{rate}×")).clicked(){controls.pending.push_back(Command::Rate(rate));}}
            ui.add(egui::DragValue::new(&mut controls.custom_rate).speed(1000.0).prefix("Custom "));if ui.button("Set").clicked(){controls.pending.push_back(Command::Rate(controls.custom_rate));}
            if advance.status==PlaybackStatus::DemandHaltedOverload {
                ui.colored_label(egui::Color32::LIGHT_RED,"Admission halted; fixed-h debt drains");
                if ui.button("Lower rate").clicked(){controls.pending.push_back(Command::Rate(runner.rate().multiplier()/10.0));}
                if ui.button("Resume admission").clicked(){controls.pending.push_back(Command::ResumeAdmission);}
            }
        });
        } else { ui.small("Direct analytic sampling: no integration backlog, force passes, replay or conservation-fidelity claim."); }
        });
    });
    egui::TopBottomPanel::bottom("celestial semantics legend").exact_height(28.0).show(context,|ui|{
        ui.horizontal(|ui|{ui.small(if info.motion.is_analytic(){"Dashed = authored ORBIT GUIDE · Solid/fading = published HISTORY · Rings/labels = navigation overlays"}else{"Dashed = instantaneous two-body ORBIT GUIDE · Solid/fading = committed HISTORY · Rings/labels = navigation overlays"});
            if let Some(gap)=info.gap_diagnostic {ui.colored_label(egui::Color32::YELLOW,gap);}else if let Some(error)=info.diagnostic {ui.colored_label(egui::Color32::LIGHT_RED,error);}
        });
    });
    draw_engineering_ui(context, controls, info, markers);
    let scale = context.pixels_per_point();
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(context,|ui|{
        let rect=ui.max_rect();
        controls.ui_context=Some(context.clone());
        controls.scene_layer=Some(ui.layer_id());
        let origin=[(rect.min.x*scale).round().max(0.0) as u32,(rect.min.y*scale).round().max(0.0) as u32];
        let size=[(rect.width()*scale).round().max(1.0) as u32,(rect.height()*scale).round().max(1.0) as u32];
        controls.viewport=Some((origin,size));
        controls.viewport_ready=origin==info.celestial_projection.origin() && size==info.celestial_projection.viewport();
        let viewport=ScreenRect{min:origin.map(f64::from),max:[(origin[0]+size[0]) as f64,(origin[1]+size[1]) as f64]};
        let response=ui.allocate_rect(rect,egui::Sense::click_and_drag());
        let mut inputs=Vec::new();let mut texts=Vec::new();
        for marker in markers {
            let id=info.ids[marker.request_index];let selected=marker.request_index==info.selected;let focused=info.camera.focused_body()==Some(id);
            if let Some([x,y])=marker.screen_pixels {
                let p=egui::pos2(x/scale,y/scale);
                let color=if selected{egui::Color32::YELLOW}else if focused{egui::Color32::LIGHT_GREEN}else{egui::Color32::LIGHT_GRAY};
                if controls.markers {
                    let alpha=marker_opacity(marker.apparent_diameter_pixels,matches!(marker.representation,SphereRepresentation::PhysicalSphere|SphereRepresentation::Surface));
                    ui.painter().circle_stroke(p,4.0/scale,egui::Stroke::new(1.3/scale,color.gamma_multiply(alpha)));
                }
                if (selected||focused)&&info.camera.mode()!=CameraMode::SurfaceInspection {
                    let radius=(marker.apparent_diameter_pixels as f32*0.5+5.0).clamp(8.0,2000.0)/scale;
                    ui.painter().circle_stroke(p,radius,egui::Stroke::new(1.5/scale,color));
                }
                if controls.labels {
                    let body=info.system.body(id).expect("body");
                    let duplicate=info.system.bodies().filter(|(_,other)|other.name()==body.name()).count()>1;
                    let name=if duplicate{format!("{} #{}",body.name(),marker.request_index)}else{body.name().into()};
                    let text=format!("{} · {}{}",name,compact_distance(marker.distance_m),if marker.occluded{" (overlay)"}else{""});
                    let galley=ui.painter().layout_no_wrap(text,egui::FontId::proportional(13.0),color);
                    inputs.push(LabelInput{body:id,marker:[x as f64,y as f64],size:[(galley.size().x*scale+8.0) as f64,(galley.size().y*scale+6.0) as f64],selected,focused,hovered:false,diameter:marker.apparent_diameter_pixels,distance_m:marker.distance_m});
                    texts.push((id,galley,color));
                }
            }
        }
        controls.layout.layout(&inputs,viewport,&[],&mut controls.placed);
        let mut label_consumed=false;
        for label in &controls.placed {
            let label_rect=egui::Rect::from_min_max(egui::pos2(label.rect.min[0] as f32/scale,label.rect.min[1] as f32/scale),egui::pos2(label.rect.max[0] as f32/scale,label.rect.max[1] as f32/scale));
            if let Some((_,galley,color))=texts.iter().find(|t|t.0==label.body) {
                ui.painter().rect_filled(label_rect,3.0,egui::Color32::from_black_alpha(190));
                if label.leader {ui.painter().line_segment([egui::pos2(label.marker[0] as f32/scale,label.marker[1] as f32/scale),label_rect.center()],egui::Stroke::new(0.8_f32,*color));}
                ui.painter().galley(label_rect.min+egui::vec2(4.0/scale,3.0/scale),galley.clone(),*color);
            }
            let hit=ui.interact(label_rect,egui::Id::new(("body label",label.body)),egui::Sense::click());
            if hit.clicked()||hit.double_clicked(){label_consumed=true;controls.pending.push_back(Command::Select(label.body));if hit.double_clicked(){controls.pending.push_back(Command::Focus{fixed:false,fit:false});}}
        }
        let input=context.input(|input|input.clone());
        if input.pointer.primary_pressed() && response.hovered(){controls.gesture_start=input.pointer.interact_pos();controls.gesture_dragged=false;}
        if let (Some(start),Some(current))=(controls.gesture_start,input.pointer.interact_pos()) && start.distance(current)>4.0 {controls.gesture_dragged=true;}
        if (response.clicked()||response.double_clicked())&&!label_consumed&&!controls.gesture_dragged && let Some(pointer)=input.pointer.interact_pos() {
            let targets:Vec<_>=markers.iter().map(|m|BodyHitTarget{body:info.ids[m.request_index],marker:if controls.markers||m.request_index==info.selected {m.screen_pixels.map(|p|p.map(f64::from))}else{None},marker_radius_pixels:8.0,label:controls.placed.iter().find(|l|l.body==info.ids[m.request_index]).map(|l|l.rect),center_in_view_m:m.center_in_view_m,radius_m:info.system.body(info.ids[m.request_index]).expect("body").properties().reference_radius_m(),occluded_overlay:m.occluded}).collect();
            let pixels=[(pointer.x*scale) as f64,(pointer.y*scale) as f64];
            if let Ok(pick)=pick_body(&targets,pixels,info.celestial_projection) && let Some(body)=controls.pick_cycle.choose(&pick,pixels) {
                controls.pending.push_back(Command::Select(body));if response.double_clicked(){controls.pending.push_back(Command::Focus{fixed:false,fit:false});}
                if pick.candidates.len()>1 {response.clone().on_hover_text("Coincident markers: repeat click to cycle every body; list and displaced labels also select.");}
            }
        }
        if input.pointer.primary_released(){controls.gesture_start=None;}
        let local_look=matches!(info.camera.mode(),CameraMode::FreeFlight|CameraMode::SurfaceInspection);
        let keyboard_blocked=context.wants_keyboard_input() || context.memory(|m|m.focused().is_some());
        controls.keyboard_blocked=keyboard_blocked;
        let automation_active={
            #[cfg(feature="developer-tools")]
            { controls.automation_owner.is_some() }
            #[cfg(not(feature="developer-tools"))]
            { false }
        };
        if (!input.focused || !controls.input_focused) && !automation_active {
            controls.pending.push_back(Command::CancelNavigation);
        } else if !controls.native_events {
            let deltas=controls.viewport_input.events(&input.events,rect,local_look,keyboard_blocked,
                |p| context.layer_id_at(p)!=Some(ui.layer_id()) || controls.placed.iter().any(|l|
                    l.rect.contains([(p.x*scale) as f64,(p.y*scale) as f64])),
                controls.manual_speed);
            for delta in deltas {controls.pending.push_back(Command::Navigation(delta));}
        }
    });
    if !controls.native_events
        && controls.input_focused
        && context.input(|i| i.focused)
        && !context.wants_keyboard_input()
        && !context.memory(|m| m.focused().is_some())
    {
        context.input(|input| {
            if input.key_pressed(egui::Key::I) {
                controls.pending.push_back(Command::SurfaceInspection);
            }
            if input.key_pressed(egui::Key::H) {
                controls.pending.push_back(Command::SurfaceHorizon);
            }
            if input.key_pressed(egui::Key::Tab) {
                let next = if input.modifiers.shift {
                    (info.selected + info.ids.len() - 1) % info.ids.len()
                } else {
                    (info.selected + 1) % info.ids.len()
                };
                controls.pending.push_back(Command::Select(info.ids[next]));
            }
            if input.key_pressed(egui::Key::F) {
                controls.pending.push_back(Command::Focus {
                    fixed: false,
                    fit: false,
                });
            }
            if input.key_pressed(egui::Key::Home) {
                controls.pending.push_back(Command::Overview);
            }
            if input.key_pressed(egui::Key::Escape) {
                controls.pending.push_back(Command::FreeFlight);
            }
        });
    }
}

#[cfg(test)]
mod analytic_publication_tests {
    use super::*;
    #[test]
    fn shared_camera_matches_authored_pose_and_survives_idle_motion() {
        let loaded =
            crate::shared_system::SharedTestSystem::load_canonical(NonZeroU64::new(71).unwrap())
                .unwrap();
        let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
        let expected = DVec3::from_array(loaded.camera.position_body_m);
        assert!((demo.camera.pose().position().local().metres() - expected).length() < 1e-8);
        let orientation = glam::DQuat::from_array(loaded.camera.orientation_xyzw);
        assert!(
            demo.camera
                .pose()
                .orientation()
                .quaternion()
                .dot(orientation)
                .abs()
                > 1.0 - 1e-14
        );
        let definition = demo
            .system
            .body(demo.ids[loaded.initial_body_index])
            .unwrap()
            .surface_definition()
            .cloned();
        demo.seek_seconds(15.0).unwrap();
        demo.update(Duration::from_millis(16));
        assert!((demo.camera.pose().position().local().metres() - expected).length() < 1e-8);
        assert_eq!(
            demo.system
                .body(demo.ids[loaded.initial_body_index])
                .unwrap()
                .surface_definition()
                .cloned(),
            definition
        );
    }
    #[test]
    fn analytic_frame_failure_retains_sampled_authority_and_last_published_time() {
        let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
        if let MotionSession::Analytic(a) = &mut demo.motion {
            a.seek_seconds(42.5, &mut demo.system).unwrap();
        }
        let before = demo
            .system
            .bodies()
            .map(|(_, b)| b.clone())
            .collect::<Vec<_>>();
        let revision = demo.system.revision();
        let other =
            crate::shared_system::SharedTestSystem::load_canonical(NonZeroU64::new(2).unwrap())
                .unwrap()
                .system;
        demo.projection =
            CelestialFrameProjection::build(&other, NonZeroU64::new(2).unwrap()).unwrap();
        demo.publish();
        assert!(!demo.coherent);
        assert!(demo.motion.paused());
        assert_eq!(demo.system.revision(), revision);
        assert_eq!(demo.system.sample_time().seconds_since_epoch(), 42.5);
        assert_eq!(
            demo.system
                .bodies()
                .map(|(_, b)| b.clone())
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(demo.motion_snapshot().published_time_s, 0.0);
        assert!(demo.motion_snapshot().latest_failure.is_some());
        demo.update(Duration::from_millis(16));
        assert!(!demo.coherent);
        assert_eq!(demo.system.revision(), revision);
        demo.command(Command::Rebuild).unwrap();
        demo.publish();
        assert!(demo.coherent);
        assert_eq!(demo.motion_snapshot().published_time_s, 42.5);
        assert!(demo.motion_snapshot().latest_failure.is_none());
        assert!(demo.motion.paused());
    }
}
