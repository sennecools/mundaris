//! Concrete app-owned selection between prescribed motion and integrated histories.
use anyhow::{Result, anyhow, ensure};
use astrum_simulation::*;
use astrum_world::*;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// An analytic single-step is a direct sample 60 seconds away, not an integration step.
pub const ANALYTIC_SINGLE_STEP_SECONDS: f64 = 60.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionSnapshot {
    pub mode: String,
    pub requested_time_s: f64,
    pub published_time_s: f64,
    pub paused: bool,
    pub rate: f64,
    pub sampling_status: String,
    pub latest_failure: Option<String>,
    pub analytic_body_count: Option<usize>,
    pub solver_iterations: Option<u64>,
    pub sampling_ms: Option<f64>,
    pub publication_ms: Option<f64>,
    pub measurement_scope: Option<String>,
}

pub enum MotionSession {
    Newtonian(Box<FixedStepRunner>),
    Analytic(Box<AnalyticSession>),
}

pub struct AnalyticSession {
    controller: TimeController,
    producer: AnalyticMotionProducer,
    epoch: SimulationInstant,
    last_successful_revision: u64,
    published_time: SimulationInstant,
    stats: MotionSampleStats,
    sampling_ms: Option<f64>,
    publication_ms: Option<f64>,
    latest_failure: Option<String>,
    suspended: bool,
    iteration_limit: u32,
}

impl AnalyticSession {
    pub fn new(world: &mut CelestialSystem, definition: CelestialMotionDefinition) -> Result<Self> {
        Self::with_iteration_limit(world, definition, 64)
    }
    /// Concrete bounded-work option, also used to exercise sampling-failure recovery.
    pub fn with_iteration_limit(
        world: &mut CelestialSystem,
        definition: CelestialMotionDefinition,
        limit: u32,
    ) -> Result<Self> {
        let epoch = world.sample_time();
        let mut producer = AnalyticMotionProducer::with_iteration_limit(world, definition, limit)?;
        let started = Instant::now();
        let stats = producer.sample(world, epoch)?;
        let mut controller = TimeController::new(epoch);
        controller.set_rate(PlaybackRate::try_multiplier(1000.0)?);
        controller.set_paused(true);
        Ok(Self {
            controller,
            producer,
            epoch,
            stats,
            last_successful_revision: world.revision(),
            published_time: epoch,
            sampling_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
            publication_ms: None,
            latest_failure: None,
            suspended: false,
            iteration_limit: limit,
        })
    }
    pub fn definition(&self) -> &CelestialMotionDefinition {
        self.producer.definition()
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.controller
            .set_paused(paused || self.latest_failure.is_some());
    }
    pub fn set_rate(&mut self, rate: PlaybackRate) {
        self.controller.set_rate(rate);
    }
    pub fn snapshot(&self, world: &CelestialSystem) -> MotionSnapshot {
        MotionSnapshot {
            mode: "prescribed_analytic".into(), requested_time_s: self.requested_time().seconds_since_epoch(),
            published_time_s: self.published_time.seconds_since_epoch(), paused: self.controller.paused(), rate: self.controller.rate().multiplier(),
            sampling_status: if self.latest_failure.is_some() { "failed" } else if self.published_time != world.sample_time() { "awaiting_frame_publication" } else { "published" }.into(),
            latest_failure: self.latest_failure.clone(), analytic_body_count: Some(self.stats.body_count),
            solver_iterations: Some(self.stats.solver_iterations), sampling_ms: self.sampling_ms, publication_ms: self.publication_ms,
            measurement_scope: Some("latest complete candidate evaluation + world commit; frame publication separately; excludes navigation/render".into()),
        }
    }
    pub fn requested_time(&self) -> SimulationInstant {
        self.controller.requested_time()
    }
    pub fn published_time(&self) -> SimulationInstant {
        self.published_time
    }
    fn fail(&mut self, error: impl std::fmt::Display) -> anyhow::Error {
        let message = format!(
            "Analytic motion rejected: {error}. Seek a supported time or reset; external edits require checked rebinding."
        );
        self.controller.set_paused(true);
        self.latest_failure = Some(message.clone());
        anyhow!(message)
    }
    /// Sample unchanged times only when the binding has changed, to diagnose staleness.
    /// Failures never modify world authority. Successful frame publication is separate.
    pub fn sample_requested(&mut self, world: &mut CelestialSystem) -> Result<bool> {
        if self.latest_failure.is_some() {
            return Ok(false);
        }
        if self.requested_time() == world.sample_time()
            && self.last_successful_revision == world.revision()
        {
            return Ok(false);
        }
        let started = Instant::now();
        match self.producer.sample(world, self.requested_time()) {
            Ok(stats) => {
                self.stats = stats;
                self.last_successful_revision = world.revision();
                self.sampling_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
                self.publication_ms = None;
                Ok(true)
            }
            Err(error) => {
                self.sampling_ms = None;
                Err(self.fail(error))
            }
        }
    }
    pub fn advance(&mut self, elapsed: Duration, world: &mut CelestialSystem) -> Result<bool> {
        if !self.suspended
            && self.latest_failure.is_none()
            && let Err(error) = self.controller.advance_wall_time(elapsed)
        {
            return Err(self.fail(format!(
                "advancing {elapsed:?} at {}x: {error}",
                self.controller.rate().multiplier()
            )));
        }
        self.sample_requested(world)
    }
    pub fn seek_seconds(&mut self, seconds: f64, world: &mut CelestialSystem) -> Result<bool> {
        self.controller.set_paused(true);
        self.latest_failure = None;
        let time = SimulationInstant::try_seconds_since_epoch(seconds)
            .map_err(|e| self.fail(format!("requested seek {seconds:?} s: {e}")))?;
        self.controller.seek(time);
        self.sample_requested(world)
    }
    pub fn single(&mut self, forward: bool, world: &mut CelestialSystem) -> Result<bool> {
        let seconds = world.sample_time().seconds_since_epoch()
            + if forward {
                ANALYTIC_SINGLE_STEP_SECONDS
            } else {
                -ANALYTIC_SINGLE_STEP_SECONDS
            };
        self.seek_seconds(seconds, world)
    }
    pub fn reset(&mut self, world: &mut CelestialSystem) -> Result<bool> {
        self.seek_seconds(self.epoch.seconds_since_epoch(), world)
    }
    pub fn frame_published(&mut self, world: &CelestialSystem, duration: Duration) {
        self.published_time = world.sample_time();
        self.publication_ms = Some(duration.as_secs_f64() * 1000.0);
        if self.last_successful_revision == world.revision()
            && self.requested_time() == world.sample_time()
        {
            self.latest_failure = None;
        }
    }
    pub fn frame_failed(&mut self, error: impl std::fmt::Display) {
        self.publication_ms = None;
        let _ = self.fail(error);
    }

    /// Build and validate an edited candidate before replacing authority/binding.
    /// The immutable authored periods, orbit geometry and epoch remain unchanged.
    pub fn edit_properties(
        &mut self,
        world: &mut CelestialSystem,
        id: BodyId,
        properties: BodyProperties,
    ) -> Result<()> {
        self.checked_edit(world, |candidate| candidate.edit_properties(id, properties))
    }
    pub fn rename(&mut self, world: &mut CelestialSystem, id: BodyId, name: &str) -> Result<()> {
        let properties = *world.body(id)?.properties();
        self.checked_edit(world, |candidate| candidate.edit_body(id, name, properties))
    }
    fn checked_edit(
        &mut self,
        world: &mut CelestialSystem,
        edit: impl FnOnce(&mut CelestialSystem) -> std::result::Result<(), CelestialSystemError>,
    ) -> Result<()> {
        ensure!(
            world.revision() == self.last_successful_revision,
            "stale analytic session; edit rejected before mutation"
        );
        // Validate immutable authoring before the world's atomic property/metadata
        // edit. Those APIs cannot change namespace/topology, so reconstruction
        // cannot fail compatibility after a successful edit. World identity never forks.
        let definition = CelestialMotionDefinition::new(world, self.definition().definitions())?;
        edit(world)?;
        self.producer =
            AnalyticMotionProducer::with_iteration_limit(world, definition, self.iteration_limit)?;
        self.last_successful_revision = world.revision();
        Ok(())
    }
}

impl MotionSession {
    pub fn is_analytic(&self) -> bool {
        matches!(self, Self::Analytic(_))
    }
    pub fn newtonian(&self) -> Option<&FixedStepRunner> {
        match self {
            Self::Newtonian(r) => Some(r),
            _ => None,
        }
    }
    pub fn newtonian_mut(&mut self) -> Result<&mut FixedStepRunner> {
        match self {
            Self::Newtonian(r) => Ok(r),
            _ => Err(anyhow!(
                "Newtonian history operation unavailable in prescribed motion"
            )),
        }
    }
    pub fn paused(&self) -> bool {
        match self {
            Self::Newtonian(r) => r.paused(),
            Self::Analytic(a) => a.controller.paused(),
        }
    }
    pub fn rate(&self) -> PlaybackRate {
        match self {
            Self::Newtonian(r) => r.rate(),
            Self::Analytic(a) => a.controller.rate(),
        }
    }
    pub fn set_paused(&mut self, value: bool) {
        match self {
            Self::Newtonian(r) => r.set_paused(value),
            Self::Analytic(a) => {
                // A failed target is not retried automatically by Resume.
                a.controller.set_paused(value || a.latest_failure.is_some());
            }
        }
    }
    pub fn set_rate(&mut self, value: PlaybackRate) {
        match self {
            Self::Newtonian(r) => r.set_rate(value),
            Self::Analytic(a) => a.controller.set_rate(value),
        }
    }
    pub fn set_lifecycle_suspended(&mut self, value: bool) {
        match self {
            Self::Newtonian(r) => r.set_lifecycle_suspended(value),
            Self::Analytic(a) => a.suspended = value,
        }
    }
    pub fn snapshot(&self, world: &CelestialSystem) -> MotionSnapshot {
        match self {
            Self::Analytic(a) => a.snapshot(world),
            Self::Newtonian(r) => MotionSnapshot {
                mode: "newtonian".into(),
                requested_time_s: r.report().requested_time.seconds_since_epoch(),
                published_time_s: world.sample_time().seconds_since_epoch(),
                paused: r.paused(),
                rate: r.rate().multiplier(),
                sampling_status: format!("{:?}", r.report().status),
                latest_failure: None,
                analytic_body_count: None,
                solver_iterations: None,
                sampling_ms: None,
                publication_ms: None,
                measurement_scope: None,
            },
        }
    }
}
