//! Small opt-in headless readback of the production celestial reverse-Z draw path.
//!
//! Capture dimensions are fixed at construction and must match the frame's content
//! viewport. Creating a capture requires a locally available GPU adapter; no GPU
//! integration test runs as part of the ordinary test suite.

use crate::{CelestialFrame, RenderPreparationError, celestial::CelestialRenderer};

/// Offscreen RGBA8 target that submits through the ordinary celestial renderer.
pub struct TerrainCaptureRenderer {
    _instance: wgpu::Instance,
    adapter_name: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: CelestialRenderer,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    last_cpu_encode: std::time::Duration,
}

impl TerrainCaptureRenderer {
    /// Creates a fixed-size offscreen renderer, requesting a headless adapter/device.
    pub fn new(width: u32, height: u32) -> Result<Self, RenderPreparationError> {
        pollster::block_on(Self::new_async(width, height))
    }

    async fn new_async(width: u32, height: u32) -> Result<Self, RenderPreparationError> {
        if width == 0 || height == 0 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        let adapter_name = adapter.get_info().name;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Mundaris terrain capture device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        if width > device.limits().max_texture_dimension_2d
            || height > device.limits().max_texture_dimension_2d
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Terrain capture color target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let unpadded_bytes_per_row = width
            .checked_mul(4)
            .ok_or(RenderPreparationError::InvalidBudget)?;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row
            .checked_add(alignment - 1)
            .map(|bytes| bytes / alignment * alignment)
            .ok_or(RenderPreparationError::InvalidBudget)?;
        let buffer_size = u64::from(padded_bytes_per_row)
            .checked_mul(u64::from(height))
            .ok_or(RenderPreparationError::InvalidBudget)?;
        if buffer_size > device.limits().max_buffer_size {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain capture readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let renderer = CelestialRenderer::new(&device, &queue, format, width, height);
        Ok(Self {
            _instance: instance,
            adapter_name,
            device,
            queue,
            renderer,
            target,
            readback,
            width,
            height,
            padded_bytes_per_row,
            last_cpu_encode: std::time::Duration::ZERO,
        })
    }

    /// Renders one already-prepared frame and returns tightly packed top-to-bottom RGBA8.
    pub fn render(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
    ) -> Result<Vec<u8>, RenderPreparationError> {
        frame.validate()?;
        if frame.projection().viewport() != [self.width, self.height]
            || frame.projection().origin() != [0, 0]
        {
            return Err(RenderPreparationError::InvalidProjection);
        }
        let start = std::time::Instant::now();
        let view = self.target.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Terrain capture encoder"),
            });
        self.renderer
            .draw(&self.device, &self.queue, &mut encoder, &view, frame)?;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        let commands = encoder.finish();
        // Host work through command encoding and upload, excluding submission,
        // device completion and readback. This is never GPU elapsed time.
        self.last_cpu_encode = start.elapsed();
        let submission = self.queue.submit([commands]);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        receiver
            .recv()
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        let mapped = self.readback.get_mapped_range(..);
        let row_bytes = self.width as usize * 4;
        let padded_row_bytes = self.padded_bytes_per_row as usize;
        let mut rgba = Vec::with_capacity(row_bytes * self.height as usize);
        for row in mapped
            .chunks_exact(padded_row_bytes)
            .take(self.height as usize)
        {
            rgba.extend_from_slice(&row[..row_bytes]);
        }
        drop(mapped);
        self.readback.unmap();
        Ok(rgba)
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Last host upload/encoding wall duration, excluding GPU wait/readback.
    pub fn last_cpu_encode(&self) -> std::time::Duration {
        self.last_cpu_encode
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}
