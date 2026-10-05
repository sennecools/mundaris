//! Focused controller and complete-terrain query timings, excluding rendering/I/O.
use anyhow::Result;
use glam::DVec3;
use mundaris_app::{celestial_camera::*, solar_system::SolarSystemPreset, terrain_inspection};
use mundaris_world::CelestialFrameProjection;
use std::{
    hint::black_box,
    num::NonZeroU64,
    time::{Duration, Instant},
};

fn main() -> Result<()> {
    let namespace = NonZeroU64::new(5128).ok_or_else(|| anyhow::anyhow!("namespace"))?;
    let world = SolarSystemPreset::gameplay().create(namespace)?;
    let frames = CelestialFrameProjection::build(&world, namespace)?;
    let pair = frames.coherent_view(&world)?;
    let earth = world
        .bodies()
        .nth(3)
        .ok_or_else(|| anyhow::anyhow!("Earth"))?
        .0;
    let mut seed = CelestialCamera::overview(&pair, DVec3::ZERO, 1e11)?;
    seed.focus(&pair, earth, true, true)?;
    seed.target_clearance(&pair, 100.0)?;
    seed.update_navigation(&pair, &NavigationInput::default(), Duration::from_secs(2))?;
    seed.enter_surface_inspection(&pair, earth)?;
    for fixture in ["idle", "look", "tangent"] {
        let input = NavigationInput {
            drag: if fixture == "look" {
                [0.1, 0.05]
            } else {
                [0.0; 2]
            },
            translation: if fixture == "tangent" {
                DVec3::X
            } else {
                DVec3::ZERO
            },
            ..Default::default()
        };
        let mut samples = Vec::new();
        let mut query_count = 0;
        let mut query_us = 0.0;
        for _ in 0..20 {
            let mut camera = seed.clone();
            let start = Instant::now();
            for _ in 0..1000 {
                camera.update_navigation(
                    black_box(&pair),
                    black_box(&input),
                    Duration::from_millis(1),
                )?;
            }
            samples.push(start.elapsed().as_secs_f64() * 1e6 / 1000.0);
            black_box(camera.pose());
            let diagnostics = camera.navigation_diagnostics();
            let baseline = seed.navigation_diagnostics();
            query_count += diagnostics.terrain_query_count - baseline.terrain_query_count;
            query_us += diagnostics.terrain_query_us - baseline.terrain_query_us;
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "fixture={fixture} samples=20 calls_per_sample=1000 controller_us_median={:.6} min={:.6} max={:.6}",
            samples[10], samples[0], samples[19]
        );
        println!(
            "fixture={fixture} incremental_complete_queries={query_count} total_query_us={query_us:.6} total_controller_calls=20000"
        );
    }
    let start = Instant::now();
    for _ in 0..20000 {
        black_box(terrain_inspection::terrain_clearance(
            &pair,
            seed.pose(),
            earth,
        )?);
    }
    println!(
        "fixture=complete_query samples=20000 query_us_mean={:.6}",
        start.elapsed().as_secs_f64() * 1e6 / 20000.0
    );
    Ok(())
}
