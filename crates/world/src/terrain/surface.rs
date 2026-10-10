//! Composable complete body-surface authority. No view, frame or LOD input enters
//! these definitions. Shapes and geology are radial graphs, not collision solids.
use super::{
    MoonTerrainConfig, MoonTerrainDefinition, MoonTerrainGenerator, MoonTerrainVersion,
    PreparedSurface, TerrainError, TerrainIdentity, TerrainSample, TerrainSeed,
};
use astrum_math::{Direction3, noise::gradient_noise, surface::SurfaceLocation};
use glam::{DMat3, DVec3};
mod authoring;
pub use authoring::{
    AffineRandomRange, GeologicalAffineControl, GeologicalDistribution,
    MoonCraterProfileDefinition, MoonFieldBandDefinition, MoonFieldDefinition,
};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
mod shape;
pub use shape::*;
mod director;
mod geology;
mod hierarchy;
pub mod ladder;
mod moon_fields;
mod moon_profile;
pub mod noise;
pub mod producer;
pub mod world_field;
mod provinces;
mod query_context;
pub use hierarchy::SurfaceDetailDiagnostics;
pub use moon_fields::{
    MAX_MOON_CRATER_INPUTS, MOON_CRATER_BAND_BUDGETS_M, MoonCraterInput, MoonFieldEvaluationF32,
    MoonFieldPointInputs,
};
pub use moon_profile::{
    MoonProfileEvaluationDiagnostics, TerrainHeightDetailLayer, TerrainHeightProfile,
};
use moon_profile::{NoopProfileObserver, ProfileObserver, TimedProfileObserver};
pub(crate) use query_context::PreparedPageKey;
pub use query_context::{
    SurfacePreparationStoreStats, SurfaceQueryCacheStats, SurfaceQueryContext,
    SurfaceQueryProfileStats,
};

/// Independent geological algorithm namespaces; parameters remain explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceAlgorithm {
    PreparedV1,
    /// Tier A world map (archetype, GPU bake) plus band-limited detail
    /// (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6, M1).
    WorldV1,
    RockyV3,
    RockyV4,
    RockyV5,
    IcyV1,
    IcyV2,
    IcyV3,
    VolcanicV1,
    VolcanicV2,
    VolcanicV3,
    /// Seeded planetary/regional/local Moon profiles with compact f64 recipes.
    MoonFieldsV1,
    /// Immutable periodic height profile sampled through seamless triplanar charts.
    MoonProfileV1,
}
impl SurfaceAlgorithm {
    pub const fn code(self) -> u64 {
        match self {
            Self::PreparedV1 => 0x5052_4550_0000_0001,
            Self::WorldV1 => 0x574f_524c_0000_0001,
            Self::RockyV3 => 0x524f_434b_0000_0003,
            Self::RockyV4 => 0x524f_434b_0000_0004,
            Self::RockyV5 => 0x524f_434b_0000_0005,
            Self::IcyV1 => 0x4943_4520_0000_0001,
            Self::IcyV2 => 0x4943_4520_0000_0002,
            Self::IcyV3 => 0x4943_4520_0000_0003,
            Self::VolcanicV1 => 0x564f_4c43_0000_0001,
            Self::VolcanicV2 => 0x564f_4c43_0000_0002,
            Self::VolcanicV3 => 0x564f_4c43_0000_0003,
            Self::MoonFieldsV1 => 0x4d4f_4f4e_4649_0001,
            Self::MoonProfileV1 => 0x4d4f_4f4e_5052_0001,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::PreparedV1 => "PreparedV1",
            Self::WorldV1 => "WorldV1",
            Self::RockyV3 => "RockyV3",
            Self::RockyV4 => "RockyV4",
            Self::RockyV5 => "RockyV5",
            Self::IcyV1 => "IcyV1",
            Self::IcyV2 => "IcyV2",
            Self::IcyV3 => "IcyV3",
            Self::VolcanicV1 => "VolcanicV1",
            Self::VolcanicV2 => "VolcanicV2",
            Self::VolcanicV3 => "VolcanicV3",
            Self::MoonFieldsV1 => "MoonFieldsV1",
            Self::MoonProfileV1 => "MoonProfileV1",
        }
    }

    pub const fn province_names(self) -> [&'static str; 4] {
        match self {
            Self::PreparedV1 | Self::WorldV1 => ["flat", "crater", "overlap", "inactive"],
            Self::RockyV3 | Self::RockyV4 | Self::RockyV5 => [
                "ancient_highlands",
                "basin_margin",
                "resurfaced_plains",
                "structural_uplands",
            ],
            Self::MoonFieldsV1 => [
                "basin_floor",
                "ancient_highlands",
                "mare_plains",
                "impact_ejecta",
            ],
            Self::MoonProfileV1 => [
                "source_highlands",
                "source_basins",
                "mare_plains",
                "source_ejecta",
            ],
            Self::IcyV1 | Self::IcyV2 | Self::IcyV3 => [
                "old_ice",
                "fracture_tectonics",
                "young_ice",
                "reworked_chaos",
            ],
            Self::VolcanicV1 | Self::VolcanicV2 | Self::VolcanicV3 => [
                "old_substrate",
                "shields_calderas",
                "flow_plains",
                "young_resurfacing",
            ],
        }
    }

    pub const fn process_names(self) -> [&'static str; 4] {
        match self {
            Self::PreparedV1 | Self::WorldV1 => {
                ["base_field", "profile_lookup", "selector_blend", "inactive"]
            }
            Self::RockyV3 | Self::RockyV4 | Self::RockyV5 => [
                "retained_impacts",
                "basin_deformation",
                "burial_smoothing",
                "structural_ridges",
            ],
            Self::MoonFieldsV1 => [
                "planetary_basin_relief",
                "regional_impact_history",
                "crater_degradation",
                "regolith_breakdown",
            ],
            Self::MoonProfileV1 => [
                "profile_macro_relief",
                "profile_regional_relief",
                "profile_local_relief",
                "regional_material_expression",
            ],
            Self::IcyV1 | Self::IcyV2 | Self::IcyV3 => [
                "retained_impacts",
                "intersecting_fractures",
                "ice_relaxation",
                "chaotic_reworking",
            ],
            Self::VolcanicV1 | Self::VolcanicV2 | Self::VolcanicV3 => [
                "retained_impacts",
                "shield_calderas",
                "flow_fronts",
                "plain_burial",
            ],
        }
    }
}

/// Direction-local geological director output. The normalized values describe
/// generated history rather than calibrated physical measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeologicalControls {
    pub age: f64,
    pub activity: f64,
    pub resurfacing: f64,
    pub impact_retention: f64,
    pub relief_potential: f64,
    pub province_weights: [f64; 4],
    pub process_strengths: [f64; 4],
    pub structural_direction: DVec3,
}

/// Correlated body history, generated once before spatial feature placement.
/// Age/activity/retention are normalized controls, not calibrated physical dates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeologicalParameters {
    pub age: f64,
    pub activity: f64,
    pub resurfacing_fraction: f64,
    pub impact_retention: f64,
    pub relief_fraction: f64,
    pub feature_scale_fraction: f64,
    pub orientation_radians: f64,
}
impl GeologicalParameters {
    pub fn generated(algorithm: SurfaceAlgorithm, seed: TerrainSeed) -> Self {
        GeologicalDistribution::generate_default(algorithm, seed)
    }
    fn validate(self) -> Result<(), TerrainError> {
        if [
            self.age,
            self.activity,
            self.resurfacing_fraction,
            self.impact_retention,
        ]
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || !self.relief_fraction.is_finite()
            || !(0.0..=0.01).contains(&self.relief_fraction)
            || !self.feature_scale_fraction.is_finite()
            || !(0.05..=0.8).contains(&self.feature_scale_fraction)
            || !self.orientation_radians.is_finite()
            || !(0.0..=std::f64::consts::TAU).contains(&self.orientation_radians)
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(())
    }
    fn identity(self) -> u64 {
        hash_values(
            0x5041_5241_4d53_0001,
            &[
                self.age,
                self.activity,
                self.resurfacing_fraction,
                self.impact_retention,
                self.relief_fraction,
                self.feature_scale_fraction,
                self.orientation_radians,
            ],
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceTerrainDefinition {
    algorithm: SurfaceAlgorithm,
    parameters: GeologicalParameters,
    distribution: Option<GeologicalDistribution>,
}
impl SurfaceTerrainDefinition {
    pub fn new(
        algorithm: SurfaceAlgorithm,
        parameters: GeologicalParameters,
    ) -> Result<Self, TerrainError> {
        parameters.validate()?;
        Ok(Self {
            algorithm,
            parameters,
            distribution: None,
        })
    }
    pub fn generated(algorithm: SurfaceAlgorithm, seed: TerrainSeed) -> Self {
        Self {
            algorithm,
            parameters: GeologicalParameters::generated(algorithm, seed),
            distribution: None,
        }
    }
    pub fn generated_with_distribution(
        algorithm: SurfaceAlgorithm,
        seed: TerrainSeed,
        distribution: GeologicalDistribution,
    ) -> Result<Self, TerrainError> {
        let parameters = distribution.generate(algorithm, seed)?;
        Ok(Self {
            algorithm,
            parameters,
            distribution: (!distribution.same_as_default(algorithm)).then_some(distribution),
        })
    }
    pub fn algorithm(self) -> SurfaceAlgorithm {
        self.algorithm
    }
    pub fn parameters(self) -> GeologicalParameters {
        self.parameters
    }
    pub fn distribution(self) -> Option<GeologicalDistribution> {
        self.distribution
    }
    pub fn configuration_identity(self) -> u64 {
        mix(self.algorithm.code()
            ^ self.parameters.identity()
            ^ self
                .distribution
                .map_or(0, GeologicalDistribution::configuration_identity))
    }
}

/// Heap retained by every compiled generator for its lazy preparation-store
/// owner (the shared `OnceLock` slot plus `Arc` control-block metadata),
/// whether or not a prepared store is ever created.
pub(super) const PREPARATION_STORE_OWNER_BYTES: usize =
    std::mem::size_of::<OnceLock<Option<Arc<query_context::SurfacePreparationStore>>>>() + 32;

/// Material semantics are separately versioned from geology. A definition must
/// select the channel semantics compatible with its geological algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceMaterialVersion {
    PreparedV1,
    RockyV1,
    RockyV2,
    IcyV1,
    IcyV2,
    VolcanicV1,
    VolcanicV2,
}
impl SurfaceMaterialVersion {
    fn for_algorithm(algorithm: SurfaceAlgorithm) -> Self {
        match algorithm {
            SurfaceAlgorithm::PreparedV1 | SurfaceAlgorithm::WorldV1 => Self::PreparedV1,
            SurfaceAlgorithm::RockyV3 => Self::RockyV1,
            SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::MoonFieldsV1
            | SurfaceAlgorithm::MoonProfileV1 => Self::RockyV2,
            SurfaceAlgorithm::IcyV1 => Self::IcyV1,
            SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::IcyV3 => Self::IcyV2,
            SurfaceAlgorithm::VolcanicV1 => Self::VolcanicV1,
            SurfaceAlgorithm::VolcanicV2 | SurfaceAlgorithm::VolcanicV3 => Self::VolcanicV2,
        }
    }
    pub const fn code(self) -> u64 {
        match self {
            Self::PreparedV1 => 0x4d41_5450_0000_0001,
            Self::RockyV1 => 0x4d41_5452_0000_0001,
            Self::RockyV2 => 0x4d41_5452_0000_0002,
            Self::IcyV1 => 0x4d41_5449_0000_0001,
            Self::IcyV2 => 0x4d41_5449_0000_0002,
            Self::VolcanicV1 => 0x4d41_5456_0000_0001,
            Self::VolcanicV2 => 0x4d41_5456_0000_0002,
        }
    }
    pub const fn channels(self) -> [&'static str; 4] {
        match self {
            Self::PreparedV1 => [
                "regolith",
                "exposed_substrate",
                "resurfaced_basalt",
                "disturbed_ejecta",
            ],
            Self::RockyV1 => [
                "regolith",
                "exposed_substrate",
                "resurfaced_basalt",
                "disturbed_ejecta",
            ],
            Self::RockyV2 => [
                "regolith",
                "exposed_substrate",
                "resurfaced_basalt",
                "disturbed_ejecta",
            ],
            Self::IcyV1 => ["clean_ice", "old_ice", "disrupted_ice", "contaminated_ice"],
            Self::IcyV2 => ["clean_ice", "old_ice", "disrupted_ice", "contaminated_ice"],
            Self::VolcanicV1 => [
                "fresh_deposits",
                "old_plains",
                "old_substrate",
                "impact_disturbance",
            ],
            Self::VolcanicV2 => [
                "fresh_deposits",
                "old_plains",
                "old_substrate",
                "impact_disturbance",
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMaterialDefinition {
    version: SurfaceMaterialVersion,
    composition: f64,
    regional_contrast: f64,
}
impl SurfaceMaterialDefinition {
    pub fn new(
        version: SurfaceMaterialVersion,
        composition: f64,
        regional_contrast: f64,
    ) -> Result<Self, TerrainError> {
        if [composition, regional_contrast]
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            version,
            composition,
            regional_contrast,
        })
    }
    pub fn version(self) -> SurfaceMaterialVersion {
        self.version
    }
    pub fn composition(self) -> f64 {
        self.composition
    }
    pub fn regional_contrast(self) -> f64 {
        self.regional_contrast
    }
    pub fn configuration_identity(self) -> u64 {
        hash_values(
            self.version.code(),
            &[self.composition, self.regional_contrast],
        )
    }
}

/// Descriptor only; no scattering, weather, density query or rendering implied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfaceAtmosphere {
    Airless,
    Descriptor {
        pressure_pa: f64,
        scale_height_m: f64,
    },
}
impl SurfaceAtmosphere {
    fn identity(self) -> Result<u64, TerrainError> {
        match self {
            Self::Airless => Ok(0x4149_524c_4553_5301),
            Self::Descriptor {
                pressure_pa,
                scale_height_m,
            } => {
                if !pressure_pa.is_finite()
                    || pressure_pa <= 0.0
                    || !scale_height_m.is_finite()
                    || scale_height_m <= 0.0
                {
                    return Err(TerrainError::InvalidConfig);
                }
                Ok(hash_values(
                    0x4154_4d4f_5350_0001,
                    &[pressure_pa, scale_height_m],
                ))
            }
        }
    }
}

/// Stable authored identity and compositional definitions. Hashes are diagnostic
/// namespaces; cache equality must also compare the complete definition/radius.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceDefinition {
    prepared: Option<Arc<PreparedSurface>>,
    identity: TerrainIdentity,
    seed: TerrainSeed,
    shape: ShapeDefinition,
    terrain: SurfaceTerrainDefinition,
    material: SurfaceMaterialDefinition,
    atmosphere: SurfaceAtmosphere,
    atmosphere_identity: u64,
    height_profile: Option<TerrainHeightProfile>,
    moon_fields: Option<MoonFieldDefinition>,
    detail_noise: Option<noise::DetailNoiseDefinition>,
    world: Option<world_field::WorldDefinition>,
}
impl SurfaceDefinition {
    pub fn new(
        identity: TerrainIdentity,
        seed: TerrainSeed,
        shape: ShapeDefinition,
        terrain: SurfaceTerrainDefinition,
        material: SurfaceMaterialDefinition,
        atmosphere: SurfaceAtmosphere,
    ) -> Result<Self, TerrainError> {
        terrain.parameters.validate()?;
        let atmosphere_identity = atmosphere.identity()?;
        // Material algorithms interpret geological channels; formats must
        // agree while composition and contrast remain independent controls.
        if material.version != SurfaceMaterialVersion::for_algorithm(terrain.algorithm) {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            prepared: None,
            identity,
            seed,
            shape,
            terrain,
            material,
            atmosphere,
            atmosphere_identity,
            height_profile: None,
            moon_fields: None,
            detail_noise: None,
            world: None,
        })
    }
    pub fn generated(
        identity: TerrainIdentity,
        seed: TerrainSeed,
        algorithm: SurfaceAlgorithm,
    ) -> Self {
        let terrain = SurfaceTerrainDefinition::generated(algorithm, seed);
        let version = SurfaceMaterialVersion::for_algorithm(algorithm);
        Self {
            prepared: None,
            identity,
            seed,
            shape: ShapeDefinition::sphere(),
            terrain,
            material: SurfaceMaterialDefinition {
                version,
                composition: unit(seed.0 ^ 0x4d41_5445_5249_0001),
                regional_contrast: 0.4,
            },
            atmosphere: SurfaceAtmosphere::Airless,
            atmosphere_identity: 0x4149_524c_4553_5301,
            height_profile: None,
            moon_fields: None,
            detail_noise: None,
            world: None,
        }
    }
    pub fn with_height_profile(
        mut self,
        profile: TerrainHeightProfile,
    ) -> Result<Self, TerrainError> {
        if self.terrain.algorithm != SurfaceAlgorithm::MoonProfileV1 {
            return Err(TerrainError::InvalidConfig);
        }
        self.height_profile = Some(profile);
        Ok(self)
    }
    pub fn with_moon_fields(
        mut self,
        definition: MoonFieldDefinition,
    ) -> Result<Self, TerrainError> {
        if self.terrain.algorithm != SurfaceAlgorithm::MoonFieldsV1 {
            return Err(TerrainError::InvalidConfig);
        }
        definition.validate()?;
        self.moon_fields = (!definition.same_as_default()).then_some(definition);
        Ok(self)
    }
    pub fn with_geological_distribution(
        mut self,
        distribution: GeologicalDistribution,
    ) -> Result<Self, TerrainError> {
        self.terrain = SurfaceTerrainDefinition::generated_with_distribution(
            self.terrain.algorithm,
            self.seed,
            distribution,
        )?;
        Ok(self)
    }
    /// The Tier A world map of a `WorldV1` surface (M1).
    pub fn with_world(mut self, world: world_field::WorldDefinition) -> Result<Self, TerrainError> {
        if self.terrain.algorithm != SurfaceAlgorithm::WorldV1 {
            return Err(TerrainError::InvalidConfig);
        }
        world.archetype.validate()?;
        self.world = Some(world);
        Ok(self)
    }
    pub fn world(&self) -> Option<&world_field::WorldDefinition> {
        self.world.as_ref()
    }
    /// Add the band-limited fBm detail layer (pipeline §9.3) to any algorithm.
    pub fn with_detail_noise(
        mut self,
        definition: noise::DetailNoiseDefinition,
    ) -> Result<Self, TerrainError> {
        definition.validate()?;
        self.detail_noise = Some(definition);
        Ok(self)
    }
    pub fn detail_noise(&self) -> Option<&noise::DetailNoiseDefinition> {
        self.detail_noise.as_ref()
    }
    pub fn moon_fields(&self) -> Option<&MoonFieldDefinition> {
        self.moon_fields.as_ref()
    }
    pub fn height_profile(&self) -> Option<&TerrainHeightProfile> {
        self.height_profile.as_ref()
    }
    pub fn identity(&self) -> TerrainIdentity {
        self.identity
    }
    /// Authored prepared sources replace geology and material-noise evaluation.
    /// The existing shape/query/resident boundary continues to own physical truth.
    pub fn from_prepared(
        identity: TerrainIdentity,
        seed: TerrainSeed,
        prepared: Arc<PreparedSurface>,
    ) -> Self {
        let mut definition = Self::generated(identity, seed, SurfaceAlgorithm::PreparedV1);
        definition.material.composition = 0.5;
        definition.material.regional_contrast = 0.0;
        definition.prepared = Some(prepared);
        definition
    }
    pub fn prepared(&self) -> Option<&Arc<PreparedSurface>> {
        self.prepared.as_ref()
    }
    pub fn seed(&self) -> TerrainSeed {
        self.seed
    }
    pub fn shape(&self) -> &ShapeDefinition {
        &self.shape
    }
    pub fn terrain(&self) -> SurfaceTerrainDefinition {
        self.terrain
    }
    pub fn material(&self) -> SurfaceMaterialDefinition {
        self.material
    }
    pub fn atmosphere(&self) -> SurfaceAtmosphere {
        self.atmosphere
    }
    fn terrain_seed(&self) -> u64 {
        mix(self.seed.0 ^ self.identity.0.rotate_left(19) ^ 0x4745_4f4c_4f47_0001)
    }
    pub fn terrain_identity(&self) -> u64 {
        let profile_identity = self.height_profile.as_ref().map_or(0, |profile| {
            profile
                .identity_words()
                .into_iter()
                .fold(0, |hash, word| mix(hash ^ word))
        });
        let prepared = self.prepared.as_ref().map_or(0, |p| {
            let mut bytes = [0; 8];
            bytes.copy_from_slice(&p.content_identity()[..8]);
            u64::from_le_bytes(bytes)
        });
        mix(self.terrain_seed()
            ^ self.terrain.configuration_identity()
            ^ profile_identity
            ^ self
                .moon_fields
                .map_or(0, MoonFieldDefinition::configuration_identity)
            ^ self.detail_noise.map_or(0, |d| d.configuration_identity())
            ^ self.world.as_ref().map_or(0, world_field::WorldDefinition::identity)
            ^ prepared)
    }
    pub fn geometry_identity(&self) -> u64 {
        mix(self.terrain_identity() ^ self.shape.configuration_identity())
    }
    pub fn material_identity(&self) -> u64 {
        mix(self.terrain_identity()
            ^ self.material.configuration_identity()
            ^ mix(self.seed.0 ^ 0x4d41_5445_5249_0001))
    }
    pub fn configuration_identity(&self) -> u64 {
        mix(self.geometry_identity() ^ self.material_identity() ^ self.atmosphere_identity)
    }
    pub fn validate_radius(&self, radius_m: f64) -> Result<(), TerrainError> {
        SurfaceGenerator::new(self, radius_m).map(|_| ())
    }
}

/// Actual bounded candidate work for a complete point query, not timings.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceQueryWork {
    pub cells_visited: u32,
    pub candidate_features: u32,
    /// Local centres passing the spherical support test. A profile can still
    /// have zero influence there; `None` means the preserved oracle has no count.
    pub accepted_features: Option<u32>,
}

/// Region-local feature-support or province-mask probes for numerical corpora.
/// These are diagnostic queries, not a body-wide feature catalogue.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceBoundaryProbe {
    pub label: &'static str,
    pub location: SurfaceLocation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSample {
    shape: ShapeSample,
    terrain: TerrainSample,
    weights: [f64; 4],
    radius_m: f64,
    normal: DVec3,
    work: SurfaceQueryWork,
}
impl SurfaceSample {
    pub fn shape(self) -> ShapeSample {
        self.shape
    }
    pub fn terrain(self) -> TerrainSample {
        self.terrain
    }
    pub fn material_weights(self) -> [f64; 4] {
        self.weights
    }
    pub fn radius_m(self) -> f64 {
        self.radius_m
    }
    pub fn normal(self) -> DVec3 {
        self.normal
    }
    pub fn position(self, location: SurfaceLocation) -> DVec3 {
        location.direction().unit() * self.radius_m
    }
    pub fn work(self) -> SurfaceQueryWork {
        self.work
    }
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // Keep existing compiled source storage unchanged in the rendering comparison.
enum GeologicalField {
    Prepared(Arc<PreparedSurface>),
    Rocky(Box<MoonTerrainGenerator>),
    Other(geology::GeologyField),
    Province(provinces::ProvinceField),
    Hierarchical(Box<hierarchy::HierarchicalField>),
    MoonFields(moon_fields::MoonFieldsV1),
    MoonProfile(moon_profile::MoonProfileField),
    World(world_field::WorldField),
}
/// Immutable compiled query adapter shared by reference and production callers.
#[derive(Debug, Clone)]
pub struct SurfaceGenerator {
    definition: SurfaceDefinition,
    radius_m: f64,
    field: GeologicalField,
    detail: Option<noise::DetailNoise>,
    height_bound_m: f64,
    envelope_m: [f64; 2],
    preparation_store: Arc<OnceLock<Option<Arc<query_context::SurfacePreparationStore>>>>,
}
impl SurfaceGenerator {
    pub fn new(definition: &SurfaceDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        definition.terrain.parameters.validate()?;
        let shape = definition.shape.conservative_radius_envelope_m(radius_m)?;
        let p = definition.terrain.parameters;
        let (field, height_bound_m) = match definition.terrain.algorithm {
            SurfaceAlgorithm::PreparedV1 => {
                let g = definition
                    .prepared
                    .as_ref()
                    .ok_or(TerrainError::InvalidConfig)?;
                g.validate_radius(radius_m)
                    .map_err(|_| TerrainError::InvalidConfig)?;
                let bounds = g.displacement_bounds_m();
                (
                    GeologicalField::Prepared(Arc::clone(g)),
                    bounds[0].abs().max(bounds[1].abs()).next_up(),
                )
            }
            SurfaceAlgorithm::RockyV3 => {
                let scale = p.relief_fraction / 0.006;
                let c = MoonTerrainConfig::new(
                    0.0015 * scale,
                    0.001 * scale * (1.1 - 0.5 * p.age),
                    [
                        p.feature_scale_fraction,
                        p.feature_scale_fraction * 0.075,
                        0.00003,
                    ],
                    [
                        0.0025 * scale * (0.35 + 0.65 * p.impact_retention),
                        0.0008 * scale,
                        0.000018 * scale,
                    ],
                )?;
                let d = MoonTerrainDefinition::with_version(
                    definition.identity,
                    TerrainSeed(definition.terrain_seed()),
                    c,
                    MoonTerrainVersion::MoonLikeV2,
                );
                let g = MoonTerrainGenerator::new(&d, radius_m)?;
                let bound = (g.conservative_absolute_height_bound_m() + radius_m * 0.001).next_up();
                (GeologicalField::Rocky(Box::new(g)), bound)
            }
            SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::VolcanicV2 => {
                let g = provinces::ProvinceField::new(
                    definition.terrain.algorithm,
                    p,
                    definition.terrain_seed(),
                    radius_m,
                )?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::Province(g), bound)
            }
            algorithm @ (SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV3) => {
                let g = hierarchy::HierarchicalField::new(
                    algorithm,
                    p,
                    definition.terrain_seed(),
                    radius_m,
                )?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::Hierarchical(Box::new(g)), bound)
            }
            SurfaceAlgorithm::MoonFieldsV1 => {
                let g = moon_fields::MoonFieldsV1::new(
                    p,
                    definition.terrain_seed(),
                    radius_m,
                    definition.moon_fields.unwrap_or_default(),
                )?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::MoonFields(g), bound)
            }
            SurfaceAlgorithm::WorldV1 => {
                let world = definition.world.as_ref().ok_or(TerrainError::InvalidConfig)?;
                let g = world_field::WorldField::new(world, radius_m)?;
                // Bicubic (Catmull-Rom) sampling overshoots its samples: the
                // absolute weights sum to at most 1.25 per axis (t = 1/2), so
                // 1.25² bounds the 2-D tensor.
                (GeologicalField::World(g), (world.height_bound_m() * 1.5625).next_up())
            }
            SurfaceAlgorithm::MoonProfileV1 => {
                let profile = definition
                    .height_profile
                    .as_ref()
                    .ok_or(TerrainError::InvalidConfig)?;
                let g = moon_profile::MoonProfileField::new(profile.clone(), radius_m)?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::MoonProfile(g), bound)
            }
            algorithm => {
                let g =
                    geology::GeologyField::new(algorithm, p, definition.terrain_seed(), radius_m)?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::Other(g), bound)
            }
        };
        let detail = definition
            .detail_noise
            .as_ref()
            .map(|d| noise::DetailNoise::new(d, definition.terrain_seed()))
            .transpose()?;
        let height_bound_m =
            (height_bound_m + detail.as_ref().map_or(0.0, noise::DetailNoise::bound_m)).next_up();
        let envelope_m = [
            (shape[0] - height_bound_m).next_down(),
            (shape[1] + height_bound_m).next_up(),
        ];
        if !height_bound_m.is_finite()
            || height_bound_m < 0.0
            || envelope_m[0] <= 0.001
            || !envelope_m[1].is_finite()
        {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            definition: definition.clone(),
            radius_m,
            field,
            detail,
            height_bound_m,
            envelope_m,
            preparation_store: Arc::new(OnceLock::new()),
        })
    }

    /// Compiled detail layer, if the definition has one.
    pub fn detail_noise(&self) -> Option<&noise::DetailNoise> {
        self.detail.as_ref()
    }

    /// Heap payload currently retained by this compiled generator's owned
    /// allocations. It counts `Box`, profile-map, and detail-metadata payloads
    /// while counting Arc-shared profile maps once, and excludes allocator metadata,
    /// the inline `SurfaceGenerator` itself, and caller-owned diagnostic output.
    /// Profile allocations shared between the inline definition and compiled
    /// field are counted once through the field. Other geological state is inline.
    /// Includes immutable prepared-source payloads when selected. Shared Arc
    /// ownership is counted conservatively once per logical generator.
    pub fn resident_heap_bytes(&self) -> usize {
        let compiled = match &self.field {
            GeologicalField::Prepared(field) => field.resident_bytes(),
            GeologicalField::Rocky(_) => std::mem::size_of::<MoonTerrainGenerator>(),
            GeologicalField::Other(_) => 0,
            GeologicalField::Province(field) => field.resident_heap_bytes(),
            GeologicalField::Hierarchical(field) => field.resident_heap_bytes(),
            GeologicalField::MoonFields(_) => 0,
            GeologicalField::MoonProfile(field) => field.resident_heap_bytes(),
            GeologicalField::World(field) => field.resident_heap_bytes(),
        };
        let store_bytes = self
            .preparation_store
            .get()
            .and_then(Option::as_ref)
            .map_or(0, |store| store.stats().retained_bytes);
        compiled
            .saturating_add(PREPARATION_STORE_OWNER_BYTES)
            .saturating_add(store_bytes)
    }

    /// Heap scratch required by the scalar and batch authoritative sampling
    /// paths. These paths use fixed-size local state and do not allocate. The
    /// diagnostic probe APIs return caller-owned `Vec`s and are outside this
    /// worker-query workspace estimate.
    pub const fn query_workspace_bytes(&self) -> usize {
        0
    }

    /// Fixed evaluator stack cache size; no current algorithm keeps one.
    pub const fn query_stack_bytes(&self) -> usize {
        0
    }

    /// Upper bound, in heap payload bytes, for a compiled surface generator
    /// used by scalar authoritative queries, across all supported algorithms,
    /// including its scalar/batch query workspace. Current compiled fields have
    /// fixed `Box` payloads plus bounded profile maps and detail metadata:
    /// Rocky V3 stores one Moon generator; Rocky V4 stores a province field and
    /// a Moon history; Rocky V5 adds a hierarchy field around that same parent.
    /// Prepared assets retain validated immutable data under their source cap.
    /// Legacy definitions, shapes and history tables are otherwise inline.
    /// It includes metadata for the lazy preparation-store owner, but excludes
    /// the optional store itself because scalar query paths never initialize it.
    /// This excludes allocator bookkeeping, the inline generator value, and
    /// caller-owned diagnostic output.
    pub fn scalar_working_heap_bound_bytes() -> usize {
        let moon = std::mem::size_of::<MoonTerrainGenerator>();
        let province_with_rocky_history = std::mem::size_of::<provinces::ProvinceField>() + moon;
        let hierarchical_with_rocky_history =
            std::mem::size_of::<hierarchy::HierarchicalField>() + province_with_rocky_history;
        moon.max(province_with_rocky_history)
            .max(hierarchical_with_rocky_history)
            .max(moon_profile::MAX_PROFILE_WORKING_HEAP_BYTES)
            .max(super::PREPARED_SOURCE_CAP_BYTES)
            .saturating_add(PREPARATION_STORE_OWNER_BYTES)
    }

    /// Upper bound, in heap payload bytes, for a compiled generator that may
    /// use every query mode, including its optional bounded prepared store.
    /// Scalar-only callers should use [`Self::scalar_working_heap_bound_bytes`]
    /// so they do not reserve capacity for state that they will never create.
    pub fn working_heap_bound_bytes() -> usize {
        Self::scalar_working_heap_bound_bytes()
            .saturating_add(query_context::PREPARED_STORE_CAP_BYTES)
    }
    pub fn definition(&self) -> &SurfaceDefinition {
        &self.definition
    }
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }
    pub fn conservative_absolute_height_bound_m(&self) -> f64 {
        self.height_bound_m
    }
    pub fn conservative_radius_envelope_m(&self) -> [f64; 2] {
        self.envelope_m
    }
    /// Evaluate direction-local geological controls. Existing historical
    /// families return `None` because their semantics predate this director.
    pub fn geological_controls(
        &self,
        location: SurfaceLocation,
    ) -> Result<Option<GeologicalControls>, TerrainError> {
        match &self.field {
            GeologicalField::Province(field) => {
                Ok(Some(field.controls(location.direction().unit())?))
            }
            GeologicalField::Hierarchical(field) => {
                Ok(Some(field.controls(location.direction().unit())?))
            }
            GeologicalField::Rocky(_)
            | GeologicalField::Other(_)
            | GeologicalField::MoonFields(_)
            | GeologicalField::World(_)
            | GeologicalField::Prepared(_) => Ok(None),
            GeologicalField::MoonProfile(_) => Ok(None),
        }
    }

    /// Derived contributions from the same complete query. Historical algorithms
    /// predate this residual decomposition and return `None`.
    pub fn detail_diagnostics(
        &self,
        location: SurfaceLocation,
    ) -> Result<Option<SurfaceDetailDiagnostics>, TerrainError> {
        match &self.field {
            GeologicalField::Hierarchical(field) => {
                Ok(Some(field.diagnostics(location.direction().unit())?))
            }
            _ => Ok(None),
        }
    }

    /// Select deterministic nearby directions with strong expression of each
    /// generated province. This is metadata tooling, not a feature catalogue.
    pub fn diagnostic_province_probes(
        &self,
        near: SurfaceLocation,
    ) -> Result<Vec<SurfaceBoundaryProbe>, TerrainError> {
        if !matches!(
            &self.field,
            GeologicalField::Province(_) | GeologicalField::Hierarchical(_)
        ) {
            return Ok(Vec::new());
        }
        let n = near.direction().unit();
        let axis = if n.y.abs() < 0.8 { DVec3::Y } else { DVec3::X };
        let u = n.cross(axis).normalize();
        let v = n.cross(u);
        let mut best: [Option<(f64, DVec3)>; 4] = [None; 4];
        for ring in 0..5 {
            let theta = (ring as f64 + 1.0) * 0.08;
            for step in 0..48 {
                let angle = step as f64 * std::f64::consts::TAU / 48.0;
                let direction = (n * theta.cos()
                    + (u * angle.cos() + v * angle.sin()) * theta.sin())
                .normalize();
                let controls = self
                    .geological_controls(SurfaceLocation::new(
                        Direction3::try_new(direction)
                            .map_err(|_| TerrainError::NonFiniteResult)?,
                    ))?
                    .ok_or(TerrainError::InvalidConfig)?;
                for (i, item) in best.iter_mut().enumerate() {
                    if item.is_none_or(|(score, _)| controls.province_weights[i] > score) {
                        *item = Some((controls.province_weights[i], direction));
                    }
                }
            }
        }
        let mut probes = Vec::with_capacity(4);
        for (i, item) in best.into_iter().enumerate() {
            if let Some((_, direction)) = item {
                probes.push(SurfaceBoundaryProbe {
                    label: match i {
                        0 => "province-0-near-max",
                        1 => "province-1-near-max",
                        2 => "province-2-near-max",
                        _ => "province-3-near-max",
                    },
                    location: SurfaceLocation::new(
                        Direction3::try_new(direction)
                            .map_err(|_| TerrainError::NonFiniteResult)?,
                    ),
                });
            }
        }
        Ok(probes)
    }
    /// Returns actual support/province transition probes. Selection is fixed
    /// and deterministic; it does not alter the complete field or choose art seeds.
    pub fn diagnostic_boundary_probes(
        &self,
        near: SurfaceLocation,
    ) -> Result<Vec<SurfaceBoundaryProbe>, TerrainError> {
        let mut output = Vec::new();
        match &self.field {
            GeologicalField::Prepared(_) => {}
            GeologicalField::Other(field) => {
                for (label, n) in field.diagnostic_boundary_probes(near.direction().unit())? {
                    output.push(SurfaceBoundaryProbe {
                        label,
                        location: SurfaceLocation::new(
                            Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                        ),
                    });
                }
            }
            GeologicalField::Province(field) => {
                for (label, n) in field.diagnostic_boundary_probes(near.direction().unit())? {
                    output.push(SurfaceBoundaryProbe {
                        label,
                        location: SurfaceLocation::new(
                            Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                        ),
                    });
                }
            }
            GeologicalField::Hierarchical(field) => {
                for (label, n) in field.diagnostic_boundary_probes(near.direction().unit())? {
                    output.push(SurfaceBoundaryProbe {
                        label,
                        location: SurfaceLocation::new(
                            Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                        ),
                    });
                }
            }
            GeologicalField::MoonFields(_)
            | GeologicalField::MoonProfile(_)
            | GeologicalField::World(_) => {}
            GeologicalField::Rocky(_) => {
                let p = self.definition.terrain.parameters;
                let frequency = 2.2 + 3.0 * p.activity;
                let lo = 0.38 - p.resurfacing_fraction * 0.9;
                let seed = mix(self.definition.terrain_seed() ^ 0x5052_4f56_494e_0001);
                let value = |n: DVec3| {
                    gradient_noise(seed, n * frequency)
                        .map(|s| s.value)
                        .map_err(|_| TerrainError::NonFiniteResult)
                };
                let n = near.direction().unit();
                let axis = if n.y.abs() < 0.8 { DVec3::Y } else { DVec3::X };
                let u = n.cross(axis).normalize();
                let v = n.cross(u);
                for (label, target) in [("rocky-plains-onset", lo), ("rocky-plains-full", lo + 0.2)]
                {
                    'search: for z in [-0.8_f64, -0.4, 0.0, 0.4, 0.8] {
                        let point = |k: usize| {
                            let a = k as f64 * std::f64::consts::TAU / 32.0;
                            (n * z + (u * a.cos() + v * a.sin()) * (1.0 - z * z).sqrt()).normalize()
                        };
                        for index in 0..32 {
                            let mut a = point(index);
                            let mut b = point(index + 1);
                            let mut da = value(a)? - target;
                            let db = value(b)? - target;
                            if da * db > 0.0 {
                                continue;
                            }
                            for _ in 0..48 {
                                let mid = (a + b).normalize();
                                let dm = value(mid)? - target;
                                if da * dm > 0.0 {
                                    a = mid;
                                    da = dm;
                                } else {
                                    b = mid;
                                }
                            }
                            let center = (a + b).normalize();
                            let tangent = (point(index + 1) - point(index)).normalize();
                            for offset in [-1e-7, 0.0, 1e-7] {
                                output.push(SurfaceBoundaryProbe {
                                    label,
                                    location: SurfaceLocation::new(
                                        Direction3::try_new(center + tangent * offset)
                                            .map_err(|_| TerrainError::NonFiniteResult)?,
                                    ),
                                });
                            }
                            break 'search;
                        }
                    }
                }
            }
        }
        Ok(output)
    }

    /// Return deterministic locations on actual fine-scale geological forms
    /// near `near`. Families with no bounded local feature in the fixed query
    /// halo return no landmarks; callers may then select a broad terrain point.
    pub fn diagnostic_landmark_probes(
        &self,
        near: SurfaceLocation,
    ) -> Result<Vec<SurfaceBoundaryProbe>, TerrainError> {
        let mut output = Vec::new();
        if let GeologicalField::Other(field) = &self.field {
            for (label, n) in field.diagnostic_landmark_probes(near.direction().unit())? {
                output.push(SurfaceBoundaryProbe {
                    label,
                    location: SurfaceLocation::new(
                        Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                    ),
                });
            }
        } else if let GeologicalField::Province(field) = &self.field {
            for (label, n) in field.diagnostic_landmark_probes(near.direction().unit())? {
                output.push(SurfaceBoundaryProbe {
                    label,
                    location: SurfaceLocation::new(
                        Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                    ),
                });
            }
        }
        if let GeologicalField::Hierarchical(field) = &self.field {
            for (label, n) in field.diagnostic_landmark_probes(near.direction().unit())? {
                output.push(SurfaceBoundaryProbe {
                    label,
                    location: SurfaceLocation::new(
                        Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                    ),
                });
            }
        }
        Ok(output)
    }

    /// Create a bounded, generator-bound cache for a worker or tile build.
    pub fn query_context(&self) -> SurfaceQueryContext<'_> {
        SurfaceQueryContext::new(self)
    }

    /// Prepare bounded f64 scalar recipe inputs for the experimental MoonFieldsV1
    /// derived-field producer. These contain terrain height and material data;
    /// callers must separately query and apply the body shape. This API is not
    /// collision or world authority. Other algorithms remain on their existing path.
    pub fn prepare_moon_field_point(
        &self,
        location: SurfaceLocation,
    ) -> Result<MoonFieldPointInputs, TerrainError> {
        let GeologicalField::MoonFields(field) = &self.field else {
            return Err(TerrainError::InvalidConfig);
        };
        let mut inputs = field.prepare(location.direction().unit())?;
        let regional_signal = inputs.regional().clamp(-1.0, 1.0);
        inputs.set_material_emphasis(
            self.definition.material.composition,
            self.definition.material.regional_contrast,
            regional_signal,
        );
        Ok(inputs)
    }

    /// Create a worker-private query context backed by the lazy, bounded store
    /// shared by contexts for this exact compiled generator.
    pub fn prepared_query_context(&self) -> SurfaceQueryContext<'_> {
        let store = self
            .preparation_store
            .get_or_init(|| query_context::SurfacePreparationStore::new().map(Arc::new))
            .clone();
        store.map_or_else(
            || self.query_context(),
            |store| SurfaceQueryContext::new_prepared(self, store),
        )
    }

    /// Create a context with aggregate CPU timing enabled on the scalar
    /// reference path. Local feature/control memoization is disabled so timing
    /// reflects repeated independent scalar evaluations.
    pub fn profiled_query_context(&self) -> SurfaceQueryContext<'_> {
        let mut context = SurfaceQueryContext::new(self);
        context.enable_profile(true);
        context.disable_local_cache();
        context
    }

    pub fn evaluate_point(&self, location: SurfaceLocation) -> Result<SurfaceSample, TerrainError> {
        self.evaluate_point_inner(location, None, &mut NoopProfileObserver)
    }

    /// Evaluate one MoonProfile point and collect opt-in source-layer timings.
    /// Other geology paths return `InvalidConfig`; use `evaluate_point` for them.
    pub fn evaluate_moon_profile_point_profiled(
        &self,
        location: SurfaceLocation,
        diagnostics: &mut MoonProfileEvaluationDiagnostics,
    ) -> Result<SurfaceSample, TerrainError> {
        if !matches!(self.field, GeologicalField::MoonProfile(_)) {
            return Err(TerrainError::InvalidConfig);
        }
        *diagnostics = MoonProfileEvaluationDiagnostics::default();
        let started = std::time::Instant::now();
        let mut observer = TimedProfileObserver(diagnostics);
        let result = self.evaluate_point_inner(location, None, &mut observer);
        diagnostics.total_surface_evaluation_ns = started.elapsed().as_nanos();
        result
    }

    fn evaluate_point_with_context(
        &self,
        location: SurfaceLocation,
        context: &mut SurfaceQueryContext<'_>,
    ) -> Result<SurfaceSample, TerrainError> {
        self.evaluate_point_inner(location, Some(context), &mut NoopProfileObserver)
    }

    fn evaluate_point_inner<O: ProfileObserver>(
        &self,
        location: SurfaceLocation,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
        observer: &mut O,
    ) -> Result<SurfaceSample, TerrainError> {
        let profiling = context
            .as_deref()
            .is_some_and(SurfaceQueryContext::profiling);
        let total_started = profiling.then(Instant::now);
        let n = location.direction().unit();
        let shape_started = profiling.then(Instant::now);
        let shape = self
            .definition
            .shape
            .query(location.direction(), self.radius_m)?;
        let shape_ns = shape_started.map_or(0, duration_ns);
        let geology_started = profiling.then(Instant::now);
        let (height_m, gradient, mut weights, work) = match &self.field {
            GeologicalField::Prepared(field) => {
                let s = field
                    .sample(n, self.radius_m)
                    .map_err(|_| TerrainError::NonFiniteResult)?;
                (
                    s.height_m,
                    s.gradient_m,
                    s.weights,
                    SurfaceQueryWork::default(),
                )
            }
            GeologicalField::Rocky(g) => {
                let p = self.definition.terrain.parameters;
                let rotation = DMat3::from_rotation_z(p.orientation_radians);
                let rotated = SurfaceLocation::new(
                    Direction3::try_new(rotation * n).map_err(|_| TerrainError::NonFiniteResult)?,
                );
                let s = if let Some(context) = context.as_deref_mut() {
                    g.evaluate_point_with_context(rotated, context)?
                } else {
                    g.evaluate_point(rotated)?
                };
                let frequency = 2.2 + 3.0 * p.activity;
                let v = gradient_noise(
                    mix(self.definition.terrain_seed() ^ 0x5052_4f56_494e_0001),
                    n * frequency,
                )
                .map_err(|_| TerrainError::NonFiniteResult)?;
                let lo = 0.38 - p.resurfacing_fraction * 0.9;
                let t = ((v.value - lo) / 0.2).clamp(0.0, 1.0);
                let plains = t * t * (3.0 - 2.0 * t);
                let plains_gradient = v.gradient * (frequency * 6.0 * t * (1.0 - t) / 0.2);
                let h = s.terrain().height_m();
                let factor = 1.0 - 0.95 * plains;
                let gradient = rotation.transpose()
                    * s.terrain().tangent_gradient_m_per_unit_direction()
                    * factor
                    - plains_gradient * (0.95 * h + self.radius_m * 0.001);
                let m = s.material();
                let ejecta = m.rock_weight() * 0.25 * (1.0 - plains);
                (
                    h * factor - plains * self.radius_m * 0.001,
                    gradient,
                    [
                        m.regolith_weight() * (1.0 - plains),
                        m.rock_weight() * (1.0 - plains) - ejecta,
                        plains + m.basalt_weight() * (1.0 - plains),
                        ejecta,
                    ],
                    // V2 examines 10 epochs x 2 layouts x 27 cells. Accepted
                    // feature count is unavailable in the preserved V2 oracle.
                    SurfaceQueryWork {
                        cells_visited: 540,
                        candidate_features: 540,
                        accepted_features: None,
                    },
                )
            }
            GeologicalField::Other(g) => {
                let s = g.evaluate(n)?;
                (s.height_m, s.gradient_m, s.weights, s.work)
            }
            GeologicalField::Province(g) => {
                let s = if let Some(context) = context.as_deref_mut() {
                    g.evaluate_with_query_context(n, context)?.0
                } else {
                    g.evaluate(n)?
                };
                (s.height_m, s.gradient_m, s.weights, s.work)
            }
            GeologicalField::Hierarchical(g) => {
                let s = if let Some(context) = context.as_deref_mut() {
                    g.evaluate_with_query_context(n, context)?
                } else {
                    g.evaluate(n)?
                };
                (s.height_m, s.gradient_m, s.weights, s.work)
            }
            GeologicalField::MoonFields(g) => g.evaluate(n, context.as_deref_mut())?,
            GeologicalField::World(field) => {
                let (h, gradient) = field.maps()?.sample(0, n);
                (h, gradient, [1.0, 0.0, 0.0, 0.0], SurfaceQueryWork::default())
            }
            GeologicalField::MoonProfile(g) => {
                let s = g.evaluate_observed(n, observer)?;
                (
                    s.height_m,
                    s.gradient_m,
                    s.weights,
                    SurfaceQueryWork::default(),
                )
            }
        };
        // Complete (unfiltered) detail layer, after the geology (§9.5).
        let (height_m, gradient) = match &self.detail {
            Some(detail) => {
                let (h, g) = detail.evaluate(n * self.radius_m, None);
                (height_m + h, gradient + g * self.radius_m)
            }
            None => (height_m, gradient),
        };
        let material_noise = match &self.field {
            GeologicalField::MoonFields(field) => field.material_modulation(n),
            GeologicalField::MoonProfile(_) | GeologicalField::Prepared(_) => 0.0,
            _ => {
                gradient_noise(mix(self.definition.seed.0 ^ 0x4d41_5445_5249_0001), n * 7.0)
                    .map_err(|_| TerrainError::NonFiniteResult)?
                    .value
            }
        };
        let geology_ns = geology_started.map_or(0, duration_ns);
        let material_started = profiling.then(Instant::now);
        if !matches!(self.field, GeologicalField::Prepared(_)) {
            let m = self.definition.material;
            for (index, w) in weights.iter_mut().enumerate() {
                let emphasis = (index as f64 - 1.5) / 1.5;
                *w *= 1.0
                    + emphasis
                        * ((m.composition - 0.5) * 0.6
                            + material_noise * m.regional_contrast * 0.3);
            }
        }
        let total = weights.iter().sum::<f64>();
        weights = weights.map(|w| w / total);
        let gradient = gradient - n * n.dot(gradient);
        let radius_m = shape.radius_m() + height_m;
        let normal = (n - (shape.gradient_m() + gradient) / radius_m).normalize();
        if !height_m.is_finite()
            || !gradient.is_finite()
            || !normal.is_finite()
            || weights
                .iter()
                .any(|w| !w.is_finite() || !(0.0..=1.0).contains(w))
            || radius_m < self.envelope_m[0]
            || radius_m > self.envelope_m[1]
        {
            return Err(TerrainError::NonFiniteResult);
        }
        if let Some(context) = context {
            context.record_surface_profile(
                shape_ns,
                geology_ns,
                material_started.map_or(0, duration_ns),
                total_started.map_or(0, duration_ns),
            );
        }
        Ok(SurfaceSample {
            shape,
            terrain: TerrainSample::from_parts(height_m, gradient),
            weights,
            radius_m,
            normal,
            work,
        })
    }
    pub fn evaluate_batch(
        &self,
        locations: &[SurfaceLocation],
        output: &mut [SurfaceSample],
    ) -> Result<(), TerrainError> {
        if locations.len() != output.len() {
            return Err(TerrainError::LengthMismatch);
        }
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate_point(*location)?;
        }
        Ok(())
    }
}

fn duration_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn hash_values(mut salt: u64, values: &[f64]) -> u64 {
    for &value in values {
        salt = mix(salt ^ if value == 0.0 { 0 } else { value.to_bits() });
    }
    salt
}

fn unit(seed: u64) -> f64 {
    (mix(seed) >> 11) as f64 / ((1u64 << 53) as f64)
}
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[cfg(test)]
mod landmark_probe_tests {
    use super::*;

    #[test]
    fn public_landmark_probes_are_family_specific_and_deterministic() {
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let definition = SurfaceDefinition::generated(
                crate::terrain::TerrainIdentity(0x1234),
                crate::terrain::TerrainSeed(0x5eed),
                algorithm,
            );
            let generator = SurfaceGenerator::new(&definition, 109_000.0).unwrap();
            let mut near = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
            let mut probes = Vec::new();
            for direction in [
                DVec3::Z,
                DVec3::X,
                DVec3::Y,
                -DVec3::Z,
                -DVec3::X,
                -DVec3::Y,
            ] {
                near = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
                probes = generator.diagnostic_landmark_probes(near).unwrap();
                if !probes.is_empty() {
                    break;
                }
            }
            let repeated = generator.diagnostic_landmark_probes(near).unwrap();
            assert_eq!(probes.len(), repeated.len());
            assert!(!probes.is_empty(), "no probes for {algorithm:?}");
            for (probe, repeat) in probes.iter().zip(&repeated) {
                assert_eq!(probe.label, repeat.label);
                assert_eq!(probe.location.direction(), repeat.location.direction());
                assert!(match algorithm {
                    SurfaceAlgorithm::IcyV1 => probe.label.starts_with("icy-fracture-epoch3-"),
                    SurfaceAlgorithm::VolcanicV1 => {
                        probe.label.starts_with("volcanic-epoch2-")
                    }
                    SurfaceAlgorithm::PreparedV1
                    | SurfaceAlgorithm::WorldV1
                    | SurfaceAlgorithm::RockyV3
                    | SurfaceAlgorithm::RockyV4
                    | SurfaceAlgorithm::IcyV2
                    | SurfaceAlgorithm::VolcanicV2
                    | SurfaceAlgorithm::RockyV5
                    | SurfaceAlgorithm::IcyV3
                    | SurfaceAlgorithm::VolcanicV3
                    | SurfaceAlgorithm::MoonFieldsV1 => false,
                    SurfaceAlgorithm::MoonProfileV1 => false,
                });
            }
        }
    }
}

#[cfg(test)]
mod memory_accounting_tests {
    use super::*;

    #[test]
    fn scalar_bound_excludes_only_the_lazy_optional_prepared_store() {
        let definition = SurfaceDefinition::generated(
            crate::terrain::TerrainIdentity(0x1234),
            crate::terrain::TerrainSeed(0x5eed),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 109_000.0).unwrap();
        let scalar_bound = SurfaceGenerator::scalar_working_heap_bound_bytes();
        let all_modes_bound = SurfaceGenerator::working_heap_bound_bytes();

        assert_eq!(
            all_modes_bound - scalar_bound,
            query_context::PREPARED_STORE_CAP_BYTES
        );
        assert!(generator.preparation_store.get().is_none());
        let _ = generator
            .evaluate_point(SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap()))
            .unwrap();
        assert!(generator.preparation_store.get().is_none());
        assert!(generator.resident_heap_bytes() <= scalar_bound);
    }

    #[test]
    fn every_supported_algorithm_fits_the_static_heap_bound() {
        for algorithm in [
            SurfaceAlgorithm::RockyV3,
            SurfaceAlgorithm::RockyV4,
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV1,
            SurfaceAlgorithm::IcyV2,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV1,
            SurfaceAlgorithm::VolcanicV2,
            SurfaceAlgorithm::VolcanicV3,
            SurfaceAlgorithm::MoonFieldsV1,
            SurfaceAlgorithm::MoonProfileV1,
        ] {
            let mut definition = SurfaceDefinition::generated(
                crate::terrain::TerrainIdentity(0x1234),
                crate::terrain::TerrainSeed(0x5eed),
                algorithm,
            );
            if algorithm == SurfaceAlgorithm::MoonProfileV1 {
                let profile =
                    TerrainHeightProfile::from_u16_le(2, 2, &[0, 0, 0, 0, 0, 0, 0, 0]).unwrap();
                definition = definition.with_height_profile(profile).unwrap();
            }
            let generator = SurfaceGenerator::new(&definition, 109_000.0).unwrap();
            assert_eq!(generator.query_workspace_bytes(), 0, "{algorithm:?}");
            assert!(
                generator.resident_heap_bytes() <= SurfaceGenerator::working_heap_bound_bytes(),
                "{algorithm:?}: {} > {}",
                generator.resident_heap_bytes(),
                SurfaceGenerator::working_heap_bound_bytes(),
            );
        }
    }
}

#[cfg(test)]
mod prepared_surface_tests {
    use super::*;

    fn generator_at_radius(radius_m: f64) -> SurfaceGenerator {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x7068_6173_6532_6601),
            TerrainSeed(0x9c41_287a),
            SurfaceAlgorithm::RockyV5,
        );
        SurfaceGenerator::new(&definition, radius_m).unwrap()
    }

    fn vector_bits(value: DVec3) -> [u64; 3] {
        [value.x.to_bits(), value.y.to_bits(), value.z.to_bits()]
    }

    fn assert_bitwise_equal(a: SurfaceSample, b: SurfaceSample) {
        assert_eq!(
            a.shape().radius_m().to_bits(),
            b.shape().radius_m().to_bits()
        );
        assert_eq!(
            vector_bits(a.shape().gradient_m()),
            vector_bits(b.shape().gradient_m())
        );
        assert_eq!(
            vector_bits(a.shape().normal()),
            vector_bits(b.shape().normal())
        );
        assert_eq!(
            a.terrain().height_m().to_bits(),
            b.terrain().height_m().to_bits()
        );
        assert_eq!(
            vector_bits(a.terrain().tangent_gradient_m_per_unit_direction()),
            vector_bits(b.terrain().tangent_gradient_m_per_unit_direction())
        );
        assert_eq!(
            a.material_weights().map(f64::to_bits),
            b.material_weights().map(f64::to_bits)
        );
        assert_eq!(a.radius_m().to_bits(), b.radius_m().to_bits());
        assert_eq!(vector_bits(a.normal()), vector_bits(b.normal()));
        assert_eq!(a.work(), b.work());
    }

    #[test]
    fn prepared_legacy_windows_match_scalar_history_bitwise_and_share_across_workers() {
        for radius_m in [109_081.776_8, 1_737_400.0] {
            let definition = SurfaceDefinition::generated(
                TerrainIdentity(0x7068_6173_6532_6601),
                TerrainSeed(0x9c41_287a),
                SurfaceAlgorithm::RockyV5,
            );
            let generator = SurfaceGenerator::new(&definition, radius_m).unwrap();
            let mut scalar = generator.query_context();
            let mut prepared_a = generator.prepared_query_context();
            let mut prepared_b = generator.prepared_query_context();
            let directions = [
                DVec3::Z,
                DVec3::X,
                DVec3::Y,
                -DVec3::Z,
                -DVec3::X,
                -DVec3::Y,
                DVec3::ONE.normalize(),
                DVec3::new(-0.32, 0.74, 0.59).normalize(),
            ];
            for (index, direction) in directions.into_iter().enumerate() {
                let location = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
                let reference = scalar.evaluate_point(location).unwrap();
                let prepared = prepared_a.evaluate_point(location).unwrap();
                assert_bitwise_equal(reference, prepared);
                if index < 2 {
                    let sibling = prepared_b.evaluate_point(location).unwrap();
                    assert_bitwise_equal(reference, sibling);
                }
            }
            assert!(prepared_a.leased_bytes() <= prepared_a.lease_bound_bytes());
            assert!(prepared_b.leased_bytes() <= prepared_b.lease_bound_bytes());
            let store = generator.prepared_query_context().store_stats();
            assert!(store.pages_built > 0);
            assert!(
                store.page_hits > 0,
                "sibling context should reuse shared pages"
            );
            assert!(store.prepared_cells > 0);
            assert!(store.retained_bytes <= store.capacity_bytes);
            assert!(
                generator.resident_heap_bytes() <= SurfaceGenerator::working_heap_bound_bytes()
            );
        }
    }

    #[test]
    fn opt_in_profile_preserves_scalar_samples_and_reports_layer_work() {
        for radius_m in [109_081.776_8, 1_737_400.0] {
            let generator = generator_at_radius(radius_m);
            let mut profiled = generator.profiled_query_context();
            for direction in [
                DVec3::X,
                DVec3::Y,
                DVec3::Z,
                DVec3::new(0.3, -0.7, 0.6).normalize(),
            ] {
                let location = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
                let reference = generator.evaluate_point(location).unwrap();
                let measured = profiled.evaluate_point(location).unwrap();
                assert_bitwise_equal(reference, measured);
            }
            // Exact counters: each sample is recorded only after its final
            // material/normal stage completes.
            assert_eq!(profiled.profile_stats().samples, 4);
            assert_eq!(profiled.stats().sample_evaluations, 4);

            // Single spans (notably material/normal, a few float ops) can be
            // shorter than one timer tick, so assert accumulated time over
            // enough distinct samples instead of four.
            const TIMING_SAMPLES: u64 = 256;
            for index in 0..TIMING_SAMPLES {
                let location = SurfaceLocation::new(
                    Direction3::try_new(fibonacci_direction(index, TIMING_SAMPLES)).unwrap(),
                );
                profiled.evaluate_point(location).unwrap();
            }
            let profile = profiled.profile_stats();
            assert_eq!(profile.samples, 4 + TIMING_SAMPLES);
            assert_eq!(profiled.stats().sample_evaluations, 4 + TIMING_SAMPLES);
            assert!(profile.surface_total_ns > 0);
            assert!(profile.legacy_history_ns > 0);
            assert!(profile.legacy_discovery_and_profile_ns > 0);
            assert!(profile.legacy_chronology_ns > 0);
            assert!(profile.province_exclusive_ns > 0);
            assert!(profile.hierarchy_exclusive_ns > 0);
            assert!(profile.final_material_normal_ns > 0);
        }
    }

    fn fibonacci_direction(index: u64, count: u64) -> DVec3 {
        let z = 1.0 - (2.0 * index as f64 + 1.0) / count as f64;
        let ring = (1.0 - z * z).sqrt();
        let azimuth = index as f64 * std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        DVec3::new(ring * azimuth.cos(), ring * azimuth.sin(), z)
    }
}
