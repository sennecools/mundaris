use glam::{DQuat, DVec3};
use mundaris_math::*;

fn position(value: DVec3) -> LocalPosition {
    LocalPosition::try_metres(value).unwrap()
}
fn transform(translation: DVec3, axis: DVec3, angle: f64) -> RigidTransform {
    RigidTransform::new(
        Displacement3::try_metres(translation).unwrap(),
        UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), angle).unwrap(),
    )
}
fn close(actual: DVec3, expected: DVec3, tolerance: f64) {
    assert!(actual.is_finite());
    assert!(
        (actual - expected).length() <= tolerance,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn analytic_composition_order_and_axes() {
    let quarter = std::f64::consts::FRAC_PI_2;
    let parent = transform(DVec3::new(10.0, 20.0, 30.0), DVec3::Z, quarter);
    let child = transform(DVec3::new(2.0, 0.0, 0.0), DVec3::Y, quarter);
    let combined = parent.compose(child).unwrap();
    close(
        combined
            .transform_position(position(DVec3::X))
            .unwrap()
            .metres(),
        DVec3::new(10.0, 22.0, 29.0),
        1e-12,
    );
    close(
        parent
            .rotation()
            .rotate_direction(Direction3::try_new(DVec3::X).unwrap())
            .unwrap()
            .unit(),
        DVec3::Y,
        1e-12,
    );
    close(
        child
            .rotation()
            .rotate_direction(Direction3::try_new(DVec3::X).unwrap())
            .unwrap()
            .unit(),
        -DVec3::Z,
        1e-12,
    );
    close(
        combined
            .rotation()
            .rotate_direction(Direction3::try_new(DVec3::X).unwrap())
            .unwrap()
            .unit(),
        -DVec3::Z,
        1e-12,
    );
    assert!(
        (child
            .compose(parent)
            .unwrap()
            .transform_position(position(DVec3::X))
            .unwrap()
            .metres()
            - DVec3::new(10.0, 22.0, 29.0))
        .length()
            > 1.0
    );
}

#[test]
fn inverse_and_bounded_property_sweep() {
    for component in [-100.0, 0.0, 100.0] {
        for angle in [-1.2, 0.0, 0.7] {
            let t = transform(
                DVec3::new(component, 20.0, -30.0),
                DVec3::new(1.0, 2.0, 3.0),
                angle,
            );
            let inverse = t.inverse().unwrap();
            for point in [DVec3::ZERO, DVec3::X, DVec3::new(-900.0, 200.0, 100.0)] {
                close(
                    inverse
                        .transform_position(t.transform_position(position(point)).unwrap())
                        .unwrap()
                        .metres(),
                    point,
                    1e-9,
                );
                for identity in [inverse.compose(t).unwrap(), t.compose(inverse).unwrap()] {
                    close(
                        identity
                            .transform_position(position(point))
                            .unwrap()
                            .metres(),
                        point,
                        1e-9,
                    );
                    for basis in [DVec3::X, DVec3::Y, DVec3::Z] {
                        close(
                            identity
                                .rotation()
                                .rotate_direction(Direction3::try_new(basis).unwrap())
                                .unwrap()
                                .unit(),
                            basis,
                            1e-12,
                        );
                    }
                }
            }
        }
    }
    let t = transform(DVec3::X, DVec3::Z, std::f64::consts::FRAC_PI_2);
    close(t.inverse().unwrap().translation().metres(), DVec3::Y, 1e-12);
}

#[test]
fn quaternion_sign_normalization_and_long_composition() {
    let step = UnitRotation::from_axis_angle(
        Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
        0.0001,
    )
    .unwrap();
    let negative = UnitRotation::try_from_quaternion(-step.quaternion()).unwrap();
    close(
        step.rotate_direction(Direction3::try_new(DVec3::X).unwrap())
            .unwrap()
            .unit(),
        negative
            .rotate_direction(Direction3::try_new(DVec3::X).unwrap())
            .unwrap()
            .unit(),
        1e-12,
    );
    let mut accumulated = UnitRotation::identity();
    for _ in 0..100_000 {
        accumulated = accumulated.compose(step);
    }
    assert!((accumulated.quaternion().length_squared() - 1.0).abs() <= 1e-12);
    let x = accumulated
        .rotate_direction(Direction3::try_new(DVec3::X).unwrap())
        .unwrap()
        .unit();
    let y = accumulated
        .rotate_direction(Direction3::try_new(DVec3::Y).unwrap())
        .unwrap()
        .unit();
    assert!(x.dot(y).abs() <= 1e-12);
    let drift = DQuat::IDENTITY * (1.0 + 1e-12);
    assert!(
        (UnitRotation::try_from_quaternion(drift)
            .unwrap()
            .quaternion()
            .length_squared()
            - 1.0)
            .abs()
            <= 1e-12
    );
}

#[test]
fn invalid_inputs_and_finite_overflow() {
    for scalar in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for value in [
            DVec3::new(scalar, 0.0, 0.0),
            DVec3::new(0.0, scalar, 0.0),
            DVec3::new(0.0, 0.0, scalar),
        ] {
            assert_eq!(LocalPosition::try_metres(value), Err(MathError::NonFinite));
            assert_eq!(Displacement3::try_metres(value), Err(MathError::NonFinite));
            assert_eq!(Direction3::try_new(value), Err(MathError::NonFinite));
            assert_eq!(
                LinearVelocity3::try_metres_per_second(value),
                Err(MathError::NonFinite)
            );
            assert_eq!(
                AngularVelocity3::try_radians_per_second(value),
                Err(MathError::NonFinite)
            );
        }
        for component in 0..4 {
            let mut values = [0.0, 0.0, 0.0, 1.0];
            values[component] = scalar;
            assert_eq!(
                UnitRotation::try_from_quaternion(DQuat::from_array(values)),
                Err(MathError::NonFinite)
            );
        }
        assert_eq!(
            UnitRotation::from_axis_angle(Direction3::try_new(DVec3::X).unwrap(), scalar),
            Err(MathError::NonFinite)
        );
    }
    for quaternion in [
        DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0),
        DQuat::from_xyzw(0.0, 0.0, 0.0, 1e-200),
        DQuat::from_xyzw(0.0, 0.0, 0.0, 2.0),
    ] {
        assert!(matches!(
            UnitRotation::try_from_quaternion(quaternion),
            Err(MathError::InvalidQuaternionNorm(_))
        ));
    }
    assert_eq!(
        Direction3::try_new(DVec3::ZERO),
        Err(MathError::ZeroDirection)
    );
    assert_eq!(Displacement3::zero().direction(), None);
    for magnitude in [f64::MAX, f64::from_bits(1)] {
        close(
            Direction3::try_new(DVec3::splat(magnitude)).unwrap().unit(),
            DVec3::splat(1.0 / 3.0_f64.sqrt()),
            1e-12,
        );
    }
    let huge = position(DVec3::splat(f64::MAX));
    assert_eq!(
        huge.displaced(Displacement3::try_metres(DVec3::splat(f64::MAX)).unwrap()),
        Err(MathError::ArithmeticOverflow)
    );
    assert_eq!(
        huge.displacement_from(position(DVec3::splat(-f64::MAX))),
        Err(MathError::ArithmeticOverflow)
    );
    let t = transform(DVec3::splat(f64::MAX), DVec3::Y, 0.0);
    assert_eq!(t.compose(t), Err(MathError::ArithmeticOverflow));
    assert_eq!(
        t.transform_position(huge),
        Err(MathError::ArithmeticOverflow)
    );
}
