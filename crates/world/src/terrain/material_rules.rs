//! Surface material rules (pipeline §10.1, §10.4; milestone M4 Surface).
//!
//! PROTOTYPE: pure f32 functions that turn per-sample terrain facts (height,
//! slope, climate, Tier A hardness / sediment / flow) into smooth weights over
//! a small fixed material set. No hard material IDs (§2 P4): every output is a
//! continuous weight and the six weights always sum to one. The WGSL mirror is
//! `crates/renderer/src/shaders/material_rules.wgsl`; keep them in step.
//!
//! The ground colours come from the archetype's `GroundPalette` (physical
//! or stylised, M4); constants marked `PROTOTYPE:` are still hardcoded.

use std::f32::consts::PI;

/// Degrees to radians.
const DEG: f32 = PI / 180.0;

/// Per-sample inputs. Slopes are angles from horizontal in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialInput {
    /// Height above sea level, metres.
    pub height_m: f32,
    /// Local slope angle, radians.
    pub slope: f32,
    /// Slope a few texels uphill, radians (talus source). Pass 0 when unknown;
    /// scree is then off.
    pub uphill_slope: f32,
    pub temperature_c: f32,
    /// 0..1.
    pub moisture: f32,
    /// Tier A rock hardness, 0..1.
    pub hardness: f32,
    /// Tier A sediment, 0..1.
    pub sediment: f32,
    /// Tier A flow accumulation, 0..1.
    pub flow: f32,
}

/// Weights over the material set; they sum to one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MaterialWeights {
    pub bedrock: f32,
    pub scree: f32,
    pub soil: f32,
    pub sand: f32,
    pub snow: f32,
    pub wet_sediment: f32,
}

impl MaterialWeights {
    pub const COUNT: usize = 6;

    pub fn as_array(&self) -> [f32; Self::COUNT] {
        [
            self.bedrock,
            self.scree,
            self.soil,
            self.sand,
            self.snow,
            self.wet_sediment,
        ]
    }

    pub fn sum(&self) -> f32 {
        self.as_array().iter().sum()
    }
}

/// Mean linear colour per material, same order as [`MaterialWeights::as_array`].
/// PROTOTYPE: placeholders until the material definitions declare their means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialPalette {
    pub mean: [[f32; 3]; MaterialWeights::COUNT],
}

impl Default for MaterialPalette {
    fn default() -> Self {
        Self {
            mean: [
                [0.18, 0.16, 0.14], // bedrock
                [0.26, 0.23, 0.20], // scree
                [0.12, 0.10, 0.07], // soil (replaced by the biome LUT tint)
                [0.45, 0.38, 0.26], // sand
                [0.85, 0.87, 0.90], // snow
                [0.09, 0.08, 0.06], // wet sediment
            ],
        }
    }
}

pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// PROTOTYPE: rule constants.
const ROCK_SLOPE: (f32, f32) = (32.0 * DEG, 52.0 * DEG);
const SOIL_SLOPE_LOSS: (f32, f32) = (15.0 * DEG, 45.0 * DEG);
const SNOW_T_C: f32 = 0.0;
const SNOW_BLEND_C: f32 = 3.0;
const SNOW_SLOPE: (f32, f32) = (35.0 * DEG, 55.0 * DEG);
const BEACH_H_M: f32 = 3.0;
const SCREE_UPHILL: (f32, f32) = (30.0 * DEG, 45.0 * DEG);
const SCREE_HERE: (f32, f32) = (18.0 * DEG, 32.0 * DEG);

/// Soil depth proxy, 0..~1.2 (§10.1): wetter ground holds more, steep and hard
/// ground holds less, sediment adds.
pub fn soil_depth(input: &MaterialInput) -> f32 {
    let base = (0.4 + 0.6 * smoothstep(0.0, 0.5, input.moisture)) * (1.0 - 0.5 * input.hardness);
    let slope_factor = smoothstep(SOIL_SLOPE_LOSS.0, SOIL_SLOPE_LOSS.1, input.slope);
    base * (1.0 - slope_factor) + 0.5 * input.sediment
}

/// Material weights for one sample. Layers compose top-down (snow, wet
/// sediment, sand, scree) and the remainder splits between bedrock and soil,
/// so the weights sum to one by construction and stay continuous.
pub fn material_weights(input: &MaterialInput) -> MaterialWeights {
    let slope = input.slope;

    let cold = 1.0
        - smoothstep(
            SNOW_T_C - SNOW_BLEND_C,
            SNOW_T_C + SNOW_BLEND_C,
            input.temperature_c,
        );
    let snow = cold * (1.0 - smoothstep(SNOW_SLOPE.0, SNOW_SLOPE.1, slope));

    let flat = 1.0 - smoothstep(3.0 * DEG, 9.0 * DEG, slope);
    let wet_raw = smoothstep(0.35, 0.7, input.flow) * flat * smoothstep(0.1, 0.4, input.moisture);

    let gentle = 1.0 - smoothstep(6.0 * DEG, 14.0 * DEG, slope);
    let beach = 1.0 - smoothstep(0.5 * BEACH_H_M, BEACH_H_M, input.height_m);
    let arid = (1.0 - smoothstep(0.15, 0.4, input.moisture)) * smoothstep(0.3, 0.6, input.sediment);
    let sand_raw = beach.max(arid) * gentle;

    let scree_raw = smoothstep(SCREE_UPHILL.0, SCREE_UPHILL.1, input.uphill_slope)
        * (1.0 - smoothstep(SCREE_HERE.0, SCREE_HERE.1, slope))
        * smoothstep(5.0 * DEG, 12.0 * DEG, slope);

    let rock = (1.0 - smoothstep(0.1, 0.35, soil_depth(input))).max(smoothstep(
        ROCK_SLOPE.0,
        ROCK_SLOPE.1,
        slope,
    ));

    let mut rem = 1.0 - snow;
    let wet_sediment = rem * wet_raw;
    rem -= wet_sediment;
    let sand = rem * sand_raw;
    rem -= sand;
    let scree = rem * scree_raw;
    rem -= scree;
    MaterialWeights {
        bedrock: rem * rock,
        scree,
        soil: rem * (1.0 - rock),
        sand,
        snow,
        wet_sediment,
    }
}

/// Weighted mean colour (linear RGB) of a weight set.
pub fn mean_colour(weights: &MaterialWeights, palette: &MaterialPalette) -> [f32; 3] {
    let w = weights.as_array();
    let mut out = [0.0; 3];
    for (wi, c) in w.iter().zip(palette.mean.iter()) {
        for k in 0..3 {
            out[k] += wi * c[k];
        }
    }
    out
}

/// Mean-matching rule (§10.2): `detail / detail_mean * target`. Averaged over
/// the detail texture this returns `target`, so orbit and ground agree.
pub fn tint_detail(detail: [f32; 3], detail_mean: [f32; 3], target: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] = detail[k] / detail_mean[k].max(1.0e-4) * target[k];
    }
    out
}

/// Integer hash to 0..1 (PCG output function, mirrors the WGSL).
pub fn hash_unit(n: u32) -> f32 {
    let state = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    let word = ((state >> ((state >> 28) + 4)) ^ state).wrapping_mul(277_803_737);
    let h = (word >> 22) ^ word;
    (h >> 8) as f32 / 16_777_216.0
}

/// Strata sample: band tone in -1..1 and the band index phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strata {
    /// Layer tone, -1 (dark / soft) to 1 (light / hard). Continuous.
    pub tone: f32,
    /// Continuous layer coordinate (integer part is the layer number).
    pub phase: f32,
}

/// Layered rock bands by height (§10.4). `warp` is a low-frequency noise value
/// in about -1..1 supplied by the caller; it tilts and wobbles the layers.
/// `period_m` is the mean layer thickness. PROTOTYPE: edge softness is fixed.
pub fn strata_band(height_m: f32, warp: f32, period_m: f32) -> Strata {
    let phase = height_m / period_m + 1.5 * warp;
    let layer = phase.floor();
    let f = phase - layer;
    let i = layer as i32 as u32;
    let a = hash_unit(i) * 2.0 - 1.0;
    let b = hash_unit(i.wrapping_add(1)) * 2.0 - 1.0;
    Strata {
        tone: a + (b - a) * smoothstep(0.8, 1.0, f),
        phase,
    }
}

/// How visible strata are: only on cliffs, more on hard layered rock.
pub fn strata_visibility(slope: f32, hardness: f32) -> f32 {
    smoothstep(35.0 * DEG, 60.0 * DEG, slope) * (0.4 + 0.6 * hardness)
}

/// Multiplier on the rock colour from strata, around 1 (mean preserving:
/// tone is zero-mean over layers).
pub fn strata_colour_factor(strata: &Strata, visibility: f32) -> f32 {
    1.0 + 0.35 * strata.tone * visibility
}

/// PROTOTYPE: mean strata layer thickness (m).
pub const STRATA_PERIOD_M: f32 = 40.0;

/// Ground palette of the material set (M4 Surface): linear mean colours of
/// rock and the loose materials, the strata contrast on rock faces and the
/// style pass on the biome soil tint. `PHYSICAL` is the measured-looking
/// set; archetypes blend toward a stylised one (art direction 2026-10-10).
/// Packed by `words` for `terrain_atlas_produce.wgsl` (`world_surface`
/// header words 24..48).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundPalette {
    pub rock: [f32; 3],
    pub scree: [f32; 3],
    pub sand: [f32; 3],
    pub wet_sediment: [f32; 3],
    /// Strata contrast on rock faces (1 = the physical bands).
    pub strata: f32,
    /// Contrast of the draw-time ground detail (1 = physical; the
    /// stylised look is smoother, without photoreal texture noise).
    pub detail: f32,
    pub soil: SoilStyle,
}

/// Style pass on the biome soil tint in OKLCh, blended in by `amount`:
/// chroma scaled and capped, lightness pulled toward `lightness_mid` by
/// `soften` (softer contrast), hue pulled by `hue_pull` toward the nearest
/// of the first `anchors` hue anchors, then fitted into sRGB by lowering
/// chroma (keeps the hue instead of clipping a channel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoilStyle {
    pub amount: f32,
    pub chroma_gain: f32,
    pub chroma_max: f32,
    pub soften: f32,
    pub lightness_mid: f32,
    pub hue_pull: f32,
    pub anchors: u32,
    pub hue_anchors_deg: [f32; 3],
}

impl GroundPalette {
    /// The M4 prototype look before stylisation.
    pub const PHYSICAL: Self = Self {
        rock: [0.17, 0.15, 0.13],
        scree: [0.24, 0.22, 0.2],
        sand: [0.42, 0.35, 0.24],
        wet_sediment: [0.09, 0.085, 0.065],
        strata: 1.0,
        detail: 1.0,
        soil: SoilStyle {
            amount: 0.0,
            chroma_gain: 1.0,
            chroma_max: 1.0,
            soften: 0.0,
            lightness_mid: 0.5,
            hue_pull: 0.0,
            anchors: 0,
            hue_anchors_deg: [0.0; 3],
        },
    };

    /// `self` blended toward `stylised` by `style` (0..1): colours in
    /// linear RGB, the soil pass by its amount.
    pub fn blend(&self, stylised: &Self, style: f32) -> Self {
        let s = style.clamp(0.0, 1.0);
        let mix3 = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|k| a[k] + (b[k] - a[k]) * s);
        Self {
            rock: mix3(self.rock, stylised.rock),
            scree: mix3(self.scree, stylised.scree),
            sand: mix3(self.sand, stylised.sand),
            wet_sediment: mix3(self.wet_sediment, stylised.wet_sediment),
            strata: self.strata + (stylised.strata - self.strata) * s,
            detail: self.detail + (stylised.detail - self.detail) * s,
            soil: SoilStyle {
                amount: stylised.soil.amount * s,
                ..stylised.soil
            },
        }
    }

    /// GPU words (f32 bits), `world_surface` header words 24..48.
    pub fn words(&self) -> [f32; 24] {
        let s = &self.soil;
        let [r, c, n, w] = [self.rock, self.scree, self.sand, self.wet_sediment];
        [
            r[0],
            r[1],
            r[2],
            c[0],
            c[1],
            c[2],
            n[0],
            n[1],
            n[2],
            w[0],
            w[1],
            w[2],
            self.strata,
            s.amount,
            s.chroma_gain,
            s.chroma_max,
            s.soften,
            s.lightness_mid,
            s.hue_pull,
            s.anchors as f32,
            s.hue_anchors_deg[0],
            s.hue_anchors_deg[1],
            s.hue_anchors_deg[2],
            self.detail,
        ]
    }
}

/// Cube root as the GPU computes it (`pow(max(x, 0), 1/3)`).
fn cbrt(x: f32) -> f32 {
    x.max(0.0).powf(1.0 / 3.0)
}

/// Linear sRGB to OKLab (Ottosson).
pub fn linear_to_oklab(c: [f32; 3]) -> [f32; 3] {
    let l = cbrt(0.412_221_46 * c[0] + 0.536_332_55 * c[1] + 0.051_445_99 * c[2]);
    let m = cbrt(0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2]);
    let s = cbrt(0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2]);
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab to linear sRGB (Ottosson).
pub fn oklab_to_linear(c: [f32; 3]) -> [f32; 3] {
    let l = c[0] + 0.396_337_78 * c[1] + 0.215_803_76 * c[2];
    let m = c[0] - 0.105_561_346 * c[1] - 0.063_854_17 * c[2];
    let s = c[0] - 0.089_484_18 * c[1] - 1.291_485_5 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn in_gamut(c: [f32; 3]) -> bool {
    c.iter().all(|v| (0.0..=1.0).contains(v))
}

/// The soil style pass (`SoilStyle`) on a linear colour.
pub fn style_soil(colour: [f32; 3], s: &SoilStyle) -> [f32; 3] {
    if s.amount <= 0.0 {
        return colour;
    }
    let [l, a, b] = linear_to_oklab(colour);
    let mut hue = b.atan2(a);
    let chroma = (a.hypot(b) * s.chroma_gain).min(s.chroma_max);
    // Signed angle difference in -π..π (floor form, as the GPU).
    let wrap = |d: f32| {
        let x = d + 3.0 * PI;
        x - 2.0 * PI * (x / (2.0 * PI)).floor() - PI
    };
    let mut pull = 0.0f32;
    let mut nearest = 4.0f32;
    for k in 0..(s.anchors.min(3) as usize) {
        let d = wrap(s.hue_anchors_deg[k] * DEG - hue);
        if d.abs() < nearest {
            nearest = d.abs();
            pull = d;
        }
    }
    hue += s.hue_pull * pull;
    let lightness = l + (s.lightness_mid - l) * s.soften;
    let at = |k: f32| oklab_to_linear([lightness, k * chroma * hue.cos(), k * chroma * hue.sin()]);
    // Largest chroma fraction that stays in sRGB (bisection, fixed steps).
    let mut styled = at(1.0);
    if !in_gamut(styled) {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..12 {
            let mid = 0.5 * (lo + hi);
            if in_gamut(at(mid)) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        styled = at(lo).map(|v| v.clamp(0.0, 1.0));
    }
    std::array::from_fn(|k| colour[k] + (styled[k] - colour[k]) * s.amount)
}

/// Ground colour under the snow rule: material weights (snow disabled; the
/// world look's snow rule is applied on top) blending rock with strata,
/// scree, the styled biome `soil` tint, sand and wet sediment from the
/// `palette`. `warp` is a low-frequency noise in about -1..1 for the strata.
#[allow(clippy::too_many_arguments)]
pub fn surface_colour(
    height_m: f32,
    slope: f32,
    moisture: f32,
    hardness: f32,
    sediment: f32,
    flow: f32,
    soil: [f32; 3],
    warp: f32,
    palette: &GroundPalette,
) -> [f32; 3] {
    let w = material_weights(&MaterialInput {
        height_m,
        slope,
        uphill_slope: 0.0,
        temperature_c: 100.0,
        moisture,
        hardness,
        sediment,
        flow,
    });
    let strata = strata_band(height_m, warp, STRATA_PERIOD_M);
    let rock = strata_colour_factor(&strata, strata_visibility(slope, hardness) * palette.strata);
    let soil = style_soil(soil, &palette.soil);
    std::array::from_fn(|k| {
        w.bedrock * palette.rock[k] * rock
            + w.scree * palette.scree[k]
            + (w.soil + w.snow) * soil[k]
            + w.sand * palette.sand[k]
            + w.wet_sediment * palette.wet_sediment[k]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> MaterialInput {
        MaterialInput {
            height_m: 800.0,
            slope: 0.0,
            uphill_slope: 0.0,
            temperature_c: 15.0,
            moisture: 0.5,
            hardness: 0.5,
            sediment: 0.1,
            flow: 0.0,
        }
    }

    #[test]
    fn weights_sum_to_one_over_a_sweep() {
        for s in 0..=9 {
            for t in 0..=6 {
                for m in 0..=4 {
                    for f in 0..=3 {
                        let i = MaterialInput {
                            height_m: [-5.0, 2.0, 50.0, 3000.0][f],
                            slope: s as f32 * 10.0 * DEG,
                            uphill_slope: (9 - s) as f32 * 10.0 * DEG,
                            temperature_c: -30.0 + t as f32 * 10.0,
                            moisture: m as f32 / 4.0,
                            hardness: (s % 3) as f32 / 2.0,
                            sediment: (t % 3) as f32 / 2.0,
                            flow: f as f32 / 3.0,
                        };
                        let w = material_weights(&i);
                        assert!((w.sum() - 1.0).abs() < 1.0e-5, "{i:?} {w:?}");
                        assert!(w.as_array().iter().all(|x| *x >= -1.0e-6));
                    }
                }
            }
        }
    }

    #[test]
    fn rock_is_monotonic_in_slope() {
        let mut last = -1.0;
        for d in 0..=90 {
            let w = material_weights(&MaterialInput {
                slope: d as f32 * DEG,
                ..base()
            });
            assert!(w.bedrock >= last - 1.0e-6, "slope {d}");
            last = w.bedrock;
        }
        assert!(last > 0.95);
        assert!(material_weights(&base()).bedrock < 0.1);
    }

    #[test]
    fn snow_increases_with_cold_and_fades_on_cliffs() {
        let mut last = -1.0;
        for k in 0..=50 {
            let w = material_weights(&MaterialInput {
                temperature_c: 20.0 - k as f32,
                slope: 10.0 * DEG,
                ..base()
            });
            assert!(w.snow >= last - 1.0e-6);
            last = w.snow;
        }
        let cold = MaterialInput {
            temperature_c: -20.0,
            ..base()
        };
        assert!(material_weights(&cold).snow > 0.99);
        let cliff = MaterialInput {
            slope: 70.0 * DEG,
            ..cold
        };
        assert!(material_weights(&cliff).snow < 0.01);
    }

    #[test]
    fn sand_sits_near_sea_level() {
        let at = |h: f32| {
            material_weights(&MaterialInput {
                height_m: h,
                slope: 2.0 * DEG,
                moisture: 0.6,
                ..base()
            })
            .sand
        };
        assert!(at(1.0) > 0.9);
        assert!(at(1.0) > at(4.0));
        assert!(at(20.0) < 0.01);
    }

    #[test]
    fn scree_sits_below_cliffs() {
        let i = MaterialInput {
            slope: 25.0 * DEG,
            uphill_slope: 55.0 * DEG,
            ..base()
        };
        assert!(material_weights(&i).scree > 0.3);
        let no_cliff = MaterialInput {
            uphill_slope: 10.0 * DEG,
            ..i
        };
        assert!(material_weights(&no_cliff).scree < 0.01);
    }

    #[test]
    fn weights_are_continuous() {
        // Small input steps give small weight steps (no hard IDs).
        let mut prev = material_weights(&base());
        for k in 1..=2000 {
            let x = k as f32 / 2000.0;
            let w = material_weights(&MaterialInput {
                slope: x * 80.0 * DEG,
                uphill_slope: (1.0 - x) * 70.0 * DEG,
                temperature_c: 10.0 - 20.0 * x,
                height_m: 20.0 - 40.0 * x,
                moisture: x,
                ..base()
            });
            for (a, b) in w.as_array().iter().zip(prev.as_array().iter()) {
                assert!((a - b).abs() < 0.05, "jump at {x}");
            }
            prev = w;
        }
    }

    #[test]
    fn strata_are_continuous_and_banded() {
        let mut prev = strata_band(0.0, 0.0, 10.0).tone;
        let mut changed = 0;
        for k in 1..=20_000 {
            let s = strata_band(k as f32 * 0.005, 0.0, 10.0);
            assert!((s.tone - prev).abs() < 0.05);
            if (s.tone - prev).abs() > 1.0e-4 {
                changed += 1;
            }
            prev = s.tone;
        }
        assert!(changed > 100);
        assert!(strata_visibility(70.0 * DEG, 1.0) > 0.99);
        assert!(strata_visibility(10.0 * DEG, 1.0) < 1.0e-6);
    }

    #[test]
    fn mean_colour_blends_and_tint_preserves_mean() {
        let p = MaterialPalette::default();
        let snow = MaterialWeights {
            snow: 1.0,
            ..Default::default()
        };
        assert_eq!(mean_colour(&snow, &p), p.mean[4]);
        let t = tint_detail([0.3, 0.3, 0.3], [0.3, 0.3, 0.3], [0.2, 0.1, 0.05]);
        assert!((t[0] - 0.2).abs() < 1.0e-6 && (t[2] - 0.05).abs() < 1.0e-6);
    }

    fn stylised() -> GroundPalette {
        GroundPalette {
            soil: SoilStyle {
                amount: 1.0,
                chroma_gain: 1.5,
                chroma_max: 0.13,
                soften: 0.25,
                lightness_mid: 0.52,
                hue_pull: 0.3,
                anchors: 2,
                hue_anchors_deg: [85.0, 135.0, 0.0],
            },
            ..GroundPalette::PHYSICAL
        }
    }

    #[test]
    fn oklab_round_trips_and_style_zero_is_identity() {
        for c in [[0.2, 0.32, 0.15], [0.8, 0.66, 0.45], [0.02, 0.5, 0.9]] {
            let back = oklab_to_linear(linear_to_oklab(c));
            assert!(
                (0..3).all(|k| (back[k] - c[k]).abs() < 1.0e-4),
                "{c:?} {back:?}"
            );
        }
        let soil = [0.05, 0.09, 0.03];
        assert_eq!(style_soil(soil, &GroundPalette::PHYSICAL.soil), soil);
        let off = GroundPalette::PHYSICAL.blend(&stylised(), 0.0);
        assert_eq!(off.soil.amount, 0.0);
        assert_eq!(off.rock, GroundPalette::PHYSICAL.rock);
    }

    #[test]
    fn soil_style_saturates_softens_and_pulls_hue_in_gamut() {
        let style = stylised().soil;
        // Forest green, dry grass, desert, near-grey and a saturated blue.
        for c in [
            [0.03, 0.08, 0.02],
            [0.21, 0.23, 0.07],
            [0.6, 0.4, 0.17],
            [0.1, 0.1, 0.1],
            [0.0, 0.1, 0.9],
        ] {
            let s = style_soil(c, &style);
            assert!(s.iter().all(|v| (0.0..=1.0).contains(v)), "{c:?} -> {s:?}");
            let (a, b) = (linear_to_oklab(c), linear_to_oklab(s));
            // Softer: lightness moves toward the mid value, never past it.
            let (da, db) = ((a[0] - 0.52).abs(), (b[0] - 0.52).abs());
            assert!(db <= da + 1.0e-4, "{c:?}: {a:?} -> {b:?}");
            // Never greyer than gain allows, capped at chroma_max.
            let (ca, cb) = (a[1].hypot(a[2]), b[1].hypot(b[2]));
            assert!(cb <= style.chroma_max + 1.0e-3, "{c:?}: chroma {cb}");
            if ca * style.chroma_gain < style.chroma_max
                && in_gamut(oklab_to_linear([b[0], a[1] * 1.5, a[2] * 1.5]))
            {
                assert!(cb >= ca * 1.4, "{c:?}: chroma {ca} -> {cb}");
            }
        }
        // A green at 145° moves toward the 135° anchor by 30 %.
        let green = oklab_to_linear([
            0.5,
            0.08 * (145.0f32 * DEG).cos(),
            0.08 * (145.0f32 * DEG).sin(),
        ]);
        let s = linear_to_oklab(style_soil(green, &style));
        let hue = s[2].atan2(s[1]) / DEG;
        assert!((hue - 142.0).abs() < 0.5, "hue {hue}");
    }
}
