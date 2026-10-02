use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};
fn benches(c: &mut Criterion) {
    let sphere = Icosphere::new();
    let mut group = c.benchmark_group("celestial_preparation");
    for n in [3, 16, 64, 256, 1024] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let mut frames = Vec::with_capacity(n);
        for i in 0..n {
            frames.push(
                tree.insert(
                    root,
                    FrameState::stationary(RigidTransform::new(
                        Displacement3::try_metres(DVec3::new(
                            (i % 32) as f64 * 12.0 - 192.0,
                            (i / 32) as f64 * 12.0 - 192.0,
                            0.0,
                        ))
                        .unwrap(),
                        UnitRotation::identity(),
                    )),
                )
                .unwrap(),
            );
        }
        let observer = FramePose::new(
            FramePosition::new(root, LocalPosition::try_metres(DVec3::Z * 1000.0).unwrap()),
            UnitRotation::identity(),
        );
        let view = PreparedView::new(
            &tree.evaluate(),
            observer,
            RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection =
            CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
        let requests: Vec<_> = frames
            .iter()
            .map(|&body_fixed_frame| CelestialRenderBody {
                body_fixed_frame,
                reference_radius_m: if n <= 64 { 10.0 } else { 0.001 },
                color: [0.2, 0.5, 1.0, 1.0],
                unlit: false,
                selected: false,
            })
            .collect();
        let mut storage = CelestialStaging::default();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(format!("view_sources/N{n}"), |b| {
            b.iter(|| {
                let view = PreparedView::new(
                    &tree.evaluate(),
                    black_box(observer),
                    RenderPrecisionBudget::near_debug(),
                )
                .unwrap();
                for &frame in &frames {
                    black_box(view.prepare_source(frame).unwrap());
                }
            })
        });
        group.bench_function(
            format!(
                "{}/N{n}",
                if n <= 64 {
                    "sphere_vertices_packing"
                } else {
                    "marker_heavy"
                }
            ),
            |b| {
                b.iter(|| {
                    let mut frame = CelestialFrame::new(&view, &mut storage, projection, &sphere);
                    frame.append_bodies(black_box(&requests)).unwrap();
                    black_box(frame.report());
                    black_box(frame.markers());
                    black_box(&frame);
                })
            },
        );
    }
    group.finish();
    let mut group = c.benchmark_group("celestial_trail_clip_narrow_pack");
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(root, LocalPosition::try_metres(DVec3::Z * 1000.0).unwrap()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    for bodies in [3, 16] {
        for samples in [1024, 8192] {
            // Explicit synthetic geometry cost probe, not a simulated-trajectory oracle.
            let lines: Vec<_> = (0..bodies)
                .flat_map(|body| {
                    (0..samples - 1).map(move |i| {
                        let points = [i, i + 1].map(|k| {
                            DVec3::new(
                                (k % 128) as f64 * 2.0 - 128.0,
                                body as f64 * 12.0 - 96.0,
                                (k / 128) as f64 * 0.01,
                            )
                        });
                        DebugLine {
                            endpoints: points.map(|p| {
                                FramePosition::new(root, LocalPosition::try_metres(p).unwrap())
                            }),
                            color: [0.2, 0.6, 1.0, 1.0],
                        }
                    })
                })
                .collect();
            let mut storage = CelestialStaging::default();
            group.throughput(Throughput::Elements(lines.len() as u64));
            group.bench_function(format!("B{bodies}/samples{samples}"), |b| {
                b.iter(|| {
                    let mut frame = CelestialFrame::new(&view, &mut storage, projection, &sphere);
                    frame
                        .append_historical_lines(root, black_box(&lines))
                        .unwrap();
                    black_box(frame.report());
                    black_box(&frame);
                })
            });
        }
    }
    group.finish();
    let mut group = c.benchmark_group("styled_polyline_clip_quad_pack");
    for bodies in [3, 16] {
        for samples in [1024, 8192] {
            let points: Vec<Vec<_>> = (0..bodies)
                .map(|body| {
                    (0..samples)
                        .map(|i| {
                            FramePosition::new(
                                root,
                                LocalPosition::try_metres(DVec3::new(
                                    (i as f64 * 0.01).cos() * 200.0,
                                    (i as f64 * 0.01).sin() * 200.0,
                                    body as f64,
                                ))
                                .unwrap(),
                            )
                        })
                        .collect()
                })
                .collect();
            let colors: Vec<_> = (0..samples)
                .map(|i| [0.2, 0.6, 1.0, 0.15 + 0.85 * i as f32 / samples as f32])
                .collect();
            let lines: Vec<_> = points
                .iter()
                .map(|points| CelestialPolyline {
                    points,
                    colors: &colors,
                    width_pixels: 1.5,
                    style: CelestialLineStyle::Solid,
                })
                .collect();
            let mut storage = CelestialStaging::default();
            group.bench_function(format!("B{bodies}/vertices{samples}"), |b| {
                b.iter(|| {
                    let mut frame = CelestialFrame::new(&view, &mut storage, projection, &sphere);
                    frame.append_polylines(black_box(&lines)).unwrap();
                    black_box(frame.report());
                    black_box(&frame);
                })
            });
        }
    }
    group.finish();
}
criterion_group! {name=celestial;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(celestial);
