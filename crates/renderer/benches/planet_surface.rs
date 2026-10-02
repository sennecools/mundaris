use criterion::{Criterion, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::{surface::*, *};
use mundaris_renderer::{planet_surface::*, *};
use std::{hint::black_box, num::NonZeroU64, time::Duration};
fn benches(c: &mut Criterion) {
    let topology = SurfaceTopology::new();
    let mut group = c.benchmark_group("planet_metadata");
    for level in [0, 5, 16, 30] {
        let patch = CubePatchAddress::try_new(CubeFace::PositiveZ, level, 0, 0).unwrap();
        group.bench_function(format!("grid16/all_stitches/level{level}/records1"), |b| {
            b.iter(|| black_box(PatchMetadata::build(black_box(patch), &topology).unwrap()))
        });
    }
    group.finish();
    let mut group = c.benchmark_group("planet_mapping_comparison");
    let patch = CubePatchAddress::try_new(CubeFace::PositiveZ, 16, 32768, 32768).unwrap();
    for cells in [8, 16, 32] {
        let count = (cells + 1) * (cells + 1);
        let mut samples = vec![DVec3::ZERO; count as usize];
        group.bench_function(format!("grid{cells}/samples{count}"), |b| {
            b.iter(|| {
                for (index, p) in samples.iter_mut().enumerate() {
                    *p = patch
                        .sample_direction(
                            index as u32 % (cells + 1),
                            index as u32 / (cells + 1),
                            cells,
                        )
                        .unwrap()
                        .unit()
                        * 6.4e6;
                }
                black_box(&samples);
            })
        });
    }
    group.finish();
    let mut group = c.benchmark_group("planet_neighbors");
    let addresses: Vec<_> = CubeFace::ALL
        .into_iter()
        .flat_map(|face| (0..32).map(move |i| CubePatchAddress::try_new(face, 5, 31, i).unwrap()))
        .collect();
    group.bench_function("cross_face_and_interior/queries768", |b| {
        b.iter(|| {
            for &p in &addresses {
                for edge in PatchEdge::ALL {
                    black_box(p.neighbor(black_box(edge)));
                }
            }
        })
    });
    group.finish();
    let mut group = c.benchmark_group("planet_bounds_culling");
    let records: Vec<_> = addresses
        .iter()
        .map(|&p| PatchMetadata::build(p, &topology).unwrap())
        .collect();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    group.bench_function("ball_extent/records192", |b| {
        b.iter(|| {
            for m in &records {
                black_box(m.ball(6.4e6, SurfaceExtent::smooth(6.4e6)).unwrap());
            }
        })
    });
    group.bench_function("horizon/records192", |b| {
        b.iter(|| {
            for m in &records {
                black_box(m.horizon_reject(
                    black_box(DVec3::Z * 6_400_002.0),
                    6.4e6,
                    SurfaceExtent::smooth(6.4e6),
                ));
            }
        })
    });
    let balls: Vec<_> = records
        .iter()
        .map(|m| m.ball(6.4e6, SurfaceExtent::smooth(6.4e6)).unwrap())
        .collect();
    group.bench_function("frustum/records192", |b| {
        b.iter(|| {
            for &(center, radius) in &balls {
                black_box(
                    projection
                        .rejects_ball(center - DVec3::Z * 6_400_002.0, radius)
                        .unwrap(),
                );
            }
        })
    });
    group.finish();
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let frame = tree.root();
    let radius = 6.4e6;
    let mut group = c.benchmark_group("planet_selection");
    for clearance in [
        1e11, 8.3e7, 1e7, 6.4e6, 1e6, 1e5, 1e4, 1e3, 100.0, 10.0, 2.0,
    ] {
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
            projection: CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1)
                .unwrap(),
        };
        let settings = LodSettings::default();
        let mut session = SurfaceLodSession::default();
        let mut r = LodReport::default();
        for _ in 0..1000 {
            r = session.update(&input, &settings).unwrap();
            if (!r.desired_estimate_incomplete && r.settled) || r.budget_constrained {
                break;
            }
        }
        assert!(!r.desired_estimate_incomplete && !r.budget_constrained);
        let workload = format!(
            "h{clearance}/cover{}/visible{}/level{}",
            r.active_patches, r.visible_patches, r.max_level
        );
        group.bench_function(format!("warm/{workload}"), |b| {
            b.iter(|| {
                black_box(session.update(black_box(&input), &settings).unwrap());
                black_box(&session);
            })
        });
        group.bench_function(format!("cold/{workload}/new32"), |b| {
            b.iter_batched(
                SurfaceLodSession::default,
                |mut session| {
                    black_box(session.update(&input, &settings).unwrap());
                    black_box(session);
                },
                criterion::BatchSize::SmallInput,
            )
        });
        let sphere = Icosphere::new();
        let mut staging = CelestialStaging::default();
        let body = CelestialRenderBody {
            body_fixed_frame: frame,
            reference_radius_m: radius,
            color: [0.2, 0.5, 1.0, 1.0],
            unlit: false,
            selected: true,
        };
        group.bench_function(format!("prepare/{workload}"), |b| {
            b.iter(|| {
                let mut frame = CelestialFrame::new(&view, &mut staging, input.projection, &sphere);
                frame
                    .append_surface(
                        body,
                        session.active_visible(),
                        session.topology(),
                        SurfaceStyle::default(),
                    )
                    .unwrap();
                black_box(frame.report());
                black_box(&frame);
            })
        });
    }
    group.finish();
}
criterion_group! {name=surface;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(surface);
