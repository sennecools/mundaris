//! Bounded continuous redraw for the bootstrap; no wakeups while non-drawable.

use std::time::{Duration, Instant};

use winit::event_loop::ControlFlow;

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

pub(crate) struct RedrawSchedule {
    next_redraw: Option<Instant>,
    uncapped: bool,
}

impl Default for RedrawSchedule {
    fn default() -> Self {
        Self {
            next_redraw: None,
            uncapped: cfg!(feature = "developer-tools")
                && std::env::var("MUNDARIS_UNCAPPED").is_ok_and(|value| value == "1"),
        }
    }
}

impl RedrawSchedule {
    pub(crate) fn update(&mut self, now: Instant, drawable: bool) -> (ControlFlow, bool) {
        if !drawable {
            self.next_redraw = None;
            return (ControlFlow::Wait, false);
        }

        if self.uncapped {
            self.next_redraw = None;
            return (ControlFlow::Poll, true);
        }

        let deadline = self.next_redraw.get_or_insert(now);
        let request_redraw = now >= *deadline;
        if request_redraw {
            // Do not catch up missed frames or move the deadline on unrelated input.
            *deadline = now + FRAME_INTERVAL;
        }
        (ControlFlow::WaitUntil(*deadline), request_redraw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_wakeups_do_not_redraw_early_or_postpone_the_deadline() {
        let mut schedule = RedrawSchedule::default();
        let start = Instant::now();
        let deadline = start + FRAME_INTERVAL;
        assert_eq!(
            schedule.update(start, true),
            (ControlFlow::WaitUntil(deadline), true)
        );
        for elapsed in [1, 5, 15] {
            assert_eq!(
                schedule.update(start + Duration::from_millis(elapsed), true),
                (ControlFlow::WaitUntil(deadline), false)
            );
        }
        assert!(schedule.update(deadline, true).1);
    }

    #[test]
    fn non_drawable_windows_wait_and_restoration_redraws_immediately() {
        let mut schedule = RedrawSchedule::default();
        let start = Instant::now();
        assert!(schedule.update(start, true).1);
        assert_eq!(schedule.update(start, false), (ControlFlow::Wait, false));
        let restored = start + Duration::from_millis(5);
        assert_eq!(
            schedule.update(restored, true),
            (ControlFlow::WaitUntil(restored + FRAME_INTERVAL), true)
        );
    }

    #[test]
    fn delayed_frames_do_not_trigger_a_catch_up_loop() {
        let mut schedule = RedrawSchedule::default();
        let start = Instant::now();
        schedule.update(start, true);
        let late = start + Duration::from_secs(2);
        assert_eq!(
            schedule.update(late, true),
            (ControlFlow::WaitUntil(late + FRAME_INTERVAL), true)
        );
        assert!(!schedule.update(late, true).1);
    }

    #[test]
    fn uncapped_measurement_still_waits_when_non_drawable() {
        let mut schedule = RedrawSchedule {
            next_redraw: None,
            uncapped: true,
        };
        let now = Instant::now();
        assert_eq!(schedule.update(now, true), (ControlFlow::Poll, true));
        assert_eq!(schedule.update(now, false), (ControlFlow::Wait, false));
        assert_eq!(schedule.update(now, true), (ControlFlow::Poll, true));
    }
}
