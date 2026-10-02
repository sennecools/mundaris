use glam::DVec3;
use mundaris_math::surface::*;
use mundaris_math::*;
use mundaris_renderer::{CelestialProjection, planet_surface::*};
use mundaris_renderer::{PreparedView, RenderPrecisionBudget};
use std::{collections::BTreeSet, num::NonZeroU64};

fn assert_cover(session: &SurfaceLodSession) {
    let cover: BTreeSet<_> = session.covering_leaves().collect();
    for face in CubeFace::ALL {
        let area: f64 = cover
            .iter()
            .filter(|p| p.face() == face)
            .map(|p| 4.0_f64.powi(-i32::from(p.level())))
            .sum();
        assert_eq!(area, 1.0);
    }
    for &p in &cover {
        let mut a = p.parent();
        while let Some(parent) = a {
            assert!(!cover.contains(&parent));
            a = parent.parent();
        }
        for edge in PatchEdge::ALL {
            let mut n = p.neighbor(edge).address;
            while !cover.contains(&n) {
                if let Some(a) = n.parent() {
                    n = a;
                } else {
                    break;
                }
            }
            if cover.contains(&n) {
                assert!(p.level() <= n.level() + 1);
            }
        }
    }
}

#[test]
fn radial_count_review_complete_balanced_readiness_and_stationary_determinism() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let frame = tree.root();
    let radius = 6.4e6;
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let settings = LodSettings::default();
    let mut session = SurfaceLodSession::default();
    for clearance in [1e11, 8.3e7, 1e7, 1e6, 1e5, 1e4, 1e3, 100.0, 10.0, 2.0] {
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    frame,
                    LocalPosition::try_metres(DVec3::Z * (radius + clearance)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: frame,
            reference_radius_m: radius,
            projection,
        };
        let mut report = LodReport::default();
        for _ in 0..1000 {
            report = session.update(&input, &settings).unwrap();
            assert_cover(&session);
            if !report.desired_estimate_incomplete && report.settled {
                break;
            }
            if report.budget_constrained {
                break;
            }
        }
        eprintln!(
            "clearance={clearance} desired={} balanced={} cover={} visible={} level={} error={} built={} cache={}/{} bytes={} incomplete={} constrained={}",
            report.desired_patches,
            report.balanced_patches,
            report.active_patches,
            report.visible_patches,
            report.max_level,
            report.max_error_pixels,
            report.metadata_built,
            report.cache_records,
            4096,
            report.cache_bytes,
            report.desired_estimate_incomplete,
            report.budget_constrained
        );
        assert!(report.visible_patches <= 4096);
        assert!(report.cache_bytes <= 1024 * 1024);
        if !report.budget_constrained {
            assert!(!report.desired_estimate_incomplete);
            assert!(report.max_error_pixels <= 0.125);
            let before: Vec<_> = session.covering_leaves().collect();
            for _ in 0..10 {
                let r = session.update(&input, &settings).unwrap();
                assert_eq!(r.splits, 0);
                assert_eq!(r.merges, 0);
                assert_eq!(session.covering_leaves().collect::<Vec<_>>(), before);
            }
        }
    }
    assert_eq!(tree.evaluate().revision(), 0);
}

#[test]
fn grazing_count_review_centres_edges_corners() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let frame = tree.root();
    let radius = 6.4e6;
    for radial in [
        DVec3::Z,
        DVec3::new(1.0, 0.0, 1.0).normalize(),
        DVec3::ONE.normalize(),
    ] {
        let tangent = SurfaceTangentBasis::new(Direction3::try_new(radial).unwrap());
        let basis = glam::DMat3::from_cols(tangent.east().unit(), radial, -tangent.north().unit());
        let orientation =
            UnitRotation::try_from_quaternion(glam::DQuat::from_mat3(&basis)).unwrap();
        let mut session = SurfaceLodSession::default();
        for clearance in [1e5, 1e4, 100.0, 2.0] {
            let projection =
                CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
            let view = PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(
                        frame,
                        LocalPosition::try_metres(radial * (radius + clearance)).unwrap(),
                    ),
                    orientation,
                ),
                RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            let input = SurfaceViewInput {
                view: &view,
                body_fixed_frame: frame,
                reference_radius_m: radius,
                projection,
            };
            let mut report = LodReport::default();
            for _ in 0..300 {
                report = session.update(&input, &LodSettings::default()).unwrap();
                assert_cover(&session);
                if (!report.desired_estimate_incomplete && report.settled)
                    || report.budget_constrained
                {
                    break;
                }
            }
            eprintln!(
                "grazing radial={radial:?} clearance={clearance} desired={} balanced={} cover={} visible={} level={} error={} cache={} incomplete={} constrained={}",
                report.desired_patches,
                report.balanced_patches,
                report.active_patches,
                report.visible_patches,
                report.max_level,
                report.max_error_pixels,
                report.cache_records,
                report.desired_estimate_incomplete,
                report.budget_constrained
            );
            assert!(report.visible_patches <= 4096);
        }
    }
}

#[test]
fn starvation_atomic_children_hysteresis_and_replay() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let radius = 6.4e6;
    let mut a = SurfaceLodSession::default();
    let mut b = SurfaceLodSession::default();
    for (clearance, work) in [
        (2.0, 0),
        (2.0, 1),
        (1e11, 1),
        (2.0, 32),
        (1e4, 32),
        (2.0, 32),
    ] {
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    root,
                    LocalPosition::try_metres(DVec3::Z * (radius + clearance)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: root,
            reference_radius_m: radius,
            projection,
        };
        let settings = LodSettings::default().with_work_limit(work).unwrap();
        for _ in 0..30 {
            let ar = a.update(&input, &settings).unwrap();
            let br = b.update(&input, &settings).unwrap();
            assert_eq!(ar.active_patches, br.active_patches);
            assert!(ar.metadata_built <= work);
            assert_eq!(
                a.covering_leaves().collect::<Vec<_>>(),
                b.covering_leaves().collect::<Vec<_>>()
            );
            assert_cover(&a);
            if work == 0 {
                assert_eq!(ar.active_patches, 6);
                assert!(ar.desired_estimate_incomplete);
            }
        }
    }
    let before: Vec<_> = a.covering_leaves().collect();
    for i in 0..1000 {
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    root,
                    LocalPosition::try_metres(DVec3::Z * (radius + 2.0 + (i % 2) as f64 * 1e-8))
                        .unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: root,
            reference_radius_m: radius,
            projection,
        };
        let report = a.update(&input, &LodSettings::default()).unwrap();
        assert_eq!(report.splits, 0);
        assert_eq!(report.merges, 0);
        assert_eq!(a.covering_leaves().collect::<Vec<_>>(), before);
    }
}

#[test]
fn large_viewport_reversal_quota_converges_without_cache_dependency_churn() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let source = tree.root();
    let radius = 6.4e6;
    let mut session = SurfaceLodSession::new(2048).unwrap();
    for (radial, clearance, w, h) in [
        (DVec3::Z, 3.0 * radius, 2250, 1290),
        (DVec3::new(0.2, 0.3, 1.0).normalize(), 1.75e6, 2250, 1290),
        (DVec3::new(-0.2, 0.4, 1.0).normalize(), 1.75e6, 2250, 1290),
        (DVec3::Z, 2.0, 3840, 2160),
        (DVec3::Z, 1e11, 1280, 800),
    ] {
        let orientation =
            UnitRotation::try_from_quaternion(glam::DQuat::from_rotation_arc(DVec3::Z, radial))
                .unwrap();
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    source,
                    LocalPosition::try_metres(radial * (radius + clearance)).unwrap(),
                ),
                orientation,
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: source,
            reference_radius_m: radius,
            projection: CelestialProjection::try_new(w, h, 60.0_f64.to_radians(), 0.1).unwrap(),
        };
        let mut r = LodReport::default();
        let mut updates = 0;
        for _ in 0..1000 {
            updates += 1;
            r = session
                .update(
                    &input,
                    &LodSettings::default().with_limits(2048, 32768, 30).unwrap(),
                )
                .unwrap();
            assert_cover(&session);
            if r.settled && !r.desired_estimate_incomplete {
                break;
            }
        }
        eprintln!(
            "quota2048 viewport={w}x{h} h={clearance} updates={updates} cover={} visible={} error={} cache={} evictions={} scratch={} constrained={}",
            r.active_patches,
            r.visible_patches,
            r.max_error_pixels,
            r.cache_records,
            r.cache_evictions,
            r.scratch_bytes,
            r.budget_constrained
        );
        assert!(
            !r.quality_pending && !r.desired_estimate_incomplete,
            "{r:?}"
        );
        for _ in 0..10 {
            let r = session.update(&input, &LodSettings::default()).unwrap();
            assert_eq!(r.metadata_built, 0);
            assert_eq!(r.cache_evictions, 0);
        }
    }
}

#[test]
fn threshold_excursions_and_deadband_retain_previous_decisions() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let frame = tree.root();
    let radius = 6.4e6;
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let topology = SurfaceTopology::new();
    let patch = CubePatchAddress::root(CubeFace::PositiveZ);
    let metadata = PatchMetadata::build(patch, &topology).unwrap();
    let (center, ball) = metadata
        .ball(radius, SurfaceExtent::smooth(radius))
        .unwrap();
    let distance_for_error = |target: f64| {
        let mut lo = radius * 2.0;
        let mut hi = 1e12;
        for _ in 0..100 {
            let mid = (lo + hi) * 0.5;
            let error = metadata.projected_error(center - DVec3::Z * mid, ball, radius, projection);
            if error > target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (lo + hi) * 0.5
    };
    let mut session = SurfaceLodSession::default();
    for (error, split) in [
        (0.13, true),
        (0.1, true),
        (0.060, false),
        (0.1, false),
        (0.13, true),
    ] {
        let view = PreparedView::new(
            &tree.evaluate(),
            FramePose::new(
                FramePosition::new(
                    frame,
                    LocalPosition::try_metres(DVec3::Z * distance_for_error(error)).unwrap(),
                ),
                UnitRotation::identity(),
            ),
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let input = SurfaceViewInput {
            view: &view,
            body_fixed_frame: frame,
            reference_radius_m: radius,
            projection,
        };
        for _ in 0..100 {
            let r = session.update(&input, &LodSettings::default()).unwrap();
            if r.settled {
                break;
            }
        }
        assert_eq!(!session.covering_leaves().any(|p| p == patch), split);
        for _ in 0..1000 {
            let r = session.update(&input, &LodSettings::default()).unwrap();
            assert_eq!(r.splits, 0);
            assert_eq!(r.merges, 0);
        }
    }
}

#[test]
fn projected_error_bounds_independent_radial_discrepancy_across_fov_and_pixels() {
    let radius = 6.4e6;
    let topology = SurfaceTopology::new();
    for (level, clearance) in [(2, 1e11), (8, 1e4), (10, 1e4), (18, 2.0)] {
        let address = CubePatchAddress::try_new(
            CubeFace::PositiveZ,
            level,
            1u32 << (level - 1),
            1u32 << (level - 1),
        )
        .unwrap();
        let metadata = PatchMetadata::build(address, &topology).unwrap();
        let (center, ball) = metadata
            .ball(radius, SurfaceExtent::smooth(radius))
            .unwrap();
        let observer = DVec3::Z * (radius + clearance);
        for height in [600, 800, 2160] {
            for fov in [30.0_f64, 60.0, 90.0] {
                let projection =
                    CelestialProjection::try_new(height * 2, height, fov.to_radians(), 0.1)
                        .unwrap();
                let bound = metadata.projected_error(center - observer, ball, radius, projection);
                let doubled =
                    CelestialProjection::try_new(height * 4, height * 2, fov.to_radians(), 0.1)
                        .unwrap();
                let doubled_bound =
                    metadata.projected_error(center - observer, ball, radius, doubled);
                if bound.is_finite() {
                    assert!((doubled_bound / bound - 2.0).abs() <= 2e-12);
                } else {
                    assert!(bound.is_infinite() && doubled_bound.is_infinite());
                }
                for mask in 0..16 {
                    for triangle in topology.indices(mask).as_chunks::<3>().0 {
                        let [a, b, c] = triangle.map(|i| {
                            address
                                .sample_direction(u32::from(i % 17), u32::from(i / 17), 16)
                                .unwrap()
                                .unit()
                                * radius
                        });
                        let point = a * 0.2 + b * 0.3 + c * 0.5;
                        let analytic = point.normalize() * radius;
                        if let (Some(p), Some(q)) = (
                            projection.project_pixels(point - observer).unwrap(),
                            projection.project_pixels(analytic - observer).unwrap(),
                        ) {
                            let error = (p[0] - q[0]).hypot(p[1] - q[1]);
                            assert!(
                                error <= bound + 1e-6,
                                "level={level} h={height} fov={fov} error={error} bound={bound}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn independent_dense_containment_error_and_triangle_horizon_oracle() {
    let topology = SurfaceTopology::new();
    let radius = 6.4e6;
    for face in CubeFace::ALL {
        for level in [0, 1, 5, 16, 30] {
            let count = 1u32 << level;
            for [x, y] in [[0, 0], [count / 2, count / 2], [count - 1, count - 1]] {
                let patch = CubePatchAddress::try_new(face, level, x, y).unwrap();
                let m = PatchMetadata::build(patch, &topology).unwrap();
                assert!(m.error_unit().is_finite() && m.error_unit() >= 64.0 * f64::EPSILON);
                for displacement in [0.0, 1000.0] {
                    let extent = SurfaceExtent {
                        min_height_m: -displacement,
                        max_height_m: displacement,
                        guaranteed_opaque_radius_m: radius - displacement,
                    };
                    let (center, r) = m.ball(radius, extent).unwrap();
                    for i in 0..=20 {
                        for j in 0..=20 {
                            let d = face
                                .direction(
                                    patch.face_uv([i as f64 / 20.0, j as f64 / 20.0]).unwrap(),
                                )
                                .unwrap()
                                .unit();
                            for h in [-displacement, displacement] {
                                assert!((d * (radius + h) - center).length() <= r + 1e-7);
                            }
                        }
                    }
                }
                let observers = [
                    DVec3::Z * (radius + 2.0),
                    DVec3::X * (radius + 1e5),
                    -DVec3::Y * (radius + 10.0),
                ];
                for mask in 0..16 {
                    for tri in topology.indices(mask).as_chunks::<3>().0 {
                        let [a, b, c] = tri.map(|i| {
                            patch
                                .sample_direction(u32::from(i % 17), u32::from(i / 17), 16)
                                .unwrap()
                                .unit()
                        });
                        for weights in [[1.0 / 3.0; 3], [0.1, 0.3, 0.6], [0.5, 0.5, 0.0]] {
                            let p = a * weights[0] + b * weights[1] + c * weights[2];
                            let error = radius * (1.0 - p.length());
                            assert!(
                                error <= radius * m.error_unit() + 1e-7,
                                "{patch:?} mask={mask} error={error}"
                            );
                        }
                        if level < 30 {
                            let normal = (b - a).cross(c - a).normalize();
                            for observer in observers {
                                if m.horizon_reject(observer, radius, SurfaceExtent::smooth(radius))
                                {
                                    assert!(normal.dot(observer) - radius * normal.dot(a) < 1e-7);
                                }
                            }
                        }
                    }
                }
                assert!(!m.horizon_reject(DVec3::ZERO, radius, SurfaceExtent::smooth(radius)));
            }
        }
    }
}

#[test]
fn five_plane_grazing_large_and_near_surface_ball_cases() {
    for (w, h) in [(1280, 800), (800, 2160), (3840, 600)] {
        let p = CelestialProjection::try_new(w, h, 60.0_f64.to_radians(), 0.1)
            .unwrap()
            .with_origin([100, 50])
            .unwrap();
        assert!(!p.rejects_ball(DVec3::new(0.0, 0.0, -2.0), 0.01).unwrap());
        assert!(p.rejects_ball(DVec3::new(0.0, 0.0, 2.0), 0.01).unwrap());
        assert!(!p.rejects_ball(DVec3::ZERO, 6.4e6).unwrap());
        assert!(!p.rejects_ball(DVec3::new(0.0, 0.0, -0.1), 0.0).unwrap());
        for (n, d) in p.frustum_planes() {
            assert!((n.length() - 1.0).abs() < 1e-14);
            let center = -n * (d + 1.0);
            assert!(!p.rejects_ball(center, 1.0 + center.length()).unwrap());
        }
    }
}
