//! PROTOTYPE (flora lane): grown species meshes for the M5 scatter.
//!
//! At start-up the species in `content/flora/species/*.ron` (file order, at
//! most `astrum_flora::scatter::MAX_SPECIES`) are grown on the CPU
//! (`astrum_flora`, two variants each, three LODs) and uploaded into one
//! vertex and one index buffer. Their climate niches are compiled into the
//! draw and cull shaders ([`species_shader`] splices the generated table over
//! `scatter_species.wgsl`), so placement (`scatter_niche.wgsl`) picks the
//! species per site. The cull pass (`scatter_cull.wgsl`) routes plants of a
//! grown species into per-(species, variant, LOD) buckets; each bucket is
//! one indexed indirect draw whose arguments live in the shared plant
//! argument buffer from word [`ARGS_WORD`]. Boulders and far plants keep the
//! procedural shapes (tinted with their species' canopy colour).
//!
//! `ASTRUM_NO_FLORA=1` keeps every plant procedural (A/B captures).
//!
//! Prototype debt: no hot reload, no wind animation, fixed LOD distances and
//! bucket capacities in WGSL, growth blocks start-up (cached per process).

use std::sync::{Arc, OnceLock};

use astrum_flora::scatter::{MAX_SPECIES, species_wgsl, splice_species};
use astrum_flora::{Kit, LOD_COUNT, Mesh, SpeciesFile, grow_meshes, load_species_dir, variant_seed};
use wgpu::util::DeviceExt;

/// Grown variants per species (`FLORA_VARIANTS` in scatter_cull.wgsl).
pub const VARIANTS: usize = 2;
pub const BUCKETS: usize = MAX_SPECIES * VARIANTS * LOD_COUNT;
/// First word of the flora draw arguments (`FLORA_ARGS_WORD`).
pub const ARGS_WORD: usize = 16;
/// Word holding the species mask (`FLORA_MASK_WORD`).
pub const MASK_WORD: usize = 5;
/// Size of the shared plant argument buffer once flora is included.
pub const ARGS_BYTES: u64 = 1024;
const FAR_CAPACITY: u32 = 98_304;
const CAPACITY: [u32; LOD_COUNT] = [1024, 4096, 8192];
/// GPU vertex: the 36-byte flora vertex plus the bucket's first instance slot.
const VERTEX_BYTES: u64 = 40;

fn bucket_base(bucket: usize) -> u32 {
    let lod = bucket % LOD_COUNT;
    let before: u32 = CAPACITY[..lod].iter().sum();
    FAR_CAPACITY + (bucket / LOD_COUNT) as u32 * CAPACITY.iter().sum::<u32>() + before
}

/// Species and CPU meshes of every bucket, grown once per process.
struct Grown {
    species: Vec<SpeciesFile>,
    vertices: Vec<u8>,
    indices: Vec<u32>,
    /// Per bucket: (index count, first index, base vertex).
    ranges: Vec<(u32, u32, i32)>,
}

fn grow_all() -> Grown {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/species");
    let mut species: Vec<SpeciesFile> = match load_species_dir(&dir) {
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
    let kit = Kit::builtin();
    let started = std::time::Instant::now();
    // Grow every (species, variant) on its own thread; results in bucket order.
    let plants: Vec<[Mesh; LOD_COUNT]> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..species.len() * VARIANTS)
            .map(|sv| {
                let sp = &species[sv / VARIANTS];
                let kit = &kit;
                scope.spawn(move || grow_meshes(sp, kit, variant_seed(sp, (sv % VARIANTS) as u32)).1)
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("flora growth thread")).collect()
    });
    let mut g = Grown { species, vertices: Vec::new(), indices: Vec::new(), ranges: Vec::with_capacity(BUCKETS) };
    let mut tris = 0usize;
    for (sv, plant) in plants.iter().enumerate() {
        for (lod, mesh) in plant.iter().enumerate() {
            let bucket = sv * LOD_COUNT + lod;
            let base_vertex = (g.vertices.len() as u64 / VERTEX_BYTES) as i32;
            let first_index = g.indices.len() as u32;
            let slot = bucket_base(bucket);
            for v in &mesh.vertices {
                v.to_bytes(&mut g.vertices);
                g.vertices.extend_from_slice(&slot.to_le_bytes());
            }
            g.indices.extend_from_slice(&mesh.indices);
            g.ranges.push((mesh.indices.len() as u32, first_index, base_vertex));
            tris += mesh.triangles();
        }
    }
    g.ranges.resize(BUCKETS, (0, 0, 0));
    tracing::info!(
        "flora: grew {:?} ({} meshes, {} triangles, {} KiB) in {:.0} ms",
        g.species.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        plants.len() * LOD_COUNT,
        tris,
        (g.vertices.len() + g.indices.len() * 4) / 1024,
        started.elapsed().as_secs_f64() * 1e3
    );
    g
}

/// `shader` with the niche table generated from the loaded species spliced
/// over the built-in one (pure string transform; unchanged when no species
/// loaded).
pub(crate) fn species_shader(shader: &str) -> String {
    let g = grown();
    if g.species.is_empty() {
        return shader.to_string();
    }
    splice_species(shader, &species_wgsl(&g.species))
}

fn grown() -> Arc<Grown> {
    static GROWN: OnceLock<Arc<Grown>> = OnceLock::new();
    GROWN.get_or_init(|| Arc::new(grow_all())).clone()
}

fn flora_enabled() -> bool {
    std::env::var_os("ASTRUM_NO_FLORA").is_none_or(|v| v == "0")
}

pub(crate) struct FloraDraw {
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    /// Words written to the plant argument buffer each frame from [`ARGS_WORD`].
    args: Vec<u32>,
    mask: u32,
}

const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Snorm8x4,
    2 => Unorm8x4,
    3 => Float32x3,
    4 => Uint8x4,
    5 => Uint32,
];

impl FloraDraw {
    /// `draw_shader` must contain `vs_flora`/`fs_flora` (scatter_flora.wgsl);
    /// layouts and targets are the scatter pipelines' own.
    pub(crate) fn new(
        device: &wgpu::Device,
        draw_shader: &wgpu::ShaderModule,
        draw_layout: &wgpu::PipelineLayout,
        shadow_layout: &wgpu::PipelineLayout,
        color_targets: &[Option<wgpu::ColorTargetState>],
    ) -> Self {
        let g = grown();
        let mask = (1u32 << g.species.len()) - 1;
        let mut args = vec![0u32; BUCKETS * 5];
        for (b, &(count, first, base)) in g.ranges.iter().enumerate() {
            args[b * 5] = count;
            args[b * 5 + 2] = first;
            args[b * 5 + 3] = base as u32;
        }
        // Keep zero-sized buffers valid.
        let vertex_bytes = if g.vertices.is_empty() {
            vec![0u8; VERTEX_BYTES as usize]
        } else {
            g.vertices.clone()
        };
        let index_words = if g.indices.is_empty() {
            vec![0u32; 3]
        } else {
            g.indices.clone()
        };
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flora vertices"),
            contents: &vertex_bytes,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_bytes: Vec<u8> = index_words.iter().flat_map(|i| i.to_le_bytes()).collect();
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Flora indices"),
            contents: &index_bytes,
            usage: wgpu::BufferUsages::INDEX,
        });
        let layout = Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_BYTES,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Flora grown plants (prototype)"),
            layout: Some(draw_layout),
            vertex: wgpu::VertexState {
                module: draw_shader,
                entry_point: Some("vs_flora"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&layout),
            },
            fragment: Some(wgpu::FragmentState {
                module: draw_shader,
                entry_point: Some("fs_flora"),
                compilation_options: Default::default(),
                targets: color_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Flora grown plants sun shadow casters (prototype)"),
            layout: Some(shadow_layout),
            vertex: wgpu::VertexState {
                module: draw_shader,
                entry_point: Some("vs_flora"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&layout),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::post::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 1.5,
                    clamp: 0.0,
                },
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            shadow_pipeline,
            vertices,
            indices,
            args,
            mask,
        }
    }

    /// Fill the flora words of this frame's plant arguments (instance counts
    /// zeroed). `words` covers the whole argument buffer.
    pub(crate) fn write_args(&self, words: &mut [u32]) {
        words[MASK_WORD] = if flora_enabled() { self.mask } else { 0 };
        words[ARGS_WORD..ARGS_WORD + self.args.len()].copy_from_slice(&self.args);
    }

    fn draws(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        if self.mask == 0 || !flora_enabled() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        for b in 0..BUCKETS {
            if self.args[b * 5] == 0 {
                continue;
            }
            pass.draw_indexed_indirect(plant_args, ((ARGS_WORD + b * 5) * 4) as u64);
        }
    }

    /// Main pass; groups 0–3 are the scatter draw's (plants at group 3).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        pass.set_pipeline(&self.pipeline);
        self.draws(pass, plant_args);
    }

    /// Sun shadow pass; groups as for the scatter shadow draw.
    pub(crate) fn draw_shadow(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        pass.set_pipeline(&self.shadow_pipeline);
        self.draws(pass, plant_args);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_fit_the_plant_buffer_and_args() {
        let last = bucket_base(BUCKETS - 1) + CAPACITY[LOD_COUNT - 1];
        assert!(last as u64 <= 262_144, "plants buffer holds 262144");
        assert!(((ARGS_WORD + BUCKETS * 5) * 4) as u64 <= ARGS_BYTES);
        // WGSL mirror: flora_base(b) = FAR + (b / 3) * 13312 + {0, 1024, 5120}.
        assert_eq!(bucket_base(4), FAR_CAPACITY + 13_312 + 1024);
    }
}
