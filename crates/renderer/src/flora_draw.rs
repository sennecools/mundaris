//! PROTOTYPE (flora lane): grown species and rocks for the M5 scatter, at
//! every distance.
//!
//! At start-up the species in `content/flora/species/*.ron` (at most
//! `astrum_flora::scatter::MAX_SPECIES`) and the planet's rocks are grown on
//! the CPU (`astrum_flora`, two variants each). Every entry has three tiers:
//! LOD0 mesh, LOD1 mesh, and an impostor baked from LOD1
//! (`astrum_flora::impostor`, 16 hemi-octahedral views, albedo + normal, lit
//! at runtime with the same shading). Their climate niches are compiled into
//! the draw and cull shaders ([`species_shader`]). The cull pass routes every
//! plant of a grown entry into per-(entry, variant, tier) buckets with
//! cross-fade bands at the tier changes (`scatter_cull.wgsl`); bucket sizes
//! come from a count → prefix-sum → scatter pass, so no bucket drops plants.
//! Plants keep their size at every tier; fades are screen-door dithers.
//!
//! `ASTRUM_NO_FLORA=1` keeps every plant procedural (A/B captures).
//!
//! Prototype debt: no hot reload, no wind animation, fixed tier bands in
//! WGSL, growth and baking block start-up (cached per process).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use astrum_flora::hash::{derive, name_key};
use astrum_flora::impostor::{ATLAS, Impostor, bake};
use astrum_flora::palette::{Palette, PlanetRock, from_lch, load_planet};
use astrum_flora::rock::{RockFile, Weathering, grow_rock_style};
use astrum_flora::scatter::{MAX_ROCKS, MAX_SPECIES, species_wgsl, splice_species};
use astrum_flora::{Kit, LOD_COUNT, Mesh, SpeciesFile, grow_meshes, load_species_for_body, variant_seed};
use wgpu::util::DeviceExt;

/// Grown variants per entry (`FLORA_VARIANTS`). Rocks: 0 dry, 1 wet.
pub const VARIANTS: usize = 2;
/// Tiers per variant (`FLORA_TIERS`): LOD0 mesh, LOD1 mesh, impostor.
pub const TIERS: usize = 3;
/// Bucket entries (`FLORA_ENTRIES`): species, then rocks.
pub const ENTRIES: usize = MAX_SPECIES + MAX_ROCKS;
pub const BUCKETS: usize = ENTRIES * VARIANTS * TIERS;
/// First word of the flora draw arguments (`FLORA_ARGS_WORD`), 5 per bucket.
pub const ARGS_WORD: usize = 16;
/// Per-bucket scatter cursors (`FLORA_CURSOR_WORD`).
pub const CURSOR_WORD: usize = 448;
/// Word holding the entry mask (`FLORA_MASK_WORD`).
pub const MASK_WORD: usize = 5;
/// Staged plant count (`FLORA_STAGE_WORD`).
pub const STAGE_WORD: usize = 12;
/// Size of the shared plant argument buffer once flora is included.
pub const ARGS_BYTES: u64 = 4096;
/// Plant buffer layout (`FLORA_*` in scatter_cull.wgsl / scatter_flora.wgsl;
/// the shaders own the values, a test keeps these in step).
#[allow(dead_code)]
pub const FAR_CAPACITY: u32 = 32_768;
#[allow(dead_code)]
pub const HEADER: u32 = 32;
pub const STAGE: u32 = 114_656;
#[allow(dead_code)]
pub const STAGE_START: u32 = FAR_CAPACITY + HEADER;
#[allow(dead_code)]
pub const SORTED_START: u32 = STAGE_START + STAGE;
/// GPU vertex: the 36-byte flora vertex plus its bucket id.
const VERTEX_BYTES: u64 = 40;
/// Impostor vertex: corner|bucket, centre, radius, layer.
const IMPOSTOR_VERTEX_BYTES: u64 = 24;

/// Species, rocks, CPU meshes and impostors, built once per process.
struct Grown {
    species: Vec<SpeciesFile>,
    rocks: Vec<PlanetRock>,
    mask: u32,
    vertices: Vec<u8>,
    indices: Vec<u32>,
    impostor_vertices: Vec<u8>,
    /// Per bucket: mesh tiers (index count, first index, base vertex);
    /// impostor tier (6, first vertex, 0).
    ranges: Vec<(u32, u32, i32)>,
    /// Texture layers (entry * VARIANTS + variant), RGBA8 `ATLAS²` each.
    albedo: Vec<u8>,
    normal: Vec<u8>,
    layers: u32,
}

fn grow_all() -> Grown {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora");
    // PROTOTYPE: Rust is the only body with life; its planet file sets the
    // palette and the rocks.
    let mut species: Vec<SpeciesFile> =
        match load_species_for_body(&root.join("species"), &root.join("planets"), "rust") {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("flora: {e}; using the built-in niche table and procedural plants");
                Vec::new()
            }
        };
    if species.len() > MAX_SPECIES {
        tracing::warn!("flora: {} species, only the first {MAX_SPECIES} are used", species.len());
        species.truncate(MAX_SPECIES);
    }
    let planet = load_planet(&root.join("planets"), "rust").ok();
    let mut rocks: Vec<(PlanetRock, RockFile)> = Vec::new();
    if let Some(p) = &planet {
        for r in p.rocks.iter().take(MAX_ROCKS) {
            match std::fs::read_to_string(root.join("rocks").join(format!("{}.ron", r.name)))
                .map_err(|e| e.to_string())
                .and_then(|t| RockFile::from_ron(&t))
            {
                Ok(file) => rocks.push((r.clone(), file)),
                Err(e) => {
                    tracing::warn!("flora: rock `{}`: {e}; rocks stay procedural", r.name);
                    rocks.clear();
                    break;
                }
            }
        }
    }
    let weathering = |wet: bool| {
        let (age, moss) = planet.as_ref().map_or((0.5, [0.05, 0.08, 0.03]), |p| {
            let pal = Palette::for_planet(p);
            let m = from_lch([pal.foliage[0] - 0.08, pal.foliage[1] * 0.8, pal.foliage[2]]);
            (p.geology_age, m.map(|v| v as f32))
        });
        if wet {
            Weathering { wetness: 0.85, age, moss, moss_cover: 0.7 }
        } else {
            Weathering { wetness: 0.15, age, moss, moss_cover: 0.05 }
        }
    };
    let kit = Kit::builtin();
    let style = planet.as_ref().map(|p| p.foliage).unwrap_or_default();
    let started = std::time::Instant::now();
    // Grow and bake every (entry, variant) on its own thread.
    let jobs: Vec<(usize, usize)> = (0..species.len())
        .chain((0..rocks.len()).map(|r| MAX_SPECIES + r))
        .flat_map(|e| (0..VARIANTS).map(move |v| (e, v)))
        .collect();
    let grown: Vec<([Mesh; LOD_COUNT], Impostor)> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(e, v)| {
                let kit = &kit;
                let species = &species;
                let rocks = &rocks;
                let w = weathering(v == 1);
                scope.spawn(move || {
                    let lods = if e < MAX_SPECIES {
                        let sp = &species[e];
                        grow_meshes(sp, kit, variant_seed(sp, v as u32)).1
                    } else {
                        let rock = &rocks[e - MAX_SPECIES].1;
                        grow_rock_style(rock, derive(name_key(&rock.name), v as u64), &w, style)
                    };
                    let imp = bake(&lods[1]);
                    (lods, imp)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("flora growth thread")).collect()
    });
    let layers = (ENTRIES * VARIANTS) as u32;
    let layer_bytes = ATLAS * ATLAS * 4;
    let mut g = Grown {
        species,
        rocks: rocks.iter().map(|r| r.0.clone()).collect(),
        mask: 0,
        vertices: Vec::new(),
        indices: Vec::new(),
        impostor_vertices: Vec::new(),
        ranges: vec![(0, 0, 0); BUCKETS],
        albedo: vec![0u8; layer_bytes * layers as usize],
        normal: vec![0u8; layer_bytes * layers as usize],
        layers,
    };
    let mut tris = 0usize;
    for (&(e, v), (lods, imp)) in jobs.iter().zip(&grown) {
        g.mask |= 1 << e;
        let ev = e * VARIANTS + v;
        for (tier, mesh) in lods.iter().take(2).enumerate() {
            let bucket = (ev * TIERS + tier) as u32;
            let base_vertex = (g.vertices.len() as u64 / VERTEX_BYTES) as i32;
            let first_index = g.indices.len() as u32;
            for vtx in &mesh.vertices {
                vtx.to_bytes(&mut g.vertices);
                g.vertices.extend_from_slice(&bucket.to_le_bytes());
            }
            g.indices.extend_from_slice(&mesh.indices);
            g.ranges[bucket as usize] = (mesh.indices.len() as u32, first_index, base_vertex);
            tris += mesh.triangles();
        }
        let bucket = (ev * TIERS + 2) as u32;
        let first_vertex = (g.impostor_vertices.len() as u64 / IMPOSTOR_VERTEX_BYTES) as u32;
        for corner in 0..6u32 {
            let iv = &mut g.impostor_vertices;
            iv.extend_from_slice(&(corner | (bucket << 8)).to_le_bytes());
            for c in imp.centre.to_array() {
                iv.extend_from_slice(&c.to_le_bytes());
            }
            iv.extend_from_slice(&imp.radius.to_le_bytes());
            iv.extend_from_slice(&(ev as u32).to_le_bytes());
        }
        g.ranges[bucket as usize] = (6, first_vertex, 0);
        g.albedo[ev * layer_bytes..(ev + 1) * layer_bytes].copy_from_slice(&imp.albedo);
        g.normal[ev * layer_bytes..(ev + 1) * layer_bytes].copy_from_slice(&imp.normal);
    }
    tracing::info!(
        "flora: grew {:?} + rocks {:?} ({} variants, {} triangles, {} KiB meshes, {} KiB impostors) in {:.0} ms",
        g.species.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        g.rocks.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        grown.len(),
        tris,
        (g.vertices.len() + g.indices.len() * 4) / 1024,
        (g.albedo.len() + g.normal.len()) / 1024,
        started.elapsed().as_secs_f64() * 1e3
    );
    g
}

/// `shader` with the niche/rock table generated from the loaded content
/// spliced over the built-in one (pure string transform; unchanged when no
/// species loaded).
pub(crate) fn species_shader(shader: &str) -> String {
    let g = grown();
    let mut out = if g.species.is_empty() { shader.to_string() } else { splice_species(shader, &species_wgsl(&g.species, &g.rocks)) };
    // Debug (measurement): ASTRUM_FLORA_FULL_M overrides the full-density
    // view distance of the plant thinning (SCATTER_FULL_M, metres).
    if let Some(m) = std::env::var("ASTRUM_FLORA_FULL_M").ok().and_then(|v| v.parse::<f32>().ok()) {
        out = out.replace("const SCATTER_FULL_M: f32 = 350.0;", &format!("const SCATTER_FULL_M: f32 = {m:.1};"));
        tracing::info!("flora: SCATTER_FULL_M overridden to {m} m");
    }
    // Debug (measurement): ASTRUM_FLORA_PER_SAMPLE = 0 (all per pixel),
    // 1 (fading instances per sample, the default) or 2 (all per sample).
    if let Some(mode) = std::env::var("ASTRUM_FLORA_PER_SAMPLE").ok().and_then(|v| v.parse::<u32>().ok()).filter(|m| *m <= 2) {
        out = out.replace("const FLORA_PER_SAMPLE: u32 = 1u;", &format!("const FLORA_PER_SAMPLE: u32 = {mode}u;"));
        tracing::info!("flora: per-sample mode {mode} (ASTRUM_FLORA_PER_SAMPLE)");
    }
    out
}

fn grown() -> Arc<Grown> {
    static GROWN: OnceLock<Arc<Grown>> = OnceLock::new();
    GROWN.get_or_init(|| Arc::new(grow_all())).clone()
}

fn flora_enabled() -> bool {
    std::env::var_os("ASTRUM_NO_FLORA").is_none_or(|v| v == "0")
}

/// Last read-back overflow counts: (flora staging/sorted, procedural)
/// rejected because their region was full, packed as hi/lo u32.
static OVERFLOW: AtomicU64 = AtomicU64::new(0);

/// Plants dropped last read-back frame because the flora staging or sorted
/// region (first) or the procedural region (second) was full. Zero means
/// nothing was dropped.
pub fn flora_overflow() -> (u32, u32) {
    let v = OVERFLOW.load(Ordering::Relaxed);
    ((v >> 32) as u32, v as u32)
}

const READBACK_IDLE: u8 = 0;
const READBACK_COPIED: u8 = 1;
const READBACK_MAPPING: u8 = 2;
const READBACK_READY: u8 = 3;

/// Layouts and modules FloraDraw builds its pipelines from (the scatter's).
pub(crate) struct FloraSetup<'a> {
    pub draw_shader: &'a wgpu::ShaderModule,
    pub cull_shader: &'a wgpu::ShaderModule,
    /// Compute layout of the plant cull (groups 0–3 incl. plants_rw).
    pub cull_layout: &'a wgpu::PipelineLayout,
    pub projection_layout: &'a wgpu::BindGroupLayout,
    pub draw_layout: &'a wgpu::BindGroupLayout,
    pub lighting_layout: &'a wgpu::BindGroupLayout,
    pub light_layout: &'a wgpu::BindGroupLayout,
    pub empty_layout: &'a wgpu::BindGroupLayout,
    pub plants: &'a wgpu::Buffer,
    pub color_targets: &'a [Option<wgpu::ColorTargetState>],
}

pub(crate) struct FloraDraw {
    /// Overflow counter read-back (async map, never stalls the frame).
    readback: wgpu::Buffer,
    readback_state: Arc<AtomicU8>,
    pipeline: crate::aa::MsaaPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    impostor_pipeline: crate::aa::MsaaPipeline,
    /// Per-sample variants for fading instances (scatter_flora.wgsl).
    pipeline_ps: crate::aa::MsaaPipeline,
    impostor_pipeline_ps: crate::aa::MsaaPipeline,
    impostor_shadow_pipeline: wgpu::RenderPipeline,
    prefix: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    impostor_vertices: wgpu::Buffer,
    albedo: wgpu::Texture,
    normal: wgpu::Texture,
    uploaded: AtomicBool,
    /// Words written to the plant argument buffer each frame from [`ARGS_WORD`].
    args: Vec<u32>,
    mask: u32,
    trace: Option<Trace>,
}

const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Snorm8x4,
    2 => Unorm8x4,
    3 => Float32x3,
    4 => Uint8x4,
    5 => Uint32,
];

const IMPOSTOR_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Uint32,
    1 => Float32x3,
    2 => Float32,
    3 => Uint32,
];

fn is_impostor(bucket: usize) -> bool {
    bucket % TIERS == 2
}

impl FloraDraw {
    pub(crate) fn new(device: &wgpu::Device, s: FloraSetup<'_>) -> Self {
        let g = grown();
        let mut args = vec![0u32; BUCKETS * 5];
        for (b, &(count, first, base)) in g.ranges.iter().enumerate() {
            args[b * 5] = count;
            args[b * 5 + 2] = first;
            args[b * 5 + 3] = base as u32;
        }
        let nonempty = |v: &[u8], n: usize| if v.is_empty() { vec![0u8; n] } else { v.to_vec() };
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flora vertices"),
            contents: &nonempty(&g.vertices, VERTEX_BYTES as usize),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_bytes: Vec<u8> = if g.indices.is_empty() {
            vec![0u8; 12]
        } else {
            g.indices.iter().flat_map(|i| i.to_le_bytes()).collect()
        };
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flora indices"),
            contents: &index_bytes,
            usage: wgpu::BufferUsages::INDEX,
        });
        let impostor_vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flora impostor vertices"),
            contents: &nonempty(&g.impostor_vertices, IMPOSTOR_VERTEX_BYTES as usize),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let texture = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: ATLAS as u32,
                    height: ATLAS as u32,
                    depth_or_array_layers: g.layers.max(1),
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let albedo = texture("Flora impostor albedo", wgpu::TextureFormat::Rgba8UnormSrgb);
        let normal = texture("Flora impostor normal", wgpu::TextureFormat::Rgba8Unorm);
        let view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Flora impostor sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flora plants + impostors"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let (albedo_view, normal_view) = (view(&albedo), view(&normal));
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Flora plants + impostors"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: s.plants.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&albedo_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&normal_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let draw_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Flora draw layout"),
            bind_group_layouts: &[Some(s.projection_layout), Some(s.draw_layout), Some(s.lighting_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Flora shadow layout"),
            bind_group_layouts: &[Some(s.light_layout), Some(s.draw_layout), Some(s.empty_layout), Some(&layout)],
            immediate_size: 0,
        });
        let mesh_layout = Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        });
        let imp_layout = Some(wgpu::VertexBufferLayout {
            array_stride: IMPOSTOR_VERTEX_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &IMPOSTOR_ATTRIBUTES,
        });
        // Main-pass pipelines: one variant per MSAA sample count (crate::aa).
        let main = |label: &'static str, vs: &'static str, fs: &'static str, impostor: bool| {
            let shader = s.draw_shader.clone();
            let layout = draw_layout.clone();
            let targets = s.color_targets.to_vec();
            crate::aa::MsaaPipeline::new(device, move |device, samples| {
                let buffers = [Some(if impostor {
                    wgpu::VertexBufferLayout {
                        array_stride: IMPOSTOR_VERTEX_BYTES,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &IMPOSTOR_ATTRIBUTES,
                    }
                } else {
                    wgpu::VertexBufferLayout {
                        array_stride: VERTEX_BYTES,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &ATTRIBUTES,
                    }
                })];
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some(vs),
                        compilation_options: Default::default(),
                        buffers: &buffers,
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some(fs),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: crate::aa::multisample(samples),
                    multiview_mask: None,
                    cache: None,
                })
            })
        };
        let shadow = |label, vs, fs, buffers: &[Option<wgpu::VertexBufferLayout<'_>>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: s.draw_shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: s.draw_shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::post::DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 2, slope_scale: 1.5, clamp: 0.0 },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = main("Flora grown plants (prototype)", "vs_flora_px", "fs_flora_px", false);
        let pipeline_ps = main("Flora grown plants, fading (prototype)", "vs_flora_ps", "fs_flora", false);
        let impostor_pipeline =
            main("Flora impostors (prototype)", "vs_impostor", "fs_impostor_px", true);
        let impostor_pipeline_ps = main("Flora impostors, fading (prototype)", "vs_impostor_ps", "fs_impostor", true);
        let shadow_pipeline =
            shadow("Flora sun shadow casters (prototype)", "vs_flora", "fs_flora_shadow", std::slice::from_ref(&mesh_layout));
        let impostor_shadow_pipeline = shadow(
            "Flora impostor shadow casters (prototype)",
            "vs_impostor_shadow",
            "fs_impostor_shadow",
            std::slice::from_ref(&imp_layout),
        );
        let compute = |label, entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(s.cull_layout),
                module: s.cull_shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let prefix = compute("Flora bucket prefix", "cs_flora_prefix");
        let scatter = compute("Flora bucket scatter", "cs_flora_scatter");
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Flora overflow read-back"),
            size: 8,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            readback,
            readback_state: Arc::new(AtomicU8::new(READBACK_IDLE)),
            pipeline,
            shadow_pipeline,
            impostor_pipeline,
            pipeline_ps,
            impostor_pipeline_ps,
            impostor_shadow_pipeline,
            prefix,
            scatter,
            group,
            vertices,
            indices,
            impostor_vertices,
            albedo,
            normal,
            uploaded: AtomicBool::new(false),
            args,
            mask: g.mask,
            trace: Trace::from_env(device),
        }
    }

    /// Upload the impostor textures once (needs the queue).
    pub(crate) fn prepare(&self, queue: &wgpu::Queue) {
        if self.uploaded.swap(true, Ordering::AcqRel) {
            return;
        }
        let g = grown();
        if g.layers == 0 || g.albedo.is_empty() {
            return;
        }
        for (texture, data) in [(&self.albedo, &g.albedo), (&self.normal, &g.normal)] {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ATLAS as u32 * 4),
                    rows_per_image: Some(ATLAS as u32),
                },
                wgpu::Extent3d { width: ATLAS as u32, height: ATLAS as u32, depth_or_array_layers: g.layers },
            );
        }
    }

    /// Fill the flora words of this frame's plant arguments (instance counts,
    /// stage count and cursors zeroed). `words` covers the whole buffer.
    pub(crate) fn write_args(&self, words: &mut [u32]) {
        self.poll_overflow();
        if let Some(t) = &self.trace {
            t.poll();
        }
        words[MASK_WORD] = if flora_enabled() { self.mask } else { 0 };
        words[STAGE_WORD] = 0;
        words[ARGS_WORD..ARGS_WORD + self.args.len()].copy_from_slice(&self.args);
        words[CURSOR_WORD..CURSOR_WORD + BUCKETS].fill(0);
    }

    /// Bucket prefix and scatter, after cs_scatter in the same compute pass
    /// (groups 1 and 3 as bound for it).
    pub(crate) fn cull(&self, pass: &mut wgpu::ComputePass<'_>) {
        if self.mask == 0 || !flora_enabled() {
            return;
        }
        pass.set_pipeline(&self.prefix);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&self.scatter);
        let groups = STAGE.div_ceil(64);
        pass.dispatch_workgroups(groups.min(65_535), groups.div_ceil(65_535), 1);
    }

    /// Copy this frame's overflow words (6, 7) for read-back; call after the
    /// cull pass, before the encoder is submitted. With `ASTRUM_FLORA_TRACE`
    /// set, also copies the draw arguments and the flora plant regions.
    pub(crate) fn copy_counters(&self, encoder: &mut wgpu::CommandEncoder, plant_args: &wgpu::Buffer, plants: &wgpu::Buffer) {
        if self.readback_state.load(Ordering::Acquire) == READBACK_IDLE {
            encoder.copy_buffer_to_buffer(plant_args, 24, &self.readback, 0, 8);
            self.readback_state.store(READBACK_COPIED, Ordering::Release);
        }
        if let Some(t) = &self.trace {
            t.copy(encoder, plant_args, plants);
        }
    }

    /// Advance the read-back: map the copy submitted last frame, or read a
    /// finished map.
    fn poll_overflow(&self) {
        match self.readback_state.load(Ordering::Acquire) {
            READBACK_COPIED => {
                self.readback_state.store(READBACK_MAPPING, Ordering::Release);
                let state = self.readback_state.clone();
                self.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { READBACK_READY } else { READBACK_IDLE }, Ordering::Release);
                });
            }
            READBACK_READY => {
                let (flora, far) = {
                    let data = self.readback.slice(..).get_mapped_range();
                    match data {
                        Ok(d) => (
                            u32::from_le_bytes([d[0], d[1], d[2], d[3]]),
                            u32::from_le_bytes([d[4], d[5], d[6], d[7]]),
                        ),
                        Err(_) => (0, 0),
                    }
                };
                self.readback.unmap();
                let packed = ((flora as u64) << 32) | far as u64;
                if OVERFLOW.swap(packed, Ordering::Relaxed) != packed && packed != 0 {
                    tracing::warn!("flora: plants dropped by full regions: {flora} grown, {far} procedural");
                }
                self.readback_state.store(READBACK_IDLE, Ordering::Release);
            }
            _ => {}
        }
    }

    fn draws(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer, mesh: &wgpu::RenderPipeline, imp: &wgpu::RenderPipeline) {
        if self.mask == 0 || !flora_enabled() {
            return;
        }
        pass.set_bind_group(3, &self.group, &[]);
        pass.set_pipeline(mesh);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for b in (0..BUCKETS).filter(|b| !is_impostor(*b)) {
            if self.args[b * 5] == 0 {
                continue;
            }
            pass.draw_indexed_indirect(plant_args, ((ARGS_WORD + b * 5) * 4) as u64);
        }
        pass.set_pipeline(imp);
        pass.set_vertex_buffer(0, self.impostor_vertices.slice(..));
        for b in (0..BUCKETS).filter(|b| is_impostor(*b)) {
            if self.args[b * 5] == 0 {
                continue;
            }
            pass.draw_indirect(plant_args, ((ARGS_WORD + b * 5) * 4) as u64);
        }
    }

    /// Selects the MSAA sample count of the main-pass pipelines (crate::aa).
    pub(crate) fn set_samples(&mut self, device: &wgpu::Device, samples: u32) {
        self.pipeline.set_samples(device, samples);
        self.impostor_pipeline.set_samples(device, samples);
        self.pipeline_ps.set_samples(device, samples);
        self.impostor_pipeline_ps.set_samples(device, samples);
    }

    /// Main pass; groups 0–2 are the scatter draw's, group 3 is set here.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        // Solid instances per pixel, fading ones per sample.
        self.draws(pass, plant_args, self.pipeline.get(), self.impostor_pipeline.get());
        self.draws(pass, plant_args, self.pipeline_ps.get(), self.impostor_pipeline_ps.get());
    }

    /// Sun shadow pass; groups 0–2 as for the scatter shadow draw.
    pub(crate) fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        self.draws(pass, plant_args, &self.shadow_pipeline, &self.impostor_shadow_pipeline);
    }
}


/// PROTOTYPE debug: `ASTRUM_FLORA_TRACE=<dir>` dumps the draw arguments and
/// the flora plant regions every `ASTRUM_FLORA_TRACE_EVERY` frames (default
/// 2) to `<dir>/flora-<frame>.bin` for the pop scorer
/// (`crates/flora/examples/flora_pop_scorer.rs`) while `<dir>/armed` exists,
/// at most `ASTRUM_FLORA_TRACE_MAX` (300) dumps. File: ARGS_BYTES of argument
/// words, the HEADER plants, then the used sorted plants (64 bytes each).
/// Never stalls: one dump in flight at a time.
struct Trace {
    dir: std::path::PathBuf,
    every: u64,
    frame: std::sync::atomic::AtomicU64,
    buffer: wgpu::Buffer,
    state: Arc<AtomicU8>,
    tag: std::sync::atomic::AtomicU64,
    written: std::sync::atomic::AtomicU64,
    max: u64,
}

const TRACE_PLANTS_BYTES: u64 = (262_144 - FAR_CAPACITY as u64) * 64;

impl Trace {
    fn from_env(device: &wgpu::Device) -> Option<Self> {
        let dir = std::path::PathBuf::from(std::env::var_os("ASTRUM_FLORA_TRACE")?);
        std::fs::create_dir_all(&dir).ok()?;
        let every = std::env::var("ASTRUM_FLORA_TRACE_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(2).max(1);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Flora trace read-back"),
            size: ARGS_BYTES + TRACE_PLANTS_BYTES,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        tracing::info!("flora: tracing plants to {}", dir.display());
        Some(Trace {
            dir,
            every,
            frame: std::sync::atomic::AtomicU64::new(0),
            buffer,
            state: Arc::new(AtomicU8::new(READBACK_IDLE)),
            tag: std::sync::atomic::AtomicU64::new(0),
            written: std::sync::atomic::AtomicU64::new(0),
            max: std::env::var("ASTRUM_FLORA_TRACE_MAX").ok().and_then(|v| v.parse().ok()).unwrap_or(300),
        })
    }

    fn copy(&self, encoder: &mut wgpu::CommandEncoder, args: &wgpu::Buffer, plants: &wgpu::Buffer) {
        let frame = self.frame.fetch_add(1, Ordering::Relaxed);
        // Only while `<dir>/armed` exists, at most `max` dumps.
        if self.written.load(Ordering::Relaxed) >= self.max || !self.dir.join("armed").exists() {
            return;
        }
        if !frame.is_multiple_of(self.every) || self.state.load(Ordering::Acquire) != READBACK_IDLE {
            return;
        }
        encoder.copy_buffer_to_buffer(args, 0, &self.buffer, 0, ARGS_BYTES);
        encoder.copy_buffer_to_buffer(plants, FAR_CAPACITY as u64 * 64, &self.buffer, ARGS_BYTES, TRACE_PLANTS_BYTES);
        self.tag.store(frame, Ordering::Relaxed);
        self.state.store(READBACK_COPIED, Ordering::Release);
    }

    fn poll(&self) {
        match self.state.load(Ordering::Acquire) {
            READBACK_COPIED => {
                self.state.store(READBACK_MAPPING, Ordering::Release);
                let state = self.state.clone();
                self.buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                    state.store(if r.is_ok() { READBACK_READY } else { READBACK_IDLE }, Ordering::Release);
                });
            }
            READBACK_READY => {
                if let Ok(data) = self.buffer.slice(..).get_mapped_range() {
                    let path = self.dir.join(format!("flora-{:06}.bin", self.tag.load(Ordering::Relaxed)));
                    // Only the used part of the sorted region.
                    let words: Vec<u32> = data[..ARGS_BYTES as usize].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
                    let total: u64 = (0..BUCKETS).map(|b| words[ARGS_WORD + b * 5 + 1] as u64).sum();
                    // File: arguments, header plants, used sorted plants.
                    let header_end = (ARGS_BYTES + HEADER as u64 * 64) as usize;
                    let sorted = (ARGS_BYTES + (SORTED_START - FAR_CAPACITY) as u64 * 64) as usize;
                    let end = (sorted + total as usize * 64).min(data.len());
                    let mut bytes = data[..header_end].to_vec();
                    bytes.extend_from_slice(&data[sorted..end]);
                    let written = self.written.fetch_add(1, Ordering::Relaxed);
                    if written >= self.max {
                        tracing::warn!("flora trace: {} dumps written, stopping", self.max);
                    } else if let Err(e) = std::fs::write(&path, &bytes) {
                        tracing::warn!("flora trace: {e}");
                    }
                }
                self.buffer.unmap();
                self.state.store(READBACK_IDLE, Ordering::Release);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_fits_the_plant_buffer_and_args() {
        // Sorted region keeps the same size as staging (cross-fades copy a
        // plant at most twice, but only inside bands).
        const { assert!(SORTED_START + STAGE <= 262_144, "plants buffer holds 262144") };
        const { assert!(BUCKETS as u32 <= HEADER * 4) };
        const { assert!(ARGS_WORD + BUCKETS * 5 <= CURSOR_WORD) };
        const { assert!(((CURSOR_WORD + BUCKETS) * 4) as u64 <= ARGS_BYTES) };
        // WGSL mirror constants.
        let cull = include_str!("shaders/scatter_cull.wgsl");
        for (name, value) in [
            ("FLORA_FAR_CAPACITY", FAR_CAPACITY),
            ("FLORA_HEADER", HEADER),
            ("FLORA_STAGE", STAGE),
            ("FLORA_STAGE_START", STAGE_START),
            ("FLORA_SORTED_START", SORTED_START),
            ("FLORA_ENTRIES", ENTRIES as u32),
            ("FLORA_BUCKETS", BUCKETS as u32),
            ("FLORA_CURSOR_WORD", CURSOR_WORD as u32),
            ("FLORA_ARGS_WORD", ARGS_WORD as u32),
            ("FLORA_STAGE_WORD", STAGE_WORD as u32),
        ] {
            assert!(cull.contains(&format!("const {name}: u32 = {value}u;")), "{name} = {value}");
        }
        let draw = include_str!("shaders/scatter_flora.wgsl");
        assert!(draw.contains(&format!("const FLORA_HEADER_START: u32 = {FAR_CAPACITY}u;")));
    }
}
