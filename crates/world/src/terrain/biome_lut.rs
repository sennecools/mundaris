//! Biome LUT (`docs/ASTRUM_TERRAIN_PIPELINE.md` §10.3): a small sRGB image
//! indexed by temperature (x) × moisture (y) that gives the surface tint of a
//! world-map body. Texel centres span the declared axis ranges; lookups clamp
//! and blend bilinearly in linear colour. The GPU producer mirrors `sample`.
//!
//! A LUT is described by RON metadata (axes, image path and an optional
//! generator recipe). The recipe makes the checked-in PNG reproducible until it
//! is painted by hand.
use super::TerrainError;
use serde::Deserialize;
use std::sync::Arc;

/// One Whittaker-style anchor of a generator recipe.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LutAnchor {
    pub name: String,
    pub temperature_c: f64,
    pub moisture: f64,
    pub srgb: (f64, f64, f64),
}

/// Deterministic generator input: anchors blended with Gaussian weights in
/// linear colour.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LutRecipe {
    pub size: u32,
    pub sigma_temperature_c: f64,
    pub sigma_moisture: f64,
    pub anchors: Vec<LutAnchor>,
}

/// LUT metadata file (`content/lut/*.ron`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LutMetadata {
    pub schema: u32,
    /// PNG path relative to the content root.
    pub image: String,
    pub temperature_c: (f64, f64),
    pub moisture: (f64, f64),
    pub recipe: Option<LutRecipe>,
}

impl LutMetadata {
    pub fn validate(&self) -> Result<(), TerrainError> {
        let axis = |(lo, hi): (f64, f64)| lo.is_finite() && hi.is_finite() && hi > lo;
        let recipe_ok = self.recipe.as_ref().is_none_or(|r| {
            (2..=512).contains(&r.size)
                && r.sigma_temperature_c > 0.0
                && r.sigma_moisture > 0.0
                && !r.anchors.is_empty()
                && r.anchors.iter().all(|a| {
                    a.temperature_c.is_finite()
                        && a.moisture.is_finite()
                        && [a.srgb.0, a.srgb.1, a.srgb.2]
                            .iter()
                            .all(|c| (0.0..=1.0).contains(c))
                })
        });
        if self.schema == 1
            && !self.image.is_empty()
            && axis(self.temperature_c)
            && axis(self.moisture)
            && recipe_ok
        {
            Ok(())
        } else {
            Err(TerrainError::InvalidConfig)
        }
    }

    /// Texels of the recipe, row-major (y = moisture), sRGB 8-bit.
    pub fn generate(&self) -> Result<Vec<[u8; 3]>, TerrainError> {
        self.validate()?;
        let recipe = self.recipe.as_ref().ok_or(TerrainError::InvalidConfig)?;
        let n = recipe.size;
        let anchors: Vec<_> = recipe
            .anchors
            .iter()
            .map(|a| {
                let c = [a.srgb.0, a.srgb.1, a.srgb.2].map(srgb_to_linear);
                (a.temperature_c, a.moisture, c)
            })
            .collect();
        let mut texels = Vec::with_capacity((n * n) as usize);
        for j in 0..n {
            let m = axis_value(self.moisture, j, n);
            for i in 0..n {
                let t = axis_value(self.temperature_c, i, n);
                let mut sum = [0.0f64; 3];
                let mut total = 0.0f64;
                for &(at, am, colour) in &anchors {
                    let dt = (t - at) / recipe.sigma_temperature_c;
                    let dm = (m - am) / recipe.sigma_moisture;
                    let w = (-0.5 * (dt * dt + dm * dm)).exp();
                    total += w;
                    for (s, c) in sum.iter_mut().zip(colour) {
                        *s += w * c;
                    }
                }
                texels.push(sum.map(|s| encode_srgb8(s / total)));
            }
        }
        Ok(texels)
    }
}

fn axis_value((lo, hi): (f64, f64), index: u32, size: u32) -> f64 {
    lo + (hi - lo) * f64::from(index) / f64::from(size - 1)
}

pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn linear_to_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn encode_srgb8(linear: f64) -> u8 {
    (linear_to_srgb(linear) * 255.0).round() as u8
}

/// A loaded LUT: axes plus square sRGB texels.
#[derive(Debug, Clone, PartialEq)]
pub struct BiomeLut {
    pub size: u32,
    pub temperature_c: (f64, f64),
    pub moisture: (f64, f64),
    /// Row-major, y = moisture.
    pub srgb: Arc<[[u8; 3]]>,
}

impl BiomeLut {
    pub fn new(
        metadata: &LutMetadata,
        size: u32,
        srgb: Vec<[u8; 3]>,
    ) -> Result<Self, TerrainError> {
        metadata.validate()?;
        if !(2..=512).contains(&size) || srgb.len() != (size * size) as usize {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            size,
            temperature_c: metadata.temperature_c,
            moisture: metadata.moisture,
            srgb: srgb.into(),
        })
    }

    /// Decode a square 8-bit RGB or RGBA PNG (alpha ignored).
    pub fn from_png(metadata: &LutMetadata, bytes: &[u8]) -> Result<Self, TerrainError> {
        let (size, srgb) = decode_png(bytes).ok_or(TerrainError::InvalidConfig)?;
        Self::new(metadata, size, srgb)
    }

    /// Stable content hash (part of terrain identity).
    pub fn identity(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ u64::from(self.size);
        for value in [
            self.temperature_c.0,
            self.temperature_c.1,
            self.moisture.0,
            self.moisture.1,
        ] {
            hash = (hash ^ value.to_bits()).wrapping_mul(0x100_0000_01b3);
        }
        for texel in self.srgb.iter() {
            for &c in texel {
                hash = (hash ^ u64::from(c)).wrapping_mul(0x100_0000_01b3);
            }
        }
        hash
    }

    /// Linear colour at (temperature, moisture): clamped bilinear over
    /// linear-decoded texels.
    pub fn sample(&self, temperature_c: f64, moisture: f64) -> [f64; 3] {
        let n = self.size;
        let coordinate = |v: f64, (lo, hi): (f64, f64)| {
            ((v - lo) / (hi - lo) * f64::from(n - 1)).clamp(0.0, f64::from(n - 1))
        };
        let x = coordinate(temperature_c, self.temperature_c);
        let y = coordinate(moisture, self.moisture);
        let (x0, y0) = ((x.floor() as u32).min(n - 2), (y.floor() as u32).min(n - 2));
        let (fx, fy) = (x - f64::from(x0), y - f64::from(y0));
        let texel = |i: u32, j: u32| {
            self.srgb[(j * n + i) as usize].map(|c| srgb_to_linear(f64::from(c) / 255.0))
        };
        let (a, b, c, d) = (
            texel(x0, y0),
            texel(x0 + 1, y0),
            texel(x0, y0 + 1),
            texel(x0 + 1, y0 + 1),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }
}

fn decode_png(bytes: &[u8]) -> Option<(u32, Vec<[u8; 3]>)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    if info.width != info.height || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return None,
    };
    let mut texels = Vec::with_capacity((info.width * info.height) as usize);
    for y in 0..info.height as usize {
        let row = &buffer[y * info.line_size..][..info.width as usize * channels];
        texels.extend(row.chunks_exact(channels).map(|p| [p[0], p[1], p[2]]));
    }
    Some((info.width, texels))
}

/// Encode square sRGB texels as an 8-bit RGB PNG.
pub fn encode_png(size: u32, srgb: &[[u8; 3]]) -> Result<Vec<u8>, TerrainError> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| TerrainError::InvalidConfig)?;
        let data: Vec<u8> = srgb.iter().flatten().copied().collect();
        writer
            .write_image_data(&data)
            .map_err(|_| TerrainError::InvalidConfig)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn content() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content")
    }

    pub(crate) fn terra_lut() -> BiomeLut {
        let root = content();
        let metadata: LutMetadata =
            ron::from_str(&std::fs::read_to_string(root.join("lut/terra_whittaker.ron")).unwrap())
                .unwrap();
        let bytes = std::fs::read(root.join(&metadata.image)).unwrap();
        BiomeLut::from_png(&metadata, &bytes).unwrap()
    }

    #[test]
    fn checked_in_lut_matches_its_recipe_and_anchors() {
        let root = content();
        let metadata: LutMetadata =
            ron::from_str(&std::fs::read_to_string(root.join("lut/terra_whittaker.ron")).unwrap())
                .unwrap();
        let lut = terra_lut();
        let Some(recipe) = metadata.recipe.as_ref() else {
            return; // painted by hand: nothing to reproduce
        };
        let generated = metadata.generate().unwrap();
        assert_eq!(lut.size, recipe.size);
        // Within one 8-bit step, allowing for platform `exp`/`powf` rounding.
        let worst = generated
            .iter()
            .zip(lut.srgb.iter())
            .flat_map(|(a, b)| (0..3).map(move |k| a[k].abs_diff(b[k])))
            .max()
            .unwrap();
        assert!(
            worst <= 1,
            "PNG differs from recipe by {worst}; regenerate it"
        );
        // Each anchor's own colour dominates at its position (weights blend
        // neighbours, so allow a broad tolerance in linear colour).
        for anchor in &recipe.anchors {
            let at = lut.sample(anchor.temperature_c, anchor.moisture);
            let want = [anchor.srgb.0, anchor.srgb.1, anchor.srgb.2].map(srgb_to_linear);
            for k in 0..3 {
                assert!(
                    (at[k] - want[k]).abs() < 0.08,
                    "{}: {at:?} vs {want:?}",
                    anchor.name
                );
            }
        }
    }

    #[test]
    fn sampling_clamps_and_interpolates_in_linear_colour() {
        let metadata = LutMetadata {
            schema: 1,
            image: "x.png".into(),
            temperature_c: (0.0, 10.0),
            moisture: (0.0, 1.0),
            recipe: None,
        };
        let lut = BiomeLut::new(
            &metadata,
            2,
            vec![[0, 0, 0], [255, 255, 255], [0, 0, 0], [255, 255, 255]],
        )
        .unwrap();
        assert_eq!(lut.sample(-50.0, 0.5), [0.0; 3]);
        assert_eq!(lut.sample(50.0, 0.5), [1.0; 3]);
        assert!((lut.sample(5.0, 0.3)[0] - 0.5).abs() < 1e-12);
        let png = encode_png(2, &lut.srgb).unwrap();
        assert_eq!(BiomeLut::from_png(&metadata, &png).unwrap(), lut);
        assert!(srgb_to_linear(linear_to_srgb(0.214)) - 0.214 < 1e-12);
    }
}
