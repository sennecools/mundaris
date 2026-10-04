//! Composition of authoritative physics, coherent projection and disposable debug views.
use crate::planet_surface::*;
use crate::{celestial_camera::*, gravity_fixtures::*, trails::*};
use crate::{
    celestial_labels::*, celestial_selection::*, interactive_clock::*, orbit_guides::*,
    playback_metrics::*, system_view::*,
};
use anyhow::Result;
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::planet_surface::*;
use mundaris_renderer::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{
    collections::VecDeque,
    num::NonZeroU64,
    time::{Duration, Instant},
};

enum Command {
    ValidationRoute,
    LookBody(BodyId),
    SurfaceInspection,
    SurfaceHorizon,
    Clearance(f64),
    TerrainGuard(Option<f64>),
    Approach,
    Pause(bool),
    Rate(f64),
    ResumeAdmission,
    Single(bool),
    Seek(u64),
    CancelSeek,
    Reset,
    Load(GravityFixture),
    Select(BodyId),
    Focus { fixed: bool, fit: bool },
    Overview,
    Subsystem,
    ReferenceOverview,
    FreeFlight,
    GuideReference(OrbitGuideReference),
    Navigation(NavigationInput),
    GapThreshold(Duration),
    FilteredOverview(Vec<BodyId>),
    Reexpress(bool),
    Rebuild,
    Mass(f64),
    Radius(f64),
    Velocity(DVec3),
    Rename(String),
    TrailMode(bool),
    TerrainPreview(bool),
    TrailReference(BodyId),
}
struct Controls {
    terrain_preview: bool,
    sun_from_star: bool,
    terrain_lighting: TerrainLighting,
    terrain_morph_ms: u64,
    surface_bounds: bool,
    surface_style: SurfaceStyle,
    approach: Option<(f64, f64, Duration)>,
    clearance_target: f64,
    terrain_guard_m: Option<f64>,
    reference_sea_level_m: Option<f64>,
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
            terrain_preview: false,
            sun_from_star: false,
            terrain_lighting: terrain_lighting_from_environment(),
            terrain_morph_ms: std::env::var("MUNDARIS_TERRAIN_MORPH_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(150)
                .min(1000),
            surface_bounds: false,
            surface_style: SurfaceStyle::default(),
            approach: None,
            clearance_target: 1e11,
            terrain_guard_m: None,
            reference_sea_level_m: None,
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
    terrain_clearance: Option<crate::terrain_inspection::TerrainClearance>,
    ready_mesh_probe: Option<crate::surface_probe::ReadyMeshProbe>,
    clearance_query_us: f64,
    terrain: crate::terrain_population::TerrainPopulation,
    scenario: GravityFixture,
    validation_route: Option<SurfaceValidationRoute>,
    surfaces: Vec<PlanetSurfaceSession>,
    surface_owners: Vec<bool>,
    system: CelestialSystem,
    runner: FixedStepRunner,
    projection: CelestialFrameProjection,
    camera: CelestialCamera,
    ids: Vec<BodyId>,
    selection: BodySelection,
    system_namespace: u64,
    tree_namespace: u64,
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
    diagnostics: SystemDiagnostics,
    baseline: DiagnosticBaseline,
    sampled_tick: u64,
    diagnostic_elapsed: Duration,
    advance: SimulationAdvanceReport,
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
struct SurfaceValidationRoute {
    elapsed: Duration,
    next: usize,
    physical_target: u64,
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

fn terrain_lighting_from_environment() -> TerrainLighting {
    terrain_lighting_configuration(
        std::env::var("MUNDARIS_TERRAIN_MODE").ok().as_deref(),
        std::env::var("MUNDARIS_TERRAIN_SUN").ok().as_deref(),
    )
}

fn terrain_lighting_configuration(mode: Option<&str>, sun: Option<&str>) -> TerrainLighting {
    let mut lighting = TerrainLighting::default();
    let preset = match sun {
        Some("overhead") => Some(TerrainSunPreset::Overhead),
        Some("side") => Some(TerrainSunPreset::Side),
        Some("grazing") => Some(TerrainSunPreset::Grazing),
        Some("terminator") => Some(TerrainSunPreset::Terminator),
        Some("night") => Some(TerrainSunPreset::Night),
        _ => None,
    };
    let sun = preset.map_or_else(
        || lighting.sun_direction_body(),
        |preset| preset.direction_body(),
    );
    let mode = match mode {
        Some("elevation") => TerrainRenderMode::Elevation,
        Some("lit") => TerrainRenderMode::Lit,
        Some("normals") => TerrainRenderMode::Normals,
        Some("diffuse") => TerrainRenderMode::Diffuse,
        Some("readability" | "surface") => TerrainRenderMode::Readability,
        Some("slope") => TerrainRenderMode::Slope,
        Some("sea-mask") => TerrainRenderMode::SeaMask,
        Some("rock-weight") => TerrainRenderMode::RockWeight,
        _ => lighting.mode(),
    };
    lighting = TerrainLighting::try_new(
        sun,
        lighting.ambient_strength(),
        lighting.diffuse_strength(),
        mode,
    )
    .unwrap_or_default();
    lighting
}

impl GravityOrbitsDemo {
    pub fn new() -> Result<Self> {
        let mut demo = Self::create(GravityFixture::Hierarchy, 1, 1)?;
        if std::env::var("MUNDARIS_PHASE4_VALIDATE").as_deref() == Ok("1") {
            demo.start_surface_validation()?;
        }
        if std::env::var("MUNDARIS_PHASE5_TERRAIN").as_deref() == Ok("1") {
            demo.command(Command::TerrainPreview(true))?;
        }
        Ok(demo)
    }
    /// Ordinary development content; the original physical fixtures remain selectable.
    pub fn solar_system(real_scale: bool) -> Result<Self> {
        let scenario = if real_scale {
            GravityFixture::RealSolarSystem
        } else {
            GravityFixture::GameplaySolarSystem
        };
        let mut demo = Self::create(scenario, 1, 1)?;
        demo.command(Command::TerrainPreview(true))?;
        Ok(demo)
    }
    /// Optional operator route through the same production controls and renderer.
    pub fn start_surface_validation(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.ids.len() == 3,
            "surface route requires the hierarchy fixture"
        );
        let physical_target = self
            .runner
            .tick()
            .checked_add(20)
            .ok_or_else(|| anyhow::anyhow!("validation tick overflow"))?;
        self.controls.pending.push_back(Command::Pause(true));
        self.controls.pending.push_back(Command::Overview);
        self.validation_route = Some(SurfaceValidationRoute {
            elapsed: Duration::ZERO,
            next: 0,
            physical_target,
        });
        Ok(())
    }
    fn create(
        scenario: GravityFixture,
        system_namespace: u64,
        tree_namespace: u64,
    ) -> Result<Self> {
        let system = scenario.create(
            NonZeroU64::new(system_namespace)
                .ok_or_else(|| anyhow::anyhow!("zero system namespace"))?,
        )?;
        let mut runner =
            FixedStepRunner::new(&system, SimulationConfig::try_new(scenario.fixed_step_s())?)?;
        runner.set_rate(PlaybackRate::try_multiplier(1000.0)?);
        let projection = CelestialFrameProjection::build(
            &system,
            NonZeroU64::new(tree_namespace)
                .ok_or_else(|| anyhow::anyhow!("zero tree namespace"))?,
        )?;
        let diagnostics = system_diagnostics(&system)?;
        let mut guides = OrbitGuides::default();
        guides.update(&system);
        let bounds = SystemViewBounds::calculate(
            &system,
            &OverviewScope::WholeSystem,
            guides.guides(),
            &[],
            false,
        )?;
        let center = bounds.center_m();
        let extent = bounds.radius_m();
        let camera =
            CelestialCamera::overview(&projection.coherent_view(&system)?, center, extent)?;
        let ids: Vec<_> = system.bodies().map(|(id, _)| id).collect();
        let selected = if matches!(
            scenario,
            GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
        ) {
            3
        } else {
            1
        };
        let mut selection = BodySelection::default();
        selection.select(&system, ids[selected])?;
        let mut controls = Controls::new(system.body(ids[selected])?);
        if matches!(
            scenario,
            GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
        ) {
            controls.terrain_preview = true;
            if std::env::var("MUNDARIS_TERRAIN_MODE").is_err() {
                controls.terrain_lighting = controls
                    .terrain_lighting
                    .with_mode(TerrainRenderMode::Readability);
            }
            controls.sun_from_star = std::env::var("MUNDARIS_TERRAIN_SUN").is_err();
            controls.surface_style.elevation_colors = true;
        }
        let advance = runner.report();
        let trails = TrailHistory::new(&system, scenario.trail_stride())?;
        let enabled = match scenario {
            GravityFixture::Hierarchy => vec![ids[1], ids[2]],
            GravityFixture::Circular => vec![ids[1]],
            GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem => system
                .bodies()
                .filter_map(|(id, body)| body.terrain().map(|_| id))
                .collect(),
        };
        let surfaces = enabled
            .into_iter()
            .map(|id| PlanetSurfaceSession::new(id, 2048))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            terrain_clearance: None,
            ready_mesh_probe: None,
            clearance_query_us: 0.0,
            terrain: crate::terrain_population::TerrainPopulation::interactive()?,
            scenario,
            validation_route: None,
            surfaces,
            surface_owners: Vec::new(),
            system,
            runner,
            projection,
            camera,
            ids,
            selection,
            system_namespace,
            tree_namespace,
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
            baseline: DiagnosticBaseline(diagnostics),
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
            self.runner.set_lifecycle_suspended(!drawable);
            self.last_wall = None;
            self.hidden = !drawable;
            self.achieved_elapsed = Duration::ZERO;
            self.achieved_rate = None;
            self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
            self.advance = self.runner.report();
            self.metrics.reset();
            self.camera.cancel_transition();
        }
    }
    pub fn reset_wall_capture(&mut self) {
        self.last_wall = None;
        self.clock.reset_capture();
    }
    fn seed_trails(&mut self) {
        self.trails.clear_and_seed(
            self.runner.branch_generation(),
            self.runner.tick(),
            &self.system,
        );
    }
    fn reseed_diagnostics(&mut self) -> Result<()> {
        self.metrics.reset();
        self.diagnostics = system_diagnostics(&self.system)?;
        self.baseline = DiagnosticBaseline(self.diagnostics);
        self.sampled_tick = self.runner.tick();
        self.diagnostic_elapsed = Duration::ZERO;
        self.achieved_elapsed = Duration::ZERO;
        self.achieved_rate = None;
        self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
        Ok(())
    }
    fn sample_diagnostics(&mut self) -> Result<()> {
        self.diagnostics = system_diagnostics(&self.system)?;
        self.sampled_tick = self.runner.tick();
        self.diagnostic_elapsed = Duration::ZERO;
        Ok(())
    }
    fn refresh_report_metadata(&mut self) {
        self.advance = SimulationAdvanceReport {
            work_steps: self.advance.work_steps,
            forward_steps: self.advance.forward_steps,
            restored_steps: self.advance.restored_steps,
            force_passes: self.advance.force_passes,
            pair_evaluations: self.advance.pair_evaluations,
            ..self.runner.report()
        };
    }
    fn command(&mut self, command: Command) -> Result<()> {
        let id = self
            .selection
            .selected()
            .ok_or_else(|| anyhow::anyhow!("no selected body"))?;
        match command {
            Command::ValidationRoute => self.start_surface_validation()?,
            Command::LookBody(target) => self
                .camera
                .look_at_body(&self.projection.coherent_view(&self.system)?, target)?,
            Command::SurfaceInspection => {
                anyhow::ensure!(
                    self.surfaces.iter().any(|s| s.body() == id),
                    "selected body has no surface capability"
                );
                self.camera
                    .enter_surface_inspection(&self.projection.coherent_view(&self.system)?, id)?;
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
                self.runner.set_paused(paused);
                self.seeking = false;
                self.sample_diagnostics()?;
                self.metrics.reset();
                self.gap_diagnostic = None;
                self.last_wall = None;
            }
            Command::Rate(rate) => {
                let rate = PlaybackRate::try_multiplier(rate)?;
                if self.runner.rate().multiplier().signum() != rate.multiplier().signum() {
                    self.seed_trails();
                }
                self.runner.set_rate(rate);
                self.metrics.reset();
            }
            Command::ResumeAdmission => self.runner.resume_admission(),
            Command::Single(forward) => {
                self.metrics.reset();
                self.runner.single_step(forward)?;
                self.seeking = false;
            }
            Command::Seek(tick) => {
                self.metrics.reset();
                self.runner.seek_tick(tick)?;
                self.seed_trails();
                self.seeking = tick != self.runner.tick();
                if !self.seeking {
                    self.sample_diagnostics()?;
                }
            }
            Command::CancelSeek => {
                self.metrics.reset();
                self.runner.cancel_seek();
                self.seeking = false;
                self.sample_diagnostics()?;
            }
            Command::Reset => {
                self.metrics.reset();
                self.runner.reset_branch(&mut self.system)?;
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Load(scenario) => {
                let system_namespace = self
                    .system_namespace
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("system namespace overflow"))?;
                let tree_namespace = self
                    .tree_namespace
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("tree namespace overflow"))?;
                let replacement = Self::create(scenario, system_namespace, tree_namespace)?;
                *self = replacement;
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
                self.guides.update(&self.system);
            }
            Command::Navigation(input) => {
                if input.drag != [0.0; 2]
                    || input.scroll_notches != 0.0
                    || input.translation != DVec3::ZERO
                {
                    self.auto_fit = false;
                    self.controls.approach = None;
                }
                self.controls.navigation = input;
            }
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
                self.runner.edit_properties(
                    &mut self.system,
                    id,
                    BodyProperties::new(mass, radius)?,
                )?;
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Radius(radius) => {
                let mass = self.system.body(id)?.properties().mass_kg();
                self.runner.edit_properties(
                    &mut self.system,
                    id,
                    BodyProperties::new(mass, radius)?,
                )?;
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Velocity(velocity) => {
                let old = *self.system.body(id)?.state();
                let state = BodyState::new(
                    old.center_in_system(),
                    LinearVelocity3::try_metres_per_second(velocity)?,
                    old.body_to_system(),
                    old.angular_velocity_in_system(),
                );
                self.runner.edit_state(&mut self.system, id, state)?;
                self.seed_trails();
                self.seeking = false;
                self.reseed_diagnostics()?;
            }
            Command::Rename(name) => self.runner.rename(&mut self.system, id, &name)?,
            Command::TerrainPreview(enabled) => {
                if enabled
                    && !matches!(
                        self.scenario,
                        GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
                    )
                {
                    let id = self.ids[1];
                    let body = self.system.body(id)?;
                    if body.terrain().is_none() {
                        let version =
                            if std::env::var("MUNDARIS_PHASE55_AB").as_deref() == Ok("legacy") {
                                mundaris_world::terrain::TerrainGeneratorVersion::V1
                            } else {
                                mundaris_world::terrain::TerrainGeneratorVersion::V2
                            };
                        let definition =
                            crate::planet_terrain::checkpoint_terrain_definition_version(
                                body.properties().reference_radius_m(),
                                version,
                            )?;
                        self.system.edit_terrain(id, Some(definition))?;
                    }
                }
                self.controls.terrain_preview = enabled;
                self.controls.surface_style.elevation_colors = enabled;
            }
            Command::TrailMode(relative) => {
                let mode = if relative {
                    TrailMode::SimultaneousBodyRelative(id)
                } else {
                    TrailMode::Inertial
                };
                self.trails.set_mode(
                    mode,
                    self.runner.branch_generation(),
                    self.runner.tick(),
                    &self.system,
                )?;
                self.controls.relative_trails = relative;
            }
            Command::TrailReference(reference) => {
                self.trails.set_mode(
                    TrailMode::SimultaneousBodyRelative(reference),
                    self.runner.branch_generation(),
                    self.runner.tick(),
                    &self.system,
                )?;
                self.controls.relative_trails = true;
            }
        }
        self.advance = self.runner.report();
        Ok(())
    }
    fn publish(&mut self) {
        let result = (|| -> Result<()> {
            if self.projection.represented_revision() != self.system.revision() {
                self.projection.publish(&self.system)?;
            }
            self.projection.coherent_view(&self.system)?;
            Ok(())
        })();
        self.coherent = result.is_ok();
        if let Err(error) = result {
            self.runner.set_paused(true);
            self.diagnostic = Some(format!(
                "Projection failed; celestial drawing suppressed: {error}. Rebuild projection to retry."
            ));
        }
    }
    /// Host duration is explicit and captured once. Surface retries never reuse it.
    pub fn update(&mut self, elapsed: Duration) {
        self.advance_surface_validation(elapsed);
        while let Some(command) = self.controls.pending.pop_front() {
            match self.command(command) {
                Ok(()) => self.diagnostic = None,
                Err(error) => self.diagnostic = Some(error.to_string()),
            }
        }
        let elapsed = if self.hidden { Duration::ZERO } else { elapsed };
        let elapsed = if elapsed > self.clock.threshold() {
            self.runner.set_paused(true);
            self.metrics.reset();
            self.camera.cancel_transition();
            self.controls.navigation = NavigationInput::default();
            self.controls.approach = None;
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
                self.advance = self.runner.report();
                return;
            }
        }
        if let Err(error) = self.runner.admit_wall_elapsed(elapsed) {
            self.runner.set_paused(true);
            self.diagnostic = Some(error.to_string());
        }
        let branch = self.runner.branch_generation();
        let seeking = self.seeking;
        let measuring = !self.runner.paused() && !self.hidden && !seeking;
        let authority_before = self.system.sample_time().seconds_since_epoch();
        let replay_before = self.runner.report().replay_remaining.is_some();
        let trails = &mut self.trails;
        let started = Instant::now();
        let mut total = [0u64; 5];
        let mut limit = 1;
        let cap = self.runner.config().work_limit().min(512);
        self.cpu_limited = false;
        self.count_limited = false;
        let result = loop {
            let result =
                self.runner
                    .pump_with_work_limit(&mut self.system, limit, |tick, world| {
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
        self.advance = match result {
            Ok(report) => report,
            Err(error) => {
                self.diagnostic = Some(error.to_string());
                self.seeking = false;
                *error.report
            }
        };
        self.advance.work_steps = total[0] as u32;
        self.advance.forward_steps = total[1] as u32;
        self.advance.restored_steps = total[2] as u32;
        self.advance.force_passes = total[3] as u32;
        self.advance.pair_evaluations = total[4];
        if self.advance.work_steps > 0 && self.pump_ms > 0.0 {
            self.throughput_steps_s = Some(self.advance.work_steps as f64 * 1000.0 / self.pump_ms);
        }
        if self.seeking
            && self.advance.backlog_ticks == 0
            && self.advance.replay_remaining.is_none()
        {
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
        self.guides.update(&self.system);
        let private_work =
            self.advance.work_steps > self.advance.forward_steps + self.advance.restored_steps;
        if measuring && !replay_before && !private_work && self.advance.replay_remaining.is_none() {
            self.metrics.record(
                elapsed,
                self.system.sample_time().seconds_since_epoch() - authority_before,
            );
        } else if replay_before || private_work || self.advance.replay_remaining.is_some() {
            self.metrics.reset();
        }
        self.achieved_rate = self
            .metrics
            .measurement(
                self.runner.config().fixed_step_s(),
                self.runner.rate().multiplier(),
            )
            .map(|m| m.achieved_rate);
        if self.coherent {
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
                elapsed,
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
            self.runner.set_paused(true);
            self.diagnostic = Some(error.to_string());
        }
        self.diagnostic_elapsed = self.diagnostic_elapsed.saturating_add(elapsed);
        if (self.diagnostic_elapsed >= Duration::from_millis(250)
            || (self.runner.paused() && self.advance.work_steps > 0))
            && let Err(error) = self.sample_diagnostics()
        {
            self.runner.set_paused(true);
            self.diagnostic = Some(error.to_string());
        }
        self.refresh_report_metadata();
    }
    fn advance_surface_validation(&mut self, elapsed: Duration) {
        if self.hidden || elapsed > self.clock.threshold() {
            return;
        }
        let Some(route) = &mut self.validation_route else {
            return;
        };
        route.elapsed = route.elapsed.saturating_add(elapsed);
        let actions = [
            (2.0, "focus Aurelia", 0),
            (4.0, "astronomical clearance", 1),
            (6.0, "continuous approach", 2),
            (37.0, "100 km", 3),
            (40.0, "10 km", 4),
            (43.0, "1 km", 5),
            (46.0, "100 m", 6),
            (49.0, "10 m", 7),
            (52.0, "2 m", 8),
            (55.0, "co-rotating inspection", 9),
            (58.0, "local lateral movement", 10),
            (60.0, "stop lateral movement", 11),
            (61.0, "curved horizon", 12),
            (64.0, "look toward Solace", 13),
            (67.0, "twenty physical steps", 14),
            (71.0, "look toward Luma", 15),
            (74.0, "depart Body Orbit", 16),
            (77.0, "whole system", 17),
            (81.0, "repeat focus", 0),
            (83.0, "repeat astronomical", 1),
            (85.0, "repeat approach", 2),
        ];
        if route.next < actions.len() && route.elapsed.as_secs_f64() >= actions[route.next].0 {
            let (_, label, action) = actions[route.next];
            route.next += 1;
            tracing::info!(
                checkpoint = label,
                world_revision = self.system.revision(),
                sample_s = self.system.sample_time().seconds_since_epoch(),
                "Phase 4 validation route"
            );
            let planet = self.ids[1];
            match action {
                0 => {
                    self.controls.pending.push_back(Command::Select(planet));
                    self.controls.pending.push_back(Command::Focus {
                        fixed: false,
                        fit: false,
                    });
                }
                1 => self.controls.pending.push_back(Command::Clearance(1e11)),
                2 => self.controls.pending.push_back(Command::Approach),
                3..=8 => {
                    self.controls.surface_style.borders = true;
                    self.controls.pending.push_back(Command::Clearance(
                        [1e5, 1e4, 1e3, 100.0, 10.0, 2.0][action - 3],
                    ));
                }
                9 => {
                    self.controls.surface_style.borders = false;
                    self.controls.pending.push_back(Command::SurfaceInspection);
                }
                10 => self
                    .controls
                    .pending
                    .push_back(Command::Navigation(NavigationInput {
                        translation: DVec3::X,
                        ..Default::default()
                    })),
                11 => self
                    .controls
                    .pending
                    .push_back(Command::Navigation(NavigationInput::default())),
                12 => self.controls.pending.push_back(Command::SurfaceHorizon),
                13 => self
                    .controls
                    .pending
                    .push_back(Command::LookBody(self.ids[0])),
                14 => {}
                15 => self
                    .controls
                    .pending
                    .push_back(Command::LookBody(self.ids[2])),
                16 => self.controls.pending.push_back(Command::Focus {
                    fixed: false,
                    fit: true,
                }),
                17 => self.controls.pending.push_back(Command::Overview),
                _ => unreachable!("fixed validation route actions"),
            }
        }
        // Distinct opportunities commit twenty real h60 steps. Twenty queued Single
        // commands in one opportunity would only request the same next tick.
        if (67.0..70.0).contains(&route.elapsed.as_secs_f64())
            && self.runner.tick() < route.physical_target
        {
            self.controls.pending.push_back(Command::Single(true));
        }
        if (58.0..60.0).contains(&route.elapsed.as_secs_f64()) {
            self.controls
                .pending
                .push_back(Command::Navigation(NavigationInput {
                    translation: DVec3::X,
                    ..Default::default()
                }));
        }
    }
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
        if width == 0 || height == 0 {
            self.set_lifecycle_drawable(false);
            return Ok(());
        }
        self.set_lifecycle_drawable(true);
        let now = Instant::now();
        let elapsed = self
            .last_wall
            .map_or(Duration::ZERO, |previous| now.duration_since(previous));
        self.last_wall = Some(now);
        let elapsed = match self.clock.classify(elapsed) {
            ClockInterval::Accepted(elapsed) => elapsed,
            ClockInterval::Hidden => Duration::ZERO,
            ClockInterval::Discontinuity(gap) => gap,
        };
        #[cfg(feature = "surface-profile")]
        let update_started = Instant::now();
        self.update(elapsed);
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
            if (first || self.camera.mode() == CameraMode::SystemOrbit)
                && let Err(error) = self.refit(!first)
            {
                self.diagnostic = Some(error.to_string());
            }
        }
        let started = Instant::now();
        self.prepare_visuals()?;
        let selected_index = self.selected_index();
        let pair = self.projection.coherent_view(&self.system)?;
        let query_started = Instant::now();
        self.terrain_clearance = self
            .camera
            .focused_body()
            .map(|body| {
                crate::terrain_inspection::terrain_clearance(&pair, self.camera.pose(), body)
            })
            .transpose()?
            .flatten();
        self.clearance_query_us = query_started.elapsed().as_secs_f64() * 1e6;
        self.ready_mesh_probe = None;
        let view = PreparedView::new(
            &pair.evaluation(),
            self.camera.pose(),
            RenderPrecisionBudget::near_debug(),
        )?;
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
                color: self.scenario.color(index),
                unlit: index == 0,
                selected: index == selected_index,
            });
        }
        if let Some(terrain) = self.terrain_clearance {
            clearance = clearance.min(terrain.clearance_m);
        }
        let near = inspection_near_plane(clearance);
        let projection = self.content_projection(near)?;
        let population_pose = self.camera.pose();
        let population_near = near;
        self.surface_owners.clear();
        self.surface_owners.resize(self.requests.len(), false);
        self.terrain.update(
            &pair,
            &view,
            projection,
            &self.requests,
            &mut self.surfaces,
            &mut self.surface_owners,
            &self.sphere,
            self.controls.terrain_preview,
            Duration::from_millis(self.controls.terrain_morph_ms),
            64,
            Some(Duration::from_millis(2)),
            elapsed,
        )?;
        if let Some(terrain) = self.terrain_clearance
            && self.terrain.active_body() == Some(terrain.body)
        {
            self.ready_mesh_probe =
                crate::surface_probe::ready_mesh_probe(&self.terrain.cover, terrain.location)?;
            if let (Some(minimum), Some(mesh)) =
                (self.controls.terrain_guard_m, self.ready_mesh_probe)
            {
                // A filtered ready mesh need not coincide with complete terrain.
                // Guard both surfaces, including the currently published morph.
                if self.camera.enforce_radial_clearance(
                    &pair,
                    terrain.body,
                    terrain.surface_radius_m.max(mesh.radius_m),
                    minimum,
                )? {
                    self.terrain_clearance = crate::terrain_inspection::terrain_clearance(
                        &pair,
                        self.camera.pose(),
                        terrain.body,
                    )?;
                    self.diagnostic = Some("Debug terrain guard moved the observer above both complete terrain and the published mesh; not collision physics.".into());
                }
            }
        }
        if let Some(terrain) = self.terrain_clearance {
            clearance = terrain.clearance_m;
            if let Some(mesh) = self.ready_mesh_probe {
                clearance = clearance.min(terrain.camera_radius_m - mesh.radius_m);
            }
        }
        let near = inspection_near_plane(clearance);
        let projection = self.content_projection(near)?;
        // The optional debug guard may have changed only the observer after ready
        // publication. Source-centred conversion must use the resulting pose.
        let view = PreparedView::new(
            &pair.evaluation(),
            self.camera.pose(),
            RenderPrecisionBudget::near_debug(),
        )?;
        if (self.camera.pose() != population_pose || near != population_near)
            && let Some(active) = self.terrain.active_body()
            && let Some(index) = self.ids.iter().position(|&id| id == active)
        {
            // A guard correction or tighter mesh-relative near plane must not
            // draw the previous observer's frustum subset for one frame.
            self.terrain.cover.prepare_visible(&SurfaceViewInput {
                view: &view,
                body_fixed_frame: self.requests[index].body_fixed_frame,
                reference_radius_m: self.requests[index].reference_radius_m,
                projection,
            })?;
        }
        let mut lighting = self.controls.terrain_lighting;
        let mut readability = None;
        if let Some(active) = self.terrain.active_body()
            && let Some(index) = self.ids.iter().position(|&id| id == active)
            && matches!(
                self.scenario,
                GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
            )
            && let Some(palette) = crate::solar_system::terrain_readability_config_with_sea_level(
                crate::solar_system::SOLAR_SYSTEM_CONTENT[index].identity,
                self.requests[index].reference_radius_m,
                self.controls.reference_sea_level_m.unwrap_or(
                    crate::solar_system::reference_sea_level_m(
                        crate::solar_system::SOLAR_SYSTEM_CONTENT[index].identity,
                    )
                    .unwrap_or(0.0),
                ),
            )?
        {
            readability = Some(palette);
        }
        if self.controls.sun_from_star
            && let Some(active) = self.terrain.active_body()
        {
            let body = pair.system().body(active)?;
            let star = pair.system().body(self.ids[0])?;
            let direction = body
                .state()
                .body_to_system()
                .inverse()
                .rotate_direction(Direction3::try_new(
                    star.state().center_in_system().metres()
                        - body.state().center_in_system().metres(),
                )?)?
                .unit();
            lighting = TerrainLighting::try_new(
                direction,
                lighting.ambient_strength(),
                lighting.diffuse_strength(),
                lighting.mode(),
            )?;
        }
        if let Some(palette) = readability {
            lighting = lighting.with_readability(palette);
        }
        let mut frame = CelestialFrame::new(&view, &mut self.staging, projection, &self.sphere);
        frame.set_terrain_lighting(lighting);
        let prepared = (|| -> Result<()> {
            for session in &self.surfaces {
                let index = self
                    .ids
                    .iter()
                    .position(|&id| id == session.body())
                    .expect("surface body");
                if self.surface_owners[index] {
                    if self.controls.terrain_preview
                        && self.terrain.active_body() == Some(session.body())
                    {
                        let mut style = self.controls.surface_style;
                        if matches!(
                            self.scenario,
                            GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
                        ) {
                            style.elevation_colors &= crate::solar_system::SOLAR_SYSTEM_CONTENT
                                [index]
                                .terrain_elevation_diagnostic;
                        }
                        frame.append_stitched_surface(
                            self.requests[index],
                            self.terrain.cover.visible(),
                            self.terrain
                                .cover
                                .surface()
                                .ok_or_else(|| anyhow::anyhow!("missing stitched terrain"))?,
                            self.terrain
                                .cover
                                .topology()
                                .ok_or_else(|| anyhow::anyhow!("missing terrain topology"))?,
                            style,
                        )?;
                        if let Some((mesh, fraction)) = self.terrain.cover.transition() {
                            frame.append_surface_transition(
                                self.requests[index],
                                mesh,
                                fraction,
                                style,
                            )?;
                        }
                    } else {
                        frame.append_surface(
                            self.requests[index],
                            session.lod().active_visible(),
                            session.lod().topology(),
                            self.controls.surface_style,
                        )?;
                    }
                }
            }
            frame.append_body_observations(&self.requests, &self.surface_owners)?;
            if self.controls.surface_bounds {
                for session in &self.surfaces {
                    let index = self
                        .ids
                        .iter()
                        .position(|&id| id == session.body())
                        .expect("surface body");
                    let body = self.requests[index];
                    for patch in session.lod().active_visible().iter().take(8) {
                        let (center, radius) = patch.metadata.ball(
                            body.reference_radius_m,
                            SurfaceExtent::smooth(body.reference_radius_m),
                        )?;
                        let mut lines = Vec::with_capacity(49);
                        for (a, b) in [
                            (DVec3::X, DVec3::Y),
                            (DVec3::Y, DVec3::Z),
                            (DVec3::Z, DVec3::X),
                        ] {
                            for i in 0..16 {
                                let endpoints = [i, i + 1].map(|k| {
                                    let angle = k as f64 * std::f64::consts::TAU / 16.0;
                                    center + radius * (a * angle.cos() + b * angle.sin())
                                });
                                lines.push(DebugLine {
                                    endpoints: [
                                        FramePosition::new(
                                            body.body_fixed_frame,
                                            LocalPosition::try_metres(endpoints[0])?,
                                        ),
                                        FramePosition::new(
                                            body.body_fixed_frame,
                                            LocalPosition::try_metres(endpoints[1])?,
                                        ),
                                    ],
                                    color: [0.3, 0.9, 0.3, 1.0],
                                });
                            }
                        }
                        if let Some((axis, _, _)) = patch.metadata.normal_envelope() {
                            lines.push(DebugLine {
                                endpoints: [
                                    FramePosition::new(
                                        body.body_fixed_frame,
                                        LocalPosition::try_metres(center)?,
                                    ),
                                    FramePosition::new(
                                        body.body_fixed_frame,
                                        LocalPosition::try_metres(center + axis * radius)?,
                                    ),
                                ],
                                color: [1.0, 0.6, 0.1, 1.0],
                            });
                        }
                        frame.append_historical_lines(body.body_fixed_frame, &lines)?;
                    }
                }
            }
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
            self.runner.set_paused(true);
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
        // Split borrows retain the coherent world/projection and prepared tree view
        // until submission. UI queues commands; it never mutates that borrowed state.
        let info = UiInfo {
            terrain_clearance: self.terrain_clearance,
            ready_mesh_probe: self.ready_mesh_probe,
            clearance_query_us: self.clearance_query_us,
            owned_surface_count: self.surface_owners.iter().filter(|&&owned| owned).count(),
            terrain_body: self.terrain.active_body(),
            terrain_cache: self.terrain.cache.report(),
            terrain_cover: &self.terrain.cover,
            terrain_work: self.terrain.work,
            surfaces: &self.surfaces,
            system: &self.system,
            projection: &self.projection,
            runner: &self.runner,
            camera: &self.camera,
            ids: &self.ids,
            selected: selected_index,
            diagnostic: self.diagnostic.as_deref(),
            diagnostics: self.diagnostics,
            drift: self.baseline.drift(self.diagnostics),
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
            measurement: self.metrics.measurement(
                self.runner.config().fixed_step_s(),
                self.runner.rate().multiplier(),
            ),
            cpu_limited: self.cpu_limited,
            count_limited: self.count_limited,
            gap_diagnostic: self.gap_diagnostic.as_deref(),
            coarse_curves: self.coarse_curves,
        };
        let controls = &mut self.controls;
        #[cfg(feature = "surface-profile")]
        let render_started = Instant::now();
        renderer.render_celestial(&frame, |context| {
            draw_ui(context, controls, &info, frame.markers())
        })?;
        #[cfg(feature = "surface-profile")]
        tracing::debug!(
            interval_ms = elapsed.as_secs_f64() * 1000.0,
            update_ms,
            pump_ms = self.pump_ms,
            preparation_ms,
            surface_ms = report.surface.profile.total.as_secs_f64() * 1000.0,
            render_present_ms = render_started.elapsed().as_secs_f64() * 1000.0,
            patches = report.surface.patches,
            samples = report.surface.samples,
            bytes = report.surface.uploaded_bytes,
            draws = report.surface.draws,
            dpi = scale,
            "surface CPU/cadence probe (render includes UI/acquire/upload/submit/present, not GPU duration)"
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
                terrain_clearance: self.terrain_clearance,
                ready_mesh_probe: self.ready_mesh_probe,
                clearance_query_us: self.clearance_query_us,
                owned_surface_count: self.surface_owners.iter().filter(|&&owned| owned).count(),
                terrain_body: self.terrain.active_body(),
                terrain_cache: self.terrain.cache.report(),
                terrain_cover: &self.terrain.cover,
                terrain_work: self.terrain.work,
                surfaces: &self.surfaces,
                system: &self.system,
                projection: &self.projection,
                runner: &self.runner,
                camera: &self.camera,
                ids: &self.ids,
                selected,
                diagnostic: self.diagnostic.as_deref(),
                diagnostics: self.diagnostics,
                drift: self.baseline.drift(self.diagnostics),
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
                measurement: self.metrics.measurement(
                    self.runner.config().fixed_step_s(),
                    self.runner.rate().multiplier(),
                ),
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
                && let (Some(reference), Some(e)) = (g.reference, g.elements)
                && e.class() == ConicClass::Elliptic
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
                    let mut color = self.scenario.color(i);
                    color[3] = if selected == Some(id) { 0.85 } else { 0.45 };
                    curve.colors.push(color);
                }
            }
        }
        Ok(())
    }
}

struct UiInfo<'a> {
    terrain_clearance: Option<crate::terrain_inspection::TerrainClearance>,
    ready_mesh_probe: Option<crate::surface_probe::ReadyMeshProbe>,
    clearance_query_us: f64,
    owned_surface_count: usize,
    terrain_body: Option<BodyId>,
    terrain_cache: crate::planet_terrain::TerrainCacheReport,
    terrain_cover: &'a crate::planet_terrain::AdaptiveTerrainCover,
    terrain_work: crate::planet_terrain::TerrainWorkReport,
    surfaces: &'a [PlanetSurfaceSession],
    system: &'a CelestialSystem,
    projection: &'a CelestialFrameProjection,
    runner: &'a FixedStepRunner,
    camera: &'a CelestialCamera,
    ids: &'a [BodyId],
    selected: usize,
    diagnostic: Option<&'a str>,
    diagnostics: SystemDiagnostics,
    drift: DiagnosticDrift,
    sampled_tick: u64,
    advance: SimulationAdvanceReport,
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
        ui.heading("Celestial system");
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
        if ui.button("Free flight / unfocus (Esc)").clicked(){controls.pending.push_back(Command::FreeFlight);}
        ui.label(format!("Control: {:?}",info.camera.mode()));
        ui.small("Orbit: left drag / wheel. Flight: WASD, Q/E, right drag, Shift boost.");
        ui.add(egui::Slider::new(&mut controls.manual_speed,1e-3..=1e3).logarithmic(true).text("Flight speed multiplier"));
        ui.small(format!("Navigation speed {} / wall s",compact_distance(info.camera.flight_speed_m_s())));
        ui.checkbox(&mut controls.markers,"Navigation markers");ui.checkbox(&mut controls.labels,"Labels");ui.checkbox(&mut controls.guide_visible,"Instantaneous orbit guides");ui.checkbox(&mut controls.trails,"Committed historical trails");
        let guide_count=if controls.guide_visible{info.guides.iter().filter(|g|g.elements.is_some_and(|e|e.class()==ConicClass::Elliptic)&&g.reference.is_some()).count()}else{0};
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
        ui.label("Newtonian point masses / fixed KDK / debug celestial shading");
        ui.label(format!("Branch {} / tick {} → target {} / h {} s",info.advance.branch_generation,info.advance.tick,info.advance.target_tick,info.runner.config().fixed_step_s()));
        ui.label(format!("Requested {:.3} s / authoritative {:.3} s",info.advance.requested_time.seconds_since_epoch(),info.system.sample_time().seconds_since_epoch()));
        ui.label(format!("Requested {}x / achieved {} / {:?}",info.runner.rate().multiplier(),info.achieved_rate.map_or_else(||"not yet measured".into(),|r|format!("{r:.3}x (wall sample)")),info.advance.status));
        ui.label(format!("Pending {} ticks / {:.3} s (fraction {:.3} s)",info.advance.backlog_ticks,info.advance.pending_simulation_seconds,info.advance.fractional_seconds));
        ui.label(format!("Latest update: {} work / {} forward / {} restore; {:.3} ms",info.advance.work_steps,info.advance.forward_steps,info.advance.restored_steps,info.pump_ms));
        ui.label(format!("Latest rejected demand {:.3} s / latest explicit cancellation {:.3} s",info.advance.rejected_simulation_seconds,info.advance.cancelled_simulation_seconds));
        if let Some(remaining)=info.advance.replay_remaining {
            ui.colored_label(egui::Color32::YELLOW,format!("Private replay: {remaining} work remaining, estimate {}",info.throughput.map_or_else(||"unmeasured".into(),|rate|format!("{:.2} wall s",remaining as f64/rate))));
            if ui.button("Cancel seek/replay (retain live world)").clicked() {controls.pending.push_back(Command::CancelSeek);}
        }
        ui.label(format!("History ticks {:?}, {} live + {} private replay bytes; snapshots restore / positive-step replay",info.advance.retained_ticks,info.advance.history_payload_bytes,info.advance.replay_history_payload_bytes));
        ui.horizontal(|ui| {
            if ui.button(if info.runner.paused() {"Resume"} else {"Pause (cancel debt)"}).clicked() {controls.pending.push_back(Command::Pause(!info.runner.paused()));}
            if ui.button("Single +").clicked() {controls.pending.push_back(Command::Single(true));}
            if ui.button("Single −").clicked() {controls.pending.push_back(Command::Single(false));}
        });
        ui.horizontal_wrapped(|ui| {for rate in [-1000.0,-10.0,-1.0,0.1,1.0,10.0,100.0,1000.0,10000.0,100000.0,1000000.0,1000000000.0] {
            if ui.button(format!("{rate}x")).clicked() {controls.pending.push_back(Command::Rate(rate));}
        }});
        if info.advance.status==PlaybackStatus::DemandHaltedOverload {
            ui.colored_label(egui::Color32::LIGHT_RED,"Demand halted: overload. Admitted debt drains at fixed h.");
            if ui.button("Resume demand admission (retains debt)").clicked() {controls.pending.push_back(Command::ResumeAdmission);}
        }
        ui.add(egui::DragValue::new(&mut controls.seek_seconds).speed(info.runner.config().fixed_step_s()).suffix(" seek s"));
        match info.runner.quantize_seek_seconds(controls.seek_seconds) {
            Ok(preview)=>{
                ui.label(format!("Nearest tick {} / {:.3} s / delta {:+.3} s",preview.tick,preview.resulting_seconds,preview.quantization_delta_s));
                if ui.button("Confirm seek (paused)").clicked() {controls.pending.push_back(Command::Seek(preview.tick));}
            }
            Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}
        }
        if ui.button("Reset branch baseline (IDs / focus retained)").clicked() {controls.pending.push_back(Command::Reset);}
        ui.horizontal_wrapped(|ui| {for (label,fixture) in [("Gameplay Solar System",GravityFixture::GameplaySolarSystem),("Real-scale Solar reference",GravityFixture::RealSolarSystem),("Load original hierarchy",GravityFixture::Hierarchy),("Load circular oracle",GravityFixture::Circular)] {if ui.button(label).clicked() {controls.pending.push_back(Command::Load(fixture));}}});
        ui.separator();
        ui.label(format!("{} bodies / {} pairs / {} new force passes this update",info.system.body_count(),pair_count(info.system.body_count()).expect("valid count"),info.advance.force_passes));
        ui.label(format!("{} far / {} surface owners · terrain active: {}",info.system.body_count()-info.owned_surface_count,info.owned_surface_count,info.terrain_body.and_then(|id|info.system.body(id).ok()).map_or("none",|body|body.name())));
        ui.label(format!("Diagnostic sample tick {} / {:.3} s",info.sampled_tick,info.diagnostics.sampled_time.seconds_since_epoch()));
        ui.label(format!("E {:.8e} J / drift {:.3e} J / normalized {:.3e}{}",info.diagnostics.total_energy_j(),info.drift.energy_j,info.drift.relative_energy,if info.drift.uses_near_zero_energy_scale {" (K0+|U0| scale)"} else {" (|E0| scale)"}));
        ui.label(format!("P drift {:.3e} kg m/s / P/Qp {:.3e}",info.drift.momentum_kg_m_s.length(),info.drift.normalized_momentum));
        ui.label(format!("Lcom drift {:.3e} kg m²/s / L/Ql {:.3e}",info.drift.angular_momentum_kg_m2_s.length(),info.drift.normalized_angular_momentum));
        ui.label(format!("COM residual {:.3e} m / COM {:?} m",info.drift.com_residual_m.length(),info.diagnostics.center_of_mass_m));
        if info.ids.len()==3 {
            let planet=info.system.body(info.ids[1]).expect("fixture planet");let moon=info.system.body(info.ids[2]).expect("fixture moon");
            let distance=(moon.state().center_in_system().metres()-planet.state().center_in_system().metres()).length();
            let speed=(moon.state().center_velocity_in_system().metres_per_second()-planet.state().center_velocity_in_system().metres_per_second()).length();
            let energy=0.5*speed*speed-GRAVITATIONAL_CONSTANT_M3_KG_S2*(planet.properties().mass_kg()+moon.properties().mass_kg())/distance;
            ui.label(format!("Moon relative: {distance:.6e} m / {speed:.6e} m/s / local Kepler {energy:.6e} J/kg (not conserved with third body)"));
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
            for value in &mut controls.velocity {ui.text_edit_singleline(value);}
            if ui.button("Apply system velocity m/s").clicked() {
                let values=controls.velocity.iter().map(|v|v.parse::<f64>()).collect::<std::result::Result<Vec<_>,_>>();
                match values {Ok(v)=>controls.pending.push_back(Command::Velocity(DVec3::new(v[0],v[1],v[2]))),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}}
            }
        });
        ui.checkbox(&mut controls.markers,"Navigation markers (no physical size change)");ui.checkbox(&mut controls.labels,"Labels");ui.checkbox(&mut controls.trails,"Actual committed-history trails");
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
        });});
    });
}

fn draw_ui(
    context: &egui::Context,
    controls: &mut Controls,
    info: &UiInfo<'_>,
    markers: &[CelestialMarker],
) {
    if info.camera.focused_body().is_some() {
        egui::Window::new("Planet surface / inspection").default_pos(egui::pos2(335.0,120.0)).default_width(360.0).vscroll(true).show(context,|ui| {
            ui.label(if controls.terrain_preview {"Procedural terrain checkpoint · adaptive ready cover"} else {"Smooth sphere · zero terrain height · one connected body"});
            if info.ids.len()==3 && ui.button("Run legacy integrated validation route").clicked() {controls.pending.push_back(Command::ValidationRoute);}
            let id=info.camera.focused_body().expect("focused surface");
            let pair=info.projection.coherent_view(info.system).expect("coherent UI");
            if let Some(c)=info.terrain_clearance {
                ui.strong(format!("Terrain clearance: {:+.2} m",c.clearance_m));
                if c.clearance_m<0.0 {ui.colored_label(egui::Color32::RED,"INSIDE TERRAIN (complete field)");}
                else {ui.label("Above complete displaced terrain");}
                ui.monospace(format!("Centre distance {:.3} m\nSphere altitude {:+.3} m\nTerrain elevation {:+.3} m\nDisplaced radius {:.3} m\nAnalytic slope {:.2}°",c.camera_radius_m,c.sphere_altitude_m,c.terrain_elevation_m,c.surface_radius_m,c.slope_angle_rad.to_degrees()));
                ui.small(format!("Body {} · direction {:?} · direct complete-footprint query {:.1} µs",info.system.body(id).expect("body").name(),c.location.direction().unit(),info.clearance_query_us));
                if let Some(mesh)=info.ready_mesh_probe {
                    let clearance=c.camera_radius_m-mesh.radius_m;
                    ui.strong(format!("Drawn mesh clearance: {clearance:+.2} m"));
                    if clearance<0.0 {ui.colored_label(egui::Color32::RED,"INSIDE DRAWN MESH — ready LOD differs from complete terrain");}
                    ui.monospace(format!("Under camera {:?} · LOD {}\nMesh footprint {:.3} m{}",mesh.patch,mesh.patch.level(),mesh.footprint_m,if mesh.morphing {" · morphing"}else{""}));
                } else {ui.colored_label(egui::Color32::YELLOW,"Drawn surface under camera unavailable / not admitted or not ready");}
                egui::ComboBox::from_label("Debug terrain guard").selected_text(controls.terrain_guard_m.map_or("Disabled".into(),|m|format!("{m} m"))).show_ui(ui,|ui| {
                    for minimum in [None,Some(2.0),Some(10.0),Some(100.0)] {
                        let label=minimum.map_or("Disabled".into(),|m|format!("{m} m"));
                        if ui.selectable_label(controls.terrain_guard_m==minimum,label).clicked() {controls.pending.push_back(Command::TerrainGuard(minimum));}
                    }
                });
                ui.small("Guard uses max(complete terrain, published mesh); radial debug navigation, not collision. Quality-pending mesh may prevent exact requested clearance.");
            } else if info.system.body(id).is_ok_and(|body|body.terrain().is_none()) {
                ui.label("Non-terrain / far-only body: no rocky terrain query");
                if let Ok(clearance)=info.camera.measured_clearance(&pair,id) {ui.label(format!("Reference-sphere altitude {}",compact_distance(clearance)));}
            } else {ui.label("Terrain clearance unavailable");}
            ui.small(format!("Near plane {:.3} m · infinite reverse-Z",info.near));
            if matches!(info.camera.mode(),CameraMode::BodyOrbit|CameraMode::SurfaceInspection)&&!info.camera.transitioning() {
                ui.add(egui::DragValue::new(&mut controls.clearance_target).speed(1.0).suffix(" m target clearance"));
                if ui.button("Approach clearance target").clicked() {controls.pending.push_back(Command::Clearance(controls.clearance_target));}
                if info.camera.mode()==CameraMode::BodyOrbit && ui.button("Continuous 30 s approach to 2 m").clicked() {controls.pending.push_back(Command::Approach);}
                ui.horizontal_wrapped(|ui| {for clearance in [1e11,1e5,1e4,1e3,100.0,10.0,2.0] {if ui.button(compact_distance(clearance)).clicked() {controls.pending.push_back(Command::Clearance(clearance));}}});
                if info.camera.mode()==CameraMode::BodyOrbit && ui.button("Surface inspection (I): co-rotating attachment").clicked() {controls.pending.push_back(Command::SurfaceInspection);}
            }
            if info.camera.mode()==CameraMode::SurfaceInspection {
                ui.small("Co-rotating; zero relative simulation derivative. WASD/QE editor offsets, right-drag look; optional displaced-terrain guard.");
                if ui.button("Look tangent / horizon (H)").clicked() {controls.pending.push_back(Command::SurfaceHorizon);}
                if ui.button("Depart to centre-look Body Orbit").clicked() {controls.pending.push_back(Command::Focus {fixed:false,fit:true});}
                if ui.button("Single physical step +h").clicked() {controls.pending.push_back(Command::Single(true));}
                ui.horizontal_wrapped(|ui| {for &target in info.ids {if target!=id&&ui.button(format!("Look at {}",info.system.body(target).expect("body").name())).clicked() {controls.pending.push_back(Command::LookBody(target));}}});
            }
            ui.checkbox(&mut controls.surface_style.borders,"Patch borders");ui.checkbox(&mut controls.surface_style.lod_colors,"LOD colours");ui.checkbox(&mut controls.surface_style.face_colors,"Face IDs / colours");ui.checkbox(&mut controls.surface_style.underside,"No-cull underside diagnostic");
            ui.checkbox(&mut controls.surface_bounds,"Bounds / normal envelope axes (bounded)");
            let mut preview=controls.terrain_preview;
            if ui.checkbox(&mut preview,"Adaptive terrain with stitched transitions").changed() {controls.pending.push_back(Command::TerrainPreview(preview));}
            if controls.terrain_preview {
                ui.checkbox(&mut controls.sun_from_star,"Use central star direction (disable for lighting presets)");
                ui.add(egui::Slider::new(&mut controls.terrain_morph_ms,0..=1000).text("Morph ms (0: static)").clamping(egui::SliderClamping::Always));
                ui.checkbox(&mut controls.surface_style.elevation_colors,"Derived terrain elevation colours");
                let mut enabled = matches!(controls.terrain_lighting.mode(),TerrainRenderMode::Lit|TerrainRenderMode::Readability);
                if ui.checkbox(&mut enabled, "Terrain lighting enabled").changed() {
                    controls.terrain_lighting = controls.terrain_lighting.with_mode(if enabled { TerrainRenderMode::Lit } else { TerrainRenderMode::Elevation });
                }
                egui::ComboBox::from_label("Terrain shading mode").selected_text(format!("{:?}", controls.terrain_lighting.mode())).show_ui(ui, |ui| {
                    for mode in TerrainRenderMode::ALL {
                        if ui.selectable_label(controls.terrain_lighting.mode() == mode, format!("{mode:?}")).clicked() {
                            controls.terrain_lighting = controls.terrain_lighting.with_mode(mode);
                        }
                    }
                });
                let mut override_sea=controls.reference_sea_level_m.is_some();
                if ui.checkbox(&mut override_sea,"Override content reference sea level (display only)").changed() {
                    controls.reference_sea_level_m=override_sea.then_some(crate::solar_system::GAMEPLAY_EARTH_SEA_LEVEL_M);
                }
                if let Some(sea)=&mut controls.reference_sea_level_m {
                    ui.add(egui::DragValue::new(sea).speed(10.0).suffix(" m sea datum"));
                }
                ui.small("Blue = height below datum; no water geometry or biomes. Rock blends analytic slope 8–16°. Height and slope are independent.");
                egui::ComboBox::from_label("Body-fixed sun preset").selected_text("Choose preset").show_ui(ui, |ui| {
                    for (label, preset) in [("Overhead", TerrainSunPreset::Overhead), ("Side", TerrainSunPreset::Side), ("Grazing", TerrainSunPreset::Grazing), ("Terminator", TerrainSunPreset::Terminator), ("Night", TerrainSunPreset::Night)] {
                        if ui.button(label).clicked() {
                            let old = controls.terrain_lighting;
                            if let Ok(value) = TerrainLighting::try_new(preset.direction_body(), old.ambient_strength(), old.diffuse_strength(), old.mode()) { controls.terrain_lighting = value; }
                        }
                    }
                });
                let mut sun = controls.terrain_lighting.sun_direction_body();
                let mut sun_changed = false;
                ui.horizontal(|ui| {
                    for (label, component) in [("Sun X", &mut sun.x), ("Y", &mut sun.y), ("Z", &mut sun.z)] {
                        sun_changed |= ui.add(egui::DragValue::new(component).speed(0.01).prefix(label)).changed();
                    }
                });
                let lighting = controls.terrain_lighting;
                if sun_changed && let Ok(value) = TerrainLighting::try_new(sun, lighting.ambient_strength(), lighting.diffuse_strength(), lighting.mode()) { controls.terrain_lighting = value; }
                let mut ambient = controls.terrain_lighting.ambient_strength();
                if ui.add(egui::Slider::new(&mut ambient, 0.0..=1.0).text("Ambient")).changed() {
                    let value = controls.terrain_lighting;
                    if let Ok(updated) = TerrainLighting::try_new(value.sun_direction_body(), ambient, value.diffuse_strength().min(1.0 - ambient), value.mode()) { controls.terrain_lighting = updated; }
                }
                let mut diffuse = controls.terrain_lighting.diffuse_strength();
                if ui.add(egui::Slider::new(&mut diffuse, 0.0..=1.0).text("Diffuse")).changed() {
                    let value = controls.terrain_lighting;
                    if let Ok(updated) = TerrainLighting::try_new(value.sun_direction_body(), value.ambient_strength().min(1.0 - diffuse), diffuse, value.mode()) { controls.terrain_lighting = updated; }
                }
                let c=info.terrain_cache;let w=info.terrain_work;
                ui.monospace(format!("Terrain: {} resident, {} pending; {} vertices / {} patches this frame, {:.3} ms; {:.2} MiB (peak {:.2}); evictions {}",c.resident_patches,w.pending_patches,w.vertices_generated,w.patches_completed,w.elapsed.as_secs_f64()*1000.0,c.resident_bytes as f64/1048576.0,c.peak_bytes as f64/1048576.0,c.evictions));
                let cover=info.terrain_cover;
                let d=cover.convergence;
                ui.monospace(format!("Desired local LOD: {:?}\nReady local LOD: {:?}\nRendered source LOD: {:?}",d.desired_local_lod,d.ready_local_lod,d.rendered_local_lod));
                ui.monospace(format!("Desired patches: {}{} · ready source: {}\nPending work: {} · queue: {} · workers: {}/{}\nActive morphs: {} · building cover: {} · blocked transactions: {}",
                    cover.report.desired_patches,if cover.report.desired_estimate_incomplete {" (incomplete estimate)"}else{""},cover.active().len(),w.pending_patches,c.queued_patches,c.worker_jobs,c.worker_count,
                    usize::from(cover.transition().is_some()),cover.construction_pending(),cover.report.deferred_transactions));
                ui.small(format!("Main thread: scheduling {:.3} ms · raw publication {:.3} ms · cover publication {:.3} ms · diagnostics {:.3} ms. Worker CPU completed this frame {:.3} ms / {} samples; last stitch {:.3} ms / morph {:.3} ms",
                    w.scheduling.as_secs_f64()*1000.0,w.publication.as_secs_f64()*1000.0,cover.result_publication.as_secs_f64()*1000.0,d.diagnostic_cpu.as_secs_f64()*1000.0,
                    w.worker_cpu.as_secs_f64()*1000.0,w.worker_samples_completed,cover.worker_stitch_cpu.as_secs_f64()*1000.0,cover.worker_morph_cpu.as_secs_f64()*1000.0));
                let e=d.local_error;
                ui.small(format!("Local certificate metres: sphere {:.3e} · interpolation {:.3e} · unresolved {:.3e} · boundary {:.3e} · morph {:.3e} · numeric {:.3e}",e.sphere_m,e.filtered_interpolation_m,e.unresolved_m,e.boundary_constraint_m,e.morph_remaining_m,e.numeric_m));
                if !d.target_certifiable {ui.colored_label(egui::Color32::YELLOW,"Local pixel target not certifiable by LOD30; not merely queued work");}
                else if cover.report.budget_constrained {ui.label("Local target certifiable; current refinement resource-constrained");}
                else if d.rendered_local_lod<d.desired_local_lod {ui.label("Local target certifiable; generation / replacement pending");}
                ui.small(format!("Worker reservations {:.2} MiB · stacks/scratch {:.2} MiB · completed cover reservation {:.2} MiB · cancellations {}",
                    c.worker_reserved_bytes as f64/1048576.0,c.worker_fixed_bytes as f64/1048576.0,c.completed_unpublished_bytes as f64/1048576.0,c.cancellations));
                if cover.transition_deferred {ui.colored_label(egui::Color32::YELLOW,"Transition reservation cannot fit replacement; complete source retained. Reset terrain or explicitly change morph duration to retry.");}
                ui.small(format!("Accounted aggregate {:.2} MiB / peak {:.2} MiB; {} pinned. Selector {:.3} ms / stitching {:.3} ms / morph construction {:.3} ms",(c.resident_bytes+c.external_bytes) as f64/1048576.0,c.peak_aggregate_bytes as f64/1048576.0,c.pinned_patches,cover.selection_preparation.as_secs_f64()*1000.0,cover.stitch_preparation.as_secs_f64()*1000.0,cover.morph_preparation.as_secs_f64()*1000.0));
                if let Some((mesh,fraction))=cover.transition() {
                    ui.small(format!("One synchronized morph: {:.1}% / {} overlay triangles; source {} / target {} changed leaves; remaining displacement {:.3} m",fraction*100.0,mesh.triangles().len(),mesh.affected_old().len(),mesh.affected_new().len(),mesh.max_displacement_m()*(1.0-fraction)));
                }
                ui.small(format!("Published source cover {} / visible regular {}; selector counts below refer to target readiness",cover.active().len(),cover.visible().len()));
                let levels=cover.visible().iter().map(|p|p.address.level());
                let min=levels.clone().min();let max=levels.max();
                ui.monospace(format!("Visible LOD {:?}–{:?} · covering {} · visible {}\nReady source leaves {} · pending {} · active morphs {}",min,max,cover.active().len(),cover.visible().len(),if cover.ready(){cover.active().len()}else{0},w.pending_patches,usize::from(cover.transition().is_some())));
                if controls.surface_style.lod_colors {
                    ui.horizontal_wrapped(|ui| {for level in 0..=max.unwrap_or(0).min(30) {
                        let color=lod_color(level);
                        ui.colored_label(egui::Color32::from_rgb((color[0]*255.0) as u8,(color[1]*255.0) as u8,(color[2]*255.0) as u8),format!("L{level}"));
                    }});
                    ui.small("Hue repeats every 12 levels; numeric LOD readouts disambiguate.");
                }
                ui.label("Adaptive displaced stitching / ready-cover morphs; terrain-under-camera query is independent of patch UV.");
            }
            if let Some(pointer)=context.input(|i|i.pointer.hover_pos()) {
                let pixels=[f64::from(pointer.x*context.pixels_per_point()),f64::from(pointer.y*context.pixels_per_point())];
                if let Some(session)=info.surfaces.iter().find(|s|s.body()==id)&&let Ok(Some(patch))=session.hovered_patch(&pair,info.camera.pose(),info.celestial_projection,pixels) {ui.monospace(format!("Pointer patch {patch:?}"));}
            }
            for session in info.surfaces {let r=session.report;
                ui.label(format!("{} {:?} · far error {:.4} px",info.system.body(session.body()).expect("body").name(),session.state(),session.far_error_pixels));
                ui.small(format!("Desired {}{} / balanced {} / active {} / visible {} / level {} / error {:.4} px",r.desired_patches,if r.desired_estimate_incomplete {" (incomplete estimate)"}else{""},r.balanced_patches,r.active_patches,r.visible_patches,r.max_level,r.max_error_pixels));
                ui.small(format!("Split {} merge {} balance {} · deferred {} constrained {}{}",r.splits,r.merges,r.balance_splits,r.deferred_transactions,r.constrained_refinements,if r.quality_pending {" · quality pending"}else{""}));
                ui.small(format!("Cover/scratch {} bytes · sample time {:.3} s",r.scratch_bytes,info.system.sample_time().seconds_since_epoch()));
                ui.small(format!("Horizon {} frustum {} · cache {} records / {} bytes, hit {} miss {} evict {}",r.horizon_culled,r.frustum_culled,r.cache_records,r.cache_bytes,r.cache_hits,r.cache_misses,r.cache_evictions));
                if r.budget_constrained {ui.colored_label(egui::Color32::YELLOW,"Budget constrains requested quality; complete coarser cover retained");}
                ui.push_id(session.body(),|ui|ui.collapsing("Bounded patch addresses / stitch masks",|ui| {for patch in session.lod().active_visible().iter().take(16) {ui.monospace(format!("{:?} mask {:04b} · {:.4} px",patch.address,patch.stitch_mask,patch.error_pixels));}}));
            }
            let r=info.report.surface;ui.small(format!("{} draws / {} samples / {} clipped fallback + {} morph triangles / {} upload bytes",r.draws,r.samples,r.fallback_triangles,r.morph_triangles,r.uploaded_bytes));
            ui.small(format!("Narrowing {:.4e} px / GPU projection {:.4e} px · staging {} bytes",r.max_projected_error_pixels,r.max_gpu_projection_error_pixels,r.allocated_staging_bytes));
        });
    }
    egui::TopBottomPanel::top("exact playback and navigation").exact_height(110.0).show(context,|ui| {
        ui.horizontal(|ui|{
            ui.heading("Mundaris");
            if ui.button(if info.runner.paused(){"▶ Resume"}else{"⏸ Pause / cancel debt"}).clicked(){controls.pending.push_back(Command::Pause(!info.runner.paused()));}
            if ui.button("Whole system (Home)").clicked(){controls.pending.push_back(Command::Overview);}
            if ui.button("Previous focus").clicked(){controls.pending.push_back(Command::Select(info.ids[(info.selected+info.ids.len()-1)%info.ids.len()]));controls.pending.push_back(Command::Focus{fixed:false,fit:false});}
            if ui.button("Next focus").clicked(){controls.pending.push_back(Command::Select(info.ids[(info.selected+1)%info.ids.len()]));controls.pending.push_back(Command::Focus{fixed:false,fit:false});}
        });
        ui.horizontal(|ui|{
            ui.strong(format!("Requested {}×",info.runner.rate().multiplier()));
                ui.strong(info.measurement.map_or_else(||"Achieved: warming / paused / replaying".into(),|m|if m.tick_limited{format!("Achieved {:.1}× ({:.1}s; tick-limited), segment {:.2}× / {:.1}s",m.achieved_rate,m.window_s,m.segment_rate,m.segment_wall_s)}else{format!("Achieved {:.1}× ({:.1}s)",m.achieved_rate,m.window_s)}));
            ui.label(format!("{:?} · {:?}{}",info.advance.status,info.camera.mode(),if info.camera.transitioning(){" · transitioning"}else{""}));
        });
        ui.horizontal(|ui|{
            ui.label(format!("Baseline exact · full N-body KDK · h={}s · authority {:.2} days · requested {:.2} days",info.runner.config().fixed_step_s(),info.advance.authoritative_time.seconds_since_epoch()/86400.0,info.advance.requested_time.seconds_since_epoch()/86400.0));
            ui.label(format!("Pending {} ticks · last {} work{}{}",info.advance.backlog_ticks,info.advance.work_steps,if info.cpu_limited{" · CPU budget limited"}else{""},if info.count_limited{" · opportunity count limited"}else{""}));
            if info.advance.backlog_ticks==0 && !info.runner.paused() && info.runner.rate().multiplier()>0.0 {
                let wait=(info.runner.config().fixed_step_s()-info.advance.fractional_seconds)/info.runner.rate().multiplier();
                if wait>0.5 {ui.small(format!("Next committed step in {wait:.1} wall s"));}
            }
        });
        ui.horizontal(|ui|{
            for rate in [1.0,100.0,1000.0,10000.0,100000.0,1000000.0] {if ui.button(format!("{rate}×")).clicked(){controls.pending.push_back(Command::Rate(rate));}}
            ui.add(egui::DragValue::new(&mut controls.custom_rate).speed(1000.0).prefix("Custom "));if ui.button("Set").clicked(){controls.pending.push_back(Command::Rate(controls.custom_rate));}
            if info.advance.status==PlaybackStatus::DemandHaltedOverload {
                ui.colored_label(egui::Color32::LIGHT_RED,"Admission halted; fixed-h debt drains");
                if ui.button("Lower rate").clicked(){controls.pending.push_back(Command::Rate(info.runner.rate().multiplier()/10.0));}
                if ui.button("Resume admission").clicked(){controls.pending.push_back(Command::ResumeAdmission);}
            }
        });
    });
    egui::TopBottomPanel::bottom("celestial semantics legend").exact_height(28.0).show(context,|ui|{
        ui.horizontal(|ui|{ui.small("Dashed = instantaneous two-body ORBIT GUIDE · Solid/fading = committed HISTORY · Rings/labels = navigation overlays");
            if let Some(gap)=info.gap_diagnostic {ui.colored_label(egui::Color32::YELLOW,gap);}else if let Some(error)=info.diagnostic {ui.colored_label(egui::Color32::LIGHT_RED,error);}
        });
    });
    draw_engineering_ui(context, controls, info, markers);
    let scale = context.pixels_per_point();
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(context,|ui|{
        let rect=ui.max_rect();
        let origin=[(rect.min.x*scale).round().max(0.0) as u32,(rect.min.y*scale).round().max(0.0) as u32];
        let size=[(rect.width()*scale).round().max(1.0) as u32,(rect.height()*scale).round().max(1.0) as u32];
        controls.viewport=Some((origin,size));
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
        let inside=input.pointer.hover_pos().is_some_and(|p|rect.contains(p)&&context.layer_id_at(p)==Some(ui.layer_id()));
        let local_look=matches!(info.camera.mode(),CameraMode::FreeFlight|CameraMode::SurfaceInspection);
        let drag=if inside && ((!local_look&&controls.gesture_dragged&&input.pointer.primary_down())||(local_look&&input.pointer.secondary_down())) {input.pointer.delta()}else{egui::Vec2::ZERO};
        let mut translation=DVec3::ZERO;
        if !context.wants_keyboard_input()&&local_look {
            for (key,axis) in [(egui::Key::W,-DVec3::Z),(egui::Key::S,DVec3::Z),(egui::Key::A,-DVec3::X),(egui::Key::D,DVec3::X),(egui::Key::Q,-DVec3::Y),(egui::Key::E,DVec3::Y)] {if input.key_down(key){translation+=axis;}}
        }
        let wheel=if inside{input.raw_scroll_delta.y as f64}else{0.0};
        if local_look && wheel!=0.0 {controls.manual_speed=(controls.manual_speed*(wheel/50.0*2.0_f64.ln()).exp()).clamp(1e-3,1e3);}
        controls.pending.push_back(Command::Navigation(NavigationInput{drag:[drag.x as f64,drag.y as f64],scroll_notches:if local_look{0.0}else{wheel/50.0},translation,speed_multiplier:controls.manual_speed*if input.modifiers.shift{4.0}else{1.0}}));
    });
    if !context.wants_keyboard_input() {
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
mod tests {
    #[test]
    fn solar_development_starts_paused_with_earth_selected_and_no_terrain_work() {
        use super::*;
        let demo = GravityOrbitsDemo::solar_system(false).unwrap();
        assert_eq!(demo.system.body_count(), 10);
        assert_eq!(
            demo.system
                .body(demo.selection.selected().unwrap())
                .unwrap()
                .name(),
            "Earth"
        );
        assert_eq!(
            demo.system
                .body(demo.ids[3])
                .unwrap()
                .properties()
                .reference_radius_m(),
            400_000.0
        );
        assert_eq!(demo.surfaces.len(), 5);
        assert!(demo.controls.terrain_preview);
        assert!(demo.runner.paused());
        assert_eq!(demo.camera.mode(), CameraMode::SystemOrbit);
        assert_eq!(demo.terrain.active_body(), None);
        assert_eq!(demo.terrain.cache.pending(), 0);
    }
    #[test]
    fn terrain_runtime_options_select_deterministic_renderer_only_settings() {
        use super::*;
        assert_eq!(
            terrain_lighting_configuration(None, None),
            TerrainLighting::default()
        );
        assert_eq!(
            terrain_lighting_configuration(Some("unknown"), Some("unknown")),
            TerrainLighting::default()
        );
        for (name, mode) in [
            ("elevation", TerrainRenderMode::Elevation),
            ("lit", TerrainRenderMode::Lit),
            ("normals", TerrainRenderMode::Normals),
            ("diffuse", TerrainRenderMode::Diffuse),
        ] {
            let config = terrain_lighting_configuration(Some(name), Some("grazing"));
            assert_eq!(config.mode(), mode);
            assert_eq!(
                config.sun_direction_body(),
                TerrainSunPreset::Grazing.direction_body()
            );
        }
    }
    use super::*;
    #[test]
    fn integrated_surface_route_keeps_body_identity_and_commits_real_steps() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let ids = demo.ids.clone();
        let initial = *demo.system.body(ids[1]).unwrap().state();
        demo.start_surface_validation().unwrap();
        for _ in 0..3750 {
            demo.update(Duration::from_millis(32));
        }
        assert_eq!(demo.ids, ids);
        assert_eq!(demo.runner.tick(), 20);
        assert_eq!(demo.system.sample_time().seconds_since_epoch(), 1200.0);
        assert_ne!(*demo.system.body(ids[1]).unwrap().state(), initial);
        assert_eq!(demo.camera.focused_body(), Some(ids[1]));
        assert!(demo.camera.clearance_m() < 2.001);
        assert!(demo.diagnostic.is_none(), "{:?}", demo.diagnostic);
    }
    #[test]
    fn near_surface_stall_and_hidden_duration_preserve_inspection_and_resume_cleanly() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.start_surface_validation().unwrap();
        for _ in 0..1750 {
            demo.update(Duration::from_millis(32));
        }
        assert_eq!(demo.camera.mode(), CameraMode::SurfaceInspection);
        assert!(demo.camera.clearance_m() < 2.001);
        demo.validation_route = None;
        demo.command(Command::Rate(1e6)).unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.update(Duration::from_millis(100));
        assert!(demo.advance.backlog_ticks > 0);
        let revision = demo.system.revision();
        let tick = demo.runner.tick();
        let pose = demo.camera.pose();
        let assert_inspection_pose = |actual: FramePose| {
            assert_eq!(actual.position(), pose.position());
            // Inspection reconstructs its normalized orientation even at zero
            // navigation duration; compare within the existing basis envelope.
            for (a, b) in actual
                .orientation()
                .quaternion()
                .to_array()
                .into_iter()
                .zip(pose.orientation().quaternion().to_array())
            {
                assert!((a - b).abs() <= 1e-12);
            }
        };
        demo.update(Duration::from_secs(36000));
        assert_eq!(demo.system.revision(), revision);
        assert_inspection_pose(demo.camera.pose());
        assert_eq!(demo.runner.tick(), tick);
        assert_eq!(demo.advance.backlog_ticks, 0);
        assert!(demo.runner.paused());
        assert!(demo.gap_diagnostic.is_some());
        demo.set_lifecycle_drawable(false);
        demo.update(Duration::from_secs(36000));
        demo.set_lifecycle_drawable(true);
        demo.update(Duration::ZERO);
        assert_inspection_pose(demo.camera.pose());
        assert_eq!(demo.system.revision(), revision);
        assert_eq!(demo.camera.mode(), CameraMode::SurfaceInspection);
        demo.command(Command::Rate(1.0)).unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.runner.tick(), tick);
        demo.command(Command::Single(true)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.runner.tick(), tick + 1);
        assert_inspection_pose(demo.camera.pose());
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
        assert!(demo.diagnostic.is_none(), "{:?}", demo.diagnostic);
    }
    #[test]
    fn egui_wheel_and_keyboard_route_to_one_observer() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let id = demo.ids[1];
        demo.camera
            .focus(
                &demo.projection.coherent_view(&demo.system).unwrap(),
                id,
                false,
                true,
            )
            .unwrap();
        let before = demo.camera.clearance_m();
        let context = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events: vec![
                egui::Event::PointerMoved(egui::pos2(900.0, 500.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, 3.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let (info, controls) = demo.ui_info(CelestialPreparationReport::default(), 0.1, 0.0);
        let _ = context.run(raw, |ctx| draw_ui(ctx, controls, &info, &[]));
        demo.update(Duration::from_millis(100));
        assert!(demo.camera.clearance_m() < before);
        assert_eq!(demo.camera.mode(), CameraMode::BodyOrbit);
    }
    #[test]
    fn exact_requested_rate_matrix_sixty_accounted_seconds() {
        for fixture in [GravityFixture::Hierarchy, GravityFixture::Circular] {
            for rate in [1.0, 100.0, 1000.0, 10000.0, 100000.0, 1000000.0] {
                let mut demo = GravityOrbitsDemo::create(fixture, 1, 1).unwrap();
                demo.command(Command::Rate(rate)).unwrap();
                demo.command(Command::Pause(false)).unwrap();
                let started = Instant::now();
                for _ in 0..3750 {
                    demo.update(Duration::from_millis(16));
                }
                while demo.advance.backlog_ticks > 0 {
                    demo.update(Duration::ZERO);
                }
                let achieved = demo
                    .metrics
                    .measurement(fixture.fixed_step_s(), rate)
                    .unwrap();
                let authority = demo.system.sample_time().seconds_since_epoch();
                let expected =
                    (60.0 * rate / fixture.fixed_step_s()).floor() * fixture.fixed_step_s();
                if rate * 0.016 / fixture.fixed_step_s() <= 512.0 {
                    assert_eq!(authority, expected);
                    assert_eq!(demo.advance.rejected_simulation_seconds, 0.0);
                } else {
                    assert!(authority < expected);
                    assert!(demo.advance.rejected_simulation_seconds > 0.0);
                }
                assert_eq!(demo.runner.config().fixed_step_s(), fixture.fixed_step_s());
                assert_eq!(demo.advance.backlog_ticks, 0);
                eprintln!(
                    "exact fixture={fixture:?} requested={rate}x accounted_wall=60s achieved_window={}x window={}s authority={}s pending={} CPU_wall={}s",
                    achieved.achieved_rate,
                    achieved.window_s,
                    authority,
                    demo.advance.backlog_ticks,
                    started.elapsed().as_secs_f64()
                );
            }
        }
    }
    #[test]
    fn long_gap_cancels_existing_debt_without_physical_or_camera_mutation() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.command(Command::Rate(1e6)).unwrap();
        demo.update(Duration::from_millis(100));
        assert!(demo.advance.backlog_ticks > 0);
        let states: Vec<_> = demo
            .system
            .bodies()
            .map(|(id, b)| (id, b.clone()))
            .collect();
        let revision = demo.system.revision();
        let pose = demo.camera.pose();
        demo.update(Duration::from_secs(36000));
        assert_eq!(demo.system.revision(), revision);
        assert_eq!(
            demo.system
                .bodies()
                .map(|(id, b)| (id, b.clone()))
                .collect::<Vec<_>>(),
            states
        );
        assert_eq!(demo.camera.pose(), pose);
        assert!(demo.runner.paused());
        assert_eq!(demo.advance.backlog_ticks, 0);
        assert!(demo.advance.cancelled_simulation_seconds > 0.0);
        assert!(demo.gap_diagnostic.is_some());
        assert!(demo.metrics.measurement(60.0, 1e6).is_none());
        demo.command(Command::Pause(false)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.system.revision(), revision);
    }
    #[test]
    fn ordered_selection_focus_and_visual_history_preserve_identity_and_world() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let id = demo.ids[2];
        let revision = demo.system.revision();
        demo.controls.pending.push_back(Command::Select(id));
        demo.controls.pending.push_back(Command::Focus {
            fixed: false,
            fit: false,
        });
        demo.update(Duration::ZERO);
        assert_eq!(demo.selection.selected(), Some(id));
        assert_eq!(demo.camera.focused_body(), Some(id));
        assert_eq!(demo.system.revision(), revision);
        demo.prepare_visuals().unwrap();
        assert_eq!(demo.trails.sample_count(), 1);
        assert!(demo.curves[2].points.len() < 2);
        assert!(demo.curves[5].points.len() >= 65);
        assert_eq!(demo.system.revision(), revision);
        demo.command(Command::Single(true)).unwrap();
        demo.update(Duration::ZERO);
        let ticks = demo.trails.retained_ticks();
        demo.command(Command::TrailMode(true)).unwrap();
        assert_eq!(demo.trails.retained_ticks(), ticks);
        demo.prepare_visuals().unwrap();
        assert_eq!(demo.system.revision(), revision + 1);
    }
    #[test]
    fn commands_backlog_edits_seek_trails_focus_and_rebuild_are_coherent() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let ids = demo.ids.clone();
        demo.command(Command::Pause(false)).unwrap();
        demo.command(Command::Rate(1000000.0)).unwrap();
        demo.update(Duration::from_millis(100));
        assert_eq!(demo.runner.tick(), 512);
        assert!(demo.advance.backlog_ticks > 0);
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
        let state = *demo.system.body(ids[1]).unwrap().state();
        let revision = demo.system.revision();
        let history = demo.trails.retained_ticks();
        assert!(demo.command(Command::Mass(f64::NAN)).is_err());
        assert_eq!(demo.system.revision(), revision);
        assert_eq!(demo.trails.retained_ticks(), history);
        assert!(!demo.runner.paused());
        demo.command(Command::Pause(true)).unwrap();
        demo.command(Command::Focus {
            fixed: false,
            fit: true,
        })
        .unwrap();
        assert_eq!(demo.system.revision(), revision);
        demo.command(Command::Rebuild).unwrap();
        assert_eq!(demo.ids, ids);
        assert_eq!(demo.system.revision(), revision);
        assert_eq!(demo.trails.retained_ticks(), history);
        demo.command(Command::Mass(6e24)).unwrap();
        assert!(demo.runner.paused());
        assert_eq!(demo.runner.tick(), 0);
        assert_eq!(*demo.system.body(ids[1]).unwrap().state(), state);
        assert_eq!(demo.trails.sample_count(), 1);
        demo.update(Duration::ZERO);
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
        demo.command(Command::Seek(3000)).unwrap();
        demo.update(Duration::ZERO);
        assert!(demo.advance.replay_remaining.is_some());
        assert_eq!(demo.trails.sample_count(), 1);
        demo.command(Command::CancelSeek).unwrap();
        assert_eq!(demo.runner.tick(), 0);
        demo.command(Command::Seek(3000)).unwrap();
        while demo.seeking {
            demo.update(Duration::ZERO);
        }
        assert_eq!(demo.runner.tick(), 3000);
        assert_eq!(demo.trails.sample_count(), 1);
        demo.command(Command::Reset).unwrap();
        assert_eq!(demo.ids, ids);
        assert_eq!(demo.runner.tick(), 0);
        assert_eq!(*demo.system.body(ids[1]).unwrap().state(), state);
        demo.command(Command::Load(GravityFixture::Circular))
            .unwrap();
        assert_ne!(demo.ids[0], ids[0]);
        assert_eq!(demo.system.body_count(), 2);
    }
    #[test]
    fn hidden_duration_and_numerical_error_record_no_fake_history() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.set_lifecycle_drawable(false);
        demo.update(Duration::from_secs(1000000));
        assert_eq!(demo.runner.tick(), 0);
        demo.set_lifecycle_drawable(true);
        demo.update(Duration::ZERO);
        assert_eq!(demo.advance.backlog_ticks, 0);
        demo.command(Command::Load(GravityFixture::Circular))
            .unwrap();
        demo.command(Command::Velocity(-DVec3::X * 900000.0))
            .unwrap();
        demo.command(Command::Single(true)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.runner.tick(), 0);
        assert_eq!(demo.trails.sample_count(), 1);
        assert!(demo.diagnostic.is_some());
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
    }
    #[test]
    fn repeated_single_steps_preserve_committed_trail_cadence() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.command(Command::Load(GravityFixture::Circular))
            .unwrap();
        for _ in 0..16 {
            demo.command(Command::Single(true)).unwrap();
            demo.update(Duration::ZERO);
        }
        assert_eq!(demo.trails.retained_ticks(), [0, 8, 16]);
        demo.command(Command::Single(false)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.trails.retained_ticks(), [15]);
        demo.command(Command::Single(false)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(demo.trails.retained_ticks(), [15]);
    }
    #[test]
    fn seek_invalidates_trails_before_private_replay_and_cancel_retains_world() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.command(Command::Load(GravityFixture::Circular))
            .unwrap();
        for _ in 0..16 {
            demo.command(Command::Single(true)).unwrap();
            demo.update(Duration::ZERO);
        }
        assert_eq!(demo.trails.sample_count(), 3);
        let time = demo.system.sample_time();
        let revision = demo.system.revision();
        demo.command(Command::Seek(10000)).unwrap();
        assert_eq!(demo.trails.retained_ticks(), [16]);
        demo.command(Command::CancelSeek).unwrap();
        assert_eq!(demo.system.sample_time(), time);
        assert_eq!(demo.system.revision(), revision);
    }
    #[test]
    fn radius_edit_updates_only_geometry_and_camera_navigation_envelope() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.camera
            .focus(
                &demo.projection.coherent_view(&demo.system).unwrap(),
                demo.ids[1],
                false,
                true,
            )
            .unwrap();
        let state = *demo.system.body(demo.ids[1]).unwrap().state();
        demo.command(Command::Radius(6.371e8)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(*demo.system.body(demo.ids[1]).unwrap().state(), state);
        assert!(demo.camera.distance_m() > 6.371e8);
        assert!(
            demo.diagnostic
                .as_deref()
                .unwrap()
                .contains("only the observer")
        );
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
    }
    #[test]
    fn projection_failure_suppresses_drawing_and_reports_actual_paused_clock() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let wrong = CelestialSystem::new(NonZeroU64::new(99).unwrap(), SimulationInstant::ZERO);
        demo.projection =
            CelestialFrameProjection::build(&wrong, NonZeroU64::new(99).unwrap()).unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.update(Duration::from_millis(100));
        assert!(!demo.coherent);
        assert!(demo.runner.paused());
        assert!(demo.advance.forward_steps > 0);
        assert_eq!(demo.advance.status, PlaybackStatus::Paused);
        assert_eq!(demo.advance.requested_time, demo.system.sample_time());
        let revision = demo.system.revision();
        demo.command(Command::Rebuild).unwrap();
        demo.update(Duration::ZERO);
        assert!(demo.coherent);
        assert_eq!(demo.system.revision(), revision);
    }
}
