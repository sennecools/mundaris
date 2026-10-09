//! Append-only frame topology and immutable, single-instant relative evaluation.

use crate::{AngularVelocity3, FramePose, FrameVelocity, KinematicPoint, LinearVelocity3};
use crate::{
    FrameDirection, FrameDisplacement, FrameMotion, FramePosition, LocalPosition, MathError,
    RigidTransform, UnitRotation, coordinates::arithmetic,
};
use std::num::NonZeroU64;

/// Opaque session handle; not persistent identity. Namespace reuse is caller error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId {
    namespace: NonZeroU64,
    index: u32,
}
impl FrameId {
    /// Diagnostic only; useful for ordered update publication, not reconstruction.
    pub fn node_index(self) -> u32 {
        self.index
    }
}

/// Checked local pose plus optional instantaneous derivatives. `None` is unknown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameState {
    parent_from_local: RigidTransform,
    motion: Option<FrameMotion>,
}
impl FrameState {
    pub fn new(parent_from_local: RigidTransform, motion: Option<FrameMotion>) -> Self {
        Self {
            parent_from_local,
            motion,
        }
    }
    pub fn stationary(parent_from_local: RigidTransform) -> Self {
        Self::new(parent_from_local, Some(FrameMotion::stationary()))
    }
    pub fn parent_from_local(self) -> RigidTransform {
        self.parent_from_local
    }
    pub fn motion(self) -> Option<FrameMotion> {
        self.motion
    }
}

/// Invalid identity, edit, numeric output, or derivative query.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FrameError {
    #[error(transparent)]
    Math(#[from] MathError),
    #[error("frame {0:?} belongs to another tree")]
    WrongTree(FrameId),
    #[error("unknown frame {0:?}")]
    UnknownFrame(FrameId),
    #[error("expected frame {expected:?}, got {actual:?}")]
    FrameMismatch { expected: FrameId, actual: FrameId },
    #[error("the identity root cannot be edited")]
    RootMutation,
    #[error("reparenting {frame:?} below {parent:?} would create a cycle")]
    Cycle { frame: FrameId, parent: FrameId },
    #[error("updates must have strictly increasing node indices")]
    UnorderedUpdates,
    #[error("frame index/depth capacity exceeded")]
    CapacityExceeded,
    #[error("frame revision overflow")]
    RevisionOverflow,
    #[error("motion is unknown on edge {0:?}")]
    MissingMotion(FrameId),
}

pub(crate) fn agree(expected: FrameId, actual: FrameId) -> Result<(), FrameError> {
    if expected == actual {
        Ok(())
    } else {
        Err(FrameError::FrameMismatch { expected, actual })
    }
}

#[derive(Debug)]
struct Node {
    parent: Option<usize>,
    depth: u32,
    state: FrameState,
}

/// One connected, identity-root tree. Nodes are contiguous and never removed/reused.
///
/// The caller must assign a fresh nonzero namespace to every independent tree.
/// Unlisted states in a publication must remain valid at the new sample instant.
/// Prepared conversions borrow the tree, so authoritative edits cannot stale them.
/// ```compile_fail
/// use std::num::NonZeroU64;
/// use astrum_math::*;
/// let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
/// let prepared = tree.evaluate().prepare_conversion(tree.root(), tree.root()).unwrap();
/// tree.update_states(1.0, &[]).unwrap();
/// let _ = prepared.rotation();
/// ```
pub struct FrameTree {
    namespace: NonZeroU64,
    nodes: Vec<Node>,
    sample_time_s: f64,
    revision: u64,
}

impl FrameTree {
    pub fn new(namespace: NonZeroU64) -> Self {
        Self {
            namespace,
            nodes: vec![Node {
                parent: None,
                depth: 0,
                state: FrameState::stationary(RigidTransform::identity()),
            }],
            sample_time_s: 0.0,
            revision: 0,
        }
    }
    pub fn root(&self) -> FrameId {
        self.id(0)
    }
    fn id(&self, index: usize) -> FrameId {
        FrameId {
            namespace: self.namespace,
            index: index as u32,
        }
    }
    fn index(&self, frame: FrameId) -> Result<usize, FrameError> {
        if frame.namespace != self.namespace {
            return Err(FrameError::WrongTree(frame));
        }
        if frame.index as usize >= self.nodes.len() {
            return Err(FrameError::UnknownFrame(frame));
        }
        Ok(frame.index as usize)
    }
    fn next_revision(&self) -> Result<u64, FrameError> {
        self.revision
            .checked_add(1)
            .ok_or(FrameError::RevisionOverflow)
    }
    pub fn insert(&mut self, parent: FrameId, state: FrameState) -> Result<FrameId, FrameError> {
        let parent = self.index(parent)?;
        let index = u32::try_from(self.nodes.len()).map_err(|_| FrameError::CapacityExceeded)?;
        let depth = self.nodes[parent]
            .depth
            .checked_add(1)
            .ok_or(FrameError::CapacityExceeded)?;
        let revision = self.next_revision()?;
        self.nodes.push(Node {
            parent: Some(parent),
            depth,
            state,
        });
        self.revision = revision;
        Ok(FrameId {
            namespace: self.namespace,
            index,
        })
    }
    /// Transactional O(U), allocation-free publication; inputs must be index ordered.
    /// Finite times may be negative or move backward. No state is extrapolated.
    pub fn update_states(
        &mut self,
        sample_time_s: f64,
        updates: &[(FrameId, FrameState)],
    ) -> Result<(), FrameError> {
        if !sample_time_s.is_finite() {
            return Err(MathError::NonFinite.into());
        }
        let revision = self.next_revision()?;
        let mut previous = None;
        for &(frame, _) in updates {
            let index = self.index(frame)?;
            if index == 0 {
                return Err(FrameError::RootMutation);
            }
            if previous.is_some_and(|previous| index <= previous) {
                return Err(FrameError::UnorderedUpdates);
            }
            previous = Some(index);
        }
        // State values are valid by construction and cannot be mutated internally.
        for &(frame, state) in updates {
            self.nodes[frame.index as usize].state = state;
        }
        self.sample_time_s = sample_time_s;
        self.revision = revision;
        Ok(())
    }
    /// Explicit new local state may change physical pose. ID and descendant state survive.
    /// Topology work is O(F), outside publication/conversion hot paths.
    pub fn reparent_with_local_state(
        &mut self,
        frame: FrameId,
        parent: FrameId,
        state: FrameState,
    ) -> Result<(), FrameError> {
        let index = self.index(frame)?;
        let parent_index = self.index(parent)?;
        if index == 0 {
            return Err(FrameError::RootMutation);
        }
        let revision = self.next_revision()?;
        let mut ancestor = Some(parent_index);
        while let Some(current) = ancestor {
            if current == index {
                return Err(FrameError::Cycle { frame, parent });
            }
            ancestor = self.nodes[current].parent;
        }
        // Build proposed depths before modifying authoritative topology.
        let mut children = vec![Vec::new(); self.nodes.len()];
        for (child, node) in self.nodes.iter().enumerate().skip(1) {
            let proposed = if child == index {
                parent_index
            } else {
                node.parent.expect("non-root node must have a parent")
            };
            children[proposed].push(child);
        }
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut depths = vec![0u32; self.nodes.len()];
        order.push(0);
        let mut cursor = 0;
        while cursor < order.len() {
            let current = order[cursor];
            for &child in &children[current] {
                depths[child] = depths[current]
                    .checked_add(1)
                    .ok_or(FrameError::CapacityExceeded)?;
                order.push(child);
            }
            cursor += 1;
        }
        assert_eq!(
            order.len(),
            self.nodes.len(),
            "validated topology must reach every node"
        );
        self.nodes[index].parent = Some(parent_index);
        self.nodes[index].state = state;
        for (node, depth) in self.nodes.iter_mut().zip(depths) {
            node.depth = depth;
        }
        self.revision = revision;
        Ok(())
    }
    pub fn evaluate(&self) -> FrameEvaluation<'_> {
        FrameEvaluation { tree: self }
    }
}

/// Immutable borrow of exactly one published instant. Copying the view copies a borrow.
#[derive(Clone, Copy)]
pub struct FrameEvaluation<'a> {
    tree: &'a FrameTree,
}
impl<'a> FrameEvaluation<'a> {
    pub fn sample_time_s(self) -> f64 {
        self.tree.sample_time_s
    }
    pub fn revision(self) -> u64 {
        self.tree.revision
    }
    pub fn root(self) -> FrameId {
        self.tree.root()
    }
    pub fn parent(self, frame: FrameId) -> Result<Option<FrameId>, FrameError> {
        Ok(self.tree.nodes[self.tree.index(frame)?]
            .parent
            .map(|index| self.tree.id(index)))
    }
    pub fn state(self, frame: FrameId) -> Result<&'a FrameState, FrameError> {
        Ok(&self.tree.nodes[self.tree.index(frame)?].state)
    }
    pub fn depth(self, frame: FrameId) -> Result<u32, FrameError> {
        Ok(self.tree.nodes[self.tree.index(frame)?].depth)
    }
    fn lca(self, from: FrameId, to: FrameId) -> Result<usize, FrameError> {
        let mut a = self.tree.index(from)?;
        let mut b = self.tree.index(to)?;
        while self.tree.nodes[a].depth > self.tree.nodes[b].depth {
            a = self.parent_index(a);
        }
        while self.tree.nodes[b].depth > self.tree.nodes[a].depth {
            b = self.parent_index(b);
        }
        while a != b {
            a = self.parent_index(a);
            b = self.parent_index(b);
        }
        Ok(a)
    }
    fn parent_index(self, index: usize) -> usize {
        self.tree.nodes[index]
            .parent
            .expect("LCA traversal cannot walk above root")
    }
    fn branch(self, mut index: usize, ancestor: usize) -> Result<RigidTransform, FrameError> {
        let mut result = RigidTransform::identity();
        while index != ancestor {
            result = self.tree.nodes[index]
                .state
                .parent_from_local
                .compose(result)?;
            index = self.parent_index(index);
        }
        Ok(result)
    }
    /// Diagnostic/coarse query, O(depth). Never used to center nearby render content.
    pub fn transform_to_root(self, frame: FrameId) -> Result<RigidTransform, FrameError> {
        self.branch(self.tree.index(frame)?, 0)
    }

    fn moving_branch(
        self,
        mut index: usize,
        ancestor: usize,
    ) -> Result<(RigidTransform, FrameMotion), FrameError> {
        let mut pose = RigidTransform::identity();
        let mut motion = FrameMotion::stationary();
        while index != ancestor {
            let state = self.tree.nodes[index].state;
            let outer_motion = state
                .motion
                .ok_or(FrameError::MissingMotion(self.tree.id(index)))?;
            let outer = state.parent_from_local;
            let rotated_origin = outer.rotation().rotate_raw(pose.translation().metres())?;
            let omega = outer_motion
                .angular_velocity_in_parent()
                .radians_per_second();
            let velocity = arithmetic(
                arithmetic(
                    outer_motion.origin_velocity_in_parent().metres_per_second()
                        + arithmetic(omega.cross(rotated_origin))?,
                )? + outer
                    .rotation()
                    .rotate_raw(motion.origin_velocity_in_parent().metres_per_second())?,
            )?;
            let angular = arithmetic(
                omega
                    + outer
                        .rotation()
                        .rotate_raw(motion.angular_velocity_in_parent().radians_per_second())?,
            )?;
            motion = FrameMotion::new(
                LinearVelocity3::try_metres_per_second(velocity)?,
                AngularVelocity3::try_radians_per_second(angular)?,
            );
            pose = outer.compose(pose)?;
            index = self.parent_index(index);
        }
        Ok((pose, motion))
    }

    /// Derivative preparation visits only edges below the LCA. Unknown is not zero.
    pub fn prepare_kinematic_conversion(
        self,
        from: FrameId,
        to: FrameId,
    ) -> Result<PreparedKinematicConversion<'a>, FrameError> {
        let ancestor = self.lca(from, to)?;
        let (source, source_motion) = self.moving_branch(from.index as usize, ancestor)?;
        let (target, target_motion) = self.moving_branch(to.index as usize, ancestor)?;
        let rotation = target.rotation().inverse().compose(source.rotation());
        Ok(PreparedKinematicConversion {
            pose: PreparedFrameConversion {
                evaluation: self,
                from,
                to,
                source,
                target,
                rotation,
            },
            source_motion,
            target_motion,
        })
    }

    pub fn convert_kinematic_point(
        self,
        point: KinematicPoint,
        target: FrameId,
    ) -> Result<KinematicPoint, FrameError> {
        self.prepare_kinematic_conversion(point.position().frame(), target)?
            .convert_point(point)
    }

    /// Coordinate re-expression, not physical attachment or a control-mode change.
    pub fn reexpress_pose(self, pose: FramePose, target: FrameId) -> Result<FramePose, FrameError> {
        let prepared = self.prepare_conversion(pose.position().frame(), target)?;
        if pose.position().frame() == target {
            return Ok(pose);
        }
        Ok(FramePose::new(
            prepared.convert_position(pose.position())?,
            prepared.rotation().compose(pose.orientation()),
        ))
    }
    /// Walk only branches below the LCA, retaining their separate origins.
    pub fn prepare_conversion(
        self,
        from: FrameId,
        to: FrameId,
    ) -> Result<PreparedFrameConversion<'a>, FrameError> {
        let ancestor = self.lca(from, to)?;
        let source = self.branch(from.index as usize, ancestor)?;
        let target = self.branch(to.index as usize, ancestor)?;
        Ok(PreparedFrameConversion {
            evaluation: self,
            from,
            to,
            source,
            target,
            rotation: target.rotation().inverse().compose(source.rotation()),
        })
    }
    pub fn convert_position(
        self,
        point: FramePosition,
        target: FrameId,
    ) -> Result<FramePosition, FrameError> {
        self.prepare_conversion(point.frame(), target)?
            .convert_position(point)
    }
    pub fn convert_displacement(
        self,
        value: FrameDisplacement,
        target: FrameId,
    ) -> Result<FrameDisplacement, FrameError> {
        self.prepare_conversion(value.frame(), target)?
            .convert_displacement(value)
    }
    pub fn convert_direction(
        self,
        value: FrameDirection,
        target: FrameId,
    ) -> Result<FrameDirection, FrameError> {
        self.prepare_conversion(value.frame(), target)?
            .convert_direction(value)
    }
}

/// Reusable O(1), allocation-free conversion tied to an immutable tree instant.
pub struct PreparedFrameConversion<'a> {
    evaluation: FrameEvaluation<'a>,
    from: FrameId,
    to: FrameId,
    source: RigidTransform,
    target: RigidTransform,
    rotation: UnitRotation,
}

/// Instantaneous point-and-derivative conversion retaining the same tree borrow.
/// No position-free velocity transform is provided: rotating frames need a lever arm.
pub struct PreparedKinematicConversion<'a> {
    pose: PreparedFrameConversion<'a>,
    source_motion: FrameMotion,
    target_motion: FrameMotion,
}
impl PreparedKinematicConversion<'_> {
    pub fn convert_point(&self, value: KinematicPoint) -> Result<KinematicPoint, FrameError> {
        agree(self.pose.from, value.position().frame())?;
        if self.pose.from == self.pose.to {
            return Ok(value);
        }
        let source_point = self
            .pose
            .source
            .rotation()
            .rotate_raw(value.position().local().metres())?;
        let lever = arithmetic(
            arithmetic(
                self.pose.source.translation().metres() - self.pose.target.translation().metres(),
            )? + source_point,
        )?;
        let source_omega = self
            .source_motion
            .angular_velocity_in_parent()
            .radians_per_second();
        let target_omega = self
            .target_motion
            .angular_velocity_in_parent()
            .radians_per_second();
        let in_lca = arithmetic(
            arithmetic(
                self.source_motion
                    .origin_velocity_in_parent()
                    .metres_per_second()
                    + arithmetic(source_omega.cross(source_point))?,
            )? + self
                .pose
                .source
                .rotation()
                .rotate_raw(value.velocity().relative().metres_per_second())?,
        )?;
        let relative = arithmetic(
            arithmetic(
                in_lca
                    - self
                        .target_motion
                        .origin_velocity_in_parent()
                        .metres_per_second(),
            )? - arithmetic(target_omega.cross(lever))?,
        )?;
        let velocity = self.pose.target.rotation().inverse().rotate_raw(relative)?;
        KinematicPoint::try_new(
            self.pose.convert_position(value.position())?,
            FrameVelocity::new(
                self.pose.to,
                LinearVelocity3::try_metres_per_second(velocity)?,
            ),
        )
    }
}
impl PreparedFrameConversion<'_> {
    pub fn rotation(&self) -> UnitRotation {
        self.rotation
    }
    pub fn sample_time_s(&self) -> f64 {
        self.evaluation.sample_time_s()
    }
    pub fn convert_position(&self, point: FramePosition) -> Result<FramePosition, FrameError> {
        agree(self.from, point.frame())?;
        if self.from == self.to {
            return Ok(point);
        }
        let lever = arithmetic(
            arithmetic(self.source.translation().metres() - self.target.translation().metres())?
                + self.source.rotation().rotate_raw(point.local().metres())?,
        )?;
        Ok(FramePosition::new(
            self.to,
            LocalPosition::try_metres(self.target.rotation().inverse().rotate_raw(lever)?)?,
        ))
    }
    pub fn convert_displacement(
        &self,
        value: FrameDisplacement,
    ) -> Result<FrameDisplacement, FrameError> {
        agree(self.from, value.frame())?;
        if self.from == self.to {
            return Ok(value);
        }
        Ok(FrameDisplacement::new(
            self.to,
            self.rotation.rotate_displacement(value.local())?,
        ))
    }
    pub fn convert_direction(&self, value: FrameDirection) -> Result<FrameDirection, FrameError> {
        agree(self.from, value.frame())?;
        if self.from == self.to {
            return Ok(value);
        }
        Ok(FrameDirection::new(
            self.to,
            self.rotation.rotate_direction(value.local())?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_handles_and_revision_overflow_are_transactional() {
        let mut tree = FrameTree::new(NonZeroU64::new(1).unwrap());
        let missing = FrameId {
            namespace: tree.namespace,
            index: 99,
        };
        assert!(matches!(
            tree.evaluate().prepare_conversion(missing, missing),
            Err(FrameError::UnknownFrame(_))
        ));
        assert!(matches!(
            tree.insert(missing, FrameState::stationary(RigidTransform::identity())),
            Err(FrameError::UnknownFrame(_))
        ));
        tree.revision = u64::MAX;
        assert_eq!(
            tree.update_states(1.0, &[]),
            Err(FrameError::RevisionOverflow)
        );
        assert_eq!(tree.sample_time_s, 0.0);
        assert_eq!(tree.nodes.len(), 1);
    }
}
