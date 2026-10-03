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
    SurfaceInspection,
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
    saved_system: Option<(LocalPosition, f64, UnitRotation, SimulationInstant)>,
    carrier_center: Option<DVec3>,
    flight_speed_m_s: f64,
    flight_log_scale: Option<f64>,
    inspection: Option<crate::planet_surface::SurfaceInspectionAnchor>,
    navigation_envelope: bool,
    terrain_clearance_guard_m: Option<f64>,
    terrain_approach: bool,
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
            flight_log_scale: None,
            inspection: None,
            navigation_envelope: true,
            terrain_clearance_guard_m: None,
            terrain_approach: false,
        };
        camera.update_pose(root)?;
        Ok(camera)
    }
    pub fn pose(&self) -> FramePose {
        self.pose
    }
    /// The transported inspection basis also drives the local debug axes.
    pub(crate) fn inspection_tangent(&self) -> Option<mundaris_math::surface::SurfaceTangentBasis> {
        self.inspection.map(|anchor| anchor.tangent)
    }
    /// Explicit co-rotating attachment; orientation/position are re-expressed first.
    /// Zero relative simulation derivative is a policy change, not velocity preservation.
    pub fn enter_surface_inspection(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<()> {
        ensure!(
            self.focused_body() == Some(body) && !self.transitioning(),
            "complete focus before surface inspection"
        );
        let frame = pair.projection().frames_for(body)?.body_fixed;
        let pose = pair.evaluation().reexpress_pose(self.pose, frame)?;
        let radius = pair.system().body(body)?.properties().reference_radius_m();
        let anchor = crate::planet_surface::SurfaceInspectionAnchor::new(
            body,
            pose.position().local().metres(),
            radius,
            None,
        )?;
        self.pose = pose;
        self.velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
        self.mode = CameraMode::SurfaceInspection;
        self.attachment = CameraAttachment::BodyFixed(body);
        self.inspection = Some(anchor);
        self.transition = None;
        Ok(())
    }
    pub fn look_surface_horizon(&mut self) -> Result<()> {
        ensure!(
            self.mode == CameraMode::SurfaceInspection,
            "enter surface inspection first"
        );
        let anchor = self.inspection.expect("inspection owns anchor");
        self.pose = FramePose::new(self.pose.position(), anchor.body_from_regional.rotation());
        Ok(())
    }
    /// Orient toward an independently simulated body without changing this observer's
    /// position, attachment, selected identity or the target's authoritative state.
    pub fn look_at_body(&mut self, pair: &CoherentCelestialView<'_>, target: BodyId) -> Result<()> {
        let frame = self.pose.position().frame();
        let target_frame = pair.projection().frames_for(target)?.translating;
        let target = pair
            .evaluation()
            .convert_position(
                FramePosition::new(target_frame, LocalPosition::origin()),
                frame,
            )?
            .local();
        let forward = Direction3::try_new(
            target
                .displacement_from(self.pose.position().local())?
                .metres(),
        )?
        .unit();
        let up = self
            .inspection
            .map_or(self.pose.orientation().quaternion() * DVec3::Y, |a| {
                a.tangent.up().unit()
            });
        let mut right = forward.cross(up);
        if right.length() < 1e-6 {
            let previous = self.pose.orientation().quaternion() * DVec3::X;
            right = previous - forward * previous.dot(forward);
        }
        let right = Direction3::try_new(right)?.unit();
        let up = Direction3::try_new(right.cross(forward))?.unit();
        let orientation = UnitRotation::try_from_quaternion(DQuat::from_mat3(
            &glam::DMat3::from_cols(right, up, -forward),
        ))?;
        self.pose = FramePose::new(self.pose.position(), orientation);
        Ok(())
    }
    pub fn measured_clearance(
        &self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<f64> {
        let frame = pair.projection().frames_for(body)?.body_fixed;
        let p = pair
            .evaluation()
            .convert_position(self.pose.position(), frame)?
            .local()
            .metres();
        Ok(p.length() - pair.system().body(body)?.properties().reference_radius_m())
    }
    /// Debug bypass only; this is a reference-sphere navigation guard, not collision.
    pub fn set_navigation_envelope(&mut self, enabled: bool) {
        self.navigation_envelope = enabled;
    }
    /// Optional navigation-only clearance above sampled terrain; disabled by default.
    pub fn set_terrain_clearance_guard(&mut self, clearance_m: Option<f64>) -> Result<()> {
        ensure!(
            clearance_m.is_none_or(|x| x.is_finite() && x >= 0.0),
            "invalid terrain clearance guard"
        );
        self.terrain_clearance_guard_m = clearance_m;
        Ok(())
    }
    /// Push an observer outward to an explicit terrain radius. This is navigation
    /// correction only; it does not modify world state or imply collision physics.
    pub fn enforce_radial_clearance(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
        surface_radius_m: f64,
        minimum_clearance_m: f64,
    ) -> Result<bool> {
        ensure!(
            surface_radius_m.is_finite()
                && surface_radius_m > 0.0
                && minimum_clearance_m.is_finite()
                && minimum_clearance_m >= 0.0,
            "invalid terrain radial clearance"
        );
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let local = pair
            .evaluation()
            .convert_position(self.pose.position(), fixed)?
            .local()
            .metres();
        let radius = local.length();
        let minimum_radius = surface_radius_m + minimum_clearance_m;
        if radius >= minimum_radius {
            return Ok(false);
        }
        let guarded = Direction3::try_new(local)?.unit() * minimum_radius;
        let fixed_pose = pair.evaluation().reexpress_pose(self.pose, fixed)?;
        let pose = FramePose::new(
            FramePosition::new(fixed, LocalPosition::try_metres(guarded)?),
            fixed_pose.orientation(),
        );
        self.pose = pair
            .evaluation()
            .reexpress_pose(pose, self.pose.position().frame())?;
        if self.mode == CameraMode::SurfaceInspection {
            let anchor = self.inspection.expect("inspection owns anchor");
            self.inspection = Some(crate::planet_surface::SurfaceInspectionAnchor::new(
                body,
                guarded,
                pair.system().body(body)?.properties().reference_radius_m(),
                Some(anchor.tangent),
            )?);
        } else if self.mode == CameraMode::BodyOrbit && self.focused_body() == Some(body) {
            let terrain_height = self
                .terrain_height_at_orbit_direction(pair, body)?
                .unwrap_or(0.0);
            let clearance = minimum_radius - self.radius_m - terrain_height;
            self.distance_m = minimum_radius;
            self.zoom_target_log = clearance.max(1.0).ln();
            self.terrain_approach = pair.system().body(body)?.terrain().is_some();
        }
        Ok(true)
    }
    /// Repeatable approach target; admitted wall-time smoothing changes only observer state.
    pub fn target_clearance(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        clearance: f64,
    ) -> Result<()> {
        if self.mode == CameraMode::SurfaceInspection && !self.transitioning() {
            ensure!(
                clearance.is_finite() && clearance >= 1.0,
                "invalid terrain clearance"
            );
            let body = self
                .focused_body()
                .ok_or_else(|| anyhow::anyhow!("inspection has no body"))?;
            let fixed = pair.projection().frames_for(body)?.body_fixed;
            let pose = pair.evaluation().reexpress_pose(self.pose, fixed)?;
            let direction = Direction3::try_new(pose.position().local().metres())?;
            let radius = crate::terrain_inspection::terrain_clearance(pair, self.pose, body)?
                .map_or(
                    pair.system().body(body)?.properties().reference_radius_m(),
                    |c| c.surface_radius_m,
                );
            let position = direction.unit() * (radius + clearance);
            self.pose = FramePose::new(
                FramePosition::new(fixed, LocalPosition::try_metres(position)?),
                pose.orientation(),
            );
            self.inspection = Some(crate::planet_surface::SurfaceInspectionAnchor::new(
                body,
                position,
                pair.system().body(body)?.properties().reference_radius_m(),
                self.inspection.map(|a| a.tangent),
            )?);
            return Ok(());
        }
        ensure!(
            self.mode == CameraMode::BodyOrbit && !self.transitioning(),
            "complete body orbit focus before approach"
        );
        self.refresh_navigation_constraint(pair)?;
        let body = self.focused_body().expect("body orbit has focus");
        let terrain_height = self.terrain_height_at_orbit_direction(pair, body)?;
        self.terrain_approach = terrain_height.is_some();
        let terrain_height = terrain_height.unwrap_or(0.0);
        ensure!(
            clearance.is_finite()
                && clearance >= 1.0
                && clearance + self.radius_m + terrain_height <= 1e15,
            "invalid navigation clearance"
        );
        self.zoom_target_log = clearance.ln();
        Ok(())
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
        if self.mode == CameraMode::FreeFlight && self.transition.is_none() {
            return None;
        }
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
    fn save_view(&mut self, pair: &CoherentCelestialView<'_>) {
        if self.transition.is_some() {
            return;
        }
        if self.mode == CameraMode::SystemOrbit {
            self.saved_system = Some((
                self.anchor,
                self.distance_m,
                self.pose.orientation(),
                pair.system().sample_time(),
            ));
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
        self.save_view(pair);
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
                let distance_m = self
                    .saved_system
                    .filter(|s| s.3 == pair.system().sample_time() && s.0.metres() == center_m)
                    .map_or(distance_m, |s| s.1.max(distance_m));
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
        self.save_view(pair);
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
        self.inspection = None;
        self.flight_log_scale = None;
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
        if let Some(minimum) = candidate.terrain_clearance_guard_m {
            candidate.apply_terrain_guard(pair, minimum)?;
        }
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
            let old_anchor = match t.source_role {
                CameraAttachment::System => anchor,
                CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => pair
                    .evaluation()
                    .convert_position(
                        FramePosition::new(
                            pair.projection().frames_for(id)?.translating,
                            LocalPosition::origin(),
                        ),
                        target_frame,
                    )?
                    .local()
                    .metres(),
            };
            let outgoing = if old_radius > 0.0 {
                Direction3::try_new(start - old_anchor)?.unit()
            } else {
                direction
            };
            let transit = separation
                .max(4.0 * old_radius)
                .max(4.0 * t.target_radius)
                .max(t.target_distance);
            // Free-flight look can face away from the old body. Pull back along
            // its actual outward radial direction, then transit outside the pair.
            let waypoint_a = start + outgoing * transit;
            let waypoint_b = anchor + direction * (transit + t.target_distance);
            let smooth = |v: f64| v * v * (3.0 - 2.0 * v);
            let p = if separation < 0.01 * t.target_distance {
                let s = smooth(u);
                start.lerp(end, s)
            } else if u < 0.2 {
                start.lerp(waypoint_a, smooth(u / 0.2))
            } else if u < 0.5 {
                let center = old_anchor + (anchor - old_anchor) * 0.5;
                let a = waypoint_a - center;
                let b = waypoint_b - center;
                let a_length = a.length();
                let b_length = b.length();
                let s = smooth((u - 0.2) / 0.3);
                let rotation = DQuat::from_rotation_arc(
                    Direction3::try_new(a)?.unit(),
                    Direction3::try_new(b)?.unit(),
                );
                let radial = DQuat::IDENTITY.slerp(rotation, s) * (a / a_length);
                center + radial * ((a_length.ln() * (1.0 - s) + b_length.ln() * s).exp())
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
                self.terrain_approach = false;
                self.anchor = t.target_anchor;
                self.radius_m = t.target_radius;
                self.distance_m = t.target_distance;
                self.orbit_basis = t.target.orientation();
                self.yaw = 0.0;
                self.pitch = 0.0;
                self.min_distance_m = if self.radius_m > 0.0 {
                    self.radius_m + minimum_clearance(self.radius_m)?
                } else {
                    // Current 60-degree padded system fit is outside twice its
                    // enclosing radius; keep system zoom outside the fitted core.
                    0.5 * t.target_distance
                };
                self.zoom_target_log = (self.distance_m - self.radius_m).ln();
            }
            return Ok(());
        }
        if self.mode == CameraMode::FreeFlight {
            return self.free_flight(pair, input, elapsed);
        }
        if self.mode == CameraMode::SurfaceInspection {
            return self.inspect_motion(pair, input, elapsed);
        }
        self.refresh_navigation_constraint(pair)?;
        self.yaw = (self.yaw - input.drag[0] * 0.005).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - input.drag[1] * 0.005).clamp(-1.5, 1.5);
        if self.terrain_approach {
            let id = self.focused_body().expect("terrain approach has a body");
            let effective_radius = self.radius_m
                + self
                    .terrain_height_at_orbit_direction(pair, id)?
                    .unwrap_or(0.0);
            let target = self.zoom_target_log - input.scroll_notches * 1.25_f64.ln();
            ensure!(
                target.is_finite() && target <= (1e15 - effective_radius).ln(),
                "1e15 m navigation zoom limit"
            );
            self.zoom_target_log = target.max(1.0_f64.ln());
            let clearance = (self.distance_m - effective_radius).max(1.0);
            let next = self.zoom_target_log
                + (clearance.ln() - self.zoom_target_log) * (-elapsed.as_secs_f64() / 0.08).exp();
            self.distance_m = effective_radius + next.exp();
            return self.update_pose(self.attachment.frame(pair.projection())?);
        }
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
        let target_log = scale.max(1.0).ln();
        let blended = self.flight_log_scale.map_or(target_log, |previous| {
            target_log + (previous - target_log) * (-elapsed.as_secs_f64() / 0.15).exp()
        });
        self.flight_log_scale = Some(blended);
        self.flight_speed_m_s = (0.5 * blended.exp()).clamp(1.0, 1e12) * input.speed_multiplier;
        let movement = if input.translation != DVec3::ZERO {
            Direction3::try_new(input.translation)?.unit()
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
        if self.navigation_envelope {
            for (id, body) in pair.system().bodies() {
                let fixed = pair.projection().frames_for(id)?.body_fixed;
                let local = pair.evaluation().reexpress_pose(self.pose, fixed)?;
                let p = local.position().local().metres();
                let radius = body.properties().reference_radius_m();
                let minimum = radius + minimum_clearance(radius)?;
                if p.length() < minimum {
                    let guarded = FramePose::new(
                        FramePosition::new(
                            fixed,
                            LocalPosition::try_metres(Direction3::try_new(p)?.unit() * minimum)?,
                        ),
                        local.orientation(),
                    );
                    self.pose = pair.evaluation().reexpress_pose(guarded, frame)?;
                }
            }
        }
        if let Some(guard) = self.terrain_clearance_guard_m {
            self.apply_terrain_guard(pair, guard)?;
        }
        Ok(())
    }
    fn inspect_motion(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        input: &NavigationInput,
        elapsed: Duration,
    ) -> Result<()> {
        let mut anchor = self.inspection.expect("inspection owns anchor");
        let radius = pair
            .system()
            .body(anchor.body)?
            .properties()
            .reference_radius_m();
        let look = UnitRotation::try_from_quaternion(
            DQuat::from_rotation_y(-input.drag[0] * 0.005)
                * DQuat::from_rotation_x(-input.drag[1] * 0.005),
        )?;
        let orientation = self.pose.orientation().compose(look);
        let clearance = self.pose.position().local().metres().length() - radius;
        self.flight_speed_m_s = (0.5 * clearance).clamp(1.0, 1e12) * input.speed_multiplier;
        let movement = if input.translation == DVec3::ZERO {
            DVec3::ZERO
        } else {
            Direction3::try_new(input.translation)?.unit()
        };
        if movement != DVec3::ZERO {
            let offset =
                orientation.quaternion() * movement * self.flight_speed_m_s * elapsed.as_secs_f64();
            let regional = anchor
                .body_from_regional
                .rotation()
                .inverse()
                .rotate_displacement(Displacement3::try_metres(offset)?)?;
            anchor.observer_in_regional = anchor.observer_in_regional.displaced(regional)?;
        }
        let p = anchor.position()?.metres();
        let minimum = radius + minimum_clearance(radius)?;
        let guarded = if self.navigation_envelope
            && pair.system().body(anchor.body)?.terrain().is_none()
            && p.length() < minimum
        {
            Direction3::try_new(p)?.unit() * minimum
        } else {
            p
        };
        let guarded = if let Some(clearance) = self.terrain_clearance_guard_m {
            let definition = pair.system().body(anchor.body)?.terrain();
            let diagnostic = definition
                .map(|definition| {
                    crate::terrain_inspection::clearance_at_position(
                        definition,
                        radius,
                        p,
                        anchor.body,
                    )
                })
                .transpose()?;
            if let Some(diagnostic) = diagnostic {
                if diagnostic.clearance_m < clearance {
                    Direction3::try_new(p)?.unit() * (diagnostic.surface_radius_m + clearance)
                } else {
                    guarded
                }
            } else {
                guarded
            }
        } else {
            guarded
        };
        if guarded != p || anchor.observer_in_regional.metres().length() > 1000.0 {
            anchor = crate::planet_surface::SurfaceInspectionAnchor::new(
                anchor.body,
                guarded,
                radius,
                Some(anchor.tangent),
            )?;
        }
        let frame = self.attachment.frame(pair.projection())?;
        self.pose = FramePose::new(FramePosition::new(frame, anchor.position()?), orientation);
        self.velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
        self.inspection = Some(anchor);
        Ok(())
    }
    fn apply_terrain_guard(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        clearance_m: f64,
    ) -> Result<()> {
        let id = match self.attachment {
            CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => id,
            CameraAttachment::System => return Ok(()),
        };
        if let Some(diagnostic) = crate::terrain_inspection::terrain_clearance(pair, self.pose, id)?
            && diagnostic.clearance_m < clearance_m
        {
            self.enforce_radial_clearance(pair, id, diagnostic.surface_radius_m, clearance_m)?;
        }
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
        if self.mode == CameraMode::FreeFlight
            || self.mode == CameraMode::SurfaceInspection
            || self.transition.is_some()
        {
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
        if self.terrain_approach && pair.system().body(id)?.terrain().is_some() {
            return Ok(());
        }
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
        self.terrain_approach = false;
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
    fn terrain_height_at_orbit_direction(
        &self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<Option<f64>> {
        let Some(definition) = pair.system().body(body)?.terrain() else {
            return Ok(None);
        };
        let rotation = self.orbit_basis.compose(UnitRotation::try_from_quaternion(
            DQuat::from_rotation_y(self.yaw) * DQuat::from_rotation_x(self.pitch),
        )?);
        let direction = rotation.rotate_direction(Direction3::try_new(DVec3::Z)?)?;
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let direction = pair
            .evaluation()
            .convert_direction(
                FrameDirection::new(self.attachment.frame(pair.projection())?, direction),
                fixed,
            )?
            .local()
            .unit();
        let radius = pair.system().body(body)?.properties().reference_radius_m();
        let sample = crate::terrain_inspection::clearance_at_position(
            definition,
            radius,
            direction * radius,
            body,
        )?;
        Ok(Some(sample.terrain_elevation_m))
    }
    /// Instantaneous pose/physical-velocity preservation between the focused body's
    /// translating/fixed debug roles. A new pivot uses Focus, not coordinate migration.
    pub fn reexpress(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        attachment: CameraAttachment,
    ) -> Result<()> {
        ensure!(
            self.mode == CameraMode::BodyOrbit && !self.transitioning(),
            "complete body focus before debug frame re-expression"
        );
        let target_body = match attachment {
            CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id) => Some(id),
            CameraAttachment::System => None,
        };
        ensure!(
            target_body == self.focused_body(),
            "debug frame re-expression must retain the focused BodyId; use Focus for a new pivot"
        );
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
    fn inspection_debug_basis_matches_transported_horizon_after_reanchoring() {
        let world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let pair = projection.coherent_view(&world).unwrap();
        let body = world.bodies().nth(1).unwrap().0;
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1.6e11).unwrap();
        camera.focus(&pair, body, true, true).unwrap();
        assert!(camera.inspection_tangent().is_none());
        camera.enter_surface_inspection(&pair, body).unwrap();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    translation: DVec3::new(1.0, 1.0, 0.0),
                    ..Default::default()
                },
                Duration::from_secs(1),
            )
            .unwrap();
        let tangent = camera.inspection_tangent().unwrap();
        let static_basis = mundaris_math::surface::SurfaceTangentBasis::new(tangent.up());
        assert!((tangent.east().unit() - static_basis.east().unit()).length() > 1e-6);
        camera.look_surface_horizon().unwrap();
        let rotation = camera.pose().orientation().quaternion();
        assert!((rotation * DVec3::X - tangent.east().unit()).length() <= 1e-12);
        assert!((rotation * DVec3::Y - tangent.up().unit()).length() <= 1e-12);
        assert!((rotation * DVec3::NEG_Z - tangent.north().unit()).length() <= 1e-12);
    }
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
        assert!(
            camera
                .reexpress(&pair, CameraAttachment::BodyFixed(ids[2]))
                .is_err()
        );
        assert_eq!(camera.focused_body(), Some(ids[1]));
        assert!(
            (camera.pose().position().local().metres() - pose.position().local().metres()).length()
                < 1e-7
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
