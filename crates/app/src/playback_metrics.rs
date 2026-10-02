//! Achieved live playback from real commits and accounted active wall time, not presets.
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Clone, Copy)]
pub struct PlaybackMeasurement {
    pub achieved_rate: f64,
    pub window_s: f64,
    pub segment_rate: f64,
    pub segment_wall_s: f64,
    pub tick_limited: bool,
}
pub struct PlaybackMetrics {
    samples: VecDeque<(f64, f64)>,
    wall: f64,
    advance: f64,
    segment_wall: f64,
    segment_advance: f64,
}
impl Default for PlaybackMetrics {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(4096),
            wall: 0.0,
            advance: 0.0,
            segment_wall: 0.0,
            segment_advance: 0.0,
        }
    }
}
impl PlaybackMetrics {
    pub fn reset(&mut self) {
        self.samples.clear();
        self.wall = 0.0;
        self.advance = 0.0;
        self.segment_wall = 0.0;
        self.segment_advance = 0.0;
    }
    /// Call once per active update; explicit seek/reset/single/replay jumps are excluded by caller.
    pub fn record(&mut self, wall: Duration, committed_seconds: f64) {
        let seconds = wall.as_secs_f64();
        if seconds <= 0.0 || !committed_seconds.is_finite() {
            return;
        }
        self.samples.push_back((seconds, committed_seconds));
        self.wall += seconds;
        self.advance += committed_seconds;
        self.segment_wall += seconds;
        self.segment_advance += committed_seconds;
        while self.samples.len() > 1
            && (self.wall - self.samples.front().expect("sample").0 >= 10.0
                || self.samples.len() > 4096)
        {
            let (w, a) = self.samples.pop_front().expect("sample");
            self.wall -= w;
            self.advance -= a;
        }
    }
    pub fn measurement(
        &self,
        physical_step_s: f64,
        requested_rate: f64,
    ) -> Option<PlaybackMeasurement> {
        (self.wall > 0.0).then(|| PlaybackMeasurement {
            achieved_rate: self.advance / self.wall,
            window_s: self.wall,
            segment_rate: self.segment_advance / self.segment_wall,
            segment_wall_s: self.segment_wall,
            tick_limited: requested_rate.abs() * self.wall < physical_step_s,
        })
    }
}
