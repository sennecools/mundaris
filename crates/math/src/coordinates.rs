//! Finite dimensional values; a containing frame or transform supplies the basis.

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
