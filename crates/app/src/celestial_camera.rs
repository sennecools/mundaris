//! One observer; selection is independent of explicit focus/attachment.
use anyhow::{Result, ensure};
use glam::{DQuat, DVec3};
use mundaris_math::*;
use mundaris_world::*;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    SystemOrbit,
    BodyOrbit,
    FreeFlight,
}
#[derive(Debug, Clone, Copy)]
pub enum FocusTarget {
    Overview { center_m: DVec3, distance_m: f64 },
    Body(BodyId),
}
#[derive(Debug, Clone, Copy)]
pub struct NavigationInput {
    pub drag: [f64; 2],
    pub scroll_notches: f64,
    pub translation: DVec3,
    pub speed_multiplier: f64,
}
impl Default for NavigationInput {
    fn default() -> Self {
        Self {
            drag: [0.0; 2],
            scroll_notches: 0.0,
            translation: DVec3::ZERO,
            speed_multiplier: 1.0,
        }
    }
}
#[derive(Clone)]
struct Transition {
    source_role: CameraAttachment,
    source: FramePose,
    target_role: CameraAttachment,
    target: FramePose,
    elapsed: Duration,
    target_distance: f64,
    target_radius: f64,
    target_anchor: LocalPosition,
}

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
#[derive(Clone)]
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
    radius_m: f64,
    mode: CameraMode,
    transition: Option<Transition>,
    zoom_target_log: f64,
    saved_bodies: Vec<(BodyId, f64, UnitRotation)>,
    saved_system: Option<(LocalPosition, f64, UnitRotation)>,
    carrier_center: Option<DVec3>,
    flight_speed_m_s: f64,
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
            distance_m: 2.5 * extent_m,
            yaw: 0.0,
            pitch: 0.0,
            min_distance_m: 1.05 * extent_m,
            radius_m: 0.0,
            mode: CameraMode::SystemOrbit,
            transition: None,
            zoom_target_log: (2.5 * extent_m).ln(),
            saved_bodies: Vec::new(),
            saved_system: None,
            carrier_center: None,
            flight_speed_m_s: 1.0,
        };
        camera.update_pose(root)?;
        Ok(camera)
    }
    pub fn pose(&self) -> FramePose {
        self.pose
    }
    pub fn set_overview_direction(&mut self, normal: DVec3) -> Result<()> {
        let normal = Direction3::try_new(normal)?.unit();
        self.orbit_basis = UnitRotation::try_from_quaternion(
            DQuat::from_rotation_arc(DVec3::Z, normal) * DQuat::from_rotation_x(0.12),
        )?;
        self.update_pose(self.pose.position().frame())
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
    pub fn mode(&self) -> CameraMode {
        self.mode
    }
    pub fn transitioning(&self) -> bool {
        self.transition.is_some()
    }
    pub fn focused_body(&self) -> Option<BodyId> {
        match self
            .transition
            .as_ref()
            .map_or(self.attachment, |t| t.target_role)
        {
            CameraAttachment::System => None,
            CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => Some(id),
        }
    }
    pub fn clearance_m(&self) -> f64 {
        (self.distance_m - self.radius_m).max(0.0)
    }
    pub fn flight_speed_m_s(&self) -> f64 {
        self.flight_speed_m_s
    }
    pub fn cancel_transition(&mut self) {
        self.transition = None;
    }
    fn save_view(&mut self) {
        if self.transition.is_some() {
            return;
        }
        if self.mode == CameraMode::SystemOrbit {
            self.saved_system = Some((self.anchor, self.distance_m, self.pose.orientation()));
        } else if self.mode == CameraMode::BodyOrbit
            && let Some(id) = self.focused_body()
        {
            let view = (id, self.clearance_m(), self.pose.orientation());
            if let Some(old) = self.saved_bodies.iter_mut().find(|b| b.0 == id) {
                *old = view;
            } else {
                self.saved_bodies.push(view);
            }
        }
    }
    /// Retarget from the currently displayed observer without a pose jump.
    pub fn transition_to(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        target: FocusTarget,
    ) -> Result<()> {
        self.save_view();
        let (role, anchor, distance, radius, orientation) = match target {
            FocusTarget::Body(id) => {
                let radius = pair.system().body(id)?.properties().reference_radius_m();
                let role = CameraAttachment::Translating(id);
                let frame = role.frame(pair.projection())?;
                let incoming = pair
                    .evaluation()
                    .reexpress_pose(self.pose, frame)?
                    .orientation();
                let (clearance, orientation) = self
                    .saved_bodies
                    .iter()
                    .find(|b| b.0 == id)
                    .map_or((3.0 * radius, incoming), |b| (b.1, b.2));
                (
                    role,
                    LocalPosition::origin(),
                    radius + clearance.max(minimum_clearance(radius)?),
                    radius,
                    orientation,
                )
            }
            FocusTarget::Overview {
                center_m,
                distance_m,
            } => {
                let orientation = self.saved_system.map_or(UnitRotation::identity(), |s| s.2);
                (
                    CameraAttachment::System,
                    LocalPosition::try_metres(center_m)?,
                    distance_m,
                    0.0,
                    orientation,
                )
            }
        };
        ensure!(
            distance.is_finite() && distance > radius && distance <= 1e15,
            "invalid navigation endpoint"
        );
        let frame = role.frame(pair.projection())?;
        let target = FramePose::new(
            FramePosition::new(
                frame,
                anchor.displaced(Displacement3::try_metres(
                    orientation.quaternion() * DVec3::Z * distance,
                )?)?,
            ),
            orientation,
        );
        let transition = Transition {
            source_role: self.attachment,
            source: self.pose,
            target_role: role,
            target,
            elapsed: Duration::ZERO,
            target_distance: distance,
            target_radius: radius,
            target_anchor: anchor,
        };
        // Validate the first sample before replacing a previous transition.
        pair.evaluation().reexpress_pose(self.pose, frame)?;
        self.transition = Some(transition);
        Ok(())
    }
    pub fn fit_overview(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        center_m: DVec3,
        distance_m: f64,
    ) -> Result<()> {
        self.transition_to(
            pair,
            FocusTarget::Overview {
                center_m,
                distance_m,
            },
        )
    }
    pub fn fit_body(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
        vertical_fov: f64,
        aspect: f64,
    ) -> Result<()> {
        ensure!(
            vertical_fov.is_finite()
                && vertical_fov > 0.0
                && vertical_fov < std::f64::consts::PI
                && aspect.is_finite()
                && aspect > 0.0,
            "invalid body fit projection"
        );
        self.transition_to(pair, FocusTarget::Body(body))?;
        let t = self.transition.as_mut().expect("new transition");
        let theta = ((vertical_fov * 0.5).tan() * aspect.min(1.0) * 0.8).atan();
        let distance = t.target_radius / theta.sin();
        t.target_distance = distance;
        t.target = FramePose::new(
            FramePosition::new(
                t.target.position().frame(),
                LocalPosition::try_metres(
                    t.target.orientation().quaternion() * DVec3::Z * distance,
                )?,
            ),
            t.target.orientation(),
        );
        Ok(())
    }
    /// Smoothly tracks geometric centre. Automatic fitting grows only with excursion;
    /// user orientation and explicit zoom survive ordinary centre updates.
    pub fn track_overview(
        &mut self,
        center: DVec3,
        minimum_fit: f64,
        grow: bool,
        elapsed: Duration,
    ) -> Result<()> {
        if self.mode != CameraMode::SystemOrbit || self.transition.is_some() {
            return Ok(());
        }
        let alpha = 1.0 - (-elapsed.as_secs_f64() / 0.2).exp();
        let anchor = LocalPosition::try_metres(self.anchor.metres().lerp(center, alpha))?;
        let distance = if grow && minimum_fit > self.distance_m * 1.1 {
            minimum_fit
        } else {
            self.distance_m
        };
        ensure!(
            distance.is_finite() && distance <= 1e15,
            "overview navigation limit"
        );
        self.anchor = anchor;
        if distance != self.distance_m {
            self.zoom_target_log = distance.ln();
        }
        self.distance_m = distance;
        self.update_pose(self.pose.position().frame())
    }
    /// Explicit unfocus preserves pose, then uses system-stationary editor motion.
    pub fn enter_free_flight(&mut self, pair: &CoherentCelestialView<'_>) -> Result<()> {
        self.save_view();
        self.transition = None;
        let root = pair.evaluation().root();
        // Keep a translating numerical carrier near a body; fixed spin is removed.
        let role = match self.attachment {
            CameraAttachment::BodyFixed(id) => CameraAttachment::Translating(id),
            role => role,
        };
        let frame = role.frame(pair.projection())?;
        let pose = pair.evaluation().reexpress_pose(self.pose, frame)?;
        let stationary = pair.evaluation().convert_kinematic_point(
            KinematicPoint::try_new(
                pair.evaluation().convert_position(pose.position(), root)?,
                FrameVelocity::new(root, LinearVelocity3::zero()),
            )?,
            frame,
        )?;
        self.pose = pose;
        self.velocity = stationary.velocity();
        self.attachment = role;
        self.mode = CameraMode::FreeFlight;
        self.carrier_center = match role {
            CameraAttachment::Translating(id) => {
                Some(pair.system().body(id)?.state().center_in_system().metres())
            }
            _ => None,
        };
        Ok(())
    }
    pub fn update_navigation(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        input: &NavigationInput,
        elapsed: Duration,
    ) -> Result<()> {
        ensure!(
            input.drag.iter().all(|v| v.is_finite())
                && input.scroll_notches.is_finite()
                && input.translation.is_finite()
                && input.speed_multiplier.is_finite()
                && input.speed_multiplier > 0.0,
            "invalid navigation input"
        );
        let mut candidate = self.clone();
        candidate.navigation_checked(pair, input, elapsed)?;
        *self = candidate;
        Ok(())
    }
    fn navigation_checked(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        input: &NavigationInput,
        elapsed: Duration,
    ) -> Result<()> {
        let active = input.drag != [0.0; 2]
            || input.scroll_notches != 0.0
            || input.translation != DVec3::ZERO;
        if active && self.transition.is_some() {
            self.enter_free_flight(pair)?;
        }
        if let Some(mut t) = self.transition.take() {
            t.elapsed = t.elapsed.saturating_add(elapsed);
            let u = (t.elapsed.as_secs_f64() / 0.9).clamp(0.0, 1.0);
            let target_frame = t.target_role.frame(pair.projection())?;
            let source_frame = t.source_role.frame(pair.projection())?;
            let source = FramePose::new(
                FramePosition::new(source_frame, t.source.position().local()),
                t.source.orientation(),
            );
            let source = pair.evaluation().reexpress_pose(source, target_frame)?;
            let start = source.position().local().metres();
            let end = t.target.position().local().metres();
            let anchor = t.target_anchor.metres();
            let separation = (end - start).length();
            let old_radius = match t.source_role {
                CameraAttachment::System => 0.0,
                CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => {
                    pair.system().body(id)?.properties().reference_radius_m()
                }
            };
            let direction = t.target.orientation().quaternion() * DVec3::Z;
            let transit = separation
                .max(4.0 * old_radius)
                .max(4.0 * t.target_radius)
                .max(t.target_distance);
            let waypoint_a = start + direction * transit;
            let waypoint_b = anchor + direction * (transit + t.target_distance);
            let smooth = |v: f64| v * v * (3.0 - 2.0 * v);
            let p = if separation < 0.01 * t.target_distance {
                let s = smooth(u);
                start.lerp(end, s)
            } else if u < 0.2 {
                start.lerp(waypoint_a, smooth(u / 0.2))
            } else if u < 0.5 {
                waypoint_a.lerp(waypoint_b, smooth((u - 0.2) / 0.3))
            } else {
                let s = smooth((u - 0.5) / 0.5);
                let clearance = ((transit + t.target_distance - t.target_radius).ln() * (1.0 - s)
                    + (t.target_distance - t.target_radius).ln() * s)
                    .exp();
                anchor + direction * (t.target_radius + clearance)
            };
            let orientation = UnitRotation::try_from_quaternion(
                source
                    .orientation()
                    .quaternion()
                    .slerp(t.target.orientation().quaternion(), smooth(u)),
            )?;
            let next = FramePose::new(
                FramePosition::new(
                    target_frame,
                    LocalPosition::try_metres(if u == 1.0 { end } else { p })?,
                ),
                orientation,
            );
            // Read-only navigation envelope, checked in local f64 coordinates.
            for role in [t.source_role, t.target_role] {
                if let CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) = role {
                    let frame = pair.projection().frames_for(id)?.translating;
                    let a = pair
                        .evaluation()
                        .convert_position(self.pose.position(), frame)?
                        .local()
                        .metres();
                    let b = pair
                        .evaluation()
                        .convert_position(next.position(), frame)?
                        .local()
                        .metres();
                    let delta = b - a;
                    let length = delta.length_squared();
                    let closest = if length > 0.0 {
                        a + delta * (-a.dot(delta) / length).clamp(0.0, 1.0)
                    } else {
                        a
                    };
                    let radius = pair.system().body(id)?.properties().reference_radius_m();
                    ensure!(
                        closest.length() >= radius,
                        "focus transition intersects reference sphere; observer retained"
                    );
                }
            }
            self.pose = next;
            self.attachment = t.target_role;
            self.velocity = FrameVelocity::new(target_frame, LinearVelocity3::zero());
            if u < 1.0 {
                self.transition = Some(t);
            } else {
                self.mode = if t.target_role == CameraAttachment::System {
                    CameraMode::SystemOrbit
                } else {
                    CameraMode::BodyOrbit
                };
                self.anchor = t.target_anchor;
                self.radius_m = t.target_radius;
                self.distance_m = t.target_distance;
                self.orbit_basis = t.target.orientation();
                self.yaw = 0.0;
                self.pitch = 0.0;
                self.min_distance_m = if self.radius_m > 0.0 {
                    self.radius_m + minimum_clearance(self.radius_m)?
                } else {
                    0.1
                };
                self.zoom_target_log = (self.distance_m - self.radius_m).ln();
            }
            return Ok(());
        }
        if self.mode == CameraMode::FreeFlight {
            return self.free_flight(pair, input, elapsed);
        }
        self.refresh_navigation_constraint(pair)?;
        self.yaw = (self.yaw - input.drag[0] * 0.005).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - input.drag[1] * 0.005).clamp(-1.5, 1.5);
        let minimum = (self.min_distance_m - self.radius_m).max(0.1);
        let target = self.zoom_target_log - input.scroll_notches * 1.25_f64.ln();
        ensure!(
            target.is_finite() && target <= (1e15 - self.radius_m).ln(),
            "1e15 m navigation zoom limit"
        );
        self.zoom_target_log = target.max(minimum.ln());
        let log = (self.distance_m - self.radius_m).max(minimum).ln();
        let next = self.zoom_target_log
            + (log - self.zoom_target_log) * (-elapsed.as_secs_f64() / 0.08).exp();
        self.distance_m = self.radius_m + next.exp();
        self.update_pose(self.attachment.frame(pair.projection())?)
    }
    fn free_flight(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        input: &NavigationInput,
        elapsed: Duration,
    ) -> Result<()> {
        if let (CameraAttachment::Translating(id), Some(previous)) =
            (self.attachment, self.carrier_center)
        {
            let current = pair.system().body(id)?.state().center_in_system().metres();
            let offset = self.pose.position().local().metres() + (previous - current);
            self.pose = FramePose::new(
                FramePosition::new(
                    self.pose.position().frame(),
                    LocalPosition::try_metres(offset)?,
                ),
                self.pose.orientation(),
            );
            self.carrier_center = Some(current);
        }
        let mut nearest = None;
        let mut scale = self.distance_m.max(1.0);
        for (id, body) in pair.system().bodies() {
            let p = pair
                .evaluation()
                .convert_position(
                    self.pose.position(),
                    pair.projection().frames_for(id)?.translating,
                )?
                .local()
                .metres();
            let distance = p.length();
            let clearance = (distance - body.properties().reference_radius_m()).max(1.0);
            if clearance < scale {
                scale = clearance;
                nearest = Some((id, distance, body.properties().reference_radius_m()));
            }
        }
        let role = if let Some((id, d, r)) = nearest
            && d < 32.0 * r
        {
            CameraAttachment::Translating(id)
        } else if let CameraAttachment::Translating(id) = self.attachment {
            let local = self.pose.position().local().metres().length();
            let r = pair.system().body(id)?.properties().reference_radius_m();
            if local < 64.0 * r {
                self.attachment
            } else {
                CameraAttachment::System
            }
        } else {
            CameraAttachment::System
        };
        let frame = role.frame(pair.projection())?;
        self.pose = pair.evaluation().reexpress_pose(self.pose, frame)?;
        self.attachment = role;
        self.carrier_center = match role {
            CameraAttachment::Translating(id) => {
                Some(pair.system().body(id)?.state().center_in_system().metres())
            }
            _ => None,
        };
        let look = UnitRotation::try_from_quaternion(
            DQuat::from_rotation_y(-input.drag[0] * 0.005)
                * DQuat::from_rotation_x(-input.drag[1] * 0.005),
        )?;
        let orientation = self.pose.orientation().compose(look);
        self.flight_speed_m_s = (0.5 * scale).clamp(1.0, 1e12) * input.speed_multiplier;
        let movement = if input.translation.length_squared() > 0.0 {
            input.translation.normalize()
        } else {
            DVec3::ZERO
        };
        let offset =
            orientation.quaternion() * movement * self.flight_speed_m_s * elapsed.as_secs_f64();
        self.pose = FramePose::new(
            self.pose.position().displaced(FrameDisplacement::new(
                frame,
                Displacement3::try_metres(offset)?,
            ))?,
            orientation,
        );
        // Wall navigation velocity is not a simulation derivative.
        let derivative = match role {
            CameraAttachment::Translating(id) => -pair
                .system()
                .body(id)?
                .state()
                .center_velocity_in_system()
                .metres_per_second(),
            _ => DVec3::ZERO,
        };
        self.velocity =
            FrameVelocity::new(frame, LinearVelocity3::try_metres_per_second(derivative)?);
        Ok(())
    }
    /// Property edits may change the navigation envelope while leaving physics
    /// unchanged. Reapply it against the coherent current body radius.
    pub fn refresh_navigation_constraint(
        &mut self,
        pair: &CoherentCelestialView<'_>,
    ) -> Result<()> {
        let id = match self.attachment {
            CameraAttachment::System => return Ok(()),
            CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => id,
        };
        if self.mode == CameraMode::FreeFlight || self.transition.is_some() {
            return Ok(());
        }
        let radius = pair.system().body(id)?.properties().reference_radius_m();
        let minimum = radius + minimum_clearance(radius)?;
        ensure!(
            minimum.is_finite(),
            "unrepresentable camera navigation distance"
        );
        self.min_distance_m = minimum;
        self.radius_m = radius;
        if self.distance_m < minimum {
            self.distance_m = minimum;
            self.zoom_target_log = (minimum - radius).ln();
            self.update_pose(self.pose.position().frame())?;
        }
        Ok(())
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
        self.min_distance_m = radius + minimum_clearance(radius)?;
        self.radius_m = radius;
        self.mode = CameraMode::BodyOrbit;
        self.transition = None;
        self.zoom_target_log = (distance - radius).ln();
        self.update_pose(frame)
    }
    pub fn orbit_zoom(&mut self, drag: [f64; 2], wheel: f64) -> Result<()> {
        ensure!(
            drag.iter().all(|x| x.is_finite()) && wheel.is_finite(),
            "invalid camera input"
        );
        let distance = self.radius_m
            + ((self.distance_m - self.radius_m) * (-wheel * 0.001).exp())
                .max(self.min_distance_m - self.radius_m);
        ensure!(distance.is_finite(), "camera zoom overflow");
        self.yaw = (self.yaw - drag[0] * 0.005).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - drag[1] * 0.005).clamp(-1.5, 1.5);
        self.distance_m = distance;
        self.zoom_target_log = (distance - self.radius_m).ln();
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
        if let Some(t) = &mut self.transition {
            t.source = FramePose::new(
                FramePosition::new(
                    t.source_role.frame(projection)?,
                    t.source.position().local(),
                ),
                t.source.orientation(),
            );
            t.target = FramePose::new(
                FramePosition::new(
                    t.target_role.frame(projection)?,
                    t.target.position().local(),
                ),
                t.target.orientation(),
            );
        }
        Ok(())
    }
}
pub fn minimum_clearance(radius_m: f64) -> Result<f64> {
    ensure!(
        radius_m.is_finite() && radius_m > 0.0,
        "invalid reference radius"
    );
    let clearance = (64.0 * (radius_m.next_up() - radius_m)).max(1.0);
    ensure!(clearance.is_finite(), "unrepresentable reference clearance");
    Ok(clearance)
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
