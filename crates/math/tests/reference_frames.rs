use glam::DVec3;
use mundaris_math::*;
use std::num::NonZeroU64;

fn tree(namespace: u64) -> FrameTree {
    FrameTree::new(NonZeroU64::new(namespace).unwrap())
}
fn state(t: DVec3, axis: DVec3, angle: f64) -> FrameState {
    FrameState::stationary(RigidTransform::new(
        Displacement3::try_metres(t).unwrap(),
        UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), angle).unwrap(),
    ))
}
fn point(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}
fn close(actual: DVec3, expected: DVec3, tolerance: f64) {
    assert!(actual.is_finite());
    assert!(
        (actual - expected).length() <= tolerance,
        "{actual:?} != {expected:?}"
    );
}

#[test]
fn analytic_sibling_ancestor_and_same_frame() {
    let mut tree = tree(1);
    let root = tree.root();
    let a = tree
        .insert(
            root,
            state(
                DVec3::new(10.0, 20.0, 30.0),
                DVec3::Z,
                std::f64::consts::FRAC_PI_2,
            ),
        )
        .unwrap();
    let b = tree
        .insert(
            root,
            state(
                DVec3::new(10.0, 22.0, 30.0),
                DVec3::Y,
                std::f64::consts::FRAC_PI_2,
            ),
        )
        .unwrap();
    let evaluation = tree.evaluate();
    close(
        evaluation
            .convert_position(point(a, DVec3::X), b)
            .unwrap()
            .local()
            .metres(),
        DVec3::new(0.0, -1.0, 0.0),
        1e-9,
    );
    close(
        evaluation
            .convert_position(point(a, DVec3::X), root)
            .unwrap()
            .local()
            .metres(),
        DVec3::new(10.0, 21.0, 30.0),
        1e-9,
    );
    close(
        evaluation
            .convert_position(point(root, DVec3::new(10.0, 21.0, 30.0)), a)
            .unwrap()
            .local()
            .metres(),
        DVec3::X,
        1e-9,
    );
    assert_eq!(
        evaluation.convert_position(point(a, DVec3::X), a).unwrap(),
        point(a, DVec3::X)
    );
    let displacement = FrameDisplacement::new(a, Displacement3::try_metres(DVec3::X).unwrap());
    close(
        evaluation
            .convert_displacement(displacement, b)
            .unwrap()
            .local()
            .metres(),
        DVec3::Y,
        1e-12,
    );
    assert!(
        point(a, DVec3::X)
            .displaced(FrameDisplacement::new(b, Displacement3::zero()))
            .is_err()
    );
    assert!(
        point(a, DVec3::X)
            .displacement_from(point(b, DVec3::X))
            .is_err()
    );
    assert!(
        evaluation
            .prepare_conversion(a, b)
            .unwrap()
            .convert_position(point(b, DVec3::X))
            .is_err()
    );
}

#[test]
fn transactional_edits_and_reparenting_descendants() {
    let mut tree = tree(1);
    let root = tree.root();
    let original = state(DVec3::X, DVec3::Y, 0.0);
    let a = tree.insert(root, original).unwrap();
    let b = tree.insert(a, original).unwrap();
    let c = tree
        .insert(root, state(DVec3::new(10.0, 0.0, 0.0), DVec3::Y, 0.0))
        .unwrap();
    let wrong = self::tree(2).root();
    let revision = tree.evaluate().revision();
    for updates in [
        vec![(root, original)],
        vec![(wrong, original)],
        vec![(b, original), (a, original)],
        vec![(a, original), (a, original)],
    ] {
        assert!(tree.update_states(5.0, &updates).is_err());
        assert_eq!(tree.evaluate().revision(), revision);
        assert_eq!(tree.evaluate().sample_time_s(), 0.0);
        assert_eq!(*tree.evaluate().state(a).unwrap(), original);
    }
    for invalid_time in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(tree.update_states(invalid_time, &[]).is_err());
    }
    for (frame, parent) in [(root, a), (a, a), (a, b), (a, wrong)] {
        assert!(
            tree.reparent_with_local_state(frame, parent, original)
                .is_err()
        );
        assert_eq!(tree.evaluate().revision(), revision);
        assert_eq!(tree.evaluate().parent(a).unwrap(), Some(root));
        assert_eq!(tree.evaluate().depth(b).unwrap(), 2);
    }
    assert_eq!(tree.evaluate().parent(root).unwrap(), None);
    tree.reparent_with_local_state(a, c, state(DVec3::new(2.0, 0.0, 0.0), DVec3::Y, 0.0))
        .unwrap();
    assert_eq!(tree.evaluate().parent(a).unwrap(), Some(c));
    assert_eq!(tree.evaluate().depth(b).unwrap(), 3);
    assert_eq!(*tree.evaluate().state(b).unwrap(), original);
    close(
        tree.evaluate()
            .convert_position(point(b, DVec3::ZERO), root)
            .unwrap()
            .local()
            .metres(),
        DVec3::new(13.0, 0.0, 0.0),
        1e-9,
    );
    tree.update_states(-2.0, &[]).unwrap();
    assert_eq!(tree.evaluate().sample_time_s(), -2.0);
}

#[test]
fn bounded_depth_and_noncommuting_property_sweep() {
    for depth in [1, 4, 8] {
        for sign in [-1.0, 0.0, 1.0] {
            let mut tree = tree(1);
            let root = tree.root();
            let mut a = root;
            let mut b = root;
            for level in 0..depth {
                a = tree
                    .insert(
                        a,
                        state(
                            DVec3::new(10.0 * sign, 20.0, 0.0),
                            DVec3::new(1.0, 2.0, 3.0),
                            0.17 * (level + 1) as f64,
                        ),
                    )
                    .unwrap();
                b = tree
                    .insert(
                        b,
                        state(
                            DVec3::new(0.0, 5.0, -30.0 * sign),
                            DVec3::new(3.0, 1.0, 2.0),
                            -0.23,
                        ),
                    )
                    .unwrap();
            }
            for value in [DVec3::ZERO, DVec3::X, DVec3::new(-200.0, 30.0, 500.0)] {
                let evaluation = tree.evaluate();
                let p = point(a, value);
                let converted = evaluation.convert_position(p, b).unwrap();
                assert_eq!(
                    converted,
                    evaluation
                        .prepare_conversion(a, b)
                        .unwrap()
                        .convert_position(p)
                        .unwrap()
                );
                close(
                    evaluation
                        .convert_position(converted, a)
                        .unwrap()
                        .local()
                        .metres(),
                    value,
                    1e-9,
                );
                let direction = evaluation
                    .convert_direction(
                        FrameDirection::new(
                            a,
                            Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
                        ),
                        b,
                    )
                    .unwrap();
                assert!((direction.local().unit().length() - 1.0).abs() <= 1e-12);
                assert_eq!(converted, evaluation.convert_position(p, b).unwrap());
            }
        }
    }
}
