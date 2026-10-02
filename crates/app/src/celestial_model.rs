//! Bounded prescribed-motion validation, deliberately not celestial mechanics.

use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_app::interactive_clock::{ClockInterval, InteractiveClock};
use mundaris_math::*;
use mundaris_renderer::*;
use mundaris_simulation::{PlaybackRate, TimeController};
use mundaris_world::*;
use std::{
    num::NonZeroU64,
    time::{Duration, Instant},
};

const NAMES: [&str; 3] = ["Solace", "Aurelia", "Luma"];
const MASSES: [f64; 3] = [1.98847e30, 5.9722e24, 7.342e22];
const RADII: [f64; 3] = [6.957e8, 6.371e6, 1.7374e6];

/// Pure explicit-time sample. Initial tilt maps body-local spin axes into system
/// axes: q(t) = q_initial * spin_local(t), omega_system = q_initial * omega_local.
fn analytic_states(time: SimulationInstant) -> Result<[BodyState; 3]> {
    let t = time.seconds_since_epoch();
    ensure!(
        (-600.0..=600.0).contains(&t),
        "fixture time must be within -600..600 seconds"
    );
    let initial = [
        UnitRotation::identity(),
        UnitRotation::from_axis_angle(Direction3::try_new(DVec3::Z)?, 23.4_f64.to_radians())?,
        UnitRotation::from_axis_angle(Direction3::try_new(DVec3::X)?, -0.3)?,
    ];
    let rates = [0.00001, 0.2, -0.07];
    let centers = [
        DVec3::ZERO,
        DVec3::new(1.5e11, 30_000.0 * t + 2e7 * (0.001 * t).sin(), 0.0),
        DVec3::new(
            1.5e11 + 384e6,
            31_000.0 * t + 5e6 * (0.0017 * t + 0.4).sin(),
            3e6 * (0.0011 * t).sin(),
        ),
    ];
    let velocities = [
        DVec3::ZERO,
        DVec3::new(0.0, 30_000.0 + 20_000.0 * (0.001 * t).cos(), 0.0),
        DVec3::new(
            0.0,
            31_000.0 + 8500.0 * (0.0017 * t + 0.4).cos(),
            3300.0 * (0.0011 * t).cos(),
        ),
    ];
    let axis = Direction3::try_new(DVec3::Y)?;
    let sample = |index: usize| -> Result<BodyState> {
        Ok(BodyState::new(
            LocalPosition::try_metres(centers[index])?,
            LinearVelocity3::try_metres_per_second(velocities[index])?,
            initial[index].compose(UnitRotation::from_axis_angle(axis, rates[index] * t)?),
            AngularVelocity3::try_radians_per_second(
                initial[index].rotate_direction(axis)?.unit() * rates[index],
            )?,
        ))
    };
    Ok([sample(0)?, sample(1)?, sample(2)?])
}

struct Fixture {
    system: CelestialSystem,
    ids: [BodyId; 3],
    projection: CelestialFrameProjection,
    tree_namespace: u64,
}
impl Fixture {
    fn new() -> Result<Self> {
        let mut system = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
        let states = analytic_states(SimulationInstant::ZERO)?;
        let mut ids = Vec::with_capacity(3);
        for i in 0..3 {
            ids.push(system.insert_body(
                NAMES[i],
                BodyProperties::new(MASSES[i], RADII[i])?,
                states[i],
            )?);
        }
        let projection = CelestialFrameProjection::build(&system, NonZeroU64::new(1).unwrap())?;
        Ok(Self {
            system,
            ids: ids.try_into().expect("three fixture bodies"),
            projection,
            tree_namespace: 1,
        })
    }
    fn sample(&mut self, time: SimulationInstant) -> Result<()> {
        if time != self.system.sample_time() {
            let states = analytic_states(time)?;
            let updates = std::array::from_fn::<_, 3, _>(|i| BodyStateUpdate {
                body: self.ids[i],
                state: states[i],
            });
            self.system.update_states(time, &updates)?;
        }
        if self.projection.represented_revision() != self.system.revision() {
            self.projection.publish(&self.system)?;
        }
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        for i in 0..3 {
            self.system.edit_body(
                self.ids[i],
                NAMES[i],
                BodyProperties::new(MASSES[i], RADII[i])?,
            )?;
        }
        let states = analytic_states(SimulationInstant::ZERO)?;
        let updates = std::array::from_fn::<_, 3, _>(|i| BodyStateUpdate {
            body: self.ids[i],
            state: states[i],
        });
        self.system
            .update_states(SimulationInstant::ZERO, &updates)?;
        self.projection.publish(&self.system)?;
        Ok(())
    }
    fn rebuild(&mut self) -> Result<()> {
        let namespace = self
            .tree_namespace
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("tree namespace overflow"))?;
        let projection =
            CelestialFrameProjection::build(&self.system, NonZeroU64::new(namespace).unwrap())?;
        self.projection = projection;
        self.tree_namespace = namespace;
        Ok(())
    }
}

#[derive(Clone)]
struct PropertyDraft {
    name: String,
    mass: String,
    radius: String,
}
impl PropertyDraft {
    fn from_body(body: &CelestialBody) -> Self {
        Self {
            name: body.name().into(),
            mass: body.properties().mass_kg().to_string(),
            radius: body.properties().reference_radius_m().to_string(),
        }
    }
    fn apply(&self, system: &mut CelestialSystem, id: BodyId) -> Result<()> {
        let properties = BodyProperties::new(self.mass.parse()?, self.radius.parse()?)?;
        system.edit_body(id, &self.name, properties)?;
        Ok(())
    }
}
enum Command {
    Seek(f64),
    Reset,
    Select(usize),
    Focus(bool),
    Reexpress(bool),
    Edit(PropertyDraft),
    Rebuild,
    Playback { rate: f64, paused: bool },
}

pub(crate) struct CelestialModelDemo {
    fixture: Fixture,
    clock: TimeController,
    selected: usize,
    observer: FramePose,
    observer_velocity: FrameVelocity,
    draft: PropertyDraft,
    seek_seconds: f64,
    pending: Option<Command>,
    diagnostic: Option<String>,
    staging: DebugStaging,
    last_tick: Instant,
    host_clock: InteractiveClock,
}
impl CelestialModelDemo {
    pub(crate) fn new() -> Result<Self> {
        let fixture = Fixture::new()?;
        let selected = 1;
        let frame = fixture
            .projection
            .frames_for(fixture.ids[selected])?
            .body_fixed;
        let draft = PropertyDraft::from_body(fixture.system.body(fixture.ids[selected])?);
        Ok(Self {
            fixture,
            clock: TimeController::new(SimulationInstant::ZERO),
            selected,
            observer: FramePose::new(
                FramePosition::new(frame, LocalPosition::try_metres(DVec3::new(0.0, 0.0, 8.0))?),
                UnitRotation::identity(),
            ),
            observer_velocity: FrameVelocity::new(frame, LinearVelocity3::zero()),
            draft,
            seek_seconds: 0.0,
            pending: None,
            diagnostic: None,
            staging: DebugStaging::default(),
            last_tick: Instant::now(),
            host_clock: InteractiveClock::default(),
        })
    }
    pub(crate) fn reset_wall_tick(&mut self) {
        self.last_tick = Instant::now();
        self.host_clock.reset_capture();
    }
    pub(crate) fn set_lifecycle_drawable(&mut self, drawable: bool) {
        self.host_clock.set_drawable(drawable);
        if !drawable {
            self.last_tick = Instant::now();
        }
    }
    fn focus(&mut self, fixed: bool) -> Result<()> {
        let frames = self
            .fixture
            .projection
            .frames_for(self.fixture.ids[self.selected])?;
        let frame = if fixed {
            frames.body_fixed
        } else {
            frames.translating
        };
        self.observer = FramePose::new(
            FramePosition::new(frame, LocalPosition::try_metres(DVec3::new(0.0, 0.0, 8.0))?),
            UnitRotation::identity(),
        );
        self.observer_velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
        Ok(())
    }
    fn command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Playback { rate, paused } => {
                self.clock.set_rate(PlaybackRate::try_multiplier(rate)?);
                self.clock.set_paused(paused);
            }
            Command::Seek(seconds) => {
                let target = SimulationInstant::try_seconds_since_epoch(seconds)?;
                // Producer validates before either requested or authoritative time changes.
                self.fixture.sample(target)?;
                self.clock.seek(target);
            }
            Command::Reset => {
                self.fixture.reset()?;
                self.clock = TimeController::new(SimulationInstant::ZERO);
                self.clock.set_paused(true);
                self.selected = 1;
                self.focus(true)?;
                self.seek_seconds = 0.0;
                self.draft = PropertyDraft::from_body(
                    self.fixture.system.body(self.fixture.ids[self.selected])?,
                );
            }
            Command::Select(index) => {
                ensure!(index < 3, "invalid fixture selection");
                self.selected = index;
                self.draft =
                    PropertyDraft::from_body(self.fixture.system.body(self.fixture.ids[index])?);
                self.focus(true)?;
            }
            Command::Focus(fixed) => self.focus(fixed)?,
            Command::Reexpress(fixed) => {
                let frames = self
                    .fixture
                    .projection
                    .frames_for(self.fixture.ids[self.selected])?;
                let target = if fixed {
                    frames.body_fixed
                } else {
                    frames.translating
                };
                let evaluation = self.fixture.projection.tree().evaluate();
                let pose = evaluation.reexpress_pose(self.observer, target)?;
                let point = evaluation.convert_kinematic_point(
                    KinematicPoint::try_new(self.observer.position(), self.observer_velocity)?,
                    target,
                )?;
                self.observer = pose;
                self.observer_velocity = point.velocity();
            }
            Command::Edit(draft) => {
                draft.apply(&mut self.fixture.system, self.fixture.ids[self.selected])?;
                self.fixture.projection.publish(&self.fixture.system)?;
            }
            Command::Rebuild => {
                // Preserve observer local pose/velocity while replacing runtime handles.
                let frames = self
                    .fixture
                    .projection
                    .frames_for(self.fixture.ids[self.selected])?;
                let fixed = self.observer.position().frame() == frames.body_fixed;
                self.fixture.rebuild()?;
                let frames = self
                    .fixture
                    .projection
                    .frames_for(self.fixture.ids[self.selected])?;
                let target = if fixed {
                    frames.body_fixed
                } else {
                    frames.translating
                };
                self.observer = FramePose::new(
                    FramePosition::new(target, self.observer.position().local()),
                    self.observer.orientation(),
                );
                self.observer_velocity =
                    FrameVelocity::new(target, self.observer_velocity.relative());
            }
        }
        Ok(())
    }
    fn tick(&mut self, delta: Duration) {
        let result = if let Some(command) = self.pending.take() {
            self.command(command)
        } else {
            self.clock
                .advance_wall_time(delta)
                .map_err(anyhow::Error::new)
                .and_then(|time| self.fixture.sample(time))
        };
        if let Err(error) = result {
            self.clock.set_paused(true);
            self.diagnostic = Some(error.to_string());
        } else if !self.clock.paused() {
            self.diagnostic = None;
        }
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
        let elapsed = now.duration_since(self.last_tick);
        let delta = match self.host_clock.classify(elapsed) {
            ClockInterval::Accepted(elapsed) => elapsed,
            ClockInterval::Hidden => Duration::ZERO,
            ClockInterval::Discontinuity(gap) => {
                self.clock.set_paused(true);
                self.diagnostic = Some(format!(
                    "Interactive clock gap {:.3} s; no catch-up requested",
                    gap.as_secs_f64()
                ));
                Duration::ZERO
            }
        };
        self.last_tick = now;
        // Successful commands clear stale editor errors; paused redraw preserves errors.
        if self.pending.is_some() {
            self.diagnostic = None;
        }
        self.tick(delta);
        let paired = self
            .fixture
            .projection
            .coherent_view(&self.fixture.system)?;
        let evaluation = paired.evaluation();
        let view = PreparedView::new(
            &evaluation,
            self.observer,
            RenderPrecisionBudget::near_debug(),
        )?;
        let id = self.fixture.ids[self.selected];
        let frames = self.fixture.projection.frames_for(id)?;
        let body = self.fixture.system.body(id)?;
        let line = |end: DVec3, color| -> Result<DebugLine> {
            Ok(DebugLine {
                endpoints: [
                    FramePosition::new(frames.body_fixed, LocalPosition::origin()),
                    FramePosition::new(frames.body_fixed, LocalPosition::try_metres(end)?),
                ],
                color,
            })
        };
        let lines = [
            line(DVec3::X * 3.0, [1.0, 0.2, 0.2, 1.0])?,
            line(DVec3::Y * 3.0, [0.2, 1.0, 0.2, 1.0])?,
            line(DVec3::Z * 3.0, [0.2, 0.4, 1.0, 1.0])?,
        ];
        let mut debug = DebugFrame::new(
            &view,
            &mut self.staging,
            DebugProjection::near_debug(width, height)?,
        );
        debug.append_lines(frames.body_fixed, &lines)?;
        let mut markers = Vec::with_capacity(2);
        for (other_id, other) in self.fixture.system.bodies() {
            if other_id == id {
                continue;
            }
            let frame = self.fixture.projection.frames_for(other_id)?.translating;
            let delta = view
                .prepare_source(frame)?
                .view_displacement(FramePosition::new(frame, LocalPosition::origin()))?
                .metres();
            let distance = delta.x.hypot(delta.y).hypot(delta.z);
            ensure!(distance.is_finite(), "invalid distant marker distance");
            markers.push((other.name(), distance, delta / distance));
        }
        let pending = &mut self.pending;
        let draft = &mut self.draft;
        let seek_seconds = &mut self.seek_seconds;
        renderer.render_debug(&debug, |context| {
            egui::Window::new("Celestial model validation")
                .default_width(440.0)
                .vscroll(true)
                .show(context, |ui| {
                    ui.label("Prescribed analytic motion; -600..600 s working epoch");
                    ui.label(format!(
                        "Requested {:.6} s / authoritative {:.6} s",
                        self.clock.requested_time().seconds_since_epoch(),
                        self.fixture.system.sample_time().seconds_since_epoch()
                    ));
                    ui.label(format!(
                        "Rate {}x / {}",
                        self.clock.rate().multiplier(),
                        if self.clock.paused() { "paused" } else { "playing" }
                    ));
                    if ui.button(if self.clock.paused() { "Resume" } else { "Pause" }).clicked() {
                        *pending = Some(Command::Playback {
                            rate: self.clock.rate().multiplier(),
                            paused: !self.clock.paused(),
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        for rate in [-100.0, -10.0, -1.0, 0.1, 1.0, 10.0, 100.0, 1000.0] {
                            if ui.button(format!("{rate}x")).clicked() {
                                *pending = Some(Command::Playback { rate, paused: false });
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(seek_seconds).speed(1.0).suffix(" s"));
                        if ui.button("Seek").clicked() {
                            *pending = Some(Command::Seek(*seek_seconds));
                        }
                        if ui.button("Reset").clicked() {
                            *pending = Some(Command::Reset);
                        }
                    });
                    ui.label(format!(
                        "System revision {} / projected {} / {} bodies",
                        self.fixture.system.revision(),
                        self.fixture.projection.represented_revision(),
                        self.fixture.system.body_count()
                    ));
                    ui.horizontal(|ui| {
                        for (i, &body_id) in self.fixture.ids.iter().enumerate() {
                            let name = self.fixture.system.body(body_id).expect("fixture body").name();
                            if ui.selectable_label(i == self.selected, name).clicked() {
                                *pending = Some(Command::Select(i));
                            }
                        }
                    });
                    ui.label(format!("Selected {id:?}"));
                    ui.horizontal(|ui| {
                        if ui.button("Focus translating").clicked() {
                            *pending = Some(Command::Focus(false));
                        }
                        if ui.button("Focus body-fixed").clicked() {
                            *pending = Some(Command::Focus(true));
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Re-express translating").clicked() {
                            *pending = Some(Command::Reexpress(false));
                        }
                        if ui.button("Re-express body-fixed").clicked() {
                            *pending = Some(Command::Reexpress(true));
                        }
                    });
                    if ui.button("Rebuild derived projection").clicked() {
                        *pending = Some(Command::Rebuild);
                    }
                    ui.label(format!("Observer frame {:?}", self.observer.position().frame()));
                    ui.label(format!(
                        "Translating {:?} / fixed {:?} (runtime only)",
                        frames.translating, frames.body_fixed
                    ));
                    ui.separator();
                    ui.label(format!("Name: {}", body.name()));
                    ui.label(format!(
                        "Mass {:.6e} kg / reference radius {:.6e} m ({:.3} km)",
                        body.properties().mass_kg(),
                        body.properties().reference_radius_m(),
                        body.properties().reference_radius_m() / 1000.0
                    ));
                    ui.label(format!("Center m: {:?}", body.state().center_in_system().metres()));
                    ui.label(format!(
                        "Velocity m/s: {:?}",
                        body.state().center_velocity_in_system().metres_per_second()
                    ));
                    ui.label(format!(
                        "Body → system quaternion xyzw: {:?}",
                        body.state().body_to_system().quaternion().to_array()
                    ));
                    ui.label(format!(
                        "Angular velocity system rad/s: {:?}",
                        body.state().angular_velocity_in_system().radians_per_second()
                    ));
                    ui.collapsing("Edit properties (atomic Apply)", |ui| {
                        ui.label("Name (1–128 UTF-8 bytes)");
                        ui.text_edit_singleline(&mut draft.name);
                        ui.label("Mass kg");
                        ui.text_edit_singleline(&mut draft.mass);
                        ui.label("Reference radius m");
                        ui.text_edit_singleline(&mut draft.radius);
                        if ui.button("Apply").clicked() {
                            *pending = Some(Command::Edit(draft.clone()));
                        }
                    });
                    if let Some(error) = &self.diagnostic {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    ui.separator();
                    ui.label("Other bodies: bearing/distance only");
                    for (name, distance, bearing) in &markers {
                        ui.label(format!("{name}: {distance:.6e} m, camera bearing {bearing:.3?}"));
                    }
                    ui.label("Reset restores properties/state, Aurelia fixed focus, 1x paused; IDs survive.");
                    ui.label("Axes are body-local debug geometry; no physical sphere is drawn.");
                });
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_samples_match_forward_reverse_and_replay() {
        let mut clock =
            TimeController::new(SimulationInstant::try_seconds_since_epoch(-600.0).unwrap());
        for n in -600..=600 {
            let t = SimulationInstant::try_seconds_since_epoch(f64::from(n)).unwrap();
            assert_eq!(
                analytic_states(t).unwrap(),
                analytic_states(clock.requested_time()).unwrap()
            );
            clock.advance_wall_time(Duration::from_secs(1)).unwrap();
        }
        clock.seek(SimulationInstant::try_seconds_since_epoch(600.0).unwrap());
        clock.set_rate(PlaybackRate::try_multiplier(-1.0).unwrap());
        for n in (-600..=600).rev() {
            let t = SimulationInstant::try_seconds_since_epoch(f64::from(n)).unwrap();
            assert_eq!(
                analytic_states(t).unwrap(),
                analytic_states(clock.requested_time()).unwrap()
            );
            clock.advance_wall_time(Duration::from_secs(1)).unwrap();
        }
        let mut fixture = Fixture::new().unwrap();
        for n in [500.0, -600.0, 0.0, 99.0, -1.0, 600.0] {
            let t = SimulationInstant::try_seconds_since_epoch(n).unwrap();
            fixture.sample(t).unwrap();
            for (id, expected) in fixture.ids.into_iter().zip(analytic_states(t).unwrap()) {
                assert_eq!(fixture.system.body(id).unwrap().state(), &expected);
            }
            fixture.reset().unwrap();
            fixture.sample(t).unwrap();
            for (id, expected) in fixture.ids.into_iter().zip(analytic_states(t).unwrap()) {
                assert_eq!(fixture.system.body(id).unwrap().state(), &expected);
            }
        }
    }
    #[test]
    fn analytic_derivatives_match_independent_finite_differences() {
        for t in [-500.0, -1.0, 0.0, 50.0, 500.0] {
            let h = 0.001;
            let at =
                analytic_states(SimulationInstant::try_seconds_since_epoch(t).unwrap()).unwrap();
            let before =
                analytic_states(SimulationInstant::try_seconds_since_epoch(t - h).unwrap())
                    .unwrap();
            let after = analytic_states(SimulationInstant::try_seconds_since_epoch(t + h).unwrap())
                .unwrap();
            for i in 0..3 {
                let derivative = (after[i].center_in_system().metres()
                    - before[i].center_in_system().metres())
                    / (2.0 * h);
                // ~1e7 m excursions, subtracting at h=1ms: <1e-5 m/s roundoff.
                assert!(
                    (derivative - at[i].center_velocity_in_system().metres_per_second()).length()
                        < 1e-5
                );
                let omega = at[i].angular_velocity_in_system().radians_per_second();
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    let axis = Direction3::try_new(axis).unwrap();
                    let derivative = (after[i]
                        .body_to_system()
                        .rotate_direction(axis)
                        .unwrap()
                        .unit()
                        - before[i]
                            .body_to_system()
                            .rotate_direction(axis)
                            .unwrap()
                            .unit())
                        / (2.0 * h);
                    let expected = omega.cross(
                        at[i]
                            .body_to_system()
                            .rotate_direction(axis)
                            .unwrap()
                            .unit(),
                    );
                    // Central difference truncation O((0.2 rad/s * h)^2).
                    assert!((derivative - expected).length() < 2e-9);
                }
            }
        }
    }
    #[test]
    fn editor_focus_reset_rebuild_and_bounded_errors_preserve_domain_contracts() {
        let mut demo = CelestialModelDemo::new().unwrap();
        let ids = demo.fixture.ids;
        let before = demo.fixture.system.body(ids[1]).unwrap().clone();
        let revision = demo.fixture.system.revision();
        for (mass, radius, name) in [
            ("NaN", "1", "name"),
            ("0", "1", "name"),
            ("1", "-2", "name"),
            ("1", "2", ""),
        ] {
            assert!(
                demo.command(Command::Edit(PropertyDraft {
                    name: name.into(),
                    mass: mass.into(),
                    radius: radius.into()
                }))
                .is_err()
            );
            assert_eq!(demo.fixture.system.body(ids[1]).unwrap(), &before);
            assert_eq!(demo.fixture.system.revision(), revision);
        }
        demo.command(Command::Edit(PropertyDraft {
            name: "edited".into(),
            mass: "1e25".into(),
            radius: "7e6".into(),
        }))
        .unwrap();
        assert_eq!(
            demo.fixture.system.body(ids[1]).unwrap().state(),
            before.state()
        );
        demo.command(Command::Seek(100.0)).unwrap();
        let revision = demo.fixture.system.revision();
        demo.command(Command::Focus(false)).unwrap();
        let pose = demo.observer;
        demo.command(Command::Reexpress(true)).unwrap();
        let returned = demo
            .fixture
            .projection
            .tree()
            .evaluate()
            .reexpress_pose(demo.observer, pose.position().frame())
            .unwrap();
        assert!(
            (returned.position().local().metres() - pose.position().local().metres()).length()
                < 1e-12
        );
        assert_eq!(demo.fixture.system.revision(), revision);
        demo.command(Command::Focus(true)).unwrap();
        let local = demo.observer;
        for t in [-600.0, 0.0, 600.0] {
            demo.command(Command::Seek(t)).unwrap();
            let view = PreparedView::new(
                &demo.fixture.projection.tree().evaluate(),
                demo.observer,
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let source = view
                .prepare_source(demo.observer.position().frame())
                .unwrap();
            let origin =
                FramePosition::new(demo.observer.position().frame(), LocalPosition::origin());
            assert_eq!(
                source.view_displacement(origin).unwrap().metres(),
                DVec3::new(0.0, 0.0, -8.0)
            );
            assert_eq!(demo.observer, local);
        }
        let revision = demo.fixture.system.revision();
        demo.command(Command::Rebuild).unwrap();
        assert_eq!(demo.fixture.system.revision(), revision);
        assert_ne!(demo.observer.position().frame(), local.position().frame());
        assert_eq!(demo.observer.position().local(), local.position().local());
        assert!(demo.command(Command::Seek(601.0)).is_err());
        assert_eq!(
            demo.fixture.system.sample_time().seconds_since_epoch(),
            600.0
        );
        demo.command(Command::Reset).unwrap();
        assert_eq!(demo.fixture.ids, ids);
        assert_eq!(demo.fixture.system.body(ids[1]).unwrap(), &before);
        assert_eq!(demo.clock.requested_time(), SimulationInstant::ZERO);
        assert_eq!(
            demo.fixture.projection.tree().evaluate().sample_time_s(),
            0.0
        );
        assert!(demo.clock.paused());
    }
}
