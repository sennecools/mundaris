//! Tier A world-map bake on the GPU (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6).
//!
//! Under ADR 0023 this bake is the authority for a world-map body's macro
//! fields; `astrum_world::terrain::tier_a` is its CPU test oracle. A bake runs
//! a fixed schedule of compute passes that can be spread over frames: M1
//! (continents, sea level, ocean blur, climate, moisture) and M2 (Shape):
//! tectonics, relief smoothing and wind deflection for the rain shadow, the
//! macro-erosion cascade, a second sea-level histogram with the re-zero and
//! shelf, and field mips. The schedule is budgeted by work (texel passes), so
//! the many small passes of coarse erosion levels cost little.
//!
//! The result is one storage buffer of `u32` words: nine runs of `6·n²`
//! values (elevation m, temperature °C, moisture 0..1, wind east, wind north,
//! `boundary_coord` i32 in 1/16 m, `aux0` = `pack4x8unorm(uplift, hardness,
//! sediment, flow)`, `aux1` = bytes (plate, boundary class, volcanic, 0),
//! landform weights (reserved, zero)) followed by the field mips
//! ([`field_mip_layout`]). Float runs hold f32 bit patterns.

/// Shader: shared noise and cube-map sampling followed by the bake stages.
const SHADER: &str = concat!(
    include_str!("shaders/terrain_noise.wgsl"),
    include_str!("shaders/cube_map.wgsl"),
    include_str!("shaders/tier_a.wgsl")
);

/// Field runs in a finished result buffer, in order.
pub const TIER_A_RESULT_RUNS: u32 = 9;
/// Result run indices.
pub mod run {
    pub const ELEVATION: u32 = 0;
    pub const TEMPERATURE: u32 = 1;
    pub const MOISTURE: u32 = 2;
    pub const WIND_EAST: u32 = 3;
    pub const WIND_NORTH: u32 = 4;
    pub const BOUNDARY_COORD: u32 = 5;
    pub const AUX0: u32 = 6;
    pub const AUX1: u32 = 7;
    pub const LANDFORM: u32 = 8;
}
/// Fields with mip chains, in mip order: elevation, temperature, moisture,
/// `boundary_coord`, `aux0`, `aux1`, landform weights.
pub const TIER_A_MIP_FIELDS: u32 = 7;
/// Result run of each mip field.
const MIP_FIELD_RUNS: [u32; 7] = [
    run::ELEVATION,
    run::TEMPERATURE,
    run::MOISTURE,
    run::BOUNDARY_COORD,
    run::AUX0,
    run::AUX1,
    run::LANDFORM,
];
/// Mip reduction per field: 0 f32 average, 1 i32 average, 2 packed unorm8.
const MIP_FIELD_MODES: [u32; 7] = [0, 0, 0, 1, 2, 2, 2];
const HISTOGRAM_BINS: u64 = 4096;
const STATS_SEA_LEVEL_2: usize = 8196;
const STATS_WORDS: u64 = 8200;
const PASS_STRIDE: u64 = 256;
const PASS_BYTES: u64 = 80;
const PARAMS_BYTES: usize = 23 * 16;
const WORKGROUP: u32 = 256;
/// Most plates in the `Plates` uniform (`archetype::MAX_PLATES`).
pub const TIER_A_MAX_PLATES: usize = 32;
/// Diffusion passes of the ocean mask (`tier_a::OCEAN_BLUR_PASSES`).
pub const TIER_A_OCEAN_BLUR_PASSES: u32 = 16;
/// Relief smoothing passes (`tier_a::RELIEF_SMOOTH_PASSES`).
pub const TIER_A_RELIEF_SMOOTH_PASSES: u32 = 4;
/// Wind blur passes (`tier_a::WIND_BLUR_PASSES`).
pub const TIER_A_WIND_BLUR_PASSES: u32 = 4;
/// Erosion fill pyramid (`tier_a::erosion`): coarsest face size, seed passes
/// per face cell, refinement passes, iterations between refills and the
/// fill gradient per texel (m).
pub const FILL_PYRAMID_MIN_CELLS: u32 = 16;
pub const FILL_SEED_PASSES_PER_CELL: u32 = 4;
pub const FILL_PASSES: u32 = 8;
pub const REFILL_INTERVAL: u32 = 20;
pub const PIT_FILL_M: f32 = 0.1;

/// Cube face-edge adjacency of the erosion neighbourhood, mirrored in the
/// shader (`FACE_EDGES`) and equal to `astrum_world::terrain::tier_a::
/// erosion::FACE_EDGES` (checked by the app's GPU tests).
pub const FACE_EDGES: [[u32; 3]; 24] = [
    [4, 1, 0],
    [5, 0, 0],
    [3, 1, 1],
    [2, 1, 0],
    [5, 1, 0],
    [4, 0, 0],
    [3, 0, 0],
    [2, 0, 1],
    [1, 3, 1],
    [0, 3, 0],
    [4, 3, 0],
    [5, 3, 1],
    [1, 2, 0],
    [0, 2, 1],
    [5, 2, 1],
    [4, 2, 0],
    [1, 1, 0],
    [0, 0, 0],
    [3, 3, 0],
    [2, 2, 0],
    [0, 1, 0],
    [1, 0, 0],
    [3, 2, 1],
    [2, 3, 1],
];

/// Stage flags (`TierAStage` in the world crate).
pub mod stage {
    pub const CONTINENTS: u32 = 1;
    pub const SEA_LEVEL: u32 = 2;
    pub const SHELF: u32 = 4;
    pub const TEMPERATURE: u32 = 8;
    pub const WIND: u32 = 16;
    pub const MOISTURE: u32 = 32;
    pub const TECTONICS: u32 = 64;
    pub const RAIN_SHADOW: u32 = 128;
    pub const EROSION: u32 = 256;
}

/// Plain bake inputs, already converted to the shader's units (radians,
/// frequencies in cycles per unit direction, lengths in metres).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TierABakeInputs {
    pub face_cells: u32,
    pub stages: u32,
    pub radius_m: f32,
    pub continent_frequency: f32,
    pub warp_frequency: f32,
    pub warp_scale: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub gain: f32,
    pub continent_seed: u32,
    pub warp_seed: u32,
    pub temperature_seed: u32,
    pub ocean_coverage: f32,
    pub land_height_m: f32,
    pub land_exponent: f32,
    pub ocean_depth_m: f32,
    pub shelf_depth_m: f32,
    pub shelf_fraction: f32,
    pub equator_c: f32,
    pub pole_c: f32,
    pub axial_tilt_rad: f32,
    pub lapse_c_per_km: f32,
    pub ocean_moderation: f32,
    pub ocean_blur_step_rad: f32,
    pub temperature_noise_c: f32,
    pub temperature_noise_frequency: f32,
    pub wind_cells: f32,
    pub wind_meridional: f32,
    pub evaporation: f32,
    pub rain: f32,
    pub moisture_iterations: u32,
    pub moisture_step_rad: f32,
    pub moisture_spread: f32,
    pub precipitation_scale: f32,
    pub rain_convergence: f32,
    pub pole: [f32; 3],
    // M2 (Shape).
    /// Largest absolute elevation (m): the second histogram's range and the
    /// re-zero clamp.
    pub height_bound_m: f32,
    /// `1 / ln(1 + body area km²)`.
    pub flow_normaliser: f32,
    /// Crust share of the continent field (0 without tectonics).
    pub crust_weight: f32,
    pub tectonic_seed: u32,
    pub hardness_seed: u32,
    pub plate_count: u32,
    /// Per plate: centre xyz, power weight, spin xyz, continental 0/1,
    /// hardness, then zeros (`tectonics::PlateSet::packed`).
    pub plates: [[f32; 16]; TIER_A_MAX_PLATES],
    pub plate_warp_frequency: f32,
    pub plate_warp_scale: f32,
    pub boundary_softness_m: f32,
    pub boundary_clamp_m: f32,
    pub collision_height_m: f32,
    pub arc_height_m: f32,
    pub trench_depth_m: f32,
    pub ridge_height_m: f32,
    pub rift_depth_m: f32,
    pub transform_height_m: f32,
    pub orogen_roughness: f32,
    pub roughness_frequency: f32,
    pub orogen_width_m: f32,
    pub arc_width_m: f32,
    pub arc_offset_m: f32,
    pub trench_width_m: f32,
    pub ridge_width_m: f32,
    pub rift_width_m: f32,
    pub transform_width_m: f32,
    pub crust_width_m: f32,
    pub hardness_noise: f32,
    pub hardness_noise_frequency: f32,
    /// Smooth-maximum temperature where boundary relief meets (m).
    pub junction_blend_m: f32,
    /// Relief smoothing and wind blur step (rad).
    pub smoothing_step_rad: f32,
    pub wind_deflection: f32,
    pub deflection_slope: f32,
    pub wind_slowdown: f32,
    pub orographic_rain: f32,
    pub lee_drying: f32,
    pub orographic_slope: f32,
    pub erosion_strength: f32,
    /// Uplift per erosion iteration where tectonic uplift is 1 (m).
    pub erosion_uplift_m: f32,
    pub tan_talus: f32,
    pub deposition: f32,
    pub erosion_capacity: f32,
    pub erosion_area_exponent: f32,
    pub erosion_slope_exponent: f32,
    pub erosion_flow_exponent: f32,
    pub thermal_rate: f32,
    pub sediment_depth_m: f32,
    /// Erosion cascade, coarse to fine: (face-cell divisor, iterations).
    pub erosion_cascade: [[u32; 2]; 3],
    /// Landform weight-rule set bytecode; empty: the weight run stays zero.
    pub landform_rules: Vec<u32>,
}

impl TierABakeInputs {
    pub fn validate(&self, device: &wgpu::Device) -> Result<(), String> {
        if !self.face_cells.is_power_of_two() || !(4..=2048).contains(&self.face_cells) {
            return Err(format!(
                "Tier A face cells {} must be a power of two",
                self.face_cells
            ));
        }
        if !(1..=512).contains(&self.moisture_iterations) {
            return Err("Tier A moisture iterations out of range".into());
        }
        if self.stages & stage::TECTONICS != 0
            && !(2..=TIER_A_MAX_PLATES as u32).contains(&self.plate_count)
        {
            return Err("Tier A plate count out of range".into());
        }
        if self.erosion_cascade.iter().any(|[divisor, iterations]| {
            !divisor.is_power_of_two() || *divisor > 16 || *iterations > 2000
        }) {
            return Err("Tier A erosion cascade out of range".into());
        }
        if !(self.height_bound_m.is_finite() && self.height_bound_m > 0.0) {
            return Err("Tier A height bound must be positive".into());
        }
        let bytes = 4 * Layout::new(self).words;
        let limit = device
            .limits()
            .max_storage_buffer_binding_size
            .min(device.limits().max_buffer_size);
        if bytes > limit {
            return Err(format!("Tier A bake needs {bytes} bytes of storage"));
        }
        Ok(())
    }

    fn packed(&self) -> Vec<u8> {
        let n = self.face_cells;
        let u = |v: [u32; 4]| {
            v.into_iter()
                .flat_map(u32::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        let f = |v: [f32; 4]| {
            v.into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<u8>>()
        };
        [
            u([n, self.stages, self.plate_count, 0]),
            f([
                self.radius_m,
                self.continent_frequency,
                self.warp_frequency,
                self.warp_scale,
            ]),
            f([
                self.octaves as f32,
                self.lacunarity,
                self.gain,
                self.crust_weight,
            ]),
            u([
                self.continent_seed,
                self.warp_seed,
                self.temperature_seed,
                self.tectonic_seed,
            ]),
            f([
                self.ocean_coverage,
                self.land_height_m,
                self.land_exponent,
                self.ocean_depth_m,
            ]),
            f([
                self.shelf_depth_m,
                self.shelf_fraction,
                self.height_bound_m,
                self.flow_normaliser,
            ]),
            f([
                self.equator_c,
                self.pole_c,
                self.axial_tilt_rad,
                self.lapse_c_per_km,
            ]),
            f([
                self.ocean_moderation,
                self.ocean_blur_step_rad,
                self.temperature_noise_c,
                self.temperature_noise_frequency,
            ]),
            f([
                self.wind_cells,
                self.wind_meridional,
                self.evaporation,
                self.rain,
            ]),
            f([
                self.moisture_step_rad,
                self.moisture_spread,
                self.precipitation_scale,
                self.rain_convergence,
            ]),
            f([self.pole[0], self.pole[1], self.pole[2], 0.0]),
            f([
                self.plate_warp_frequency,
                self.plate_warp_scale,
                self.boundary_softness_m,
                self.boundary_clamp_m,
            ]),
            f([
                self.collision_height_m,
                self.arc_height_m,
                self.trench_depth_m,
                self.ridge_height_m,
            ]),
            f([
                self.rift_depth_m,
                self.transform_height_m,
                self.orogen_roughness,
                self.roughness_frequency,
            ]),
            f([
                self.orogen_width_m,
                self.arc_width_m,
                self.arc_offset_m,
                self.trench_width_m,
            ]),
            f([
                self.ridge_width_m,
                self.rift_width_m,
                self.transform_width_m,
                self.crust_width_m,
            ]),
            f([self.hardness_noise, self.hardness_noise_frequency, self.junction_blend_m, 0.0]),
            u([self.hardness_seed, 0, 0, 0]),
            f([
                self.smoothing_step_rad,
                self.wind_deflection,
                self.deflection_slope,
                self.wind_slowdown,
            ]),
            f([
                self.orographic_rain,
                self.lee_drying,
                self.orographic_slope,
                0.0,
            ]),
            f([
                self.erosion_strength,
                self.erosion_uplift_m,
                self.tan_talus,
                self.deposition,
            ]),
            f([
                self.erosion_capacity,
                self.erosion_area_exponent,
                self.erosion_slope_exponent,
                self.erosion_flow_exponent,
            ]),
            f([self.thermal_rate, self.sediment_depth_m, 0.0, 0.0]),
        ]
        .concat()
    }

    fn packed_plates(&self) -> Vec<u8> {
        self.plates
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }

    fn has(&self, flag: u32) -> bool {
        self.stages & flag != 0
    }

    /// Erosion cascade levels that run: iterations > 0 and at least 4 cells.
    fn active_levels(&self) -> Vec<(u32, u32)> {
        if !self.has(stage::EROSION) {
            return Vec::new();
        }
        self.erosion_cascade
            .iter()
            .map(|[divisor, iterations]| (*divisor, *iterations))
            .filter(|(divisor, iterations)| *iterations > 0 && self.face_cells / divisor >= 4)
            .collect()
    }
}

/// Field mip levels in a result buffer: `(offset in words, face cells)` of
/// elevation; temperature and moisture of the same level follow at
/// `+6·cells²` and `+12·cells²`. Level 0 is the result runs; levels 1.. halve
/// the face cells down to 4 and follow the nine result runs, the seven mip
/// fields of a level in [`TIER_A_MIP_FIELDS`] order.
pub fn field_mip_layout(face_cells: u32) -> Vec<(u32, u32)> {
    let mut levels = vec![(0, face_cells)];
    let mut offset = TIER_A_RESULT_RUNS * 6 * face_cells * face_cells;
    let mut cells = face_cells / 2;
    while cells >= 4 {
        levels.push((offset, cells));
        offset += TIER_A_MIP_FIELDS * 6 * cells * cells;
        cells /= 2;
    }
    levels
}

/// Word offset and face cells of mip `level` of mip field `field` (0
/// elevation, 1 temperature, 2 moisture, 3 `boundary_coord`, 4 `aux0`,
/// 5 `aux1`, 6 landform weights).
pub fn field_mip_offset(face_cells: u32, level: usize, field: usize) -> (u32, u32) {
    let (offset, cells) = field_mip_layout(face_cells)[level];
    let len = 6 * cells * cells;
    if level == 0 {
        (MIP_FIELD_RUNS[field] * len, cells)
    } else {
        (offset + field as u32 * len, cells)
    }
}

/// Words of the result buffer (result runs and mips).
fn result_words(face_cells: u32) -> u64 {
    let levels = field_mip_layout(face_cells);
    let (offset, cells) = *levels.last().expect("level zero");
    if levels.len() == 1 {
        u64::from(TIER_A_RESULT_RUNS) * 6 * u64::from(cells) * u64::from(cells)
    } else {
        u64::from(offset) + u64::from(TIER_A_MIP_FIELDS) * 6 * u64::from(cells) * u64::from(cells)
    }
}

/// Scratch fields a validation read-back can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TierAScratch {
    /// Elevation after shape and tectonic relief, before erosion (m).
    PreErosion,
    /// Elevation after erosion, before the re-zero (m).
    Eroded,
    Uplift,
    Hardness,
    /// Distance to the nearest plate boundary (m).
    BoundaryDistance,
    /// Smoothed relief slope (m/m); zero without a rain shadow.
    SlopeEast,
    SlopeNorth,
    /// Drainage area of the last erosion level, upsampled to full size (km²).
    Discharge,
    /// Accumulated deposit (m).
    Deposit,
}

/// Erosion runs of one cascade level.
#[derive(Debug, Clone)]
struct ErosionLevel {
    cells: u32,
    iterations: u32,
    base: u32,
    uplift: u32,
    hardness: u32,
    precipitation: u32,
    h: [u32; 2],
    w: [u32; 2],
    q: [u32; 2],
    qs_in: u32,
    qs_out: u32,
    total: u32,
    slope: u32,
    deposit: u32,
    surface: u32,
    difference: u32,
    /// Fill pyramid above this level: (cells, max-pooled relief, surfaces).
    pyramid: Vec<(u32, u32, [u32; 2])>,
}

/// Word offsets of every run of one bake.
#[derive(Debug, Clone)]
struct Layout {
    len: u32,
    words: u64,
    raw: u32,
    crust: u32,
    dh: u32,
    uplift: u32,
    hardness: u32,
    distance: u32,
    pre_erosion: u32,
    ocean: [u32; 2],
    slope: [u32; 2],
    carried: [u32; 2],
    precipitation: u32,
    eroded: u32,
    /// Full-size discharge and deposit when the finest level is coarser.
    full_discharge: u32,
    full_deposit: u32,
    full_flux: u32,
    /// Box-downsampled inputs per divisor 2, 4, 8, 16: (base, uplift,
    /// hardness, precipitation).
    reduced: Vec<(u32, [u32; 4])>,
    levels: Vec<ErosionLevel>,
}

impl Layout {
    fn new(inputs: &TierABakeInputs) -> Self {
        let n = inputs.face_cells;
        let len = 6 * n * n;
        let mut next = result_words(n);
        let mut take = |cells: u32| -> u32 {
            let at = next;
            next += 6 * u64::from(cells) * u64::from(cells);
            at as u32
        };
        let raw = take(n);
        let crust = take(n);
        let dh = take(n);
        let uplift = take(n);
        let hardness = take(n);
        let distance = take(n);
        let pre_erosion = take(n);
        let ocean = [take(n), take(n)];
        let slope = [take(n), take(n)];
        let carried = [take(n), take(n)];
        let precipitation = take(n);
        let eroded = take(n);
        let active = inputs.active_levels();
        let mut reduced = Vec::new();
        let mut levels = Vec::new();
        let (full_discharge, full_deposit, full_flux);
        if !active.is_empty() {
            let deepest = active
                .iter()
                .map(|(divisor, _)| *divisor)
                .max()
                .unwrap_or(1);
            let mut divisor = 2;
            while divisor <= deepest {
                let cells = n / divisor;
                reduced.push((
                    divisor,
                    [take(cells), take(cells), take(cells), take(cells)],
                ));
                divisor *= 2;
            }
            let moisture = run::MOISTURE * len;
            for (divisor, iterations) in &active {
                let cells = n / divisor;
                let inputs_of = if *divisor == 1 {
                    [pre_erosion, uplift, hardness, moisture]
                } else {
                    reduced
                        .iter()
                        .find(|(d, _)| d == divisor)
                        .expect("every divisor reduced")
                        .1
                };
                // The full-size level reuses runs free after the moisture. The
                // slope runs stay intact: the landform weights read them last.
                let (h, w, q, qs_in, qs_out, total) = if *divisor == 1 {
                    (
                        [raw, crust],
                        [dh, ocean[0]],
                        [take(n), take(n)],
                        carried[0],
                        carried[1],
                        precipitation,
                    )
                } else {
                    (
                        [take(cells), take(cells)],
                        [take(cells), take(cells)],
                        [take(cells), take(cells)],
                        take(cells),
                        take(cells),
                        take(cells),
                    )
                };
                let mut pyramid = Vec::new();
                let mut size = cells;
                while size / 2 >= FILL_PYRAMID_MIN_CELLS {
                    size /= 2;
                    pyramid.push((size, take(size), [take(size), take(size)]));
                }
                levels.push(ErosionLevel {
                    cells,
                    iterations: *iterations,
                    base: inputs_of[0],
                    uplift: inputs_of[1],
                    hardness: inputs_of[2],
                    precipitation: inputs_of[3],
                    h,
                    w,
                    q,
                    qs_in,
                    qs_out,
                    total,
                    slope: take(cells),
                    deposit: take(cells),
                    surface: take(cells),
                    difference: take(cells),
                    pyramid,
                });
            }
            full_deposit = take(n);
            full_discharge = take(n);
            full_flux = carried[1];
        } else {
            // No erosion: deposit and discharge read as zeros.
            full_deposit = take(n);
            full_discharge = full_deposit;
            full_flux = full_deposit;
        }
        Self {
            len,
            words: next,
            raw,
            crust,
            dh,
            uplift,
            hardness,
            distance,
            pre_erosion,
            ocean,
            slope,
            carried,
            precipitation,
            eroded,
            full_discharge,
            full_deposit,
            full_flux,
            reduced,
            levels,
        }
    }

    fn result(&self, index: u32) -> u32 {
        index * self.len
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    LandformWeights,
    Tectonics,
    Continents,
    SeaLevel,
    Shape,
    Blur,
    Slope,
    Climate,
    WindDeflect,
    WindBlur,
    MoistureStep,
    MoistureFinish,
    Reduce,
    ErodeSet,
    ErodeLift,
    FillRefine,
    FillStep,
    ErodeRoute,
    ErodeAccumulate,
    ErodeUpdate,
    ErodeFinish,
    Histogram2,
    SeaLevel2,
    FinishShape,
    FieldMip,
}

const ENTRY_POINTS: [(Stage, &str); 25] = [
    (Stage::Tectonics, "tectonics"),
    (Stage::Continents, "continents"),
    (Stage::SeaLevel, "sea_level"),
    (Stage::Shape, "shape"),
    (Stage::Blur, "blur"),
    (Stage::Slope, "slope"),
    (Stage::Climate, "climate"),
    (Stage::WindDeflect, "wind_deflect"),
    (Stage::WindBlur, "wind_blur"),
    (Stage::MoistureStep, "moisture_step"),
    (Stage::MoistureFinish, "moisture_finish"),
    (Stage::Reduce, "reduce"),
    (Stage::ErodeSet, "erode_set"),
    (Stage::ErodeLift, "erode_lift"),
    (Stage::FillRefine, "fill_refine"),
    (Stage::FillStep, "fill_step"),
    (Stage::ErodeRoute, "erode_route"),
    (Stage::ErodeAccumulate, "erode_accumulate"),
    (Stage::ErodeUpdate, "erode_update"),
    (Stage::ErodeFinish, "erode_finish"),
    (Stage::Histogram2, "histogram_2"),
    (Stage::SeaLevel2, "sea_level_2"),
    (Stage::FinishShape, "finish_shape"),
    (Stage::LandformWeights, "landform_weights"),
    (Stage::FieldMip, "field_mip"),
];

/// One scheduled pass: entry point, texel grid, sampling grid, mode,
/// scalars and run offsets (`Pass` in the shader).
#[derive(Debug, Clone, Copy)]
struct PassSpec {
    stage: Stage,
    n: u32,
    sample_n: u32,
    mode: u32,
    scalars: [f32; 4],
    offsets: [u32; 12],
}

impl PassSpec {
    fn new(stage: Stage, n: u32, offsets: &[u32]) -> Self {
        let mut all = [0u32; 12];
        all[..offsets.len()].copy_from_slice(offsets);
        Self {
            stage,
            n,
            sample_n: n,
            mode: 0,
            scalars: [0.0; 4],
            offsets: all,
        }
    }

    fn mode(mut self, mode: u32) -> Self {
        self.mode = mode;
        self
    }

    fn scalars(mut self, x: f32, y: f32) -> Self {
        self.scalars = [x, y, 0.0, 0.0];
        self
    }

    fn sample(mut self, n: u32) -> Self {
        self.sample_n = n;
        self
    }

    fn single_thread(&self) -> bool {
        matches!(self.stage, Stage::SeaLevel | Stage::SeaLevel2)
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(PASS_BYTES as usize);
        for v in [self.n, self.sample_n, self.mode, 0] {
            out.extend(v.to_le_bytes());
        }
        for v in self.scalars {
            out.extend(v.to_le_bytes());
        }
        for v in self.offsets {
            out.extend(v.to_le_bytes());
        }
        out
    }
}

/// Fill the water surface of `level` (relief `h` → surface `w[current]`):
/// max-pool pyramid, seed its coarsest level and refine down
/// (`tier_a::erosion::refill`).
fn schedule_refill(schedule: &mut Vec<PassSpec>, level: &ErosionLevel, h: u32, current: usize) {
    let mut heights = vec![(level.cells, h)];
    for (cells, hmax, _) in &level.pyramid {
        let (_, finer) = *heights.last().expect("level zero");
        schedule.push(PassSpec::new(Stage::Reduce, *cells, &[finer, *hmax]).mode(1));
        heights.push((*cells, *hmax));
    }
    let top = level.pyramid.len();
    let surfaces = |k: usize| -> [u32; 2] {
        if k == 0 {
            [level.w[current], level.w[1 - current]]
        } else {
            level.pyramid[k - 1].2
        }
    };
    for k in (0..=top).rev() {
        let (cells, hk) = heights[k];
        let [a, b] = surfaces(k);
        let passes = if k == top {
            schedule.push(PassSpec::new(Stage::ErodeSet, cells, &[hk, a]).mode(2));
            FILL_SEED_PASSES_PER_CELL * cells
        } else {
            let parent = surfaces(k + 1)[0];
            schedule.push(PassSpec::new(Stage::FillRefine, cells, &[hk, parent, a]));
            FILL_PASSES
        };
        let step = PIT_FILL_M * (1u32 << k) as f32;
        for pass in 0..passes {
            let (src, dst) = if pass % 2 == 0 { (a, b) } else { (b, a) };
            schedule
                .push(PassSpec::new(Stage::FillStep, cells, &[hk, src, dst]).scalars(step, 0.0));
        }
    }
}

/// The pass schedule of one bake.
fn schedule(
    inputs: &TierABakeInputs,
    layout: &Layout,
) -> (Vec<PassSpec>, Vec<(TierAScratch, u32)>) {
    let n = inputs.face_cells;
    let l = layout;
    let result = |index| l.result(index);
    let mut s = vec![
        PassSpec::new(
            Stage::Tectonics,
            n,
            &[
                l.crust,
                l.dh,
                l.uplift,
                l.hardness,
                l.distance,
                result(run::BOUNDARY_COORD),
                result(run::AUX1),
            ],
        ),
        PassSpec::new(Stage::Continents, n, &[l.raw, l.crust]),
        PassSpec::new(Stage::SeaLevel, n, &[]),
        PassSpec::new(Stage::Shape, n, &[l.raw, l.dh, l.pre_erosion, l.ocean[0]]),
    ];
    // Relief smoothing (rain shadow): h_s ends in ocean[0].
    let rain_shadow = inputs.has(stage::RAIN_SHADOW);
    if rain_shadow {
        for pass in 0..TIER_A_RELIEF_SMOOTH_PASSES {
            let (src, dst) = if pass % 2 == 0 {
                (l.ocean[0], l.ocean[1])
            } else {
                (l.ocean[1], l.ocean[0])
            };
            s.push(
                PassSpec::new(Stage::Blur, n, &[src, dst]).scalars(inputs.smoothing_step_rad, 0.0),
            );
        }
    }
    s.push(
        PassSpec::new(
            Stage::Slope,
            n,
            &[
                l.ocean[0],
                l.pre_erosion,
                l.slope[0],
                l.slope[1],
                l.ocean[1],
            ],
        )
        .scalars(inputs.smoothing_step_rad, 0.0),
    );
    // Ocean blur from ocean[1]; an even pass count ends there again.
    let blur = if inputs.ocean_blur_step_rad > 0.0 {
        TIER_A_OCEAN_BLUR_PASSES
    } else {
        0
    };
    for pass in 0..blur {
        let (src, dst) = if pass % 2 == 0 {
            (l.ocean[1], l.ocean[0])
        } else {
            (l.ocean[0], l.ocean[1])
        };
        s.push(PassSpec::new(Stage::Blur, n, &[src, dst]).scalars(inputs.ocean_blur_step_rad, 0.0));
    }
    let ocean = l.ocean[1];
    s.push(PassSpec::new(
        Stage::Climate,
        n,
        &[
            l.pre_erosion,
            ocean,
            result(run::TEMPERATURE),
            result(run::WIND_EAST),
            result(run::WIND_NORTH),
        ],
    ));
    let wind = [result(run::WIND_EAST), result(run::WIND_NORTH)];
    if rain_shadow && inputs.has(stage::WIND) {
        s.push(PassSpec::new(
            Stage::WindDeflect,
            n,
            &[wind[0], wind[1], l.slope[0], l.slope[1]],
        ));
        // Ping-pong through the carried-moisture runs; even count ends in
        // the result runs.
        for pass in 0..TIER_A_WIND_BLUR_PASSES {
            let (src, dst) = if pass % 2 == 0 {
                (wind, l.carried)
            } else {
                (l.carried, wind)
            };
            s.push(
                PassSpec::new(Stage::WindBlur, n, &[src[0], src[1], dst[0], dst[1]])
                    .scalars(inputs.smoothing_step_rad, 0.0),
            );
        }
        s.push(PassSpec::new(Stage::ErodeSet, n, &[0, l.carried[0]]).mode(1));
        s.push(PassSpec::new(Stage::ErodeSet, n, &[0, l.carried[1]]).mode(1));
    }
    if inputs.has(stage::MOISTURE) {
        for pass in 0..inputs.moisture_iterations {
            let (src, dst) = if pass % 2 == 0 {
                (l.carried[0], l.carried[1])
            } else {
                (l.carried[1], l.carried[0])
            };
            s.push(PassSpec::new(
                Stage::MoistureStep,
                n,
                &[
                    src,
                    dst,
                    wind[0],
                    wind[1],
                    l.pre_erosion,
                    result(run::TEMPERATURE),
                    l.precipitation,
                    l.slope[0],
                    l.slope[1],
                ],
            ));
        }
    }
    s.push(PassSpec::new(
        Stage::MoistureFinish,
        n,
        &[l.precipitation, result(run::MOISTURE)],
    ));

    // Erosion cascade.
    let mut eroded = l.pre_erosion;
    let mut deposit = l.full_deposit;
    let mut discharge = l.full_discharge;
    if !l.levels.is_empty() {
        let mut fine = (l.pre_erosion, l.uplift, l.hardness, result(run::MOISTURE));
        for (divisor, runs) in &l.reduced {
            let cells = n / divisor;
            for (src, dst) in [fine.0, fine.1, fine.2, fine.3].into_iter().zip(runs) {
                s.push(PassSpec::new(Stage::Reduce, cells, &[src, *dst]));
            }
            fine = (runs[0], runs[1], runs[2], runs[3]);
        }
        let last = l.levels.len() - 1;
        let mut previous: Option<(&ErosionLevel, usize)> = None;
        for (index, level) in l.levels.iter().enumerate() {
            let cells = level.cells;
            match previous {
                None => {
                    s.push(PassSpec::new(
                        Stage::ErodeSet,
                        cells,
                        &[level.base, level.h[0]],
                    ));
                    for run in [level.q[0], level.qs_out, level.deposit] {
                        s.push(PassSpec::new(Stage::ErodeSet, cells, &[0, run]).mode(1));
                    }
                }
                Some((coarse, current)) => {
                    s.push(
                        PassSpec::new(
                            Stage::ErodeSet,
                            coarse.cells,
                            &[coarse.h[current], coarse.difference, coarse.base],
                        )
                        .mode(3),
                    );
                    s.push(
                        PassSpec::new(
                            Stage::ErodeLift,
                            cells,
                            &[
                                level.base,
                                coarse.difference,
                                coarse.q[current],
                                coarse.qs_out,
                                coarse.deposit,
                                level.h[0],
                                level.q[0],
                                level.qs_out,
                                level.deposit,
                            ],
                        )
                        .sample(coarse.cells),
                    );
                }
            }
            let mut current = 0usize;
            schedule_refill(&mut s, level, level.h[0], 0);
            for iteration in 0..level.iterations {
                let next = 1 - current;
                s.push(PassSpec::new(
                    Stage::ErodeRoute,
                    cells,
                    &[level.w[current], level.total, level.slope],
                ));
                s.push(PassSpec::new(
                    Stage::ErodeAccumulate,
                    cells,
                    &[
                        level.w[current],
                        level.total,
                        level.q[current],
                        level.qs_out,
                        level.precipitation,
                        level.q[next],
                        level.qs_in,
                    ],
                ));
                s.push(PassSpec::new(
                    Stage::ErodeUpdate,
                    cells,
                    &[
                        level.h[current],
                        level.w[current],
                        level.q[next],
                        level.qs_in,
                        level.slope,
                        level.uplift,
                        level.hardness,
                        level.h[next],
                        level.qs_out,
                        level.deposit,
                        level.surface,
                    ],
                ));
                s.push(
                    PassSpec::new(
                        Stage::FillStep,
                        cells,
                        &[level.h[next], level.surface, level.w[next]],
                    )
                    .scalars(PIT_FILL_M, inputs.erosion_uplift_m + PIT_FILL_M),
                );
                current = next;
                if (iteration + 1) % REFILL_INTERVAL == 0
                    || (index == last && iteration + 1 == level.iterations)
                {
                    schedule_refill(&mut s, level, level.h[current], current);
                }
            }
            previous = Some((level, current));
        }
        let (level, current) = previous.expect("at least one level");
        if level.cells == n {
            s.push(PassSpec::new(
                Stage::ErodeFinish,
                n,
                &[level.h[current], level.w[current], level.deposit, l.eroded],
            ));
            deposit = level.deposit;
            discharge = level.q[current];
        } else {
            // The finest level is coarser than the bake: fill, then upsample
            // the filled difference, discharge and deposit to full size.
            s.push(PassSpec::new(
                Stage::ErodeFinish,
                level.cells,
                &[
                    level.h[current],
                    level.w[current],
                    level.deposit,
                    level.surface,
                ],
            ));
            s.push(
                PassSpec::new(
                    Stage::ErodeSet,
                    level.cells,
                    &[level.surface, level.difference, level.base],
                )
                .mode(3),
            );
            s.push(
                PassSpec::new(
                    Stage::ErodeLift,
                    n,
                    &[
                        l.pre_erosion,
                        level.difference,
                        level.q[current],
                        level.qs_out,
                        level.deposit,
                        l.eroded,
                        l.full_discharge,
                        l.full_flux,
                        l.full_deposit,
                    ],
                )
                .sample(level.cells),
            );
        }
        eroded = l.eroded;
    }
    s.push(PassSpec::new(Stage::Histogram2, n, &[eroded]));
    s.push(PassSpec::new(Stage::SeaLevel2, n, &[]));
    s.push(PassSpec::new(
        Stage::FinishShape,
        n,
        &[
            eroded,
            ocean,
            l.uplift,
            l.hardness,
            deposit,
            discharge,
            result(run::ELEVATION),
            result(run::TEMPERATURE),
            result(run::AUX0),
        ],
    ));
    // Landform weights (result run 8) from the set's rule bytecode.
    if !inputs.landform_rules.is_empty() {
        s.push(PassSpec::new(
            Stage::LandformWeights,
            n,
            &[
                result(run::ELEVATION),
                result(run::TEMPERATURE),
                result(run::MOISTURE),
                result(run::BOUNDARY_COORD),
                result(run::AUX0),
                result(run::AUX1),
                l.slope[0],
                l.slope[1],
                result(run::LANDFORM),
            ],
        ));
    }

    let mips = field_mip_layout(n);
    for level in 0..mips.len() - 1 {
        for (field, mode) in MIP_FIELD_MODES.into_iter().enumerate() {
            let (src, _) = field_mip_offset(n, level, field);
            let (dst, cells) = field_mip_offset(n, level + 1, field);
            s.push(PassSpec::new(Stage::FieldMip, cells, &[src, dst]).mode(mode));
        }
    }
    let erosion = !l.levels.is_empty();
    let scratch = vec![
        (TierAScratch::PreErosion, l.pre_erosion),
        (TierAScratch::Eroded, eroded),
        (TierAScratch::Uplift, l.uplift),
        (TierAScratch::Hardness, l.hardness),
        (TierAScratch::BoundaryDistance, l.distance),
        (TierAScratch::SlopeEast, l.slope[0]),
        (TierAScratch::SlopeNorth, l.slope[1]),
    ]
    .into_iter()
    .chain(
        erosion
            .then_some([
                (TierAScratch::Discharge, discharge),
                (TierAScratch::Deposit, deposit),
            ])
            .into_iter()
            .flatten(),
    )
    .collect();
    (s, scratch)
}

/// Compute pipelines shared by every bake.
pub(crate) struct TierAPipelines {
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<(Stage, wgpu::ComputePipeline)>,
}

impl TierAPipelines {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tier A bake"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let storage = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Tier A bake"),
            entries: &[
                entry(0, uniform),
                entry(1, storage),
                entry(2, storage),
                entry(
                    3,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(PASS_BYTES),
                    },
                ),
                entry(4, uniform),
                entry(
                    5,
                    wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Tier A bake"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipelines = ENTRY_POINTS
            .into_iter()
            .map(|(stage, entry_point)| {
                (
                    stage,
                    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                        label: Some(entry_point),
                        layout: Some(&pipeline_layout),
                        module: &module,
                        entry_point: Some(entry_point),
                        compilation_options: Default::default(),
                        cache: None,
                    }),
                )
            })
            .collect();
        Self { layout, pipelines }
    }

    fn pipeline(&self, stage: Stage) -> &wgpu::ComputePipeline {
        &self
            .pipelines
            .iter()
            .find(|(s, _)| *s == stage)
            .expect("every stage has a pipeline")
            .1
    }
}

/// One body's bake in progress, then its result buffer.
pub(crate) struct TierABake {
    face_cells: u32,
    schedule: Vec<PassSpec>,
    scratch_runs: Vec<(TierAScratch, u32)>,
    next: usize,
    scratch: Option<(
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::BindGroup,
    )>,
    result: wgpu::Buffer,
    done: bool,
}

impl TierABake {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipelines: &TierAPipelines,
        inputs: &TierABakeInputs,
    ) -> Result<Self, String> {
        inputs.validate(device)?;
        let n = inputs.face_cells;
        let layout = Layout::new(inputs);
        let (schedule, scratch_runs) = schedule(inputs, &layout);
        let fields = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake fields"),
            size: 4 * layout.words,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let stats = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake histogram"),
            size: 4 * STATS_WORDS,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake parameters"),
            size: PARAMS_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params, 0, &inputs.packed());
        let plates = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A plates"),
            size: (TIER_A_MAX_PLATES * 64) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&plates, 0, &inputs.packed_plates());
        // Landform rules (at least one word: empty sets bind a zero word).
        let mut rule_words = inputs.landform_rules.clone();
        if rule_words.is_empty() {
            rule_words.push(0);
        }
        let rule_bytes: Vec<u8> = rule_words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let landform_rules = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A landform rules"),
            size: rule_bytes.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&landform_rules, 0, &rule_bytes);
        let passes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A bake passes"),
            size: PASS_STRIDE * schedule.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut pass_bytes = vec![0u8; (PASS_STRIDE * schedule.len() as u64) as usize];
        for (index, spec) in schedule.iter().enumerate() {
            let at = index * PASS_STRIDE as usize;
            pass_bytes[at..at + PASS_BYTES as usize].copy_from_slice(&spec.bytes());
        }
        queue.write_buffer(&passes, 0, &pass_bytes);
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Tier A bake"),
            layout: &pipelines.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fields.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: stats.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &passes,
                        offset: 0,
                        size: wgpu::BufferSize::new(PASS_BYTES),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: plates.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: landform_rules.as_entire_binding(),
                },
            ],
        });
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A fields"),
            size: 4 * result_words(n),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Ok(Self {
            face_cells: n,
            schedule,
            scratch_runs,
            next: 0,
            scratch: Some((fields, stats, params, passes, plates, group)),
            result,
            done: false,
        })
    }

    pub(crate) fn face_cells(&self) -> u32 {
        self.face_cells
    }

    pub(crate) fn done(&self) -> bool {
        self.done
    }

    pub(crate) fn result(&self) -> &wgpu::Buffer {
        &self.result
    }

    /// Texel passes of `spec` (single-thread passes count as a full pass).
    fn cost(&self, spec: &PassSpec) -> u64 {
        let n = if spec.single_thread() {
            self.face_cells
        } else {
            spec.n
        };
        6 * u64::from(n) * u64::from(n)
    }

    /// Encode passes worth up to `budget` full-resolution passes (at least
    /// one pass); on the last one, copy the result runs and mips out of the
    /// scratch buffer and release it.
    pub(crate) fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        pipelines: &TierAPipelines,
        budget: usize,
    ) {
        let Some((fields, _, _, _, _, group)) = &self.scratch else {
            return;
        };
        let full = 6 * u64::from(self.face_cells) * u64::from(self.face_cells);
        let budget = (budget as u64).saturating_mul(full);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Tier A bake"),
                timestamp_writes: None,
            });
            let mut spent = 0u64;
            while self.next < self.schedule.len() {
                let spec = self.schedule[self.next];
                let cost = self.cost(&spec);
                if spent > 0 && spent.saturating_add(cost) > budget {
                    break;
                }
                spent += cost;
                pass.set_pipeline(pipelines.pipeline(spec.stage));
                pass.set_bind_group(0, group, &[(self.next as u64 * PASS_STRIDE) as u32]);
                if spec.single_thread() {
                    pass.dispatch_workgroups(1, 1, 1);
                } else {
                    let groups = (6 * spec.n * spec.n).div_ceil(WORKGROUP);
                    let (x, y) = if groups <= 65_535 {
                        (groups, 1)
                    } else {
                        (65_535, groups.div_ceil(65_535))
                    };
                    pass.dispatch_workgroups(x, y, 1);
                }
                self.next += 1;
            }
        }
        if self.next == self.schedule.len() {
            encoder.copy_buffer_to_buffer(fields, 0, &self.result, 0, self.result.size());
            self.done = true;
        }
    }

    /// Release the scratch buffers once the final copy has been submitted.
    pub(crate) fn release_scratch(&mut self) {
        if self.done {
            self.scratch = None;
        }
    }

    fn stats(&self) -> Option<&wgpu::Buffer> {
        self.scratch.as_ref().map(|s| &s.1)
    }

    fn fields(&self) -> Option<&wgpu::Buffer> {
        self.scratch.as_ref().map(|s| &s.0)
    }

    /// Passes in the schedule and their total cost in full-resolution passes.
    fn schedule_cost(&self) -> (usize, f64) {
        let full = 6.0 * f64::from(self.face_cells) * f64::from(self.face_cells);
        let total: u64 = self.schedule.iter().map(|spec| self.cost(spec)).sum();
        (self.schedule.len(), total as f64 / full)
    }
}

/// Read-back of a finished bake, validation only.
#[derive(Debug, Clone)]
pub struct TierAReadback {
    pub face_cells: u32,
    /// Result runs then mips as f32 bit patterns ([`Self::run`] for float
    /// runs, [`Self::run_words`] for integer and packed runs).
    pub fields: Vec<f32>,
    /// Normalised noise at sea level and at the 0.1 % and 99.9 % percentiles.
    pub sea_level: f32,
    pub noise_low: f32,
    pub noise_high: f32,
    /// Second sea level (m) subtracted by the re-zero.
    pub sea_level_m: f32,
    /// Named scratch runs (bit patterns), when requested.
    pub scratch: Vec<(TierAScratch, Vec<u32>)>,
    /// Scheduled passes and their cost in full-resolution passes.
    pub passes: usize,
    pub full_passes: f64,
}

impl TierAReadback {
    pub fn run(&self, index: usize) -> &[f32] {
        let len = 6 * (self.face_cells * self.face_cells) as usize;
        &self.fields[index * len..(index + 1) * len]
    }

    /// Run `index` as raw words (`boundary_coord`, `aux0`, `aux1`, landform).
    pub fn run_words(&self, index: usize) -> Vec<u32> {
        self.run(index).iter().map(|v| v.to_bits()).collect()
    }

    /// Mip `level` of mip field `field` (see [`field_mip_offset`]).
    pub fn field_mip(&self, field: usize, level: usize) -> &[f32] {
        let (offset, cells) = field_mip_offset(self.face_cells, level, field);
        let start = offset as usize;
        &self.fields[start..start + 6 * (cells * cells) as usize]
    }

    /// A requested scratch run as f32.
    pub fn scratch_f32(&self, run: TierAScratch) -> Option<Vec<f32>> {
        self.scratch
            .iter()
            .find(|(r, _)| *r == run)
            .map(|(_, words)| words.iter().map(|w| f32::from_bits(*w)).collect())
    }
}

/// Bake `inputs` on a caller-owned device and read every field back.
/// Validation only: blocking, allocates per call.
pub fn tier_a_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    inputs: &TierABakeInputs,
) -> Result<TierAReadback, String> {
    tier_a_for_validation_budgeted(device, queue, inputs, usize::MAX)
}

/// As [`tier_a_for_validation`], but encoding at most `passes` full-resolution
/// pass equivalents per submission, like the runtime spreads a bake over
/// frames.
pub fn tier_a_for_validation_budgeted(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    inputs: &TierABakeInputs,
    passes: usize,
) -> Result<TierAReadback, String> {
    TierAValidation::new(device).bake(device, queue, inputs, passes)
}

/// Compiled Tier A pipelines reused across validation bakes, so comparisons
/// between bakes do not also compare two driver compilations of the shader
/// (observed to differ once right after a shader change).
pub struct TierAValidation {
    pipelines: TierAPipelines,
}

impl TierAValidation {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            pipelines: TierAPipelines::new(device),
        }
    }

    /// Bake `inputs` with at most `passes` full-resolution pass equivalents
    /// per submission.
    pub fn bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: &TierABakeInputs,
        passes: usize,
    ) -> Result<TierAReadback, String> {
        bake_for_validation(device, queue, &self.pipelines, inputs, passes, false)
    }

    /// As [`Self::bake`], also reading back the named scratch runs.
    pub fn bake_with_scratch(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: &TierABakeInputs,
        passes: usize,
    ) -> Result<TierAReadback, String> {
        bake_for_validation(device, queue, &self.pipelines, inputs, passes, true)
    }

    /// Wall time of one whole bake on the GPU (encode, submit, wait), without
    /// read-back: a diagnostic, not a frame-time measurement.
    pub fn time_bake(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: &TierABakeInputs,
    ) -> Result<std::time::Duration, String> {
        let mut bake = TierABake::new(device, queue, &self.pipelines, inputs)?;
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        let started = std::time::Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        bake.encode(&mut encoder, &self.pipelines, usize::MAX);
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| error.to_string())?;
        Ok(started.elapsed())
    }
}

fn bake_for_validation(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipelines: &TierAPipelines,
    inputs: &TierABakeInputs,
    passes: usize,
    with_scratch: bool,
) -> Result<TierAReadback, String> {
    let mut bake = TierABake::new(device, queue, pipelines, inputs)?;
    let mut encoder = device.create_command_encoder(&Default::default());
    bake.encode(&mut encoder, pipelines, passes.max(1));
    while !bake.done() {
        queue.submit([std::mem::replace(
            &mut encoder,
            device.create_command_encoder(&Default::default()),
        )
        .finish()]);
        bake.encode(&mut encoder, pipelines, passes.max(1));
    }
    let read =
        |encoder: &mut wgpu::CommandEncoder, source: &wgpu::Buffer, offset: u64, size: u64| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Tier A validation readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_buffer_to_buffer(source, offset, &buffer, 0, size);
            buffer
        };
    let fields = read(&mut encoder, bake.result(), 0, bake.result().size());
    let stats = read(
        &mut encoder,
        bake.stats().ok_or("bake released its scratch early")?,
        0,
        4 * STATS_WORDS,
    );
    let run_bytes = 4 * 6 * u64::from(bake.face_cells()) * u64::from(bake.face_cells());
    let scratch: Vec<(TierAScratch, wgpu::Buffer)> = if with_scratch {
        let source = bake.fields().ok_or("bake released its scratch early")?;
        bake.scratch_runs
            .iter()
            .map(|(run, offset)| {
                (
                    *run,
                    read(&mut encoder, source, 4 * u64::from(*offset), run_bytes),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    queue.submit([encoder.finish()]);
    for buffer in [&fields, &stats]
        .into_iter()
        .chain(scratch.iter().map(|(_, b)| b))
    {
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|error| error.to_string())?;
    let words = |buffer: &wgpu::Buffer| -> Result<Vec<u32>, String> {
        let view = buffer
            .slice(..)
            .get_mapped_range()
            .map_err(|error| error.to_string())?;
        Ok(view
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect())
    };
    let values = words(&fields)?.into_iter().map(f32::from_bits).collect();
    let stat_words = words(&stats)?;
    let at = HISTOGRAM_BINS as usize;
    let (passes, full_passes) = bake.schedule_cost();
    let scratch = scratch
        .iter()
        .map(|(run, buffer)| Ok((*run, words(buffer)?)))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(TierAReadback {
        face_cells: bake.face_cells(),
        fields: values,
        sea_level: f32::from_bits(stat_words[at]),
        noise_low: f32::from_bits(stat_words[at + 1]),
        noise_high: f32::from_bits(stat_words[at + 2]),
        sea_level_m: f32::from_bits(stat_words[STATS_SEA_LEVEL_2]),
        scratch,
        passes,
        full_passes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_a_shader_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(SHADER)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
    }

    #[test]
    fn packed_parameters_match_the_uniform_layout() {
        assert_eq!(TierABakeInputs::default().packed().len(), PARAMS_BYTES);
        assert_eq!(
            TierABakeInputs::default().packed_plates().len(),
            TIER_A_MAX_PLATES * 64
        );
        let spec = PassSpec::new(Stage::Blur, 4, &[1, 2]);
        assert_eq!(spec.bytes().len() as u64, PASS_BYTES);
    }

    #[test]
    fn shader_face_edge_table_matches_the_rust_table() {
        let source = include_str!("shaders/tier_a.wgsl");
        let entries: Vec<String> = FACE_EDGES
            .iter()
            .map(|[g, e, f]| format!("vec3<u32>({g}u, {e}u, {f}u)"))
            .collect();
        let table = source
            .split("FACE_EDGES: array<vec3<u32>, 24> = array<vec3<u32>, 24>(")
            .nth(1)
            .and_then(|rest| rest.split(");").next())
            .expect("FACE_EDGES in the shader");
        let found: Vec<String> = table
            .split("vec3<u32>(")
            .skip(1)
            .map(|e| format!("vec3<u32>({}", e.split(')').next().unwrap_or("")) + ")")
            .collect();
        assert_eq!(found, entries);
    }

    #[test]
    fn layout_and_schedule_cover_every_stage_within_bounds() {
        let inputs = TierABakeInputs {
            face_cells: 64,
            stages: 0x1ff,
            moisture_iterations: 4,
            plate_count: 4,
            height_bound_m: 1.0e4,
            ocean_blur_step_rad: 0.01,
            erosion_cascade: [[4, 3], [2, 2], [1, 2]],
            ..Default::default()
        };
        let layout = Layout::new(&inputs);
        let (passes, scratch) = schedule(&inputs, &layout);
        for spec in &passes {
            let words = 6 * u64::from(spec.n) * u64::from(spec.n);
            let used = spec.offsets.iter().filter(|o| **o > 0).count();
            assert!(used > 0 || spec.single_thread() || spec.stage == Stage::ErodeSet);
            for offset in spec.offsets {
                assert!(u64::from(offset) + words <= layout.words, "{spec:?}");
            }
        }
        assert!(passes.iter().any(|p| p.stage == Stage::ErodeUpdate));
        assert!(scratch.iter().any(|(r, _)| *r == TierAScratch::Discharge));
        // 512² Terra-like bake fits the 512 MiB binding.
        let big = TierABakeInputs {
            face_cells: 512,
            erosion_cascade: [[4, 200], [2, 100], [1, 60]],
            ..inputs
        };
        assert!(4 * Layout::new(&big).words < 512 << 20);
    }
}
