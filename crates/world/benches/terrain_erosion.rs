use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::*;
use std::{hint::black_box, time::Duration};

fn definition(version: TerrainGeneratorVersion, erosion_octaves: u8) -> TerrainDefinition {
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
    let config = TerrainConfig::new(bands, controls)
        .unwrap()
        .with_erosion(ErosionConfig::new(erosion_octaves, 1.0).unwrap());
    TerrainDefinition::new(TerrainIdentity(17), TerrainSeed(0x5eed), version, config)
}

fn terrain_erosion(c: &mut Criterion) {
    let locations: Vec<_> = (0..289)
        .map(|i| {
            SurfaceLocation::new(
                Direction3::try_new(DVec3::new(1.0, i as f64 / 289.0, 0.3)).unwrap(),
            )
        })
        .collect();
    let footprints = [50_000.0, 10_000.0, 1_000.0, 100.0, 10.0, 2.0];
    let mut group = c.benchmark_group("terrain_erosion_v1_v2");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));

    for version in [TerrainGeneratorVersion::V1, TerrainGeneratorVersion::V2] {
        // The V2 default is three octaves; include it explicitly as the production case.
        let cases = if version == TerrainGeneratorVersion::V2 {
            vec![
                (1, "configured"),
                (2, "configured"),
                (3, "production_default3"),
                (4, "configured"),
                (5, "configured"),
            ]
        } else {
            vec![(1, "legacy")]
        };
        for (count, label) in cases {
            let generator =
                TerrainGenerator::new(&definition(version, count), 6_371_000.0).unwrap();
            let cases = if version == TerrainGeneratorVersion::V2 && count == 3 {
                footprints.into_iter().chain([0.0]).collect::<Vec<_>>()
            } else {
                vec![0.0]
            };
            for rho in cases {
                let footprint = TerrainFootprint::new(rho).unwrap();
                let mut output32 = vec![TerrainSample::default(); 32];
                let mut output289 = vec![TerrainSample::default(); 289];
                let batch32 = &locations[..32];
                let batch289 = &locations[..];
                let name = format!("{version:?}_{label}_erosion{count}_rho{rho:.0}m");

                // The report is emitted outside timed loops; it counts actual gradient-noise
                // calls and feature callback evaluations, not allocations or profiler data.
                let report = generator
                    .evaluate_batch(batch289, footprint, &mut output289)
                    .unwrap();
                eprintln!(
                    "{name}: active/faded/skipped erosion octaves={}/{}/{}, noise primitives/sample={:.2}, features/sample={:.2}",
                    report.active_erosion_octaves(),
                    report.faded_erosion_octaves(),
                    report.skipped_erosion_octaves(),
                    report.primitive_calls() as f64 / report.sample_count() as f64,
                    report.erosion_feature_evaluations() as f64 / report.sample_count() as f64,
                );

                group.throughput(Throughput::Elements(1));
                group.bench_function(format!("{name}_scalar_complete_field"), |b| {
                    b.iter(|| {
                        black_box(
                            generator
                                .evaluate_point(black_box(TerrainQuery {
                                    location: locations[0],
                                    footprint,
                                }))
                                .unwrap(),
                        )
                    })
                });
                group.throughput(Throughput::Elements(32));
                group.bench_function(format!("{name}_batch32_complete_field"), |b| {
                    b.iter(|| {
                        generator
                            .evaluate_batch(black_box(batch32), footprint, black_box(&mut output32))
                            .unwrap();
                        black_box(&output32);
                    })
                });
                group.throughput(Throughput::Elements(289));
                group.bench_function(format!("{name}_batch289_complete_field"), |b| {
                    b.iter(|| {
                        generator
                            .evaluate_batch(
                                black_box(batch289),
                                footprint,
                                black_box(&mut output289),
                            )
                            .unwrap();
                        black_box(&output289);
                    })
                });
            }
        }
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    targets = terrain_erosion
}
criterion_main!(benches);
