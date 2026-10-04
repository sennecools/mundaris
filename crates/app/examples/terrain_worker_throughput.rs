//! Bounded headless CPU throughput comparison for serial and worker terrain generation.
use anyhow::{Context, Result, ensure};
use mundaris_app::{planet_terrain::*, solar_system::*};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use std::{
    fs::File,
    io::{BufWriter, Write},
    num::NonZeroU64,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

const PATCHES_PER_FOOTPRINT: usize = 64;
const VERTICES_PER_PATCH: usize = 17 * 17;
const CASE_DEADLINE: Duration = Duration::from_secs(20);
const WORKER_COUNTS: [usize; 4] = [0, 1, 2, 4];
const LEVELS: [(&str, u8); 3] = [("coarse", 2), ("regional", 8), ("fine", 18)];
const FACES: [CubeFace; 6] = [
    CubeFace::PositiveX,
    CubeFace::NegativeX,
    CubeFace::PositiveY,
    CubeFace::NegativeY,
    CubeFace::PositiveZ,
    CubeFace::NegativeZ,
];

fn addresses(level: u8) -> Result<Vec<CubePatchAddress>> {
    let side = 1u32 << level;
    let mut result = Vec::with_capacity(PATCHES_PER_FOOTPRINT);
    // Interleave every face and spread x/y deterministically across the chart.
    for index in 0..PATCHES_PER_FOOTPRINT {
        let face = FACES[index % FACES.len()];
        let cell = (index / FACES.len()) as u32;
        let x = cell % side;
        let y = cell / side;
        result.push(CubePatchAddress::try_new(face, level, x, y)?);
    }
    Ok(result)
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = PathBuf::from(
        args.next()
            .context("usage: terrain_worker_throughput OUTPUT.csv")?,
    );
    ensure!(
        args.next().is_none(),
        "usage: terrain_worker_throughput OUTPUT.csv"
    );
    let world =
        SolarSystemPreset::gameplay().create(NonZeroU64::new(714).context("invalid namespace")?)?;
    let (body, state) = world
        .bodies()
        .nth(SolarBody::Earth as usize)
        .context("gameplay Earth missing")?;
    let identity = TerrainGeometryIdentity::new(
        body,
        state
            .terrain()
            .context("gameplay Earth terrain missing")?
            .clone(),
        state.terrain_revision(),
        state.properties().reference_radius_m(),
    )?;
    let mut csv = BufWriter::new(File::create(&output)?);
    writeln!(
        csv,
        "footprint,worker_count,patches,vertices,elapsed_ms,patches_per_second,vertices_per_second,main_update_median_ms,main_update_worst_ms,worker_cpu_ms,cancelled,peak_accounted_bytes"
    )?;
    println!(
        "footprint,workers,patches,vertices,elapsed_ms,patches_per_second,vertices_per_second,main_update_median_ms,main_update_worst_ms,worker_cpu_ms,cancelled,peak_accounted_bytes"
    );

    for (footprint, level) in LEVELS {
        let addresses = addresses(level)?;
        let vertices = PATCHES_PER_FOOTPRINT * VERTICES_PER_PATCH;
        for workers in WORKER_COUNTS {
            let mut cache =
                TerrainPatchCache::new_with_workers(TERRAIN_CPU_CAP_BYTES, 256, workers)?;
            for &address in &addresses {
                ensure!(
                    cache.request(&identity, address),
                    "terrain request rejected"
                );
            }
            let mut updates = Vec::new();
            let mut worker_cpu = Duration::ZERO;
            let start = Instant::now();
            let deadline = start + CASE_DEADLINE;
            if workers == 0 {
                let update_start = Instant::now();
                let work = cache.generate(vertices, GENERATION_MICROBATCH, None)?;
                updates.push(update_start.elapsed());
                worker_cpu += work.worker_cpu;
            } else {
                while addresses
                    .iter()
                    .any(|&address| cache.peek(&identity, address).is_none())
                {
                    ensure!(
                        Instant::now() < deadline,
                        "{footprint} / {workers} worker case exceeded 20s"
                    );
                    let update_start = Instant::now();
                    let work = cache.generate(vertices, GENERATION_MICROBATCH, None)?;
                    updates.push(update_start.elapsed());
                    worker_cpu += work.worker_cpu;
                    if addresses
                        .iter()
                        .all(|&address| cache.peek(&identity, address).is_some())
                    {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
            }
            let elapsed = start.elapsed();
            ensure!(
                elapsed <= CASE_DEADLINE,
                "{footprint} / {workers} worker case exceeded 20s"
            );
            let cache_report = cache.report();
            let update_ms = updates
                .iter()
                .map(|d| d.as_secs_f64() * 1000.0)
                .collect::<Vec<_>>();
            let mut sorted = update_ms.clone();
            sorted.sort_by(f64::total_cmp);
            let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
            let worst = sorted.last().copied().unwrap_or(0.0);
            let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
            let patches_per_second = PATCHES_PER_FOOTPRINT as f64 / elapsed.as_secs_f64();
            let vertices_per_second = vertices as f64 / elapsed.as_secs_f64();
            let peak = cache_report.peak_aggregate_bytes;
            let cancelled = cache_report.cancellations;
            let worker_cpu_ms = worker_cpu.as_secs_f64() * 1000.0;
            writeln!(
                csv,
                "{footprint},{workers},{PATCHES_PER_FOOTPRINT},{vertices},{elapsed_ms:.3},{patches_per_second:.3},{vertices_per_second:.3},{median:.3},{worst:.3},{worker_cpu_ms:.3},{cancelled},{peak}"
            )?;
            println!(
                "{footprint},{workers},{PATCHES_PER_FOOTPRINT},{vertices},{elapsed_ms:.3},{patches_per_second:.3},{vertices_per_second:.3},{median:.3},{worst:.3},{worker_cpu_ms:.3},{cancelled},{peak}"
            );
        }
    }
    csv.flush()?;
    Ok(())
}
