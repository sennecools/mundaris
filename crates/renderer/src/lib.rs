//! Native GPU rendering for Astrum.
//!
//! This crate renders the scene into an offscreen texture that the application
//! presents inside its UI, and owns the disposable frame resources.
//! It does not own authoritative world or simulation state.

#![forbid(unsafe_code)]

mod celestial;
mod celestial_lines;
mod celestial_view;
#[cfg(feature = "surface-profile")]
mod cpu_profile;
mod debug;
mod gpu_profile;
mod lighting;
#[cfg(feature = "developer-tools")]
pub mod native_capture;
mod post;
mod render_settings;
mod shadows;
pub mod sky;
pub mod terrain_atlas;
pub mod tier_a;
#[cfg(feature = "terrain-capture")]
pub mod terrain_capture;
mod view;
pub use celestial::*;
pub use celestial_lines::{
    CelestialLineStyle, CelestialPolyline, LineStyleScale, PolylinePreparationReport,
};
pub use celestial_view::*;
#[cfg(feature = "surface-profile")]
pub use cpu_profile::CpuStageTimer;
pub use debug::{DebugFrame, DebugLine, DebugProjection, DebugStaging};
pub use gpu_profile::{
    CpuUploadProfile, GpuProfile, GpuScopeSpan, TimestampAvailability, TimestampProfilingMetrics,
};
pub use lighting::{
    Brdf, Cascades, FrameLighting, MAX_OCCLUDERS, ShadowView, SurfaceMaterial,
    disk_visible_fraction, fit_cascades, shadow_range, slice_sphere,
};
pub use render_settings::{
    AoSettings, BloomSettings, ExposureMode, ExposureSettings, LightingSettings, MAX_CASCADES,
    OverlaySettings, RenderSettings, SHADOW_RESOLUTIONS, ShadowSettings, Tonemapper,
};
pub use terrain_atlas::{
    ATLAS_BOUNDS_GRID, AtlasBounds, AtlasChart, AtlasCollisionPage, AtlasFieldsConstants,
    AtlasImageLevel, AtlasInstance, AtlasOctave, AtlasProduceJob, AtlasProfileLayer,
    AtlasSampleSource, AtlasShadowFrame, AtlasSource, AtlasTileKind, AtlasWater, AtlasWorldSource, AtlasWorldSurface, MAX_ATLAS_JOBS_PER_FRAME,
    MAX_ATLAS_OCTAVES, MAX_COLLISION_CELLS, MAX_COLLISION_JOBS_PER_FRAME, ProducedTileReadback,
    TerrainAtlasConfig, TerrainAtlasFrame, TerrainAtlasReport, TerrainViewMode,
    collision_for_validation, lattice_hash_for_validation, produce_for_validation,
};
pub use view::*;

use tracing::info;

/// Failures that can occur while creating the GPU context or rendering a frame.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    #[error(transparent)]
    Preparation(#[from] RenderPreparationError),
    /// No compatible GPU adapter was available.
    #[error("requesting a compatible GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    /// The GPU rejected the requested device configuration.
    #[error("requesting a GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("renderer submission identity space is exhausted")]
    SubmissionIdExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderSkipReason {
    NotRenderedYet,
    Suspended,
    GpuPollFailed,
    PreparationFailed,
    SubmissionIdExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderOutcome {
    Skipped(RenderSkipReason),
    Submitted {
        submission_id: u64,
        presentation_requested: bool,
    },
}

impl Default for RenderOutcome {
    fn default() -> Self {
        Self::Skipped(RenderSkipReason::NotRenderedYet)
    }
}

/// Nonoverlapping CPU wall scopes for scene rendering. Submission IDs associate
/// these scopes with GPU samples. UI composition and presentation belong to the
/// application's UI toolkit and are not included.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NativeRenderTimings {
    pub poll_ms: f64,
    pub scene_encode_ms: f64,
    pub submit_ms: f64,
    pub total_ms: f64,
}

/// One GPU instance, adapter, device and queue shared by the scene renderer and
/// the application's UI toolkit. All handles are cheap clones of the same objects.
#[derive(Clone)]
pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    timestamp_availability: TimestampAvailability,
}

impl GpuContext {
    /// Creates the high-performance device with the limits and features the
    /// scene renderer needs (texture-array layers for the terrain atlas,
    /// timestamp queries when available).
    pub fn new() -> Result<Self, RendererError> {
        pollster::block_on(Self::new_async(false))
    }

    /// The platform's software fallback adapter (WARP on Windows): the full
    /// production path for validation runs that must not occupy the GPU.
    pub fn new_software() -> Result<Self, RendererError> {
        pollster::block_on(Self::new_async(true))
    }

    async fn new_async(fallback: bool) -> Result<Self, RendererError> {
        // WGPU_BACKEND and related variables may select a backend for diagnosis.
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: fallback,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await?;
        let adapter_info = adapter.get_info();
        let requested_features = gpu_profile::available_features(&adapter);
        let timestamp_availability = gpu_profile::availability(requested_features);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Astrum device"),
                required_features: requested_features,
                // The terrain atlas needs more texture-array layers than the
                // portable default; request what the adapter offers, capped.
                required_limits: wgpu::Limits {
                    max_texture_array_layers: adapter.limits().max_texture_array_layers.min(2048),
                    // Tier A bakes of large bodies (1024² per face) need about
                    // 300 MB of storage; the portable default is 128 MiB.
                    max_storage_buffer_binding_size: adapter
                        .limits()
                        .max_storage_buffer_binding_size
                        .min(512 << 20),
                    max_buffer_size: adapter.limits().max_buffer_size.min(512 << 20),
                    ..wgpu::Limits::default()
                },
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;
        info!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            "GPU adapter and device initialized"
        );
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            timestamp_availability,
        })
    }
}

/// Colour format of the scene texture handed to the UI. The scene is rendered
/// through an sRGB view so stored bytes are display-encoded.
pub const SCENE_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SCENE_RENDER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

struct SceneTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl SceneTarget {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Scene viewport colour"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SCENE_TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[SCENE_RENDER_FORMAT],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Scene viewport sRGB render view"),
            format: Some(SCENE_RENDER_FORMAT),
            ..Default::default()
        });
        Self {
            texture,
            view,
            width,
            height,
        }
    }
}

/// Renders the scene into an offscreen viewport texture. Presentation, window
/// ownership and UI belong to the application.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Option<SceneTarget>,
    debug: Option<debug::DebugRenderer>,
    celestial: Option<celestial::CelestialRenderer>,
    timestamp_availability: TimestampAvailability,
    timestamp_slot: Option<gpu_profile::AsyncTimestampSlot>,
    last_render_outcome: RenderOutcome,
    submission_id: u64,
    native_render_timings: NativeRenderTimings,
    settings: RenderSettings,
    #[cfg(feature = "developer-tools")]
    native_capture: native_capture::NativeCapture,
    #[cfg(feature = "developer-tools")]
    developer_timestamp_gate: gpu_profile::DeveloperTimestampGate,
}

impl Renderer {
    pub fn new(context: &GpuContext) -> Self {
        #[cfg(feature = "developer-tools")]
        let adapter_info = context.adapter.get_info();
        Self {
            device: context.device.clone(),
            queue: context.queue.clone(),
            target: None,
            debug: None,
            celestial: None,
            timestamp_availability: context.timestamp_availability,
            timestamp_slot: gpu_profile::AsyncTimestampSlot::new(&context.device),
            last_render_outcome: RenderOutcome::default(),
            submission_id: 0,
            native_render_timings: NativeRenderTimings::default(),
            settings: RenderSettings::default(),
            #[cfg(feature = "developer-tools")]
            native_capture: native_capture::NativeCapture::new(
                adapter_info.name,
                adapter_info.backend,
            ),
            #[cfg(feature = "developer-tools")]
            developer_timestamp_gate: gpu_profile::DeveloperTimestampGate::default(),
        }
    }

    /// Applies render settings from the next frame on. Invalid settings are
    /// rejected and the previous settings stay in effect.
    pub fn set_render_settings(&mut self, settings: RenderSettings) -> Result<(), String> {
        settings.validate()?;
        if settings != self.settings {
            self.settings = settings;
            if let Some(celestial) = &mut self.celestial {
                celestial.set_settings(&self.device, settings);
            }
        }
        Ok(())
    }

    pub fn render_settings(&self) -> RenderSettings {
        self.settings
    }

    /// Cascades and caster counts of the latest celestial frame.
    pub fn shadow_report(&self) -> ShadowReport {
        self.celestial
            .as_ref()
            .map_or_else(Default::default, |c| c.shadow_report())
    }

    /// CPU wall scopes for the latest submitted scene frame; GPU times are separate.
    pub fn native_render_timings(&self) -> NativeRenderTimings {
        self.native_render_timings
    }

    /// Outcome of the most recent render call.
    pub fn last_render_outcome(&self) -> RenderOutcome {
        self.last_render_outcome
    }

    /// Timestamp-query support enabled for this adapter.
    pub fn timestamp_availability(&self) -> TimestampAvailability {
        self.timestamp_availability
    }

    /// Latest completed nonblocking GPU query profile. A frame skips timestamp
    /// submission while its single bounded readback slot is busy.
    pub fn latest_gpu_profile(&self) -> gpu_profile::GpuProfile {
        self.timestamp_slot
            .as_ref()
            .map_or_else(Default::default, |slot| slot.latest)
    }

    /// Submission that produced `latest_gpu_profile`, when timestamp readback is complete.
    pub fn latest_gpu_profile_submission(&self) -> Option<u64> {
        self.timestamp_slot
            .as_ref()
            .and_then(gpu_profile::AsyncTimestampSlot::latest_submission_id)
    }

    /// Low-cost accounting for explicit requests, query reservations, readback,
    /// and their source submission IDs.
    pub fn timestamp_profiling_metrics(&self) -> TimestampProfilingMetrics {
        self.timestamp_slot
            .as_ref()
            .map_or_else(Default::default, gpu_profile::AsyncTimestampSlot::metrics)
    }

    /// Suppress automatic new timestamp queries while developer diagnostics are idle.
    /// Any query already in flight continues to be polled without waiting.
    #[cfg(feature = "developer-tools")]
    pub fn set_developer_observation_mode(&mut self, enabled: bool) {
        self.developer_timestamp_gate.set_observation_mode(enabled);
    }

    /// Arm one query for the next submitted celestial frame. Returns false when
    /// timestamp queries are unavailable on the selected adapter.
    #[cfg(feature = "developer-tools")]
    pub fn request_developer_gpu_timing(&mut self) -> bool {
        if self.timestamp_slot.is_none() {
            return false;
        }
        self.developer_timestamp_gate.request();
        if let Some(slot) = &mut self.timestamp_slot {
            slot.record_explicit_request();
        }
        true
    }

    /// Atlas sources (keys) whose Tier A world-map bake completed in the last
    /// recorded frame.
    pub fn take_terrain_ready_sources(&mut self) -> Vec<(u64, Option<String>)> {
        self.celestial
            .as_mut()
            .map_or_else(Vec::new, |c| c.take_ready_sources())
    }
    /// Collision pages read back since the last call (pipeline §15.1).
    pub fn take_terrain_collision_pages(&mut self) -> Vec<AtlasCollisionPage> {
        self.celestial
            .as_mut()
            .map_or_else(Vec::new, |c| c.take_collision_pages())
    }
    /// Produced atlas height bounds delivered since the last call.
    pub fn take_terrain_atlas_bounds(&mut self) -> Vec<AtlasBounds> {
        self.celestial
            .as_mut()
            .map_or_else(Vec::new, |c| c.take_atlas_bounds())
    }
    pub fn terrain_atlas_report(&self) -> TerrainAtlasReport {
        self.celestial
            .as_ref()
            .map_or_else(Default::default, |c| c.atlas_report())
    }
    /// Device limit on atlas texture-array layers.
    pub fn terrain_atlas_layer_limit(&self) -> u32 {
        self.device.limits().max_texture_array_layers
    }
    /// Last submitted sky upload accounting; unavailable before first submission.
    pub fn last_sky_resource_report(&self) -> Option<sky::SkyResourceReport> {
        self.celestial.as_ref().and_then(|r| {
            let report = r.last_sky_resource_report();
            (report.catalogue_upload_count > 0).then_some(report)
        })
    }

    /// The scene texture of the latest render, in [`SCENE_TEXTURE_FORMAT`].
    pub fn scene_texture(&self) -> Option<&wgpu::Texture> {
        self.target.as_ref().map(|target| &target.texture)
    }

    /// Current scene viewport size in physical pixels, if allocated.
    pub fn scene_size(&self) -> Option<[u32; 2]> {
        self.target
            .as_ref()
            .map(|target| [target.width, target.height])
    }

    #[cfg(feature = "developer-tools")]
    pub fn enable_native_capture(&mut self) -> Result<(), String> {
        if self.native_capture.is_enabled() {
            return Ok(());
        }
        self.native_capture.enable(true, SCENE_TEXTURE_FORMAT)
    }

    #[cfg(feature = "developer-tools")]
    pub fn request_native_capture(&mut self, capture_id: u64) -> Result<(), String> {
        self.native_capture.request(capture_id)
    }

    #[cfg(feature = "developer-tools")]
    pub fn poll_native_capture(
        &mut self,
    ) -> Result<Option<native_capture::NativeCaptureFrame>, String> {
        self.device
            .poll(wgpu::PollType::Poll)
            .map_err(|error| format!("polling native capture readback: {error}"))?;
        self.native_capture.poll()
    }

    #[cfg(feature = "developer-tools")]
    pub fn cancel_native_capture(&mut self) {
        self.native_capture.cancel();
    }

    /// Sizes the scene texture to the viewport, in physical pixels. A zero size
    /// suspends rendering. A new texture is allocated only when the size changes.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.target = None;
            #[cfg(feature = "developer-tools")]
            self.native_capture.resized();
            return;
        }
        if self
            .target
            .as_ref()
            .is_some_and(|target| target.width == width && target.height == height)
        {
            return;
        }
        #[cfg(feature = "developer-tools")]
        self.native_capture.resized();
        self.target = Some(SceneTarget::new(&self.device, width, height));
        if let Some(debug) = &mut self.debug {
            debug.resize(&self.device, width, height);
        }
        if let Some(celestial) = &mut self.celestial {
            celestial.resize(&self.device, width, height);
        }
    }

    /// Clears the scene texture without drawing scene content.
    pub fn render_empty(&mut self) -> Result<(), RendererError> {
        self.render_frame(None, None)
    }

    /// Draws a completely validated, view-bound debug frame.
    pub fn render_debug(&mut self, frame: &DebugFrame<'_, '_, '_>) -> Result<(), RendererError> {
        self.render_frame(Some(frame), None)
    }

    /// Draws a completely validated celestial frame into the scene texture.
    pub fn render_celestial(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
    ) -> Result<(), RendererError> {
        if let Err(error) = frame.validate() {
            self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
            return Err(error.into());
        }
        self.render_frame(None, Some(frame))
    }

    fn render_frame(
        &mut self,
        debug_frame: Option<&DebugFrame<'_, '_, '_>>,
        celestial_frame: Option<&CelestialFrame<'_, '_, '_>>,
    ) -> Result<(), RendererError> {
        let render_clock = std::time::Instant::now();
        self.native_render_timings = NativeRenderTimings::default();
        let Some(target) = &self.target else {
            self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::Suspended);
            return Ok(());
        };

        if let Err(error) = self.device.poll(wgpu::PollType::Poll) {
            self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::GpuPollFailed);
            return Err(RendererError::Preparation(
                RenderPreparationError::GpuProgress(error.to_string()),
            ));
        }
        let poll_timestamp_slot = celestial_frame.is_some();
        #[cfg(feature = "developer-tools")]
        let poll_timestamp_slot =
            poll_timestamp_slot || self.developer_timestamp_gate.observation_mode();
        let timestamp_slot_idle = poll_timestamp_slot
            && self
                .timestamp_slot
                .as_mut()
                .is_some_and(|slot| slot.available());
        #[cfg(feature = "developer-tools")]
        let timestamp_wanted = self
            .developer_timestamp_gate
            .wants_sample(celestial_frame.is_some());
        #[cfg(feature = "developer-tools")]
        let timestamp_active = self
            .developer_timestamp_gate
            .begin_if_idle(celestial_frame.is_some(), timestamp_slot_idle);
        #[cfg(not(feature = "developer-tools"))]
        let timestamp_wanted = celestial_frame.is_some();
        #[cfg(not(feature = "developer-tools"))]
        let timestamp_active = timestamp_wanted && timestamp_slot_idle;

        if timestamp_wanted && let Some(slot) = &mut self.timestamp_slot {
            slot.record_eligible_frame();
            if timestamp_slot_idle {
                slot.record_accepted_reservation();
            } else {
                slot.record_busy_skip(self.submission_id.checked_add(1));
            }
        }
        self.native_render_timings.poll_ms = render_clock.elapsed().as_secs_f64() * 1000.0;

        let Some(submission_id) = self.submission_id.checked_add(1) else {
            self.last_render_outcome =
                RenderOutcome::Skipped(RenderSkipReason::SubmissionIdExhausted);
            return Err(RendererError::SubmissionIdExhausted);
        };

        let (width, height) = (target.width, target.height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Astrum scene encoder"),
            });
        let frame_timing = timestamp_active
            && self
                .timestamp_slot
                .as_ref()
                .is_some_and(|slot| slot.queries.inside_encoders());
        let frame_start = if frame_timing {
            let mut timing_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Astrum frame timing start"),
                    });
            if let Some(slot) = &self.timestamp_slot {
                timing_encoder.write_timestamp(
                    &slot.queries.set,
                    (gpu_profile::FRAME_QUERY_PAIR * 2) as u32,
                );
            }
            Some(timing_encoder.finish())
        } else {
            None
        };

        let scene_clock = std::time::Instant::now();
        if debug_frame.is_none() && celestial_frame.is_none() {
            // Nothing to draw: clear so the UI never shows stale scene pixels.
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Scene clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.025,
                            g: 0.035,
                            b: 0.06,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        if let Some(frame) = debug_frame {
            let debug = self.debug.get_or_insert_with(|| {
                debug::DebugRenderer::new(&self.device, SCENE_RENDER_FORMAT, width, height)
            });
            if let Err(error) =
                debug.draw(&self.device, &self.queue, &mut encoder, &target.view, frame)
            {
                self.last_render_outcome =
                    RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
                return Err(error.into());
            }
        }
        let mut scope_mask = 0;
        if let Some(frame) = celestial_frame {
            let settings = self.settings;
            let celestial = self.celestial.get_or_insert_with(|| {
                let mut renderer = celestial::CelestialRenderer::new(
                    &self.device,
                    &self.queue,
                    SCENE_RENDER_FORMAT,
                    width,
                    height,
                );
                renderer.set_settings(&self.device, settings);
                renderer
            });
            let draw_result = celestial.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &target.view,
                frame,
                timestamp_active
                    .then(|| self.timestamp_slot.as_ref().map(|slot| &slot.queries))
                    .flatten(),
            );
            match draw_result {
                Ok(mask) => scope_mask = mask,
                Err(error) => {
                    self.last_render_outcome =
                        RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
                    return Err(error.into());
                }
            }
        }
        self.native_render_timings.scene_encode_ms = scene_clock.elapsed().as_secs_f64() * 1000.0;

        #[cfg(feature = "developer-tools")]
        self.native_capture.encode_copy(
            &self.device,
            &mut encoder,
            &target.texture,
            SCENE_TEXTURE_FORMAT,
            width,
            height,
        );

        if frame_timing && let Some(slot) = &self.timestamp_slot {
            encoder.write_timestamp(
                &slot.queries.set,
                (gpu_profile::FRAME_QUERY_PAIR * 2 + 1) as u32,
            );
            scope_mask |= gpu_profile::FRAME_SCOPE_BIT;
        }
        if timestamp_active && let Some(slot) = &self.timestamp_slot {
            slot.resolve(&mut encoder);
        }
        let submit_clock = std::time::Instant::now();
        self.queue
            .submit(frame_start.into_iter().chain([encoder.finish()]));
        if let Some(celestial) = &mut self.celestial {
            celestial.on_submitted();
        }
        self.submission_id = submission_id;
        #[cfg(feature = "developer-tools")]
        self.native_capture.submitted(submission_id);
        if timestamp_active && let Some(slot) = &mut self.timestamp_slot {
            slot.map(self.queue.get_timestamp_period(), scope_mask, submission_id);
            #[cfg(feature = "developer-tools")]
            self.developer_timestamp_gate.submitted();
        }
        self.native_render_timings.submit_ms = submit_clock.elapsed().as_secs_f64() * 1000.0;
        self.native_render_timings.total_ms = render_clock.elapsed().as_secs_f64() * 1000.0;
        self.last_render_outcome = RenderOutcome::Submitted {
            submission_id,
            presentation_requested: true,
        };
        Ok(())
    }
}
