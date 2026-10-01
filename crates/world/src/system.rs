use crate::{
    BodyId, BodyProperties, BodyPropertyError, BodyState, CelestialBody, SimulationInstant,
    body::validate_name,
};
use std::num::NonZeroU64;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CelestialSystemError {
    #[error("body belongs to another system: {0:?}")]
    WrongSystem(BodyId),
    #[error("unknown body: {0:?}")]
    UnknownBody(BodyId),
    #[error("body index capacity exceeded")]
    CapacityExceeded,
    #[error("system revision overflow")]
    RevisionOverflow,
    #[error("duplicate body update: {0:?}")]
    DuplicateBodyUpdate(BodyId),
    #[error("a new instant requires a state for every body")]
    IncompleteTimeUpdate,
    #[error(transparent)]
    Property(#[from] BodyPropertyError),
}

#[derive(Debug, Clone, Copy)]
pub struct BodyStateUpdate {
    pub body: BodyId,
    pub state: BodyState,
}

/// Append-only authoritative state. Not cloneable: identity must not fork silently.
pub struct CelestialSystem {
    pub(crate) namespace: NonZeroU64,
    sample_time: SimulationInstant,
    revision: u64,
    bodies: Vec<CelestialBody>,
    duplicate_flags: Vec<bool>,
}
impl CelestialSystem {
    pub fn new(namespace: NonZeroU64, initial_time: SimulationInstant) -> Self {
        Self {
            namespace,
            sample_time: initial_time,
            revision: 0,
            bodies: Vec::new(),
            duplicate_flags: Vec::new(),
        }
    }
    fn id(&self, index: usize) -> BodyId {
        BodyId {
            namespace: self.namespace,
            index: index as u32,
        }
    }
    fn index(&self, id: BodyId) -> Result<usize, CelestialSystemError> {
        if id.namespace != self.namespace {
            return Err(CelestialSystemError::WrongSystem(id));
        }
        if id.index as usize >= self.bodies.len() {
            return Err(CelestialSystemError::UnknownBody(id));
        }
        Ok(id.index as usize)
    }
    fn next_revision(&self) -> Result<u64, CelestialSystemError> {
        self.revision
            .checked_add(1)
            .ok_or(CelestialSystemError::RevisionOverflow)
    }
    pub fn sample_time(&self) -> SimulationInstant {
        self.sample_time
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }
    /// Dense deterministic enumeration for future simulation working buffers.
    pub fn bodies(&self) -> impl ExactSizeIterator<Item = (BodyId, &CelestialBody)> + Clone {
        self.bodies
            .iter()
            .enumerate()
            .map(|(index, body)| (self.id(index), body))
    }
    pub fn body(&self, id: BodyId) -> Result<&CelestialBody, CelestialSystemError> {
        Ok(&self.bodies[self.index(id)?])
    }
    pub fn insert_body(
        &mut self,
        name: impl Into<String>,
        properties: BodyProperties,
        state: BodyState,
    ) -> Result<BodyId, CelestialSystemError> {
        let name = name.into();
        validate_name(&name)?;
        let index =
            u32::try_from(self.bodies.len()).map_err(|_| CelestialSystemError::CapacityExceeded)?;
        let revision = self.next_revision()?;
        self.bodies.push(CelestialBody {
            name,
            properties,
            state,
        });
        self.duplicate_flags.push(false);
        self.revision = revision;
        Ok(BodyId {
            namespace: self.namespace,
            index,
        })
    }
    /// Atomic editor edit; neither properties nor metadata can change kinematics.
    pub fn edit_body(
        &mut self,
        id: BodyId,
        name: impl Into<String>,
        properties: BodyProperties,
    ) -> Result<(), CelestialSystemError> {
        let index = self.index(id)?;
        let name = name.into();
        validate_name(&name)?;
        let revision = self.next_revision()?;
        self.bodies[index].name = name;
        self.bodies[index].properties = properties;
        self.revision = revision;
        Ok(())
    }
    pub fn edit_properties(
        &mut self,
        id: BodyId,
        properties: BodyProperties,
    ) -> Result<(), CelestialSystemError> {
        let index = self.index(id)?;
        let revision = self.next_revision()?;
        self.bodies[index].properties = properties;
        self.revision = revision;
        Ok(())
    }
    pub fn edit_state(&mut self, id: BodyId, state: BodyState) -> Result<(), CelestialSystemError> {
        self.update_states(self.sample_time, &[BodyStateUpdate { body: id, state }])
    }
    /// Transactional O(U), allocation-free after insertion. Caller order is arbitrary.
    /// At a different time, all bodies must be supplied. At the same time subsets
    /// are allowed. Validated `BodyState` values cannot contain invalid math.
    pub fn update_states(
        &mut self,
        sample_time: SimulationInstant,
        updates: &[BodyStateUpdate],
    ) -> Result<(), CelestialSystemError> {
        let revision = self.next_revision()?;
        for update in updates {
            self.index(update.body)?;
        }
        let mut duplicate = None;
        for update in updates {
            let flag = &mut self.duplicate_flags[update.body.index as usize];
            if *flag {
                duplicate = Some(update.body);
                break;
            }
            *flag = true;
        }
        for update in updates {
            self.duplicate_flags[update.body.index as usize] = false;
        }
        if let Some(id) = duplicate {
            return Err(CelestialSystemError::DuplicateBodyUpdate(id));
        }
        if sample_time != self.sample_time && updates.len() != self.bodies.len() {
            return Err(CelestialSystemError::IncompleteTimeUpdate);
        }
        for update in updates {
            self.bodies[update.body.index as usize].state = update.state;
        }
        self.sample_time = sample_time;
        self.revision = revision;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::*;
    #[test]
    fn invalid_index_and_revision_overflow_preserve_state() {
        let mut system = CelestialSystem::new(NonZeroU64::new(1).unwrap(), SimulationInstant::ZERO);
        let state = BodyState::new(
            LocalPosition::origin(),
            LinearVelocity3::zero(),
            UnitRotation::identity(),
            AngularVelocity3::zero(),
        );
        let id = system
            .insert_body("first", BodyProperties::new(1.0, 1.0).unwrap(), state)
            .unwrap();
        let unknown = BodyId {
            namespace: system.namespace,
            index: 99,
        };
        assert!(matches!(
            system.edit_state(unknown, state),
            Err(CelestialSystemError::UnknownBody(_))
        ));
        system.revision = u64::MAX;
        assert_eq!(
            system.edit_state(id, state),
            Err(CelestialSystemError::RevisionOverflow)
        );
        assert_eq!(
            system.edit_body(id, "new", BodyProperties::new(2.0, 2.0).unwrap()),
            Err(CelestialSystemError::RevisionOverflow)
        );
        assert!(
            system
                .insert_body("next", BodyProperties::new(1.0, 1.0).unwrap(), state)
                .is_err()
        );
        assert_eq!(system.body(id).unwrap().name(), "first");
        assert_eq!(system.body_count(), 1);
        assert_eq!(system.sample_time(), SimulationInstant::ZERO);
    }
}
