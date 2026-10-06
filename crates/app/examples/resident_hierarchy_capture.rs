use anyhow::{Context, Result, ensure};
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::developer_capture::{DeveloperCapture, write_pair};
use mundaris_app::developer_protocol::DevCommand;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn tile(demo: &mut GravityOrbitsDemo, revision: u64) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: true,
        family: "rocky_v5".into(),
        seed: 0,
        radius_m: 80_000.0,
        face: "positive_z".into(),
        level: 9,
        x: 157,
        y: 39,
        cells: 64,
        revision,
    })
}

fn hierarchy(
    demo: &mut GravityOrbitsDemo,
    refine: bool,
    morph_duration_ms: u64,
    child_delays_ms: [u64; 4],
    request_mask: u8,
    cancel_pending: bool,
    diagnostic_validate: bool,
) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuHierarchy {
        enabled: true,
        refine,
        morph_duration_ms,
        child_delays_ms,
        request_mask,
        cancel_pending,
        diagnostic_validate,
    })
}

fn view(demo: &mut GravityOrbitsDemo, mode: u8, camera_offset_m: [f64; 3]) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuTileView {
        mode,
        camera_offset_m,
        sun_direction_body: [0.3, 0.8, 0.5],
        reference_cpu: false,
    })
}

fn validate_patches(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    samples: &mut Vec<Value>,
    mode: u8,
    label: &str,
    refine: bool,
    morph_duration_ms: u64,
) -> Result<()> {
    view(demo, mode, [0.0; 3])?;
    hierarchy(demo, refine, morph_duration_ms, [0; 4], 0x0f, false, true)?;
    for patch in 0..5 {
        capture(
            demo,
            output,
            &format!("{label}_patch_{patch}"),
            Duration::ZERO,
            samples,
        )?;
    }
    Ok(())
}

fn draw_frame(
    demo: &mut GravityOrbitsDemo,
    elapsed: Duration,
    samples: &mut Vec<Value>,
) -> Result<DeveloperCapture> {
    let started = Instant::now();
    let frame = demo.developer_offscreen_frame(elapsed, 960, 540)?;
    samples.push(json!({
        "frame_number": frame.snapshot.general.frame_number,
        "outer_call_ms": started.elapsed().as_secs_f64() * 1000.0,
        "resident_hierarchy": frame.snapshot.resident_hierarchy,
        "performance": frame.snapshot.performance,
    }));
    Ok(frame)
}

fn capture(
    demo: &mut GravityOrbitsDemo,
    output: &Path,
    name: &str,
    elapsed: Duration,
    samples: &mut Vec<Value>,
) -> Result<Value> {
    let mut frame = draw_frame(demo, elapsed, samples)?;
    let metadata = frame
        .snapshot
        .capture
        .as_mut()
        .context("capture metadata missing")?;
    metadata.scene = name.into();
    metadata.image = format!("{name}.png");
    write_pair(output, &frame)?;
    Ok(frame.snapshot.resident_hierarchy.unwrap_or(Value::Null))
}

fn run_until(
    demo: &mut GravityOrbitsDemo,
    samples: &mut Vec<Value>,
    timeout: Duration,
    predicate: impl Fn(&Value) -> bool,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let frame = draw_frame(demo, Duration::from_millis(16), samples)?;
        last = frame.snapshot.resident_hierarchy.unwrap_or(Value::Null);
        if predicate(&last) {
            return Ok(last);
        }
        thread::sleep(Duration::from_millis(4));
    }
    Ok(last)
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: cargo run --locked -p mundaris_app --features developer-tools --example resident_hierarchy_capture -- <output-directory>")?;
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );
    fs::create_dir_all(&output)?;

    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session("resident-hierarchy-capture");
    let moon = demo
        .developer_inventory("resident-hierarchy-capture")?
        .into_iter()
        .find(|body| body.name == "Moon")
        .context("real Solar System fixture has no Moon")?;
    demo.developer_apply_command(&DevCommand::Select { body: moon.handle })?;

    let mut samples = Vec::new();
    tile(&mut demo, 0)?;
    capture(
        &mut demo,
        &output,
        "parent_cold",
        Duration::ZERO,
        &mut samples,
    )?;

    hierarchy(&mut demo, true, 150, [0, 0, 0, 0], 0b0111, false, false)?;
    capture(
        &mut demo,
        &output,
        "three_children_requested",
        Duration::ZERO,
        &mut samples,
    )?;
    let partial = run_until(&mut demo, &mut samples, Duration::from_secs(20), |state| {
        state["gpu_ready_child_count"] == 3
    })?;
    ensure!(
        partial["gpu_ready_child_count"] == 3,
        "three-child partial state was not reached"
    );
    capture(
        &mut demo,
        &output,
        "three_children_ready_parent_fallback",
        Duration::ZERO,
        &mut samples,
    )?;

    hierarchy(&mut demo, true, 150, [0, 0, 0, 500], 0x0f, false, false)?;
    capture(
        &mut demo,
        &output,
        "fourth_child_delayed_parent_fallback",
        Duration::ZERO,
        &mut samples,
    )?;
    let complete = run_until(&mut demo, &mut samples, Duration::from_secs(30), |state| {
        state["draw_children_gpu_ready"] == true
    })?;
    ensure!(
        complete["draw_children_gpu_ready"] == true,
        "all four children did not become resident"
    );
    capture(
        &mut demo,
        &output,
        "four_children_parent_endpoint",
        Duration::ZERO,
        &mut samples,
    )?;

    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        2,
        "normal_parent_endpoint_t0",
        true,
        150,
    )?;
    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        3,
        "material_parent_endpoint_t0",
        true,
        150,
    )?;
    hierarchy(&mut demo, true, 1_000, [0; 4], 0x0f, false, false)?;
    capture(
        &mut demo,
        &output,
        "refinement_start_child_endpoint_t0",
        Duration::ZERO,
        &mut samples,
    )?;
    let midpoint = capture(
        &mut demo,
        &output,
        "children_morphed_t075",
        Duration::from_millis(750),
        &mut samples,
    )?;
    ensure!(
        (midpoint["morph_fraction"].as_f64().unwrap_or(-1.0) - 0.75).abs() < 1e-6,
        "controlled child morph did not reach 0.75"
    );
    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        2,
        "normal_children_t075",
        true,
        1_000,
    )?;
    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        3,
        "material_children_t075",
        true,
        1_000,
    )?;
    capture(
        &mut demo,
        &output,
        "children_morphed_t1",
        Duration::from_millis(250),
        &mut samples,
    )?;
    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        2,
        "normal_children_t1",
        true,
        1_000,
    )?;
    validate_patches(
        &mut demo,
        &output,
        &mut samples,
        3,
        "material_children_t1",
        true,
        1_000,
    )?;

    hierarchy(&mut demo, false, 300, [0; 4], 0x0f, false, false)?;
    capture(
        &mut demo,
        &output,
        "reverse_mid_morph",
        Duration::from_millis(75),
        &mut samples,
    )?;
    hierarchy(&mut demo, false, 300, [0; 4], 0x0f, false, true)?;
    for patch in 0..5 {
        capture(
            &mut demo,
            &output,
            &format!("mid_morph_patch_{patch}"),
            Duration::ZERO,
            &mut samples,
        )?;
    }
    hierarchy(&mut demo, true, 300, [0; 4], 0x0f, false, false)?;
    capture(
        &mut demo,
        &output,
        "reverse_toward_children",
        Duration::from_millis(75),
        &mut samples,
    )?;

    view(&mut demo, 5, [90.0, 0.0, -140.0])?;
    capture(
        &mut demo,
        &output,
        "camera_child_edge",
        Duration::ZERO,
        &mut samples,
    )?;
    view(&mut demo, 5, [90.0, 90.0, -140.0])?;
    capture(
        &mut demo,
        &output,
        "camera_child_corner",
        Duration::ZERO,
        &mut samples,
    )?;

    // The parent center is the shared four-child corner. The preceding
    // translated views inspect the outer footprint; inspect this junction
    // separately and cross it without changing resident content or morph.
    for (name, mode, offset) in [
        ("shared_border_before", 5, [-15.0, 0.0, -190.0]),
        ("shared_corner_uv", 4, [0.0, 0.0, -190.0]),
        ("shared_corner_grid", 5, [0.0, 0.0, -190.0]),
        ("shared_corner_normals", 2, [0.0, 0.0, -190.0]),
        ("shared_border_after", 5, [15.0, 0.0, -190.0]),
    ] {
        view(&mut demo, mode, offset)?;
        capture(&mut demo, &output, name, Duration::ZERO, &mut samples)?;
    }

    hierarchy(&mut demo, false, 150, [0; 4], 0x0f, false, false)?;
    for _ in 0..12 {
        draw_frame(&mut demo, Duration::from_millis(16), &mut samples)?;
    }
    let parent_only = capture(
        &mut demo,
        &output,
        "repeat_parent_only",
        Duration::ZERO,
        &mut samples,
    )?;
    let uploads_before = parent_only["aggregate_resources"]["cumulative_content_upload_bytes"]
        .as_u64()
        .unwrap_or(0);
    hierarchy(&mut demo, true, 0, [0; 4], 0x0f, false, false)?;
    let repeated = run_until(&mut demo, &mut samples, Duration::from_secs(10), |state| {
        state["draw_children_gpu_ready"] == true && state["morph_fraction"] == 1.0
    })?;
    let uploads_after = repeated["aggregate_resources"]["cumulative_content_upload_bytes"]
        .as_u64()
        .unwrap_or(0);
    ensure!(
        uploads_after == uploads_before,
        "repeated hierarchy demand uploaded unchanged child content"
    );
    capture(
        &mut demo,
        &output,
        "repeat_children_resident_no_upload",
        Duration::ZERO,
        &mut samples,
    )?;

    tile(&mut demo, 2)?;
    capture(
        &mut demo,
        &output,
        "new_parent_revision_before_stale_work",
        Duration::ZERO,
        &mut samples,
    )?;
    hierarchy(&mut demo, true, 150, [500; 4], 0x0f, false, false)?;
    let building = run_until(&mut demo, &mut samples, Duration::from_secs(10), |state| {
        state["child_states"]
            .as_array()
            .is_some_and(|slots| slots.iter().all(|slot| slot["state"] == "building"))
    })?;
    ensure!(
        building["child_states"]
            .as_array()
            .is_some_and(|slots| slots.iter().all(|slot| slot["state"] == "building")),
        "all delayed child jobs did not start before cancellation"
    );
    capture(
        &mut demo,
        &output,
        "delayed_children_building_before_epoch_cancel",
        Duration::ZERO,
        &mut samples,
    )?;
    hierarchy(&mut demo, true, 150, [0; 4], 0x0f, true, false)?;
    capture(
        &mut demo,
        &output,
        "same_parent_reissued_under_new_epoch",
        Duration::ZERO,
        &mut samples,
    )?;
    let late = run_until(&mut demo, &mut samples, Duration::from_secs(30), |state| {
        state["rejected_late_results"].as_u64().unwrap_or(0) >= 4
            && state["draw_children_gpu_ready"] == true
    })?;
    ensure!(
        late["rejected_late_results"].as_u64().unwrap_or(0) >= 4,
        "late old child results were not rejected"
    );
    capture(
        &mut demo,
        &output,
        "late_old_results_rejected_new_parent_resident",
        Duration::ZERO,
        &mut samples,
    )?;

    fs::write(
        output.join("resident_hierarchy_frames.json"),
        serde_json::to_vec_pretty(&samples)?,
    )?;
    Ok(())
}
