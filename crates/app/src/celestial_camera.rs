//! One observer; selection is independent of explicit focus/attachment.
use anyhow::{Result, ensure};
use astrum_math::*;
use astrum_world::*;
use glam::{DQuat, DVec3};
use std::time::Duration;

/// Observational controller state; wall-navigation speed is not a simulation derivative.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NavigationDiagnostics {
    #[serde(default)]
    pub last_wheel_notches: Option<f64>,
    #[serde(default)]
    pub window_focused: Option<bool>,
    #[serde(default)]
    pub viewport_keyboard_owned: Option<bool>,
    #[serde(default)]
    pub viewport_gesture_owned: Option<bool>,
    pub attachment_policy: String,
    pub transitioning: bool,
    pub base_speed_m_s: f64,
    pub base_source: String,
    pub user_multiplier: f64,
    pub boost_multiplier: f64,
    pub effective_speed_m_s: f64,
    pub requested_clearance_m: Option<f64>,
    pub requested_distance_m: Option<f64>,
    pub zoom_target_meaning: String,
    pub pending_forward_m: f64,
    pub safeguard: String,
    #[serde(default)]
    pub safeguard_minimum_clearance_m: Option<f64>,
    pub local_radians_per_logical_pixel: f64,
    pub orbit_radians_per_logical_pixel: f64,
    pub wheel_log_per_notch: f64,
    pub logical_viewport_height: f64,
    pub terrain_query_count: u64,
    pub terrain_query_us: f64,
}

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
    /// Temporary boost, separate from the user multiplier.
    pub boost_multiplier: f64,
}
impl Default for NavigationInput {
    fn default() -> Self {
        Self {
            drag: [0.0; 2],
            scroll_notches: 0.0,
            translation: DVec3::ZERO,
            speed_multiplier: 1.0,
            boost_multiplier: 1.0,
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

/// Where camera clearance reads the terrain surface (ADR 0023).
#[derive(Debug, Clone)]
pub enum SurfaceSource {
    /// Read-back GPU colliders, the runtime terrain authority. Queries may be
    /// pending for a few frames; misses are forwarded to the producer.
    Colliders(crate::planet_lod::collision::ColliderView),
    /// The CPU surface generator: the test oracle, for headless tests that
    /// run the camera without a renderer.
    CpuOracle,
}

impl Default for SurfaceSource {
    fn default() -> Self {
        Self::Colliders(Default::default())
    }
}

#[derive(Clone, PartialEq)]
#[allow(clippy::large_enum_variant)] // Retain value identity without changing camera/source behavior for this comparison.
enum TerrainAuthority {
    Legacy(astrum_world::terrain::TerrainDefinition),
    Compositional(astrum_world::terrain::SurfaceDefinition),
}
impl TerrainAuthority {
    fn for_body(body: &CelestialBody) -> Option<Self> {
        body.surface_definition()
            .cloned()
            .map(Self::Compositional)
            .or_else(|| body.terrain().cloned().map(Self::Legacy))
    }
}

/// Lowest radius the body's surface can reach: the definition's conservative
/// envelope (a bound, not a surface evaluation). Legacy terrain is constrained
/// to a 10% radial envelope.
fn lowest_surface_radius_m(definition: &TerrainAuthority, radius_m: f64) -> Result<f64> {
    Ok(match definition {
        TerrainAuthority::Compositional(definition) => {
            astrum_world::terrain::SurfaceGenerator::new(definition, radius_m)?
                .conservative_radius_envelope_m()[0]
        }
        TerrainAuthority::Legacy(_) => radius_m * 0.9,
    })
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
    orbit_look_offset: UnitRotation,
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
    inspection: Option<crate::surface_anchor::SurfaceInspectionAnchor>,
    navigation_envelope: bool,
    terrain_clearance_guard_m: Option<f64>,
    terrain_approach: bool,
    logical_viewport_height: f64,
    fov_y_rad: f64,
    wheel_pending: DVec3,
    base_speed_m_s: f64,
    user_multiplier: f64,
    boost_multiplier: f64,
    response_clearance_m: f64,
    base_source: &'static str,
    clearance_sample: Option<crate::terrain_inspection::TerrainClearance>,
    sampled_definition: Option<TerrainAuthority>,
    sampled_radius_m: Option<f64>,
    terrain_query_count: u64,
    terrain_query_us: f64,
    surface: SurfaceSource,
    /// A fixture pose placed while its surface was pending; checked for
    /// ground clearance once the surface resolves.
    pending_surface_check: Option<BodyId>,
    /// A surface-inspection clearance target requested while its surface was
    /// pending; applied once the collider page arrives.
    pending_target_clearance: Option<f64>,
    surface_heading: DVec3,
    surface_pitch: f64,
    last_wheel_notches: Option<f64>,
    #[cfg(feature = "developer-tools")]
    developer_fixture_pose: Option<FramePose>,
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
            orbit_look_offset: UnitRotation::identity(),
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
            logical_viewport_height: 1080.0,
            fov_y_rad: 60_f64.to_radians(),
            wheel_pending: DVec3::ZERO,
            base_speed_m_s: 1.0,
            user_multiplier: 1.0,
            boost_multiplier: 1.0,
            response_clearance_m: 2.5 * extent_m,
            base_source: "overview_distance",
            clearance_sample: None,
            sampled_definition: None,
            sampled_radius_m: None,
            terrain_query_count: 0,
            terrain_query_us: 0.0,
            surface: SurfaceSource::default(),
            pending_surface_check: None,
            pending_target_clearance: None,
            surface_heading: -DVec3::Z,
            surface_pitch: 0.0,
            last_wheel_notches: None,
            #[cfg(feature = "developer-tools")]
            developer_fixture_pose: None,
        };
        camera.update_pose(root)?;
        Ok(camera)
    }
    pub fn pose(&self) -> FramePose {
        self.pose
    }
    /// Use the prepared content projection, not window dimensions or a guessed FOV.
    pub fn set_navigation_projection(
        &mut self,
        projection: astrum_renderer::CelestialProjection,
        pixels_per_logical_pixel: f64,
    ) -> Result<()> {
        ensure!(
            pixels_per_logical_pixel.is_finite() && pixels_per_logical_pixel > 0.0,
            "invalid navigation DPI scale"
        );
        self.logical_viewport_height = projection.viewport()[1] as f64 / pixels_per_logical_pixel;
        self.fov_y_rad = projection.vertical_fov_rad();
        Ok(())
    }
    fn local_response(&self) -> f64 {
        let c = self.response_clearance_m.max(1.0);
        2.0 * (self.fov_y_rad * 0.5).tan() / self.logical_viewport_height
            * (0.04 + 0.96 * c / (c + 10_000.0))
    }
    fn orbit_response(&self) -> f64 {
        let gain = if self.mode == CameraMode::SystemOrbit {
            1.0
        } else {
            (self.response_clearance_m.max(1.0) / self.distance_m.max(1.0))
                .sqrt()
                .clamp(1e-8, 1.0)
        };
        2.0 * (self.fov_y_rad * 0.5).tan() / self.logical_viewport_height * gain
    }
    fn wheel_response(&self) -> f64 {
        (1.25_f64.ln() * (self.fov_y_rad * 0.5).tan() / 30_f64.to_radians().tan()).clamp(0.05, 0.6)
    }
    pub fn navigation_diagnostics(&self) -> NavigationDiagnostics {
        let minimum = match self.mode {
            CameraMode::SystemOrbit => None,
            CameraMode::BodyOrbit => Some(1.0),
            CameraMode::SurfaceInspection => self
                .terrain_clearance_guard_m
                .or(self.navigation_envelope.then_some(1.0)),
            CameraMode::FreeFlight => {
                if self.terrain_sampled() {
                    self.terrain_clearance_guard_m
                        .or(self.navigation_envelope.then_some(1.0))
                        .map(|x| {
                            if self.navigation_envelope {
                                x.max(1.0)
                            } else {
                                x
                            }
                        })
                } else {
                    self.navigation_envelope.then_some(1.0)
                }
            }
        };
        NavigationDiagnostics {
            last_wheel_notches: self.last_wheel_notches,
            window_focused: None,
            viewport_keyboard_owned: None,
            viewport_gesture_owned: None,
            attachment_policy: match self.mode {
                CameraMode::SystemOrbit => "system_pivot",
                CameraMode::BodyOrbit => "body_pivot",
                CameraMode::SurfaceInspection => "body_fixed_editor",
                CameraMode::FreeFlight => "system_stationary_editor",
            }
            .into(),
            transitioning: self.transitioning(),
            base_speed_m_s: self.base_speed_m_s,
            base_source: self.base_source.into(),
            user_multiplier: self.user_multiplier,
            boost_multiplier: self.boost_multiplier,
            effective_speed_m_s: self.base_speed_m_s * self.user_multiplier * self.boost_multiplier,
            requested_clearance_m: (self.mode == CameraMode::BodyOrbit && !self.transitioning())
                .then(|| self.zoom_target_log.exp()),
            requested_distance_m: (self.mode == CameraMode::SystemOrbit && !self.transitioning())
                .then(|| self.zoom_target_log.exp()),
            zoom_target_meaning: match self.mode {
                CameraMode::SystemOrbit => "pivot_distance",
                CameraMode::BodyOrbit => {
                    if self.terrain_approach {
                        "complete_terrain_clearance"
                    } else {
                        "reference_sphere_clearance"
                    }
                }
                _ => "view_forward_destination",
            }
            .into(),
            pending_forward_m: self
                .wheel_pending
                .dot(self.pose.orientation().quaternion() * -DVec3::Z),
            safeguard: if self.mode == CameraMode::SystemOrbit {
                "overview_pivot_distance_minimum"
            } else if minimum.is_none() {
                "disabled"
            } else if self.base_source == "gpu_collider" {
                "sampled_gpu_collider_radial"
            } else if self.terrain_sampled() {
                "sampled_complete_terrain_radial"
            } else if self.base_source == "terrain_pending" {
                "pending_surface_far_field_floor"
            } else {
                "reference_sphere_radial_fallback"
            }
            .into(),
            safeguard_minimum_clearance_m: minimum,
            local_radians_per_logical_pixel: self.local_response(),
            orbit_radians_per_logical_pixel: self.orbit_response(),
            wheel_log_per_notch: self.wheel_response(),
            logical_viewport_height: self.logical_viewport_height,
            terrain_query_count: self.terrain_query_count,
            terrain_query_us: self.terrain_query_us,
        }
    }
    /// Reuse this complete sample only through `sample_clearance`, which validates
    /// body, definition, radius and direction. Snapshot collection never queries.
    pub fn recorded_terrain_clearance(
        &self,
    ) -> Option<crate::terrain_inspection::TerrainClearance> {
        self.clearance_sample
    }
    fn sample_clearance(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
        pose: FramePose,
    ) -> Result<f64> {
        let celestial = pair.system().body(body)?;
        let radius = celestial.properties().reference_radius_m();
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let p = pair
            .evaluation()
            .convert_position(pose.position(), fixed)?
            .local()
            .metres();
        let distance = p.length();
        let direction = Direction3::try_new(p)?.unit();
        let Some(definition) = TerrainAuthority::for_body(celestial) else {
            self.clearance_sample = None;
            self.sampled_definition = None;
            self.sampled_radius_m = None;
            self.base_source = "reference_sphere_fallback";
            return Ok(distance - radius);
        };
        let reused = self.clearance_sample.filter(|s| {
            s.body == body
                && self.sampled_definition.as_ref() == Some(&definition)
                && self.sampled_radius_m == Some(radius)
                && (s.location.direction().unit() - direction).length() <= 1e-14
        });
        let sample = if let Some(mut s) = reused {
            s.camera_radius_m = distance;
            s.sphere_altitude_m = distance - radius;
            s.clearance_m = distance - s.surface_radius_m;
            s
        } else {
            let start = std::time::Instant::now();
            let queried = self.query_surface(pair, body, pose)?;
            self.terrain_query_us += start.elapsed().as_secs_f64() * 1e6;
            self.terrain_query_count += 1;
            let Some(s) = queried else {
                // Pending: assume the lowest surface that can exist here (the
                // far-field collider's minimum, else the definition's radius
                // envelope), so a pending query never lifts the observer; the
                // real surface takes over when its page arrives.
                let far_field = match &self.surface {
                    SurfaceSource::Colliders(view) => {
                        astrum_world::terrain::surface_query::SurfaceQuery::far_field_bounds_m(
                            view, body, direction,
                        )
                        .map(|[low, _]| radius + low)
                    }
                    SurfaceSource::CpuOracle => None,
                };
                let surface = match far_field {
                    Some(floor) => floor,
                    None => lowest_surface_radius_m(&definition, radius)?,
                };
                self.base_source = "terrain_pending";
                return Ok(distance - surface);
            };
            self.sampled_definition = Some(definition);
            self.sampled_radius_m = Some(radius);
            s
        };
        self.clearance_sample = Some(sample);
        self.base_source = match self.surface {
            SurfaceSource::Colliders(_) => "gpu_collider",
            SurfaceSource::CpuOracle => "complete_terrain",
        };
        Ok(sample.clearance_m)
    }
    /// A terrain sample (GPU collider or CPU oracle) backs the clearance.
    fn terrain_sampled(&self) -> bool {
        matches!(self.base_source, "gpu_collider" | "complete_terrain")
    }
    /// Clearance at `pose` from this camera's surface source; `None` when the
    /// body has no terrain or its surface there is still pending.
    pub(crate) fn query_surface(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
        pose: FramePose,
    ) -> Result<Option<crate::terrain_inspection::TerrainClearance>> {
        let celestial = pair.system().body(body)?;
        if !celestial.has_surface() {
            return Ok(None);
        }
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let p = pair
            .evaluation()
            .convert_position(pose.position(), fixed)?
            .local()
            .metres();
        match &mut self.surface {
            SurfaceSource::CpuOracle => {
                crate::terrain_inspection::clearance_at_body_position(celestial, p, body)
            }
            SurfaceSource::Colliders(view) => Ok(crate::terrain_inspection::clearance_from_query(
                view, celestial, p, body,
            )?
            .ready()),
        }
    }
    /// Evaluate clearance on the CPU test oracle instead of read-back GPU
    /// colliders. For headless tests that run the camera without a renderer.
    pub fn use_cpu_oracle_surface(&mut self) {
        self.surface = SurfaceSource::CpuOracle;
        self.clearance_sample = None;
    }
    pub fn surface_source(&self) -> &SurfaceSource {
        &self.surface
    }
    /// Adopt the latest collider snapshot (no effect on the CPU oracle source).
    pub fn refresh_surface(&mut self, view: crate::planet_lod::collision::ColliderView) {
        if let SurfaceSource::Colliders(current) = &mut self.surface {
            current.refresh(view);
        }
    }
    /// Query misses since the last call, to schedule their collider pages.
    pub fn take_surface_misses(&mut self) -> Vec<(BodyId, DVec3)> {
        match &mut self.surface {
            SurfaceSource::Colliders(view) => view.take_misses(),
            SurfaceSource::CpuOracle => Vec::new(),
        }
    }
    /// The transported inspection basis also drives the local debug axes.
    pub(crate) fn inspection_tangent(&self) -> Option<astrum_math::surface::SurfaceTangentBasis> {
        self.inspection.map(|anchor| anchor.tangent)
    }
    /// Explicit co-rotating attachment; orientation/position are re-expressed first.
    /// Zero relative simulation derivative is a policy change, not velocity preservation.
    pub fn enter_surface_inspection(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<()> {
        let frame = pair.projection().frames_for(body)?.body_fixed;
        let pose = pair.evaluation().reexpress_pose(self.pose, frame)?;
        let radius = pair.system().body(body)?.properties().reference_radius_m();
        let anchor = crate::surface_anchor::SurfaceInspectionAnchor::new(
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
        self.initialize_surface_angles(anchor.tangent.up().unit());
        self.transition = None;
        self.wheel_pending = DVec3::ZERO;
        self.flight_log_scale = None;
        self.response_clearance_m = self.sample_clearance(pair, body, self.pose)?;
        Ok(())
    }
    /// Set a fixture-controlled observer while retaining the normal surface
    /// inspection controller and the same published body-fixed frame.
    pub(crate) fn developer_set_surface_pose(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
        pose: FramePose,
    ) -> Result<()> {
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let mut pose = pair.evaluation().reexpress_pose(pose, fixed)?;
        let mut position = pose.position().local().metres();
        let radius = pair.system().body(body)?.properties().reference_radius_m();
        ensure!(
            position.is_finite() && position.length_squared() > 0.0,
            "fixture observer position must be finite and nonzero"
        );
        let mut clearance = self.sample_clearance(pair, body, pose)?;
        ensure!(
            clearance.is_finite(),
            "fixture observer clearance is nonfinite"
        );
        // A pending surface cannot validate the pose yet: place it as authored
        // and check it once the collider page arrives (`update_navigation`).
        let pending = self.base_source == "terrain_pending";
        self.pending_surface_check = pending.then_some(body);
        if pending {
            clearance = clearance.max(1.0);
        } else if clearance < 10.0 {
            position = position.normalize() * (position.length() + 10.0 - clearance);
            pose = FramePose::new(
                FramePosition::new(fixed, LocalPosition::try_metres(position)?),
                pose.orientation(),
            );
            clearance = self.sample_clearance(pair, body, pose)?;
        }
        ensure!(
            clearance > 0.0,
            "fixture observer must clear the published complete surface; clearance {clearance} m"
        );
        let anchor = crate::surface_anchor::SurfaceInspectionAnchor::new(
            body,
            position,
            radius,
            self.inspection.map(|previous| previous.tangent),
        )?;
        self.pose = pose;
        self.velocity = FrameVelocity::new(fixed, LinearVelocity3::zero());
        self.attachment = CameraAttachment::BodyFixed(body);
        self.mode = CameraMode::SurfaceInspection;
        self.anchor = LocalPosition::origin();
        self.radius_m = radius;
        self.distance_m = position.length();
        self.min_distance_m = radius + minimum_clearance(radius)?;
        self.response_clearance_m = clearance;
        self.zoom_target_log = self.response_clearance_m.max(1.0).ln();
        self.inspection = Some(anchor);
        self.transition = None;
        self.wheel_pending = DVec3::ZERO;
        self.terrain_approach = true;
        self.initialize_surface_angles(anchor.tangent.up().unit());
        #[cfg(feature = "developer-tools")]
        {
            self.developer_fixture_pose = Some(pose);
        }
        Ok(())
    }
    pub fn look_surface_horizon(&mut self) -> Result<()> {
        ensure!(
            self.mode == CameraMode::SurfaceInspection,
            "enter surface inspection first"
        );
        let anchor = self.inspection.expect("inspection owns anchor");
        self.pose = FramePose::new(self.pose.position(), anchor.body_from_regional.rotation());
        self.surface_heading = anchor.tangent.north().unit();
        self.surface_pitch = 0.0;
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
        if let Some(anchor) = self.inspection {
            self.initialize_surface_angles(anchor.tangent.up().unit());
        }
        Ok(())
    }
    fn initialize_surface_angles(&mut self, up: DVec3) {
        let q = self.pose.orientation().quaternion();
        let forward = q * -DVec3::Z;
        let projected = forward - up * forward.dot(up);
        self.surface_pitch = forward.dot(up).atan2(projected.length());
        self.surface_heading = if projected.length() > 1e-6 {
            projected.normalize()
        } else {
            up.cross(q * DVec3::X).try_normalize().unwrap_or(DVec3::X)
        };
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
            self.inspection = Some(crate::surface_anchor::SurfaceInspectionAnchor::new(
                body,
                guarded,
                pair.system().body(body)?.properties().reference_radius_m(),
                Some(anchor.tangent),
            )?);
        } else if self.mode == CameraMode::BodyOrbit && self.focused_body() == Some(body) {
            let surface_offset = self
                .surface_offset_at_orbit_direction(pair, body)?
                .unwrap_or(0.0);
            let clearance = minimum_radius - self.radius_m - surface_offset;
            self.distance_m = minimum_radius;
            self.zoom_target_log = clearance.max(1.0).ln();
            self.terrain_approach = pair.system().body(body)?.has_surface();
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
            let radius = match self.query_surface(pair, body, self.pose)? {
                Some(c) => c.surface_radius_m,
                // Pending surface: keep the observer where it is and apply the
                // target once the collider page arrives (`update_navigation`).
                None if pair.system().body(body)?.has_surface() => {
                    self.pending_target_clearance = Some(clearance);
                    return Ok(());
                }
                None => pair.system().body(body)?.properties().reference_radius_m(),
            };
            let position = direction.unit() * (radius + clearance);
            self.pose = FramePose::new(
                FramePosition::new(fixed, LocalPosition::try_metres(position)?),
                pose.orientation(),
            );
            self.inspection = Some(crate::surface_anchor::SurfaceInspectionAnchor::new(
                body,
                position,
                pair.system().body(body)?.properties().reference_radius_m(),
                self.inspection.map(|a| a.tangent),
            )?);
            // The radial target changes the observer even when navigation is idle.
            // Refresh the recorded sample used by UI and developer snapshots.
            self.sample_clearance(pair, body, self.pose)?;
            return Ok(());
        }
        ensure!(
            self.mode == CameraMode::BodyOrbit && !self.transitioning(),
            "complete body orbit focus before approach"
        );
        self.refresh_navigation_constraint(pair)?;
        let body = self.focused_body().expect("body orbit has focus");
        let surface_offset = self.surface_offset_at_orbit_direction(pair, body)?;
        // Approach is a property of the body; its sample may still be pending.
        self.terrain_approach = pair.system().body(body)?.has_surface();
        let surface_offset = surface_offset.unwrap_or(0.0);
        ensure!(
            clearance.is_finite()
                && clearance >= 1.0
                && clearance + self.radius_m + surface_offset <= 1e15,
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
        if let Some(t) = self
            .transition
            .take()
            .filter(|t| t.elapsed != Duration::ZERO)
        {
            // Rebuild the pivot from the displayed pose; never replay a cancelled
            // endpoint when the next idle update arrives.
            self.anchor = t.target_anchor;
            self.radius_m = t.target_radius;
            let p = self.pose.position().local().metres() - self.anchor.metres();
            self.distance_m = p.length();
            if let Some(direction) = p.try_normalize()
                && let Ok(basis) = UnitRotation::try_from_quaternion(
                    DQuat::from_rotation_arc(DVec3::Z, direction).normalize(),
                )
            {
                self.orbit_basis = basis;
                self.orbit_look_offset = basis.inverse().compose(self.pose.orientation());
                self.yaw = 0.0;
                self.pitch = 0.0;
                self.mode = if self.attachment == CameraAttachment::System {
                    CameraMode::SystemOrbit
                } else {
                    CameraMode::BodyOrbit
                };
                self.inspection = None;
                self.terrain_approach = false;
            }
        }
        self.wheel_pending = DVec3::ZERO;
        // Freeze the currently displayed quantity. A terrain orbit can be below
        // its reference sphere; sphere altitude is not its cancelled zoom target.
        let clearance = self
            .clearance_sample
            .filter(|s| self.terrain_approach && Some(s.body) == self.focused_body())
            .map_or(self.distance_m - self.radius_m, |s| s.clearance_m);
        self.zoom_target_log = clearance.max(1.0).ln();
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
        self.wheel_pending = DVec3::ZERO;
        self.carrier_center = match role {
            CameraAttachment::Translating(id) => {
                Some(pair.system().body(id)?.state().center_in_system().metres())
            }
            _ => None,
        };
        Ok(())
    }
    /// Leave flight without snapping to a radial look direction. A pivot orbit
    /// transports the incoming look offset; Frame Selected deliberately travels.
    pub fn enter_body_orbit(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<()> {
        let role = CameraAttachment::Translating(body);
        let frame = role.frame(pair.projection())?;
        let pose = pair.evaluation().reexpress_pose(self.pose, frame)?;
        let p = pose.position().local().metres();
        let basis = UnitRotation::try_from_quaternion(DQuat::from_rotation_arc(
            DVec3::Z,
            Direction3::try_new(p)?.unit(),
        ))?;
        self.pose = pose;
        self.attachment = role;
        self.mode = CameraMode::BodyOrbit;
        self.transition = None;
        self.inspection = None;
        self.anchor = LocalPosition::origin();
        self.orbit_basis = basis;
        self.orbit_look_offset = basis.inverse().compose(pose.orientation());
        self.yaw = 0.0;
        self.pitch = 0.0;
        self.distance_m = p.length();
        self.radius_m = pair.system().body(body)?.properties().reference_radius_m();
        self.response_clearance_m = self.sample_clearance(pair, body, pose)?;
        self.terrain_approach = pair.system().body(body)?.has_surface();
        self.zoom_target_log = self.response_clearance_m.max(1.0).ln();
        self.wheel_pending = DVec3::ZERO;
        self.velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
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
                && (1e-3..=1e3).contains(&input.speed_multiplier)
                && input.boost_multiplier.is_finite()
                && (1.0..=4.0).contains(&input.boost_multiplier),
            "invalid navigation input"
        );
        if let Some(body) = self.pending_surface_check {
            // A fixture pose placed before its surface was read back: once the
            // surface resolves, lift an observer that is under or within 1 m of
            // the ground to 10 m, as an immediate placement would have.
            if let Some(surface) = self.query_surface(pair, body, self.pose)? {
                self.pending_surface_check = None;
                if surface.clearance_m < 1.0 {
                    self.enforce_radial_clearance(pair, body, surface.surface_radius_m, 10.0)?;
                    #[cfg(feature = "developer-tools")]
                    if self.developer_fixture_pose.is_some() {
                        self.developer_fixture_pose = Some(self.pose);
                    }
                }
                self.response_clearance_m = self.sample_clearance(pair, body, self.pose)?;
            }
        }
        if let Some(clearance) = self.pending_target_clearance {
            // Dropped if the observer left surface inspection meanwhile.
            match (self.mode, self.focused_body()) {
                (CameraMode::SurfaceInspection, Some(body)) if !self.transitioning() => {
                    if self.query_surface(pair, body, self.pose)?.is_some() {
                        self.pending_target_clearance = None;
                        self.target_clearance(pair, clearance)?;
                        #[cfg(feature = "developer-tools")]
                        if self.developer_fixture_pose.is_some() {
                            self.developer_fixture_pose = Some(self.pose);
                        }
                    }
                }
                _ => self.pending_target_clearance = None,
            }
        }
        #[cfg(feature = "developer-tools")]
        if self.developer_fixture_pose.is_some() {
            let active = input.drag != [0.0; 2]
                || input.scroll_notches != 0.0
                || input.translation != DVec3::ZERO;
            // The fixture only holds the idle surface inspection pose it placed.
            // A pending transition or another mode (overview, body orbit, flight)
            // is an explicit command that the frozen pose must not swallow.
            let fixture_state =
                self.mode == CameraMode::SurfaceInspection && self.transition.is_none();
            if !active && fixture_state {
                return Ok(());
            }
            // Explicit navigation or a camera command returns control to the
            // ordinary camera path.
            self.developer_fixture_pose = None;
        }
        let mut candidate = self.clone();
        if input.scroll_notches != 0.0 {
            candidate.last_wheel_notches = Some(input.scroll_notches);
        }
        candidate.user_multiplier = input.speed_multiplier;
        candidate.boost_multiplier = input.boost_multiplier;
        candidate.navigation_checked(pair, input, elapsed)?;
        if let Some(minimum) = candidate.terrain_clearance_guard_m {
            candidate.apply_terrain_guard(pair, minimum)?;
        }
        if let Some(body) = candidate.focused_body() {
            candidate.response_clearance_m =
                candidate.sample_clearance(pair, body, candidate.pose)?;
        } else if candidate.mode == CameraMode::SystemOrbit {
            candidate.clearance_sample = None;
            candidate.sampled_definition = None;
            candidate.sampled_radius_m = None;
            candidate.base_source = "overview_distance";
        }
        if matches!(
            candidate.mode,
            CameraMode::SystemOrbit | CameraMode::BodyOrbit
        ) {
            candidate.base_speed_m_s =
                (0.5 * candidate.response_clearance_m.max(1.0)).clamp(1.0, 1e12);
            candidate.flight_speed_m_s =
                candidate.base_speed_m_s * candidate.user_multiplier * candidate.boost_multiplier;
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
            // Interrupt at the displayed pose. Retain surface policy when already
            // in surface navigation; otherwise acquire the requested pivot orbit.
            let role = self.transition.as_ref().map(|t| t.target_role);
            match role {
                Some(CameraAttachment::Translating(id) | CameraAttachment::BodyFixed(id)) => {
                    if self.mode == CameraMode::SurfaceInspection {
                        self.enter_surface_inspection(pair, id)?;
                    } else {
                        self.enter_body_orbit(pair, id)?;
                    }
                }
                _ => {
                    let root = pair.evaluation().root();
                    self.pose = pair.evaluation().reexpress_pose(self.pose, root)?;
                    self.attachment = CameraAttachment::System;
                    self.mode = CameraMode::SystemOrbit;
                    self.transition = None;
                    let p = self.pose.position().local().metres() - self.anchor.metres();
                    self.distance_m = p.length();
                    self.radius_m = 0.0;
                    self.orbit_basis = UnitRotation::try_from_quaternion(
                        DQuat::from_rotation_arc(DVec3::Z, Direction3::try_new(p)?.unit()),
                    )?;
                    self.orbit_look_offset =
                        self.orbit_basis.inverse().compose(self.pose.orientation());
                    self.yaw = 0.0;
                    self.pitch = 0.0;
                    self.zoom_target_log = self.distance_m.ln();
                }
            }
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
                        // Complete terrain can legitimately lie below its
                        // reference sphere. Recovery must permit outward escape
                        // from that incoming pose, without moving further inward.
                        closest.length() + 1e-6 >= radius.min(a.length()),
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
                self.orbit_look_offset = UnitRotation::identity();
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
        if let Some(body) = self.focused_body() {
            self.response_clearance_m = self.sample_clearance(pair, body, self.pose)?;
            if !self.terrain_approach && pair.system().body(body)?.has_surface() {
                self.terrain_approach = true;
                self.zoom_target_log = self.response_clearance_m.max(1.0).ln();
            }
        } else {
            self.response_clearance_m = self.distance_m;
        }
        let response = self.orbit_response();
        self.yaw = (self.yaw - input.drag[0] * response).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - input.drag[1] * response).clamp(-1.5, 1.5);
        if self.terrain_approach {
            let id = self.focused_body().expect("terrain approach has a body");
            let effective_radius = self.radius_m
                + self
                    .surface_offset_at_orbit_direction(pair, id)?
                    .unwrap_or(0.0);
            let target = self.zoom_target_log - input.scroll_notches * self.wheel_response();
            ensure!(
                target.is_finite() && target <= (1e15 - effective_radius).ln(),
                "1e15 m navigation zoom limit"
            );
            self.zoom_target_log = target.max(1.0_f64.ln());
            // Pivot rotation changes sampled elevation. Carry the incoming
            // complete clearance to the new radial direction before smoothing;
            // do not reinterpret that rotation as wheel approach/recede.
            let clearance = self.response_clearance_m.max(1.0);
            let next = self.zoom_target_log
                + (clearance.ln() - self.zoom_target_log) * (-elapsed.as_secs_f64() / 0.08).exp();
            self.distance_m = effective_radius + next.exp();
            return self.update_pose(self.attachment.frame(pair.projection())?);
        }
        let minimum = (self.min_distance_m - self.radius_m).max(0.1);
        let target = self.zoom_target_log - input.scroll_notches * self.wheel_response();
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
        if let Some((id, _, _)) = nearest {
            scale = self.sample_clearance(pair, id, self.pose)?;
        } else {
            self.base_source = "overview_distance";
            self.clearance_sample = None;
            self.sampled_definition = None;
            self.sampled_radius_m = None;
        }
        self.response_clearance_m = scale;
        let response = self.local_response();
        let look = UnitRotation::try_from_quaternion(
            DQuat::from_rotation_y(-input.drag[0] * response)
                * DQuat::from_rotation_x(-input.drag[1] * response),
        )?;
        self.pose = FramePose::new(self.pose.position(), self.pose.orientation().compose(look));
        self.queue_flight_wheel(pair, nearest.map(|n| n.0), input.scroll_notches)?;
        self.integrate_flight(pair, nearest.map(|n| n.0), input, elapsed, false)?;
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
                // Legacy terrain is constrained to a 10% radial envelope.
                // Compositional shapes have their own validated bound.
                let outside_surface = if let Some(definition) = body.surface_definition() {
                    let outer = astrum_world::terrain::SurfaceGenerator::new(definition, radius)?
                        .conservative_radius_envelope_m()[1];
                    p.length() > (outer + 1.0).next_up()
                } else {
                    p.length() > radius * 1.1
                };
                if outside_surface {
                    continue;
                }
                let minimum = if body.has_surface() {
                    let c = self.sample_clearance(pair, id, self.pose)?;
                    p.length() - c + 1.0
                } else {
                    radius + minimum_clearance(radius)?
                };
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
        if let Some((id, _, _)) = nearest {
            self.response_clearance_m = self.sample_clearance(pair, id, self.pose)?;
        }
        Ok(())
    }
    fn inspect_motion(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        input: &NavigationInput,
        elapsed: Duration,
    ) -> Result<()> {
        let anchor = self
            .inspection
            .ok_or_else(|| anyhow::anyhow!("surface anchor missing"))?;
        self.response_clearance_m = self.sample_clearance(pair, anchor.body, self.pose)?;
        let response = self.local_response();
        let up = anchor.tangent.up().unit();
        let orientation = self.pose.orientation().quaternion();
        let yaw = DQuat::from_axis_angle(up, -input.drag[0] * response);
        self.surface_heading = (yaw * self.surface_heading).normalize();
        let right = self.surface_heading.cross(up).normalize();
        let next_pitch = if input.drag[1] == 0.0 {
            self.surface_pitch
        } else {
            (self.surface_pitch - input.drag[1] * response).clamp(
                -std::f64::consts::FRAC_PI_2 + 1e-4,
                std::f64::consts::FRAC_PI_2 - 1e-4,
            )
        };
        let pitch = DQuat::from_axis_angle(right, next_pitch - self.surface_pitch);
        self.surface_pitch = next_pitch;
        self.pose = FramePose::new(
            self.pose.position(),
            UnitRotation::try_from_quaternion(pitch * yaw * orientation)?,
        );
        self.queue_flight_wheel(pair, Some(anchor.body), input.scroll_notches)?;
        self.integrate_flight(pair, Some(anchor.body), input, elapsed, true)
    }
    fn queue_flight_wheel(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: Option<BodyId>,
        notches: f64,
    ) -> Result<()> {
        if notches == 0.0 {
            return Ok(());
        }
        let destination = FramePose::new(
            self.pose.position().displaced(FrameDisplacement::new(
                self.pose.position().frame(),
                Displacement3::try_metres(self.wheel_pending)?,
            ))?,
            self.pose.orientation(),
        );
        let scale = if let Some(body) = body {
            self.sample_clearance(pair, body, destination)?
        } else {
            self.response_clearance_m
        };
        let exponent = -notches * self.wheel_response();
        ensure!(
            exponent.is_finite() && exponent <= 700.0,
            "flight wheel overflow"
        );
        let distance = scale.max(1.0) * -exponent.exp_m1();
        let pending =
            self.wheel_pending + self.pose.orientation().quaternion() * -DVec3::Z * distance;
        ensure!(
            pending.is_finite() && pending.length() <= 1e15,
            "flight navigation limit"
        );
        self.wheel_pending = pending;
        Ok(())
    }
    fn integrate_flight(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: Option<BodyId>,
        input: &NavigationInput,
        elapsed: Duration,
        surface: bool,
    ) -> Result<()> {
        // Small bounded wall steps constrain scale/basis integration error, never
        // partition or multiply the raw event deltas. Zero time still updates targets.
        let mut remaining = elapsed.as_secs_f64();
        ensure!(remaining <= 60.0, "navigation interval too large");
        loop {
            let dt = remaining.min(1.0 / 240.0);
            let clearance = if let Some(body) = body {
                self.sample_clearance(pair, body, self.pose)?
            } else {
                self.response_clearance_m
            };
            self.response_clearance_m = clearance;
            let target_log = (0.5 * clearance.max(1.0)).clamp(1.0, 1e12).ln();
            let previous = self.flight_log_scale.unwrap_or(target_log);
            let blended = target_log + (previous - target_log) * (-dt / 0.15).exp();
            self.flight_log_scale = Some(blended);
            self.base_speed_m_s = blended.exp();
            self.flight_speed_m_s =
                self.base_speed_m_s * input.speed_multiplier * input.boost_multiplier;
            let q = self.pose.orientation().quaternion();
            let movement = if surface {
                let anchor = self
                    .inspection
                    .ok_or_else(|| anyhow::anyhow!("surface anchor missing"))?;
                let up = anchor.tangent.up().unit();
                let tangent_forward = self.surface_heading;
                let right = tangent_forward.cross(up).normalize();
                right * input.translation.x + up * input.translation.y
                    - tangent_forward * input.translation.z
            } else {
                q * input.translation
            };
            let movement = movement.try_normalize().unwrap_or(DVec3::ZERO);
            let wheel = self.wheel_pending * -(-dt / 0.08).exp_m1();
            self.wheel_pending -= wheel;
            let p = self.pose.position().local().metres()
                + movement * self.flight_speed_m_s * dt
                + wheel;
            let frame = self.pose.position().frame();
            self.pose = FramePose::new(
                FramePosition::new(frame, LocalPosition::try_metres(p)?),
                self.pose.orientation(),
            );
            if surface {
                let id = body.ok_or_else(|| anyhow::anyhow!("surface body missing"))?;
                let minimum = self
                    .terrain_clearance_guard_m
                    .or(self.navigation_envelope.then_some(1.0));
                if let Some(minimum) = minimum {
                    let c = self.sample_clearance(pair, id, self.pose)?;
                    if c < minimum {
                        let guarded = Direction3::try_new(p)?.unit() * (p.length() + minimum - c);
                        self.pose = FramePose::new(
                            FramePosition::new(frame, LocalPosition::try_metres(guarded)?),
                            self.pose.orientation(),
                        );
                        self.wheel_pending = DVec3::ZERO;
                    }
                }
                let anchor = self
                    .inspection
                    .ok_or_else(|| anyhow::anyhow!("surface anchor missing"))?;
                let p = self.pose.position().local().metres();
                let radius = pair.system().body(id)?.properties().reference_radius_m();
                let next = crate::surface_anchor::SurfaceInspectionAnchor::new(
                    id,
                    p,
                    radius,
                    Some(anchor.tangent),
                )?;
                let transport =
                    DQuat::from_rotation_arc(anchor.tangent.up().unit(), next.tangent.up().unit());
                let transported = (transport * self.pose.orientation().quaternion()).normalize();
                self.surface_heading = (transport * self.surface_heading).normalize();
                let up = next.tangent.up().unit();
                let forward =
                    self.surface_heading * self.surface_pitch.cos() + up * self.surface_pitch.sin();
                let right = self.surface_heading.cross(up).normalize();
                let stable = DQuat::from_mat3(&glam::DMat3::from_cols(
                    right,
                    right.cross(forward),
                    -forward,
                ))
                .normalize();
                let orientation = transported.slerp(stable, -(-dt / 0.35).exp_m1());
                self.pose = FramePose::new(
                    self.pose.position(),
                    UnitRotation::try_from_quaternion(orientation.normalize())?,
                );
                self.inspection = Some(next);
                self.velocity = FrameVelocity::new(frame, LinearVelocity3::zero());
            }
            if dt == 0.0 || remaining <= dt {
                break;
            }
            remaining -= dt;
        }
        if let Some(body) = body {
            self.response_clearance_m = self.sample_clearance(pair, body, self.pose)?;
        }
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
        if let Some(diagnostic) = self.query_surface(pair, id, self.pose)?
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
        if self.terrain_approach && pair.system().body(id)?.has_surface() {
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
            rotation.compose(self.orbit_look_offset),
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
        self.orbit_look_offset = UnitRotation::identity();
        self.distance_m = distance;
        self.min_distance_m = radius + minimum_clearance(radius)?;
        self.radius_m = radius;
        self.mode = CameraMode::BodyOrbit;
        self.terrain_approach = false;
        self.transition = None;
        self.zoom_target_log = (distance - radius).ln();
        self.response_clearance_m = distance - radius;
        self.update_pose(frame)
    }
    /// Reference-sphere fixture helper. Native controls use `update_navigation`
    /// with a coherent pair, so complete-terrain queries can inform the response.
    pub fn orbit_zoom(&mut self, drag: [f64; 2], wheel: f64) -> Result<()> {
        ensure!(
            drag.iter().all(|x| x.is_finite()) && wheel.is_finite(),
            "invalid camera input"
        );
        let distance = self.radius_m
            + ((self.distance_m - self.radius_m) * (-wheel * self.wheel_response()).exp())
                .max(self.min_distance_m - self.radius_m);
        ensure!(distance.is_finite(), "camera zoom overflow");
        self.yaw = (self.yaw - drag[0] * self.orbit_response()).rem_euclid(std::f64::consts::TAU);
        self.pitch = (self.pitch - drag[1] * self.orbit_response()).clamp(-1.5, 1.5);
        self.distance_m = distance;
        self.response_clearance_m = (distance - self.radius_m).max(1.0);
        self.zoom_target_log = (distance - self.radius_m).ln();
        self.update_pose(self.pose.position().frame())
    }
    fn surface_offset_at_orbit_direction(
        &mut self,
        pair: &CoherentCelestialView<'_>,
        body: BodyId,
    ) -> Result<Option<f64>> {
        if !pair.system().body(body)?.has_surface() {
            return Ok(None);
        }
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
        let pose = FramePose::new(
            FramePosition::new(fixed, LocalPosition::try_metres(direction * radius)?),
            UnitRotation::identity(),
        );
        self.sample_clearance(pair, body, pose)?;
        Ok(self.clearance_sample.map(|s| s.surface_radius_m - radius))
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
    fn rolled_surface_entry_stabilizes_smoothly_and_polar_transport_stays_orthogonal() {
        let world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(5129).unwrap())
            .unwrap();
        let projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(5129).unwrap()).unwrap();
        let pair = projection.coherent_view(&world).unwrap();
        let body = world.bodies().nth(1).unwrap().0;
        let radius = world.body(body).unwrap().properties().reference_radius_m();
        let fixed = projection.frames_for(body).unwrap().body_fixed;
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1e11).unwrap();
        camera.focus(&pair, body, true, true).unwrap();
        // Static orientation/position seed only, not ordinary approach evidence.
        let p = DVec3::new(100.0, radius + 100_000.0, 0.0);
        let anchor =
            crate::surface_anchor::SurfaceInspectionAnchor::new(body, p, radius, None).unwrap();
        let horizon = anchor.body_from_regional.rotation().quaternion();
        camera.pose = FramePose::new(
            FramePosition::new(fixed, LocalPosition::try_metres(p).unwrap()),
            UnitRotation::try_from_quaternion(horizon * DQuat::from_rotation_z(0.7)).unwrap(),
        );
        let incoming = camera.pose;
        camera.enter_surface_inspection(&pair, body).unwrap();
        assert_eq!(incoming, camera.pose);
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::ZERO)
            .unwrap();
        let zero_angle = incoming
            .orientation()
            .quaternion()
            .angle_between(camera.pose.orientation().quaternion());
        assert!(zero_angle <= 1e-6);
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::from_millis(1))
            .unwrap();
        let first = incoming
            .orientation()
            .quaternion()
            .angle_between(camera.pose.orientation().quaternion());
        assert!(first > 0.0 && first < 0.01);
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::from_secs(3))
            .unwrap();
        let remaining_roll = (camera.pose.orientation().quaternion() * DVec3::X)
            .dot(camera.inspection.unwrap().tangent.up().unit())
            .abs();
        assert!(remaining_roll < 0.001);
        camera.look_surface_horizon().unwrap();
        let before = camera.pose.position().local().metres();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    translation: -DVec3::Z,
                    ..Default::default()
                },
                Duration::from_secs(1),
            )
            .unwrap();
        let tangent = camera.inspection.unwrap().tangent;
        let orthogonal = tangent.east().unit().dot(tangent.up().unit()).abs();
        let roll = (camera.pose.orientation().quaternion() * DVec3::X)
            .dot(tangent.up().unit())
            .abs();
        assert!(orthogonal <= 1e-12 && roll <= 1e-6);
        assert!((camera.pose.position().local().metres() - before).length() > 1000.0);
        let pose = camera.pose;
        let rebuilt =
            CelestialFrameProjection::build(&world, NonZeroU64::new(5130).unwrap()).unwrap();
        camera.remap_projection(&rebuilt).unwrap();
        assert_eq!(pose.position().local(), camera.pose.position().local());
        assert_eq!(pose.orientation(), camera.pose.orientation());
        println!(
            "rolled_entry: zero_angle_rad={zero_angle:e} first_1ms_angle_rad={first:e} residual_roll={remaining_roll:e}; polar_transport: orthogonality={orthogonal:e} unintended_roll={roll:e}; remap position/orientation residual=0"
        );
    }
    #[test]
    fn interrupted_transition_and_cancelled_targets_do_not_replay_or_enter_advanced_flight() {
        let world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(5129).unwrap())
            .unwrap();
        let projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(5129).unwrap()).unwrap();
        let pair = projection.coherent_view(&world).unwrap();
        let body = world.bodies().nth(1).unwrap().0;
        let mut camera = CelestialCamera::overview(&pair, DVec3::ZERO, 1e11).unwrap();
        camera
            .transition_to(&pair, FocusTarget::Body(body))
            .unwrap();
        camera
            .update_navigation(
                &pair,
                &NavigationInput::default(),
                Duration::from_millis(100),
            )
            .unwrap();
        let before = camera.pose;
        camera.enter_surface_inspection(&pair, body).unwrap();
        let returned = pair
            .evaluation()
            .reexpress_pose(camera.pose, before.position().frame())
            .unwrap();
        let residual =
            (returned.position().local().metres() - before.position().local().metres()).length();
        assert!(residual < 1e-3);
        camera
            .transition_to(&pair, FocusTarget::Body(body))
            .unwrap();
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    drag: [0.01, 0.0],
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        assert_eq!(camera.mode, CameraMode::SurfaceInspection);
        assert!(!camera.transitioning());
        camera
            .update_navigation(
                &pair,
                &NavigationInput {
                    scroll_notches: 0.5,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
        camera.cancel_transition();
        let before = camera.pose.position().local();
        camera
            .update_navigation(&pair, &NavigationInput::default(), Duration::from_secs(1))
            .unwrap();
        assert_eq!(camera.pose.position().local(), before);
        println!(
            "interrupt: common_frame_position_residual_m={residual:e}; surface policy retained; cancelled wheel displacement replay=0"
        );
    }
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
                    // Two tangent axes, not the superseded camera-space up axis.
                    translation: DVec3::new(1.0, 0.0, -1.0),
                    ..Default::default()
                },
                Duration::from_secs(1),
            )
            .unwrap();
        let tangent = camera.inspection_tangent().unwrap();
        let static_basis = astrum_math::surface::SurfaceTangentBasis::new(tangent.up());
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
        // Normalized notches now reach the 1 m reference safeguard; the old
        // raw-pixel fixture happened to stop outside 1.05 radii.
        assert!(camera.distance_m() >= 6.371e6 + 1.0);
        let before = camera.pose;
        let rebuilt = CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
        camera.remap_projection(&rebuilt).unwrap();
        assert_ne!(camera.pose.position().frame(), before.position().frame());
        assert_eq!(camera.pose.position().local(), before.position().local());
        assert_eq!(world.revision(), revision);
    }
}
