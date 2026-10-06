//! Composable complete body-surface authority. No view, frame or LOD input enters
//! these definitions. Shapes and geology are radial graphs, not collision solids.
use super::{
    MoonTerrainConfig, MoonTerrainDefinition, MoonTerrainGenerator, MoonTerrainVersion,
    TerrainError, TerrainIdentity, TerrainSample, TerrainSeed,
};
use glam::{DMat3, DVec3};
use mundaris_math::{Direction3, noise::gradient_noise, surface::SurfaceLocation};
mod shape;
pub use shape::*;
mod director;
mod geology;
mod hierarchy;
mod provinces;
pub use hierarchy::SurfaceDetailDiagnostics;

/// Independent geological algorithm namespaces; parameters remain explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceAlgorithm {
    RockyV3,
    RockyV4,
    RockyV5,
    IcyV1,
    IcyV2,
    IcyV3,
    VolcanicV1,
    VolcanicV2,
    VolcanicV3,
}
impl SurfaceAlgorithm {
    pub const fn code(self) -> u64 {
        match self {
            Self::RockyV3 => 0x524f_434b_0000_0003,
            Self::RockyV4 => 0x524f_434b_0000_0004,
            Self::RockyV5 => 0x524f_434b_0000_0005,
            Self::IcyV1 => 0x4943_4520_0000_0001,
            Self::IcyV2 => 0x4943_4520_0000_0002,
            Self::IcyV3 => 0x4943_4520_0000_0003,
            Self::VolcanicV1 => 0x564f_4c43_0000_0001,
            Self::VolcanicV2 => 0x564f_4c43_0000_0002,
            Self::VolcanicV3 => 0x564f_4c43_0000_0003,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::RockyV3 => "RockyV3",
            Self::RockyV4 => "RockyV4",
            Self::RockyV5 => "RockyV5",
            Self::IcyV1 => "IcyV1",
            Self::IcyV2 => "IcyV2",
            Self::IcyV3 => "IcyV3",
            Self::VolcanicV1 => "VolcanicV1",
            Self::VolcanicV2 => "VolcanicV2",
            Self::VolcanicV3 => "VolcanicV3",
        }
    }

    pub const fn province_names(self) -> [&'static str; 4] {
        match self {
            Self::RockyV3 | Self::RockyV4 | Self::RockyV5 => [
                "ancient_highlands",
                "basin_margin",
                "resurfaced_plains",
                "structural_uplands",
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
            Self::RockyV3 | Self::RockyV4 | Self::RockyV5 => [
                "retained_impacts",
                "basin_deformation",
                "burial_smoothing",
                "structural_ridges",
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
        let salt = mix(seed.0 ^ algorithm.code() ^ 0x5048_454e_4f54_0001);
        let age = 0.12 + 0.86 * unit(salt ^ 1);
        let activity = (1.0 - age) * (0.55 + 0.45 * unit(salt ^ 2));
        let (resurfacing, retention, relief, scale) = match algorithm {
            SurfaceAlgorithm::RockyV3 => (
                0.05 + 0.78 * activity,
                0.25 + 0.75 * age,
                0.004 + 0.005 * age,
                0.17 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::RockyV5 => (
                0.05 + 0.78 * activity,
                0.25 + 0.75 * age,
                0.004 + 0.005 * age,
                0.17 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::IcyV1 => (
                0.15 + 0.75 * activity,
                0.10 + 0.65 * age,
                0.002 + 0.004 * activity,
                0.16 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::IcyV3 => (
                0.15 + 0.75 * activity,
                0.10 + 0.65 * age,
                0.002 + 0.004 * activity,
                0.16 + 0.18 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::VolcanicV1 => (
                0.45 + 0.48 * (1.0 - age),
                0.05 + 0.65 * age,
                0.003 + 0.006 * activity,
                0.18 + 0.22 * unit(salt ^ 3),
            ),
            SurfaceAlgorithm::VolcanicV2 | SurfaceAlgorithm::VolcanicV3 => (
                0.45 + 0.48 * (1.0 - age),
                0.05 + 0.65 * age,
                0.003 + 0.006 * activity,
                0.18 + 0.22 * unit(salt ^ 3),
            ),
        };
        Self {
            age,
            activity,
            resurfacing_fraction: resurfacing,
            impact_retention: retention,
            relief_fraction: relief,
            feature_scale_fraction: scale,
            orientation_radians: unit(salt ^ 4) * std::f64::consts::TAU,
        }
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
        })
    }
    pub fn generated(algorithm: SurfaceAlgorithm, seed: TerrainSeed) -> Self {
        Self {
            algorithm,
            parameters: GeologicalParameters::generated(algorithm, seed),
        }
    }
    pub fn algorithm(self) -> SurfaceAlgorithm {
        self.algorithm
    }
    pub fn parameters(self) -> GeologicalParameters {
        self.parameters
    }
    pub fn configuration_identity(self) -> u64 {
        mix(self.algorithm.code() ^ self.parameters.identity())
    }
}

/// Material semantics are separately versioned from geology. A definition must
/// select the channel semantics compatible with its geological algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceMaterialVersion {
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
            SurfaceAlgorithm::RockyV3 => Self::RockyV1,
            SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::RockyV5 => Self::RockyV2,
            SurfaceAlgorithm::IcyV1 => Self::IcyV1,
            SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::IcyV3 => Self::IcyV2,
            SurfaceAlgorithm::VolcanicV1 => Self::VolcanicV1,
            SurfaceAlgorithm::VolcanicV2 | SurfaceAlgorithm::VolcanicV3 => Self::VolcanicV2,
        }
    }
    pub const fn code(self) -> u64 {
        match self {
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
    identity: TerrainIdentity,
    seed: TerrainSeed,
    shape: ShapeDefinition,
    terrain: SurfaceTerrainDefinition,
    material: SurfaceMaterialDefinition,
    atmosphere: SurfaceAtmosphere,
    atmosphere_identity: u64,
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
            identity,
            seed,
            shape,
            terrain,
            material,
            atmosphere,
            atmosphere_identity,
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
        }
    }
    pub fn identity(&self) -> TerrainIdentity {
        self.identity
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
        mix(self.terrain_seed() ^ self.terrain.configuration_identity())
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
enum GeologicalField {
    Rocky(Box<MoonTerrainGenerator>),
    Other(geology::GeologyField),
    Province(provinces::ProvinceField),
    Hierarchical(Box<hierarchy::HierarchicalField>),
}
/// Immutable compiled query adapter shared by reference and production callers.
#[derive(Debug, Clone)]
pub struct SurfaceGenerator {
    definition: SurfaceDefinition,
    radius_m: f64,
    field: GeologicalField,
    height_bound_m: f64,
    envelope_m: [f64; 2],
}
impl SurfaceGenerator {
    pub fn new(definition: &SurfaceDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        definition.terrain.parameters.validate()?;
        let shape = definition.shape.conservative_radius_envelope_m(radius_m)?;
        let p = definition.terrain.parameters;
        let (field, height_bound_m) = match definition.terrain.algorithm {
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
            algorithm => {
                let g =
                    geology::GeologyField::new(algorithm, p, definition.terrain_seed(), radius_m)?;
                let bound = g.absolute_height_bound_m();
                (GeologicalField::Other(g), bound)
            }
        };
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
            height_bound_m,
            envelope_m,
        })
    }

    /// Heap payload currently retained by this compiled generator's owned
    /// allocations. It counts `Box` payloads and excludes allocator metadata,
    /// the inline `SurfaceGenerator` itself, and caller-owned diagnostic output.
    /// `SurfaceDefinition` and `ShapeDefinition` are fixed-size values without
    /// heap storage, and the geology algorithms retain no variable-sized data.
    pub fn resident_heap_bytes(&self) -> usize {
        match &self.field {
            GeologicalField::Rocky(_) => std::mem::size_of::<MoonTerrainGenerator>(),
            GeologicalField::Other(_) => 0,
            GeologicalField::Province(field) => field.resident_heap_bytes(),
            GeologicalField::Hierarchical(field) => field.resident_heap_bytes(),
        }
    }

    /// Heap scratch required by the scalar and batch authoritative sampling
    /// paths. These paths use fixed-size local state and do not allocate. The
    /// diagnostic probe APIs return caller-owned `Vec`s and are outside this
    /// worker-query workspace estimate.
    pub const fn query_workspace_bytes(&self) -> usize {
        0
    }

    /// Upper bound, in heap payload bytes, for a compiled surface generator
    /// across all supported algorithms, including its scalar/batch query
    /// workspace. Current compiled fields have only fixed `Box` payloads:
    /// Rocky V3 stores one Moon generator; Rocky V4 stores a province field and
    /// a Moon history; Rocky V5 adds a hierarchy field around that same parent.
    /// Definitions, shapes, history tables, and other field state are inline.
    /// This excludes allocator bookkeeping, the inline generator value, and
    /// caller-owned diagnostic output.
    pub fn working_heap_bound_bytes() -> usize {
        let moon = std::mem::size_of::<MoonTerrainGenerator>();
        let province_with_rocky_history = std::mem::size_of::<provinces::ProvinceField>() + moon;
        let hierarchical_with_rocky_history =
            std::mem::size_of::<hierarchy::HierarchicalField>() + province_with_rocky_history;
        moon.max(province_with_rocky_history)
            .max(hierarchical_with_rocky_history)
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
            GeologicalField::Rocky(_) | GeologicalField::Other(_) => Ok(None),
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

    pub fn evaluate_point(&self, location: SurfaceLocation) -> Result<SurfaceSample, TerrainError> {
        let n = location.direction().unit();
        let shape = self
            .definition
            .shape
            .query(location.direction(), self.radius_m)?;
        let (height_m, gradient, mut weights, work) = match &self.field {
            GeologicalField::Rocky(g) => {
                let p = self.definition.terrain.parameters;
                let rotation = DMat3::from_rotation_z(p.orientation_radians);
                let rotated = SurfaceLocation::new(
                    Direction3::try_new(rotation * n).map_err(|_| TerrainError::NonFiniteResult)?,
                );
                let s = g.evaluate_point(rotated)?;
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
                let s = g.evaluate(n)?;
                (s.height_m, s.gradient_m, s.weights, s.work)
            }
            GeologicalField::Hierarchical(g) => {
                let s = g.evaluate(n)?;
                (s.height_m, s.gradient_m, s.weights, s.work)
            }
        };
        let material_noise =
            gradient_noise(mix(self.definition.seed.0 ^ 0x4d41_5445_5249_0001), n * 7.0)
                .map_err(|_| TerrainError::NonFiniteResult)?
                .value;
        let m = self.definition.material;
        for (index, w) in weights.iter_mut().enumerate() {
            let emphasis = (index as f64 - 1.5) / 1.5;
            *w *= 1.0
                + emphasis
                    * ((m.composition - 0.5) * 0.6 + material_noise * m.regional_contrast * 0.3);
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
                    SurfaceAlgorithm::RockyV3
                    | SurfaceAlgorithm::RockyV4
                    | SurfaceAlgorithm::IcyV2
                    | SurfaceAlgorithm::VolcanicV2
                    | SurfaceAlgorithm::RockyV5
                    | SurfaceAlgorithm::IcyV3
                    | SurfaceAlgorithm::VolcanicV3 => false,
                });
            }
        }
    }
}

#[cfg(test)]
mod memory_accounting_tests {
    use super::*;

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
        ] {
            let definition = SurfaceDefinition::generated(
                crate::terrain::TerrainIdentity(0x1234),
                crate::terrain::TerrainSeed(0x5eed),
                algorithm,
            );
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
