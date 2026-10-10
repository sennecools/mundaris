//! Colour grade as a 3D LUT on the tonemapped, display-encoded image (look
//! preset, docs/RENDER_PIPELINE_HDR.md amendment 2026-10-10c). The LUT is
//! generated from a few grade parameters; an authored `.cube` can replace
//! the generator later without touching the shader.

use crate::LookPreset;

/// Per-planet parameters of the stylised look (art direction 2026-10-10: each
/// planet's sky and grade follow its own atmosphere and star). Authored in
/// `lighting.json` `bodies.<id>.look`; `Default` is the generic stylised look.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StylisedLook {
    /// Multiplier on the atmosphere's scattered light colour.
    pub sky_tint: [f32; 3],
    /// Saturation of the scattered sky light (1 = physical).
    pub sky_saturation: f32,
    /// Multiplier on atmospheric density (aerial perspective).
    pub haze: f32,
    /// Multiplier on the scattered sky light.
    pub sky: f32,
    /// Multiplier on the shading sky light (coloured shadows).
    pub shadow_sky: f32,
    /// Multiplier on bloom intensity.
    pub bloom: f32,
    pub grade: Grade,
}

impl Default for StylisedLook {
    fn default() -> Self {
        Self {
            sky_tint: [0.8, 1.05, 1.2],
            sky_saturation: 1.6,
            haze: 1.6,
            sky: 1.7,
            shadow_sky: 3.0,
            bloom: 2.5,
            grade: Grade {
                lift: [0.02, 0.016, 0.05],
                gain: [1.05, 1.0, 0.95],
                saturation: 1.45,
                contrast: 1.0,
            },
        }
    }
}

impl StylisedLook {
    pub fn validate(&self) -> bool {
        let g = &self.grade;
        self.sky_tint
            .iter()
            .chain(&g.lift)
            .chain(&g.gain)
            .chain(&[
                self.sky_saturation,
                self.haze,
                self.sky,
                self.shadow_sky,
                self.bloom,
                g.saturation,
                g.contrast,
            ])
            .all(|v| v.is_finite() && (0.0..=20.0).contains(v))
    }
}

/// Edge length of the LUT cube.
pub(crate) const LUT_SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Grade {
    /// Added to shadows, fading out toward white (coloured, lifted shadows).
    pub lift: [f32; 3],
    /// Per-channel gain (warmth).
    pub gain: [f32; 3],
    /// Luma-preserving saturation (1 = unchanged).
    pub saturation: f32,
    /// Contrast around mid grey (1 = unchanged).
    pub contrast: f32,
}

impl Grade {
    pub(crate) const IDENTITY: Self = Self {
        lift: [0.0; 3],
        gain: [1.0; 3],
        saturation: 1.0,
        contrast: 1.0,
    };

    /// The grade in effect: identity for the physical look, else the
    /// planet's stylised grade (violet-blue lifted shadows, warm highlights,
    /// vivid saturated colour; lift kept small so shadows stay coloured).
    pub(crate) fn for_look(preset: LookPreset, look: &StylisedLook) -> Self {
        match preset {
            LookPreset::Physical => Self::IDENTITY,
            LookPreset::Stylised => look.grade,
        }
    }

    /// Grades one display-encoded colour (0..1 per channel).
    pub(crate) fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        let v: [f32; 3] = std::array::from_fn(|i| {
            let lifted = (c[i] + self.lift[i] * (1.0 - c[i])) * self.gain[i];
            0.5 + (lifted - 0.5) * self.contrast
        });
        let luma = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
        v.map(|x| (luma + (x - luma) * self.saturation).clamp(0.0, 1.0))
    }

    /// `Rgb10a2Unorm` texels of the LUT (10 bits per channel, so the LUT
    /// adds no 8-bit steps of its own), red fastest, then green, then blue.
    pub(crate) fn lut(&self) -> Vec<u8> {
        let n = LUT_SIZE as usize;
        let mut bytes = Vec::with_capacity(n * n * n * 4);
        let step = 1.0 / (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let out = self.apply([r as f32 * step, g as f32 * step, b as f32 * step]);
                    let [r10, g10, b10] = out.map(|x| (x * 1023.0).round() as u32);
                    let texel = r10 | (g10 << 10) | (b10 << 20) | (3 << 30);
                    bytes.extend(texel.to_le_bytes());
                }
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_grade_is_identity_and_stylised_lifts_shadows() {
        let lut = Grade::IDENTITY.lut();
        assert_eq!(lut.len(), (LUT_SIZE * LUT_SIZE * LUT_SIZE * 4) as usize);
        assert_eq!(u32::from_le_bytes(lut[..4].try_into().unwrap()), 3 << 30);
        assert_eq!(
            u32::from_le_bytes(lut[lut.len() - 4..].try_into().unwrap()),
            u32::MAX
        );
        let stylised = Grade::for_look(LookPreset::Stylised, &StylisedLook::default());
        let black = stylised.apply([0.0; 3]);
        assert!(black[2] > black[0] && black[0] > 0.0, "{black:?}");
        let white = stylised.apply([1.0; 3]);
        assert!(white[0] > white[2], "warm highlights {white:?}");
    }
}
