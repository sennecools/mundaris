use crate::{BodyId, BodyState, CelestialSystem};
use mundaris_math::*;
use std::num::NonZeroU64;

/// Runtime debugging/attachment handles, never domain or persistent identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyFrames {
    pub translating: FrameId,
    pub body_fixed: FrameId,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FrameProjectionError {
    #[error("projection belongs to another system or incompatible topology")]
    FrameProjectionMismatch,
    #[error("body belongs to another system: {0:?}")]
    WrongSystem(BodyId),
    #[error("body has not been projected: {0:?}")]
    UnknownBody(BodyId),
    #[error(transparent)]
    FramePublicationFailed(#[from] FrameError),
    #[error(transparent)]
    Math(#[from] MathError),
}

/// Disposable derived tree. No arbitrary mutable tree access is exposed.
pub struct CelestialFrameProjection {
    tree: FrameTree,
    namespace: NonZeroU64,
    body_frames: Vec<BodyFrames>,
    updates: Vec<(FrameId, FrameState)>,
    represented_revision: u64,
}
fn states(state: BodyState) -> Result<[FrameState; 2], MathError> {
    Ok([
        FrameState::new(
            RigidTransform::new(
                Displacement3::try_metres(state.center_in_system().metres())?,
                UnitRotation::identity(),
            ),
            Some(FrameMotion::new(
                state.center_velocity_in_system(),
                AngularVelocity3::zero(),
            )),
        ),
        FrameState::new(
            RigidTransform::new(Displacement3::zero(), state.body_to_system()),
            Some(FrameMotion::new(
                LinearVelocity3::zero(),
                state.angular_velocity_in_system(),
            )),
        ),
    ])
}
impl CelestialFrameProjection {
    /// Supply a fresh tree namespace on every independent build, including rebuilds.
    pub fn build(
        system: &CelestialSystem,
        tree_namespace: NonZeroU64,
    ) -> Result<Self, FrameProjectionError> {
        let mut projection = Self {
            tree: FrameTree::new(tree_namespace),
            namespace: system.namespace,
            body_frames: Vec::new(),
            updates: Vec::new(),
            represented_revision: 0,
        };
        projection.publish(system)?;
        Ok(projection)
    }
    /// Full O(B) coherent publication, with reusable staging and stable existing IDs.
    /// All fallible checks precede appends/updates. The private topology then guarantees
    /// inserts cannot fail: depth is two, capacities/revisions are preflighted.
    pub fn publish(&mut self, system: &CelestialSystem) -> Result<(), FrameProjectionError> {
        if self.namespace != system.namespace || self.body_frames.len() > system.body_count() {
            return Err(FrameProjectionError::FrameProjectionMismatch);
        }
        let total_frames = system
            .body_count()
            .checked_mul(2)
            .and_then(|count| count.checked_add(1))
            .ok_or(FrameError::CapacityExceeded)?;
        if total_frames as u128 > u32::MAX as u128 + 1 {
            return Err(FrameError::CapacityExceeded.into());
        }
        let append_count = (system.body_count() - self.body_frames.len()) * 2;
        self.tree
            .evaluate()
            .revision()
            .checked_add(append_count as u64)
            .and_then(|r| r.checked_add(1))
            .ok_or(FrameError::RevisionOverflow)?;
        // Numeric conversion only copies checked f64 vectors; validate before any append.
        for (_, body) in system.bodies() {
            states(*body.state())?;
        }
        self.updates.clear();
        self.updates.reserve(system.body_count() * 2);
        self.body_frames
            .reserve(system.body_count() - self.body_frames.len());
        for (index, (_, body)) in system.bodies().enumerate() {
            let [anchor, fixed] = states(*body.state()).expect("prevalidated checked state");
            let frames = if index < self.body_frames.len() {
                self.body_frames[index]
            } else {
                let translating = self
                    .tree
                    .insert(self.tree.root(), anchor)
                    .expect("preflighted anchor insertion");
                let body_fixed = self
                    .tree
                    .insert(translating, fixed)
                    .expect("preflighted fixed insertion");
                let frames = BodyFrames {
                    translating,
                    body_fixed,
                };
                self.body_frames.push(frames);
                frames
            };
            self.updates.push((frames.translating, anchor));
            self.updates.push((frames.body_fixed, fixed));
        }
        self.tree
            .update_states(system.sample_time().seconds_since_epoch(), &self.updates)?;
        self.represented_revision = system.revision();
        Ok(())
    }
    pub fn frames_for(&self, id: BodyId) -> Result<BodyFrames, FrameProjectionError> {
        if id.namespace != self.namespace {
            return Err(FrameProjectionError::WrongSystem(id));
        }
        self.body_frames
            .get(id.index as usize)
            .copied()
            .ok_or(FrameProjectionError::UnknownBody(id))
    }
    pub fn tree(&self) -> &FrameTree {
        &self.tree
    }
    pub fn represented_revision(&self) -> u64 {
        self.represented_revision
    }
}
