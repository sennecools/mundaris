//! Integer physical ticks, explicit admitted demand, bounded work and private replay.

use crate::{
    IntegrationWorkspace, PlaybackRate, SimulationError, SimulationInstant, TimeController,
    history::History, pair_count,
};
use astrum_world::{BodyId, BodyProperties, BodyState, CelestialSystem};
use std::time::Duration;

const MAX_EXACT_TICK: u64 = (1u64 << 53) - 1;

#[derive(Debug, Clone, Copy)]
pub struct SimulationConfig {
    fixed_step_s: f64,
    work_limit: u32,
    backlog_limit: u64,
    history_ticks: usize,
    history_bytes: usize,
}
impl SimulationConfig {
    pub fn try_new(fixed_step_s: f64) -> Result<Self, SimulationError> {
        if !fixed_step_s.is_finite() || fixed_step_s <= 0.0 {
            return Err(SimulationError::InvalidConfig);
        }
        Ok(Self {
            fixed_step_s,
            work_limit: 512,
            backlog_limit: 65536,
            history_ticks: 2048,
            history_bytes: 16 * 1024 * 1024,
        })
    }
    pub fn with_limits(
        mut self,
        work_steps: u32,
        backlog_ticks: u64,
        history_ticks: usize,
        history_bytes: usize,
    ) -> Result<Self, SimulationError> {
        if work_steps == 0
            || backlog_ticks == 0
            || !(2..=2048).contains(&history_ticks)
            || history_bytes > 16 * 1024 * 1024
        {
            return Err(SimulationError::InvalidConfig);
        }
        self.work_limit = work_steps;
        self.backlog_limit = backlog_ticks;
        self.history_ticks = history_ticks;
        self.history_bytes = history_bytes;
        Ok(self)
    }
    pub fn fixed_step_s(self) -> f64 {
        self.fixed_step_s
    }
    pub fn work_limit(self) -> u32 {
        self.work_limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Paused,
    Playing,
    Lagging,
    DemandHaltedOverload,
    LifecycleSuspended,
    Replaying,
    OriginBoundary,
    NumericalFailure,
}

#[derive(Debug, Clone, Copy)]
pub struct SimulationAdvanceReport {
    pub authoritative_time: SimulationInstant,
    pub requested_time: SimulationInstant,
    pub tick: u64,
    pub target_tick: u64,
    pub work_steps: u32,
    pub forward_steps: u32,
    pub restored_steps: u32,
    pub force_passes: u32,
    pub pair_evaluations: u64,
    pub backlog_ticks: u64,
    pub pending_simulation_seconds: f64,
    pub fractional_seconds: f64,
    pub requested_rate: f64,
    pub status: PlaybackStatus,
    pub rejected_simulation_seconds: f64,
    pub cancelled_simulation_seconds: f64,
    pub replay_remaining: Option<u64>,
    pub retained_ticks: Option<(u64, u64)>,
    pub history_payload_bytes: usize,
    pub replay_history_payload_bytes: usize,
    pub branch_generation: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("simulation pump failed at committed tick {tick}: {source}",tick=.report.tick)]
pub struct SimulationPumpError {
    pub source: SimulationError,
    pub report: Box<SimulationAdvanceReport>,
}

#[derive(Debug, Clone, Copy)]
pub struct QuantizedSeek {
    pub tick: u64,
    pub resulting_seconds: f64,
    pub quantization_delta_s: f64,
}

struct Replay {
    target: u64,
    tick: u64,
    initialized: bool,
}

/// Does not own world or frames. The optional callback observes only successful
/// live commits. No private replay intermediate reaches world/history visualization.
pub struct FixedStepRunner {
    config: SimulationConfig,
    clock: TimeController,
    epoch: SimulationInstant,
    tick: u64,
    target: u64,
    branch: u64,
    segment_anchor: f64,
    segment_elapsed: Duration,
    baseline: Box<[BodyState]>,
    work: IntegrationWorkspace,
    history: History,
    replay_work: IntegrationWorkspace,
    replay_history: History,
    replay: Option<Replay>,
    seek: Option<u64>,
    single: bool,
    overload: bool,
    suspended: bool,
    status: PlaybackStatus,
    rejected: f64,
    cancelled: f64,
}
impl FixedStepRunner {
    pub fn new(
        system: &CelestialSystem,
        config: SimulationConfig,
    ) -> Result<Self, SimulationError> {
        tick_time(system.sample_time(), config.fixed_step_s, 0)?;
        crate::system_diagnostics(system)?;
        let work = IntegrationWorkspace::new(system, config.fixed_step_s)?;
        let baseline = work.states().to_vec().into_boxed_slice();
        // Live and private replay rings share one payload ceiling. Reserving both
        // before stepping preserves cancellation without exceeding the byte policy.
        let ring_byte_budget = config.history_bytes / 2;
        let mut history = History::new(&baseline, config.history_ticks, ring_byte_budget)?;
        history.push(0, &baseline);
        let replay_history = History::new(&baseline, config.history_ticks, ring_byte_budget)?;
        let mut clock = TimeController::new(system.sample_time());
        clock.set_paused(true);
        Ok(Self {
            config,
            clock,
            epoch: system.sample_time(),
            tick: 0,
            target: 0,
            branch: 0,
            segment_anchor: system.sample_time().seconds_since_epoch(),
            segment_elapsed: Duration::ZERO,
            baseline,
            replay_work: work.clone(),
            work,
            history,
            replay_history,
            replay: None,
            seek: None,
            single: false,
            overload: false,
            suspended: false,
            status: PlaybackStatus::Paused,
            rejected: 0.0,
            cancelled: 0.0,
        })
    }
    pub fn tick(&self) -> u64 {
        self.tick
    }
    pub fn branch_generation(&self) -> u64 {
        self.branch
    }
    pub fn config(&self) -> SimulationConfig {
        self.config
    }
    pub fn paused(&self) -> bool {
        self.clock.paused()
    }
    pub fn rate(&self) -> PlaybackRate {
        self.clock.rate()
    }
    fn authority(&self) -> SimulationInstant {
        self.work.expected_time
    }
    fn segment(&mut self, anchor: SimulationInstant) {
        self.clock.seek(anchor);
        self.segment_anchor = anchor.seconds_since_epoch();
        self.segment_elapsed = Duration::ZERO;
    }
    fn clear_demand(&mut self) {
        let cancelled = self.clock.requested_time().seconds_since_epoch()
            - self.authority().seconds_since_epoch();
        if cancelled != 0.0 {
            self.cancelled = cancelled;
        }
        self.target = self.tick;
        self.segment(self.authority());
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.clear_demand();
        self.clock.set_paused(paused);
        self.seek = None;
        self.replay = None;
        self.single = false;
        self.status = if paused {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        };
    }
    pub fn set_rate(&mut self, rate: PlaybackRate) {
        let old = self.clock.rate().multiplier();
        let new = rate.multiplier();
        if new == 0.0 {
            self.set_paused(true);
        } else if old.signum() != new.signum() {
            self.clear_demand();
            self.seek = None;
            self.replay = None;
        } else {
            self.segment(self.clock.requested_time());
        }
        self.clock.set_rate(rate);
        if self.overload && new.abs() < old.abs() {
            self.overload = false;
            self.status = if self.clock.paused() {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Playing
            };
        }
    }
    /// Overload resume retains admitted debt and fraction, distinct from paused Resume.
    pub fn resume_admission(&mut self) {
        self.overload = false;
        self.segment(self.clock.requested_time());
        self.status = if self.clock.paused() {
            PlaybackStatus::Paused
        } else {
            PlaybackStatus::Playing
        };
    }
    pub fn set_lifecycle_suspended(&mut self, suspended: bool) {
        if suspended != self.suspended {
            self.clear_demand();
            self.seek = None;
            self.replay = None;
            self.single = false;
            self.suspended = suspended;
            self.status = if suspended {
                PlaybackStatus::LifecycleSuspended
            } else if self.clock.paused() {
                PlaybackStatus::Paused
            } else {
                PlaybackStatus::Playing
            };
        }
    }
    /// Equal admitted cumulative Duration and command timestamps imply identical demand.
    /// Over-cap intervals reject atomically, latch overload and preserve existing debt.
    pub fn admit_wall_elapsed(&mut self, elapsed: Duration) -> Result<(), SimulationError> {
        if self.suspended || self.clock.paused() || self.replay.is_some() || self.seek.is_some() {
            return Ok(());
        }
        let seconds = elapsed.as_secs_f64() * self.clock.rate().multiplier();
        if !seconds.is_finite() {
            return Err(SimulationError::TimeResolution);
        }
        if self.overload {
            self.rejected = seconds;
            return Ok(());
        }
        let cumulative = self
            .segment_elapsed
            .checked_add(elapsed)
            .ok_or(SimulationError::TimeResolution)?;
        let requested_s =
            self.segment_anchor + cumulative.as_secs_f64() * self.clock.rate().multiplier();
        if !requested_s.is_finite() {
            return Err(SimulationError::TimeResolution);
        }
        let requested_s = requested_s.max(self.epoch.seconds_since_epoch());
        let coordinate =
            (requested_s - self.epoch.seconds_since_epoch()) / self.config.fixed_step_s;
        let target_f = if self.clock.rate().multiplier() < 0.0 {
            coordinate.ceil()
        } else {
            coordinate.floor()
        };
        if !target_f.is_finite() || target_f > MAX_EXACT_TICK as f64 {
            return Err(SimulationError::TimeResolution);
        }
        let target = target_f as u64;
        tick_time(self.epoch, self.config.fixed_step_s, target)?;
        if target.abs_diff(self.tick) > self.config.backlog_limit {
            self.overload = true;
            self.rejected = seconds;
            self.status = PlaybackStatus::DemandHaltedOverload;
            return Ok(());
        }
        self.segment_elapsed = cumulative;
        self.clock.seek(
            SimulationInstant::try_seconds_since_epoch(requested_s)
                .map_err(|_| SimulationError::TimeResolution)?,
        );
        self.target = target;
        Ok(())
    }
    pub fn quantize_seek_seconds(&self, seconds: f64) -> Result<QuantizedSeek, SimulationError> {
        let coordinate = (seconds - self.epoch.seconds_since_epoch()) / self.config.fixed_step_s;
        if !coordinate.is_finite() || coordinate < 0.0 || coordinate > MAX_EXACT_TICK as f64 {
            return Err(SimulationError::TimeResolution);
        }
        let floor = coordinate.floor();
        let tick = (if coordinate - floor > 0.5 {
            floor + 1.0
        } else {
            floor
        }) as u64;
        let resulting_seconds =
            tick_time(self.epoch, self.config.fixed_step_s, tick)?.seconds_since_epoch();
        Ok(QuantizedSeek {
            tick,
            resulting_seconds,
            quantization_delta_s: resulting_seconds - seconds,
        })
    }
    pub fn seek_tick(&mut self, target: u64) -> Result<(), SimulationError> {
        let time = tick_time(self.epoch, self.config.fixed_step_s, target)?;
        self.set_paused(true);
        self.overload = false;
        self.target = target;
        self.segment(time);
        self.seek = (target != self.tick).then_some(target);
        Ok(())
    }
    pub fn cancel_seek(&mut self) {
        self.set_paused(true);
    }
    pub fn single_step(&mut self, forward: bool) -> Result<(), SimulationError> {
        let target = if forward {
            self.tick
                .checked_add(1)
                .ok_or(SimulationError::TimeResolution)?
        } else {
            self.tick.saturating_sub(1)
        };
        tick_time(self.epoch, self.config.fixed_step_s, target)?;
        self.set_paused(true);
        self.overload = false;
        self.target = target;
        self.segment(tick_time(self.epoch, self.config.fixed_step_s, target)?);
        self.single = true;
        if target == self.tick {
            self.status = PlaybackStatus::OriginBoundary;
        }
        Ok(())
    }
    /// Headless exact forward demand. Integration still respects fixed h and the
    /// work/admission ceilings; this is not a seek or an immediate world mutation.
    pub fn request_forward_to_tick(&mut self, target: u64) -> Result<(), SimulationError> {
        if self.suspended || target < self.tick || self.replay.is_some() || self.seek.is_some() {
            return Err(SimulationError::InvalidConfig);
        }
        let time = tick_time(self.epoch, self.config.fixed_step_s, target)?;
        if self.overload || target - self.tick > self.config.backlog_limit {
            self.overload = true;
            self.status = PlaybackStatus::DemandHaltedOverload;
            self.rejected =
                time.seconds_since_epoch() - self.clock.requested_time().seconds_since_epoch();
            return Ok(());
        }
        self.clock.set_paused(false);
        self.segment(time);
        self.target = target;
        self.single = false;
        Ok(())
    }
    pub fn report(&self) -> SimulationAdvanceReport {
        let requested = self.clock.requested_time();
        SimulationAdvanceReport {
            authoritative_time: self.authority(),
            requested_time: requested,
            tick: self.tick,
            target_tick: self.target,
            work_steps: 0,
            forward_steps: 0,
            restored_steps: 0,
            force_passes: 0,
            pair_evaluations: 0,
            backlog_ticks: self.target.abs_diff(self.tick),
            pending_simulation_seconds: requested.seconds_since_epoch()
                - self.authority().seconds_since_epoch(),
            fractional_seconds: requested.seconds_since_epoch()
                - tick_time(self.epoch, self.config.fixed_step_s, self.target)
                    .expect("validated target")
                    .seconds_since_epoch(),
            requested_rate: self.rate().multiplier(),
            status: self.status,
            rejected_simulation_seconds: self.rejected,
            cancelled_simulation_seconds: self.cancelled,
            replay_remaining: self
                .replay
                .as_ref()
                .map(|r| r.target.saturating_sub(r.tick) + u64::from(!r.initialized) + 1),
            retained_ticks: self.history.range(),
            history_payload_bytes: self.history.bytes(),
            replay_history_payload_bytes: self.replay_history.bytes(),
            branch_generation: self.branch,
        }
    }
    fn start_replay(&mut self, target: u64) {
        self.replay = Some(Replay {
            target,
            tick: 0,
            initialized: false,
        });
        self.status = PlaybackStatus::Replaying;
    }
    pub fn pump(
        &mut self,
        system: &mut CelestialSystem,
        after_commit: impl FnMut(u64, &CelestialSystem),
    ) -> Result<SimulationAdvanceReport, SimulationPumpError> {
        self.pump_with_work_limit(system, self.config.work_limit, after_commit)
    }
    /// Caller-controlled bounded scheduling, including restores and private replay.
    /// Invalid limits leave world and runner untouched. Physical h/order never change.
    pub fn pump_with_work_limit(
        &mut self,
        system: &mut CelestialSystem,
        max_work_units: u32,
        mut after_commit: impl FnMut(u64, &CelestialSystem),
    ) -> Result<SimulationAdvanceReport, SimulationPumpError> {
        if max_work_units == 0 || max_work_units > self.config.work_limit {
            return Err(SimulationPumpError {
                source: SimulationError::InvalidConfig,
                report: Box::new(self.report()),
            });
        }
        let mut counters = [0u32; 4]; // work, forward, restores, force passes
        let result = self.pump_inner(system, max_work_units, &mut after_commit, &mut counters);
        if let Err(source) = result {
            self.clock.set_paused(true);
            self.replay = None;
            self.seek = None;
            self.single = false;
            self.status = PlaybackStatus::NumericalFailure;
            let mut report = self.report();
            Self::counters(&mut report, counters, self.work.ids.len());
            return Err(SimulationPumpError {
                source,
                report: Box::new(report),
            });
        }
        let mut report = self.report();
        Self::counters(&mut report, counters, self.work.ids.len());
        Ok(report)
    }
    fn counters(report: &mut SimulationAdvanceReport, c: [u32; 4], bodies: usize) {
        report.work_steps = c[0];
        report.forward_steps = c[1];
        report.restored_steps = c[2];
        report.force_passes = c[3];
        report.pair_evaluations = pair_count(bodies).expect("setup pair count") * u64::from(c[3]);
    }
    fn pump_inner(
        &mut self,
        system: &mut CelestialSystem,
        max_work_units: u32,
        after_commit: &mut impl FnMut(u64, &CelestialSystem),
        c: &mut [u32; 4],
    ) -> Result<(), SimulationError> {
        self.work.check(system)?;
        if self.suspended {
            return Ok(());
        }
        if !self.clock.paused()
            && self.tick == 0
            && self.target == 0
            && self.rate().multiplier() < 0.0
        {
            self.set_paused(true);
            self.status = PlaybackStatus::OriginBoundary;
            return Ok(());
        }
        while c[0] < max_work_units {
            if let Some(replay) = &mut self.replay {
                if !replay.initialized {
                    self.replay_work.restore(&self.baseline)?;
                    self.replay_work.promote();
                    self.replay_history.clear();
                    self.replay_history.push(0, &self.baseline);
                    replay.initialized = true;
                    c[0] += 1;
                    c[3] += 1;
                } else if replay.tick < replay.target {
                    tick_time(self.epoch, self.config.fixed_step_s, replay.tick + 1)?;
                    self.replay_work.prepare_step()?;
                    self.replay_work.promote();
                    replay.tick += 1;
                    self.replay_history
                        .push(replay.tick, self.replay_work.states());
                    c[0] += 1;
                    c[3] += 1;
                } else {
                    let target = replay.target;
                    self.replay_work.publish_current(
                        system,
                        tick_time(self.epoch, self.config.fixed_step_s, target)?,
                    )?;
                    std::mem::swap(&mut self.work, &mut self.replay_work);
                    std::mem::swap(&mut self.history, &mut self.replay_history);
                    self.tick = target;
                    self.replay = None;
                    self.seek = None;
                    c[0] += 1;
                    c[2] += 1;
                    after_commit(self.tick, system);
                    self.segment(self.clock.requested_time());
                    if self.tick == 0 && self.rate().multiplier() < 0.0 {
                        self.set_paused(true);
                        self.status = PlaybackStatus::OriginBoundary;
                    }
                }
                continue;
            }
            if self.target == self.tick {
                break;
            }
            if self.clock.paused() && self.seek.is_none() && !self.single {
                break;
            }
            let next = if let Some(target) = self.seek {
                target
            } else if self.target < self.tick {
                self.tick - 1
            } else {
                self.tick + 1
            };
            if self.seek.is_some() || next < self.tick {
                if let Some(states) = self.history.get(next) {
                    self.work.restore(states)?;
                    c[3] += 1;
                    self.work.commit_candidate(
                        system,
                        tick_time(self.epoch, self.config.fixed_step_s, next)?,
                    )?;
                    self.history.discard_after(next);
                    self.tick = next;
                    self.seek = None;
                    c[0] += 1;
                    c[2] += 1;
                    after_commit(self.tick, system);
                } else {
                    self.start_replay(next);
                }
            } else {
                let time = tick_time(self.epoch, self.config.fixed_step_s, next)?;
                self.work.prepare_step()?;
                c[3] += 1;
                self.work.commit_candidate(system, time)?;
                self.tick = next;
                self.history.push(next, self.work.states());
                c[0] += 1;
                c[1] += 1;
                after_commit(self.tick, system);
            }
            if self.tick == 0 && self.rate().multiplier() < 0.0 {
                self.set_paused(true);
                self.status = PlaybackStatus::OriginBoundary;
                break;
            }
        }
        if self.target == self.tick {
            self.single = false;
        }
        if self.replay.is_some() {
            self.status = PlaybackStatus::Replaying;
        } else if self.overload {
            self.status = PlaybackStatus::DemandHaltedOverload;
        } else if self.status != PlaybackStatus::OriginBoundary
            && self.status != PlaybackStatus::NumericalFailure
        {
            self.status = if self.clock.paused() {
                PlaybackStatus::Paused
            } else if self.target != self.tick {
                PlaybackStatus::Lagging
            } else {
                PlaybackStatus::Playing
            };
        }
        Ok(())
    }
    pub fn reset_branch(&mut self, system: &mut CelestialSystem) -> Result<(), SimulationError> {
        self.work.check(system)?;
        let cancelled = self.clock.requested_time().seconds_since_epoch()
            - self.authority().seconds_since_epoch();
        self.work.restore(&self.baseline)?;
        self.work.commit_candidate(system, self.epoch)?;
        self.tick = 0;
        self.history.clear();
        self.history.push(0, &self.baseline);
        self.set_paused(true);
        self.set_rate(PlaybackRate::NORMAL);
        self.overload = false;
        self.status = PlaybackStatus::Paused;
        self.cancelled = cancelled;
        Ok(())
    }
    /// External mutation is detected, never silently regathered. Explicit paused
    /// rebranch captures current data; append/h changes require this full setup.
    pub fn rebranch_after_edit(&mut self, system: &CelestialSystem) -> Result<(), SimulationError> {
        if !self.paused() {
            return Err(SimulationError::StaleSession);
        }
        let branch = self
            .branch
            .checked_add(1)
            .ok_or(SimulationError::InvalidConfig)?;
        let mut replacement = Self::new(system, self.config)?;
        replacement.branch = branch;
        *self = replacement;
        Ok(())
    }
    pub fn change_fixed_step(
        &mut self,
        system: &CelestialSystem,
        config: SimulationConfig,
    ) -> Result<(), SimulationError> {
        self.work.check(system)?;
        let branch = self
            .branch
            .checked_add(1)
            .ok_or(SimulationError::InvalidConfig)?;
        let mut replacement = Self::new(system, config)?;
        replacement.branch = branch;
        replacement.cancelled = self.clock.requested_time().seconds_since_epoch()
            - self.authority().seconds_since_epoch();
        *self = replacement;
        Ok(())
    }
    fn preflight_edit(
        &self,
        system: &CelestialSystem,
        id: BodyId,
        properties: Option<BodyProperties>,
        state: Option<BodyState>,
    ) -> Result<Self, SimulationError> {
        self.work.check(system)?;
        system.body(id)?;
        let branch = self
            .branch
            .checked_add(1)
            .ok_or(SimulationError::InvalidConfig)?;
        // Scratch-only proposal: no authoritative identity fork. Every fallible
        // numerical/time/capacity check occurs before the world mutation.
        let mut replacement = Self::new(system, self.config)?;
        let index = replacement
            .work
            .ids
            .iter()
            .position(|&body| body == id)
            .expect("checked body ID");
        if let Some(p) = properties {
            replacement.work.masses_kg[index] = p.mass_kg();
        }
        if let Some(s) = state {
            replacement.baseline[index] = s;
        }
        replacement.work.restore(&replacement.baseline)?;
        replacement.work.promote();
        crate::diagnostics::dense_diagnostics(
            system.sample_time(),
            &replacement.work.masses_kg,
            &replacement.baseline,
        )?;
        replacement.replay_work = replacement.work.clone();
        replacement.history.clear();
        replacement.history.push(0, &replacement.baseline);
        replacement.branch = branch;
        replacement.cancelled = self.clock.requested_time().seconds_since_epoch()
            - self.authority().seconds_since_epoch();
        Ok(replacement)
    }
    pub fn edit_properties(
        &mut self,
        system: &mut CelestialSystem,
        id: BodyId,
        properties: BodyProperties,
    ) -> Result<(), SimulationError> {
        let mut replacement = self.preflight_edit(system, id, Some(properties), None)?;
        system.edit_properties(id, properties)?;
        replacement.work.expected_revision = system.revision();
        *self = replacement;
        Ok(())
    }
    pub fn edit_state(
        &mut self,
        system: &mut CelestialSystem,
        id: BodyId,
        state: BodyState,
    ) -> Result<(), SimulationError> {
        let mut replacement = self.preflight_edit(system, id, None, Some(state))?;
        system.edit_state(id, state)?;
        replacement.work.expected_revision = system.revision();
        *self = replacement;
        Ok(())
    }
    pub fn rename(
        &mut self,
        system: &mut CelestialSystem,
        id: BodyId,
        name: &str,
    ) -> Result<(), SimulationError> {
        self.work.check(system)?;
        let properties = *system.body(id)?.properties();
        system.edit_body(id, name, properties)?;
        self.work.expected_revision = system.revision();
        Ok(())
    }
}

fn tick_time(
    epoch: SimulationInstant,
    h: f64,
    tick: u64,
) -> Result<SimulationInstant, SimulationError> {
    if tick > MAX_EXACT_TICK {
        return Err(SimulationError::TimeResolution);
    }
    let t = epoch.seconds_since_epoch() + tick as f64 * h;
    if !t.is_finite() {
        return Err(SimulationError::TimeResolution);
    }
    let ulp = t.abs().next_up() - t.abs();
    let adjacent = if tick == 0 {
        epoch.seconds_since_epoch() + h - t
    } else {
        t - (epoch.seconds_since_epoch() + (tick - 1) as f64 * h)
    };
    if !ulp.is_finite()
        || ulp > h / 1024.0
        || adjacent <= 0.0
        || (adjacent - h).abs() > (1e-12 * h).max(4.0 * ulp)
    {
        return Err(SimulationError::TimeResolution);
    }
    SimulationInstant::try_seconds_since_epoch(t).map_err(|_| SimulationError::TimeResolution)
}
