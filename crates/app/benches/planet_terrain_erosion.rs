use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use glam::DVec3;
use mundaris_app::{gravity_fixtures::GravityFixture, planet_terrain::*};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use mundaris_world::terrain::*;
use std::{
    hint::black_box,
    num::NonZeroU64,
    time::{Duration, Instant},
};

const RADIUS: f64 = 6_371_000.0;
const RHOS: [f64; 7] = [50_000.0, 10_000.0, 1_000.0, 100.0, 10.0, 2.0, 0.0];

fn main_benchmarks(c: &mut Criterion) {
    let v1 = checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V1).unwrap();
    let v2 = checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V2).unwrap();
    let v1 = TerrainGenerator::new(&v1, RADIUS).unwrap();
    let v2 = TerrainGenerator::new(&v2, RADIUS).unwrap();
    let directions = (0..289)
        .map(|i| {
            SurfaceLocation::new(
                Direction3::try_new(DVec3::new(1.0, i as f64 / 289.0, 0.3)).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let mut output = vec![TerrainSample::default(); 289];
    let mut group = c.benchmark_group("app_live_terrain_erosion");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    for (version, field, footprints) in [("v1", &v1, &[0.0][..]), ("v2", &v2, &RHOS[..])] {
        for &rho in footprints {
            let footprint = TerrainFootprint::new(rho).unwrap();
            group.throughput(Throughput::Elements(1));
            group.bench_function(format!("{version}_scalar_rho{rho}"), |b| {
                b.iter(|| {
                    black_box(
                        field
                            .evaluate_point(TerrainQuery {
                                location: black_box(directions[0]),
                                footprint,
                            })
                            .unwrap(),
                    )
                })
            });
            group.throughput(Throughput::Elements(32));
            group.bench_function(format!("{version}_batch32_rho{rho}"), |b| {
                b.iter(|| {
                    field
                        .evaluate_batch(
                            black_box(&directions[..32]),
                            footprint,
                            black_box(&mut output[..32]),
                        )
                        .unwrap();
                    black_box(&output[..32]);
                })
            });
            group.throughput(Throughput::Elements(289));
            group.bench_function(format!("{version}_batch289_rho{rho}"), |b| {
                b.iter(|| {
                    field
                        .evaluate_batch(black_box(&directions), footprint, black_box(&mut output))
                        .unwrap();
                    black_box(&output);
                })
            });
        }
    }
    for (version, field) in [("v1", &v1), ("v2", &v2)] {
        for rho in RHOS {
            let report = field
                .evaluate_batch(
                    &directions,
                    TerrainFootprint::new(rho).unwrap(),
                    &mut output,
                )
                .unwrap();
            eprintln!(
                "live terrain {version} rho={rho} primitives={} features={} active={} faded={} skipped={}",
                report.primitive_calls(),
                report.erosion_feature_evaluations(),
                report.active_erosion_octaves(),
                report.faded_erosion_octaves(),
                report.skipped_erosion_octaves()
            );
        }
    }
    group.finish();

    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(81).unwrap())
        .unwrap();
    let body = world.bodies().nth(1).unwrap().0;
    assert_eq!(
        world.body(body).unwrap().properties().reference_radius_m(),
        RADIUS
    );
    let identity = TerrainGeometryIdentity::new(
        body,
        checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V2).unwrap(),
        TerrainRevision::default(),
        RADIUS,
    )
    .unwrap();
    let levels = [0, 4, 6, 10, 13, 16, 18, 19];
    let addresses =
        levels.map(|level| CubePatchAddress::try_new(CubeFace::PositiveX, level, 0, 0).unwrap());
    let mut patches = c.benchmark_group("app_live_terrain_patch");
    patches
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    patches.throughput(Throughput::Elements(289));
    for (level, address) in levels.into_iter().zip(addresses) {
        patches.bench_function(
            format!("complete_cache_miss_geometry289_level{level}"),
            |b| {
                b.iter_batched(
                    || TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap(),
                    |mut cache| {
                        cache.request(&identity, address);
                        while cache.peek(&identity, address).is_none() {
                            let report = cache.generate(289, GENERATION_MICROBATCH, None).unwrap();
                            assert!(
                                report.vertices_generated != 0 || report.patches_completed != 0,
                                "generation stalled"
                            );
                        }
                        black_box((cache.peek(&identity, address), cache.report()));
                        drop(cache);
                    },
                    criterion::BatchSize::LargeInput,
                )
            },
        );
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap();
        cache.request(&identity, address);
        while cache.peek(&identity, address).is_none() {
            cache.generate(289, GENERATION_MICROBATCH, None).unwrap();
        }
        let report = cache.report();
        eprintln!(
            "live terrain patch level={level} rho={} bytes={} peak_bytes={} patches={}",
            RADIUS * 2.0 / (16.0 * (1u64 << level) as f64),
            report.resident_bytes,
            report.peak_bytes,
            report.resident_patches
        );
    }
    patches.finish();

    for level in [4, 18] {
        for budget in [8, 16, 32, 64].map(|n| n.min(MAX_GENERATION_BATCH)) {
            let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 32).unwrap();
            let mut durations = Vec::new();
            let mut bytes = Vec::new();
            for index in 0..32 {
                let address = CubePatchAddress::try_new(
                    CubeFace::PositiveX,
                    level,
                    index % (1 << level),
                    index / (1 << level),
                )
                .unwrap();
                cache.request(&identity, address);
                while cache.peek(&identity, address).is_none() {
                    let start = Instant::now();
                    let report = cache.generate(budget, budget, None).unwrap();
                    durations.push(start.elapsed());
                    bytes.push(report.allocation_delta_bytes);
                }
            }
            durations.sort();
            bytes.sort();
            eprintln!(
                "live terrain chunks level={level} budget={budget} chunks={} median={:?} worst={:?} median_bytes={} worst_bytes={}",
                durations.len(),
                durations[durations.len() / 2],
                durations[durations.len() - 1],
                bytes[bytes.len() / 2],
                bytes[bytes.len() - 1]
            );
        }
    }
}

criterion_group! {name=terrain_erosion; config=Criterion::default(); targets=main_benchmarks}
criterion_main!(terrain_erosion);
