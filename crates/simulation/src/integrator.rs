//! Full-time KDK candidates are isolated from committed state and acceleration.

use crate::{GravityError, GravityEvaluationReport, evaluate_resolved_accelerations};
use glam::DVec3;
use mundaris_math::*;
use mundaris_world::{BodyId, BodyState, BodyStateUpdate, CelestialSystem, CelestialSystemError};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SimulationError {
    #[error(transparent)]
    Gravity(#[from] GravityError),
    #[error(transparent)]
    Math(#[from] MathError),
    #[error(transparent)]
    World(#[from] CelestialSystemError),
    #[error("stale simulation session; pause and explicitly rebranch after external mutation")]
    StaleSession,
    #[error("invalid simulation configuration or resource capacity")]
    InvalidConfig,
    #[error("tick/time overflow or insufficient epoch resolution")]
    TimeResolution,
    #[error("no completely validated candidate")]
    MissingCandidate,
    #[error("unrepresentable diagnostic arithmetic")]
    DiagnosticArithmetic,
}

/// Reused O(N) dense scratch. Creation gathers stable insertion order and counts
/// initialization separately. Preparing a candidate never changes the live world.
#[derive(Clone)]
pub struct IntegrationWorkspace {
    pub(crate) ids: Vec<BodyId>,
    pub(crate) masses_kg: Vec<f64>,
    pub(crate) states: Vec<BodyState>,
    positions_m: Vec<DVec3>,
    acceleration_m_s2: Vec<DVec3>,
    candidate_positions_m: Vec<DVec3>,
    half_velocities_m_s: Vec<DVec3>,
    next_acceleration_m_s2: Vec<DVec3>,
    pub(crate) updates: Vec<BodyStateUpdate>,
    fixed_step_s: f64,
    pub(crate) expected_revision: u64,
    pub(crate) expected_time: SimulationInstant,
    ready: bool,
}

impl IntegrationWorkspace {
    pub fn new(system: &CelestialSystem, fixed_step_s: f64) -> Result<Self, SimulationError> {
        if !fixed_step_s.is_finite() || fixed_step_s <= 0.0 {
            return Err(SimulationError::InvalidConfig);
        }
        let ids: Vec<_> = system.bodies().map(|(id, _)| id).collect();
        let masses_kg: Vec<f64> = system
            .bodies()
            .map(|(_, b)| b.properties().mass_kg())
            .collect();
        let states: Vec<_> = system.bodies().map(|(_, b)| *b.state()).collect();
        let positions_m: Vec<_> = states
            .iter()
            .map(|s| s.center_in_system().metres())
            .collect();
        let count = states.len();
        let mut acceleration_m_s2 = vec![DVec3::ZERO; count];
        evaluate_resolved_accelerations(
            &masses_kg,
            &positions_m,
            &mut acceleration_m_s2,
            fixed_step_s,
        )?;
        let updates = ids
            .iter()
            .zip(&states)
            .map(|(&body, &state)| BodyStateUpdate { body, state })
            .collect();
        Ok(Self {
            ids,
            masses_kg,
            states,
            positions_m,
            acceleration_m_s2,
            candidate_positions_m: vec![DVec3::ZERO; count],
            half_velocities_m_s: vec![DVec3::ZERO; count],
            next_acceleration_m_s2: vec![DVec3::ZERO; count],
            updates,
            fixed_step_s,
            expected_revision: system.revision(),
            expected_time: system.sample_time(),
            ready: false,
        })
    }
    pub(crate) fn check(&self, system: &CelestialSystem) -> Result<(), SimulationError> {
        if system.revision() != self.expected_revision
            || system.sample_time() != self.expected_time
            || system.body_count() != self.ids.len()
            || !system
                .bodies()
                .zip(&self.ids)
                .all(|((id, _), expected)| id == *expected)
        {
            return Err(SimulationError::StaleSession);
        }
        Ok(())
    }
    pub fn fixed_step_s(&self) -> f64 {
        self.fixed_step_s
    }
    pub fn states(&self) -> &[BodyState] {
        &self.states
    }
    /// One new force pass; the cached old pass has already checked old separation.
    pub fn prepare_step(&mut self) -> Result<GravityEvaluationReport, SimulationError> {
        self.ready = false;
        let h = self.fixed_step_s;
        for i in 0..self.states.len() {
            let v = self.states[i]
                .center_velocity_in_system()
                .metres_per_second();
            let half = v + (h * 0.5) * self.acceleration_m_s2[i];
            self.half_velocities_m_s[i] =
                LinearVelocity3::try_metres_per_second(half)?.metres_per_second();
            self.candidate_positions_m[i] =
                LocalPosition::try_metres(self.positions_m[i] + h * half)?.metres();
        }
        let report = evaluate_resolved_accelerations(
            &self.masses_kg,
            &self.candidate_positions_m,
            &mut self.next_acceleration_m_s2,
            h,
        )?;
        for i in 0..self.states.len() {
            let state = self.states[i];
            let velocity = LinearVelocity3::try_metres_per_second(
                self.half_velocities_m_s[i] + (h * 0.5) * self.next_acceleration_m_s2[i],
            )?;
            let omega = state.angular_velocity_in_system().radians_per_second();
            let speed = crate::gravity::distance(omega);
            let orientation = if speed == 0.0 {
                state.body_to_system()
            } else {
                UnitRotation::from_axis_angle(Direction3::try_new(omega)?, speed * h)?
                    .compose(state.body_to_system())
            };
            self.updates[i].state = BodyState::new(
                LocalPosition::try_metres(self.candidate_positions_m[i])?,
                velocity,
                orientation,
                state.angular_velocity_in_system(),
            );
        }
        self.ready = true;
        Ok(report)
    }
    /// All candidates publish in one world transaction; only then promote the cache.
    pub fn commit_candidate(
        &mut self,
        system: &mut CelestialSystem,
        instant: SimulationInstant,
    ) -> Result<(), SimulationError> {
        self.check(system)?;
        if !self.ready {
            return Err(SimulationError::MissingCandidate);
        }
        system.update_states(instant, &self.updates)?;
        self.promote();
        self.expected_revision = system.revision();
        self.expected_time = instant;
        Ok(())
    }
    pub(crate) fn promote(&mut self) {
        assert!(self.ready, "only a complete KDK candidate can be promoted");
        for (state, update) in self.states.iter_mut().zip(&self.updates) {
            *state = update.state;
        }
        std::mem::swap(&mut self.positions_m, &mut self.candidate_positions_m);
        std::mem::swap(
            &mut self.acceleration_m_s2,
            &mut self.next_acceleration_m_s2,
        );
        self.ready = false;
    }
}
