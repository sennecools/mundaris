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
            if !r.desired_estimate_incomplete && !r.quality_pending {
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

fn radial_hit(direction: DVec3, triangles: &[[DVec3; 3]]) -> f64 {
    triangles
        .iter()
        .filter_map(|&[a, b, c]| {
            let e1 = b - a;
            let e2 = c - a;
            let p = direction.cross(e2);
            let determinant = e1.dot(p);
            if determinant.abs() < 1e-20 {
                return None;
            }
            let inverse = 1.0 / determinant;
            let t = -a;
            let u = t.dot(p) * inverse;
            let q = t.cross(e1);
            let v = direction.dot(q) * inverse;
            if u < 0.0 || v < 0.0 || u + v > 1.0 {
                return None;
            }
            let distance = e2.dot(q) * inverse;
            (distance > 0.0).then_some(distance)
        })
        .fold(f64::MAX, f64::min)
}

#[test]
fn matched_radial_handoff_displacement_stays_subpixel() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let source = tree.root();
    let radius = 6.4e6;
    let observer = DVec3::Z * (radius + 4e8);
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(source, LocalPosition::try_metres(observer).unwrap()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let input = SurfaceViewInput {
        view: &view,
        body_fixed_frame: source,
        reference_radius_m: radius,
        projection,
    };
    let mut session = SurfaceLodSession::default();
    for _ in 0..100 {
        let r = session.update(&input, &LodSettings::default()).unwrap();
        if !r.quality_pending && !r.desired_estimate_incomplete {
            break;
        }
    }
    let sphere = Icosphere::new();
    let old: Vec<_> = sphere
        .indices()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| t.map(|i| sphere.vertices()[i as usize]))
        .collect();
    let mut new = Vec::new();
    for patch in session.active_visible() {
        let vertices: Vec<_> = (0..289)
            .map(|i| {
                patch
                    .address
                    .sample_direction(i % 17, i / 17, 16)
                    .unwrap()
                    .unit()
            })
            .collect();
        new.extend(
            session
                .topology()
                .indices(patch.stitch_mask)
                .as_chunks::<3>()
                .0
                .iter()
                .map(|t| t.map(|i| vertices[i as usize])),
        );
    }
    let mut maximum: f64 = 0.0;
    for i in 0..21 {
        for j in 0..21 {
            let direction =
                DVec3::new(-0.95 + i as f64 * 0.095, -0.95 + j as f64 * 0.095, 1.0).normalize();
            let a = radial_hit(direction, &old);
            let b = radial_hit(direction, &new);
            assert!(a <= 1.0 + 1e-14 && b <= 1.0 + 1e-14);
            let old = projection
                .project_pixels(direction * (radius * a) - observer)
                .unwrap()
                .unwrap();
            let new = projection
                .project_pixels(direction * (radius * b) - observer)
                .unwrap()
                .unwrap();
            maximum = maximum.max((old[0] - new[0]).hypot(old[1] - new[1]));
        }
    }
    eprintln!(
        "handoff 441 matched radial directions: max exact projected displacement={maximum} px"
    );
    assert!(maximum <= 0.35);
}
