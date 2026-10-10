//! Toggleable render pipeline settings (docs/RENDER_PIPELINE_HDR.md §4).
//!
//! Plain data with every default in one place. The application registry
//! exposes each field under a stable `render.*` id; session overrides are not
//! content. Validation enforces the hard limits the GPU passes rely on.

/// Display transform applied after exposure and bloom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tonemapper {
    #[default]
    AgX,
    AcesFitted,
    /// Exposure only, then clamp: reference for A/B comparison.
    Clamp,
}

impl Tonemapper {
    pub const ALL: [Self; 3] = [Self::AgX, Self::AcesFitted, Self::Clamp];
    pub fn name(self) -> &'static str {
        match self {
            Self::AgX => "agx",
            Self::AcesFitted => "aces",
            Self::Clamp => "clamp",
        }
    }
    pub(crate) fn shader_index(self) -> u32 {
        match self {
            Self::AgX => 0,
            Self::AcesFitted => 1,
            Self::Clamp => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExposureMode {
    /// Histogram-metered EV100 with eye adaptation.
    #[default]
    Auto,
    Manual,
}

impl ExposureMode {
    pub const ALL: [Self; 2] = [Self::Auto, Self::Manual];
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// Anti-aliasing method (amendment 2026-10-10 anti-aliasing). MSAA counts the
/// adapter lacks fall back to the largest supported count below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AntiAliasing {
    Off,
    /// Post-process edge blur on the tonemapped image (FXAA 3.11 quality).
    Fxaa,
    Msaa2,
    /// Default: user choice 2026-10-10 after the side-by-side comparison.
    #[default]
    Msaa4,
    Msaa8,
    /// Temporal: jittered frames reprojected and clipped (camera motion only).
    Taa,
}

impl AntiAliasing {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Fxaa,
        Self::Msaa2,
        Self::Msaa4,
        Self::Msaa8,
        Self::Taa,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Fxaa => "fxaa",
            Self::Msaa2 => "msaa2",
            Self::Msaa4 => "msaa4",
            Self::Msaa8 => "msaa8",
            Self::Taa => "taa",
        }
    }
    /// Main-pass samples per pixel this method asks for.
    pub fn samples(self) -> u32 {
        match self {
            Self::Off | Self::Fxaa | Self::Taa => 1,
            Self::Msaa2 => 2,
            Self::Msaa4 => 4,
            Self::Msaa8 => 8,
        }
    }
    pub fn fxaa(self) -> bool {
        self == Self::Fxaa
    }
    pub fn taa(self) -> bool {
        self == Self::Taa
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureSettings {
    pub mode: ExposureMode,
    /// Manual exposure value at ISO 100.
    pub ev100: f32,
    /// Stops added to the metered or manual exposure (positive = brighter).
    pub compensation: f32,
    pub min_ev100: f32,
    pub max_ev100: f32,
    /// Adaptation rates per second: each frame closes 1 - e^(-rate·dt) of the
    /// gap to the metered target (towards bright, towards dark).
    pub speed_up: f32,
    pub speed_down: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomSettings {
    pub enabled: bool,
    /// Energy-conserving blend weight of the blurred image.
    pub intensity: f32,
    /// Upsample filter radius in source texels.
    pub radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowSettings {
    pub enabled: bool,
    /// Camera-relative cascades, 1 to 4.
    pub cascades: u32,
    /// Square resolution of each cascade map.
    pub resolution: u32,
    /// Farthest shadowed view distance; the horizon bounds it further.
    pub max_distance_m: f32,
    /// Depth range of the first cascade beyond the nearest visible ground.
    pub first_cascade_m: f32,
    /// Receiver offset along the normal, in cascade texels.
    pub normal_bias: f32,
    /// Penumbra scale relative to the physical sun disk (0 = hard).
    pub softness: f32,
    /// Caster LOD range factor (1 = receiver detail, 0.5 = one level coarser).
    pub caster_detail: f32,
    /// Analytic body-on-body sun occlusion (eclipses, planet shadow).
    pub eclipses: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AoSettings {
    pub enabled: bool,
    /// World-space sampling radius near the camera.
    pub radius_m: f32,
    /// Radius growth with view distance (fraction of depth).
    pub distance_scale: f32,
    pub intensity: f32,
    pub half_res: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightingSettings {
    /// Multiplier on the authored sun illuminance.
    pub sun_scale: f32,
    /// Multiplier on the authored ambient illuminance.
    pub ambient_scale: f32,
    /// Multiplier on the authored sky light (0 = off).
    pub sky_scale: f32,
    /// Place the sun at a fixed local elevation/azimuth above the nearest
    /// body instead of at the star (terrain review); illuminance unchanged.
    pub studio_sun: bool,
    /// Studio sun elevation above the local horizon.
    pub sun_elevation_deg: f32,
    /// Studio sun azimuth relative to the view direction on the horizon:
    /// 0 = ahead (back light), 90 = from the right, 180 = behind the camera.
    pub sun_azimuth_deg: f32,
}

/// Overall look (art direction 2026-10-10): physical light, or the stylised
/// preset (hazier bright sky, coloured lifted shadows, warm grade, bloom).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LookPreset {
    #[default]
    Physical,
    Stylised,
}

impl LookPreset {
    pub const ALL: [Self; 2] = [Self::Physical, Self::Stylised];
    pub fn name(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Stylised => "stylised",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookSettings {
    pub preset: LookPreset,
    /// Multiplier on atmospheric scattering density (aerial perspective).
    pub haze: f32,
    /// Multiplier on the scattered sky light (0 = no atmosphere drawn).
    pub sky_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlaySettings {
    pub line_width_scale: f32,
    pub opacity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    pub tonemap: Tonemapper,
    /// Blue-noise dither before 8-bit quantisation.
    pub dither: bool,
    pub exposure: ExposureSettings,
    pub bloom: BloomSettings,
    pub shadows: ShadowSettings,
    pub ao: AoSettings,
    pub lighting: LightingSettings,
    pub overlays: OverlaySettings,
    pub anti_aliasing: AntiAliasing,
    pub look: LookSettings,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            tonemap: Tonemapper::AgX,
            dither: true,
            exposure: ExposureSettings {
                mode: ExposureMode::Auto,
                ev100: 14.0,
                compensation: 0.0,
                // Low enough to adapt to starlit night sides.
                min_ev100: -1.0,
                max_ev100: 18.0,
                speed_up: 3.0,
                speed_down: 2.0,
            },
            bloom: BloomSettings {
                enabled: true,
                intensity: 0.04,
                radius: 1.0,
            },
            shadows: ShadowSettings {
                enabled: true,
                cascades: 4,
                resolution: 2048,
                max_distance_m: 300_000.0,
                first_cascade_m: 40.0,
                normal_bias: 1.5,
                softness: 1.0,
                caster_detail: 1.0,
                eclipses: true,
            },
            ao: AoSettings {
                enabled: true,
                radius_m: 4.0,
                distance_scale: 0.02,
                intensity: 1.0,
                half_res: true,
            },
            lighting: LightingSettings {
                sun_scale: 1.0,
                ambient_scale: 1.0,
                sky_scale: 1.0,
                studio_sun: false,
                sun_elevation_deg: 15.0,
                sun_azimuth_deg: 100.0,
            },
            overlays: OverlaySettings {
                line_width_scale: 1.0,
                opacity: 1.0,
            },
            // User choice 2026-10-10 after the side-by-side comparison.
            anti_aliasing: AntiAliasing::Msaa4,
            look: LookSettings {
                preset: LookPreset::Physical,
                haze: 1.0,
                sky_scale: 1.0,
            },
        }
    }
}

/// Shadow map resolutions the pipeline accepts.
pub const SHADOW_RESOLUTIONS: [u32; 3] = [1024, 2048, 4096];
pub const MAX_CASCADES: u32 = 4;

impl RenderSettings {
    /// Hard limits only; editor ranges live in the application registry.
    pub fn validate(&self) -> Result<(), String> {
        let finite = [
            self.exposure.ev100,
            self.exposure.compensation,
            self.exposure.min_ev100,
            self.exposure.max_ev100,
            self.exposure.speed_up,
            self.exposure.speed_down,
            self.bloom.intensity,
            self.bloom.radius,
            self.shadows.max_distance_m,
            self.shadows.first_cascade_m,
            self.shadows.normal_bias,
            self.shadows.softness,
            self.shadows.caster_detail,
            self.ao.radius_m,
            self.ao.distance_scale,
            self.ao.intensity,
            self.lighting.sun_scale,
            self.lighting.ambient_scale,
            self.lighting.sky_scale,
            self.lighting.sun_elevation_deg,
            self.lighting.sun_azimuth_deg,
            self.overlays.line_width_scale,
            self.overlays.opacity,
            self.look.haze,
            self.look.sky_scale,
        ];
        if finite.iter().any(|value| !value.is_finite()) {
            return Err("render settings contain a nonfinite value".into());
        }
        let check = |ok: bool, what: &str| if ok { Ok(()) } else { Err(what.to_owned()) };
        check(
            self.exposure.min_ev100 <= self.exposure.max_ev100,
            "exposure min exceeds max",
        )?;
        check(
            (-10.0..=30.0).contains(&self.exposure.ev100),
            "exposure ev100 outside -10..30",
        )?;
        check(
            self.exposure.speed_up > 0.0 && self.exposure.speed_down > 0.0,
            "exposure speeds must be positive",
        )?;
        check(
            (0.0..=1.0).contains(&self.bloom.intensity) && self.bloom.radius > 0.0,
            "bloom intensity/radius out of range",
        )?;
        check(
            (1..=MAX_CASCADES).contains(&self.shadows.cascades),
            "shadow cascades must be 1..4",
        )?;
        check(
            SHADOW_RESOLUTIONS.contains(&self.shadows.resolution),
            "unsupported shadow resolution",
        )?;
        check(
            self.shadows.max_distance_m > 1.0
                && self.shadows.first_cascade_m > 0.0
                && self.shadows.normal_bias >= 0.0
                && self.shadows.softness >= 0.0
                && (0.05..=4.0).contains(&self.shadows.caster_detail),
            "shadow distances/bias/softness out of range",
        )?;
        check(
            self.ao.radius_m > 0.0 && self.ao.distance_scale >= 0.0 && self.ao.intensity >= 0.0,
            "ambient occlusion parameters out of range",
        )?;
        check(
            self.lighting.sun_scale >= 0.0
                && self.lighting.ambient_scale >= 0.0
                && self.lighting.sky_scale >= 0.0,
            "lighting scales must be non-negative",
        )?;
        check(
            self.overlays.line_width_scale > 0.0 && (0.0..=1.0).contains(&self.overlays.opacity),
            "overlay style out of range",
        )?;
        check(
            (0.0..=20.0).contains(&self.look.haze) && (0.0..=20.0).contains(&self.look.sky_scale),
            "look haze/sky out of range",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_and_limits_reject() {
        RenderSettings::default().validate().unwrap();
        let mut settings = RenderSettings::default();
        settings.shadows.cascades = 5;
        assert!(settings.validate().is_err());
        let mut settings = RenderSettings::default();
        settings.exposure.min_ev100 = 20.0;
        assert!(settings.validate().is_err());
        let mut settings = RenderSettings::default();
        settings.bloom.intensity = f32::NAN;
        assert!(settings.validate().is_err());
    }
}
