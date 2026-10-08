//! Distinguish elapsed stage time from time actually scheduled on the calling CPU thread.
use std::time::{Duration, Instant};

/// Diagnostic clock pair. Thread CPU time excludes descheduling and waits; OS
/// accounting resolution may be coarser than short stages (including zero deltas).
/// Failure to read that clock is reported as unavailable, never as zero CPU work.
pub struct CpuStageTimer {
    wall: Instant,
    thread: Option<cpu_time::ThreadTime>,
}
impl Default for CpuStageTimer {
    fn default() -> Self {
        Self::new()
    }
}
impl CpuStageTimer {
    pub fn new() -> Self {
        Self {
            wall: Instant::now(),
            thread: cpu_time::ThreadTime::try_now().ok(),
        }
    }
    pub fn wall_elapsed(&self) -> Duration {
        self.wall.elapsed()
    }
    pub fn thread_elapsed(&self) -> Option<Duration> {
        self.thread.as_ref().and_then(|t| t.try_elapsed().ok())
    }
}
