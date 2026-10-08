//! Bounded, disposable multi-resolution samples for resident terrain filtering.
//!
//! The world surface remains authoritative. This cache stores complete raw
//! radius/material samples at canonical cube-grid directions and reconstructs
//! requested render-filter locations bilinearly.

use std::sync::{Arc, Mutex, atomic::AtomicUsize};

use glam::DVec3;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::{
    SurfaceDefinition, SurfaceGenerator, SurfaceQueryContext, SurfaceSample,
};

use super::TileBuildError;

const CACHE_SLOTS: usize = 131_072;
const MAX_CELLS: u32 = 128;
const MAX_BATCH_LOCATIONS: usize = 917;
const MAX_UNIQUE_NODES: usize = MAX_BATCH_LOCATIONS * 4;
const CACHE_BYTE_CAP: usize = 16 * 1024 * 1024;

/// Reconstructed render sample. It is deliberately not a `SurfaceSample`:
/// interpolation does not preserve the complete authority's shape/normal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DerivedSample {
    pub radius_m: f64,
    pub material_weights: [f64; 4],
}

/// Bounded counters for one generator-bound derived field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DerivedFieldStats {
    pub batches: u64,
    pub raw_node_queries: u64,
    pub requested_nodes: u64,
    pub unique_nodes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub duplicate_node_evaluations: u64,
    /// Table vector payload plus field/table metadata and one Arc control-header
    /// estimate. Excludes allocator bookkeeping and size-class rounding.
    pub retained_cache_bytes: usize,
    /// Declared logical allocation cap, including the metadata estimate.
    pub cache_capacity_bytes: usize,
    /// Peak logical scratch capacity used by the most recent non-empty batch.
    pub last_batch_scratch_bytes: usize,
    /// Maximum logical batch scratch, including raw complete sample outputs.
    pub batch_scratch_bound_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NodeKey([i64; 3]);

#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    key: NodeKey,
    sample: DerivedSample,
}

struct CacheTable {
    entries: Box<[Option<CacheEntry>]>,
    stats: DerivedFieldStats,
}

struct SharedDerivedFieldInner {
    definition: SurfaceDefinition,
    radius_bits: u64,
    table: Mutex<CacheTable>,
}

/// Immutable field identity with a bounded, shared raw-node cache.
///
/// Clones share only disposable derived render data. Identity is checked using
/// the complete definition and exact body-radius bits before every batch.
#[derive(Clone)]
pub struct SharedDerivedField {
    inner: Arc<SharedDerivedFieldInner>,
}

#[derive(Debug, Clone, Copy)]
struct Stencil {
    keys: [NodeKey; 4],
    weights: [f64; 4],
}

impl SharedDerivedField {
    /// Allocate the fixed direct-mapped table. Its payload is below 16 MiB.
    pub fn new(generator: &SurfaceGenerator) -> Result<Self, TileBuildError> {
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(CACHE_SLOTS)
            .map_err(|_| TileBuildError::InvalidSample)?;
        // The exact-capacity check makes into_boxed_slice a non-shrinking move.
        if entries.capacity() != CACHE_SLOTS {
            return Err(TileBuildError::InvalidSample);
        }
        entries.resize_with(CACHE_SLOTS, || None);
        let table_payload_bytes = entries
            .capacity()
            .saturating_mul(std::mem::size_of::<Option<CacheEntry>>());
        let retained_cache_bytes = table_payload_bytes
            .saturating_add(std::mem::size_of::<SharedDerivedFieldInner>())
            .saturating_add(2 * std::mem::size_of::<AtomicUsize>())
            .saturating_add(std::mem::size_of::<SharedDerivedField>());
        if retained_cache_bytes > CACHE_BYTE_CAP {
            return Err(TileBuildError::InvalidSample);
        }
        let entries = entries.into_boxed_slice();
        let batch_scratch_bound_bytes = batch_scratch_bound_bytes();
        Ok(Self {
            inner: Arc::new(SharedDerivedFieldInner {
                definition: generator.definition().clone(),
                radius_bits: generator.radius_m().to_bits(),
                table: Mutex::new(CacheTable {
                    entries,
                    stats: DerivedFieldStats {
                        retained_cache_bytes,
                        cache_capacity_bytes: CACHE_BYTE_CAP,
                        batch_scratch_bound_bytes,
                        ..DerivedFieldStats::default()
                    },
                }),
            }),
        })
    }

    pub fn is_bound_to(&self, generator: &SurfaceGenerator) -> bool {
        self.inner.definition == *generator.definition()
            && self.inner.radius_bits == generator.radius_m().to_bits()
    }

    pub fn stats(&self) -> Result<DerivedFieldStats, TileBuildError> {
        self.inner
            .table
            .lock()
            .map(|table| table.stats)
            .map_err(|_| TileBuildError::InvalidSample)
    }

    /// Reconstruct a bounded batch of directions from canonical raw grid nodes.
    /// Each batch takes at most two cache locks; authoritative evaluations run
    /// outside the lock through the caller's prepared worker context.
    pub fn evaluate_batch(
        &self,
        generator: &SurfaceGenerator,
        context: &mut SurfaceQueryContext<'_>,
        level: u8,
        cells: u32,
        locations: &[SurfaceLocation],
        output: &mut [DerivedSample],
    ) -> Result<(), TileBuildError> {
        if !self.is_bound_to(generator) || !context.is_bound_to(generator) {
            return Err(TileBuildError::InvalidSample);
        }
        if level > 30 {
            return Err(TileBuildError::InvalidAddress);
        }
        if !cells.is_power_of_two() || !(1..=MAX_CELLS).contains(&cells) {
            return Err(TileBuildError::InvalidCells);
        }
        if locations.len() != output.len() || locations.len() > MAX_BATCH_LOCATIONS {
            return Err(TileBuildError::InvalidSample);
        }
        if locations.is_empty() {
            return Ok(());
        }

        let stencils = locations.iter();
        let mut stencil_values = Vec::new();
        stencil_values
            .try_reserve_exact(locations.len())
            .map_err(|_| TileBuildError::InvalidSample)?;
        if stencil_values.capacity() != locations.len() {
            return Err(TileBuildError::InvalidSample);
        }
        for location in stencils {
            stencil_values.push(make_stencil(*location, level, cells)?);
        }
        let stencils = stencil_values;
        let requested_nodes = stencils.len().saturating_mul(4);
        if requested_nodes > MAX_UNIQUE_NODES {
            return Err(TileBuildError::InvalidSample);
        }
        let mut keys = Vec::new();
        keys.try_reserve_exact(requested_nodes)
            .map_err(|_| TileBuildError::InvalidSample)?;
        if keys.capacity() != requested_nodes {
            return Err(TileBuildError::InvalidSample);
        }
        for stencil in &stencils {
            keys.extend(stencil.keys);
        }
        keys.sort_unstable();
        keys.dedup();
        if keys.len() > MAX_UNIQUE_NODES {
            return Err(TileBuildError::InvalidSample);
        }

        let mut samples = Vec::new();
        samples
            .try_reserve_exact(keys.len())
            .map_err(|_| TileBuildError::InvalidSample)?;
        if samples.capacity() != keys.len() {
            return Err(TileBuildError::InvalidSample);
        }
        samples.resize(keys.len(), None::<DerivedSample>);
        let mut missing = Vec::new();
        missing
            .try_reserve_exact(keys.len())
            .map_err(|_| TileBuildError::InvalidSample)?;
        if missing.capacity() != keys.len() {
            return Err(TileBuildError::InvalidSample);
        }
        missing.resize(keys.len(), false);

        {
            let mut table = self
                .inner
                .table
                .lock()
                .map_err(|_| TileBuildError::InvalidSample)?;
            table.stats.batches = table.stats.batches.saturating_add(1);
            table.stats.requested_nodes = table
                .stats
                .requested_nodes
                .saturating_add(requested_nodes as u64);
            table.stats.unique_nodes = table.stats.unique_nodes.saturating_add(keys.len() as u64);
            for (index, key) in keys.iter().enumerate() {
                if let Some(entry) = table.entries[cache_index(*key)]
                    && entry.key == *key
                {
                    samples[index] = Some(entry.sample);
                    table.stats.cache_hits = table.stats.cache_hits.saturating_add(1);
                    continue;
                }
                table.stats.cache_misses = table.stats.cache_misses.saturating_add(1);
                missing[index] = true;
            }
        }

        let raw_node_queries = missing.iter().filter(|is_missing| **is_missing).count();
        let mut raw_samples = Vec::<SurfaceSample>::new();
        raw_samples
            .try_reserve_exact(raw_node_queries)
            .map_err(|_| TileBuildError::InvalidSample)?;
        if raw_samples.capacity() != raw_node_queries {
            return Err(TileBuildError::InvalidSample);
        }
        for (key, is_missing) in keys.iter().zip(&missing) {
            if *is_missing {
                let direction = Direction3::try_new(DVec3::new(
                    key.0[0] as f64,
                    key.0[1] as f64,
                    key.0[2] as f64,
                ))
                .map_err(|_| TileBuildError::InvalidSample)?;
                raw_samples.push(context.evaluate_point(SurfaceLocation::new(direction))?);
            }
        }
        let mut raw_sample_index = 0;
        for (index, is_missing) in missing.iter().copied().enumerate() {
            if is_missing {
                let complete = raw_samples
                    .get(raw_sample_index)
                    .copied()
                    .ok_or(TileBuildError::InvalidSample)?;
                raw_sample_index += 1;
                samples[index] = Some(DerivedSample {
                    radius_m: complete.radius_m(),
                    material_weights: complete.material_weights(),
                });
            }
        }
        let actual_scratch_bytes = scratch_capacity_bytes(
            stencils.capacity(),
            keys.capacity(),
            samples.capacity(),
            missing.capacity(),
            raw_samples.capacity(),
        );
        if actual_scratch_bytes > batch_scratch_bound_bytes() {
            return Err(TileBuildError::InvalidSample);
        }

        {
            let mut table = self
                .inner
                .table
                .lock()
                .map_err(|_| TileBuildError::InvalidSample)?;
            table.stats.raw_node_queries = table
                .stats
                .raw_node_queries
                .saturating_add(raw_node_queries as u64);
            for ((key, sample), was_missing) in keys
                .iter()
                .copied()
                .zip(samples.iter().copied())
                .zip(missing.iter().copied())
            {
                if !was_missing {
                    continue;
                }
                let Some(sample) = sample else {
                    return Err(TileBuildError::InvalidSample);
                };
                insert_sample(&mut table, key, sample);
            }
            table.stats.last_batch_scratch_bytes = actual_scratch_bytes;
        }

        for (stencil, result) in stencils.iter().zip(output.iter_mut()) {
            let mut reconstructed = DerivedSample {
                radius_m: 0.0,
                material_weights: [0.0; 4],
            };
            for (key, weight) in stencil.keys.iter().zip(stencil.weights) {
                let index = keys
                    .binary_search(key)
                    .map_err(|_| TileBuildError::InvalidSample)?;
                let sample = samples[index].ok_or(TileBuildError::InvalidSample)?;
                reconstructed.radius_m += sample.radius_m * weight;
                for (out, input) in reconstructed
                    .material_weights
                    .iter_mut()
                    .zip(sample.material_weights)
                {
                    *out += input * weight;
                }
            }
            if !reconstructed.radius_m.is_finite()
                || reconstructed.radius_m <= 0.0
                || reconstructed
                    .material_weights
                    .iter()
                    .any(|weight| !weight.is_finite() || *weight < 0.0)
            {
                return Err(TileBuildError::InvalidSample);
            }
            *result = reconstructed;
        }
        Ok(())
    }
}

fn batch_scratch_bound_bytes() -> usize {
    MAX_BATCH_LOCATIONS
        .saturating_mul(std::mem::size_of::<Stencil>() + 4 * std::mem::size_of::<NodeKey>())
        .saturating_add(
            MAX_UNIQUE_NODES.saturating_mul(std::mem::size_of::<Option<DerivedSample>>()),
        )
        .saturating_add(MAX_UNIQUE_NODES.saturating_mul(std::mem::size_of::<bool>()))
        .saturating_add(MAX_UNIQUE_NODES.saturating_mul(std::mem::size_of::<SurfaceSample>()))
}

fn scratch_capacity_bytes(
    stencil_capacity: usize,
    key_capacity: usize,
    sample_capacity: usize,
    missing_capacity: usize,
    raw_sample_capacity: usize,
) -> usize {
    stencil_capacity
        .saturating_mul(std::mem::size_of::<Stencil>())
        .saturating_add(key_capacity.saturating_mul(std::mem::size_of::<NodeKey>()))
        .saturating_add(
            sample_capacity.saturating_mul(std::mem::size_of::<Option<DerivedSample>>()),
        )
        .saturating_add(missing_capacity.saturating_mul(std::mem::size_of::<bool>()))
        .saturating_add(raw_sample_capacity.saturating_mul(std::mem::size_of::<SurfaceSample>()))
}

fn make_stencil(
    location: SurfaceLocation,
    level: u8,
    cells: u32,
) -> Result<Stencil, TileBuildError> {
    // face_uv uses the global deterministic dominant-axis tie rule (X, then Y,
    // then Z), independent of the requesting tile's face.
    let (face, uv) = location.face_uv();
    let cells_per_face = i64::from(cells)
        .checked_mul(1_i64 << level)
        .ok_or(TileBuildError::InvalidSample)?;
    let chart = [uv[0], uv[1]].map(|coordinate| {
        ((coordinate + 1.0) * 0.5 * cells_per_face as f64).clamp(0.0, cells_per_face as f64)
    });
    if chart.iter().any(|value| !value.is_finite()) {
        return Err(TileBuildError::InvalidSample);
    }
    let base = chart.map(|value| (value.floor() as i64).min(cells_per_face - 1));
    let fraction = [chart[0] - base[0] as f64, chart[1] - base[1] as f64];
    let weights = [
        (1.0 - fraction[0]) * (1.0 - fraction[1]),
        fraction[0] * (1.0 - fraction[1]),
        (1.0 - fraction[0]) * fraction[1],
        fraction[0] * fraction[1],
    ];
    let nodes = [
        [base[0], base[1]],
        [base[0] + 1, base[1]],
        [base[0], base[1] + 1],
        [base[0] + 1, base[1] + 1],
    ];
    let [normal, u, v] = face.basis();
    let optional_keys = nodes.map(|[x, y]| {
        let a = 2 * x - cells_per_face;
        let b = 2 * y - cells_per_face;
        let integer_components = std::array::from_fn(|axis| {
            (normal[axis] * cells_per_face as f64 + u[axis] * a as f64 + v[axis] * b as f64) as i64
        });
        canonical_node_key(integer_components)
    });
    let mut keys = [NodeKey([0; 3]); 4];
    for (index, key) in optional_keys.into_iter().enumerate() {
        keys[index] = key.ok_or(TileBuildError::InvalidSample)?;
    }
    Ok(Stencil { keys, weights })
}

fn canonical_node_key(mut components: [i64; 3]) -> Option<NodeKey> {
    let divisor = gcd_i64(
        gcd_i64(components[0].unsigned_abs(), components[1].unsigned_abs()),
        components[2].unsigned_abs(),
    );
    if divisor == 0 {
        return None;
    }
    for component in &mut components {
        *component /= i64::try_from(divisor).ok()?;
    }
    Some(NodeKey(components))
}

fn gcd_i64(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn cache_index(key: NodeKey) -> usize {
    let mut hash = 0x9e37_79b9_7f4a_7c15_u64;
    for component in key.0 {
        hash ^= (component as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        hash = hash.rotate_left(27).wrapping_mul(0x94d0_49bb_1331_11eb);
    }
    (hash ^ (hash >> 31)) as usize & (CACHE_SLOTS - 1)
}

fn insert_sample(table: &mut CacheTable, key: NodeKey, sample: DerivedSample) {
    let slot = cache_index(key);
    match table.entries[slot] {
        Some(entry) if entry.key == key => {
            table.stats.duplicate_node_evaluations =
                table.stats.duplicate_node_evaluations.saturating_add(1);
        }
        Some(_) => {
            table.stats.cache_evictions = table.stats.cache_evictions.saturating_add(1);
            table.entries[slot] = Some(CacheEntry { key, sample });
        }
        None => table.entries[slot] = Some(CacheEntry { key, sample }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::surface::CubeFace;
    use mundaris_world::terrain::{SurfaceAlgorithm, TerrainIdentity, TerrainSeed};

    fn make_generator(radius_m: f64, seed: u64) -> SurfaceGenerator {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4d4f_4f4e),
            TerrainSeed(seed),
            SurfaceAlgorithm::RockyV5,
        );
        SurfaceGenerator::new(&definition, radius_m).expect("valid fixed test definition")
    }

    fn node_key(face: CubeFace, cells_per_face: i64, x: i64, y: i64) -> NodeKey {
        let [normal, u, v] = face.basis();
        let a = 2 * x - cells_per_face;
        let b = 2 * y - cells_per_face;
        let components = std::array::from_fn(|axis| {
            (normal[axis] * cells_per_face as f64 + u[axis] * a as f64 + v[axis] * b as f64) as i64
        });
        canonical_node_key(components).unwrap_or(NodeKey([0; 3]))
    }

    #[test]
    fn canonical_raw_nodes_match_on_all_cube_face_edges_and_corners() {
        let n = 32;
        for along in 0..=n {
            assert_eq!(
                node_key(CubeFace::PositiveX, n, n, along),
                node_key(CubeFace::NegativeZ, n, 0, along)
            );
            assert_eq!(
                node_key(CubeFace::PositiveX, n, along, n),
                node_key(CubeFace::PositiveY, n, n, along)
            );
            assert_eq!(
                node_key(CubeFace::PositiveX, n, along, 0),
                node_key(CubeFace::NegativeY, n, n, n - along)
            );
            assert_eq!(
                node_key(CubeFace::PositiveX, n, 0, along),
                node_key(CubeFace::PositiveZ, n, n, along)
            );
        }

        for sx in [-n, n] {
            for sy in [-n, n] {
                for sz in [-n, n] {
                    let expected =
                        canonical_node_key([sx, sy, sz]).expect("nonzero cube corner key");
                    let corner = DVec3::new(sx as f64, sy as f64, sz as f64);
                    for face in CubeFace::ALL {
                        let [normal, u, v] = face.basis();
                        let denominator = corner.dot(normal) as i64;
                        if denominator != n {
                            continue;
                        }
                        let x = (corner.dot(u) as i64 + n) / 2;
                        let y = (corner.dot(v) as i64 + n) / 2;
                        assert_eq!(node_key(face, n, x, y), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn direct_mapped_collisions_evict_and_allow_deterministic_rebuild() {
        let mut slots = vec![None; CACHE_SLOTS];
        let mut first = None;
        let mut second = None;
        for component in 1..=CACHE_SLOTS as i64 + 1 {
            let key = NodeKey([component, component.wrapping_mul(17), 1]);
            let slot = cache_index(key);
            if let Some(previous) = slots[slot] {
                first = Some(previous);
                second = Some(key);
                break;
            }
            slots[slot] = Some(key);
        }
        let first = first.expect("pigeonhole collision in fixed-size table");
        let second = second.expect("colliding second key");
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(CACHE_SLOTS)
            .expect("bounded test cache allocation");
        assert_eq!(entries.capacity(), CACHE_SLOTS);
        entries.resize_with(CACHE_SLOTS, || None);
        let mut table = CacheTable {
            entries: entries.into_boxed_slice(),
            stats: DerivedFieldStats::default(),
        };
        let sample = DerivedSample {
            radius_m: 10.0,
            material_weights: [0.25; 4],
        };
        insert_sample(&mut table, first, sample);
        insert_sample(
            &mut table,
            second,
            DerivedSample {
                radius_m: 20.0,
                ..sample
            },
        );
        assert_eq!(table.stats.cache_evictions, 1);
        assert_eq!(
            table.entries[cache_index(second)].map(|entry| entry.key),
            Some(second)
        );
        insert_sample(&mut table, first, sample);
        assert_eq!(table.stats.cache_evictions, 2);
        assert_eq!(
            table.entries[cache_index(first)].map(|entry| entry.sample),
            Some(sample)
        );
    }

    #[test]
    fn dominant_chart_and_cache_reuse_ignore_request_order_and_bind_exact_identity() {
        let generator = make_generator(109_081.776_8, 7);
        let field = SharedDerivedField::new(&generator).expect("bounded cache allocation");
        assert!(field.is_bound_to(&generator));
        assert!(!field.is_bound_to(&make_generator(109_081.776_8, 8)));
        assert!(!field.is_bound_to(&make_generator(1_737_400.0, 7)));

        let positive_x = CubeFace::PositiveX
            .direction([1.0, 0.25])
            .expect("edge direction");
        let negative_z = CubeFace::NegativeZ
            .direction([-1.0, 0.25])
            .expect("same edge direction");
        let positive_x_location = SurfaceLocation::new(positive_x);
        let negative_z_location = SurfaceLocation::new(negative_z);
        assert_eq!(
            positive_x_location.face_uv().0,
            negative_z_location.face_uv().0
        );
        assert_eq!(
            make_stencil(positive_x_location, 8, 32)
                .expect("valid stencil")
                .keys,
            make_stencil(negative_z_location, 8, 32)
                .expect("same global chart stencil")
                .keys
        );

        // A halo tap just beyond the +Z face chart must choose +X globally and
        // gather its edge nodes from that chart, outside the requesting patch.
        let crossed_face = SurfaceLocation::new(
            Direction3::try_new(DVec3::new(1.000_000_1, 0.25, 1.0)).expect("finite cross-face tap"),
        );
        let (global_face, global_uv) = crossed_face.face_uv();
        assert_eq!(global_face, CubeFace::PositiveX);
        let stencil = make_stencil(crossed_face, 8, 32).expect("global halo stencil");
        let n = 32_i64 * (1_i64 << 8);
        let chart =
            global_uv.map(|coordinate| ((coordinate + 1.0) * 0.5 * n as f64).clamp(0.0, n as f64));
        let base = chart.map(|coordinate| (coordinate.floor() as i64).min(n - 1));
        let [normal, u, v] = global_face.basis();
        let expected = [
            [base[0], base[1]],
            [base[0] + 1, base[1]],
            [base[0], base[1] + 1],
            [base[0] + 1, base[1] + 1],
        ]
        .map(|[x, y]| {
            let a = 2 * x - n;
            let b = 2 * y - n;
            canonical_node_key(std::array::from_fn(|axis| {
                (normal[axis] * n as f64 + u[axis] * a as f64 + v[axis] * b as f64) as i64
            }))
            .expect("nonzero global halo node")
        });
        assert_eq!(stencil.keys, expected);

        let center = SurfaceLocation::new(
            CubeFace::PositiveZ
                .direction([0.12, -0.18])
                .expect("interior direction"),
        );
        let other = SurfaceLocation::new(
            CubeFace::PositiveZ
                .direction([-0.31, 0.27])
                .expect("second interior direction"),
        );
        let requests = [center, other];
        let mut output = [DerivedSample {
            radius_m: 0.0,
            material_weights: [0.0; 4],
        }; 2];
        let mut context = generator.prepared_query_context();
        field
            .evaluate_batch(&generator, &mut context, 8, 32, &requests, &mut output)
            .expect("first derived batch");
        let first_stats = field.stats().expect("cache stats");
        let first_output = output;
        field
            .evaluate_batch(
                &generator,
                &mut context,
                8,
                32,
                &[other, center],
                &mut output,
            )
            .expect("reordered derived batch");
        let second_stats = field.stats().expect("cache stats");
        assert_eq!(output[0], first_output[1]);
        assert_eq!(output[1], first_output[0]);
        assert_eq!(second_stats.raw_node_queries, first_stats.raw_node_queries);
        assert!(second_stats.cache_hits > first_stats.cache_hits);
        assert!(second_stats.retained_cache_bytes <= 16 * 1024 * 1024);
        assert!(second_stats.last_batch_scratch_bytes <= second_stats.batch_scratch_bound_bytes);
    }

    #[test]
    fn seven_tap_reconstruction_is_compared_with_complete_surface_oracle() {
        let probes = [
            (CubeFace::PositiveZ, [0.13, -0.22]),
            (CubeFace::PositiveX, [0.72, -0.15]),
            (CubeFace::NegativeY, [-0.31, 0.48]),
            (CubeFace::PositiveZ, [0.9998, 0.13]),
        ];
        for radius_m in [109_081.776_8, 1_737_400.0] {
            let generator = make_generator(radius_m, 7);
            let field = SharedDerivedField::new(&generator).expect("bounded cache allocation");
            let mut context = generator.prepared_query_context();
            for level in [4u8, 8, 12, 16] {
                let mut max_radial_error_m = 0.0f64;
                let mut max_material_error = 0.0f64;
                for (face, uv) in probes {
                    let center = face.direction(uv).expect("interior center");
                    let n = center.unit();
                    let step = 2.0 / ((1u64 << level) as f64 * 32.0) * 0.25;
                    let mut locations = Vec::with_capacity(7);
                    locations.push(SurfaceLocation::new(center));
                    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                        let tangent = axis - n * n.dot(axis);
                        for sign in [1.0, -1.0] {
                            let direction =
                                Direction3::try_new((n + tangent * (step * sign)).normalize())
                                    .expect("finite filter direction");
                            locations.push(SurfaceLocation::new(direction));
                        }
                    }
                    let mut derived = [DerivedSample {
                        radius_m: 0.0,
                        material_weights: [0.0; 4],
                    }; 7];
                    field
                        .evaluate_batch(
                            &generator,
                            &mut context,
                            level,
                            32,
                            &locations,
                            &mut derived,
                        )
                        .expect("bounded seven-tap batch");
                    let mut exact_radius = 0.0;
                    let mut derived_radius = 0.0;
                    let mut exact_material = [0.0; 4];
                    let mut derived_material = [0.0; 4];
                    for (index, location) in locations.iter().copied().enumerate() {
                        let weight = if index == 0 { 0.25 } else { 0.125 };
                        let exact = generator
                            .evaluate_point(location)
                            .expect("complete oracle sample");
                        exact_radius += exact.radius_m() * weight;
                        derived_radius += derived[index].radius_m * weight;
                        for channel in 0..4 {
                            exact_material[channel] += exact.material_weights()[channel] * weight;
                            derived_material[channel] +=
                                derived[index].material_weights[channel] * weight;
                        }
                    }
                    max_radial_error_m =
                        max_radial_error_m.max((derived_radius - exact_radius).abs());
                    max_material_error = max_material_error.max(
                        exact_material
                            .iter()
                            .zip(derived_material)
                            .map(|(exact, derived)| (exact - derived).abs())
                            .fold(0.0, f64::max),
                    );
                }
                assert!(max_radial_error_m.is_finite());
                assert!(max_material_error.is_finite());
                if level == 16 {
                    assert!(
                        max_radial_error_m < 1.0,
                        "radius={radius_m} m, fine-level radial error={max_radial_error_m} m"
                    );
                    assert!(
                        max_material_error < 0.05,
                        "radius={radius_m} m, material-weight error={max_material_error}"
                    );
                }
                let stats = field.stats().expect("derived-field stats");
                assert!(stats.retained_cache_bytes <= stats.cache_capacity_bytes);
                assert!(stats.last_batch_scratch_bytes <= stats.batch_scratch_bound_bytes);
                println!(
                    "derived-oracle radius_m={radius_m:.4} level={level} probes={} max_radial_error_m={max_radial_error_m:.9} max_material_weight_error={max_material_error:.9} raw_queries={} cache_hits={} cache_misses={} evictions={} retained_cache_bytes={} cache_capacity_bytes={} scratch_bytes={} scratch_bound_bytes={}",
                    probes.len(),
                    stats.raw_node_queries,
                    stats.cache_hits,
                    stats.cache_misses,
                    stats.cache_evictions,
                    stats.retained_cache_bytes,
                    stats.cache_capacity_bytes,
                    stats.last_batch_scratch_bytes,
                    stats.batch_scratch_bound_bytes,
                );
            }
        }
    }
}
