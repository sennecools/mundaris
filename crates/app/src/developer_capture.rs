//! Paired native capture data and safe PNG/JSON publication.

use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::developer_snapshot::DeveloperSnapshot;

#[derive(Debug)]
pub struct DeveloperCapture {
    pub rgba: Vec<u8>,
    pub snapshot: DeveloperSnapshot,
}

/// Publish the native image and its snapshot as one non-overwriting pair.
pub fn write_pair(output: &Path, capture: &DeveloperCapture) -> Result<()> {
    let metadata = capture
        .snapshot
        .capture
        .as_ref()
        .context("capture metadata missing")?;
    ensure!(
        capture.rgba.len() == metadata.width as usize * metadata.height as usize * 4,
        "capture RGBA byte count does not match its dimensions"
    );
    let image = output.join(&metadata.image);
    let json = output.join(format!("{}.json", metadata.scene));
    ensure!(
        !image.exists() && !json.exists(),
        "refusing to replace existing capture files for {}",
        metadata.scene
    );
    fs::create_dir_all(output)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let staging = output.join(format!(".capture-{}-{stamp}", std::process::id()));
    fs::create_dir(&staging)?;
    let staged_image = staging.join("frame.png");
    let staged_json = staging.join("frame.json");
    let mut image_published = false;
    let mut json_published = false;
    let result = (|| -> Result<()> {
        let file = fs::File::create(&staged_image)?;
        let mut encoder = png::Encoder::new(file, metadata.width, metadata.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&capture.rgba)?;
        writer.finish()?;
        capture.snapshot.write_json(&staged_json)?;
        fs::hard_link(&staged_image, &image)
            .context("publishing capture PNG without replacement")?;
        image_published = true;
        fs::hard_link(&staged_json, &json)
            .context("publishing capture JSON without replacement")?;
        json_published = true;
        Ok(())
    })();
    if result.is_err() {
        if image_published {
            fs::remove_file(&image).context("rolling back owned capture PNG")?;
        }
        if json_published {
            fs::remove_file(&json).context("rolling back owned capture JSON")?;
        }
    }
    for path in [&staged_image, &staged_json] {
        if path.exists() {
            fs::remove_file(path).context("cleaning capture staging file")?;
        }
    }
    fs::remove_dir(&staging).context("cleaning capture staging directory")?;
    result?;
    Ok(())
}

pub fn default_output_directory() -> PathBuf {
    PathBuf::from("target/astrum-diagnostics")
}
