use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::{
    Direction3,
    surface::{DirectionalCap, SurfaceLocation},
};
use mundaris_world::terrain::*;
use std::{hint::black_box, time::Duration};

fn terrain(c: &mut Criterion) {
    eprintln!(
        "analytic fixtures: R=6371000m, A=1000m, V1 bands/primitive calls=0, generated patches=0, caller outputs=289; native bytes definition={}, config={}, sample={}, batch_payload={}",
        std::mem::size_of::<TerrainDefinition>(),
        std::mem::size_of::<TerrainConfig>(),
        std::mem::size_of::<TerrainSample>(),
        289 * std::mem::size_of::<TerrainSample>()
    );
    let axis = Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap();
    let field = AnalyticTerrain::linear(6.371e6, 1000.0, axis).unwrap();
    let flat = AnalyticTerrain::constant(6.371e6, 0.0).unwrap();
    let locations: Vec<_> = (0..289)
        .map(|i| {
            SurfaceLocation::new(
                Direction3::try_new(DVec3::new(1.0, i as f64 / 289.0, 0.3)).unwrap(),
            )
        })
        .collect();
    let mut output = vec![TerrainSample::default(); 289];
    let mut group = c.benchmark_group("terrain_analytic_foundation");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    group.throughput(Throughput::Elements(1));
    for rho in [50_000.0, 2.0] {
        let query = TerrainQuery {
            location: locations[0],
            footprint: TerrainFootprint::new(rho).unwrap(),
        };
        group.bench_function(format!("linear_scalar_rho_{rho}"), |b| {
            b.iter(|| black_box(field).evaluate_point(black_box(query)))
        });
    }
    let sample = field.evaluate_point(TerrainQuery {
        location: locations[0],
        footprint: TerrainFootprint::COMPLETE,
    });
    group.bench_function("normal_only", |b| {
        b.iter(|| {
            black_box(sample)
                .normal_body(black_box(locations[0]), black_box(field.radius_m()))
                .unwrap()
        })
    });
    let cap = DirectionalCap::new(axis, 0.1).unwrap();
    group.bench_function("cap_certificate", |b| {
        b.iter(|| black_box(field).bounds_for_region(black_box(cap)))
    });
    group.throughput(Throughput::Elements(289));
    for (name, terrain) in [("flat", flat), ("linear", field)] {
        group.bench_function(format!("{name}_batch289"), |b| {
            b.iter(|| {
                terrain
                    .evaluate_batch(
                        black_box(&locations),
                        TerrainFootprint::COMPLETE,
                        black_box(&mut output),
                    )
                    .unwrap();
                black_box(&output);
            })
        });
    }
    group.throughput(Throughput::Elements(32));
    group.bench_function("linear_chunk32", |b| {
        b.iter(|| {
            field
                .evaluate_batch(
                    black_box(&locations[..32]),
                    TerrainFootprint::COMPLETE,
                    black_box(&mut output[..32]),
                )
                .unwrap();
            black_box(&output[..32]);
        })
    });
    group.finish();
}
criterion_group!(benches, terrain);
criterion_main!(benches);
