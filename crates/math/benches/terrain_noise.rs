use criterion::{Criterion, SamplingMode, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::noise::{gradient_noise, gradient_noise_value};

fn noise(c: &mut Criterion) {
    let mut group = c.benchmark_group("terrain_noise");
    group
        .sample_size(20)
        .measurement_time(std::time::Duration::from_millis(500))
        .warm_up_time(std::time::Duration::from_millis(100))
        .sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(1));
    group.bench_function("value_only", |b| {
        let mut i = 0u64;
        b.iter(|| {
            let p = DVec3::new(
                i as f64 * 0.013,
                (i % 101) as f64 * 0.019,
                (i % 37) as f64 * 0.027,
            );
            std::hint::black_box(
                gradient_noise_value(
                    std::hint::black_box(i.wrapping_mul(17)),
                    std::hint::black_box(p),
                )
                .unwrap(),
            );
            i = i.wrapping_add(1);
        });
    });
    group.bench_function("gradient_value_and_derivatives", |b| {
        let mut i = 0u64;
        b.iter(|| {
            let p = DVec3::new(
                i as f64 * 0.013,
                (i % 101) as f64 * 0.019,
                (i % 37) as f64 * 0.027,
            );
            let s = gradient_noise(
                std::hint::black_box(i.wrapping_mul(17)),
                std::hint::black_box(p),
            )
            .unwrap();
            std::hint::black_box(s);
            i = i.wrapping_add(1);
        });
    });
    group.throughput(Throughput::Elements(289));
    group.bench_function("patch_289_scalar_samples", |b| {
        b.iter(|| {
            for index in 0..289 {
                let x = (index % 17) as f64;
                let y = (index / 17) as f64;
                std::hint::black_box(
                    gradient_noise(
                        std::hint::black_box(17),
                        std::hint::black_box(DVec3::new(x * 0.03125, y * 0.03125, 0.5)),
                    )
                    .unwrap(),
                );
            }
        });
    });
    group.throughput(Throughput::Elements(32));
    group.bench_function("patch_32_sample_batch", |b| {
        b.iter(|| {
            for index in 0..32 {
                let x = (index % 8) as f64;
                let y = (index / 8) as f64;
                std::hint::black_box(
                    gradient_noise(
                        std::hint::black_box(17),
                        std::hint::black_box(DVec3::new(x * 0.125, y * 0.125, 0.5)),
                    )
                    .unwrap(),
                );
            }
        });
    });
    group.finish();
}
criterion_group!(benches, noise);
criterion_main!(benches);
