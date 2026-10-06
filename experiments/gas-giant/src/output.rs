//! Reproducible snapshots and a lightweight browser-based field viewer.
use std::{fs::{self, OpenOptions}, io::{BufWriter, Write}, path::Path};

use serde::Serialize;

use crate::solver::Solver;

/// Write one self-describing JSON snapshot and its companion diagnostic PNG.
/// Existing members of either output pair are never replaced.
pub fn write_frame(
    directory: &Path,
    solver: &Solver,
    diagnostics: &impl Serialize,
    vortices: &impl Serialize,
) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(directory)?;
    let f = solver.fields();
    let stem = format!("frame-{:08}", f.tick);
    let json_path = directory.join(format!("{stem}.json"));
    let png_path = directory.join(format!("{stem}.png"));
    if json_path.exists() || png_path.exists() {
        return Err(format!("snapshot pair already exists for tick {}", f.tick).into());
    }
    let panel_values = [f.u, f.vorticity, f.buoyancy, f.h, f.tracer];
    let names = ["u", "vorticity", "buoyancy_anomaly", "depth_anomaly", "tracer"];
    let mut ranges = serde_json::Map::new();
    for (name, values) in names.iter().zip(panel_values) {
        ranges.insert((*name).into(), range(values));
    }
    let speed: Vec<f64> = f.u.iter().zip(f.v).map(|(u, v)| u.hypot(*v)).collect();
    ranges.insert("speed".into(), range(&speed));
    let payload = serde_json::json!({
        "schema": 1,
        "model": format!("{:?}", solver.config().model).to_lowercase(),
        "geometry": "periodic_square_[0,2pi)^2_not_sphere",
        "units": { "u_v": "normalized_by_wave_speed", "vorticity": "nondimensional",
            "buoyancy": "effective_thermal_variable_not_kelvin", "h": "dimensionless_depth_anomaly",
            "tracer": "passive_scalar", "speed": "normalized_by_wave_speed",
            "time": "nondimensional; physical_time_s derived via config time_unit_s" },
        "resolution": f.resolution, "tick": f.tick,
        "time_nondimensional": f.time_nondimensional,
        "physical_time_s": f.time_nondimensional * solver.derived().time_unit_s,
        "config": solver.config(), "derived": solver.derived(),
        "diagnostics": diagnostics, "vortices": vortices,
        "field_ranges": ranges,
        "fields": { "u": f.u, "v": f.v, "vorticity": f.vorticity,
            "buoyancy_anomaly": f.buoyancy, "depth_anomaly": f.h,
            "tracer": f.tracer, "speed": speed }
    });
    // Serialize first; then reserve both names exclusively before writing either payload.
    let json = serde_json::to_vec_pretty(&payload)?;
    let json_file = OpenOptions::new().write(true).create_new(true).open(&json_path)?;
    let png_file = match OpenOptions::new().write(true).create_new(true).open(&png_path) {
        Ok(file) => file,
        Err(error) => { let _ = fs::remove_file(&json_path); return Err(error.into()); }
    };
    if let Err(error) = (|| -> Result<(), Box<dyn std::error::Error>> {
        use png::{BitDepth, ColorType};
        let n = f.resolution;
        let scale = 4usize;
        let mut image = vec![0u8; n * scale * 3 * n * scale * 2 * 3];
        let panels: [(&str, &[f64]); 6] = [("u", f.u), ("vorticity", f.vorticity),
            ("buoyancy_anomaly", f.buoyancy), ("depth_anomaly", f.h),
            ("tracer", f.tracer), ("speed", &speed)];
        for (panel, (name, values)) in panels.iter().enumerate() {
            let (lo, hi) = extrema(values);
            for y in 0..n * scale { for x in 0..n * scale {
                let value = values[(y / scale) * n + x / scale];
                let t = if hi > lo { ((value - lo) / (hi - lo)).clamp(0., 1.) } else { 0.5 };
                let color = palette(name, t);
                let px = (panel % 3) * n * scale + x;
                let py = (panel / 3) * n * scale + y;
                let offset = (py * n * scale * 3 + px * 3) as usize;
                image[offset..offset + 3].copy_from_slice(&color);
            }}
        }}
        let mut json_writer = BufWriter::new(json_file);
        json_writer.write_all(&json)?;
        json_writer.flush()?;
        let mut encoder = png::Encoder::new(BufWriter::new(png_file), (n * scale * 3) as u32, (n * scale * 2) as u32);
        encoder.set_color(ColorType::Rgb); encoder.set_depth(BitDepth::Eight);
        encoder.write_header()?.write_image_data(&image)?;
        Ok(())
    })() {
        let _ = fs::remove_file(json_path); let _ = fs::remove_file(png_path); return Err(error);
    }
    Ok(())
}

fn extrema(values: &[f64]) -> (f64, f64) {
    values.iter().copied().filter(|v| v.is_finite()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
}
fn range(values: &[f64]) -> serde_json::Value {
    let (min, max) = extrema(values);
    serde_json::json!({"min": min, "max": max, "normalization": "per-frame per-field linear min/max"})
}
fn palette(name: &str, t: f64) -> [u8; 3] {
    // Each diagnostic uses a distinct, deliberately non-geographic sequential/diverging map.
    let (a, b, c) = match name {
        "u" => ([25., 28., 95.], [245., 229., 155.], [180., 38., 45.]),
        "vorticity" => ([30., 88., 150.], [247., 244., 225.], [177., 36., 71.]),
        "buoyancy_anomaly" => ([34., 71., 120.], [238., 217., 162.], [164., 56., 37.]),
        "depth_anomaly" => ([38., 54., 108.], [232., 223., 181.], [81., 146., 128.]),
        "tracer" => ([15., 19., 38.], [100., 140., 186.], [242., 193., 108.]),
        _ => ([17., 25., 48.], [60., 151., 176.], [255., 224., 126.]),
    };
    let (left, right, q) = if t < 0.5 { (a, b, t * 2.) } else { (b, c, (t - 0.5) * 2.) };
    std::array::from_fn(|i| (left[i] * (1. - q) + right[i] * q).round() as u8)
}

/// Install the self-contained viewer beside snapshots.
pub fn write_viewer(directory: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(directory.join("viewer.html"), include_str!("../viewer.html"))?;
    Ok(())
}
