//! Analytically sampled validation fixture. No integration or celestial-domain state.

use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::*;
use std::{num::NonZeroU64, time::Instant};

#[derive(Default)]
struct NamespaceCounter(u64);
impl NamespaceCounter {
    fn next(&mut self) -> Result<NonZeroU64> {
        self.0 = self
            .0
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("session namespace overflow"))?;
        NonZeroU64::new(self.0).ok_or_else(|| anyhow::anyhow!("namespace must be nonzero"))
    }
}

fn position(frame: FrameId, value: DVec3) -> Result<FramePosition> {
    Ok(FramePosition::new(frame, LocalPosition::try_metres(value)?))
}
fn rigid(t: DVec3, rotation: UnitRotation) -> Result<RigidTransform> {
    Ok(RigidTransform::new(Displacement3::try_metres(t)?, rotation))
}

fn analytic_states(time_s: f64, offset_m: f64) -> Result<[FrameState; 2]> {
    ensure!(
        time_s.is_finite() && (0.0..=600.0).contains(&time_s),
        "sample time outside 0..600 seconds"
    );
    let anchor = FrameState::new(
        rigid(
            DVec3::new(
                offset_m + 30_000.0 * time_s,
                20_000_000.0 * (0.001 * time_s).sin(),
                0.0,
            ),
            UnitRotation::identity(),
        )?,
        Some(FrameMotion::new(
            LinearVelocity3::try_metres_per_second(DVec3::new(
                30_000.0,
                20_000.0 * (0.001 * time_s).cos(),
                0.0,
            ))?,
            AngularVelocity3::zero(),
        )),
    );
    let rotation = UnitRotation::from_axis_angle(Direction3::try_new(DVec3::Y)?, 0.2 * time_s)?;
    let spin = FrameState::new(
        rigid(DVec3::ZERO, rotation)?,
        Some(FrameMotion::new(
            LinearVelocity3::zero(),
            AngularVelocity3::try_radians_per_second(DVec3::new(0.0, 0.2, 0.0))?,
        )),
    );
    Ok([anchor, spin])
}

struct Fixture {
    tree: FrameTree,
    anchor: FrameId,
    spin: FrameId,
    regional: FrameId,
    sibling: FrameId,
    primitives: Vec<Vec<DebugLine>>,
    offset_m: f64,
}
impl Fixture {
    fn new(namespace: NonZeroU64) -> Result<Self> {
        let mut tree = FrameTree::new(namespace);
        let root = tree.root();
        let [anchor_state, spin_state] = analytic_states(0.0, 1.5e11)?;
        let anchor = tree.insert(root, anchor_state)?;
        let spin = tree.insert(anchor, spin_state)?;
        let regional = tree.insert(
            spin,
            FrameState::stationary(rigid(
                DVec3::new(0.0, 6_371_000.0, 0.0),
                UnitRotation::identity(),
            )?),
        )?;
        let sibling = tree.insert(
            anchor,
            FrameState::stationary(rigid(
                DVec3::new(384_000_000.0, 0.0, 0.0),
                UnitRotation::identity(),
            )?),
        )?;
        let line = |a, b, color| -> Result<DebugLine> {
            Ok(DebugLine {
                endpoints: [position(regional, a)?, position(regional, b)?],
                color,
            })
        };
        let primitives = vec![
            vec![
                line(DVec3::ZERO, DVec3::X, [1.0, 0.1, 0.1, 1.0])?,
                line(DVec3::ZERO, DVec3::Y, [0.1, 1.0, 0.1, 1.0])?,
                line(DVec3::ZERO, DVec3::Z, [0.1, 0.3, 1.0, 1.0])?,
            ],
            wire_box(
                regional,
                DVec3::new(0.0, 1.0, -5.0),
                DVec3::new(1.0, 2.0, 1.0),
                [0.9, 0.9, 0.9, 1.0],
            )?,
            wire_box(
                regional,
                DVec3::new(20.0, 5.0, -30.0),
                DVec3::new(2.0, 10.0, 2.0),
                [0.3, 0.9, 0.6, 1.0],
            )?,
            wire_box(
                regional,
                DVec3::new(0.0, 100.0, -3000.0),
                DVec3::splat(200.0),
                [1.0, 0.7, 0.2, 1.0],
            )?,
            vec![line(
                DVec3::new(-0.1, 1.1, -4.49),
                DVec3::new(0.1, 1.1, -4.49),
                [1.0, 0.2, 0.8, 1.0],
            )?],
            wire_box(
                regional,
                DVec3::new(0.0, 1.11, -4.49),
                DVec3::new(0.2, 0.001, 0.001),
                [0.2, 0.8, 1.0, 1.0],
            )?,
        ];
        Ok(Self {
            tree,
            anchor,
            spin,
            regional,
            sibling,
            primitives,
            offset_m: 1.5e11,
        })
    }
    fn sample(&mut self, time_s: f64, stress: bool) -> Result<()> {
        let offset_m = if stress { 1e16 } else { 1.5e11 };
        if self.tree.evaluate().sample_time_s() == time_s && self.offset_m == offset_m {
            return Ok(());
        }
        let [anchor, spin] = analytic_states(time_s, offset_m)?;
        self.tree
            .update_states(time_s, &[(self.anchor, anchor), (self.spin, spin)])?;
        self.offset_m = offset_m;
        Ok(())
    }
}
fn wire_box(
    frame: FrameId,
    center: DVec3,
    dimensions: DVec3,
    color: [f32; 4],
) -> Result<Vec<DebugLine>> {
    let mut corners = [DVec3::ZERO; 8];
    for (index, corner) in corners.iter_mut().enumerate() {
        *corner = center
            + dimensions
                * 0.5
                * DVec3::new(
                    if index & 1 == 0 { -1.0 } else { 1.0 },
                    if index & 2 == 0 { -1.0 } else { 1.0 },
                    if index & 4 == 0 { -1.0 } else { 1.0 },
                );
    }
    let mut lines = Vec::with_capacity(12);
    for index in 0..8 {
        for bit in [1, 2, 4] {
            if index & bit == 0 {
                lines.push(DebugLine {
                    endpoints: [
                        position(frame, corners[index])?,
                        position(frame, corners[index | bit])?,
                    ],
                    color,
                });
            }
        }
    }
    Ok(lines)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ObserverState {
    pose: FramePose,
    velocity: FrameVelocity,
}
impl ObserverState {
    fn regional(frame: FrameId, p: DVec3, v: DVec3) -> Result<Self> {
        Ok(Self {
            pose: FramePose::new(position(frame, p)?, UnitRotation::identity()),
            velocity: FrameVelocity::new(frame, LinearVelocity3::try_metres_per_second(v)?),
        })
    }
    fn reexpress(&mut self, evaluation: FrameEvaluation<'_>, target: FrameId) -> Result<()> {
        let pose = evaluation.reexpress_pose(self.pose, target)?;
        let point = evaluation.convert_kinematic_point(
            KinematicPoint::try_new(self.pose.position(), self.velocity)?,
            target,
        )?;
        // Both queries succeeded in one evaluation before publishing observer state.
        *self = Self {
            pose,
            velocity: point.velocity(),
        };
        Ok(())
    }
}

/// Prescribed trajectory and analytic derivative, held at endpoints outside 0..30.
fn approach(elapsed_s: f64) -> Result<(f64, f64)> {
    ensure!(elapsed_s.is_finite(), "non-finite approach time");
    if elapsed_s <= 0.0 {
        return Ok((1e8, 0.0));
    }
    if elapsed_s >= 30.0 {
        return Ok((0.0, 0.0));
    }
    let u = elapsed_s / 30.0;
    let a = 1.0 - 3.0 * u * u + 2.0 * u * u * u;
    let separation = 1e8 * (12.0 * a).exp_m1() / 12.0_f64.exp_m1();
    let derivative =
        1e8 * 12.0 * (12.0 * a).exp() * (-6.0 * u + 6.0 * u * u) / (30.0 * 12.0_f64.exp_m1());
    Ok((separation, derivative))
}

#[derive(Default, Debug, Clone, Copy)]
struct Residuals {
    position_m: f64,
    velocity_m_s: f64,
    basis: f64,
}
fn residuals(actual: ObserverState, expected: ObserverState) -> Result<Residuals> {
    ensure!(
        actual.pose.position().frame() == expected.pose.position().frame(),
        "residual basis mismatch"
    );
    let mut basis: f64 = 0.0;
    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
        let direction = Direction3::try_new(axis)?;
        basis = basis.max(
            (actual
                .pose
                .orientation()
                .rotate_direction(direction)?
                .unit()
                - expected
                    .pose
                    .orientation()
                    .rotate_direction(direction)?
                    .unit())
            .length(),
        );
    }
    Ok(Residuals {
        position_m: (actual.pose.position().local().metres()
            - expected.pose.position().local().metres())
        .length(),
        velocity_m_s: (actual.velocity.relative().metres_per_second()
            - expected.velocity.relative().metres_per_second())
        .length(),
        basis,
    })
}
fn check_handoff(residual: Residuals) -> Result<()> {
    ensure!(
        residual.position_m.is_finite() && residual.position_m <= 1e-3,
        "handoff position residual {} m",
        residual.position_m
    );
    ensure!(
        residual.velocity_m_s.is_finite() && residual.velocity_m_s <= 5e-4,
        "handoff velocity residual {} m/s",
        residual.velocity_m_s
    );
    ensure!(
        residual.basis.is_finite() && residual.basis <= 1e-12,
        "handoff basis residual {}",
        residual.basis
    );
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Attached,
    Approach,
    Manual,
}
struct Controls {
    time_s: f64,
    elapsed_s: f64,
    playing: bool,
    stress: bool,
    mode: Mode,
    local_x_m: f64,
    handed_off: bool,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            time_s: 0.0,
            elapsed_s: 0.0,
            playing: true,
            stress: false,
            mode: Mode::Attached,
            local_x_m: 0.0,
            handed_off: false,
        }
    }
}
#[derive(Clone, Copy)]
enum Command {
    Reset,
    Attached,
    Approach,
    Reexpress(FrameId),
    Move(f64),
}

pub(crate) struct ReferenceFrameDemo {
    namespaces: NamespaceCounter,
    fixture: Fixture,
    observer: ObserverState,
    controls: Controls,
    staging: DebugStaging,
    last_tick: Instant,
    residual: Residuals,
    diagnostic: Option<String>,
    pending: Option<Command>,
}
impl ReferenceFrameDemo {
    pub(crate) fn new() -> Result<Self> {
        let mut namespaces = NamespaceCounter::default();
        let fixture = Fixture::new(namespaces.next()?)?;
        let observer =
            ObserverState::regional(fixture.regional, DVec3::new(0.0, 1.7, 0.0), DVec3::ZERO)?;
        Ok(Self {
            namespaces,
            fixture,
            observer,
            controls: Controls::default(),
            staging: DebugStaging::default(),
            last_tick: Instant::now(),
            residual: Residuals::default(),
            diagnostic: None,
            pending: None,
        })
    }
    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Reset => {
                self.fixture = Fixture::new(self.namespaces.next()?)?;
                self.controls = Controls::default();
                self.observer = ObserverState::regional(
                    self.fixture.regional,
                    DVec3::new(0.0, 1.7, 0.0),
                    DVec3::ZERO,
                )?;
                self.residual = Residuals::default();
            }
            Command::Attached => {
                self.controls.mode = Mode::Attached;
                self.controls.local_x_m = 0.0;
            }
            Command::Approach => {
                self.controls = Controls {
                    mode: Mode::Approach,
                    playing: false,
                    ..Controls::default()
                };
                self.fixture.sample(0.0, false)?;
                self.residual = Residuals::default();
            }
            Command::Reexpress(target) => {
                ensure!(!self.controls.playing, "pause before manual re-expression");
                // A root-based inspection resets the stress fixture while paused.
                if self.controls.stress {
                    ensure!(
                        self.observer.pose.position().frame() == self.fixture.regional,
                        "stress observer must be regional"
                    );
                    self.controls.stress = false;
                    self.fixture.sample(self.controls.time_s, false)?;
                }
                let before = self.observer;
                let mut after = before;
                after.reexpress(self.fixture.tree.evaluate(), target)?;
                let mut returned = after;
                returned.reexpress(self.fixture.tree.evaluate(), before.pose.position().frame())?;
                let residual = residuals(returned, before)?;
                check_handoff(residual)?;
                self.observer = after;
                self.residual = residual;
                self.controls.mode = Mode::Manual;
            }
            Command::Move(delta) => {
                ensure!(!self.controls.playing, "pause before local movement");
                let mut observer = self.observer;
                observer.reexpress(self.fixture.tree.evaluate(), self.fixture.regional)?;
                let position = observer.pose.position().displaced(FrameDisplacement::new(
                    self.fixture.regional,
                    Displacement3::try_metres(DVec3::new(delta, 0.0, 0.0))?,
                ))?;
                observer.pose = FramePose::new(position, observer.pose.orientation());
                self.observer = observer;
                self.controls.local_x_m = position.local().metres().x;
                self.controls.mode = Mode::Manual;
            }
        }
        Ok(())
    }
    fn sample_observer(&mut self) -> Result<()> {
        match self.controls.mode {
            Mode::Attached => {
                self.observer = ObserverState::regional(
                    self.fixture.regional,
                    DVec3::new(self.controls.local_x_m, 1.7, 0.0),
                    DVec3::ZERO,
                )?
            }
            Mode::Manual => {}
            Mode::Approach => {
                let (separation, velocity) = approach(self.controls.elapsed_s)?;
                let prescribed = ObserverState::regional(
                    self.fixture.regional,
                    DVec3::new(self.controls.local_x_m, 1.7, separation),
                    DVec3::new(0.0, 0.0, velocity),
                )?;
                if separation > 1e5 {
                    self.controls.handed_off = false;
                    let mut far = prescribed;
                    far.reexpress(self.fixture.tree.evaluate(), self.fixture.tree.root())?;
                    self.observer = far;
                } else if !self.controls.handed_off {
                    let mut near = prescribed;
                    near.reexpress(self.fixture.tree.evaluate(), self.fixture.tree.root())?;
                    near.reexpress(self.fixture.tree.evaluate(), self.fixture.regional)?;
                    let residual = residuals(near, prescribed)?;
                    check_handoff(residual)?;
                    self.observer = near;
                    self.residual = residual;
                    self.controls.handed_off = true;
                } else {
                    self.observer = prescribed;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn render(
        &mut self,
        renderer: &mut Renderer,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let now = Instant::now();
        let delta = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if let Some(command) = self.pending.take() {
            if let Err(error) = self.command(command) {
                self.diagnostic = Some(error.to_string());
            } else {
                self.diagnostic = None;
            }
        } else if self.controls.playing {
            self.controls.time_s = (self.controls.time_s + delta).min(600.0);
            if self.controls.mode == Mode::Approach {
                self.controls.elapsed_s = (self.controls.elapsed_s + delta).min(30.0);
            }
            if self.controls.time_s == 600.0
                || (self.controls.mode == Mode::Approach && self.controls.elapsed_s == 30.0)
            {
                self.controls.playing = false;
            }
        }
        self.fixture
            .sample(self.controls.time_s, self.controls.stress)?;
        self.sample_observer()?;
        let evaluation = self.fixture.tree.evaluate();
        let view = PreparedView::new(
            &evaluation,
            self.observer.pose,
            RenderPrecisionBudget::near_debug(),
        )?;
        let source = view.prepare_source(self.fixture.regional)?;
        let mut frame = DebugFrame::new(
            &view,
            &mut self.staging,
            DebugProjection::near_debug(width, height)?,
        );
        let mut omitted = 0;
        // Explicit representation selection checks every finite vertex first, then
        // omits a whole primitive beyond this fixture's range. No failed batch uploads.
        for primitive in &self.fixture.primitives {
            let mut outside = false;
            for line in primitive {
                for &point in &line.endpoints {
                    let delta = source.view_displacement(point)?.metres();
                    outside |=
                        delta.x.hypot(delta.y).hypot(delta.z) > view.budget().max_distance_m();
                }
            }
            if outside {
                omitted += primitive.len() * 2;
            } else {
                frame.append_lines(self.fixture.regional, primitive)?;
            }
        }
        let root_position = evaluation
            .convert_position(self.observer.pose.position(), evaluation.root())?
            .local()
            .metres();
        let box_relative = source
            .view_displacement(position(self.fixture.regional, DVec3::new(0.0, 1.0, -5.0))?)?
            .metres();
        let markers = [
            marker(&view, position(self.fixture.sibling, DVec3::ZERO)?)?,
            marker(&view, position(evaluation.root(), DVec3::ZERO)?)?,
        ];
        let controls = &mut self.controls;
        let pending = &mut self.pending;
        let observer = self.observer;
        let residual = self.residual;
        let diagnostic = &self.diagnostic;
        let regional = self.fixture.regional;
        let spin = self.fixture.spin;
        let root = evaluation.root();
        let max_error = frame.max_component_error_m();
        let drawn = frame.vertex_count();
        renderer.render_debug(&frame, |context| {
            egui::Window::new("Reference-frame validation")
                .default_width(420.0)
                .show(context, |ui| {
                    ui.label("Abstract debug primitives / analytic fixture");
                    ui.checkbox(&mut controls.playing, "Play analytic samples");
                    ui.add(
                        egui::Slider::new(&mut controls.time_s, 0.0..=600.0).text("Sample seconds"),
                    );
                    if controls.mode == Mode::Approach {
                        ui.add(
                            egui::Slider::new(&mut controls.elapsed_s, 0.0..=30.0)
                                .text("Approach seconds"),
                        );
                    }
                    ui.add_enabled_ui(controls.mode == Mode::Attached, |ui| {
                        ui.checkbox(&mut controls.stress, "Shared 1e16 m stress offset");
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Reset").clicked() {
                            *pending = Some(Command::Reset);
                        }
                        if ui.button("Attached").clicked() {
                            *pending = Some(Command::Attached);
                        }
                        if ui.button("Begin approach (paused)").clicked() {
                            *pending = Some(Command::Approach);
                        }
                    });
                    ui.add_enabled_ui(!controls.playing, |ui| {
                        ui.horizontal(|ui| {
                            for (label, frame) in
                                [("Root", root), ("Rotating", spin), ("Regional", regional)]
                            {
                                if ui.button(label).clicked() {
                                    *pending = Some(Command::Reexpress(frame));
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            for delta in [-1.0, -0.01, 0.01, 1.0] {
                                if ui.button(format!("{delta:+} m X")).clicked() {
                                    *pending = Some(Command::Move(delta));
                                }
                            }
                        });
                    });
                    ui.separator();
                    let label = if observer.pose.position().frame() == root {
                        "root"
                    } else if observer.pose.position().frame() == spin {
                        "rotating"
                    } else {
                        "regional"
                    };
                    ui.label(format!(
                        "Instant {:.6} s / revision {}",
                        evaluation.sample_time_s(),
                        evaluation.revision()
                    ));
                    ui.label(format!(
                        "Observer: {label} {:?}",
                        observer.pose.position().frame()
                    ));
                    ui.label(format!(
                        "Local pose m: {:?}",
                        observer.pose.position().local().metres()
                    ));
                    ui.label(format!(
                        "Relative velocity m/s: {:?}",
                        observer.velocity.relative().metres_per_second()
                    ));
                    ui.label(format!(
                        "Anchor translation m: {:?}",
                        evaluation
                            .state(self.fixture.anchor)
                            .map(|state| state.parent_from_local().translation().metres())
                    ));
                    ui.label(format!("Root-query diagnostic m: {root_position:?}"));
                    ui.label(
                        "Render origin: observer; camera axes; source subtraction before rotation",
                    );
                    ui.label(format!("Box view m: {box_relative:?}"));
                    ui.label(format!(
                        "Drawn {drawn} / explicitly omitted {omitted} vertices"
                    ));
                    ui.label(if drawn == 0 {
                        "Outside near debug draw range"
                    } else {
                        "Inside near debug draw range"
                    });
                    ui.label(format!("Max narrowing error: {max_error:.3e} m"));
                    ui.label(format!(
                        "Handoff / round-trip residual: {:.3e} m, {:.3e} m/s, basis {:.3e}",
                        residual.position_m, residual.velocity_m_s, residual.basis
                    ));
                    if let Some(diagnostic) = diagnostic {
                        ui.colored_label(egui::Color32::LIGHT_RED, diagnostic);
                    }
                    for (label, (distance, bearing)) in
                        ["Distant sibling marker", "Root-origin marker"]
                            .into_iter()
                            .zip(markers)
                    {
                        ui.label(format!("{label}: {distance:.6e} m (bearing only)"));
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(120.0, 40.0), egui::Sense::hover());
                        let center = rect.center();
                        let offset = egui::vec2(bearing[0] * 50.0, -bearing[1] * 15.0);
                        ui.painter().line_segment(
                            [center, center + offset],
                            egui::Stroke::new(1.0_f32, egui::Color32::YELLOW),
                        );
                        ui.painter()
                            .circle_filled(center + offset, 3.0, egui::Color32::YELLOW);
                    }
                });
        })?;
        Ok(())
    }
}

fn marker(view: &PreparedView<'_>, point: FramePosition) -> Result<(f64, [f32; 2])> {
    let delta = view
        .prepare_source(point.frame())?
        .view_displacement(point)?
        .metres();
    let distance = delta.x.hypot(delta.y).hypot(delta.z);
    ensure!(distance.is_finite(), "non-finite marker distance");
    let unit = if distance == 0.0 {
        DVec3::ZERO
    } else {
        delta / distance
    };
    // Only bounded dimensionless bearing coordinates narrow at the UI boundary.
    Ok((distance, [unit.x as f32, unit.y as f32]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_all_samples_both_offsets_and_independent_observer() {
        let mut fixture = Fixture::new(NonZeroU64::new(1).unwrap()).unwrap();
        let observer =
            ObserverState::regional(fixture.regional, DVec3::new(0.0, 1.7, 0.0), DVec3::ZERO)
                .unwrap();
        let originals = fixture.primitives.clone();
        let mut max_cpu: f64 = 0.0;
        let mut max_gpu: f64 = 0.0;
        for stress in [false, true] {
            for n in 0..=36_000 {
                fixture.sample(f64::from(n) / 60.0, stress).unwrap();
                let view = PreparedView::new(
                    &fixture.tree.evaluate(),
                    observer.pose,
                    RenderPrecisionBudget::near_debug(),
                )
                .unwrap();
                let source = view.prepare_source(fixture.regional).unwrap();
                for primitive in &fixture.primitives {
                    for line in primitive {
                        for &point in &line.endpoints {
                            let expected =
                                point.local().metres() - observer.pose.position().local().metres();
                            let actual = source.view_displacement(point).unwrap().metres();
                            assert!(actual.is_finite());
                            max_cpu = max_cpu.max((actual - expected).length());
                            max_gpu = max_gpu
                                .max(source.try_position(point).unwrap().max_component_error_m());
                        }
                    }
                }
            }
        }
        assert!(max_cpu <= 1e-9);
        assert!(max_gpu <= 1e-3);
        for (actual, original) in fixture
            .primitives
            .iter()
            .flatten()
            .zip(originals.iter().flatten())
        {
            assert_eq!(actual.endpoints, original.endpoints);
        }
        fixture.sample(0.0, false).unwrap();
        let root_observer =
            ObserverState::regional(fixture.tree.root(), DVec3::ZERO, DVec3::ZERO).unwrap();
        let point = position(fixture.regional, DVec3::new(20.0, 5.0, -30.0)).unwrap();
        let before = PreparedView::new(
            &fixture.tree.evaluate(),
            root_observer.pose,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap()
        .prepare_source(fixture.regional)
        .unwrap()
        .view_displacement(point)
        .unwrap();
        fixture.sample(100.0, false).unwrap();
        let after = PreparedView::new(
            &fixture.tree.evaluate(),
            root_observer.pose,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap()
        .prepare_source(fixture.regional)
        .unwrap()
        .view_displacement(point)
        .unwrap();
        assert!((before.metres() - after.metres()).length() > 1e6);
        eprintln!(
            "replay 72,002 samples: max CPU error {max_cpu:.6e} m, max narrowing {max_gpu:.6e} m"
        );
    }
    #[test]
    fn approach_derivative_handoff_local_movement_and_repeatability() {
        assert_eq!(approach(-1.0).unwrap(), (1e8, 0.0));
        assert_eq!(approach(0.0).unwrap(), (1e8, 0.0));
        assert_eq!(approach(30.0).unwrap(), (0.0, 0.0));
        assert_eq!(approach(31.0).unwrap(), (0.0, 0.0));
        for time in [1.0, 10.0, 20.0, 29.0] {
            let h = 1e-4;
            let derivative =
                (approach(time + h).unwrap().0 - approach(time - h).unwrap().0) / (2.0 * h);
            assert!((derivative - approach(time).unwrap().1).abs() <= 1e-2);
        }
        let mut demo = ReferenceFrameDemo::new().unwrap();
        demo.command(Command::Approach).unwrap();
        for n in 0..=1800 {
            let time = f64::from(n) / 60.0;
            demo.controls.elapsed_s = time;
            demo.controls.time_s = time;
            demo.fixture.sample(time, false).unwrap();
            demo.sample_observer().unwrap();
            let expected = demo.observer;
            demo.sample_observer().unwrap();
            // Handoff's first sample can have root quantization, subsequent local
            // samples use prescribed coordinates; compare under the root envelope.
            let mut expected_local = expected;
            expected_local
                .reexpress(demo.fixture.tree.evaluate(), demo.fixture.regional)
                .unwrap();
            let mut actual_local = demo.observer;
            actual_local
                .reexpress(demo.fixture.tree.evaluate(), demo.fixture.regional)
                .unwrap();
            check_handoff(residuals(actual_local, expected_local).unwrap()).unwrap();
        }
        assert!(demo.controls.handed_off);
        check_handoff(demo.residual).unwrap();
        let before = demo.observer;
        demo.command(Command::Move(0.01)).unwrap();
        assert!(
            (demo.observer.pose.position().local().metres().x
                - before.pose.position().local().metres().x
                - 0.01)
                .abs()
                <= 1e-9
        );
        eprintln!("approach handoff: {:?}", demo.residual);
    }
    #[test]
    fn failed_observer_migration_preserves_state_and_namespaces_do_not_repeat() {
        let mut demo = ReferenceFrameDemo::new().unwrap();
        let before = demo.observer;
        let other = FrameTree::new(NonZeroU64::new(99).unwrap());
        assert!(
            demo.observer
                .reexpress(demo.fixture.tree.evaluate(), other.root())
                .is_err()
        );
        assert_eq!(demo.observer, before);
        let root = demo.fixture.tree.root();
        demo.command(Command::Reset).unwrap();
        assert_ne!(root, demo.fixture.tree.root());
        let mut counter = NamespaceCounter(u64::MAX);
        assert!(counter.next().is_err());
    }
}
