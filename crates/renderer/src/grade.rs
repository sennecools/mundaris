//! Colour grade as a 3D LUT on the tonemapped, display-encoded image (look
//! preset, docs/RENDER_PIPELINE_HDR.md amendment 2026-10-10c). The LUT is
//! generated from a few grade parameters; an authored `.cube` can replace
//! the generator later without touching the shader.

use crate::LookPreset;

/// Edge length of the LUT cube.
pub(crate) const LUT_SIZE: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Grade {
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

    /// The grade of a look preset. Stylised follows the art-direction
    /// references: violet-blue lifted shadows, warm highlights, more
    /// saturation, slightly softer contrast.
    pub(crate) fn for_look(preset: LookPreset) -> Self {
        match preset {
            LookPreset::Physical => Self::IDENTITY,
            LookPreset::Stylised => Self {
                lift: [0.045, 0.035, 0.095],
                gain: [1.05, 1.0, 0.95],
                saturation: 1.2,
                contrast: 0.92,
            },
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

    /// RGBA8 texels of the LUT, red fastest, then green, then blue.
    pub(crate) fn lut(&self) -> Vec<u8> {
        let n = LUT_SIZE as usize;
        let mut bytes = Vec::with_capacity(n * n * n * 4);
        let step = 1.0 / (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let out = self.apply([r as f32 * step, g as f32 * step, b as f32 * step]);
                    bytes.extend(out.map(|x| (x * 255.0).round() as u8));
                    bytes.push(255);
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
        assert_eq!(&lut[..4], &[0, 0, 0, 255]);
        assert_eq!(&lut[lut.len() - 4..], &[255, 255, 255, 255]);
        let stylised = Grade::for_look(LookPreset::Stylised);
        let black = stylised.apply([0.0; 3]);
        assert!(black[2] > black[0] && black[0] > 0.0, "{black:?}");
        let white = stylised.apply([1.0; 3]);
        assert!(white[0] > white[2], "warm highlights {white:?}");
    }
}
