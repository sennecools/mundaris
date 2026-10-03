//! Small, renderer-owned lighting controls for inspecting planetary surfaces.

use glam::DVec3;

use crate::RenderPreparationError;

/// Surface visualization selected by the terrain renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum TerrainRenderMode {
    Elevation = 0,
    Lit = 1,
    Normals = 2,
    Diffuse = 3,
    Readability = 4,
    Slope = 5,
    SeaMask = 6,
    RockWeight = 7,
}

impl TerrainRenderMode {
    pub const ALL: [Self; 8] = [
        Self::Elevation,
        Self::Lit,
        Self::Normals,
        Self::Diffuse,
        Self::Readability,
        Self::Slope,
        Self::SeaMask,
        Self::RockWeight,
    ];
}

/// Renderer-only thresholds supplied by the application; values are metres and degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainReadability {
    sea_level_m: f64,
    highland_start_m: f64,
    highland_full_m: f64,
    pale_start_m: f64,
    pale_full_m: f64,
    rock_start_degrees: f64,
    rock_full_degrees: f64,
}

impl TerrainReadability {
    pub fn try_new(
        sea_level_m: f64,
        highland_start_m: f64,
        highland_full_m: f64,
        pale_start_m: f64,
        pale_full_m: f64,
        rock_start_degrees: f64,
        rock_full_degrees: f64,
    ) -> Result<Self, RenderPreparationError> {
        let value = Self {
            sea_level_m,
            highland_start_m,
            highland_full_m,
            pale_start_m,
            pale_full_m,
            rock_start_degrees,
            rock_full_degrees,
        };
        if ![
            sea_level_m,
            highland_start_m,
            highland_full_m,
            pale_start_m,
            pale_full_m,
            rock_start_degrees,
            rock_full_degrees,
        ]
        .iter()
        .all(|v| v.is_finite())
            || highland_start_m <= sea_level_m
            || highland_full_m <= highland_start_m
            || pale_start_m <= highland_full_m
            || pale_full_m <= pale_start_m
            || rock_start_degrees < 0.0
            || rock_full_degrees <= rock_start_degrees
            || rock_full_degrees > 90.0
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(value)
    }
}

/// Deterministic body-fixed directions useful for lighting inspection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainSunPreset {
    Overhead,
    Side,
    Grazing,
    Terminator,
    Night,
}

impl TerrainSunPreset {
    pub const ALL: [Self; 5] = [
        Self::Overhead,
        Self::Side,
        Self::Grazing,
        Self::Terminator,
        Self::Night,
    ];

    pub fn direction_body(self) -> DVec3 {
        match self {
            Self::Overhead => DVec3::Z,
            Self::Side => (DVec3::X + DVec3::Z).normalize(),
            Self::Grazing => (DVec3::X + DVec3::Z * 0.1).normalize(),
            Self::Terminator => DVec3::X,
            Self::Night => -DVec3::Z,
        }
    }
}

/// Renderer-only lighting for a frame, shared by its terrain bodies in each body's
/// fixed axes. The vector points from surface to sun, never relative to the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainLighting {
    sun_body: DVec3,
    ambient: f32,
    diffuse: f32,
    mode: TerrainRenderMode,
    readability: Option<TerrainReadability>,
}

impl Default for TerrainLighting {
    fn default() -> Self {
        Self {
            sun_body: DVec3::new(0.8, 0.3, 0.25).normalize(),
            ambient: 0.06,
            diffuse: 0.94,
            mode: TerrainRenderMode::Lit,
            readability: None,
        }
    }
}

impl TerrainLighting {
    /// Constructs lighting with a finite unit direction and bounded strengths.
    /// Ambient and diffuse are constrained to [0, 1] with sum at most one, so
    /// their combined contribution cannot overexpose the surface.
    pub fn try_new(
        sun_body: DVec3,
        ambient: f32,
        diffuse: f32,
        mode: TerrainRenderMode,
    ) -> Result<Self, RenderPreparationError> {
        let scale = sun_body.abs().max_element();
        if !sun_body.is_finite() || !scale.is_finite() || scale == 0.0 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let scaled = sun_body / scale;
        let length = scaled.length();
        if !length.is_finite()
            || length == 0.0
            || !ambient.is_finite()
            || !diffuse.is_finite()
            || !(0.0..=1.0).contains(&ambient)
            || !(0.0..=1.0).contains(&diffuse)
            || ambient + diffuse > 1.0
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(Self {
            sun_body: scaled / length,
            ambient,
            diffuse,
            mode,
            readability: None,
        })
    }

    pub fn sun_direction_body(self) -> DVec3 {
        self.sun_body
    }

    pub fn ambient_strength(self) -> f32 {
        self.ambient
    }

    pub fn diffuse_strength(self) -> f32 {
        self.diffuse
    }

    pub fn mode(self) -> TerrainRenderMode {
        self.mode
    }

    pub fn with_mode(self, mode: TerrainRenderMode) -> Self {
        Self { mode, ..self }
    }

    pub fn with_readability(mut self, config: TerrainReadability) -> Self {
        self.readability = Some(config);
        self
    }

    /// Packs the 64-byte uniform: lighting controls followed by readability thresholds.
    pub(crate) fn packed(self, target_srgb: bool) -> [f32; 16] {
        let config = self.readability.unwrap_or(TerrainReadability {
            sea_level_m: 0.0,
            highland_start_m: 300.0,
            highland_full_m: 1800.0,
            pale_start_m: 3000.0,
            pale_full_m: 5000.0,
            rock_start_degrees: 12.0,
            rock_full_degrees: 35.0,
        });
        [
            self.sun_body.x as f32,
            self.sun_body.y as f32,
            self.sun_body.z as f32,
            self.ambient,
            self.diffuse,
            self.mode as u32 as f32,
            f32::from(target_srgb),
            0.0,
            config.sea_level_m as f32,
            config.highland_start_m as f32,
            config.highland_full_m as f32,
            config.pale_start_m as f32,
            config.pale_full_m as f32,
            config.rock_start_degrees.to_radians() as f32,
            config.rock_full_degrees.to_radians() as f32,
            0.0,
        ]
    }
}

/// Cyclic diagnostic colors with twelve distinguishable hues.
pub fn lod_color(level: u8) -> [f32; 4] {
    let sector = f32::from(level % 12) * 0.5;
    let x = 1.0 - (sector % 2.0 - 1.0).abs();
    let (r, g, b) = match sector as u8 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    [0.2 + r * 0.8, 0.2 + g * 0.8, 0.2 + b * 0.8, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_presets_are_normalized_and_deterministic() {
        let default = TerrainLighting::default();
        assert_eq!(default.mode(), TerrainRenderMode::Lit);
        assert_eq!(default.ambient_strength(), 0.06);
        assert_eq!(default.diffuse_strength(), 0.94);
        assert!((default.sun_direction_body().length() - 1.0).abs() < 1e-15);
        for preset in TerrainSunPreset::ALL {
            let direction = preset.direction_body();
            assert!((direction.length() - 1.0).abs() < 1e-15);
            assert_eq!(direction, preset.direction_body());
        }
        assert_eq!(TerrainSunPreset::Overhead.direction_body(), DVec3::Z);
        assert_eq!(TerrainSunPreset::Night.direction_body(), -DVec3::Z);
    }

    #[test]
    fn constructor_handles_extreme_directions_and_rejects_invalid_input() {
        for direction in [DVec3::splat(f64::MAX), DVec3::splat(f64::MIN_POSITIVE)] {
            let light =
                TerrainLighting::try_new(direction, 0.1, 0.9, TerrainRenderMode::Lit).unwrap();
            assert!((light.sun_direction_body().length() - 1.0).abs() < 1e-15);
        }
        for direction in [DVec3::ZERO, DVec3::new(f64::NAN, 0.0, 0.0)] {
            assert!(TerrainLighting::try_new(direction, 0.1, 0.9, TerrainRenderMode::Lit).is_err());
        }
        for (ambient, diffuse) in [
            (f32::NAN, 0.0),
            (0.0, f32::INFINITY),
            (-0.1, 0.1),
            (0.1, 1.1),
            (0.6, 0.5),
        ] {
            assert!(
                TerrainLighting::try_new(DVec3::Z, ambient, diffuse, TerrainRenderMode::Lit)
                    .is_err()
            );
        }
    }
}
