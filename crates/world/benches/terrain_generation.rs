use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::*;
use std::{hint::black_box, time::Duration};

fn definition() -> TerrainDefinition {
    let bands = [
        TerrainBandConfig::new(
            1800.0,
            TerrainScale::Angular {
                lowest_cycles_per_body: 2.0,
            },
            3,
        )
        .unwrap(),
        TerrainBandConfig::new(
            2400.0,
            TerrainScale::Metres {
                longest_wavelength_m: 800_000.0,
            },
            3,
        )
        .unwrap(),
        TerrainBandConfig::new(
            800.0,
            TerrainScale::Metres {
                longest_wavelength_m: 64_000.0,
            },
            3,
        )
        .unwrap(),
        TerrainBandConfig::new(
            140.0,
            TerrainScale::Metres {
                longest_wavelength_m: 2_000.0,
            },
            3,
        )
        .unwrap(),
        TerrainBandConfig::new(
            2.0,
            TerrainScale::Metres {
                longest_wavelength_m: 32.0,
            },
            2,
        )
        .unwrap(),
    ];
    let controls = TerrainControls::new(0.1, 2.0, 0.48, 0.8, 0.3, 0.08).unwrap();
    TerrainDefinition::new(
        TerrainIdentity(17),
        TerrainSeed(0x5eed),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(bands, controls).unwrap(),
    )
}

fn terrain(c: &mut Criterion) {
    let generator = TerrainGenerator::new(&definition(), 6_371_000.0).unwrap();
    let locations: Vec<_> = (0..289)
        .map(|i| {
            SurfaceLocation::new(
                Direction3::try_new(DVec3::new(1.0, i as f64 / 289.0, 0.3)).unwrap(),
            )
        })
        .collect();
    let mut output = vec![TerrainSample::default(); 289];
    let axis = Direction3::try_new(DVec3::new(1.0, 2.0, 3.0)).unwrap();
    let analytic = AnalyticTerrain::linear(6.371e6, 1000.0, axis).unwrap();
    let flat = AnalyticTerrain::constant(6.371e6, 0.0).unwrap();
    let analytic_query = TerrainQuery {
        location: locations[0],
        footprint: TerrainFootprint::COMPLETE,
    };
    let mut foundation = c.benchmark_group("terrain_analytic_foundation");
    foundation
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    foundation.throughput(Throughput::Elements(1));
    for rho in [50_000.0, 2.0] {
        foundation.bench_function(format!("linear_scalar_rho_{rho}"), |b| {
            b.iter(|| {
                black_box(analytic).evaluate_point(black_box(TerrainQuery {
                    footprint: TerrainFootprint::new(rho).unwrap(),
                    ..analytic_query
                }))
            })
        });
    }
    let sample = analytic.evaluate_point(analytic_query);
    foundation.bench_function("normal_only", |b| {
        b.iter(|| {
            black_box(sample)
                .normal_body(black_box(locations[0]), 6.371e6)
                .unwrap()
        })
    });
    let cap = mundaris_math::surface::DirectionalCap::new(axis, 0.1).unwrap();
    foundation.bench_function("cap_certificate", |b| {
        b.iter(|| black_box(analytic).bounds_for_region(black_box(cap)))
    });
    foundation.throughput(Throughput::Elements(289));
    for (name, terrain) in [("flat_batch289", flat), ("linear_batch289", analytic)] {
        foundation.bench_function(name, |b| {
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
    foundation.finish();
    let mut group = c.benchmark_group("terrain_v1_generation");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    let full = generator
        .evaluate_point(TerrainQuery {
            location: locations[0],
            footprint: TerrainFootprint::COMPLETE,
        })
        .unwrap();
    group.throughput(Throughput::Elements(1));
    group.bench_function("v1_complete_scalar_elevation_and_derivatives", |b| {
        b.iter(|| {
            black_box(
                generator
                    .evaluate_point(black_box(TerrainQuery {
                        location: locations[0],
                        footprint: TerrainFootprint::COMPLETE,
                    }))
                    .unwrap(),
            )
        })
    });
    group.bench_function("v1_complete_scalar_normal", |b| {
        b.iter(|| {
            black_box(
                full.normal_body(black_box(locations[0]), 6_371_000.0)
                    .unwrap(),
            )
        })
    });
    for rho in [50_000.0, 1_000.0, 128.0, 2.0, 0.0] {
        let footprint = TerrainFootprint::new(rho).unwrap();
        group.throughput(Throughput::Elements(289));
        group.bench_function(format!("v1_filtered_batch289_rho_{rho:.0}m"), |b| {
            b.iter(|| {
                let report = generator
                    .evaluate_batch(black_box(&locations), footprint, black_box(&mut output))
                    .unwrap();
                black_box((report.primitive_calls(), &output));
            })
        });
        let report = generator
            .evaluate_batch(&locations, footprint, &mut output)
            .unwrap();
        eprintln!(
            "V1 profile rho={rho:.0}m: samples={}, actual primitive calls={}, calls/sample={:.2}, output bytes={}",
            report.sample_count(),
            report.primitive_calls(),
            report.primitive_calls() as f64 / 289.0,
            289 * std::mem::size_of::<TerrainSample>()
        );
    }
    group.finish();
    let report = generator
        .evaluate_batch(&locations, TerrainFootprint::COMPLETE, &mut output)
        .unwrap();
    eprintln!(
        "V1 complete batch: actual primitive calls={}, samples={}, TerrainSample bytes={}",
        report.primitive_calls(),
        report.sample_count(),
        std::mem::size_of::<TerrainSample>()
    );
}

fn legacy_analytic(c: &mut Criterion) {
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
    let mut group = c.benchmark_group("terrain_analytic_foundation_whole_batch");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    group.throughput(Throughput::Elements(289));
    for (name, terrain) in [
        ("flat_analytic_whole_batch289", flat),
        ("linear_analytic_whole_batch289", field),
    ] {
        group.bench_function(name, |b| {
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
    group.finish();
}

criterion_group! {name=benches; config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500)); targets=terrain, legacy_analytic}
criterion_main!(benches);
