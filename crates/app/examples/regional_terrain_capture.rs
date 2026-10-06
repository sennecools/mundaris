use anyhow::{Context, Result, ensure};
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::developer_capture::{DeveloperCapture, write_pair};
use mundaris_app::developer_protocol::DevCommand;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const FRAME_STEP: Duration = Duration::from_millis(16);
const STAGE_FRAMES: usize = 22;
static TRACE_WRITER: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

fn tile(demo: &mut GravityOrbitsDemo) -> Result<()> {
    tile_with_seed(demo, 0)
}

fn tile_with_seed(demo: &mut GravityOrbitsDemo, seed: u64) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: true,
        family: "rocky_v5".into(),
        seed,
        radius_m: 80_000.0,
        face: "positive_z".into(),
        level: 9,
        x: 157,
        y: 39,
        cells: 32,
        revision: 0,
    })
}

fn regional(demo: &mut GravityOrbitsDemo, worker_delay_ms: u64, pressure: bool) -> Result<()> {
    regional_with_workers(demo, worker_delay_ms, pressure, 4)
}

fn regional_with_workers(
    demo: &mut GravityOrbitsDemo,
    worker_delay_ms: u64,
    pressure: bool,
    worker_count: usize,
) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuRegional {
        enabled: true,
        max_depth: 4,
        gpu_slots: if pressure { 12 } else { 384 },
        cpu_tiles: 512,
        worker_count,
        worker_delay_ms,
        upload_tiles_per_frame: if pressure { 1 } else { 2 },
        upload_bytes_per_frame: if pressure { 39_200 } else { 1_048_576 },
        publication_groups_per_frame: 2,
        transition_limit: 4,
        morph_duration_ms: 150,
        split_error_px: 0.15,
        merge_error_px: 0.075,
    })
}

fn view(demo: &mut GravityOrbitsDemo, camera_offset_m: [f64; 3]) -> Result<()> {
    view_mode(demo, 0, camera_offset_m)
}

fn view_mode(demo: &mut GravityOrbitsDemo, mode: u8, camera_offset_m: [f64; 3]) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuTileView {
        mode,
        camera_offset_m,
        sun_direction_body: [0.3, 0.8, 0.5],
        reference_cpu: false,
    })
}

fn interpolate(from: [f64; 3], to: [f64; 3], fraction: f64) -> [f64; 3] {
    std::array::from_fn(|axis| from[axis] + (to[axis] - from[axis]) * fraction)
}

fn draw_frame(
    demo: &mut GravityOrbitsDemo,
    elapsed: Duration,
    route: &str,
    samples: &mut Vec<Value>,
    trace_path: &Path,
) -> Result<DeveloperCapture> {
    let started = Instant::now();
    let frame = demo.developer_offscreen_frame(elapsed, 960, 540)?;
    let row = json!({
        "route": route,
        "frame_number": frame.snapshot.general.frame_number,
        "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
        "outer_call_ms": started.elapsed().as_secs_f64() * 1000.0,
        "viewport_size_px": frame.snapshot.camera.viewport_size_pixels,
        "resident_regional": frame.snapshot.resident_regional.as_ref().map(compact_regional_snapshot),
        "performance": frame.snapshot.performance,
    });
    append_trace_row(trace_path, samples, row)?;
    Ok(frame)
}

fn compact_regional_snapshot(snapshot: &Value) -> Value {
    const FIELDS: &[&str] = &[
        "active_transitions",
        "active_transitions_reported",
        "build_failures",
        "cache_evictions",
        "cache_hits",
        "cache_misses",
        "cancelled_before_start",
        "cancelled_during_work",
        "completion_backlog",
        "completion_capacity",
        "cpu_cache_pressure",
        "cpu_cached_bytes",
        "cpu_cached_tiles",
        "cpu_tile_cap",
        "desired_capacity_pressure",
        "desired_count",
        "drawable_count",
        "estimated_completed_unpublished_bytes",
        "estimated_worker_scratch_bytes",
        "gpu_active_morph_patches",
        "gpu_admission_pressure",
        "gpu_balanced_split_frontier_count",
        "gpu_base_dependency_count",
        "gpu_cache_hits",
        "gpu_deferred_uploads",
        "gpu_evictions",
        "gpu_frontier_reserved_parent",
        "gpu_minimum_peak_split_slots",
        "gpu_slots",
        "gpu_reuploads",
        "gpu_retained_cover_fallback",
        "gpu_upload_bytes_per_frame",
        "gpu_upload_tiles_per_frame",
        "upload_byte_capacity",
        "upload_tile_capacity",
        "jobs_started",
        "max_desired_patches",
        "publication_candidates",
        "publication_ms",
        "publication_pressure",
        "quality_pending",
        "ready",
        "rebuilds",
        "refinement_debt",
        "requests_issued",
        "resident_count",
        "scheduler_time_micros",
        "selection_time_micros",
        "slot_pressure",
        "stale_completions",
        "tick",
        "topology_deferred",
        "upload_backlog_bytes",
        "upload_backlog_tiles",
        "upload_byte_capacity",
        "upload_pressure",
        "upload_tile_capacity",
        "worker_queued",
        "worker_running",
    ];
    let mut compact = serde_json::Map::new();
    for field in FIELDS {
        if let Some(value) = snapshot.get(field) {
            compact.insert((*field).to_owned(), value.clone());
        }
    }

    compact.insert(
        "desired_by_level".into(),
        json!(desired_level_counts(snapshot)),
    );

    if let Some(events) = snapshot.get("events").and_then(Value::as_array) {
        let recent: Vec<_> = events.iter().rev().take(4).rev().cloned().collect();
        compact.insert("recent_events".into(), json!(recent));
        compact.insert("event_count".into(), json!(events.len()));
    }
    if let Some(resources) = snapshot.get("resources") {
        compact.insert("resources".into(), resources.clone());
    }
    Value::Object(compact)
}

fn desired_level_counts(snapshot: &Value) -> BTreeMap<u64, usize> {
    let mut counts = BTreeMap::<u64, usize>::new();
    if let Some(desired) = snapshot.get("desired").and_then(Value::as_array) {
        for patch in desired {
            if let Some(level) = patch.get("level").and_then(Value::as_u64) {
                *counts.entry(level).or_default() += 1;
            }
        }
    }
    counts
}

fn append_trace_row(_trace_path: &Path, samples: &mut Vec<Value>, row: Value) -> Result<()> {
    samples.push(row.clone());
    let mut trace = TRACE_WRITER
        .get()
        .context("regional terrain trace writer is not initialized")?
        .lock()
        .map_err(|_| anyhow::anyhow!("regional terrain trace writer lock is poisoned"))?;
    serde_json::to_writer(&mut *trace, &row)?;
    trace.write_all(b"\n")?;
    trace.flush()?;
    Ok(())
}

fn capture(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    name: &str,
    route: &str,
    samples: &mut Vec<Value>,
    trace_path: &Path,
) -> Result<Value> {
    let frame = draw_frame(demo, Duration::ZERO, route, samples, trace_path)?;
    write_capture_frame(output, name, frame)
}

fn write_capture_frame(output: &Path, name: &str, mut frame: DeveloperCapture) -> Result<Value> {
    let metadata = frame
        .snapshot
        .capture
        .as_mut()
        .context("capture metadata missing")?;
    metadata.scene = name.into();
    metadata.image = format!("{name}.png");
    write_pair(output, &frame)
        .with_context(|| format!("write regional capture {name} to {}", output.display()))?;
    Ok(frame.snapshot.resident_regional.unwrap_or(Value::Null))
}

fn run_route(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    trace_path: &Path,
    label: &str,
    samples: &mut Vec<Value>,
) -> Result<()> {
    let positions = [
        ("a_start", [0.0, 0.0, 3000.0]),
        ("b", [80.0, 0.0, -170.0]),
        ("c", [-80.0, 0.0, -170.0]),
        ("b_revisit", [80.0, 0.0, -170.0]),
        ("a_return", [0.0, 0.0, 3000.0]),
    ];
    let pressure = label.starts_with("pressure_");
    let mut cache_hits_at_c = None;
    let mut previous_offset = positions[0].1;
    for (index, (name, offset)) in positions.into_iter().enumerate() {
        let movement = match name {
            "b" => "dive_stop_b",
            "c" => "lateral_stop_c",
            "b_revisit" => "backtrack_stop_b",
            "a_return" => "retreat_a",
            _ => "a_start",
        };
        let stage = format!("{label}_{movement}");
        for frame_index in 1..=STAGE_FRAMES {
            let interpolated = interpolate(
                previous_offset,
                offset,
                frame_index as f64 / STAGE_FRAMES as f64,
            );
            view(demo, interpolated)?;
            draw_frame(demo, FRAME_STEP, &stage, samples, trace_path)?;
        }
        previous_offset = offset;
        if label == "delay_0ms" && name == "b" {
            wait_for_morph_capture(demo, output, offset, samples, trace_path)?;
        }
        if pressure && name == "c" {
            wait_for_slot_pressure(demo, &stage, samples, trace_path)?;
        }
        let mut state = capture(demo, output, &stage, &stage, samples, trace_path)?;
        ensure!(
            state["ready"].is_boolean(),
            "{stage} snapshot omitted regional ready state"
        );
        ensure!(
            state["quality_pending"].is_boolean(),
            "{stage} snapshot omitted regional quality_pending state"
        );
        ensure!(
            state["refinement_debt"].is_number(),
            "{stage} snapshot omitted regional refinement_debt"
        );
        if name == "a_start" {
            ensure!(
                state["desired_count"] == 1,
                "{stage} should retain only the canonical root in the coarse view"
            );
        }
        if pressure && name == "c" {
            ensure!(
                state["gpu_admission_pressure"] == true,
                "regional GPU admission pressure did not bind at C"
            );
            ensure!(
                state["gpu_balanced_split_frontier_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0),
                "regional GPU pressure reported no balanced split frontiers"
            );
            let minimum_peak_slots = state["gpu_minimum_peak_split_slots"]
                .as_u64()
                .context("regional GPU pressure omitted minimum split-frontier capacity")?;
            ensure!(
                minimum_peak_slots > 12,
                "a regional split frontier fit the 12-slot GPU capacity"
            );
            ensure!(
                state["ready"] == true,
                "regional root coverage was lost under pressure"
            );
            ensure!(
                state["quality_pending"] == true,
                "regional pressure route did not expose quality lag at C"
            );
            ensure!(
                state["gpu_slots"]
                    .as_array()
                    .is_some_and(|slots| slots.len() == 12),
                "regional pressure snapshot omitted the configured 12 GPU slots"
            );
            ensure_pressure_ancestor_cover(&state, &stage)?;
            ensure_pressure_upload_budget(&state, samples, &stage, 1, 39_200)?;
            ensure!(
                state["refinement_debt"]
                    .as_f64()
                    .is_some_and(|debt| debt > 0.0),
                "regional pressure route did not accumulate refinement debt at C"
            );
            view_mode(demo, 10, offset)?;
            let fallback = capture(
                demo,
                output,
                "pressure_c_debug_fallback_quality_pending",
                "pressure_c_debug_fallback_quality_pending",
                samples,
                trace_path,
            )?;
            ensure!(
                fallback["quality_pending"] == true,
                "mode-10 pressure capture did not retain its quality-pending label"
            );
            view(demo, offset)?;
        } else if name == "c" {
            state = wait_for_idle(demo, &stage, offset, samples, trace_path)?;
            ensure_deep_desired(&state, &stage)?;
            cache_hits_at_c = state["cache_hits"].as_u64();
            capture(
                demo,
                output,
                &format!("{label}_c_converged"),
                &format!("{label}_c_converged"),
                samples,
                trace_path,
            )?;
        }
        if name == "b" && label == "delay_0ms" {
            state = wait_for_idle(demo, &stage, offset, samples, trace_path)?;
            ensure_mixed_desired(&state, &stage)?;
            ensure_deep_desired(&state, &stage)?;
            for (mode, suffix) in [(6, "lod"), (7, "slots"), (9, "parent_dependencies")] {
                view_mode(demo, mode, offset)?;
                capture(
                    demo,
                    output,
                    &format!("delay_0ms_b_debug_{suffix}"),
                    &format!("delay_0ms_b_debug_{suffix}"),
                    samples,
                    trace_path,
                )?;
            }
            view(demo, offset)?;
            capture(
                demo,
                output,
                &format!("{label}_b_converged_before_backtrack"),
                &format!("{label}_b_converged_before_backtrack"),
                samples,
                trace_path,
            )?;
            let overview_offset = [0.0, 0.0, 400.0];
            view_mode(demo, 7, overview_offset)?;
            let overview = capture(
                demo,
                output,
                "delay_0ms_b_debug_overview_slots",
                "delay_0ms_b_debug_overview_slots",
                samples,
                trace_path,
            )?;
            ensure!(
                overview["ready"] == true,
                "overview slot diagnostic lost regional root coverage"
            );
            view(demo, offset)?;
            state = wait_for_idle(demo, &stage, offset, samples, trace_path)?;
            ensure_deep_desired(&state, &stage)?;
        }
        if name == "b_revisit" && !pressure {
            state = wait_for_idle(demo, &stage, offset, samples, trace_path)?;
            ensure_deep_desired(&state, &stage)?;
            let after = state["cache_hits"].as_u64().unwrap_or(0);
            ensure!(
                cache_hits_at_c.is_some_and(|before| after > before),
                "{stage} did not reuse cached CPU tiles after the C-to-B backtrack"
            );
            capture(
                demo,
                output,
                &format!("{label}_b_revisit_converged"),
                &format!("{label}_b_revisit_converged"),
                samples,
                trace_path,
            )?;
        }
        if index == positions.len() - 1 {
            state = wait_for_idle(demo, &stage, offset, samples, trace_path)?;
            ensure!(
                state["refinement_debt"]
                    .as_f64()
                    .is_some_and(|debt| debt <= 1.0e-6),
                "{stage} did not converge after retreat under the current capacity"
            );
            capture(
                demo,
                output,
                &format!("{label}_converged"),
                &format!("{label}_converged"),
                samples,
                trace_path,
            )?;
        }
    }
    Ok(())
}

fn run_capture_free_timing_route(
    demo: &mut GravityOrbitsDemo,
    trace_path: &Path,
    samples: &mut Vec<Value>,
) -> Result<()> {
    let mut previous_offset = [0.0, 0.0, 3000.0];
    for (name, offset) in [
        ("a_start", [0.0, 0.0, 3000.0]),
        ("b", [80.0, 0.0, -170.0]),
        ("c", [-80.0, 0.0, -170.0]),
        ("b_revisit", [80.0, 0.0, -170.0]),
        ("a_return", [0.0, 0.0, 3000.0]),
    ] {
        let route = format!("capture_free_timing_{name}");
        for frame_index in 1..=STAGE_FRAMES {
            view(
                demo,
                interpolate(
                    previous_offset,
                    offset,
                    frame_index as f64 / STAGE_FRAMES as f64,
                ),
            )?;
            draw_frame(demo, FRAME_STEP, &route, samples, trace_path)?;
        }
        previous_offset = offset;
    }
    let state = wait_for_idle(
        demo,
        "capture_free_timing_a_return",
        previous_offset,
        samples,
        trace_path,
    )?;
    ensure!(
        state["quality_pending"] == false,
        "capture-free timing route did not return to settled coarse coverage"
    );
    Ok(())
}

fn run_cancellation_probe(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    trace_path: &Path,
    samples: &mut Vec<Value>,
) -> Result<()> {
    const A: [f64; 3] = [0.0, 0.0, 3000.0];
    const B: [f64; 3] = [80.0, 0.0, -170.0];
    tile_with_seed(demo, 1)?;
    regional_with_workers(demo, 500, false, 2)?;
    view(demo, A)?;
    let root = wait_for_idle(
        demo,
        "cancellation_probe_seed1_root_ready",
        A,
        samples,
        trace_path,
    )?;
    ensure!(
        root["ready"] == true && root["desired_count"] == 1,
        "cancellation probe did not begin from drawable single-root coverage"
    );
    capture(
        demo,
        output,
        "cancellation_probe_seed1_root_ready",
        "cancellation_probe_seed1_root_ready",
        samples,
        trace_path,
    )?;

    view(demo, B)?;
    draw_frame(
        demo,
        FRAME_STEP,
        "cancellation_probe_dive_to_b",
        samples,
        trace_path,
    )?;
    view(demo, B)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let _pending = loop {
        ensure!(
            Instant::now() < deadline,
            "cancellation probe did not create running and queued work at B within 10 seconds"
        );
        let frame = draw_frame(
            demo,
            FRAME_STEP,
            "cancellation_probe_stationary_b",
            samples,
            trace_path,
        )?;
        let state = frame
            .snapshot
            .resident_regional
            .context("cancellation probe omitted its B snapshot")?;
        ensure!(
            state["quality_pending"] == true,
            "500 ms cancellation probe unexpectedly settled B before retreat"
        );
        if state["worker_running"].as_u64().unwrap_or(0) > 0
            && state["worker_queued"].as_u64().unwrap_or(0) > 0
        {
            break state;
        }
        std::thread::yield_now();
    };
    capture(
        demo,
        output,
        "cancellation_probe_seed1_b_work_in_flight",
        "cancellation_probe_seed1_b_work_in_flight",
        samples,
        trace_path,
    )?;

    view(demo, A)?;
    draw_frame(
        demo,
        FRAME_STEP,
        "cancellation_probe_immediate_retreat_to_a",
        samples,
        trace_path,
    )?;
    let drained = wait_for_idle(
        demo,
        "cancellation_probe_seed1_a_cancelled_work_drained",
        A,
        samples,
        trace_path,
    )?;
    ensure!(
        drained["ready"] == true && drained["desired_count"] == 1 && drained["drawable_count"] == 1,
        "cancellation probe lost its drawable ancestor root after retreat"
    );
    ensure!(
        drained["stale_completions"].is_number(),
        "cancellation probe snapshot omitted stale-completion rejection accounting"
    );
    ensure!(
        drained["cancelled_before_start"]
            .as_u64()
            .is_some_and(|count| count > 0)
            && drained["cancelled_during_work"]
                .as_u64()
                .is_some_and(|count| count > 0),
        "cancellation probe did not observe queued and running worker cancellation"
    );
    capture(
        demo,
        output,
        "cancellation_probe_seed1_a_cancelled_work_drained",
        "cancellation_probe_seed1_a_cancelled_work_drained",
        samples,
        trace_path,
    )?;
    tile(demo)?;
    Ok(())
}

fn wait_for_morph_capture(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    offset: [f64; 3],
    samples: &mut Vec<Value>,
    trace_path: &Path,
) -> Result<()> {
    let route = "delay_0ms_dive_stop_b_debug_morph_wait";
    let deadline = Instant::now() + Duration::from_secs(90);
    view_mode(demo, 8, offset)?;
    while Instant::now() < deadline {
        let frame = draw_frame(demo, FRAME_STEP, route, samples, trace_path)?;
        if frame
            .snapshot
            .resident_regional
            .as_ref()
            .is_some_and(|state| state["active_transitions"].as_u64().unwrap_or(0) > 0)
        {
            let transition =
                write_capture_frame(output, "delay_0ms_b_debug_morph_transition", frame)?;
            ensure!(
                transition["active_transitions"].as_u64().unwrap_or(0) > 0,
                "mode-8 capture missed the active regional morph"
            );
            view(demo, offset)?;
            return Ok(());
        }
        std::thread::yield_now();
    }
    view(demo, offset)?;
    anyhow::bail!("regional B stop did not expose a morph transition within 90 seconds")
}

fn wait_for_slot_pressure(
    demo: &mut GravityOrbitsDemo,
    route: &str,
    samples: &mut Vec<Value>,
    trace_path: &Path,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(90);
    while Instant::now() < deadline {
        let frame = draw_frame(demo, FRAME_STEP, route, samples, trace_path)?;
        if frame
            .snapshot
            .resident_regional
            .as_ref()
            .is_some_and(|state| state["slot_pressure"].as_bool() == Some(true))
        {
            return Ok(());
        }
        std::thread::yield_now();
    }
    anyhow::bail!(
        "regional route {route} did not reach physical GPU slot pressure within 90 seconds"
    )
}

fn ensure_mixed_desired(state: &Value, route: &str) -> Result<()> {
    let desired = state["desired"]
        .as_array()
        .context("regional snapshot omitted desired patch records")?;
    let levels: BTreeSet<_> = desired
        .iter()
        .filter_map(|patch| patch["level"].as_u64())
        .collect();
    ensure!(
        (16..=256).contains(&desired.len()),
        "{route} selected {} patches outside the expected 16..=256 local-detail range",
        desired.len()
    );
    ensure!(
        !(desired.len() == 64 && levels.len() == 1 && levels.contains(&12)),
        "{route} collapsed to a uniform 64-leaf L12 cover"
    );
    ensure!(
        levels.len() > 1,
        "{route} did not produce a mixed desired hierarchy"
    );
    Ok(())
}

fn parse_patch_address(value: &Value) -> Option<(String, u8, u32, u32)> {
    let encoded = value
        .as_str()?
        .strip_prefix("CubePatchAddress { ")?
        .strip_suffix(" }")?;
    let mut face = None;
    let mut level = None;
    let mut x = None;
    let mut y = None;
    for field in encoded.split(',') {
        let (key, value) = field.trim().split_once(':')?;
        match key.trim() {
            "face" => face = Some(value.trim().to_owned()),
            "level" => level = value.trim().parse().ok(),
            "x" => x = value.trim().parse().ok(),
            "y" => y = value.trim().parse().ok(),
            _ => return None,
        }
    }
    Some((face?, level?, x?, y?))
}

fn ensure_pressure_ancestor_cover(state: &Value, route: &str) -> Result<()> {
    let roots = state["configuration"]["roots"]
        .as_array()
        .context("regional pressure snapshot omitted configured roots")?;
    let resident = state["resident"]
        .as_array()
        .context("regional pressure snapshot omitted resident addresses")?;
    let drawable = state["drawable"]
        .as_array()
        .context("regional pressure snapshot omitted drawable addresses")?;
    ensure!(
        !drawable.is_empty() && state["drawable_count"].as_u64().unwrap_or(0) > 0,
        "{route} did not retain a drawable regional cover"
    );
    for root_value in roots {
        let root = parse_patch_address(root_value)
            .with_context(|| format!("{route} has an invalid configured root address"))?;
        ensure!(
            resident.contains(root_value),
            "{route} lost configured root ancestor {root_value}"
        );
        for address in drawable {
            let patch = parse_patch_address(address)
                .with_context(|| format!("{route} has an invalid drawable address {address}"))?;
            let level_delta = patch.1.checked_sub(root.1);
            let descends_from_root = level_delta.is_some_and(|delta| {
                delta < u32::BITS as u8
                    && patch.0 == root.0
                    && (patch.2 >> delta) == root.2
                    && (patch.3 >> delta) == root.3
            });
            ensure!(
                descends_from_root,
                "{route} drawable patch {address} is outside retained root {root_value}"
            );
        }
    }
    Ok(())
}

fn ensure_pressure_upload_budget(
    state: &Value,
    samples: &[Value],
    route: &str,
    tile_limit: u64,
    byte_limit: u64,
) -> Result<()> {
    ensure!(
        state["upload_tile_capacity"].as_u64() == Some(tile_limit)
            && state["upload_byte_capacity"].as_u64() == Some(byte_limit),
        "{route} snapshot does not report the configured per-frame tile and byte caps"
    );
    let route_samples: Vec<_> = samples
        .iter()
        .filter(|sample| sample["route"].as_str() == Some(route))
        .collect();
    ensure!(
        !route_samples.is_empty(),
        "{route} produced no per-frame upload samples"
    );
    let mut max_tiles = 0;
    let mut max_bytes = 0;
    let mut exact_tile_limit_observed = false;
    for sample in &route_samples {
        let regional = &sample["resident_regional"];
        let tiles = regional["gpu_upload_tiles_per_frame"].as_u64().unwrap_or(0);
        let bytes = regional["gpu_upload_bytes_per_frame"].as_u64().unwrap_or(0);
        max_tiles = max_tiles.max(tiles);
        max_bytes = max_bytes.max(bytes);
        exact_tile_limit_observed |= tiles == tile_limit;
        ensure!(
            tiles <= tile_limit && bytes <= byte_limit,
            "{route} exceeded per-frame regional upload limits: {tiles} tiles / {bytes} bytes, limits {tile_limit} / {byte_limit}"
        );
    }
    ensure!(
        exact_tile_limit_observed,
        "{route} trace never exercised its {tile_limit}-tile upload allowance"
    );
    eprintln!(
        "regional upload budget route={route} frames={} max_tiles={} max_bytes={} caps_tiles={} caps_bytes={}",
        route_samples.len(),
        max_tiles,
        max_bytes,
        tile_limit,
        byte_limit
    );
    Ok(())
}

fn ensure_deep_desired(state: &Value, route: &str) -> Result<()> {
    let desired = state["desired"]
        .as_array()
        .context("regional snapshot omitted desired patch records")?;
    let max_level = desired
        .iter()
        .filter_map(|patch| patch["level"].as_u64())
        .max()
        .unwrap_or(0);
    ensure!(
        desired.len() > 1 && max_level > 9,
        "{route} did not select detail below the L9 regional root"
    );
    ensure!(
        state["quality_pending"] == false
            && state["refinement_debt"]
                .as_f64()
                .is_some_and(|debt| debt <= 1.0e-6),
        "{route} did not settle its detail with zero refinement debt"
    );
    Ok(())
}

fn wait_for_idle(
    demo: &mut GravityOrbitsDemo,
    route: &str,
    offset: [f64; 3],
    samples: &mut Vec<Value>,
    trace_path: &Path,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let started = Instant::now();
    let mut frames = 0;
    view(demo, offset)?;
    let mut previous_desired = None;
    let mut consecutive_stable = 0;
    while Instant::now() < deadline {
        let frame = draw_frame(demo, FRAME_STEP, route, samples, trace_path)?;
        frames += 1;
        if let Some(state) = &frame.snapshot.resident_regional
            && state["quality_pending"].as_bool() == Some(false)
            && state["worker_queued"].as_u64() == Some(0)
            && state["worker_running"].as_u64() == Some(0)
            && state["completion_backlog"].as_u64() == Some(0)
        {
            let desired = serde_json::to_string(&state["desired"])?;
            if previous_desired.as_ref() == Some(&desired) {
                consecutive_stable += 1;
            } else {
                consecutive_stable = 0;
            }
            previous_desired = Some(desired);
            if consecutive_stable >= 1 {
                let milestone = json!({
                    "event": "convergence",
                    "route": route,
                    "wait_wall_ms": started.elapsed().as_secs_f64() * 1000.0,
                    "wait_frames": frames,
                    "viewport_size_px": frame.snapshot.camera.viewport_size_pixels,
                    "desired_count": state["desired_count"],
                    "desired_by_level": desired_level_counts(state),
                    "drawable_count": state["drawable_count"],
                    "refinement_debt": state["refinement_debt"],
                    "quality_pending": state["quality_pending"],
                    "selection_time_micros": state["selection_time_micros"],
                });
                append_trace_row(trace_path, samples, milestone)?;
                return Ok(state.clone());
            }
        } else {
            previous_desired = None;
            consecutive_stable = 0;
        }
        std::thread::yield_now();
    }
    anyhow::bail!("regional route {route} did not converge and drain workers within 90 seconds")
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().map(PathBuf::from).context(
            "usage: cargo run --release --locked -p mundaris_app --features developer-tools --example regional_terrain_capture -- <output-directory> [--probe-pressure-only|--delay500-probe-pressure-only]",
        )?;
    let run_mode = args.next();
    let probe_pressure_only = run_mode
        .as_deref()
        .is_some_and(|argument| argument == "--probe-pressure-only");
    let delay_500_probe_pressure_only = run_mode
        .as_deref()
        .is_some_and(|argument| argument == "--delay500-probe-pressure-only");
    ensure!(
        args.next().is_none(),
        "unexpected extra command-line argument"
    );
    ensure!(
        run_mode.is_none() || probe_pressure_only || delay_500_probe_pressure_only,
        "unknown regional capture mode"
    );
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );
    fs::create_dir_all(&output)?;
    let trace_path = output.join("regional_terrain_frames.jsonl");
    let trace_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&trace_path)
        .with_context(|| format!("create trace file {}", trace_path.display()))?;
    TRACE_WRITER
        .set(Mutex::new(trace_file))
        .map_err(|_| anyhow::anyhow!("regional terrain trace writer initialized twice"))?;

    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session("regional-terrain-capture");
    let moon = demo
        .developer_inventory("regional-terrain-capture")?
        .into_iter()
        .find(|body| body.name == "Moon")
        .context("real Solar System fixture has no Moon")?;
    demo.developer_apply_command(&DevCommand::Select { body: moon.handle })?;
    tile(&mut demo)?;

    let mut samples = Vec::new();
    if probe_pressure_only {
        run_cancellation_probe(&mut demo, &output, &trace_path, &mut samples)?;
    } else if delay_500_probe_pressure_only {
        regional(&mut demo, 500, false)?;
        run_route(&mut demo, &output, &trace_path, "delay_500ms", &mut samples)?;
        run_cancellation_probe(&mut demo, &output, &trace_path, &mut samples)?;
    } else {
        for worker_delay_ms in [0, 50, 250, 500] {
            regional(&mut demo, worker_delay_ms, false)?;
            run_route(
                &mut demo,
                &output,
                &trace_path,
                &format!("delay_{worker_delay_ms}ms"),
                &mut samples,
            )?;
            if worker_delay_ms == 0 {
                run_capture_free_timing_route(&mut demo, &trace_path, &mut samples)?;
            }
        }
        run_cancellation_probe(&mut demo, &output, &trace_path, &mut samples)?;
    }
    regional(&mut demo, 0, true)?;
    run_route(
        &mut demo,
        &output,
        &trace_path,
        "pressure_12slots_1upload",
        &mut samples,
    )?;
    demo.developer_apply_command(&DevCommand::GpuRegional {
        enabled: false,
        max_depth: 4,
        gpu_slots: 12,
        cpu_tiles: 512,
        worker_count: 4,
        worker_delay_ms: 0,
        upload_tiles_per_frame: 1,
        upload_bytes_per_frame: 39_200,
        publication_groups_per_frame: 2,
        transition_limit: 4,
        morph_duration_ms: 150,
        split_error_px: 0.15,
        merge_error_px: 0.075,
    })?;
    fs::write(
        output.join("regional_terrain_frames.json"),
        serde_json::to_vec_pretty(&samples)?,
    )?;
    Ok(())
}
