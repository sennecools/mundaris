use anyhow::{Context, Result, ensure};
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::developer_capture::write_pair;
use mundaris_app::developer_protocol::DevCommand;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[allow(clippy::too_many_arguments)] // Keeps capture coordinates explicit at each scenario call.
fn configure(
    demo: &mut GravityOrbitsDemo,
    family: &str,
    radius_m: f64,
    face: &str,
    level: u8,
    x: u32,
    y: u32,
    revision: u64,
) -> Result<()> {
    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: true,
        family: family.into(),
        seed: 0,
        radius_m,
        face: face.into(),
        level,
        x,
        y,
        cells: 64,
        revision,
    })?;
    // Every new content key returns to the production GPU path, even if the
    // preceding capture used the CPU comparison view.
    demo.developer_apply_command(&DevCommand::GpuTileView {
        mode: 0,
        camera_offset_m: [0.0; 3],
        sun_direction_body: [0.3, 0.8, 0.5],
        reference_cpu: false,
    })
}

fn capture(demo: &mut GravityOrbitsDemo, output: &Path, name: &str) -> Result<()> {
    let mut frame = demo.developer_offscreen_frame(Duration::ZERO, 960, 540)?;
    let metadata = frame
        .snapshot
        .capture
        .as_mut()
        .context("capture metadata missing")?;
    metadata.scene = name.into();
    metadata.image = format!("{name}.png");
    write_pair(output, &frame)
}

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: cargo run --locked -p mundaris-app --example resident_tile_capture -- <output_directory>")?;
    ensure!(
        !output.exists(),
        "refusing to overwrite {}",
        output.display()
    );

    let session = "resident-tile-capture";
    let mut demo = GravityOrbitsDemo::solar_system(true)?;
    demo.developer_set_session(session);
    let moon = demo
        .developer_inventory(session)?
        .into_iter()
        .find(|body| body.name == "Moon")
        .context("real Solar System fixture has no Moon")?;
    demo.developer_apply_command(&DevCommand::Select { body: moon.handle })?;

    configure(&mut demo, "rocky_v5", 80_000.0, "positive_z", 9, 157, 39, 0)?;
    capture(&mut demo, &output, "cold")?;
    let mut warm = None;
    let mut warm_samples = Vec::with_capacity(120);
    for sample in 0..120 {
        let frame = demo.developer_offscreen_frame(Duration::ZERO, 960, 540)?;
        warm_samples.push(serde_json::json!({
            "sample": sample + 1,
            "performance": frame.snapshot.performance,
            "resident_tile": frame.snapshot.resident_tile,
        }));
        warm = Some(frame);
    }
    let mut warm = warm.context("warm fixture produced no frame")?;
    let metadata = warm
        .snapshot
        .capture
        .as_mut()
        .context("warm metadata missing")?;
    metadata.scene = "warm_120".into();
    metadata.image = "warm_120.png".into();
    write_pair(&output, &warm)?;
    fs::write(
        output.join("resident_tile_warm_frames.json"),
        serde_json::to_vec_pretty(&warm_samples)?,
    )?;

    demo.developer_apply_command(&DevCommand::GpuTileView {
        mode: 0,
        camera_offset_m: [24.0, -18.0, 60.0],
        sun_direction_body: [-0.7, 0.25, 0.6],
        reference_cpu: false,
    })?;
    capture(&mut demo, &output, "camera_light")?;
    for (mode, name) in [
        (1, "height"),
        (2, "normals"),
        (3, "material"),
        (4, "uv"),
        (5, "grid"),
        (0, "cpu_reference"),
    ] {
        demo.developer_apply_command(&DevCommand::GpuTileView {
            mode,
            camera_offset_m: [0.0; 3],
            sun_direction_body: [0.3, 0.8, 0.5],
            reference_cpu: name == "cpu_reference",
        })?;
        capture(&mut demo, &output, name)?;
    }

    configure(&mut demo, "rocky_v5", 80_000.0, "positive_z", 9, 157, 39, 1)?;
    capture(&mut demo, &output, "invalidation_revision")?;
    configure(
        &mut demo,
        "rocky_v5",
        100_000.0,
        "positive_x",
        9,
        157,
        39,
        1,
    )?;
    capture(&mut demo, &output, "invalidation_radius_definition")?;
    for (family, name) in [
        ("rocky_v5", "rocky"),
        ("icy_v3", "icy"),
        ("volcanic_v3", "volcanic"),
    ] {
        configure(&mut demo, family, 80_000.0, "positive_z", 9, 157, 39, 0)?;
        capture(&mut demo, &output, &format!("family_{name}"))?;
    }

    // Each radius keeps a roughly 250 m chart footprint by selecting a dyadic
    // level from its actual physical size. Face centres plus edge/corner patches
    // exercise the same PreparedView and shader reconstruction path.
    let precision = [(109_000.0, 9u8), (6_371_000.0, 15u8), (70_000_000.0, 19u8)];
    for (radius, level) in precision {
        for (face, face_name) in [
            ("positive_x", "px"),
            ("negative_x", "nx"),
            ("positive_y", "py"),
            ("negative_y", "ny"),
            ("positive_z", "pz"),
            ("negative_z", "nz"),
        ] {
            let address = [
                (0, 0, "corner"),
                ((1u32 << level) / 2, (1u32 << level) / 2, "center"),
                ((1u32 << level) - 1, (1u32 << level) / 2, "edge"),
            ];
            for (x, y, region) in address {
                configure(&mut demo, "rocky_v5", radius, face, level, x, y, 0)?;
                let name = format!("precision_r{}_{}_{}", radius as u64, face_name, region);
                capture(&mut demo, &output, &name)?;
            }
        }
    }

    demo.developer_apply_command(&DevCommand::GpuTile {
        enabled: false,
        family: "rocky_v5".into(),
        seed: 0,
        radius_m: 80_000.0,
        face: "positive_z".into(),
        level: 9,
        x: 157,
        y: 39,
        cells: 64,
        revision: 0,
    })?;
    Ok(())
}
