//! Shared storage/index instancing in the existing celestial depth pass.
use super::{SurfaceStaging, SurfaceTopology};
use crate::RenderPreparationError;
use std::ops::Range;
pub(crate) struct PlanetSurfaceRenderer {
    pipeline: wgpu::RenderPipeline,
    underside: wgpu::RenderPipeline,
    clipped_pipeline: wgpu::RenderPipeline,
    clipped_underside: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    samples: wgpu::Buffer,
    instances: wgpu::Buffer,
    fallback: wgpu::Buffer,
    capacities: [u64; 3],
    indices: wgpu::Buffer,
    ranges: [Range<u32>; 16],
    index_bytes: u64,
    lighting: wgpu::Buffer,
    target_srgb: bool,
    last_upload_profile: crate::gpu_profile::CpuUploadProfile,
}
impl PlanetSurfaceRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        projection_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Planet samples / instances"),
            entries: &[
                entry(0, 48),
                entry(1, 64),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(64),
                    },
                    count: None,
                },
            ],
        });
        let samples = buffer(device, 48, wgpu::BufferUsages::STORAGE);
        let instances = buffer(device, 64, wgpu::BufferUsages::STORAGE);
        let lighting = buffer(device, 64, wgpu::BufferUsages::UNIFORM);
        let group = binding(device, &layout, &samples, &instances, &lighting);
        let topology = SurfaceTopology::new();
        let mut bytes = Vec::new();
        let mut ranges = std::array::from_fn(|_| 0..0);
        for (mask, range) in ranges.iter_mut().enumerate() {
            let start = bytes.len() as u32 / 2;
            for index in topology.indices(mask as u8) {
                bytes.extend_from_slice(&index.to_le_bytes());
            }
            *range = start..bytes.len() as u32 / 2;
        }
        let index_bytes = bytes.len() as u64;
        let indices = buffer(device, index_bytes, wgpu::BufferUsages::INDEX);
        queue.write_buffer(&indices, 0, &bytes);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Planet surface debug / terrain lighting"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/planet_surface.wgsl").into()),
        });
        let regular_pipeline = pipeline(
            device,
            format,
            projection_layout,
            &layout,
            &shader,
            false,
            false,
        );
        let underside = pipeline(
            device,
            format,
            projection_layout,
            &layout,
            &shader,
            false,
            true,
        );
        let clipped_pipeline = pipeline(
            device,
            format,
            projection_layout,
            &layout,
            &shader,
            true,
            false,
        );
        let clipped_underside = pipeline(
            device,
            format,
            projection_layout,
            &layout,
            &shader,
            true,
            true,
        );
        Self {
            pipeline: regular_pipeline,
            underside,
            clipped_pipeline,
            clipped_underside,
            layout,
            group,
            samples,
            instances,
            fallback: buffer(device, 80, wgpu::BufferUsages::VERTEX),
            capacities: [48, 64, 80],
            indices,
            ranges,
            index_bytes,
            lighting,
            target_srgb: format.is_srgb(),
            last_upload_profile: crate::gpu_profile::CpuUploadProfile::default(),
        }
    }
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        staging: &SurfaceStaging,
    ) -> Result<(), RenderPreparationError> {
        let sizes = [
            staging.samples.len() as u64,
            staging.instances.len() as u64,
            staging.fallback.len() as u64,
        ];
        let proposed = std::array::from_fn::<_, 3, _>(|i| {
            if sizes[i] > self.capacities[i] {
                sizes[i].div_ceil(4096) * 4096
            } else {
                self.capacities[i]
            }
        });
        if proposed.iter().sum::<u64>() + self.index_bytes + 64 > 80 * 1024 * 1024
            || proposed[..2]
                .iter()
                .any(|&n| n > u64::from(device.limits().max_storage_buffer_binding_size))
            || proposed
                .iter()
                .any(|&n| n > device.limits().max_buffer_size)
        {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let mut rebound = false;
        let growth_events = proposed
            .iter()
            .zip(self.capacities)
            .filter(|(next, old)| **next > *old)
            .count() as u32;
        let mut growth_wait = std::time::Duration::ZERO;
        if proposed
            .iter()
            .zip(self.capacities)
            .any(|(&next, old)| next > old)
        {
            // Growth is rare. Complete earlier submissions before destroying old
            // allocations, so in-flight generations cannot bypass the 80 MiB cap.
            let wait_start = std::time::Instant::now();
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .map_err(|e| RenderPreparationError::GpuProgress(e.to_string()))?;
            growth_wait = wait_start.elapsed();
        }
        for (i, &size) in proposed.iter().enumerate() {
            if size > self.capacities[i] {
                let target = match i {
                    0 => &mut self.samples,
                    1 => &mut self.instances,
                    _ => &mut self.fallback,
                };
                target.destroy();
                *target = buffer(
                    device,
                    size,
                    if i == 2 {
                        wgpu::BufferUsages::VERTEX
                    } else {
                        wgpu::BufferUsages::STORAGE
                    },
                );
                self.capacities[i] = size;
                rebound |= i < 2;
            }
        }
        if rebound {
            self.group = binding(
                device,
                &self.layout,
                &self.samples,
                &self.instances,
                &self.lighting,
            );
        }
        let api_start = std::time::Instant::now();
        let mut bytes = [0u8; 64];
        for (value, out) in staging
            .lighting
            .packed(self.target_srgb)
            .into_iter()
            .zip(bytes.as_chunks_mut::<4>().0)
        {
            out.copy_from_slice(&value.to_le_bytes());
        }
        queue.write_buffer(&self.lighting, 0, &bytes);
        for (buffer, bytes) in [
            (&self.samples, &staging.samples),
            (&self.instances, &staging.instances),
            (&self.fallback, &staging.fallback),
        ] {
            if !bytes.is_empty() {
                queue.write_buffer(buffer, 0, bytes);
            }
        }
        let uploaded = 64u64 + sizes.iter().sum::<u64>();
        self.last_upload_profile = crate::gpu_profile::CpuUploadProfile {
            bytes_uploaded: uploaded,
            resident_capacity_bytes: self.capacities.iter().sum::<u64>() + self.index_bytes + 64,
            buffer_growth_events: growth_events,
            growth_wait_events: u32::from(growth_events > 0),
            growth_wait,
            upload_api_duration: api_start.elapsed(),
        };
        Ok(())
    }

    pub fn last_upload_profile(&self) -> crate::gpu_profile::CpuUploadProfile {
        self.last_upload_profile
    }
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &wgpu::BindGroup,
        staging: &SurfaceStaging,
        timestamps: Option<&crate::gpu_profile::CelestialQueries>,
    ) {
        if !staging.instances.is_empty() {
            if let Some(queries) = timestamps.filter(|q| q.inside_passes()) {
                queries.write_scope(pass, 3);
            }
            pass.set_pipeline(if staging.underside {
                &self.underside
            } else {
                &self.pipeline
            });
            pass.set_bind_group(0, projection, &[]);
            pass.set_bind_group(1, &self.group, &[]);
            pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
            for (mask, instances) in staging.buckets.iter().enumerate() {
                if !instances.is_empty() {
                    pass.draw_indexed(self.ranges[mask].clone(), 0, instances.clone());
                }
            }
            if let Some(queries) = timestamps.filter(|q| q.inside_passes()) {
                queries.end_scope(pass, 3);
            }
        }
        if !staging.fallback.is_empty() {
            if let Some(queries) = timestamps.filter(|q| q.inside_passes()) {
                queries.write_scope(pass, 4);
            }
            pass.set_pipeline(if staging.underside {
                &self.clipped_underside
            } else {
                &self.clipped_pipeline
            });
            pass.set_bind_group(0, projection, &[]);
            pass.set_bind_group(1, &self.group, &[]);
            pass.set_vertex_buffer(0, self.fallback.slice(..));
            pass.draw(0..(staging.fallback.len() / 80) as u32, 0..1);
            if let Some(queries) = timestamps.filter(|q| q.inside_passes()) {
                queries.end_scope(pass, 4);
            }
        }
    }
}
fn entry(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}
fn buffer(device: &wgpu::Device, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Bounded planetary surface buffer"),
        size,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
fn binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    samples: &wgpu::Buffer,
    instances: &wgpu::Buffer,
    lighting: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Planet packed storage"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: samples.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: lighting.as_entire_binding(),
            },
        ],
    })
}
fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    projection: &wgpu::BindGroupLayout,
    storage: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    clipped: bool,
    underside: bool,
) -> wgpu::RenderPipeline {
    let groups = [projection, storage];
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Surface reverse-Z layout"),
        bind_group_layouts: &groups,
        push_constant_ranges: &[],
    });
    let attributes =
        wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4,2=>Float32x4,3=>Float32x4,4=>Float32x4];
    let buffers = [wgpu::VertexBufferLayout {
        array_stride: 80,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attributes,
    }];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Planet batched surface"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(if clipped { "vs_clipped" } else { "vs_main" }),
            compilation_options: Default::default(),
            buffers: if clipped { &buffers } else { &[] },
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: if underside {
                None
            } else {
                Some(wgpu::Face::Back)
            },
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::GreaterEqual,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    })
}
