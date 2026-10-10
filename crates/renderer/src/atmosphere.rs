//! Atmosphere of the nearest body: sky and aerial perspective (look preset
//! prototype, docs/RENDER_PIPELINE_HDR.md amendment 2026-10-10c). The
//! application supplies the body and its authored layers per frame; the post
//! chain draws `shaders/atmosphere.wgsl` after the AO composite.

use glam::DVec3;

use crate::{FrameLighting, LookPreset, RenderSettings};

/// Authored layers of a body's atmosphere (Earth-like defaults:
/// Rayleigh 5.8/13.5/33.1e-6 per m, H 8 km; Mie 21e-6 per m, H 1.2 km).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Atmosphere {
    /// Shell top above the reference radius.
    pub top_height_m: f64,
    /// Rayleigh scattering coefficients per metre at sea level (linear rgb).
    pub rayleigh_per_m: [f32; 3],
    pub rayleigh_scale_height_m: f64,
    /// Mie (aerosol) scattering coefficient per metre at sea level.
    pub mie_per_m: f32,
    pub mie_scale_height_m: f64,
    /// Henyey-Greenstein asymmetry of the aerosols.
    pub mie_g: f32,
}

impl Atmosphere {
    pub fn validate(&self) -> bool {
        self.top_height_m.is_finite()
            && self.top_height_m > 0.0
            && self.rayleigh_scale_height_m > 0.0
            && self.mie_scale_height_m > 0.0
            && self
                .rayleigh_per_m
                .iter()
                .all(|b| b.is_finite() && *b >= 0.0)
            && self.mie_per_m.is_finite()
            && self.mie_per_m >= 0.0
            && (-0.99..=0.99).contains(&self.mie_g)
    }
}

/// The atmosphere around the camera this frame: the body's centre in view
/// metres, its reference radius and its layers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameAtmosphere {
    pub center_view_m: DVec3,
    pub radius_m: f64,
    pub atmosphere: Atmosphere,
    /// The body's stylised look (used when the look preset is stylised).
    pub look: crate::StylisedLook,
}

impl FrameAtmosphere {
    pub fn validate(&self) -> bool {
        self.center_view_m.is_finite()
            && self.radius_m.is_finite()
            && self.radius_m > 0.0
            && self.atmosphere.validate()
            && self.look.validate()
    }
}

/// Byte size of the shader `Atmosphere` uniform.
pub(crate) const ATMOSPHERE_BYTES: u64 = 8 * 16;

/// Uniform rows for `atmosphere.wgsl`, or `None` when nothing is drawn (no
/// atmosphere, no light, or a debug view).
pub(crate) fn pack(
    atmosphere: Option<&FrameAtmosphere>,
    lighting: Option<&FrameLighting>,
    settings: &RenderSettings,
    tan_half: f64,
    aspect: f64,
    near_m: f64,
) -> Option<[[f32; 4]; 8]> {
    let atmosphere = atmosphere?;
    let lighting = lighting?;
    let look = settings.look;
    if look.sky_scale <= 0.0 && look.haze <= 0.0 {
        return None;
    }
    let distance = atmosphere.center_view_m.length();
    let up = -atmosphere.center_view_m / distance.max(1e-9);
    let altitude = distance - atmosphere.radius_m;
    let sun = lighting.sun_center_view_m;
    let sun_distance = sun.length().max(1.0);
    let sun_dir = sun / sun_distance;
    // Illuminance at the body (inverse square from the reference distance).
    let body_to_sun = (sun - atmosphere.center_view_m).length().max(1.0);
    let lux = lighting.illuminance_lux
        * (lighting.reference_distance_m / body_to_sun).powi(2)
        * f64::from(settings.lighting.sun_scale);
    let a = &atmosphere.atmosphere;
    let (tint, saturation, haze_boost, sky_boost) = match look.preset {
        LookPreset::Physical => ([1.0, 1.0, 1.0], 1.0, 1.0, 1.0),
        // The planet's stylised sky (lighting.json bodies.<id>.look).
        LookPreset::Stylised => {
            let s = &atmosphere.look;
            (s.sky_tint, s.sky_saturation, s.haze, s.sky)
        }
    };
    let s = |v: f64| v as f32;
    Some([
        [s(up.x), s(up.y), s(up.z), s(altitude)],
        [s(sun_dir.x), s(sun_dir.y), s(sun_dir.z), s(lux)],
        [
            s(atmosphere.radius_m),
            s(a.top_height_m),
            s(a.rayleigh_scale_height_m),
            s(a.mie_scale_height_m),
        ],
        [
            a.rayleigh_per_m[0],
            a.rayleigh_per_m[1],
            a.rayleigh_per_m[2],
            a.mie_per_m,
        ],
        [a.mie_g, 0.0, s(tan_half), s(aspect)],
        [
            s(near_m),
            1.0,
            look.haze * haze_boost,
            look.sky_scale * sky_boost,
        ],
        [tint[0], tint[1], tint[2], saturation],
        [0.0; 4],
    ])
}
