//! Filter pipeline (recipe schema 2): the base node graph followed by an ordered
//! list of grid filters, Gaea / World Machine style. Each stage may carry a mask
//! (height, slope, drainage area, noise) that blends its result with the input,
//! so processes can be applied selectively. Every filter works on a periodic
//! grid, so the output still tiles. Stages can change resolution, so erosion runs
//! coarse to fine (multi-scale).
//!
//! Filters:
//! - `base`: evaluate the graph (height, hardness, uplift) at a resolution.
//! - `exemplar_synthesis`: alternative first stage; quilt a periodic height grid
//!   from public-domain DTM windows (exemplar.rs). The graph then only supplies
//!   hardness (use constants).
//! - `resample`: change resolution (periodic Catmull-Rom up, box down); hardness
//!   and uplift are re-evaluated from the graph.
//! - `stream_power`: implicit stream-power incision with uplift (fluvial.rs).
//! - `droplets`: particle hydraulic erosion (after Beyer 2015).
//! - `thermal`: isotropic talus relaxation, hardness-aware, mass conserving.
//! - `channel_carve`: carve channels with depth growing with drainage area.
//! - `deposit`: raise low-slope, high-drainage ground toward its surroundings.
//! - `smooth`, `detail`, `warp`, `strata`, `ridge_sharpen`, `relief`,
//!   `snapshot`, `blend_snapshot`.

use crate::bake::{BakeOutput, outlet_mask, upsample2};
use crate::derive::{box_blur, downsample, flow_accumulation};
use crate::drainage::{breach_from_outlets, fill_depressions, fill_from_outlets};
use crate::fluvial;
use crate::graph::{GraphRecipe, Program};
use crate::noise::{layer_seed, layer_value, pcg};
use crate::recipe::{DerivedRecipe, FluvialRecipe, NoiseKind, NoiseLayer};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

pub const PIPELINE_SCHEMA: u32 = 2;
const MAX_STAGES: usize = 64;
const NOISE_ROLE_BASE: u32 = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PipelineRecipe {
    pub schema: u32,
    pub id: String,
    pub version: u32,
    pub seed: u64,
    pub footprint_m: f64,
    pub graph: GraphRecipe,
    pub pipeline: Vec<Stage>,
    pub derived: DerivedRecipe,
    /// Final relief: heights are scaled about their minimum to span this.
    #[serde(default)]
    pub output_relief_m: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    /// Label for reports and previews; defaults to the filter kind.
    #[serde(default)]
    pub name: Option<String>,
    pub filter: Filter,
    #[serde(default)]
    pub mask: Option<Mask>,
}

/// Periodic noise for filters and masks; evaluated in tile coordinates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseSpec {
    pub kind: NoiseKind,
    pub frequency: u32,
    pub octaves: u32,
    pub gain: f64,
    pub seed: u32,
    #[serde(default = "yes")]
    pub rotate_octaves: bool,
    #[serde(default = "two")]
    pub sharpness: f64,
}

fn yes() -> bool {
    true
}
fn two() -> f64 {
    2.0
}

impl NoiseSpec {
    fn layer(&self) -> NoiseLayer {
        NoiseLayer {
            kind: self.kind,
            frequency: self.frequency,
            octaves: self.octaves,
            lacunarity: 2,
            gain: self.gain,
            weight: 1.0,
            sharpness: self.sharpness,
            rotate_octaves: self.rotate_octaves,
            slope_damping: 0.0,
            domain: None,
        }
    }

    fn grid(&self, n: usize, recipe_seed: u64, role_offset: u32) -> Vec<f64> {
        let layer = self.layer();
        let role = NOISE_ROLE_BASE + self.seed * 2 + role_offset;
        let mut out = vec![0.0; n * n];
        parallel_rows(&mut out, n, |x, y| {
            let u = (x as f64 + 0.5) / n as f64;
            let v = (y as f64 + 0.5) / n as f64;
            layer_value(&layer, u, v, recipe_seed, role)
        });
        out
    }
}

/// Selective application: the product of the given smoothstep ramps.
/// A ramp `[a, b]` rises from 0 at a to 1 at b (falls if a > b).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    /// Height as a fraction of the current relief (0 = lowest, 1 = highest).
    #[serde(default)]
    pub height: Option<[f64; 2]>,
    /// Slope as rise over run.
    #[serde(default)]
    pub slope: Option<[f64; 2]>,
    /// log10 of the drainage area in m².
    #[serde(default)]
    pub flow: Option<[f64; 2]>,
    #[serde(default)]
    pub noise: Option<NoiseMask>,
    #[serde(default)]
    pub invert: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseMask {
    pub noise: NoiseSpec,
    pub ramp: [f64; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StreamPowerFilter {
    pub iterations: u32,
    pub fluvial: FluvialRecipe,
    /// Stable slope (rise over run) for the per-receiver talus limit.
    pub talus: f64,
    /// Talus spread with hardness: talus × (1 + talus_hardness × (2h − 1)).
    #[serde(default)]
    pub talus_hardness: f64,
    pub outlet_fraction: f64,
    pub outlet_points: u32,
    /// Breach depressions (carve spill paths) instead of filling them.
    #[serde(default)]
    pub breach: bool,
    pub fill_slope: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DropletFilter {
    /// Droplets released per grid cell (at random positions).
    pub droplets_per_cell: f64,
    pub max_steps: u32,
    /// Direction persistence 0..1.
    pub inertia: f64,
    /// Sediment capacity factor.
    pub capacity: f64,
    /// Minimum slope used for capacity (avoids zero capacity on flats).
    pub min_slope: f64,
    pub erode: f64,
    pub deposit: f64,
    pub evaporate: f64,
    pub gravity: f64,
    /// Droplet height units per cell for a slope of 1 (45°). The classic
    /// parameters assume heights normalized to about 1 over a few hundred cells,
    /// i.e. per-cell drops near 0.01; 0.01 reproduces that for real slopes.
    pub slope_scale: f64,
    /// Erosion brush radius in cells.
    pub radius_cells: f64,
    /// Erodibility multiplier is 1 − hardness_factor × hardness.
    #[serde(default)]
    pub hardness_factor: f64,
    pub seed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Filter {
    Base {
        resolution: u32,
    },
    /// First stage in place of `base`: example-based synthesis (exemplar.rs).
    ExemplarSynthesis(crate::exemplar::ExemplarRecipe),
    Resample {
        resolution: u32,
        /// Re-evaluate the graph base exactly at the new resolution and only
        /// interpolate the residual (everything stages added). Avoids
        /// interpolation ripples on large smooth relief.
        #[serde(default)]
        rebase: bool,
    },
    StreamPower(StreamPowerFilter),
    Droplets(DropletFilter),
    Thermal {
        iterations: u32,
        /// Stable slope (rise over run).
        talus: f64,
        /// Fraction of the excess moved per iteration, 0..1.
        rate: f64,
        #[serde(default)]
        talus_hardness: f64,
    },
    ChannelCarve {
        /// Carve depth at and above `reference_area_m2`.
        depth_m: f64,
        reference_area_m2: f64,
        /// Channels start at this drainage area.
        min_area_m2: f64,
        /// depth ∝ (area / reference)^exponent below the reference.
        exponent: f64,
        /// Channel cross-section width (box-blur diameter).
        width_m: f64,
    },
    Deposit {
        /// Ground flatter than this slope collects sediment (full below half).
        max_slope: f64,
        /// log10 drainage area (m²) where deposition starts / is full.
        flow: [f64; 2],
        /// Window radius of the surroundings the floor is raised toward.
        radius_m: f64,
        /// Maximum raise.
        depth_m: f64,
    },
    Smooth {
        radius_m: f64,
        strength: f64,
    },
    Detail {
        noise: NoiseSpec,
        amplitude_m: f64,
    },
    Warp {
        noise: NoiseSpec,
        amplitude_m: f64,
    },
    Strata {
        steps: u32,
        exponent: f64,
    },
    RidgeSharpen {
        radius_m: f64,
        amount: f64,
    },
    Relief {
        relief_m: f64,
    },
    /// Landslide limit: lower every cell so no slope exceeds `talus` (rise over
    /// run, optionally spread by hardness). Exact in one pass, never raises
    /// ground, not mass conserving (failed material leaves the tile, as in
    /// threshold-hillslope models). Unlike `thermal`, it fixes long over-steep
    /// slopes immediately instead of relaxing them only from their ends.
    SlopeLimit {
        talus: f64,
        #[serde(default)]
        talus_hardness: f64,
    },
    /// Impact-history simulation (`evolution.rs`).
    ImpactEvolution(crate::evolution::EvolutionRecipe),
    /// Branching downslope gully/rill texture (gullies.rs).
    Gullies {
        gullies: crate::recipe::GullyRecipe,
        seed: u32,
    },
    Snapshot {
        id: String,
    },
    BlendSnapshot {
        id: String,
        amount: f64,
    },
}

impl Filter {
    fn kind(&self) -> &'static str {
        match self {
            Filter::Base { .. } => "base",
            Filter::ExemplarSynthesis(_) => "exemplar_synthesis",
            Filter::Resample { .. } => "resample",
            Filter::StreamPower(_) => "stream_power",
            Filter::Droplets(_) => "droplets",
            Filter::Thermal { .. } => "thermal",
            Filter::ChannelCarve { .. } => "channel_carve",
            Filter::Deposit { .. } => "deposit",
            Filter::Smooth { .. } => "smooth",
            Filter::Detail { .. } => "detail",
            Filter::Warp { .. } => "warp",
            Filter::Strata { .. } => "strata",
            Filter::RidgeSharpen { .. } => "ridge_sharpen",
            Filter::Relief { .. } => "relief",
            Filter::SlopeLimit { .. } => "slope_limit",
            Filter::ImpactEvolution(_) => "impact_evolution",
            Filter::Gullies { .. } => "gullies",
            Filter::Snapshot { .. } => "snapshot",
            Filter::BlendSnapshot { .. } => "blend_snapshot",
        }
    }
}

fn finite_in(value: f64, lo: f64, hi: f64) -> bool {
    value.is_finite() && (lo..=hi).contains(&value)
}

impl PipelineRecipe {
    pub fn load(path: &Path) -> Result<(Self, Vec<u8>)> {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading recipe {}", path.display()))?;
        let recipe: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing pipeline recipe {}", path.display()))?;
        recipe.validate()?;
        Ok((recipe, bytes))
    }

    /// The exemplar synthesis of a recipe that starts from public-domain DTM
    /// windows (its bundle is `public-domain-derived`).
    pub fn exemplar(&self) -> Option<&crate::exemplar::ExemplarRecipe> {
        self.pipeline.iter().find_map(|s| match &s.filter {
            Filter::ExemplarSynthesis(e) => Some(e),
            _ => None,
        })
    }

    /// Resolution after the last resolution-setting stage.
    pub fn output_resolution(&self) -> u32 {
        self.pipeline
            .iter()
            .filter_map(|s| match &s.filter {
                Filter::Base { resolution } | Filter::Resample { resolution, .. } => {
                    Some(*resolution)
                }
                Filter::ExemplarSynthesis(e) => Some(e.resolution),
                _ => None,
            })
            .next_back()
            .unwrap_or(0)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == PIPELINE_SCHEMA,
            "pipeline recipes use schema {PIPELINE_SCHEMA}"
        );
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 64
                && self
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "recipe id must be 1..=64 of [a-z0-9_]"
        );
        ensure!(
            finite_in(self.footprint_m, 16.0, 1.0e6),
            "footprint_m outside 16..=1e6"
        );
        crate::graph::check(&self.graph, self.footprint_m)?;
        ensure!(
            !self.pipeline.is_empty() && self.pipeline.len() <= MAX_STAGES,
            "pipeline needs 1..={MAX_STAGES} stages"
        );
        ensure!(
            matches!(
                self.pipeline[0].filter,
                Filter::Base { .. } | Filter::ExemplarSynthesis(_)
            ),
            "the first stage must be base or exemplar_synthesis"
        );
        let exemplar_start = matches!(self.pipeline[0].filter, Filter::ExemplarSynthesis(_));
        let mut snapshots = Vec::new();
        for (i, stage) in self.pipeline.iter().enumerate() {
            let at = |m: &str| format!("stage {i} ({}): {m}", stage.filter.kind());
            match &stage.filter {
                Filter::Base { resolution } | Filter::Resample { resolution, .. } => {
                    ensure!(
                        resolution.is_power_of_two() && (16..=4096).contains(resolution),
                        at("resolution must be a power of two in 16..=4096")
                    );
                    if let Filter::Resample { rebase: true, .. } = &stage.filter {
                        ensure!(
                            !exemplar_start,
                            at("rebase needs a graph base; the exemplar start has none")
                        );
                    }
                }
                Filter::ExemplarSynthesis(e) => {
                    ensure!(i == 0, at("only valid as the first stage"));
                    ensure!(stage.mask.is_none(), at("the first stage takes no mask"));
                    e.validate().with_context(|| at("invalid parameters"))?;
                }
                Filter::StreamPower(s) => {
                    ensure!(s.iterations <= 100_000, at("iterations > 100000"));
                    ensure!(
                        finite_in(s.talus, 0.01, 20.0)
                            && finite_in(s.talus_hardness, 0.0, 0.9)
                            && finite_in(s.outlet_fraction, 0.0001, 0.5)
                            && finite_in(s.fill_slope, 0.0001, 0.5)
                            && s.outlet_points <= 4096,
                        at("talus/outlet/fill parameters out of range")
                    );
                    let f = &s.fluvial;
                    ensure!(
                        finite_in(f.dt, 0.0, 1.0e3)
                            && finite_in(f.erodibility, 0.0, 1.0)
                            && finite_in(f.area_exponent, 0.1, 1.0)
                            && finite_in(f.uplift_m_per_step, 0.0, 100.0)
                            && finite_in(f.diffusion_m2_per_step, 0.0, 1.0e4)
                            && finite_in(f.routing_jitter, 0.0, 1.0)
                            && finite_in(f.erodibility_hardness, 0.0, 1.0)
                            && finite_in(f.mfd_exponent, 0.0, 10.0)
                            && finite_in(f.threshold_m_per_step, 0.0, 100.0),
                        at("fluvial parameters out of range")
                    );
                }
                Filter::Droplets(d) => ensure!(
                    finite_in(d.droplets_per_cell, 0.0, 64.0)
                        && (1..=4096).contains(&d.max_steps)
                        && finite_in(d.inertia, 0.0, 1.0)
                        && finite_in(d.capacity, 0.0, 1000.0)
                        && finite_in(d.min_slope, 0.0, 10.0)
                        && finite_in(d.erode, 0.0, 1.0)
                        && finite_in(d.deposit, 0.0, 1.0)
                        && finite_in(d.evaporate, 0.0, 1.0)
                        && finite_in(d.gravity, 0.0, 100.0)
                        && finite_in(d.radius_cells, 0.5, 16.0)
                        && finite_in(d.slope_scale, 1.0e-4, 10.0)
                        && finite_in(d.hardness_factor, 0.0, 1.0),
                    at("droplet parameters out of range")
                ),
                Filter::Thermal {
                    iterations,
                    talus,
                    rate,
                    talus_hardness,
                } => ensure!(
                    *iterations <= 100_000
                        && finite_in(*talus, 0.01, 20.0)
                        && finite_in(*rate, 0.0, 1.0)
                        && finite_in(*talus_hardness, 0.0, 0.9),
                    at("thermal parameters out of range")
                ),
                Filter::ChannelCarve {
                    depth_m,
                    reference_area_m2,
                    min_area_m2,
                    exponent,
                    width_m,
                } => ensure!(
                    finite_in(*depth_m, 0.0, 1000.0)
                        && finite_in(*reference_area_m2, 1.0, 1.0e12)
                        && finite_in(*min_area_m2, 0.0, *reference_area_m2)
                        && finite_in(*exponent, 0.0, 4.0)
                        && finite_in(*width_m, 0.0, self.footprint_m * 0.25),
                    at("channel parameters out of range")
                ),
                Filter::Deposit {
                    max_slope,
                    flow,
                    radius_m,
                    depth_m,
                } => ensure!(
                    finite_in(*max_slope, 0.0, 10.0)
                        && flow.iter().all(|v| finite_in(*v, 0.0, 12.0))
                        && finite_in(*radius_m, 0.0, self.footprint_m * 0.25)
                        && finite_in(*depth_m, 0.0, 1000.0),
                    at("deposit parameters out of range")
                ),
                Filter::Smooth { radius_m, strength } => ensure!(
                    finite_in(*radius_m, 0.0, self.footprint_m * 0.25)
                        && finite_in(*strength, 0.0, 1.0),
                    at("smooth parameters out of range")
                ),
                Filter::Detail { noise, amplitude_m } | Filter::Warp { noise, amplitude_m } => {
                    crate::recipe::validate_layer(&noise.layer()).with_context(|| at("noise"))?;
                    ensure!(
                        finite_in(*amplitude_m, -1000.0, 1000.0),
                        at("amplitude out of range")
                    );
                }
                Filter::Strata { steps, exponent } => ensure!(
                    (1..=256).contains(steps) && finite_in(*exponent, 1.0, 32.0),
                    at("strata needs steps 1..=256 and exponent 1..=32")
                ),
                Filter::RidgeSharpen { radius_m, amount } => ensure!(
                    finite_in(*radius_m, 0.0, self.footprint_m * 0.25)
                        && finite_in(*amount, -4.0, 4.0),
                    at("ridge parameters out of range")
                ),
                Filter::Gullies { gullies: g, .. } => ensure!(
                    (1..=8).contains(&g.octaves)
                        && finite_in(g.wavelength_m, 1.0, self.footprint_m)
                        && finite_in(g.lacunarity, 1.2, 4.0)
                        && finite_in(g.amplitude_m, 0.0, 1000.0)
                        && finite_in(g.gain, 0.0, 1.0)
                        && finite_in(g.cell_ratio, 0.25, 4.0)
                        && finite_in(g.slope_full, 0.001, 10.0)
                        && finite_in(g.detail, 0.0, 10.0)
                        && finite_in(g.fade_radius_m, 1.0, self.footprint_m * 0.25),
                    at("gully parameters out of range")
                ),
                Filter::Relief { relief_m } => {
                    ensure!(
                        finite_in(*relief_m, 1.0, 10_000.0),
                        at("relief out of range")
                    )
                }
                Filter::ImpactEvolution(recipe) => recipe
                    .validate(self.footprint_m)
                    .with_context(|| at("impact_evolution"))?,
                Filter::SlopeLimit {
                    talus,
                    talus_hardness,
                } => ensure!(
                    finite_in(*talus, 0.05, 20.0) && finite_in(*talus_hardness, 0.0, 0.9),
                    at("slope limit parameters out of range")
                ),
                Filter::Snapshot { id } => snapshots.push(id.clone()),
                Filter::BlendSnapshot { id, amount } => {
                    ensure!(snapshots.contains(id), at("unknown snapshot"));
                    ensure!(finite_in(*amount, 0.0, 1.0), at("amount outside 0..=1"));
                }
            }
            if let Some(mask) = &stage.mask {
                for ramp in [mask.height, mask.slope, mask.flow].into_iter().flatten() {
                    ensure!(
                        ramp.iter().all(|v| v.is_finite()) && ramp[0] != ramp[1],
                        at("mask ramps need finite, distinct ends")
                    );
                }
                if let Some(noise) = &mask.noise {
                    crate::recipe::validate_layer(&noise.noise.layer())
                        .with_context(|| at("mask noise"))?;
                }
            }
        }
        let out = self.output_resolution();
        let d = &self.derived;
        ensure!(
            d.channel_resolution.is_power_of_two()
                && d.channel_resolution >= 16
                && d.channel_resolution <= out,
            "derived.channel_resolution must be a power of two in 16..=output resolution"
        );
        if let Some(relief) = self.output_relief_m {
            ensure!(
                finite_in(relief, 1.0, 10_000.0),
                "output_relief_m outside 1..=10000"
            );
        }
        Ok(())
    }
}

/// Evaluate `f(x, y)` for every cell of an `n²` grid on all cores.
fn parallel_rows(out: &mut [f64], n: usize, f: impl Fn(usize, usize) -> f64 + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |t| t.get())
        .clamp(1, 64);
    let rows_per = n.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (chunk, rows) in out.chunks_mut(rows_per * n).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (k, value) in rows.iter_mut().enumerate() {
                    let index = chunk * rows_per * n + k;
                    *value = f(index % n, index / n);
                }
            });
        }
    });
}

fn wrap(v: i64, n: usize) -> usize {
    v.rem_euclid(n as i64) as usize
}

fn extrema(values: &[f64]) -> (f64, f64) {
    values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        })
}

fn ramp(range: [f64; 2], x: f64) -> f64 {
    let t = ((x - range[0]) / (range[1] - range[0])).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn slope_grid(h: &[f64], n: usize, cell_m: f64) -> Vec<f64> {
    let mut out = vec![0.0; n * n];
    parallel_rows(&mut out, n, |x, y| {
        let (x, y) = (x as i64, y as i64);
        let gx = (h[y as usize * n + wrap(x + 1, n)] - h[y as usize * n + wrap(x - 1, n)])
            / (2.0 * cell_m);
        let gy = (h[wrap(y + 1, n) * n + x as usize] - h[wrap(y - 1, n) * n + x as usize])
            / (2.0 * cell_m);
        gx.hypot(gy)
    });
    out
}

/// Drainage area in m² over the depression-filled surface.
fn drainage_area(h: &[f64], n: usize, cell_m: f64) -> Vec<f64> {
    drainage(h, n, cell_m).0
}

/// Drainage area (m²) and, per cell, whether depression filling raised it
/// (routing there is artificial: straight lines across lake floors).
fn drainage(h: &[f64], n: usize, cell_m: f64) -> (Vec<f64>, Vec<bool>) {
    let mut filled = h.to_vec();
    let lowest = extrema(h).0;
    fill_depressions(&mut filled, n, lowest, 1.0e-4 * cell_m);
    let area = flow_accumulation(&filled, n, 1.1)
        .into_iter()
        .map(|cells| cells * cell_m * cell_m)
        .collect();
    let raised = filled
        .iter()
        .zip(h)
        .map(|(f, h)| f - h > 1.0e-6 * cell_m)
        .collect();
    (area, raised)
}

/// Periodic Gaussian-like blur: three box passes of the given radius in metres.
fn blur(values: &[f64], n: usize, cell_m: f64, radius_m: f64) -> Vec<f64> {
    let r = ((radius_m / cell_m) / 3.0f64.sqrt()).round() as usize;
    if r == 0 {
        return values.to_vec();
    }
    let r = r.min(n / 2 - 1);
    box_blur(&box_blur(&box_blur(values, n, r), n, r), n, r)
}

fn resample(values: &[f64], from: usize, to: usize) -> Vec<f64> {
    let mut out = values.to_vec();
    let mut n = from;
    while n < to {
        out = upsample2(&out, n);
        n *= 2;
    }
    if n > to {
        out = downsample(&out, n, n / to);
    }
    out
}

/// Periodic Catmull-Rom sample at fractional cell coordinates (cell centres at
/// integer + 0.5 are at integer coordinates here).
fn sample_cubic(h: &[f64], n: usize, x: f64, y: f64) -> f64 {
    let weights = |t: f64| {
        let (t2, t3) = (t * t, t * t * t);
        [
            0.5 * (-t3 + 2.0 * t2 - t),
            0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
            0.5 * (-3.0 * t3 + 4.0 * t2 + t),
            0.5 * (t3 - t2),
        ]
    };
    let (fx, fy) = (x.floor(), y.floor());
    let (wx, wy) = (weights(x - fx), weights(y - fy));
    let (ix, iy) = (fx as i64, fy as i64);
    let mut total = 0.0;
    for (j, wyj) in wy.iter().enumerate() {
        let row = wrap(iy - 1 + j as i64, n) * n;
        let mut acc = 0.0;
        for (i, wxi) in wx.iter().enumerate() {
            acc += wxi * h[row + wrap(ix - 1 + i as i64, n)];
        }
        total += wyj * acc;
    }
    total
}

/// Grid state carried through the pipeline.
struct Field {
    n: usize,
    cell_m: f64,
    height: Vec<f64>,
    hardness: Vec<f64>,
    uplift: Vec<f64>,
    /// Relief produced by impact_evolution stages, carried across stages so
    /// later stages keep degrading earlier craters (but never the terrain).
    craters: Vec<f64>,
    snapshots: HashMap<String, (usize, Vec<f64>)>,
}

struct Env<'a> {
    program: &'a Program,
    recipe_seed: u64,
}

impl Env<'_> {
    /// Hardness and uplift from the graph at resolution `n` (uplift falls back
    /// to normalized height).
    fn fields(&self, n: usize, height: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let base = self.program.evaluate(n);
        let hardness = base.hardness.iter().map(|&h| f64::from(h)).collect();
        let uplift = base.uplift.unwrap_or_else(|| {
            let (lo, hi) = extrema(height);
            let span = (hi - lo).max(1.0e-12);
            height.iter().map(|h| (h - lo) / span).collect()
        });
        (hardness, uplift)
    }
}

fn mask_grid(mask: &Mask, field: &Field, ctx: &Env<'_>) -> Vec<f64> {
    let n = field.n;
    let mut m = vec![1.0; n * n];
    if let Some(range) = mask.height {
        let (lo, hi) = extrema(&field.height);
        let span = (hi - lo).max(1.0e-12);
        for (v, h) in m.iter_mut().zip(&field.height) {
            *v *= ramp(range, (h - lo) / span);
        }
    }
    if let Some(range) = mask.slope {
        for (v, s) in m.iter_mut().zip(slope_grid(&field.height, n, field.cell_m)) {
            *v *= ramp(range, s);
        }
    }
    if let Some(range) = mask.flow {
        for (v, a) in m
            .iter_mut()
            .zip(drainage_area(&field.height, n, field.cell_m))
        {
            *v *= ramp(range, a.max(1.0).log10());
        }
    }
    if let Some(noise) = &mask.noise {
        for (v, x) in m.iter_mut().zip(noise.noise.grid(n, ctx.recipe_seed, 0)) {
            *v *= ramp(noise.ramp, x);
        }
    }
    if mask.invert {
        for v in &mut m {
            *v = 1.0 - *v;
        }
    }
    m
}

/// Particle hydraulic erosion (after Hans Theobald Beyer 2015). Heights are
/// handled in cell units so parameters are resolution independent; the grid
/// wraps. Deterministic: droplets run sequentially from hashed start points.
fn droplets(
    height_m: &mut [f64],
    hardness: &[f64],
    n: usize,
    cell_m: f64,
    d: &DropletFilter,
    seed: u32,
) {
    let unit_m = cell_m / d.slope_scale;
    let mut h: Vec<f64> = height_m.iter().map(|v| v / unit_m).collect();
    // Brush: cells within the radius, weights ∝ (1 − r/R), normalized.
    let reach = d.radius_cells.ceil() as i64;
    let mut brush = Vec::new();
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let r = ((dx * dx + dy * dy) as f64).sqrt();
            if r < d.radius_cells {
                brush.push((dx, dy, d.radius_cells - r));
            }
        }
    }
    let total: f64 = brush.iter().map(|b| b.2).sum();
    for b in &mut brush {
        b.2 /= total;
    }
    let count = (d.droplets_per_cell * (n * n) as f64).round() as u64;
    let nf = n as f64;
    let sample = |h: &[f64], x: f64, y: f64| -> (f64, f64, f64) {
        let (ix, iy) = (x.floor() as i64, y.floor() as i64);
        let (fx, fy) = (x - ix as f64, y - iy as f64);
        let (x0, x1, y0, y1) = (wrap(ix, n), wrap(ix + 1, n), wrap(iy, n), wrap(iy + 1, n));
        let (a, b, c, e) = (
            h[y0 * n + x0],
            h[y0 * n + x1],
            h[y1 * n + x0],
            h[y1 * n + x1],
        );
        let gx = (b - a) * (1.0 - fy) + (e - c) * fy;
        let gy = (c - a) * (1.0 - fx) + (e - b) * fx;
        let value =
            a * (1.0 - fx) * (1.0 - fy) + b * fx * (1.0 - fy) + c * (1.0 - fx) * fy + e * fx * fy;
        (value, gx, gy)
    };
    for k in 0..count {
        let h1 = pcg((k as u32) ^ pcg(seed ^ (k >> 32) as u32));
        let h2 = pcg(h1 ^ 0x9e37_79b9);
        let (mut x, mut y) = (
            f64::from(h1) / f64::from(u32::MAX) * nf,
            f64::from(h2) / f64::from(u32::MAX) * nf,
        );
        let (mut dx, mut dy) = (0.0, 0.0);
        let (mut speed, mut water, mut sediment) = (1.0, 1.0, 0.0);
        for _ in 0..d.max_steps {
            let (ix, iy) = (x.floor() as i64, y.floor() as i64);
            let (fx, fy) = (x - ix as f64, y - iy as f64);
            let (old_h, gx, gy) = sample(&h, x, y);
            dx = dx * d.inertia - gx * (1.0 - d.inertia);
            dy = dy * d.inertia - gy * (1.0 - d.inertia);
            let len = dx.hypot(dy);
            if len < 1.0e-12 {
                break;
            }
            dx /= len;
            dy /= len;
            x += dx;
            y += dy;
            let (new_h, _, _) = sample(&h, x, y);
            let delta = new_h - old_h;
            let capacity = (-delta).max(d.min_slope) * speed * water * d.capacity;
            let (cx0, cx1, cy0, cy1) = (wrap(ix, n), wrap(ix + 1, n), wrap(iy, n), wrap(iy + 1, n));
            if delta > 0.0 {
                // Moving uphill: fill the pit behind it (bilinear, Beyer).
                let amount = delta.min(sediment);
                sediment -= amount;
                h[cy0 * n + cx0] += amount * (1.0 - fx) * (1.0 - fy);
                h[cy0 * n + cx1] += amount * fx * (1.0 - fy);
                h[cy1 * n + cx0] += amount * (1.0 - fx) * fy;
                h[cy1 * n + cx1] += amount * fx * fy;
            } else if sediment > capacity {
                // Over capacity: deposit with the same brush as erosion, so
                // deposits are smooth sheets rather than single-cell spikes.
                let amount = (sediment - capacity) * d.deposit;
                sediment -= amount;
                for &(bx, by, w) in &brush {
                    h[wrap(iy + by, n) * n + wrap(ix + bx, n)] += amount * w;
                }
            } else {
                let amount = ((capacity - sediment) * d.erode).min(-delta);
                for &(bx, by, w) in &brush {
                    let i = wrap(iy + by, n) * n + wrap(ix + bx, n);
                    let take = amount * w * (1.0 - d.hardness_factor * hardness[i]);
                    h[i] -= take;
                    sediment += take;
                }
            }
            // Gains speed going down (delta < 0), loses it going up.
            speed = (speed * speed - delta * d.gravity).max(0.0).sqrt();
            water *= 1.0 - d.evaporate;
            if water < 0.01 {
                break;
            }
        }
    }
    for (out, v) in height_m.iter_mut().zip(h) {
        *out = v * unit_m;
    }
}

/// Lower every cell to at most `min over neighbours (h_j + limit_i · distance)`
/// (periodic, 8-connected), so the steepest descent from any cell never exceeds
/// its limit (metres per cell). Dijkstra-style: cells are finalized in
/// increasing height, so the result is exact and order independent.
pub(crate) fn slope_limit(h: &mut [f64], n: usize, limit: &[f64]) {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    struct Key(f64);
    impl PartialEq for Key {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other) == std::cmp::Ordering::Equal
        }
    }
    impl Eq for Key {}
    impl PartialOrd for Key {
        fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(other))
        }
    }
    impl Ord for Key {
        fn cmp(&self, other: &Self) -> std::cmp::Ordering {
            self.0.total_cmp(&other.0)
        }
    }
    let mut heap: BinaryHeap<Reverse<(Key, u32)>> = h
        .iter()
        .enumerate()
        .map(|(i, &v)| Reverse((Key(v), i as u32)))
        .collect();
    let mut done = vec![false; n * n];
    while let Some(Reverse((Key(value), index))) = heap.pop() {
        let i = index as usize;
        if done[i] || value > h[i] {
            continue;
        }
        done[i] = true;
        let (x, y) = ((i % n) as i64, (i / n) as i64);
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let j = wrap(y + dy, n) * n + wrap(x + dx, n);
                if done[j] {
                    continue;
                }
                let distance = if dx != 0 && dy != 0 {
                    std::f64::consts::SQRT_2
                } else {
                    1.0
                };
                let cap = h[i] + limit[j] * distance;
                if h[j] > cap {
                    h[j] = cap;
                    heap.push(Reverse((Key(cap), j as u32)));
                }
            }
        }
    }
}

/// Isotropic talus relaxation over 8 neighbours; conservative and Jacobi-style.
/// Note: on a long uniform over-steep slope inflow balances outflow, so it only
/// changes the slope's ends; use `slope_limit` to enforce a critical angle.
// Grid, material and the three talus parameters stay explicit at this one call site.
#[allow(clippy::too_many_arguments)]
fn thermal(
    h: &mut [f64],
    hardness: &[f64],
    n: usize,
    cell_m: f64,
    iterations: u32,
    talus: f64,
    rate: f64,
    talus_hardness: f64,
) {
    const OFFSETS: [(i64, i64, f64); 8] = [
        (-1, 0, 1.0),
        (1, 0, 1.0),
        (0, -1, 1.0),
        (0, 1, 1.0),
        (-1, -1, std::f64::consts::SQRT_2),
        (1, -1, std::f64::consts::SQRT_2),
        (-1, 1, std::f64::consts::SQRT_2),
        (1, 1, std::f64::consts::SQRT_2),
    ];
    let limit: Vec<f64> = hardness
        .iter()
        .map(|&k| talus * (1.0 + talus_hardness * (2.0 * k - 1.0)) * cell_m)
        .collect();
    let mut delta = vec![0.0; n * n];
    for _ in 0..iterations {
        delta.fill(0.0);
        for y in 0..n as i64 {
            for x in 0..n as i64 {
                let i = y as usize * n + x as usize;
                for &(dx, dy, dist) in &OFFSETS {
                    let j = wrap(y + dy, n) * n + wrap(x + dx, n);
                    let excess = h[i] - h[j] - limit[i] * dist;
                    if excess > 0.0 {
                        // Each pair is visited from both sides; 1/16 keeps the
                        // explicit step stable for 8 neighbours.
                        let moved = rate * excess / 16.0;
                        delta[i] -= moved;
                        delta[j] += moved;
                    }
                }
            }
        }
        for (v, d) in h.iter_mut().zip(&delta) {
            *v += d;
        }
    }
}

fn apply_filter(stage: &Stage, field: &mut Field, ctx: &Env<'_>, seed_index: u32) -> Result<()> {
    let n = field.n;
    let cell_m = field.cell_m;
    match &stage.filter {
        Filter::Base { .. } | Filter::Resample { .. } | Filter::ExemplarSynthesis(_) => {
            unreachable!("handled by run")
        }
        Filter::StreamPower(s) => {
            let mut h = field.height.clone();
            let (lo, hi) = extrema(&h);
            let level = lo + s.outlet_fraction * (hi - lo);
            let outlet = outlet_mask(&h, n, Some(level), s.outlet_points);
            if s.breach {
                breach_from_outlets(&mut h, n, &outlet, s.fill_slope * cell_m);
            } else {
                fill_from_outlets(&mut h, n, &outlet, s.fill_slope * cell_m);
            }
            let talus: Vec<f64> = field
                .hardness
                .iter()
                .map(|&k| s.talus * (1.0 + s.talus_hardness * (2.0 * k - 1.0)))
                .collect();
            let erodibility: Vec<f64> = field
                .hardness
                .iter()
                .map(|&k| 1.0 - s.fluvial.erodibility_hardness * k)
                .collect();
            fluvial::erode(
                &mut h,
                &fluvial::FluvialInputs {
                    n,
                    cell_m,
                    uplift: &field.uplift,
                    outlet: &outlet,
                    iterations: s.iterations,
                    talus: &talus,
                    erodibility_scale: &erodibility,
                    fill_slope: s.fill_slope,
                    breach: s.breach,
                    seed: layer_seed(ctx.recipe_seed, 300 + seed_index, n as u32),
                },
                &s.fluvial,
            );
            field.height = h;
        }
        Filter::Droplets(d) => {
            droplets(
                &mut field.height,
                &field.hardness,
                n,
                cell_m,
                d,
                layer_seed(ctx.recipe_seed, 400 + d.seed, n as u32),
            );
        }
        Filter::Thermal {
            iterations,
            talus,
            rate,
            talus_hardness,
        } => thermal(
            &mut field.height,
            &field.hardness,
            n,
            cell_m,
            *iterations,
            *talus,
            *rate,
            *talus_hardness,
        ),
        Filter::ChannelCarve {
            depth_m,
            reference_area_m2,
            min_area_m2,
            exponent,
            width_m,
        } => {
            let (area, raised) = drainage(&field.height, n, cell_m);
            let carve: Vec<f64> = area
                .iter()
                .zip(&raised)
                .map(|(&a, &raised)| {
                    if raised || a <= *min_area_m2 {
                        0.0
                    } else {
                        depth_m
                            * ((a - min_area_m2) / (reference_area_m2 - min_area_m2))
                                .min(1.0)
                                .powf(*exponent)
                    }
                })
                .collect();
            let carve = blur(&carve, n, cell_m, 0.5 * width_m);
            for (h, c) in field.height.iter_mut().zip(carve) {
                *h -= c;
            }
        }
        Filter::Deposit {
            max_slope,
            flow,
            radius_m,
            depth_m,
        } => {
            let slope = slope_grid(&field.height, n, cell_m);
            let area = drainage_area(&field.height, n, cell_m);
            let target = blur(&field.height, n, cell_m, *radius_m);
            for i in 0..n * n {
                let weight = ramp([*max_slope, 0.5 * max_slope], slope[i])
                    * ramp(*flow, area[i].max(1.0).log10());
                let raise = (target[i] - field.height[i]).clamp(0.0, *depth_m);
                field.height[i] += weight * raise;
            }
        }
        Filter::Smooth { radius_m, strength } => {
            let blurred = blur(&field.height, n, cell_m, *radius_m);
            for (h, b) in field.height.iter_mut().zip(blurred) {
                *h += strength * (b - *h);
            }
        }
        Filter::Detail { noise, amplitude_m } => {
            for (h, v) in field
                .height
                .iter_mut()
                .zip(noise.grid(n, ctx.recipe_seed, 0))
            {
                *h += amplitude_m * v;
            }
        }
        Filter::Warp { noise, amplitude_m } => {
            let wx = noise.grid(n, ctx.recipe_seed, 0);
            let wy = noise.grid(n, ctx.recipe_seed, 1);
            let source = field.height.clone();
            let k = amplitude_m / cell_m;
            parallel_rows(&mut field.height, n, |x, y| {
                let i = y * n + x;
                sample_cubic(&source, n, x as f64 + k * wx[i], y as f64 + k * wy[i])
            });
        }
        Filter::Strata { steps, exponent } => {
            let (lo, hi) = extrema(&field.height);
            let span = (hi - lo).max(1.0e-12);
            let s = f64::from(*steps);
            for h in &mut field.height {
                let t = ((*h - lo) / span).clamp(0.0, 1.0) * s;
                let step = t.floor().min(s - 1.0);
                *h = lo + (step + (t - step).powf(*exponent)) / s * span;
            }
        }
        Filter::RidgeSharpen { radius_m, amount } => {
            let blurred = blur(&field.height, n, cell_m, *radius_m);
            for (h, b) in field.height.iter_mut().zip(blurred) {
                let convex = (*h - b).max(0.0);
                *h += amount * convex;
            }
        }
        Filter::Gullies { gullies, seed } => crate::gullies::apply(
            &mut field.height,
            n,
            cell_m,
            gullies,
            layer_seed(ctx.recipe_seed, 500 + seed, n as u32),
        ),
        Filter::ImpactEvolution(recipe) => crate::evolution::run(
            &mut field.height,
            &mut field.craters,
            n,
            cell_m,
            recipe,
            layer_seed(ctx.recipe_seed, 700 + recipe.seed, n as u32),
        ),
        Filter::SlopeLimit {
            talus,
            talus_hardness,
        } => {
            let limit: Vec<f64> = field
                .hardness
                .iter()
                .map(|&k| talus * (1.0 + talus_hardness * (2.0 * k - 1.0)) * cell_m)
                .collect();
            slope_limit(&mut field.height, n, &limit);
        }
        Filter::Relief { relief_m } => {
            let (lo, hi) = extrema(&field.height);
            let scale = relief_m / (hi - lo).max(1.0e-9);
            for h in &mut field.height {
                *h = lo + (*h - lo) * scale;
            }
        }
        Filter::Snapshot { id } => {
            field
                .snapshots
                .insert(id.clone(), (n, field.height.clone()));
        }
        Filter::BlendSnapshot { id, amount } => {
            let (sn, snap) = field.snapshots.get(id).context("unknown snapshot")?;
            let snap = resample(snap, *sn, n);
            for (h, s) in field.height.iter_mut().zip(snap) {
                *h += amount * (s - *h);
            }
        }
    }
    Ok(())
}

/// Terrain statistics for authoring, independent of any reference asset:
/// slope percentiles (rise over run) and RMS relief per octave wavelength band
/// (difference of Gaussian-like blurs). Real landscapes show roughly power-law
/// band energy down to the hillslope scale (Perron et al. 2008); a band that
/// collapses relative to its neighbours means missing detail at that scale.
pub fn metrics(height_m: &[f64], n: usize, cell_m: f64) -> serde_json::Value {
    let mut slopes = slope_grid(height_m, n, cell_m);
    slopes.sort_by(f64::total_cmp);
    let q = |p: f64| slopes[((slopes.len() - 1) as f64 * p) as usize];
    let mut bands = Vec::new();
    let mut wavelength = 4.0 * cell_m;
    let mut finer = height_m.to_vec();
    while wavelength <= n as f64 * cell_m / 4.0 {
        let coarser = blur(height_m, n, cell_m, wavelength);
        let mean_sq = finer
            .iter()
            .zip(&coarser)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>()
            / (n * n) as f64;
        bands.push(serde_json::json!([
            (wavelength * 10.0).round() / 10.0,
            (mean_sq.sqrt() * 1000.0).round() / 1000.0
        ]));
        finer = coarser;
        wavelength *= 2.0;
    }
    let (lo, hi) = extrema(height_m);
    serde_json::json!({
        "relief_m": hi - lo,
        "slope_p10_p50_p90_p99": [q(0.1), q(0.5), q(0.9), q(0.99)],
        "band_rms_m": bands,
    })
}

/// Run the pipeline. `on_stage(index, name, height, n, cell_m)` sees every
/// stage's output (for previews).
pub fn run(
    recipe: &PipelineRecipe,
    mut on_stage: impl FnMut(usize, &str, &[f64], usize, f64) -> Result<()>,
) -> Result<BakeOutput> {
    recipe.validate()?;
    let program = Program::compile(&recipe.graph, recipe.footprint_m, recipe.seed)?;
    let ctx = Env {
        program: &program,
        recipe_seed: recipe.seed,
    };
    let mut field: Option<Field> = None;
    // Exemplar start: the synthesized grid is the base, so the erosion delta
    // (derived incision/deposition) is measured against it, not the constant graph.
    let mut exemplar_base: Option<(usize, Vec<f64>)> = None;
    let mut reports = Vec::new();
    for (index, stage) in recipe.pipeline.iter().enumerate() {
        let started = Instant::now();
        let name = stage
            .name
            .clone()
            .unwrap_or_else(|| stage.filter.kind().to_string());
        let mut detail: Option<serde_json::Value> = None;
        match &stage.filter {
            Filter::ExemplarSynthesis(e) => {
                let n = e.resolution as usize;
                let (height, report) =
                    crate::exemplar::synthesize(e, crate::exemplar::thread_count())
                        .with_context(|| format!("stage {index} ({name})"))?;
                detail = Some(report);
                let (hardness, uplift) = ctx.fields(n, &height);
                exemplar_base = Some((n, height.clone()));
                field = Some(Field {
                    n,
                    cell_m: recipe.footprint_m / n as f64,
                    height,
                    hardness,
                    uplift,
                    craters: vec![0.0; n * n],
                    snapshots: HashMap::new(),
                });
            }
            Filter::Base { resolution } => {
                let n = *resolution as usize;
                let base = program.evaluate(n);
                let height = base.height_m;
                let (hardness, uplift) = ctx.fields(n, &height);
                field = Some(Field {
                    n,
                    cell_m: recipe.footprint_m / n as f64,
                    height,
                    hardness,
                    uplift,
                    craters: vec![0.0; n * n],
                    snapshots: HashMap::new(),
                });
            }
            Filter::Resample { resolution, rebase } => {
                let f = field.as_mut().context("resample before base")?;
                let to = *resolution as usize;
                f.height = if *rebase {
                    let old = program.evaluate(f.n).height_m;
                    let residual: Vec<f64> =
                        f.height.iter().zip(&old).map(|(h, b)| h - b).collect();
                    let fresh = program.evaluate(to).height_m;
                    fresh
                        .iter()
                        .zip(resample(&residual, f.n, to))
                        .map(|(b, r)| b + r)
                        .collect()
                } else {
                    resample(&f.height, f.n, to)
                };
                f.craters = resample(&f.craters, f.n, to);
                f.n = to;
                f.cell_m = recipe.footprint_m / to as f64;
                let (hardness, uplift) = ctx.fields(to, &f.height);
                f.hardness = hardness;
                f.uplift = uplift;
            }
            _ => {
                let f = field.as_mut().context("filter before base")?;
                let before = stage.mask.as_ref().map(|_| f.height.clone());
                let mask = stage.mask.as_ref().map(|m| mask_grid(m, f, &ctx));
                apply_filter(stage, f, &ctx, index as u32)?;
                if let (Some(before), Some(mask)) = (before, mask) {
                    for ((h, b), m) in f.height.iter_mut().zip(before).zip(mask) {
                        *h = b + m * (*h - b);
                    }
                }
            }
        }
        let f = field.as_ref().context("pipeline produced no field")?;
        ensure!(
            f.height.iter().all(|h| h.is_finite()),
            "stage {index} ({name}) produced non-finite heights"
        );
        let (lo, hi) = extrema(&f.height);
        let seconds = started.elapsed().as_secs_f64();
        let mut report = serde_json::json!({
            "stage": index, "name": name, "kind": stage.filter.kind(),
            "resolution": f.n, "seconds": seconds, "relief_m": hi - lo,
        });
        if let Some(detail) = detail {
            report["detail"] = detail;
        }
        reports.push(report);
        on_stage(index, &name, &f.height, f.n, f.cell_m)?;
    }
    let f = field.context("pipeline produced no field")?;
    let base_final = match &exemplar_base {
        Some((from, base)) => resample(base, *from, f.n),
        None => program.evaluate(f.n).height_m,
    };
    let mut height_m = f.height;
    let mut erosion_delta_m: Vec<f64> = height_m
        .iter()
        .zip(&base_final)
        .map(|(h, b)| h - b)
        .collect();
    if let Some(target) = recipe.output_relief_m {
        let (lo, hi) = extrema(&height_m);
        let scale = target / (hi - lo).max(1.0e-9);
        for h in &mut height_m {
            *h = lo + (*h - lo) * scale;
        }
        for d in &mut erosion_delta_m {
            *d *= scale;
        }
    }
    if f.n < 16 {
        bail!("output resolution too small");
    }
    Ok(BakeOutput {
        resolution: f.n as u32,
        cell_size_m: f.cell_m,
        height_m,
        erosion_delta_m,
        stages: Vec::new(),
        reports: Some(serde_json::Value::Array(reports)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rough(n: usize) -> Vec<f64> {
        (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f64, (i / n) as f64);
                let t = std::f64::consts::TAU / n as f64;
                40.0 * (x * t).sin() * (2.0 * y * t).cos()
                    + 15.0 * (5.0 * x * t + 1.0).sin()
                    + f64::from(pcg(i as u32) % 1000) * 0.02
            })
            .collect()
    }

    #[test]
    fn slope_limit_enforces_the_critical_slope_without_raising_ground() {
        let n = 64;
        let cell = 2.0;
        let original = rough(n);
        let mut h = original.clone();
        let talus = 0.7;
        slope_limit(&mut h, n, &vec![talus * cell; n * n]);
        assert!(h.iter().zip(&original).all(|(a, b)| a <= b));
        let steepest = slope_grid(&h, n, cell).into_iter().fold(0.0, f64::max);
        // Per-edge limits bound each axis, so the central-difference gradient
        // magnitude can reach √2 × talus where both axes are at the limit.
        assert!(
            steepest <= talus * std::f64::consts::SQRT_2 + 1e-9,
            "max slope {steepest}"
        );
        for y in 0..n as i64 {
            for x in 0..n as i64 {
                let i = y as usize * n + x as usize;
                for (dx, dy, d) in [(1, 0, 1.0), (0, 1, 1.0), (1, 1, std::f64::consts::SQRT_2)] {
                    let j = wrap(y + dy, n) * n + wrap(x + dx, n);
                    assert!((h[i] - h[j]).abs() <= talus * cell * d + 1e-9);
                }
            }
        }
    }

    #[test]
    fn thermal_conserves_mass_and_lowers_spikes() {
        let n = 32;
        let mut h = vec![0.0; n * n];
        h[n * 16 + 16] = 100.0;
        let hardness = vec![0.5; n * n];
        let before: f64 = h.iter().sum();
        thermal(&mut h, &hardness, n, 1.0, 200, 1.0, 0.5, 0.0);
        let after: f64 = h.iter().sum();
        assert!((before - after).abs() < 1e-9);
        assert!(h[n * 16 + 16] < 20.0, "spike {}", h[n * 16 + 16]);
    }

    #[test]
    fn stream_power_reaches_the_analytic_slope_area_steady_state() {
        // Uniform uplift U against K·A^m·S (n = 1): at steady state every cell's
        // slope to its receiver is S = U / (K · A^m) (Braun & Willett 2013).
        let (n, cell) = (48usize, 10.0);
        let recipe = FluvialRecipe {
            dt: 1.0,
            erodibility: 0.01,
            area_exponent: 0.5,
            uplift_m_per_step: 0.05,
            initial_scale: 1.0,
            diffusion_m2_per_step: 0.0,
            routing_jitter: 0.0,
            erodibility_hardness: 0.0,
            mfd_exponent: 0.0,
            threshold_m_per_step: 0.0,
        };
        let mut h: Vec<f64> = (0..n * n)
            .map(|i| f64::from(pcg(i as u32) % 100) * 1e-3)
            .collect();
        let mut outlet = vec![false; n * n];
        outlet[0] = true;
        h[0] = 0.0;
        fluvial::erode(
            &mut h,
            &fluvial::FluvialInputs {
                n,
                cell_m: cell,
                uplift: &vec![1.0; n * n],
                outlet: &outlet,
                iterations: 6000,
                talus: &vec![1.0e6; n * n],
                erodibility_scale: &vec![1.0; n * n],
                fill_slope: 1.0e-6,
                breach: true,
                seed: 1,
            },
            &recipe,
        );
        // D8 receivers and drainage area on the final surface.
        let mut receiver = vec![usize::MAX; n * n];
        let mut slope = vec![0.0; n * n];
        for i in 0..n * n {
            if outlet[i] {
                continue;
            }
            let (x, y) = ((i % n) as i64, (i / n) as i64);
            for dy in -1..=1i64 {
                for dx in -1..=1i64 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let j = wrap(y + dy, n) * n + wrap(x + dx, n);
                    let d = if dx != 0 && dy != 0 {
                        std::f64::consts::SQRT_2
                    } else {
                        1.0
                    } * cell;
                    let s = (h[i] - h[j]) / d;
                    if s > slope[i] {
                        slope[i] = s;
                        receiver[i] = j;
                    }
                }
            }
        }
        let mut order: Vec<usize> = (0..n * n).collect();
        order.sort_by(|&a, &b| h[b].total_cmp(&h[a]));
        let mut area = vec![cell * cell; n * n];
        for &i in &order {
            if receiver[i] != usize::MAX {
                area[receiver[i]] += area[i];
            }
        }
        let mut ratios: Vec<f64> = (0..n * n)
            .filter(|&i| !outlet[i] && receiver[i] != usize::MAX)
            .map(|i| {
                let expected = recipe.uplift_m_per_step / (recipe.erodibility * area[i].sqrt());
                slope[i] / expected
            })
            .collect();
        ratios.sort_by(f64::total_cmp);
        let median = ratios[ratios.len() / 2];
        assert!(
            (0.8..1.25).contains(&median),
            "median S/S_expected {median}"
        );
    }
}
