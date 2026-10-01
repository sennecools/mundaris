//! One observer; selection is independent of explicit focus/attachment.
use anyhow::{Result, ensure};
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_world::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAttachment {
    System,
    Translating(BodyId),
    BodyFixed(BodyId),
}
impl CameraAttachment {
    fn frame(self, projection: &CelestialFrameProjection) -> Result<FrameId> {
        Ok(match self {
            Self::System => projection.tree().root(),
            Self::Translating(id) => projection.frames_for(id)?.translating,
            Self::BodyFixed(id) => projection.frames_for(id)?.body_fixed,
        })
    }
}
pub struct CelestialCamera {
    pose: FramePose,
    velocity: FrameVelocity,
    attachment: CameraAttachment,
    anchor: LocalPosition,
    orbit_basis: UnitRotation,
    distance_m: f64,
    yaw: f64,
    pitch: f64,
    min_distance_m: f64,
}
impl CelestialCamera {
    pub fn overview(
        pair: &CoherentCelestialView<'_>,
        initial_center: DVec3,
        extent_m: f64,
    ) -> Result<Self> {
        ensure!(
            extent_m.is_finite() && extent_m > 0.0,
            "invalid overview extent"
        );
        let root = pair.evaluation().root();
        let anchor = LocalPosition::try_metres(initial_center)?;
        let mut camera = Self {
            pose: FramePose::new(FramePosition::new(root, anchor), UnitRotation::identity()),
            velocity: FrameVelocity::new(root, LinearVelocity3::zero()),
            attachment: CameraAttachment::System,
            anchor,
            orbit_basis: UnitRotation::identity(),
            distance_m: 4.0 * extent_m,
            yaw: 0.0,
            pitch: 0.0,
            min_distance_m: 0.1,
        };
        camera.update_pose(root)?;
        Ok(camera)
    }
    pub fn pose(&self) -> FramePose {
        self.pose
    }
    pub fn velocity(&self) -> FrameVelocity {
        self.velocity
    }
    pub fn attachment(&self) -> CameraAttachment {
        self.attachment
    }
    pub fn distance_m(&self) -> f64 {
        self.distance_m
    }
    fn update_pose(&mut self, frame: FrameId) -> Result<()> {
        let rotation = self.orbit_basis.compose(UnitRotation::try_from_quaternion(
            DQuat::from_rotation_y(self.yaw) * DQuat::from_rotation_x(self.pitch),
        )?);
        let offset =
            rotation.rotate_displacement(Displacement3::try_metres(DVec3::Z * self.distance_m)?)?;
        self.pose = FramePose::new(
            FramePosition::new(frame, self.anchor.displaced(offset)?),
            rotation,
        );
        self.velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
        Ok(())
    }
    pub fn focus(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        id: BodyId,
        body_fixed: bool,
        fit: bool,
    ) -> Result<()> {
        let radius = pair.system().body(id)?.properties().reference_radius_m();
        let attachment = if body_fixed {
            CameraAttachment::BodyFixed(id)
        } else {
            CameraAttachment::Translating(id)
        };
        let frame = attachment.frame(pair.projection())?;
        let distance = if fit || self.attachment == CameraAttachment::System {
            4.0 * radius
        } else {
            self.distance_m.max(4.0 * radius)
        };
        ensure!(distance.is_finite(), "unrepresentable camera fit distance");
        self.attachment = attachment;
        self.anchor = LocalPosition::origin();
        self.orbit_basis = UnitRotation::identity();
        self.distance_m = distance;
        self.min_distance_m = 1.05 * radius;
        self.update_pose(frame)
    }
    pub fn orbit_zoom(&mut self, drag: [f64; 2], wheel: f64) -> Result<()> {
        ensure!(
            drag.iter().all(|x| x.is_finite()) && wheel.is_finite(),
            "invalid camera input"
        );
        let distance = (self.distance_m * (-wheel * 0.001).exp()).max(self.min_distance_m);
        ensure!(distance.is_finite(), "camera zoom overflow");
        self.yaw = (self.yaw - drag[0] * 0.005).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - drag[1] * 0.005).clamp(-1.5, 1.5);
        self.distance_m = distance;
        self.update_pose(self.pose.position().frame())
    }
    /// Coordinate and instantaneous physical-velocity preservation, not attachment.
    pub fn reexpress(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        attachment: CameraAttachment,
    ) -> Result<()> {
        let target = attachment.frame(pair.projection())?;
        let evaluation = pair.evaluation();
        let pose = evaluation.reexpress_pose(self.pose, target)?;
        let point = evaluation.convert_kinematic_point(
            KinematicPoint::try_new(self.pose.position(), self.velocity)?,
            target,
        )?;
        let conversion = evaluation.prepare_conversion(self.pose.position().frame(), target)?;
        let anchor = conversion
            .convert_position(FramePosition::new(
                self.pose.position().frame(),
                self.anchor,
            ))?
            .local();
        let basis = conversion.rotation().compose(self.orbit_basis);
        self.pose = pose;
        self.velocity = point.velocity();
        self.anchor = anchor;
        self.orbit_basis = basis;
        self.attachment = attachment;
        Ok(())
    }
    /// Rebuild changes only disposable handles, preserving role/body/local values.
    pub fn remap_projection(&mut self, projection: &CelestialFrameProjection) -> Result<()> {
        let frame = self.attachment.frame(projection)?;
        self.pose = FramePose::new(
            FramePosition::new(frame, self.pose.position().local()),
            self.pose.orientation(),
        );
        self.velocity = FrameVelocity::new(frame, self.velocity.relative());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gravity_fixtures::GravityFixture;
    use std::num::NonZeroU64;
    #[test]
    fn focus_navigation_reexpression_and_rebuild_preserve_world() {
        let world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let pair = projection.coherent_view(&world).unwrap();
        let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
        let revision = world.revision();
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
        camera.focus(&pair, ids[1], false, true).unwrap();
        let pose = camera.pose;
        let velocity = camera.velocity;
        camera
            .reexpress(&pair, CameraAttachment::BodyFixed(ids[1]))
            .unwrap();
        camera
            .reexpress(&pair, CameraAttachment::Translating(ids[1]))
            .unwrap();
        assert!(
            (camera.pose.position().local().metres() - pose.position().local().metres()).length()
                < 1e-7
        );
        assert!(
            (camera.velocity.relative().metres_per_second()
                - velocity.relative().metres_per_second())
            .length()
                < 1e-6
        );
        camera.orbit_zoom([40.0, 30.0], 500.0).unwrap();
        assert!(camera.distance_m() >= 1.05 * 6.371e6);
        let before = camera.pose;
        let rebuilt = CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
        camera.remap_projection(&rebuilt).unwrap();
        assert_ne!(camera.pose.position().frame(), before.position().frame());
        assert_eq!(camera.pose.position().local(), before.position().local());
        assert_eq!(world.revision(), revision);
    }
}
