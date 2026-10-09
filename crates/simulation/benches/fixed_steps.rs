mod common;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use astrum_math::SimulationInstant;
use astrum_simulation::*;
use astrum_world::BodyStateUpdate;
use std::{hint::black_box, time::Duration};
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("fixed_steps");
    for n in [3, 16, 64, 256, 1024] {
        let mut system = common::fixture(n);
        let mut workspace = IntegrationWorkspace::new(&system, 0.001).unwrap();
        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("candidate/N{n}"), |b| {
            b.iter(|| {
                black_box(workspace.prepare_step().unwrap());
                black_box(&workspace);
            })
        });
        group.bench_function(format!("initialization_force_and_gather/N{n}"), |b| {
            b.iter(|| {
                black_box(IntegrationWorkspace::new(black_box(&system), 0.001).unwrap());
            })
        });
        let mut updates: Vec<_> = system
            .bodies()
            .map(|(body, b)| BodyStateUpdate {
                body,
                state: *b.state(),
            })
            .collect();
        let mut masses = vec![0.0; n];
        group.bench_function(format!("dense_gather/N{n}"), |b| {
            b.iter(|| {
                for (i, (id, body)) in black_box(&system).bodies().enumerate() {
                    updates[i] = BodyStateUpdate {
                        body: id,
                        state: *body.state(),
                    };
                    masses[i] = body.properties().mass_kg();
                }
                black_box(&updates);
                black_box(&masses);
            })
        });
        let mut time = 0.0;
        group.bench_function(format!("new_time_world_publication/N{n}"), |b| {
            b.iter(|| {
                time += 0.001;
                system
                    .update_states(
                        black_box(SimulationInstant::try_seconds_since_epoch(time).unwrap()),
                        black_box(&updates),
                    )
                    .unwrap();
                black_box(&system);
            })
        });
        // Timed physical batches use matching world/cache revisions, prepared outside timing.
        let mut system = common::fixture(n);
        let mut workspace = IntegrationWorkspace::new(&system, 0.001).unwrap();
        let mut tick = 0u64;
        group.throughput(Throughput::Elements(512));
        group.bench_function(format!("committed_512_no_history/N{n}"), |b| {
            b.iter(|| {
                for _ in 0..512 {
                    tick += 1;
                    workspace.prepare_step().unwrap();
                    workspace
                        .commit_candidate(
                            &mut system,
                            SimulationInstant::try_seconds_since_epoch(tick as f64 * 0.001)
                                .unwrap(),
                        )
                        .unwrap();
                }
                black_box(&system);
            })
        });
        let mut system = common::fixture(n);
        let mut runner =
            FixedStepRunner::new(&system, SimulationConfig::try_new(0.001).unwrap()).unwrap();
        group.bench_function(format!("committed_512_history/N{n}"), |b| {
            b.iter(|| {
                runner.request_forward_to_tick(runner.tick() + 512).unwrap();
                black_box(runner.pump(&mut system, |_, _| {}).unwrap());
                black_box(&runner);
                black_box(&system);
            })
        });
        let mut system = common::fixture(n);
        let mut runner =
            FixedStepRunner::new(&system, SimulationConfig::try_new(0.001).unwrap()).unwrap();
        group.bench_function(format!("private_replay_512/N{n}"), |b| {
            b.iter(|| {
                runner.seek_tick(1_000_000).unwrap();
                black_box(runner.pump(&mut system, |_, _| {}).unwrap());
                black_box(&runner);
                black_box(&system);
            })
        });
        group.throughput(Throughput::Elements(2));
        group.bench_function(format!("retained_restore_roundtrip/N{n}"), |b| {
            b.iter(|| {
                runner.cancel_seek();
                runner.single_step(true).unwrap();
                runner.pump(&mut system, |_, _| {}).unwrap();
                runner.single_step(false).unwrap();
                black_box(runner.pump(&mut system, |_, _| {}).unwrap());
                black_box(&system);
            })
        });
        group.throughput(Throughput::Elements(1));
        group.bench_function(format!("diagnostics/N{n}"), |b| {
            b.iter(|| {
                black_box(system_diagnostics(black_box(&system)).unwrap());
            })
        });
    }
    group.finish();
    let mut group = c.benchmark_group("bounded_single_work_stress");
    for n in [64, 256, 1024] {
        let mut system = common::fixture(n);
        let mut runner =
            FixedStepRunner::new(&system, SimulationConfig::try_new(0.001).unwrap()).unwrap();
        group.bench_function(format!("N{n}"), |b| {
            b.iter(|| {
                runner.request_forward_to_tick(runner.tick() + 1).unwrap();
                black_box(
                    runner
                        .pump_with_work_limit(&mut system, 1, |_, _| {})
                        .unwrap(),
                );
                black_box(&system);
            })
        });
    }
    group.finish();
}
criterion_group! {name=fixed;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(fixed);
