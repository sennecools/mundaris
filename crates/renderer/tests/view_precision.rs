use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::*;
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
fn close(a: DVec3, b: DVec3, t: f64) {
    assert!(a.is_finite());
    assert!((a - b).length() <= t, "{a:?} != {b:?}");
}

#[test]
fn observer_centering_camera_axes_and_batch_failures() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let observer = point(root, DVec3::new(20.0, 1.7, 30.0));
    for yaw in [0.0, std::f64::consts::FRAC_PI_2] {
        let orientation =
            UnitRotation::from_axis_angle(Direction3::try_new(DVec3::Y).unwrap(), yaw).unwrap();
        let pose = FramePose::new(observer, orientation);
        let view =
            PreparedView::new(&tree.evaluate(), pose, RenderPrecisionBudget::near_debug()).unwrap();
        let source = view.prepare_source(root).unwrap();
        assert_eq!(
            source.try_position(observer).unwrap().gpu_xyz(),
            [0.0, 0.0, 0.0]
        );
        let ahead = point(
            root,
            observer.local().metres() + DVec3::new(-5.0 * yaw.sin(), 0.0, -5.0 * yaw.cos()),
        );
        close(
            source.view_displacement(ahead).unwrap().metres(),
            DVec3::new(0.0, 0.0, -5.0),
            1e-9,
        );
        let first = source.try_position(ahead).unwrap();
        let mut output = [first; 2];
        source
            .write_positions(&[observer, ahead], &mut output)
            .unwrap();
        assert_eq!(output[1], first);
        assert!(source.write_positions(&[observer], &mut output).is_err());
        let other = FrameTree::new(NonZeroU64::new(2).unwrap());
        let bad = point(other.root(), DVec3::ZERO);
        assert!(matches!(
            source.write_positions(&[ahead, bad], &mut output),
            Err(RenderPreparationError::Vertex { index: 1, .. })
        ));
    }
}

#[test]
fn source_centering_before_body_rotation_and_extreme_common_ancestry() {
    for offset in [0.0, 1.5e11, 1e16] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let common = tree
            .insert(root, state(DVec3::new(offset, 0.0, 0.0), 0.0))
            .unwrap();
        let body = tree.insert(common, state(DVec3::ZERO, 0.71)).unwrap();
        let regional = tree
            .insert(body, state(DVec3::new(0.0, 6_371_000.0, 0.0), 0.0))
            .unwrap();
        let observer = FramePose::new(
            point(regional, DVec3::new(0.0, 1.7, 0.0)),
            UnitRotation::identity(),
        );
        let nearby = point(regional, DVec3::new(0.001, 1.7, -5.0));
        for angle in [0.0, 0.7, 1.4] {
            tree.update_states(
                angle,
                &[(
                    common,
                    state(DVec3::new(offset, 20_000.0 * angle, 0.0), angle),
                )],
            )
            .unwrap();
            let view = PreparedView::new(
                &tree.evaluate(),
                observer,
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let source = view.prepare_source(regional).unwrap();
            close(
                source.view_displacement(nearby).unwrap().metres(),
                DVec3::new(0.001, 0.0, -5.0),
                1e-9,
            );
            assert!(source.try_position(nearby).unwrap().max_component_error_m() <= 1e-5);
            // Observer is represented in body coordinates: source subtraction still
            // precedes the nontrivial camera rotation, retaining small body deltas.
            let evaluation = tree.evaluate();
            let in_body = evaluation.reexpress_pose(observer, body).unwrap();
            let body_view =
                PreparedView::new(&evaluation, in_body, RenderPrecisionBudget::near_debug())
                    .unwrap();
            close(
                body_view
                    .prepare_source(regional)
                    .unwrap()
                    .view_displacement(nearby)
                    .unwrap()
                    .metres(),
                DVec3::new(0.001, 0.0, -5.0),
                1e-7,
            );
        }
    }
}

#[test]
fn million_metre_source_points_are_subtracted_before_camera_rotation() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let origin = DVec3::new(6_371_000.0, 6_371_000.0, -6_371_000.0);
    let camera = UnitRotation::from_axis_angle(
        Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
        0.91,
    )
    .unwrap();
    let observer = FramePose::new(point(root, origin), camera);
    let view = PreparedView::new(
        &tree.evaluate(),
        observer,
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let source = view.prepare_source(root).unwrap();
    for delta in [DVec3::new(0.001, 0.01, -5.0), DVec3::new(20.0, 3.3, -30.0)] {
        let actual = source
            .view_displacement(point(root, origin + delta))
            .unwrap()
            .metres();
        let analytic = camera.quaternion().conjugate() * delta;
        close(actual, analytic, 1e-9);
    }
}
