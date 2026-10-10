//! Climate niche of a species: where it grows (genesis design §5.2,
//! prototype of G2 without the competition simulation).
//!
//! A niche is a set of trapezoid envelopes over the site facts the GPU
//! placement pass can read (temperature, moisture, height above sea level,
//! slope, Tier A sediment as the soil depth) plus the layer the species
//! fills. Placement (`scatter.rs` on the CPU, `scatter_niche.wgsl` on the
//! GPU) turns the niches into forest cover and species choice.

use serde::{Deserialize, Serialize};

use crate::params::{ParamDesc, ParamKind, ParamValue, Params};

/// Trapezoid membership: 1 in `[min, max]`, linear to 0 over `falloff`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub min: f64,
    pub max: f64,
    pub falloff: f64,
}

impl Envelope {
    pub fn eval(&self, x: f64) -> f64 {
        let d = (self.min - x).max(x - self.max).max(0.0);
        (1.0 - d / self.falloff.max(1e-6)).clamp(0.0, 1.0)
    }
}

/// Vegetation layer a species fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layer {
    /// Forest-forming trees (the forest field's canopy).
    Canopy,
    /// Fringe and understory shrubs.
    Shrub,
}

impl Layer {
    pub const ALL: &'static [Layer] = &[Layer::Canopy, Layer::Shrub];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|l| *l == self).unwrap()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Niche {
    pub layer: Layer,
    /// Air temperature, °C (the page climate).
    pub temperature_c: Envelope,
    /// Moisture 0..1 (the page climate).
    pub moisture: Envelope,
    /// Height above sea level, m; `max` is the species' treeline.
    pub height_m: Envelope,
    /// Slope (degrees) at which the species is half gone, and the half-width
    /// of the fade around it.
    pub max_slope_deg: f64,
    pub slope_falloff_deg: f64,
    /// Sediment (Tier A soil depth proxy, 0..1) for full vigour; thinner
    /// soil thins the species to SOIL_FLOOR at bare rock.
    pub soil_min: f64,
    /// Relative abundance among the species that fit a site.
    pub prior: f64,
    /// Size range (multiplier of the grown mesh).
    pub scale: (f64, f64),
}

/// Facts at a placement site (the same inputs the GPU pass samples).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Site {
    pub temperature_c: f64,
    pub moisture: f64,
    pub height_m: f64,
    /// Slope from horizontal, radians.
    pub slope: f64,
    /// Tier A sediment, 0..1.
    pub sediment: f64,
}

/// Vigour on bare rock (sediment 0) relative to deep soil. Tier A sediment is
/// 0 on about half of Rust's land (eroding uplands), so soil only thins.
pub const SOIL_FLOOR: f64 = 0.6;

pub fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Niche {
    /// Suitability 0..1 of `site` (mirrored by `fl_suit` in
    /// scatter_niche.wgsl).
    pub fn suitability(&self, s: &Site) -> f64 {
        let env = self.temperature_c.eval(s.temperature_c)
            * self.moisture.eval(s.moisture)
            * self.height_m.eval(s.height_m);
        let max = self.max_slope_deg.to_radians();
        let fall = self.slope_falloff_deg.to_radians();
        let steep = smoothstep(max - fall, max + fall, s.slope);
        let soil = (SOIL_FLOOR + (1.0 - SOIL_FLOOR) * (s.sediment + 1e-3) / (self.soil_min + 1e-3)).clamp(0.0, 1.0);
        // Nothing grows at or below sea level (shore fade over 15 m).
        let shore = smoothstep(0.0, 15.0, s.height_m);
        env * (1.0 - steep) * soil * shore
    }
}

const LAYERS: &[&str] = &["Canopy", "Shrub"];

macro_rules! env_params {
    ($group:literal, $field:ident, $unit:literal, $lo:expr, $hi:expr, $fall:expr) => {
        [
            ParamDesc {
                key: concat!(stringify!($field), ".min"),
                label: "",
                group: $group,
                unit: $unit,
                help: "Lower edge of full suitability",
                kind: ParamKind::Float { min: $lo, max: $hi, log: false },
                get: |n: &Niche| ParamValue::Float(n.$field.min),
                set: |n: &mut Niche, v| n.$field.min = v.as_f64(),
            },
            ParamDesc {
                key: concat!(stringify!($field), ".max"),
                label: "",
                group: $group,
                unit: $unit,
                help: "Upper edge of full suitability",
                kind: ParamKind::Float { min: $lo, max: $hi, log: false },
                get: |n: &Niche| ParamValue::Float(n.$field.max),
                set: |n: &mut Niche, v| n.$field.max = v.as_f64(),
            },
            ParamDesc {
                key: concat!(stringify!($field), ".falloff"),
                label: "",
                group: $group,
                unit: $unit,
                help: "Fade width outside the range",
                kind: ParamKind::Float { min: 0.0, max: $fall, log: false },
                get: |n: &Niche| ParamValue::Float(n.$field.falloff),
                set: |n: &mut Niche, v| n.$field.falloff = v.as_f64(),
            },
        ]
    };
}

fn float(
    key: &'static str,
    unit: &'static str,
    help: &'static str,
    min: f64,
    max: f64,
    get: fn(&Niche) -> ParamValue,
    set: fn(&mut Niche, ParamValue),
) -> ParamDesc<Niche> {
    ParamDesc { key, label: "", group: "Niche", unit, help, kind: ParamKind::Float { min, max, log: false }, get, set }
}

static NICHE_PARAMS: std::sync::LazyLock<Vec<ParamDesc<Niche>>> = std::sync::LazyLock::new(|| {
    let mut v = vec![ParamDesc {
        key: "layer",
        label: "",
        group: "Niche",
        unit: "",
        help: "Vegetation layer",
        kind: ParamKind::Choice { options: LAYERS },
        get: |n: &Niche| ParamValue::Choice(n.layer.index()),
        set: |n: &mut Niche, v| {
            if let ParamValue::Choice(i) = v {
                n.layer = Layer::ALL[i];
            }
        },
    }];
    v.extend(env_params!("Temperature", temperature_c, "°C", -100.0, 100.0, 50.0));
    v.extend(env_params!("Moisture", moisture, "", 0.0, 1.0, 1.0));
    v.extend(env_params!("Height", height_m, "m", -1000.0, 12000.0, 3000.0));
    v.push(float("max_slope_deg", "deg", "Slope at which the species is half gone", 0.0, 90.0, |n| ParamValue::Float(n.max_slope_deg), |n, v| n.max_slope_deg = v.as_f64()));
    v.push(float("slope_falloff_deg", "deg", "Half-width of the slope fade", 0.0, 45.0, |n| ParamValue::Float(n.slope_falloff_deg), |n, v| n.slope_falloff_deg = v.as_f64()));
    v.push(float("soil_min", "", "Sediment for full vigour", 0.0, 1.0, |n| ParamValue::Float(n.soil_min), |n, v| n.soil_min = v.as_f64()));
    v.push(float("prior", "", "Abundance among fitting species", 0.0, 10.0, |n| ParamValue::Float(n.prior), |n, v| n.prior = v.as_f64()));
    v.push(float("scale.min", "", "Smallest size multiplier", 0.1, 3.0, |n| ParamValue::Float(n.scale.0), |n, v| n.scale.0 = v.as_f64()));
    v.push(float("scale.max", "", "Largest size multiplier", 0.1, 3.0, |n| ParamValue::Float(n.scale.1), |n, v| n.scale.1 = v.as_f64()));
    v
});

impl Params for Niche {
    fn descriptors() -> &'static [ParamDesc<Self>] {
        NICHE_PARAMS.as_slice()
    }
}

impl Niche {
    pub fn validate(&self) -> Result<(), &'static str> {
        for d in Self::descriptors() {
            if d.kind.check((d.get)(self)).is_err() {
                return Err(d.key);
            }
        }
        if self.temperature_c.min > self.temperature_c.max
            || self.moisture.min > self.moisture.max
            || self.height_m.min > self.height_m.max
            || self.scale.0 > self.scale.1
        {
            return Err("range min above max");
        }
        Ok(())
    }
}
