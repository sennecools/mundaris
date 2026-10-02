//! Host timing policy, independent of numerical timesteps and requested simulation time.
use std::time::Duration;

pub const DEFAULT_GAP_THRESHOLD: Duration = Duration::from_millis(250);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockInterval {
    Accepted(Duration),
    Hidden,
    Discontinuity(Duration),
}
/// Synthetic monotonic durations make sleep/lifecycle tests independent of real sleeping.
pub struct InteractiveClock {
    drawable: bool,
    fresh: bool,
    threshold: Duration,
    last_gap: Option<Duration>,
}
impl Default for InteractiveClock {
    fn default() -> Self {
        Self {
            drawable: true,
            fresh: true,
            threshold: DEFAULT_GAP_THRESHOLD,
            last_gap: None,
        }
    }
}
impl InteractiveClock {
    pub fn set_drawable(&mut self, drawable: bool) {
        if drawable != self.drawable {
            self.fresh = true;
            self.drawable = drawable;
        }
    }
    pub fn reset_capture(&mut self) {
        self.fresh = true;
    }
    pub fn threshold(&self) -> Duration {
        self.threshold
    }
    pub fn set_threshold(&mut self, threshold: Duration) -> bool {
        if threshold.is_zero() {
            return false;
        }
        self.threshold = threshold;
        true
    }
    pub fn last_gap(&self) -> Option<Duration> {
        self.last_gap
    }
    pub fn classify(&mut self, elapsed: Duration) -> ClockInterval {
        if !self.drawable {
            return ClockInterval::Hidden;
        }
        if self.fresh {
            self.fresh = false;
            return ClockInterval::Accepted(Duration::ZERO);
        }
        if elapsed > self.threshold {
            self.last_gap = Some(elapsed);
            ClockInterval::Discontinuity(elapsed)
        } else {
            ClockInterval::Accepted(elapsed)
        }
    }
}
