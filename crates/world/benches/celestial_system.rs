use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use astrum_math::*;
use astrum_world::*;
use std::{hint::black_box, num::NonZeroU64};

fn fixture(count: usize) -> CelestialSystem {
    let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    for _ in 0..count {
        world
            .insert_body(
                "body",
                BodyProperties::new(1e24, 1e6).unwrap(),
                BodyState::new(
                    LocalPosition::origin(),
                    LinearVelocity3::zero(),
                    UnitRotation::identity(),
                    AngularVelocity3::zero(),
                ),
            )
            .unwrap();
    }
    world
}
fn benches(c: &mut Criterion) {
    let mut lookup = c.benchmark_group("body_lookup_edit");
    for count in [64, 1024, 16384] {
        let mut world = fixture(count);
        let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
        lookup.throughput(Throughput::Elements(count as u64));
        lookup.bench_with_input(BenchmarkId::new("lookup", count), &count, |b, _| {
            b.iter(|| {
                for &id in &ids {
                    black_box(world.body(black_box(id)).unwrap());
                }
            })
        });
        lookup.bench_with_input(BenchmarkId::new("edit", count), &count, |b, _| {
            b.iter(|| {
                for &id in &ids {
                    world
                        .edit_properties(id, black_box(BodyProperties::new(2e24, 1e6).unwrap()))
                        .unwrap();
                }
                black_box(&world);
            })
        });
    }
    lookup.finish();
    let mut group = c.benchmark_group("full_state_publication");
    for count in [3, 16, 64, 256, 1024, 4096] {
        let mut world = fixture(count);
        let updates: Vec<_> = world
            .bodies()
            .map(|(body, record)| BodyStateUpdate {
                body,
                state: *record.state(),
            })
            .collect();
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                world
                    .update_states(black_box(SimulationInstant::ZERO), black_box(&updates))
                    .unwrap();
                black_box(&world);
            })
        });
    }
    group.finish();
    let mut group = c.benchmark_group("new_time_full_state_publication");
    for count in [3, 16, 64, 256, 1024] {
        let mut world = fixture(count);
        let mut time = 0.0;
        let updates: Vec<_> = world
            .bodies()
            .map(|(body, b)| BodyStateUpdate {
                body,
                state: *b.state(),
            })
            .collect();
        group.throughput(Throughput::Elements(count as u64));
        group.bench_with_input(BenchmarkId::from_parameter(count), &count, |b, _| {
            b.iter(|| {
                time += 1.0;
                world
                    .update_states(
                        black_box(SimulationInstant::try_seconds_since_epoch(time).unwrap()),
                        black_box(&updates),
                    )
                    .unwrap();
                black_box(&world);
            })
        });
    }
    group.finish();
}
criterion_group!(world_benches, benches);
criterion_main!(world_benches);
