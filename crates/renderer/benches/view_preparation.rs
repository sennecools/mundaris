use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use astrum_math::*;
use astrum_renderer::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration};

fn fixture(offset: f64) -> (FrameTree, FrameId, FramePose) {
    let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let root = tree.root();
    let rotation =
        UnitRotation::from_axis_angle(Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap(), 0.7)
            .unwrap();
    let common = tree
        .insert(
            root,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(offset, 0.0, 0.0)).unwrap(),
                rotation,
            )),
        )
        .unwrap();
    let regional = tree
        .insert(
            common,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(0.0, 6_371_000.0, 0.0)).unwrap(),
                rotation,
            )),
        )
        .unwrap();
    let observer = FramePose::new(
        FramePosition::new(
            regional,
            LocalPosition::try_metres(DVec3::new(0.0, 1.7, 0.0)).unwrap(),
        ),
        rotation,
    );
    (tree, regional, observer)
}
fn views(c: &mut Criterion) {
    let mut preparation = c.benchmark_group("view_prepare");
    for (name, offset) in [
        ("100m", 0.0),
        ("10km", 0.0),
        ("shared1.5e11", 1.5e11),
        ("shared1e16", 1e16),
    ] {
        let (tree, source, observer) = fixture(offset);
        let evaluation = tree.evaluate();
        preparation.bench_function(name, |bencher| {
            bencher.iter(|| {
                let view = PreparedView::new(
                    &evaluation,
                    black_box(observer),
                    RenderPrecisionBudget::near_debug(),
                )
                .unwrap();
                black_box(view.prepare_source(black_box(source)).unwrap());
            })
        });
    }
    preparation.finish();
    let mut batches = c.benchmark_group("view_batch");
    for (name, offset, range, error) in [
        ("100m", 0.0, 100.0, 1e-5),
        ("10km", 0.0, 10_000.0, 1e-3),
        ("shared1.5e11", 1.5e11, 100.0, 1e-5),
        ("shared1e16", 1e16, 100.0, 1e-5),
    ] {
        let (tree, source, observer) = fixture(offset);
        let evaluation = tree.evaluate();
        let view = PreparedView::new(
            &evaluation,
            observer,
            RenderPrecisionBudget::try_new(range, error).unwrap(),
        )
        .unwrap();
        let prepared = view.prepare_source(source).unwrap();
        for count in [1024, 65_536] {
            let points: Vec<_> = (0..count)
                .map(|index| {
                    FramePosition::new(
                        source,
                        LocalPosition::try_metres(DVec3::new(
                            range * 0.5 * (index as f64 / count as f64),
                            1.7,
                            -range * 0.25,
                        ))
                        .unwrap(),
                    )
                })
                .collect();
            let mut output = vec![prepared.try_position(points[0]).unwrap(); count];
            prepared.write_positions(&points, &mut output).unwrap();
            for (&point, &output) in points.iter().zip(&output) {
                assert_eq!(
                    view.prepare_source(source)
                        .unwrap()
                        .try_position(point)
                        .unwrap(),
                    output
                );
            }
            batches.throughput(Throughput::Elements(count as u64));
            batches.bench_function(format!("{name}/{count}"), |bencher| {
                bencher.iter(|| {
                    prepared
                        .write_positions(black_box(&points), black_box(&mut output))
                        .unwrap();
                    black_box(&output);
                })
            });
        }
    }
    batches.finish();
}
fn config() -> Criterion {
    Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500))
}
criterion_group! { name=benches; config=config(); targets=views }
criterion_main!(benches);
