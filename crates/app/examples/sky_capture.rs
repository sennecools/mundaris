//! Usage: sky_capture <output-directory> [width height] [sequence-frames]

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output = args.next().map(PathBuf::from).ok_or_else(|| {
        anyhow::anyhow!("usage: sky_capture <output-directory> [width height] [sequence-frames]")
    })?;
    let width_arg = args.next();
    let height_arg = args.next();
    anyhow::ensure!(
        width_arg.is_some() == height_arg.is_some(),
        "width and height must be supplied together"
    );
    let width = width_arg
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(960);
    let height = height_arg
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(640);
    let sequence_frames = args
        .next()
        .map(|v| v.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(0);
    if args.next().is_some() {
        anyhow::bail!("usage: sky_capture <output-directory> [width height] [sequence-frames]");
    }
    mundaris_app::sky_capture::capture_first_look_with_options(
        &output,
        mundaris_app::sky_capture::SkyCaptureOptions {
            width,
            height,
            sequence_frames,
        },
    )
}
