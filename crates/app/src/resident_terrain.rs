//! Deterministic CPU derivation of resident radial terrain tiles.
//!
//! The complete `SurfaceGenerator` remains the authority. This module only
//! samples and filters that field into a finite renderer representation.

use std::time::{Duration, Instant};

use glam::DVec3;
use mundaris_math::surface::SurfaceLocation;
use mundaris_math::{Direction3, surface::CubePatchAddress};
use mundaris_world::terrain::{SurfaceAtmosphere, SurfaceGenerator, TerrainError};

pub use mundaris_renderer::resident_tile::{TileData, TileKey, TileTexel};

/// Prototype tile density. The payload stores one texel of halo on every side.
pub const DEFAULT_TILE_CELLS: u32 = 64;
/// Version of the CPU filtering policy recorded in every tile key.
pub const TILE_FILTER_VERSION: u32 = mundaris_renderer::resident_tile::TILE_FILTER_VERSION;
/// Version of the radial displacement/material texel layout.
pub const TILE_FORMAT_VERSION: u32 = mundaris_renderer::resident_tile::TILE_FORMAT_VERSION;

/// Inputs that change authoritative or derived tile content.
#[derive(Debug, Clone, Copy)]
pub struct TileBuildIdentity {
    pub body_identity: u64,
    pub surface_revision: u64,
    pub material_revision: u64,
}

/// Build cost and CPU payload accounting, separate from GPU residency metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileBuildDiagnostics {
    pub elapsed: Duration,
    pub authoritative_query_count: u64,
    pub texel_dimensions: [u32; 2],
    pub patch_grid_vertex_dimensions: [u32; 2],
    pub payload_bytes: usize,
    /// Estimated direct builder stack state; generator stack frames are excluded.
    pub builder_stack_scratch_bytes_estimate: usize,
    /// Heap scratch requested by the authority's scalar query path.
    pub surface_query_heap_scratch_bytes: usize,
    pub surface_generator_retained_heap_bytes: usize,
    pub surface_generator_heap_bound_bytes: usize,
    /// Nominal cube-chart spacing at the anchor, before local chart compression.
    pub nominal_grid_spacing_radians: f64,
    pub nominal_grid_spacing_m: f64,
    /// Reconstructed physical spacing between adjacent grid points at patch center.
    pub actual_chart_center_grid_spacing_m: [f64; 2],
    /// Nominal stencil step and its physical scale at the anchor radius.
    pub filter_step_radians: f64,
    pub filter_nominal_width_m: f64,
    /// Complete-surface distances for the six projected global-axis taps.
    pub filter_actual_surface_offsets_m: [f64; 6],
}

/// Difference between complete authority and the sampled derived tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileApproximationMetrics {
    pub texel_query_count: u64,
    pub triangle_centroid_query_count: u64,
    pub max_texel_radial_error_m: f64,
    pub rms_texel_radial_error_m: f64,
    pub max_normal_angular_error_radians: f64,
    pub rms_normal_angular_error_radians: f64,
    /// Euclidean error from each rendered triangle centroid to the complete
    /// radial surface queried along that centroid's body-origin direction.
    pub max_triangle_centroid_error_m: f64,
    pub rms_triangle_centroid_error_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileBuildError {
    InvalidCells,
    InvalidAddress,
    InvalidSample,
    Surface(TerrainError),
}

impl std::fmt::Display for TileBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCells => f.write_str("tile cells must be a power of two in 1..=128"),
            Self::InvalidAddress => f.write_str("tile address cannot be sampled"),
            Self::InvalidSample => f.write_str("tile sample is not finite or representable"),
            Self::Surface(error) => write!(f, "authoritative surface query failed: {error}"),
        }
    }
}

impl std::error::Error for TileBuildError {}

impl From<TerrainError> for TileBuildError {
    fn from(error: TerrainError) -> Self {
        Self::Surface(error)
    }
}

/// Generic builder for all current compositional surface families.
#[derive(Debug, Default, Clone, Copy)]
pub struct ResidentTileBuilder;

impl ResidentTileBuilder {
    /// Construct the complete cache identity without evaluating the surface.
    /// This lets callers decide whether a resident payload is stale first.
    pub fn tile_key(
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
        address: CubePatchAddress,
        cells: u32,
    ) -> Result<TileKey, TileBuildError> {
        if !cells.is_power_of_two() || !(1..=TileData::MAX_CELLS).contains(&cells) {
            return Err(TileBuildError::InvalidCells);
        }
        Ok(TileKey {
            body_identity: identity.body_identity,
            definition_words: exact_definition_words(generator),
            radius_bits: generator.radius_m().to_bits(),
            surface_revision: identity.surface_revision,
            material_revision: identity.material_revision,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address,
            cells,
        })
    }

    /// Build one filtered tile from complete world queries. Cells is a
    /// prototype parameter; 64 is the default and 32/128 are accepted for
    /// matched representation measurements.
    pub fn build(
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
        address: CubePatchAddress,
        cells: u32,
    ) -> Result<(TileData, TileBuildDiagnostics), TileBuildError> {
        if !cells.is_power_of_two() || !(1..=TileData::MAX_CELLS).contains(&cells) {
            return Err(TileBuildError::InvalidCells);
        }
        let started = Instant::now();
        let anchor_direction =
            canonical_direction_with_cells(address, i64::from(cells), i64::from(cells), cells * 2)?;
        let anchor_sample = evaluate(generator, anchor_direction)?;
        let anchor_radius_m = anchor_sample.radius_m();
        let anchor_filter_directions = filter_directions(anchor_direction, address, cells)?;
        let anchor_position = anchor_sample.position(SurfaceLocation::new(anchor_direction));
        let mut filter_actual_surface_offsets_m = [0.0; 6];
        for (index, direction) in anchor_filter_directions.into_iter().skip(1).enumerate() {
            let sample = evaluate(generator, direction)?;
            filter_actual_surface_offsets_m[index] =
                (sample.position(SurfaceLocation::new(direction)) - anchor_position).length();
        }
        let side = cells.checked_add(3).ok_or(TileBuildError::InvalidCells)?;
        let texel_count = usize::try_from(side)
            .ok()
            .and_then(|side| side.checked_mul(side))
            .ok_or(TileBuildError::InvalidCells)?;
        let mut texels = Vec::with_capacity(texel_count);
        let mut min_offset = f64::INFINITY;
        let mut max_offset = f64::NEG_INFINITY;
        let mut query_count = 1u64;

        for y in -1i64..=cells as i64 + 1 {
            for x in -1i64..=cells as i64 + 1 {
                let direction = canonical_direction_with_cells(address, x, y, cells)?;
                let mut radial_sum = 0.0;
                let mut material_sum = [0.0f64; 4];
                for (sample_index, sample_direction) in
                    filter_directions(direction, address, cells)?
                        .into_iter()
                        .enumerate()
                {
                    let sample = evaluate(generator, sample_direction)?;
                    let weight = if sample_index == 0 { 0.25 } else { 0.125 };
                    radial_sum += sample.radius_m() * weight;
                    for (sum, value) in material_sum.iter_mut().zip(sample.material_weights()) {
                        *sum += value * weight;
                    }
                    query_count += 1;
                }
                let offset = radial_sum - anchor_radius_m;
                let material = normalize_material(material_sum)?;
                let radial_offset_m = offset as f32;
                if !offset.is_finite()
                    || !radial_offset_m.is_finite()
                    || material.iter().any(|value| !value.is_finite())
                {
                    return Err(TileBuildError::InvalidSample);
                }
                let represented_offset = f64::from(radial_offset_m);
                min_offset = min_offset.min(represented_offset);
                max_offset = max_offset.max(represented_offset);
                texels.push(TileTexel {
                    radial_offset_m,
                    material,
                });
            }
        }

        let key = Self::tile_key(generator, identity, address, cells)?;
        let payload_bytes = texel_count * std::mem::size_of::<TileTexel>();
        let tile = TileData {
            key,
            anchor_radius_m,
            min_max_radial_offset_m: [min_offset, max_offset],
            texels,
        };
        tile.validate().map_err(|_| TileBuildError::InvalidSample)?;
        let chart_step = 1.0 / f64::from(cells);
        let half_step = chart_step * 0.5;
        let center = [0.5, 0.5];
        let left = tile
            .position_local([center[0] - half_step, center[1]])
            .map_err(|_| TileBuildError::InvalidSample)?;
        let right = tile
            .position_local([center[0] + half_step, center[1]])
            .map_err(|_| TileBuildError::InvalidSample)?;
        let down = tile
            .position_local([center[0], center[1] - half_step])
            .map_err(|_| TileBuildError::InvalidSample)?;
        let up = tile
            .position_local([center[0], center[1] + half_step])
            .map_err(|_| TileBuildError::InvalidSample)?;
        let actual_chart_center_grid_spacing_m = [(right - left).length(), (up - down).length()];
        if actual_chart_center_grid_spacing_m
            .iter()
            .any(|spacing| !spacing.is_finite())
            || filter_actual_surface_offsets_m
                .iter()
                .any(|spacing| !spacing.is_finite())
        {
            return Err(TileBuildError::InvalidSample);
        }
        let nominal_grid_spacing_radians = 2.0 / ((1u64 << address.level()) as f64 * cells as f64);
        let filter_step_radians = nominal_grid_spacing_radians * 0.25;
        let builder_stack_scratch_bytes_estimate = std::mem::size_of::<[Direction3; 7]>()
            + std::mem::size_of::<[f64; 4]>()
            + std::mem::size_of::<mundaris_world::terrain::SurfaceSample>();
        let surface_query_heap_scratch_bytes = generator.query_workspace_bytes();
        let surface_generator_retained_heap_bytes = generator.resident_heap_bytes();
        let surface_generator_heap_bound_bytes = SurfaceGenerator::working_heap_bound_bytes();
        Ok((
            tile,
            TileBuildDiagnostics {
                elapsed: started.elapsed(),
                authoritative_query_count: query_count + 6,
                texel_dimensions: [side, side],
                patch_grid_vertex_dimensions: [cells + 1, cells + 1],
                payload_bytes,
                builder_stack_scratch_bytes_estimate,
                surface_query_heap_scratch_bytes,
                surface_generator_retained_heap_bytes,
                surface_generator_heap_bound_bytes,
                nominal_grid_spacing_radians,
                nominal_grid_spacing_m: nominal_grid_spacing_radians * anchor_radius_m,
                actual_chart_center_grid_spacing_m,
                filter_step_radians,
                filter_nominal_width_m: filter_step_radians * anchor_radius_m,
                filter_actual_surface_offsets_m,
            },
        ))
    }

    /// Measure the filtered tile against the complete field at its texels and
    /// the actual triangle centroids formed by the supplied tile geometry.
    pub fn measure_approximation(
        generator: &SurfaceGenerator,
        tile: &TileData,
    ) -> Result<TileApproximationMetrics, TileBuildError> {
        let cells = tile.key.cells;
        if !cells.is_power_of_two() || !(1..=TileData::MAX_CELLS).contains(&cells) {
            return Err(TileBuildError::InvalidCells);
        }
        let mut max_texel = 0.0f64;
        let mut sum_texel_sq = 0.0f64;
        let mut texel_queries = 0u64;
        let side = cells + 3;
        for y in 0..=cells {
            for x in 0..=cells {
                let grid_direction =
                    canonical_direction_with_cells(tile.key.address, x as i64, y as i64, cells)?;
                let complete = evaluate(generator, grid_direction)?.radius_m();
                let index = ((y + 1) * side + x + 1) as usize;
                let represented =
                    tile.anchor_radius_m + f64::from(tile.texels[index].radial_offset_m);
                let error = (represented - complete).abs();
                max_texel = max_texel.max(error);
                sum_texel_sq += error * error;
                texel_queries += 1;
            }
        }

        let mut max_centroid = 0.0f64;
        let mut sum_centroid_sq = 0.0f64;
        let mut centroid_queries = 0u64;
        let mut max_normal = 0.0f64;
        let mut sum_normal_sq = 0.0f64;
        let mut normal_queries = 0u64;
        for y in 0..cells {
            for x in 0..cells {
                let p00 = tile
                    .position_local([x as f64 / cells as f64, y as f64 / cells as f64])
                    .map_err(|_| TileBuildError::InvalidSample)?;
                let p10 = tile
                    .position_local([(x + 1) as f64 / cells as f64, y as f64 / cells as f64])
                    .map_err(|_| TileBuildError::InvalidSample)?;
                let p01 = tile
                    .position_local([x as f64 / cells as f64, (y + 1) as f64 / cells as f64])
                    .map_err(|_| TileBuildError::InvalidSample)?;
                let p11 = tile
                    .position_local([(x + 1) as f64 / cells as f64, (y + 1) as f64 / cells as f64])
                    .map_err(|_| TileBuildError::InvalidSample)?;
                // Match renderer create_grid topology: [a,b,c] and [b,d,c],
                // with a=p00, b=p10, c=p01, d=p11.
                for [a, b, c] in [[p00, p10, p01], [p10, p11, p01]] {
                    let represented_centroid = (a + b + c) / 3.0;
                    let body_centroid = tile
                        .anchor_position_body()
                        .map_err(|_| TileBuildError::InvalidSample)?
                        + represented_centroid;
                    let direction = Direction3::try_new(body_centroid)
                        .map_err(|_| TileBuildError::InvalidSample)?;
                    let complete =
                        evaluate(generator, direction)?.position(SurfaceLocation::new(direction));
                    let error = (complete - body_centroid).length();
                    if !error.is_finite() {
                        return Err(TileBuildError::InvalidSample);
                    }
                    max_centroid = max_centroid.max(error);
                    sum_centroid_sq += error * error;
                    centroid_queries += 1;
                }
            }
        }
        for y in 0..=cells {
            for x in 0..=cells {
                let direction =
                    canonical_direction_with_cells(tile.key.address, x as i64, y as i64, cells)?;
                let complete_normal = evaluate(generator, direction)?.normal();
                let represented_normal = tile
                    .normal_local([x, y])
                    .map_err(|_| TileBuildError::InvalidSample)?;
                let error = complete_normal
                    .dot(represented_normal)
                    .clamp(-1.0, 1.0)
                    .acos();
                if !error.is_finite() {
                    return Err(TileBuildError::InvalidSample);
                }
                max_normal = max_normal.max(error);
                sum_normal_sq += error * error;
                normal_queries += 1;
            }
        }
        let texel_denominator = texel_queries.max(1) as f64;
        let centroid_denominator = centroid_queries.max(1) as f64;
        Ok(TileApproximationMetrics {
            texel_query_count: texel_queries,
            triangle_centroid_query_count: centroid_queries,
            max_texel_radial_error_m: max_texel,
            rms_texel_radial_error_m: (sum_texel_sq / texel_denominator).sqrt(),
            max_normal_angular_error_radians: max_normal,
            rms_normal_angular_error_radians: (sum_normal_sq / normal_queries.max(1) as f64).sqrt(),
            max_triangle_centroid_error_m: max_centroid,
            rms_triangle_centroid_error_m: (sum_centroid_sq / centroid_denominator).sqrt(),
        })
    }
}

fn evaluate(
    generator: &SurfaceGenerator,
    direction: Direction3,
) -> Result<mundaris_world::terrain::SurfaceSample, TileBuildError> {
    generator
        .evaluate_point(SurfaceLocation::new(direction))
        .map_err(Into::into)
}

fn filter_directions(
    direction: Direction3,
    address: CubePatchAddress,
    cells: u32,
) -> Result<[Direction3; 7], TileBuildError> {
    let center = direction.unit();
    let step = 0.25 * 2.0 / ((1u64 << address.level()) as f64 * cells as f64);
    let axes = [DVec3::X, DVec3::Y, DVec3::Z];
    let mut failed = false;
    let directions = std::array::from_fn(|index| {
        if index == 0 {
            direction
        } else {
            let axis = axes[(index - 1) / 2];
            let sign = if index % 2 == 1 { 1.0 } else { -1.0 };
            let projected = axis - center * center.dot(axis);
            match Direction3::try_new(center + projected * (sign * step)) {
                Ok(sample_direction) => sample_direction,
                Err(_) => {
                    failed = true;
                    direction
                }
            }
        }
    });
    if failed {
        Err(TileBuildError::InvalidSample)
    } else {
        Ok(directions)
    }
}

fn normalize_material(values: [f64; 4]) -> Result<[f32; 4], TileBuildError> {
    let sum = values.iter().sum::<f64>();
    if !sum.is_finite() || sum <= 0.0 {
        return Err(TileBuildError::InvalidSample);
    }
    let mut result = values.map(|value| (value / sum) as f32);
    let total = result.iter().sum::<f32>();
    if !total.is_finite() || total <= 0.0 {
        return Err(TileBuildError::InvalidSample);
    }
    result = result.map(|value| value / total);
    Ok(result)
}

/// Build exact configuration words in source-defined field order. Diagnostic
/// hashes alone are not suitable for complete-definition cache equality.
fn exact_definition_words(generator: &SurfaceGenerator) -> Vec<u64> {
    let definition = generator.definition();
    let mut words = Vec::with_capacity(32);
    words.extend([
        0x5355_5246_4445_4601,
        definition.identity().0,
        definition.seed().0,
        definition.terrain().algorithm().code(),
    ]);
    let parameters = definition.terrain().parameters();
    words.extend([
        canonical_f64_bits(parameters.age),
        canonical_f64_bits(parameters.activity),
        canonical_f64_bits(parameters.resurfacing_fraction),
        canonical_f64_bits(parameters.impact_retention),
        canonical_f64_bits(parameters.relief_fraction),
        canonical_f64_bits(parameters.feature_scale_fraction),
        canonical_f64_bits(parameters.orientation_radians),
    ]);
    let shape = definition.shape();
    append_string_words(&mut words, shape.algorithm_name());
    words.extend(shape.axes_fractions().map(canonical_f64_bits));
    words.extend(shape.irregular_amplitudes().map(canonical_f64_bits));
    words.push(shape.seed().unwrap_or(0));
    let material = definition.material();
    words.extend([
        material.version().code(),
        canonical_f64_bits(material.composition()),
        canonical_f64_bits(material.regional_contrast()),
    ]);
    match definition.atmosphere() {
        SurfaceAtmosphere::Airless => words.push(0x4149_524c_4553_5301),
        SurfaceAtmosphere::Descriptor {
            pressure_pa,
            scale_height_m,
        } => {
            words.push(0x4154_4d4f_5350_0001);
            words.extend([
                canonical_f64_bits(pressure_pa),
                canonical_f64_bits(scale_height_m),
            ]);
        }
    }
    words
}

fn canonical_f64_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

fn append_string_words(words: &mut Vec<u64>, value: &str) {
    words.push(value.len() as u64);
    for chunk in value.as_bytes().chunks(8) {
        let mut bytes = [0u8; 8];
        bytes[..chunk.len()].copy_from_slice(chunk);
        words.push(u64::from_le_bytes(bytes));
    }
}

/// Direction from exact signed dyadic cube coordinates. Reduction precedes
/// f64 conversion, giving shared edges/corners identical normalization input.
fn canonical_direction_with_cells(
    address: CubePatchAddress,
    i: i64,
    j: i64,
    cells: u32,
) -> Result<Direction3, TileBuildError> {
    let denominator = u64::from(cells)
        .checked_shl(u32::from(address.level()))
        .ok_or(TileBuildError::InvalidAddress)?;
    let a = 2 * (i64::from(cells) * i64::from(address.coordinates()[0]) + i) - denominator as i64;
    let b = 2 * (i64::from(cells) * i64::from(address.coordinates()[1]) + j) - denominator as i64;
    let [normal, u, v] = address.face().basis();
    let mut xyz = std::array::from_fn::<_, 3, _>(|axis| {
        normal[axis] as i64 * denominator as i64 + u[axis] as i64 * a + v[axis] as i64 * b
    });
    let mut divisor = denominator;
    while divisor > 1 && divisor.is_multiple_of(2) && xyz.iter().all(|component| component % 2 == 0)
    {
        xyz = xyz.map(|component| component / 2);
        divisor /= 2;
    }
    let raw = DVec3::new(xyz[0] as f64, xyz[1] as f64, xyz[2] as f64);
    Direction3::try_new(raw).map_err(|_| TileBuildError::InvalidAddress)
}
