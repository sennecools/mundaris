//! GPU residency and draw pipelines for the distant sky.
use super::{SkyDefinition, SkyPrepared, SkyResourceReport};
use std::{sync::Arc, time::Instant};

#[repr(C)]
#[derive(Clone, Copy)]
struct StarInstance {
    position_flux: [f32; 4],
    color_radius: [f32; 4],
}

pub(crate) struct SkyRenderer {
    uniforms: wgpu::Buffer,
    detail_uniforms: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    star_pipeline: crate::aa::MsaaPipeline,
    background_pipeline: crate::aa::MsaaPipeline,
    star_buffer: wgpu::Buffer,
    star_capacity: u64,
    star_count: u32,
    texture_capacity_bytes: u64,
    sampler: wgpu::Sampler,
    resident: Option<Arc<SkyDefinition>>,
    target_srgb: bool,
    report: SkyResourceReport,
    resource_growth_events: u64,
    catalogue_upload_count: u64,
    background_upload_count: u64,
}

impl SkyRenderer {
    /// `targets` are the main-pass colour attachments; the sky writes radiance
    /// into the first and leaves the others untouched.
    pub(crate) fn new(device: &wgpu::Device, targets: &[wgpu::TextureFormat]) -> Self {
        let format = targets[0];
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Distant sky"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sky.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Distant sky resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(112),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(208),
                    },
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distant sky frame uniforms"),
            size: 112,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let detail_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Static sky focal chart axes"),
            size: 208,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Distant sky placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Distant sky wrapped longitude sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let view = texture.create_view(&Default::default());
        let detail_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Sky focal chart placeholder"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let detail_view = detail_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Distant sky bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&detail_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: detail_uniforms.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Distant sky pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        // One variant per MSAA sample count (crate::aa).
        let make_pipeline = |label: &'static str, entry: &'static str, star: bool| {
            let pipeline_layout = pipeline_layout.clone();
            let shader = shader.clone();
            let targets = targets.to_vec();
            crate::aa::MsaaPipeline::new(device, move |device, samples| {
                let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
                let star_layout = [Some(wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                })];
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        buffers: if star { &star_layout } else { &[] },
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(if star {
                            "star_fragment"
                        } else {
                            "background_fragment"
                        }),
                        compilation_options: Default::default(),
                        targets: &std::iter::once(Some(wgpu::ColorTargetState {
                            format,
                            blend: if star {
                                Some(wgpu::BlendState {
                                    color: wgpu::BlendComponent {
                                        src_factor: wgpu::BlendFactor::One,
                                        dst_factor: wgpu::BlendFactor::One,
                                        operation: wgpu::BlendOperation::Add,
                                    },
                                    alpha: wgpu::BlendComponent {
                                        src_factor: wgpu::BlendFactor::Zero,
                                        dst_factor: wgpu::BlendFactor::One,
                                        operation: wgpu::BlendOperation::Add,
                                    },
                                })
                            } else {
                                None
                            },
                            write_mask: wgpu::ColorWrites::ALL,
                        }))
                        .chain(targets[1..].iter().map(|&format| {
                            Some(wgpu::ColorTargetState {
                                format,
                                blend: None,
                                write_mask: wgpu::ColorWrites::empty(),
                            })
                        }))
                        .collect::<Vec<_>>(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: crate::aa::multisample(samples),
                    multiview_mask: None,
                    cache: None,
                })
            })
        };
        let star_pipeline = make_pipeline("Distant star Gaussian sprites", "star_vertex", true);
        let background_pipeline =
            make_pipeline("Distant sky background", "background_vertex", false);
        let star_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Distant sky star instances"),
            size: 32,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            uniforms,
            detail_uniforms,
            bind_group_layout: layout,
            bind_group,
            star_pipeline,
            background_pipeline,
            star_buffer,
            star_capacity: 32,
            star_count: 0,
            texture_capacity_bytes: 8,
            sampler,
            resident: None,
            // Float HDR targets hold linear radiance, like an sRGB view.
            target_srgb: format.is_srgb() || format == wgpu::TextureFormat::Rgba16Float,
            report: SkyResourceReport::default(),
            resource_growth_events: 0,
            catalogue_upload_count: 0,
            background_upload_count: 0,
        }
    }

    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: Option<&SkyPrepared>,
    ) -> SkyResourceReport {
        let start = Instant::now();
        let mut static_bytes = 0u64;
        let mut generation_ms = None;
        let mut frame_bytes = 0u64;
        let mut generation_duration = std::time::Duration::ZERO;
        let mut transient_bound = None;
        if let Some(frame) = frame.filter(|f| f.report().stars_drawn || f.report().background_drawn)
        {
            let definition = frame.definition();
            if !self
                .resident
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, definition))
            {
                let generated = Instant::now();
                let stars = pack_stars(definition);
                let background =
                    super::background::precompute(definition.background(), definition.morphology());
                let details = definition
                    .morphology()
                    .map(|m| super::background::precompute_details(definition.background(), m))
                    .unwrap_or_default();
                let bytes = star_bytes(&stars);
                let base_payload: u64 = background.iter().map(|m| m.rgba.capacity() as u64).sum();
                let details_payload: u64 = details
                    .iter()
                    .flatten()
                    .map(|m| m.rgba.capacity() as u64)
                    .sum();
                let base_scratch = u64::from(definition.background().width)
                    * u64::from(definition.background().height)
                    * 15;
                let detail_scratch = definition
                    .morphology()
                    .map_or(0, |m| u64::from(m.detail_size).pow(2) * 15);
                // Color12 source and next levels coexist with encoded mips. This
                // bounds both generation stages and the subsequent byte packing.
                let star_payload = (stars.capacity() * std::mem::size_of::<StarInstance>()) as u64;
                transient_bound = Some(
                    star_payload
                        + (base_payload + base_scratch)
                            .max(base_payload + details_payload + detail_scratch)
                            .max(base_payload + details_payload + bytes.capacity() as u64),
                );
                generation_duration = generated.elapsed();
                generation_ms = Some(generation_duration.as_secs_f64() * 1000.0);
                if bytes.len() as u64 > self.star_capacity {
                    self.star_capacity = (bytes.len() as u64).next_power_of_two().max(32);
                    self.star_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Distant sky star instances"),
                        size: self.star_capacity,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    self.resource_growth_events += 1;
                }
                if !bytes.is_empty() {
                    queue.write_buffer(&self.star_buffer, 0, &bytes);
                }
                self.star_count = stars.len() as u32;
                static_bytes += bytes.len() as u64;
                self.catalogue_upload_count += 1;
                static_bytes += self.upload_background(
                    device,
                    queue,
                    &background,
                    &details,
                    definition.morphology(),
                );
                self.background_upload_count += 1;
                self.resident = Some(Arc::clone(definition));
            }
            let mut bytes = frame.uniform_bytes();
            bytes[92..96]
                .copy_from_slice(&(if self.target_srgb { 1.0_f32 } else { 0.0_f32 }).to_le_bytes());
            queue.write_buffer(&self.uniforms, 0, &bytes);
            frame_bytes = bytes.len() as u64;
        }
        self.report = SkyResourceReport {
            generation_ms,
            upload_api_ms: start
                .elapsed()
                .saturating_sub(generation_duration)
                .as_secs_f64()
                * 1000.0,
            static_upload_bytes: static_bytes,
            frame_upload_bytes: frame_bytes,
            gpu_capacity_bytes: 112 + 208 + self.star_capacity + self.texture_capacity_bytes,
            cpu_capacity_bytes: self
                .resident
                .as_ref()
                .map_or(0, |d| d.cpu_capacity_bytes() as u64),
            resource_growth_events: self.resource_growth_events,
            catalogue_upload_count: self.catalogue_upload_count,
            background_upload_count: self.background_upload_count,
            transient_generation_payload_bound_bytes: transient_bound,
        };
        self.report
    }

    fn upload_background(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mips: &[super::background::BackgroundMip],
        details: &[Vec<super::background::BackgroundMip>],
        morphology: Option<&super::SkyMorphology>,
    ) -> u64 {
        let first = &mips[0];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Deterministic distant sky background"),
            size: wgpu::Extent3d {
                width: first.width,
                height: first.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, mip) in mips.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &mip.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mip.width * 4),
                    rows_per_image: Some(mip.height),
                },
                wgpu::Extent3d {
                    width: mip.width,
                    height: mip.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let mut upload_bytes: u64 = mips
            .iter()
            .map(|mip| u64::from(mip.width) * u64::from(mip.height) * 4)
            .sum();
        let detail_first = details.first().and_then(|levels| levels.first());
        let detail_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bounded sky focal charts"),
            size: wgpu::Extent3d {
                width: detail_first.map_or(1, |m| m.width),
                height: detail_first.map_or(1, |m| m.height),
                depth_or_array_layers: details.len().max(1) as u32,
            },
            mip_level_count: details.first().map_or(1, |levels| levels.len()) as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut detail_bytes = 0;
        for (layer, levels) in details.iter().enumerate() {
            for (level, mip) in levels.iter().enumerate() {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &detail_texture,
                        mip_level: level as u32,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: layer as u32,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &mip.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(mip.width * 4),
                        rows_per_image: Some(mip.height),
                    },
                    wgpu::Extent3d {
                        width: mip.width,
                        height: mip.height,
                        depth_or_array_layers: 1,
                    },
                );
                detail_bytes += mip.rgba.len() as u64;
            }
        }
        let mut chart_bytes = [0_u8; 208];
        if let Some(morphology) = morphology {
            for (index, complex) in morphology.complexes.iter().enumerate() {
                for (axis_index, axis) in complex.axes().into_iter().enumerate() {
                    let fourth = if axis_index == 0 {
                        (1.0 / complex.half_extent_rad.tan()) as f32
                    } else {
                        0.0
                    };
                    for (component, value) in [axis.x as f32, axis.y as f32, axis.z as f32, fourth]
                        .into_iter()
                        .enumerate()
                    {
                        let start = index * 48 + axis_index * 16 + component * 4;
                        chart_bytes[start..start + 4].copy_from_slice(&value.to_le_bytes());
                    }
                }
            }
            chart_bytes[192..196]
                .copy_from_slice(&(morphology.complexes.len() as u32).to_le_bytes());
        }
        queue.write_buffer(&self.detail_uniforms, 0, &chart_bytes);
        self.texture_capacity_bytes = upload_bytes + detail_bytes.max(4);
        upload_bytes += detail_bytes + chart_bytes.len() as u64;
        let view = texture.create_view(&Default::default());
        let detail_view = detail_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Distant sky bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&detail_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: self.detail_uniforms.as_entire_binding(),
                },
            ],
        });
        self.resource_growth_events += 1;
        upload_bytes
    }

    /// Selects the MSAA sample count of the sky pipelines (crate::aa).
    pub(crate) fn set_samples(&mut self, device: &wgpu::Device, samples: u32) {
        self.background_pipeline.set_samples(device, samples);
        self.star_pipeline.set_samples(device, samples);
    }

    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, frame: &SkyPrepared) {
        let report = frame.report();
        if !report.stars_drawn && !report.background_drawn {
            return;
        }
        pass.set_bind_group(0, &self.bind_group, &[]);
        if report.background_drawn {
            pass.set_pipeline(self.background_pipeline.get());
            pass.draw(0..3, 0..1);
        }
        if report.stars_drawn && self.star_count > 0 {
            pass.set_pipeline(self.star_pipeline.get());
            pass.set_vertex_buffer(0, self.star_buffer.slice(..));
            pass.draw(0..6, 0..self.star_count);
        }
    }
    pub(crate) fn report(&self) -> SkyResourceReport {
        self.report
    }
}

fn pack_stars(definition: &SkyDefinition) -> Vec<StarInstance> {
    definition
        .stars()
        .iter()
        .map(|star| StarInstance {
            position_flux: [
                (star.position_m.x / 1e18) as f32,
                (star.position_m.y / 1e18) as f32,
                (star.position_m.z / 1e18) as f32,
                star.flux,
            ],
            color_radius: [
                star.color[0],
                star.color[1],
                star.color[2],
                star.radius_pixels,
            ],
        })
        .collect()
}
fn star_bytes(stars: &[StarInstance]) -> Vec<u8> {
    stars
        .iter()
        .flat_map(|s| {
            s.position_flux
                .into_iter()
                .chain(s.color_radius)
                .flat_map(f32::to_le_bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn sky_shader_validates_with_naga() {
        let module = naga::front::wgsl::parse_str(include_str!("shaders/sky.wgsl")).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }
}
