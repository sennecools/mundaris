mod common;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use astrum_simulation::*;
use std::{hint::black_box, time::Duration};
fn benches(c: &mut Criterion) {
    let mut group = c.benchmark_group("gravity_force_pass");
    for n in [3, 16, 64, 256, 1024] {
        let system = common::fixture(n);
        let masses: Vec<_> = system
            .bodies()
            .map(|(_, b)| b.properties().mass_kg())
            .collect();
        let positions: Vec<_> = system
            .bodies()
            .map(|(_, b)| b.state().center_in_system().metres())
            .collect();
        let mut output = vec![DVec3::ZERO; n];
        evaluate_resolved_accelerations(&masses, &positions, &mut output, 0.001).unwrap();
        group.throughput(Throughput::Elements(pair_count(n).unwrap()));
        group.bench_function(format!("N{n}"), |b| {
            b.iter(|| {
                black_box(
                    evaluate_accelerations(
                        black_box(&masses),
                        black_box(&positions),
                        black_box(&mut output),
                    )
                    .unwrap(),
                );
                black_box(&output);
            })
        });
    }
    group.finish();
}
criterion_group! {name=gravity;config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500));targets=benches}
criterion_main!(gravity);
