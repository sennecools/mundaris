//! Small opt-in headless readback of the production celestial reverse-Z draw path.
//!
//! Capture dimensions are fixed at construction and must match the frame's content
//! viewport. Creating a capture requires a locally available GPU adapter; no GPU
//! integration test runs as part of the ordinary test suite.

use crate::{
    CelestialFrame, RenderPreparationError, TimestampAvailability, celestial::CelestialRenderer,
};

/// Offscreen RGBA8 target that submits through the ordinary celestial renderer.
pub struct TerrainCaptureRenderer {
    _instance: wgpu::Instance,
    adapter_name: String,
    adapter_backend: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: CelestialRenderer,
    target: wgpu::Texture,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    last_cpu_encode: std::time::Duration,
    timestamp_availability: TimestampAvailability,
    timestamp_queries: Option<crate::gpu_profile::CelestialQueries>,
    timestamp_resolve: Option<wgpu::Buffer>,
    timestamp_readback: Option<wgpu::Buffer>,
    last_gpu_profile: crate::gpu_profile::GpuProfile,
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
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
        let info = adapter.get_info();
        let requested_features = crate::gpu_profile::available_features(&adapter);
        let timestamp_availability = crate::gpu_profile::availability(requested_features);
        let adapter_name = info.name;
        let adapter_backend = format!("{:?}", info.backend);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("Mundaris terrain capture device"),
                required_features: requested_features,
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
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
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
        let timestamp_queries = crate::gpu_profile::CelestialQueries::new(&device);
        let timestamp_resolve = timestamp_queries.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Terrain capture timestamp resolve"),
                size: u64::from(crate::gpu_profile::QUERY_COUNT) * 8,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        });
        let timestamp_readback = timestamp_queries.as_ref().map(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Terrain capture timestamp readback"),
                size: u64::from(crate::gpu_profile::QUERY_COUNT) * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let renderer = CelestialRenderer::new(&device, &queue, format, width, height);
        Ok(Self {
            _instance: instance,
            adapter_name,
            adapter_backend,
            device,
            queue,
            renderer,
            target,
            readback,
            width,
            height,
            padded_bytes_per_row,
            last_cpu_encode: std::time::Duration::ZERO,
            timestamp_availability,
            timestamp_queries,
            timestamp_resolve,
            timestamp_readback,
            last_gpu_profile: crate::gpu_profile::GpuProfile::default(),
        })
    }

    /// Renders one already-prepared frame and returns tightly packed top-to-bottom RGBA8.
    pub fn render(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
    ) -> Result<Vec<u8>, RenderPreparationError> {
        // Capture fixtures must report invalid production draw state as a test
        // failure, not panic during backend teardown. Pop on every Result path.
        let error_scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.render_inner(frame);
        if let Some(error) = pollster::block_on(error_scope.pop()) {
            return Err(RenderPreparationError::GpuProgress(error.to_string()));
        }
        result
    }

    fn render_inner(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
    ) -> Result<Vec<u8>, RenderPreparationError> {
        frame.validate()?;
        let [x, y] = frame.projection().origin();
        let [w, h] = frame.projection().viewport();
        if x.checked_add(w).is_none_or(|end| end > self.width)
            || y.checked_add(h).is_none_or(|end| end > self.height)
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
        let scope_mask = self.renderer.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            frame,
            self.timestamp_queries.as_ref(),
        )?;
        if let (Some(queries), Some(resolve), Some(timestamp_readback)) = (
            self.timestamp_queries.as_ref(),
            self.timestamp_resolve.as_ref(),
            self.timestamp_readback.as_ref(),
        ) {
            encoder.resolve_query_set(&queries.set, 0..crate::gpu_profile::QUERY_COUNT, resolve, 0);
            encoder.copy_buffer_to_buffer(
                resolve,
                0,
                timestamp_readback,
                0,
                u64::from(crate::gpu_profile::QUERY_COUNT) * 8,
            );
        }
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
        self.renderer.on_submitted();
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
        if let Some(timestamp_readback) = &self.timestamp_readback {
            let (sender, receiver) = std::sync::mpsc::channel();
            timestamp_readback.map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
            receiver
                .recv()
                .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?
                .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
            let mapped = timestamp_readback
                .get_mapped_range(..)
                .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
            let ticks: Vec<u64> = mapped
                .as_chunks::<8>()
                .0
                .iter()
                .map(|bytes| u64::from_le_bytes(*bytes))
                .collect();
            let period = self.queue.get_timestamp_period();
            self.last_gpu_profile = crate::gpu_profile::decode(&ticks, period, scope_mask);
            drop(mapped);
            timestamp_readback.unmap();
        }
        let mapped = self
            .readback
            .get_mapped_range(..)
            .map_err(|error| RenderPreparationError::GpuProgress(error.to_string()))?;
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
    /// Backend selected for this offscreen adapter (not a native-window timing claim).
    pub fn adapter_backend(&self) -> &str {
        &self.adapter_backend
    }

    /// Last host upload/encoding wall duration, excluding GPU wait/readback.
    pub fn last_cpu_encode(&self) -> std::time::Duration {
        self.last_cpu_encode
    }

    /// Adapter timestamp capability. Capture currently waits for readback, but
    /// capability alone is not a GPU timing measurement.
    pub fn timestamp_availability(&self) -> TimestampAvailability {
        self.timestamp_availability
    }

    /// Latest timestamp-query values from the last completed capture render.
    pub fn last_gpu_profile(&self) -> crate::gpu_profile::GpuProfile {
        self.last_gpu_profile
    }

    /// Same-submission sky residency and upload accounting, excluding readback.
    pub fn last_sky_resource_report(&self) -> crate::sky::SkyResourceReport {
        self.renderer.last_sky_resource_report()
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}
