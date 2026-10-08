//! Seeded, view-independent Moon field with bounded spatial crater discovery.
//!
//! Heights and distances are metres. Gradients are tangent derivatives in
//! metres per unit direction, matching `TerrainSample`. The complete source is
//! f64; feature profiles are compact polynomials so a later field producer can
//! prepare the same immutable recipes in an anchored body-local domain.
use super::{
    GeologicalParameters, MoonCraterProfileDefinition, MoonFieldDefinition, SurfaceQueryWork,
    TerrainError, mix,
    query_context::{CachedFeature, CellKey, MOON_FIELDS_FEATURE_FAMILY, SurfaceQueryContext},
    unit,
};
use glam::{DMat3, DVec3};
use std::ops::{Add, Mul, Neg, Sub};

pub(super) const BASIN_COUNT: usize = 8;
pub(super) const LAYOUTS: usize = 2;
pub(super) const JITTER: f64 = 0.16;
pub(super) const SHELL: f64 = 0.30;
pub(super) const SUPPORT: f64 = 0.52;
pub const MOON_CRATER_BAND_BUDGETS_M: [f64; 3] = [
    MoonFieldDefinition::default_v1().bands[0].height_budget_m,
    MoonFieldDefinition::default_v1().bands[1].height_budget_m,
    MoonFieldDefinition::default_v1().bands[2].height_budget_m,
];
pub const MAX_MOON_CRATER_INPUTS: usize = 3 * LAYOUTS * 27;
const _: () = assert!(1.0 - JITTER - SHELL > SUPPORT);

/// One accepted compact crater recipe at a canonical body-local query point.
/// `q2` is the squared normalized chord distance prepared in f64 on the CPU.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MoonCraterInput {
    band: u8,
    q2: f64,
    freshness: f64,
    regional_strength: f64,
}
impl MoonCraterInput {
    pub fn band(self) -> u8 {
        self.band
    }
    pub fn q2(self) -> f64 {
        self.q2
    }
    pub fn freshness(self) -> f64 {
        self.freshness
    }
    pub fn regional_strength(self) -> f64 {
        self.regional_strength
    }
}

/// Bounded f64 recipe inputs for the experimental Moon field producer. It
/// carries no absolute or observer-relative position and is disposable render
/// data. Its replay returns terrain height and material weights only; callers
/// must separately query and apply the body's shape. This is not collision or
/// world authority; `SurfaceGenerator::evaluate_point` remains authoritative.
#[derive(Debug, Clone, PartialEq)]
pub struct MoonFieldPointInputs {
    global_height_m: f64,
    plains: f64,
    highlands: f64,
    regional: f64,
    material_emphasis: [f64; 4],
    definition: MoonFieldDefinition,
    craters: [MoonCraterInput; MAX_MOON_CRATER_INPUTS],
    crater_count: u16,
}
impl MoonFieldPointInputs {
    pub fn global_height_m(&self) -> f64 {
        self.global_height_m
    }
    pub fn plains(&self) -> f64 {
        self.plains
    }
    pub fn highlands(&self) -> f64 {
        self.highlands
    }
    pub fn regional(&self) -> f64 {
        self.regional
    }
    pub fn material_emphasis_factors(&self) -> [f64; 4] {
        self.material_emphasis
    }
    pub fn craters(&self) -> &[MoonCraterInput] {
        &self.craters[..self.crater_count as usize]
    }
    pub fn crater_count(&self) -> usize {
        self.crater_count as usize
    }

    pub(super) fn set_material_emphasis(
        &mut self,
        composition: f64,
        regional_contrast: f64,
        regional_signal: f64,
    ) {
        self.material_emphasis = std::array::from_fn(|index| {
            let emphasis = (index as f64 - 1.5) / 1.5;
            1.0 + emphasis * ((composition - 0.5) * 0.6 + regional_signal * regional_contrast * 0.3)
        });
    }

    /// Replay the exact scalar polynomial recipes in f64 for source agreement.
    /// The result is terrain relief plus material weights, with no shape term.
    pub fn evaluate_f64(&self) -> (f64, [f64; 4]) {
        let height = self.global_height_m
            + self
                .definition
                .bands
                .iter()
                .enumerate()
                .map(|(band, authored_band)| {
                    let budget = authored_band.height_budget_m;
                    let mut numerator = 0.0;
                    let mut denominator = 1.0;
                    for crater in self.craters() {
                        if crater.band as usize != band {
                            continue;
                        }
                        let window = (1.0 - crater.q2).powi(3);
                        numerator += window
                            * crater_profile_f64(
                                *crater,
                                self.regional,
                                self.definition.crater_profile,
                            )
                            * budget;
                        denominator += window;
                    }
                    numerator / denominator
                })
                .sum::<f64>();
        let mut impact_weight = 0.0;
        for (band, authored_band) in self.definition.bands.iter().enumerate() {
            let budget = authored_band.height_budget_m;
            let mut numerator = 0.0;
            let mut denominator = 1.0;
            for crater in self.craters() {
                if crater.band as usize != band {
                    continue;
                }
                let window = (1.0 - crater.q2).powi(3);
                numerator += window
                    * crater_profile_f64(*crater, self.regional, self.definition.crater_profile)
                    * budget;
                denominator += window;
            }
            impact_weight += (numerator / denominator).abs() / (budget + 1.0);
        }
        let mut weights = [
            (0.62 + 0.30 * self.plains - 0.12 * impact_weight).clamp(0.0, 1.0),
            (0.22 + 0.54 * self.highlands - 0.15 * impact_weight).clamp(0.0, 1.0),
            (0.08 + 0.78 * self.plains).clamp(0.0, 1.0),
            (0.08 + 0.75 * impact_weight).clamp(0.0, 1.0),
        ];
        let total = weights.iter().sum::<f64>();
        weights.iter_mut().for_each(|weight| *weight /= total);
        weights
            .iter_mut()
            .zip(self.material_emphasis)
            .for_each(|(weight, factor)| *weight *= factor);
        let total = weights.iter().sum::<f64>();
        weights.iter_mut().for_each(|weight| *weight /= total);
        (height, weights)
    }

    /// Exact f32 mirror of the compact recipe evaluator after quantizing every
    /// prepared scalar input independently. This returns terrain relief and
    /// material weights only, with no shape term, and is an experimental
    /// producer comparison rather than world authority.
    pub fn evaluate_f32(&self) -> MoonFieldEvaluationF32 {
        let global = self.global_height_m as f32;
        let plains = self.plains as f32;
        let highlands = self.highlands as f32;
        let regional = self.regional as f32;
        let mut band_heights = [0.0f32; 3];
        for crater in self.craters() {
            let q2 = crater.q2 as f32;
            let window = (1.0 - q2).powi(3);
            let profile = crater_profile_f32(
                q2,
                crater.freshness as f32,
                crater.regional_strength as f32,
                regional,
                self.definition.crater_profile,
            );
            band_heights[crater.band as usize] += window
                * profile
                * self.definition.bands[crater.band as usize].height_budget_m as f32;
        }
        let mut band_denominators = [1.0f32; 3];
        for crater in self.craters() {
            let q2 = crater.q2 as f32;
            band_denominators[crater.band as usize] += (1.0 - q2).powi(3);
        }
        band_heights
            .iter_mut()
            .zip(band_denominators)
            .for_each(|(height, denominator)| *height /= denominator);
        let height = global + band_heights.iter().sum::<f32>();
        let impact_weight = band_heights
            .iter()
            .zip(self.definition.bands)
            .map(|(band, config)| band.abs() / (config.height_budget_m as f32 + 1.0))
            .sum::<f32>();
        let mut weights = [
            (0.62 + 0.30 * plains - 0.12 * impact_weight).clamp(0.0, 1.0),
            (0.22 + 0.54 * highlands - 0.15 * impact_weight).clamp(0.0, 1.0),
            (0.08 + 0.78 * plains).clamp(0.0, 1.0),
            (0.08 + 0.75 * impact_weight).clamp(0.0, 1.0),
        ];
        let total = weights.iter().sum::<f32>();
        weights.iter_mut().for_each(|weight| *weight /= total);
        weights
            .iter_mut()
            .zip(self.material_emphasis)
            .for_each(|(weight, factor)| *weight *= factor as f32);
        let total = weights.iter().sum::<f32>();
        weights.iter_mut().for_each(|weight| *weight /= total);
        MoonFieldEvaluationF32 {
            height_m: height,
            material_weights: weights,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoonFieldEvaluationF32 {
    pub height_m: f32,
    pub material_weights: [f32; 4],
}

#[derive(Debug, Clone, Copy)]
struct Differential {
    value: f64,
    gradient: DVec3,
}

impl Differential {
    fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: DVec3::ZERO,
        }
    }
    fn variable(value: f64, gradient: DVec3) -> Self {
        Self { value, gradient }
    }
    fn square(self) -> Self {
        self * self
    }
    fn cube(self) -> Self {
        self * self * self
    }
    fn smooth01(self) -> Self {
        if self.value <= 0.0 {
            Self::constant(0.0)
        } else if self.value >= 1.0 {
            Self::constant(1.0)
        } else {
            let t2 = self.value * self.value;
            let value = t2 * (3.0 - 2.0 * self.value);
            Self::variable(
                value,
                self.gradient * (6.0 * self.value * (1.0 - self.value)),
            )
        }
    }
    fn range(self, low: f64, high: f64) -> Self {
        ((self - Self::constant(low)) * (1.0 / (high - low))).smooth01()
    }
}
impl Add for Differential {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::variable(self.value + rhs.value, self.gradient + rhs.gradient)
    }
}
impl Sub for Differential {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::variable(self.value - rhs.value, self.gradient - rhs.gradient)
    }
}
impl Mul for Differential {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::variable(
            self.value * rhs.value,
            self.gradient * rhs.value + rhs.gradient * self.value,
        )
    }
}
impl Mul<f64> for Differential {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self::variable(self.value * rhs, self.gradient * rhs)
    }
}
impl Add<f64> for Differential {
    type Output = Self;
    fn add(self, rhs: f64) -> Self {
        self + Self::constant(rhs)
    }
}
impl Sub<f64> for Differential {
    type Output = Self;
    fn sub(self, rhs: f64) -> Self {
        self - Self::constant(rhs)
    }
}
impl Neg for Differential {
    type Output = Self;
    fn neg(self) -> Self {
        Self::variable(-self.value, -self.gradient)
    }
}

#[derive(Debug, Clone, Copy)]
struct CraterRecipe {
    key: u64,
    center: DVec3,
    freshness: f64,
    regional_strength: f64,
}

fn cache_recipe(recipe: CraterRecipe, edge_m: f64, band: usize) -> CachedFeature {
    CachedFeature {
        center: recipe.center,
        key: recipe.key,
        lineage_key: recipe.key,
        edge_m,
        level: band,
    }
}

fn uncache_recipe(
    feature: CachedFeature,
    highlands: f64,
    regional_strength: super::AffineRandomRange,
) -> CraterRecipe {
    CraterRecipe {
        key: feature.key,
        center: feature.center,
        freshness: unit(feature.key ^ 0x4d4f_4f4e_4652_4553),
        regional_strength: regional_strength.sample(highlands).clamp(0.0, 1.0),
    }
}

#[derive(Debug, Clone)]
pub(super) struct MoonFieldsV1 {
    pub(super) radius_m: f64,
    pub(super) seed: u64,
    pub(super) parameters: GeologicalParameters,
    pub(super) axes: [DVec3; 4],
    pub(super) basins: [DVec3; BASIN_COUNT],
    pub(super) rotation: DMat3,
    bound_m: f64,
    pub(super) definition: MoonFieldDefinition,
}

impl MoonFieldsV1 {
    pub(super) fn new(
        parameters: GeologicalParameters,
        seed: u64,
        radius_m: f64,
        definition: MoonFieldDefinition,
    ) -> Result<Self, TerrainError> {
        if !radius_m.is_finite() || radius_m <= 0.0 || radius_m > 1.0e8 {
            return Err(TerrainError::InvalidRadius);
        }
        parameters.validate()?;
        definition.validate()?;
        let seed = mix(seed ^ 0x4d4f_4f4e_4649_454c);
        let vector = |key: u64| {
            let raw = DVec3::new(
                unit(key ^ 0x4158_4953_0000_0001) * 2.0 - 1.0,
                unit(key ^ 0x4158_4953_0000_0002) * 2.0 - 1.0,
                unit(key ^ 0x4158_4953_0000_0003) * 2.0 - 1.0,
            );
            let length_squared = raw.length_squared();
            if length_squared > f64::MIN_POSITIVE {
                raw / length_squared.sqrt()
            } else {
                // A seeded vector can theoretically land exactly at zero.
                // Give that rare seed a deterministic, finite axis.
                DVec3::X
            }
        };
        let axes = std::array::from_fn(|i| vector(seed ^ (i as u64).wrapping_mul(0x9e37_79b9)));
        let basins = std::array::from_fn(|i| vector(seed ^ 0x4241_5349_4e00_0000 ^ i as u64));
        let rotation = DMat3::from_rotation_z(parameters.orientation_radians);
        // The global polynomial is bounded by 0.64 relief units. Each compact
        // crater band is a normalized weighted average within its budget.
        let band_budget_total = definition
            .bands
            .iter()
            .map(|band| band.height_budget_m)
            .sum::<f64>();
        let bound_m =
            (radius_m * parameters.relief_fraction * 2.4 + band_budget_total * 1.3).next_up();
        if !bound_m.is_finite() || bound_m >= radius_m * 0.1 {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            radius_m,
            seed,
            parameters,
            axes,
            basins,
            rotation,
            bound_m,
            definition,
        })
    }

    pub(super) fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }

    pub(super) fn material_modulation(&self, direction: DVec3) -> f64 {
        let point = self.rotation.transpose() * direction.normalize();
        self.global(point).regional.value.clamp(-1.0, 1.0)
    }

    pub(super) fn prepare(&self, direction: DVec3) -> Result<MoonFieldPointInputs, TerrainError> {
        let point = self.rotation.transpose() * direction.normalize();
        let global = self.global(point);
        let mut inputs = MoonFieldPointInputs {
            global_height_m: global.height.value,
            plains: global.plains,
            highlands: global.highlands,
            regional: global.regional.value,
            material_emphasis: [1.0; 4],
            definition: self.definition,
            craters: [MoonCraterInput::default(); MAX_MOON_CRATER_INPUTS],
            crater_count: 0,
        };
        for (band, band_definition) in self.definition.bands.iter().enumerate() {
            let edge = band_definition.edge_m;
            let support_m = edge * SUPPORT;
            for layout in 0..LAYOUTS {
                let shift = if layout == 0 {
                    DVec3::ZERO
                } else {
                    DVec3::new(0.31, -0.27, 0.19)
                };
                let cell = (point * (self.radius_m / edge) - shift)
                    .floor()
                    .to_array()
                    .map(|x| x as i64);
                for x in cell[0] - 1..=cell[0] + 1 {
                    for y in cell[1] - 1..=cell[1] + 1 {
                        for z in cell[2] - 1..=cell[2] + 1 {
                            let Some(recipe) = self.recipe(x, y, z, band, layout, edge) else {
                                continue;
                            };
                            let delta = point * self.radius_m - recipe.center;
                            let q2 = delta.length_squared() / support_m.powi(2);
                            if q2 >= 1.0 {
                                continue;
                            }
                            let index = inputs.crater_count as usize;
                            let Some(slot) = inputs.craters.get_mut(index) else {
                                return Err(TerrainError::InvalidConfig);
                            };
                            *slot = MoonCraterInput {
                                band: band as u8,
                                q2,
                                freshness: recipe.freshness,
                                regional_strength: recipe.regional_strength,
                            };
                            inputs.crater_count += 1;
                        }
                    }
                }
            }
        }
        Ok(inputs)
    }

    pub(super) fn evaluate(
        &self,
        n: DVec3,
        context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<(f64, DVec3, [f64; 4], SurfaceQueryWork), TerrainError> {
        self.evaluate_weighted(n, context, None)
    }

    /// Band-limited derived evaluation. Each crater band contribution is scaled
    /// by its footprint weight and skipped when that weight is zero. `None`
    /// is the complete authoritative evaluation.
    pub(super) fn evaluate_weighted(
        &self,
        n: DVec3,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
        band_weights: Option<[f64; 3]>,
    ) -> Result<(f64, DVec3, [f64; 4], SurfaceQueryWork), TerrainError> {
        let n = self.rotation.transpose() * n;
        let global = self.global(n);
        let mut total = global.height;
        let mut impact_weight = 0.0;
        let mut work = SurfaceQueryWork::default();
        for (band, band_definition) in self.definition.bands.iter().enumerate() {
            let weight = band_weights.map_or(1.0, |weights| weights[band]);
            if weight <= 0.0 {
                continue;
            }
            let mut contribution =
                self.evaluate_band(n, band, global.regional, &mut work, context.as_deref_mut())?;
            if band_weights.is_some() {
                contribution = contribution * weight;
            }
            total = total + contribution;
            impact_weight += contribution.value.abs() / (band_definition.height_budget_m + 1.0);
        }
        let gradient = self.rotation * total.gradient;
        let regolith = (0.62 + 0.30 * global.plains - 0.12 * impact_weight).clamp(0.0, 1.0);
        let substrate = (0.22 + 0.54 * global.highlands - 0.15 * impact_weight).clamp(0.0, 1.0);
        let basalt = (0.08 + 0.78 * global.plains).clamp(0.0, 1.0);
        let ejecta = (0.08 + 0.75 * impact_weight).clamp(0.0, 1.0);
        Ok((
            total.value,
            gradient,
            normalize_weights([regolith, substrate, basalt, ejecta]),
            work,
        ))
    }

    /// Per-basin `[scale, bowl depth]` exactly as sampled by `global`.
    pub(super) fn basin_parameters(&self) -> [[f64; 2]; BASIN_COUNT] {
        std::array::from_fn(|index| {
            [
                self.definition
                    .basin_scale
                    .sample(unit(self.seed ^ index as u64 ^ 0x4241_5349_4e53)),
                self.definition
                    .basin_bowl_depth
                    .sample(unit(self.seed ^ index as u64)),
            ]
        })
    }

    /// The 64-bit hash salt of one crater band/layout lattice.
    pub(super) fn lattice_salt(&self, band: usize, layout: usize) -> u64 {
        self.seed ^ (band as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ layout as u64
    }

    fn global(&self, n: DVec3) -> GlobalSample {
        let x = Differential::variable(n.dot(self.axes[0]), self.axes[0]);
        let y = Differential::variable(n.dot(self.axes[1]), self.axes[1]);
        let z = Differential::variable(n.dot(self.axes[2]), self.axes[2]);
        let cross = Differential::variable(n.dot(self.axes[3]), self.axes[3]);
        let structure_weights = self.definition.structure_weights;
        let structure = x.square() * structure_weights[0]
            + y * z * structure_weights[1]
            + cross.square() * structure_weights[2]
            + structure_weights[3];
        let mut basin = Differential::constant(0.0);
        for (index, center) in self.basins.iter().enumerate() {
            let dot = n.dot(*center);
            let q = Differential::variable(1.0 - dot, -*center);
            let scale = self
                .definition
                .basin_scale
                .sample(unit(self.seed ^ index as u64 ^ 0x4241_5349_4e53));
            let bowl = (Differential::constant(1.0) - q * (1.0 / scale)).range(0.0, 1.0);
            let bowl = bowl.square();
            let rim = (q * (1.0 / scale)).range(0.62, 0.95);
            basin = basin
                - bowl
                    * self
                        .definition
                        .basin_bowl_depth
                        .sample(unit(self.seed ^ index as u64))
                + rim * bowl * self.definition.basin_rim_strength;
        }
        let plains = (structure + self.definition.plains_offset)
            .range(self.definition.plains_low, self.definition.plains_high);
        let highlands = Differential::constant(1.0) - plains;
        let relief_weights = self.definition.global_relief_weights;
        let height = structure
            * (self.radius_m * self.parameters.relief_fraction * relief_weights[0])
            + basin * (self.radius_m * self.parameters.relief_fraction * relief_weights[1])
            - plains * (self.radius_m * self.parameters.relief_fraction * relief_weights[2]);
        GlobalSample {
            height,
            regional: structure + basin,
            plains: plains.value,
            highlands: highlands.value,
        }
    }

    fn evaluate_band(
        &self,
        point: DVec3,
        band: usize,
        regional: Differential,
        work: &mut SurfaceQueryWork,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<Differential, TerrainError> {
        let edge = self.definition.bands[band].edge_m;
        let support_m = edge * SUPPORT;
        let mut numerator = Differential::constant(0.0);
        let mut denominator = Differential::constant(1.0);
        for layout in 0..LAYOUTS {
            let shift = if layout == 0 {
                DVec3::ZERO
            } else {
                DVec3::new(0.31, -0.27, 0.19)
            };
            let center_cell = (point * (self.radius_m / edge) - shift)
                .floor()
                .to_array()
                .map(|x| x as i64);
            for x in center_cell[0] - 1..=center_cell[0] + 1 {
                for y in center_cell[1] - 1..=center_cell[1] + 1 {
                    for z in center_cell[2] - 1..=center_cell[2] + 1 {
                        work.cells_visited = work.cells_visited.saturating_add(1);
                        let recipe = if let Some(cache) = context.as_deref_mut() {
                            cache
                                .feature(
                                    CellKey::new(MOON_FIELDS_FEATURE_FAMILY, band, layout, x, y, z),
                                    || {
                                        self.recipe(x, y, z, band, layout, edge)
                                            .map(|recipe| cache_recipe(recipe, edge, band))
                                    },
                                )
                                .map(|feature| {
                                    uncache_recipe(
                                        feature,
                                        self.global(feature.center / self.radius_m).highlands,
                                        self.definition.crater_regional_strength,
                                    )
                                })
                        } else {
                            self.recipe(x, y, z, band, layout, edge)
                        };
                        let Some(recipe) = recipe else {
                            continue;
                        };
                        work.candidate_features = work.candidate_features.saturating_add(1);
                        let delta = point * self.radius_m - recipe.center;
                        let q2 = Differential::variable(
                            delta.length_squared() / support_m.powi(2),
                            delta * (2.0 * self.radius_m / support_m.powi(2)),
                        );
                        if q2.value >= 1.0 {
                            continue;
                        }
                        let accepted = work.accepted_features.get_or_insert(0);
                        *accepted = accepted.saturating_add(1);
                        let window = (Differential::constant(1.0) - q2).cube();
                        let profile = self.profile(q2, recipe, regional);
                        numerator = numerator
                            + window * profile * self.definition.bands[band].height_budget_m;
                        denominator = denominator + window;
                    }
                }
            }
        }
        Ok(numerator * reciprocal(denominator))
    }

    fn recipe(
        &self,
        x: i64,
        y: i64,
        z: i64,
        band: usize,
        layout: usize,
        edge: f64,
    ) -> Option<CraterRecipe> {
        let key = cell_hash(
            self.seed ^ (band as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ layout as u64,
            x,
            y,
            z,
        );
        let shift = if layout == 0 {
            DVec3::ZERO
        } else {
            DVec3::new(0.31, -0.27, 0.19)
        };
        let jitter = DVec3::new(unit(key ^ 1), unit(key ^ 2), unit(key ^ 3)) * 2.0 - DVec3::ONE;
        let raw = (DVec3::new(x as f64, y as f64, z as f64)
            + DVec3::splat(0.5)
            + shift
            + jitter * JITTER)
            * edge;
        let distance = raw.length();
        if raw.length_squared() <= f64::MIN_POSITIVE
            || (distance - self.radius_m).abs() > edge * SHELL
        {
            return None;
        }
        let center = raw * (self.radius_m / distance);
        Some(CraterRecipe {
            key,
            center,
            freshness: unit(key ^ 0x4d4f_4f4e_4652_4553),
            regional_strength: self
                .definition
                .crater_regional_strength
                .sample(self.global(center / self.radius_m).highlands)
                .clamp(0.0, 1.0),
        })
    }

    fn profile(
        &self,
        q2: Differential,
        recipe: CraterRecipe,
        regional: Differential,
    ) -> Differential {
        let one = Differential::constant(1.0);
        let inside = one - q2;
        let fresh = 1.0 - recipe.freshness;
        let degraded = recipe.freshness;
        let config = self.definition.crater_profile;
        let bowl = -(inside.square())
            * (config.bowl_base + config.bowl_freshness * fresh)
            * recipe.regional_strength;
        // 256/27 normalizes q(1-q)^3 to unit height at q=1/4.
        let rim = q2
            * inside.cube()
            * (256.0 / 27.0)
            * (config.rim_base + config.rim_freshness * fresh)
            * recipe.regional_strength;
        let ejecta_zone = q2.range(0.42, 0.98);
        let ejecta =
            ejecta_zone * inside.square() * (config.ejecta_base + config.ejecta_freshness * fresh);
        let degradation = (regional + config.degradation_offset)
            .range(config.degradation_low, config.degradation_high);
        let subdued = one - degradation * (config.degradation_strength * degraded);
        (bowl + rim + ejecta) * subdued
    }
}

#[derive(Debug, Clone, Copy)]
struct GlobalSample {
    height: Differential,
    regional: Differential,
    plains: f64,
    highlands: f64,
}

fn reciprocal(value: Differential) -> Differential {
    let inv = 1.0 / value.value;
    Differential::variable(inv, value.gradient * (-inv * inv))
}

fn crater_profile_f64(
    recipe: MoonCraterInput,
    regional: f64,
    config: MoonCraterProfileDefinition,
) -> f64 {
    let q2 = recipe.q2;
    let inside = 1.0 - q2;
    let fresh = 1.0 - recipe.freshness;
    let degraded = recipe.freshness;
    let bowl = -(inside * inside)
        * (config.bowl_base + config.bowl_freshness * fresh)
        * recipe.regional_strength;
    let rim = q2
        * inside.powi(3)
        * (256.0 / 27.0)
        * (config.rim_base + config.rim_freshness * fresh)
        * recipe.regional_strength;
    let ejecta_zone = smooth_range_f64(q2, 0.42, 0.98);
    let ejecta =
        ejecta_zone * inside * inside * (config.ejecta_base + config.ejecta_freshness * fresh);
    let degradation = smooth_range_f64(
        regional + config.degradation_offset,
        config.degradation_low,
        config.degradation_high,
    );
    (bowl + rim + ejecta) * (1.0 - degradation * (config.degradation_strength * degraded))
}

fn crater_profile_f32(
    q2: f32,
    freshness: f32,
    regional_strength: f32,
    regional: f32,
    config: MoonCraterProfileDefinition,
) -> f32 {
    let inside = 1.0f32 - q2;
    let fresh = 1.0f32 - freshness;
    let bowl = -(inside * inside)
        * (config.bowl_base as f32 + config.bowl_freshness as f32 * fresh)
        * regional_strength;
    let rim = q2
        * inside.powi(3)
        * (256.0 / 27.0)
        * (config.rim_base as f32 + config.rim_freshness as f32 * fresh)
        * regional_strength;
    let ejecta = smooth_range_f32(q2, 0.42, 0.98)
        * inside
        * inside
        * (config.ejecta_base as f32 + config.ejecta_freshness as f32 * fresh);
    let degradation = smooth_range_f32(
        regional + config.degradation_offset as f32,
        config.degradation_low as f32,
        config.degradation_high as f32,
    );
    (bowl + rim + ejecta) * (1.0 - degradation * (config.degradation_strength as f32 * freshness))
}

fn smooth_range_f64(value: f64, low: f64, high: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn smooth_range_f32(value: f32, low: f32, high: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn normalize_weights(mut weights: [f64; 4]) -> [f64; 4] {
    let total = weights.iter().sum::<f64>();
    if total > 0.0 && total.is_finite() {
        weights.iter_mut().for_each(|weight| *weight /= total);
    } else {
        weights = [0.25; 4];
    }
    weights
}

fn cell_hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    mix(seed ^ mix(x as u64) ^ mix((y as u64).rotate_left(21)) ^ mix((z as u64).rotate_left(42)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{
        ShapeDefinition, SurfaceAlgorithm, SurfaceDefinition, SurfaceMaterialDefinition,
        SurfaceMaterialVersion, SurfaceTerrainDefinition, TerrainIdentity, TerrainSeed,
    };
    use mundaris_math::{Direction3, surface::SurfaceLocation};

    fn locations() -> Vec<SurfaceLocation> {
        [
            DVec3::X,
            DVec3::Y,
            DVec3::Z,
            -DVec3::X,
            -DVec3::Y,
            -DVec3::Z,
            DVec3::new(1.0, 1.0, 0.0),
            DVec3::new(-1.0, 1.0, 1.0),
            DVec3::new(0.31, -0.57, 0.76),
        ]
        .into_iter()
        .map(|direction| SurfaceLocation::new(Direction3::try_new(direction).unwrap()))
        .collect()
    }

    #[test]
    fn seeds_scales_and_query_context_are_deterministic_and_bounded() {
        assert_ne!(
            SurfaceAlgorithm::MoonFieldsV1.code(),
            SurfaceAlgorithm::RockyV5.code()
        );
        assert_eq!(SurfaceAlgorithm::MoonFieldsV1.name(), "MoonFieldsV1");
        assert_eq!(
            crate::terrain::SurfaceMaterialVersion::for_algorithm(SurfaceAlgorithm::MoonFieldsV1),
            crate::terrain::SurfaceMaterialVersion::RockyV2,
        );
        for seed in [2, 7, 19, 43] {
            for radius in [109_081.776_8, 1_737_400.0] {
                let definition = SurfaceDefinition::generated(
                    TerrainIdentity(0x4d4f_4f4e_0000_0001),
                    TerrainSeed(seed),
                    SurfaceAlgorithm::MoonFieldsV1,
                );
                let generator = super::super::SurfaceGenerator::new(&definition, radius).unwrap();
                let points = locations();
                let expected = points
                    .iter()
                    .map(|point| generator.evaluate_point(*point).unwrap())
                    .collect::<Vec<_>>();
                let repeated_generator =
                    super::super::SurfaceGenerator::new(&definition, radius).unwrap();
                for (point, expected_sample) in points.iter().zip(&expected) {
                    assert_eq!(
                        &repeated_generator.evaluate_point(*point).unwrap(),
                        expected_sample
                    );
                    let prepared = generator.prepare_moon_field_point(*point).unwrap();
                    assert!(prepared.crater_count() <= MAX_MOON_CRATER_INPUTS);
                    assert!(prepared.craters().iter().all(|recipe| {
                        recipe.q2().is_finite()
                            && (0.0..1.0).contains(&recipe.q2())
                            && recipe.freshness().is_finite()
                            && recipe.regional_strength().is_finite()
                    }));
                    let (height_f64, weights_f64) = prepared.evaluate_f64();
                    assert!((height_f64 - expected_sample.terrain().height_m()).abs() < 1.0e-8);
                    assert!(
                        weights_f64
                            .iter()
                            .zip(expected_sample.material_weights())
                            .all(|(a, b)| (a - b).abs() < 1.0e-12)
                    );
                    let mirrored = prepared.evaluate_f32();
                    assert!((mirrored.height_m as f64 - height_f64).abs() < 0.05);
                    assert!(
                        mirrored
                            .material_weights
                            .iter()
                            .zip(weights_f64)
                            .all(|(a, b)| (*a as f64 - b).abs() < 2.0e-5)
                    );
                }
                let mut context = generator.query_context();
                let first = context.evaluate_point(points[0]).unwrap();
                assert_eq!(first, expected[0]);
                for (point, expected_sample) in points.iter().rev().zip(expected.iter().rev()) {
                    let actual = context.evaluate_point(*point).unwrap();
                    assert_eq!(&actual, expected_sample, "seed={seed} radius={radius}m");
                    assert!(
                        actual.terrain().height_m().abs()
                            <= generator.conservative_absolute_height_bound_m()
                    );
                    assert!(
                        actual
                            .terrain()
                            .tangent_gradient_m_per_unit_direction()
                            .is_finite()
                    );
                    assert!(actual.normal().is_finite());
                    let weights = actual.material_weights();
                    assert!(
                        weights
                            .iter()
                            .all(|weight| weight.is_finite() && (0.0..=1.0).contains(weight))
                    );
                    assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
                    assert!(actual.work().cells_visited <= 3 * 2 * 27);
                }
                assert!(context.stats().feature_hits > 0);
            }
        }
    }

    #[test]
    fn tiny_direction_steps_remain_continuous_across_feature_cells() {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4d4f_4f4e_0000_0001),
            TerrainSeed(19),
            SurfaceAlgorithm::MoonFieldsV1,
        );
        let generator = super::super::SurfaceGenerator::new(&definition, 109_081.776_8).unwrap();
        for direction in [
            DVec3::new(0.37, 0.82, -0.43).normalize(),
            DVec3::new(-0.61, 0.28, 0.74).normalize(),
            DVec3::new(0.09, -0.99, 0.12).normalize(),
        ] {
            let tangent = direction.cross(DVec3::Y).normalize_or_zero();
            let tangent = if tangent.length_squared() > 0.0 {
                tangent
            } else {
                direction.cross(DVec3::X).normalize()
            };
            let epsilon = 1.0e-9;
            let before = (direction - tangent * epsilon).normalize();
            let after = (direction + tangent * epsilon).normalize();
            let sample = |n| {
                generator
                    .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                    .unwrap()
            };
            let a = sample(before);
            let b = sample(after);
            assert!((a.terrain().height_m() - b.terrain().height_m()).abs() < 0.1);
        }
    }

    #[test]
    fn regional_contrast_affects_materials_and_prepared_height_excludes_shape() {
        let seed = TerrainSeed(19);
        let identity = TerrainIdentity(0x4d4f_4f4e_0000_0001);
        let generated =
            SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonFieldsV1);
        let terrain = SurfaceTerrainDefinition::generated(SurfaceAlgorithm::MoonFieldsV1, seed);
        let shape = ShapeDefinition::ellipsoid([1.0, 0.8, 1.2]).unwrap();
        let make_generator = |contrast| {
            let material = SurfaceMaterialDefinition::new(
                SurfaceMaterialVersion::RockyV2,
                generated.material().composition(),
                contrast,
            )
            .unwrap();
            let definition = SurfaceDefinition::new(
                identity,
                seed,
                shape,
                terrain,
                material,
                generated.atmosphere(),
            )
            .unwrap();
            super::super::SurfaceGenerator::new(&definition, 109_081.776_8).unwrap()
        };
        let no_contrast = make_generator(0.0);
        let contrast = make_generator(1.0);
        let mut changed = false;
        for direction in locations() {
            let prepared = contrast.prepare_moon_field_point(direction).unwrap();
            if prepared.regional().abs() < 0.05 {
                continue;
            }
            let contrasted = contrast.evaluate_point(direction).unwrap();
            let baseline = no_contrast.evaluate_point(direction).unwrap();
            assert!(
                contrasted
                    .material_weights()
                    .iter()
                    .zip(baseline.material_weights())
                    .any(|(a, b)| (a - b).abs() > 1.0e-7),
                "regional contrast should modulate weights at regional signal {}",
                prepared.regional()
            );
            let (prepared_height, prepared_weights) = prepared.evaluate_f64();
            assert!((prepared_height - contrasted.terrain().height_m()).abs() < 1.0e-8);
            assert!(
                prepared_weights
                    .iter()
                    .zip(contrasted.material_weights())
                    .all(|(a, b)| (a - b).abs() < 1.0e-12)
            );
            assert!((contrasted.shape().radius_m() - 109_081.776_8).abs() > 1.0);
            // Prepared replay is terrain relief only; shape remains a separate
            // sample term that the caller must apply independently.
            assert!(
                (contrasted.radius_m()
                    - contrasted.shape().radius_m()
                    - contrasted.terrain().height_m())
                .abs()
                    < 1.0e-8
            );
            changed = true;
            break;
        }
        assert!(
            changed,
            "test directions should include nonzero regional signal"
        );
    }

    #[test]
    fn rotated_analytic_gradient_matches_directional_height_difference() {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4d4f_4f4e_0000_0001),
            TerrainSeed(7),
            SurfaceAlgorithm::MoonFieldsV1,
        );
        let generator = super::super::SurfaceGenerator::new(&definition, 109_081.776_8).unwrap();
        let direction = DVec3::new(0.37, -0.66, 0.65).normalize();
        let tangent = direction.cross(DVec3::new(0.1, 0.2, 0.9)).normalize();
        let location = |n| SurfaceLocation::new(Direction3::try_new(n).unwrap());
        let center = generator.evaluate_point(location(direction)).unwrap();
        let epsilon = 1.0e-7;
        let before = generator
            .evaluate_point(location((direction - tangent * epsilon).normalize()))
            .unwrap()
            .terrain()
            .height_m();
        let after = generator
            .evaluate_point(location((direction + tangent * epsilon).normalize()))
            .unwrap()
            .terrain()
            .height_m();
        let numeric = (after - before) / (2.0 * epsilon);
        let analytic = center
            .terrain()
            .tangent_gradient_m_per_unit_direction()
            .dot(tangent);
        assert!(
            (numeric - analytic).abs() <= analytic.abs().max(1.0) * 2.0e-3,
            "numeric={numeric}, analytic={analytic}"
        );
    }
}
