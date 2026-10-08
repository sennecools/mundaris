//! Offline bake of periodic terrain library bundles (`mundaris.terrain-bundle.v1`).
//!
//! A validated recipe drives periodic base noise and grid hydraulic/thermal
//! erosion on the GPU, then derived semantic channels on the CPU. Bundles are
//! immutable published content; atlas pages built from them are disposable.
//! Contract: `docs/PLANET_DATA_PIPELINE.md`.
#![forbid(unsafe_code)]

pub mod bake;
pub mod bundle;
pub mod derive;
pub mod drainage;
pub mod fluvial;
pub mod gpu;
pub mod noise;
pub mod preview;
pub mod recipe;

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Bake `recipe_path` and publish the bundle under `library_dir/<recipe id>`.
pub fn bake_and_publish(recipe_path: &Path, library_dir: &Path) -> Result<PathBuf> {
    let started = Instant::now();
    let (recipe, recipe_bytes) = recipe::Recipe::load(recipe_path)?;
    let mut gpu = gpu::Gpu::new()?;
    let output = bake::bake(&recipe, &mut gpu)?;
    let derive_started = Instant::now();
    let derived = derive::derive(
        &output.height_m,
        &output.erosion_delta_m,
        output.resolution as usize,
        output.cell_size_m,
        &recipe.derived,
    );
    let derive_seconds = derive_started.elapsed().as_secs_f64();
    bundle::publish(
        library_dir,
        &bundle::PublishInputs {
            recipe: &recipe,
            recipe_bytes: &recipe_bytes,
            output: &output,
            derived: &derived,
            adapter: &gpu.identity,
            total_seconds: started.elapsed().as_secs_f64(),
            derive_seconds,
            peak_gpu_allocated_bytes: gpu.peak_allocated_bytes,
        },
    )
}
