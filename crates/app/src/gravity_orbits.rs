//! Composition of authoritative physics, coherent projection and disposable debug views.
use crate::developer_snapshot::{
    DeveloperSnapshot, PerformanceSnapshot, RenderingSnapshot, SnapshotInput,
};
use crate::motion_session::{AnalyticSession, MotionSession, MotionSnapshot};
use crate::{celestial_camera::*, trails::*};
use crate::{
    celestial_labels::*, celestial_selection::*, interactive_clock::*, orbit_guides::*,
    playback_metrics::*, system_view::*,
};
use anyhow::{Context, Result};
use astrum_math::*;
use astrum_renderer::*;
use astrum_simulation::*;
use astrum_world::*;
use glam::DVec3;
use std::{
    collections::VecDeque,
    num::NonZeroU64,
    time::{Duration, Instant},
};

enum Command {
    Visual(visual_controls::VisualCommand),
    #[cfg(feature = "developer-tools")]
    LookBody(BodyId),
    SurfaceInspection,
    #[cfg(feature = "developer-tools")]
    BodyOrbit,
    SurfaceHorizon,
    Clearance(f64),
    Pause(bool),
    Rate(f64),
    Single(bool),
    Seek(u64),
    SeekSeconds(f64),
    Reset,
    Select(BodyId),
    Focus {
        fixed: bool,
        fit: bool,
    },
    Overview,
    FreeFlight,
    GuideReference(OrbitGuideReference),
    Navigation(NavigationInput),
    TimedNavigation(Instant, NavigationInput),
    #[cfg(feature = "developer-tools")]
    CancelNavigation,
    #[cfg(test)]
    Rebuild,
    Mass(f64),
    Radius(f64),
    Velocity(DVec3),
    Rename(String),
    TrailMode(bool),
}
#[cfg(feature = "developer-tools")]
mod developer;
mod frame_host;
mod planet_editing;
pub mod navigation_input;
mod studio_bridge;
mod visual_controls;
struct Controls {
    profiler: crate::profiler::Profiler,
    #[cfg(feature = "developer-tools")]
    automation_owner: Option<String>,
    #[cfg(feature = "developer-tools")]
    automation_stop: bool,
    terrain_preview: bool,
    sun_from_star: bool,
    terrain_view: TerrainViewMode,
    /// Session render settings (registry: `crate::render_settings`).
    render_settings: astrum_renderer::RenderSettings,
    pending: VecDeque<Command>,
    name: String,
    mass: String,
    radius: String,
    velocity: [String; 3],
    markers: bool,
    labels: bool,
    trails: bool,
    relative_trails: bool,
    guide_visible: bool,
    all_history: bool,
    full_trails: bool,
    manual_speed: f64,
    navigation: NavigationInput,
    viewport_input: navigation_input::ViewportInput,
    input_focused: bool,
    /// Set once the UI toolkit delivers viewport input; navigation then
    /// advances on wall-clock timestamps instead of frame elapsed time.
    native_events: bool,
    pixels_per_point: f64,
    layout: LabelLayout,
    placed: Vec<PlacedLabel>,
    pick_cycle: PickCycle,
    gesture_start: Option<[f32; 2]>,
    gesture_dragged: bool,
    /// Bounded session log shown in the Studio log tab.
    log: VecDeque<crate::studio::view::LogItem>,
    logged_diagnostic: Option<String>,
}
impl Controls {
    fn new(body: &CelestialBody) -> Self {
        Self {
            profiler: {
                let mut profiler = crate::profiler::Profiler::default();
                profiler.enabled = std::env::var("ASTRUM_PERFORMANCE_LAB").is_ok_and(|s| s == "1")
                    || std::env::var("ASTRUM_PROFILE").is_ok_and(|s| s == "1");
                crate::engine_profile::set_enabled(profiler.enabled);
                crate::engine_profile::set_budget_ns("Frame", Some(100_000_000));
                profiler
            },
            #[cfg(feature = "developer-tools")]
            automation_owner: None,
            #[cfg(feature = "developer-tools")]
            automation_stop: false,
            terrain_preview: false,
            sun_from_star: false,
            terrain_view: terrain_view_from_environment(),
            render_settings: astrum_renderer::RenderSettings::default(),
            pending: VecDeque::new(),
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
            all_history: false,
            full_trails: false,
            manual_speed: 1.0,
            navigation: NavigationInput::default(),
            viewport_input: navigation_input::ViewportInput::default(),
            input_focused: true,
            native_events: false,
            pixels_per_point: 1.0,
            layout: LabelLayout::default(),
            placed: Vec::new(),
            pick_cycle: PickCycle::default(),
            gesture_start: None,
            gesture_dragged: false,
            log: VecDeque::new(),
            logged_diagnostic: None,
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
    /// Planet editor state of world-map bodies (M1 Step 6).
    planet_editor: crate::planet_editor::PlanetEditor,
    presentation: Vec<crate::shared_system::BodyPresentation>,
    /// Authored light and surface reflectance.
    lighting: crate::scene_lighting::SceneLighting,
    /// Index of the lighting star in body order.
    sun_index: usize,
    scene_sha256: String,
    camera_sha256: String,
    surface_owners: Vec<bool>,
    system: CelestialSystem,
    motion: MotionSession,
    projection: CelestialFrameProjection,
    camera: CelestialCamera,
    ids: Vec<BodyId>,
    selection: BodySelection,
    // Read by projection rebuilds, which only tests exercise today.
    #[cfg_attr(not(test), allow(dead_code))]
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
    /// Markers of the latest submitted frame, for viewport picking.
    last_markers: Vec<CelestialMarker>,
    /// View model of the latest frame for the Studio UI.
    view: crate::studio::view::StudioView,
    /// UI toolkit render time of the previous frame, reported by the host.
    ui_render_ms: Option<f64>,
    /// Sun elevation above the local horizon of the nearest body at the
    /// camera: (body index, degrees).
    sun_elevation: Option<(usize, f64)>,
}
struct VisualCurve {
    points: Vec<FramePosition>,
    colors: Vec<[f32; 4]>,
    width: f32,
    style: CelestialLineStyle,
    relative: Vec<DVec3>,
}

impl GravityOrbitsDemo {
    /// Star disk luminance that reproduces the authored illuminance at its
    /// reference distance, times the session sun scale.
    fn sun_disk_radiance(&self) -> f64 {
        let radius = self
            .system
            .body(self.ids[self.sun_index])
            .map_or(1.0, |body| body.properties().reference_radius_m());
        let light = astrum_renderer::FrameLighting {
            sun_center_view_m: DVec3::ZERO,
            sun_radius_m: radius,
            sun_color: self.lighting.sun_color,
            illuminance_lux: self.lighting.illuminance_lux,
            reference_distance_m: self.lighting.reference_distance_m,
            ambient_lux: 0.0,
            ambient_color: [1.0; 3],
            bounce_fraction: 0.0,
            sky_fraction: 0.0,
            sky_color: [1.0; 3],
            occluders: Vec::new(),
        };
        light.sun_disk_radiance() * f64::from(self.controls.render_settings.lighting.sun_scale)
    }
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

/// Terrain view selected at launch; `ASTRUM_TERRAIN_MODE` takes a view-mode name.
fn terrain_view_from_environment() -> TerrainViewMode {
    std::env::var("ASTRUM_TERRAIN_MODE")
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
            collision,
            motion: definition,
            presentation,
            initial_body_index: selected,
            camera: camera_state,
            scene_sha256,
            camera_sha256,
        } = loaded;
        let lighting = crate::scene_lighting::SceneLighting::load_canonical()?;
        let sun_index = presentation
            .iter()
            .position(|body| body.semantic_id == lighting.sun_body)
            .with_context(|| {
                format!(
                    "lighting sun body {} is not in the system",
                    lighting.sun_body
                )
            })?;
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
        // The GPU colliders are not read back before the first frame, so the
        // canonical pose is placed as authored and checked once its surface
        // resolves (ADR 0023).
        camera.developer_set_surface_pose(&pair, body, pose)?;
        camera.enter_surface_inspection(&pair, body)?;
        Ok(Self {
            #[cfg(feature = "developer-tools")]
            developer_session: None,
            #[cfg(feature = "developer-tools")]
            developer_snapshot: None,
            #[cfg(feature = "developer-tools")]
            #[cfg(feature = "developer-tools")]
            developer_navigation: None,
            navigation_snapshot_path: std::env::var_os("ASTRUM_NAVIGATION_SNAPSHOT")
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
            atlas: crate::planet_lod::PlanetLod::new(Some(lod)).with_collision(collision),
            planet_editor: crate::planet_editor::PlanetEditor::new(&presentation),
            presentation,
            lighting,
            sun_index,
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
            last_markers: Vec::new(),
            view: Default::default(),
            ui_render_ms: None,
            sun_elevation: None,
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
    /// Viewport input from the UI toolkit, in viewport logical pixels.
    /// Camera policy and cancellation remain app-owned.
    pub fn viewport_event(&mut self, event: navigation_input::ViewportEvent) {
        self.controls.native_events = true;
        use navigation_input::{PointerButton, ViewportEvent};
        match event {
            ViewportEvent::FocusChanged(focused) => {
                self.controls.input_focused = focused;
                if !focused {
                    self.cancel_navigation_input();
                    self.reset_wall_capture();
                }
                return;
            }
            ViewportEvent::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
            } => {
                self.controls.gesture_start = Some(pos);
                self.controls.gesture_dragged = false;
            }
            ViewportEvent::PointerMoved(pos) => {
                if let Some(start) = self.controls.gesture_start
                    && (pos[0] - start[0]).hypot(pos[1] - start[1]) > 4.0
                {
                    self.controls.gesture_dragged = true;
                }
            }
            _ => {}
        }
        if self.hidden {
            return;
        }
        let local_look = matches!(
            self.camera.mode(),
            CameraMode::SurfaceInspection | CameraMode::FreeFlight
        );
        let inputs = self.controls.viewport_input.events(
            &[event],
            local_look,
            !self.controls.input_focused,
            self.controls.manual_speed,
        );
        let at = Instant::now();
        for input in inputs {
            self.controls
                .pending
                .push_back(Command::TimedNavigation(at, input));
        }
    }

    /// Keyboard shortcut from the viewport.
    pub fn viewport_shortcut(&mut self, shortcut: crate::studio::view::Shortcut) {
        use crate::studio::view::Shortcut;
        let at = self.selected_index();
        let count = self.ids.len();
        let command = match shortcut {
            Shortcut::Focus => Command::Focus {
                fixed: false,
                fit: false,
            },
            Shortcut::SurfaceNavigation => Command::SurfaceInspection,
            Shortcut::Horizon => Command::SurfaceHorizon,
            Shortcut::Overview => Command::Overview,
            Shortcut::FreeFlight => Command::FreeFlight,
            Shortcut::NextBody => Command::Select(self.ids[(at + 1) % count]),
            Shortcut::PreviousBody => Command::Select(self.ids[(at + count - 1) % count]),
        };
        self.controls.pending.push_back(command);
    }

    /// Click in the viewport (logical pixels): picks the body under the cursor.
    /// A press that turned into a drag is a navigation gesture, not a pick.
    pub fn viewport_click(&mut self, pos: [f32; 2], double: bool) {
        let dragged = self.controls.gesture_dragged;
        self.controls.gesture_start = None;
        self.controls.gesture_dragged = false;
        if dragged {
            return;
        }
        let Ok(projection) = self.content_projection(0.1) else {
            return;
        };
        let scale = self.controls.pixels_per_point;
        let targets: Vec<_> = self
            .last_markers
            .iter()
            .filter_map(|m| {
                let body = *self.ids.get(m.request_index)?;
                Some(BodyHitTarget {
                    body,
                    marker: if self.controls.markers || m.request_index == self.selected_index() {
                        m.screen_pixels.map(|p| p.map(f64::from))
                    } else {
                        None
                    },
                    marker_radius_pixels: 8.0,
                    label: self
                        .controls
                        .placed
                        .iter()
                        .find(|l| l.body == body)
                        .map(|l| l.rect),
                    center_in_view_m: m.center_in_view_m,
                    radius_m: self
                        .system
                        .body(body)
                        .ok()?
                        .properties()
                        .reference_radius_m(),
                    occluded_overlay: m.occluded,
                })
            })
            .collect();
        let pixels = [f64::from(pos[0]) * scale, f64::from(pos[1]) * scale];
        if let Ok(pick) = pick_body(&targets, pixels, projection)
            && let Some(body) = self.controls.pick_cycle.choose(&pick, pixels)
        {
            self.controls.pending.push_back(Command::Select(body));
            if double {
                self.controls.pending.push_back(Command::Focus {
                    fixed: false,
                    fit: false,
                });
            }
        }
    }

    /// Alt+click (logical pixels): selects the globe under the cursor and places the
    /// camera above that point in surface inspection, looking toward the horizon.
    pub fn viewport_fly_to(&mut self, pos: [f32; 2]) {
        self.controls.gesture_start = None;
        self.controls.gesture_dragged = false;
        let scale = self.controls.pixels_per_point;
        let pixels = [f64::from(pos[0]) * scale, f64::from(pos[1]) * scale];
        match self.fly_to(pixels) {
            Ok(Some(text)) => self.log(crate::studio::view::Tone::Normal, text),
            Ok(None) => {}
            Err(error) => self.log(
                crate::studio::view::Tone::Warn,
                format!("Fly to point failed: {error:#}"),
            ),
        }
    }

    /// `None` when the pointer is not over a body with a surface.
    fn fly_to(&mut self, pixels: [f64; 2]) -> Result<Option<String>> {
        // Pitch below the horizon and clearance bounds for "inspecting the map".
        const PITCH_DOWN_RAD: f64 = 25.0 * std::f64::consts::PI / 180.0;
        let projection = self.content_projection(0.1)?;
        let pair = self.projection.coherent_view(&self.system)?;
        let camera_pose = self.camera.pose();
        let mut targets = Vec::new();
        for m in &self.last_markers {
            let Some(&body) = self.ids.get(m.request_index) else {
                continue;
            };
            let celestial = self.system.body(body)?;
            if !celestial.has_surface() {
                continue;
            }
            let fixed = pair.projection().frames_for(body)?.body_fixed;
            targets.push(SurfacePickTarget {
                body,
                center_in_view_m: m.center_in_view_m,
                radius_m: celestial.properties().reference_radius_m(),
                body_fixed_from_view: pair
                    .evaluation()
                    .reexpress_pose(camera_pose, fixed)?
                    .orientation()
                    .quaternion(),
            });
        }
        let Some(hit) = pick_surface_point(&targets, pixels, projection)? else {
            return Ok(None);
        };
        let body = hit.body;
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let radius = self.system.body(body)?.properties().reference_radius_m();
        let clearance = (radius / 50.0).clamp(2_000.0, 50_000.0);
        let up = hit.point_body_m.normalize();
        // Keep looking the way the camera already faces, projected onto the horizon.
        let current = pair
            .evaluation()
            .reexpress_pose(camera_pose, fixed)?
            .orientation()
            .quaternion()
            * -DVec3::Z;
        let heading = (current - up * current.dot(up))
            .try_normalize()
            .unwrap_or_else(|| up.any_orthonormal_vector());
        let forward = heading * PITCH_DOWN_RAD.cos() - up * PITCH_DOWN_RAD.sin();
        let right = forward.cross(up).normalize();
        let view_up = right.cross(forward);
        let pose = FramePose::new(
            FramePosition::new(
                fixed,
                LocalPosition::try_metres(up * (radius + clearance))?,
            ),
            UnitRotation::try_from_quaternion(glam::DQuat::from_mat3(&glam::DMat3::from_cols(
                right, view_up, -forward,
            )))?,
        );
        // Validate on a candidate so a failed placement cannot alter the observer.
        // A pending surface places the pose as authored (ADR 0023).
        let mut camera = self.camera.clone();
        let measured = camera
            .query_surface(&pair, body, pose)?
            .map(|surface| surface.clearance_m);
        camera.developer_set_surface_pose(&pair, body, pose)?;
        if let Some(measured) = measured {
            camera.target_clearance(&pair, measured)?;
        }
        camera.enter_surface_inspection(&pair, body)?;
        self.camera = camera;
        self.auto_fit = false;
        self.selection.select(&self.system, body)?;
        self.controls.refresh_draft(self.system.body(body)?);
        let name = self.system.body(body)?.name().to_owned();
        Ok(Some(format!(
            "Flew to {name} at {:.1} km above the point",
            clearance / 1000.0
        )))
    }

    /// Click on a placed body label (index into the published labels).
    pub fn viewport_label_click(&mut self, index: usize, double: bool) {
        let Some(label) = self.controls.placed.get(index) else {
            return;
        };
        self.controls.pending.push_back(Command::Select(label.body));
        if double {
            self.controls.pending.push_back(Command::Focus {
                fixed: false,
                fit: false,
            });
        }
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
            #[cfg(feature = "developer-tools")]
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
            }
            #[cfg(feature = "developer-tools")]
            Command::BodyOrbit => {
                self.camera
                    .enter_body_orbit(&self.projection.coherent_view(&self.system)?, id)?;
            }
            Command::SurfaceHorizon => self.camera.look_surface_horizon()?,
            Command::Clearance(clearance) => {
                self.camera
                    .target_clearance(&self.projection.coherent_view(&self.system)?, clearance)?;
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
            #[cfg(feature = "developer-tools")]
            Command::CancelNavigation => self.cancel_navigation_input(),
            #[cfg(test)]
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
    /// Prepares one frame and renders the scene into `renderer`'s viewport
    /// texture of `width` x `height` physical pixels; `pixels_per_point` is the
    /// UI scale used for input and labels.
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        width: u32,
        height: u32,
        pixels_per_point: f32,
    ) -> Result<()> {
        renderer.resize(width, height);
        self.render_host(
            &mut frame_host::FrameHost {
                renderer,
                pixels_per_point,
            },
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
        crate::engine_profile::set_enabled(self.controls.profiler.enabled);
        if crate::engine_profile::is_enabled() {
            renderer.request_profile_timing();
        }
        crate::engine_profile::begin_frame(self.developer_frame_number);
        let frame_span = crate::engine_profile::span("Frame");
        let now = Instant::now();
        self.publish_planet_edits(now);
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
            renderer.render_empty()?;
            self.publish_view(None, &[]);
            return Ok(());
        }
        let scale = renderer.pixels_per_point() as f64;
        // The scene texture is exactly the viewport: origin (0, 0), full size.
        let new_viewport = Some(([0, 0], [width, height]));
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
        {
            let _span = crate::engine_profile::span("Orbit visuals");
            self.prepare_visuals()
                .context("orbit visual preparation failed")?;
        }
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
        let mut centres: Vec<(DVec3, f64)> = Vec::with_capacity(self.ids.len());
        let sun_disk_nits = self.sun_disk_radiance();
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
            centres.push((center, radius));
            let presentation = &self.presentation[index];
            self.requests.push(CelestialRenderBody {
                body_fixed_frame: frames.body_fixed,
                reference_radius_m: radius,
                color: presentation.color,
                unlit: presentation.unlit,
                selected: index == selected_index,
                // A world-map body seen as a plain sphere shows its archetype's
                // average colour (pipeline §6.1).
                material: match body.surface_definition().and_then(|d| d.world()) {
                    Some(world) => {
                        let c = world.archetype.average_colour;
                        astrum_renderer::SurfaceMaterial {
                            albedo: [c.0, c.1, c.2],
                            ..self.lighting.material(&presentation.semantic_id)
                        }
                    }
                    None => self.lighting.material(&presentation.semantic_id),
                },
                emission_nits: if index == self.sun_index {
                    sun_disk_nits as f32
                } else {
                    0.0
                },
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
        frame.set_view_mode(self.controls.terrain_view);
        let overlays = self.controls.render_settings.overlays;
        frame.set_line_style(astrum_renderer::LineStyleScale {
            width: overlays.line_width_scale,
            opacity: overlays.opacity,
        });
        let (mut sun_centre, sun_radius) = centres[self.sun_index];
        let lighting_settings = self.controls.render_settings.lighting;
        if lighting_settings.studio_sun
            && let Some((index, (centre, _))) = centres
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != self.sun_index)
                .min_by(|a, b| (a.1.0.length() - a.1.1).total_cmp(&(b.1.0.length() - b.1.1)))
        {
            // Local horizon at the camera over the nearest body; azimuth is
            // measured from the view direction so the light stays where the
            // reviewer wants it while turning.
            let up = -centre.normalize();
            let forward = DVec3::NEG_Z - up * DVec3::NEG_Z.dot(up);
            let forward = if forward.length_squared() < 1e-6 {
                let frames = pair.projection().frames_for(self.ids[index])?;
                let axis = view
                    .prepare_source(frames.body_fixed)?
                    .view_direction(Direction3::try_new(DVec3::Y)?)?
                    .unit();
                (axis - up * axis.dot(up)).normalize_or(up.any_orthonormal_vector())
            } else {
                forward.normalize()
            };
            let right = forward.cross(up).normalize();
            let (elevation, azimuth) = (
                f64::from(lighting_settings.sun_elevation_deg).to_radians(),
                f64::from(lighting_settings.sun_azimuth_deg).to_radians(),
            );
            let direction = elevation.cos() * (azimuth.cos() * forward + azimuth.sin() * right)
                + elevation.sin() * up;
            sun_centre = direction * sun_centre.length();
        }
        self.sun_elevation = centres
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.sun_index)
            .min_by(|a, b| (a.1.0.length() - a.1.1).total_cmp(&(b.1.0.length() - b.1.1)))
            .map(|(index, (centre, _))| {
                let up = -centre.normalize();
                let sun = sun_centre.normalize();
                (index, up.dot(sun).clamp(-1.0, 1.0).asin().to_degrees())
            });
        frame.set_lighting(astrum_renderer::FrameLighting {
            sun_center_view_m: sun_centre,
            sun_radius_m: sun_radius,
            sun_color: self.lighting.sun_color,
            illuminance_lux: self.lighting.illuminance_lux,
            reference_distance_m: self.lighting.reference_distance_m,
            ambient_lux: self.lighting.ambient_lux,
            ambient_color: self.lighting.ambient_color,
            bounce_fraction: self.lighting.bounce_fraction,
            sky_fraction: self.lighting.sky_fraction,
            sky_color: self.lighting.sky_color,
            occluders: centres
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != self.sun_index)
                .map(|(_, occluder)| *occluder)
                .take(astrum_renderer::MAX_OCCLUDERS)
                .collect(),
        })?;
        renderer.set_render_settings(self.controls.render_settings)?;
        if self.controls.terrain_preview {
            let _span = crate::engine_profile::span("Atlas terrain preparation");
            self.atlas
                .receive_bounds(renderer.take_terrain_atlas_bounds());
            self.atlas
                .receive_ready_sources(renderer.take_terrain_ready_sources());
            // PROTOTYPE (M3 Water): hydrology on read-back world bakes.
            self.atlas
                .receive_world_fields(renderer.take_terrain_world_fields());
            for (source, words) in self.atlas.take_world_rivers() {
                renderer.set_terrain_world_rivers(source, words);
            }
            // Read-back colliders (ADR 0023): store arrived pages and schedule
            // the pages under last frame's camera query misses.
            self.atlas
                .receive_collision(renderer.take_terrain_collision_pages());
            self.atlas
                .request_collision(self.camera.take_surface_misses());
            let mut inputs = Vec::new();
            for (index, &id) in self.ids.iter().enumerate() {
                let body = pair.system().body(id)?;
                let Some(definition) = body.surface_definition() else {
                    continue;
                };
                inputs.push(crate::planet_lod::AtlasBodyInput {
                    body: id,
                    body_fixed_frame: pair.projection().frames_for(id)?.body_fixed,
                    definition,
                    radius_m: body.properties().reference_radius_m(),
                    revision: body.terrain_revision().value(),
                    material: self
                        .lighting
                        .material(&self.presentation[index].semantic_id),
                });
            }
            let shadow_settings = self.controls.render_settings.shadows;
            let atlas_frame = self.atlas.prepare(
                &view,
                projection,
                &inputs,
                self.controls.terrain_view.shader_mode(),
                renderer.terrain_atlas_layer_limit(),
                shadow_settings
                    .enabled
                    .then_some(crate::planet_lod::ShadowCasterPolicy {
                        settings: shadow_settings,
                        sun_centre_view_m: sun_centre,
                    }),
            )?;
            for body in self.atlas.drawn_bodies() {
                if let Some(index) = self.ids.iter().position(|id| id == body) {
                    self.surface_owners[index] = true;
                }
            }
            frame.set_terrain_atlas(atlas_frame);
            self.camera.refresh_surface(self.atlas.collider_view());
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
            renderer.render_empty()?;
            self.publish_view(None, &[]);
            return Ok(());
        }
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
        let snapshot_span = crate::engine_profile::span("Snapshot collection");
        let mut snapshot = DeveloperSnapshot::collect(SnapshotInput {
            navigation: Some(NavigationDiagnostics {
                window_focused: Some(self.controls.input_focused),
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
        let shadows = renderer.shadow_report();
        let aa = renderer.anti_aliasing_report();
        let adapter = renderer.adapter_label();
        snapshot.render_settings = Some(serde_json::json!({
            "settings": crate::render_settings::listing(&crate::render_settings::RenderState {
                settings: self.controls.render_settings,
                view_mode: self.controls.terrain_view,
            }),
            "lighting_sha256": self.lighting.sha256,
            "sun": self.sun_elevation.map(|(index, elevation)| serde_json::json!({
                "body": self.presentation[index].semantic_id,
                "elevation_deg": elevation,
            })),
            "shadows": {
                "cascades": shadows.cascades,
                "casters": shadows.casters,
                "splits_m": shadows.splits_m,
                "texel_m": shadows.texel_m,
            },
            "adapter": adapter,
            "anti_aliasing": {
                "requested_samples": aa.requested_samples,
                "samples": aa.samples,
                "supported_mask": aa.supported_mask,
                "fxaa": aa.fxaa,
                "last_compile_ms": aa.last_compile_ms,
            },
        }));
        // Opt-in native evidence scratch export of this exact prepared state.
        // Disabled for ordinary launches; collection remains observational.
        if let Some(path) = &self.navigation_snapshot_path {
            snapshot.write_json(path)?;
        }
        drop(snapshot_span);
        let render_started = Instant::now();
        let submission_span = crate::engine_profile::span("GPU submission");
        let markers = frame.markers().to_vec();
        renderer.render_celestial(&frame)?;
        drop(submission_span);
        snapshot.performance.render_present_ms =
            Some(render_started.elapsed().as_secs_f64() * 1000.0);
        let ui_started = Instant::now();
        {
            let _span = crate::engine_profile::span("UI view");
            self.publish_view(Some(&snapshot), &markers);
        }
        snapshot.performance.ui_build_cpu_ms = Some(ui_started.elapsed().as_secs_f64() * 1000.0);
        snapshot.performance.ui_render_ms = self.ui_render_ms;
        let native_cpu = renderer.native_render_timings();
        snapshot.performance.native_render_cpu_ms = Some([
            native_cpu.poll_ms,
            native_cpu.scene_encode_ms,
            native_cpu.submit_ms,
            native_cpu.total_ms,
        ]);
        snapshot.performance.native_submission_id = renderer.native_submission_id();
        snapshot.performance.native_gpu_timestamp_sampling = renderer.native_timestamp_sampling();
        snapshot.performance.native_presentation_mode = Some(renderer.presentation_mode_label());
        snapshot.performance.native_redraw_uncapped = cfg!(feature = "developer-tools")
            && std::env::var("ASTRUM_UNCAPPED").is_ok_and(|value| value == "1");
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
        if std::mem::take(&mut self.controls.profiler.capture_requested) {
            self.performance_capture.request();
        }
        if std::mem::take(&mut self.controls.profiler.capture_stop_requested) {
            self.performance_capture.stop();
        }
        snapshot.performance.profiler_publication_ms =
            Some(profiler_started.elapsed().as_secs_f64() * 1000.0);
        snapshot.performance.host_frame_ms =
            Some(host_frame_started.elapsed().as_secs_f64() * 1000.0);
        self.controls.profiler.observe_frame(&snapshot);
        if profile_sample_due {
            self.controls.profiler.ingest(&snapshot);
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

#[cfg(test)]
mod analytic_publication_tests {
    use super::*;
    #[test]
    fn shared_camera_matches_authored_pose_and_survives_idle_motion() {
        let loaded =
            crate::shared_system::SharedTestSystem::load_canonical(NonZeroU64::new(71).unwrap())
                .unwrap();
        let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
        // ADR 0023: the runtime camera reads read-back GPU colliders, never the
        // CPU oracle.
        assert!(matches!(
            demo.camera.surface_source(),
            crate::celestial_camera::SurfaceSource::Colliders(_)
        ));
        let expected = DVec3::from_array(loaded.camera.position_body_m);
        assert!(
            (demo.camera.pose().position().local().metres() - expected).length() < 1e-8,
            "{} vs {expected}",
            demo.camera.pose().position().local().metres()
        );
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
        assert!(
            (demo.camera.pose().position().local().metres() - expected).length() < 1e-8,
            "{} vs {expected}",
            demo.camera.pose().position().local().metres()
        );
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
