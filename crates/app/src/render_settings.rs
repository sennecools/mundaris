//! Discoverable render settings registry (docs/STUDIO_UI.md §2,
//! docs/RENDER_PIPELINE_HDR.md §4).
//!
//! Every tunable render value is registered once here with a stable id, a
//! group, an editor range and its default. The Studio inspector widgets, the
//! developer `setting` command and the snapshot listing are all generated from
//! [`SPECS`]; nothing else enumerates settings. Values are session state, not
//! content.
use anyhow::{Result, bail, ensure};
use mundaris_renderer::{
    ExposureMode, RenderSettings, SHADOW_RESOLUTIONS, TerrainViewMode, Tonemapper,
};
use serde_json::{Value, json};

/// A setting value in registry form. Choices are indices into the spec's options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Float(f64),
    Choice(usize),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingKind {
    Bool,
    Float {
        min: f64,
        max: f64,
        logarithmic: bool,
        unit: &'static str,
    },
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettingSpec {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub kind: SettingKind,
}

/// The registry's editable state: renderer settings plus the debug view.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderState {
    pub settings: RenderSettings,
    pub view_mode: TerrainViewMode,
}

const fn float(min: f64, max: f64, unit: &'static str) -> SettingKind {
    SettingKind::Float {
        min,
        max,
        logarithmic: false,
        unit,
    }
}
const fn log(min: f64, max: f64, unit: &'static str) -> SettingKind {
    SettingKind::Float {
        min,
        max,
        logarithmic: true,
        unit,
    }
}
const fn spec(
    id: &'static str,
    group: &'static str,
    label: &'static str,
    kind: SettingKind,
) -> SettingSpec {
    SettingSpec {
        id,
        group,
        label,
        kind,
    }
}

const VIEW_MODES: &[&str] = &[
    "lit",
    "unlit",
    "height",
    "normals",
    "grid",
    "level",
    "morph_fade",
    "ao",
    "shadows",
    "luminance",
];
const TONEMAPPERS: &[&str] = &["agx", "aces", "clamp"];
const EXPOSURE_MODES: &[&str] = &["auto", "manual"];
const CASCADE_COUNTS: &[&str] = &["1", "2", "3", "4"];
const RESOLUTIONS: &[&str] = &["1024", "2048", "4096"];
const SUN_MODES: &[&str] = &["star", "studio"];

pub const SPECS: &[SettingSpec] = &[
    spec(
        "render.view_mode",
        "View",
        "View mode",
        SettingKind::Choice(VIEW_MODES),
    ),
    spec(
        "render.tonemap.operator",
        "Display",
        "Tonemapper",
        SettingKind::Choice(TONEMAPPERS),
    ),
    spec("render.dither", "Display", "Dither", SettingKind::Bool),
    spec(
        "render.exposure.mode",
        "Exposure",
        "Mode",
        SettingKind::Choice(EXPOSURE_MODES),
    ),
    spec(
        "render.exposure.ev100",
        "Exposure",
        "Manual EV100",
        float(-2.0, 20.0, "EV"),
    ),
    spec(
        "render.exposure.compensation",
        "Exposure",
        "Compensation",
        float(-5.0, 5.0, "EV"),
    ),
    spec(
        "render.exposure.min",
        "Exposure",
        "Auto min",
        float(-4.0, 20.0, "EV"),
    ),
    spec(
        "render.exposure.max",
        "Exposure",
        "Auto max",
        float(-2.0, 24.0, "EV"),
    ),
    spec(
        "render.exposure.speed_up",
        "Exposure",
        "Adapt to bright",
        float(0.1, 10.0, "EV/s"),
    ),
    spec(
        "render.exposure.speed_down",
        "Exposure",
        "Adapt to dark",
        float(0.1, 10.0, "EV/s"),
    ),
    spec(
        "render.lighting.sun_mode",
        "Lighting",
        "Sun",
        SettingKind::Choice(SUN_MODES),
    ),
    spec(
        "render.lighting.sun_elevation",
        "Lighting",
        "Studio sun elevation",
        float(-10.0, 90.0, "°"),
    ),
    spec(
        "render.lighting.sun_azimuth",
        "Lighting",
        "Studio sun azimuth (from view)",
        float(0.0, 360.0, "°"),
    ),
    spec(
        "render.lighting.sun_scale",
        "Lighting",
        "Sun intensity",
        float(0.0, 4.0, "×"),
    ),
    spec(
        "render.lighting.ambient_scale",
        "Lighting",
        "Ambient + bounce",
        float(0.0, 10.0, "×"),
    ),
    spec(
        "render.shadows.enabled",
        "Shadows",
        "Sun shadows",
        SettingKind::Bool,
    ),
    spec(
        "render.shadows.eclipses",
        "Shadows",
        "Eclipses / body shadows",
        SettingKind::Bool,
    ),
    spec(
        "render.shadows.cascades",
        "Shadows",
        "Cascades",
        SettingKind::Choice(CASCADE_COUNTS),
    ),
    spec(
        "render.shadows.resolution",
        "Shadows",
        "Resolution",
        SettingKind::Choice(RESOLUTIONS),
    ),
    spec(
        "render.shadows.max_distance",
        "Shadows",
        "Max distance",
        log(1_000.0, 1_000_000.0, "m"),
    ),
    spec(
        "render.shadows.first_cascade",
        "Shadows",
        "First cascade",
        log(5.0, 1_000.0, "m"),
    ),
    spec(
        "render.shadows.softness",
        "Shadows",
        "Penumbra (sun size)",
        float(0.0, 4.0, "×"),
    ),
    spec(
        "render.shadows.normal_bias",
        "Shadows",
        "Normal bias",
        float(0.0, 5.0, "texels"),
    ),
    spec(
        "render.shadows.caster_detail",
        "Shadows",
        "Caster detail",
        float(0.25, 1.0, "×"),
    ),
    spec(
        "render.ao.enabled",
        "Ambient occlusion",
        "GTAO",
        SettingKind::Bool,
    ),
    spec(
        "render.ao.radius",
        "Ambient occlusion",
        "Radius",
        log(0.25, 50.0, "m"),
    ),
    spec(
        "render.ao.distance_scale",
        "Ambient occlusion",
        "Radius growth",
        float(0.0, 0.05, "×depth"),
    ),
    spec(
        "render.ao.intensity",
        "Ambient occlusion",
        "Intensity",
        float(0.0, 3.0, ""),
    ),
    spec(
        "render.ao.half_res",
        "Ambient occlusion",
        "Half resolution",
        SettingKind::Bool,
    ),
    spec("render.bloom.enabled", "Bloom", "Bloom", SettingKind::Bool),
    spec(
        "render.bloom.intensity",
        "Bloom",
        "Intensity",
        float(0.0, 0.3, ""),
    ),
    spec(
        "render.bloom.radius",
        "Bloom",
        "Radius",
        float(0.25, 3.0, "texels"),
    ),
    spec(
        "render.overlays.line_width",
        "Overlays",
        "Line width",
        float(0.5, 3.0, "×"),
    ),
    spec(
        "render.overlays.opacity",
        "Overlays",
        "Line opacity",
        float(0.0, 1.0, ""),
    ),
];

pub fn index_of(id: &str) -> Option<usize> {
    SPECS.iter().position(|spec| spec.id == id)
}

fn choice_index<T: PartialEq + Copy>(all: &[T], value: T) -> usize {
    all.iter().position(|v| *v == value).unwrap_or(0)
}

/// Current value of setting `index`.
pub fn get(state: &RenderState, index: usize) -> SettingValue {
    use SettingValue::{Bool, Choice, Float};
    let s = &state.settings;
    let f = |v: f32| Float(f64::from(v));
    match SPECS[index].id {
        "render.view_mode" => Choice(choice_index(&TerrainViewMode::ALL, state.view_mode)),
        "render.tonemap.operator" => Choice(choice_index(&Tonemapper::ALL, s.tonemap)),
        "render.dither" => Bool(s.dither),
        "render.exposure.mode" => Choice(choice_index(&ExposureMode::ALL, s.exposure.mode)),
        "render.exposure.ev100" => f(s.exposure.ev100),
        "render.exposure.compensation" => f(s.exposure.compensation),
        "render.exposure.min" => f(s.exposure.min_ev100),
        "render.exposure.max" => f(s.exposure.max_ev100),
        "render.exposure.speed_up" => f(s.exposure.speed_up),
        "render.exposure.speed_down" => f(s.exposure.speed_down),
        "render.lighting.sun_mode" => Choice(usize::from(s.lighting.studio_sun)),
        "render.lighting.sun_elevation" => f(s.lighting.sun_elevation_deg),
        "render.lighting.sun_azimuth" => f(s.lighting.sun_azimuth_deg),
        "render.lighting.sun_scale" => f(s.lighting.sun_scale),
        "render.lighting.ambient_scale" => f(s.lighting.ambient_scale),
        "render.shadows.enabled" => Bool(s.shadows.enabled),
        "render.shadows.eclipses" => Bool(s.shadows.eclipses),
        "render.shadows.cascades" => Choice(s.shadows.cascades.clamp(1, 4) as usize - 1),
        "render.shadows.resolution" => {
            Choice(choice_index(&SHADOW_RESOLUTIONS, s.shadows.resolution))
        }
        "render.shadows.max_distance" => f(s.shadows.max_distance_m),
        "render.shadows.first_cascade" => f(s.shadows.first_cascade_m),
        "render.shadows.softness" => f(s.shadows.softness),
        "render.shadows.normal_bias" => f(s.shadows.normal_bias),
        "render.shadows.caster_detail" => f(s.shadows.caster_detail),
        "render.ao.enabled" => Bool(s.ao.enabled),
        "render.ao.radius" => f(s.ao.radius_m),
        "render.ao.distance_scale" => f(s.ao.distance_scale),
        "render.ao.intensity" => f(s.ao.intensity),
        "render.ao.half_res" => Bool(s.ao.half_res),
        "render.bloom.enabled" => Bool(s.bloom.enabled),
        "render.bloom.intensity" => f(s.bloom.intensity),
        "render.bloom.radius" => f(s.bloom.radius),
        "render.overlays.line_width" => f(s.overlays.line_width_scale),
        "render.overlays.opacity" => f(s.overlays.opacity),
        id => unreachable!("registry id {id} has no accessor"),
    }
}

/// Sets setting `index`, checking its kind and range. The resulting settings
/// are validated as a whole; on failure nothing changes.
pub fn set(state: &mut RenderState, index: usize, value: SettingValue) -> Result<()> {
    let spec = SPECS
        .get(index)
        .ok_or_else(|| anyhow::anyhow!("unknown setting index {index}"))?;
    match (spec.kind, value) {
        (SettingKind::Bool, SettingValue::Bool(_)) => {}
        (SettingKind::Float { min, max, .. }, SettingValue::Float(v)) => {
            ensure!(
                v.is_finite() && (min..=max).contains(&v),
                "{} must be within {min}..{max}",
                spec.id
            );
        }
        (SettingKind::Choice(options), SettingValue::Choice(i)) => {
            ensure!(i < options.len(), "{} has no option {i}", spec.id);
        }
        _ => bail!("{} expects a {:?} value", spec.id, spec.kind),
    }
    let mut next = *state;
    let s = &mut next.settings;
    let b = |v: SettingValue| matches!(v, SettingValue::Bool(true));
    let f = |v: SettingValue| match v {
        SettingValue::Float(x) => x as f32,
        _ => 0.0,
    };
    let c = |v: SettingValue| match v {
        SettingValue::Choice(i) => i,
        _ => 0,
    };
    match spec.id {
        "render.view_mode" => next.view_mode = TerrainViewMode::ALL[c(value)],
        "render.tonemap.operator" => s.tonemap = Tonemapper::ALL[c(value)],
        "render.dither" => s.dither = b(value),
        "render.exposure.mode" => s.exposure.mode = ExposureMode::ALL[c(value)],
        "render.exposure.ev100" => s.exposure.ev100 = f(value),
        "render.exposure.compensation" => s.exposure.compensation = f(value),
        "render.exposure.min" => s.exposure.min_ev100 = f(value),
        "render.exposure.max" => s.exposure.max_ev100 = f(value),
        "render.exposure.speed_up" => s.exposure.speed_up = f(value),
        "render.exposure.speed_down" => s.exposure.speed_down = f(value),
        "render.lighting.sun_mode" => s.lighting.studio_sun = c(value) == 1,
        "render.lighting.sun_elevation" => s.lighting.sun_elevation_deg = f(value),
        "render.lighting.sun_azimuth" => s.lighting.sun_azimuth_deg = f(value),
        "render.lighting.sun_scale" => s.lighting.sun_scale = f(value),
        "render.lighting.ambient_scale" => s.lighting.ambient_scale = f(value),
        "render.shadows.enabled" => s.shadows.enabled = b(value),
        "render.shadows.eclipses" => s.shadows.eclipses = b(value),
        "render.shadows.cascades" => s.shadows.cascades = c(value) as u32 + 1,
        "render.shadows.resolution" => s.shadows.resolution = SHADOW_RESOLUTIONS[c(value)],
        "render.shadows.max_distance" => s.shadows.max_distance_m = f(value),
        "render.shadows.first_cascade" => s.shadows.first_cascade_m = f(value),
        "render.shadows.softness" => s.shadows.softness = f(value),
        "render.shadows.normal_bias" => s.shadows.normal_bias = f(value),
        "render.shadows.caster_detail" => s.shadows.caster_detail = f(value),
        "render.ao.enabled" => s.ao.enabled = b(value),
        "render.ao.radius" => s.ao.radius_m = f(value),
        "render.ao.distance_scale" => s.ao.distance_scale = f(value),
        "render.ao.intensity" => s.ao.intensity = f(value),
        "render.ao.half_res" => s.ao.half_res = b(value),
        "render.bloom.enabled" => s.bloom.enabled = b(value),
        "render.bloom.intensity" => s.bloom.intensity = f(value),
        "render.bloom.radius" => s.bloom.radius = f(value),
        "render.overlays.line_width" => s.overlays.line_width_scale = f(value),
        "render.overlays.opacity" => s.overlays.opacity = f(value),
        id => bail!("registry id {id} has no accessor"),
    }
    next.settings
        .validate()
        .map_err(|error| anyhow::anyhow!("{}: {error}", spec.id))?;
    *state = next;
    Ok(())
}

/// Parses a JSON value for setting `index`: booleans, numbers, or option names.
pub fn value_from_json(index: usize, value: &Value) -> Result<SettingValue> {
    let spec = &SPECS[index];
    Ok(match spec.kind {
        SettingKind::Bool => SettingValue::Bool(
            value
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("{} expects a boolean", spec.id))?,
        ),
        SettingKind::Float { .. } => SettingValue::Float(
            value
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("{} expects a number", spec.id))?,
        ),
        SettingKind::Choice(options) => {
            let name = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{} expects one of {options:?}", spec.id))?;
            SettingValue::Choice(
                options
                    .iter()
                    .position(|option| *option == name)
                    .ok_or_else(|| anyhow::anyhow!("{} expects one of {options:?}", spec.id))?,
            )
        }
    })
}

fn value_json(index: usize, value: SettingValue) -> Value {
    match (SPECS[index].kind, value) {
        (_, SettingValue::Bool(b)) => json!(b),
        (_, SettingValue::Float(f)) => json!(f),
        (SettingKind::Choice(options), SettingValue::Choice(i)) => json!(options.get(i)),
        (_, SettingValue::Choice(i)) => json!(i),
    }
}

/// Full registry listing for snapshots and `settings.list`.
pub fn listing(state: &RenderState) -> Value {
    let defaults = RenderState::default();
    Value::Array(
        SPECS
            .iter()
            .enumerate()
            .map(|(index, spec)| {
                let kind = match spec.kind {
                    SettingKind::Bool => json!({"type": "bool"}),
                    SettingKind::Float {
                        min,
                        max,
                        logarithmic,
                        unit,
                    } => json!({"type": "float", "min": min, "max": max, "log": logarithmic, "unit": unit}),
                    SettingKind::Choice(options) => json!({"type": "choice", "options": options}),
                };
                json!({
                    "id": spec.id,
                    "group": spec.group,
                    "label": spec.label,
                    "kind": kind,
                    "value": value_json(index, get(state, index)),
                    "default": value_json(index, get(&defaults, index)),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_round_trips_its_default_and_is_unique() {
        let mut state = RenderState::default();
        let mut ids = std::collections::HashSet::new();
        for (index, spec) in SPECS.iter().enumerate() {
            assert!(ids.insert(spec.id), "duplicate id {}", spec.id);
            assert!(spec.id.starts_with("render."));
            let value = get(&state, index);
            if let (SettingKind::Float { min, max, .. }, SettingValue::Float(v)) =
                (spec.kind, value)
            {
                assert!(
                    (min..=max).contains(&v),
                    "{} default outside editor range",
                    spec.id
                );
            }
            set(&mut state, index, value).unwrap();
            assert_eq!(get(&state, index), value, "{}", spec.id);
        }
        assert_eq!(state, RenderState::default());
        assert_eq!(VIEW_MODES.len(), TerrainViewMode::ALL.len());
        for (name, mode) in VIEW_MODES.iter().zip(TerrainViewMode::ALL) {
            assert_eq!(*name, mode.name());
        }
    }

    #[test]
    fn invalid_values_leave_state_unchanged() {
        let mut state = RenderState::default();
        let ev = index_of("render.exposure.ev100").unwrap();
        assert!(set(&mut state, ev, SettingValue::Float(99.0)).is_err());
        assert!(set(&mut state, ev, SettingValue::Bool(true)).is_err());
        let min = index_of("render.exposure.min").unwrap();
        // Min above the max fails whole-settings validation.
        assert!(set(&mut state, min, SettingValue::Float(19.0)).is_err());
        assert_eq!(state, RenderState::default());
        let mode = index_of("render.view_mode").unwrap();
        let shadows = value_from_json(mode, &json!("shadows")).unwrap();
        set(&mut state, mode, shadows).unwrap();
        assert_eq!(state.view_mode, TerrainViewMode::Shadows);
        assert!(value_from_json(mode, &json!("bogus")).is_err());
        assert_eq!(listing(&state).as_array().unwrap().len(), SPECS.len());
    }
}
