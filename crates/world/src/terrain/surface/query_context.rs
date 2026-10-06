//! Bounded, tile-local memoization for immutable surface feature queries.
use super::{GeologicalControls, SurfaceGenerator, SurfaceSample, TerrainError};
use glam::DVec3;
use mundaris_math::surface::SurfaceLocation;

pub(super) const PROVINCE_FEATURE_FAMILY: u8 = 1;
pub(super) const HIERARCHICAL_FEATURE_FAMILY: u8 = 2;
const CACHE_CAPACITY: usize = 8192;

/// Compact immutable data derived from one geological feature cell.
#[derive(Debug, Clone, Copy)]
pub(super) struct CachedFeature {
    pub center: DVec3,
    pub key: u64,
    pub lineage_key: u64,
    pub edge_m: f64,
    pub level: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellKey {
    family: u8,
    band: u8,
    layout: u8,
    x: i64,
    y: i64,
    z: i64,
}

#[derive(Debug, Clone, Copy)]
struct CacheEntry {
    key: CellKey,
    feature: Option<CachedFeature>,
    controls: Option<GeologicalControls>,
}

/// Per-build cache statistics. Counts include direct-map collisions as misses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceQueryCacheStats {
    pub workspace_bytes: usize,
    pub feature_hits: u64,
    pub feature_misses: u64,
    pub controls_hits: u64,
    pub controls_misses: u64,
}

/// A reusable query path bound to exactly one immutable surface generator.
/// Its fixed direct-mapped table is safe to discard after a tile build.
#[derive(Debug)]
pub struct SurfaceQueryContext<'a> {
    generator: &'a SurfaceGenerator,
    entries: Box<[Option<CacheEntry>]>,
    stats: SurfaceQueryCacheStats,
}

impl<'a> SurfaceQueryContext<'a> {
    /// Maximum cache table payload allocated by one context, excluding allocator
    /// bookkeeping. One context is intended to belong to one worker or tile build.
    pub const fn workspace_bound_bytes() -> usize {
        CACHE_CAPACITY * std::mem::size_of::<Option<CacheEntry>>()
    }

    pub(super) fn new(generator: &'a SurfaceGenerator) -> Self {
        let entries = vec![None; CACHE_CAPACITY].into_boxed_slice();
        let stats = SurfaceQueryCacheStats {
            workspace_bytes: Self::workspace_bound_bytes(),
            ..SurfaceQueryCacheStats::default()
        };
        Self {
            generator,
            entries,
            stats,
        }
    }

    /// Evaluate through the bounded per-build cache. The generator borrow
    /// prevents accidentally reusing entries with another seed/configuration.
    pub fn evaluate_point(
        &mut self,
        location: SurfaceLocation,
    ) -> Result<SurfaceSample, TerrainError> {
        self.generator.evaluate_point_with_context(location, self)
    }

    pub fn stats(&self) -> SurfaceQueryCacheStats {
        self.stats
    }

    pub(super) fn feature<F>(
        &mut self,
        family: u8,
        band: usize,
        layout: usize,
        x: i64,
        y: i64,
        z: i64,
        create: F,
    ) -> Option<CachedFeature>
    where
        F: FnOnce() -> Option<CachedFeature>,
    {
        let key = CellKey {
            family,
            band: band as u8,
            layout: layout as u8,
            x,
            y,
            z,
        };
        let index = cell_hash(key) as usize & (self.entries.len() - 1);
        if let Some(entry) = self.entries[index]
            .as_ref()
            .filter(|entry| entry.key == key)
        {
            self.stats.feature_hits = self.stats.feature_hits.saturating_add(1);
            return entry.feature;
        }
        self.stats.feature_misses = self.stats.feature_misses.saturating_add(1);
        let feature = create();
        self.entries[index] = Some(CacheEntry {
            key,
            feature,
            controls: None,
        });
        feature
    }

    pub(super) fn controls<F>(
        &mut self,
        family: u8,
        band: usize,
        layout: usize,
        x: i64,
        y: i64,
        z: i64,
        create: F,
    ) -> Result<GeologicalControls, TerrainError>
    where
        F: FnOnce() -> Result<GeologicalControls, TerrainError>,
    {
        let key = CellKey {
            family,
            band: band as u8,
            layout: layout as u8,
            x,
            y,
            z,
        };
        let index = cell_hash(key) as usize & (self.entries.len() - 1);
        if let Some(entry) = self.entries[index]
            .as_ref()
            .filter(|entry| entry.key == key)
        {
            if let Some(controls) = entry.controls {
                self.stats.controls_hits = self.stats.controls_hits.saturating_add(1);
                return Ok(controls);
            }
        }
        self.stats.controls_misses = self.stats.controls_misses.saturating_add(1);
        let controls = create()?;
        if let Some(entry) = self.entries[index]
            .as_mut()
            .filter(|entry| entry.key == key)
        {
            entry.controls = Some(controls);
        }
        Ok(controls)
    }
}

fn cell_hash(key: CellKey) -> u64 {
    let mut value = (key.x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (key.y as u64).rotate_left(21)
        ^ (key.z as u64).rotate_left(43)
        ^ (u64::from(key.family) << 56)
        ^ (u64::from(key.band) << 48)
        ^ (u64::from(key.layout) << 40);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
