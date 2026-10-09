use glam::DVec3;
use astrum_math::*;
use std::num::NonZeroU64;

fn point(frame: FrameId, p: DVec3, v: DVec3) -> KinematicPoint {
    KinematicPoint::try_new(
        FramePosition::new(frame, LocalPosition::try_metres(p).unwrap()),
        FrameVelocity::new(frame, LinearVelocity3::try_metres_per_second(v).unwrap()),
    )
    .unwrap()
}
fn state(t: DVec3, axis: DVec3, angle: f64, u: DVec3, omega: DVec3) -> FrameState {
    FrameState::new(
        RigidTransform::new(
            Displacement3::try_metres(t).unwrap(),
            UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), angle).unwrap(),
        ),
        Some(FrameMotion::new(
            LinearVelocity3::try_metres_per_second(u).unwrap(),
            AngularVelocity3::try_radians_per_second(omega).unwrap(),
        )),
    )
}
fn close(a: DVec3, b: DVec3, tolerance: f64) {
    assert!(a.is_finite());
    assert!((a - b).length() <= tolerance, "{a:?} != {b:?}");
}

#[test]
fn analytic_translation_spin_and_nested_nonparallel_motion() {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let translated = tree
        .insert(
            root,
            state(
                DVec3::X,
                DVec3::Z,
                0.0,
                DVec3::new(30_000.0, 0.0, 0.0),
                DVec3::ZERO,
            ),
        )
        .unwrap();
    let p = point(translated, DVec3::new(3.0, 0.0, 0.0), DVec3::ZERO);
    let converted = tree.evaluate().convert_kinematic_point(p, root).unwrap();
    close(
        converted.velocity().relative().metres_per_second(),
        DVec3::new(30_000.0, 0.0, 0.0),
        1e-9,
    );
    close(
        tree.evaluate()
            .convert_kinematic_point(converted, translated)
            .unwrap()
            .velocity()
            .relative()
            .metres_per_second(),
        DVec3::ZERO,
        1e-9,
    );
    let spinning = tree
        .insert(
            root,
            state(
                DVec3::ZERO,
                DVec3::Z,
                0.0,
                DVec3::ZERO,
                DVec3::new(0.0, 0.0, 2.0),
            ),
        )
        .unwrap();
    close(
        tree.evaluate()
            .convert_kinematic_point(
                point(spinning, DVec3::new(3.0, 0.0, 0.0), DVec3::ZERO),
                root,
            )
            .unwrap()
            .velocity()
            .relative()
            .metres_per_second(),
        DVec3::new(0.0, 6.0, 0.0),
        1e-9,
    );
    let outer = tree
        .insert(
            root,
            state(
                DVec3::new(10.0, 20.0, 30.0),
                DVec3::Z,
                std::f64::consts::FRAC_PI_2,
                DVec3::new(1.0, 2.0, 3.0),
                DVec3::new(0.0, 0.0, 2.0),
            ),
        )
        .unwrap();
    let inner = tree
        .insert(
            outer,
            state(
                DVec3::new(2.0, 0.0, 0.0),
                DVec3::Y,
                0.0,
                DVec3::new(4.0, 0.0, 0.0),
                DVec3::new(0.0, 3.0, 0.0),
            ),
        )
        .unwrap();
    // Composed origin=(10,22,30), u=(-3,6,3), omega=(-3,0,2).
    // R*(1,0,0)=(0,1,0), omega cross lever=(-2,0,-3), R*v=(-2,0,0).
    let local = point(inner, DVec3::X, DVec3::new(0.0, 2.0, 0.0));
    let global = tree
        .evaluate()
        .convert_kinematic_point(local, root)
        .unwrap();
    close(
        global.position().local().metres(),
        DVec3::new(10.0, 23.0, 30.0),
        1e-9,
    );
    close(
        global.velocity().relative().metres_per_second(),
        DVec3::new(-7.0, 6.0, 0.0),
        1e-8,
    );
    let recovered = tree
        .evaluate()
        .convert_kinematic_point(global, inner)
        .unwrap();
    close(recovered.position().local().metres(), DVec3::X, 1e-9);
    close(
        recovered.velocity().relative().metres_per_second(),
        DVec3::new(0.0, 2.0, 0.0),
        1e-8,
    );
}

#[test]
fn local_analytic_derivative_matches_central_difference() {
    let sample = |time: f64| {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let child = tree
            .insert(
                root,
                state(
                    DVec3::new(2.0 * time, time.sin(), 0.0),
                    DVec3::Z,
                    0.7 * time,
                    DVec3::new(2.0, time.cos(), 0.0),
                    DVec3::new(0.0, 0.0, 0.7),
                ),
            )
            .unwrap();
        tree.evaluate()
            .convert_kinematic_point(
                point(child, DVec3::new(3.0 + time, 2.0, 0.0), DVec3::X),
                root,
            )
            .unwrap()
    };
    let time = 1.2;
    let h = 1e-4;
    let difference = (sample(time + h).position().local().metres()
        - sample(time - h).position().local().metres())
        / (2.0 * h);
    close(
        sample(time).velocity().relative().metres_per_second(),
        difference,
        1e-6,
    );
}

#[test]
fn unknown_motion_shared_ancestry_and_frame_mismatch() {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let unknown = tree
        .insert(root, FrameState::new(RigidTransform::identity(), None))
        .unwrap();
    let a = tree
        .insert(unknown, FrameState::stationary(RigidTransform::identity()))
        .unwrap();
    let b = tree
        .insert(unknown, FrameState::stationary(RigidTransform::identity()))
        .unwrap();
    let p = point(a, DVec3::X, DVec3::ZERO);
    assert!(tree.evaluate().convert_kinematic_point(p, b).is_ok());
    assert_eq!(
        tree.evaluate().convert_kinematic_point(p, root),
        Err(FrameError::MissingMotion(unknown))
    );
    assert!(tree.evaluate().convert_position(p.position(), root).is_ok());
    let p = point(unknown, DVec3::X, DVec3::ZERO);
    assert_eq!(
        tree.evaluate().convert_kinematic_point(p, unknown).unwrap(),
        p
    );
    assert!(
        KinematicPoint::try_new(
            p.position(),
            FrameVelocity::new(root, LinearVelocity3::zero())
        )
        .is_err()
    );
}

#[test]
fn astronomical_pose_velocity_handoff_and_common_motion_cancellation() {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let anchor = tree
        .insert(
            root,
            state(
                DVec3::new(1.5e11, 0.0, 0.0),
                DVec3::Y,
                0.0,
                DVec3::new(30_000.0, 20_000.0, 0.0),
                DVec3::ZERO,
            ),
        )
        .unwrap();
    let spin = tree
        .insert(
            anchor,
            state(
                DVec3::ZERO,
                DVec3::Y,
                0.73,
                DVec3::ZERO,
                DVec3::new(0.0, 0.2, 0.0),
            ),
        )
        .unwrap();
    let regional = tree
        .insert(
            spin,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(0.0, 6_371_000.0, 0.0)).unwrap(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    let local = point(
        regional,
        DVec3::new(0.01, 1.7, 0.0),
        DVec3::new(0.0, 0.0, -3.0),
    );
    let pose = FramePose::new(
        local.position(),
        UnitRotation::from_axis_angle(Direction3::try_new(DVec3::X).unwrap(), 0.3).unwrap(),
    );
    let evaluation = tree.evaluate();
    for target in [spin, root] {
        let target_pose = evaluation.reexpress_pose(pose, target).unwrap();
        let returned = evaluation.reexpress_pose(target_pose, regional).unwrap();
        let target_point = evaluation.convert_kinematic_point(local, target).unwrap();
        let returned_point = evaluation
            .convert_kinematic_point(target_point, regional)
            .unwrap();
        let (position_budget, velocity_budget) = if target == root {
            (1e-3, 5e-4)
        } else {
            (1e-7, 1e-6)
        };
        close(
            returned.position().local().metres(),
            local.position().local().metres(),
            position_budget,
        );
        close(
            returned_point.velocity().relative().metres_per_second(),
            local.velocity().relative().metres_per_second(),
            velocity_budget,
        );
        for basis in [DVec3::X, DVec3::Y, DVec3::Z] {
            close(
                returned
                    .orientation()
                    .rotate_direction(Direction3::try_new(basis).unwrap())
                    .unwrap()
                    .unit(),
                pose.orientation()
                    .rotate_direction(Direction3::try_new(basis).unwrap())
                    .unwrap()
                    .unit(),
                1e-12,
            );
        }
    }
    let before = evaluation.convert_kinematic_point(local, spin).unwrap();
    tree.update_states(
        1.0,
        &[(
            anchor,
            state(
                DVec3::new(1e16, 0.0, 0.0),
                DVec3::Y,
                1.0,
                DVec3::new(90_000.0, 0.0, 0.0),
                DVec3::Y,
            ),
        )],
    )
    .unwrap();
    assert_eq!(
        tree.evaluate()
            .convert_kinematic_point(local, spin)
            .unwrap(),
        before
    );
}

#[test]
fn body_scale_motion_round_trip_and_finite_derivative_overflow() {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let body = tree
        .insert(
            root,
            state(
                DVec3::new(1e6, 2e6, -3e6),
                DVec3::new(1.0, 2.0, 3.0),
                0.7,
                DVec3::new(30_000.0, 20_000.0, 0.0),
                DVec3::new(0.1, 0.2, 0.3),
            ),
        )
        .unwrap();
    let local = point(
        body,
        DVec3::new(0.01, 6_371_001.7, -30.0),
        DVec3::new(0.1, 0.2, 0.3),
    );
    let converted = tree
        .evaluate()
        .convert_kinematic_point(local, root)
        .unwrap();
    let returned = tree
        .evaluate()
        .convert_kinematic_point(converted, body)
        .unwrap();
    close(
        returned.position().local().metres(),
        local.position().local().metres(),
        1e-7,
    );
    close(
        returned.velocity().relative().metres_per_second(),
        local.velocity().relative().metres_per_second(),
        1e-6,
    );
    tree.update_states(
        1.0,
        &[(
            body,
            state(
                DVec3::ZERO,
                DVec3::Y,
                0.0,
                DVec3::ZERO,
                DVec3::new(0.0, 0.0, f64::MAX),
            ),
        )],
    )
    .unwrap();
    assert!(matches!(
        tree.evaluate()
            .convert_kinematic_point(point(body, DVec3::new(3.0, 0.0, 0.0), DVec3::ZERO), root),
        Err(FrameError::Math(MathError::ArithmeticOverflow))
    ));
}
