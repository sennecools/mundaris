//! 8-bit inspection images. Previews are for humans only and are never loaded
//! as data; each file name says what it shows.

use crate::bake::BakeOutput;
use crate::derive::{DerivedFields, downsample};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

const MAX_PREVIEW: usize = 1024;

fn write_png(path: &Path, side: usize, color: png::ColorType, data: &[u8]) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), side as u32, side as u32);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()?
        .write_image_data(data)
        .with_context(|| format!("encoding {}", path.display()))
}

fn normalized_u8(values: &[f64]) -> Vec<u8> {
    let (lo, hi) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let span = (hi - lo).max(1.0e-12);
    values
        .iter()
        .map(|v| ((v - lo) / span * 255.0).round() as u8)
        .collect()
}

fn unit_u8(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Lambert shading of the height field lit from the north-west at 45 degrees.
fn hillshade(height_m: &[f64], n: usize, cell_m: f64) -> Vec<f64> {
    let light = [-0.5f64, 0.5, std::f64::consts::FRAC_1_SQRT_2];
    let at = |x: usize, y: usize| height_m[(y % n) * n + (x % n)];
    let mut out = vec![0.0; n * n];
    for y in 0..n {
        for x in 0..n {
            let gx = (at(x + 1, y) - at(x + n - 1, y)) / (2.0 * cell_m);
            let gy = (at(x, y + 1) - at(x, y + n - 1)) / (2.0 * cell_m);
            let norm = (gx * gx + gy * gy + 1.0).sqrt();
            let shade = (-gx * light[0] - gy * light[1] + light[2]) / norm;
            out[y * n + x] = shade.clamp(0.0, 1.0);
        }
    }
    out
}

pub fn write_all(dir: &Path, output: &BakeOutput, derived: &DerivedFields) -> Result<()> {
    let n = output.resolution as usize;
    let factor = (n / MAX_PREVIEW).max(1);
    let side = n / factor;
    let reduce = |values: &[f64]| downsample(values, n, factor);

    write_png(
        &dir.join("height_normalized_preview.png"),
        side,
        png::ColorType::Grayscale,
        &normalized_u8(&reduce(&output.height_m)),
    )?;
    let shade = hillshade(&output.height_m, n, output.cell_size_m);
    write_png(
        &dir.join("hillshade_preview.png"),
        side,
        png::ColorType::Grayscale,
        &reduce(&shade)
            .iter()
            .map(|&v| unit_u8(v))
            .collect::<Vec<_>>(),
    )?;
    for (name, values) in [
        ("flow_preview.png", &derived.flow),
        ("spawn_preview.png", &derived.spawn),
    ] {
        write_png(
            &dir.join(name),
            side,
            png::ColorType::Grayscale,
            &reduce(values)
                .iter()
                .map(|&v| unit_u8(v))
                .collect::<Vec<_>>(),
        )?;
    }
    let wetness = reduce(&derived.wetness);
    let exposure = reduce(&derived.exposure);
    let clim: Vec<u8> = wetness
        .iter()
        .zip(&exposure)
        .flat_map(|(&w, &e)| [unit_u8(w), unit_u8(e), 0])
        .collect();
    write_png(
        &dir.join("clim_rg_preview.png"),
        side,
        png::ColorType::Rgb,
        &clim,
    )?;
    write_png(
        &dir.join("erosion_delta_preview.png"),
        side,
        png::ColorType::Grayscale,
        &normalized_u8(&reduce(&output.erosion_delta_m)),
    )
}
