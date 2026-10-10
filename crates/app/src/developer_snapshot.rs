//! Canonical, read-only developer state shared by native summaries and capture tools.
//! Values use SI units; absent measurements are `None`, never inferred zero.

use anyhow::{Result, ensure};
use astrum_math::FramePose;
use astrum_renderer::{CelestialProjection, GpuProfile};
use astrum_world::{BodyId, CoherentCelestialView};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    celestial_camera::{CameraMode, NavigationDiagnostics},
    motion_session::MotionSnapshot,
};

/// Version of the JSON contract, independent of engine/world persistence formats.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 8;
/// UI-only advisory ratio. This does not alter admission or the terrain cap.
pub const MEMORY_NEAR_CAP_RATIO: f64 = 0.95;

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
    /// Active geometry backend, independent of requested detail.
    #[serde(default)]
    pub backend: String,
    pub active_body: Option<BodySnapshot>,
    /// Selected world authority, independent of currently ready mesh quality.
    #[serde(default)]
    pub generator_algorithm: Option<String>,
    /// The proof used for mesh quality, rather than a sampled quality estimate.
    #[serde(default)]
    pub certificate_kind: Option<String>,
    /// Mesh demand can be a resolution guide while complete quality is unproven.
    #[serde(default)]
    pub refinement_demand_kind: Option<String>,
    #[serde(default)]
    pub target_certifiable: Option<bool>,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RenderingSnapshot {
    pub terrain_render_mode: String,
    pub terrain_enabled: bool,
    pub patch_borders_enabled: bool,
    pub lod_colors_enabled: bool,
    pub navigation_markers_enabled: bool,
}

/// Native asynchronous sampling counts; reservations can abort before submission.
/// A busy skip identifies query availability, not expensive skipped work.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimestampSamplingSnapshot {
    pub explicit_requests: u64,
    pub eligible_frames: u64,
    pub accepted_reservations: u64,
    pub actual_submissions: u64,
    pub valid_completions: u64,
    pub busy_skips: u64,
    pub map_failures: u64,
    pub decode_failures: u64,
    pub invalid_samples: u64,
    pub last_busy_skip_candidate_submission_id: Option<u64>,
    pub last_busy_skip_source_submission_id: Option<u64>,
    pub last_submitted_source_submission_id: Option<u64>,
    pub last_completed_source_submission_id: Option<u64>,
    pub last_map_failure_source_submission_id: Option<u64>,
    pub last_decode_failure_source_submission_id: Option<u64>,
    pub last_invalid_sample_source_submission_id: Option<u64>,
}

impl From<astrum_renderer::TimestampProfilingMetrics> for TimestampSamplingSnapshot {
    fn from(metrics: astrum_renderer::TimestampProfilingMetrics) -> Self {
        Self {
            explicit_requests: metrics.explicit_requests,
            eligible_frames: metrics.eligible_frames,
            accepted_reservations: metrics.accepted_reservations,
            actual_submissions: metrics.actual_submissions,
            valid_completions: metrics.valid_completions,
            busy_skips: metrics.busy_skips,
            map_failures: metrics.map_failures,
            decode_failures: metrics.decode_failures,
            invalid_samples: metrics.invalid_samples,
            last_busy_skip_candidate_submission_id: metrics.last_busy_skip_candidate_submission_id,
            last_busy_skip_source_submission_id: metrics.last_busy_skip_source_submission_id,
            last_submitted_source_submission_id: metrics.last_submitted_source_submission_id,
            last_completed_source_submission_id: metrics.last_completed_source_submission_id,
            last_map_failure_source_submission_id: metrics.last_map_failure_source_submission_id,
            last_decode_failure_source_submission_id: metrics
                .last_decode_failure_source_submission_id,
            last_invalid_sample_source_submission_id: metrics
                .last_invalid_sample_source_submission_id,
        }
    }
}

/// CPU stages are host elapsed times. GPU regular terrain and fallback are separate.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PerformanceSnapshot {
    /// Scene renderer CPU scopes ordered poll, scene encode, submit, total.
    /// UI composition and presentation run in the UI toolkit and are excluded.
    #[serde(default)]
    pub native_render_cpu_ms: Option<[f64; 4]>,
    #[serde(default)]
    pub native_submission_id: Option<u64>,
    /// Cumulative nonblocking native timestamp sampling counters, with source IDs.
    #[serde(default)]
    pub native_gpu_timestamp_sampling: Option<TimestampSamplingSnapshot>,
    #[serde(default)]
    pub native_presentation_mode: Option<String>,
    #[serde(default)]
    pub native_redraw_uncapped: bool,
    /// Host cost of bounded profiler publication/capture bookkeeping.
    #[serde(default)]
    pub profiler_publication_ms: Option<f64>,
    pub frame_cpu_ms: Option<f64>,
    /// Entire host frame, including UI, driver/acquire/present and diagnostics.
    /// This is elapsed host time, not a GPU execution duration or CPU-only time.
    #[serde(default)]
    pub host_frame_ms: Option<f64>,
    /// Elapsed render submission, UI, acquire and present; includes resident preparation.
    #[serde(default)]
    pub render_present_ms: Option<f64>,
    /// Publication of the compact diagnostic snapshot, including previous snapshot retirement.
    #[serde(default)]
    pub diagnostics_ms: Option<f64>,
    #[serde(default)]
    pub ui_build_cpu_ms: Option<f64>,
    /// CPU wall time the UI toolkit spent rendering the previous frame
    /// (between its before- and after-rendering notifications).
    #[serde(default)]
    pub ui_render_ms: Option<f64>,
    pub update_ms: Option<f64>,
    pub terrain_update_ms: Option<f64>,
    pub preparation_ms: Option<f64>,
    pub terrain_preparation_ms: Option<f64>,
    /// Host validation/upload/draw preparation for resident GPU terrain.
    #[serde(default)]
    pub gpu_preparation_cpu_ms: Option<f64>,
    pub gpu_terrain_ms: Option<f64>,
    #[serde(default)]
    pub gpu_frame_ms: Option<f64>,
    #[serde(default)]
    pub gpu_main_pass_ms: Option<f64>,
    #[serde(default)]
    pub gpu_overlay_ms: Option<f64>,
    pub gpu_transition_fallback_ms: Option<f64>,
    pub gpu_timing_scope: String,
    #[serde(default)]
    pub gpu_timestamp_capability: String,
    #[serde(default)]
    pub gpu_source_frame: Option<u64>,
    /// Timestamped scopes of the same GPU sample, relative to its GPU frame start.
    #[serde(default)]
    pub gpu_scopes: Vec<GpuScopeSnapshot>,
    pub upload_bytes: Option<u64>,
    /// Plants the cull pass dropped because their region was full (latest
    /// read-back): grown-flora buckets and procedural far plants. Zero means
    /// nothing was capped (flora lane overflow counter).
    #[serde(default)]
    pub flora_dropped_grown: Option<u32>,
    #[serde(default)]
    pub flora_dropped_procedural: Option<u32>,
}

/// One GPU timestamp scope on the sampled frame's GPU clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuScopeSnapshot {
    pub name: String,
    pub depth: u8,
    pub start_ms: f64,
    pub end_ms: f64,
}
impl PerformanceSnapshot {
    /// Native uses `latest_completed`.
    pub fn with_gpu(mut self, profile: GpuProfile, scope: &str) -> Self {
        self.gpu_frame_ms = profile.frame.map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_main_pass_ms = profile.celestial_pass.map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_overlay_ms = profile.overlay_pass.map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_terrain_ms = profile.terrain.map(|d| d.as_secs_f64() * 1000.0);
        let (grown, procedural) = astrum_renderer::flora_overflow();
        self.flora_dropped_grown = Some(grown);
        self.flora_dropped_procedural = Some(procedural);
        self.gpu_transition_fallback_ms = profile
            .transition_fallback
            .map(|d| d.as_secs_f64() * 1000.0);
        self.gpu_scopes = profile
            .scopes
            .iter()
            .flatten()
            .map(|scope| GpuScopeSnapshot {
                name: scope.name.into(),
                depth: scope.depth,
                start_ms: scope.start.as_secs_f64() * 1000.0,
                end_ms: scope.end.as_secs_f64() * 1000.0,
            })
            .collect();
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
    /// Loaded authored scene/camera/terrain bytes shared by user and replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_scene: Option<Value>,
    #[serde(default)]
    pub motion: Option<MotionSnapshot>,
    #[serde(default)]
    pub development: Option<DevelopmentSnapshot>,
    /// Atlas terrain runtime (ADR 0016).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain_atlas: Option<Value>,
    /// Render registry listing and shadow cascade state
    /// (docs/RENDER_PIPELINE_HDR.md §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_settings: Option<Value>,
    /// Prepared authority identity; no sampling or generation during collection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prepared_sources: Vec<Value>,
    /// Bounded sampled CPU timeline; timestamps use its own monotonic epoch.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "shared_profile"
    )]
    pub engine_profile: Option<std::sync::Arc<Value>>,
}

mod shared_profile {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(
        value: &Option<Arc<Value>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_deref().serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Arc<Value>>, D::Error> {
        Option::<Value>::deserialize(deserializer).map(|value| value.map(Arc::new))
    }
}

/// Session and frame provenance. A presentation request is not monitor evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevelopmentSnapshot {
    pub session_id: String,
    pub observation_age_ms: f64,
    pub stale: bool,
    pub drawable: bool,
    pub command_sequence: u64,
    pub prepared_frame: u64,
    pub submitted_frame: Option<u64>,
    pub presentation_requested: bool,
    pub capture_id: Option<String>,
    pub current_errors: Vec<String>,
    pub asynchronous_measurement_source: String,
    pub gpu_measurement_source_frame: Option<u64>,
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
    pub terrain_clearance_m: Option<f64>,
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
                drawn_mesh_clearance_m: None,
                fov_y_degrees: input.projection.vertical_fov_rad().to_degrees(),
                near_plane_m: input.projection.near_m(),
                viewport_origin_pixels: input.projection.origin(),
                viewport_size_pixels: input.projection.viewport(),
                navigation: input.navigation,
            },
            // Filled by the atlas runtime (`PlanetLod::annotate_terrain`).
            terrain: TerrainSnapshot::default(),
            work: WorkSnapshot::default(),
            memory: MemorySnapshot::default(),
            rendering: input.rendering,
            performance: input.performance,
            warnings: Vec::new(),
            capture: None,
            motion: input.motion,
            development: None,
            terrain_atlas: None,
            render_settings: None,
            prepared_sources: world
                .bodies()
                .enumerate()
                .filter_map(|(index, (_, body))| {
                    let definition = body.surface_definition()?;
                    let source = definition.prepared()?;
                    Some(serde_json::json!({
                        "body_index": index, "body_name": body.name(),
                        "algorithm": "PreparedV1", "authority_level": 0,
                        "content_identity": source.content_identity(),
                        "authored_revision": source.revision(),
                        "displacement_bounds_m": source.displacement_bounds_m(),
                        "retained_source_payload_bytes": source.resident_bytes(),
                        "coordinate_space": "body_local_unit_direction"
                    }))
                })
                .collect(),
            engine_profile: None,
            shared_scene: None,
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
