//! Canonical, read-only developer state shared by native summaries and capture tools.
//! Values use SI units; absent measurements are `None`, never inferred zero.

use anyhow::{Result, ensure};
use mundaris_math::FramePose;
use mundaris_renderer::{
    CelestialPreparationReport, CelestialProjection, GpuProfile, planet_surface::TerrainRenderMode,
};
use mundaris_world::{BodyId, CoherentCelestialView};
use serde::{Deserialize, Serialize};

use crate::{
    celestial_camera::{CameraMode, NavigationDiagnostics},
    motion_session::MotionSnapshot,
    terrain_population::TerrainPopulation,
};

/// Version of the JSON contract, independent of engine/world persistence formats.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 4;
/// UI-only advisory ratio. This does not alter admission or the terrain cap.
pub const MEMORY_NEAR_CAP_RATIO: f64 = 0.95;

pub fn render_mode_name(mode: TerrainRenderMode) -> &'static str {
    match mode {
        TerrainRenderMode::Natural => "natural",
        TerrainRenderMode::Elevation => "elevation",
        TerrainRenderMode::Lit => "lit",
        TerrainRenderMode::Normals => "normals",
        TerrainRenderMode::Diffuse => "diffuse",
        TerrainRenderMode::Readability => "readability",
        TerrainRenderMode::Slope => "slope",
        TerrainRenderMode::SeaMask => "sea_mask",
        TerrainRenderMode::RockWeight => "rock_weight",
    }
}

/// A body association within this snapshot's world, not a persisted runtime handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySnapshot {
    pub index: usize,
    pub name: String,
}

/// Stable names rather than Rust debug strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotCameraMode {
    SystemOrbit,
    BodyOrbit,
    FreeFlight,
    SurfaceInspection,
}
impl SnapshotCameraMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::SystemOrbit => "System orbit",
            Self::BodyOrbit => "Body orbit",
            Self::FreeFlight => "Free flight",
            Self::SurfaceInspection => "Surface inspection",
        }
    }
}
impl From<CameraMode> for SnapshotCameraMode {
    fn from(value: CameraMode) -> Self {
        match value {
            CameraMode::SystemOrbit => Self::SystemOrbit,
            CameraMode::BodyOrbit => Self::BodyOrbit,
            CameraMode::FreeFlight => Self::FreeFlight,
            CameraMode::SurfaceInspection => Self::SurfaceInspection,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneralSnapshot {
    pub frame_number: u64,
    pub simulation_time_s: f64,
    pub world_revision: u64,
    pub selected_body: Option<BodySnapshot>,
    pub focused_body: Option<BodySnapshot>,
    pub camera_mode: SnapshotCameraMode,
    pub paused: bool,
    pub simulation_speed: f64,
}

/// The pose is expressed in the semantic reference frame, never a serialized FrameId.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CameraSnapshot {
    pub position_m: [f64; 3],
    pub orientation_xyzw: [f64; 4],
    pub reference_frame: String,
    pub frame_body: Option<BodySnapshot>,
    pub reference_body: Option<BodySnapshot>,
    pub body_distance_m: Option<f64>,
    pub reference_altitude_m: Option<f64>,
    pub terrain_clearance_m: Option<f64>,
    pub drawn_mesh_clearance_m: Option<f64>,
    pub fov_y_degrees: f64,
    pub near_plane_m: f64,
    pub viewport_origin_pixels: [u32; 2],
    pub viewport_size_pixels: [u32; 2],
    /// Copied camera-controller diagnostics; collection performs no navigation queries.
    #[serde(default)]
    pub navigation: Option<NavigationDiagnostics>,
}

/// Radial levels describe the camera's radial chain, not whole-view visual quality.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TerrainSnapshot {
    pub active_body: Option<BodySnapshot>,
    pub source_radial_lod: Option<u8>,
    pub ready_radial_lod: Option<u8>,
    pub desired_radial_lod: Option<u8>,
    pub source_leaf_count: usize,
    pub visible_leaf_count: usize,
    pub quality_pending: Option<bool>,
    pub settled: Option<bool>,
    pub ready: bool,
    pub active_morph: bool,
    pub morph_fraction: Option<f64>,
    pub construction_pending: bool,
    pub budget_constrained: bool,
    pub transition_deferred: bool,
}
impl TerrainSnapshot {
    /// Human summary derived from recorded flags, never from an idle work queue.
    pub fn status(&self) -> &'static str {
        if self.active_body.is_none() {
            "Not active"
        } else if self.transition_deferred || self.budget_constrained {
            "Resource constrained"
        } else if !self.ready {
            "Preparing"
        } else if self.active_morph {
            "Transitioning"
        } else if self.quality_pending == Some(true) {
            "Refining"
        } else if self.settled == Some(true) {
            "Settled"
        } else {
            "Updating"
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkSnapshot {
    pub worker_count: usize,
    pub worker_busy_count: usize,
    pub pending_requests: usize,
    pub raw_resident_patches: usize,
}

/// Conservatively accounted CPU terrain bytes, including reservations; not RSS/VRAM.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    pub used_bytes: u64,
    pub cap_bytes: u64,
    pub headroom_bytes: u64,
}
impl MemorySnapshot {
    pub fn near_cap(&self) -> bool {
        self.cap_bytes > 0 && self.used_bytes as f64 / self.cap_bytes as f64 > MEMORY_NEAR_CAP_RATIO
    }
}

/// Enable flags are developer settings; draw flags identify layers in this frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderingSnapshot {
    pub terrain_render_mode: String,
    pub terrain_enabled: bool,
    pub ocean_enabled: bool,
    pub clouds_enabled: bool,
    pub atmosphere_enabled: bool,
    pub patch_borders_enabled: bool,
    pub lod_colors_enabled: bool,
    pub navigation_markers_enabled: bool,
    pub ocean_drawn: bool,
    pub clouds_drawn: bool,
    pub atmosphere_drawn: bool,
}
impl RenderingSnapshot {
    pub fn with_draw_report(mut self, report: CelestialPreparationReport) -> Self {
        self.ocean_drawn = report.planetary_ocean_draws > 0;
        self.clouds_drawn = report.planetary_cloud_draws > 0;
        self.atmosphere_drawn = report.planetary_atmosphere_draws > 0;
        self
    }
}

/// CPU stages are host elapsed times. GPU regular terrain and fallback are separate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    pub frame_cpu_ms: Option<f64>,
    pub update_ms: Option<f64>,
    pub terrain_update_ms: Option<f64>,
    pub preparation_ms: Option<f64>,
    pub terrain_preparation_ms: Option<f64>,
    pub gpu_terrain_ms: Option<f64>,
    pub gpu_transition_fallback_ms: Option<f64>,
    pub gpu_timing_scope: String,
    pub upload_bytes: Option<u64>,
}
impl PerformanceSnapshot {
    /// Native uses `latest_completed`; blocking offscreen readback uses `same_frame`.
    pub fn with_gpu(mut self, profile: GpuProfile, scope: &str) -> Self {
        self.gpu_terrain_ms = profile.terrain.map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_transition_fallback_ms = profile
            .transition_fallback
            .map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_timing_scope = scope.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureMetadata {
    pub scene: String,
    pub image: String,
    pub width: u32,
    pub height: u32,
    pub terrain_updates: usize,
    pub worker_count: usize,
    pub step_ms: u64,
    pub morph_duration_ms: u64,
    pub adapter: String,
    pub backend: String,
}

/// Decorative sky inputs and measured renderer observations, separate from bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkySnapshot {
    pub preset: String,
    pub version: u32,
    pub seed: u64,
    pub classification: String,
    pub anchor_frame: String,
    pub anchor_m: [f64; 3],
    pub galactic_to_system_xyzw: [f64; 4],
    pub finite_star_distance_range_m: [f64; 2],
    pub supported_observer_radius_m: f64,
    pub enabled: bool,
    pub stars_drawn: bool,
    pub background_drawn: bool,
    pub outside_envelope: bool,
    pub intensity: f32,
    pub star_intensity: f32,
    pub background_intensity: f32,
    pub halo_strength: f32,
    pub galactic_yaw_rad: f64,
    pub galactic_roll_rad: f64,
    pub star_count: usize,
    #[serde(default)]
    pub background_size_pixels: Option<[u32; 2]>,
    #[serde(default)]
    pub focal_chart_count: Option<usize>,
    #[serde(default)]
    pub focal_chart_size_pixels: Option<u32>,
    #[serde(default)]
    pub transient_generation_payload_bound_bytes: Option<u64>,
    pub cpu_preparation_ms: f64,
    pub generation_ms: Option<f64>,
    pub upload_api_ms: Option<f64>,
    pub gpu_sky_ms: Option<f64>,
    pub cpu_capacity_bytes: Option<u64>,
    pub gpu_capacity_bytes: Option<u64>,
    pub static_upload_bytes: Option<u64>,
    pub frame_upload_bytes: Option<u64>,
    pub resource_growth_events: Option<u64>,
    pub catalogue_upload_count: Option<u64>,
    pub background_upload_count: Option<u64>,
    pub resource_timing_scope: String,
    pub gpu_timing_scope: String,
}
impl SkySnapshot {
    pub fn collect(
        definition: &mundaris_renderer::sky::SkyDefinition,
        settings: mundaris_renderer::sky::SkySettings,
        report: mundaris_renderer::sky::SkyPreparationReport,
        resources: Option<mundaris_renderer::sky::SkyResourceReport>,
        gpu: GpuProfile,
        resource_scope: &str,
        gpu_scope: &str,
    ) -> Self {
        Self {
            preset: definition.identity().preset.into(),
            version: definition.identity().version,
            seed: definition.identity().seed,
            classification: crate::sky_definition::CLASSIFICATION.into(),
            anchor_frame: "system_inertial".into(),
            anchor_m: definition.anchor_m().to_array(),
            galactic_to_system_xyzw: definition.galactic_to_system().to_array(),
            finite_star_distance_range_m: [
                crate::sky_definition::FINITE_STAR_MIN_DISTANCE_M,
                crate::sky_definition::FINITE_STAR_MAX_DISTANCE_M,
            ],
            supported_observer_radius_m: crate::sky_definition::SUPPORTED_OBSERVER_RADIUS_M,
            enabled: report.enabled,
            stars_drawn: report.stars_drawn,
            background_drawn: report.background_drawn,
            outside_envelope: report.outside_envelope,
            intensity: settings.intensity,
            star_intensity: settings.star_intensity,
            background_intensity: settings.background_intensity,
            halo_strength: settings.halo_strength,
            galactic_yaw_rad: settings.galactic_yaw_rad,
            galactic_roll_rad: settings.galactic_roll_rad,
            star_count: report.star_count,
            background_size_pixels: Some([
                definition.background().width,
                definition.background().height,
            ]),
            focal_chart_count: Some(definition.morphology().map_or(0, |m| m.complexes.len())),
            focal_chart_size_pixels: definition.morphology().map(|m| m.detail_size),
            transient_generation_payload_bound_bytes: resources
                .and_then(|r| r.transient_generation_payload_bound_bytes),
            cpu_preparation_ms: report.cpu_preparation_ms,
            generation_ms: resources.and_then(|r| r.generation_ms),
            upload_api_ms: resources.map(|r| r.upload_api_ms),
            gpu_sky_ms: gpu.sky.map(|d| d.as_secs_f64() * 1000.0),
            cpu_capacity_bytes: resources.map(|r| r.cpu_capacity_bytes),
            gpu_capacity_bytes: resources.map(|r| r.gpu_capacity_bytes),
            static_upload_bytes: resources.map(|r| r.static_upload_bytes),
            frame_upload_bytes: resources.map(|r| r.frame_upload_bytes),
            resource_growth_events: resources.map(|r| r.resource_growth_events),
            catalogue_upload_count: resources.map(|r| r.catalogue_upload_count),
            background_upload_count: resources.map(|r| r.background_upload_count),
            resource_timing_scope: resource_scope.into(),
            gpu_timing_scope: gpu_scope.into(),
        }
    }
}

/// One developer-visible interpretation of a coherent prepared engine frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeveloperSnapshot {
    pub schema_version: u32,
    pub general: GeneralSnapshot,
    pub camera: CameraSnapshot,
    pub terrain: TerrainSnapshot,
    pub work: WorkSnapshot,
    pub memory: MemorySnapshot,
    pub rendering: RenderingSnapshot,
    pub performance: PerformanceSnapshot,
    pub warnings: Vec<String>,
    pub capture: Option<CaptureMetadata>,
    #[serde(default)]
    pub motion: Option<MotionSnapshot>,
    #[serde(default)]
    pub sky: Option<SkySnapshot>,
}

/// Already-measured frame inputs. Collection performs no terrain query/generation.
pub struct SnapshotInput<'a> {
    pub pair: &'a CoherentCelestialView<'a>,
    pub pose: FramePose,
    pub camera_mode: CameraMode,
    pub selected_body: Option<BodyId>,
    pub focused_body: Option<BodyId>,
    pub reference_body: Option<BodyId>,
    pub frame_number: u64,
    pub paused: bool,
    pub simulation_speed: f64,
    pub projection: CelestialProjection,
    pub terrain: &'a TerrainPopulation,
    pub terrain_clearance_m: Option<f64>,
    pub drawn_mesh_clearance_m: Option<f64>,
    pub rendering: RenderingSnapshot,
    pub performance: PerformanceSnapshot,
    pub navigation: Option<NavigationDiagnostics>,
    pub motion: Option<MotionSnapshot>,
}

impl DeveloperSnapshot {
    pub fn collect(input: SnapshotInput<'_>) -> Result<Self> {
        let world = input.pair.system();
        let body = |id: BodyId| -> Result<BodySnapshot> {
            let index = world
                .bodies()
                .position(|(other, _)| other == id)
                .ok_or_else(|| anyhow::anyhow!("snapshot body association missing"))?;
            Ok(BodySnapshot {
                index,
                name: world.body(id)?.name().into(),
            })
        };
        ensure!(
            input.simulation_speed.is_finite(),
            "nonfinite snapshot simulation speed"
        );
        if let Some(motion) = &input.motion {
            ensure!(
                motion.requested_time_s.is_finite(),
                "nonfinite requested motion time"
            );
            ensure!(
                motion.published_time_s == world.sample_time().seconds_since_epoch(),
                "motion publication time does not match coherent world time"
            );
            ensure!(
                motion.paused == input.paused,
                "motion pause state does not match snapshot state"
            );
            ensure!(
                motion.rate == input.simulation_speed,
                "motion rate does not match snapshot speed"
            );
        }
        let frame = input.pose.position().frame();
        let mut frame_body = None;
        let mut reference_frame = if frame == input.pair.projection().tree().root() {
            Some("system")
        } else {
            None
        };
        for (id, _) in world.bodies() {
            let frames = input.pair.projection().frames_for(id)?;
            if frame == frames.body_fixed || frame == frames.translating {
                reference_frame = Some(if frame == frames.body_fixed {
                    "body_fixed"
                } else {
                    "body_translating"
                });
                frame_body = Some(body(id)?);
                break;
            }
        }
        let mut distance = None;
        let mut altitude = None;
        if let Some(id) = input.reference_body {
            let fixed = input.pair.projection().frames_for(id)?.body_fixed;
            let p = input
                .pair
                .evaluation()
                .convert_position(input.pose.position(), fixed)?
                .local()
                .metres();
            distance = Some(p.length());
            altitude = Some(p.length() - world.body(id)?.properties().reference_radius_m());
        }
        let active = input.terrain.active_body();
        let cover = &input.terrain.cover;
        let diagnostic = cover.convergence;
        let cache = input.terrain.cache.report();
        let used = (cache.resident_bytes + cache.external_bytes) as u64;
        let cap = crate::planet_terrain::TERRAIN_CPU_CAP_BYTES as u64;
        let mut snapshot = Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            general: GeneralSnapshot {
                frame_number: input.frame_number,
                simulation_time_s: world.sample_time().seconds_since_epoch(),
                world_revision: world.revision(),
                selected_body: input.selected_body.map(body).transpose()?,
                focused_body: input.focused_body.map(body).transpose()?,
                camera_mode: input.camera_mode.into(),
                paused: input.paused,
                simulation_speed: input.simulation_speed,
            },
            camera: CameraSnapshot {
                position_m: input.pose.position().local().metres().to_array(),
                orientation_xyzw: input.pose.orientation().quaternion().to_array(),
                reference_frame: reference_frame
                    .ok_or_else(|| anyhow::anyhow!("snapshot frame role missing"))?
                    .into(),
                frame_body,
                reference_body: input.reference_body.map(body).transpose()?,
                body_distance_m: distance,
                reference_altitude_m: altitude,
                terrain_clearance_m: input.terrain_clearance_m.filter(|x| x.is_finite()),
                drawn_mesh_clearance_m: input.drawn_mesh_clearance_m.filter(|x| x.is_finite()),
                fov_y_degrees: input.projection.vertical_fov_rad().to_degrees(),
                near_plane_m: input.projection.near_m(),
                viewport_origin_pixels: input.projection.origin(),
                viewport_size_pixels: input.projection.viewport(),
                navigation: input.navigation,
            },
            terrain: TerrainSnapshot {
                active_body: active.map(body).transpose()?,
                source_radial_lod: active.and(diagnostic.rendered_local_lod),
                ready_radial_lod: active.and(diagnostic.ready_local_lod),
                desired_radial_lod: active.and(diagnostic.desired_local_lod),
                source_leaf_count: cover.active().len(),
                visible_leaf_count: cover.visible().len(),
                quality_pending: active.map(|_| cover.report.quality_pending),
                settled: active.map(|_| cover.report.settled),
                ready: cover.ready(),
                active_morph: cover.transition().is_some(),
                morph_fraction: cover.transition().map(|(_, fraction)| fraction),
                construction_pending: cover.construction_pending(),
                budget_constrained: cover.report.budget_constrained,
                transition_deferred: cover.transition_deferred,
            },
            work: WorkSnapshot {
                worker_count: cache.worker_count,
                worker_busy_count: cache.worker_jobs,
                pending_requests: input.terrain.cache.pending(),
                raw_resident_patches: cache.resident_patches,
            },
            memory: MemorySnapshot {
                used_bytes: used,
                cap_bytes: cap,
                headroom_bytes: cap.saturating_sub(used),
            },
            rendering: input.rendering,
            performance: input.performance,
            warnings: Vec::new(),
            capture: None,
            motion: input.motion,
            sky: None,
        };
        snapshot.refresh_warnings();
        Ok(snapshot)
    }

    /// Recompute only objective advisory flags, e.g. after same-frame GPU readback.
    pub fn refresh_warnings(&mut self) {
        self.warnings.clear();
        if self.memory.near_cap() {
            self.warnings.push("terrain_memory_near_cap".into());
        }
        if self.terrain.quality_pending == Some(true) {
            self.warnings.push("terrain_quality_pending".into());
        }
        if self.terrain.active_morph {
            self.warnings.push("active_transition".into());
        }
        if let (Some(source), Some(desired)) = (
            self.terrain.source_radial_lod,
            self.terrain.desired_radial_lod,
        ) && source < desired
        {
            self.warnings.push("source_below_desired".into());
        }
        if self.performance.gpu_terrain_ms.is_none() {
            self.warnings.push("gpu_timing_unavailable".into());
        }
    }

    pub fn write_json(&self, path: &std::path::Path) -> Result<()> {
        use anyhow::Context;
        let bytes = serde_json::to_vec_pretty(self).context("serializing developer snapshot")?;
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }
}

/// Compact, signed if requested, metres/kilometres/megametres/AU as appropriate.
pub fn format_distance(value: Option<f64>, signed: bool) -> String {
    let Some(value) = value.filter(|v| v.is_finite()) else {
        return "Unavailable".into();
    };
    let (scale, unit) = if value.abs() >= 149_597_870_700.0 {
        (149_597_870_700.0, "AU")
    } else if value.abs() >= 1_000_000.0 {
        (1_000_000.0, "Mm")
    } else if value.abs() >= 1_000.0 {
        (1_000.0, "km")
    } else {
        (1.0, "m")
    };
    let scaled = value / scale;
    let decimals = if scaled.abs() >= 100.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    if signed {
        format!("{scaled:+.decimals$} {unit}")
    } else {
        format!("{scaled:.decimals$} {unit}")
    }
}

pub fn format_milliseconds(value: Option<f64>) -> String {
    value
        .filter(|v| v.is_finite())
        .map_or_else(|| "Unavailable".into(), |v| format!("{v:.2} ms"))
}

pub fn format_bytes(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / 1_048_576.0)
}
