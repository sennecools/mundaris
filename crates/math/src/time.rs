//! Dimensioned time value only; playback and evolution policy live elsewhere.

/// Finite seconds relative to a caller-defined working epoch, not calendar time.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct SimulationInstant(f64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstantError {
    #[error("simulation instant/delta must be finite")]
    NonFinite,
    #[error("simulation time arithmetic overflow")]
    ArithmeticOverflow,
}

impl SimulationInstant {
    pub const ZERO: Self = Self(0.0);

    pub fn try_seconds_since_epoch(seconds: f64) -> Result<Self, InstantError> {
        if seconds.is_finite() {
            Ok(Self(seconds))
        } else {
            Err(InstantError::NonFinite)
        }
    }

    pub fn seconds_since_epoch(self) -> f64 {
        self.0
    }

    pub fn checked_add_seconds(self, delta_seconds: f64) -> Result<Self, InstantError> {
        if !delta_seconds.is_finite() {
            return Err(InstantError::NonFinite);
        }
        Self::try_seconds_since_epoch(self.0 + delta_seconds)
            .map_err(|_| InstantError::ArithmeticOverflow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_seconds_and_checked_arithmetic() {
        for seconds in [-600.0, 0.0, 600.0, f64::MAX] {
            let instant = SimulationInstant::try_seconds_since_epoch(seconds).unwrap();
            assert_eq!(instant.seconds_since_epoch(), seconds);
        }
        for seconds in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                SimulationInstant::try_seconds_since_epoch(seconds),
                Err(InstantError::NonFinite)
            );
            assert_eq!(
                SimulationInstant::ZERO.checked_add_seconds(seconds),
                Err(InstantError::NonFinite)
            );
        }
        assert_eq!(
            SimulationInstant::try_seconds_since_epoch(f64::MAX)
                .unwrap()
                .checked_add_seconds(f64::MAX),
            Err(InstantError::ArithmeticOverflow)
        );
        assert_eq!(
            SimulationInstant::ZERO
                .checked_add_seconds(-2.0)
                .unwrap()
                .seconds_since_epoch(),
            -2.0
        );
    }
}
