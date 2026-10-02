use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use mundaris_app::planet_terrain::*;
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_world::terrain::*;
use std::{hint::black_box, num::NonZeroU64, time::Duration, time::Instant};

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
    TerrainDefinition::new(
        TerrainIdentity(17),
        TerrainSeed(0x5eed),
        TerrainGeneratorVersion::V1,
        TerrainConfig::new(
            bands,
            TerrainControls::new(0.1, 2.0, 0.48, 0.8, 0.3, 0.08).unwrap(),
        )
        .unwrap(),
    )
}

fn complete(
    cache: &mut TerrainPatchCache,
    identity: &TerrainGeometryIdentity,
    address: CubePatchAddress,
    budget: usize,
) {
    assert!(cache.request(identity, address));
    while cache.peek(identity, address).is_none() {
        let result = cache
            .generate(budget, budget.min(GENERATION_MICROBATCH), None)
            .unwrap();
        assert!(
            result.vertices_generated != 0 || result.patches_completed != 0,
            "generation stalled"
        );
    }
}

fn benches(c: &mut Criterion) {
    let world = mundaris_app::gravity_fixtures::GravityFixture::Hierarchy
        .create(NonZeroU64::new(81).unwrap())
        .unwrap();
    let body = world.bodies().nth(1).unwrap().0;
    let radius_m = world.body(body).unwrap().properties().reference_radius_m();
    let identity =
        TerrainGeometryIdentity::new(body, definition(), TerrainRevision::default(), radius_m)
            .unwrap();
    let addresses: Vec<_> = [0, 4, 10, 18]
        .into_iter()
        .map(|level| CubePatchAddress::try_new(CubeFace::PositiveX, level, 0, 0).unwrap())
        .collect();
    let mut group = c.benchmark_group("app_planet_terrain_generation");
    group
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    group.throughput(Throughput::Elements(289));
    for (&level, &address) in [0, 4, 10, 18].iter().zip(&addresses) {
        group.bench_function(
            format!("complete_cache_miss_geometry289_level{level}"),
            |b| {
                b.iter_batched(
                    || TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap(),
                    |mut cache| {
                        complete(&mut cache, &identity, address, 289);
                        let state = cache.report();
                        black_box((&cache.peek(&identity, address), state));
                    },
                    criterion::BatchSize::LargeInput,
                )
            },
        );
    }
    let address = addresses[2];
    let mut ready = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 8).unwrap();
    complete(&mut ready, &identity, address, 289);
    group.throughput(Throughput::Elements(1));
    group.bench_function("cache_hit", |b| {
        b.iter(|| {
            black_box(ready.get(black_box(&identity), black_box(address)).unwrap());
        })
    });
    group.bench_function("eviction_and_regeneration", |b| {
        b.iter_batched(
            || {
                let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 1).unwrap();
                complete(&mut cache, &identity, addresses[0], 289);
                cache
            },
            |mut cache| {
                complete(&mut cache, &identity, address, 289);
                let before = cache.report();
                complete(&mut cache, &identity, addresses[0], 289);
                black_box((cache.report(), before));
            },
            criterion::BatchSize::LargeInput,
        )
    });
    group.finish();

    let mut chunks = c.benchmark_group("app_planet_terrain_generation_chunk_budget");
    chunks
        .sample_size(20)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500));
    for budget in [1, 8, 16, 32] {
        chunks.bench_function(format!("generate_one_patch_batch{budget}"), |b| {
            b.iter_batched(
                || {
                    let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 2).unwrap();
                    cache.request(&identity, address);
                    cache
                },
                |mut cache| {
                    let mut generated = 0;
                    while generated < 289 {
                        generated += cache
                            .generate(budget, budget, None)
                            .unwrap()
                            .vertices_generated;
                    }
                    black_box((cache.report(), cache.peek(&identity, address)));
                },
                criterion::BatchSize::LargeInput,
            )
        });
    }
    chunks.finish();

    for budget in [1, 8, 16, 32] {
        let mut diagnostic = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 32).unwrap();
        let mut durations = Vec::new();
        let mut vertices = 0;
        let mut completed = 0;
        let mut max_delta = 0;
        for index in 0..32 {
            let patch = CubePatchAddress::try_new(CubeFace::PositiveX, 18, index, 0).unwrap();
            diagnostic.request(&identity, patch);
            while diagnostic.peek(&identity, patch).is_none() {
                let start = Instant::now();
                let report = diagnostic.generate(budget, budget, None).unwrap();
                durations.push(start.elapsed());
                vertices += report.vertices_generated;
                completed += report.patches_completed;
                max_delta = max_delta.max(report.allocation_delta_bytes);
            }
        }
        durations.sort();
        eprintln!(
            "chunk budget={budget}, level18 rho={:.6}m, chunks={}, vertices={vertices}, completed={completed}, median={:?}, worst={:?}, max_allocation_delta={max_delta}, cache={:?}",
            radius_m * 2.0 / (16.0 * 262144.0),
            durations.len(),
            durations[durations.len() / 2],
            durations.last().unwrap(),
            diagnostic.report()
        );
    }
    for (cap, count) in [
        (TERRAIN_CPU_CAP_BYTES, MAX_TERRAIN_PATCHES),
        (256 * 1024, 64),
    ] {
        let mut cache = TerrainPatchCache::new(cap, count).unwrap();
        let empty = cache.resident_bytes();
        for i in 0..(count + 256) {
            let patch = CubePatchAddress::try_new(
                CubeFace::PositiveX,
                7,
                (i % 128) as u32,
                (i / 128) as u32,
            )
            .unwrap();
            complete(&mut cache, &identity, patch, 289);
        }
        let report = cache.report();
        eprintln!(
            "traversal cap={cap}, count_cap={count}, empty_bookkeeping={empty}, incremental_bytes_per_patch={}, state={report:?}",
            (report.resident_bytes - empty) / report.resident_patches
        );
        assert!(report.resident_bytes <= cap);
    }
}

criterion_group! {name=terrain; config=Criterion::default().sample_size(20).warm_up_time(Duration::from_millis(100)).measurement_time(Duration::from_millis(500)); targets=benches}
criterion_main!(terrain);
