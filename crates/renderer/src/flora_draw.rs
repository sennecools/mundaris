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

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use astrum_flora::hash::{derive, name_key};
use astrum_flora::palette::{Palette, PlanetRock, from_lch, load_planet};
use astrum_flora::rock::{RockFile, Weathering, grow_rock};
use astrum_flora::scatter::{MAX_ROCKS, MAX_SPECIES, species_wgsl, splice_species};
use astrum_flora::{Kit, LOD_COUNT, Mesh, SpeciesFile, grow_meshes, load_species_for_body, variant_seed};
use wgpu::util::DeviceExt;

/// Grown variants per entry (`FLORA_VARIANTS` in scatter_cull.wgsl). Rocks:
/// variant 0 dry, variant 1 wet (weathered, mossy).
pub const VARIANTS: usize = 2;
/// Bucket entries: up to MAX_SPECIES plants, then MAX_ROCKS rock archetypes.
pub const ENTRIES: usize = MAX_SPECIES + MAX_ROCKS;
pub const BUCKETS: usize = ENTRIES * VARIANTS * LOD_COUNT;
/// First word of the flora draw arguments (`FLORA_ARGS_WORD`).
pub const ARGS_WORD: usize = 16;
/// Word holding the entry mask (`FLORA_MASK_WORD`).
pub const MASK_WORD: usize = 5;
/// Size of the shared plant argument buffer once flora is included.
pub const ARGS_BYTES: u64 = 2048;
const FAR_CAPACITY: u32 = 65_536;
const CAPACITY: [u32; LOD_COUNT] = [512, 2048, 4096];
/// GPU vertex: the 36-byte flora vertex plus the bucket's first instance slot.
const VERTEX_BYTES: u64 = 40;

fn bucket_base(bucket: usize) -> u32 {
    let lod = bucket % LOD_COUNT;
    let before: u32 = CAPACITY[..lod].iter().sum();
    FAR_CAPACITY + (bucket / LOD_COUNT) as u32 * CAPACITY.iter().sum::<u32>() + before
}

/// Species, rocks and CPU meshes of every bucket, grown once per process.
struct Grown {
    species: Vec<SpeciesFile>,
    rocks: Vec<PlanetRock>,
    mask: u32,
    vertices: Vec<u8>,
    indices: Vec<u32>,
    /// Per bucket: (index count, first index, base vertex).
    ranges: Vec<(u32, u32, i32)>,
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
    let started = std::time::Instant::now();
    // Grow every (entry, variant) on its own thread.
    let jobs: Vec<(usize, usize)> = (0..species.len())
        .chain((0..rocks.len()).map(|r| MAX_SPECIES + r))
        .flat_map(|e| (0..VARIANTS).map(move |v| (e, v)))
        .collect();
    let meshes: Vec<[Mesh; LOD_COUNT]> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|&(e, v)| {
                let kit = &kit;
                let species = &species;
                let rocks = &rocks;
                let w = weathering(v == 1);
                scope.spawn(move || {
                    if e < MAX_SPECIES {
                        let sp = &species[e];
                        grow_meshes(sp, kit, variant_seed(sp, v as u32)).1
                    } else {
                        let rock = &rocks[e - MAX_SPECIES].1;
                        grow_rock(rock, derive(name_key(&rock.name), v as u64), &w)
                    }
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().expect("flora growth thread")).collect()
    });
    let mut g = Grown {
        species,
        rocks: rocks.iter().map(|r| r.0.clone()).collect(),
        mask: 0,
        vertices: Vec::new(),
        indices: Vec::new(),
        ranges: vec![(0, 0, 0); BUCKETS],
    };
    let mut tris = 0usize;
    for (&(e, v), lods) in jobs.iter().zip(&meshes) {
        g.mask |= 1 << e;
        for (lod, mesh) in lods.iter().enumerate() {
            let bucket = (e * VARIANTS + v) * LOD_COUNT + lod;
            let base_vertex = (g.vertices.len() as u64 / VERTEX_BYTES) as i32;
            let first_index = g.indices.len() as u32;
            let slot = bucket_base(bucket);
            for vtx in &mesh.vertices {
                vtx.to_bytes(&mut g.vertices);
                g.vertices.extend_from_slice(&slot.to_le_bytes());
            }
            g.indices.extend_from_slice(&mesh.indices);
            g.ranges[bucket] = (mesh.indices.len() as u32, first_index, base_vertex);
            tris += mesh.triangles();
        }
    }
    tracing::info!(
        "flora: grew {:?} + rocks {:?} ({} meshes, {} triangles, {} KiB) in {:.0} ms",
        g.species.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        g.rocks.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        meshes.len() * LOD_COUNT,
        tris,
        (g.vertices.len() + g.indices.len() * 4) / 1024,
        started.elapsed().as_secs_f64() * 1e3
    );
    g
}

/// `shader` with the niche/rock table generated from the loaded content
/// spliced over the built-in one (pure string transform; unchanged when no
/// species loaded).
pub(crate) fn species_shader(shader: &str) -> String {
    let g = grown();
    if g.species.is_empty() {
        return shader.to_string();
    }
    splice_species(shader, &species_wgsl(&g.species, &g.rocks))
}

fn grown() -> Arc<Grown> {
    static GROWN: OnceLock<Arc<Grown>> = OnceLock::new();
    GROWN.get_or_init(|| Arc::new(grow_all())).clone()
}

fn flora_enabled() -> bool {
    std::env::var_os("ASTRUM_NO_FLORA").is_none_or(|v| v == "0")
}

/// Last read-back overflow counts: (flora buckets, procedural plants)
/// rejected because their region was full, packed as hi/lo u32.
static OVERFLOW: AtomicU64 = AtomicU64::new(0);

/// Plants dropped last read-back frame because a bucket (first) or the
/// procedural far region (second) was full. Zero means nothing was capped.
pub fn flora_overflow() -> (u32, u32) {
    let v = OVERFLOW.load(Ordering::Relaxed);
    ((v >> 32) as u32, v as u32)
}

const READBACK_IDLE: u8 = 0;
const READBACK_COPIED: u8 = 1;
const READBACK_MAPPING: u8 = 2;
const READBACK_READY: u8 = 3;

pub(crate) struct FloraDraw {
    /// Overflow counter read-back (async map, never stalls the frame).
    readback: wgpu::Buffer,
    readback_state: Arc<AtomicU8>,
    pipeline: crate::aa::MsaaPipeline,
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
        let mask = g.mask;
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
        // One variant per MSAA sample count (crate::aa).
        let pipeline = {
            let draw_shader = draw_shader.clone();
            let draw_layout = draw_layout.clone();
            let color_targets = color_targets.to_vec();
            crate::aa::MsaaPipeline::new(device, move |device, samples| {
                let layout = Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_BYTES,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &ATTRIBUTES,
                });
                let (draw_shader, draw_layout, color_targets) =
                    (&draw_shader, &draw_layout, &color_targets[..]);
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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
                    multisample: crate::aa::multisample(samples),
                    multiview_mask: None,
                    cache: None,
                })
            })
        };
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
            vertices,
            indices,
            args,
            mask,
        }
    }

    /// Fill the flora words of this frame's plant arguments (instance counts
    /// zeroed). `words` covers the whole argument buffer.
    pub(crate) fn write_args(&self, words: &mut [u32]) {
        self.poll_overflow();
        words[MASK_WORD] = if flora_enabled() { self.mask } else { 0 };
        words[ARGS_WORD..ARGS_WORD + self.args.len()].copy_from_slice(&self.args);
    }

    /// Copy this frame's overflow words (6, 7) for read-back; call after the
    /// cull pass, before the encoder is submitted.
    pub(crate) fn copy_counters(&self, encoder: &mut wgpu::CommandEncoder, plant_args: &wgpu::Buffer) {
        if self.readback_state.load(Ordering::Acquire) == READBACK_IDLE {
            encoder.copy_buffer_to_buffer(plant_args, 24, &self.readback, 0, 8);
            self.readback_state.store(READBACK_COPIED, Ordering::Release);
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
                        Ok(d) => (u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])),
                        Err(_) => (0, 0),
                    }
                };
                self.readback.unmap();
                let packed = ((flora as u64) << 32) | far as u64;
                if OVERFLOW.swap(packed, Ordering::Relaxed) != packed && packed != 0 {
                    tracing::warn!("flora: plants dropped by full buckets: {flora} grown, {far} procedural");
                }
                self.readback_state.store(READBACK_IDLE, Ordering::Release);
            }
            _ => {}
        }
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

    /// Selects the MSAA sample count of the main-pass pipeline (crate::aa).
    pub(crate) fn set_samples(&mut self, device: &wgpu::Device, samples: u32) {
        self.pipeline.set_samples(device, samples);
    }

    /// Main pass; groups 0–3 are the scatter draw's (plants at group 3).
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, plant_args: &wgpu::Buffer) {
        pass.set_pipeline(self.pipeline.get());
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
        // WGSL mirror: flora_base(b) = FAR + (b / 3) * 6656 + {0, 512, 2560}.
        assert_eq!(bucket_base(4), FAR_CAPACITY + 6656 + 512);
    }
}
