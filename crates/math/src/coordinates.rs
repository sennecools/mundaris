//! Finite dimensional values; a containing frame or transform supplies the basis.

use crate::{FrameError, FrameId, UnitRotation};
use glam::DVec3;

/// Invalid numeric input or non-finite arithmetic output.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum MathError {
    #[error("non-finite numeric input")]
    NonFinite,
    #[error("finite arithmetic overflowed")]
    ArithmeticOverflow,
    #[error("a direction cannot be zero")]
    ZeroDirection,
    #[error("quaternion squared norm {0} differs from unity by more than 1e-10")]
    InvalidQuaternionNorm(f64),
}

pub(crate) fn finite(value: DVec3) -> Result<DVec3, MathError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(MathError::NonFinite)
    }
}

pub(crate) fn arithmetic(value: DVec3) -> Result<DVec3, MathError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(MathError::ArithmeticOverflow)
    }
}

/// A point in metres, with origin supplied by the containing frame/operation.
///
/// A velocity cannot be used as a displacement.
/// ```compile_fail
/// use mundaris_math::{LocalPosition, LinearVelocity3};
/// LocalPosition::origin().displaced(LinearVelocity3::zero());
/// ```
/// Points cannot be added to points.
/// ```compile_fail
/// use mundaris_math::LocalPosition;
/// let invalid = LocalPosition::origin() + LocalPosition::origin();
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocalPosition(DVec3);

impl LocalPosition {
    pub fn try_metres(value: DVec3) -> Result<Self, MathError> {
        Ok(Self(finite(value)?))
    }
    pub fn origin() -> Self {
        Self(DVec3::ZERO)
    }
    pub fn metres(self) -> DVec3 {
        self.0
    }
    pub fn displaced(self, offset: Displacement3) -> Result<Self, MathError> {
        Ok(Self(arithmetic(self.0 + offset.0)?))
    }
    pub fn displacement_from(self, origin: Self) -> Result<Displacement3, MathError> {
        Ok(Displacement3(arithmetic(self.0 - origin.0)?))
    }
}

/// An offset in metres, with no origin; zero is valid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Displacement3(DVec3);

impl Displacement3 {
    pub fn try_metres(value: DVec3) -> Result<Self, MathError> {
        Ok(Self(finite(value)?))
    }
    pub fn zero() -> Self {
        Self(DVec3::ZERO)
    }
    pub fn metres(self) -> DVec3 {
        self.0
    }
    /// Scaled normalization handles both subnormal and very large finite values.
    /// `None` means exactly zero, not a small-length epsilon classification.
    pub fn direction(self) -> Option<Direction3> {
        normalize(self.0).map(Direction3)
    }
}

fn normalize(value: DVec3) -> Option<DVec3> {
    let scale = value.abs().max_element();
    if scale == 0.0 {
        return None;
    }
    let scaled = value / scale;
    Some(scaled / scaled.length())
}

/// A dimensionless unit direction, never zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Direction3(DVec3);

impl Direction3 {
    pub fn try_new(value: DVec3) -> Result<Self, MathError> {
        Ok(Self(
            normalize(finite(value)?).ok_or(MathError::ZeroDirection)?,
        ))
    }
    pub fn unit(self) -> DVec3 {
        self.0
    }
}

/// Derivative of point components relative to their frame, in metres/second.
/// ```compile_fail
/// use mundaris_math::{Displacement3, LinearVelocity3};
/// let velocity: LinearVelocity3 = Displacement3::zero();
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearVelocity3(DVec3);

impl LinearVelocity3 {
    pub fn try_metres_per_second(value: DVec3) -> Result<Self, MathError> {
        Ok(Self(finite(value)?))
    }
    pub fn zero() -> Self {
        Self(DVec3::ZERO)
    }
    pub fn metres_per_second(self) -> DVec3 {
        self.0
    }
}

/// Axial angular velocity in radians/second, not Euler angle rates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngularVelocity3(DVec3);

impl AngularVelocity3 {
    pub fn try_radians_per_second(value: DVec3) -> Result<Self, MathError> {
        Ok(Self(finite(value)?))
    }
    pub fn zero() -> Self {
        Self(DVec3::ZERO)
    }
    pub fn radians_per_second(self) -> DVec3 {
        self.0
    }
}

/// A point labelled with an opaque runtime frame handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePosition {
    frame: FrameId,
    local: LocalPosition,
}
impl FramePosition {
    pub fn new(frame: FrameId, local: LocalPosition) -> Self {
        Self { frame, local }
    }
    pub fn frame(self) -> FrameId {
        self.frame
    }
    pub fn local(self) -> LocalPosition {
        self.local
    }
    pub fn displacement_from(self, origin: Self) -> Result<FrameDisplacement, FrameError> {
        crate::frames::agree(self.frame, origin.frame)?;
        Ok(FrameDisplacement::new(
            self.frame,
            self.local.displacement_from(origin.local)?,
        ))
    }
    pub fn displaced(self, offset: FrameDisplacement) -> Result<Self, FrameError> {
        crate::frames::agree(self.frame, offset.frame)?;
        Ok(Self::new(self.frame, self.local.displaced(offset.local)?))
    }
}

/// A metre offset labelled with its basis, not an origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDisplacement {
    frame: FrameId,
    local: Displacement3,
}
impl FrameDisplacement {
    pub fn new(frame: FrameId, local: Displacement3) -> Self {
        Self { frame, local }
    }
    pub fn frame(self) -> FrameId {
        self.frame
    }
    pub fn local(self) -> Displacement3 {
        self.local
    }
}

/// A dimensionless direction labelled with its basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameDirection {
    frame: FrameId,
    local: Direction3,
}
impl FrameDirection {
    pub fn new(frame: FrameId, local: Direction3) -> Self {
        Self { frame, local }
    }
    pub fn frame(self) -> FrameId {
        self.frame
    }
    pub fn local(self) -> Direction3 {
        self.local
    }
}

/// Derivative relative to a frame, in metres/second; requires a point to convert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameVelocity {
    frame: FrameId,
    relative: LinearVelocity3,
}
impl FrameVelocity {
    pub fn new(frame: FrameId, relative: LinearVelocity3) -> Self {
        Self { frame, relative }
    }
    pub fn frame(self) -> FrameId {
        self.frame
    }
    pub fn relative(self) -> LinearVelocity3 {
        self.relative
    }
}

/// A mathematical pose: position and pose-local-to-frame orientation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePose {
    position: FramePosition,
    orientation: UnitRotation,
}
impl FramePose {
    pub fn new(position: FramePosition, orientation: UnitRotation) -> Self {
        Self {
            position,
            orientation,
        }
    }
    pub fn position(self) -> FramePosition {
        self.position
    }
    pub fn orientation(self) -> UnitRotation {
        self.orientation
    }
}

/// Point and relative derivative in the same frame, at one caller-supplied instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KinematicPoint {
    position: FramePosition,
    velocity: FrameVelocity,
}
impl KinematicPoint {
    pub fn try_new(position: FramePosition, velocity: FrameVelocity) -> Result<Self, FrameError> {
        crate::frames::agree(position.frame(), velocity.frame())?;
        Ok(Self { position, velocity })
    }
    pub fn position(self) -> FramePosition {
        self.position
    }
    pub fn velocity(self) -> FrameVelocity {
        self.velocity
    }
}
