//! Deterministic CPU derivation of resident radial terrain tiles.
//!
//! The complete `SurfaceGenerator` remains the authority. This module only
//! samples and filters that field into a finite renderer representation.

use std::time::{Duration, Instant};

use glam::DVec3;
use mundaris_math::surface::SurfaceLocation;
use mundaris_math::{Direction3, surface::CubePatchAddress};
use mundaris_world::terrain::{
    SurfaceAtmosphere, SurfaceGenerator, SurfaceQueryContext, TerrainError,
};

pub use mundaris_renderer::resident_tile::{TileData, TileKey, TileTexel};

/// Prototype tile density. The payload stores one texel of halo on every side.
pub const DEFAULT_TILE_CELLS: u32 = 64;
/// Version of the CPU filtering policy recorded in every tile key.
pub const TILE_FILTER_VERSION: u32 = mundaris_renderer::resident_tile::TILE_FILTER_VERSION;
/// Version of the radial displacement/material texel layout.
pub const TILE_FORMAT_VERSION: u32 = mundaris_renderer::resident_tile::TILE_FORMAT_VERSION;
const QUERY_CACHE_FILTER_STEP_LIMIT_M: f64 = 8.0;

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
    /// Per-build bounded cache lookups, kept separate from authoritative queries.
    pub query_cache_hits: u64,
    pub query_cache_misses: u64,
    pub query_feature_cache_hits: u64,
    pub query_feature_cache_misses: u64,
    pub query_controls_cache_hits: u64,
    pub query_controls_cache_misses: u64,
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
    /// Fixed query-cache payload allocated by one cached tile build.
    pub const fn query_cache_workspace_bound_bytes() -> usize {
        SurfaceQueryContext::workspace_bound_bytes()
    }

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
        Self::build_inner(generator, identity, address, cells, None)
    }

    /// Build a tile with a bounded query context borrowed from this generator.
    /// The context lives only for this build and does not alter query results.
    pub fn build_cached(
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
        address: CubePatchAddress,
        cells: u32,
    ) -> Result<(TileData, TileBuildDiagnostics), TileBuildError> {
        // At broad physical sample spacing, adjacent stencil queries rarely
        // reuse cells. The threshold is expressed in metres so the policy is
        // independent of body radius and tile level.
        if query_filter_step_m(generator.radius_m(), address, cells)?
            > QUERY_CACHE_FILTER_STEP_LIMIT_M
        {
            return Self::build_inner(generator, identity, address, cells, None);
        }
        let mut context = generator.query_context();
        Self::build_inner(generator, identity, address, cells, Some(&mut context))
    }

    /// Explicit name for the uncached reference path used in matched comparisons.
    pub fn build_uncached(
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
        address: CubePatchAddress,
        cells: u32,
    ) -> Result<(TileData, TileBuildDiagnostics), TileBuildError> {
        Self::build_inner(generator, identity, address, cells, None)
    }

    fn build_inner(
        generator: &SurfaceGenerator,
        identity: TileBuildIdentity,
        address: CubePatchAddress,
        cells: u32,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<(TileData, TileBuildDiagnostics), TileBuildError> {
        if !cells.is_power_of_two() || !(1..=TileData::MAX_CELLS).contains(&cells) {
            return Err(TileBuildError::InvalidCells);
        }
        let started = Instant::now();
        let anchor_direction =
            canonical_direction_with_cells(address, i64::from(cells), i64::from(cells), cells * 2)?;
        let anchor_sample = evaluate(generator, anchor_direction, context.as_deref_mut())?;
        let anchor_radius_m = anchor_sample.radius_m();
        let anchor_filter_directions = filter_directions(anchor_direction, address, cells)?;
        let anchor_position = anchor_sample.position(SurfaceLocation::new(anchor_direction));
        let mut filter_actual_surface_offsets_m = [0.0; 6];
        for (index, direction) in anchor_filter_directions.into_iter().skip(1).enumerate() {
            let sample = evaluate(generator, direction, context.as_deref_mut())?;
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
                    let sample = evaluate(generator, sample_direction, context.as_deref_mut())?;
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
        let query_cache_stats = context
            .as_deref()
            .map(SurfaceQueryContext::stats)
            .unwrap_or_default();
        let surface_query_heap_scratch_bytes = query_cache_stats.workspace_bytes;
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
                query_cache_hits: query_cache_stats
                    .feature_hits
                    .saturating_add(query_cache_stats.controls_hits),
                query_cache_misses: query_cache_stats
                    .feature_misses
                    .saturating_add(query_cache_stats.controls_misses),
                query_feature_cache_hits: query_cache_stats.feature_hits,
                query_feature_cache_misses: query_cache_stats.feature_misses,
                query_controls_cache_hits: query_cache_stats.controls_hits,
                query_controls_cache_misses: query_cache_stats.controls_misses,
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
                let complete = evaluate(generator, grid_direction, None)?.radius_m();
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
                    let complete = evaluate(generator, direction, None)?
                        .position(SurfaceLocation::new(direction));
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
                let complete_normal = evaluate(generator, direction, None)?.normal();
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
    context: Option<&mut SurfaceQueryContext<'_>>,
) -> Result<mundaris_world::terrain::SurfaceSample, TileBuildError> {
    let location = SurfaceLocation::new(direction);
    match context {
        Some(context) => context.evaluate_point(location),
        None => generator.evaluate_point(location),
    }
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

fn query_filter_step_m(
    radius_m: f64,
    address: CubePatchAddress,
    cells: u32,
) -> Result<f64, TileBuildError> {
    let chart_divisions = 1u64
        .checked_shl(u32::from(address.level()))
        .and_then(|divisions| divisions.checked_mul(u64::from(cells)))
        .ok_or(TileBuildError::InvalidAddress)?;
    Ok(radius_m * 0.5 / chart_divisions as f64)
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

#[cfg(test)]
mod query_context_tests {
    use super::*;
    use glam::DVec3;
    use mundaris_math::surface::{CubeFace, CubePatchAddress};
    use mundaris_world::terrain::{
        SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
    };

    fn generator() -> SurfaceGenerator {
        generator_at_radius(1_737_400.0)
    }

    fn gameplay_generator() -> SurfaceGenerator {
        let production_moon_seed = 0x4d4f_4f4e;
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(production_moon_seed),
            TerrainSeed(production_moon_seed),
            SurfaceAlgorithm::RockyV5,
        );
        let gameplay_radius_m = 1_737_400.0 * (400_000.0 / 6_371_000.0);
        SurfaceGenerator::new(&definition, gameplay_radius_m).unwrap()
    }

    fn generator_at_radius(radius_m: f64) -> SurfaceGenerator {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x2e_cafe_0001),
            TerrainSeed(0x005e_ed2a),
            SurfaceAlgorithm::RockyV5,
        );
        SurfaceGenerator::new(&definition, radius_m).unwrap()
    }

    fn assert_sample_bits_equal(
        left: mundaris_world::terrain::SurfaceSample,
        right: mundaris_world::terrain::SurfaceSample,
    ) {
        assert_eq!(
            left.shape().direction().to_array().map(f64::to_bits),
            right.shape().direction().to_array().map(f64::to_bits)
        );
        assert_eq!(
            left.shape().radius_m().to_bits(),
            right.shape().radius_m().to_bits()
        );
        assert_eq!(
            left.shape().gradient_m().to_array().map(f64::to_bits),
            right.shape().gradient_m().to_array().map(f64::to_bits)
        );
        assert_eq!(
            left.shape().normal().to_array().map(f64::to_bits),
            right.shape().normal().to_array().map(f64::to_bits)
        );
        assert_eq!(
            left.terrain().height_m().to_bits(),
            right.terrain().height_m().to_bits()
        );
        assert_eq!(
            left.terrain()
                .tangent_gradient_m_per_unit_direction()
                .to_array()
                .map(f64::to_bits),
            right
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .to_array()
                .map(f64::to_bits)
        );
        assert_eq!(
            left.material_weights().map(f64::to_bits),
            right.material_weights().map(f64::to_bits)
        );
        assert_eq!(left.radius_m().to_bits(), right.radius_m().to_bits());
        assert_eq!(
            left.normal().to_array().map(f64::to_bits),
            right.normal().to_array().map(f64::to_bits)
        );
        assert_eq!(left.work(), right.work());
    }

    #[test]
    fn cached_query_and_adjacent_tile_payloads_match_the_uncached_oracle_bitwise() {
        let generator = generator();
        let identity = TileBuildIdentity {
            body_identity: 0xface_1234,
            surface_revision: 9,
            material_revision: 4,
        };
        for level in [4u8, 8, 12, 16] {
            let tile_count = 1u32 << level;
            let x = tile_count / 3;
            let y = tile_count / 2;
            for address in [
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap(),
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, x + 1, y).unwrap(),
            ] {
                let (oracle, _) =
                    ResidentTileBuilder::build_uncached(&generator, identity, address, 32).unwrap();
                let (cached, diagnostics) =
                    ResidentTileBuilder::build_cached(&generator, identity, address, 32).unwrap();
                assert_eq!(oracle.key, cached.key);
                assert_eq!(
                    oracle.anchor_radius_m.to_bits(),
                    cached.anchor_radius_m.to_bits()
                );
                assert_eq!(
                    oracle.min_max_radial_offset_m.map(f64::to_bits),
                    cached.min_max_radial_offset_m.map(f64::to_bits)
                );
                assert_eq!(oracle.texels.len(), cached.texels.len());
                for (left, right) in oracle.texels.iter().zip(&cached.texels) {
                    assert_eq!(
                        left.radial_offset_m.to_bits(),
                        right.radial_offset_m.to_bits()
                    );
                    assert_eq!(
                        left.material.map(f32::to_bits),
                        right.material.map(f32::to_bits)
                    );
                }
                if query_filter_step_m(generator.radius_m(), address, 32).unwrap()
                    <= QUERY_CACHE_FILTER_STEP_LIMIT_M
                {
                    assert!(diagnostics.surface_query_heap_scratch_bytes > 0);
                    assert!(diagnostics.query_cache_hits > 0);
                } else {
                    assert_eq!(diagnostics.surface_query_heap_scratch_bytes, 0);
                    assert_eq!(diagnostics.query_cache_hits, 0);
                }
            }
        }

        let mut locations = vec![
            DVec3::X,
            -DVec3::X,
            DVec3::Y,
            -DVec3::Y,
            DVec3::Z,
            -DVec3::Z,
            DVec3::new(1.0, 1.0, 1.0),
        ];
        let near = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
        locations.extend(
            generator
                .diagnostic_boundary_probes(near)
                .unwrap()
                .into_iter()
                .map(|probe| probe.location.direction().unit()),
        );
        let mut random = 0x91e1_0da5_c79e_7b1d_u64;
        for _ in 0..96 {
            let mut next = || {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((random >> 11) as f64 / ((1u64 << 53) as f64)) * 2.0 - 1.0
            };
            locations.push(DVec3::new(next(), next(), next()));
        }
        let mut context = generator.query_context();
        for direction in locations {
            let direction = Direction3::try_new(direction).unwrap();
            let location = SurfaceLocation::new(direction);
            let oracle = generator.evaluate_point(location).unwrap();
            let cached = context.evaluate_point(location).unwrap();
            assert_sample_bits_equal(oracle, cached);
        }
    }

    #[test]
    fn gameplay_radius_high_lod_tiles_and_queries_match_bitwise() {
        let generator = gameplay_generator();
        assert_eq!(generator.radius_m().to_bits(), 4_682_232_455_833_781_654);
        assert_eq!(generator.definition().identity().0, 0x4d4f_4f4e);
        assert_eq!(generator.definition().seed().0, 0x4d4f_4f4e);
        let identity = TileBuildIdentity {
            body_identity: 0xface_1234,
            surface_revision: 9,
            material_revision: 4,
        };
        for level in [8u8, 10, 12, 16] {
            let tile_count = 1u32 << level;
            let x = tile_count / 3;
            let y = tile_count / 2;
            for address in [
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap(),
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, x + 1, y).unwrap(),
            ] {
                let (oracle, _) =
                    ResidentTileBuilder::build_uncached(&generator, identity, address, 32).unwrap();
                let (cached, diagnostics) =
                    ResidentTileBuilder::build_cached(&generator, identity, address, 32).unwrap();
                assert_eq!(oracle.key, cached.key);
                assert_eq!(
                    oracle.anchor_radius_m.to_bits(),
                    cached.anchor_radius_m.to_bits()
                );
                assert_eq!(
                    oracle.min_max_radial_offset_m.map(f64::to_bits),
                    cached.min_max_radial_offset_m.map(f64::to_bits)
                );
                for (left, right) in oracle.texels.iter().zip(&cached.texels) {
                    assert_eq!(
                        left.radial_offset_m.to_bits(),
                        right.radial_offset_m.to_bits()
                    );
                    assert_eq!(
                        left.material.map(f32::to_bits),
                        right.material.map(f32::to_bits)
                    );
                }
                assert!(diagnostics.surface_query_heap_scratch_bytes > 0);
                assert!(diagnostics.query_cache_hits > 0);
            }
        }

        let near = SurfaceLocation::new(Direction3::try_new(DVec3::Z).unwrap());
        let mut directions = vec![
            DVec3::X,
            -DVec3::X,
            DVec3::Y,
            -DVec3::Y,
            DVec3::Z,
            -DVec3::Z,
        ];
        directions.extend(
            generator
                .diagnostic_boundary_probes(near)
                .unwrap()
                .into_iter()
                .map(|probe| probe.location.direction().unit()),
        );
        let mut random = 0xd8b4_61c3_5a27_0091_u64;
        for _ in 0..48 {
            let mut next = || {
                random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
                ((random >> 11) as f64 / ((1u64 << 53) as f64)) * 2.0 - 1.0
            };
            directions.push(DVec3::new(next(), next(), next()));
        }
        let mut context = generator.query_context();
        for direction in directions {
            let location = SurfaceLocation::new(Direction3::try_new(direction).unwrap());
            assert_sample_bits_equal(
                generator.evaluate_point(location).unwrap(),
                context.evaluate_point(location).unwrap(),
            );
        }
    }

    #[test]
    #[ignore = "release-mode same-process comparison; run explicitly for Phase 2E evidence"]
    fn release_cached_vs_uncached_tile_benchmark() {
        benchmark_fixture(
            "generic_real_scale",
            &generator(),
            TileBuildIdentity {
                body_identity: 0xface_1234,
                surface_revision: 9,
                material_revision: 4,
            },
        );
        benchmark_fixture(
            "native_gameplay_moon",
            &gameplay_generator(),
            TileBuildIdentity {
                body_identity: 5,
                surface_revision: 0,
                material_revision: 0,
            },
        );
    }

    fn benchmark_fixture(fixture: &str, generator: &SurfaceGenerator, identity: TileBuildIdentity) {
        let definition_words = exact_definition_words(generator);
        let definition = generator.definition();
        let radius_m = generator.radius_m();
        let radius_bits = radius_m.to_bits();
        let terrain_identity = definition.identity().0;
        let terrain_seed = definition.seed().0;
        let mut corpus = Vec::new();
        for level in [4u8, 8, 12, 16] {
            let tile_count = 1u32 << level;
            let x = tile_count / 3;
            let y = tile_count / 2;
            corpus.push(CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap());
            corpus.push(CubePatchAddress::try_new(CubeFace::PositiveZ, level, x + 1, y).unwrap());
        }
        let mut uncached_nanos = Vec::new();
        let mut cached_nanos = Vec::new();
        let mut uncached_by_level: [Vec<u64>; 4] = std::array::from_fn(|_| Vec::new());
        let mut cached_by_level: [Vec<u64>; 4] = std::array::from_fn(|_| Vec::new());
        let mut feature_hits = 0u64;
        let mut feature_misses = 0u64;
        let mut controls_hits = 0u64;
        let mut controls_misses = 0u64;
        let mut feature_hits_by_level = [0u64; 4];
        let mut feature_misses_by_level = [0u64; 4];
        let mut controls_hits_by_level = [0u64; 4];
        let mut controls_misses_by_level = [0u64; 4];
        let mut workspace_bytes = 0usize;
        for repetition in 0..3 {
            for (index, address) in corpus.iter().copied().enumerate() {
                for cached_first in [(index + repetition) % 2 == 0] {
                    let mut run_cached = || {
                        let started = Instant::now();
                        let (_, diagnostics) =
                            ResidentTileBuilder::build_cached(&generator, identity, address, 32)
                                .unwrap();
                        let elapsed_ns = started.elapsed().as_nanos() as u64;
                        cached_nanos.push(elapsed_ns);
                        cached_by_level[index / 2].push(elapsed_ns);
                        feature_hits =
                            feature_hits.saturating_add(diagnostics.query_feature_cache_hits);
                        feature_misses =
                            feature_misses.saturating_add(diagnostics.query_feature_cache_misses);
                        controls_hits =
                            controls_hits.saturating_add(diagnostics.query_controls_cache_hits);
                        controls_misses =
                            controls_misses.saturating_add(diagnostics.query_controls_cache_misses);
                        feature_hits_by_level[index / 2] = feature_hits_by_level[index / 2]
                            .saturating_add(diagnostics.query_feature_cache_hits);
                        feature_misses_by_level[index / 2] = feature_misses_by_level[index / 2]
                            .saturating_add(diagnostics.query_feature_cache_misses);
                        controls_hits_by_level[index / 2] = controls_hits_by_level[index / 2]
                            .saturating_add(diagnostics.query_controls_cache_hits);
                        controls_misses_by_level[index / 2] = controls_misses_by_level[index / 2]
                            .saturating_add(diagnostics.query_controls_cache_misses);
                        workspace_bytes = diagnostics.surface_query_heap_scratch_bytes;
                    };
                    let mut run_uncached = || {
                        let started = Instant::now();
                        ResidentTileBuilder::build_uncached(&generator, identity, address, 32)
                            .unwrap();
                        let elapsed_ns = started.elapsed().as_nanos() as u64;
                        uncached_nanos.push(elapsed_ns);
                        uncached_by_level[index / 2].push(elapsed_ns);
                    };
                    if cached_first {
                        run_cached();
                        run_uncached();
                    } else {
                        run_uncached();
                        run_cached();
                    }
                }
            }
        }
        let uncached_median_ns = median(&mut uncached_nanos);
        let cached_median_ns = median(&mut cached_nanos);
        let uncached_mean_ns = mean(&uncached_nanos);
        let cached_mean_ns = mean(&cached_nanos);
        let speedup = uncached_median_ns as f64 / cached_median_ns as f64;
        println!(
            "PHASE_2E_QUERY_CACHE_JSON {{\"fixture\":\"{}\",\"scope\":\"aggregate\",\"build\":\"release\",\"algorithm\":\"RockyV5\",\"terrain_identity\":{},\"terrain_seed\":{},\"radius_m\":{:.14},\"radius_bits\":{},\"definition_words\":{:?},\"cells\":32,\"unique_tiles\":{},\"paired_builds_per_path\":{},\"uncached_median_ns\":{},\"cached_median_ns\":{},\"uncached_mean_ns\":{},\"cached_mean_ns\":{},\"median_speedup\":{:.6},\"workspace_bytes\":{},\"feature_hits\":{},\"feature_misses\":{},\"controls_hits\":{},\"controls_misses\":{}}}",
            fixture,
            terrain_identity,
            terrain_seed,
            radius_m,
            radius_bits,
            definition_words,
            corpus.len(),
            uncached_nanos.len(),
            uncached_median_ns,
            cached_median_ns,
            uncached_mean_ns,
            cached_mean_ns,
            speedup,
            workspace_bytes,
            feature_hits,
            feature_misses,
            controls_hits,
            controls_misses,
        );
        for (index, level) in [4, 8, 12, 16].into_iter().enumerate() {
            let uncached_median_ns = median(&mut uncached_by_level[index]);
            let cached_median_ns = median(&mut cached_by_level[index]);
            println!(
                "PHASE_2E_QUERY_CACHE_JSON {{\"fixture\":\"{}\",\"scope\":\"lod\",\"algorithm\":\"RockyV5\",\"terrain_identity\":{},\"terrain_seed\":{},\"radius_m\":{:.14},\"radius_bits\":{},\"definition_words\":{:?},\"level\":{},\"paired_builds\":{},\"uncached_median_ns\":{},\"cached_median_ns\":{},\"median_speedup\":{:.6},\"feature_hits\":{},\"feature_misses\":{},\"controls_hits\":{},\"controls_misses\":{}}}",
                fixture,
                terrain_identity,
                terrain_seed,
                radius_m,
                radius_bits,
                definition_words,
                level,
                uncached_by_level[index].len(),
                uncached_median_ns,
                cached_median_ns,
                uncached_median_ns as f64 / cached_median_ns as f64,
                feature_hits_by_level[index],
                feature_misses_by_level[index],
                controls_hits_by_level[index],
                controls_misses_by_level[index],
            );
        }
    }

    fn median(samples: &mut [u64]) -> u64 {
        samples.sort_unstable();
        samples[samples.len() / 2]
    }

    fn mean(samples: &[u64]) -> u64 {
        samples.iter().copied().sum::<u64>() / samples.len() as u64
    }
}
