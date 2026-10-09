use glam::DVec3;
use astrum_math::*;
use std::num::NonZeroU64;

fn point(frame: FrameId, value: DVec3) -> FramePosition {
    FramePosition::new(frame, LocalPosition::try_metres(value).unwrap())
}
fn state(t: DVec3, angle: f64) -> FrameState {
    FrameState::stationary(RigidTransform::new(
        Displacement3::try_metres(t).unwrap(),
        UnitRotation::from_axis_angle(
            Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
            angle,
        )
        .unwrap(),
    ))
}
fn error(a: DVec3, b: DVec3) -> f64 {
    assert!(a.is_finite() && b.is_finite());
    (a - b).length()
}

#[test]
fn shared_extreme_ancestry_is_excluded_and_flattening_loses_detail() {
    for offset in [0.0, 1.5e11, 1e16] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let common = tree
            .insert(root, state(DVec3::new(offset, 0.0, 0.0), 0.0))
            .unwrap();
        let a = tree
            .insert(common, state(DVec3::new(10.0, 20.0, 30.0), 0.7))
            .unwrap();
        let b = tree
            .insert(common, state(DVec3::new(-30.0, 10.0, 4.0), -0.2))
            .unwrap();
        let source = point(a, DVec3::new(0.001, 1.7, -5.0));
        let expected = tree.evaluate().convert_position(source, b).unwrap();
        for angle in [0.0, 0.3, 1.9, -2.0] {
            tree.update_states(
                angle,
                &[(
                    common,
                    state(DVec3::new(offset, 1000.0 * angle, 0.0), angle),
                )],
            )
            .unwrap();
            let actual = tree.evaluate().convert_position(source, b).unwrap();
            assert!(error(actual.local().metres(), expected.local().metres()) <= 1e-9);
            assert_eq!(
                *tree.evaluate().state(a).unwrap(),
                state(DVec3::new(10.0, 20.0, 30.0), 0.7)
            );
        }
    }
    assert_eq!(1e16 + 0.001, 1e16);
}

#[test]
fn body_scale_and_root_round_trip_envelopes() {
    for offset in [1.5e11, 1e16] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let body = tree
            .insert(root, state(DVec3::new(offset, 0.0, 0.0), 0.7))
            .unwrap();
        let regional = tree
            .insert(body, state(DVec3::new(0.0, 6_371_000.0, 0.0), -0.4))
            .unwrap();
        for local in [
            DVec3::new(0.001, 1.7, -5.0),
            DVec3::new(20.0, 5.0, -30.0),
            DVec3::new(0.0, 100.0, -3000.0),
        ] {
            let p = point(regional, local);
            let evaluation = tree.evaluate();
            let in_body = evaluation.convert_position(p, body).unwrap();
            assert!(
                error(
                    evaluation
                        .convert_position(in_body, regional)
                        .unwrap()
                        .local()
                        .metres(),
                    local
                ) <= 1e-7
            );
            let in_root = evaluation.convert_position(p, root).unwrap();
            let residual = error(
                evaluation
                    .convert_position(in_root, regional)
                    .unwrap()
                    .local()
                    .metres(),
                local,
            );
            if offset == 1.5e11 {
                assert!(residual <= 1e-3, "root residual {residual}");
            } else if local.x == 0.001 {
                assert!(residual > 1e-3);
            }
        }
    }
}

#[test]
fn astronomical_root_delta_retains_only_source_precision() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let origin = point(root, DVec3::new(1.5e11, 0.0, 0.0));
    for delta in [1.7, 20.0, 0.01] {
        let rounded = point(root, DVec3::new(1.5e11 + delta, 0.0, 0.0));
        let actual = rounded.displacement_from(origin).unwrap().local().metres();
        assert!(error(actual, DVec3::new(delta, 0.0, 0.0)) <= 1e-3);
        if delta == 0.01 {
            assert_ne!(actual.x, delta);
        }
    }
}
