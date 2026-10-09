//! Window surface on the shared wgpu device: configuration, resize and frame
//! acquisition. FIFO presentation paces the Studio at the display rate.

use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use mundaris_renderer::GpuContext;
use tracing::{info, warn};
use winit::window::Window;

pub struct WindowSurface {
    surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

impl WindowSurface {
    pub fn new(gpu: &GpuContext, window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let surface = gpu
            .instance
            .create_surface(window)
            .context("creating the window surface")?;
        // The shared device was created without a surface; confirm this one fits it.
        ensure!(
            gpu.adapter.is_surface_supported(&surface),
            "the selected GPU adapter cannot present to this window"
        );
        let capabilities = surface.get_capabilities(&gpu.adapter);
        // egui blends in gamma space and expects a non-sRGB framebuffer.
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .context("the window surface offers no format")?;
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .context("the window surface offers no alpha mode")?;
        let uncapped = cfg!(feature = "developer-tools")
            && std::env::var("MUNDARIS_UNCAPPED").is_ok_and(|value| value == "1");
        let present_mode = if uncapped {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        if size.width > 0 && size.height > 0 {
            surface.configure(&gpu.device, &config);
        }
        info!(?format, ?present_mode, "Studio window surface configured");
        Ok(Self { surface, config })
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if (self.config.width, self.config.height) != (width, height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(device, &self.config);
        }
    }

    /// Next presentable texture; `None` skips this frame (lost, outdated,
    /// occluded or timed out surfaces are reconfigured or retried next frame).
    pub fn acquire(&mut self, device: &wgpu::Device) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => Some(frame),
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(device, &self.config);
                None
            }
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => None,
            wgpu::CurrentSurfaceTexture::Validation => {
                warn!("surface validation error while acquiring a frame");
                None
            }
        }
    }
}
