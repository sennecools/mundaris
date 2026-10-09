use astrum_app::engine_profile::{self, ProfileSnapshot, span};
use serde::Serialize;
use std::{env, hint::black_box, process, time::Instant};

const REPEATS: usize = 5;
const WARMUP_ITERATIONS: usize = 10_000;

#[derive(Serialize)]
struct TimingSummary {
    p50_ns: u64,
    p95_ns: u64,
    max_ns: u64,
}

#[derive(Serialize)]
struct LoopMeasurements {
    total_wall: TimingSummary,
    p50_wall_ns_per_iteration: f64,
    p95_wall_ns_per_iteration: f64,
    max_wall_ns_per_iteration: f64,
}

#[derive(Serialize)]
struct SnapshotMeasurements {
    collection_wall: TimingSummary,
    serialization_wall: TimingSummary,
    serialized_bytes: usize,
}

#[derive(Serialize)]
struct DerivedMeasurements {
    enabled_to_disabled_ratio: Option<f64>,
    disabled_incremental_ns_per_span: f64,
    enabled_incremental_ns_per_span: f64,
    estimated_extra_ns_per_frame: f64,
    estimated_extra_ms_per_frame: f64,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    build_profile: &'static str,
    iterations_per_sample: usize,
    warmup_iterations_per_mode: usize,
    repeats: usize,
    spans_per_estimated_frame: usize,
    timing_scope: &'static str,
    baseline: LoopMeasurements,
    disabled: LoopMeasurements,
    enabled: LoopMeasurements,
    snapshot: SnapshotMeasurements,
    derived: DerivedMeasurements,
    last_profile: ProfileSnapshot,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("profile overhead measurement failed: {error}");
        process::exit(2);
    }
}

fn run() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let iterations = parse_arg(args.next(), 200_000, "iterations")?;
    let spans_per_frame = parse_arg(args.next(), 100, "spans_per_frame")?;
    if iterations == 0 || spans_per_frame == 0 {
        anyhow::bail!("iterations and spans_per_frame must be greater than zero");
    }
    if args.next().is_some() {
        anyhow::bail!("usage: profile_overhead [iterations] [spans_per_frame]");
    }

    engine_profile::set_enabled(false);
    let baseline = measure_loop(iterations, || baseline_loop(iterations));

    engine_profile::set_enabled(false);
    warmup_disabled();
    let disabled = measure_loop(iterations, || {
        span_loop("profile_overhead.disabled", iterations)
    });

    engine_profile::set_enabled(true);
    engine_profile::begin_frame(1);
    warmup_enabled();
    let enabled = measure_loop(iterations, || {
        span_loop("profile_overhead.enabled", iterations)
    });
    engine_profile::set_enabled(false);

    let (snapshot, snapshot_measurements) = measure_snapshot()?;
    let baseline_ns = baseline.p50_wall_ns_per_iteration;
    let disabled_ns = disabled.p50_wall_ns_per_iteration;
    let enabled_ns = enabled.p50_wall_ns_per_iteration;
    let enabled_incremental = (enabled_ns - baseline_ns).max(0.0);
    let extra_per_frame = enabled_incremental * spans_per_frame as f64;
    let report = Report {
        schema_version: 1,
        build_profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        iterations_per_sample: iterations,
        warmup_iterations_per_mode: WARMUP_ITERATIONS,
        repeats: REPEATS,
        spans_per_estimated_frame: spans_per_frame,
        timing_scope: "single-thread CPU wall time; loop and black_box cost included",
        baseline,
        disabled,
        enabled,
        snapshot: snapshot_measurements,
        derived: DerivedMeasurements {
            enabled_to_disabled_ratio: if disabled_ns > 0.0 {
                Some(enabled_ns / disabled_ns)
            } else {
                None
            },
            disabled_incremental_ns_per_span: disabled_ns - baseline_ns,
            enabled_incremental_ns_per_span: enabled_ns - baseline_ns,
            estimated_extra_ns_per_frame: extra_per_frame,
            estimated_extra_ms_per_frame: extra_per_frame / 1_000_000.0,
        },
        last_profile: snapshot,
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_arg<T>(value: Option<String>, default: T, label: &str) -> anyhow::Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match value {
        Some(value) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid {label}: {error}")),
        None => Ok(default),
    }
}

fn measure_loop(iterations: usize, mut run_once: impl FnMut() -> u64) -> LoopMeasurements {
    let mut total_ns = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let elapsed = run_once();
        total_ns.push(elapsed);
    }
    LoopMeasurements {
        total_wall: summarize(&total_ns),
        p50_wall_ns_per_iteration: percentile_float(&total_ns, 50) / iterations as f64,
        p95_wall_ns_per_iteration: percentile_float(&total_ns, 95) / iterations as f64,
        max_wall_ns_per_iteration: total_ns.iter().copied().max().unwrap_or(0) as f64
            / iterations as f64,
    }
}

fn baseline_loop(iterations: usize) -> u64 {
    let started = Instant::now();
    let mut accumulator = 0usize;
    for index in 0..iterations {
        accumulator = accumulator.wrapping_add(black_box(index));
    }
    black_box(accumulator);
    elapsed_ns(started)
}

fn span_loop(name: &'static str, iterations: usize) -> u64 {
    let started = Instant::now();
    for _ in 0..iterations {
        black_box(span(name));
    }
    elapsed_ns(started)
}

fn warmup_disabled() {
    for _ in 0..WARMUP_ITERATIONS {
        black_box(span("profile_overhead.disabled"));
    }
}

fn warmup_enabled() {
    for _ in 0..WARMUP_ITERATIONS {
        black_box(span("profile_overhead.enabled"));
    }
}

fn measure_snapshot() -> anyhow::Result<(ProfileSnapshot, SnapshotMeasurements)> {
    let mut collection_ns = Vec::with_capacity(REPEATS);
    let mut serialization_ns = Vec::with_capacity(REPEATS);
    let mut serialized_bytes = 0;
    let mut last_snapshot = engine_profile::snapshot();
    black_box(serde_json::to_vec(&last_snapshot)?);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let snapshot = engine_profile::snapshot();
        collection_ns.push(elapsed_ns(started));

        let started = Instant::now();
        let bytes = serde_json::to_vec(&snapshot)?;
        serialization_ns.push(elapsed_ns(started));
        serialized_bytes = bytes.len();
        black_box(bytes);
        last_snapshot = snapshot;
    }
    Ok((
        last_snapshot,
        SnapshotMeasurements {
            collection_wall: summarize(&collection_ns),
            serialization_wall: summarize(&serialization_ns),
            serialized_bytes,
        },
    ))
}

fn summarize(samples: &[u64]) -> TimingSummary {
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    TimingSummary {
        p50_ns: percentile(&ordered, 50),
        p95_ns: percentile(&ordered, 95),
        max_ns: ordered.last().copied().unwrap_or(0),
    }
}

fn percentile(ordered: &[u64], percentile: usize) -> u64 {
    if ordered.is_empty() {
        return 0;
    }
    let rank = ordered
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .max(1);
    ordered[rank - 1]
}

fn percentile_float(samples: &[u64], percentile_rank: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut ordered = samples.to_vec();
    ordered.sort_unstable();
    percentile(&ordered, percentile_rank) as f64
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}
