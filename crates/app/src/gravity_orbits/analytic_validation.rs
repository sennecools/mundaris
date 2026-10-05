//! Opt-in native evidence route through ordinary session, focus and time commands.
use super::*;

/// Startup handshake for the opt-in native runner, never ordinary navigation.
pub(super) struct Route {
    elapsed: Duration,
    next: usize,
    ready_path: Option<std::path::PathBuf>,
    started: bool,
}

impl Route {
    pub(super) fn new(ready_path: Option<std::path::PathBuf>) -> Self {
        Self {
            elapsed: Duration::ZERO,
            next: 0,
            started: ready_path.is_none(),
            ready_path,
        }
    }

    fn start_if_ready(&mut self, marker_present: bool, focused: bool) -> bool {
        if !self.started && marker_present && focused {
            self.started = true;
            tracing::info!("analytic native route startup readiness accepted");
        }
        self.started
    }
}

pub(super) fn advance(demo: &mut GravityOrbitsDemo, elapsed: Duration) {
    if demo.hidden || elapsed > demo.clock.threshold() {
        return;
    }
    let Some(route) = &mut demo.analytic_validation else {
        return;
    };
    if !route.started {
        let marker_present = route.ready_path.as_ref().is_some_and(|p| p.is_file());
        let focused = demo.controls.input_focused
            && demo.controls.viewport_ready
            && demo
                .controls
                .ui_context
                .as_ref()
                .is_some_and(|c| c.input(|i| i.focused));
        if !route.start_if_ready(marker_present, focused) {
            return;
        }
    }
    let (at, next) = (&mut route.elapsed, &mut route.next);
    *at = at.saturating_add(elapsed);
    let thresholds = [1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
    if *next >= thresholds.len() || at.as_secs_f64() < thresholds[*next] {
        return;
    }
    let commands = match *next {
        0 => vec![
            Command::Pause(true),
            Command::Focus {
                fixed: false,
                fit: true,
            },
        ],
        1 => vec![Command::Rate(1000.0), Command::Pause(false)],
        2 => vec![Command::Rate(-1000.0)],
        3 => vec![Command::SeekSeconds(31_557_600_000.25)],
        4 => vec![
            Command::SurfaceInspection,
            Command::Rate(1000.0),
            Command::Pause(false),
        ],
        5 => vec![
            Command::Pause(true),
            Command::Select(demo.ids[4]),
            Command::Focus {
                fixed: false,
                fit: true,
            },
        ],
        6 => vec![
            Command::SurfaceInspection,
            Command::SeekSeconds(-31_557_600_000.25),
        ],
        7 => vec![Command::Reset],
        _ => unreachable!("bounded route stage"),
    };
    tracing::info!(
        stage = *next,
        elapsed_s = at.as_secs_f64(),
        "analytic native route commands queued (scripted, not human acceptance)"
    );
    *next += 1;
    demo.controls.pending.extend(commands);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_both_marker_and_native_focus() {
        let mut route = Route::new(Some("runner-ready".into()));
        assert!(!route.start_if_ready(false, false));
        assert!(!route.start_if_ready(true, false));
        assert!(!route.start_if_ready(false, true));
        assert_eq!(route.elapsed, Duration::ZERO);
        assert_eq!(route.next, 0);
        assert!(route.start_if_ready(true, true));
        // Only startup is gated. Later focus loss retains ordinary cancellation
        // behavior; it cannot pause the route to manufacture missed checkpoints.
        assert!(route.start_if_ready(false, false));
    }

    #[test]
    fn legacy_route_starts_without_handshake() {
        let mut route = Route::new(None);
        assert!(route.start_if_ready(false, false));
        assert_eq!(route.elapsed, Duration::ZERO);
        assert_eq!(route.next, 0);
    }
}
