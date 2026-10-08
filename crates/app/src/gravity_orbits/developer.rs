//! Checked development operations over the production session.
use super::*;
use crate::developer_protocol::{BodyInventory, DevCommand};
use anyhow::{Context, bail, ensure};
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
    pub fn developer_accuracy(&self, renderer: &Renderer) -> Result<serde_json::Value> {
        let snapshot = self
            .developer_snapshot
            .as_ref()
            .context("no submitted snapshot")?;
        let body = self.planetary.body.context("no resident body")?;
        let authority = self.system.body(body)?;
        let definition = authority
            .surface_definition()
            .context("no surface authority")?;
        let generator = mundaris_world::terrain::SurfaceGenerator::new(
            definition,
            authority.properties().reference_radius_m(),
        )?;
        let (draw, active, fallback) = renderer
            .last_submitted_regional_draw()
            .context("no current submitted retained draw")?;
        let projection = self.content_projection(snapshot.camera.near_plane_m)?;
        ensure!(
            projection.viewport() == snapshot.camera.viewport_size_pixels
                && projection.origin() == snapshot.camera.viewport_origin_pixels,
            "accuracy viewport differs from submitted snapshot"
        );
        let mut result = crate::terrain_accuracy::sample_visible_terrain_accuracy(
            draw, active, fallback, projection, generator, 1.0,
        );
        result["submission_id"] = serde_json::json!(snapshot.performance.native_submission_id);
        result["frame_number"] = serde_json::json!(snapshot.general.frame_number);
        result["shared_scene"] = serde_json::json!(snapshot.shared_scene);
        result["integration_note"] = serde_json::json!(
            "One accuracy observation; replay postprocessing integrates fixed weighted samples over actual observation times. Its 1-second sample integral is not a full replay integral."
        );
        Ok(result)
    }
    pub(crate) fn developer_morph_duration_ms(&self) -> u64 {
        self.controls.terrain_morph_ms
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
                view,
                frame,
                event,
                export,
                zoom,
                pan,
                fill_window,
            } => {
                self.controls
                    .performance_lab
                    .configure(
                        self.developer_snapshot.as_ref(),
                        crate::performance_lab::ProfilerControls {
                            enabled: *enabled,
                            freeze: *freeze,
                            view: view.as_deref(),
                            frame: *frame,
                            event: *event,
                            export: *export,
                            zoom: *zoom,
                            pan: *pan,
                            fill_window: *fill_window,
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
                let clearance = crate::terrain_inspection::clearance_at_body_position(
                    pair.system().body(body)?,
                    pose.position().local().metres(),
                    body,
                )?
                .context("capture pose requires complete terrain authority")?
                .clearance_m;
                ensure!(
                    clearance >= 1.0,
                    "capture pose requires at least 1 m clearance"
                );
                // The existing fixture helper enforces complete-source clearance.
                // Validate on a candidate so failed placement cannot alter the observer.
                let mut camera = self.camera.clone();
                camera.developer_set_surface_pose(&pair, body, pose)?;
                camera.target_clearance(&pair, clearance)?;
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
                let mode = TerrainRenderMode::ALL
                    .into_iter()
                    .find(|&m| crate::developer_snapshot::render_mode_name(m) == mode)
                    .ok_or_else(|| anyhow::anyhow!("unsupported render mode"))?;
                self.command(Command::Visual(visual_controls::VisualCommand::RenderMode(
                    mode,
                )))
            }
            DevCommand::Layer { layer, enabled } => {
                self.command(Command::Visual(visual_controls::VisualCommand::Layer(
                    visual_controls::Layer::named(layer)?,
                    *enabled,
                )))
            }
            DevCommand::ResidentCoverHold { enabled } => {
                ensure!(
                    !*enabled || self.planetary.runtime.has_coverage(),
                    "hold requires resident coverage"
                );
                self.planetary.hold_cover = *enabled;
                Ok(())
            }
            DevCommand::ClusterRendering {
                mode,
                debug,
                triangle_edges,
                cluster_edges,
                freeze,
            } => {
                use mundaris_renderer::{ClusterDebug, ClusterMode, ClusterSettings};
                let mode = match mode.as_str() {
                    "reference" => ClusterMode::Reference,
                    "culling" => ClusterMode::Culling,
                    "lod" => ClusterMode::Lod,
                    _ => bail!("unsupported cluster mode"),
                };
                let debug = match debug.as_str() {
                    "lit" => ClusterDebug::Lit,
                    "clusters" => ClusterDebug::Clusters,
                    "lod" => ClusterDebug::Lod,
                    "residency" => ClusterDebug::Residency,
                    _ => bail!("unsupported cluster debug mode"),
                };
                ensure!(
                    !*freeze
                        || (mode != ClusterMode::Reference
                            && self.controls.cluster_report.resident_regions > 0),
                    "freeze requires a resident cluster cut"
                );
                self.controls.cluster_settings = ClusterSettings {
                    mode,
                    debug,
                    triangle_edges: *triangle_edges,
                    cluster_edges: *cluster_edges,
                    freeze: *freeze,
                };
                Ok(())
            }
            DevCommand::SkySetting { .. } => bail!("sky presentation is retired"),
            DevCommand::GpuTile { .. }
            | DevCommand::GpuTileView { .. }
            | DevCommand::GpuHierarchy { .. }
            | DevCommand::GpuRegional { .. } => {
                bail!("standalone fixture scenes are retired; use shared-system body navigation")
            }
        }
    }
}
