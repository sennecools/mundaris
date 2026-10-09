use astrum_app::{interactive_clock::*, playback_metrics::*};
use std::time::Duration;
#[test]
fn lifecycle_gaps_boundary_and_once_only_capture() {
    let mut clock = InteractiveClock::default();
    assert_eq!(
        clock.classify(Duration::from_secs(36000)),
        ClockInterval::Accepted(Duration::ZERO)
    );
    assert_eq!(
        clock.classify(Duration::from_millis(250)),
        ClockInterval::Accepted(Duration::from_millis(250))
    );
    assert_eq!(
        clock.classify(Duration::from_secs(3)),
        ClockInterval::Discontinuity(Duration::from_secs(3))
    );
    assert_eq!(
        clock.classify(Duration::ZERO),
        ClockInterval::Accepted(Duration::ZERO)
    );
    clock.set_drawable(false);
    clock.set_drawable(false);
    assert_eq!(
        clock.classify(Duration::from_secs(36000)),
        ClockInterval::Hidden
    );
    clock.set_drawable(true);
    assert_eq!(
        clock.classify(Duration::from_secs(36000)),
        ClockInterval::Accepted(Duration::ZERO)
    );
    assert_eq!(
        clock.classify(Duration::from_secs(36000)),
        ClockInterval::Discontinuity(Duration::from_secs(36000))
    );
}
#[test]
fn actual_commits_signed_rolling_windows_and_tick_quantization() {
    let mut metrics = PlaybackMetrics::default();
    for _ in 0..40 {
        metrics.record(Duration::from_millis(250), 0.0);
    }
    let m = metrics.measurement(60.0, 1.0).unwrap();
    assert_eq!(m.achieved_rate, 0.0);
    assert!(m.tick_limited);
    metrics.reset();
    for _ in 0..100 {
        metrics.record(Duration::from_millis(100), 1000.0);
    }
    assert!((metrics.measurement(60.0, 10000.0).unwrap().achieved_rate - 10000.0).abs() < 1e-8);
    metrics.reset();
    metrics.record(Duration::from_secs(1), -60.0);
    assert_eq!(
        metrics.measurement(60.0, -60.0).unwrap().achieved_rate,
        -60.0
    );
    metrics.reset();
    assert!(metrics.measurement(60.0, 1.0).is_none());
}
