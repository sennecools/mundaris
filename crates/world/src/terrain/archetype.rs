//! Planet archetypes and sampled planet parameters
//! (`docs/ASTRUM_TERRAIN_PIPELINE.md` §5).
//!
//! An archetype is authored data (RON) describing parameter *ranges* and the
//! Tier A stages a body runs. A concrete body samples every range with seeds
//! derived per stage from its body seed (§5.1), so changing one stage's inputs
//! never reshuffles another stage. Editor overrides replace sampled values by
//! name ([`PlanetParams::set`]).
use super::TerrainError;
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

/// Tier A stages (§6.5) available in M1 (World map and planet editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TierAStage {
    Continents,
    SeaLevel,
    Shelf,
    Temperature,
    Wind,
    Moisture,
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
            && colour_valid(self.average_colour);
        if ok {
            Ok(())
        } else {
            Err(TerrainError::InvalidConfig)
        }
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
        PlanetParams {
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
    pub continent_seed: u32,
    pub warp_seed: u32,
    pub temperature_seed: u32,
}

/// One editor-adjustable parameter: name, authored bounds and accessors.
pub struct ParamField {
    pub name: &'static str,
    pub min: f64,
    pub max: f64,
    get: fn(&PlanetParams) -> f64,
    set: fn(&mut PlanetParams, f64),
}

macro_rules! fields {
    ($($name:ident: $min:expr, $max:expr;)*) => {
        &[$(ParamField {
            name: stringify!($name),
            min: $min,
            max: $max,
            get: |p| p.$name,
            set: |p, v| p.$name = v,
        }),*]
    };
}

/// Continuous parameters exposed to overrides (planet editor, terrain files).
pub const PARAM_FIELDS: &[ParamField] = fields! {
    ocean_coverage: 0.0, 0.98;
    continent_wavelength_m: 1.0e3, 1.0e8;
    warp_wavelength_m: 1.0e3, 1.0e8;
    warp_strength: 0.0, 2.0;
    land_height_m: 0.0, 2.0e4;
    ocean_depth_m: 1.0, 2.0e4;
    shelf_depth_m: 0.0, 2.0e3;
    equator_c: -250.0, 500.0;
    pole_c: -250.0, 500.0;
    axial_tilt_deg: 0.0, 90.0;
    lapse_c_per_km: 0.0, 50.0;
    ocean_moderation: 0.0, 1.0;
    temperature_noise_c: 0.0, 50.0;
    evaporation: 0.0, 1.0;
    rain: 0.0, 1.0;
    precipitation_scale: 1.0e-3, 1.0e3;
};

impl PlanetArchetype {
    /// Planet editor slider range of a parameter: the authored range widened
    /// by half its span on each side, or the current `value` within ×0.5–2
    /// for fixed parameters; always inside the field's validation bounds.
    pub fn editor_range(&self, name: &str, value: f64) -> Option<(f64, f64)> {
        let field = PARAM_FIELDS.iter().find(|f| f.name == name)?;
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
            _ => None,
        };
        let (low, high) = match authored {
            Some(r) => {
                let pad = 0.5 * (r.1 - r.0).max(1e-3 * r.1.abs().max(1.0));
                (r.0 - pad, r.1 + pad)
            }
            None if value != 0.0 => (0.5 * value.min(2.0 * value), 2.0 * value.max(0.5 * value)),
            None => (field.min, field.min + 0.1 * (field.max - field.min)),
        };
        let low = low.max(field.min);
        let high = high.min(field.max);
        (low < high).then_some((low, high))
    }
}

impl PlanetParams {
    pub fn get(&self, name: &str) -> Option<f64> {
        PARAM_FIELDS
            .iter()
            .find(|f| f.name == name)
            .map(|f| (f.get)(self))
    }

    /// Override one named parameter; rejects unknown names and values outside
    /// the field's bounds.
    pub fn set(&mut self, name: &str, value: f64) -> Result<(), TerrainError> {
        let field = PARAM_FIELDS
            .iter()
            .find(|f| f.name == name)
            .ok_or(TerrainError::InvalidConfig)?;
        if !value.is_finite() || !(field.min..=field.max).contains(&value) {
            return Err(TerrainError::InvalidConfig);
        }
        (field.set)(self, value);
        Ok(())
    }

    /// Words participating in the owning definition's terrain identity.
    pub fn identity(&self) -> u64 {
        let words = PARAM_FIELDS.iter().map(|f| (f.get)(self).to_bits()).chain([
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
        ]);
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
    }
}
