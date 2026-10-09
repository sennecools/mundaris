//! HDR frame targets and post-processing passes (docs/RENDER_PIPELINE_HDR.md §2).
//!
//! A small explicit pass list, not a render graph. The main pass writes
//! pre-exposed direct radiance, octahedral view normals and ambient radiance
//! (MRT) with infinite reverse-Z depth. Afterwards: GTAO → ambient × AO
//! composite → luminance histogram and adaptation → bloom → tonemap into the
//! sRGB scene texture. Transient targets follow the viewport size.

use crate::RenderSettings;
use crate::gpu_profile::{CelestialQueries, pair};

pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub(crate) const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
pub(crate) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Float;
const POST_BYTES: u64 = 7 * 16;
const MAX_BLOOM_LEVELS: u32 = 6;
/// Initial exposure: EV100 14 (bright daylight), flagged for an immediate snap.
const INITIAL_EV100: f32 = 14.0;

/// Main-pass colour targets in attachment order.
pub(crate) const SCENE_TARGETS: [wgpu::TextureFormat; 3] = [HDR_FORMAT, NORMAL_FORMAT, HDR_FORMAT];

struct Target {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

fn target(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Target {
        _texture: texture,
        view,
    }
}

struct Targets {
    size: [u32; 2],
    ao_size: [u32; 2],
    bloom_levels: u32,
    direct: Target,
    normal: Target,
    ambient: Target,
    depth: Target,
    ao: Target,
    _bloom: wgpu::Texture,
    bloom_views: Vec<wgpu::TextureView>,
    gtao_group: wgpu::BindGroup,
    composite_group: wgpu::BindGroup,
    exposure_group: wgpu::BindGroup,
    tonemap_group: wgpu::BindGroup,
    /// Source of each downsample level (level 0 reads the HDR target).
    down_groups: Vec<wgpu::BindGroup>,
    /// Source mip of each upsample step, indexed by source level.
    up_groups: Vec<wgpu::BindGroup>,
}

pub(crate) struct PostProcess {
    post: wgpu::Buffer,
    exposure: wgpu::Buffer,
    histogram: wgpu::Buffer,
    sampler: wgpu::Sampler,
    gtao_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    exposure_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    bloom_layout: wgpu::BindGroupLayout,
    gtao: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    histogram_pipeline: wgpu::ComputePipeline,
    adapt: wgpu::ComputePipeline,
    down_first: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    tonemap: wgpu::RenderPipeline,
    targets: Option<Targets>,
    half_res_ao: bool,
    frame: u64,
    last_frame: Option<std::time::Instant>,
}

/// Per-frame post inputs that are not settings.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PostFrame {
    pub near_m: f64,
    pub focal_px: f64,
    pub view_mode: u32,
    /// Debug views bypass exposure, metering and tonemapping.
    pub passthrough: bool,
}

fn entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    ty: wgpu::BindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty,
        count: None,
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: wgpu::BufferSize::new(POST_BYTES),
        },
    )
}

fn texture_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    filterable: bool,
) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
    )
}

fn depth_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        wgpu::ShaderStages::FRAGMENT,
        wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
    )
}

fn storage_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    read_only: bool,
) -> wgpu::BindGroupLayoutEntry {
    entry(
        binding,
        visibility,
        wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
    )
}

fn layout(
    device: &wgpu::Device,
    label: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    })
}

fn module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

fn fullscreen(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    entry_point: &str,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(entry_point),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
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
};

#[allow(clippy::too_many_arguments)] // One fullscreen draw.
fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    view: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
    writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: writes,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..3, 0..1);
}

fn buffer(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

/// Bloom chain depth for a half-resolution base of `size`.
pub(crate) fn bloom_levels(size: [u32; 2]) -> u32 {
    let base = (size[0] / 2).min(size[1] / 2).max(1);
    (base.ilog2().saturating_sub(2)).clamp(1, MAX_BLOOM_LEVELS)
}

impl PostProcess {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output: wgpu::TextureFormat,
    ) -> Self {
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let compute = wgpu::ShaderStages::COMPUTE;
        let post = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Post parameters"),
            size: POST_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let exposure = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Exposure state"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let initial = 1.44 / 2f32.powf(INITIAL_EV100);
        queue.write_buffer(
            &exposure,
            0,
            &[initial, INITIAL_EV100, initial, -1.0]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let histogram = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Luminance histogram"),
            size: 256 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Post linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let gtao_layout = layout(
            device,
            "GTAO inputs",
            &[
                uniform_entry(0, fragment),
                depth_entry(1),
                texture_entry(2, fragment, false),
            ],
        );
        let composite_layout = layout(
            device,
            "AO composite inputs",
            &[
                uniform_entry(0, fragment),
                depth_entry(1),
                texture_entry(2, fragment, false),
                texture_entry(3, fragment, false),
            ],
        );
        let exposure_layout = layout(
            device,
            "Exposure inputs",
            &[
                uniform_entry(0, compute),
                texture_entry(1, compute, false),
                storage_entry(2, compute, false),
                storage_entry(3, compute, false),
            ],
        );
        let tonemap_layout = layout(
            device,
            "Tonemap inputs",
            &[
                uniform_entry(0, fragment),
                texture_entry(1, fragment, false),
                texture_entry(2, fragment, true),
                entry(
                    3,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                storage_entry(4, fragment, true),
                texture_entry(5, fragment, false),
                depth_entry(6),
            ],
        );
        let bloom_layout = layout(
            device,
            "Bloom inputs",
            &[
                uniform_entry(0, fragment),
                texture_entry(1, fragment, true),
                entry(
                    2,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        );
        let gtao_shader = module(
            device,
            "GTAO",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/gtao.wgsl")
            ),
        );
        let composite_shader = module(
            device,
            "AO composite",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/ao_filter.wgsl"),
                include_str!("shaders/ao_composite.wgsl")
            ),
        );
        let exposure_shader = module(
            device,
            "Exposure",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/exposure.wgsl")
            ),
        );
        let bloom_shader = module(
            device,
            "Bloom",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/bloom.wgsl")
            ),
        );
        let tonemap_shader = module(
            device,
            "Tonemap",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/ao_filter.wgsl"),
                include_str!("shaders/tonemap.wgsl")
            ),
        );
        let compute_pipeline = |entry_point: &str| {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry_point),
                bind_group_layouts: &[Some(&exposure_layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry_point),
                layout: Some(&pipeline_layout),
                module: &exposure_shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            gtao: fullscreen(
                device,
                "GTAO",
                &gtao_layout,
                &gtao_shader,
                "fs_gtao",
                AO_FORMAT,
                None,
            ),
            composite: fullscreen(
                device,
                "AO composite",
                &composite_layout,
                &composite_shader,
                "fs_composite",
                HDR_FORMAT,
                Some(ADDITIVE),
            ),
            histogram_pipeline: compute_pipeline("histogram_main"),
            adapt: compute_pipeline("adapt_main"),
            down_first: fullscreen(
                device,
                "Bloom first downsample",
                &bloom_layout,
                &bloom_shader,
                "fs_down_first",
                HDR_FORMAT,
                None,
            ),
            down: fullscreen(
                device,
                "Bloom downsample",
                &bloom_layout,
                &bloom_shader,
                "fs_down",
                HDR_FORMAT,
                None,
            ),
            up: fullscreen(
                device,
                "Bloom upsample",
                &bloom_layout,
                &bloom_shader,
                "fs_up",
                HDR_FORMAT,
                Some(ADDITIVE),
            ),
            tonemap: fullscreen(
                device,
                "Tonemap",
                &tonemap_layout,
                &tonemap_shader,
                "fs_tonemap",
                output,
                None,
            ),
            post,
            exposure,
            histogram,
            sampler,
            gtao_layout,
            composite_layout,
            exposure_layout,
            tonemap_layout,
            bloom_layout,
            targets: None,
            half_res_ao: true,
            frame: 0,
            last_frame: None,
        }
    }

    pub(crate) fn exposure_buffer(&self) -> &wgpu::Buffer {
        &self.exposure
    }

    /// (Re)creates transient targets when the viewport or AO resolution changes.
    pub(crate) fn ensure(&mut self, device: &wgpu::Device, size: [u32; 2], half_res_ao: bool) {
        let size = [size[0].max(1), size[1].max(1)];
        if self
            .targets
            .as_ref()
            .is_some_and(|t| t.size == size && self.half_res_ao == half_res_ao)
        {
            return;
        }
        self.half_res_ao = half_res_ao;
        let ao_size = if half_res_ao {
            [size[0].div_ceil(2), size[1].div_ceil(2)]
        } else {
            size
        };
        let direct = target(device, "HDR direct radiance", size, HDR_FORMAT);
        let normal = target(device, "View normals", size, NORMAL_FORMAT);
        let ambient = target(device, "HDR ambient radiance", size, HDR_FORMAT);
        let depth = target(device, "Scene infinite reverse-Z", size, DEPTH_FORMAT);
        let ao = target(device, "Ambient occlusion", ao_size, AO_FORMAT);
        let levels = bloom_levels(size);
        let bloom = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Bloom chain"),
            size: wgpu::Extent3d {
                width: (size[0] / 2).max(1),
                height: (size[1] / 2).max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let bloom_views: Vec<_> = (0..levels)
            .map(|level| {
                bloom.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("Bloom level"),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let group =
            |label: &str, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupEntry]| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout,
                    entries,
                })
            };
        let sampler = wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        };
        let bloom_group = |source: &wgpu::TextureView| {
            group(
                "Bloom source",
                &self.bloom_layout,
                &[buffer(0, &self.post), view(1, source), sampler.clone()],
            )
        };
        let down_groups = (0..levels as usize)
            .map(|level| {
                bloom_group(if level == 0 {
                    &direct.view
                } else {
                    &bloom_views[level - 1]
                })
            })
            .collect();
        let up_groups = bloom_views.iter().map(bloom_group).collect();
        let targets = Targets {
            gtao_group: group(
                "GTAO inputs",
                &self.gtao_layout,
                &[
                    buffer(0, &self.post),
                    view(1, &depth.view),
                    view(2, &normal.view),
                ],
            ),
            composite_group: group(
                "AO composite inputs",
                &self.composite_layout,
                &[
                    buffer(0, &self.post),
                    view(1, &depth.view),
                    view(2, &ambient.view),
                    view(3, &ao.view),
                ],
            ),
            exposure_group: group(
                "Exposure inputs",
                &self.exposure_layout,
                &[
                    buffer(0, &self.post),
                    view(1, &direct.view),
                    buffer(2, &self.histogram),
                    buffer(3, &self.exposure),
                ],
            ),
            tonemap_group: group(
                "Tonemap inputs",
                &self.tonemap_layout,
                &[
                    buffer(0, &self.post),
                    view(1, &direct.view),
                    view(2, &bloom_views[0]),
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    buffer(4, &self.exposure),
                    view(5, &ao.view),
                    view(6, &depth.view),
                ],
            ),
            down_groups,
            up_groups,
            size,
            ao_size,
            bloom_levels: levels,
            direct,
            normal,
            ambient,
            depth,
            ao,
            _bloom: bloom,
            bloom_views,
        };
        self.targets = Some(targets);
    }

    /// Main-pass attachments: direct, normal, ambient colour views and depth.
    pub(crate) fn scene_views(&self) -> Option<([&wgpu::TextureView; 3], &wgpu::TextureView)> {
        self.targets.as_ref().map(|t| {
            (
                [&t.direct.view, &t.normal.view, &t.ambient.view],
                &t.depth.view,
            )
        })
    }

    pub(crate) fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.depth.view)
    }

    /// Encodes every post pass ending in `output`. Returns the timed scope bits.
    pub(crate) fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        settings: &RenderSettings,
        frame: PostFrame,
        timestamps: Option<&CelestialQueries>,
    ) -> u32 {
        let Some(t) = &self.targets else {
            return 0;
        };
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .map_or(0.0, |last| now.duration_since(last).as_secs_f32().min(0.25));
        self.last_frame = Some(now);
        self.frame = self.frame.wrapping_add(1);
        let ao_enabled = settings.ao.enabled;
        let bloom_enabled = settings.bloom.enabled && !frame.passthrough;
        let values: [[f32; 4]; 7] = [
            [
                t.size[0] as f32,
                t.size[1] as f32,
                t.ao_size[0] as f32,
                t.ao_size[1] as f32,
            ],
            [
                frame.near_m as f32,
                frame.focal_px as f32,
                0.0,
                frame.view_mode as f32,
            ],
            [
                settings.ao.radius_m,
                settings.ao.distance_scale,
                settings.ao.intensity,
                if ao_enabled { 1.0 } else { 0.0 },
            ],
            [
                match settings.exposure.mode {
                    crate::ExposureMode::Auto => 0.0,
                    crate::ExposureMode::Manual => 1.0,
                },
                settings.exposure.ev100,
                settings.exposure.compensation,
                dt,
            ],
            [
                settings.exposure.min_ev100,
                settings.exposure.max_ev100,
                settings.exposure.speed_up,
                settings.exposure.speed_down,
            ],
            [
                if bloom_enabled { 1.0 } else { 0.0 },
                settings.bloom.intensity,
                settings.bloom.radius,
                t.bloom_levels as f32,
            ],
            [
                settings.tonemap.shader_index() as f32,
                if settings.dither { 1.0 } else { 0.0 },
                (self.frame % 1024) as f32,
                if frame.passthrough { 1.0 } else { 0.0 },
            ],
        ];
        queue.write_buffer(
            &self.post,
            0,
            &values
                .iter()
                .flatten()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let mut mask = 0u32;
        let pass = fullscreen_pass;
        let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
        if ao_enabled && !frame.passthrough {
            pass(
                encoder,
                "GTAO",
                &t.ao.view,
                clear,
                timestamps.map(|q| q.pass_writes(pair::AO)),
                &self.gtao,
                &t.gtao_group,
            );
            mask |= 1 << pair::AO;
        }
        pass(
            encoder,
            "Ambient × AO composite",
            &t.direct.view,
            wgpu::LoadOp::Load,
            timestamps.map(|q| q.pass_writes(pair::AO_COMPOSITE)),
            &self.composite,
            &t.composite_group,
        );
        mask |= 1 << pair::AO_COMPOSITE;
        {
            let mut compute = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Exposure histogram and adaptation"),
                timestamp_writes: timestamps
                    .and_then(|q| q.compute_writes(pair::EXPOSURE, true, true)),
            });
            compute.set_bind_group(0, &t.exposure_group, &[]);
            compute.set_pipeline(&self.histogram_pipeline);
            compute.dispatch_workgroups(t.size[0].div_ceil(16), t.size[1].div_ceil(16), 1);
            compute.set_pipeline(&self.adapt);
            compute.dispatch_workgroups(1, 1, 1);
        }
        mask |= 1 << pair::EXPOSURE;
        if bloom_enabled {
            let levels = t.bloom_levels as usize;
            let steps = levels + levels.saturating_sub(1);
            let mut step = 0usize;
            let writes = |step: usize| {
                timestamps
                    .and_then(|q| q.pass_writes_partial(pair::BLOOM, step == 0, step + 1 == steps))
            };
            for level in 0..levels {
                pass(
                    encoder,
                    "Bloom downsample",
                    &t.bloom_views[level],
                    clear,
                    writes(step),
                    if level == 0 {
                        &self.down_first
                    } else {
                        &self.down
                    },
                    &t.down_groups[level],
                );
                step += 1;
            }
            for level in (1..levels).rev() {
                pass(
                    encoder,
                    "Bloom upsample",
                    &t.bloom_views[level - 1],
                    wgpu::LoadOp::Load,
                    writes(step),
                    &self.up,
                    &t.up_groups[level],
                );
                step += 1;
            }
            mask |= 1 << pair::BLOOM;
        }
        pass(
            encoder,
            "Tonemap",
            output,
            clear,
            timestamps.map(|q| q.pass_writes(pair::TONEMAP)),
            &self.tonemap,
            &t.tonemap_group,
        );
        mask | 1 << pair::TONEMAP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_shaders_parse_and_validate() {
        for (name, source) in [
            (
                "gtao",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/gtao.wgsl")
                ),
            ),
            (
                "composite",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/ao_filter.wgsl"),
                    include_str!("shaders/ao_composite.wgsl")
                ),
            ),
            (
                "exposure",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/exposure.wgsl")
                ),
            ),
            (
                "bloom",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/bloom.wgsl")
                ),
            ),
            (
                "tonemap",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/ao_filter.wgsl"),
                    include_str!("shaders/tonemap.wgsl")
                ),
            ),
        ] {
            let module = naga::front::wgsl::parse_str(source)
                .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        }
    }

    #[test]
    fn bloom_levels_follow_viewport() {
        assert_eq!(bloom_levels([1920, 1080]), 6);
        assert_eq!(bloom_levels([660, 725]), 6);
        assert_eq!(bloom_levels([64, 64]), 3);
        assert_eq!(bloom_levels([2, 2]), 1);
    }
}
