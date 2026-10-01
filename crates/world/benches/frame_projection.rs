use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use mundaris_math::*;
use mundaris_world::*;
use std::{hint::black_box, num::NonZeroU64};

fn ns(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
fn state() -> BodyState {
    BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    )
}
fn fixture(count: usize) -> CelestialSystem {
    let mut world = CelestialSystem::new(ns(1), SimulationInstant::ZERO);
    for _ in 0..count {
        world
            .insert_body("body", BodyProperties::new(1e24, 1e6).unwrap(), state())
            .unwrap();
    }
    world
}
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("frame_projection");
    let mut namespace = 1u64;
    for count in [64, 1024, 4096] {
        let world = fixture(count);
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::new("build", count), &count, |b, _| {
            b.iter(|| {
                namespace = namespace.checked_add(1).unwrap();
                black_box(
                    CelestialFrameProjection::build(black_box(&world), ns(namespace)).unwrap(),
                );
            })
        });
        namespace = namespace.checked_add(1).unwrap();
        let mut projection = CelestialFrameProjection::build(&world, ns(namespace)).unwrap();
        group.bench_with_input(BenchmarkId::new("republish_full", count), &count, |b, _| {
            b.iter(|| {
                projection.publish(black_box(&world)).unwrap();
                black_box(&projection);
            })
        });
        group.bench_with_input(BenchmarkId::new("live_append", count), &count, |b, _| {
            b.iter_batched(
                || {
                    namespace = namespace.checked_add(1).unwrap();
                    let world = fixture(count);
                    let projection =
                        CelestialFrameProjection::build(&world, ns(namespace)).unwrap();
                    (world, projection)
                },
                |(mut world, mut projection)| {
                    world
                        .insert_body("appended", BodyProperties::new(1.0, 1.0).unwrap(), state())
                        .unwrap();
                    projection.publish(&world).unwrap();
                    black_box((world, projection))
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}
criterion_group!(projection_benches, benches);
criterion_main!(projection_benches);
