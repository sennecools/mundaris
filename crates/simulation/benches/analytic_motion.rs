use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::*;
use mundaris_simulation::*;
use mundaris_world::*;
use std::{f64::consts::TAU, hint::black_box, num::NonZeroU64, time::Duration};

fn fixture(count: usize) -> (CelestialSystem, AnalyticMotionProducer) {
    let mut world = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    let ids: Vec<_> = (0..count)
        .map(|_| {
            world
                .insert_body("body", BodyProperties::new(1.0, 1.0).unwrap(), state)
                .unwrap()
        })
        .collect();
    let spin = AxialSpin::new(
        UnitRotation::identity(),
        Direction3::try_new(DVec3::Z).unwrap(),
        TAU / 86400.0,
        SimulationInstant::ZERO,
    )
    .unwrap();
    let definitions: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, &body)| BodyMotion {
            body,
            spin,
            translation: if i == 0 {
                CelestialTranslation::Stationary(LocalPosition::origin())
            } else {
                CelestialTranslation::Elliptic(
                    EllipticOrbit::new(
                        ids[0],
                        1.5e11 + i as f64 * 1e8,
                        0.6,
                        UnitRotation::identity(),
                        31_557_600.0 + i as f64 * 86400.0,
                        0.0,
                        SimulationInstant::ZERO,
                    )
                    .unwrap(),
                )
            },
        })
        .collect();
    let definition = CelestialMotionDefinition::new(&world, &definitions).unwrap();
    let producer = AnalyticMotionProducer::new(&world, definition).unwrap();
    (world, producer)
}
fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("analytic_complete_sample_commit");
    group
        .sample_size(30)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    for count in [10, 100, 1000] {
        group.throughput(Throughput::Elements(count as u64));
        for (label, seconds) in [
            ("near", 7_889_400.0),
            ("plus_1000_years", 31_557_600_000.0),
            ("minus_1000_years", -31_557_600_000.0),
        ] {
            let (mut world, mut producer) = fixture(count);
            let times = [
                SimulationInstant::try_seconds_since_epoch(seconds).unwrap(),
                SimulationInstant::try_seconds_since_epoch(seconds + 1234.0).unwrap(),
            ];
            let stats = producer.sample(&mut world, times[0]).unwrap();
            eprintln!(
                "fixture={label} bodies={count} time_s={seconds} iterations_total={} iterations_max={} scope=complete_candidate_plus_world_commit profile=release",
                stats.solver_iterations, stats.maximum_solver_iterations
            );
            let mut i = 0;
            group.bench_with_input(BenchmarkId::new(label, count), &count, |b, _| {
                b.iter(|| {
                    i ^= 1;
                    black_box(
                        producer
                            .sample(black_box(&mut world), black_box(times[i]))
                            .unwrap(),
                    )
                })
            });
        }
    }
    group.finish();
}
criterion_group!(benches, benchmarks);
criterion_main!(benches);
