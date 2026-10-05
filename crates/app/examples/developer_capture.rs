use anyhow::{Context, Result, ensure};
use mundaris_app::developer_capture::{
    DeveloperScene, capture_analytic_playback, capture_navigation_route, capture_scene,
    capture_scene_at, default_output_directory, write_pair,
};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let scene_arg = args.next();
    let second = args.next();
    let (time_s, output) = match second.as_deref().and_then(|s| s.parse::<f64>().ok()) {
        Some(time_s) => (
            Some(time_s),
            args.next()
                .map(PathBuf::from)
                .unwrap_or_else(default_output_directory),
        ),
        None => (
            None,
            second
                .map(PathBuf::from)
                .unwrap_or_else(default_output_directory),
        ),
    };
    ensure!(
        args.next().is_none(),
        "usage: developer_capture [<scene> [<signed_time_s> [<output_directory>]]]"
    );
    if scene_arg.as_deref() == Some("navigation-route") {
        capture_navigation_route(&output)?;
        println!(
            "wrote ordinary-control navigation route to {}",
            output.display()
        );
    } else if scene_arg.as_deref() == Some("analytic-playback") {
        ensure!(
            time_s.is_none(),
            "analytic-playback does not accept an explicit time"
        );
        capture_analytic_playback(&output)?;
        println!("wrote analytic playback sequence to {}", output.display());
    } else if scene_arg.as_deref() == Some("all") {
        for scene in DeveloperScene::ALL {
            let capture = capture_scene(scene)?;
            write_pair(&output, &capture)?;
            println!("wrote {} capture to {}", scene.name(), output.display());
        }
    } else {
        let scene = scene_arg
            .as_deref()
            .unwrap_or("earth-orbit")
            .parse::<DeveloperScene>()
            .context("parsing developer capture scene")?;
        let capture = if let Some(time_s) = time_s {
            capture_scene_at(scene, time_s)?
        } else {
            capture_scene(scene)?
        };
        write_pair(&output, &capture)?;
        println!("wrote {} capture to {}", scene.name(), output.display());
    }
    Ok(())
}
