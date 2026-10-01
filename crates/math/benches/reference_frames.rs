use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};

fn state(index: usize) -> FrameState {
    FrameState::new(
        RigidTransform::new(
            Displacement3::try_metres(DVec3::new(index as f64 * 0.1, 2.0, -3.0)).unwrap(),
            UnitRotation::from_axis_angle(
                Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
                0.01 * (index + 1) as f64,
            )
            .unwrap(),
        ),
        Some(FrameMotion::new(
            LinearVelocity3::try_metres_per_second(DVec3::new(1.0, 2.0, 3.0)).unwrap(),
            AngularVelocity3::try_radians_per_second(DVec3::new(0.01, 0.02, 0.03)).unwrap(),
        )),
    )
}
fn branches(depth: usize) -> (FrameTree, FrameId, FrameId) {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let mut a = tree.root();
    let mut b = a;
    for index in 0..depth {
        a = tree.insert(a, state(index)).unwrap();
        b = tree.insert(b, state(index + 1)).unwrap();
    }
    (tree, a, b)
}
fn preparation(c: &mut Criterion) {
    let mut group = c.benchmark_group("prepare");
    for depth in [1, 4, 8, 32] {
        let (tree, a, b) = branches(depth);
        let root = tree.root();
        let sibling = tree.evaluate().parent(a).unwrap().unwrap();
        // Insert an actual sibling of the depth-d leaf for the sibling group.
        let mut tree = tree;
        let sibling = tree.insert(sibling, state(99)).unwrap();
        for (kind, from, to) in [
            ("same", a, a),
            ("sibling", a, sibling),
            ("ancestor", root, a),
            ("cross", a, b),
        ] {
            group.bench_function(format!("pose/{kind}/depth{depth}"), |bencher| {
                bencher.iter(|| {
                    black_box(
                        tree.evaluate()
                            .prepare_conversion(black_box(from), black_box(to))
                            .unwrap(),
                    )
                })
            });
            group.bench_function(format!("motion/{kind}/depth{depth}"), |bencher| {
                bencher.iter(|| {
                    black_box(
                        tree.evaluate()
                            .prepare_kinematic_conversion(black_box(from), black_box(to))
                            .unwrap(),
                    )
                })
            });
        }
    }
    group.finish();
}
fn repeated(c: &mut Criterion) {
    let (tree, a, b) = branches(8);
    let evaluation = tree.evaluate();
    let prepared = evaluation.prepare_conversion(a, b).unwrap();
    let mut group = c.benchmark_group("points");
    for count in [1024, 65_536] {
        let points: Vec<_> = (0..count)
            .map(|index| {
                FramePosition::new(
                    a,
                    LocalPosition::try_metres(DVec3::new(index as f64 * 0.001, 1.7, -5.0)).unwrap(),
                )
            })
            .collect();
        let mut output = vec![points[0]; count];
        for &point in &points {
            let direct = evaluation.convert_position(point, b).unwrap();
            let batch = prepared.convert_position(point).unwrap();
            assert!(direct.local().metres().is_finite());
            assert!((direct.local().metres() - batch.local().metres()).length() <= 1e-9);
        }
        group.throughput(Throughput::Elements(count as u64));
        group.bench_function(format!("direct/{count}"), |bencher| {
            bencher.iter(|| {
                for (point, output) in black_box(&points).iter().zip(&mut output) {
                    *output = evaluation.convert_position(*point, b).unwrap();
                }
                black_box(&output);
            })
        });
        group.bench_function(format!("prepared/{count}"), |bencher| {
            bencher.iter(|| {
                let prepared = evaluation
                    .prepare_conversion(black_box(a), black_box(b))
                    .unwrap();
                for (point, output) in black_box(&points).iter().zip(&mut output) {
                    *output = prepared.convert_position(*point).unwrap();
                }
                black_box(&output);
            })
        });
    }
    group.finish();
}
fn publication(c: &mut Criterion) {
    let mut group = c.benchmark_group("publication");
    // Size denotes mutable non-root frames: 64 updates need 64 edges plus root.
    for frames in [64, 4096] {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let root = tree.root();
        let ids: Vec<_> = (0..frames)
            .map(|index| tree.insert(root, state(index)).unwrap())
            .collect();
        for updated in [1, 64] {
            let updates: Vec<_> = ids
                .iter()
                .take(updated)
                .enumerate()
                .map(|(index, &id)| (id, state(index + 1)))
                .collect();
            for attached in [0, 65_536] {
                let points = vec![FramePosition::new(ids[0], LocalPosition::origin()); attached];
                let mut sample = 0.0;
                group.throughput(Throughput::Elements(updated as u64));
                group.bench_function(
                    format!("update/F{frames}/U{updated}/attached{attached}"),
                    |bencher| {
                        bencher.iter(|| {
                            sample += 1.0;
                            black_box(&points);
                            tree.update_states(black_box(sample), black_box(&updates))
                                .unwrap();
                            black_box(&tree);
                        })
                    },
                );
                group.throughput(Throughput::Elements(8));
                group.bench_function(
                    format!("visible_prepare/F{frames}/U{updated}/attached{attached}"),
                    |bencher| {
                        bencher.iter(|| {
                            for &id in black_box(&ids[..8]) {
                                black_box(tree.evaluate().prepare_conversion(id, root).unwrap());
                            }
                        })
                    },
                );
            }
        }
    }
    group.finish();
}
fn config() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500))
}
criterion_group! { name=benches; config=config(); targets=preparation,repeated,publication }
criterion_main!(benches);
