//! GPU height/normal atlas for planetary terrain (ADR 0016).
//!
//! A compute producer writes band-limited heights and per-pixel normals for
//! requested quadtree nodes straight into texture-array layers; there is no CPU
//! payload or upload queue. Selected nodes are then drawn with one instanced
//! draw over a shared grid, using CDLOD morphing towards the coarser grid and a
//! short arrival fade from ancestor data. Atlas contents are disposable derived
//! data: the app owns layer assignment, identity and residency, and the world
//! crate's CPU reference remains the terrain authority.
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

/// Largest number of producer jobs accepted in one frame.
pub const MAX_ATLAS_JOBS_PER_FRAME: usize = 256;
const TILE_BYTES: u64 = 448;
const INSTANCE_BYTES: u64 = 176;
const DISPATCH_STRIDE: u64 = 256;
const MAX_DISPATCHES: u64 = 2 * MAX_ATLAS_JOBS_PER_FRAME as u64;
const READBACK_SLOTS: usize = 4;
/// Height bounds are reported per cell of a 4x4 grid over each tile.
pub const ATLAS_BOUNDS_GRID: usize = 4;
const BOUNDS_WORDS_PER_JOB: usize = 2 * ATLAS_BOUNDS_GRID * ATLAS_BOUNDS_GRID;
const BOUNDS_BYTES: u64 = (4 * BOUNDS_WORDS_PER_JOB * MAX_ATLAS_JOBS_PER_FRAME) as u64;

/// Representation policy shared by producer, residency and draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainAtlasConfig {
    /// Data cells per atlas tile edge (power of two).
    pub cells: u32,
    /// Grid cells per drawn node edge; a node samples a sub-rectangle of a
    /// data tile `log2(cells / draw_cells)` levels above it.
    pub draw_cells: u32,
    pub layers: u32,
    /// Normal-map texels per geometry cell (1 or 2).
    pub normal_scale: u32,
}

impl TerrainAtlasConfig {
    pub fn height_side(self) -> u32 {
        self.cells + 3
    }
    pub fn normal_side(self) -> u32 {
        self.cells * self.normal_scale + 3
    }
    pub fn validate(self, layer_limit: u32) -> Result<(), String> {
        if !self.cells.is_power_of_two() || !(16..=256).contains(&self.cells) {
            return Err(format!(
                "atlas cells {} must be a power of two in 16..=256",
                self.cells
            ));
        }
        if !self.draw_cells.is_power_of_two() || !(8..=self.cells).contains(&self.draw_cells) {
            return Err(format!(
                "draw cells {} must be a power of two in 8..={}",
                self.draw_cells, self.cells
            ));
        }
        if !(1..=2).contains(&self.normal_scale) {
            return Err(format!("normal scale {} must be 1 or 2", self.normal_scale));
        }
        if self.layers < 32 || self.layers > layer_limit {
            return Err(format!(
                "atlas layers {} must be within 32..={layer_limit}",
                self.layers
            ));
        }
        Ok(())
    }
}

/// One pre-filtered level of a periodic u16 profile image.
#[derive(Debug, Clone)]
pub struct AtlasImageLevel {
    pub width: u32,
    pub height: u32,
    pub values: Arc<[u16]>,
}

/// Immutable per-body producer input.
#[derive(Debug, Clone)]
pub enum AtlasSource {
    Profile {
        macro_levels: Vec<AtlasImageLevel>,
        /// `None` reuses the macro image.
        detail_levels: Option<Vec<AtlasImageLevel>>,
        sample_range: [u16; 2],
    },
    Fields(Box<AtlasFieldsConstants>),
}

/// MoonFieldsV1 constants mirrored by the GPU producer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasFieldsConstants {
    pub axes: [[f32; 3]; 4],
    pub basins: [[f32; 3]; 8],
    /// Per basin `[scale, bowl depth]`.
    pub basin_params: [[f32; 2]; 8],
    /// Columns of the field rotation (field-to-body).
    pub rotation_columns: [[f32; 3]; 3],
    pub structure_weights: [f32; 4],
    /// `[offset, low, high, basin rim strength]`.
    pub plains: [f32; 4],
    /// `[R * relief * weight0, ... weight1, ... weight2, radius]`.
    pub relief: [f32; 4],
    /// Per band `[edge, height budget, support radius]`.
    pub bands: [[f32; 3]; 3],
    /// Per band lattice salts for layouts 0 and 1.
    pub salts: [[u64; 2]; 3],
    /// `[start, span]` of crater regional strength.
    pub regional_strength: [f32; 2],
    /// bowl base/freshness, rim base/freshness, ejecta base/freshness,
    /// degradation offset/low/high/strength, jitter, shell.
    pub crater: [f32; 12],
}

/// Cube-chart placement of one tile: chart-centre direction `n0` with cube
/// point length `q0_length`, face axes and the chart width on the cube face.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasChart {
    pub n0: [f32; 3],
    pub q0_length: f32,
    pub face_u: [f32; 3],
    pub width: f32,
    pub face_v: [f32; 3],
}

/// One profile layer prepared for a tile: chart origin per body-axis
/// component (already reduced modulo the mip width) and `K = 0.5 F W`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasProfileLayer {
    pub origin: [f32; 3],
    pub scale: f32,
    pub mip: u32,
    pub width: u32,
    pub amplitude_m: f32,
    pub cubic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AtlasTileKind {
    Profile {
        macro_layers: Vec<AtlasProfileLayer>,
        detail_layers: Vec<AtlasProfileLayer>,
    },
    Fields {
        /// Base lattice cell per band * 2 + layout.
        cells: [[i32; 3]; 6],
        fractions: [[f32; 3]; 6],
        band_weights: [f32; 3],
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AtlasProduceJob {
    pub source: u64,
    pub layer: u32,
    /// Echoed with the produced height bounds.
    pub token: u64,
    pub chart: AtlasChart,
    pub radius_m: f32,
    pub kind: AtlasTileKind,
}

/// Atlas layer plus the sub-rectangle `[origin, origin + scale]` of its chart
/// that covers the drawn node.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasSampleSource {
    pub layer: u32,
    pub origin: [f32; 2],
    pub scale: f32,
}

/// One drawn node. Transforms are camera-relative and already narrowed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasInstance {
    pub anchor_view_m: [f32; 3],
    pub radius_m: f32,
    pub body_to_view: [[f32; 3]; 3],
    pub chart: AtlasChart,
    pub level: u8,
    pub own: AtlasSampleSource,
    pub parent: AtlasSampleSource,
    pub morph_start_m: f32,
    pub morph_end_m: f32,
    /// 0 = parent data, 1 = own data fully arrived.
    pub arrival: f32,
    pub skirt_m: f32,
    pub sun_body: [f32; 3],
    pub mode: u32,
}

/// Per-frame atlas work and draw list staged by the app.
#[derive(Debug, Clone, Default)]
pub struct TerrainAtlasFrame {
    pub config: Option<TerrainAtlasConfig>,
    pub sources: Vec<(u64, Arc<AtlasSource>)>,
    pub jobs: Vec<AtlasProduceJob>,
    pub instances: Vec<AtlasInstance>,
}

/// Produced radial height ranges of one job over a 4x4 grid of its chart
/// (row-major, `[min, max]`), delivered a few frames later. Cells include
/// samples on their shared edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtlasBounds {
    pub token: u64,
    pub cells: [[f32; 2]; ATLAS_BOUNDS_GRID * ATLAS_BOUNDS_GRID],
}

impl AtlasBounds {
    pub fn range(&self) -> [f32; 2] {
        self.cells
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |acc, c| {
                [acc[0].min(c[0]), acc[1].max(c[1])]
            })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerrainAtlasReport {
    pub jobs: u32,
    pub instances: u32,
    pub sources: u32,
    pub dropped_readbacks: u32,
}

struct SourceGpu {
    _textures: [wgpu::Texture; 2],
    _constants: [wgpu::Buffer; 2],
    group: wgpu::BindGroup,
    last_used: u64,
}

struct Readback {
    buffer: wgpu::Buffer,
    tokens: Vec<u64>,
    // 0 idle, 1 copy recorded, 2 mapping, 3 mapped, 4 failed
    state: Arc<AtomicU8>,
}

pub(crate) struct TerrainAtlasRenderer {
    config: TerrainAtlasConfig,
    _height: wgpu::Texture,
    _normal: wgpu::Texture,
    produce_heights: wgpu::ComputePipeline,
    produce_normals: wgpu::ComputePipeline,
    source_layout: wgpu::BindGroupLayout,
    produce_group: wgpu::BindGroup,
    tiles: wgpu::Buffer,
    dispatch: wgpu::Buffer,
    bounds: wgpu::Buffer,
    readbacks: Vec<Readback>,
    sources: std::collections::HashMap<u64, SourceGpu>,
    pipeline: wgpu::RenderPipeline,
    draw_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    draw_layout: wgpu::BindGroupLayout,
    height_view: wgpu::TextureView,
    normal_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    grid_uniform: wgpu::Buffer,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    staged_instances: u32,
    frame: u64,
    results: Vec<AtlasBounds>,
    report: TerrainAtlasReport,
}

impl TerrainAtlasRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        projection_layout: &wgpu::BindGroupLayout,
        config: TerrainAtlasConfig,
    ) -> Result<Self, String> {
        config.validate(device.limits().max_texture_array_layers)?;
        let array = |side: u32, format, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: config.layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let height = array(
            config.height_side(),
            wgpu::TextureFormat::R32Float,
            "Terrain atlas heights",
        );
        let normal = array(
            config.normal_side(),
            wgpu::TextureFormat::Rgba8Snorm,
            "Terrain atlas normals",
        );
        let array_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let height_view = array_view(&height);
        let normal_view = array_view(&normal);

        let produce_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Terrain atlas producer"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/terrain_atlas_produce.wgsl").into(),
            ),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_texture = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        };
        let produce_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Terrain atlas producer outputs"),
            entries: &[
                storage(0, true),
                storage_texture(1, wgpu::TextureFormat::R32Float),
                storage_texture(2, wgpu::TextureFormat::Rgba8Snorm),
                storage(3, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let image = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let source_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Terrain atlas producer source"),
            entries: &[image(0), image(1), uniform(2), uniform(3)],
        });
        let produce_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Terrain atlas producer layout"),
                bind_group_layouts: &[&produce_layout, &source_layout],
                push_constant_ranges: &[],
            });
        let compute = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&produce_pipeline_layout),
                module: &produce_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let produce_heights = compute("produce_heights");
        let produce_normals = compute("produce_normals");
        let tiles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas producer jobs"),
            size: TILE_BYTES * MAX_ATLAS_JOBS_PER_FRAME as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dispatch = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas producer dispatches"),
            size: DISPATCH_STRIDE * MAX_DISPATCHES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bounds = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas produced height bounds"),
            size: BOUNDS_BYTES,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readbacks = (0..READBACK_SLOTS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Terrain atlas bounds readback"),
                    size: BOUNDS_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                tokens: Vec::new(),
                state: Arc::new(AtomicU8::new(0)),
            })
            .collect();
        let storage_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let produce_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Terrain atlas producer outputs"),
            layout: &produce_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: tiles.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&storage_view(&height)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&storage_view(&normal)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bounds.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &dispatch,
                        offset: 0,
                        size: wgpu::BufferSize::new(16),
                    }),
                },
            ],
        });

        // Draw resources.
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Terrain atlas draw"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain_atlas.wgsl").into()),
        });
        let vertex_fragment = wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Terrain atlas draw resources"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                    visibility: vertex_fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: vertex_fragment,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(32),
                    },
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Terrain atlas normal sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let grid_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas grid"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_capacity = 512;
        let instances = instance_buffer(device, instance_capacity);
        let draw_group = draw_group(
            device,
            &draw_layout,
            &height_view,
            &normal_view,
            &sampler,
            &instances,
            &grid_uniform,
        );
        let draw_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Terrain atlas draw layout"),
            bind_group_layouts: &[projection_layout, &draw_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Terrain atlas reverse-Z instanced draw"),
            layout: Some(&draw_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
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
                cull_mode: None,
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
        });
        // Static grid contents are uploaded by the first `prepare`.
        let (vertex_bytes, index_values) = grid_mesh(config.draw_cells);
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas grid vertices"),
            size: vertex_bytes.len() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas grid indices"),
            size: (index_values.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Ok(Self {
            config,
            _height: height,
            _normal: normal,
            produce_heights,
            produce_normals,
            source_layout,
            produce_group,
            tiles,
            dispatch,
            bounds,
            readbacks,
            sources: Default::default(),
            pipeline,
            draw_group,
            instances,
            instance_capacity,
            draw_layout,
            height_view,
            normal_view,
            sampler,
            grid_uniform,
            vertices,
            indices,
            index_count: index_values.len() as u32,
            staged_instances: 0,
            frame: 0,
            results: Vec::new(),
            report: TerrainAtlasReport::default(),
        })
    }

    pub(crate) fn config(&self) -> TerrainAtlasConfig {
        self.config
    }

    pub(crate) fn report(&self) -> TerrainAtlasReport {
        self.report
    }

    pub(crate) fn take_bounds(&mut self) -> Vec<AtlasBounds> {
        std::mem::take(&mut self.results)
    }

    fn collect_readbacks(&mut self) {
        for readback in &mut self.readbacks {
            match readback.state.load(Ordering::Acquire) {
                3 => {
                    {
                        let view = readback.buffer.slice(..).get_mapped_range();
                        for (index, token) in readback.tokens.iter().enumerate() {
                            let word = |offset: usize| {
                                let start = (index * BOUNDS_WORDS_PER_JOB + offset) * 4;
                                u32::from_le_bytes(view[start..start + 4].try_into().unwrap())
                            };
                            let mut cells = [[0.0f32; 2]; ATLAS_BOUNDS_GRID * ATLAS_BOUNDS_GRID];
                            let mut valid = true;
                            for (cell, value) in cells.iter_mut().enumerate() {
                                let (low, high) = (word(cell * 2), word(cell * 2 + 1));
                                valid &= low <= high;
                                *value = [unordered(low), unordered(high)];
                            }
                            if valid {
                                self.results.push(AtlasBounds {
                                    token: *token,
                                    cells,
                                });
                            }
                        }
                    }
                    readback.buffer.unmap();
                    readback.tokens.clear();
                    readback.state.store(0, Ordering::Release);
                }
                4 => {
                    readback.tokens.clear();
                    readback.state.store(0, Ordering::Release);
                }
                _ => {}
            }
        }
    }

    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: &TerrainAtlasFrame,
    ) -> Result<(), String> {
        self.frame += 1;
        if self.frame == 1 {
            let (vertex_bytes, index_values) = grid_mesh(self.config.draw_cells);
            queue.write_buffer(&self.vertices, 0, &vertex_bytes);
            let index_bytes: Vec<u8> = index_values.iter().flat_map(|i| i.to_le_bytes()).collect();
            queue.write_buffer(&self.indices, 0, &index_bytes);
            let grid = [
                self.config.cells as f32,
                (self.config.cells * self.config.normal_scale) as f32,
                self.config.height_side() as f32,
                self.config.normal_side() as f32,
                self.config.draw_cells as f32,
                0.0,
                0.0,
                0.0,
            ];
            queue.write_buffer(&self.grid_uniform, 0, &f32_bytes(&grid));
        }
        self.collect_readbacks();
        if frame.jobs.len() > MAX_ATLAS_JOBS_PER_FRAME {
            return Err(format!(
                "{} atlas jobs exceed the per-frame cap",
                frame.jobs.len()
            ));
        }
        for (key, source) in &frame.sources {
            if let Some(existing) = self.sources.get_mut(key) {
                existing.last_used = self.frame;
            } else {
                let gpu = upload_source(device, queue, &self.source_layout, source)?;
                self.sources.insert(
                    *key,
                    SourceGpu {
                        last_used: self.frame,
                        ..gpu
                    },
                );
            }
        }
        let frame_number = self.frame;
        self.sources
            .retain(|_, source| frame_number - source.last_used < 600);

        self.report = TerrainAtlasReport {
            jobs: frame.jobs.len() as u32,
            instances: frame.instances.len() as u32,
            sources: self.sources.len() as u32,
            dropped_readbacks: self.report.dropped_readbacks,
        };

        if !frame.jobs.is_empty() {
            let mut ordered: Vec<&AtlasProduceJob> = frame.jobs.iter().collect();
            ordered.sort_by_key(|job| job.source);
            let mut tile_bytes = Vec::with_capacity(ordered.len() * TILE_BYTES as usize);
            for job in &ordered {
                if job.layer >= self.config.layers {
                    return Err(format!("atlas job layer {} out of range", job.layer));
                }
                if !self.sources.contains_key(&job.source) {
                    return Err(format!(
                        "atlas job references unbound source {}",
                        job.source
                    ));
                }
                pack_tile(&mut tile_bytes, job);
            }
            queue.write_buffer(&self.tiles, 0, &tile_bytes);
            let mut initial = Vec::with_capacity(ordered.len() * BOUNDS_WORDS_PER_JOB * 4);
            for _ in 0..ordered.len() * BOUNDS_WORDS_PER_JOB / 2 {
                initial.extend_from_slice(&u32::MAX.to_le_bytes());
                initial.extend_from_slice(&0u32.to_le_bytes());
            }
            queue.write_buffer(&self.bounds, 0, &initial);

            // Group consecutive jobs per source; one dispatch pair per group.
            let mut groups: Vec<(u64, u32, u32)> = Vec::new();
            for (index, job) in ordered.iter().enumerate() {
                match groups.last_mut() {
                    Some((source, _, count)) if *source == job.source => *count += 1,
                    _ => groups.push((job.source, index as u32, 1)),
                }
            }
            let separate_normals = self.config.normal_scale > 1;
            let mut dispatch_bytes = vec![0u8; (DISPATCH_STRIDE * MAX_DISPATCHES) as usize];
            let mut dispatches = Vec::new();
            for &(source, base, count) in &groups {
                let mut push = |side: u32, cells: u32, heights: bool| {
                    let slot = dispatches.len();
                    let offset = slot * DISPATCH_STRIDE as usize;
                    for (i, value) in [base, side, cells, base].into_iter().enumerate() {
                        dispatch_bytes[offset + i * 4..offset + i * 4 + 4]
                            .copy_from_slice(&value.to_le_bytes());
                    }
                    dispatches.push((source, slot as u32, side, count, heights));
                };
                push(self.config.height_side(), self.config.cells, true);
                if separate_normals {
                    push(
                        self.config.normal_side(),
                        self.config.cells * self.config.normal_scale,
                        false,
                    );
                }
            }
            queue.write_buffer(
                &self.dispatch,
                0,
                &dispatch_bytes[..dispatches.len() * DISPATCH_STRIDE as usize],
            );
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Terrain atlas producer"),
                    timestamp_writes: None,
                });
                for &(source, slot, side, count, heights) in &dispatches {
                    pass.set_pipeline(if heights {
                        &self.produce_heights
                    } else {
                        &self.produce_normals
                    });
                    pass.set_bind_group(0, &self.produce_group, &[slot * DISPATCH_STRIDE as u32]);
                    pass.set_bind_group(1, &self.sources[&source].group, &[]);
                    let groups_xy = side.div_ceil(8);
                    pass.dispatch_workgroups(groups_xy, groups_xy, count);
                }
            }
            if let Some(readback) = self
                .readbacks
                .iter_mut()
                .find(|r| r.state.load(Ordering::Acquire) == 0)
            {
                encoder.copy_buffer_to_buffer(
                    &self.bounds,
                    0,
                    &readback.buffer,
                    0,
                    (ordered.len() * BOUNDS_WORDS_PER_JOB * 4) as u64,
                );
                readback.tokens = ordered.iter().map(|job| job.token).collect();
                readback.state.store(1, Ordering::Release);
            } else {
                self.report.dropped_readbacks += 1;
            }
        }

        let count = frame.instances.len() as u64;
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instances = instance_buffer(device, self.instance_capacity);
            self.draw_group = draw_group(
                device,
                &self.draw_layout,
                &self.height_view,
                &self.normal_view,
                &self.sampler,
                &self.instances,
                &self.grid_uniform,
            );
        }
        if count > 0 {
            let mut bytes = Vec::with_capacity((count * INSTANCE_BYTES) as usize);
            for instance in &frame.instances {
                pack_instance(&mut bytes, instance);
            }
            queue.write_buffer(&self.instances, 0, &bytes);
        }
        self.staged_instances = count as u32;
        Ok(())
    }

    /// Start mapping bounds copied in the just-submitted command buffer.
    pub(crate) fn on_submitted(&mut self) {
        for readback in &mut self.readbacks {
            if readback.state.load(Ordering::Acquire) != 1 {
                continue;
            }
            readback.state.store(2, Ordering::Release);
            let state = Arc::clone(&readback.state);
            readback
                .buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    state.store(if result.is_ok() { 3 } else { 4 }, Ordering::Release);
                });
        }
    }

    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, projection_group: &wgpu::BindGroup) {
        if self.staged_instances == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, projection_group, &[]);
        pass.set_bind_group(1, &self.draw_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..self.staged_instances);
    }
}

/// Heights and normals of one produced layer, read back for validation.
#[derive(Debug, Clone)]
pub struct ProducedTileReadback {
    pub heights: Vec<f32>,
    pub normals: Vec<[f32; 3]>,
    pub bounds: Option<(f32, f32)>,
}

/// Run the producer for `jobs` on a caller-owned device and read every job's
/// layer back. Validation-only path: blocking, allocates per call.
pub fn produce_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: TerrainAtlasConfig,
    sources: &[(u64, Arc<AtlasSource>)],
    jobs: &[AtlasProduceJob],
) -> Result<Vec<ProducedTileReadback>, String> {
    let projection_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Validation projection"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(64),
            },
            count: None,
        }],
    });
    let mut atlas = TerrainAtlasRenderer::new(
        device,
        wgpu::TextureFormat::Rgba8Unorm,
        &projection_layout,
        config,
    )?;
    let frame = TerrainAtlasFrame {
        config: Some(config),
        sources: sources.to_vec(),
        jobs: jobs.to_vec(),
        instances: Vec::new(),
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    atlas.prepare(device, queue, &mut encoder, &frame)?;
    let read_layer = |encoder: &mut wgpu::CommandEncoder,
                      texture: &wgpu::Texture,
                      side: u32,
                      layer: u32|
     -> (wgpu::Buffer, u32) {
        let row = (side * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atlas validation readback"),
            size: u64::from(row * side),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(side),
                },
            },
            wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
        );
        (buffer, row)
    };
    let mut buffers = Vec::new();
    for job in jobs {
        let heights = read_layer(
            &mut encoder,
            &atlas._height,
            config.height_side(),
            job.layer,
        );
        let normals = read_layer(
            &mut encoder,
            &atlas._normal,
            config.normal_side(),
            job.layer,
        );
        buffers.push((heights, normals));
    }
    queue.submit([encoder.finish()]);
    atlas.on_submitted();
    let mut output = Vec::new();
    for ((heights, height_row), (normals, normal_row)) in &buffers {
        for buffer in [heights, normals] {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        let side = config.height_side() as usize;
        let view = heights.slice(..).get_mapped_range();
        let mut height_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *height_row as usize + x * 4;
                height_values.push(f32::from_le_bytes(view[at..at + 4].try_into().unwrap()));
            }
        }
        drop(view);
        let side = config.normal_side() as usize;
        let view = normals.slice(..).get_mapped_range();
        let mut normal_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *normal_row as usize + x * 4;
                let snorm = |b: u8| (f32::from(b as i8) / 127.0).max(-1.0);
                normal_values.push([snorm(view[at]), snorm(view[at + 1]), snorm(view[at + 2])]);
            }
        }
        drop(view);
        output.push(ProducedTileReadback {
            heights: height_values,
            normals: normal_values,
            bounds: None,
        });
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| error.to_string())?;
    atlas.collect_readbacks();
    for result in atlas.take_bounds() {
        if let Some(index) = jobs.iter().position(|job| job.token == result.token) {
            let [low, high] = result.range();
            output[index].bounds = Some((low, high));
        }
    }
    Ok(output)
}

fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terrain atlas instances"),
        size: capacity * INSTANCE_BYTES,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn draw_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    height: &wgpu::TextureView,
    normal: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    instances: &wgpu::Buffer,
    grid: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Terrain atlas draw resources"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(height),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(normal),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: grid.as_entire_binding(),
            },
        ],
    })
}

fn upload_source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    source: &AtlasSource,
) -> Result<SourceGpu, String> {
    let image = |levels: &[AtlasImageLevel], label| -> Result<wgpu::Texture, String> {
        let first = levels.first().ok_or("profile source has no levels")?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: first.width,
                height: first.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R16Uint,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (mip, level) in levels.iter().enumerate() {
            if level.width != first.width >> mip
                || level.height != first.height >> mip
                || level.values.len() != (level.width * level.height) as usize
            {
                return Err("profile mip chain is not a halving sequence".into());
            }
            let bytes: Vec<u8> = level.values.iter().flat_map(|v| v.to_le_bytes()).collect();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(level.width * 2),
                    rows_per_image: Some(level.height),
                },
                wgpu::Extent3d {
                    width: level.width,
                    height: level.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        Ok(texture)
    };
    let placeholder = || {
        image(
            &[AtlasImageLevel {
                width: 1,
                height: 1,
                values: Arc::from([0u16]),
            }],
            "Terrain atlas unused profile",
        )
    };
    let uniform = |bytes: &[u8], label| {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.len().max(16) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buffer, 0, bytes);
        buffer
    };
    let (macro_texture, detail_texture, profile_bytes, fields_bytes) = match source {
        AtlasSource::Profile {
            macro_levels,
            detail_levels,
            sample_range,
        } => {
            let macro_texture = image(macro_levels, "Terrain atlas macro profile")?;
            let detail_texture = match detail_levels {
                Some(levels) => image(levels, "Terrain atlas detail profile")?,
                None => image(macro_levels, "Terrain atlas detail profile (shared)")?,
            };
            let range = [
                f32::from(sample_range[0]),
                f32::from(sample_range[1]) - f32::from(sample_range[0]),
                0.0,
                0.0,
            ];
            (
                macro_texture,
                detail_texture,
                f32_bytes(&range),
                vec![0u8; FIELDS_CONSTANT_BYTES],
            )
        }
        AtlasSource::Fields(constants) => (
            placeholder()?,
            placeholder()?,
            f32_bytes(&[0.0; 4]),
            pack_fields(constants),
        ),
    };
    let profile_buffer = uniform(&profile_bytes, "Terrain atlas profile constants");
    let fields_buffer = uniform(&fields_bytes, "Terrain atlas field constants");
    let view =
        |texture: &wgpu::Texture| texture.create_view(&wgpu::TextureViewDescriptor::default());
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Terrain atlas producer source"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view(&macro_texture)),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view(&detail_texture)),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: profile_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: fields_buffer.as_entire_binding(),
            },
        ],
    });
    Ok(SourceGpu {
        _textures: [macro_texture, detail_texture],
        _constants: [profile_buffer, fields_buffer],
        group,
        last_used: 0,
    })
}

const FIELDS_CONSTANT_BYTES: usize = 36 * 16;

fn pack_fields(c: &AtlasFieldsConstants) -> Vec<u8> {
    let mut out = Vec::with_capacity(FIELDS_CONSTANT_BYTES);
    let v3 = |out: &mut Vec<u8>, v: [f32; 3], w: f32| out.extend(f32_bytes(&[v[0], v[1], v[2], w]));
    for axis in c.axes {
        v3(&mut out, axis, 0.0);
    }
    for basin in c.basins {
        v3(&mut out, basin, 0.0);
    }
    for params in c.basin_params {
        out.extend(f32_bytes(&[params[0], params[1], 0.0, 0.0]));
    }
    for column in c.rotation_columns {
        v3(&mut out, column, 0.0);
    }
    out.extend(f32_bytes(&c.structure_weights));
    out.extend(f32_bytes(&c.plains));
    out.extend(f32_bytes(&c.relief));
    for band in c.bands {
        v3(&mut out, band, 0.0);
    }
    for salts in c.salts {
        for salt in salts {
            out.extend_from_slice(&(salt as u32).to_le_bytes());
            out.extend_from_slice(&((salt >> 32) as u32).to_le_bytes());
        }
    }
    out.extend(f32_bytes(&[
        c.regional_strength[0],
        c.regional_strength[1],
        0.0,
        0.0,
    ]));
    out.extend(f32_bytes(&c.crater));
    debug_assert_eq!(out.len(), FIELDS_CONSTANT_BYTES);
    out
}

fn pack_tile(out: &mut Vec<u8>, job: &AtlasProduceJob) {
    let start = out.len();
    let chart = job.chart;
    out.extend(f32_bytes(&[
        chart.n0[0],
        chart.n0[1],
        chart.n0[2],
        chart.q0_length,
    ]));
    out.extend(f32_bytes(&[
        chart.face_u[0],
        chart.face_u[1],
        chart.face_u[2],
        chart.width,
    ]));
    out.extend(f32_bytes(&[
        chart.face_v[0],
        chart.face_v[1],
        chart.face_v[2],
        0.0,
    ]));
    let (kind, macro_count, detail_count) = match &job.kind {
        AtlasTileKind::Profile {
            macro_layers,
            detail_layers,
        } => (0u32, macro_layers.len() as u32, detail_layers.len() as u32),
        AtlasTileKind::Fields { .. } => (1, 0, 0),
    };
    for value in [job.layer, kind, macro_count, detail_count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend(f32_bytes(&[job.radius_m, 0.0, 0.0, 0.0]));
    let mut origins = [[0.0f32; 4]; 5];
    let mut infos = [[0.0f32; 4]; 5];
    let mut cells = [[0i32; 4]; 6];
    let mut fractions = [[0.0f32; 4]; 6];
    let mut weights = [0.0f32; 4];
    match &job.kind {
        AtlasTileKind::Profile {
            macro_layers,
            detail_layers,
        } => {
            for (i, layer) in macro_layers.iter().chain(detail_layers).take(5).enumerate() {
                origins[i] = [
                    layer.origin[0],
                    layer.origin[1],
                    layer.origin[2],
                    layer.scale,
                ];
                infos[i] = [
                    layer.mip as f32,
                    layer.width as f32,
                    layer.amplitude_m,
                    if layer.cubic { 1.0 } else { 0.0 },
                ];
            }
        }
        AtlasTileKind::Fields {
            cells: base,
            fractions: fraction,
            band_weights,
        } => {
            for slot in 0..6 {
                cells[slot] = [base[slot][0], base[slot][1], base[slot][2], 0];
                fractions[slot] = [fraction[slot][0], fraction[slot][1], fraction[slot][2], 0.0];
            }
            weights = [band_weights[0], band_weights[1], band_weights[2], 0.0];
        }
    }
    for value in origins.iter().chain(&infos) {
        out.extend(f32_bytes(value));
    }
    for cell in cells {
        for value in cell {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    for value in &fractions {
        out.extend(f32_bytes(value));
    }
    out.extend(f32_bytes(&weights));
    debug_assert_eq!((out.len() - start) as u64, TILE_BYTES);
}

fn pack_instance(out: &mut Vec<u8>, instance: &AtlasInstance) {
    let c = instance.chart;
    let b = instance.body_to_view;
    let rows: [[f32; 4]; 11] = [
        [
            instance.anchor_view_m[0],
            instance.anchor_view_m[1],
            instance.anchor_view_m[2],
            instance.radius_m,
        ],
        [b[0][0], b[0][1], b[0][2], 0.0],
        [b[1][0], b[1][1], b[1][2], 0.0],
        [b[2][0], b[2][1], b[2][2], 0.0],
        [c.n0[0], c.n0[1], c.n0[2], c.q0_length],
        [c.face_u[0], c.face_u[1], c.face_u[2], c.width],
        [
            c.face_v[0],
            c.face_v[1],
            c.face_v[2],
            f32::from(instance.level),
        ],
        [
            instance.own.layer as f32,
            instance.own.origin[0],
            instance.own.origin[1],
            instance.own.scale,
        ],
        [
            instance.parent.layer as f32,
            instance.parent.origin[0],
            instance.parent.origin[1],
            instance.parent.scale,
        ],
        [
            instance.morph_start_m,
            instance.morph_end_m,
            instance.arrival,
            instance.skirt_m,
        ],
        [
            instance.sun_body[0],
            instance.sun_body[1],
            instance.sun_body[2],
            instance.mode as f32,
        ],
    ];
    for row in &rows {
        out.extend(f32_bytes(row));
    }
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn unordered(key: u32) -> f32 {
    let bits = if key & 0x8000_0000 != 0 {
        key & 0x7fff_ffff
    } else {
        !key
    };
    f32::from_bits(bits)
}

/// Shared grid with a one-vertex skirt ring; vertex = (s, t, skirt flag).
pub fn grid_mesh(cells: u32) -> (Vec<u8>, Vec<u32>) {
    let side = cells + 1;
    let mut vertices = Vec::with_capacity(((side * side + 4 * side) * 12) as usize);
    let mut push = |s: f32, t: f32, skirt: f32| {
        vertices.extend(f32_bytes(&[s, t, skirt]));
    };
    for j in 0..side {
        for i in 0..side {
            push(i as f32 / cells as f32, j as f32 / cells as f32, 0.0);
        }
    }
    let grid = |i: u32, j: u32| j * side + i;
    let mut indices = Vec::with_capacity((cells * cells * 6 + 4 * cells * 6) as usize);
    for j in 0..cells {
        for i in 0..cells {
            let (a, b, c, d) = (
                grid(i, j),
                grid(i + 1, j),
                grid(i, j + 1),
                grid(i + 1, j + 1),
            );
            indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    // Skirt ring: each edge gets its own duplicated vertices (z = 1).
    // Edge walk k -> grid (i, j): bottom, right, top (reversed), left (reversed).
    let edge_point = |edge: usize, k: u32| match edge {
        0 => (k, 0),
        1 => (cells, k),
        2 => (cells - k, cells),
        _ => (0, cells - k),
    };
    let mut next = side * side;
    for edge_index in 0..4 {
        let edge = |k| edge_point(edge_index, k);
        let base = next;
        for k in 0..side {
            let (i, j) = edge(k);
            push(i as f32 / cells as f32, j as f32 / cells as f32, 1.0);
            next += 1;
        }
        for k in 0..cells {
            let (i0, j0) = edge(k);
            let (i1, j1) = edge(k + 1);
            let (top0, top1) = (grid(i0, j0), grid(i1, j1));
            let (low0, low1) = (base + k, base + k + 1);
            indices.extend_from_slice(&[top0, low0, top1, top1, low0, low1]);
        }
    }
    (vertices, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_shaders_parse_and_validate() {
        for (name, source) in [
            (
                "produce",
                include_str!("shaders/terrain_atlas_produce.wgsl"),
            ),
            ("draw", include_str!("shaders/terrain_atlas.wgsl")),
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
    fn grid_mesh_has_interior_and_skirt_ring() {
        let (vertices, indices) = grid_mesh(16);
        let vertex_count = vertices.len() / 12;
        assert_eq!(vertex_count, 17 * 17 + 4 * 17);
        assert_eq!(indices.len(), 16 * 16 * 6 + 4 * 16 * 6);
        assert!(indices.iter().all(|&i| (i as usize) < vertex_count));
    }

    #[test]
    fn ordered_float_keys_round_trip_and_sort() {
        let ordered = |value: f32| {
            let bits = value.to_bits();
            if bits & 0x8000_0000 != 0 {
                !bits
            } else {
                bits | 0x8000_0000
            }
        };
        let values = [-300.5f32, -1.0, -0.0, 0.0, 2.5, 600.0];
        for pair in values.windows(2) {
            assert!(ordered(pair[0]) <= ordered(pair[1]));
        }
        for value in values {
            assert_eq!(unordered(ordered(value)).to_bits(), value.to_bits());
        }
    }

    #[test]
    fn packed_sizes_match_shader_layouts() {
        let mut bytes = Vec::new();
        pack_tile(
            &mut bytes,
            &AtlasProduceJob {
                source: 0,
                layer: 0,
                token: 0,
                chart: AtlasChart::default(),
                radius_m: 1.0,
                kind: AtlasTileKind::Fields {
                    cells: [[0; 3]; 6],
                    fractions: [[0.0; 3]; 6],
                    band_weights: [1.0; 3],
                },
            },
        );
        assert_eq!(bytes.len() as u64, TILE_BYTES);
        let mut bytes = Vec::new();
        pack_instance(&mut bytes, &AtlasInstance::default());
        assert_eq!(bytes.len() as u64, INSTANCE_BYTES);
        assert_eq!(
            pack_fields(&AtlasFieldsConstants::default()).len(),
            FIELDS_CONSTANT_BYTES
        );
    }

    #[test]
    fn config_validation_bounds_representation() {
        let ok = TerrainAtlasConfig {
            cells: 128,
            draw_cells: 32,
            layers: 512,
            normal_scale: 2,
        };
        assert!(ok.validate(2048).is_ok());
        assert!(ok.validate(256).is_err());
        assert!(
            TerrainAtlasConfig { cells: 100, ..ok }
                .validate(2048)
                .is_err()
        );
        assert_eq!(ok.height_side(), 131);
        assert_eq!(ok.normal_side(), 259);
    }
}
