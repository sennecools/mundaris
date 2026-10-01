//! Composition of authoritative physics, coherent projection and disposable debug views.
use crate::{celestial_camera::*, gravity_fixtures::*, trails::*};
use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{
    num::NonZeroU64,
    time::{Duration, Instant},
};

enum Command {
    Pause(bool),
    Rate(f64),
    ResumeAdmission,
    Single(bool),
    Seek(u64),
    CancelSeek,
    Reset,
    Load(GravityFixture),
    Select(usize),
    Focus { fixed: bool, fit: bool },
    Overview,
    Reexpress(bool),
    Rebuild,
    Mass(f64),
    Radius(f64),
    Velocity(DVec3),
    Rename(String),
    TrailMode(bool),
    Orbit([f64; 2], f64),
}
struct Controls {
    pending: Option<Command>,
    seek_seconds: f64,
    name: String,
    mass: String,
    radius: String,
    velocity: [String; 3],
    markers: bool,
    labels: bool,
    trails: bool,
    relative_trails: bool,
}
impl Controls {
    fn new(body: &CelestialBody) -> Self {
        Self {
            pending: None,
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
    system: CelestialSystem,
    runner: FixedStepRunner,
    projection: CelestialFrameProjection,
    camera: CelestialCamera,
    ids: Vec<BodyId>,
    selected: usize,
    system_namespace: u64,
    tree_namespace: u64,
    overview_center_m: DVec3,
    overview_extent_m: f64,
    trails: TrailHistory,
    trail_lines: Vec<DebugLine>,
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
}
impl GravityOrbitsDemo {
    pub fn new() -> Result<Self> {
        Self::create(GravityFixture::Hierarchy, 1, 1)
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
        let center = diagnostics.center_of_mass_m;
        let extent = system
            .bodies()
            .map(|(_, b)| {
                (b.state().center_in_system().metres() - center).length()
                    + b.properties().reference_radius_m()
            })
            .fold(1.0, f64::max);
        let camera =
            CelestialCamera::overview(&projection.coherent_view(&system)?, center, extent)?;
        let ids: Vec<_> = system.bodies().map(|(id, _)| id).collect();
        let selected = 1;
        let controls = Controls::new(system.body(ids[selected])?);
        let advance = runner.report();
        let trails = TrailHistory::new(&system, scenario.trail_stride())?;
        Ok(Self {
            system,
            runner,
            projection,
            camera,
            ids,
            selected,
            system_namespace,
            tree_namespace,
            overview_center_m: center,
            overview_extent_m: extent,
            trails,
            trail_lines: Vec::new(),
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
        })
    }
    pub fn set_lifecycle_drawable(&mut self, drawable: bool) {
        if !drawable || self.hidden {
            self.runner.set_lifecycle_suspended(!drawable);
            self.last_wall = None;
            self.hidden = !drawable;
            self.achieved_elapsed = Duration::ZERO;
            self.achieved_rate = None;
            self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
            self.advance = self.runner.report();
        }
    }
    pub fn reset_wall_capture(&mut self) {
        self.last_wall = None;
    }
    fn seed_trails(&mut self) {
        self.trails.clear_and_seed(
            self.runner.branch_generation(),
            self.runner.tick(),
            &self.system,
        );
    }
    fn reseed_diagnostics(&mut self) -> Result<()> {
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
        let id = self.ids[self.selected];
        match command {
            Command::Pause(paused) => {
                self.runner.set_paused(paused);
                self.seeking = false;
                self.sample_diagnostics()?;
            }
            Command::Rate(rate) => {
                let rate = PlaybackRate::try_multiplier(rate)?;
                if self.runner.rate().multiplier().signum() != rate.multiplier().signum() {
                    self.seed_trails();
                }
                self.runner.set_rate(rate);
            }
            Command::ResumeAdmission => self.runner.resume_admission(),
            Command::Single(forward) => {
                self.runner.single_step(forward)?;
                self.seeking = false;
            }
            Command::Seek(tick) => {
                self.runner.seek_tick(tick)?;
                self.seeking = true;
                if tick == self.runner.tick() {
                    self.seed_trails();
                    self.seeking = false;
                    self.sample_diagnostics()?;
                }
            }
            Command::CancelSeek => {
                self.runner.cancel_seek();
                self.seeking = false;
                self.sample_diagnostics()?;
            }
            Command::Reset => {
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
            Command::Select(index) => {
                ensure!(index < self.ids.len(), "invalid body selection");
                self.selected = index;
                self.controls
                    .refresh_draft(self.system.body(self.ids[index])?);
            }
            Command::Focus { fixed, fit } => self.camera.focus(
                &self.projection.coherent_view(&self.system)?,
                id,
                fixed,
                fit,
            )?,
            Command::Overview => {
                self.camera = CelestialCamera::overview(
                    &self.projection.coherent_view(&self.system)?,
                    self.overview_center_m,
                    self.overview_extent_m,
                )?
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
            Command::TrailMode(relative) => {
                let mode = if relative {
                    TrailMode::SimultaneousBodyRelative(self.ids[1])
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
            Command::Orbit(drag, wheel) => self.camera.orbit_zoom(drag, wheel)?,
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
        if let Some(command) = self.controls.pending.take() {
            match self.command(command) {
                Ok(()) => self.diagnostic = None,
                Err(error) => self.diagnostic = Some(error.to_string()),
            }
        }
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
        let trails = &mut self.trails;
        let started = Instant::now();
        let result = self.runner.pump(&mut self.system, |tick, world| {
            if !seeking {
                trails.record_committed(branch, tick, world);
            }
        });
        self.pump_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.advance = match result {
            Ok(report) => report,
            Err(error) => {
                self.diagnostic = Some(error.to_string());
                self.seeking = false;
                *error.report
            }
        };
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
        self.achieved_elapsed = self.achieved_elapsed.saturating_add(elapsed);
        if self.achieved_elapsed >= Duration::from_millis(250) {
            self.achieved_rate = Some(
                (self.system.sample_time().seconds_since_epoch() - self.achieved_anchor_s)
                    / self.achieved_elapsed.as_secs_f64(),
            );
            self.achieved_anchor_s = self.system.sample_time().seconds_since_epoch();
            self.achieved_elapsed = Duration::ZERO;
        }
        self.refresh_report_metadata();
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
        self.update(elapsed);
        if !self.coherent {
            let (info, controls) = self.ui_info(CelestialPreparationReport::default(), 0.0, 0.0);
            renderer.render(|ctx| draw_ui(ctx, controls, &info, &[]))?;
            return Ok(());
        }
        let started = Instant::now();
        let pair = self.projection.coherent_view(&self.system)?;
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
            if surface > 0.0 {
                clearance = clearance.min(surface);
            }
            self.requests.push(CelestialRenderBody {
                body_fixed_frame: frames.body_fixed,
                reference_radius_m: radius,
                color: body_color(index),
                unlit: index == 0,
                selected: index == self.selected,
            });
        }
        let near = if clearance == f64::MAX {
            0.1
        } else {
            (0.01 * clearance).max(0.1)
        };
        let projection = CelestialProjection::try_new(width, height, 60.0_f64.to_radians(), near)?;
        let trail_source = if self.controls.trails {
            Some(self.trails.prepare_lines(
                &self.system,
                &self.projection,
                &mut self.trail_lines,
            )?)
        } else {
            None
        };
        let mut frame = CelestialFrame::new(&view, &mut self.staging, projection, &self.sphere);
        let prepared = (|| -> Result<()> {
            frame.append_bodies(&self.requests)?;
            if let Some(source) = trail_source {
                frame.append_historical_lines(source, &self.trail_lines)?;
            }
            let selected = self.requests[self.selected];
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
            let lines = [line(0)?, line(1)?, line(2)?];
            frame.append_historical_lines(source, &lines)?;
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
            system: &self.system,
            projection: &self.projection,
            runner: &self.runner,
            camera: &self.camera,
            ids: &self.ids,
            selected: self.selected,
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
        };
        let controls = &mut self.controls;
        renderer.render_celestial(&frame, |context| {
            draw_ui(context, controls, &info, frame.markers())
        })?;
        Ok(())
    }
    fn ui_info(
        &mut self,
        report: CelestialPreparationReport,
        near: f64,
        preparation_ms: f64,
    ) -> (UiInfo<'_>, &mut Controls) {
        (
            UiInfo {
                system: &self.system,
                projection: &self.projection,
                runner: &self.runner,
                camera: &self.camera,
                ids: &self.ids,
                selected: self.selected,
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
            },
            &mut self.controls,
        )
    }
}

struct UiInfo<'a> {
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
}
fn draw_ui(
    context: &egui::Context,
    controls: &mut Controls,
    info: &UiInfo<'_>,
    markers: &[CelestialMarker],
) {
    let scale = context.pixels_per_point();
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("celestial navigation overlays"),
    ));
    for marker in markers {
        if let Some([x, y]) = marker.screen_pixels {
            let position = egui::pos2(x / scale, y / scale);
            let selected = marker.request_index == info.selected;
            let color = if selected {
                egui::Color32::YELLOW
            } else {
                egui::Color32::LIGHT_GRAY
            };
            if controls.markers {
                painter.circle_stroke(
                    position,
                    if selected { 8.0 / scale } else { 4.0 / scale },
                    egui::Stroke::new(1.0_f32, color),
                );
            }
            if controls.labels {
                let body = info
                    .system
                    .body(info.ids[marker.request_index])
                    .expect("mapped render request body");
                painter.text(
                    position + egui::vec2(10.0 / scale, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!(
                        "{} {:.4e} m{} {:?}",
                        body.name(),
                        marker.distance_m,
                        if marker.occluded {
                            " (occluded overlay)"
                        } else {
                            ""
                        },
                        marker.representation
                    ),
                    egui::FontId::proportional(13.0),
                    color,
                );
            }
        }
    }
    egui::Window::new("Gravity / orbit validation").default_width(435.0).vscroll(true).show(context,|ui| {
        ui.label("Newtonian point masses / fixed KDK / debug celestial shading");
        ui.label(format!("Branch {} / tick {} → target {} / h {} s",info.advance.branch_generation,info.advance.tick,info.advance.target_tick,info.runner.config().fixed_step_s()));
        ui.label(format!("Requested {:.3} s / authoritative {:.3} s",info.advance.requested_time.seconds_since_epoch(),info.system.sample_time().seconds_since_epoch()));
        ui.label(format!("Requested {}x / achieved {} / {:?}",info.runner.rate().multiplier(),info.achieved_rate.map_or_else(||"not yet measured".into(),|r|format!("{r:.3}x (wall sample)")),info.advance.status));
        ui.label(format!("Pending {} ticks / {:.3} s (fraction {:.3} s)",info.advance.backlog_ticks,info.advance.pending_simulation_seconds,info.advance.fractional_seconds));
        ui.label(format!("Latest update: {} work / {} forward / {} restore; {:.3} ms",info.advance.work_steps,info.advance.forward_steps,info.advance.restored_steps,info.pump_ms));
        ui.label(format!("Latest rejected demand {:.3} s / latest explicit cancellation {:.3} s",info.advance.rejected_simulation_seconds,info.advance.cancelled_simulation_seconds));
        if let Some(remaining)=info.advance.replay_remaining {
            ui.colored_label(egui::Color32::YELLOW,format!("Private replay: {remaining} work remaining, estimate {}",info.throughput.map_or_else(||"unmeasured".into(),|rate|format!("{:.2} wall s",remaining as f64/rate))));
            if ui.button("Cancel seek/replay (retain live world)").clicked() {controls.pending=Some(Command::CancelSeek);}
        }
        ui.label(format!("History ticks {:?}, {} live + {} private replay bytes; snapshots restore / positive-step replay",info.advance.retained_ticks,info.advance.history_payload_bytes,info.advance.replay_history_payload_bytes));
        ui.horizontal(|ui| {
            if ui.button(if info.runner.paused() {"Resume"} else {"Pause (cancel debt)"}).clicked() {controls.pending=Some(Command::Pause(!info.runner.paused()));}
            if ui.button("Single +").clicked() {controls.pending=Some(Command::Single(true));}
            if ui.button("Single −").clicked() {controls.pending=Some(Command::Single(false));}
        });
        ui.horizontal_wrapped(|ui| {for rate in [-1000.0,-10.0,-1.0,0.1,1.0,10.0,100.0,1000.0,100000.0,1000000.0,1000000000.0] {
            if ui.button(format!("{rate}x")).clicked() {controls.pending=Some(Command::Rate(rate));}
        }});
        if info.advance.status==PlaybackStatus::DemandHaltedOverload {
            ui.colored_label(egui::Color32::LIGHT_RED,"Demand halted: overload. Admitted debt drains at fixed h.");
            if ui.button("Resume demand admission (retains debt)").clicked() {controls.pending=Some(Command::ResumeAdmission);}
        }
        ui.add(egui::DragValue::new(&mut controls.seek_seconds).speed(info.runner.config().fixed_step_s()).suffix(" seek s"));
        match info.runner.quantize_seek_seconds(controls.seek_seconds) {
            Ok(preview)=>{
                ui.label(format!("Nearest tick {} / {:.3} s / delta {:+.3} s",preview.tick,preview.resulting_seconds,preview.quantization_delta_s));
                if ui.button("Confirm seek (paused)").clicked() {controls.pending=Some(Command::Seek(preview.tick));}
            }
            Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}
        }
        if ui.button("Reset branch baseline (IDs / focus retained)").clicked() {controls.pending=Some(Command::Reset);}
        ui.horizontal(|ui| {for (label,fixture) in [("Load original hierarchy",GravityFixture::Hierarchy),("Load circular oracle",GravityFixture::Circular)] {if ui.button(label).clicked() {controls.pending=Some(Command::Load(fixture));}}});
        ui.separator();
        ui.label(format!("{} bodies / {} pairs / {} new force passes this update",info.system.body_count(),pair_count(info.system.body_count()).expect("valid count"),info.advance.force_passes));
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
        ui.horizontal(|ui| {for (i,&id) in info.ids.iter().enumerate() {if ui.selectable_label(i==info.selected,info.system.body(id).expect("fixture body").name()).clicked() {controls.pending=Some(Command::Select(i));}}});
        ui.horizontal_wrapped(|ui| {for (label,command) in [("Focus translating",Command::Focus {fixed:false,fit:false}),("Fit physical body",Command::Focus {fixed:false,fit:true}),("Inspect fixed spin",Command::Focus {fixed:true,fit:true}),("Overview",Command::Overview)] {if ui.button(label).clicked() {controls.pending=Some(command);}}});
        ui.horizontal(|ui| {
            if ui.button("Re-express translating").clicked() {controls.pending=Some(Command::Reexpress(false));}
            if ui.button("Re-express fixed").clicked() {controls.pending=Some(Command::Reexpress(true));}
        });
        ui.label("Drag outside panel: orbit / wheel: exponential zoom / Tab: next body");
        let body=info.system.body(info.ids[info.selected]).expect("selected fixture body");
        ui.label(format!("ID {:?} / {}",info.ids[info.selected],body.name()));
        ui.label(format!("Mass {:.6e} kg / physical reference radius {:.6e} m",body.properties().mass_kg(),body.properties().reference_radius_m()));
        ui.label(format!("System position {:?} m",body.state().center_in_system().metres()));
        ui.label(format!("System velocity {:?} m/s / speed {:.6e} m/s",body.state().center_velocity_in_system().metres_per_second(),body.state().center_velocity_in_system().metres_per_second().length()));
        ui.label(format!("Orientation {:?} / spin {:?} rad/s",body.state().body_to_system().quaternion().to_array(),body.state().angular_velocity_in_system().radians_per_second()));
        ui.collapsing("Authoring (physical edits start a paused new branch)",|ui| {
            ui.text_edit_singleline(&mut controls.name);if ui.button("Apply name (retain history)").clicked() {controls.pending=Some(Command::Rename(controls.name.clone()));}
            ui.text_edit_singleline(&mut controls.mass);if ui.button("Apply mass kg").clicked() {match controls.mass.parse() {Ok(value)=>controls.pending=Some(Command::Mass(value)),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,format!("{error}"));}}}
            ui.text_edit_singleline(&mut controls.radius);if ui.button("Apply radius m (geometry only)").clicked() {match controls.radius.parse() {Ok(value)=>controls.pending=Some(Command::Radius(value)),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,format!("{error}"));}}}
            for value in &mut controls.velocity {ui.text_edit_singleline(value);}
            if ui.button("Apply system velocity m/s").clicked() {
                let values=controls.velocity.iter().map(|v|v.parse::<f64>()).collect::<std::result::Result<Vec<_>,_>>();
                match values {Ok(v)=>controls.pending=Some(Command::Velocity(DVec3::new(v[0],v[1],v[2]))),Err(error)=>{ui.colored_label(egui::Color32::LIGHT_RED,error.to_string());}}
            }
        });
        ui.checkbox(&mut controls.markers,"Navigation markers (no physical size change)");ui.checkbox(&mut controls.labels,"Labels");ui.checkbox(&mut controls.trails,"Actual committed-history trails");
        let mut relative=controls.relative_trails;if ui.checkbox(&mut relative,"Simultaneous history relative to selected fixture companion").changed() {controls.pending=Some(Command::TrailMode(relative));}
        let mode_label=match info.trail_mode {TrailMode::Inertial=>"Inertial system-space history".to_string(),TrailMode::SimultaneousBodyRelative(id)=>format!("History relative to {} at each sample",info.system.body(id).expect("trail reference").name())};
        ui.label(mode_label);ui.label(format!("Trail stride {} ticks / {}/{} complete samples / {} bytes / times {:?} s",info.trail_stride,info.trail_count,info.trail_capacity,info.trail_bytes,info.trail_times));
        ui.collapsing("Renderer / frame diagnostics",|ui| {
            ui.label(format!("World revision {} / projected {}",info.system.revision(),info.projection.represented_revision()));
            ui.label(format!("Observer {:?} / frame {:?} / depth {}",info.camera.attachment(),info.camera.pose().position().frame(),info.projection.tree().evaluate().depth(info.camera.pose().position().frame()).unwrap_or(0)));
            ui.label(format!("{} triangles / {} markers / {} historical+axis segments",info.report.triangles,info.report.markers,info.report.trail_segments));
            ui.label(format!("{} subpixel / {} culled / {} precision fallbacks",info.report.subpixel_bodies,info.report.culled_bodies,info.report.precision_fallbacks));
            ui.label(format!("Narrowing {:.3e} m / projected {:.3e} px / prepare {:.3} ms",info.report.max_narrowing_error_m,info.report.max_projected_error_pixels,info.preparation_ms));
            ui.label(format!("Reverse-Z near {:.3e} m / observer distance {:.3e} m",info.near,info.camera.distance_m()));
            if ui.button("Rebuild disposable projection").clicked() {controls.pending=Some(Command::Rebuild);}
        });
        if let Some(error)=info.diagnostic {ui.colored_label(egui::Color32::LIGHT_RED,error);}
    });
    if !context.is_pointer_over_area() {
        context.input(|input| {
            if controls.markers
                && input.pointer.primary_clicked()
                && let Some(pos) = input.pointer.interact_pos()
                && let Some(index) = select_marker(markers, [pos.x * scale, pos.y * scale])
            {
                controls.pending = Some(Command::Select(index));
            }
            let drag = if input.pointer.primary_down() {
                input.pointer.delta()
            } else {
                egui::Vec2::ZERO
            };
            let wheel = input.raw_scroll_delta.y;
            if drag != egui::Vec2::ZERO || wheel != 0.0 {
                controls.pending =
                    Some(Command::Orbit([drag.x as f64, drag.y as f64], wheel as f64));
            }
        });
    }
    if !context.wants_keyboard_input() && context.input(|i| i.key_pressed(egui::Key::Tab)) {
        let backwards = context.input(|i| i.modifiers.shift);
        let index = if backwards {
            (info.selected + info.ids.len() - 1) % info.ids.len()
        } else {
            (info.selected + 1) % info.ids.len()
        };
        controls.pending = Some(Command::Select(index));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn radius_edit_updates_only_geometry_and_camera_navigation_envelope() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        demo.command(Command::Focus {
            fixed: false,
            fit: true,
        })
        .unwrap();
        let state = *demo.system.body(demo.ids[1]).unwrap().state();
        demo.command(Command::Radius(6.371e8)).unwrap();
        demo.update(Duration::ZERO);
        assert_eq!(*demo.system.body(demo.ids[1]).unwrap().state(), state);
        assert!(demo.camera.distance_m() >= 1.05 * 6.371e8);
        assert!(demo.projection.coherent_view(&demo.system).is_ok());
    }
    #[test]
    fn projection_failure_suppresses_drawing_and_reports_actual_paused_clock() {
        let mut demo = GravityOrbitsDemo::new().unwrap();
        let wrong = CelestialSystem::new(NonZeroU64::new(99).unwrap(), SimulationInstant::ZERO);
        demo.projection =
            CelestialFrameProjection::build(&wrong, NonZeroU64::new(99).unwrap()).unwrap();
        demo.command(Command::Pause(false)).unwrap();
        demo.update(Duration::from_secs(1));
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
