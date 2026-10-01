use mundaris_math::{InstantError, SimulationInstant};
use std::time::Duration;

/// Simulation seconds per monotonic host second. No engine-wide rate limit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackRate(f64);

#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum TimeError {
    #[error("playback rate must be finite")]
    NonFiniteRate,
    #[error("wall duration times playback rate overflowed")]
    TimeArithmeticOverflow,
    #[error(transparent)]
    Instant(#[from] InstantError),
}

impl PlaybackRate {
    pub const NORMAL: Self = Self(1.0);
    pub fn try_multiplier(multiplier: f64) -> Result<Self, TimeError> {
        if multiplier.is_finite() {
            Ok(Self(multiplier))
        } else {
            Err(TimeError::NonFiniteRate)
        }
    }
    pub fn multiplier(self) -> f64 {
        self.0
    }
}

/// Changes requested time only. Errors preserve the previous requested instant.
#[derive(Debug, Clone)]
pub struct TimeController {
    requested: SimulationInstant,
    rate: PlaybackRate,
    paused: bool,
}

impl TimeController {
    pub fn new(requested: SimulationInstant) -> Self {
        Self {
            requested,
            rate: PlaybackRate::NORMAL,
            paused: false,
        }
    }
    pub fn requested_time(&self) -> SimulationInstant {
        self.requested
    }
    pub fn rate(&self) -> PlaybackRate {
        self.rate
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn set_rate(&mut self, rate: PlaybackRate) {
        self.rate = rate;
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
    pub fn seek(&mut self, target: SimulationInstant) {
        self.requested = target;
    }
    pub fn advance_wall_time(&mut self, elapsed: Duration) -> Result<SimulationInstant, TimeError> {
        if !self.paused {
            let delta = elapsed.as_secs_f64() * self.rate.0;
            if !delta.is_finite() {
                return Err(TimeError::TimeArithmeticOverflow);
            }
            let requested = self.requested.checked_add_seconds(delta)?;
            self.requested = requested;
        }
        Ok(self.requested)
    }
}
