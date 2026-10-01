//! Native GPU and editor-UI infrastructure for Mundaris.
//!
//! This crate owns the presentation surface and its disposable frame resources.
//! It does not own authoritative world or simulation state.

#![forbid(unsafe_code)]

use std::sync::Arc;

use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiWinitState;
use tracing::{info, warn};
use wgpu::SurfaceError;
use winit::{event::WindowEvent, window::Window};

/// Failures that can occur while preparing or presenting a native renderer.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
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
}

impl Renderer {
    /// Creates a surface and GPU device for the supplied native window.
    pub fn new(window: Arc<Window>) -> Result<Self, RendererError> {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(Arc::clone(&window))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await?;
        let adapter_info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Mundaris device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await?;

        let surface_capabilities = surface.get_capabilities(&adapter);
        let format = surface_capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| surface_capabilities.formats.first().copied())
            .ok_or(RendererError::NoSurfaceFormats)?;
        let alpha_mode = surface_capabilities
            .alpha_modes
            .first()
            .copied()
            .ok_or(RendererError::NoSurfaceAlphaModes)?;
        let window_size = window.inner_size();
        let suspended = window_size.width == 0 || window_size.height == 0;
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.width.max(1),
            height: window_size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        let egui_context = egui::Context::default();
        let egui_state = EguiWinitState::new(
            egui_context.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());

        info!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            "GPU adapter initialized"
        );

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
        })
    }

    /// Passes native window input to egui and reports whether it consumed the event.
    pub fn on_window_event(&mut self, event: &WindowEvent) -> egui_winit::EventResponse {
        self.egui_state.on_window_event(self.window.as_ref(), event)
    }

    /// Updates presentation dimensions, deferring configuration while minimized.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            self.suspended = true;
            return;
        }

        self.suspended = false;
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Draws the clear frame and small bootstrap UI, then presents it.
    pub fn render(&mut self) -> Result<(), RendererError> {
        if self.suspended {
            return Ok(());
        }

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            Err(SurfaceError::Timeout) => {
                warn!("timed out acquiring the next presentation frame");
                return Ok(());
            }
            Err(SurfaceError::OutOfMemory) => {
                return Err(RendererError::OutOfMemory);
            }
            Err(SurfaceError::Other) => {
                warn!("surface could not acquire a frame; retrying on the next redraw");
                return Ok(());
            }
        };

        let raw_input = self.egui_state.take_egui_input(self.window.as_ref());
        let full_output = self.egui_context.run(raw_input, |context| {
            egui::Window::new("Mundaris")
                .collapsible(false)
                .resizable(false)
                .show(context, |ui| {
                    ui.label("Bootstrap environment");
                    ui.label("Renderer initialized");
                });
        });
        self.egui_state
            .handle_platform_output(self.window.as_ref(), full_output.platform_output);

        let paint_jobs = self
            .egui_context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (texture_id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *texture_id, image_delta);
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
        let extra_command_buffers = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Mundaris clear and UI pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            });
            self.egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        self.queue
            .submit(extra_command_buffers.into_iter().chain([encoder.finish()]));
        frame.present();

        for texture_id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(texture_id);
        }

        Ok(())
    }
}
