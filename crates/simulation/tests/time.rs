use mundaris_simulation::*;
use std::time::Duration;

#[test]
fn rates_pause_resume_seek_and_reset_are_explicit() {
    for rate in [1.0, 10.0, 0.1, -10.0, 0.0] {
        let mut clock = TimeController::new(SimulationInstant::ZERO);
        clock.set_rate(PlaybackRate::try_multiplier(rate).unwrap());
        assert_eq!(clock.requested_time(), SimulationInstant::ZERO);
        assert_eq!(
            clock
                .advance_wall_time(Duration::from_secs(2))
                .unwrap()
                .seconds_since_epoch(),
            2.0 * rate
        );
        clock.set_paused(true);
        let frozen = clock.requested_time();
        assert_eq!(
            clock.advance_wall_time(Duration::from_secs(20)).unwrap(),
            frozen
        );
        clock.set_paused(false);
        assert_eq!(
            clock
                .advance_wall_time(Duration::from_secs(1))
                .unwrap()
                .seconds_since_epoch(),
            3.0 * rate
        );
        clock.seek(SimulationInstant::try_seconds_since_epoch(-5.0).unwrap());
        assert_eq!(clock.requested_time().seconds_since_epoch(), -5.0);
        clock.seek(SimulationInstant::ZERO);
        assert_eq!(clock.requested_time(), SimulationInstant::ZERO);
    }
}

#[test]
fn overflow_and_invalid_rates_are_transactional() {
    for rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            PlaybackRate::try_multiplier(rate),
            Err(TimeError::NonFiniteRate)
        );
    }
    let mut clock = TimeController::new(SimulationInstant::ZERO);
    clock.set_rate(PlaybackRate::try_multiplier(f64::MAX).unwrap());
    assert!(clock.advance_wall_time(Duration::from_secs(2)).is_err());
    assert_eq!(clock.requested_time(), SimulationInstant::ZERO);
    clock.seek(SimulationInstant::try_seconds_since_epoch(f64::MAX).unwrap());
    assert!(clock.advance_wall_time(Duration::from_secs(1)).is_err());
    assert_eq!(clock.requested_time().seconds_since_epoch(), f64::MAX);
}
