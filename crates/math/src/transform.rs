//! Rigid column-vector transforms, never scale, shear or reflection.

use crate::{Direction3, Displacement3, LocalPosition, MathError, coordinates::arithmetic};
use glam::{DQuat, DVec3};

/// Checked double-precision unit quaternion; `q` and `-q` denote the same rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UnitRotation(DQuat);

impl UnitRotation {
    pub fn identity() -> Self {
        Self(DQuat::IDENTITY)
    }
    /// Accepts squared-norm drift at most `1e-10`, then corrects roundoff once.
    pub fn try_from_quaternion(value: DQuat) -> Result<Self, MathError> {
        if !value.is_finite() {
            return Err(MathError::NonFinite);
        }
        let norm = value.length_squared();
        if !norm.is_finite() || (norm - 1.0).abs() > 1e-10 {
            return Err(MathError::InvalidQuaternionNorm(norm));
        }
        Ok(Self(value.normalize()))
    }
    pub fn from_axis_angle(axis: Direction3, angle_rad: f64) -> Result<Self, MathError> {
        if !angle_rad.is_finite() {
            return Err(MathError::NonFinite);
        }
        Ok(Self(
            DQuat::from_axis_angle(axis.unit(), angle_rad).normalize(),
        ))
    }
    /// Explicit numerical escape hatch; components are `(x,y,z,w)`.
    pub fn quaternion(self) -> DQuat {
        self.0
    }
    /// Applies `inner` first, then `self`; normalizes accumulated roundoff.
    pub fn compose(self, inner: Self) -> Self {
        Self((self.0 * inner.0).normalize())
    }
    pub fn inverse(self) -> Self {
        Self(self.0.conjugate())
    }
    pub fn rotate_displacement(self, value: Displacement3) -> Result<Displacement3, MathError> {
        Displacement3::try_metres(self.rotate_raw(value.metres())?)
    }
    pub fn rotate_direction(self, value: Direction3) -> Result<Direction3, MathError> {
        Direction3::try_new(self.rotate_raw(value.unit())?)
    }
    pub(crate) fn rotate_raw(self, value: DVec3) -> Result<DVec3, MathError> {
        arithmetic(self.0 * value)
    }
}

/// `T_destination_from_source`: source origin in destination axes plus rotation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidTransform {
    translation: Displacement3,
    rotation: UnitRotation,
}

impl RigidTransform {
    pub fn new(translation: Displacement3, rotation: UnitRotation) -> Self {
        Self {
            translation,
            rotation,
        }
    }
    pub fn identity() -> Self {
        Self::new(Displacement3::zero(), UnitRotation::identity())
    }
    pub fn translation(self) -> Displacement3 {
        self.translation
    }
    pub fn rotation(self) -> UnitRotation {
        self.rotation
    }
    /// Self after inner. Finite inputs can still overflow translation arithmetic.
    pub fn compose(self, inner: Self) -> Result<Self, MathError> {
        let translation = arithmetic(
            self.translation.metres() + self.rotation.rotate_raw(inner.translation.metres())?,
        )?;
        Ok(Self::new(
            Displacement3::try_metres(translation)?,
            self.rotation.compose(inner.rotation),
        ))
    }
    pub fn inverse(self) -> Result<Self, MathError> {
        let rotation = self.rotation.inverse();
        Ok(Self::new(
            Displacement3::try_metres(rotation.rotate_raw(-self.translation.metres())?)?,
            rotation,
        ))
    }
    pub fn transform_position(self, value: LocalPosition) -> Result<LocalPosition, MathError> {
        LocalPosition::try_metres(arithmetic(
            self.translation.metres() + self.rotation.rotate_raw(value.metres())?,
        )?)
    }
}
