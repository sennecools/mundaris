//! Planet archetypes and sampled planet parameters
//! (`docs/ASTRUM_TERRAIN_PIPELINE.md` §5).
//!
//! An archetype is authored data (RON) describing parameter *ranges* and the
//! Tier A stages a body runs. A concrete body samples every range with seeds
//! derived per stage from its body seed (§5.1), so changing one stage's inputs
//! never reshuffles another stage. Editor overrides replace sampled values by
//! name ([`PlanetParams::set`]).
use super::TerrainError;
use astrum_core::params::{ParamDesc, ParamKind, ParamValue, Params};
use serde::{Deserialize, Serialize};

/// Inclusive `(min, max)` authored range, sampled uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Range(pub f64, pub f64);

impl Range {
    fn valid(self, low: f64, high: f64) -> bool {
        self.0.is_finite()
            && self.1.is_finite()
            && low <= self.0
            && self.0 <= self.1
            && self.1 <= high
    }
    fn sample(self, unit: f64) -> f64 {
        self.0 + (self.1 - self.0) * unit
    }
}

/// Inclusive integer `(min, max)` authored range, sampled uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountRange(pub u32, pub u32);

impl CountRange {
    fn valid(self, low: u32, high: u32) -> bool {
        low <= self.0 && self.0 <= self.1 && self.1 <= high
    }
    fn sample(self, unit: f64) -> u32 {
        let span = f64::from(self.1 - self.0 + 1);
        (self.0 + (unit * span).floor() as u32).min(self.1)
    }
}

/// Tier A stages (§6.5): M1 (World map and planet editor) plus M2 (Shape)
/// tectonics, rain shadow and macro erosion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TierAStage {
    Continents,
    SeaLevel,
    Shelf,
    Temperature,
    Wind,
    Moisture,
    /// Plates, boundary profiles (uplift, trenches, ridges), crust type and
    /// rock hardness (§6.5.1, §6.5.3).
    Tectonics,
    /// Wind deflected around smoothed relief, orographic rain and lee drying
    /// (§6.5.5–6.5.6).
    RainShadow,
    /// Stream-power macro erosion with thermal talus and deposition (§6.5.7).
    Erosion,
}

/// Face resolution for bodies up to `max_radius_km` (§6.2; resolution never
/// depends on viewing distance, P6).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolutionBand {
    pub max_radius_km: f64,
    pub face_cells: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinentRanges {
    /// Dominant continent wavelength on the surface.
    pub wavelength_km: Range,
    pub octaves: u32,
    pub lacunarity: f64,
    pub gain: f64,
    /// Domain-warp wavelength and strength (as a fraction of the continent
    /// wavelength) that roughen coastlines.
    pub warp_wavelength_km: Range,
    pub warp_strength: Range,
    /// Highest land and deepest ocean relative to sea level.
    pub land_height_m: Range,
    pub land_exponent: f64,
    pub ocean_depth_m: Range,
    /// Continental shelf: depth at its outer edge and its width as a fraction
    /// of the noise distance from the coast to the deepest ocean.
    pub shelf_depth_m: f64,
    pub shelf_fraction: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemperatureRanges {
    pub equator_c: Range,
    pub pole_c: Range,
    pub axial_tilt_deg: Range,
    pub lapse_c_per_km: f64,
    /// How much the ocean pulls coastal temperatures towards the global mean.
    pub ocean_moderation: f64,
    /// Distance over which the ocean mask spreads inland for moderation
    /// (diffusion length, independent of bake resolution).
    pub ocean_blur_km: f64,
    pub noise_c: f64,
    pub noise_wavelength_km: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindRanges {
    /// Circulation cells per hemisphere (three on Earth).
    pub cells_per_hemisphere: u32,
    /// North-south component relative to the east-west one.
    pub meridional_fraction: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoistureRanges {
    pub iterations: u32,
    /// Upwind distance per iteration on the surface, independent of bake
    /// resolution (P6: an editor preview and the full bake agree).
    pub step_km: f64,
    /// Share of each step's moisture taken from four samples offset half a
    /// step to either side, so plumes spread instead of streaking.
    pub spread: f64,
    pub evaporation: Range,
    pub rain: Range,
    /// Accumulated precipitation that maps to moisture `1 - 1/e`.
    pub precipitation_scale: f64,
    /// Rain modulation by the circulation cells: rain is scaled by
    /// `1 + convergence · cos(2 · cells · |latitude|)`, wetter where the
    /// cells converge (equator, ~60°) and drier where they diverge (~30°,
    /// poles). 0 disables it.
    pub convergence: f64,
}

/// Plates and their boundary profiles (§6.5.1, M2 design §1). Heights and
/// depths are the full-strength profile amplitudes; widths are distances from
/// the plate boundary on the surface.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TectonicsRanges {
    /// Major plates on a jittered Fibonacci lattice (rounded).
    pub plate_count: Range,
    pub microplates: CountRange,
    /// Lattice jitter as a fraction of `π / √plates`.
    pub jitter: f64,
    /// Power-diagram weight deficit of microplates (smaller cells).
    pub micro_shrink: f64,
    pub continental_fraction: Range,
    /// Domain warp of plate assignment: jagged instead of straight boundaries.
    pub warp_wavelength_km: f64,
    pub warp_strength: Range,
    /// Angular speed scale of the plates' Euler rotations (relative units:
    /// convergence saturates towards 0.4).
    pub speed: Range,
    /// Share of crust type (continental or oceanic) in the continent field
    /// before the sea-level percentile.
    pub crust_weight: Range,
    pub collision_height_m: Range,
    pub arc_height_m: Range,
    pub trench_depth_m: Range,
    pub ridge_height_m: Range,
    pub rift_depth_m: f64,
    pub transform_height_m: f64,
    /// Half width of continental collision belts.
    pub orogen_width_km: Range,
    /// Relative roughness of orogenic relief (belts, arcs) from 4-octave
    /// noise: seeds the drainage network erosion incises.
    pub roughness: f64,
    pub roughness_wavelength_km: f64,
    pub arc_width_km: f64,
    /// Distance of coastal ranges and island arcs from the boundary (at least
    /// `arc_width_km`, so they vanish at the boundary itself).
    pub arc_offset_km: f64,
    pub trench_width_km: f64,
    pub ridge_width_km: f64,
    pub rift_width_km: f64,
    pub transform_width_km: f64,
    /// Distance over which crust type blends across a boundary.
    pub crust_width_km: f64,
    /// Soft-minimum length over neighbouring boundaries: outputs stay
    /// continuous where the nearest boundary switches.
    pub softness_km: f64,
    /// `boundary_coord` is clamped to ± this distance.
    pub boundary_clamp_km: f64,
    /// Smooth-maximum temperature where the relief of boundaries meeting at a
    /// triple junction merges (lane proposal `plate-junctions-PROPOSAL.md`).
    #[serde(default = "default_junction_blend_km")]
    pub junction_blend_km: f64,
}

fn default_junction_blend_km() -> f64 {
    0.4
}

/// Rock hardness (§6.5.3): per-plate rock family (oceanic plates volcanic,
/// continental crystalline or sedimentary), hard volcanic belts, noise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardnessRanges {
    pub crystalline: f64,
    pub volcanic: f64,
    pub sedimentary: f64,
    /// Amplitude of 3-octave noise added to hardness.
    pub noise: f64,
    pub noise_wavelength_km: f64,
}

/// Rain shadow (§6.5.5–6.5.6): wind deflected around relief smoothed over the
/// moisture step, orographic rain on windward slopes, drying in the lee.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RainShadowRanges {
    /// Share of the uphill wind component turned along the relief.
    pub deflection: f64,
    /// Slope at which deflection reaches half strength.
    pub deflection_slope: f64,
    /// Wind slow-down over steep relief (fraction at saturated slope).
    pub slowdown: f64,
    /// Extra rain-out per step at `reference_slope` of uphill wind.
    pub orographic_rain: f64,
    /// Rain reduction per step at `reference_slope` of downhill wind.
    pub lee_drying: f64,
    pub reference_slope: f64,
}

/// Macro erosion (§6.5.7, M2 design §1): stream power
/// `dh/dt = U − K(1 − 0.8·hardness)·Q^m·S^n` on multiple flow directions,
/// with thermal talus and deposition, as a coarse-to-fine cascade.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErosionRanges {
    /// K: incision in metres per iteration at `Q^m·S^n = 1` (Q in km²).
    pub strength: Range,
    /// Total rock uplift over the cascade where tectonic uplift is 1.
    pub uplift_m: Range,
    pub talus_deg: f64,
    /// Share of excess sediment deposited per iteration.
    pub deposition: f64,
    /// Transport capacity relative to the local incision rate.
    pub capacity: f64,
    pub area_exponent: f64,
    pub slope_exponent: f64,
    /// Multiple-flow-direction exponent (weights ∝ slope^p).
    pub flow_exponent: f64,
    /// Share of the excess over the talus slope moved per iteration.
    pub thermal_rate: f64,
    /// Deposit thickness that maps to sediment `1 − 1/e`.
    pub sediment_depth_m: f64,
    /// Cascade levels, coarse to fine: (face-cell divisor, iterations).
    pub cascade: [(u32, u32); 3],
}

/// Snow cover rule (§10.1): `temperature < T_snow` blended over `blend_c`,
/// fading out between the two slope angles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnowRule {
    /// Material definition relative to the content root.
    pub material: String,
    pub temperature_c: f64,
    pub blend_c: f64,
    pub slope_deg: (f64, f64),
}

/// Climate detail at Tier B: band-limited fBm added to the world map's
/// temperature and moisture before the biome LUT, so biome borders break up
/// and refine as a node gets finer. It is zero-mean and band-limited like the
/// height detail, so distant colour is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClimateDetail {
    pub base_wavelength_m: f64,
    pub min_wavelength_m: f64,
    pub lacunarity: f64,
    pub gain: f64,
    /// Largest temperature and moisture offsets (all octaves aligned).
    pub temperature_c: f64,
    pub moisture: f64,
}

impl ClimateDetail {
    /// Unit-amplitude noise definition (amplitudes sum to 1 over all octaves).
    pub fn noise_definition(&self) -> crate::terrain::noise::DetailNoiseDefinition {
        let mut total = 0.0;
        let mut wavelength = self.base_wavelength_m;
        let mut amplitude = 1.0;
        while wavelength >= self.min_wavelength_m * (1.0 - 1.0e-9) && total < 1e6 {
            total += amplitude;
            amplitude *= self.gain;
            wavelength /= self.lacunarity;
        }
        crate::terrain::noise::DetailNoiseDefinition {
            version: 1,
            seed_salt: 0xc11a_7e00,
            base_wavelength_m: self.base_wavelength_m,
            min_wavelength_m: self.min_wavelength_m,
            lacunarity: self.lacunarity,
            gain: self.gain,
            amplitude_m: 1.0 / total.max(1.0),
        }
    }

    fn valid(&self) -> bool {
        self.noise_definition().validate().is_ok()
            && (0.0..=50.0).contains(&self.temperature_c)
            && (0.0..=1.0).contains(&self.moisture)
    }
}

/// Flat water of M1 (World map and planet editor): opaque, tinted from the
/// shallow to the deep colour as `1 - exp(-depth / depth_scale_m)`. Real
/// water shading is M3 (Water).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterLook {
    pub shallow_linear: (f32, f32, f32),
    pub deep_linear: (f32, f32, f32),
    pub depth_scale_m: f64,
}

/// Surface material definition (`content/materials/*/material.ron`, App. C.2).
/// Until detail textures arrive (M4, Surface) a material is its declared
/// linear mean albedo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialAsset {
    pub schema: u32,
    pub name: String,
    pub mean_albedo_linear: (f64, f64, f64),
}

impl MaterialAsset {
    pub fn validate(&self) -> Result<(), TerrainError> {
        let m = self.mean_albedo_linear;
        if self.schema == 1
            && !self.name.is_empty()
            && [m.0, m.1, m.2].iter().all(|v| (0.0..=1.0).contains(v))
        {
            Ok(())
        } else {
            Err(TerrainError::InvalidConfig)
        }
    }
}

/// Most plates (major plus micro) a body may have: the GPU `Plates` uniform.
pub const MAX_PLATES: usize = 32;
/// Most microplates; major plates are capped at `MAX_PLATES - MAX_MICROPLATES`.
pub const MAX_MICROPLATES: u32 = 6;
/// Most major plates (`plate_count` rounds and clamps to this).
pub const MAX_MAJOR_PLATES: f64 = (MAX_PLATES - MAX_MICROPLATES as usize) as f64;

/// Erosion cascade levels with iterations: divisors are powers of two up to
/// 16, strictly decreasing (coarse to fine).
pub fn cascade_valid(cascade: &[(u32, u32); 3]) -> bool {
    let active: Vec<(u32, u32)> = cascade.iter().copied().filter(|l| l.1 > 0).collect();
    cascade.iter().all(|(divisor, iterations)| {
        divisor.is_power_of_two() && *divisor <= 16 && *iterations <= 2000
    }) && active.windows(2).all(|pair| pair[0].0 > pair[1].0)
}

fn colour_valid(c: (f32, f32, f32)) -> bool {
    [c.0, c.1, c.2].iter().all(|v| (0.0..=1.0).contains(v))
}

/// Authored archetype (`content/archetypes/*.ron`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanetArchetype {
    pub schema: u32,
    pub name: String,
    pub stages: Vec<TierAStage>,
    pub resolution: Vec<ResolutionBand>,
    pub ocean_coverage: Range,
    pub continents: ContinentRanges,
    pub temperature: TemperatureRanges,
    pub wind: WindRanges,
    pub moisture: MoistureRanges,
    pub tectonics: TectonicsRanges,
    pub hardness: HardnessRanges,
    pub rain_shadow: RainShadowRanges,
    pub erosion: ErosionRanges,
    /// Landform set (`landform::LandformSetFile`), relative to the content
    /// root: relief recipes on top of the macro elevation (M2 Shape).
    #[serde(default)]
    pub landforms: Option<String>,
    /// Biome LUT metadata (`biome_lut::LutMetadata`), relative to the content
    /// root.
    pub biome_lut: String,
    pub snow: SnowRule,
    pub climate_detail: ClimateDetail,
    pub water: WaterLook,
    /// Linear colour of the whole body seen as a few pixels (§6.1).
    pub average_colour: (f32, f32, f32),
}

impl PlanetArchetype {
    pub fn validate(&self) -> Result<(), TerrainError> {
        let c = &self.continents;
        let t = &self.temperature;
        let m = &self.moisture;
        let ok = self.schema == 1
            && !self.name.is_empty()
            && !self.stages.is_empty()
            && !self.resolution.is_empty()
            && self.resolution.iter().all(|band| {
                band.max_radius_km.is_finite()
                    && band.max_radius_km > 0.0
                    && band.face_cells.is_power_of_two()
                    && (16..=1024).contains(&band.face_cells)
            })
            && self.ocean_coverage.valid(0.0, 0.98)
            && c.wavelength_km.valid(1.0, 1.0e5)
            && (1..=10).contains(&c.octaves)
            && (1.5..=4.0).contains(&c.lacunarity)
            && c.gain > 0.0
            && c.gain < 1.0
            && c.warp_wavelength_km.valid(1.0, 1.0e5)
            && c.warp_strength.valid(0.0, 2.0)
            && c.land_height_m.valid(0.0, 2.0e4)
            && (0.25..=4.0).contains(&c.land_exponent)
            && c.ocean_depth_m.valid(1.0, 2.0e4)
            && (0.0..=2.0e3).contains(&c.shelf_depth_m)
            && (0.0..0.9).contains(&c.shelf_fraction)
            && t.equator_c.valid(-250.0, 500.0)
            && t.pole_c.valid(-250.0, 500.0)
            && t.axial_tilt_deg.valid(0.0, 90.0)
            && (0.0..=50.0).contains(&t.lapse_c_per_km)
            && (0.0..=1.0).contains(&t.ocean_moderation)
            && t.ocean_blur_km.is_finite()
            && (0.0..=1.0e4).contains(&t.ocean_blur_km)
            && (0.0..=50.0).contains(&t.noise_c)
            && t.noise_wavelength_km.is_finite()
            && t.noise_wavelength_km >= 1.0
            && (1..=6).contains(&self.wind.cells_per_hemisphere)
            && (0.0..=1.0).contains(&self.wind.meridional_fraction)
            && (1..=512).contains(&m.iterations)
            && m.step_km.is_finite()
            && (0.1..=1.0e4).contains(&m.step_km)
            && (0.0..=1.0).contains(&m.spread)
            && m.evaporation.valid(0.0, 1.0)
            && m.rain.valid(0.0, 1.0)
            && m.precipitation_scale.is_finite()
            && m.precipitation_scale > 0.0
            && (0.0..=0.95).contains(&m.convergence)
            && !self.biome_lut.is_empty()
            && self.climate_detail.valid()
            && !self.snow.material.is_empty()
            && self.snow.temperature_c.is_finite()
            && self.snow.blend_c.is_finite()
            && self.snow.blend_c > 0.0
            && (0.0..90.0).contains(&self.snow.slope_deg.0)
            && self.snow.slope_deg.0 < self.snow.slope_deg.1
            && self.snow.slope_deg.1 <= 90.0
            && colour_valid(self.water.shallow_linear)
            && colour_valid(self.water.deep_linear)
            && self.water.depth_scale_m.is_finite()
            && self.water.depth_scale_m > 0.0
            && colour_valid(self.average_colour)
            && self.shape_valid();
        if ok {
            Ok(())
        } else {
            Err(TerrainError::InvalidConfig)
        }
    }

    /// M2 (Shape) sections: tectonics, hardness, rain shadow, erosion.
    fn shape_valid(&self) -> bool {
        let t = &self.tectonics;
        let h = &self.hardness;
        let r = &self.rain_shadow;
        let e = &self.erosion;
        let km = |v: f64, low: f64, high: f64| v.is_finite() && (low..=high).contains(&v);
        let unit = |v: f64| (0.0..=1.0).contains(&v);
        t.plate_count.valid(2.0, MAX_MAJOR_PLATES)
            && t.microplates.valid(0, MAX_MICROPLATES)
            && unit(t.jitter)
            && (0.0..=0.5).contains(&t.micro_shrink)
            && t.continental_fraction.valid(0.0, 1.0)
            && km(t.warp_wavelength_km, 1.0, 1.0e5)
            && t.warp_strength.valid(0.0, 1.0)
            && t.speed.valid(0.01, 2.0)
            && t.crust_weight.valid(0.0, 0.9)
            && t.collision_height_m.valid(0.0, 1.0e4)
            && t.arc_height_m.valid(0.0, 1.0e4)
            && t.trench_depth_m.valid(0.0, 1.2e4)
            && t.ridge_height_m.valid(0.0, 5.0e3)
            && km(t.rift_depth_m, 0.0, 5.0e3)
            && km(t.transform_height_m, 0.0, 5.0e3)
            && t.orogen_width_km.valid(10.0, 1.0e3)
            && unit(t.roughness)
            && km(t.roughness_wavelength_km, 1.0, 1.0e4)
            && [
                t.arc_width_km,
                t.trench_width_km,
                t.ridge_width_km,
                t.rift_width_km,
                t.transform_width_km,
                t.crust_width_km,
                t.softness_km,
            ]
            .iter()
            .all(|w| km(*w, 1.0, 1.0e3))
            && km(t.arc_offset_km, t.arc_width_km, 1.0e3)
            && km(t.boundary_clamp_km, 10.0, 1.0e3)
            && km(t.junction_blend_km, 0.05, 5.0)
            && [h.crystalline, h.volcanic, h.sedimentary]
                .iter()
                .all(|v| unit(*v))
            && (0.0..=0.5).contains(&h.noise)
            && km(h.noise_wavelength_km, 1.0, 1.0e5)
            && unit(r.deflection)
            && km(r.deflection_slope, 1.0e-4, 1.0)
            && (0.0..=0.9).contains(&r.slowdown)
            && unit(r.orographic_rain)
            && unit(r.lee_drying)
            && km(r.reference_slope, 1.0e-4, 1.0)
            && e.strength.valid(0.0, 100.0)
            && e.uplift_m.valid(0.0, 5.0e3)
            && (5.0..=80.0).contains(&e.talus_deg)
            && unit(e.deposition)
            && km(e.capacity, 0.0, 100.0)
            && km(e.area_exponent, 0.1, 1.0)
            && km(e.slope_exponent, 0.5, 2.0)
            && km(e.flow_exponent, 0.5, 8.0)
            && (0.0..=0.2).contains(&e.thermal_rate)
            && km(e.sediment_depth_m, 0.1, 1.0e4)
            && e.cascade.iter().all(|(divisor, iterations)| {
                divisor.is_power_of_two() && *divisor <= 16 && *iterations <= 2000
            })
            && cascade_valid(&e.cascade)
    }

    /// Face cells for a body of `radius_m`: the first band that contains it,
    /// else the last band.
    pub fn face_cells(&self, radius_m: f64) -> u32 {
        let radius_km = radius_m / 1000.0;
        self.resolution
            .iter()
            .find(|band| radius_km <= band.max_radius_km)
            .or(self.resolution.last())
            .map_or(256, |band| band.face_cells)
    }

    /// Sample every range with per-stage seeds derived from `body_seed`.
    pub fn sample(&self, body_seed: u64) -> PlanetParams {
        let c = &self.continents;
        let t = &self.temperature;
        let m = &self.moisture;
        let draw = |stage: u64, index: u64| unit(stage_seed(body_seed, stage) ^ index);
        let tc = &self.tectonics;
        let hd = &self.hardness;
        let rs = &self.rain_shadow;
        let er = &self.erosion;
        PlanetParams {
            plate_count: tc.plate_count.sample(draw(STAGE_TECTONICS, 0)).round(),
            micro_count: tc.microplates.sample(draw(STAGE_TECTONICS, 1)),
            plate_jitter: tc.jitter,
            micro_shrink: tc.micro_shrink,
            continental_fraction: tc.continental_fraction.sample(draw(STAGE_TECTONICS, 2)),
            plate_warp_wavelength_m: 1000.0 * tc.warp_wavelength_km,
            plate_warp_strength: tc.warp_strength.sample(draw(STAGE_TECTONICS, 3)),
            plate_speed: tc.speed.sample(draw(STAGE_TECTONICS, 4)),
            crust_weight: tc.crust_weight.sample(draw(STAGE_TECTONICS, 5)),
            collision_height_m: tc.collision_height_m.sample(draw(STAGE_TECTONICS, 6)),
            arc_height_m: tc.arc_height_m.sample(draw(STAGE_TECTONICS, 7)),
            trench_depth_m: tc.trench_depth_m.sample(draw(STAGE_TECTONICS, 8)),
            ridge_height_m: tc.ridge_height_m.sample(draw(STAGE_TECTONICS, 9)),
            rift_depth_m: tc.rift_depth_m,
            transform_height_m: tc.transform_height_m,
            orogen_width_m: 1000.0 * tc.orogen_width_km.sample(draw(STAGE_TECTONICS, 10)),
            orogen_roughness: tc.roughness,
            orogen_roughness_wavelength_m: 1000.0 * tc.roughness_wavelength_km,
            arc_width_m: 1000.0 * tc.arc_width_km,
            arc_offset_m: 1000.0 * tc.arc_offset_km,
            trench_width_m: 1000.0 * tc.trench_width_km,
            ridge_width_m: 1000.0 * tc.ridge_width_km,
            rift_width_m: 1000.0 * tc.rift_width_km,
            transform_width_m: 1000.0 * tc.transform_width_km,
            crust_width_m: 1000.0 * tc.crust_width_km,
            boundary_softness_m: 1000.0 * tc.softness_km,
            boundary_clamp_m: 1000.0 * tc.boundary_clamp_km,
            junction_blend_m: 1000.0 * tc.junction_blend_km,
            hardness_crystalline: hd.crystalline,
            hardness_volcanic: hd.volcanic,
            hardness_sedimentary: hd.sedimentary,
            hardness_noise: hd.noise,
            hardness_noise_wavelength_m: 1000.0 * hd.noise_wavelength_km,
            wind_deflection: rs.deflection,
            deflection_slope: rs.deflection_slope,
            wind_slowdown: rs.slowdown,
            orographic_rain: rs.orographic_rain,
            lee_drying: rs.lee_drying,
            orographic_slope: rs.reference_slope,
            erosion_strength: er.strength.sample(draw(STAGE_EROSION, 0)),
            erosion_uplift_m: er.uplift_m.sample(draw(STAGE_EROSION, 1)),
            talus_deg: er.talus_deg,
            deposition: er.deposition,
            erosion_capacity: er.capacity,
            erosion_area_exponent: er.area_exponent,
            erosion_slope_exponent: er.slope_exponent,
            erosion_flow_exponent: er.flow_exponent,
            thermal_rate: er.thermal_rate,
            sediment_depth_m: er.sediment_depth_m,
            erosion_cascade: er.cascade,
            tectonic_seed: stage_seed(body_seed, STAGE_TECTONICS) as u32,
            hardness_seed: stage_seed(body_seed, STAGE_HARDNESS) as u32,
            ocean_coverage: self.ocean_coverage.sample(draw(STAGE_SEA_LEVEL, 0)),
            continent_wavelength_m: 1000.0 * c.wavelength_km.sample(draw(STAGE_CONTINENTS, 0)),
            continent_octaves: c.octaves,
            continent_lacunarity: c.lacunarity,
            continent_gain: c.gain,
            warp_wavelength_m: 1000.0 * c.warp_wavelength_km.sample(draw(STAGE_CONTINENTS, 1)),
            warp_strength: c.warp_strength.sample(draw(STAGE_CONTINENTS, 2)),
            land_height_m: c.land_height_m.sample(draw(STAGE_CONTINENTS, 3)),
            land_exponent: c.land_exponent,
            ocean_depth_m: c.ocean_depth_m.sample(draw(STAGE_CONTINENTS, 4)),
            shelf_depth_m: c.shelf_depth_m,
            shelf_fraction: c.shelf_fraction,
            equator_c: t.equator_c.sample(draw(STAGE_TEMPERATURE, 0)),
            pole_c: t.pole_c.sample(draw(STAGE_TEMPERATURE, 1)),
            axial_tilt_deg: t.axial_tilt_deg.sample(draw(STAGE_TEMPERATURE, 2)),
            lapse_c_per_km: t.lapse_c_per_km,
            ocean_moderation: t.ocean_moderation,
            ocean_blur_m: 1000.0 * t.ocean_blur_km,
            temperature_noise_c: t.noise_c,
            temperature_noise_wavelength_m: 1000.0 * t.noise_wavelength_km,
            wind_cells: self.wind.cells_per_hemisphere,
            wind_meridional: self.wind.meridional_fraction,
            moisture_iterations: m.iterations,
            moisture_step_m: 1000.0 * m.step_km,
            moisture_spread: m.spread,
            evaporation: m.evaporation.sample(draw(STAGE_MOISTURE, 0)),
            rain: m.rain.sample(draw(STAGE_MOISTURE, 1)),
            precipitation_scale: m.precipitation_scale,
            rain_convergence: m.convergence,
            continent_seed: stage_seed(body_seed, STAGE_CONTINENTS) as u32,
            warp_seed: stage_seed(body_seed, STAGE_WARP) as u32,
            temperature_seed: stage_seed(body_seed, STAGE_TEMPERATURE) as u32,
        }
    }
}

const STAGE_CONTINENTS: u64 = 0x434f_4e54;
const STAGE_WARP: u64 = 0x5741_5250;
const STAGE_SEA_LEVEL: u64 = 0x5345_414c;
const STAGE_TEMPERATURE: u64 = 0x5445_4d50;
const STAGE_MOISTURE: u64 = 0x4d4f_4953;
const STAGE_TECTONICS: u64 = 0x5445_4354;
const STAGE_HARDNESS: u64 = 0x4841_5244;
const STAGE_EROSION: u64 = 0x4552_4f44;

/// `hash(body_seed, stage)` (§5.1).
pub fn stage_seed(body_seed: u64, stage: u64) -> u64 {
    splitmix(splitmix(body_seed) ^ stage.wrapping_mul(0x9e37_79b9_7f4a_7c15))
}

fn unit(seed: u64) -> f64 {
    (splitmix(seed) >> 11) as f64 / (1u64 << 53) as f64
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Concrete sampled parameters of one body (§5.4 `PlanetParams`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanetParams {
    pub ocean_coverage: f64,
    pub continent_wavelength_m: f64,
    pub continent_octaves: u32,
    pub continent_lacunarity: f64,
    pub continent_gain: f64,
    pub warp_wavelength_m: f64,
    pub warp_strength: f64,
    pub land_height_m: f64,
    pub land_exponent: f64,
    pub ocean_depth_m: f64,
    pub shelf_depth_m: f64,
    pub shelf_fraction: f64,
    pub equator_c: f64,
    pub pole_c: f64,
    pub axial_tilt_deg: f64,
    pub lapse_c_per_km: f64,
    pub ocean_moderation: f64,
    pub ocean_blur_m: f64,
    pub temperature_noise_c: f64,
    pub temperature_noise_wavelength_m: f64,
    pub wind_cells: u32,
    pub wind_meridional: f64,
    pub moisture_iterations: u32,
    pub moisture_step_m: f64,
    pub moisture_spread: f64,
    pub evaporation: f64,
    pub rain: f64,
    pub precipitation_scale: f64,
    pub rain_convergence: f64,
    pub continent_seed: u32,
    pub warp_seed: u32,
    pub temperature_seed: u32,
    // Tectonics (M2): see `TectonicsRanges`; lengths in metres.
    /// Major plates; rounded and clamped to `2..=MAX_MAJOR_PLATES` on use.
    pub plate_count: f64,
    pub micro_count: u32,
    pub plate_jitter: f64,
    pub micro_shrink: f64,
    pub continental_fraction: f64,
    pub plate_warp_wavelength_m: f64,
    pub plate_warp_strength: f64,
    pub plate_speed: f64,
    pub crust_weight: f64,
    pub collision_height_m: f64,
    pub arc_height_m: f64,
    pub trench_depth_m: f64,
    pub ridge_height_m: f64,
    pub rift_depth_m: f64,
    pub transform_height_m: f64,
    pub orogen_width_m: f64,
    pub orogen_roughness: f64,
    pub orogen_roughness_wavelength_m: f64,
    pub arc_width_m: f64,
    pub arc_offset_m: f64,
    pub trench_width_m: f64,
    pub ridge_width_m: f64,
    pub rift_width_m: f64,
    pub transform_width_m: f64,
    pub crust_width_m: f64,
    pub boundary_softness_m: f64,
    pub boundary_clamp_m: f64,
    pub junction_blend_m: f64,
    // Hardness (M2): see `HardnessRanges`.
    pub hardness_crystalline: f64,
    pub hardness_volcanic: f64,
    pub hardness_sedimentary: f64,
    pub hardness_noise: f64,
    pub hardness_noise_wavelength_m: f64,
    // Rain shadow (M2): see `RainShadowRanges`.
    pub wind_deflection: f64,
    pub deflection_slope: f64,
    pub wind_slowdown: f64,
    pub orographic_rain: f64,
    pub lee_drying: f64,
    pub orographic_slope: f64,
    // Erosion (M2): see `ErosionRanges`.
    pub erosion_strength: f64,
    pub erosion_uplift_m: f64,
    pub talus_deg: f64,
    pub deposition: f64,
    pub erosion_capacity: f64,
    pub erosion_area_exponent: f64,
    pub erosion_slope_exponent: f64,
    pub erosion_flow_exponent: f64,
    pub thermal_rate: f64,
    pub sediment_depth_m: f64,
    pub erosion_cascade: [(u32, u32); 3],
    pub tectonic_seed: u32,
    pub hardness_seed: u32,
}

impl PlanetParams {
    /// Major plates in use: `plate_count` rounded, within `2..=MAX_MAJOR_PLATES`.
    pub fn major_plates(&self) -> usize {
        self.plate_count.round().clamp(2.0, MAX_MAJOR_PLATES) as usize
    }

    /// Microplates in use (at most `MAX_MICROPLATES`).
    pub fn microplates(&self) -> usize {
        self.micro_count.min(MAX_MICROPLATES) as usize
    }
}

/// One editor-adjustable planet parameter: the shared descriptor
/// (`astrum_core::params`, docs/STUDIO_UI.md amendment 2026-10-10b). Every
/// planet parameter is a `ParamKind::Float`; its bounds validate overrides.
pub type ParamField = ParamDesc<PlanetParams>;

/// `"Group": field: min, max[, "unit"];` The label derives from the field name.
macro_rules! fields {
    ($($group:literal: $name:ident: $min:expr, $max:expr $(, $unit:literal)?;)*) => {
        &[$(ParamDesc {
            key: stringify!($name),
            label: "",
            group: $group,
            unit: fields!(@unit $($unit)?),
            help: "",
            kind: ParamKind::Float { min: $min, max: $max, log: false },
            get: |p| ParamValue::Float(p.$name),
            set: |p, v| p.$name = v.as_f64(),
        }),*]
    };
    (@unit) => { "" };
    (@unit $unit:literal) => { $unit };
}

/// Continuous parameters exposed to overrides (planet editor, terrain files).
pub const PARAM_FIELDS: &[ParamField] = fields! {
    "Continents": ocean_coverage: 0.0, 0.98;
    "Continents": continent_wavelength_m: 1.0e3, 1.0e8, "m";
    "Continents": warp_wavelength_m: 1.0e3, 1.0e8, "m";
    "Continents": warp_strength: 0.0, 2.0;
    "Continents": land_height_m: 0.0, 2.0e4, "m";
    "Ocean": ocean_depth_m: 1.0, 2.0e4, "m";
    "Ocean": shelf_depth_m: 0.0, 2.0e3, "m";
    "Temperature": equator_c: -250.0, 500.0, "°C";
    "Temperature": pole_c: -250.0, 500.0, "°C";
    "Temperature": axial_tilt_deg: 0.0, 90.0, "°";
    "Temperature": lapse_c_per_km: 0.0, 50.0;
    "Temperature": ocean_moderation: 0.0, 1.0;
    "Temperature": temperature_noise_c: 0.0, 50.0, "°C";
    "Moisture": evaporation: 0.0, 1.0;
    "Moisture": rain: 0.0, 1.0;
    "Moisture": precipitation_scale: 1.0e-3, 1.0e3;
    "Moisture": rain_convergence: 0.0, 0.95;
    "Tectonics": plate_count: 2.0, MAX_MAJOR_PLATES;
    "Tectonics": continental_fraction: 0.0, 1.0;
    "Tectonics": plate_warp_strength: 0.0, 1.0;
    "Tectonics": plate_speed: 0.01, 2.0;
    "Tectonics": crust_weight: 0.0, 0.9;
    "Tectonics": collision_height_m: 0.0, 1.0e4, "m";
    "Tectonics": arc_height_m: 0.0, 1.0e4, "m";
    "Tectonics": trench_depth_m: 0.0, 1.2e4, "m";
    "Tectonics": ridge_height_m: 0.0, 5.0e3, "m";
    "Tectonics": orogen_width_m: 1.0e4, 1.0e6, "m";
    "Tectonics": orogen_roughness: 0.0, 1.0;
    "Tectonics": junction_blend_m: 50.0, 5.0e3, "m";
    "Rock hardness": hardness_noise: 0.0, 0.5;
    "Rain shadow": wind_deflection: 0.0, 1.0;
    "Rain shadow": orographic_rain: 0.0, 1.0;
    "Rain shadow": lee_drying: 0.0, 1.0;
    "Erosion": erosion_strength: 0.0, 100.0;
    "Erosion": erosion_uplift_m: 0.0, 5.0e3, "m";
    "Erosion": talus_deg: 5.0, 80.0, "°";
    "Erosion": deposition: 0.0, 1.0;
};

impl PlanetArchetype {
    /// Planet editor slider range of a parameter: the authored range widened
    /// by half its span on each side, or the current `value` within ×0.5–2
    /// for fixed parameters; always inside the field's validation bounds.
    pub fn editor_range(&self, name: &str, value: f64) -> Option<(f64, f64)> {
        let (min, max) = float_bounds(PARAM_FIELDS.iter().find(|f| f.key == name)?);
        let c = &self.continents;
        let t = &self.temperature;
        let m = &self.moisture;
        let km = |r: Range| Range(r.0 * 1000.0, r.1 * 1000.0);
        let authored = match name {
            "ocean_coverage" => Some(self.ocean_coverage),
            "continent_wavelength_m" => Some(km(c.wavelength_km)),
            "warp_wavelength_m" => Some(km(c.warp_wavelength_km)),
            "warp_strength" => Some(c.warp_strength),
            "land_height_m" => Some(c.land_height_m),
            "ocean_depth_m" => Some(c.ocean_depth_m),
            "equator_c" => Some(t.equator_c),
            "pole_c" => Some(t.pole_c),
            "axial_tilt_deg" => Some(t.axial_tilt_deg),
            "evaporation" => Some(m.evaporation),
            "rain" => Some(m.rain),
            "plate_count" => Some(self.tectonics.plate_count),
            "continental_fraction" => Some(self.tectonics.continental_fraction),
            "plate_warp_strength" => Some(self.tectonics.warp_strength),
            "plate_speed" => Some(self.tectonics.speed),
            "crust_weight" => Some(self.tectonics.crust_weight),
            "collision_height_m" => Some(self.tectonics.collision_height_m),
            "arc_height_m" => Some(self.tectonics.arc_height_m),
            "trench_depth_m" => Some(self.tectonics.trench_depth_m),
            "ridge_height_m" => Some(self.tectonics.ridge_height_m),
            "orogen_width_m" => Some(km(self.tectonics.orogen_width_km)),
            "erosion_strength" => Some(self.erosion.strength),
            "erosion_uplift_m" => Some(self.erosion.uplift_m),
            _ => None,
        };
        let (low, high) = match authored {
            Some(r) => {
                let pad = 0.5 * (r.1 - r.0).max(1e-3 * r.1.abs().max(1.0));
                (r.0 - pad, r.1 + pad)
            }
            None if value != 0.0 => (0.5 * value.min(2.0 * value), 2.0 * value.max(0.5 * value)),
            None => (min, min + 0.1 * (max - min)),
        };
        let low = low.max(min);
        let high = high.min(max);
        (low < high).then_some((low, high))
    }
}

/// Validation bounds of a planet parameter (all are `ParamKind::Float`).
pub fn float_bounds(field: &ParamField) -> (f64, f64) {
    match field.kind {
        ParamKind::Float { min, max, .. } => (min, max),
        _ => (f64::NEG_INFINITY, f64::INFINITY),
    }
}

impl Params for PlanetParams {
    fn descriptors() -> &'static [ParamField] {
        PARAM_FIELDS
    }
}

impl PlanetParams {
    pub fn get(&self, name: &str) -> Option<f64> {
        self.param(name).map(ParamValue::as_f64)
    }

    /// Override one named parameter; rejects unknown names and values outside
    /// the field's bounds.
    pub fn set(&mut self, name: &str, value: f64) -> Result<(), TerrainError> {
        self.set_param(name, ParamValue::Float(value))
            .map_err(|_| TerrainError::InvalidConfig)
    }

    /// Words participating in the owning definition's terrain identity.
    pub fn identity(&self) -> u64 {
        let words =
            PARAM_FIELDS
                .iter()
                .map(|f| (f.get)(self).as_f64().to_bits())
                .chain([
                    u64::from(self.continent_octaves),
                    self.continent_lacunarity.to_bits(),
                    self.continent_gain.to_bits(),
                    self.land_exponent.to_bits(),
                    self.shelf_fraction.to_bits(),
                    self.ocean_blur_m.to_bits(),
                    self.temperature_noise_wavelength_m.to_bits(),
                    u64::from(self.wind_cells),
                    self.wind_meridional.to_bits(),
                    u64::from(self.moisture_iterations),
                    self.moisture_step_m.to_bits(),
                    self.moisture_spread.to_bits(),
                    u64::from(self.continent_seed),
                    u64::from(self.warp_seed),
                    u64::from(self.temperature_seed),
                    u64::from(self.micro_count),
                    self.plate_jitter.to_bits(),
                    self.micro_shrink.to_bits(),
                    self.plate_warp_wavelength_m.to_bits(),
                    self.orogen_roughness_wavelength_m.to_bits(),
                    self.rift_depth_m.to_bits(),
                    self.transform_height_m.to_bits(),
                    self.arc_width_m.to_bits(),
                    self.arc_offset_m.to_bits(),
                    self.trench_width_m.to_bits(),
                    self.ridge_width_m.to_bits(),
                    self.rift_width_m.to_bits(),
                    self.transform_width_m.to_bits(),
                    self.crust_width_m.to_bits(),
                    self.boundary_softness_m.to_bits(),
                    self.boundary_clamp_m.to_bits(),
                    self.junction_blend_m.to_bits(),
                    self.hardness_crystalline.to_bits(),
                    self.hardness_volcanic.to_bits(),
                    self.hardness_sedimentary.to_bits(),
                    self.hardness_noise_wavelength_m.to_bits(),
                    self.deflection_slope.to_bits(),
                    self.wind_slowdown.to_bits(),
                    self.orographic_slope.to_bits(),
                    self.erosion_capacity.to_bits(),
                    self.erosion_area_exponent.to_bits(),
                    self.erosion_slope_exponent.to_bits(),
                    self.erosion_flow_exponent.to_bits(),
                    self.thermal_rate.to_bits(),
                    self.sediment_depth_m.to_bits(),
                    u64::from(self.tectonic_seed),
                    u64::from(self.hardness_seed),
                ])
                .chain(self.erosion_cascade.iter().map(|(divisor, iterations)| {
                    u64::from(*divisor) << 32 | u64::from(*iterations)
                }));
        words.fold(0x5449_4552_4100_0001, |hash, word| splitmix(hash ^ word))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn terra() -> PlanetArchetype {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/archetypes/terra.ron");
        ron::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn terra_archetype_parses_validates_and_samples_reproducibly() {
        let terra = terra();
        terra.validate().unwrap();
        assert_eq!(terra.face_cells(338_950.0), 512);
        let a = terra.sample(7);
        assert_eq!(a, terra.sample(7));
        assert_ne!(a, terra.sample(8));
        assert!((terra.ocean_coverage.0..=terra.ocean_coverage.1).contains(&a.ocean_coverage));
        let mut bad = terra.clone();
        bad.ocean_coverage = Range(0.9, 0.1);
        assert!(bad.validate().is_err());
    }

    #[test]
    fn planet_descriptors_are_unique_floats_and_round_trip() {
        let mut params = terra().sample(3);
        for (index, field) in PARAM_FIELDS.iter().enumerate() {
            assert_eq!(PlanetParams::param_index(field.key), Some(index));
            assert!(matches!(field.kind, ParamKind::Float { .. }), "{}", field.key);
            let (min, max) = float_bounds(field);
            let value = 0.5 * (min + max);
            params.set(field.key, value).unwrap();
            assert_eq!(params.get(field.key), Some(value));
        }
        assert_eq!(
            params.set_param("rain", ParamValue::Int(0)),
            Err(astrum_core::params::ParamError::WrongKind)
        );
    }

    #[test]
    fn stage_seeds_are_independent_and_overrides_are_bounded() {
        let terra = terra();
        let mut params = terra.sample(7);
        let before = params;
        params.set("rain", 0.5).unwrap();
        assert_eq!(params.rain, 0.5);
        assert_eq!(params.continent_seed, before.continent_seed);
        assert_eq!(params.land_height_m, before.land_height_m);
        assert!(params.set("rain", 2.0).is_err());
        assert!(params.set("unknown", 1.0).is_err());
        assert_ne!(params.identity(), before.identity());
        assert_ne!(
            stage_seed(7, STAGE_CONTINENTS),
            stage_seed(7, STAGE_MOISTURE)
        );
        // M2 stages draw from their own seeds: erosion overrides leave the
        // plates alone, and the new sliders are bounded.
        let mut eroded = before;
        eroded.set("erosion_strength", 5.0).unwrap();
        assert_eq!(eroded.tectonic_seed, before.tectonic_seed);
        assert_eq!(eroded.plate_count, before.plate_count);
        assert!(eroded.set("plate_count", 64.0).is_err());
        let seeds = [
            STAGE_TECTONICS,
            STAGE_HARDNESS,
            STAGE_EROSION,
            STAGE_MOISTURE,
        ];
        for (i, a) in seeds.iter().enumerate() {
            for b in &seeds[i + 1..] {
                assert_ne!(stage_seed(7, *a), stage_seed(7, *b));
            }
        }
        assert!((2..=26).contains(&before.major_plates()));
        assert!(cascade_valid(&[(4, 200), (2, 100), (1, 60)]));
        assert!(!cascade_valid(&[(1, 20), (2, 10), (0, 0)]));
    }
}
