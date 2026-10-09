//! Offline bake of periodic terrain library bundles (`mundaris.terrain-bundle.v1`).
//!
//! A validated recipe drives periodic base noise and grid hydraulic/thermal
//! erosion on the GPU, then derived semantic channels on the CPU. Bundles are
//! immutable published content; atlas pages built from them are disposable.
//! Contract: `docs/PLANET_DATA_PIPELINE.md`.
#![forbid(unsafe_code)]

pub mod bake;
pub mod bundle;
pub mod craters;
pub mod derive;
pub mod drainage;
pub mod evolution;
pub mod exemplar;
pub mod fft;
pub mod fluvial;
pub mod font;
pub mod gpu;
pub mod graph;
pub mod gullies;
pub mod inspect;
pub mod noise;
pub mod pipeline;
pub mod preview;
pub mod recipe;
pub mod score;
pub mod tiff;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn is_pipeline(path: &Path) -> Result<bool> {
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("reading recipe {}", path.display()))?,
    )
    .with_context(|| format!("parsing recipe {}", path.display()))?;
    Ok(value.get("schema").and_then(serde_json::Value::as_u64)
        == Some(u64::from(pipeline::PIPELINE_SCHEMA)))
}

/// Preview a recipe without publishing. Schema 1: the base only (graph or
/// layers, no erosion) at `resolution`. Schema 2: the whole pipeline, with a
/// hillshade after every stage (`NN_<name>.png`); `resolution` is ignored.
pub fn preview_base(recipe_path: &Path, out_dir: &Path, resolution: u32) -> Result<String> {
    if is_pipeline(recipe_path)? {
        let (recipe, _) = pipeline::PipelineRecipe::load(recipe_path)?;
        std::fs::create_dir_all(out_dir)?;
        let started = Instant::now();
        let mut lines = Vec::new();
        let mut last = Instant::now();
        let output = pipeline::run(&recipe, |index, name, height, n, cell_m| {
            let seconds = last.elapsed().as_secs_f64();
            preview::write_hillshade(
                &out_dir.join(format!("{index:02}_{name}.png")),
                height,
                n,
                cell_m,
            )?;
            let (lo, hi) = height
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                    (lo.min(v), hi.max(v))
                });
            let stage_metrics = pipeline::metrics(height, n, cell_m);
            lines.push(format!(
                "{index:02} {name} {n}² {seconds:.2} s, relief {:.0} m, slope p50/p99 {:.2}/{:.2}, finest band rms {:.2} m",
                hi - lo,
                stage_metrics["slope_p10_p50_p90_p99"][1].as_f64().unwrap_or(0.0),
                stage_metrics["slope_p10_p50_p90_p99"][3].as_f64().unwrap_or(0.0),
                stage_metrics["band_rms_m"][0][1].as_f64().unwrap_or(0.0),
            ));
            last = Instant::now();
            Ok(())
        })?;
        preview::write_base(
            out_dir,
            "final",
            &output.height_m,
            output.resolution as usize,
            output.cell_size_m,
        )?;
        let metrics = pipeline::metrics(
            &output.height_m,
            output.resolution as usize,
            output.cell_size_m,
        );
        std::fs::write(
            out_dir.join("metrics.json"),
            serde_json::to_string_pretty(&metrics)?,
        )?;
        lines.push(format!("metrics {metrics}"));
        lines.push(format!(
            "{}: pipeline {:.2} s",
            recipe.id,
            started.elapsed().as_secs_f64()
        ));
        return Ok(lines.join("\n"));
    }
    let (recipe, _) = recipe::Recipe::load(recipe_path)?;
    anyhow::ensure!(
        resolution.is_power_of_two() && (16..=4096).contains(&resolution),
        "preview resolution must be a power of two in 16..=4096"
    );
    let started = Instant::now();
    let height_m = if let Some(graph) = &recipe.graph {
        graph::Program::compile(graph, recipe.footprint_m, recipe.seed)?
            .evaluate(resolution as usize)
            .height_m
    } else {
        let base = recipe.base.as_ref().expect("validated recipe has a base");
        let (height, _) = gpu::Gpu::new()?.base(base, recipe.seed, resolution)?;
        height.iter().map(|&v| f64::from(v)).collect()
    };
    let seconds = started.elapsed().as_secs_f64();
    let (lo, hi) = height_m
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    preview::write_base(
        out_dir,
        &recipe.id,
        &height_m,
        resolution as usize,
        recipe.cell_size_m(resolution),
    )?;
    Ok(format!(
        "{} {resolution}²: base {seconds:.2} s, height {lo:.1}..{hi:.1} m",
        recipe.id
    ))
}

/// Bake `recipe_path` (schema 1 or 2) and publish the bundle under
/// `library_dir/<recipe id>`.
pub fn bake_and_publish(recipe_path: &Path, library_dir: &Path) -> Result<PathBuf> {
    let started = Instant::now();
    let (info, recipe_bytes, output, derived_recipe, adapter, peak) = if is_pipeline(recipe_path)? {
        let (recipe, bytes) = pipeline::PipelineRecipe::load(recipe_path)?;
        let output = pipeline::run(&recipe, |_, _, _, _, _| Ok(()))?;
        let adapter = gpu::AdapterIdentity {
            name: "none (CPU-only pipeline)".into(),
            backend: "none".into(),
            driver: String::new(),
            driver_info: String::new(),
            vendor: 0,
            device: 0,
        };
        (
            bundle::RecipeInfo::pipeline(&recipe)?,
            bytes,
            output,
            recipe.derived,
            adapter,
            0,
        )
    } else {
        let (recipe, bytes) = recipe::Recipe::load(recipe_path)?;
        let mut gpu = gpu::Gpu::new()?;
        let output = bake::bake(&recipe, &mut gpu)?;
        (
            bundle::RecipeInfo::layered(&recipe)?,
            bytes,
            output,
            recipe.derived,
            gpu.identity.clone(),
            gpu.peak_allocated_bytes,
        )
    };
    let derive_started = Instant::now();
    let derived = derive::derive(
        &output.height_m,
        &output.erosion_delta_m,
        output.resolution as usize,
        output.cell_size_m,
        &derived_recipe,
    );
    let derive_seconds = derive_started.elapsed().as_secs_f64();
    bundle::publish(
        library_dir,
        &bundle::PublishInputs {
            recipe: &info,
            recipe_bytes: &recipe_bytes,
            output: &output,
            derived: &derived,
            adapter: &adapter,
            total_seconds: started.elapsed().as_secs_f64(),
            derive_seconds,
            peak_gpu_allocated_bytes: peak,
        },
    )
}
