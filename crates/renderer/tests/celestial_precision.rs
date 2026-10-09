use glam::DVec3;
use astrum_math::*;
use astrum_renderer::*;
use std::num::NonZeroU64;

#[test]
fn topology_chordal_bound_reverse_depth_and_shaders() {
    let sphere = Icosphere::new();
    assert_eq!(sphere.vertices().len(), 642);
    assert_eq!(sphere.indices().len(), 3840);
    for v in sphere.vertices() {
        assert!((v.length() - 1.0).abs() <= 1e-12);
    }
    for triangle in sphere.indices().as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|i| sphere.vertices()[i as usize]);
        let normal = (b - a).cross(c - a).normalize();
        assert!(normal.dot(a) > 0.0);
        assert!(1.0 - normal.dot(a) <= 0.005);
    }
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut previous = f32::MAX;
    for (i, d) in [0.1, 1e3, 1e8, 1e12].into_iter().enumerate() {
        let clip = projection.gpu_clip([0.0, 0.0, -d]);
        assert!(clip[3] > 0.0);
        let depth = clip[2] / clip[3];
        assert!(depth > 0.0 && depth < previous);
        previous = depth;
        if i == 0 {
            assert!((depth - 1.0).abs() <= 2e-6);
        }
    }
    assert_eq!(projection.gpu_bytes().len(), 64);
    for shader in [
        concat!(
            include_str!("../src/shaders/lighting.wgsl"),
            include_str!("../src/shaders/celestial.wgsl")
        ),
        include_str!("../src/shaders/celestial_lines.wgsl"),
    ] {
        let module = naga::front::wgsl::parse_str(shader).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap();
    }
}

#[test]
fn observer_relative_sphere_precision_markers_and_poison() {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let frame = tree
        .insert(
            root,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::X * 1.5e11).unwrap(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    let sphere = Icosphere::new();
    let mut storage = CelestialStaging::default();
    for (width, height) in [(1280, 800), (3840, 2160)] {
        for (camera_frame, position, radius) in [
            (frame, DVec3::Z * 4e6, 1e6),
            (root, DVec3::new(1.5e11, 0.0, 4e11), 6.957e8),
        ] {
            let observer = FramePose::new(
                FramePosition::new(camera_frame, LocalPosition::try_metres(position).unwrap()),
                UnitRotation::identity(),
            );
            let view = PreparedView::new(
                &tree.evaluate(),
                observer,
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let projection =
                CelestialProjection::try_new(width, height, 60.0_f64.to_radians(), 0.1).unwrap();
            let mut output = CelestialFrame::new(&view, &mut storage, projection, &sphere);
            let body = CelestialRenderBody {
                body_fixed_frame: frame,
                reference_radius_m: radius,
                color: [0.2, 0.6, 1.0, 1.0],
                unlit: false,
                selected: true,
                material: Default::default(),
                emission_nits: 0.0,
            };
            output.append_bodies(&[body]).unwrap();
            assert_eq!(output.report().triangles, 1280);
            assert!(output.report().max_projected_error_pixels <= 0.05);
            assert!(output.report().max_narrowing_error_m <= 0.001 * radius);
            assert_eq!(
                output.markers()[0].screen_pixels,
                Some([width as f32 / 2.0, height as f32 / 2.0])
            );
            output
                .append_bodies(&[CelestialRenderBody {
                    reference_radius_m: f64::NAN,
                    ..body
                }])
                .unwrap_err();
            assert!(output.validate().is_err());
        }
    }
    assert!(reference_sphere_occludes(
        DVec3::new(0.0, 0.0, -100.0),
        DVec3::new(0.0, 0.0, -50.0),
        10.0
    ));
}

#[test]
fn f64_trail_clipping_and_marker_selection() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let sphere = Icosphere::new();
    let mut storage = CelestialStaging::default();
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(root, LocalPosition::origin()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let mut frame = CelestialFrame::new(&view, &mut storage, projection, &sphere);
    for [a, b] in [
        [DVec3::new(-1e11, 0.0, -1e11), DVec3::new(1e11, 0.0, -1e11)],
        [DVec3::new(0.0, 0.0, 10.0), DVec3::new(0.0, 0.0, -10.0)],
        [DVec3::new(1e11, 1e11, 1e11), DVec3::new(1e11, 1e11, 2e11)],
    ] {
        frame
            .append_historical_lines(
                root,
                &[DebugLine {
                    endpoints: [a, b]
                        .map(|p| FramePosition::new(root, LocalPosition::try_metres(p).unwrap())),
                    color: [1.0; 4],
                }],
            )
            .unwrap();
    }
    assert_eq!(frame.report().trail_segments, 2);
    assert!(frame.report().max_projected_error_pixels <= 0.05);
    assert_eq!(projection.project_marker(DVec3::ZERO).unwrap(), None);
    assert_eq!(projection.project_marker(DVec3::Z).unwrap(), None);
    let marker = CelestialMarker {
        request_index: 0,
        screen_pixels: Some([100.0, 100.0]),
        distance_m: 1.0,
        depth_m: 1.0,
        occluded: false,
        representation: SphereRepresentation::SubpixelMarker,
        apparent_diameter_pixels: 0.1,
        center_in_view_m: DVec3::new(0.0, 0.0, -1.0),
    };
    let markers = [
        CelestialMarker {
            request_index: 2,
            depth_m: 2.0,
            ..marker
        },
        CelestialMarker {
            request_index: 1,
            ..marker
        },
        marker,
    ];
    assert_eq!(select_marker(&markers, [100.0, 100.0]), Some(0));
    assert_eq!(select_marker(&markers, [109.0, 100.0]), None);
    assert!(
        frame
            .append_historical_lines(
                root,
                &[DebugLine {
                    endpoints: [FramePosition::new(root, LocalPosition::origin()); 2],
                    color: [f32::NAN; 4]
                }]
            )
            .is_err()
    );
    assert!(frame.validate().is_err());
}

#[test]
fn content_viewport_unprojection_and_styled_frame_poisoning() {
    let projection = CelestialProjection::try_new(800, 600, 1.0, 0.1)
        .unwrap()
        .with_origin([250, 100])
        .unwrap();
    for point in [
        DVec3::new(0.0, 0.0, -100.0),
        DVec3::new(10.0, -20.0, -100.0),
    ] {
        let screen = projection.project_pixels(point).unwrap().unwrap();
        let ray = projection.unproject_ray(screen).unwrap();
        assert!((ray - point.normalize()).length() < 1e-12);
    }
    assert_eq!(
        projection
            .project_marker(DVec3::new(0.0, 0.0, -100.0))
            .unwrap(),
        Some([650.0, 400.0])
    );
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let sphere = Icosphere::new();
    let mut storage = CelestialStaging::default();
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(root, LocalPosition::origin()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let points = [
        DVec3::new(0.0, 0.0, 1.0),
        DVec3::new(0.1, 0.0, -10.0),
        DVec3::new(1e11, 0.0, -1e12),
    ]
    .map(|p| FramePosition::new(root, LocalPosition::try_metres(p).unwrap()));
    let mut frame = CelestialFrame::new(&view, &mut storage, projection, &sphere);
    frame
        .append_polylines(&[CelestialPolyline {
            points: &points,
            colors: &[[1.0; 4]; 3],
            width_pixels: 2.5,
            style: CelestialLineStyle::Dashed,
        }])
        .unwrap();
    assert!(frame.report().max_projected_error_pixels <= 0.05);
    assert!(frame.report().polyline_segments > 0);
    assert!(
        frame
            .append_polylines(&[CelestialPolyline {
                points: &points,
                colors: &[[f32::NAN; 4]; 3],
                width_pixels: 2.5,
                style: CelestialLineStyle::Solid
            }])
            .is_err()
    );
    assert!(frame.validate().is_err());
}
