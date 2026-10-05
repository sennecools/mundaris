//! Authored prescribed motion, independent of mass, radius, rendering and gravity.

use crate::{BodyId, CelestialSystem, CelestialSystemError, SimulationInstant};
use mundaris_math::{Direction3, LocalPosition, UnitRotation};

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CelestialMotionError {
    #[error("invalid finite orbital dimensions, eccentricity, period or phase")]
    InvalidOrbit,
    #[error("spin rate must be finite")]
    InvalidSpin,
    #[error("each body requires exactly one complete motion definition")]
    IncompleteDefinitions,
    #[error("duplicate motion definition for {0:?}")]
    DuplicateBody(BodyId),
    #[error("orbital references contain a cycle")]
    ReferenceCycle,
    #[error(transparent)]
    System(#[from] CelestialSystemError),
}

/// XY orbital plane, +X periapsis; rotation maps that plane into system axes.
/// The reference supplies translation/velocity only, never axial rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EllipticOrbit {
    reference: BodyId,
    semi_major_axis_m: f64,
    eccentricity: f64,
    plane_to_system: UnitRotation,
    period_seconds: f64,
    mean_anomaly_at_epoch_radians: f64,
    epoch: SimulationInstant,
}
impl EllipticOrbit {
    pub fn new(
        reference: BodyId,
        semi_major_axis_m: f64,
        eccentricity: f64,
        plane_to_system: UnitRotation,
        period_seconds: f64,
        mean_anomaly_at_epoch_radians: f64,
        epoch: SimulationInstant,
    ) -> Result<Self, CelestialMotionError> {
        if !semi_major_axis_m.is_finite()
            || semi_major_axis_m <= 0.0
            || !eccentricity.is_finite()
            || !(0.0..1.0).contains(&eccentricity)
            || !period_seconds.is_finite()
            || period_seconds <= 0.0
            || !mean_anomaly_at_epoch_radians.is_finite()
            || mean_anomaly_at_epoch_radians.abs() > std::f64::consts::TAU
        {
            return Err(CelestialMotionError::InvalidOrbit);
        }
        Ok(Self {
            reference,
            semi_major_axis_m,
            eccentricity,
            plane_to_system,
            period_seconds,
            mean_anomaly_at_epoch_radians,
            epoch,
        })
    }
    pub fn reference(self) -> BodyId {
        self.reference
    }
    pub fn semi_major_axis_m(self) -> f64 {
        self.semi_major_axis_m
    }
    pub fn eccentricity(self) -> f64 {
        self.eccentricity
    }
    pub fn plane_to_system(self) -> UnitRotation {
        self.plane_to_system
    }
    pub fn period_seconds(self) -> f64 {
        self.period_seconds
    }
    pub fn mean_anomaly_at_epoch_radians(self) -> f64 {
        self.mean_anomaly_at_epoch_radians
    }
    pub fn epoch(self) -> SimulationInstant {
        self.epoch
    }
}

/// Independent rotation about a body-local axis; signed rate permits retrograde spin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxialSpin {
    orientation_at_epoch: UnitRotation,
    axis_in_body: Direction3,
    rate_radians_per_second: f64,
    epoch: SimulationInstant,
}
impl AxialSpin {
    pub fn new(
        orientation_at_epoch: UnitRotation,
        axis_in_body: Direction3,
        rate_radians_per_second: f64,
        epoch: SimulationInstant,
    ) -> Result<Self, CelestialMotionError> {
        if !rate_radians_per_second.is_finite() {
            return Err(CelestialMotionError::InvalidSpin);
        }
        Ok(Self {
            orientation_at_epoch,
            axis_in_body,
            rate_radians_per_second,
            epoch,
        })
    }
    pub fn orientation_at_epoch(self) -> UnitRotation {
        self.orientation_at_epoch
    }
    pub fn axis_in_body(self) -> Direction3 {
        self.axis_in_body
    }
    pub fn rate_radians_per_second(self) -> f64 {
        self.rate_radians_per_second
    }
    pub fn epoch(self) -> SimulationInstant {
        self.epoch
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CelestialTranslation {
    Stationary(LocalPosition),
    Elliptic(EllipticOrbit),
}

/// Complete authored translation and spin for one namespaced body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyMotion {
    pub body: BodyId,
    pub translation: CelestialTranslation,
    pub spin: AxialSpin,
}

/// Immutable world-domain definition. Setup validates identities, completeness and
/// topology; evaluation order is independent of body/definition insertion order.
pub struct CelestialMotionDefinition {
    namespace: std::num::NonZeroU64,
    definitions: Vec<BodyMotion>,
    references: Vec<Option<usize>>,
    evaluation_order: Vec<usize>,
}
impl CelestialMotionDefinition {
    pub fn new(
        system: &CelestialSystem,
        definitions: &[BodyMotion],
    ) -> Result<Self, CelestialMotionError> {
        let count = system.body_count();
        let mut dense = vec![None; count];
        for definition in definitions {
            system.body(definition.body)?;
            let slot = &mut dense[definition.body.index as usize];
            if slot.replace(*definition).is_some() {
                return Err(CelestialMotionError::DuplicateBody(definition.body));
            }
        }
        let definitions = dense
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(CelestialMotionError::IncompleteDefinitions)?;
        let mut references = Vec::with_capacity(count);
        for definition in &definitions {
            references.push(match definition.translation {
                CelestialTranslation::Stationary(_) => None,
                CelestialTranslation::Elliptic(orbit) => {
                    system.body(orbit.reference())?;
                    Some(orbit.reference().index as usize)
                }
            });
        }
        // Iterative single-parent DFS: O(N), no recursion even for deep hierarchies.
        let mut marks = vec![0u8; count];
        let mut path = Vec::with_capacity(count);
        let mut evaluation_order = Vec::with_capacity(count);
        for start in 0..count {
            let mut current = Some(start);
            while let Some(index) = current {
                if marks[index] == 2 {
                    break;
                }
                if marks[index] == 1 {
                    return Err(CelestialMotionError::ReferenceCycle);
                }
                marks[index] = 1;
                path.push(index);
                current = references[index];
            }
            while let Some(index) = path.pop() {
                marks[index] = 2;
                evaluation_order.push(index);
            }
        }
        Ok(Self {
            namespace: system.namespace,
            definitions,
            references,
            evaluation_order,
        })
    }
    pub fn definitions(&self) -> &[BodyMotion] {
        &self.definitions
    }
    /// Checks namespace and append-only topology, including empty systems.
    pub fn is_compatible_with(&self, system: &CelestialSystem) -> bool {
        self.namespace == system.namespace && self.definitions.len() == system.body_count()
    }
    pub fn reference_indices(&self) -> &[Option<usize>] {
        &self.references
    }
    pub fn evaluation_order(&self) -> &[usize] {
        &self.evaluation_order
    }
}
