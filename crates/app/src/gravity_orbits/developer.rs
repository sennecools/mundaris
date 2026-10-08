//! Checked development operations over the production session.
use super::*;
use crate::developer_protocol::{BodyInventory, DevCommand};
use anyhow::{Context, bail, ensure};
use mundaris_world::terrain::{SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed};
use sha2::{Digest, Sha256};
fn body_handle(session: &str, namespace: u64, index: usize) -> String {
    format!(
        "body-{:x}",
        Sha256::digest(format!("{session}/{namespace}/{index}"))
    )
}

fn stable_body_identity(name: &str) -> u64 {
    // Keep the Slice1B2 rocky Moon identifier stable across tile revisions;
    // other authored body names receive deterministic, distinct identities.
    if name == "Moon" {
        return 5_931_033_225_171_238_913;
    }
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn generated_definition_identity(name: &str, revision: u64) -> u64 {
    stable_body_identity(name) ^ revision.rotate_left(13)
}

impl GravityOrbitsDemo {
    pub fn developer_set_session(&mut self, session: &str) {
        self.developer_session = Some(session.into());
    }
    pub fn developer_last_snapshot(&self) -> Option<&DeveloperSnapshot> {
        self.developer_snapshot.as_ref()
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
                    semantic_identity: matches!(
                        self.scenario,
                        GravityFixture::GameplaySolarSystem | GravityFixture::RealSolarSystem
                    )
                    .then(|| {
                        format!(
                            "solar:{:?}",
                            crate::solar_system::SOLAR_SYSTEM_CONTENT[index].identity
                        )
                        .to_lowercase()
                    }),
                    name: body.name().into(),
                    mass_kg: body.properties().mass_kg(),
                    reference_radius_m: body.properties().reference_radius_m(),
                    position_m: body.state().center_in_system().metres().to_array(),
                    velocity_m_s: body
                        .state()
                        .center_velocity_in_system()
                        .metres_per_second()
                        .to_array(),
                    surface_available: self.surfaces.get(index).is_some(),
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
            DevCommand::SkySetting { setting, value } => {
                ensure!(value.is_finite(), "nonfinite sky value");
                let mut sky = self.controls.sky;
                match setting.as_str() {
                    "intensity" => sky.intensity = *value as f32,
                    "star_intensity" => sky.star_intensity = *value as f32,
                    "background_intensity" => sky.background_intensity = *value as f32,
                    "halo_strength" => sky.halo_strength = *value as f32,
                    "galactic_yaw_rad" => sky.galactic_yaw_rad = *value,
                    "galactic_roll_rad" => sky.galactic_roll_rad = *value,
                    _ => bail!("unsupported sky setting"),
                };
                self.command(Command::Visual(visual_controls::VisualCommand::Sky(sky)))
            }
            DevCommand::GpuTile {
                enabled,
                family,
                seed,
                radius_m,
                face,
                level,
                x,
                y,
                cells,
                revision,
            } => {
                if !enabled {
                    return self.developer_disable_resident_tile();
                }
                let config = super::resident_fixture::ResidentTileConfig::parse(
                    family, *seed, *radius_m, face, *level, *x, *y, *cells, *revision,
                )?;
                self.developer_enable_resident_tile(config)
            }
            DevCommand::GpuTileView {
                mode,
                camera_offset_m,
                sun_direction_body,
                reference_cpu,
            } => {
                ensure!(
                    *mode <= 5 || self.resident_regional.enabled,
                    "modes 6..=10 require the regional fixture"
                );
                self.resident_tile.set_view(
                    *mode,
                    *camera_offset_m,
                    *sun_direction_body,
                    *reference_cpu,
                )?;
                if self.resident_tile.enabled {
                    self.developer_place_resident_camera()?;
                    self.resident_tile.validation_pending =
                        !self.resident_tile.reference_cpu && !self.resident_regional.enabled;
                }
                Ok(())
            }
            DevCommand::GpuHierarchy {
                enabled,
                refine,
                morph_duration_ms,
                child_delays_ms,
                request_mask,
                cancel_pending,
                diagnostic_validate,
            } => self.developer_configure_resident_hierarchy(
                super::hierarchy_fixture::HierarchySettings {
                    enabled: *enabled,
                    refine: *refine,
                    morph_duration_ms: *morph_duration_ms,
                    child_delays_ms: *child_delays_ms,
                    request_mask: *request_mask,
                    cancel_pending: *cancel_pending,
                    diagnostic_validate: *diagnostic_validate,
                },
            ),
            DevCommand::GpuRegional {
                enabled,
                max_depth,
                gpu_slots,
                cpu_tiles,
                worker_count,
                worker_delay_ms,
                upload_tiles_per_frame,
                upload_bytes_per_frame,
                publication_groups_per_frame,
                transition_limit,
                morph_duration_ms,
                split_error_px,
                merge_error_px,
            } => self.developer_configure_resident_regional(
                super::regional_fixture::RegionalSettings {
                    enabled: *enabled,
                    max_depth: *max_depth,
                    gpu_slots: *gpu_slots,
                    cpu_tiles: *cpu_tiles,
                    worker_count: *worker_count,
                    worker_delay_ms: *worker_delay_ms,
                    upload_tiles_per_frame: *upload_tiles_per_frame,
                    upload_bytes_per_frame: *upload_bytes_per_frame,
                    publication_groups_per_frame: *publication_groups_per_frame,
                    transition_limit: *transition_limit,
                    morph_duration_ms: *morph_duration_ms,
                    split_error_px: *split_error_px,
                    merge_error_px: *merge_error_px,
                },
            ),
        }
    }

    fn developer_configure_resident_regional(
        &mut self,
        settings: super::regional_fixture::RegionalSettings,
    ) -> Result<()> {
        settings.validate()?;
        if !settings.enabled {
            return self.resident_regional.configure(settings, None, None, None);
        }
        ensure!(
            self.resident_tile.enabled,
            "GPU regional fixture requires an active GpuTile parent"
        );
        ensure!(
            self.resident_tile.stale_reason.is_none(),
            "regional parent authority is stale"
        );
        let body = self
            .resident_tile
            .body
            .context("regional parent body unavailable")?;
        let parent = self
            .resident_tile
            .tile
            .as_ref()
            .context("regional parent tile unavailable")?
            .clone();
        let state = self.system.body(body)?;
        let definition = state
            .surface_definition()
            .context("regional fixture requires compositional authority")?;
        let generator = SurfaceGenerator::new(definition, state.properties().reference_radius_m())?;
        let identity = crate::resident_terrain::TileBuildIdentity {
            body_identity: stable_body_identity(state.name()),
            surface_revision: state.terrain_revision().value(),
            material_revision: definition.material_identity(),
        };
        ensure!(
            crate::resident_terrain::ResidentTileBuilder::tile_key(
                &generator,
                identity,
                parent.key.address,
                parent.key.cells
            )? == parent.key,
            "regional parent key no longer matches authority"
        );
        self.resident_regional.configure(
            settings,
            Some(parent),
            Some(generator),
            Some(identity),
        )?;
        self.resident_hierarchy.disable();
        self.resident_tile.validation_pending = false;
        self.resident_tile.reference_cpu = false;
        Ok(())
    }

    fn developer_configure_resident_hierarchy(
        &mut self,
        settings: super::hierarchy_fixture::HierarchySettings,
    ) -> Result<()> {
        let enabled = settings.enabled;
        let (parent, generator, identity) = if enabled {
            anyhow::ensure!(
                self.resident_tile.enabled,
                "GPU hierarchy requires an active GpuTile parent"
            );
            anyhow::ensure!(
                self.resident_tile.stale_reason.is_none(),
                "GPU hierarchy parent authority is stale"
            );
            let body = self
                .resident_tile
                .body
                .context("GPU hierarchy parent body is unavailable")?;
            let parent = self
                .resident_tile
                .tile
                .as_ref()
                .context("GPU hierarchy parent tile is unavailable")?
                .clone();
            let state = self.system.body(body)?;
            let definition = state
                .surface_definition()
                .context("GPU hierarchy requires a compositional published surface")?;
            let generator =
                SurfaceGenerator::new(definition, state.properties().reference_radius_m())?;
            let identity = crate::resident_terrain::TileBuildIdentity {
                body_identity: stable_body_identity(state.name()),
                surface_revision: state.terrain_revision().value(),
                material_revision: definition.material_identity(),
            };
            let key = crate::resident_terrain::ResidentTileBuilder::tile_key(
                &generator,
                identity,
                parent.key.address,
                parent.key.cells,
            )?;
            anyhow::ensure!(
                key == parent.key,
                "GPU hierarchy parent key no longer matches published authority"
            );
            (Some(parent), Some(generator), Some(identity))
        } else {
            (None, None, None)
        };
        self.resident_hierarchy
            .configure(settings, parent, generator, identity)
    }

    fn developer_enable_resident_tile(
        &mut self,
        config: super::resident_fixture::ResidentTileConfig,
    ) -> Result<()> {
        self.resident_regional.disable();
        let body = self
            .selection
            .selected()
            .or_else(|| self.ids.get(self.selected_index()).copied())
            .ok_or_else(|| anyhow::anyhow!("resident tile requires a selected body"))?;
        ensure!(
            self.surfaces.iter().any(|session| session.body() == body),
            "resident tile fixture requires an existing production surface session"
        );
        if self.resident_tile.body.is_some_and(|old| old != body) {
            self.developer_disable_resident_tile()?;
        }
        let same = self.resident_tile.enabled
            && self.resident_tile.body == Some(body)
            && self.resident_tile.config.as_ref() == Some(&config);
        if same {
            let current = self.system.body(body)?;
            if let Some(definition) = current.surface_definition() {
                let generator =
                    SurfaceGenerator::new(definition, current.properties().reference_radius_m())?;
                let key = crate::resident_terrain::ResidentTileBuilder::tile_key(
                    &generator,
                    crate::resident_terrain::TileBuildIdentity {
                        body_identity: stable_body_identity(current.name()),
                        surface_revision: current.terrain_revision().value(),
                        material_revision: definition.material_identity(),
                    },
                    config.address,
                    config.cells,
                )?;
                if self
                    .resident_tile
                    .tile
                    .as_ref()
                    .is_some_and(|tile| tile.key == key)
                {
                    return Ok(());
                }
            }
            // An authority edit happened outside this explicit command. Treat
            // this request as the requested rebuild instead of reusing stale data.
            self.resident_hierarchy.disable();
        }
        if !same && self.resident_hierarchy.enabled {
            self.resident_hierarchy.disable();
        }

        if self.resident_tile.restore.is_none() {
            let current = self.system.body(body)?;
            self.resident_tile.restore = Some(super::resident_fixture::RestoreAuthority {
                body,
                radius_m: current.properties().reference_radius_m(),
                legacy: current.terrain().cloned(),
                surface: current.surface_definition().cloned(),
                camera: self.camera.clone(),
            });
        }

        let current = self.system.body(body)?;
        let definition_identity = generated_definition_identity(current.name(), config.revision);
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(definition_identity),
            TerrainSeed(config.seed),
            config.family,
        );
        self.system
            .edit_surface_definition(body, Some(definition))?;
        self.developer_edit_radius(body, config.radius_m)?;
        self.publish();
        ensure!(self.coherent, "resident tile authority publication failed");

        let state = self.system.body(body)?;
        let definition = state
            .surface_definition()
            .ok_or_else(|| anyhow::anyhow!("resident tile surface publication missing"))?;
        let generator = SurfaceGenerator::new(definition, state.properties().reference_radius_m())?;
        let identity = crate::resident_terrain::TileBuildIdentity {
            body_identity: stable_body_identity(state.name()),
            surface_revision: state.terrain_revision().value(),
            material_revision: definition.material_identity(),
        };
        let (tile, build) = crate::resident_terrain::ResidentTileBuilder::build(
            &generator,
            identity,
            config.address,
            config.cells,
        )?;
        let approximation =
            crate::resident_terrain::ResidentTileBuilder::measure_approximation(&generator, &tile)?;
        let cpu_reference =
            super::resident_fixture::CpuReference::build(&generator, config.address)?;
        let publication = self.resident_tile.slot_state.request(&tile.key)?;
        ensure!(
            self.resident_tile
                .slot_state
                .accept_publication(&publication, &tile.key),
            "resident tile publication became stale before installation"
        );
        self.resident_tile.enabled = true;
        self.resident_tile.body = Some(body);
        self.resident_tile.config = Some(config);
        self.resident_tile.tile = Some(std::sync::Arc::new(tile));
        self.resident_tile.publication = Some(publication);
        self.resident_tile.cpu_reference = Some(cpu_reference);
        self.resident_tile.build = Some(build);
        self.resident_tile.approximation = Some(approximation);
        self.resident_tile.validation_pending = true;
        self.resident_tile.gpu_validation = None;
        self.resident_tile.stale_reason = None;
        self.developer_place_resident_camera()?;
        Ok(())
    }

    fn developer_edit_radius(&mut self, body: BodyId, radius_m: f64) -> Result<()> {
        let mass = self.system.body(body)?.properties().mass_kg();
        let properties = BodyProperties::new(mass, radius_m)?;
        match &mut self.motion {
            MotionSession::Newtonian(runner) => {
                runner.edit_properties(&mut self.system, body, properties)?
            }
            MotionSession::Analytic(session) => {
                session.edit_properties(&mut self.system, body, properties)?
            }
        }
        Ok(())
    }

    fn developer_disable_resident_tile(&mut self) -> Result<()> {
        self.resident_regional.disable();
        self.resident_hierarchy.disable();
        if let Some(saved) = self.resident_tile.restore.take() {
            self.developer_edit_radius(saved.body, saved.radius_m)?;
            if saved.surface.is_some() {
                self.system
                    .edit_surface_definition(saved.body, saved.surface)?;
            } else {
                self.system.edit_terrain(saved.body, saved.legacy)?;
            }
            self.publish();
            ensure!(self.coherent, "resident tile authority restoration failed");
            self.camera = saved.camera;
        }
        self.resident_tile.enabled = false;
        self.resident_tile.body = None;
        self.resident_tile.config = None;
        self.resident_tile.tile = None;
        self.resident_tile.publication = None;
        self.resident_tile.cpu_reference = None;
        self.resident_tile.build = None;
        self.resident_tile.approximation = None;
        self.resident_tile.validation_pending = false;
        self.resident_tile.gpu_validation = None;
        self.resident_tile.stale_reason = None;
        Ok(())
    }

    fn developer_place_resident_camera(&mut self) -> Result<()> {
        let body = self
            .resident_tile
            .body
            .ok_or_else(|| anyhow::anyhow!("resident tile body is unavailable"))?;
        let pair = self.projection.coherent_view(&self.system)?;
        let fixed = pair.projection().frames_for(body)?.body_fixed;
        let pose = self.resident_tile.camera_pose(fixed)?;
        self.camera
            .developer_set_surface_pose(&pair, body, pose)
            .with_context(|| {
                format!(
                    "placing resident tile observer for body {:?}, tile radius {} m, revision {}",
                    body,
                    self.resident_tile
                        .tile
                        .as_ref()
                        .map_or(0.0, |tile| tile.anchor_radius_m),
                    self.resident_tile
                        .config
                        .as_ref()
                        .map_or(0, |config| config.revision),
                )
            })?;
        self.auto_fit = false;
        self.controls.approach = None;
        Ok(())
    }
    pub fn developer_offscreen_frame(
        &mut self,
        elapsed: Duration,
        width: u32,
        height: u32,
    ) -> Result<crate::developer_capture::DeveloperCapture> {
        ensure!(width > 0 && height > 0, "not_drawable");
        let mut renderer = match self.developer_offscreen.take() {
            Some(r) => r,
            None => {
                self.terrain = crate::terrain_population::TerrainPopulation::new()?;
                mundaris_renderer::terrain_capture::TerrainCaptureRenderer::new(width, height)?
            }
        };
        let mut rgba = Vec::new();
        let result = self.render_host(
            &mut frame_host::FrameHost::Offscreen(&mut renderer, &mut rgba),
            width,
            height,
            Some(elapsed),
        );
        let adapter = renderer.adapter_name().into();
        let backend = renderer.adapter_backend().into();
        self.developer_offscreen = Some(renderer);
        result?;
        let mut snapshot = self
            .developer_snapshot
            .clone()
            .ok_or_else(|| anyhow::anyhow!("no prepared observation"))?;
        snapshot.capture = Some(crate::developer_snapshot::CaptureMetadata {
            scene: format!("frame-{}", self.developer_frame_number),
            image: format!("frame-{}.png", self.developer_frame_number),
            width,
            height,
            terrain_updates: self.developer_frame_number as usize,
            worker_count: 0,
            step_ms: elapsed.as_millis() as u64,
            morph_duration_ms: self.controls.terrain_morph_ms,
            adapter,
            backend,
        });
        Ok(crate::developer_capture::DeveloperCapture { rgba, snapshot })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_world::terrain::SurfaceAlgorithm;
    #[test]
    fn generated_definition_and_tile_key_ignore_prior_request_order() {
        let direct = SurfaceDefinition::generated(
            TerrainIdentity(generated_definition_identity("Moon", 7)),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let _intermediate = SurfaceDefinition::generated(
            TerrainIdentity(generated_definition_identity("Moon", 2)),
            TerrainSeed(99),
            SurfaceAlgorithm::IcyV3,
        );
        let via_intermediate = SurfaceDefinition::generated(
            TerrainIdentity(generated_definition_identity("Moon", 7)),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        assert_eq!(direct, via_intermediate);
        let direct_generator = SurfaceGenerator::new(&direct, 80_000.0).unwrap();
        let alternate_generator = SurfaceGenerator::new(&via_intermediate, 80_000.0).unwrap();
        let identity = crate::resident_terrain::TileBuildIdentity {
            body_identity: stable_body_identity("Moon"),
            surface_revision: 11,
            material_revision: direct.material_identity(),
        };
        let address = mundaris_math::surface::CubePatchAddress::try_new(
            mundaris_math::surface::CubeFace::PositiveZ,
            9,
            157,
            39,
        )
        .unwrap();
        let direct_key = crate::resident_terrain::ResidentTileBuilder::tile_key(
            &direct_generator,
            identity,
            address,
            64,
        )
        .unwrap();
        let alternate_key = crate::resident_terrain::ResidentTileBuilder::tile_key(
            &alternate_generator,
            identity,
            address,
            64,
        )
        .unwrap();
        assert_eq!(direct_key, alternate_key);
    }
    #[test]
    fn projection_rebuild_preserves_handles_and_world_replacement_invalidates_them() {
        let mut demo = GravityOrbitsDemo::solar_system(false).unwrap();
        demo.developer_set_session("session");
        let before = demo.developer_inventory("session").unwrap();
        demo.command(Command::Rebuild).unwrap();
        assert_eq!(
            before[3].handle,
            demo.developer_inventory("session").unwrap()[3].handle
        );
        demo.command(Command::Load(GravityFixture::Circular))
            .unwrap();
        assert!(
            demo.developer_apply_command(&DevCommand::Select {
                body: before[3].handle.clone()
            })
            .is_err()
        );
        assert_eq!(demo.developer_session.as_deref(), Some("session"));
    }
    #[test]
    fn unfocused_ui_does_not_cancel_explicit_automation_focus() {
        let mut demo = GravityOrbitsDemo::solar_system(false).unwrap();
        demo.developer_set_session("session");
        demo.developer_set_automation(Some("controller"));
        let body = demo.developer_inventory("session").unwrap()[3]
            .handle
            .clone();
        demo.developer_apply_command(&DevCommand::Focus {
            body,
            body_fixed: false,
        })
        .unwrap();
        let context = egui::Context::default();
        let ui_context = context.clone();
        let _ = context.run_ui(
            egui::RawInput {
                focused: false,
                ..Default::default()
            },
            |root| {
                let (info, controls) =
                    demo.ui_info(CelestialPreparationReport::default(), 0.1, 0.0);
                draw_ui(&ui_context, root, controls, &info, &[]);
            },
        );
        for _ in 0..60 {
            demo.update(Duration::from_millis(16));
        }
        assert_eq!(demo.camera.focused_body(), Some(demo.ids[3]));
        assert_eq!(demo.camera.mode(), CameraMode::BodyOrbit);
        assert!(!demo.camera.transitioning());
    }
}
