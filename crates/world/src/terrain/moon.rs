//! Independent moon-like body-fixed terrain algorithms. Version codes use a
//! separate namespace from the legacy terrain generators.
use super::{TerrainError, TerrainIdentity, TerrainSample, TerrainSeed};
use crate::terrain::SurfaceQueryContext;
use glam::{DMat3, DVec3};
use mundaris_math::{Direction3, noise::gradient_noise, surface::SurfaceLocation};
mod ancient;

const MIN_CELL_EDGE_M: f64 = 8.0;
const MAX_RADIUS_M: f64 = 1.0e8;
const CELL_JITTER: f64 = 0.2;
// .65 support + .12 shell projection < .8, the minimum raw-center
// distance from a cell outside the one-cell halo with .2 jitter.
const FEATURE_SUPPORT_FRACTION: f64 = 0.65;
const REGOLITH_RELIEF_BOUND_M: f64 = 1.5;
const REGOLITH_WAVELENGTH_FRACTION: f64 = 0.00005;
const FEATURE_SHELL_FILTER_FRACTION: f64 = 0.12;

/// Moon terrain version codes are local to this independent algorithm family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MoonTerrainVersion {
    MoonLikeV1,
    /// Dense impact history and structured ancient highlands.
    MoonLikeV2,
}
impl MoonTerrainVersion {
    pub const fn code(self) -> u32 {
        match self {
            Self::MoonLikeV1 => 1,
            Self::MoonLikeV2 => 2,
        }
    }
}

/// Validated relief and impact scale controls, expressed as fractions of body
/// radius so one definition adapts coherently to differently sized moons.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoonTerrainConfig {
    basin_relief_fraction: f64,
    highland_relief_fraction: f64,
    impact_scale_fractions: [f64; 3],
    impact_relief_fractions: [f64; 3],
    absolute_relief_fraction: f64,
}
impl MoonTerrainConfig {
    pub fn new(
        basin_relief_fraction: f64,
        highland_relief_fraction: f64,
        impact_scale_fractions: [f64; 3],
        impact_relief_fractions: [f64; 3],
    ) -> Result<Self, TerrainError> {
        if !basin_relief_fraction.is_finite()
            || basin_relief_fraction < 0.0
            || !highland_relief_fraction.is_finite()
            || highland_relief_fraction < 0.0
            || impact_scale_fractions
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0 || *v > 1.0)
            || impact_relief_fractions
                .iter()
                .any(|v| !v.is_finite() || *v < 0.0)
            || impact_scale_fractions[0] <= impact_scale_fractions[1]
            || impact_scale_fractions[1] <= impact_scale_fractions[2]
        {
            return Err(TerrainError::InvalidConfig);
        }
        let impacts = impact_relief_fractions.iter().sum::<f64>();
        let absolute_relief_fraction = basin_relief_fraction + highland_relief_fraction + impacts;
        if !absolute_relief_fraction.is_finite() || absolute_relief_fraction >= 0.1 {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            basin_relief_fraction: positive_zero(basin_relief_fraction),
            highland_relief_fraction: positive_zero(highland_relief_fraction),
            impact_scale_fractions: impact_scale_fractions.map(positive_zero),
            impact_relief_fractions: impact_relief_fractions.map(positive_zero),
            absolute_relief_fraction,
        })
    }
    pub fn basin_relief_fraction(self) -> f64 {
        self.basin_relief_fraction
    }
    pub fn highland_relief_fraction(self) -> f64 {
        self.highland_relief_fraction
    }
    pub fn impact_scale_fractions(self) -> [f64; 3] {
        self.impact_scale_fractions
    }
    pub fn impact_relief_fractions(self) -> [f64; 3] {
        self.impact_relief_fractions
    }
    fn absolute_height_bound_m(self, radius_m: f64) -> f64 {
        (outward_product(self.absolute_relief_fraction, radius_m) + REGOLITH_RELIEF_BOUND_M)
            .next_up()
    }
}
impl Default for MoonTerrainConfig {
    fn default() -> Self {
        Self {
            basin_relief_fraction: 0.0015,
            highland_relief_fraction: 0.001,
            impact_scale_fractions: [0.24, 0.018, 0.00003],
            impact_relief_fractions: [0.0025, 0.0008, 0.000018],
            absolute_relief_fraction: 0.005818,
        }
    }
}

/// Stable explicit generation identity, independent of runtime body handles.
#[derive(Debug, Clone, PartialEq)]
pub struct MoonTerrainDefinition {
    identity: TerrainIdentity,
    seed: TerrainSeed,
    config: MoonTerrainConfig,
    version: MoonTerrainVersion,
}
impl MoonTerrainDefinition {
    pub fn new(identity: TerrainIdentity, seed: TerrainSeed, config: MoonTerrainConfig) -> Self {
        Self::with_version(identity, seed, config, MoonTerrainVersion::MoonLikeV1)
    }
    pub fn with_version(
        identity: TerrainIdentity,
        seed: TerrainSeed,
        config: MoonTerrainConfig,
        version: MoonTerrainVersion,
    ) -> Self {
        Self {
            identity,
            seed,
            config,
            version,
        }
    }
    pub fn identity(&self) -> TerrainIdentity {
        self.identity
    }
    pub fn seed(&self) -> TerrainSeed {
        self.seed
    }
    pub const fn version(&self) -> MoonTerrainVersion {
        self.version
    }
    pub fn config(&self) -> &MoonTerrainConfig {
        &self.config
    }
}

/// Three correlated surface-material weights.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoonMaterialSample {
    regolith_weight: f64,
    rock_weight: f64,
    basalt_weight: f64,
}
impl MoonMaterialSample {
    pub fn regolith_weight(self) -> f64 {
        self.regolith_weight
    }
    pub fn rock_weight(self) -> f64 {
        self.rock_weight
    }
    pub fn basalt_weight(self) -> f64 {
        self.basalt_weight
    }
}

/// Complete authoritative height and analytic tangent gradient with its
/// correlated material field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoonSurfaceSample {
    terrain: TerrainSample,
    material: MoonMaterialSample,
}
impl MoonSurfaceSample {
    pub fn terrain(self) -> TerrainSample {
        self.terrain
    }
    pub fn material(self) -> MoonMaterialSample {
        self.material
    }
}

#[derive(Debug, Clone, Copy)]
struct FieldValue {
    height_m: f64,
    gradient: DVec3,
    highland: f64,
    basalt: f64,
    rock: f64,
}

/// Stateless local-cell generator. Each cell layout examines only its fixed
/// 3x3x3 support halo in body-fixed space; there is no planet catalogue.
#[derive(Debug, Clone)]
pub struct MoonTerrainGenerator {
    identity: TerrainIdentity,
    seed: TerrainSeed,
    config: MoonTerrainConfig,
    radius_m: f64,
    cell_edges_m: [f64; 3],
    cell_rotations: [DMat3; 3],
    absolute_height_bound_m: f64,
    ancient: Option<ancient::AncientField>,
}
impl MoonTerrainGenerator {
    pub fn new(definition: &MoonTerrainDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        if !radius_m.is_finite() || radius_m <= 0.0 || radius_m > MAX_RADIUS_M {
            return Err(TerrainError::InvalidRadius);
        }
        let ancient = match definition.version {
            MoonTerrainVersion::MoonLikeV1 => None,
            MoonTerrainVersion::MoonLikeV2 => {
                Some(ancient::AncientField::new(definition, radius_m)?)
            }
        };
        let absolute_height_bound_m = ancient.as_ref().map_or_else(
            || definition.config.absolute_height_bound_m(radius_m),
            ancient::AncientField::absolute_height_bound_m,
        );
        if !absolute_height_bound_m.is_finite()
            || absolute_height_bound_m > 0.1 * radius_m
            || radius_m - absolute_height_bound_m <= (64.0 * f64::EPSILON * radius_m).max(1e-3)
        {
            return Err(TerrainError::InvalidRadius);
        }
        let cell_edges_m = definition
            .config
            .impact_scale_fractions
            .map(|fraction| (radius_m * fraction).max(MIN_CELL_EDGE_M));
        let cell_rotations = [0, 1, 2].map(|scale| {
            let salt = mix(definition.seed.0
                ^ definition.identity.0.rotate_left(19)
                ^ (scale as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ 0x524f_5441_5445_0001);
            let angle = |tag: u64| {
                let unit = (mix(salt ^ tag) >> 11) as f64 / ((1u64 << 53) as f64);
                unit * std::f64::consts::TAU
            };
            DMat3::from_rotation_z(angle(0x585f_4158_4953_0001))
                * DMat3::from_rotation_y(angle(0x595f_4158_4953_0001))
                * DMat3::from_rotation_x(angle(0x5a5f_4158_4953_0001))
        });
        Ok(Self {
            identity: definition.identity,
            seed: definition.seed,
            config: definition.config,
            radius_m,
            cell_edges_m,
            cell_rotations,
            absolute_height_bound_m,
            ancient,
        })
    }
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }
    pub fn version(&self) -> MoonTerrainVersion {
        if self.ancient.is_some() {
            MoonTerrainVersion::MoonLikeV2
        } else {
            MoonTerrainVersion::MoonLikeV1
        }
    }
    pub fn conservative_absolute_height_bound_m(&self) -> f64 {
        self.absolute_height_bound_m
    }
    pub fn evaluate_point(
        &self,
        location: SurfaceLocation,
    ) -> Result<MoonSurfaceSample, TerrainError> {
        let value = match &self.ancient {
            Some(field) => field.evaluate(location)?,
            None => self.evaluate_with_cell_halo(location, 1)?,
        };
        if !value.height_m.is_finite() || !value.gradient.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        let material = material_sample(value, self.radius_m);
        Ok(MoonSurfaceSample {
            terrain: TerrainSample::from_parts(value.height_m, value.gradient),
            material,
        })
    }

    pub(super) fn evaluate_point_with_context(
        &self,
        location: SurfaceLocation,
        context: &mut SurfaceQueryContext<'_>,
    ) -> Result<MoonSurfaceSample, TerrainError> {
        let value = match &self.ancient {
            Some(field) => field.evaluate_with_context(location, context)?,
            None => self.evaluate_with_cell_halo(location, 1)?,
        };
        if !value.height_m.is_finite() || !value.gradient.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        let material = material_sample(value, self.radius_m);
        Ok(MoonSurfaceSample {
            terrain: TerrainSample::from_parts(value.height_m, value.gradient),
            material,
        })
    }
    pub fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        output: &mut [MoonSurfaceSample],
    ) -> Result<(), TerrainError> {
        if locations.len() != output.len() {
            return Err(TerrainError::LengthMismatch);
        }
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate_point(*location)?;
        }
        Ok(())
    }
    fn evaluate_with_cell_halo(
        &self,
        location: SurfaceLocation,
        halo: i64,
    ) -> Result<FieldValue, TerrainError> {
        let direction = location.direction().unit();
        let position = direction * self.radius_m;
        let (basin, highland, highland_mask) = self.broad_fields(direction)?;
        let (broad_gradient, highland_mask_gradient) = self.broad_gradient(direction)?;
        let wavelength = (self.radius_m * REGOLITH_WAVELENGTH_FRACTION).max(MIN_CELL_EDGE_M);
        let grain = gradient_noise(
            self.salt(0x5245_474f_4c49_5448),
            direction * (self.radius_m / wavelength),
        )
        .map_err(|_| TerrainError::NonFiniteResult)?;
        let grain_mask = 0.6 + 0.4 * highland_mask;
        let grain_height = REGOLITH_RELIEF_BOUND_M * grain.value * grain_mask;
        let grain_gradient = (grain.gradient * (self.radius_m / wavelength) * grain_mask
            + highland_mask_gradient * (0.4 * grain.value))
            * REGOLITH_RELIEF_BOUND_M;
        let mut value = FieldValue {
            height_m: basin + highland + grain_height,
            gradient: broad_gradient + tangent_project(direction, grain_gradient),
            highland: highland_mask,
            basalt: 0.0,
            rock: 0.0,
        };
        // Epochs encode a fixed chronology: broad impacts are oldest, and each
        // finer scale overprints a bounded local residual on the earlier field.
        // Within an epoch, hash age then cell coordinates give a total order.
        for scale in 0..3 {
            let edge = self.cell_edges_m[scale];
            let rotation = self.cell_rotations[scale];
            let position_local = rotation * position;
            let center = position_local / edge;
            let base = checked_cell(center)?;
            let mut candidates = [ImpactCandidate::ZERO; 343];
            let mut candidate_count = 0;
            for z in -halo..=halo {
                for y in -halo..=halo {
                    for x in -halo..=halo {
                        let cell = [base[0] + x, base[1] + y, base[2] + z];
                        let key = feature_key(self.seed, self.identity, scale, cell);
                        let feature = feature_value(
                            position_local,
                            edge,
                            self.radius_m,
                            self.config.impact_relief_fractions[scale] * self.radius_m,
                            cell,
                            key,
                        );
                        if feature.influence > 0.0 {
                            candidates[candidate_count] = ImpactCandidate {
                                age: feature.age,
                                cell,
                                height_m: feature.height_m,
                                gradient_per_m: rotation.transpose() * feature.gradient_per_m,
                                influence: feature.influence,
                                influence_gradient_per_m: rotation.transpose()
                                    * feature.influence_gradient_per_m,
                                basalt: feature.basalt,
                                rock: feature.rock,
                            };
                            candidate_count += 1;
                        }
                    }
                }
            }
            candidates[..candidate_count]
                .sort_unstable_by_key(|candidate| (candidate.age, candidate.cell));
            let mut displacement = 0.0;
            let mut displacement_gradient = DVec3::ZERO;
            for candidate in &candidates[..candidate_count] {
                let old = displacement;
                displacement =
                    old * (1.0 - candidate.influence) + candidate.height_m * candidate.influence;
                displacement_gradient = displacement_gradient * (1.0 - candidate.influence)
                    + tangent_project(direction, candidate.gradient_per_m * self.radius_m)
                        * candidate.influence
                    + (candidate.height_m - old)
                        * tangent_project(
                            direction,
                            candidate.influence_gradient_per_m * self.radius_m,
                        );
                value.basalt = value.basalt * (1.0 - candidate.influence)
                    + candidate.basalt * candidate.influence;
                value.rock =
                    value.rock * (1.0 - candidate.influence) + candidate.rock * candidate.influence;
            }
            value.height_m += displacement;
            value.gradient += displacement_gradient;
        }
        Ok(value)
    }
    fn broad_fields(&self, n: DVec3) -> Result<(f64, f64, f64), TerrainError> {
        let macro_a = gradient_noise(self.salt(0x4d41_4352_4f00_0001), n * 2.7)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let macro_b = gradient_noise(
            self.salt(0x4d41_4352_4f00_0002),
            n * 4.1 + DVec3::splat(11.),
        )
        .map_err(|_| TerrainError::NonFiniteResult)?;
        let regional = gradient_noise(self.salt(0x4849_4748_0000_0001), n * 13.3)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let basin_value = macro_a.value * 0.68 + macro_b.value * 0.32;
        let basin = self.radius_m * self.config.basin_relief_fraction * basin_value;
        let field =
            (macro_a.value * 0.52 + macro_b.value * 0.28 + regional.value * 0.20).clamp(-1.0, 1.0);
        let mask = smoothstep(-0.12, 0.44, field);
        let ridge = 1.0 - (regional.value * regional.value + 0.05 * 0.05).sqrt();
        let highland = self.radius_m * self.config.highland_relief_fraction * mask * ridge;
        Ok((basin, highland, mask))
    }
    fn broad_gradient(&self, n: DVec3) -> Result<(DVec3, DVec3), TerrainError> {
        let a = gradient_noise(self.salt(0x4d41_4352_4f00_0001), n * 2.7)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let b = gradient_noise(
            self.salt(0x4d41_4352_4f00_0002),
            n * 4.1 + DVec3::splat(11.),
        )
        .map_err(|_| TerrainError::NonFiniteResult)?;
        let r = gradient_noise(self.salt(0x4849_4748_0000_0001), n * 13.3)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let basin_grad = (a.gradient * 2.7 * 0.68 + b.gradient * 4.1 * 0.32)
            * (self.radius_m * self.config.basin_relief_fraction);
        let field = a.value * 0.52 + b.value * 0.28 + r.value * 0.20;
        let field_grad =
            a.gradient * (2.7 * 0.52) + b.gradient * (4.1 * 0.28) + r.gradient * (13.3 * 0.20);
        let mask = smoothstep(-0.12, 0.44, field);
        let mask_derivative = smoothstep_derivative(-0.12, 0.44, field);
        let ridge_raw = (r.value * r.value + 0.05 * 0.05).sqrt();
        let ridge = 1.0 - ridge_raw;
        let ridge_grad = -r.gradient * (13.3 * r.value / ridge_raw);
        let highland_grad = (mask_derivative * field_grad * ridge + ridge_grad * mask)
            * (self.radius_m * self.config.highland_relief_fraction);
        Ok((
            tangent_project(n, basin_grad + highland_grad),
            tangent_project(n, mask_derivative * field_grad),
        ))
    }
    fn salt(&self, tag: u64) -> u64 {
        mix(self.seed.0 ^ self.identity.0.rotate_left(19) ^ tag ^ 0x4d4f_4f4e_0000_0001)
    }
}

#[derive(Debug, Clone, Copy)]
struct ImpactCandidate {
    age: u64,
    cell: [i64; 3],
    height_m: f64,
    gradient_per_m: DVec3,
    influence: f64,
    influence_gradient_per_m: DVec3,
    basalt: f64,
    rock: f64,
}
impl ImpactCandidate {
    const ZERO: Self = Self {
        age: u64::MAX,
        cell: [i64::MAX; 3],
        height_m: 0.0,
        gradient_per_m: DVec3::ZERO,
        influence: 0.0,
        influence_gradient_per_m: DVec3::ZERO,
        basalt: 0.0,
        rock: 0.0,
    };
}
#[derive(Debug, Clone, Copy)]
struct FeatureValue {
    age: u64,
    height_m: f64,
    gradient_per_m: DVec3,
    influence: f64,
    influence_gradient_per_m: DVec3,
    basalt: f64,
    rock: f64,
}
impl FeatureValue {
    const ZERO: Self = Self {
        age: u64::MAX,
        height_m: 0.0,
        gradient_per_m: DVec3::ZERO,
        influence: 0.0,
        influence_gradient_per_m: DVec3::ZERO,
        basalt: 0.0,
        rock: 0.0,
    };
}

fn feature_value(
    position: DVec3,
    edge_m: f64,
    radius_m: f64,
    relief_m: f64,
    cell: [i64; 3],
    key: u64,
) -> FeatureValue {
    let unit = |tag: u64| -> f64 {
        let bits = mix(key ^ tag) >> 11;
        bits as f64 * (1.0 / ((1u64 << 53) as f64))
    };
    let jitter = DVec3::new(
        unit(0x4a49_5454_4552_0001),
        unit(0x4a49_5454_4552_0002),
        unit(0x4a49_5454_4552_0003),
    ) * (2.0 * CELL_JITTER)
        - DVec3::splat(CELL_JITTER);
    let raw_center = (DVec3::new(cell[0] as f64, cell[1] as f64, cell[2] as f64) + jitter) * edge_m;
    let raw_radius = raw_center.length();
    if !raw_radius.is_finite()
        || (raw_radius - radius_m).abs() > edge_m * FEATURE_SHELL_FILTER_FRACTION
    {
        return FeatureValue::ZERO;
    }
    let center = raw_center * (radius_m / raw_radius);
    let delta = position - center;
    let support = edge_m * FEATURE_SUPPORT_FRACTION;
    let distance = delta.length();
    if distance >= support || relief_m == 0.0 {
        return FeatureValue::ZERO;
    }
    let crater_radius = support * (0.48 + 0.37 * unit(0x5241_4449_5553_0001));
    let base_s = distance / crater_radius;
    let base_s_gradient = if distance > 0.0 {
        delta / (crater_radius * distance)
    } else {
        DVec3::ZERO
    };
    // Smooth, feature-fixed wall corrugation changes the rim's shape as well as
    // its height. Compact support still uses the unwarped chord distance.
    let wall_axis_a = unit_vector(key ^ 0x5741_4c4c_0000_0001);
    let wall_axis_b = unit_vector(key ^ 0x5741_4c4c_0000_0002);
    let phase_a = 4.0 * delta.dot(wall_axis_a) / crater_radius
        + std::f64::consts::TAU * unit(0x5048_4153_4500_0001);
    let phase_b = 7.0 * delta.dot(wall_axis_b) / crater_radius
        + std::f64::consts::TAU * unit(0x5048_4153_4500_0002);
    let wall_pattern = (phase_a.sin() + 0.5 * phase_b.sin()) * (2.0 / 3.0);
    let wall_pattern_gradient = (wall_axis_a * (4.0 * phase_a.cos())
        + wall_axis_b * (3.5 * phase_b.cos()))
        * ((2.0 / 3.0) / crater_radius);
    let warp_gate = smoothstep(0.2, 0.6, base_s);
    let warp_gate_derivative = smoothstep_derivative(0.2, 0.6, base_s);
    let s = base_s + 0.16 * wall_pattern * warp_gate;
    let s_gradient = base_s_gradient * (1.0 + 0.16 * wall_pattern * warp_gate_derivative)
        + wall_pattern_gradient * (0.16 * warp_gate);
    let age = mix(key ^ 0x4147_4500_0000_0001);
    let age_maturity = (age >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
    let asym_axis = unit_vector(key);
    let asym = 0.08 + 0.20 * age_maturity;
    let asym_value = 1.0 + asym * delta.dot(asym_axis) / support;
    let bowl_strength = 0.58 + 0.16 * age_maturity;
    let rim_strength = 0.10 + 0.14 * age_maturity;
    let ejecta_strength = 0.06 * age_maturity;
    let rim_sector = 0.30 + 0.70 * smoothstep(-0.25, 0.4, wall_pattern);
    let rim_sector_gradient =
        wall_pattern_gradient * (0.70 * smoothstep_derivative(-0.25, 0.4, wall_pattern));
    let bowl = -bowl_strength * (-2.0 * s * s).exp();
    let ungated_rim_base = rim_strength * (-((s - 0.76) / 0.16).powi(2)).exp();
    let rim_base = ungated_rim_base * warp_gate;
    let rim = rim_base * rim_sector;
    let ungated_ejecta = ejecta_strength * (-((s - 1.25) / 0.22).powi(2)).exp();
    let ejecta = ungated_ejecta * warp_gate;
    let wall_envelope = (-((s - 0.8) / 0.5).powi(2)).exp();
    // Noncentral profiles vanish around the origin: an offset Gaussian ring
    // otherwise has a nonzero radial derivative at zero distance (a cusp).
    let ungated_wall_relief = 0.045 * wall_pattern * wall_envelope;
    let wall_relief = ungated_wall_relief * warp_gate;
    let core = bowl + rim + ejecta + wall_relief;
    let core_derivative_s = 4.0 * bowl_strength * s * (-2.0 * s * s).exp()
        - 2.0 * rim * (s - 0.76) / (0.16 * 0.16)
        - 2.0 * ejecta * (s - 1.25) / (0.22 * 0.22)
        - 2.0 * wall_relief * (s - 0.8) / (0.5 * 0.5);
    let cutoff = 1.0 - smoothstep(0.78, 1.0, distance / support);
    let cutoff_derivative = -smoothstep_derivative(0.78, 1.0, distance / support) / support;
    let core_gradient = s_gradient * core_derivative_s
        + rim_sector_gradient * rim_base
        + wall_pattern_gradient * (0.045 * wall_envelope * warp_gate)
        + base_s_gradient
            * warp_gate_derivative
            * (ungated_rim_base * rim_sector + ungated_ejecta + ungated_wall_relief);
    let cutoff_gradient = if distance > 1e-9 {
        delta * (cutoff_derivative / distance)
    } else {
        DVec3::ZERO
    };
    // |core| <= .74+.24+.06+.045; |asymmetry| <= 1.28. Their product
    // times .65 is < .903, leaving numerical margin within each epoch budget.
    const PROFILE_ENVELOPE_SCALE: f64 = 0.65;
    let profile = relief_m * PROFILE_ENVELOPE_SCALE * core * asym_value * cutoff;
    let profile_gradient = relief_m
        * PROFILE_ENVELOPE_SCALE
        * (core_gradient * asym_value * cutoff
            + asym_axis * (core * asym / support * cutoff)
            + cutoff_gradient * (core * asym_value));
    let influence = cutoff;
    let s_bowl = (1.0 - smoothstep(0.46, 0.88, s)).clamp(0.0, 1.0);
    let s_rim = (1.0 - ((s - 0.78) / 0.18).abs()).clamp(0.0, 1.0);
    FeatureValue {
        age,
        height_m: profile,
        gradient_per_m: profile_gradient,
        influence,
        influence_gradient_per_m: cutoff_gradient,
        basalt: s_bowl,
        rock: s_rim * rim_sector,
    }
}

fn material_sample(value: FieldValue, radius_m: f64) -> MoonMaterialSample {
    let slope = value.gradient.length() / (radius_m + value.height_m).max(1.0);
    let basalt = 0.08 + 0.67 * value.basalt + 0.12 * (1.0 - value.highland);
    let rock = 0.14 + 0.48 * value.highland + 0.33 * value.rock + 0.18 * slope.min(1.0);
    let regolith =
        (0.78 - 0.28 * value.highland - 0.53 * value.basalt - 0.16 * value.rock).max(0.02);
    let total = regolith + rock + basalt;
    MoonMaterialSample {
        regolith_weight: regolith / total,
        rock_weight: rock / total,
        basalt_weight: basalt / total,
    }
}

fn checked_cell(value: DVec3) -> Result<[i64; 3], TerrainError> {
    if !value.is_finite() || value.abs().max_element() >= i64::MAX as f64 - 4.0 {
        return Err(TerrainError::NonFiniteResult);
    }
    Ok(value.floor().to_array().map(|x| x as i64))
}
fn feature_key(seed: TerrainSeed, identity: TerrainIdentity, scale: usize, cell: [i64; 3]) -> u64 {
    let mut value =
        seed.0 ^ identity.0.rotate_left(19) ^ (scale as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for (coordinate, tag) in cell.into_iter().zip([
        0xd6e8_feb8_6659_fd93,
        0xa5a3_56e4_27f8_862f,
        0x9e37_79b1_85eb_ca87,
    ]) {
        value = mix(value ^ (coordinate as u64).wrapping_mul(tag));
    }
    mix(value ^ 0x494d_5041_4354_0001)
}
fn unit_vector(key: u64) -> DVec3 {
    let value = |tag: u64| -> f64 {
        ((mix(key ^ tag) >> 11) as f64) * (1.0 / ((1u64 << 53) as f64)) * 2.0 - 1.0
    };
    Direction3::try_new(DVec3::new(
        value(0x4158_4953_0000_0001),
        value(0x4158_4953_0000_0002),
        value(0x4158_4953_0000_0003),
    ))
    .map_or(DVec3::X, |d| d.unit())
}
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn smoothstep_derivative(edge0: f64, edge1: f64, x: f64) -> f64 {
    if x <= edge0 || x >= edge1 {
        0.0
    } else {
        let t = (x - edge0) / (edge1 - edge0);
        6.0 * t * (1.0 - t) / (edge1 - edge0)
    }
}
fn tangent_project(n: DVec3, v: DVec3) -> DVec3 {
    v - n * v.dot(n)
}
fn positive_zero(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value }
}
fn outward_product(a: f64, b: f64) -> f64 {
    if a == 0.0 {
        0.0
    } else {
        (a * b * (1.0 + 64.0 * f64::EPSILON)).next_up()
    }
}
fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod halo_tests {
    use super::*;

    #[test]
    fn warped_walls_center_and_support_have_independent_cartesian_gradients() {
        let key = 0x17325;
        let edge = 1000.0;
        let unit = |tag| (mix(key ^ tag) >> 11) as f64 / ((1u64 << 53) as f64);
        let jitter = DVec3::new(
            unit(0x4a49_5454_4552_0001),
            unit(0x4a49_5454_4552_0002),
            unit(0x4a49_5454_4552_0003),
        ) * (2.0 * CELL_JITTER)
            - DVec3::splat(CELL_JITTER);
        let center = (DVec3::Z + jitter) * edge;
        let radius = center.length();
        let sample = |position| feature_value(position, edge, radius, 100.0, [0, 0, 1], key);
        let axis = DVec3::new(0.31, 0.72, -0.55).normalize();
        for distance in [
            0.0, 20.0, 80.0, 120.0, 180.0, 260.0, 340.0, 506.99, 507.01, 649.99, 650.01,
        ] {
            let position = center + axis * distance;
            let value = sample(position);
            for tangent in [DVec3::X, DVec3::Y, DVec3::Z] {
                let epsilon = 1e-4;
                let derivative = (sample(position + tangent * epsilon).height_m
                    - sample(position - tangent * epsilon).height_m)
                    / (2.0 * epsilon);
                let analytic = value.gradient_per_m.dot(tangent);
                assert!(
                    (derivative - analytic).abs() < 1e-6,
                    "at {distance} m: finite difference {derivative}, analytic {analytic}"
                );
            }
            assert!(value.height_m.abs() <= 100.0);
        }
    }

    #[test]
    fn three_cell_halo_oracle_matches_the_fixed_support_halo() {
        let definition = MoonTerrainDefinition::new(
            TerrainIdentity(0x1234),
            TerrainSeed(0x5678),
            MoonTerrainConfig::default(),
        );
        for radius in [109_000.0, 1_737_000.0, 100_000_000.0] {
            let generator = MoonTerrainGenerator::new(&definition, radius).unwrap();
            for n in [
                DVec3::new(1.0, 1.0, 1.0).normalize(),
                DVec3::new(1.0, 1.0, 0.0).normalize(),
                DVec3::new(-1.0, 0.5, 0.5).normalize(),
                DVec3::new(0.123, -0.992, 0.027).normalize(),
            ] {
                let location = SurfaceLocation::new(Direction3::try_new(n).unwrap());
                let one = generator.evaluate_with_cell_halo(location, 1).unwrap();
                let three = generator.evaluate_with_cell_halo(location, 3).unwrap();
                assert_eq!(one.height_m.to_bits(), three.height_m.to_bits());
                assert_eq!(one.gradient, three.gradient);
                assert_eq!(one.basalt, three.basalt);
                assert_eq!(one.rock, three.rock);
            }
        }
    }

    #[test]
    fn overlapping_impacts_preserve_the_analytic_gradient() {
        let definition = MoonTerrainDefinition::new(
            TerrainIdentity(0x4455),
            TerrainSeed(0x9911),
            MoonTerrainConfig::default(),
        );
        let field = MoonTerrainGenerator::new(&definition, 109_000.0).unwrap();
        let mut overlap = None;
        for index in 0..1024 {
            let angle = index as f64 * 2.399963229728653;
            let z = 1.0 - 2.0 * (index as f64 + 0.5) / 1024.0;
            let radial = (1.0 - z * z).sqrt();
            let n = DVec3::new(radial * angle.cos(), radial * angle.sin(), z);
            for scale in 0..3 {
                let edge = field.cell_edges_m[scale];
                let rotation = field.cell_rotations[scale];
                let q_local = rotation * (n * field.radius_m);
                let base = checked_cell(q_local / edge).unwrap();
                let mut found = 0;
                for dz in -1..=1 {
                    for dy in -1..=1 {
                        for dx in -1..=1 {
                            let cell = [base[0] + dx, base[1] + dy, base[2] + dz];
                            let key = feature_key(field.seed, field.identity, scale, cell);
                            if key & 3 == 0 {
                                continue;
                            }
                            found += usize::from(
                                feature_value(
                                    q_local,
                                    edge,
                                    field.radius_m,
                                    field.config.impact_relief_fractions[scale] * field.radius_m,
                                    cell,
                                    key,
                                )
                                .influence
                                    > 0.0,
                            );
                        }
                    }
                }
                if found > 1 {
                    overlap = Some(n);
                    break;
                }
            }
            if overlap.is_some() {
                break;
            }
        }
        let n = overlap.expect("the deterministic scan fixture includes impact overlaps");
        let tangent = n.cross(DVec3::new(0.31, -0.72, 0.55)).normalize();
        let epsilon = 1.0e-8;
        let center = field
            .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
            .unwrap()
            .terrain();
        let plus = field
            .evaluate_point(SurfaceLocation::new(
                Direction3::try_new(n + epsilon * tangent).unwrap(),
            ))
            .unwrap()
            .terrain()
            .height_m();
        let minus = field
            .evaluate_point(SurfaceLocation::new(
                Direction3::try_new(n - epsilon * tangent).unwrap(),
            ))
            .unwrap()
            .terrain()
            .height_m();
        let finite_difference = (plus - minus) / (2.0 * epsilon);
        let analytic = center.tangent_gradient_m_per_unit_direction().dot(tangent);
        assert!(
            (finite_difference - analytic).abs() <= 0.08 + analytic.abs() * 0.015,
            "overlap gradient mismatch: fd={finite_difference}, analytic={analytic}"
        );
    }

    #[test]
    fn grid_plane_crossings_keep_the_analytic_gradient_continuous() {
        let definition = MoonTerrainDefinition::new(
            TerrainIdentity(0xabcde),
            TerrainSeed(0x31337),
            MoonTerrainConfig::default(),
        );
        let field = MoonTerrainGenerator::new(&definition, 109_000.0).unwrap();
        for scale in 0..3 {
            let edge = field.cell_edges_m[scale];
            let rotation = field.cell_rotations[scale];
            let cell_index = (field.radius_m * 0.4 / edge).floor().max(1.0);
            let x = edge * cell_index;
            let local_normal = DVec3::new(x, (field.radius_m * field.radius_m - x * x).sqrt(), 0.0)
                / field.radius_m;
            let body_normal = rotation.transpose() * local_normal;
            let local_tangent = tangent_project(local_normal, DVec3::X).normalize();
            let body_tangent = rotation.transpose() * local_tangent;
            let epsilon = 1.0e-8;
            let sample = |n: DVec3| {
                field
                    .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                    .unwrap()
                    .terrain()
            };
            let center = sample(body_normal);
            let finite_difference = (sample(body_normal + epsilon * body_tangent).height_m()
                - sample(body_normal - epsilon * body_tangent).height_m())
                / (2.0 * epsilon);
            let analytic = center
                .tangent_gradient_m_per_unit_direction()
                .dot(body_tangent);
            assert!(
                (finite_difference - analytic).abs() <= 0.1 + analytic.abs() * 0.02,
                "scale {scale} grid-plane gradient mismatch: fd={finite_difference}, analytic={analytic}"
            );
        }
    }
}
