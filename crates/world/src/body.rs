use astrum_math::{AngularVelocity3, LinearVelocity3, LocalPosition, UnitRotation};
use std::num::NonZeroU64;

/// Opaque runtime identity, unrelated to names and reference frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyId {
    pub(crate) namespace: NonZeroU64,
    pub(crate) index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BodyPropertyError {
    #[error("mass must be finite")]
    NonFiniteMass,
    #[error("mass must be positive")]
    NonPositiveMass,
    #[error("reference radius must be finite")]
    NonFiniteRadius,
    #[error("reference radius must be positive")]
    NonPositiveRadius,
    #[error("name must contain 1–128 UTF-8 bytes")]
    InvalidName,
}

/// Positive mass and coarse characteristic size; no shape or gravity model implied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyProperties {
    mass_kg: f64,
    reference_radius_m: f64,
}
impl BodyProperties {
    pub fn new(mass_kg: f64, reference_radius_m: f64) -> Result<Self, BodyPropertyError> {
        if !mass_kg.is_finite() {
            return Err(BodyPropertyError::NonFiniteMass);
        }
        if mass_kg <= 0.0 {
            return Err(BodyPropertyError::NonPositiveMass);
        }
        if !reference_radius_m.is_finite() {
            return Err(BodyPropertyError::NonFiniteRadius);
        }
        if reference_radius_m <= 0.0 {
            return Err(BodyPropertyError::NonPositiveRadius);
        }
        Ok(Self {
            mass_kg,
            reference_radius_m,
        })
    }
    pub fn mass_kg(self) -> f64 {
        self.mass_kg
    }
    pub fn reference_radius_m(self) -> f64 {
        self.reference_radius_m
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), BodyPropertyError> {
    if (1..=128).contains(&name.len()) {
        Ok(())
    } else {
        Err(BodyPropertyError::InvalidName)
    }
}

/// All fields are checked math values, making invalid numeric states unrepresentable.
/// Center/velocity and axial angular velocity use system axes; orientation maps
/// body-local axes to system axes. All fields share the containing system's time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyState {
    center_in_system: LocalPosition,
    center_velocity_in_system: LinearVelocity3,
    body_to_system: UnitRotation,
    angular_velocity_in_system: AngularVelocity3,
}
impl BodyState {
    pub fn new(
        center_in_system: LocalPosition,
        center_velocity_in_system: LinearVelocity3,
        body_to_system: UnitRotation,
        angular_velocity_in_system: AngularVelocity3,
    ) -> Self {
        Self {
            center_in_system,
            center_velocity_in_system,
            body_to_system,
            angular_velocity_in_system,
        }
    }
    pub fn center_in_system(self) -> LocalPosition {
        self.center_in_system
    }
    pub fn center_velocity_in_system(self) -> LinearVelocity3 {
        self.center_velocity_in_system
    }
    pub fn body_to_system(self) -> UnitRotation {
        self.body_to_system
    }
    pub fn angular_velocity_in_system(self) -> AngularVelocity3 {
        self.angular_velocity_in_system
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CelestialBody {
    pub(crate) name: String,
    pub(crate) properties: BodyProperties,
    pub(crate) state: BodyState,
    pub(crate) terrain: Option<crate::terrain::TerrainDefinition>,
    pub(crate) surface_definition: Option<crate::terrain::SurfaceDefinition>,
    pub(crate) terrain_revision: crate::terrain::TerrainRevision,
}
impl CelestialBody {
    pub fn terrain(&self) -> Option<&crate::terrain::TerrainDefinition> {
        self.terrain.as_ref()
    }
    pub fn terrain_revision(&self) -> crate::terrain::TerrainRevision {
        self.terrain_revision
    }
    /// New compositional surface authority, when this body uses the surface path.
    /// Legacy terrain remains available through [`Self::terrain`].
    pub fn surface_definition(&self) -> Option<&crate::terrain::SurfaceDefinition> {
        self.surface_definition.as_ref()
    }
    /// Whether this body has either a legacy terrain or compositional surface authority.
    pub fn has_surface(&self) -> bool {
        self.terrain.is_some() || self.surface_definition.is_some()
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn properties(&self) -> &BodyProperties {
        &self.properties
    }
    pub fn state(&self) -> &BodyState {
        &self.state
    }
}
