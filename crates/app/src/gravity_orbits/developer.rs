//! Checked development operations over the production session.
use super::*;
use crate::developer_protocol::{BodyInventory, DevCommand};
use anyhow::{bail, ensure};
use sha2::{Digest, Sha256};
fn body_handle(session: &str, namespace: u64, index: usize) -> String {
    format!(
        "body-{:x}",
        Sha256::digest(format!("{session}/{namespace}/{index}"))
    )
}

impl GravityOrbitsDemo {
    pub fn developer_set_session(&mut self, session: &str) {
        self.developer_session = Some(session.into());
    }
    pub fn developer_last_snapshot(&self) -> Option<&DeveloperSnapshot> {
        self.developer_snapshot.as_ref()
    }
    /// Atlas arrival fade duration from the content LOD policy.
    pub(crate) fn developer_morph_duration_ms(&self) -> u64 {
        self.atlas
            .policy()
            .map_or(0, |policy| (policy.arrival_seconds * 1000.0).round() as u64)
    }
    pub fn developer_set_automation(&mut self, owner: Option<&str>) {
        self.controls.automation_owner = owner.map(str::to_owned);
    }
    pub fn developer_take_stop(&mut self) -> bool {
        std::mem::take(&mut self.controls.automation_stop)
    }
    pub fn developer_cancel(&mut self) -> Result<()> {
        self.developer_navigation = None;
        self.command(Command::CancelNavigation)
    }
    pub fn developer_inventory(&self, session_id: &str) -> Result<Vec<BodyInventory>> {
        self.system
            .bodies()
            .enumerate()
            .map(|(index, (_, body))| {
                Ok(BodyInventory {
                    handle: body_handle(session_id, self.system_namespace, index),
                    semantic_identity: Some(self.presentation[index].semantic_id.clone()),
                    name: body.name().into(),
                    mass_kg: body.properties().mass_kg(),
                    reference_radius_m: body.properties().reference_radius_m(),
                    position_m: body.state().center_in_system().metres().to_array(),
                    velocity_m_s: body
                        .state()
                        .center_velocity_in_system()
                        .metres_per_second()
                        .to_array(),
                    surface_available: body.has_surface(),
                    state_reference_frame: "system_inertial".into(),
                    supported_operations: vec!["select".into(), "focus".into(), "look_at".into()],
                })
            })
            .collect()
    }
    fn developer_body(&self, handle: &str) -> Result<BodyId> {
        let session = self
            .developer_session
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("session_identity_unavailable"))?;
        self.ids
            .iter()
            .copied()
            .enumerate()
            .find_map(|(index, id)| {
                (body_handle(session, self.system_namespace, index) == handle).then_some(id)
            })
            .ok_or_else(|| anyhow::anyhow!("invalid or stale body handle"))
    }
    pub fn developer_apply_command(&mut self, command: &DevCommand) -> Result<()> {
        match command {
            DevCommand::Profiler {
                enabled,
                freeze,
                export,
            } => {
                self.controls
                    .profiler
                    .configure(
                        self.developer_snapshot.as_ref(),
                        crate::profiler::ProfilerControls {
                            enabled: *enabled,
                            freeze: *freeze,
                            export: *export,
                        },
                    )
                    .map_err(|error| anyhow::anyhow!(error))?;
                Ok(())
            }
            DevCommand::Select { body } => {
                self.command(Command::Select(self.developer_body(body)?))
            }
            DevCommand::Focus { body, body_fixed } => {
                let body = self.developer_body(body)?;
                self.command(Command::Select(body))?;
                self.command(Command::Focus {
                    fixed: *body_fixed,
                    fit: true,
                })
            }
            DevCommand::LookAt { body } => {
                self.command(Command::LookBody(self.developer_body(body)?))
            }
            DevCommand::Overview => self.command(Command::Overview),
            DevCommand::NavigationMode { mode } => self.command(match mode.as_str() {
                "system_orbit" | "overview" => Command::Overview,
                "body_orbit" => Command::BodyOrbit,
                "surface_horizon" => Command::SurfaceHorizon,
                "surface_inspection" => Command::SurfaceInspection,
                "free_flight" => Command::FreeFlight,
                _ => bail!("unsupported navigation mode"),
            }),
            DevCommand::Navigation {
                drag,
                scroll_notches,
                translation,
                speed_multiplier,
                boost_multiplier,
                duration_s,
            } => {
                ensure!(
                    drag.iter()
                        .chain(translation)
                        .chain([
                            scroll_notches,
                            speed_multiplier,
                            boost_multiplier,
                            duration_s
                        ])
                        .all(|v| v.is_finite()),
                    "nonfinite navigation input"
                );
                ensure!(
                    *duration_s >= 0.0
                        && *duration_s <= 30.0
                        && *speed_multiplier > 0.0
                        && *boost_multiplier > 0.0,
                    "invalid navigation duration/speed"
                );
                let input = NavigationInput {
                    drag: *drag,
                    scroll_notches: *scroll_notches,
                    translation: DVec3::from_array(*translation),
                    speed_multiplier: *speed_multiplier,
                    boost_multiplier: *boost_multiplier,
                };
                self.command(Command::Navigation(input))?;
                self.developer_navigation = Some((
                    Duration::from_secs_f64(*duration_s),
                    NavigationInput {
                        drag: [0.0; 2],
                        scroll_notches: 0.0,
                        ..input
                    },
                ));
                Ok(())
            }
            DevCommand::Clearance { meters } => {
                ensure!(meters.is_finite() && *meters > 0.0, "invalid clearance");
                self.command(Command::Clearance(*meters))
            }
            DevCommand::SurfacePose {
                body,
                position_body_m,
                orientation_xyzw,
            } => {
                let body = self.developer_body(body)?;
                let pair = self.projection.coherent_view(&self.system)?;
                let fixed = pair.projection().frames_for(body)?.body_fixed;
                let pose = FramePose::new(
                    FramePosition::new(
                        fixed,
                        LocalPosition::try_metres(DVec3::from_array(*position_body_m))?,
                    ),
                    UnitRotation::try_from_quaternion(glam::DQuat::from_array(*orientation_xyzw))?,
                );
                // Validate on a candidate so failed placement cannot alter the observer.
                let mut camera = self.camera.clone();
                // With the surface already read back, reject poses under the
                // ground; otherwise the pose is placed as authored and lifted to
                // 10 m if the surface turns out to be within 1 m (ADR 0023).
                let clearance = camera
                    .query_surface(&pair, body, pose)?
                    .map(|surface| surface.clearance_m);
                if let Some(clearance) = clearance {
                    ensure!(
                        clearance >= 1.0,
                        "capture pose requires at least 1 m clearance"
                    );
                }
                camera.developer_set_surface_pose(&pair, body, pose)?;
                if let Some(clearance) = clearance {
                    camera.target_clearance(&pair, clearance)?;
                }
                camera.enter_surface_inspection(&pair, body)?;
                self.camera = camera;
                self.developer_navigation = None;
                self.command(Command::Select(body))
            }
            DevCommand::Pause { paused } => self.set_paused(*paused),
            DevCommand::Rate { multiplier } => {
                ensure!(multiplier.is_finite(), "nonfinite rate");
                self.set_playback_rate(*multiplier)
            }
            DevCommand::Seek { seconds } => {
                ensure!(seconds.is_finite(), "nonfinite seek");
                self.seek_seconds(*seconds)
            }
            DevCommand::SingleStep { forward } => self.single_step(*forward),
            DevCommand::Reset => self.reset_motion(),
            DevCommand::RenderMode { mode } => {
                let mode = TerrainViewMode::from_name(mode)
                    .ok_or_else(|| anyhow::anyhow!("unsupported render mode"))?;
                self.command(Command::Visual(visual_controls::VisualCommand::RenderMode(
                    mode,
                )))
            }
            DevCommand::Setting { id, value } => {
                let index = crate::render_settings::index_of(id)
                    .ok_or_else(|| anyhow::anyhow!("unknown setting {id}"))?;
                let value = crate::render_settings::value_from_json(index, value)?;
                self.apply_render_setting(index, value)
            }
            DevCommand::ResetRenderSettings => {
                self.controls.render_settings = Default::default();
                self.controls.terrain_view = TerrainViewMode::Lit;
                Ok(())
            }
            DevCommand::Layer { layer, enabled } => {
                self.command(Command::Visual(visual_controls::VisualCommand::Layer(
                    visual_controls::Layer::named(layer)?,
                    *enabled,
                )))
            }
            DevCommand::ResidentCoverHold { enabled } => {
                // Freeze atlas request generation; drawing continues.
                self.atlas.hold = *enabled;
                Ok(())
            }
        }
    }
}
