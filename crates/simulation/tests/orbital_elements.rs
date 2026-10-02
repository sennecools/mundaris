use glam::{DQuat, DVec3};
use mundaris_simulation::*;

#[test]
fn independent_circular_eccentric_inclined_conics() {
    let mu = 6.67430e-11 * (1e24 + 1e20);
    let a = 1e7;
    for e in [0.0, 0.3] {
        let rotation = DQuat::from_rotation_x(0.7) * DQuat::from_rotation_z(0.4);
        let r = rotation * DVec3::X * (a * (1.0 - e));
        let v = rotation * DVec3::Y * (mu * (1.0 + e) / (a * (1.0 - e))).sqrt();
        let elements = osculating_elements(r, v, 1e24, 1e20).unwrap();
        assert_eq!(elements.class(), ConicClass::Elliptic);
        assert!((elements.semi_major_axis_m().unwrap() / a - 1.0).abs() < 1e-12);
        assert!((elements.eccentricity() - e).abs() < 1e-12);
        assert!((elements.semilatus_rectum_m() / (a * (1.0 - e * e)) - 1.0).abs() < 1e-12);
        assert!((elements.normal() - rotation * DVec3::Z).length() < 1e-12);
        assert!((elements.elliptic_position_m(0.0).unwrap() - r).length() < 1e-7);
        assert!(
            (elements
                .elliptic_position_m(std::f64::consts::PI)
                .unwrap()
                .length()
                / (a * (1.0 + e))
                - 1.0)
                .abs()
                < 1e-12
        );
        assert_eq!(elements.periapsis_defined(), e != 0.0);
        let [low, high] = elements.elliptic_bounds_m().unwrap();
        for i in 0..1000 {
            let p = elements
                .elliptic_position_m(i as f64 * std::f64::consts::TAU / 1000.0)
                .unwrap();
            assert!((p - low).min_element() >= -1e-7 && (high - p).min_element() >= -1e-7);
        }
    }
}

#[test]
fn unavailable_states_never_evaluate_an_ellipse() {
    let circular = (6.67430e-11 * (1e24 + 1e20) / 1e7_f64).sqrt();
    for (factor, class) in [
        (2.0_f64.sqrt(), ConicClass::NearParabolic),
        (2.0, ConicClass::Hyperbolic),
    ] {
        let e = osculating_elements(DVec3::X * 1e7, DVec3::Y * (circular * factor), 1e24, 1e20)
            .unwrap();
        assert_eq!(e.class(), class);
        assert!(e.elliptic_position_m(0.0).is_err());
        assert!(e.elliptic_bounds_m().is_err());
    }
    assert_eq!(
        osculating_elements(DVec3::X * 1e7, DVec3::X * circular, 1e24, 1e20)
            .unwrap()
            .class(),
        ConicClass::Degenerate
    );
    for r in [DVec3::ZERO, DVec3::splat(f64::MAX), DVec3::splat(f64::NAN)] {
        assert!(osculating_elements(r, DVec3::Y * circular, 1e24, 1e20).is_err());
    }
}
