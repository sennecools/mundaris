//! Bounded host-stage measurements; not native FPS or a gravity-solver comparison.
use anyhow::{Context, Result, ensure};
use mundaris_app::GravityOrbitsDemo;
use serde_json::json;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .context("provide a fresh output JSON path")?;
    ensure!(!output.exists(), "refusing to replace existing evidence");
    let mut fixtures = Vec::new();
    for real_scale in [false, true] {
        for time_s in [0.0, 31_557_600_000.0, -31_557_600_000.0] {
            let mut app = GravityOrbitsDemo::solar_system(real_scale)?;
            app.seek_seconds(time_s)?;
            app.set_playback_rate(1.0)?;
            app.set_paused(false)?;
            for _ in 0..5 {
                app.update(Duration::from_millis(1));
            }
            let mut samples = Vec::new();
            for index in 0..30 {
                let started = Instant::now();
                app.update(Duration::from_millis(1));
                let update_us = started.elapsed().as_secs_f64() * 1e6;
                let motion = app.motion_snapshot();
                ensure!(
                    motion.latest_failure.is_none() && !motion.paused,
                    "measurement fixture stopped: {motion:?}"
                );
                samples.push(json!({"index":index, "requested_time_s":motion.requested_time_s,
                    "published_time_s":motion.published_time_s, "world_revision":app.world().revision(),
                    "sampling_us":motion.sampling_ms.map(|x|x*1000.0),
                    "frame_publication_us":motion.publication_ms.map(|x|x*1000.0),
                    "app_update_us":update_us, "body_count":motion.analytic_body_count,
                    "solver_iterations":motion.solver_iterations}));
            }
            fixtures.push(json!({"real_scale":real_scale,"start_time_s":time_s,"warmup_count":5,"sample_count":30,"samples":samples}));
        }
    }
    let result = json!({"profile":if cfg!(debug_assertions){"debug"}else{"release"},
        "sampling_scope":"complete AnalyticMotionProducer candidate evaluation + transactional world commit",
        "publication_scope":"CelestialFrameProjection publication + coherent view validation",
        "app_update_scope":"GravityOrbitsDemo::update including motion, guides, trails and navigation; excludes render-time terrain admission, render preparation, GPU and presentation",
        "fixtures":fixtures});
    std::fs::write(&output, serde_json::to_vec_pretty(&result)?)?;
    println!("{}", output.display());
    Ok(())
}
