//! Coarse-to-fine bake: each stage erodes at one resolution, then the result is
//! upsampled and the base-noise detail newly representable at the next
//! resolution is re-injected before eroding again.

use crate::drainage::fill_from_outlets;
use crate::fluvial;
use crate::gpu::Gpu;
use crate::recipe::{Process, Recipe};

/// Seed role for fluvial routing jitter (distinct from noise roles).
const ROUTING_ROLE: u32 = 32;
use anyhow::{Result, anyhow, ensure};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
pub struct StageReport {
    pub resolution: u32,
    pub iterations: u32,
    pub process: Process,
    pub cell_size_m: f64,
    pub base_seconds: f64,
    pub erosion_seconds: f64,
    /// |Σ(terrain + sediment) − Σ input| / (cells × input relief), all in cells.
    /// Only a conservation check when the stage is closed (no outlet).
    pub mass_relative_change: f64,
    /// Base-level outlet height, if the recipe opens the tile.
    pub outlet_level_m: Option<f64>,
    /// Cells raised by depression filling before erosion.
    pub filled_cells: usize,
    pub input_relief_m: f64,
    pub output_relief_m: f64,
    pub residual_water_mean_cells: f64,
}

pub struct BakeOutput {
    pub resolution: u32,
    pub cell_size_m: f64,
    /// Final height including deposited suspended sediment.
    pub height_m: Vec<f64>,
    /// Final stage output minus final stage input (positive = deposition).
    pub erosion_delta_m: Vec<f64>,
    pub stages: Vec<StageReport>,
}

fn relief(values: &[f64]) -> f64 {
    let (lo, hi) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    hi - lo
}

/// Periodic Catmull-Rom upsample of an `n²` grid to `2n²` at pixel centres.
pub fn upsample2(values: &[f64], n: usize) -> Vec<f64> {
    let m = 2 * n;
    let weights = |t: f64| {
        let t2 = t * t;
        let t3 = t2 * t;
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    };
    // Fine pixel centre x maps to coarse coordinate x/2 − 0.25, so each fine
    // pixel has one of two fixed fractional offsets.
    let taps = |fine: usize| {
        let c = fine as f64 * 0.5 - 0.25;
        let base = c.floor();
        (base as i64, weights(c - base))
    };
    let wrap = |i: i64| i.rem_euclid(n as i64) as usize;
    let mut horizontal = vec![0.0; n * m];
    for y in 0..n {
        for x in 0..m {
            let (b, w) = taps(x);
            horizontal[y * m + x] = (0..4)
                .map(|k| w[k] * values[y * n + wrap(b - 1 + k as i64)])
                .sum();
        }
    }
    let mut out = vec![0.0; m * m];
    for y in 0..m {
        let (b, w) = taps(y);
        for x in 0..m {
            out[y * m + x] = (0..4)
                .map(|k| w[k] * horizontal[wrap(b - 1 + k as i64) * m + x])
                .sum();
        }
    }
    out
}

pub fn bake(recipe: &Recipe, gpu: &mut Gpu) -> Result<BakeOutput> {
    recipe.validate()?;
    let mut previous: Option<(Vec<f64>, Vec<f64>)> = None; // (eroded, base) in metres
    let mut stages = Vec::new();
    let mut output = None;
    for stage in &recipe.stages {
        let n = stage.resolution as usize;
        let cell_m = recipe.cell_size_m(stage.resolution);
        let started = Instant::now();
        let (base_f32, hardness) = gpu.base(&recipe.base, recipe.seed, stage.resolution)?;
        let base: Vec<f64> = base_f32.iter().map(|&v| f64::from(v)).collect();
        let base_seconds = started.elapsed().as_secs_f64();

        let mut input_m: Vec<f64> = match &previous {
            None if stage.process == Process::Fluvial => base
                .iter()
                .map(|b| b * recipe.fluvial.initial_scale)
                .collect(),
            None => base.clone(),
            Some((eroded, previous_base)) => {
                let coarse = n / 2;
                let eroded_up = upsample2(eroded, coarse);
                let base_up = upsample2(previous_base, coarse);
                eroded_up
                    .iter()
                    .zip(&base)
                    .zip(&base_up)
                    .map(|((e, b), bu)| e + stage.detail_scale * (b - bu))
                    .collect()
            }
        };
        let input_min_m = input_m.iter().copied().fold(f64::INFINITY, f64::min);
        let outlet_level_m = (recipe.hydraulic.outlet_fraction > 0.0)
            .then(|| input_min_m + recipe.hydraulic.outlet_fraction * relief(&input_m));
        let outlet: Vec<bool> = input_m
            .iter()
            .map(|&h| outlet_level_m.is_some_and(|level| h <= level))
            .collect();
        let mut filled_cells = 0;
        if outlet_level_m.is_some() && recipe.hydraulic.fill_slope > 0.0 {
            filled_cells = fill_from_outlets(
                &mut input_m,
                n,
                &outlet,
                recipe.hydraulic.fill_slope * cell_m,
            );
        }
        let input_relief_m = relief(&input_m);

        let started = Instant::now();
        let (eroded_m, mass_relative_change, residual_water_mean_cells) = match stage.process {
            Process::Fluvial => {
                let mut height = input_m.clone();
                let (lo, hi) = base
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                        (lo.min(v), hi.max(v))
                    });
                let span = (hi - lo).max(1.0e-12);
                let uplift: Vec<f64> = base.iter().map(|b| (b - lo) / span).collect();
                let (hlo, hhi) = hardness
                    .iter()
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
                        (lo.min(v), hi.max(v))
                    });
                let hspan = f64::from(hhi - hlo).max(1.0e-12);
                let thermal = &recipe.thermal;
                let talus: Vec<f64> = hardness
                    .iter()
                    .map(|&h| {
                        let normalized = f64::from(h - hlo) / hspan;
                        thermal.talus_tangent
                            * (1.0 + thermal.talus_hardness * (2.0 * normalized - 1.0))
                    })
                    .collect();
                let erodibility_scale: Vec<f64> = hardness
                    .iter()
                    .map(|&h| 1.0 - recipe.fluvial.erodibility_hardness * f64::from(h))
                    .collect();
                fluvial::erode(
                    &mut height,
                    &fluvial::FluvialInputs {
                        n,
                        cell_m,
                        uplift: &uplift,
                        outlet: &outlet,
                        iterations: stage.iterations,
                        talus: &talus,
                        erodibility_scale: &erodibility_scale,
                        fill_slope: recipe.hydraulic.fill_slope,
                        seed: crate::noise::layer_seed(recipe.seed, ROUTING_ROLE, stage.resolution),
                    },
                    &recipe.fluvial,
                );
                let change = (height.iter().sum::<f64>() - input_m.iter().sum::<f64>()).abs()
                    / ((n * n) as f64 * input_relief_m.max(cell_m));
                (height, change, 0.0)
            }
            Process::Hydraulic => {
                let input_cells: Vec<f32> = input_m.iter().map(|&h| (h / cell_m) as f32).collect();
                let outlet_level_cells =
                    outlet_level_m.map_or(f32::MIN, |level| (level / cell_m) as f32);
                let result = gpu.erode(
                    &input_cells,
                    &hardness,
                    stage.resolution,
                    stage.iterations,
                    outlet_level_cells,
                    &recipe.hydraulic,
                    &recipe.thermal,
                )?;
                let sum_in: f64 = input_cells.iter().map(|&v| f64::from(v)).sum();
                let sum_out: f64 = result
                    .terrain
                    .iter()
                    .zip(&result.sediment)
                    .map(|(&t, &s)| f64::from(t) + f64::from(s))
                    .sum();
                let eroded: Vec<f64> = result
                    .terrain
                    .iter()
                    .zip(&result.sediment)
                    .map(|(&t, &s)| (f64::from(t) + f64::from(s)) * cell_m)
                    .collect();
                let change = (sum_out - sum_in).abs()
                    / ((n * n) as f64 * (input_relief_m / cell_m).max(1.0));
                let water =
                    result.water.iter().map(|&w| f64::from(w)).sum::<f64>() / (n * n) as f64;
                (eroded, change, water)
            }
        };
        let erosion_seconds = started.elapsed().as_secs_f64();
        ensure!(
            eroded_m.iter().all(|v| v.is_finite()),
            "erosion produced non-finite heights at resolution {n}; reduce dt or rates"
        );
        stages.push(StageReport {
            resolution: stage.resolution,
            iterations: stage.iterations,
            process: stage.process,
            cell_size_m: cell_m,
            base_seconds,
            erosion_seconds,
            mass_relative_change,
            outlet_level_m,
            filled_cells,
            input_relief_m,
            output_relief_m: relief(&eroded_m),
            residual_water_mean_cells,
        });
        let delta: Vec<f64> = eroded_m.iter().zip(&input_m).map(|(e, i)| e - i).collect();
        output = Some((eroded_m.clone(), delta, n as u32, cell_m));
        previous = Some((eroded_m, base));
    }
    let (mut height_m, mut erosion_delta_m, resolution, cell_size_m) =
        output.ok_or_else(|| anyhow!("recipe has no stages"))?;
    if let Some(target) = recipe.output_relief_m {
        let scale = target / relief(&height_m).max(1.0e-9);
        let lowest = height_m.iter().copied().fold(f64::INFINITY, f64::min);
        for h in &mut height_m {
            *h = lowest + (*h - lowest) * scale;
        }
        for d in &mut erosion_delta_m {
            *d *= scale;
        }
    }
    Ok(BakeOutput {
        resolution,
        cell_size_m,
        height_m,
        erosion_delta_m,
        stages,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsample_reproduces_constants_and_linear_periodic_ramps() {
        let n = 8;
        let constant = upsample2(&vec![2.5; n * n], n);
        assert!(constant.iter().all(|v| (v - 2.5).abs() < 1e-12));
        // A sinusoid at low frequency is reproduced closely and stays periodic.
        let wave: Vec<f64> = (0..n * n)
            .map(|i| (std::f64::consts::TAU * ((i % n) as f64 + 0.5) / n as f64).sin())
            .collect();
        let up = upsample2(&wave, n);
        let m = 2 * n;
        for (x, &value) in up.iter().take(m).enumerate() {
            let expected = (std::f64::consts::TAU * (x as f64 + 0.5) / m as f64).sin();
            assert!(
                (value - expected).abs() < 0.03,
                "x={x} {value} vs {expected}"
            );
        }
    }
}
