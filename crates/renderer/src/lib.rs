//! Native GPU and editor-UI infrastructure for Mundaris.
//!
//! This crate owns the presentation surface and its disposable frame resources.
//! It does not own authoritative world or simulation state.

#![forbid(unsafe_code)]

mod celestial;
mod celestial_lines;
mod celestial_view;
#[cfg(feature = "surface-profile")]
mod cpu_profile;
mod debug;
mod gpu_profile;
#[cfg(feature = "developer-tools")]
pub mod native_capture;
pub mod sky;
pub mod terrain_atlas;
#[cfg(feature = "terrain-capture")]
pub mod terrain_capture;
mod view;
pub use celestial::*;
pub use celestial_lines::{CelestialLineStyle, CelestialPolyline, PolylinePreparationReport};
pub use celestial_view::*;
#[cfg(feature = "surface-profile")]
pub use cpu_profile::CpuStageTimer;
pub use debug::{DebugFrame, DebugLine, DebugProjection, DebugStaging};
pub use gpu_profile::{
    CpuUploadProfile, GpuProfile, TimestampAvailability, TimestampProfilingMetrics,
};
pub use terrain_atlas::{
    ATLAS_BOUNDS_GRID, AtlasBounds, AtlasChart, AtlasFieldsConstants, AtlasImageLevel,
    AtlasInstance, AtlasProduceJob, AtlasProfileLayer, AtlasSampleSource, AtlasSource,
    AtlasTileKind, MAX_ATLAS_JOBS_PER_FRAME, ProducedTileReadback, TerrainAtlasConfig,
    TerrainAtlasFrame, TerrainAtlasReport, TerrainViewMode, produce_for_validation,
};
pub use view::*;

use std::sync::Arc;

use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use tracing::{info, warn};

use winit::{event::WindowEvent, window::Window};

/// Failures that can occur while preparing or presenting a native renderer.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    #[error(transparent)]
    Preparation(#[from] RenderPreparationError),
    /// The window surface could not be created.
    #[error("creating the window presentation surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    /// No compatible GPU adapter was available.
    #[error("requesting a compatible GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    /// The GPU rejected the requested device configuration.
    #[error("requesting a GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// The adapter does not expose any compatible presentation format.
    #[error("the selected adapter exposes no surface formats")]
    NoSurfaceFormats,
    /// The adapter does not expose any compatible alpha mode.
    #[error("the selected adapter exposes no surface alpha modes")]
    NoSurfaceAlphaModes,
    /// The GPU ran out of memory while acquiring a presentation frame.
    #[error("GPU ran out of memory while acquiring a frame")]
    OutOfMemory,
    #[error("renderer submission identity space is exhausted")]
    SubmissionIdExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderSkipReason {
    NotRenderedYet,
    Suspended,
    GpuPollFailed,
    SurfaceLost,
    SurfaceOutdated,
    SurfaceTimeout,
    SurfaceOther,
    OutOfMemory,
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

/// Nonoverlapping CPU wall scopes for native rendering; scene encoding includes
/// resident preparation. Submission IDs associate these scopes with GPU samples.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NativeRenderTimings {
    pub poll_ms: f64,
    pub acquire_ms: f64,
    pub ui_prepare_ms: f64,
    /// Includes resident preparation and native scene command encoding.
    pub scene_encode_ms: f64,
    pub ui_encode_ms: f64,
    pub submit_ms: f64,
    pub present_ms: f64,
    pub total_ms: f64,
}

/// Owns the native presentation surface, GPU device, and minimal egui integration.
pub struct Renderer {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    egui_context: egui::Context,
    egui_state: EguiWinitState,
    egui_renderer: egui_wgpu::Renderer,
    window: Arc<Window>,
    suspended: bool,
    debug: Option<debug::DebugRenderer>,
    celestial: Option<celestial::CelestialRenderer>,
    timestamp_availability: TimestampAvailability,
    timestamp_slot: Option<gpu_profile::AsyncTimestampSlot>,
    last_render_outcome: RenderOutcome,
    submission_id: u64,
    native_render_timings: NativeRenderTimings,
    #[cfg(feature = "developer-tools")]
    surface_copy_src_supported: bool,
    #[cfg(feature = "developer-tools")]
    native_capture: native_capture::NativeCapture,
    #[cfg(feature = "developer-tools")]
    developer_timestamp_gate: gpu_profile::DeveloperTimestampGate,
}

impl Renderer {
    /// CPU wall scopes for the latest submitted native frame; GPU times are separate.
    pub fn native_render_timings(&self) -> NativeRenderTimings {
        self.native_render_timings
    }

    /// Effective surface configuration, including developer measurement overrides.
    pub fn presentation_mode(&self) -> wgpu::PresentMode {
        self.surface_config.present_mode
    }

    pub fn pixels_per_point(&self) -> f32 {
        self.egui_context.pixels_per_point()
    }

    /// Outcome of the most recent render wrapper call.
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
    /// Creates a surface and GPU device for the supplied native window.
    pub fn new(window: Arc<Window>) -> Result<Self, RendererError> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
                apply_limit_buckets: false,
            })
            .await?;
        let adapter_info = adapter.get_info();
        let requested_features = gpu_profile::available_features(&adapter);
        let timestamp_availability = gpu_profile::availability(requested_features);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Mundaris device"),
                required_features: requested_features,
                // The terrain atlas needs more texture-array layers than the
                // portable default; request what the adapter offers, capped.
                required_limits: wgpu::Limits {
                    max_texture_array_layers: adapter.limits().max_texture_array_layers.min(2048),
                    ..wgpu::Limits::default()
                },
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let surface_capabilities = surface.get_capabilities(&adapter);
        #[cfg(feature = "developer-tools")]
        let surface_copy_src_supported = surface_capabilities
            .usages
            .contains(wgpu::TextureUsages::COPY_SRC);
        let format = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .ok_or(RendererError::NoSurfaceFormats)?;
        let alpha_mode = surface_capabilities
            .alpha_modes
            .first()
            .copied()
            .ok_or(RendererError::NoSurfaceAlphaModes)?;
        let uncapped = cfg!(feature = "developer-tools")
            && std::env::var("MUNDARIS_UNCAPPED").is_ok_and(|value| value == "1");
        let present_mode =
            gpu_profile::select_present_mode(&surface_capabilities.present_modes, uncapped);
        let desired_maximum_frame_latency = 2;
        let window_size = window.inner_size();
        let suspended = window_size.width == 0 || window_size.height == 0;
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.width.max(1),
            height: window_size.height.max(1),
            present_mode,
            desired_maximum_frame_latency,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        if !suspended {
            surface.configure(&device, &surface_config);
        }

        let egui_context = egui::Context::default();
        let egui_state = EguiWinitState::new(
            egui_context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());

        info!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            supported_present_modes = ?surface_capabilities.present_modes,
            selected_present_mode = ?present_mode,
            desired_maximum_frame_latency,
            "GPU adapter and presentation initialized"
        );

        let timestamp_slot = gpu_profile::AsyncTimestampSlot::new(&device);
        Ok(Self {
            _instance: instance,
            surface,
            device,
            queue,
            surface_config,
            egui_context,
            egui_state,
            egui_renderer,
            window,
            suspended,
            debug: None,
            celestial: None,
            timestamp_availability,
            timestamp_slot,
            last_render_outcome: RenderOutcome::default(),
            submission_id: 0,
            native_render_timings: NativeRenderTimings::default(),
            #[cfg(feature = "developer-tools")]
            surface_copy_src_supported,
            #[cfg(feature = "developer-tools")]
            native_capture: native_capture::NativeCapture::new(
                adapter_info.name,
                adapter_info.backend,
            ),
            #[cfg(feature = "developer-tools")]
            developer_timestamp_gate: gpu_profile::DeveloperTimestampGate::default(),
        })
    }

    #[cfg(feature = "developer-tools")]
    pub fn enable_native_capture(&mut self) -> Result<(), String> {
        if self.native_capture.is_enabled() {
            return Ok(());
        }
        self.native_capture
            .enable(self.surface_copy_src_supported, self.surface_config.format)?;
        self.surface_config.usage |= wgpu::TextureUsages::COPY_SRC;
        if !self.suspended {
            self.surface.configure(&self.device, &self.surface_config);
        }
        Ok(())
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

    /// Passes native window input to egui and reports whether it needs a repaint.
    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        self.egui_state
            .on_window_event(self.window.as_ref(), event)
            .repaint
    }

    /// Updates presentation dimensions, deferring configuration while minimized.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.suspended = true;
            #[cfg(feature = "developer-tools")]
            self.native_capture.resized();
            return;
        }

        let size_changed =
            self.surface_config.width != width || self.surface_config.height != height;
        if !size_changed && !self.suspended {
            return;
        }
        if size_changed {
            #[cfg(feature = "developer-tools")]
            self.native_capture.resized();
        }
        self.suspended = false;
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        if let Some(debug) = &mut self.debug {
            debug.resize(&self.device, width, height);
        }
        if let Some(celestial) = &mut self.celestial {
            celestial.resize(&self.device, width, height);
        }
    }

    /// Clears and presents a frame containing UI supplied by the application.
    pub fn render(
        &mut self,
        ui: impl FnMut(&egui::Context, &mut egui::Ui),
    ) -> Result<(), RendererError> {
        self.render_frame(None, None, ui)
    }

    /// Draws a completely validated, view-bound debug frame before application UI.
    pub fn render_debug(
        &mut self,
        frame: &DebugFrame<'_, '_, '_>,
        ui: impl FnMut(&egui::Context, &mut egui::Ui),
    ) -> Result<(), RendererError> {
        self.render_frame(Some(frame), None, ui)
    }

    pub fn render_celestial(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
        ui: impl FnMut(&egui::Context, &mut egui::Ui),
    ) -> Result<(), RendererError> {
        if let Err(error) = frame.validate() {
            self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
            return Err(error.into());
        }
        self.render_frame(None, Some(frame), ui)
    }

    fn render_frame(
        &mut self,
        debug_frame: Option<&DebugFrame<'_, '_, '_>>,
        celestial_frame: Option<&CelestialFrame<'_, '_, '_>>,
        mut ui: impl FnMut(&egui::Context, &mut egui::Ui),
    ) -> Result<(), RendererError> {
        let render_clock = std::time::Instant::now();
        self.native_render_timings = NativeRenderTimings::default();
        self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
        if self.suspended {
            self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::Suspended);
            return Ok(());
        }

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
        let acquire_clock = std::time::Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.surface_config);
                self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::SurfaceLost);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.surface_config);
                self.last_render_outcome =
                    RenderOutcome::Skipped(RenderSkipReason::SurfaceOutdated);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                warn!("timed out acquiring the next presentation frame");
                self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::SurfaceTimeout);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                warn!("surface could not acquire a frame; retrying on the next redraw");
                self.last_render_outcome = RenderOutcome::Skipped(RenderSkipReason::SurfaceOther);
                return Ok(());
            }
        };

        self.native_render_timings.acquire_ms = acquire_clock.elapsed().as_secs_f64() * 1000.0;
        let ui_clock = std::time::Instant::now();

        let Some(submission_id) = self.submission_id.checked_add(1) else {
            self.last_render_outcome =
                RenderOutcome::Skipped(RenderSkipReason::SubmissionIdExhausted);
            return Err(RendererError::SubmissionIdExhausted);
        };

        let raw_input = self.egui_state.take_egui_input(self.window.as_ref());
        let full_output = self.egui_context.run_ui(raw_input, |root_ui| {
            let context = root_ui.ctx().clone();
            ui(&context, root_ui);
        });
        self.egui_state
            .handle_platform_output(self.window.as_ref(), full_output.platform_output);

        let paint_jobs = self
            .egui_context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (texture_id, image_deltas) in &full_output.textures_delta.set {
            for image_delta in image_deltas {
                self.egui_renderer.update_texture(
                    &self.device,
                    &self.queue,
                    *texture_id,
                    image_delta,
                );
            }
        }

        let screen_descriptor = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Mundaris frame encoder"),
            });
        let frame_timing = timestamp_active
            && self
                .timestamp_slot
                .as_ref()
                .is_some_and(|slot| slot.queries.inside_encoders());
        let extra_command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        let frame_start = if frame_timing {
            let mut timing_encoder =
                self.device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Mundaris frame timing start"),
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

        self.native_render_timings.ui_prepare_ms = ui_clock.elapsed().as_secs_f64() * 1000.0;
        let scene_clock = std::time::Instant::now();
        if let Some(frame) = debug_frame {
            let debug = self.debug.get_or_insert_with(|| {
                debug::DebugRenderer::new(
                    &self.device,
                    self.surface_config.format,
                    self.surface_config.width,
                    self.surface_config.height,
                )
            });
            if let Err(error) = debug.draw(&self.device, &self.queue, &mut encoder, &view, frame) {
                self.last_render_outcome =
                    RenderOutcome::Skipped(RenderSkipReason::PreparationFailed);
                return Err(error.into());
            }
        }
        let mut scope_mask = 0;
        if let Some(frame) = celestial_frame {
            let celestial = self.celestial.get_or_insert_with(|| {
                celestial::CelestialRenderer::new(
                    &self.device,
                    &self.queue,
                    self.surface_config.format,
                    self.surface_config.width,
                    self.surface_config.height,
                )
            });
            let draw_result = celestial.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
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
        let ui_encode_clock = std::time::Instant::now();
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Mundaris frame clear and editor UI"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if debug_frame.is_some() || celestial_frame.is_some() {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.025,
                                g: 0.035,
                                b: 0.06,
                                a: 1.0,
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        #[cfg(feature = "developer-tools")]
        self.native_capture.encode_copy(
            &self.device,
            &mut encoder,
            &frame.texture,
            self.surface_config.format,
            self.surface_config.width,
            self.surface_config.height,
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
        self.native_render_timings.ui_encode_ms = ui_encode_clock.elapsed().as_secs_f64() * 1000.0;
        let submit_clock = std::time::Instant::now();
        self.queue.submit(
            frame_start
                .into_iter()
                .chain(extra_command_buffers)
                .chain([encoder.finish()]),
        );
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
        let present_clock = std::time::Instant::now();
        // Wayland uses this notification to coordinate compositor frame callbacks.
        self.window.pre_present_notify();
        self.queue.present(frame);
        self.native_render_timings.present_ms = present_clock.elapsed().as_secs_f64() * 1000.0;
        self.native_render_timings.total_ms = render_clock.elapsed().as_secs_f64() * 1000.0;
        self.last_render_outcome = RenderOutcome::Submitted {
            submission_id,
            presentation_requested: true,
        };

        for texture_id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(texture_id);
        }

        Ok(())
    }
}
