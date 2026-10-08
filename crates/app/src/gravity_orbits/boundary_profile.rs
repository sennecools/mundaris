//! Observational publication scheduling counters. Unknown eligibility is never idle time.
use std::time::{Duration, Instant};

#[derive(Default, Clone, Debug, serde::Serialize)]
pub(super) struct BoundarySchedulingSnapshot {
    pub dispatch_checks: u64,
    pub worker_unavailable_checks: u64,
    pub admission_budget_checks: u64,
    pub prepared_capacity_checks: u64,
    pub transition_capacity_checks: u64,
    pub no_eligible_work_checks: u64,
    pub eligible_dispatches: u64,
    pub completion_tick_dispatch_skips: u64,
    pub eligibility_unknown_ticks: u64,
    /// Host preparation after a candidate passed dependencies, before dispatch.
    /// Includes host scheduling/preemption; does not measure pure worker starvation.
    pub eligible_idle_dispatch_preparation_us: u64,
    pub maximum_eligible_idle_dispatch_preparation_us: u64,
    pub completion_drains: u64,
    pub prepared_to_main_consume_us: u64,
    pub maximum_prepared_to_main_consume_us: u64,
}

#[derive(Default)]
pub(super) struct BoundarySchedulingMetrics {
    totals: BoundarySchedulingSnapshot,
    eligible_idle_start: Option<Instant>,
}

impl BoundarySchedulingMetrics {
    pub fn counters(&mut self) -> &mut BoundarySchedulingSnapshot {
        &mut self.totals
    }

    /// Called only after exact residency, dependencies and closure checks pass.
    pub fn observe_eligible_idle(&mut self, now: Instant, worker_idle: bool, capacity: bool) {
        if worker_idle && capacity {
            self.eligible_idle_start.get_or_insert(now);
        } else {
            self.close(now);
        }
    }

    pub fn close(&mut self, now: Instant) {
        if let Some(start) = self.eligible_idle_start.take() {
            let elapsed = micros(now.saturating_duration_since(start));
            self.totals.eligible_idle_dispatch_preparation_us = self
                .totals
                .eligible_idle_dispatch_preparation_us
                .saturating_add(elapsed);
            self.totals.maximum_eligible_idle_dispatch_preparation_us = self
                .totals
                .maximum_eligible_idle_dispatch_preparation_us
                .max(elapsed);
        }
    }

    /// Include an open interval in an end-of-run export without double counting.
    pub fn snapshot(&self, now: Instant) -> BoundarySchedulingSnapshot {
        let mut result = self.totals.clone();
        if let Some(start) = self.eligible_idle_start {
            let elapsed = micros(now.saturating_duration_since(start));
            result.eligible_idle_dispatch_preparation_us = result
                .eligible_idle_dispatch_preparation_us
                .saturating_add(elapsed);
            result.maximum_eligible_idle_dispatch_preparation_us = result
                .maximum_eligible_idle_dispatch_preparation_us
                .max(elapsed);
        }
        result
    }

    pub fn consume_prepared(&mut self, prepared: Instant, consumed: Instant) {
        let elapsed = micros(consumed.saturating_duration_since(prepared));
        self.totals.completion_drains = self.totals.completion_drains.saturating_add(1);
        self.totals.prepared_to_main_consume_us = self
            .totals
            .prepared_to_main_consume_us
            .saturating_add(elapsed);
        self.totals.maximum_prepared_to_main_consume_us =
            self.totals.maximum_prepared_to_main_consume_us.max(elapsed);
    }
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_and_worker_state_close_eligible_intervals() {
        let start = Instant::now();
        let mut metrics = BoundarySchedulingMetrics::default();
        metrics.observe_eligible_idle(start, true, false);
        assert_eq!(
            metrics
                .snapshot(start + Duration::from_secs(1))
                .eligible_idle_dispatch_preparation_us,
            0
        );
        metrics.observe_eligible_idle(start, true, true);
        metrics.observe_eligible_idle(start + Duration::from_micros(10), false, true);
        assert_eq!(
            metrics
                .snapshot(start + Duration::from_secs(1))
                .eligible_idle_dispatch_preparation_us,
            10
        );
    }

    #[test]
    fn final_snapshot_includes_open_interval_without_mutation_or_double_counting() {
        let start = Instant::now();
        let end = start + Duration::from_micros(30);
        let mut metrics = BoundarySchedulingMetrics::default();
        metrics.observe_eligible_idle(start, true, true);
        assert_eq!(
            metrics.snapshot(end).eligible_idle_dispatch_preparation_us,
            30
        );
        assert_eq!(
            metrics.snapshot(end).eligible_idle_dispatch_preparation_us,
            30
        );
        metrics.close(end);
        assert_eq!(
            metrics.snapshot(end).eligible_idle_dispatch_preparation_us,
            30
        );
        metrics.consume_prepared(start, end);
        assert_eq!(metrics.snapshot(end).prepared_to_main_consume_us, 30);
    }
}
