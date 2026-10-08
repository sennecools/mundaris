//! Body-relative surface inspection anchor used by camera navigation.
use anyhow::Result;
use glam::DVec3;
use mundaris_math::{surface::*, *};
use mundaris_world::BodyId;

/// Explicit f64 regional pose relative to fixed body axes. Never a tree node or patch ID.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceInspectionAnchor {
    pub body: BodyId,
    pub location: SurfaceLocation,
    pub body_from_regional: RigidTransform,
    pub observer_in_regional: LocalPosition,
    pub tangent: SurfaceTangentBasis,
}
impl SurfaceInspectionAnchor {
    pub fn new(
        body: BodyId,
        p: DVec3,
        radius: f64,
        previous: Option<SurfaceTangentBasis>,
    ) -> Result<Self> {
        let up = Direction3::try_new(p)?;
        let location = SurfaceLocation::new(up);
        let tangent = previous.map_or_else(|| SurfaceTangentBasis::new(up), |t| t.transported(up));
        let basis =
            glam::DMat3::from_cols(tangent.east().unit(), up.unit(), -tangent.north().unit());
        let rotation = UnitRotation::try_from_quaternion(glam::DQuat::from_mat3(&basis))?;
        let body_from_regional =
            RigidTransform::new(Displacement3::try_metres(up.unit() * radius)?, rotation);
        let observer_in_regional = body_from_regional
            .inverse()?
            .transform_position(LocalPosition::try_metres(p)?)?;
        Ok(Self {
            body,
            location,
            body_from_regional,
            observer_in_regional,
            tangent,
        })
    }
    pub fn position(self) -> Result<LocalPosition> {
        Ok(self
            .body_from_regional
            .transform_position(self.observer_in_regional)?)
    }
}
