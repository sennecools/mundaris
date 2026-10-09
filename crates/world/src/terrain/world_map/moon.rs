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
//! Biome weights (highland, mare, crater floor, crater rim and ejecta) are
//! painted by the same events and alpha-composited in time order, so younger
//! events cover older ones and boundaries blend.

use super::cube::{CubeMap, texel_directions};
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
                    && ok(self.biomes.ejecta_max_age_ga, 0.0, 4.5),
                "biomes",
            ),
        ];
        match checks.iter().find(|(pass, _)| !pass) {
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
struct Shape {
    depth: f64,
    rim: f64,
    /// Flat-floor radius as a fraction of the crater radius.
    floor: f64,
    peak: f64,
}

fn shape(diameter_m: f64, simple_complex_m: f64, depth_scale: f64) -> Shape {
    let d_km = diameter_m / 1000.0;
    if diameter_m < simple_complex_m {
        Shape {
            depth: 196.0 * d_km.powf(1.010) * depth_scale,
            rim: 36.0 * d_km.powf(1.014) * depth_scale,
            floor: 0.0,
            peak: 0.0,
        }
    } else {
        let depth = 1044.0 * d_km.powf(0.301) * depth_scale;
        Shape {
            depth,
            rim: 236.0 * d_km.powf(0.399) * depth_scale,
            floor: (0.187 * d_km.powf(0.249)).min(0.7),
            peak: 0.25 * depth,
        }
    }
}

/// Fresh relief relative to the pre-impact reference at radial distance `s`
/// (crater radii): bowl or flat floor with peak inside, rim and ejecta outside.
fn profile(s: f64, sh: &Shape, extent: f64) -> f64 {
    if s < 1.0 {
        let t = if s <= sh.floor {
            0.0
        } else {
            (s - sh.floor) / (1.0 - sh.floor)
        };
        let peak = if sh.peak > 0.0 {
            sh.peak * (-(s / 0.12).powi(2)).exp()
        } else {
            0.0
        };
        -sh.depth * (1.0 - t * t) + sh.rim * t * t + peak
    } else if s < extent {
        let t = ((s - 1.0) / (extent - 1.0)).clamp(0.0, 1.0);
        sh.rim * s.powi(-3) * (1.0 - t * t * (3.0 - 2.0 * t))
    } else {
        0.0
    }
}

/// `profile` blurred radially by σ (crater radii): a 7-tap Gaussian along the
/// radius, the age-softened shape.
fn degraded(s: f64, sh: &Shape, extent: f64, sigma: f64) -> f64 {
    if sigma < 1e-3 {
        return profile(s, sh, extent);
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
        .map(|&(k, w)| w * profile((s + k * sigma).abs(), sh, extent))
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
        impacts.push(Impact {
            age_ga,
            center,
            diameter_m,
            basin: false,
            harmonics: harmonics(rng),
        });
    }
    let b = &recipe.basins;
    for _ in 0..b.count {
        let diameter_m = b.d_min_m * (b.d_max_m / b.d_min_m).powf(rng.unit());
        let age_ga = b.age_min_ga + (b.age_max_ga - b.age_min_ga) * rng.unit();
        let center = rng.direction();
        impacts.push(Impact {
            age_ga,
            center,
            diameter_m,
            basin: true,
            harmonics: harmonics(rng),
        });
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

    // 2–3. Bombardment with mare flooding inserted at its age.
    let impacts = sample_impacts(recipe, &mut rng);
    let c = &recipe.craters;
    let mut stats = BakeStats::default();
    let mut emplaced: Vec<(Impact, f64)> = Vec::new(); // impact, rim crest level
    let mut flooded = false;
    let mut local: Vec<usize> = Vec::new();
    for imp in &impacts {
        if !flooded && imp.age_ga < recipe.mare.age_ga {
            stats.flooded_basins = flood(recipe, &dirs, &blocks, &emplaced, &mut h, &mut w);
            flooded = true;
        }
        let radius = 0.5 * imp.diameter_m;
        let sh = shape(
            imp.diameter_m,
            c.simple_complex_m,
            if imp.basin {
                recipe.basins.depth_scale
            } else {
                1.0
            },
        );
        let reach = c.ejecta_extent * radius * (1.0 + c.irregularity);
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
        let sigma = (2.0 * kappa * imp.age_ga * 1000.0).sqrt() / radius;
        // Tangent frame for azimuthal rim irregularity.
        let e1 = imp.center.any_orthonormal_vector();
        let e2 = imp.center.cross(e1);
        let paints = imp.diameter_m >= recipe.biomes.crater_min_d_m;
        // Heavily softened craters read as highland; ejecta only on young craters.
        let fresh = (1.0 - sigma / recipe.biomes.degraded_ratio.max(1e-9)).clamp(0.0, 1.0);
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
            let s = s_of(d) / (1.0 + c.irregularity * f);
            let relief = degraded(s, &sh, c.ejecta_extent, sigma);
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
        }
        let crest = reference + sh.rim;
        emplaced.push((*imp, crest));
        if imp.basin {
            stats.basins += 1;
        } else {
            stats.craters += 1;
        }
    }
    if !flooded {
        stats.flooded_basins = flood(recipe, &dirs, &blocks, &emplaced, &mut h, &mut w);
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
            },
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
        assert!(profile(0.0, &sh, 2.5) < -900.0, "Pike depth ≈ 0.2 D");
        assert!(profile(1.0, &sh, 2.5) > 150.0);
        assert!(profile(2.6, &sh, 2.5) == 0.0);
        // Degradation lowers the rim and fills the bowl.
        assert!(degraded(1.0, &sh, 2.5, 0.2) < profile(1.0, &sh, 2.5));
        assert!(degraded(0.0, &sh, 2.5, 0.2) > profile(0.0, &sh, 2.5));
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
