//! Tier A world-map bake, CPU test oracle (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6).
//!
//! Under ADR 0023 the GPU bake (`renderer::tier_a`) produces the fields the
//! terrain uses; this f64 bake defines the same functions and checks the GPU
//! within tolerance. M1 (World map and planet editor) stages: continents with a
//! domain warp, sea level from an area-weighted histogram (exact ocean
//! coverage), a continental shelf, temperature (insolation by latitude and
//! axial tilt, lapse rate, ocean moderation), three-cell wind bands and moisture
//! advection. M2 (Shape) adds tectonics ([`tectonics`]: plates, boundary
//! profiles, crust, hardness), a rain shadow ([`climate`]: wind deflected
//! around smoothed relief, orographic rain, lee drying) and macro erosion
//! ([`erosion`]), followed by a second sea-level histogram in metres that
//! re-zeroes the eroded relief and applies the shelf as a monotone remap.
//!
//! Stage order (M2 design §1): tectonics → continents → sea level → shape + dh
//! → relief smoothing and slope → ocean blur → climate → wind deflection →
//! moisture → erosion → histogram 2 → re-zero and shelf → final temperature →
//! landform weights (not yet: run 8 stays zero) → mips.
//!
//! Every stage is per texel, except the histograms and the iterated passes
//! (ocean diffusion, relief smoothing, wind blur, moisture), which sample the
//! previous iteration through [`CubeMap::bilinear`] so face edges need no
//! special cases, and erosion, which uses the integer face-edge neighbour
//! table shared with the GPU ([`erosion::FACE_EDGES`]).
pub mod climate;
pub mod erosion;
#[cfg(test)]
mod preview;
pub mod tectonics;

use super::{
    TerrainError,
    archetype::{PlanetParams, TierAStage},
    noise::gradient_noise,
    world_map::{CubeMap, texel_direction},
};
use glam::DVec3;

pub use climate::{
    base_temperature, evaporation_factor, insolation, rain_modulation, tangent_frame, wind,
};

/// Sea-level histogram resolution over the normalised noise range `[-1, 1]`
/// (first histogram) and over `±height bound` metres (second histogram).
pub const HISTOGRAM_BINS: usize = 4096;
/// Integer area weight of the largest (face-centre) texel.
pub const AREA_WEIGHT_UNITS: f64 = 1024.0;
/// Diffusion passes spreading the ocean mask; each pass steps
/// `ocean_blur_m / sqrt(OCEAN_BLUR_PASSES)` so the diffusion length is the
/// authored distance at any resolution.
pub const OCEAN_BLUR_PASSES: u32 = 16;
/// Relief smoothing passes (the ocean-blur kernel on `max(h, 0)`), stepping
/// `moisture_step / sqrt(RELIEF_SMOOTH_PASSES)`: about one moisture step.
pub const RELIEF_SMOOTH_PASSES: u32 = 4;
/// Vector blur passes of the deflected wind, same step as relief smoothing.
pub const WIND_BLUR_PASSES: u32 = 4;
/// `boundary_coord` fixed point: units per metre (i32, 1/16 m).
pub const BOUNDARY_UNITS_PER_M: f64 = 16.0;

/// Inputs of one body's bake.
#[derive(Debug, Clone, PartialEq)]
pub struct TierAInputs {
    pub params: PlanetParams,
    pub stages: Vec<TierAStage>,
    pub radius_m: f64,
    /// Body-fixed rotation axis (latitude reference).
    pub pole: DVec3,
    pub face_cells: usize,
    /// Landform weight-rule set bytecode (`landform::expr::encode_set`);
    /// empty: no landforms, the weight run stays zero.
    pub landform_rules: Vec<u32>,
}

impl TierAInputs {
    pub fn has(&self, stage: TierAStage) -> bool {
        self.stages.contains(&stage)
    }
}

/// Baked fields. Elevation is metres relative to sea level, temperature °C,
/// moisture 0..1, wind east/north components in the local tangent frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TierAFields {
    pub elevation: CubeMap<f32>,
    pub temperature: CubeMap<f32>,
    pub moisture: CubeMap<f32>,
    pub wind_east: CubeMap<f32>,
    pub wind_north: CubeMap<f32>,
    /// Normalised noise values at sea level and at the 0.1 % and 99.9 %
    /// area percentiles (the deepest ocean and highest land references).
    pub sea_level: f64,
    pub noise_low: f64,
    pub noise_high: f64,
    /// Area-weighted fraction of texels below sea level.
    pub ocean_fraction: f64,
}

/// M2 (Shape) result fields exactly as the GPU stores them (result runs 5–8).
#[derive(Debug, Clone, PartialEq)]
pub struct TierAShape {
    /// Signed distance to the plate boundary (linear through it), i32 fixed
    /// point in 1/16 m ([`BOUNDARY_UNITS_PER_M`]), clamped to ±clamp.
    pub boundary_coord: CubeMap<i32>,
    /// `pack4x8unorm(uplift, hardness, sediment, flow)` ([`pack_unorm4`]).
    pub aux0: CubeMap<u32>,
    /// Bytes: plate index, boundary class ([`tectonics::BoundaryClass`]),
    /// volcanic mask (unorm), zero.
    pub aux1: CubeMap<u32>,
    /// Four landform weights (unorm8). Reserved: the landform-weight rules
    /// arrive with the landform bytecode, until then every weight is zero.
    pub landform: CubeMap<u32>,
}

/// Intermediate fields for tests and diagnostics (not part of the result).
#[derive(Debug, Clone, PartialEq)]
pub struct TierADiagnostics {
    /// Elevation after shape + tectonic dh, before erosion (sea level of the
    /// first histogram at 0, no shelf).
    pub pre_erosion: CubeMap<f32>,
    /// Elevation after erosion, before the re-zero.
    pub eroded: CubeMap<f32>,
    pub uplift: CubeMap<f32>,
    pub hardness: CubeMap<f32>,
    /// Distance to the nearest plate boundary (m, unclamped).
    pub boundary_distance: CubeMap<f32>,
    /// Smoothed relief slope east/north (m/m), zero without a rain shadow.
    pub slope_east: CubeMap<f32>,
    pub slope_north: CubeMap<f32>,
    /// Precipitation-weighted drainage area (km²) of the final erosion level.
    pub discharge: CubeMap<f32>,
    /// Accumulated deposit thickness (m).
    pub deposit: CubeMap<f32>,
    /// Second sea level (m) subtracted by the re-zero.
    pub sea_level_m: f64,
    pub erosion: Option<erosion::ErosionStats>,
}

/// A full bake: the result fields, the M2 fields and diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct TierAOutput {
    pub fields: TierAFields,
    pub shape: TierAShape,
    pub diagnostics: TierADiagnostics,
}

/// Area weight of a texel at chart position `(u, v)`: its solid angle relative
/// to a face-centre texel, quantised so CPU and GPU histograms agree.
pub fn area_weight(u: f64, v: f64) -> u32 {
    (AREA_WEIGHT_UNITS / (1.0 + u * u + v * v).powf(1.5)).round() as u32
}

/// Chart coordinates of texel (i, j) on an `n`-cell face.
pub fn texel_uv(i: usize, j: usize, n: usize) -> (f64, f64) {
    (
        -1.0 + (2.0 * i as f64 + 1.0) / n as f64,
        -1.0 + (2.0 * j as f64 + 1.0) / n as f64,
    )
}

/// Normalised fBm in about `[-0.87, 0.87]`: octave `k` has frequency
/// `frequency * lacunarity^k`, amplitude `gain^k` and seed `seed + k`.
pub fn fbm(p: DVec3, frequency: f64, octaves: u32, lacunarity: f64, gain: f64, seed: u32) -> f64 {
    let (mut sum, mut norm, mut amplitude, mut f) = (0.0, 0.0, 1.0, frequency);
    for k in 0..octaves {
        sum += amplitude * gradient_noise(p * f, seed.wrapping_add(k)).0;
        norm += amplitude;
        amplitude *= gain;
        f *= lacunarity;
    }
    sum / norm
}

/// Domain warp of unit direction `d` by three 3-octave fBm fields at
/// `frequency` (cycles per unit direction) with seeds `seed`, `seed + 16`,
/// `seed + 32`, displaced by `scale` radians per unit noise.
pub fn warp_direction(d: DVec3, frequency: f64, scale: f64, seed: u32) -> DVec3 {
    let warp = DVec3::new(
        fbm(d, frequency, 3, 2.0, 0.5, seed),
        fbm(d, frequency, 3, 2.0, 0.5, seed.wrapping_add(16)),
        fbm(d, frequency, 3, 2.0, 0.5, seed.wrapping_add(32)),
    );
    (d + warp * scale).normalize()
}

/// Normalised continent noise at unit direction `d` (before sea level).
pub fn continent_noise(d: DVec3, params: &PlanetParams, radius_m: f64) -> f64 {
    // Displacement in radians, proportional to the continent wavelength.
    let warped = warp_direction(
        d,
        radius_m / params.warp_wavelength_m,
        params.warp_strength * params.continent_wavelength_m / radius_m,
        params.warp_seed,
    );
    fbm(
        warped,
        radius_m / params.continent_wavelength_m,
        params.continent_octaves,
        params.continent_lacunarity,
        params.continent_gain,
        params.continent_seed,
    )
}

/// Sea level: the normalised noise value below which `coverage` of the
/// area-weighted histogram lies (linear within the crossing bin).
pub fn sea_level_from_histogram(histogram: &[u64], coverage: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    let target = coverage * total as f64;
    let mut below = 0.0;
    for (bin, &count) in histogram.iter().enumerate() {
        let next = below + count as f64;
        if next >= target && count > 0 {
            let fraction = (target - below) / count as f64;
            return -1.0 + 2.0 * (bin as f64 + fraction) / histogram.len() as f64;
        }
        below = next;
    }
    1.0
}

pub fn histogram_bin(r: f64) -> usize {
    (((r + 1.0) * 0.5 * HISTOGRAM_BINS as f64).floor().max(0.0) as usize).min(HISTOGRAM_BINS - 1)
}

/// Area percentiles that set the elevation scale: land reaches its full height
/// at the 99.9 % noise value and the ocean its full depth at the 0.1 % value.
pub const RELIEF_PERCENTILES: [f64; 2] = [0.001, 0.999];

/// Elevation (m, sea level 0) from normalised noise `r`, sea level `s` and the
/// relief percentiles `low`, `high`.
pub fn shape_elevation(
    r: f64,
    s: f64,
    low: f64,
    high: f64,
    params: &PlanetParams,
    shelf: bool,
) -> f64 {
    if r >= s {
        let t = ((r - s) / (high - s).max(1e-9)).clamp(0.0, 1.0);
        return params.land_height_m * t.powf(params.land_exponent);
    }
    let t = ((s - r) / (s - low).max(1e-9)).clamp(0.0, 1.0);
    let depth = if shelf {
        shelf_depth(t, params)
    } else {
        params.ocean_depth_m * deep(t)
    };
    -depth
}

/// Ocean depth profile without a shelf: `1 − (1 − t)³` of the full depth.
fn deep(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Depth with the continental shelf at normalised distance `t` from the coast
/// (M1 shelf: linear to the shelf edge, then the deep profile).
fn shelf_depth(t: f64, params: &PlanetParams) -> f64 {
    if params.shelf_fraction <= 0.0 {
        return params.ocean_depth_m * deep(t);
    }
    if t < params.shelf_fraction {
        params.shelf_depth_m * t / params.shelf_fraction
    } else {
        let u = (t - params.shelf_fraction) / (1.0 - params.shelf_fraction);
        params.shelf_depth_m + (params.ocean_depth_m - params.shelf_depth_m).max(0.0) * deep(u)
    }
}

/// The shelf as a monotone remap of an unshelved depth (m, ≥ 0): inverts the
/// deep profile to `t`, applies the M1 shelf, and continues as the identity
/// below the full ocean depth (trenches). Exactly the M1 shelf for M1 inputs.
pub fn shelf_remap(depth_m: f64, params: &PlanetParams) -> f64 {
    let od = params.ocean_depth_m;
    if depth_m <= 0.0 || depth_m >= od {
        return depth_m;
    }
    let t = 1.0 - (1.0 - depth_m / od).cbrt();
    shelf_depth(t, params)
}

/// Largest absolute macro elevation of a bake with `stages` (M2 design §4,
/// without landform recipe bounds): land plus collision belt, arc and erosion
/// uplift, or ocean plus trench and rift. The re-zero clamps to it.
pub fn height_bound_m(params: &PlanetParams, stages: &[TierAStage]) -> f64 {
    let tectonics = stages.contains(&TierAStage::Tectonics);
    let erosion = stages.contains(&TierAStage::Erosion);
    let (rise, sink) = if tectonics {
        (
            (params.collision_height_m.max(params.transform_height_m) + params.arc_height_m)
                * (1.0 + params.orogen_roughness)
                + params.ridge_height_m,
            params.trench_depth_m + params.rift_depth_m,
        )
    } else {
        (0.0, 0.0)
    };
    let uplift = if erosion {
        params.erosion_uplift_m
    } else {
        0.0
    };
    (params.land_height_m + rise + uplift).max(params.ocean_depth_m + sink)
}

/// Bin of elevation `h` (m) in the second histogram over `±bound`.
pub fn elevation_bin(h: f64, bound: f64) -> usize {
    (((h / bound + 1.0) * 0.5 * HISTOGRAM_BINS as f64)
        .floor()
        .max(0.0) as usize)
        .min(HISTOGRAM_BINS - 1)
}

/// `pack4x8unorm` (WGSL): byte `k` is `round(clamp(v[k], 0, 1) · 255)`.
pub fn pack_unorm4(v: [f64; 4]) -> u32 {
    v.iter().enumerate().fold(0, |word, (k, x)| {
        word | (((x.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32) << (8 * k))
    })
}

/// `unpack4x8unorm` (WGSL).
pub fn unpack_unorm4(word: u32) -> [f64; 4] {
    std::array::from_fn(|k| f64::from((word >> (8 * k)) & 0xff) / 255.0)
}

/// `boundary_coord` metres to its i32 fixed point (1/16 m), rounding halves
/// up as the GPU does (`floor(x + 0.5)`; WGSL `round` ties to even).
pub fn boundary_fixed(m: f64) -> i32 {
    (m * BOUNDARY_UNITS_PER_M + 0.5).floor() as i32
}

/// Fill `out[k] = f(k)` on all available cores. Every value depends only on
/// its index, so the result is the same as a serial loop.
pub(crate) fn par_fill<T: Send>(out: &mut [T], f: impl Fn(usize) -> T + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(32);
    if threads <= 1 || out.len() < 4096 {
        for (k, v) in out.iter_mut().enumerate() {
            *v = f(k);
        }
        return;
    }
    let chunk = out.len().div_ceil(threads);
    std::thread::scope(|scope| {
        for (c, part) in out.chunks_mut(chunk).enumerate() {
            let f = &f;
            scope.spawn(move || {
                for (k, v) in part.iter_mut().enumerate() {
                    *v = f(c * chunk + k);
                }
            });
        }
    });
}

/// `par_fill` into a new vector.
pub(crate) fn par_map<T: Send + Clone + Default>(
    len: usize,
    f: impl Fn(usize) -> T + Sync,
) -> Vec<T> {
    let mut out = vec![T::default(); len];
    par_fill(&mut out, f);
    out
}

fn cube_from(n: usize, values: impl IntoIterator<Item = f64>) -> CubeMap<f32> {
    let mut map = CubeMap::new(n, 0.0f32);
    for (slot, v) in map.data_mut().iter_mut().zip(values) {
        *slot = v as f32;
    }
    map
}

/// Bake every stage the inputs request.
pub fn bake(inputs: &TierAInputs) -> Result<TierAFields, TerrainError> {
    bake_full(inputs).map(|output| output.fields)
}

/// Bake every stage the inputs request, with the M2 fields and diagnostics.
pub fn bake_full(inputs: &TierAInputs) -> Result<TierAOutput, TerrainError> {
    let n = inputs.face_cells;
    let p = &inputs.params;
    if !(4..=4096).contains(&n)
        || !inputs.radius_m.is_finite()
        || inputs.radius_m <= 0.0
        || !inputs.pole.is_finite()
        || (inputs.pole.length() - 1.0).abs() > 1e-6
    {
        return Err(TerrainError::InvalidConfig);
    }
    let has = |stage| inputs.has(stage);
    let texels = 6 * n * n;
    let directions: Vec<DVec3> = (0..6)
        .flat_map(|face| (0..n).flat_map(move |j| (0..n).map(move |i| (face, i, j))))
        .map(|(face, i, j)| texel_direction(face, i, j, n))
        .collect();
    let weights: Vec<u32> = (0..texels)
        .map(|k| {
            let (i, j) = (k % n, (k / n) % n);
            let (u, v) = texel_uv(i, j, n);
            area_weight(u, v)
        })
        .collect();
    let frames: Vec<(DVec3, DVec3)> = directions
        .iter()
        .map(|d| tangent_frame(*d, inputs.pole))
        .collect();

    // Tectonics: plates, boundary distance and profiles, crust, hardness.
    let samples: Vec<tectonics::TectonicSample> = if has(TierAStage::Tectonics) {
        let set = tectonics::plates(p, p.tectonic_seed);
        par_map(texels, |k| {
            tectonics::tectonic_sample(directions[k], &set, p, inputs.radius_m)
        })
    } else {
        vec![tectonics::TectonicSample::none(); texels]
    };

    // Continents (blended with crust type) and the first sea level.
    let crust_weight = if has(TierAStage::Tectonics) {
        p.crust_weight
    } else {
        0.0
    };
    let raw: Vec<f64> = par_map(texels, |k| {
        let noise = if has(TierAStage::Continents) {
            continent_noise(directions[k], p, inputs.radius_m)
        } else {
            0.0
        };
        (1.0 - crust_weight) * noise + crust_weight * 0.5 * samples[k].crust
    });
    let mut histogram = vec![0u64; HISTOGRAM_BINS];
    for (r, w) in raw.iter().zip(&weights) {
        histogram[histogram_bin(*r)] += u64::from(*w);
    }
    let sea_level = if has(TierAStage::SeaLevel) {
        sea_level_from_histogram(&histogram, p.ocean_coverage)
    } else {
        -1.0
    };
    let noise_low = sea_level_from_histogram(&histogram, RELIEF_PERCENTILES[0]);
    let noise_high = sea_level_from_histogram(&histogram, RELIEF_PERCENTILES[1]);
    // Shape: unshelved elevation plus the tectonic relief.
    let pre_erosion = cube_from(
        n,
        raw.iter()
            .zip(&samples)
            .map(|(r, s)| shape_elevation(*r, sea_level, noise_low, noise_high, p, false) + s.dh_m),
    );

    // Relief smoothing and slope (rain shadow), ocean blur, climate, wind
    // deflection and moisture.
    let geometry = climate::Geometry {
        n,
        radius_m: inputs.radius_m,
        pole: inputs.pole,
        directions: &directions,
        frames: &frames,
    };
    let rain_shadow = has(TierAStage::RainShadow);
    let (slope_east, slope_north) = if rain_shadow {
        climate::relief_slope(&geometry, &pre_erosion, p)
    } else {
        (CubeMap::new(n, 0.0f32), CubeMap::new(n, 0.0f32))
    };
    let ocean = climate::ocean_mask(&geometry, &pre_erosion, p);
    let mut temperature = climate::temperature(&geometry, &pre_erosion, &ocean, inputs);
    let (mut wind_east, mut wind_north) = climate::wind_bands(&geometry, inputs);
    if rain_shadow && has(TierAStage::Wind) {
        (wind_east, wind_north) = climate::deflect_wind(
            &geometry,
            &wind_east,
            &wind_north,
            &slope_east,
            &slope_north,
            p,
        );
    }
    let moisture = climate::moisture(
        &geometry,
        inputs,
        &climate::MoistureInputs {
            elevation: &pre_erosion,
            temperature: &temperature,
            wind_east: &wind_east,
            wind_north: &wind_north,
            slope: rain_shadow.then_some((&slope_east, &slope_north)),
        },
    );

    // Macro erosion on the pre-erosion relief.
    let uplift = cube_from(n, samples.iter().map(|s| s.uplift));
    let hardness = cube_from(n, samples.iter().map(|s| s.hardness));
    let eroded = if has(TierAStage::Erosion) {
        Some(erosion::erode(&erosion::ErosionInput {
            face_cells: n,
            radius_m: inputs.radius_m,
            params: p,
            elevation: &pre_erosion,
            uplift: &uplift,
            hardness: &hardness,
            precipitation: &moisture,
        }))
    } else {
        None
    };
    let (eroded_elevation, discharge, deposit) = match &eroded {
        Some(e) => (e.elevation.clone(), e.discharge.clone(), e.deposit.clone()),
        None => (
            pre_erosion.clone(),
            CubeMap::new(n, 0.0f32),
            CubeMap::new(n, 0.0f32),
        ),
    };

    // Second histogram in metres, re-zero and shelf.
    let bound = height_bound_m(p, &inputs.stages);
    let sea_level_m = if has(TierAStage::SeaLevel) {
        let mut histogram = vec![0u64; HISTOGRAM_BINS];
        for (h, w) in eroded_elevation.data().iter().zip(&weights) {
            histogram[elevation_bin(f64::from(*h), bound)] += u64::from(*w);
        }
        sea_level_from_histogram(&histogram, p.ocean_coverage) * bound
    } else {
        0.0
    };
    let shelf = has(TierAStage::Shelf);
    let elevation = cube_from(
        n,
        eroded_elevation
            .data()
            .iter()
            .map(|h| rezero(f64::from(*h), sea_level_m, bound, shelf, p)),
    );
    if has(TierAStage::Temperature) {
        temperature = climate::temperature(&geometry, &elevation, &ocean, inputs);
    }
    let total_weight: u64 = weights.iter().map(|w| u64::from(*w)).sum();
    let ocean_weight: u64 = elevation
        .data()
        .iter()
        .zip(&weights)
        .filter(|(h, _)| **h < 0.0)
        .map(|(_, w)| u64::from(*w))
        .sum();

    // Pack the M2 result runs.
    let flow_norm = erosion::flow_normaliser(inputs.radius_m);
    let mut shape = TierAShape {
        boundary_coord: CubeMap::new(n, 0i32),
        aux0: CubeMap::new(n, 0u32),
        aux1: CubeMap::new(n, 0u32),
        landform: CubeMap::new(n, 0u32),
    };
    for (k, s) in samples.iter().enumerate() {
        let sediment = 1.0 - (-f64::from(deposit.data()[k]) / p.sediment_depth_m).exp();
        let flow = (1.0 + f64::from(discharge.data()[k])).ln() * flow_norm;
        shape.boundary_coord.data_mut()[k] = boundary_fixed(s.boundary_coord_m);
        shape.aux0.data_mut()[k] = pack_unorm4([s.uplift, s.hardness, sediment, flow]);
        shape.aux1.data_mut()[k] = u32::from(s.plate)
            | (u32::from(s.class as u8) << 8)
            | (pack_unorm4([s.volcanic, 0.0, 0.0, 0.0]) << 16);
    }
    // Landform weights (run 8): the rule bytecode over the stored result
    // fields of each texel (packed bytes as stored, so the GPU pass reads
    // the same inputs); four unorm8 weights.
    if !inputs.landform_rules.is_empty() {
        for k in 0..texels {
            let fields = landform_rule_fields(
                f64::from(elevation.data()[k]),
                f64::from(temperature.data()[k]),
                f64::from(moisture.data()[k]),
                shape.boundary_coord.data()[k],
                shape.aux0.data()[k],
                shape.aux1.data()[k],
                f64::from(slope_east.data()[k]).hypot(f64::from(slope_north.data()[k])),
            );
            let weights = crate::terrain::landform::expr::evaluate_set(
                &inputs.landform_rules,
                &fields,
            )
            .unwrap_or_default();
            let lane = |i: usize| weights.get(i).copied().unwrap_or(0.0);
            shape.landform.data_mut()[k] = pack_unorm4([lane(0), lane(1), lane(2), lane(3)]);
        }
    }
    let boundary_distance = cube_from(n, samples.iter().map(|s| s.distance_m));

    Ok(TierAOutput {
        fields: TierAFields {
            elevation,
            temperature,
            moisture,
            wind_east,
            wind_north,
            sea_level,
            noise_low,
            noise_high,
            ocean_fraction: ocean_weight as f64 / total_weight as f64,
        },
        shape,
        diagnostics: TierADiagnostics {
            pre_erosion,
            eroded: eroded_elevation,
            uplift,
            hardness,
            boundary_distance,
            slope_east,
            slope_north,
            discharge,
            deposit,
            sea_level_m,
            erosion: eroded.map(|e| e.stats),
        },
    })
}

/// Landform weight-rule inputs (`landform::expr::field`) of one Tier A texel
/// from its stored results: elevation (m), temperature (°C), moisture,
/// `boundary_coord` (i32, 1/16 m), packed `aux0` and `aux1`, and the
/// smoothed macro relief slope (rise over run). The GPU `landform_weights`
/// pass mirrors it in f32.
pub fn landform_rule_fields(
    elevation: f64,
    temperature: f64,
    moisture: f64,
    boundary_coord: i32,
    aux0: u32,
    aux1: u32,
    slope_macro: f64,
) -> crate::terrain::landform::expr::RuleFields {
    use crate::terrain::landform::expr::field;
    let [uplift, hardness, sediment, _] = unpack_unorm4(aux0);
    let volcanic = unpack_unorm4(aux1)[2];
    let m = moisture.clamp(0.0, 1.0);
    let x = ((temperature + 15.0) / 20.0).clamp(0.0, 1.0);
    let mut f = [0.0; field::COUNT];
    f[field::UPLIFT as usize] = uplift;
    f[field::SEDIMENT as usize] = sediment;
    f[field::MOISTURE as usize] = m;
    f[field::ARID as usize] = 1.0 - m;
    f[field::TEMPERATURE as usize] = temperature;
    f[field::COLD as usize] = 1.0 - x * x * (3.0 - 2.0 * x);
    f[field::HARDNESS as usize] = hardness;
    f[field::SLOPE_MACRO as usize] = slope_macro;
    f[field::ELEVATION as usize] = elevation;
    f[field::BOUNDARY_DISTANCE as usize] = (f64::from(boundary_coord) / BOUNDARY_UNITS_PER_M).abs();
    f[field::VOLCANIC as usize] = volcanic;
    f[field::OCEAN as usize] = if elevation < 0.0 { 1.0 } else { 0.0 };
    f
}

/// Re-zero eroded elevation `h` at the second sea level `s2` (m), apply the
/// shelf as a monotone depth remap and clamp to `±bound`.
pub fn rezero(h: f64, s2: f64, bound: f64, shelf: bool, params: &PlanetParams) -> f64 {
    let z = h - s2;
    let z = if z < 0.0 && shelf {
        -shelf_remap(-z, params)
    } else {
        z
    };
    z.clamp(-bound, bound)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain::archetype::tests::terra;

    pub(crate) fn inputs(n: usize, seed: u64) -> TierAInputs {
        let archetype = terra();
        TierAInputs {
            params: archetype.sample(seed),
            stages: archetype.stages.clone(),
            radius_m: 338_950.0,
            pole: DVec3::Y,
            face_cells: n,
            landform_rules: Vec::new(),
        }
    }

    pub(crate) fn area_mean(fields: &CubeMap<f32>, n: usize, keep: impl Fn(usize) -> bool) -> f64 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for (k, v) in fields.data().iter().enumerate() {
            if keep(k) {
                let (u, w) = texel_uv(k % n, (k / n) % n, n);
                let a = f64::from(area_weight(u, w));
                sum += f64::from(*v) * a;
                weight += a;
            }
        }
        sum / weight
    }

    #[test]
    fn ocean_coverage_is_exact_and_bake_is_deterministic() {
        let input = inputs(64, 7);
        let output = bake_full(&input).unwrap();
        let fields = &output.fields;
        // §19.4: ocean coverage within ±1 % of the target (the second
        // histogram makes it exact after erosion).
        assert!(
            (fields.ocean_fraction - input.params.ocean_coverage).abs() < 0.01,
            "{} vs {}",
            fields.ocean_fraction,
            input.params.ocean_coverage
        );
        assert_eq!(output, bake_full(&input).unwrap());
        let other = bake(&inputs(64, 8)).unwrap();
        assert_ne!(fields.elevation, other.elevation);
        let (low, high) = fields
            .elevation
            .data()
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
        assert!(low < -1000.0 && high > 500.0, "{low} {high}");
        let bound = height_bound_m(&input.params, &input.stages);
        assert!(f64::from(high) <= bound && f64::from(low) >= -bound);
        // Tectonic relief reaches beyond the M1 land height.
        assert!(f64::from(high) > input.params.land_height_m, "{high}");
    }

    #[test]
    fn m1_stages_without_m2_keep_the_m1_shelf() {
        // Without tectonics and erosion the re-zero shift is a fraction of a
        // metre and the depth remap reproduces the M1 shelf.
        let mut input = inputs(32, 7);
        input.stages.retain(|s| {
            !matches!(
                s,
                TierAStage::Tectonics | TierAStage::RainShadow | TierAStage::Erosion
            )
        });
        let output = bake_full(&input).unwrap();
        let p = &input.params;
        assert!(output.diagnostics.sea_level_m.abs() < 10.0);
        let f = &output.fields;
        for t in [0.01, 0.05, 0.2, 0.5, 0.9] {
            let unshelved = p.ocean_depth_m * deep(t);
            assert!((shelf_remap(unshelved, p) - shelf_depth(t, p)).abs() < 1e-6);
        }
        assert!(
            (f.ocean_fraction - p.ocean_coverage).abs() < 0.01,
            "{}",
            f.ocean_fraction
        );
        assert!(output.shape.boundary_coord.data().iter().all(|v| *v == 0));
    }

    #[test]
    fn temperature_falls_with_latitude_and_altitude() {
        let n = 64;
        let input = inputs(n, 7);
        let fields = bake(&input).unwrap();
        let directions: Vec<DVec3> = (0..6 * n * n)
            .map(|k| texel_direction(k / (n * n), k % n, (k / n) % n, n))
            .collect();
        let band = |lo: f64, hi: f64| {
            area_mean(&fields.temperature, n, |k| {
                let x = directions[k].dot(DVec3::Y).abs();
                (lo..hi).contains(&x)
            })
        };
        let (tropics, mid, polar) = (band(0.0, 0.3), band(0.4, 0.7), band(0.85, 1.0));
        assert!(tropics > mid && mid > polar, "{tropics} {mid} {polar}");
        assert!(tropics - polar > 25.0);
        // Altitude, on the baked field: without the lapse rate every land
        // texel is warmer by lapse · height, scaled by the ocean moderation
        // factor 1 - moderation · mask (mask in 0..1); oceans are unchanged.
        let mut flat = input.clone();
        flat.params.lapse_c_per_km = 0.0;
        let without = bake(&flat).unwrap();
        let lapse = input.params.lapse_c_per_km;
        let mut high_land = 0;
        for k in 0..6 * n * n {
            let h = f64::from(fields.elevation.data()[k]);
            let cooling =
                f64::from(without.temperature.data()[k]) - f64::from(fields.temperature.data()[k]);
            let full = lapse * h.max(0.0) / 1000.0;
            let least = full * (1.0 - input.params.ocean_moderation);
            assert!(
                cooling <= full + 1e-4 && cooling >= least - 1e-4,
                "texel {k}: cooled {cooling}, expected within [{least}, {full}]"
            );
            if h > 1000.0 {
                high_land += 1;
            }
        }
        assert!(high_land > 0, "the bake has land above 1 km");
    }

    #[test]
    fn circulation_cells_give_a_wet_equator_and_dry_subtropics() {
        let n = 64;
        let base = inputs(n, 7);
        let directions: Vec<DVec3> = (0..6 * n * n)
            .map(|k| texel_direction(k / (n * n), k % n, (k / n) % n, n))
            .collect();
        // Mean moisture over all texels in a latitude band (degrees).
        let band = |fields: &TierAFields, lo: f64, hi: f64| {
            area_mean(&fields.moisture, n, |k| {
                let latitude = directions[k].dot(base.pole).abs().asin().to_degrees();
                (lo..hi).contains(&latitude)
            })
        };
        let with = bake_full(&base).unwrap();
        let mut flat = base.clone();
        flat.params.rain_convergence = 0.0;
        let without = bake_full(&flat).unwrap();
        let gap = |f: &TierAFields| band(f, 0.0, 10.0) - band(f, 25.0, 35.0);
        assert!(
            gap(&with.fields) > 0.0,
            "equator wetter than the subtropics"
        );
        assert!(
            gap(&with.fields) > gap(&without.fields) + 0.05,
            "convergence widens the gap: {} vs {}",
            gap(&with.fields),
            gap(&without.fields)
        );
        // Moisture only: the relief before erosion (which uses the rain) is
        // unchanged.
        assert_eq!(
            with.diagnostics.pre_erosion, without.diagnostics.pre_erosion,
            "moisture only"
        );
    }

    #[test]
    fn coasts_are_wetter_than_continental_interiors() {
        let n = 64;
        let fields = bake(&inputs(n, 7)).unwrap();
        let land = |k: usize| fields.elevation.data()[k] >= 0.0;
        let ocean = |k: usize| fields.elevation.data()[k] < 0.0;
        let neighbours = |k: usize| crate::terrain::world_map::neighbours(k, n);
        let coastal: Vec<usize> = (0..6 * n * n)
            .filter(|&k| land(k) && neighbours(k).iter().any(|&m| ocean(m)))
            .collect();
        // Interior: land whose 2-ring has no ocean.
        let interior: Vec<usize> = (0..6 * n * n)
            .filter(|&k| {
                land(k)
                    && neighbours(k)
                        .iter()
                        .all(|&m| land(m) && neighbours(m).iter().all(|&q| land(q)))
            })
            .collect();
        assert!(!coastal.is_empty() && !interior.is_empty());
        let mean = |set: &[usize]| {
            set.iter()
                .map(|&k| f64::from(fields.moisture.data()[k]))
                .sum::<f64>()
                / set.len() as f64
        };
        assert!(
            mean(&coastal) > mean(&interior) + 0.05,
            "{} {}",
            mean(&coastal),
            mean(&interior)
        );
        assert!(
            fields
                .moisture
                .data()
                .iter()
                .all(|m| (0.0..=1.0).contains(m))
        );
    }

    #[test]
    fn stage_seeds_are_independent() {
        let base = inputs(32, 7);
        let a = bake_full(&base).unwrap();
        // Moisture changes leave the relief before erosion and tectonics.
        let mut wetter = base.clone();
        wetter.params.set("rain", base.params.rain * 0.5).unwrap();
        let b = bake_full(&wetter).unwrap();
        assert_eq!(a.diagnostics.pre_erosion, b.diagnostics.pre_erosion);
        assert_eq!(a.shape.boundary_coord, b.shape.boundary_coord);
        assert_eq!(a.shape.aux1, b.shape.aux1);
        assert_ne!(a.fields.moisture, b.fields.moisture);
        // Erosion changes leave plates, boundary distance, uplift, moisture.
        let mut eroded = base.clone();
        eroded
            .params
            .set("erosion_strength", base.params.erosion_strength * 0.5)
            .unwrap();
        let c = bake_full(&eroded).unwrap();
        assert_eq!(a.shape.boundary_coord, c.shape.boundary_coord);
        assert_eq!(a.shape.aux1, c.shape.aux1);
        assert_eq!(a.diagnostics.uplift, c.diagnostics.uplift);
        assert_eq!(a.fields.moisture, c.fields.moisture);
        assert_ne!(a.fields.elevation, c.fields.elevation);
    }

    #[test]
    fn packing_matches_wgsl_unorm() {
        assert_eq!(pack_unorm4([0.0, 1.0, 0.5, 2.0]), 0xff80_ff00);
        assert_eq!(unpack_unorm4(0xff80_ff00), [0.0, 1.0, 128.0 / 255.0, 1.0]);
        assert_eq!(boundary_fixed(-1.03125), -16);
    }
}
