//! Versioned, validated bake recipes. Every authored value that varies between
//! library bundles lives here; algorithms in the other modules consume it.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

pub const RECIPE_SCHEMA: u32 = 1;
const MAX_RESOLUTION: u32 = 4096;
const MAX_OCTAVES: u32 = 16;
const MAX_LAYERS: usize = 8;
const MAX_ITERATIONS: u32 = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema: u32,
    /// Stable bundle identifier, used as the output directory name.
    pub id: String,
    /// Authored revision of this recipe; bump on any change.
    pub version: u32,
    pub seed: u64,
    /// Physical edge length of the periodic tile.
    pub footprint_m: f64,
    /// Layered base noise (GPU). Exactly one of `base` and `graph` is set.
    #[serde(default)]
    pub base: Option<BaseRecipe>,
    /// Node-graph base terrain (CPU f64, `graph.rs`).
    #[serde(default)]
    pub graph: Option<crate::graph::GraphRecipe>,
    /// Coarse-to-fine erosion stages. The last stage defines the output resolution.
    pub stages: Vec<StageRecipe>,
    pub fluvial: FluvialRecipe,
    pub hydraulic: HydraulicRecipe,
    pub thermal: ThermalRecipe,
    pub derived: DerivedRecipe,
    /// Optional final relief: heights are scaled about their minimum so the
    /// published tile spans exactly this many metres.
    pub output_relief_m: Option<f64>,
    /// Gully filter parameters; required when any stage sets `gullies`.
    #[serde(default)]
    pub gullies: Option<GullyRecipe>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoiseKind {
    Fbm,
    Ridged,
    /// |n| folded: rounded hills and sharp creases.
    Billow,
}

/// One periodic noise sum. Frequencies are whole cycles per tile and the
/// lacunarity is an integer so every octave tiles exactly.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseLayer {
    pub kind: NoiseKind,
    pub frequency: u32,
    pub octaves: u32,
    pub lacunarity: u32,
    pub gain: f64,
    /// Contribution of this layer to the summed field.
    pub weight: f64,
    /// Ridge exponent; ignored by fBm.
    #[serde(default = "default_sharpness")]
    pub sharpness: f64,
    /// Transform each octave's domain by the integer similarity 2 + i (×√5,
    /// rotated 26.57°) instead of scaling by `lacunarity`, which must then be 2.
    /// Integer matrices keep every octave periodic while breaking the lattice's
    /// axis alignment between octaves.
    #[serde(default)]
    pub rotate_octaves: bool,
    /// Derivative damping (Quilez): each octave is divided by
    /// 1 + slope_damping · |Σ ∇n|², with ∇n the octave-local noise gradient
    /// accumulated over the octaves so far, so steep areas gather less detail.
    #[serde(default)]
    pub slope_damping: f64,
    /// Integer domain matrix D (rows) applied before the octaves: q = D·p.
    /// Integer entries keep the layer periodic; unequal or oblique rows give
    /// features a structural grain (e.g. `[[1, 0], [0, 3]]` stretches them
    /// threefold along x).
    #[serde(default)]
    pub domain: Option<[[i32; 2]; 2]>,
}

fn default_sharpness() -> f64 {
    2.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WarpRecipe {
    pub layer: NoiseLayer,
    /// Displacement in tile units (1.0 = one tile).
    pub amplitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HardnessRecipe {
    pub layer: NoiseLayer,
    pub base: f64,
    pub variation: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BaseRecipe {
    /// Physical height of a unit noise sum.
    pub amplitude_m: f64,
    pub layers: Vec<NoiseLayer>,
    pub warp: Option<WarpRecipe>,
    pub hardness: HardnessRecipe,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Process {
    /// Stream-power incision with uplift on the CPU (drainage structure).
    Fluvial,
    /// Virtual-pipe water, sediment and thermal relaxation on the GPU (detail).
    Hydraulic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StageRecipe {
    pub resolution: u32,
    pub iterations: u32,
    pub process: Process,
    /// Fraction of the base-noise detail newly resolvable at this resolution
    /// that is added to the upsampled previous stage (ignored on stage 0).
    pub detail_scale: f64,
    /// Apply the recipe's gully filter to this stage's input before erosion.
    #[serde(default)]
    pub gullies: bool,
}

/// Stream-power law dh/dt = U·uplift − K·A^m·S (n = 1), A in m².
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FluvialRecipe {
    pub dt: f64,
    pub erodibility: f64,
    pub area_exponent: f64,
    /// Uplift in metres per unit time where the normalized uplift field is 1.
    pub uplift_m_per_step: f64,
    /// Initial height as a fraction of the base noise when stage 0 is fluvial.
    pub initial_scale: f64,
    /// Linear hillslope diffusivity in m² per unit time.
    pub diffusion_m2_per_step: f64,
    /// Relative per-neighbour jitter of receiver slopes (0 = plain D8).
    pub routing_jitter: f64,
    /// Erodibility multiplier is 1 − erodibility_hardness × hardness.
    pub erodibility_hardness: f64,
    /// Drainage area routing: 0 accumulates along the single steepest receiver
    /// (D8); p > 0 spreads it over all lower neighbours with Freeman FD8 weights
    /// tanβᵖ·L, which keeps divergent hillslopes from incising one gully per cell.
    #[serde(default)]
    pub mfd_exponent: f64,
    /// Erosion threshold θ in metres per unit time: incision is reduced by θ·dt
    /// and never negative, so weak flow (hillslopes) does not channelize.
    #[serde(default)]
    pub threshold_m_per_step: f64,
}

/// Virtual-pipe parameters in grid units (cell length 1, heights in cells).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HydraulicRecipe {
    pub dt: f64,
    pub gravity: f64,
    pub rain: f64,
    pub evaporation: f64,
    pub capacity: f64,
    pub dissolve: f64,
    pub deposit: f64,
    pub min_tilt: f64,
    /// Water depth (cells) above which capacity stops growing.
    pub depth_reference: f64,
    /// Speed cap (cells per unit time) used for carrying capacity.
    pub max_speed: f64,
    pub max_erosion_per_step: f64,
    /// Fraction of each stage's input relief, above its minimum, that acts as
    /// a base-level outlet removing water and sediment. 0 closes the tile.
    pub outlet_fraction: f64,
    /// Minimum drainage slope (rise over run) used to fill closed depressions
    /// toward the outlet before each stage. 0 disables filling.
    pub fill_slope: f64,
    /// Rain-free steps appended to every hydraulic stage.
    pub settle_iterations: u32,
    /// 0: every cell at or below the outlet level is fixed base level. N > 0:
    /// only the N lowest local minima below the outlet level are base level
    /// (point sinks), so low ground keeps eroding instead of staying flat.
    #[serde(default)]
    pub outlet_points: u32,
}

/// Branching gully filter applied to the input of stages with `gullies: true`
/// (see `gullies.rs`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GullyRecipe {
    /// Stripe wavelength of the first (coarsest) octave.
    pub wavelength_m: f64,
    pub octaves: u32,
    /// Wavelength ratio between successive octaves.
    pub lacunarity: f64,
    /// Height amplitude of the first octave; later octaves scale by `gain`.
    pub amplitude_m: f64,
    pub gain: f64,
    /// Pivot cell size relative to the octave wavelength.
    pub cell_ratio: f64,
    /// Slope (rise over run) at which gullies reach full strength.
    pub slope_full: f64,
    /// Exponent keeping finer octaves off coarser ridges and creases (0 = off).
    pub detail: f64,
    /// Window radius of the local relief that sets the ridge/valley fade target.
    pub fade_radius_m: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ThermalRecipe {
    /// Stable slope (rise over run) above which material slides.
    pub talus_tangent: f64,
    pub rate: f64,
    /// Fluvial stages: talus = talus_tangent × (1 + talus_hardness × (2·hardness_n − 1)),
    /// with hardness_n the stage's hardness normalized to [0, 1].
    pub talus_hardness: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WetnessRecipe {
    pub flow: f64,
    pub deposition: f64,
    pub shelter: f64,
    pub bias: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExposureRecipe {
    pub relief: f64,
    pub convexity: f64,
    pub bias: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpawnRecipe {
    pub flow: f64,
    pub incision: f64,
    pub steepness: f64,
    pub steep_tangent: f64,
    pub bias: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DerivedRecipe {
    /// Resolution of `clim.rg8` and `spawn.r8`; must divide the height resolution.
    pub channel_resolution: u32,
    /// Multiple-flow-direction slope exponent.
    pub flow_exponent: f64,
    /// Radius of the local-relief neighbourhood.
    pub relief_radius_m: f64,
    pub wetness: WetnessRecipe,
    pub exposure: ExposureRecipe,
    pub spawn: SpawnRecipe,
}

impl Recipe {
    pub fn load(path: &Path) -> Result<(Self, Vec<u8>)> {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading recipe {}", path.display()))?;
        let recipe: Recipe = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing recipe {}", path.display()))?;
        recipe.validate()?;
        Ok((recipe, bytes))
    }

    pub fn sha256(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    pub fn output_resolution(&self) -> u32 {
        self.stages.last().map_or(0, |stage| stage.resolution)
    }

    pub fn cell_size_m(&self, resolution: u32) -> f64 {
        self.footprint_m / f64::from(resolution)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == RECIPE_SCHEMA,
            "unsupported recipe schema {}",
            self.schema
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
            self.footprint_m.is_finite() && (16.0..=1.0e6).contains(&self.footprint_m),
            "footprint_m outside 16..=1e6"
        );
        ensure!(
            self.base.is_some() != self.graph.is_some(),
            "set exactly one of base and graph"
        );
        if let Some(graph) = &self.graph {
            crate::graph::check(graph, self.footprint_m)?;
        }
        if let Some(base) = &self.base {
            ensure!(
                finite_in(base.amplitude_m, 0.0, 10_000.0),
                "base.amplitude_m outside 0..=10000"
            );
            ensure!(
                !base.layers.is_empty() && base.layers.len() <= MAX_LAYERS,
                "base needs 1..={MAX_LAYERS} layers"
            );
            for layer in &base.layers {
                validate_layer(layer)?;
            }
            if let Some(warp) = &base.warp {
                validate_layer(&warp.layer)?;
                ensure!(
                    finite_in(warp.amplitude, 0.0, 1.0),
                    "warp.amplitude outside 0..=1"
                );
            }
            validate_layer(&base.hardness.layer)?;
            ensure!(
                finite_in(base.hardness.base, 0.0, 0.95)
                    && finite_in(base.hardness.variation, 0.0, 0.95),
                "hardness base/variation outside 0..=0.95"
            );
        }

        ensure!(
            !self.stages.is_empty() && self.stages.len() <= 6,
            "need 1..=6 stages"
        );
        for (i, stage) in self.stages.iter().enumerate() {
            ensure!(
                stage.resolution.is_power_of_two()
                    && (16..=MAX_RESOLUTION).contains(&stage.resolution),
                "stage {i} resolution must be a power of two in 16..={MAX_RESOLUTION}"
            );
            ensure!(
                stage.iterations <= MAX_ITERATIONS,
                "stage {i} iterations > {MAX_ITERATIONS}"
            );
            ensure!(
                finite_in(stage.detail_scale, 0.0, 1.0),
                "stage {i} detail_scale outside 0..=1"
            );
            if i > 0 {
                ensure!(
                    stage.resolution == self.stages[i - 1].resolution * 2,
                    "stage {i} must double the previous resolution"
                );
            }
        }

        let f = &self.fluvial;
        for (name, value, lo, hi) in [
            ("dt", f.dt, 0.0, 1.0e3),
            ("erodibility", f.erodibility, 0.0, 1.0),
            ("area_exponent", f.area_exponent, 0.1, 1.0),
            ("uplift_m_per_step", f.uplift_m_per_step, 0.0, 100.0),
            ("initial_scale", f.initial_scale, 0.0, 1.0),
            ("diffusion_m2_per_step", f.diffusion_m2_per_step, 0.0, 1.0e4),
            ("erodibility_hardness", f.erodibility_hardness, 0.0, 1.0),
            ("routing_jitter", f.routing_jitter, 0.0, 1.0),
            ("mfd_exponent", f.mfd_exponent, 0.0, 10.0),
            ("threshold_m_per_step", f.threshold_m_per_step, 0.0, 100.0),
        ] {
            ensure!(
                finite_in(value, lo, hi),
                "fluvial.{name} outside {lo}..={hi}"
            );
        }
        let fluvial_used = self.stages.iter().any(|s| s.process == Process::Fluvial);
        if self.stages.iter().any(|s| s.gullies) {
            let g = self
                .gullies
                .as_ref()
                .context("stages use gullies but the recipe has no gullies section")?;
            ensure!(
                (1..=8).contains(&g.octaves),
                "gullies.octaves outside 1..=8"
            );
            for (name, value, lo, hi) in [
                ("wavelength_m", g.wavelength_m, 1.0, self.footprint_m),
                ("lacunarity", g.lacunarity, 1.2, 4.0),
                ("amplitude_m", g.amplitude_m, 0.0, 1000.0),
                ("gain", g.gain, 0.0, 1.0),
                ("cell_ratio", g.cell_ratio, 0.25, 4.0),
                ("slope_full", g.slope_full, 0.001, 10.0),
                ("detail", g.detail, 0.0, 10.0),
                (
                    "fade_radius_m",
                    g.fade_radius_m,
                    1.0,
                    self.footprint_m * 0.25,
                ),
            ] {
                ensure!(
                    finite_in(value, lo, hi),
                    "gullies.{name} outside {lo}..={hi}"
                );
            }
        }
        if let Some(relief) = self.output_relief_m {
            ensure!(
                finite_in(relief, 1.0, 10_000.0),
                "output_relief_m outside 1..=10000"
            );
        }
        let h = &self.hydraulic;
        for (name, value, lo, hi) in [
            ("dt", h.dt, 1.0e-4, 1.0),
            ("gravity", h.gravity, 0.0, 100.0),
            ("rain", h.rain, 0.0, 1.0),
            ("evaporation", h.evaporation, 0.0, 10.0),
            ("capacity", h.capacity, 0.0, 100.0),
            ("dissolve", h.dissolve, 0.0, 100.0),
            ("deposit", h.deposit, 0.0, 100.0),
            ("min_tilt", h.min_tilt, 0.0, 1.0),
            ("depth_reference", h.depth_reference, 1.0e-4, 100.0),
            ("max_speed", h.max_speed, 0.0, 100.0),
            ("max_erosion_per_step", h.max_erosion_per_step, 0.0, 10.0),
            ("outlet_fraction", h.outlet_fraction, 0.0, 0.5),
            ("fill_slope", h.fill_slope, 0.0, 0.5),
        ] {
            ensure!(
                finite_in(value, lo, hi),
                "hydraulic.{name} outside {lo}..={hi}"
            );
        }
        ensure!(h.outlet_points <= 4096, "outlet_points > 4096");
        ensure!(
            h.settle_iterations <= MAX_ITERATIONS,
            "settle_iterations > {MAX_ITERATIONS}"
        );
        ensure!(
            !fluvial_used || (h.outlet_fraction > 0.0 && h.fill_slope > 0.0),
            "fluvial stages need an outlet and a positive fill_slope"
        );
        ensure!(
            h.fill_slope == 0.0 || h.outlet_fraction > 0.0,
            "fill_slope requires an outlet (outlet_fraction > 0)"
        );
        ensure!(
            finite_in(self.thermal.talus_tangent, 0.0, 10.0)
                && finite_in(self.thermal.talus_hardness, 0.0, 0.9)
                && finite_in(self.thermal.rate, 0.0, 100.0),
            "thermal parameters out of range"
        );

        let d = &self.derived;
        let out = self.output_resolution();
        ensure!(
            d.channel_resolution.is_power_of_two()
                && d.channel_resolution >= 16
                && d.channel_resolution <= out,
            "derived.channel_resolution must be a power of two in 16..=output resolution"
        );
        ensure!(
            finite_in(d.flow_exponent, 0.1, 10.0),
            "flow_exponent outside 0.1..=10"
        );
        ensure!(
            finite_in(
                d.relief_radius_m,
                self.cell_size_m(out),
                self.footprint_m * 0.25
            ),
            "relief_radius_m outside one cell..=quarter footprint"
        );
        for value in [
            d.wetness.flow,
            d.wetness.deposition,
            d.wetness.shelter,
            d.wetness.bias,
            d.exposure.relief,
            d.exposure.convexity,
            d.exposure.bias,
            d.spawn.flow,
            d.spawn.incision,
            d.spawn.steepness,
            d.spawn.bias,
        ] {
            ensure!(
                finite_in(value, -10.0, 10.0),
                "derived weight outside -10..=10"
            );
        }
        ensure!(
            finite_in(d.spawn.steep_tangent, 0.01, 10.0),
            "spawn.steep_tangent out of range"
        );
        Ok(())
    }
}

fn finite_in(value: f64, lo: f64, hi: f64) -> bool {
    value.is_finite() && (lo..=hi).contains(&value)
}

pub(crate) fn validate_layer(layer: &NoiseLayer) -> Result<()> {
    ensure!(
        (1..=1024).contains(&layer.frequency),
        "noise frequency outside 1..=1024"
    );
    ensure!(
        (1..=MAX_OCTAVES).contains(&layer.octaves),
        "octaves outside 1..={MAX_OCTAVES}"
    );
    ensure!(
        (2..=4).contains(&layer.lacunarity),
        "lacunarity outside 2..=4"
    );
    let per_octave = if layer.rotate_octaves {
        ensure!(
            layer.lacunarity == 2,
            "rotate_octaves requires lacunarity 2"
        );
        5f64.sqrt()
    } else {
        f64::from(layer.lacunarity)
    };
    let top = f64::from(layer.frequency) * per_octave.powi(layer.octaves as i32 - 1);
    ensure!(
        top <= 65_536.0,
        "highest octave frequency exceeds 65536 cycles per tile"
    );
    ensure!(
        finite_in(layer.slope_damping, 0.0, 100.0),
        "slope_damping outside 0..=100"
    );
    if let Some([[a, b], [c, d]]) = layer.domain {
        ensure!(
            [a, b, c, d].iter().all(|v| v.abs() <= 16) && a * d - b * c != 0,
            "domain must be a non-singular integer matrix with entries in -16..=16"
        );
    }
    ensure!(finite_in(layer.gain, 0.0, 1.0), "gain outside 0..=1");
    ensure!(
        finite_in(layer.weight, -10.0, 10.0),
        "weight outside -10..=10"
    );
    ensure!(
        finite_in(layer.sharpness, 0.25, 8.0),
        "sharpness outside 0.25..=8"
    );
    Ok(())
}
