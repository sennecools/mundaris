//! Ordinary observer-local planetary terrain over the resident tile runtime.
use super::regional_fixture::RegionalFixture;
use crate::resident_terrain::TileBuildIdentity;
use anyhow::{Context, Result};
use glam::{DMat3, DVec3};
use mundaris_math::{Direction3, FrameId, FramePosition, LocalPosition};
use mundaris_renderer::{
    CelestialProjection, PreparedView, RegionalResidentDraw, RegionalResidentReport, TileDraw,
    TileSlotState, planet_surface::TerrainLighting,
};
use mundaris_world::{
    BodyId, CoherentCelestialView,
    terrain::{SurfaceDefinition, SurfaceGenerator},
};
use std::time::Duration;

pub(super) struct PlanetaryTerrain {
    pub enabled: bool,
    pub body: Option<BodyId>,
    pub runtime: RegionalFixture,
    binding: Option<(SurfaceDefinition, u64, u64)>,
    binding_ready: bool,
    last_summary: Option<crate::developer_snapshot::ResidentDiagnosticSnapshot>,
    radial_levels: (Option<u8>, Option<u8>),
}

impl PlanetaryTerrain {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            body: None,
            runtime: RegionalFixture::default(),
            binding: None,
            binding_ready: false,
            last_summary: None,
            radial_levels: (None, None),
        }
    }

    /// Read-only publication signature for native authoring evidence and tests.
    pub(super) fn binding_signature(&self) -> Option<(&SurfaceDefinition, u64, u64)> {
        self.binding
            .as_ref()
            .map(|(definition, radius_bits, revision)| (definition, *radius_bits, *revision))
    }

    pub(super) fn binding_ready(&self) -> bool {
        self.binding_ready
    }

    /// Configure from immutable world authority. A busy old pool drains before
    /// replacement; rapid body changes cannot accumulate detached worker pools.
    pub fn bind(
        &mut self,
        body: BodyId,
        identity: u64,
        pair: &CoherentCelestialView<'_>,
    ) -> Result<bool> {
        let state = pair.system().body(body)?;
        let definition = state
            .surface_definition()
            .context("resident body lacks surface authority")?;
        let radius = state.properties().reference_radius_m();
        let revision = state.terrain_revision().value();
        if self.body == Some(body)
            && self.binding.as_ref().is_some_and(|(d, r, v)| {
                d == definition && *r == radius.to_bits() && *v == revision
            })
        {
            self.binding_ready = true;
            return Ok(true);
        }
        self.binding_ready = false;
        self.last_summary = None;
        let generator = SurfaceGenerator::new(definition, radius)?;
        if self.runtime.configure_planetary(
            generator,
            TileBuildIdentity {
                body_identity: identity,
                surface_revision: revision,
                material_revision: revision,
            },
            32,
        )? {
            self.body = Some(body);
            self.binding = Some((definition.clone(), radius.to_bits(), revision));
            self.binding_ready = true;
        }
        Ok(self.binding_ready)
    }

    #[allow(clippy::too_many_arguments)] // One coherent frame preparation input.
    pub fn prepare(
        &mut self,
        view: &PreparedView<'_>,
        body_frame: FrameId,
        projection: CelestialProjection,
        lighting: TerrainLighting,
        report: RegionalResidentReport,
        elapsed: Duration,
        deterministic: bool,
    ) -> Result<Option<RegionalResidentDraw>> {
        if !self.binding_ready {
            return Ok(None);
        }
        let source = view.prepare_source(body_frame)?;
        let direction =
            |v| -> Result<DVec3> { Ok(source.view_direction(Direction3::try_new(v)?)?.unit()) };
        let body_to_view = DMat3::from_cols(
            direction(DVec3::X)?,
            direction(DVec3::Y)?,
            direction(DVec3::Z)?,
        );
        self.runtime.observe(report);
        self.runtime.set_planetary_view(body_to_view, projection)?;
        let projection_scale = projection.focal_pixels();
        self.runtime.advance(
            source.observer_in_source().metres(),
            projection_scale,
            elapsed,
            deterministic,
        )?;
        self.radial_levels = self
            .runtime
            .radial_levels(source.observer_in_source().metres());
        let Some(tile) = self.runtime.template_tile() else {
            return Ok(None);
        };
        let anchor = tile.anchor_position_body()?;
        let anchor_view_m = source
            .view_displacement(FramePosition::new(
                body_frame,
                LocalPosition::try_metres(anchor)?,
            ))?
            .metres();
        // The runtime assigns real slot tokens; this template only supplies the
        // f64 view transform and presentation inputs for each independent tile.
        let publication = TileSlotState::default().request(&tile.key)?;
        let draw = TileDraw {
            tile,
            publication,
            anchor_view_m,
            body_to_view,
            mode: match lighting.mode() {
                mundaris_renderer::planet_surface::TerrainRenderMode::Normals => 2,
                mundaris_renderer::planet_surface::TerrainRenderMode::Elevation => 1,
                // Uniform albedo preserves geometry lighting while excluding
                // material weights and per-tile height normalization.
                mundaris_renderer::planet_surface::TerrainRenderMode::Diffuse => 11,
                _ => 0,
            },
            sun_body: lighting.sun_direction_body(),
        };
        Ok(Some(self.runtime.draw(&draw)?))
    }

    #[cfg(test)]
    pub fn snapshot(&self) -> Option<serde_json::Value> {
        self.enabled.then(|| self.runtime.snapshot()).flatten()
    }

    fn snapshot_frame(&self) -> Option<crate::developer_snapshot::ResidentDiagnosticSnapshot> {
        self.enabled
            .then(|| self.runtime.snapshot_frame())
            .flatten()
    }

    pub fn annotate(
        &mut self,
        snapshot: &mut crate::developer_snapshot::DeveloperSnapshot,
        ids: &[BodyId],
        system: &mundaris_world::CelestialSystem,
        active_view: bool,
    ) -> Result<()> {
        let value = self.snapshot_frame();
        // Cloning shares the compact summary and typed history arcs instead of
        // deep-cloning terrain trace values or materializing the history arrays.
        self.last_summary = value.clone();
        self.annotate_value(snapshot, ids, system, value, active_view)
    }

    /// UI consumes the preceding submitted summary. Full evidence is collected
    /// once after current submission, rather than formatted twice per frame.
    pub fn annotate_previous(
        &self,
        snapshot: &mut crate::developer_snapshot::DeveloperSnapshot,
        ids: &[BodyId],
        system: &mundaris_world::CelestialSystem,
        active_view: bool,
    ) -> Result<()> {
        self.annotate_value(
            snapshot,
            ids,
            system,
            self.last_summary.clone(),
            active_view,
        )
    }

    fn annotate_value(
        &self,
        snapshot: &mut crate::developer_snapshot::DeveloperSnapshot,
        ids: &[BodyId],
        system: &mundaris_world::CelestialSystem,
        value: Option<crate::developer_snapshot::ResidentDiagnosticSnapshot>,
        active_view: bool,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        snapshot.terrain.backend = "RESIDENT TILE".into();
        if let Some(body) = self.body {
            let state = system.body(body)?;
            snapshot.terrain.active_body = ids.iter().position(|&id| id == body).map(|index| {
                crate::developer_snapshot::BodySnapshot {
                    index,
                    name: state.name().into(),
                }
            });
            snapshot.terrain.generator_algorithm = state
                .surface_definition()
                .map(|d| format!("{:?}", d.terrain().algorithm()));
        }
        snapshot.terrain.refinement_demand_kind = Some("projected_error_proxy".into());
        snapshot.terrain.certificate_kind = Some("uncertified_relief_sagitta_proxy".into());
        snapshot.terrain.target_certifiable = Some(false);
        snapshot.terrain.morph_fraction = None;
        snapshot.terrain.transition_deferred = false;
        snapshot.terrain.ready = self.binding_ready && self.runtime.has_coverage();
        snapshot.terrain.desired_radial_lod = self.radial_levels.0;
        snapshot.terrain.source_radial_lod = self.radial_levels.1;
        snapshot.terrain.ready_radial_lod = self.radial_levels.1;
        snapshot.resident_planetary = value;
        if let Some(value) = &snapshot.resident_planetary {
            let count = |field| value[field].as_u64().unwrap_or(0) as usize;
            let pending = !snapshot.terrain.ready
                || value["quality_pending"].as_bool().unwrap_or(true)
                || value["target_quality_reached"].as_bool() == Some(false);
            snapshot.terrain.quality_pending = Some(pending);
            snapshot.terrain.settled = Some(!pending);
            snapshot.terrain.source_leaf_count = count("drawable_count");
            snapshot.terrain.visible_leaf_count = count("visible_drawable_count");
            snapshot.terrain.active_morph = count("active_transitions") != 0;
            snapshot.terrain.construction_pending = count("worker_outstanding") != 0
                || value["publication_worker_in_flight"].as_bool() == Some(true)
                || value["publication_pipeline"]["fully_prepared_results"]
                    .as_u64()
                    .unwrap_or(0)
                    != 0;
            snapshot.terrain.budget_constrained = [
                "cpu_cache_pressure",
                "desired_capacity_pressure",
                "slot_pressure",
                "gpu_admission_pressure",
            ]
            .iter()
            .any(|field| value[*field].as_bool() == Some(true));
            snapshot.work.worker_count =
                value["configuration"]["worker_count"].as_u64().unwrap_or(0) as usize;
            snapshot.work.worker_busy_count = count("worker_running");
            snapshot.work.pending_requests = count("worker_queued") + count("completion_backlog");
            snapshot.work.raw_resident_patches = count("cpu_cached_tiles");
            snapshot.memory.used_bytes = value["cpu_cached_bytes"].as_u64().unwrap_or(0);
            snapshot.memory.cap_bytes = value["cpu_byte_cap"].as_u64().unwrap_or(0);
            snapshot.memory.headroom_bytes = snapshot
                .memory
                .cap_bytes
                .saturating_sub(snapshot.memory.used_bytes);
        } else {
            snapshot.terrain.quality_pending = Some(true);
            snapshot.terrain.settled = Some(false);
        }
        if !active_view {
            snapshot.terrain.active_body = None;
            snapshot.terrain.generator_algorithm = None;
            snapshot.terrain.ready = false;
            snapshot.terrain.quality_pending = None;
            snapshot.terrain.settled = None;
            snapshot.terrain.source_leaf_count = 0;
            snapshot.terrain.visible_leaf_count = 0;
            snapshot.terrain.active_morph = false;
            snapshot.terrain.construction_pending = false;
            snapshot.terrain.budget_constrained = false;
            snapshot.terrain.desired_radial_lod = None;
            snapshot.terrain.source_radial_lod = None;
            snapshot.terrain.ready_radial_lod = None;
        }
        snapshot.refresh_warnings();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn solar_system_defaults_to_world_owned_resident_moon() {
        let mut demo = crate::GravityOrbitsDemo::solar_system(false).unwrap();
        assert!(demo.planetary.enabled);
        let (body, definition, radius, revision) = demo
            .system
            .bodies()
            .find(|(_, state)| state.name() == "Moon")
            .map(|(id, state)| {
                (
                    id,
                    state.surface_definition().unwrap().clone(),
                    state.properties().reference_radius_m(),
                    state.terrain_revision(),
                )
            })
            .unwrap();
        let pair = demo.projection.coherent_view(&demo.system).unwrap();
        demo.planetary.bind(body, 1, &pair).unwrap();
        let current = demo.system.body(body).unwrap();
        assert_eq!(current.surface_definition(), Some(&definition));
        assert_eq!(current.properties().reference_radius_m(), radius);
        assert_eq!(current.terrain_revision(), revision);
        let snapshot = demo.planetary.snapshot().unwrap();
        assert_eq!(
            snapshot["configuration"]["roots"].as_array().unwrap().len(),
            6
        );
        assert!(!demo.planetary.runtime.has_coverage());
        demo.use_legacy_terrain();
        assert!(!demo.planetary.enabled);
    }

    #[test]
    fn changed_world_authority_never_marks_old_binding_ready() {
        use glam::DVec3;
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
        };
        let mut demo = crate::GravityOrbitsDemo::solar_system(false).unwrap();
        let body = demo
            .system
            .bodies()
            .find(|(_, state)| state.name() == "Moon")
            .unwrap()
            .0;
        let radius = demo
            .system
            .body(body)
            .unwrap()
            .properties()
            .reference_radius_m();
        {
            let pair = demo.projection.coherent_view(&demo.system).unwrap();
            assert!(demo.planetary.bind(body, 1, &pair).unwrap());
        }
        demo.planetary
            .runtime
            .advance(
                DVec3::Z * (radius + 1000.0),
                400.0,
                std::time::Duration::from_millis(16),
                false,
            )
            .unwrap();
        let changed = SurfaceDefinition::generated(
            TerrainIdentity(0x2d1234),
            TerrainSeed(0x2d5678),
            SurfaceAlgorithm::RockyV5,
        );
        demo.system
            .edit_surface_definition(body, Some(changed.clone()))
            .unwrap();
        let pair = demo.projection.coherent_view(&demo.system).unwrap();
        let ready = demo.planetary.bind(body, 1, &pair).unwrap();
        assert_eq!(demo.planetary.binding_ready, ready);
        if ready {
            assert_eq!(demo.planetary.binding.as_ref().unwrap().0, changed);
            assert!(!demo.planetary.runtime.has_coverage());
        } else {
            assert_ne!(demo.planetary.binding.as_ref().unwrap().0, changed);
            assert!(!demo.planetary.binding_ready);
        }
    }
}
