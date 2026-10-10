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
    /// A cloud layer, if the planet has one.
    pub clouds: Option<Clouds>,
}

/// A thin stylised cloud layer at one altitude (noise coverage on a sphere,
/// fixed to the body).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clouds {
    pub altitude_m: f64,
    /// Fraction of the sky covered, 0..1.
    pub coverage: f32,
    /// Size of the cloud features.
    pub scale_m: f64,
    /// Opacity of a full cloud, 0..1.
    pub opacity: f32,
}

impl Clouds {
    pub fn validate(&self) -> bool {
        self.altitude_m.is_finite()
            && self.altitude_m > 0.0
            && self.scale_m.is_finite()
            && self.scale_m > 0.0
            && (0.0..=1.0).contains(&self.coverage)
            && (0.0..=1.0).contains(&self.opacity)
    }
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
            && self.clouds.is_none_or(|c| c.validate())
    }
}

/// The atmosphere around the camera this frame: the body's centre in view
/// metres, its reference radius and its layers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameAtmosphere {
    pub center_view_m: DVec3,
    pub radius_m: f64,
    /// The body's x, y, z axes in view space (cloud noise is body-fixed).
    pub body_axes_view: [DVec3; 3],
    pub atmosphere: Atmosphere,
    /// The body's stylised look (used when the look preset is stylised).
    pub look: crate::StylisedLook,
}

impl FrameAtmosphere {
    pub fn validate(&self) -> bool {
        self.center_view_m.is_finite()
            && self.radius_m.is_finite()
            && self.radius_m > 0.0
            && self.body_axes_view.iter().all(|a| a.is_finite())
            && self.atmosphere.validate()
            && self.look.validate()
    }
}

/// Rows of the shader `Atmosphere` uniform.
pub(crate) const ATMOSPHERE_ROWS: usize = 12;
/// Byte size of the shader `Atmosphere` uniform.
pub(crate) const ATMOSPHERE_BYTES: u64 = ATMOSPHERE_ROWS as u64 * 16;

/// Uniform rows for `atmosphere.wgsl`, or `None` when nothing is drawn (no
/// atmosphere, no light, or a debug view).
pub(crate) fn pack(
    atmosphere: Option<&FrameAtmosphere>,
    lighting: Option<&FrameLighting>,
    settings: &RenderSettings,
    tan_half: f64,
    aspect: f64,
    near_m: f64,
) -> Option<[[f32; 4]; ATMOSPHERE_ROWS]> {
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
    // The stylised sky is a ground-level look: it fades back to physical as
    // the camera climbs out of the atmosphere, so the limb seen from orbit
    // stays a thin rim (art direction: no heavy glow).
    let stylised = match look.preset {
        LookPreset::Physical => 0.0,
        LookPreset::Stylised => {
            let top = a.top_height_m.max(1.0);
            1.0 - ((altitude - 0.5 * top) / (2.0 * top)).clamp(0.0, 1.0) as f32
        }
    };
    let s = &atmosphere.look;
    let mix = |physical: f32, styled: f32| physical + (styled - physical) * stylised;
    let tint = [0, 1, 2].map(|i| mix(1.0, s.sky_tint[i]));
    let (saturation, haze_boost, sky_boost) = (
        mix(1.0, s.sky_saturation),
        mix(1.0, s.haze),
        mix(1.0, s.sky),
    );
    // Sun disc: physical angular radius and luminance; the stylised look
    // draws it larger with a soft glow.
    let angular_radius = (lighting.sun_radius_m / sun_distance).clamp(1e-5, 0.2);
    let disc_radius = angular_radius * f64::from(mix(1.0, 3.0));
    let glow = mix(0.0, 1.0);
    let clouds = a.clouds;
    let axes = atmosphere.body_axes_view;
    let f = |v: f64| v as f32;
    Some([
        [f(up.x), f(up.y), f(up.z), f(altitude)],
        [f(sun_dir.x), f(sun_dir.y), f(sun_dir.z), f(lux)],
        [
            f(atmosphere.radius_m),
            f(a.top_height_m),
            f(a.rayleigh_scale_height_m),
            f(a.mie_scale_height_m),
        ],
        [
            a.rayleigh_per_m[0],
            a.rayleigh_per_m[1],
            a.rayleigh_per_m[2],
            a.mie_per_m,
        ],
        [a.mie_g, 0.0, f(tan_half), f(aspect)],
        [
            f(near_m),
            1.0,
            look.haze * haze_boost,
            look.sky_scale * sky_boost,
        ],
        [tint[0], tint[1], tint[2], saturation],
        clouds.map_or([0.0; 4], |c| {
            [f(c.altitude_m), c.coverage, f(c.scale_m), c.opacity]
        }),
        [f(axes[0].x), f(axes[0].y), f(axes[0].z), stylised],
        [f(axes[1].x), f(axes[1].y), f(axes[1].z), 0.0],
        [f(axes[2].x), f(axes[2].y), f(axes[2].z), 0.0],
        [
            f(disc_radius.cos()),
            f(angular_radius),
            glow,
            if clouds.is_some() { 1.0 } else { 0.0 },
        ],
    ])
}
