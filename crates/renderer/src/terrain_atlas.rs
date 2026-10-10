//! GPU height/normal atlas for planetary terrain (ADR 0016).
//!
//! A compute producer writes band-limited heights and per-pixel normals for
//! requested quadtree nodes straight into texture-array layers; there is no CPU
//! payload or upload queue. Selected nodes are then drawn with one instanced
//! draw over a shared grid, using CDLOD morphing towards the coarser grid and a
//! short arrival fade from ancestor data. Atlas contents are disposable caches:
//! the app owns layer assignment, identity and residency. Under ADR 0023 the
//! producer evaluates the authoritative surface function; the world crate's CPU
//! evaluation is its test oracle.
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

/// Draw shader: shared scene lighting followed by the atlas draw stages.
const DRAW_SHADER: &str = concat!(
    include_str!("shaders/lighting.wgsl"),
    include_str!("shaders/terrain_atlas.wgsl")
);
/// Producer shader: shared split-lattice noise and cube-map sampling followed by
/// the atlas producer.
const PRODUCE_SHADER: &str = concat!(
    include_str!("shaders/terrain_noise.wgsl"),
    include_str!("shaders/cube_map.wgsl"),
    include_str!("shaders/terrain_atlas_produce.wgsl"),
    include_str!("shaders/landform_eval.wgsl")
);

/// Largest number of producer jobs accepted in one frame.
pub const MAX_ATLAS_JOBS_PER_FRAME: usize = 256;
const TILE_BYTES: u64 = 592;
const INSTANCE_BYTES: u64 = 224;
const DISPATCH_STRIDE: u64 = 256;
/// Largest number of collision pages produced in one frame.
pub const MAX_COLLISION_JOBS_PER_FRAME: usize = 32;
/// Largest collision page edge in cells; pages hold `(cells + 1)^2` samples.
pub const MAX_COLLISION_CELLS: u32 = 64;
const COLLISION_SAMPLE_BYTES: u64 = 16;
const COLLISION_BYTES: u64 = COLLISION_SAMPLE_BYTES
    * MAX_COLLISION_JOBS_PER_FRAME as u64
    * ((MAX_COLLISION_CELLS + 1) * (MAX_COLLISION_CELLS + 1)) as u64;
/// Producer job slots: atlas jobs first, collision jobs after them.
const MAX_TILE_SLOTS: usize = MAX_ATLAS_JOBS_PER_FRAME + MAX_COLLISION_JOBS_PER_FRAME;
const MAX_DISPATCHES: u64 = (2 * MAX_ATLAS_JOBS_PER_FRAME + MAX_COLLISION_JOBS_PER_FRAME) as u64;
const READBACK_SLOTS: usize = 4;
/// Tier A bake passes encoded per frame for a world source nobody waits on.
const TIER_A_PASSES_PER_FRAME: usize = 24;
/// Dyadic ladders of the producer's noise anchors (M2 design §3).
pub const ATLAS_LADDERS: usize = 4;
/// Ladder levels the producer's `ladder_split` supports.
pub const ATLAS_LADDER_LEVELS: std::ops::RangeInclusive<i32> = -11..=19;
/// Height bounds are reported per cell of a 4x4 grid over each tile.
pub const ATLAS_BOUNDS_GRID: usize = 4;
const BOUNDS_WORDS_PER_JOB: usize = 2 * ATLAS_BOUNDS_GRID * ATLAS_BOUNDS_GRID;
const BOUNDS_BYTES: u64 = (4 * BOUNDS_WORDS_PER_JOB * MAX_ATLAS_JOBS_PER_FRAME) as u64;

/// Terrain visualization drawn by the atlas renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TerrainViewMode {
    #[default]
    Lit,
    Height,
    Normals,
    Grid,
    Level,
    MorphFade,
    /// Albedo only, no light.
    Unlit,
    /// Ambient occlusion buffer.
    AoOnly,
    /// Cascade tint x sun visibility.
    Shadows,
    /// False colour in stops relative to the current exposure.
    Luminance,
    /// Hypsometric ramp over the terrain height (sea level 0).
    Elevation,
    /// Water where the ground is below sea level, land grey elsewhere.
    OceanMask,
    /// Page temperature, -40..+40 °C.
    Temperature,
    /// Page moisture, 0..1.
    Moisture,
    /// Page wind: hue from direction, brightness from speed.
    Wind,
    /// Page albedo (biome/snow/water colour) without light.
    Biome,
}

impl TerrainViewMode {
    pub const ALL: [Self; 16] = [
        Self::Lit,
        Self::Unlit,
        Self::Height,
        Self::Normals,
        Self::Grid,
        Self::Level,
        Self::MorphFade,
        Self::AoOnly,
        Self::Shadows,
        Self::Luminance,
        Self::Elevation,
        Self::OceanMask,
        Self::Temperature,
        Self::Moisture,
        Self::Wind,
        Self::Biome,
    ];

    /// Debug views that bypass exposure, metering and tonemapping.
    pub fn passthrough(self) -> bool {
        !matches!(self, Self::Lit | Self::AoOnly | Self::Luminance)
    }

    /// Selector consumed by `terrain_atlas.wgsl`.
    pub fn shader_mode(self) -> u32 {
        match self {
            Self::Lit => 0,
            Self::Height => 1,
            Self::Normals => 2,
            Self::Grid => 5,
            Self::Level => 6,
            Self::MorphFade => 8,
            Self::Unlit => 9,
            Self::AoOnly => 10,
            Self::Shadows => 11,
            Self::Luminance => 12,
            Self::Elevation => 13,
            Self::OceanMask => 14,
            Self::Temperature => 15,
            Self::Moisture => 16,
            Self::Wind => 17,
            Self::Biome => 18,
        }
    }

    /// Stable name used by snapshots, commands and the environment.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lit => "lit",
            Self::Height => "height",
            Self::Normals => "normals",
            Self::Grid => "grid",
            Self::Level => "level",
            Self::MorphFade => "morph_fade",
            Self::Unlit => "unlit",
            Self::AoOnly => "ao",
            Self::Shadows => "shadows",
            Self::Luminance => "luminance",
            Self::Elevation => "elevation",
            Self::OceanMask => "ocean",
            Self::Temperature => "temperature",
            Self::Moisture => "moisture",
            Self::Wind => "wind",
            Self::Biome => "biome",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

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
    /// Tier A world map baked on the GPU when the source is first bound
    /// (`crate::tier_a`); tiles become available once it reports ready.
    World(Box<AtlasWorldSource>),
}

/// World-map source: Tier A bake inputs and the surface colour constants.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasWorldSource {
    pub bake: crate::tier_a::TierABakeInputs,
    pub surface: AtlasWorldSurface,
}

/// Colour of a world-map body (pipeline §10, M1): biome LUT (temperature ×
/// moisture, sRGB texels, texel centres spanning the axes), snow rule and
/// flat water. Colours are linear unless named sRGB.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AtlasWorldSurface {
    pub lut_size: u32,
    /// Row-major, y = moisture.
    pub lut_srgb: Vec<[u8; 3]>,
    pub temperature_c: [f32; 2],
    pub moisture: [f32; 2],
    pub snow_temperature_c: f32,
    pub snow_blend_c: f32,
    pub snow_slope_rad: [f32; 2],
    pub snow_albedo: [f32; 3],
    pub water_shallow: [f32; 3],
    pub water_deep: [f32; 3],
    pub water_depth_scale_m: f32,
    /// Largest Tier B climate-detail offsets (unit-amplitude octaves in the
    /// job scale by these): temperature in °C, moisture.
    pub climate_temperature_c: f32,
    pub climate_moisture: f32,
    /// Packed landform set (`astrum_world::terrain::landform::gpu`), appended
    /// after the LUT; empty for macro-only bodies.
    pub landforms: Vec<u32>,
}

/// Words before the LUT texels in the packed surface buffer.
const WORLD_SURFACE_HEADER_WORDS: usize = 24;

impl AtlasWorldSurface {
    /// Packed storage words (`world_surface` in the producer shader).
    fn packed(&self) -> Result<Vec<u8>, String> {
        let n = self.lut_size;
        if !(2..=512).contains(&n) || self.lut_srgb.len() != (n * n) as usize {
            return Err("world surface LUT must be square, 2..=512 texels".into());
        }
        let f = |v: f32| v.to_bits();
        let mut words: Vec<u32> = vec![
            n,
            f(self.climate_temperature_c),
            f(self.climate_moisture),
            0,
            f(self.temperature_c[0]),
            f(self.temperature_c[1]),
            f(self.moisture[0]),
            f(self.moisture[1]),
            f(self.snow_temperature_c),
            f(self.snow_blend_c),
            f(self.snow_slope_rad[0]),
            f(self.snow_slope_rad[1]),
            f(self.snow_albedo[0]),
            f(self.snow_albedo[1]),
            f(self.snow_albedo[2]),
            0,
            f(self.water_shallow[0]),
            f(self.water_shallow[1]),
            f(self.water_shallow[2]),
            f(self.water_depth_scale_m),
            f(self.water_deep[0]),
            f(self.water_deep[1]),
            f(self.water_deep[2]),
            0,
        ];
        debug_assert_eq!(words.len(), WORLD_SURFACE_HEADER_WORDS);
        words.extend(
            self.lut_srgb
                .iter()
                .map(|c| u32::from(c[0]) | u32::from(c[1]) << 8 | u32::from(c[2]) << 16),
        );
        // Landform block (landform_eval.wgsl); one zero word means none.
        if self.landforms.is_empty() {
            words.push(0);
        } else {
            words.extend_from_slice(&self.landforms);
        }
        Ok(words.into_iter().flat_map(u32::to_le_bytes).collect())
    }
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
    /// Tier A macro elevation from one mip level (bicubic) of the source's
    /// world fields: offset in f32 values and face cells
    /// (`tier_a::field_mip_layout`; climate mips follow at +6·cells² and
    /// +12·cells²).
    World {
        mip_offset: u32,
        mip_cells: u32,
        /// Tier A face cells: level 0 of the wind runs (page climate).
        base_cells: u32,
        /// The tile's elevation lookup, split on the CPU in f64 (pipeline
        /// §4.4) so f32 never holds an absolute texel coordinate: texel
        /// (x, y) on cube face `face` (`CubeFace::ALL` order) of the mip,
        /// `t = (u + 1)/2 · n − 0.5` as in `world_map::CubeMap`, is
        /// `texel_origin + texel_fraction + texel_jacobian · st`. Gnomonic
        /// tile charts make this affine map exact on the tile's own face.
        face: u32,
        /// Integer texel of st = (0, 0).
        texel_origin: [i32; 2],
        /// Fraction in [0, 1) of st = (0, 0).
        texel_fraction: [f32; 2],
        /// Columns d texel / d s and d texel / d t.
        texel_jacobian: [[f32; 2]; 2],
    },
}

/// One band-limited fBm layer on a dyadic ladder (`noise::DetailNoise` in
/// the world crate): `octaves` octaves of wavelength `s_ladder · 2^level`
/// from `first_level` down by one level each, seeds
/// `octave_seed(salt, (ladder, level))` and amplitude `amplitude_m · gain^k`.
/// The producer computes each octave's band-limit weight. `octaves = 0`
/// disables the layer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasLadderLayer {
    pub salt: u32,
    pub ladder: u32,
    pub first_level: i32,
    pub octaves: u32,
    pub amplitude_m: f32,
    pub gain: f32,
}

impl AtlasLadderLayer {
    fn validate(&self) -> Result<(), String> {
        if self.octaves == 0 {
            return Ok(());
        }
        let finest = self.first_level - (self.octaves.min(255) as i32 - 1);
        if self.ladder as usize >= ATLAS_LADDERS
            || self.octaves > 255
            || !ATLAS_LADDER_LEVELS.contains(&self.first_level)
            || !ATLAS_LADDER_LEVELS.contains(&finest)
        {
            return Err(format!("atlas ladder layer out of range: {self:?}"));
        }
        Ok(())
    }

    /// (salt, ladder | octaves << 8 | (first_level + 128) << 16, amplitude
    /// bits, gain bits): `ladder_layer` in the producer shader.
    fn packed(&self) -> [u32; 4] {
        [
            self.salt,
            self.ladder | self.octaves << 8 | ((self.first_level + 128) as u32) << 16,
            self.amplitude_m.to_bits(),
            self.gain.to_bits(),
        ]
    }
}

/// Noise octaves of one job (M2 design §3): the fixed-point anchors of the
/// chart centre per ladder, `A_m = round(c / s_m · 2^11)` and the residual
/// `r_m = c / s_m − A_m · 2^-11` (computed on the CPU in f64; see
/// `ladder::tile_anchors`), the band-limit texel, and the height-detail and
/// climate-detail layers. The producer derives every octave's lattice cell
/// and fraction from the anchors exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasOctaves {
    pub anchor_cells: [[i32; 3]; ATLAS_LADDERS],
    pub anchor_residuals: [[f32; 3]; ATLAS_LADDERS],
    /// Texel footprint (m) the octaves' band-limit weights are computed for.
    pub texel_m: f32,
    pub detail: AtlasLadderLayer,
    /// World maps: temperature from the octave seeds, moisture from the
    /// seeds xor the moisture salt.
    pub climate: AtlasLadderLayer,
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
    /// Ladder anchors and noise layers.
    pub octaves: AtlasOctaves,
}

/// Atlas layer plus the sub-rectangle `[origin, origin + scale]` of its chart
/// that covers the drawn node.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasSampleSource {
    pub layer: u32,
    pub origin: [f32; 2],
    pub scale: f32,
}

/// Flat-water look of a world-map body (linear colours; M1, until M3).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AtlasWater {
    pub shallow: [f32; 3],
    pub deep: [f32; 3],
    /// Depth at which the colour is `1 - 1/e` of the way to `deep`.
    pub depth_scale_m: f32,
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
    pub material: crate::SurfaceMaterial,
    /// Flat water at the reference radius over ground below it (world maps
    /// with oceans, M1): drawn at height 0 with the sphere normal and a
    /// depth-tinted colour, through the coast contour in the normal page.
    pub water: Option<AtlasWater>,
    pub mode: u32,
    /// Edges (`PatchEdge::bit`: s=0, s=1, t=0, t=1) bordering a coarser node;
    /// their vertices are drawn fully morphed (pipeline §9.8).
    pub coarser_edges: u8,
    /// Edges bordering a finer node; drawn unmorphed so both sides meet on
    /// this node's own level. The coarser rule wins at a shared corner.
    pub finer_edges: u8,
}

/// Per-frame atlas work and draw list staged by the app.
#[derive(Debug, Clone, Default)]
pub struct TerrainAtlasFrame {
    pub config: Option<TerrainAtlasConfig>,
    pub sources: Vec<(u64, Arc<AtlasSource>)>,
    pub jobs: Vec<AtlasProduceJob>,
    pub instances: Vec<AtlasInstance>,
    /// Sun shadow cascades of the shadowed body and their casters.
    pub shadow: Option<AtlasShadowFrame>,
    /// Collision pages to evaluate and read back (pipeline §15.1); `layer` is
    /// unused. At most `MAX_COLLISION_JOBS_PER_FRAME`.
    pub collision_jobs: Vec<AtlasProduceJob>,
    /// Cells per collision page edge, `1..=MAX_COLLISION_CELLS`.
    pub collision_cells: u32,
    /// Sources the app no longer uses (a rebind or release): freed now,
    /// including any unfinished Tier A bake.
    pub retired_sources: Vec<u64>,
}

/// One read-back collision page: `(cells + 1)^2` samples on the job's chart at
/// `st = (i, j) / cells`, row-major; heights are radial offsets in metres and
/// normals unit vectors in body axes. Delivered a few frames after the job.
#[derive(Debug, Clone, PartialEq)]
pub struct AtlasCollisionPage {
    pub token: u64,
    pub cells: u32,
    pub heights: Vec<f32>,
    pub normals: Vec<[f32; 3]>,
}

/// Cascades fitted by the app and, per cascade, casters selected at a detail
/// matched to its texel size inside its light-space box, from resident data
/// only (no producer requests).
#[derive(Debug, Clone, Default)]
pub struct AtlasShadowFrame {
    pub cascades: crate::Cascades,
    pub casters: [Vec<AtlasInstance>; 4],
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
    pub collision_jobs: u32,
    /// Collision batches not read back because every staging buffer was busy.
    pub dropped_collision_readbacks: u32,
}

struct SourceGpu {
    _textures: [wgpu::Texture; 2],
    _constants: [wgpu::Buffer; 3],
    group: wgpu::BindGroup,
    last_used: u64,
    /// World sources: the Tier A bake and its result buffer (bound in `group`).
    bake: Option<crate::tier_a::TierABake>,
    _world: wgpu::Buffer,
}

struct Readback {
    buffer: wgpu::Buffer,
    tokens: Vec<u64>,
    /// Collision page edge cells of the recorded batch (collision ring only).
    cells: u32,
    // 0 idle, 1 copy recorded, 2 mapping, 3 mapped, 4 failed
    state: Arc<AtomicU8>,
}

pub(crate) struct TerrainAtlasRenderer {
    config: TerrainAtlasConfig,
    tier_a: crate::tier_a::TierAPipelines,
    /// World sources whose Tier A bake finished in a recorded frame.
    /// Finished world bakes `(key, None)` and sources that failed to upload
    /// `(key, Some(error))`, reported once.
    ready_sources: Vec<(u64, Option<String>)>,
    /// Sources that failed to upload; not retried until retired.
    failed_sources: std::collections::HashSet<u64>,
    _height: wgpu::Texture,
    _normal: wgpu::Texture,
    _albedo: wgpu::Texture,
    _climate: wgpu::Texture,
    produce_heights: wgpu::ComputePipeline,
    produce_normals: wgpu::ComputePipeline,
    produce_collision: wgpu::ComputePipeline,
    source_layout: wgpu::BindGroupLayout,
    produce_group: wgpu::BindGroup,
    tiles: wgpu::Buffer,
    dispatch: wgpu::Buffer,
    bounds: wgpu::Buffer,
    readbacks: Vec<Readback>,
    collision_out: wgpu::Buffer,
    collision_readbacks: Vec<Readback>,
    collision_results: Vec<AtlasCollisionPage>,
    sources: std::collections::HashMap<u64, SourceGpu>,
    pipeline: wgpu::RenderPipeline,
    draw_group: wgpu::BindGroup,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_group: wgpu::BindGroup,
    shadow_instances: wgpu::Buffer,
    shadow_capacity: u64,
    /// Instance range of each cascade inside `shadow_instances`.
    shadow_ranges: [(u32, u32); 4],
    draw_layout: wgpu::BindGroupLayout,
    height_view: wgpu::TextureView,
    normal_view: wgpu::TextureView,
    albedo_view: wgpu::TextureView,
    climate_view: wgpu::TextureView,
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
        targets: &[wgpu::TextureFormat],
        projection_layout: &wgpu::BindGroupLayout,
        lighting_layout: &wgpu::BindGroupLayout,
        light_layout: &wgpu::BindGroupLayout,
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
        // sRGB-encoded linear albedo, alpha 1 where the page owns its colour
        // (world maps), 0 where the instance material applies.
        let albedo = array(
            config.normal_side(),
            wgpu::TextureFormat::Rgba8Unorm,
            "Terrain atlas albedo",
        );
        // Temperature °C, moisture, wind east, wind north (zero off world maps).
        let climate = array(
            config.normal_side(),
            wgpu::TextureFormat::Rgba16Float,
            "Terrain atlas climate",
        );
        let array_view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let height_view = array_view(&height);
        let normal_view = array_view(&normal);
        let albedo_view = array_view(&albedo);
        let climate_view = array_view(&climate);

        let produce_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Terrain atlas producer"),
            source: wgpu::ShaderSource::Wgsl(PRODUCE_SHADER.into()),
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
                // Binding 5 (the per-job octave origins) retired with the
                // ladder anchors in `Tile` (M2).
                storage(6, false),
                storage_texture(7, wgpu::TextureFormat::Rgba8Unorm),
                storage_texture(8, wgpu::TextureFormat::Rgba16Float),
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
            entries: &[
                image(0),
                image(1),
                uniform(2),
                uniform(3),
                storage(4, true),
                storage(5, true),
            ],
        });
        let produce_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Terrain atlas producer layout"),
                bind_group_layouts: &[Some(&produce_layout), Some(&source_layout)],
                immediate_size: 0,
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
        let produce_collision = compute("produce_collision");
        let tiles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain atlas producer jobs"),
            size: TILE_BYTES * MAX_TILE_SLOTS as u64,
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
        let collision_out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Terrain collision pages"),
            size: COLLISION_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let collision_readbacks = (0..READBACK_SLOTS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Terrain collision readback"),
                    size: COLLISION_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                tokens: Vec::new(),
                cells: 0,
                state: Arc::new(AtomicU8::new(0)),
            })
            .collect();
        let readbacks = (0..READBACK_SLOTS)
            .map(|_| Readback {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Terrain atlas bounds readback"),
                    size: BOUNDS_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                tokens: Vec::new(),
                cells: 0,
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
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: collision_out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&storage_view(&albedo)),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&storage_view(&climate)),
                },
            ],
        });

        // Draw resources.
        let draw_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Terrain atlas draw"),
            source: wgpu::ShaderSource::Wgsl(DRAW_SHADER.into()),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
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
            [&height_view, &normal_view, &albedo_view, &climate_view],
            &sampler,
            &instances,
            &grid_uniform,
        );
        let draw_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Terrain atlas draw layout"),
            bind_group_layouts: &[
                Some(projection_layout),
                Some(&draw_layout),
                Some(lighting_layout),
            ],
            immediate_size: 0,
        });
        let color_targets: Vec<_> = targets
            .iter()
            .map(|&format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Terrain atlas reverse-Z instanced draw"),
            layout: Some(&draw_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &draw_shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &color_targets,
            }),
            primitive: wgpu::PrimitiveState {
                front_face: wgpu::FrontFace::Ccw,
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
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Terrain atlas shadow caster layout"),
                bind_group_layouts: &[Some(light_layout), Some(&draw_layout)],
                immediate_size: 0,
            });
        // Depth-only casters share the draw vertex stage; the projection is
        // the cascade's view -> light-clip transform. Orthographic depth with
        // slope-scaled bias; receivers add normal offset.
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Terrain atlas sun shadow casters"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                })],
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
        let shadow_capacity = 1024;
        let shadow_instances = instance_buffer(device, shadow_capacity);
        let shadow_group = self::draw_group(
            device,
            &draw_layout,
            [&height_view, &normal_view, &albedo_view, &climate_view],
            &sampler,
            &shadow_instances,
            &grid_uniform,
        );
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
            tier_a: crate::tier_a::TierAPipelines::new(device),
            ready_sources: Vec::new(),
            failed_sources: Default::default(),
            _height: height,
            _normal: normal,
            _albedo: albedo,
            _climate: climate,
            produce_heights,
            produce_normals,
            produce_collision,
            source_layout,
            produce_group,
            tiles,
            dispatch,
            bounds,
            readbacks,
            collision_out,
            collision_readbacks,
            collision_results: Vec::new(),
            sources: Default::default(),
            pipeline,
            draw_group,
            instances,
            instance_capacity,
            shadow_pipeline,
            shadow_group,
            shadow_instances,
            shadow_capacity,
            shadow_ranges: [(0, 0); 4],
            draw_layout,
            height_view,
            normal_view,
            albedo_view,
            climate_view,
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

    pub(crate) fn take_collision_pages(&mut self) -> Vec<AtlasCollisionPage> {
        std::mem::take(&mut self.collision_results)
    }

    /// World sources whose Tier A fields are complete as of the last recorded
    /// frame (tiles for them may be requested from the next frame on), and
    /// sources that failed to upload, with the error.
    pub(crate) fn take_ready_sources(&mut self) -> Vec<(u64, Option<String>)> {
        std::mem::take(&mut self.ready_sources)
    }

    fn collect_collision_readbacks(&mut self) {
        for readback in &mut self.collision_readbacks {
            match readback.state.load(Ordering::Acquire) {
                3 => {
                    // A failed mapping drops the batch; its pages are requested again.
                    if let Ok(view) = readback.buffer.slice(..).get_mapped_range() {
                        let side = (readback.cells + 1) as usize;
                        let samples = side * side;
                        for (page, token) in readback.tokens.iter().enumerate() {
                            let mut heights = Vec::with_capacity(samples);
                            let mut normals = Vec::with_capacity(samples);
                            for sample in 0..samples {
                                let start =
                                    (page * samples + sample) * COLLISION_SAMPLE_BYTES as usize;
                                let word = |i: usize| {
                                    f32::from_le_bytes(
                                        view[start + 4 * i..start + 4 * i + 4].try_into().unwrap(),
                                    )
                                };
                                heights.push(word(0));
                                normals.push([word(1), word(2), word(3)]);
                            }
                            self.collision_results.push(AtlasCollisionPage {
                                token: *token,
                                cells: readback.cells,
                                heights,
                                normals,
                            });
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

    fn collect_readbacks(&mut self) {
        self.collect_collision_readbacks();
        for readback in &mut self.readbacks {
            match readback.state.load(Ordering::Acquire) {
                3 => {
                    {
                        // A failed mapping drops this batch; its nodes keep their
                        // conservative bounds until produced again.
                        let Ok(view) = readback.buffer.slice(..).get_mapped_range() else {
                            readback.buffer.unmap();
                            readback.tokens.clear();
                            readback.state.store(0, Ordering::Release);
                            continue;
                        };
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
        for key in &frame.retired_sources {
            self.sources.remove(key);
            self.failed_sources.remove(key);
        }
        for (key, source) in &frame.sources {
            if let Some(existing) = self.sources.get_mut(key) {
                existing.last_used = self.frame;
            } else if !self.failed_sources.contains(key) {
                // A source the device cannot hold (for example a Tier A bake
                // beyond the storage limits) is skipped and reported, so other
                // bodies keep drawing.
                match upload_source(device, queue, &self.source_layout, &self.tier_a, source) {
                    Ok(gpu) => {
                        self.sources.insert(
                            *key,
                            SourceGpu {
                                last_used: self.frame,
                                ..gpu
                            },
                        );
                    }
                    Err(error) => {
                        self.failed_sources.insert(*key);
                        self.ready_sources.push((*key, Some(error)));
                    }
                }
            }
        }
        let frame_number = self.frame;
        self.sources
            .retain(|_, source| frame_number - source.last_used < 600);
        // Advance Tier A bakes. A source with jobs this frame is finished now so
        // its jobs read complete fields; the app normally waits for readiness.
        let needed: std::collections::HashSet<u64> = frame
            .jobs
            .iter()
            .chain(&frame.collision_jobs)
            .map(|job| job.source)
            .collect();
        for (key, source) in self.sources.iter_mut() {
            if let Some(bake) = source.bake.as_mut().filter(|bake| !bake.done()) {
                let budget = if needed.contains(key) {
                    usize::MAX
                } else {
                    TIER_A_PASSES_PER_FRAME
                };
                bake.encode(encoder, &self.tier_a, budget);
                if bake.done() {
                    self.ready_sources.push((*key, None));
                }
            }
        }

        self.report = TerrainAtlasReport {
            jobs: frame.jobs.len() as u32,
            instances: frame.instances.len() as u32,
            sources: self.sources.len() as u32,
            dropped_readbacks: self.report.dropped_readbacks,
            collision_jobs: frame.collision_jobs.len() as u32,
            dropped_collision_readbacks: self.report.dropped_collision_readbacks,
        };

        if frame.jobs.is_empty() && frame.collision_jobs.is_empty() {
            self.stage_instances(device, queue, frame);
            return Ok(());
        }
        let validate = |job: &AtlasProduceJob| -> Result<(), String> {
            if !self.sources.contains_key(&job.source) {
                return Err(format!(
                    "atlas job references unbound source {}",
                    job.source
                ));
            }
            job.octaves.detail.validate()?;
            job.octaves.climate.validate()
        };
        let mut ordered: Vec<&AtlasProduceJob> = frame.jobs.iter().collect();
        ordered.sort_by_key(|job| job.source);
        let mut collision: Vec<&AtlasProduceJob> = frame.collision_jobs.iter().collect();
        collision.sort_by_key(|job| job.source);
        for job in &ordered {
            if job.layer >= self.config.layers {
                return Err(format!("atlas job layer {} out of range", job.layer));
            }
            validate(job)?;
        }
        for job in &collision {
            validate(job)?;
        }
        // Tile slots: atlas jobs from 0, collision jobs from MAX_ATLAS_JOBS_PER_FRAME.
        for (first, jobs) in [(0, &ordered), (MAX_ATLAS_JOBS_PER_FRAME, &collision)] {
            if jobs.is_empty() {
                continue;
            }
            let mut tile_bytes = Vec::with_capacity(jobs.len() * TILE_BYTES as usize);
            for job in jobs.iter() {
                pack_tile(&mut tile_bytes, job);
            }
            queue.write_buffer(&self.tiles, first as u64 * TILE_BYTES, &tile_bytes);
        }
        if !ordered.is_empty() {
            let mut initial = Vec::with_capacity(ordered.len() * BOUNDS_WORDS_PER_JOB * 4);
            for _ in 0..ordered.len() * BOUNDS_WORDS_PER_JOB / 2 {
                initial.extend_from_slice(&u32::MAX.to_le_bytes());
                initial.extend_from_slice(&0u32.to_le_bytes());
            }
            queue.write_buffer(&self.bounds, 0, &initial);
        }

        // Consecutive jobs per source share one dispatch per pass kind:
        // `(source, first slot, count)`.
        let group = |jobs: &[&AtlasProduceJob], first: usize| {
            let mut groups: Vec<(u64, u32, u32)> = Vec::new();
            for (index, job) in jobs.iter().enumerate() {
                match groups.last_mut() {
                    Some((source, _, count)) if *source == job.source => *count += 1,
                    _ => groups.push((job.source, (first + index) as u32, 1)),
                }
            }
            groups
        };
        #[derive(Clone, Copy)]
        enum Pass {
            Heights,
            Normals,
            Collision,
        }
        let mut dispatch_bytes = vec![0u8; (DISPATCH_STRIDE * MAX_DISPATCHES) as usize];
        let mut dispatches = Vec::new();
        let mut push = |source: u64, words: [u32; 4], count: u32, pass: Pass| {
            let slot = dispatches.len();
            let offset = slot * DISPATCH_STRIDE as usize;
            for (i, value) in words.into_iter().enumerate() {
                dispatch_bytes[offset + i * 4..offset + i * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
            dispatches.push((source, slot as u32, words[1], count, pass));
        };
        for (source, base, count) in group(&ordered, 0) {
            push(
                source,
                [base, self.config.height_side(), self.config.cells, base],
                count,
                Pass::Heights,
            );
            if self.config.normal_scale > 1 {
                let cells = self.config.cells * self.config.normal_scale;
                push(
                    source,
                    [base, self.config.normal_side(), cells, base],
                    count,
                    Pass::Normals,
                );
            }
        }
        let collision_cells = frame.collision_cells;
        if !collision.is_empty() {
            if collision.len() > MAX_COLLISION_JOBS_PER_FRAME
                || !(1..=MAX_COLLISION_CELLS).contains(&collision_cells)
            {
                return Err(format!(
                    "{} collision jobs of {collision_cells} cells exceed the per-frame caps",
                    collision.len()
                ));
            }
            for (source, base, count) in group(&collision, MAX_ATLAS_JOBS_PER_FRAME) {
                // The fourth word is the first output page of the group.
                let page = base - MAX_ATLAS_JOBS_PER_FRAME as u32;
                push(
                    source,
                    [base, collision_cells + 1, collision_cells, page],
                    count,
                    Pass::Collision,
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
            for &(source, slot, side, count, kind) in &dispatches {
                pass.set_pipeline(match kind {
                    Pass::Heights => &self.produce_heights,
                    Pass::Normals => &self.produce_normals,
                    Pass::Collision => &self.produce_collision,
                });
                pass.set_bind_group(0, &self.produce_group, &[slot * DISPATCH_STRIDE as u32]);
                pass.set_bind_group(1, &self.sources[&source].group, &[]);
                let groups_xy = side.div_ceil(8);
                pass.dispatch_workgroups(groups_xy, groups_xy, count);
            }
        }
        if !ordered.is_empty() {
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
        if !collision.is_empty() {
            let side = u64::from(collision_cells + 1);
            if let Some(readback) = self
                .collision_readbacks
                .iter_mut()
                .find(|r| r.state.load(Ordering::Acquire) == 0)
            {
                encoder.copy_buffer_to_buffer(
                    &self.collision_out,
                    0,
                    &readback.buffer,
                    0,
                    collision.len() as u64 * side * side * COLLISION_SAMPLE_BYTES,
                );
                readback.tokens = collision.iter().map(|job| job.token).collect();
                readback.cells = collision_cells;
                readback.state.store(1, Ordering::Release);
            } else {
                self.report.dropped_collision_readbacks += 1;
            }
        }
        self.stage_instances(device, queue, frame);
        Ok(())
    }

    fn stage_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &TerrainAtlasFrame,
    ) {
        let count = frame.instances.len() as u64;
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instances = instance_buffer(device, self.instance_capacity);
            self.draw_group = draw_group(
                device,
                &self.draw_layout,
                [
                    &self.height_view,
                    &self.normal_view,
                    &self.albedo_view,
                    &self.climate_view,
                ],
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
    }

    /// Start mapping bounds and collision pages copied in the just-submitted
    /// command buffer.
    pub(crate) fn on_submitted(&mut self) {
        for source in self.sources.values_mut() {
            if let Some(bake) = source.bake.as_mut() {
                bake.release_scratch();
            }
        }
        for readback in self
            .readbacks
            .iter_mut()
            .chain(&mut self.collision_readbacks)
        {
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

    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection_group: &wgpu::BindGroup,
        lighting_group: &wgpu::BindGroup,
    ) {
        if self.staged_instances == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, projection_group, &[]);
        pass.set_bind_group(1, &self.draw_group, &[]);
        pass.set_bind_group(2, lighting_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..self.staged_instances);
    }

    /// Uploads per-cascade caster lists back to back.
    pub(crate) fn prepare_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        cascades: &[Vec<AtlasInstance>],
    ) {
        let total: usize = cascades.iter().map(Vec::len).sum();
        if total as u64 > self.shadow_capacity {
            self.shadow_capacity = (total as u64).next_power_of_two();
            self.shadow_instances = instance_buffer(device, self.shadow_capacity);
            self.shadow_group = draw_group(
                device,
                &self.draw_layout,
                [
                    &self.height_view,
                    &self.normal_view,
                    &self.albedo_view,
                    &self.climate_view,
                ],
                &self.sampler,
                &self.shadow_instances,
                &self.grid_uniform,
            );
        }
        let mut bytes = Vec::with_capacity(total * INSTANCE_BYTES as usize);
        self.shadow_ranges = [(0, 0); 4];
        let mut start = 0u32;
        for (c, list) in cascades.iter().enumerate().take(4) {
            for instance in list {
                pack_instance(&mut bytes, instance);
            }
            self.shadow_ranges[c] = (start, start + list.len() as u32);
            start += list.len() as u32;
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.shadow_instances, 0, &bytes);
        }
    }

    /// Draws one cascade's casters; the light group selects the cascade by offset.
    pub(crate) fn draw_shadow(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        light_group: &wgpu::BindGroup,
        cascade: usize,
    ) {
        let (start, end) = self.shadow_ranges[cascade.min(3)];
        if end <= start || self.frame == 0 {
            return;
        }
        pass.set_pipeline(&self.shadow_pipeline);
        pass.set_bind_group(
            0,
            light_group,
            &[crate::shadows::ShadowMaps::offset(cascade)],
        );
        pass.set_bind_group(1, &self.shadow_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, start..end);
    }
}

/// Heights and normals of one produced layer, read back for validation.
#[derive(Debug, Clone)]
pub struct ProducedTileReadback {
    pub heights: Vec<f32>,
    pub normals: Vec<[f32; 3]>,
    /// Flat-water mask stored in the normal page's w (1 where a world map's
    /// ground is below sea level).
    pub water: Vec<f32>,
    /// Page albedo per normal texel: linear rgb (decoded from sRGB) and the
    /// ownership alpha.
    pub albedo: Vec<[f32; 4]>,
    /// Page climate per normal texel: temperature °C, moisture, wind east,
    /// wind north (decoded from f16; zero off world maps).
    pub climate: Vec<[f32; 4]>,
    pub bounds: Option<(f32, f32)>,
}

/// IEEE binary16 to f32 (validation readback of the climate page).
fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let fraction = f32::from(bits & 0x3ff);
    match exponent {
        0 => sign * fraction * 2f32.powi(-24),
        31 if fraction == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f32.powi(exponent - 15),
    }
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
        &crate::post::SCENE_TARGETS,
        &projection_layout,
        &crate::celestial::lighting_layout(device),
        &crate::shadows::ShadowMaps::light_layout(device),
        config,
    )?;
    let frame = TerrainAtlasFrame {
        config: Some(config),
        sources: sources.to_vec(),
        jobs: jobs.to_vec(),
        ..Default::default()
    };
    let mut encoder = device.create_command_encoder(&Default::default());
    atlas.prepare(device, queue, &mut encoder, &frame)?;
    let read_layer = |encoder: &mut wgpu::CommandEncoder,
                      texture: &wgpu::Texture,
                      side: u32,
                      layer: u32,
                      texel_bytes: u32|
     -> (wgpu::Buffer, u32) {
        let row = (side * texel_bytes).div_ceil(256) * 256;
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
            4,
        );
        let normals = read_layer(
            &mut encoder,
            &atlas._normal,
            config.normal_side(),
            job.layer,
            4,
        );
        let albedo = read_layer(
            &mut encoder,
            &atlas._albedo,
            config.normal_side(),
            job.layer,
            4,
        );
        let climate = read_layer(
            &mut encoder,
            &atlas._climate,
            config.normal_side(),
            job.layer,
            8,
        );
        buffers.push((heights, normals, albedo, climate));
    }
    queue.submit([encoder.finish()]);
    atlas.on_submitted();
    let mut output = Vec::new();
    for (
        (heights, height_row),
        (normals, normal_row),
        (albedo, albedo_row),
        (climate, climate_row),
    ) in &buffers
    {
        for buffer in [heights, normals, albedo, climate] {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        let side = config.height_side() as usize;
        let view = heights
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut height_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *height_row as usize + x * 4;
                height_values.push(f32::from_le_bytes(view[at..at + 4].try_into().unwrap()));
            }
        }
        drop(view);
        let side = config.normal_side() as usize;
        let view = normals
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut normal_values = Vec::with_capacity(side * side);
        let mut water_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *normal_row as usize + x * 4;
                let snorm = |b: u8| (f32::from(b as i8) / 127.0).max(-1.0);
                normal_values.push([snorm(view[at]), snorm(view[at + 1]), snorm(view[at + 2])]);
                water_values.push(snorm(view[at + 3]));
            }
        }
        drop(view);
        let view = albedo
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut albedo_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *albedo_row as usize + x * 4;
                let linear = |b: u8| {
                    let c = f32::from(b) / 255.0;
                    if c <= 0.04045 {
                        c / 12.92
                    } else {
                        ((c + 0.055) / 1.055).powf(2.4)
                    }
                };
                albedo_values.push([
                    linear(view[at]),
                    linear(view[at + 1]),
                    linear(view[at + 2]),
                    f32::from(view[at + 3]) / 255.0,
                ]);
            }
        }
        drop(view);
        let view = climate
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        let mut climate_values = Vec::with_capacity(side * side);
        for y in 0..side {
            for x in 0..side {
                let at = y * *climate_row as usize + x * 8;
                let half = |k: usize| {
                    f16_to_f32(u16::from_le_bytes([view[at + 2 * k], view[at + 2 * k + 1]]))
                };
                climate_values.push([half(0), half(1), half(2), half(3)]);
            }
        }
        drop(view);
        output.push(ProducedTileReadback {
            heights: height_values,
            normals: normal_values,
            water: water_values,
            albedo: albedo_values,
            climate: climate_values,
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

/// Produce collision pages through the runtime path (producer dispatch, staging
/// ring, asynchronous map) on a caller-owned device. Each "frame" submits and
/// waits for its work, so the result also reports how many frames of pipeline
/// latency passed before the pages arrived. Validation only.
pub fn collision_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    config: TerrainAtlasConfig,
    sources: &[(u64, Arc<AtlasSource>)],
    jobs: &[AtlasProduceJob],
    cells: u32,
) -> Result<(Vec<AtlasCollisionPage>, u32), String> {
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
        &crate::post::SCENE_TARGETS,
        &projection_layout,
        &crate::celestial::lighting_layout(device),
        &crate::shadows::ShadowMaps::light_layout(device),
        config,
    )?;
    let mut frame = TerrainAtlasFrame {
        config: Some(config),
        sources: sources.to_vec(),
        collision_jobs: jobs.to_vec(),
        collision_cells: cells,
        ..Default::default()
    };
    let mut pages = Vec::new();
    for submissions in 1..=240u32 {
        let mut encoder = device.create_command_encoder(&Default::default());
        atlas.prepare(device, queue, &mut encoder, &frame)?;
        frame.collision_jobs.clear();
        queue.submit([encoder.finish()]);
        atlas.on_submitted();
        // Each "frame" waits for its own GPU work, so the count is frames of
        // pipeline latency (readback ring), not wall-clock time that other
        // GPU users on the adapter would stretch.
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        pages.extend(atlas.take_collision_pages());
        if pages.len() >= jobs.len() {
            return Ok((pages, submissions));
        }
    }
    Err(format!(
        "{} of {} collision pages arrived",
        pages.len(),
        jobs.len()
    ))
}

/// Evaluate the producer's integer lattice hash (`lattice_bits` in
/// `terrain_noise.wgsl`) for `(cell, seed)` inputs on a caller-owned device.
/// Validation-only path: blocking, allocates per call.
pub fn lattice_hash_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    inputs: &[([i32; 3], u32)],
) -> Result<Vec<[u32; 3]>, String> {
    const ENTRY: &str = "
@group(0) @binding(0) var<storage, read> hash_inputs: array<vec4<i32>>;
@group(0) @binding(1) var<storage, read_write> hash_outputs: array<vec4<u32>>;
@compute @workgroup_size(64)
fn hash_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&hash_inputs) {
        return;
    }
    let c = hash_inputs[id.x];
    hash_outputs[id.x] = vec4<u32>(lattice_bits(c.xyz, bitcast<u32>(c.w)), 0u);
}
";
    if inputs.is_empty() || inputs.len() > 64 * 65_535 {
        return Err("hash validation needs 1..=4194240 inputs".into());
    }
    let packed: Vec<u8> = inputs
        .iter()
        .flat_map(|(cell, seed)| {
            [cell[0], cell[1], cell[2], *seed as i32]
                .into_iter()
                .flat_map(i32::to_le_bytes)
        })
        .collect();
    let words = run_noise_kernel(device, queue, ENTRY, "hash_main", &packed, inputs.len(), 4)?;
    Ok(words
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| [chunk[0], chunk[1], chunk[2]])
        .collect())
}

/// One probe of the producer's ladder noise (`terrain_noise.wgsl`): octave
/// `(ladder, level)` of a layer with seed salt `salt`, split from a tile's
/// anchor on that ladder at `local` metres from the tile centre, with the 4th
/// lattice coordinate `w_cell + w_frac`. `cell4` is hashed with `salt` as the
/// seed (`lattice_bits4`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LadderNoiseProbe {
    pub anchor_cell: [i32; 3],
    pub anchor_residual: [f32; 3],
    pub ladder: u32,
    pub level: i32,
    pub salt: u32,
    pub local: [f32; 3],
    pub w_cell: i32,
    pub w_frac: f32,
    pub cell4: [i32; 4],
}

/// GPU results of one [`LadderNoiseProbe`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LadderNoiseResult {
    /// `ladder_octave_seed(salt, ladder, level)`.
    pub seed: u32,
    /// `ladder_octave_offset(seed)`.
    pub offset: [f32; 3],
    /// `lattice_bits4(cell4, salt)`.
    pub bits4: [u32; 4],
    /// `ladder_split`: integer cell and f32 fraction (without the offset).
    pub cell: [i32; 3],
    pub fraction: [f32; 3],
    /// `ladder_noise3`: value and d value / d local.
    pub noise3: [f32; 4],
    /// `ladder_noise4`: value, d value / d local, d value / d w.
    pub noise4: [f32; 5],
}

/// Evaluate the producer's ladder seeds, offsets, 4D hash, split and 3D/4D
/// octave noise for `probes` on a caller-owned device. Validation-only path:
/// blocking, allocates per call.
pub fn ladder_noise_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    probes: &[LadderNoiseProbe],
) -> Result<Vec<LadderNoiseResult>, String> {
    const ENTRY: &str = "
struct Probe {
    a: vec4<u32>,
    b: vec4<u32>,
    c: vec4<u32>,
    d: vec4<u32>,
    e: vec4<u32>,
}
struct ProbeResult {
    a: vec4<u32>,
    b: vec4<u32>,
    c: vec4<u32>,
    d: vec4<u32>,
    e: vec4<u32>,
    f: vec4<u32>,
}
@group(0) @binding(0) var<storage, read> probe_inputs: array<Probe>;
@group(0) @binding(1) var<storage, read_write> probe_outputs: array<ProbeResult>;
@compute @workgroup_size(64)
fn probe_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&probe_inputs) {
        return;
    }
    let p = probe_inputs[id.x];
    let anchor = bitcast<vec3<i32>>(p.a.xyz);
    let ladder = p.a.w;
    let residual = bitcast<vec3<f32>>(p.b.xyz);
    let level = bitcast<i32>(p.b.w);
    let local = bitcast<vec3<f32>>(p.c.xyz);
    let salt = p.c.w;
    let seed = ladder_octave_seed(salt, ladder, level);
    let split = ladder_split(anchor, residual, ladder, level, local);
    let n3 = ladder_noise3(anchor, residual, ladder, level, seed, local);
    let n4 = ladder_noise4(anchor, residual, ladder, level, seed, local, bitcast<i32>(p.d.x),
        bitcast<f32>(p.d.y));
    var o: ProbeResult;
    o.a = vec4<u32>(seed, bitcast<vec3<u32>>(ladder_octave_offset(seed)));
    o.b = lattice_bits4(bitcast<vec4<i32>>(p.e), salt);
    o.c = vec4<u32>(bitcast<vec3<u32>>(split.cell), 0u);
    o.d = vec4<u32>(bitcast<vec3<u32>>(split.q), bitcast<u32>(n3.x));
    o.e = vec4<u32>(bitcast<vec3<u32>>(n3.yzw), bitcast<u32>(n4.value));
    o.f = vec4<u32>(bitcast<vec3<u32>>(n4.gradient), bitcast<u32>(n4.dw));
    probe_outputs[id.x] = o;
}
";
    if probes.is_empty() || probes.len() > 64 * 65_535 {
        return Err("ladder validation needs 1..=4194240 probes".into());
    }
    let f = f32::to_bits;
    let packed: Vec<u8> = probes
        .iter()
        .flat_map(|p| {
            let c = p.anchor_cell.map(|v| v as u32);
            let r = p.anchor_residual.map(f);
            let l = p.local.map(f);
            let e = p.cell4.map(|v| v as u32);
            [
                c[0],
                c[1],
                c[2],
                p.ladder,
                r[0],
                r[1],
                r[2],
                p.level as u32,
                l[0],
                l[1],
                l[2],
                p.salt,
                p.w_cell as u32,
                f(p.w_frac),
                0,
                0,
                e[0],
                e[1],
                e[2],
                e[3],
            ]
            .into_iter()
            .flat_map(u32::to_le_bytes)
        })
        .collect();
    let words = run_noise_kernel(
        device,
        queue,
        ENTRY,
        "probe_main",
        &packed,
        probes.len(),
        24,
    )?;
    let float = f32::from_bits;
    Ok(words
        .as_chunks::<24>()
        .0
        .iter()
        .map(|w| LadderNoiseResult {
            seed: w[0],
            offset: [float(w[1]), float(w[2]), float(w[3])],
            bits4: [w[4], w[5], w[6], w[7]],
            cell: [w[8] as i32, w[9] as i32, w[10] as i32],
            fraction: [float(w[12]), float(w[13]), float(w[14])],
            noise3: [float(w[15]), float(w[16]), float(w[17]), float(w[18])],
            noise4: [
                float(w[19]),
                float(w[20]),
                float(w[21]),
                float(w[22]),
                float(w[23]),
            ],
        })
        .collect())
}

/// Run a validation kernel `entry` (appended to `terrain_noise.wgsl`) over
/// `count` records: binding 0 holds `inputs`, binding 1 receives
/// `output_words` u32 per record, which are read back.
fn run_noise_kernel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    entry: &str,
    entry_point: &str,
    inputs: &[u8],
    count: usize,
    output_words: usize,
) -> Result<Vec<u32>, String> {
    let source = format!("{}{entry}", include_str!("shaders/terrain_noise.wgsl"));
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Noise validation"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Noise validation"),
        layout: None,
        module: &module,
        entry_point: Some(entry_point),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Noise validation inputs"),
        size: inputs.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&input, 0, inputs);
    let bytes = (count * output_words * 4) as u64;
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Noise validation outputs"),
        size: bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Noise validation readback"),
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Noise validation"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: input.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: output.as_entire_binding(),
            },
        ],
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((count as u32).div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, bytes);
    queue.submit([encoder.finish()]);
    readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| error.to_string())?;
    let view = readback
        .slice(..)
        .get_mapped_range()
        .map_err(|error| error.to_string())?;
    Ok(view
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect())
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
    // Height, normal, albedo and climate atlas views.
    [height, normal, albedo, climate]: [&wgpu::TextureView; 4],
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
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(albedo),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(climate),
            },
        ],
    })
}

fn upload_source(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    tier_a: &crate::tier_a::TierAPipelines,
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
        AtlasSource::World(_) => (
            placeholder()?,
            placeholder()?,
            f32_bytes(&[0.0; 4]),
            vec![0u8; FIELDS_CONSTANT_BYTES],
        ),
    };
    // World sources bake their Tier A fields over the next frames; other
    // sources bind a small unused buffer.
    let bake = match source {
        AtlasSource::World(world) => Some(crate::tier_a::TierABake::new(
            device,
            queue,
            tier_a,
            &world.bake,
        )?),
        _ => None,
    };
    let world = bake.as_ref().map_or_else(
        || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Terrain atlas unused world fields"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        },
        |bake| bake.result().clone(),
    );
    let surface_bytes = match source {
        AtlasSource::World(world) => world.surface.packed()?,
        _ => vec![0u8; 16],
    };
    let surface = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terrain atlas world surface"),
        size: surface_bytes.len() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&surface, 0, &surface_bytes);
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
            wgpu::BindGroupEntry {
                binding: 4,
                resource: world.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: surface.as_entire_binding(),
            },
        ],
    });
    Ok(SourceGpu {
        _textures: [macro_texture, detail_texture],
        _constants: [profile_buffer, fields_buffer, surface],
        group,
        last_used: 0,
        bake,
        _world: world,
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
        AtlasTileKind::World { .. } => (2, 0, 0),
    };
    for value in [job.layer, kind, macro_count, detail_count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend(f32_bytes(&[job.radius_m, job.octaves.texel_m, 0.0, 0.0]));
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
        AtlasTileKind::World {
            face,
            texel_origin,
            texel_fraction,
            texel_jacobian,
            ..
        } => {
            // The Fields-only band slots carry the split texel lookup.
            cells[0] = [texel_origin[0], texel_origin[1], *face as i32, 0];
            fractions[0] = [texel_fraction[0], texel_fraction[1], 0.0, 0.0];
            fractions[1] = [
                texel_jacobian[0][0],
                texel_jacobian[0][1],
                texel_jacobian[1][0],
                texel_jacobian[1][1],
            ];
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
    let (mip_offset, mip_cells, base_cells) = match job.kind {
        AtlasTileKind::World {
            mip_offset,
            mip_cells,
            base_cells,
            ..
        } => (mip_offset, mip_cells, base_cells),
        _ => (0, 0, 0),
    };
    for value in [0, base_cells, mip_offset, mip_cells] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    // Ladder anchors: 4 ladders × xyz packed into three vec4s each.
    let octaves = &job.octaves;
    for value in octaves.anchor_cells.as_flattened() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend(f32_bytes(octaves.anchor_residuals.as_flattened()));
    for value in octaves
        .detail
        .packed()
        .into_iter()
        .chain(octaves.climate.packed())
    {
        out.extend_from_slice(&value.to_le_bytes());
    }
    debug_assert_eq!((out.len() - start) as u64, TILE_BYTES);
}

fn pack_instance(out: &mut Vec<u8>, instance: &AtlasInstance) {
    let water = instance.water.unwrap_or_default();
    let c = instance.chart;
    let b = instance.body_to_view;
    let rows: [[f32; 4]; 14] = [
        [
            instance.anchor_view_m[0],
            instance.anchor_view_m[1],
            instance.anchor_view_m[2],
            instance.radius_m,
        ],
        [b[0][0], b[0][1], b[0][2], instance.mode as f32],
        [b[1][0], b[1][1], b[1][2], f32::from(instance.coarser_edges)],
        [b[2][0], b[2][1], b[2][2], f32::from(instance.finer_edges)],
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
            instance.material.albedo[0],
            instance.material.albedo[1],
            instance.material.albedo[2],
            instance.material.brdf.shader_index(),
        ],
        [
            if instance.water.is_some() { 1.0 } else { 0.0 },
            water.depth_scale_m,
            0.0,
            0.0,
        ],
        [water.shallow[0], water.shallow[1], water.shallow[2], 0.0],
        [water.deep[0], water.deep[1], water.deep[2], 0.0],
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
        for (name, source) in [("produce", PRODUCE_SHADER), ("draw", DRAW_SHADER)] {
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
    fn f16_decoding_matches_known_values() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x7bff), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert!(f16_to_f32(0x7e00).is_nan());
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
                octaves: AtlasOctaves::default(),
            },
        );
        assert_eq!(bytes.len() as u64, TILE_BYTES);
        let mut octaves = AtlasOctaves {
            texel_m: 0.75,
            detail: AtlasLadderLayer {
                salt: 0xdead_beef,
                ladder: 2,
                first_level: -3,
                octaves: 9,
                amplitude_m: 24.0,
                gain: 0.5,
            },
            climate: AtlasLadderLayer {
                salt: 7,
                ladder: 1,
                first_level: 14,
                octaves: 9,
                amplitude_m: 0.2,
                gain: 0.6,
            },
            ..Default::default()
        };
        octaves.anchor_cells[1][2] = -5;
        octaves.anchor_residuals[3][0] = 0.25;
        let mut bytes = Vec::new();
        pack_tile(
            &mut bytes,
            &AtlasProduceJob {
                source: 0,
                layer: 0,
                token: 0,
                chart: AtlasChart::default(),
                radius_m: 1.0,
                kind: AtlasTileKind::World {
                    mip_offset: 0,
                    mip_cells: 4,
                    base_cells: 4,
                    face: 5,
                    texel_origin: [-3, 7],
                    texel_fraction: [0.25, 0.5],
                    texel_jacobian: [[2.0, 0.0], [0.0, 2.0]],
                },
                octaves,
            },
        );
        assert_eq!(bytes.len() as u64, TILE_BYTES);
        // WGSL Tile: band_cell[0] at byte 240 (= (-3, 7, face 5)), band_frac[0]
        // at 336 and the Jacobian in band_frac[1] at 352.
        let word = |offset: usize| <[u8; 4]>::try_from(&bytes[offset..offset + 4]).unwrap();
        assert_eq!(i32::from_le_bytes(word(240)), -3);
        assert_eq!(i32::from_le_bytes(word(244)), 7);
        assert_eq!(i32::from_le_bytes(word(248)), 5);
        assert_eq!(f32::from_le_bytes(word(340)), 0.5);
        assert_eq!(f32::from_le_bytes(word(352)), 2.0);
        // scale.y = band-limit texel; anchor_cell from 464 (ladder 1 z is
        // element 5), anchor_residual from 512 (ladder 3 x is element 9),
        // detail at 560 and climate at 576 (`ladder_layer`).
        assert_eq!(f32::from_le_bytes(word(68)), 0.75);
        assert_eq!(i32::from_le_bytes(word(464 + 5 * 4)), -5);
        assert_eq!(f32::from_le_bytes(word(512 + 9 * 4)), 0.25);
        assert_eq!(u32::from_le_bytes(word(560)), 0xdead_beef);
        let packed = u32::from_le_bytes(word(564));
        assert_eq!(packed & 3, 2);
        assert_eq!((packed >> 8) & 255, 9);
        assert_eq!(((packed >> 16) & 255) as i32 - 128, -3);
        assert_eq!(f32::from_le_bytes(word(568)), 24.0);
        assert_eq!(f32::from_le_bytes(word(572)), 0.5);
        assert_eq!(u32::from_le_bytes(word(576)), 7);
        assert_eq!(
            ((u32::from_le_bytes(word(580)) >> 16) & 255) as i32 - 128,
            14
        );
        assert!(octaves.detail.validate().is_ok());
        for bad in [
            AtlasLadderLayer {
                ladder: 4,
                ..octaves.detail
            },
            AtlasLadderLayer {
                first_level: 20,
                ..octaves.detail
            },
            AtlasLadderLayer {
                first_level: -4,
                octaves: 9,
                ..octaves.detail
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
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
