//! Moon world-map generation (W1): a deterministic CPU bake of macro elevation
//! and biome weights on a cube map, built by history instead of by noise:
//!
//! 1. **Crust:** gentle regional relief from spherical gradient noise.
//! 2. **Bombardment:** basins and craters arrive oldest first. Sizes follow the
//!    Neukum 2001 production function and formation ages its chronology.
//!    Each impact reads the current ground, excavates a bowl (complex craters:
//!    flat floor and central peak), and drapes rim and ejecta over its
//!    surroundings, partly burying older craters. Shapes follow lunar
//!    morphometry (Pike 1977). Each crater is softened by its own age with
//!    lunar scale-dependent diffusion, κ_eff(D) = κ_1km·(D/1 km)^0.9 (Fassett et
//!    al. 2018), approximated as a radial blur of its profile.
//! 3. **Mare flooding:** at the mare age, the largest older basins are flooded
//!    with flat lava to a fill level; younger impacts then crater the mare.
//!
//! Optional recipe sections (each absent = the v1 behaviour, byte-identical):
//! - `morphology`: per-crater depth/rim variation, oblique elliptical
//!   craters, wall terraces, peak rings and multi-ring basin scarps.
//! - `secondaries`: chains of shallow, radially elongated secondary craters.
//! - `rays`: bright streaks around young craters (rim/ejecta biome).
//! - `lava`: several flooding episodes of connected lava that spills into
//!   adjacent lowlands, buries craters formed in between (ghost craters),
//!   prefers the near side and fades through a mixed shoreline.
//!
//! Biome weights (highland, mare, crater floor, crater rim and ejecta) are
//! painted by the same events and alpha-composited in time order, so younger
//! events cover older ones and boundaries blend.

use super::cube::{CubeMap, neighbours, texel_directions};
use super::neukum;
use glam::DVec3;
use mundaris_math::noise::gradient_noise_value;
use serde::{Deserialize, Serialize};

/// Biome channels, in storage order. `crater_floor` and `crater_rim_ejecta`
/// are the two sub-biomes of the crater biome.
pub const BIOMES: [&str; 4] = ["highland", "mare", "crater_floor", "crater_rim_ejecta"];
const HIGHLAND: usize = 0;
const MARE: usize = 1;
const FLOOR: usize = 2;
const RIM: usize = 3;

pub const RECIPE_SCHEMA: &str = "mundaris.world-map-recipe.moon.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoonWorldMapRecipe {
    pub schema: String,
    pub id: String,
    pub seed: u64,
    /// Game radius of the body.
    pub radius_m: f64,
    /// World scale (`docs/GAME_AND_SYSTEM_DIRECTION.md`, world scale and body
    /// sizes): the map is generated on the real-size equivalent body of radius
    /// `radius_m / world_scale`, with every other length in this recipe in real
    /// metres, and all resulting heights are multiplied by `world_scale`.
    #[serde(default = "unit_scale")]
    pub world_scale: f64,
    /// Texels per cube-face edge.
    pub face_resolution: u32,
    pub crust: CrustRecipe,
    pub craters: CraterRecipe,
    pub basins: BasinRecipe,
    pub degradation: DegradationRecipe,
    pub mare: MareRecipe,
    pub biomes: BiomeRecipe,
    /// Crater morphology beyond the Pike profile (terraces, peak rings,
    /// multi-ring basins, per-crater variation, oblique impacts). Absent: the
    /// plain v1 profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morphology: Option<MorphologyRecipe>,
    /// Secondary crater chains around large primaries. Absent: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondaries: Option<SecondaryRecipe>,
    /// Bright ray systems of young craters, painted as rim/ejecta biome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rays: Option<RayRecipe>,
    /// Connected, multi-episode lava flooding. Absent: the v1 single flood
    /// inside 0.95 basin radii.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lava: Option<LavaRecipe>,
}

/// Complex-crater and basin morphology (lunar values: Pike 1977; Baker et al.
/// 2011 for peak rings, Dring/Drim ≈ 0.5; Pike & Spudis 1987 for √2 ring
/// spacing).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MorphologyRecipe {
    /// Per-crater depth and rim height vary by up to ± this fraction.
    pub shape_variation: f64,
    /// Fraction of impacts that are oblique (elongated).
    pub oblique_fraction: f64,
    /// Largest length/width ratio of an oblique crater.
    pub max_elongation: f64,
    /// Most wall terraces on the largest complex craters (0 = none).
    pub terrace_count_max: u32,
    /// 0 = smooth wall, 1 = fully stepped benches and scarps.
    pub terrace_strength: f64,
    /// Central peaks give way to a peak ring of half the rim diameter above
    /// this diameter (transitional from about 0.7 of it).
    pub peak_ring_d_m: f64,
    /// Basins at least this large get an outer ring scarp at √2 rim radii.
    pub multi_ring_d_m: f64,
    /// Outer ring height as a fraction of the rim height.
    pub outer_ring_height: f64,
    /// Minimum radial blur of every crater profile, in map texels: band-limits
    /// small craters to the map resolution so they bake as soft dimples, not
    /// square texel blocks (0 = off).
    pub antialias_texels: f64,
}

/// Secondary craters: chains of small, shallow, radially elongated craters
/// thrown out by a large primary (largest secondaries ≈ 4 % of the primary).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecondaryRecipe {
    /// Primaries at least this large throw resolvable secondaries.
    pub primary_min_d_m: f64,
    /// Chains for a primary of `primary_min_d_m`; grows with √(D / min).
    pub chains_per_primary: f64,
    pub members_min: u32,
    pub members_max: u32,
    /// Largest secondary diameter as a fraction of the primary's.
    pub size_ratio_max: f64,
    /// Chain start distance from the primary centre, in primary radii.
    pub range_radii: [f64; 2],
    /// Depth relative to a primary crater of the same size.
    pub depth_scale: f64,
    /// Length/width ratio along the radial direction.
    pub elongation: f64,
}

/// Ray systems: narrow bright streaks of fresh ejecta that fade with age
/// (lunar rays survive about 1 Ga, the Copernican period).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RayRecipe {
    pub min_d_m: f64,
    pub max_age_ga: f64,
    /// Ray reach, in crater radii.
    pub extent_radii: f64,
    pub count: u32,
    /// Ray half-width, in crater radii.
    pub width_radii: f64,
    pub opacity: f64,
}

/// Lava flooding as connected flow in time: each episode raises the level and
/// floods every texel below it that is connected to the basin centre (so lava
/// spills into neighbouring lowlands and embays craters), and craters formed
/// between episodes are partly buried.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LavaRecipe {
    /// Episodes from `mare.age_ga` to `end_age_ga`; episode k of n fills to
    /// (k+1)/n of `mare.fill_fraction`.
    pub episodes: u32,
    pub end_age_ga: f64,
    /// Lava stops this many basin radii from the basin centre.
    pub spill_radii: f64,
    /// Width of the regolith-mixed shoreline outside the flow (real m).
    pub shore_mix_m: f64,
    /// Basins towards this direction flood first (thin near-side crust).
    pub nearside_direction: [f64; 3],
    /// Flood ranking = diameter × (1 + bias · cos(angle to near side)).
    pub nearside_bias: f64,
    /// Outside the basin rim, the lava level varies by ± this fraction of
    /// the basin depth (smooth noise), so flows end in lobes instead of
    /// tracing a circle across flat ground.
    pub lobe_amplitude: f64,
    /// Wavelength of the lobe noise (real m).
    pub lobe_wavelength_m: f64,
}

/// Pre-bombardment relief: fBm of spherical gradient noise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CrustRecipe {
    pub amplitude_m: f64,
    /// Wavelength of the first octave.
    pub wavelength_m: f64,
    pub octaves: u32,
    pub gain: f64,
}

/// Neukum-distributed impacts on a surface of the given age.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CraterRecipe {
    pub surface_age_ga: f64,
    pub d_min_m: f64,
    pub d_max_m: f64,
    /// Simple-to-complex transition diameter (lunar ≈ 15–20 km).
    pub simple_complex_m: f64,
    /// Continuous ejecta reach, in crater radii.
    pub ejecta_extent: f64,
    /// Fraction of the pre-impact relief that survives inside a new crater.
    pub inheritance: f64,
    /// Relative amplitude of rim irregularity (angular harmonics).
    pub irregularity: f64,
    /// Safety cap on the sampled crater count.
    pub max_count: u32,
}

/// Explicit large basins (beyond the crater population's range).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BasinRecipe {
    pub count: u32,
    pub d_min_m: f64,
    pub d_max_m: f64,
    /// Basin depth as a fraction of the complex-crater (Pike) depth: large
    /// basins relax and are shallower than the scaling law predicts.
    pub depth_scale: f64,
    pub age_min_ga: f64,
    pub age_max_ga: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DegradationRecipe {
    /// κ at 1 km scale (Fassett & Thomson 2014: ≈ 5.5 m²/Myr).
    pub kappa_1km_m2_per_myr: f64,
    /// Size exponent of κ_eff(D) (Fassett et al. 2018: 0.9).
    pub exponent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MareRecipe {
    pub age_ga: f64,
    /// How many of the largest older basins/craters are flooded.
    pub flooded: u32,
    /// Fill level between the basin's lowest point (0) and its rim crest (1).
    pub fill_fraction: f64,
    /// Width over which the mare biome fades at the shoreline (m of depth).
    pub shore_depth_m: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BiomeRecipe {
    /// Craters at least this large paint the crater biome.
    pub crater_min_d_m: f64,
    /// Craters larger than this do not paint the crater biome: unflooded
    /// basins read as highland terrain, not as one huge crater floor.
    /// Absent: no upper limit (v1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crater_max_d_m: Option<f64>,
    /// A crater stops painting once its degradation σ/R exceeds this.
    pub degraded_ratio: f64,
    /// Ejecta stops painting the rim/ejecta biome beyond this many radii.
    pub ejecta_biome_extent: f64,
    /// Only craters younger than this keep a visible ejecta blanket (on the
    /// Moon, roughly the Eratosthenian and Copernican craters); older ejecta
    /// fades back to the underlying biome over the 0.3 Ga before.
    pub ejecta_max_age_ga: f64,
}

fn unit_scale() -> f64 {
    1.0
}

impl MoonWorldMapRecipe {
    /// Radius of the real-size equivalent body the map is generated on.
    pub fn equivalent_radius_m(&self) -> f64 {
        self.radius_m / self.world_scale
    }

    pub fn validate(&self) -> Result<(), String> {
        let ok = |v: f64, lo: f64, hi: f64| v.is_finite() && (lo..=hi).contains(&v);
        let c = &self.craters;
        let b = &self.basins;
        let checks = [
            (self.schema == RECIPE_SCHEMA, "schema"),
            (ok(self.radius_m, 1.0e3, 1.0e8), "radius_m"),
            (ok(self.world_scale, 1.0e-3, 1.0), "world_scale"),
            (
                self.face_resolution.is_power_of_two()
                    && (16..=4096).contains(&self.face_resolution),
                "face_resolution must be a power of two in 16..=4096",
            ),
            (
                ok(
                    self.crust.amplitude_m,
                    0.0,
                    0.05 * self.equivalent_radius_m(),
                ) && ok(
                    self.crust.wavelength_m,
                    1.0,
                    10.0 * self.equivalent_radius_m(),
                ) && (1..=12).contains(&self.crust.octaves)
                    && ok(self.crust.gain, 0.0, 1.0),
                "crust",
            ),
            (
                ok(c.surface_age_ga, 0.0, 4.5)
                    && ok(c.d_min_m, 10.0, c.d_max_m)
                    && ok(c.d_max_m, c.d_min_m, 300_000.0)
                    && ok(c.simple_complex_m, 0.0, 1.0e6)
                    && ok(c.ejecta_extent, 1.2, 6.0)
                    && ok(c.inheritance, 0.0, 1.0)
                    && ok(c.irregularity, 0.0, 0.4)
                    && (1..=200_000).contains(&c.max_count),
                "craters",
            ),
            (
                b.count <= 64
                    && ok(b.d_min_m, 10.0, b.d_max_m)
                    && ok(b.d_max_m, b.d_min_m, 4.0 * self.equivalent_radius_m())
                    && ok(b.depth_scale, 0.0, 2.0)
                    && ok(b.age_min_ga, 0.0, b.age_max_ga)
                    && ok(b.age_max_ga, b.age_min_ga, 4.5),
                "basins",
            ),
            (
                ok(self.degradation.kappa_1km_m2_per_myr, 0.0, 1.0e4)
                    && ok(self.degradation.exponent, 0.0, 2.0),
                "degradation",
            ),
            (
                ok(self.mare.age_ga, 0.0, 4.5)
                    && self.mare.flooded <= 64
                    && ok(self.mare.fill_fraction, 0.0, 1.0)
                    && ok(self.mare.shore_depth_m, 0.0, 1.0e5),
                "mare",
            ),
            (
                ok(self.biomes.crater_min_d_m, 0.0, 1.0e7)
                    && ok(self.biomes.degraded_ratio, 0.0, 10.0)
                    && ok(
                        self.biomes.ejecta_biome_extent,
                        1.0,
                        c.ejecta_extent.max(1.0),
                    )
                    && ok(self.biomes.ejecta_max_age_ga, 0.0, 4.5)
                    && self
                        .biomes
                        .crater_max_d_m
                        .is_none_or(|max| ok(max, self.biomes.crater_min_d_m, 1.0e7)),
                "biomes",
            ),
        ];
        let m = self.morphology.as_ref();
        let s = self.secondaries.as_ref();
        let r = self.rays.as_ref();
        let l = self.lava.as_ref();
        let optional = [
            (
                m.is_none_or(|m| {
                    ok(m.shape_variation, 0.0, 0.5)
                        && ok(m.oblique_fraction, 0.0, 1.0)
                        && ok(m.max_elongation, 1.0, 4.0)
                        && m.terrace_count_max <= 8
                        && ok(m.terrace_strength, 0.0, 1.0)
                        && ok(m.peak_ring_d_m, c.simple_complex_m, 1.0e7)
                        && ok(m.multi_ring_d_m, m.peak_ring_d_m, 1.0e7)
                        && ok(m.outer_ring_height, 0.0, 2.0)
                        && ok(m.antialias_texels, 0.0, 4.0)
                }),
                "morphology",
            ),
            (
                s.is_none_or(|s| {
                    ok(s.primary_min_d_m, c.d_min_m, 1.0e7)
                        && ok(s.chains_per_primary, 0.0, 64.0)
                        && (1..=32).contains(&s.members_min)
                        && (s.members_min..=32).contains(&s.members_max)
                        && ok(s.size_ratio_max, 0.001, 0.2)
                        && ok(s.range_radii[0], 1.0, s.range_radii[1])
                        && ok(s.range_radii[1], s.range_radii[0], 20.0)
                        && ok(s.depth_scale, 0.0, 1.0)
                        && ok(s.elongation, 1.0, 4.0)
                }),
                "secondaries",
            ),
            (
                r.is_none_or(|r| {
                    ok(r.min_d_m, 0.0, 1.0e7)
                        && ok(r.max_age_ga, 0.0, 4.5)
                        && ok(r.extent_radii, 1.0, 30.0)
                        && (1..=64).contains(&r.count)
                        && ok(r.width_radii, 0.001, 2.0)
                        && ok(r.opacity, 0.0, 1.0)
                }),
                "rays",
            ),
            (
                l.is_none_or(|l| {
                    (1..=8).contains(&l.episodes)
                        && ok(l.end_age_ga, 0.0, self.mare.age_ga)
                        && ok(l.spill_radii, 0.5, 4.0)
                        && ok(l.shore_mix_m, 0.0, 1.0e6)
                        && l.nearside_direction.iter().all(|v| v.is_finite())
                        && DVec3::from_array(l.nearside_direction).length() > 1e-9
                        && ok(l.nearside_bias, 0.0, 4.0)
                        && ok(l.lobe_amplitude, 0.0, 2.0)
                        && ok(l.lobe_wavelength_m, 100.0, 1.0e7)
                }),
                "lava",
            ),
        ];
        match checks.iter().chain(&optional).find(|(pass, _)| !pass) {
            Some((_, what)) => Err(format!("world-map recipe: invalid {what}")),
            None => Ok(()),
        }
    }
}

/// Baked fields plus a summary of what happened.
#[derive(Debug, Clone)]
pub struct MoonWorldMap {
    pub recipe: MoonWorldMapRecipe,
    /// Height above the reference radius, game metres (scaled by `world_scale`).
    pub elevation: CubeMap<f32>,
    /// Biome weights in [`BIOMES`] order, summing to 1.
    pub biomes: CubeMap<[f32; 4]>,
    pub stats: BakeStats,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BakeStats {
    pub craters: usize,
    pub secondaries: usize,
    pub basins: usize,
    pub flooded_basins: usize,
    pub elevation_min_m: f64,
    pub elevation_max_m: f64,
    /// Area fraction per biome (weight-averaged over texels).
    pub biome_fractions: [f64; 4],
}

/// One impact event.
#[derive(Debug, Clone, Copy)]
struct Impact {
    age_ga: f64,
    center: DVec3,
    diameter_m: f64,
    basin: bool,
    harmonics: [(f64, f64); 4],
    /// Multipliers on the Pike depth and rim height (1 without morphology).
    depth_mul: f64,
    rim_mul: f64,
    /// Length/width ratio (1 = round) and the major-axis azimuth in the
    /// crater's tangent frame (`center.any_orthonormal_vector()`, its cross).
    elongation: f64,
    azimuth: f64,
    secondary: bool,
    /// Seed of the ray pattern; 0 = no rays.
    ray_seed: u64,
}

impl Impact {
    fn new(age_ga: f64, center: DVec3, diameter_m: f64, basin: bool, h: [(f64, f64); 4]) -> Self {
        Self {
            age_ga,
            center,
            diameter_m,
            basin,
            harmonics: h,
            depth_mul: 1.0,
            rim_mul: 1.0,
            elongation: 1.0,
            azimuth: 0.0,
            secondary: false,
            ray_seed: 0,
        }
    }

    /// Tangent frame used for azimuths around the centre.
    fn frame(&self) -> (DVec3, DVec3) {
        let e1 = self.center.any_orthonormal_vector();
        (e1, self.center.cross(e1))
    }
}

/// SplitMix64: deterministic, seedable, no ambient state.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn direction(&mut self) -> DVec3 {
        let z = 2.0 * self.unit() - 1.0;
        let phi = std::f64::consts::TAU * self.unit();
        let r = (1.0 - z * z).max(0.0).sqrt();
        DVec3::new(r * phi.cos(), r * phi.sin(), z)
    }
}

/// Fresh lunar crater morphometry (Pike 1977), metres.
#[derive(Clone)]
struct Shape {
    depth: f64,
    rim: f64,
    /// Flat-floor radius as a fraction of the crater radius.
    floor: f64,
    peak: f64,
    /// 0 = central peak, 1 = peak ring at half the rim radius.
    peak_ring: f64,
    terraces: u32,
    terrace_strength: f64,
    /// Outer ring scarp height (m); 0 = none.
    outer_ring: f64,
}

/// Outer ring radius of multi-ring basins, in rim radii (√2 spacing).
const OUTER_RING_S: f64 = std::f64::consts::SQRT_2;

fn shape(diameter_m: f64, simple_complex_m: f64, depth_scale: f64) -> Shape {
    let d_km = diameter_m / 1000.0;
    if diameter_m < simple_complex_m {
        Shape {
            depth: 196.0 * d_km.powf(1.010) * depth_scale,
            rim: 36.0 * d_km.powf(1.014) * depth_scale,
            floor: 0.0,
            peak: 0.0,
            peak_ring: 0.0,
            terraces: 0,
            terrace_strength: 0.0,
            outer_ring: 0.0,
        }
    } else {
        let depth = 1044.0 * d_km.powf(0.301) * depth_scale;
        Shape {
            depth,
            rim: 236.0 * d_km.powf(0.399) * depth_scale,
            floor: (0.187 * d_km.powf(0.249)).min(0.7),
            peak: 0.25 * depth,
            peak_ring: 0.0,
            terraces: 0,
            terrace_strength: 0.0,
            outer_ring: 0.0,
        }
    }
}

/// `shape` plus the optional morphology for one impact.
fn impact_shape(imp: &Impact, recipe: &MoonWorldMapRecipe) -> Shape {
    let c = &recipe.craters;
    let depth_scale = if imp.basin {
        recipe.basins.depth_scale
    } else {
        1.0
    };
    let mut sh = shape(imp.diameter_m, c.simple_complex_m, depth_scale);
    let Some(m) = &recipe.morphology else {
        return sh;
    };
    sh.depth *= imp.depth_mul;
    sh.rim *= imp.rim_mul;
    sh.peak *= imp.depth_mul;
    if imp.diameter_m >= c.simple_complex_m && !imp.secondary {
        // Terraces grow from one just above the transition to the maximum at
        // about four times it.
        let excess = imp.diameter_m / c.simple_complex_m - 1.0;
        sh.terraces = ((excess / 3.0 * f64::from(m.terrace_count_max)).round() as u32)
            .clamp(1, m.terrace_count_max.max(1))
            .min(m.terrace_count_max);
        sh.terrace_strength = m.terrace_strength;
        sh.peak_ring = smoothstep(0.7 * m.peak_ring_d_m, m.peak_ring_d_m, imp.diameter_m);
        if imp.diameter_m >= m.multi_ring_d_m {
            sh.outer_ring = m.outer_ring_height * sh.rim;
        }
    }
    sh
}

/// Benches and scarps on the inner wall: `t` (0 at the floor edge, 1 at the
/// rim) is pulled towards a staircase whose steps shift with azimuth
/// (`wobble`), so terraces read as slumped blocks rather than contour lines.
fn terraced(t: f64, sh: &Shape, wobble: f64) -> f64 {
    if sh.terraces == 0 || sh.terrace_strength <= 0.0 {
        return t;
    }
    let n = f64::from(sh.terraces);
    let shift = 0.35 * wobble;
    let x = t * n + shift;
    let k = x.floor();
    let stair = (k + smoothstep(0.55, 1.0, x - k) - shift) / n;
    let window = smoothstep(0.0, 0.08, t) * (1.0 - smoothstep(0.88, 0.97, t));
    t + sh.terrace_strength * window * (stair.clamp(0.0, 1.0) - t)
}

/// Fresh relief relative to the pre-impact reference at radial distance `s`
/// (crater radii): bowl or flat floor with peak inside, rim and ejecta outside.
/// `wobble` is the azimuthal irregularity at this point (shifts terraces).
fn profile(s: f64, sh: &Shape, extent: f64, wobble: f64) -> f64 {
    if s < 1.0 {
        let t = if s <= sh.floor {
            0.0
        } else {
            terraced((s - sh.floor) / (1.0 - sh.floor), sh, wobble)
        };
        let peak = if sh.peak > 0.0 {
            let central = (-(s / 0.12).powi(2)).exp();
            if sh.peak_ring > 0.0 {
                let ring = (-((s - 0.5) / 0.05).powi(2)).exp();
                sh.peak * ((1.0 - sh.peak_ring) * central + sh.peak_ring * ring)
            } else {
                sh.peak * central
            }
        } else {
            0.0
        };
        -sh.depth * (1.0 - t * t) + sh.rim * t * t + peak
    } else if s < extent {
        let t = ((s - 1.0) / (extent - 1.0)).clamp(0.0, 1.0);
        let taper = 1.0 - t * t * (3.0 - 2.0 * t);
        let ejecta = sh.rim * s.powi(-3) * taper;
        if sh.outer_ring > 0.0 {
            // Inward-facing scarp: steep inner face, gentle outer decay.
            let rise = smoothstep(OUTER_RING_S - 0.05, OUTER_RING_S, s);
            let decay = (-(s - OUTER_RING_S).max(0.0) / 0.25).exp();
            ejecta + sh.outer_ring * rise * decay * taper
        } else {
            ejecta
        }
    } else {
        0.0
    }
}

/// `profile` blurred radially by σ (crater radii): a 7-tap Gaussian along the
/// radius, the age-softened shape.
fn degraded(s: f64, sh: &Shape, extent: f64, sigma: f64, wobble: f64) -> f64 {
    if sigma < 1e-3 {
        return profile(s, sh, extent, wobble);
    }
    const TAPS: [(f64, f64); 7] = [
        (-3.0, 0.006),
        (-2.0, 0.061),
        (-1.0, 0.242),
        (0.0, 0.382),
        (1.0, 0.242),
        (2.0, 0.061),
        (3.0, 0.006),
    ];
    TAPS.iter()
        .map(|&(k, w)| w * profile((s + k * sigma).abs(), sh, extent, wobble))
        .sum::<f64>()
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Run `f(index)` over every texel on all cores, writing the results in order.
fn parallel_map<T: Send + Copy>(out: &mut [T], f: impl Fn(usize) -> T + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |t| t.get())
        .clamp(1, 64);
    let per = out.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (c, chunk) in out.chunks_mut(per).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (k, v) in chunk.iter_mut().enumerate() {
                    *v = f(c * per + k);
                }
            });
        }
    });
}

fn sample_impacts(recipe: &MoonWorldMapRecipe, rng: &mut Rng) -> Vec<Impact> {
    let r = recipe.equivalent_radius_m();
    let area_km2 = 4.0 * std::f64::consts::PI * (r / 1000.0).powi(2);
    let c = &recipe.craters;
    let (d_min, d_max) = (c.d_min_m / 1000.0, c.d_max_m / 1000.0);
    let expected = (neukum::cumulative(d_min, c.surface_age_ga)
        - neukum::cumulative(d_max, c.surface_age_ga))
        * area_km2;
    let count = (expected.round() as u64).min(u64::from(c.max_count));
    let mut impacts = Vec::with_capacity(count as usize);
    let harmonics = |rng: &mut Rng| {
        let mut h = [(0.0, 0.0); 4];
        for (k, v) in h.iter_mut().enumerate() {
            *v = (
                (rng.unit() - 0.5) * 2.0 / (k as f64 + 1.5),
                rng.unit() * std::f64::consts::TAU,
            );
        }
        h
    };
    for _ in 0..count {
        let diameter_m = 1000.0 * neukum::diameter_quantile(rng.unit(), d_min, d_max);
        let age_ga = neukum::age_quantile(rng.unit(), c.surface_age_ga);
        let center = rng.direction();
        impacts.push(Impact::new(
            age_ga,
            center,
            diameter_m,
            false,
            harmonics(rng),
        ));
    }
    let b = &recipe.basins;
    for _ in 0..b.count {
        let diameter_m = b.d_min_m * (b.d_max_m / b.d_min_m).powf(rng.unit());
        let age_ga = b.age_min_ga + (b.age_max_ga - b.age_min_ga) * rng.unit();
        let center = rng.direction();
        impacts.push(Impact::new(
            age_ga,
            center,
            diameter_m,
            true,
            harmonics(rng),
        ));
    }
    // Optional features draw from their own streams, so the v1 population
    // above stays identical when they are switched on or off.
    if let Some(m) = &recipe.morphology {
        let mut rng = Rng(recipe.seed ^ 0x6D6F_7270_686F_6C6F);
        for imp in &mut impacts {
            imp.depth_mul = 1.0 + m.shape_variation * (2.0 * rng.unit() - 1.0);
            imp.rim_mul = 1.0 + m.shape_variation * (2.0 * rng.unit() - 1.0);
            let oblique = rng.unit() < m.oblique_fraction;
            let (e, az) = (rng.unit(), rng.unit());
            if oblique && !imp.basin {
                imp.elongation = 1.0 + (m.max_elongation - 1.0) * e * e;
                imp.azimuth = az * std::f64::consts::PI;
            }
        }
    }
    if let Some(s) = &recipe.secondaries {
        let mut rng = Rng(recipe.seed ^ 0x7365_636F_6E64_6172);
        let r_body = recipe.equivalent_radius_m();
        let primaries: Vec<Impact> = impacts
            .iter()
            .filter(|p| p.diameter_m >= s.primary_min_d_m)
            .copied()
            .collect();
        for p in &primaries {
            let chains =
                (s.chains_per_primary * (p.diameter_m / s.primary_min_d_m).sqrt()).round() as u32;
            let (e1, e2) = p.frame();
            for _ in 0..chains {
                let az = rng.unit() * std::f64::consts::TAU;
                let span = s.members_max - s.members_min + 1;
                let members = s.members_min + (rng.next() % u64::from(span)) as u32;
                let mut rho = 0.5
                    * p.diameter_m
                    * (s.range_radii[0] + (s.range_radii[1] - s.range_radii[0]) * rng.unit());
                // Chains curve slightly: the azimuth drifts along the chain.
                let drift = (rng.unit() - 0.5) * 0.04;
                for k in 0..members {
                    let d = p.diameter_m * s.size_ratio_max * (0.45 + 0.55 * rng.unit());
                    let a = az + drift * f64::from(k);
                    let toward = e1 * a.cos() + e2 * a.sin();
                    let phi = rho / r_body;
                    let center = (p.center * phi.cos() + toward * phi.sin()).normalize();
                    let radial = toward * phi.cos() - p.center * phi.sin();
                    let mut sec = Impact::new(p.age_ga, center, d, false, harmonics(&mut rng));
                    let (f1, f2) = sec.frame();
                    sec.azimuth = radial.dot(f2).atan2(radial.dot(f1));
                    sec.elongation = s.elongation;
                    sec.depth_mul = s.depth_scale;
                    sec.secondary = true;
                    impacts.push(sec);
                    // Overlapping members merge into a gouge.
                    rho += d * (0.7 + 0.4 * rng.unit());
                }
            }
        }
    }
    if recipe.rays.is_some() {
        let mut rng = Rng(recipe.seed ^ 0x7261_7973_7261_7973);
        for imp in &mut impacts {
            imp.ray_seed = rng.next() | 1;
        }
    }
    // Oldest first; ties broken by size then position for determinism.
    impacts.sort_by(|a, b| {
        b.age_ga
            .total_cmp(&a.age_ga)
            .then(b.diameter_m.total_cmp(&a.diameter_m))
            .then(a.center.x.total_cmp(&b.center.x))
    });
    impacts
}

/// Coarse texel blocks for culling: each impact only tests blocks whose
/// bounding cap can reach it.
struct Blocks {
    centers: Vec<DVec3>,
    /// Angular radius of each block (max angle from its centre to a texel).
    radii: Vec<f64>,
    members: Vec<Vec<usize>>,
}

impl Blocks {
    fn new(dirs: &[DVec3], n: usize) -> Self {
        let b = (n / 16).max(1);
        let per_face = n.div_ceil(b);
        let mut members = vec![Vec::new(); 6 * per_face * per_face];
        for (k, _) in dirs.iter().enumerate() {
            let (face, j, i) = (k / (n * n), (k / n) % n, k % n);
            members[(face * per_face + j / b) * per_face + i / b].push(k);
        }
        let centers: Vec<DVec3> = members
            .iter()
            .map(|m| m.iter().map(|&k| dirs[k]).sum::<DVec3>().normalize())
            .collect();
        let radii = members
            .iter()
            .zip(&centers)
            .map(|(m, c)| {
                m.iter()
                    .map(|&k| dirs[k].dot(*c).clamp(-1.0, 1.0).acos())
                    .fold(0.0, f64::max)
            })
            .collect();
        Self {
            centers,
            radii,
            members,
        }
    }

    /// Texel indices (ascending) within angle `theta` of `center`.
    fn within(&self, dirs: &[DVec3], center: DVec3, theta: f64, out: &mut Vec<usize>) {
        out.clear();
        let cos_theta = theta.min(std::f64::consts::PI).cos();
        for (b, c) in self.centers.iter().enumerate() {
            if c.dot(center).clamp(-1.0, 1.0).acos() <= theta + self.radii[b] + 1e-9 {
                out.extend(
                    self.members[b]
                        .iter()
                        .copied()
                        .filter(|&k| dirs[k].dot(center) >= cos_theta),
                );
            }
        }
        out.sort_unstable();
    }
}

/// Composite `target` biome over `w` with opacity `alpha`.
fn paint(w: &mut [f32; 4], target: usize, alpha: f64) {
    let a = alpha.clamp(0.0, 1.0) as f32;
    if a <= 0.0 {
        return;
    }
    for (k, v) in w.iter_mut().enumerate() {
        *v = *v * (1.0 - a) + if k == target { a } else { 0.0 };
    }
}

/// Bake the Moon world map.
pub fn bake(recipe: &MoonWorldMapRecipe) -> Result<MoonWorldMap, String> {
    recipe.validate()?;
    let n = recipe.face_resolution as usize;
    let r_body = recipe.equivalent_radius_m();
    let dirs = texel_directions(n);
    let blocks = Blocks::new(&dirs, n);
    let mut rng = Rng(recipe.seed ^ 0x6D6F_6F6E_5F77_6D31);

    // 1. Crust.
    let mut h = vec![0.0f64; dirs.len()];
    let crust = &recipe.crust;
    let norm: f64 = (0..crust.octaves).map(|o| crust.gain.powi(o as i32)).sum();
    parallel_map(&mut h, |i| {
        let mut sum = 0.0;
        for o in 0..crust.octaves {
            let scale = r_body / (crust.wavelength_m / 2f64.powi(o as i32));
            let seed = recipe.seed.wrapping_add(u64::from(o) * 0x9E37_79B9);
            sum += crust.gain.powi(o as i32)
                * gradient_noise_value(seed, dirs[i] * scale).unwrap_or(0.0);
        }
        crust.amplitude_m * sum / norm
    });
    let mut highland = [0.0f32; 4];
    highland[HIGHLAND] = 1.0;
    let mut w = vec![highland; dirs.len()];

    // 2–3. Bombardment with mare flooding inserted at its age(s).
    let impacts = sample_impacts(recipe, &mut rng);
    let c = &recipe.craters;
    let mut stats = BakeStats::default();
    let mut emplaced: Vec<(Impact, f64)> = Vec::new(); // impact, rim crest level
    let episode_ages: Vec<f64> = match &recipe.lava {
        None => vec![recipe.mare.age_ga],
        Some(l) if l.episodes > 1 => (0..l.episodes)
            .map(|e| {
                recipe.mare.age_ga
                    - (recipe.mare.age_ga - l.end_age_ga) * f64::from(e) / f64::from(l.episodes - 1)
            })
            .collect(),
        Some(_) => vec![recipe.mare.age_ga],
    };
    let mut next_episode = 0usize;
    let mut lava_basins: Option<Vec<LavaBasin>> = None;
    let mut run_episode = |episode: usize,
                           emplaced: &[(Impact, f64)],
                           h: &mut [f64],
                           w: &mut [[f32; 4]]| match &recipe.lava {
        None => flood(recipe, &dirs, &blocks, emplaced, h, w),
        Some(l) => lava_episode(
            recipe,
            l,
            episode as u32,
            &dirs,
            &blocks,
            emplaced,
            &mut lava_basins,
            h,
            w,
        ),
    };
    let mut local: Vec<usize> = Vec::new();
    for imp in &impacts {
        while next_episode < episode_ages.len() && imp.age_ga < episode_ages[next_episode] {
            stats.flooded_basins = run_episode(next_episode, &emplaced, &mut h, &mut w);
            next_episode += 1;
        }
        let radius = 0.5 * imp.diameter_m;
        let sh = impact_shape(imp, recipe);
        let rays = recipe.rays.as_ref().filter(|r| {
            imp.ray_seed != 0
                && !imp.secondary
                && imp.diameter_m >= r.min_d_m
                && imp.age_ga < r.max_age_ga
        });
        let extent = rays.map_or(c.ejecta_extent, |r| c.ejecta_extent.max(r.extent_radii));
        let mut reach = extent * radius * (1.0 + c.irregularity);
        if imp.elongation > 1.0 {
            reach *= imp.elongation.sqrt();
        }
        let cos_reach = (reach / r_body).min(std::f64::consts::PI).cos();
        blocks.within(&dirs, imp.center, cos_reach.acos(), &mut local);
        if local.is_empty() {
            continue;
        }
        // Reference: mean current height inside the crater.
        let s_of = |d: DVec3| d.dot(imp.center).clamp(-1.0, 1.0).acos() * r_body / radius;
        let (mut sum, mut cnt) = (0.0, 0.0);
        for &i in &local {
            if s_of(dirs[i]) < 1.0 {
                sum += h[i];
                cnt += 1.0;
            }
        }
        let reference = if cnt > 0.0 {
            sum / cnt
        } else {
            local.iter().map(|&i| h[i]).sum::<f64>() / local.len() as f64
        };
        // Age softening: σ = √(2 κ_eff t), κ_eff = κ_1km (D / 1 km)^e.
        let kappa = recipe.degradation.kappa_1km_m2_per_myr
            * (imp.diameter_m / 1000.0).powf(recipe.degradation.exponent);
        let age_sigma = (2.0 * kappa * imp.age_ga * 1000.0).sqrt() / radius;
        let mut sigma = age_sigma;
        if let Some(m) = &recipe.morphology {
            // Texel size at a face centre (texels shrink by up to √3·… toward
            // corners, so this is conservative).
            let texel_m = 2.0 * r_body / n as f64;
            sigma = sigma.max(m.antialias_texels * texel_m / radius);
        }
        // Tangent frame for azimuthal rim irregularity.
        let (e1, e2) = imp.frame();
        let paints = imp.diameter_m >= recipe.biomes.crater_min_d_m
            && recipe
                .biomes
                .crater_max_d_m
                .is_none_or(|max| imp.diameter_m <= max);
        // Rays: (azimuth, length in radii, bead phase) per streak.
        let ray_set: Vec<(f64, f64, f64)> = rays.map_or_else(Vec::new, |r| {
            let mut rr = Rng(imp.ray_seed);
            (0..r.count)
                .map(|k| {
                    let theta = std::f64::consts::TAU * (f64::from(k) + 0.8 * (rr.unit() - 0.5))
                        / f64::from(r.count);
                    let len = r.extent_radii * (0.45 + 0.55 * rr.unit());
                    (theta, len, rr.unit() * std::f64::consts::TAU)
                })
                .collect()
        });
        let ray_strength = rays.map_or(0.0, |r| {
            r.opacity * (1.0 - smoothstep(0.6 * r.max_age_ga, r.max_age_ga, imp.age_ga))
        });
        // Heavily softened craters read as highland; ejecta only on young craters.
        // Freshness follows age only, not the anti-aliasing blur.
        let fresh = (1.0 - age_sigma / recipe.biomes.degraded_ratio.max(1e-9)).clamp(0.0, 1.0);
        let ejecta_age = recipe.biomes.ejecta_max_age_ga;
        let young = 1.0 - smoothstep(ejecta_age - 0.3, ejecta_age, imp.age_ga);
        for &i in &local {
            let d = dirs[i];
            let theta = d.dot(e2).atan2(d.dot(e1));
            let f: f64 = imp
                .harmonics
                .iter()
                .enumerate()
                .map(|(k, &(a, ph))| a * ((k as f64 + 2.0) * theta + ph).cos())
                .sum();
            let mut rho = s_of(d);
            if imp.elongation > 1.0 {
                // Area-preserving ellipse stretched along the azimuth.
                let q = imp.elongation.sqrt();
                let a = theta - imp.azimuth;
                rho = ((rho * a.cos() / q).powi(2) + (rho * a.sin() * q).powi(2)).sqrt();
            }
            let s = rho / (1.0 + c.irregularity * f);
            let relief = degraded(s, &sh, c.ejecta_extent, sigma, f);
            // Inside: replace old relief (keeping `inheritance` of it); outside: drape.
            let inside = 1.0 - smoothstep(0.85, 1.0, s);
            h[i] += relief + inside * (reference - h[i]) * (1.0 - c.inheritance);
            if paints {
                let wall = (1.0 - smoothstep(1.0, recipe.biomes.ejecta_biome_extent, s))
                    * fresh
                    * if s > 1.0 { young } else { 1.0 };
                paint(&mut w[i], RIM, wall);
                let floor_edge = sh.floor.max(0.45);
                paint(
                    &mut w[i],
                    FLOOR,
                    fresh * (1.0 - smoothstep(floor_edge, floor_edge + 0.15, s)),
                );
            }
            if let Some(r) = rays.filter(|_| s > 1.0) {
                let mut streak = 0.0f64;
                for &(ray_theta, len, phase) in &ray_set {
                    if s >= len {
                        continue;
                    }
                    let mut dt = (theta - ray_theta).rem_euclid(std::f64::consts::TAU);
                    if dt > std::f64::consts::PI {
                        dt -= std::f64::consts::TAU;
                    }
                    // Constant width in crater radii, so the streak narrows
                    // in azimuth with distance.
                    let across = (-(dt * s / r.width_radii).powi(2)).exp();
                    let along = 1.0 - smoothstep(0.5 * len, len, s);
                    let beads = 0.65 + 0.35 * (6.0 * s + phase).cos();
                    streak = streak.max(across * along * beads);
                }
                paint(
                    &mut w[i],
                    RIM,
                    ray_strength * streak * smoothstep(1.0, 1.4, s),
                );
            }
        }
        let crest = reference + sh.rim;
        emplaced.push((*imp, crest));
        if imp.basin {
            stats.basins += 1;
        } else if imp.secondary {
            stats.secondaries += 1;
        } else {
            stats.craters += 1;
        }
    }
    while next_episode < episode_ages.len() {
        stats.flooded_basins = run_episode(next_episode, &emplaced, &mut h, &mut w);
        next_episode += 1;
    }

    // Shrink the real-size relief to the game's world scale.
    h.iter_mut().for_each(|v| *v *= recipe.world_scale);
    let mut elevation = CubeMap::new(n, 0.0f32);
    let mut biomes = CubeMap::new(n, [0.0f32; 4]);
    for (k, v) in elevation.data_mut().iter_mut().enumerate() {
        *v = h[k] as f32;
    }
    let mut fractions = [0.0f64; 4];
    for (k, v) in biomes.data_mut().iter_mut().enumerate() {
        let total: f32 = w[k].iter().sum();
        *v = w[k].map(|x| x / total.max(1e-6));
        for (f, x) in fractions.iter_mut().zip(v.iter()) {
            *f += f64::from(*x) / h.len() as f64;
        }
    }
    stats.elevation_min_m = h.iter().copied().fold(f64::INFINITY, f64::min);
    stats.elevation_max_m = h.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    stats.biome_fractions = fractions;
    Ok(MoonWorldMap {
        recipe: recipe.clone(),
        elevation,
        biomes,
        stats,
    })
}

/// Flood the largest basins emplaced so far; returns how many were flooded.
fn flood(
    recipe: &MoonWorldMapRecipe,
    dirs: &[DVec3],
    blocks: &Blocks,
    emplaced: &[(Impact, f64)],
    h: &mut [f64],
    w: &mut [[f32; 4]],
) -> usize {
    let mut largest: Vec<&(Impact, f64)> = emplaced.iter().collect();
    largest.sort_by(|a, b| b.0.diameter_m.total_cmp(&a.0.diameter_m));
    let m = &recipe.mare;
    let mut flooded = 0;
    for (imp, crest) in largest.into_iter().take(m.flooded as usize) {
        let radius = 0.5 * imp.diameter_m;
        let cos_r = (0.95 * radius / recipe.equivalent_radius_m())
            .min(std::f64::consts::PI)
            .cos();
        let mut inside = Vec::new();
        blocks.within(dirs, imp.center, cos_r.acos(), &mut inside);
        let Some(low) = inside.iter().map(|&i| h[i]).reduce(f64::min) else {
            continue;
        };
        let level = low + m.fill_fraction * (crest - low).max(0.0);
        for &i in &inside {
            if h[i] < level {
                let depth = level - h[i];
                h[i] = level;
                paint(
                    &mut w[i],
                    MARE,
                    smoothstep(0.0, m.shore_depth_m.max(1e-9), depth),
                );
            }
        }
        flooded += 1;
    }
    flooded
}

/// A basin chosen for lava flooding, fixed at the first episode.
struct LavaBasin {
    center: DVec3,
    radius_m: f64,
    /// Lowest ground inside the basin when it was chosen.
    low: f64,
    crest: f64,
}

/// One lava episode (`LavaRecipe`): raise each chosen basin's lava level and
/// flood every texel below it that is connected to the basin centre, then mix
/// a regolith shoreline outside the flow. Returns the basin count.
#[allow(clippy::too_many_arguments)]
fn lava_episode(
    recipe: &MoonWorldMapRecipe,
    lava: &LavaRecipe,
    episode: u32,
    dirs: &[DVec3],
    blocks: &Blocks,
    emplaced: &[(Impact, f64)],
    basins: &mut Option<Vec<LavaBasin>>,
    h: &mut [f64],
    w: &mut [[f32; 4]],
) -> usize {
    let r_body = recipe.equivalent_radius_m();
    let m = &recipe.mare;
    let n = recipe.face_resolution as usize;
    let selected = basins.get_or_insert_with(|| {
        let near = DVec3::from_array(lava.nearside_direction).normalize();
        let score = |i: &Impact| i.diameter_m * (1.0 + lava.nearside_bias * i.center.dot(near));
        let mut ranked: Vec<&(Impact, f64)> =
            emplaced.iter().filter(|(i, _)| !i.secondary).collect();
        ranked.sort_by(|a, b| score(&b.0).total_cmp(&score(&a.0)));
        let mut inside = Vec::new();
        ranked
            .into_iter()
            .take(m.flooded as usize)
            .filter_map(|(imp, crest)| {
                let radius_m = 0.5 * imp.diameter_m;
                let angle = (0.95 * radius_m / r_body).min(std::f64::consts::PI);
                blocks.within(dirs, imp.center, angle, &mut inside);
                let low = inside.iter().map(|&i| h[i]).reduce(f64::min)?;
                Some(LavaBasin {
                    center: imp.center,
                    radius_m,
                    low,
                    crest: *crest,
                })
            })
            .collect()
    });
    let fill = m.fill_fraction * f64::from(episode + 1) / f64::from(lava.episodes);
    let texel_m = 2.0 * r_body / n as f64;
    let mix_steps = (lava.shore_mix_m / texel_m).ceil() as usize;
    let mut flooded = vec![false; h.len()];
    let mut seeds = Vec::new();
    for b in selected.iter() {
        let level = b.low + fill * (b.crest - b.low).max(0.0);
        // Beyond the basin rim the available lava thins out: the effective
        // level falls by up to the basin depth at the spill radius, so the
        // shoreline follows terrain contours instead of stopping on a circle.
        let depth_range = (b.crest - b.low).max(0.0);
        // The fall continues past the spill radius, so lava stops on terrain
        // rather than on a circle; the hard cut at twice the spill radius
        // only bounds the search.
        let lobe_scale = r_body / lava.lobe_wavelength_m;
        let level_at = |i: usize| {
            let s = dirs[i].dot(b.center).clamp(-1.0, 1.0).acos() * r_body / b.radius_m;
            let t = ((s - 1.0) / (lava.spill_radii - 1.0).max(1e-9)).max(0.0);
            let lobes = if s > 1.0 && lava.lobe_amplitude > 0.0 {
                let p = dirs[i] * lobe_scale;
                let noise = gradient_noise_value(recipe.seed ^ 0x6C6F_6265, p).unwrap_or(0.0)
                    + 0.5 * gradient_noise_value(recipe.seed ^ 0x6C6F_6266, p * 2.0).unwrap_or(0.0);
                // Fade in over the first half radius past the rim.
                lava.lobe_amplitude * depth_range * noise * smoothstep(1.0, 1.5, s)
            } else {
                0.0
            };
            level - depth_range * t * t + lobes
        };
        let cos_spill = (2.0 * lava.spill_radii * b.radius_m / r_body)
            .min(std::f64::consts::PI)
            .cos();
        flooded.iter_mut().for_each(|f| *f = false);
        let seed_angle = (0.5 * b.radius_m / r_body).min(std::f64::consts::PI);
        blocks.within(dirs, b.center, seed_angle, &mut seeds);
        let mut queue: std::collections::VecDeque<usize> = seeds
            .iter()
            .copied()
            .filter(|&i| h[i] < level_at(i))
            .collect();
        for &i in &queue {
            flooded[i] = true;
        }
        let mut cells = Vec::new();
        while let Some(i) = queue.pop_front() {
            cells.push(i);
            for nb in neighbours(i, n) {
                if !flooded[nb] && h[nb] < level_at(nb) && dirs[nb].dot(b.center) >= cos_spill {
                    flooded[nb] = true;
                    queue.push_back(nb);
                }
            }
        }
        for &i in &cells {
            let surface = level_at(i);
            let depth = surface - h[i];
            h[i] = surface;
            let deep = smoothstep(0.0, m.shore_depth_m.max(1e-9), depth);
            let alpha = if mix_steps > 0 {
                0.5 + 0.5 * deep
            } else {
                deep
            };
            paint(&mut w[i], MARE, alpha);
        }
        // Shoreline: lateral regolith mixing fades the mare outward.
        let mut frontier = cells;
        for step in 1..=mix_steps {
            let mut next = Vec::new();
            for &i in &frontier {
                for nb in neighbours(i, n) {
                    if !flooded[nb] {
                        flooded[nb] = true;
                        next.push(nb);
                    }
                }
            }
            let fade = 1.0 - step as f64 / (mix_steps + 1) as f64;
            for &i in &next {
                paint(&mut w[i], MARE, 0.5 * fade * fade);
            }
            frontier = next;
        }
    }
    selected.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn small_recipe() -> MoonWorldMapRecipe {
        MoonWorldMapRecipe {
            schema: RECIPE_SCHEMA.into(),
            id: "test".into(),
            seed: 7,
            radius_m: 20_000.0,
            world_scale: 1.0,
            face_resolution: 32,
            crust: CrustRecipe {
                amplitude_m: 100.0,
                wavelength_m: 20_000.0,
                octaves: 3,
                gain: 0.5,
            },
            craters: CraterRecipe {
                surface_age_ga: 4.0,
                d_min_m: 3_000.0,
                d_max_m: 10_000.0,
                simple_complex_m: 15_000.0,
                ejecta_extent: 2.5,
                inheritance: 0.1,
                irregularity: 0.08,
                max_count: 500,
            },
            basins: BasinRecipe {
                count: 1,
                d_min_m: 20_000.0,
                d_max_m: 20_000.0,
                depth_scale: 0.5,
                age_min_ga: 4.0,
                age_max_ga: 4.0,
            },
            degradation: DegradationRecipe {
                kappa_1km_m2_per_myr: 5.5,
                exponent: 0.9,
            },
            mare: MareRecipe {
                age_ga: 3.6,
                flooded: 1,
                fill_fraction: 0.5,
                shore_depth_m: 50.0,
            },
            biomes: BiomeRecipe {
                crater_min_d_m: 3_000.0,
                degraded_ratio: 0.5,
                ejecta_biome_extent: 1.8,
                ejecta_max_age_ga: 4.5,
                crater_max_d_m: None,
            },
            morphology: None,
            secondaries: None,
            rays: None,
            lava: None,
        }
    }

    /// `small_recipe` with every optional section switched on.
    fn full_recipe() -> MoonWorldMapRecipe {
        let mut r = small_recipe();
        r.face_resolution = 64;
        r.morphology = Some(MorphologyRecipe {
            shape_variation: 0.15,
            oblique_fraction: 0.2,
            max_elongation: 2.0,
            terrace_count_max: 4,
            terrace_strength: 0.8,
            peak_ring_d_m: 18_000.0,
            multi_ring_d_m: 19_000.0,
            outer_ring_height: 0.6,
            antialias_texels: 0.6,
        });
        r.secondaries = Some(SecondaryRecipe {
            primary_min_d_m: 9_000.0,
            chains_per_primary: 4.0,
            members_min: 3,
            members_max: 6,
            size_ratio_max: 0.08,
            range_radii: [1.5, 4.0],
            depth_scale: 0.5,
            elongation: 1.6,
        });
        r.rays = Some(RayRecipe {
            min_d_m: 3_000.0,
            max_age_ga: 4.5,
            extent_radii: 6.0,
            count: 10,
            width_radii: 0.15,
            opacity: 0.8,
        });
        r.lava = Some(LavaRecipe {
            episodes: 3,
            end_age_ga: 3.2,
            spill_radii: 2.0,
            shore_mix_m: 1_500.0,
            nearside_direction: [1.0, 0.0, 0.0],
            nearside_bias: 0.5,
            lobe_amplitude: 0.3,
            lobe_wavelength_m: 8_000.0,
        });
        r
    }

    #[test]
    fn full_recipe_is_deterministic_normalised_and_round_trips() {
        let r = full_recipe();
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(
            serde_json::from_str::<MoonWorldMapRecipe>(&json).unwrap(),
            r
        );
        let (a, b) = (bake(&r).unwrap(), bake(&r).unwrap());
        assert_eq!(a.elevation, b.elevation);
        assert_eq!(a.biomes, b.biomes);
        assert!(a.stats.secondaries > 0, "{:?}", a.stats);
        assert!(a.elevation.data().iter().all(|v| v.is_finite()));
        for v in a.biomes.data() {
            assert!((v.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn optional_sections_do_not_disturb_the_v1_population() {
        // Switching features on must not reshuffle the primary impacts.
        let mut rng_a = Rng(1);
        let mut rng_b = Rng(1);
        let base = small_recipe();
        let full = full_recipe();
        let mut base64 = base.clone();
        base64.face_resolution = full.face_resolution;
        let a = sample_impacts(&base64, &mut rng_a);
        let b = sample_impacts(&full, &mut rng_b);
        let primaries: Vec<_> = b.iter().filter(|i| !i.secondary).collect();
        assert_eq!(a.len(), primaries.len());
        for (x, y) in a.iter().zip(primaries) {
            assert_eq!(
                (x.age_ga, x.center, x.diameter_m),
                (y.age_ga, y.center, y.diameter_m)
            );
        }
    }

    #[test]
    fn lava_spills_beyond_the_basin_disc() {
        let mut disc = full_recipe();
        disc.lava = None;
        let (old, new) = (bake(&disc).unwrap(), bake(&full_recipe()).unwrap());
        assert!(
            new.stats.biome_fractions[MARE] > old.stats.biome_fractions[MARE],
            "{:?} vs {:?}",
            new.stats.biome_fractions,
            old.stats.biome_fractions
        );
    }

    #[test]
    fn terraces_add_benches_and_large_basins_get_rings() {
        let mut sh = shape(60_000.0, 15_000.0, 1.0);
        let smooth = sh.clone();
        sh.terraces = 4;
        sh.terrace_strength = 1.0;
        // Slope along the wall: terraced walls have flatter benches and
        // steeper scarps than the smooth wall.
        let slopes = |sh: &Shape| -> Vec<f64> {
            (0..200)
                .map(|k| {
                    let s = sh.floor + (1.0 - sh.floor) * (0.1 + 0.75 * k as f64 / 200.0);
                    (profile(s + 1e-4, sh, 2.5, 0.0) - profile(s, sh, 2.5, 0.0)) / 1e-4
                })
                .collect()
        };
        let (a, b) = (slopes(&smooth), slopes(&sh));
        let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
        let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        assert!(
            min(&b) < 0.5 * min(&a),
            "benches {} vs {}",
            min(&b),
            min(&a)
        );
        assert!(max(&b) > 1.5 * max(&a), "scarps {} vs {}", max(&b), max(&a));
        // Profiles stay monotone up the wall (up to the central peak's tail).
        assert!(b.iter().all(|v| *v >= -1e-2));

        let mut ring = shape(300_000.0, 15_000.0, 1.0);
        ring.peak_ring = 1.0;
        ring.outer_ring = 0.6 * ring.rim;
        assert!(profile(0.5, &ring, 2.5, 0.0) > profile(0.0, &ring, 2.5, 0.0) + 0.5 * ring.peak);
        assert!(
            profile(OUTER_RING_S + 0.02, &ring, 2.5, 0.0)
                > profile(OUTER_RING_S - 0.1, &ring, 2.5, 0.0)
        );
    }

    #[test]
    fn neighbours_cross_face_edges_symmetrically() {
        let n = 8;
        for k in 0..6 * n * n {
            for nb in neighbours(k, n) {
                assert_ne!(nb, k);
                assert!(neighbours(nb, n).contains(&k), "{k} -> {nb}");
            }
        }
    }

    #[test]
    fn bake_is_deterministic_and_weights_are_normalised() {
        let a = bake(&small_recipe()).unwrap();
        let b = bake(&small_recipe()).unwrap();
        assert_eq!(a.elevation, b.elevation);
        assert_eq!(a.biomes, b.biomes);
        assert!(a.stats.craters > 0 && a.stats.basins == 1 && a.stats.flooded_basins == 1);
        for v in a.biomes.data() {
            assert!((v.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        }
        assert!(a.elevation.data().iter().all(|v| v.is_finite()));
        assert!(a.stats.biome_fractions[MARE] > 0.0);
    }

    #[test]
    fn a_fresh_crater_has_a_bowl_and_a_raised_rim() {
        let sh = shape(5_000.0, 15_000.0, 1.0);
        assert!(profile(0.0, &sh, 2.5, 0.0) < -900.0, "Pike depth ≈ 0.2 D");
        assert!(profile(1.0, &sh, 2.5, 0.0) > 150.0);
        assert!(profile(2.6, &sh, 2.5, 0.0) == 0.0);
        // Degradation lowers the rim and fills the bowl.
        assert!(degraded(1.0, &sh, 2.5, 0.2, 0.0) < profile(1.0, &sh, 2.5, 0.0));
        assert!(degraded(0.0, &sh, 2.5, 0.2, 0.0) > profile(0.0, &sh, 2.5, 0.0));
    }

    #[test]
    fn world_scale_shrinks_relief_but_keeps_the_pattern() {
        let real = small_recipe();
        let mut game = real.clone();
        game.radius_m *= 0.1;
        game.world_scale = 0.1;
        let (a, b) = (bake(&real).unwrap(), bake(&game).unwrap());
        assert_eq!(a.biomes, b.biomes);
        for (x, y) in a.elevation.data().iter().zip(b.elevation.data()) {
            assert!((x * 0.1 - y).abs() <= 1e-3 * x.abs().max(1.0), "{x} {y}");
        }
    }

    #[test]
    fn mare_flooding_leaves_a_flat_floor() {
        let r = small_recipe();
        let map = bake(&r).unwrap();
        // Texels painted mostly mare share one level within millimetres
        // (younger craters may have hit it, so allow outliers below 20 %).
        let mare: Vec<f32> = map
            .elevation
            .data()
            .iter()
            .zip(map.biomes.data())
            .filter(|(_, w)| w[MARE] > 0.99)
            .map(|(h, _)| *h)
            .collect();
        assert!(!mare.is_empty());
        let mut sorted = mare.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        let flat = mare.iter().filter(|h| (*h - median).abs() < 0.01).count();
        assert!(flat * 5 >= mare.len() * 4, "{flat} of {}", mare.len());
    }
}
