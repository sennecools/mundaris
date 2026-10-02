use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::{planet_surface::*, *};
use std::num::NonZeroU64;

#[test]
fn reverse_depth_actual_adjacent_values_surface_horizon_moon_star() {
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut previous = f32::MAX;
    for z in [2.0, 10.0, 100.0, 1000.0, 5000.0, 1e8, 1e9, 1.5e11] {
        let clip = projection.gpu_clip([0.0, 0.0, -z as f32]);
        let depth = clip[2] / clip[3];
        assert!(depth > 0.0 && depth < previous);
        previous = depth;
        let next = f32::from_bits(depth.to_bits() + 1);
        let quantization = (0.1 / f64::from(next) - 0.1 / f64::from(depth)).abs();
        eprintln!("depth z={z} adjacent reconstruction delta={quantization} m");
        if z <= 10.0 {
            assert!(quantization < 2e-6);
        }
        if z <= 5000.0 {
            let separation = if z <= 1000.0 { 0.01 } else { 1.0 };
            let other = projection.gpu_clip([0.0, 0.0, -(z + separation) as f32]);
            assert!(other[2] / other[3] < depth);
        }
    }
}

#[test]
fn source_centred_surface_preparation_extreme_ancestor_and_one_opaque_owner() {
    let radius = 6.4e6;
    let mut expected = None;
    for offset in [0.0, 1.5e11, 1e16] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let source = tree
            .insert(
                root,
                FrameState::stationary(RigidTransform::new(
                    Displacement3::try_metres(DVec3::X * offset).unwrap(),
                    UnitRotation::from_axis_angle(Direction3::try_new(DVec3::Y).unwrap(), 0.7)
                        .unwrap(),
                )),
            )
            .unwrap();
        let observer = FramePose::new(
            FramePosition::new(
                source,
                LocalPosition::try_metres(DVec3::Z * (radius + 2.0)).unwrap(),
            ),
            UnitRotation::identity(),
        );
        let view = PreparedView::new(
            &tree.evaluate(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let prepared = view.prepare_source(source).unwrap();
        let line = FramePosition::new(
            source,
            LocalPosition::try_metres(DVec3::new(0.01, 0.0, radius - 1.0)).unwrap(),
        );
        let delta = prepared.view_displacement(line).unwrap().metres();
        assert!((delta - DVec3::new(0.01, 0.0, -3.0)).length() <= 1e-9);
        if let Some(previous) = expected {
            assert_eq!(delta, previous);
        } else {
            expected = Some(delta);
        }
        assert!(prepared.try_position(line).unwrap().max_component_error_m() <= 1e-5);
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: source,
            reference_radius_m: radius,
            projection,
        };
        let mut session = SurfaceLodSession::default();
        let mut r = LodReport::default();
        for _ in 0..1000 {
            r = session.update(&input, &LodSettings::default()).unwrap();
            if !r.desired_estimate_incomplete {
                break;
            }
        }
        assert!(!r.desired_estimate_incomplete);
        let sphere = Icosphere::new();
        let mut staging = CelestialStaging::default();
        let body = CelestialRenderBody {
            body_fixed_frame: source,
            reference_radius_m: radius,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame
            .append_surface(
                body,
                session.active_visible(),
                session.topology(),
                SurfaceStyle::default(),
            )
            .unwrap();
        frame.append_body_observations(&[body], &[true]).unwrap();
        assert_eq!(frame.markers().len(), 1);
        assert_eq!(
            frame.markers()[0].representation,
            SphereRepresentation::Surface
        );
        assert_eq!(frame.report().triangles, 0);
        assert!(frame.report().surface.triangles + frame.report().surface.fallback_triangles > 0);
        assert!(frame.report().surface.max_projected_error_pixels <= 0.05);
        assert!(frame.report().surface.max_gpu_projection_error_pixels <= 0.05);
        assert!(frame.report().surface.draws <= 17);
        assert_eq!(tree.evaluate().revision(), 1);
    }
    let module =
        naga::front::wgsl::parse_str(include_str!("../src/shaders/planet_surface.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
