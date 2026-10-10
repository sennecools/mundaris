//! Surface material rules (pipeline §10.1, §10.4; milestone M4 Surface).
//!
//! PROTOTYPE: pure f32 functions that turn per-sample terrain facts (height,
//! slope, climate, Tier A hardness / sediment / flow) into smooth weights over
//! a small fixed material set. No hard material IDs (§2 P4): every output is a
//! continuous weight and the six weights always sum to one. The WGSL mirror is
//! `crates/renderer/src/shaders/material_rules.wgsl`; keep them in step.
//!
//! Not wired into the renderer yet. Constants marked `PROTOTYPE:` are
//! hardcoded and move to archetype data when the look is accepted.

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
const BEACH_H_M: f32 = 6.0;
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
}
