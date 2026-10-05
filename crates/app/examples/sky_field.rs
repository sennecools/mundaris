//! Diagnostic raw cached pixels and pre-quantization directional samples.
#[cfg(feature = "terrain-capture")]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use glam::DVec3;
    use std::{
        fs::{self, OpenOptions},
        io::{BufWriter, Write},
        path::PathBuf,
    };
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("usage: sky_field <new-output-directory>")?;
    anyhow::ensure!(!output.exists(), "output already exists");
    fs::create_dir_all(&output)?;
    let definition = mundaris_app::sky_definition::default_sky()?;
    for (index, image) in mundaris_renderer::sky::inspect_background(&definition)
        .into_iter()
        .enumerate()
    {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join(format!("cached-{index}.png")))?;
        let mut encoder = png::Encoder::new(BufWriter::new(file), image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&image.rgba)?;
    }
    let mut samples = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join("field-samples.csv"))?,
    );
    writeln!(
        samples,
        "longitude_rad,latitude_rad,linear_r,linear_g,linear_b"
    )?;
    for y in -100..=100 {
        for x in -100..=100 {
            let longitude = f64::from(x) * 0.004;
            let latitude = f64::from(y) * 0.002;
            let direction = DVec3::new(
                latitude.cos() * longitude.cos(),
                latitude.sin(),
                latitude.cos() * longitude.sin(),
            );
            let [r, g, b] = mundaris_renderer::sky::sample_background(&definition, direction)?;
            writeln!(samples, "{longitude},{latitude},{r},{g},{b}")?;
        }
    }
    Ok(())
}
#[cfg(not(feature = "terrain-capture"))]
fn main() {
    eprintln!("sky_field requires --features terrain-capture");
}
