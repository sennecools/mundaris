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
/// Shader `Taa` uniform (aa_taa.wgsl).
const TAA_BYTES: u64 = 6 * 16;
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
    ms_target(device, label, size, format, 1)
}

fn ms_target(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    samples: u32,
) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    Target {
        _texture: texture,
        view,
    }
}

/// Multisampled main-pass targets and the bind group that resolves them.
struct MsTargets {
    direct: Target,
    normal: Target,
    ambient: Target,
    depth: Target,
    resolve_group: wgpu::BindGroup,
}

/// TAA history ping-pong: frame k writes `history[k % 2]` reading the other.
struct TaaTargets {
    history: [Target; 2],
    groups: [wgpu::BindGroup; 2],
}

/// Tonemapped image before FXAA: written through an sRGB view, read as unorm.
struct LdrTarget {
    _texture: wgpu::Texture,
    render_view: wgpu::TextureView,
    fxaa_group: wgpu::BindGroup,
}

struct Targets {
    size: [u32; 2],
    samples: u32,
    ms: Option<MsTargets>,
    ldr: Option<LdrTarget>,
    taa: Option<TaaTargets>,
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
    resolve_layout: wgpu::BindGroupLayout,
    resolve: wgpu::RenderPipeline,
    taa_layout: wgpu::BindGroupLayout,
    taa_pipeline: wgpu::RenderPipeline,
    taa_buffer: wgpu::Buffer,
    /// History written last frame (index into `TaaTargets::history`).
    taa_parity: usize,
    /// The history holds a previous frame (false after (re)allocation).
    taa_valid: bool,
    fxaa_layout: wgpu::BindGroupLayout,
    fxaa: wgpu::RenderPipeline,
    /// Scene view format; the FXAA input texture shares it.
    output: wgpu::TextureFormat,
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
    /// TAA parameters of this frame (shader `Taa` struct), when TAA is on.
    pub taa: Option<[[f32; 4]; 6]>,
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
        let ms_texture = |binding: u32, sample_type: wgpu::TextureSampleType| {
            entry(
                binding,
                fragment,
                wgpu::BindingType::Texture {
                    sample_type,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: true,
                },
            )
        };
        let unfiltered = wgpu::TextureSampleType::Float { filterable: false };
        let resolve_layout = layout(
            device,
            "MSAA resolve inputs",
            &[
                ms_texture(0, unfiltered),
                ms_texture(1, unfiltered),
                ms_texture(2, unfiltered),
                ms_texture(3, wgpu::TextureSampleType::Depth),
            ],
        );
        let resolve_shader = module(
            device,
            "MSAA resolve",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/aa_resolve.wgsl")
            ),
        );
        let resolve = {
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("MSAA resolve"),
                bind_group_layouts: &[Some(&resolve_layout)],
                immediate_size: 0,
            });
            let targets: Vec<_> = SCENE_TARGETS
                .iter()
                .map(|&format| {
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })
                })
                .collect();
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("MSAA resolve"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &resolve_shader,
                    entry_point: Some("vs_fullscreen"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &resolve_shader,
                    entry_point: Some("fs_resolve"),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: Default::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let taa_layout = layout(
            device,
            "TAA inputs",
            &[
                entry(
                    0,
                    fragment,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(TAA_BYTES),
                    },
                ),
                texture_entry(1, fragment, false),
                depth_entry(2),
                texture_entry(3, fragment, true),
                entry(
                    4,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        );
        let taa_shader = module(
            device,
            "TAA",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/aa_taa.wgsl")
            ),
        );
        let taa_pipeline = fullscreen(
            device,
            "TAA",
            &taa_layout,
            &taa_shader,
            "fs_taa",
            HDR_FORMAT,
            None,
        );
        let taa_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("TAA parameters"),
            size: TAA_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fxaa_layout = layout(
            device,
            "FXAA inputs",
            &[
                texture_entry(0, fragment, true),
                entry(
                    1,
                    fragment,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        );
        let fxaa_shader = module(
            device,
            "FXAA",
            concat!(
                include_str!("shaders/post_common.wgsl"),
                include_str!("shaders/aa_fxaa.wgsl")
            ),
        );
        let fxaa = fullscreen(
            device,
            "FXAA",
            &fxaa_layout,
            &fxaa_shader,
            "fs_fxaa",
            output,
            None,
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
            resolve_layout,
            resolve,
            taa_layout,
            taa_pipeline,
            taa_buffer,
            taa_parity: 0,
            taa_valid: false,
            fxaa_layout,
            fxaa,
            output,
            targets: None,
            half_res_ao: true,
            frame: 0,
            last_frame: None,
        }
    }

    pub(crate) fn exposure_buffer(&self) -> &wgpu::Buffer {
        &self.exposure
    }

    /// (Re)creates transient targets when the viewport, AO resolution, MSAA
    /// sample count or FXAA use changes.
    pub(crate) fn ensure(
        &mut self,
        device: &wgpu::Device,
        size: [u32; 2],
        half_res_ao: bool,
        samples: u32,
        fxaa: bool,
        taa: bool,
    ) {
        let size = [size[0].max(1), size[1].max(1)];
        if self.targets.as_ref().is_some_and(|t| {
            t.size == size
                && self.half_res_ao == half_res_ao
                && t.samples == samples
                && t.ldr.is_some() == fxaa
                && t.taa.is_some() == taa
        }) {
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
        let ms = (samples > 1).then(|| {
            let direct = ms_target(device, "MSAA direct radiance", size, HDR_FORMAT, samples);
            let normal = ms_target(device, "MSAA view normals", size, NORMAL_FORMAT, samples);
            let ambient = ms_target(device, "MSAA ambient radiance", size, HDR_FORMAT, samples);
            let depth = ms_target(device, "MSAA reverse-Z", size, DEPTH_FORMAT, samples);
            let resolve_group = group(
                "MSAA resolve inputs",
                &self.resolve_layout,
                &[
                    view(0, &direct.view),
                    view(1, &normal.view),
                    view(2, &ambient.view),
                    view(3, &depth.view),
                ],
            );
            MsTargets {
                direct,
                normal,
                ambient,
                depth,
                resolve_group,
            }
        });
        let ldr = fxaa.then(|| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Tonemapped image before FXAA"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.output.remove_srgb_suffix(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[self.output],
            });
            let render_view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Tonemapped image (sRGB render view)"),
                format: Some(self.output),
                ..Default::default()
            });
            let read_view = texture.create_view(&Default::default());
            let fxaa_group = group(
                "FXAA inputs",
                &self.fxaa_layout,
                &[
                    view(0, &read_view),
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            );
            LdrTarget {
                _texture: texture,
                render_view,
                fxaa_group,
            }
        });
        let taa = taa.then(|| {
            let history = [
                target(device, "TAA history A", size, HDR_FORMAT),
                target(device, "TAA history B", size, HDR_FORMAT),
            ];
            let groups = [0, 1].map(|index: usize| {
                group(
                    "TAA inputs",
                    &self.taa_layout,
                    &[
                        buffer(0, &self.taa_buffer),
                        view(1, &direct.view),
                        view(2, &depth.view),
                        view(3, &history[1 - index].view),
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                )
            });
            TaaTargets { history, groups }
        });
        // New history textures hold nothing yet.
        self.taa_valid = false;
        let targets = Targets {
            samples,
            ms,
            ldr,
            taa,
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
    /// The multisampled targets when MSAA is on; [`Self::encode`] resolves them.
    pub(crate) fn scene_views(&self) -> Option<([&wgpu::TextureView; 3], &wgpu::TextureView)> {
        self.targets.as_ref().map(|t| match &t.ms {
            Some(ms) => (
                [&ms.direct.view, &ms.normal.view, &ms.ambient.view],
                &ms.depth.view,
            ),
            None => (
                [&t.direct.view, &t.normal.view, &t.ambient.view],
                &t.depth.view,
            ),
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
        if let Some(ms) = &t.ms {
            // One pass resolves the colour MRT (weighted) and the depth.
            let attachment = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("MSAA resolve"),
                color_attachments: &[
                    attachment(&t.direct.view),
                    attachment(&t.normal.view),
                    attachment(&t.ambient.view),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.resolve);
            pass.set_bind_group(0, &ms.resolve_group, &[]);
            pass.draw(0..3, 0..1);
        }
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
        if let (Some(taa), Some(mut values)) = (&t.taa, frame.taa) {
            // Lit HDR (direct + ambient × AO) is final here: TAA writes the
            // other history and copies it back over `direct`, so exposure,
            // bloom and tonemap read the converged image.
            if !self.taa_valid {
                values[5][2] = 1.0;
            }
            queue.write_buffer(
                &self.taa_buffer,
                0,
                &values
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            );
            let index = 1 - self.taa_parity;
            pass(
                encoder,
                "TAA",
                &taa.history[index].view,
                clear,
                None,
                &self.taa_pipeline,
                &taa.groups[index],
            );
            encoder.copy_texture_to_texture(
                taa.history[index]._texture.as_image_copy(),
                t.direct._texture.as_image_copy(),
                wgpu::Extent3d {
                    width: t.size[0],
                    height: t.size[1],
                    depth_or_array_layers: 1,
                },
            );
            self.taa_parity = index;
            self.taa_valid = true;
        } else {
            self.taa_valid = false;
        }
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
        // With FXAA the tonemap writes an intermediate that FXAA filters into
        // `output`; both count as the tonemap scope.
        let tonemap_view = t.ldr.as_ref().map_or(output, |ldr| &ldr.render_view);
        pass(
            encoder,
            "Tonemap",
            tonemap_view,
            clear,
            timestamps.and_then(|q| q.pass_writes_partial(pair::TONEMAP, true, t.ldr.is_none())),
            &self.tonemap,
            &t.tonemap_group,
        );
        if let Some(ldr) = &t.ldr {
            pass(
                encoder,
                "FXAA",
                output,
                clear,
                timestamps.and_then(|q| q.pass_writes_partial(pair::TONEMAP, false, true)),
                &self.fxaa,
                &ldr.fxaa_group,
            );
        }
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
            (
                "msaa resolve",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/aa_resolve.wgsl")
                ),
            ),
            (
                "fxaa",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/aa_fxaa.wgsl")
                ),
            ),
            (
                "taa",
                concat!(
                    include_str!("shaders/post_common.wgsl"),
                    include_str!("shaders/aa_taa.wgsl")
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
