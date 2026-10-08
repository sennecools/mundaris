//! Bounded, tile-local memoization for immutable surface feature queries.
use super::{GeologicalControls, SurfaceGenerator, SurfaceSample, TerrainError};
use glam::DVec3;
use mundaris_math::surface::SurfaceLocation;
use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(super) const PROVINCE_FEATURE_FAMILY: u8 = 1;
pub(super) const HIERARCHICAL_FEATURE_FAMILY: u8 = 2;
const CACHE_CAPACITY: usize = 8192;
pub(super) const PREPARED_STORE_CAP_BYTES: usize = 32 * 1024 * 1024;
const PREPARED_LEASE_CAP_BYTES: usize = 256 * 1024;
const PREPARED_LEASE_SLOTS: usize = 64;
const PREPARED_STORE_SLOTS: usize = 4096;

/// Complete key inside one generator-owned store. The owning generator pins
/// definition, seed, radius, family and configuration for this key's lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PreparedPageKey {
    pub family: u8,
    pub scale: u8,
    pub layout: u8,
    pub cell: [i64; 3],
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfacePreparationStoreStats {
    pub capacity_bytes: usize,
    pub retained_bytes: usize,
    pub high_water_bytes: usize,
    pub transient_build_reservation_bytes: usize,
    pub pages_built: u64,
    pub page_hits: u64,
    pub page_misses: u64,
    pub page_evictions: u64,
    pub capacity_fallbacks: u64,
    pub prepared_cells: u64,
}

/// Aggregate CPU spans for sampled or batch-level generation diagnostics.
/// `legacy_history_ns` is inclusive of crater descriptor discovery and
/// position-dependent crater profile/gradient work. Province/hierarchy values
/// are exclusive of their nested parent/history evaluations.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceQueryProfileStats {
    pub samples: u64,
    pub surface_total_ns: u64,
    pub shape_ns: u64,
    pub geology_ns: u64,
    pub legacy_history_ns: u64,
    pub legacy_discovery_and_profile_ns: u64,
    pub legacy_chronology_ns: u64,
    pub province_exclusive_ns: u64,
    pub hierarchy_exclusive_ns: u64,
    pub final_material_normal_ns: u64,
}

#[derive(Debug, Default)]
struct StoreCounters {
    retained: AtomicUsize,
    high_water: AtomicUsize,
    pages_built: AtomicU64,
    page_hits: AtomicU64,
    page_misses: AtomicU64,
    page_evictions: AtomicU64,
    capacity_fallbacks: AtomicU64,
    transient_high_water: AtomicUsize,
    prepared_cells: AtomicU64,
}

struct PreparedPage {
    payload: Arc<dyn Any + Send + Sync>,
    bytes: usize,
    counters: Arc<StoreCounters>,
}

impl Drop for PreparedPage {
    fn drop(&mut self) {
        self.counters
            .retained
            .fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

struct PreparedStoreSlot {
    key: PreparedPageKey,
    page: Arc<PreparedPage>,
    last_use: u64,
}

struct PreparedStoreState {
    pages: Vec<Option<PreparedStoreSlot>>,
    tick: u64,
}

/// Generator-shared bounded immutable pages. Construction happens under the
/// store lock on misses, coalescing concurrent requests for the same key; page
/// evaluation happens only after the lock has been released.
pub(super) struct SurfacePreparationStore {
    state: Mutex<PreparedStoreState>,
    counters: Arc<StoreCounters>,
    fixed_bytes: usize,
}

impl std::fmt::Debug for SurfacePreparationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfacePreparationStore")
            .field("stats", &self.stats())
            .finish()
    }
}

impl SurfacePreparationStore {
    pub(super) fn new() -> Option<Self> {
        let mut pages = Vec::new();
        pages.try_reserve_exact(PREPARED_STORE_SLOTS).ok()?;
        if pages.capacity() > PREPARED_STORE_SLOTS {
            return None;
        }
        pages.resize_with(PREPARED_STORE_SLOTS, || None);
        let fixed_bytes = std::mem::size_of::<Self>()
            + std::mem::size_of::<StoreCounters>()
            + 32
            + pages.capacity() * std::mem::size_of::<Option<PreparedStoreSlot>>();
        if fixed_bytes >= PREPARED_STORE_CAP_BYTES {
            return None;
        }
        Some(Self {
            state: Mutex::new(PreparedStoreState { pages, tick: 0 }),
            counters: Arc::new(StoreCounters::default()),
            fixed_bytes,
        })
    }

    fn get_or_build<T, F>(
        &self,
        key: PreparedPageKey,
        build_reservation_bytes: usize,
        build: F,
    ) -> Option<(Arc<T>, Arc<PreparedPage>, u64)>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> (T, usize, u64),
    {
        let mut state = self.state.lock().ok()?;
        state.tick = state.tick.wrapping_add(1);
        let tick = state.tick;
        let index = prepared_key_hash(key) as usize % state.pages.len();
        if let Some(slot) = state.pages[index].as_mut().filter(|slot| slot.key == key) {
            slot.last_use = tick;
            self.counters.page_hits.fetch_add(1, Ordering::Relaxed);
            return slot
                .page
                .payload
                .clone()
                .downcast::<T>()
                .ok()
                .map(|typed| (typed, Arc::clone(&slot.page), 0));
        }
        self.counters.page_misses.fetch_add(1, Ordering::Relaxed);

        if build_reservation_bytes == 0
            || build_reservation_bytes > PREPARED_STORE_CAP_BYTES - self.fixed_bytes
        {
            self.counters
                .capacity_fallbacks
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        while self.fixed_bytes
            + self.counters.retained.load(Ordering::Relaxed)
            + build_reservation_bytes
            > PREPARED_STORE_CAP_BYTES
        {
            let Some(oldest) = state
                .pages
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| {
                    slot.as_ref()
                        .filter(|slot| Arc::strong_count(&slot.page) == 1)
                        .map(|slot| (index, slot.last_use))
                })
                .min_by_key(|(_, last_use)| *last_use)
                .map(|(index, _)| index)
            else {
                self.counters
                    .capacity_fallbacks
                    .fetch_add(1, Ordering::Relaxed);
                return None;
            };
            if let Some(slot) = state.pages[oldest].take() {
                self.counters.page_evictions.fetch_add(1, Ordering::Relaxed);
                drop(slot);
            }
        }

        let retained_before_build = self.counters.retained.load(Ordering::Relaxed);
        self.counters
            .transient_high_water
            .fetch_max(build_reservation_bytes, Ordering::Relaxed);
        self.counters.high_water.fetch_max(
            self.fixed_bytes + retained_before_build + build_reservation_bytes,
            Ordering::Relaxed,
        );

        if let Some(slot) = state.pages[index].take() {
            self.counters.page_evictions.fetch_add(1, Ordering::Relaxed);
            drop(slot);
        }
        let (payload, payload_bytes, prepared_cells) = build();
        let bytes = payload_bytes.saturating_add(std::mem::size_of::<PreparedPage>() + 32);
        if payload_bytes == 0
            || bytes > build_reservation_bytes
            || self.fixed_bytes + self.counters.retained.load(Ordering::Relaxed) + bytes
                > PREPARED_STORE_CAP_BYTES
        {
            self.counters
                .capacity_fallbacks
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let previous = self.counters.retained.fetch_add(bytes, Ordering::Relaxed);
        let retained_high_water = previous + bytes;
        self.counters
            .high_water
            .fetch_max(retained_high_water + self.fixed_bytes, Ordering::Relaxed);
        self.counters.pages_built.fetch_add(1, Ordering::Relaxed);
        self.counters
            .prepared_cells
            .fetch_add(prepared_cells, Ordering::Relaxed);
        let page = Arc::new(PreparedPage {
            payload: Arc::new(payload),
            bytes,
            counters: Arc::clone(&self.counters),
        });
        let typed = page.payload.clone().downcast::<T>().ok()?;
        state.pages[index] = Some(PreparedStoreSlot {
            key,
            page: Arc::clone(&page),
            last_use: tick,
        });
        Some((typed, page, prepared_cells))
    }

    pub(super) fn stats(&self) -> SurfacePreparationStoreStats {
        SurfacePreparationStoreStats {
            capacity_bytes: PREPARED_STORE_CAP_BYTES,
            retained_bytes: self.fixed_bytes + self.counters.retained.load(Ordering::Relaxed),
            high_water_bytes: self.counters.high_water.load(Ordering::Relaxed),
            transient_build_reservation_bytes: self
                .counters
                .transient_high_water
                .load(Ordering::Relaxed),
            pages_built: self.counters.pages_built.load(Ordering::Relaxed),
            page_hits: self.counters.page_hits.load(Ordering::Relaxed),
            page_misses: self.counters.page_misses.load(Ordering::Relaxed),
            page_evictions: self.counters.page_evictions.load(Ordering::Relaxed),
            capacity_fallbacks: self.counters.capacity_fallbacks.load(Ordering::Relaxed),
            prepared_cells: self.counters.prepared_cells.load(Ordering::Relaxed),
        }
    }
}

fn prepared_key_hash(key: PreparedPageKey) -> u64 {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone)]
struct LocalPreparedLease {
    key: PreparedPageKey,
    bytes: usize,
    page: Arc<PreparedPage>,
}

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
pub(super) struct CellKey {
    family: u8,
    band: u8,
    layout: u8,
    x: i64,
    y: i64,
    z: i64,
}

impl CellKey {
    pub(super) const fn new(
        family: u8,
        band: usize,
        layout: usize,
        x: i64,
        y: i64,
        z: i64,
    ) -> Self {
        Self {
            family,
            band: band as u8,
            layout: layout as u8,
            x,
            y,
            z,
        }
    }
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
    pub prepared_page_hits: u64,
    pub prepared_page_misses: u64,
    pub prepared_page_lease_acquisitions: u64,
    pub prepared_cells: u64,
    /// Position-dependent evaluation of descriptors already organized into
    /// prepared windows. One-time descriptor-cell setup is `prepared_cells`.
    pub candidate_discoveries: u64,
    pub sample_evaluations: u64,
    pub preparation_bytes: u64,
}

/// A reusable query path bound to exactly one immutable surface generator.
/// Its fixed direct-mapped table is safe to discard after a tile build.
pub struct SurfaceQueryContext<'a> {
    generator: &'a SurfaceGenerator,
    entries: Box<[Option<CacheEntry>]>,
    stats: SurfaceQueryCacheStats,
    prepared_store: Option<Arc<SurfacePreparationStore>>,
    leases: [Option<LocalPreparedLease>; PREPARED_LEASE_SLOTS],
    lease_bytes: usize,
    profile_enabled: bool,
    local_cache_enabled: bool,
    profile: SurfaceQueryProfileStats,
}

impl<'a> SurfaceQueryContext<'a> {
    /// Maximum cache table payload allocated by one context, excluding allocator
    /// bookkeeping. One context is intended to belong to one worker or tile build.
    pub const fn workspace_bound_bytes() -> usize {
        CACHE_CAPACITY * std::mem::size_of::<Option<CacheEntry>>()
    }

    pub const fn prepared_lease_bound_bytes() -> usize {
        PREPARED_LEASE_CAP_BYTES
    }

    pub(super) fn new(generator: &'a SurfaceGenerator) -> Self {
        Self::new_with_store(generator, None)
    }

    pub(super) fn new_prepared(
        generator: &'a SurfaceGenerator,
        store: Arc<SurfacePreparationStore>,
    ) -> Self {
        Self::new_with_store(generator, Some(store))
    }

    fn new_with_store(
        generator: &'a SurfaceGenerator,
        prepared_store: Option<Arc<SurfacePreparationStore>>,
    ) -> Self {
        let entries = vec![None; CACHE_CAPACITY].into_boxed_slice();
        let lease_table_bytes =
            std::mem::size_of::<[Option<LocalPreparedLease>; PREPARED_LEASE_SLOTS]>();
        let stats = SurfaceQueryCacheStats {
            workspace_bytes: Self::workspace_bound_bytes()
                + prepared_store
                    .as_ref()
                    .map_or(0, |_| lease_table_bytes + PREPARED_LEASE_CAP_BYTES),
            ..SurfaceQueryCacheStats::default()
        };
        Self {
            generator,
            entries,
            stats,
            prepared_store,
            leases: std::array::from_fn(|_| None),
            lease_bytes: 0,
            profile_enabled: false,
            local_cache_enabled: true,
            profile: SurfaceQueryProfileStats::default(),
        }
    }

    pub fn is_bound_to(&self, generator: &SurfaceGenerator) -> bool {
        std::ptr::eq(self.generator, generator)
    }

    pub fn total_workspace_bound_bytes(&self) -> usize {
        Self::workspace_bound_bytes()
            .saturating_add(std::mem::size_of::<
                [Option<LocalPreparedLease>; PREPARED_LEASE_SLOTS],
            >())
            .saturating_add(PREPARED_LEASE_CAP_BYTES)
    }

    pub fn lease_bound_bytes(&self) -> usize {
        PREPARED_LEASE_CAP_BYTES
    }

    pub fn leased_bytes(&self) -> usize {
        self.lease_bytes
    }

    pub fn store_stats(&self) -> SurfacePreparationStoreStats {
        self.prepared_store
            .as_ref()
            .map_or_else(SurfacePreparationStoreStats::default, |store| store.stats())
    }

    pub(crate) fn is_prepared(&self) -> bool {
        self.prepared_store.is_some()
    }

    pub(crate) fn prepared_page<T, F>(
        &mut self,
        slot: usize,
        key: PreparedPageKey,
        build_reservation_bytes: usize,
        build: F,
    ) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
        F: FnOnce() -> (T, usize, u64),
    {
        if slot >= PREPARED_LEASE_SLOTS {
            return None;
        }
        if let Some(lease) = self.leases[slot].as_ref().filter(|lease| lease.key == key) {
            self.stats.prepared_page_hits = self.stats.prepared_page_hits.saturating_add(1);
            return lease.page.payload.clone().downcast::<T>().ok();
        }
        let store = self.prepared_store.as_ref()?;
        self.stats.prepared_page_misses = self.stats.prepared_page_misses.saturating_add(1);
        let (typed, page, prepared_cells) =
            store.get_or_build::<T, _>(key, build_reservation_bytes, build)?;
        let bytes = page.bytes;
        let existing_bytes = self.leases[slot].as_ref().map_or(0, |lease| lease.bytes);
        if bytes > PREPARED_LEASE_CAP_BYTES
            || self
                .lease_bytes
                .saturating_sub(existing_bytes)
                .saturating_add(bytes)
                > PREPARED_LEASE_CAP_BYTES
        {
            return None;
        }
        if let Some(previous) = self.leases[slot].take() {
            self.lease_bytes = self.lease_bytes.saturating_sub(previous.bytes);
        }
        self.leases[slot] = Some(LocalPreparedLease { key, bytes, page });
        self.lease_bytes = self.lease_bytes.saturating_add(bytes);
        self.stats.prepared_page_lease_acquisitions = self
            .stats
            .prepared_page_lease_acquisitions
            .saturating_add(1);
        self.stats.prepared_cells = self.stats.prepared_cells.saturating_add(prepared_cells);
        self.stats.preparation_bytes = self.stats.preparation_bytes.saturating_add(bytes as u64);
        Some(typed)
    }

    pub fn preparation_stats(&self) -> SurfaceQueryCacheStats {
        self.stats
    }

    pub fn enable_profile(&mut self, enabled: bool) {
        self.profile_enabled = enabled;
        if !enabled {
            self.profile = SurfaceQueryProfileStats::default();
        }
    }

    pub(super) fn disable_local_cache(&mut self) {
        self.local_cache_enabled = false;
    }

    pub fn profile_stats(&self) -> SurfaceQueryProfileStats {
        self.profile
    }

    pub(crate) fn profiling(&self) -> bool {
        self.profile_enabled
    }

    pub(crate) fn profile_mut(&mut self) -> Option<&mut SurfaceQueryProfileStats> {
        self.profile_enabled.then_some(&mut self.profile)
    }

    pub(super) fn record_surface_profile(
        &mut self,
        shape_ns: u64,
        geology_ns: u64,
        material_normal_ns: u64,
        total_ns: u64,
    ) {
        self.stats.sample_evaluations = self.stats.sample_evaluations.saturating_add(1);
        if !self.profile_enabled {
            return;
        }
        self.profile.samples = self.profile.samples.saturating_add(1);
        self.profile.shape_ns = self.profile.shape_ns.saturating_add(shape_ns);
        self.profile.geology_ns = self.profile.geology_ns.saturating_add(geology_ns);
        self.profile.final_material_normal_ns = self
            .profile
            .final_material_normal_ns
            .saturating_add(material_normal_ns);
        self.profile.surface_total_ns = self.profile.surface_total_ns.saturating_add(total_ns);
    }

    pub(crate) fn record_legacy_profile(
        &mut self,
        total_ns: u64,
        discovery_and_profile_ns: u64,
        chronology_ns: u64,
    ) {
        if let Some(profile) = self.profile_mut() {
            profile.legacy_history_ns = profile.legacy_history_ns.saturating_add(total_ns);
            profile.legacy_discovery_and_profile_ns = profile
                .legacy_discovery_and_profile_ns
                .saturating_add(discovery_and_profile_ns);
            profile.legacy_chronology_ns =
                profile.legacy_chronology_ns.saturating_add(chronology_ns);
        }
    }

    pub(crate) fn legacy_history_elapsed_ns(&self) -> u64 {
        self.profile.legacy_history_ns
    }

    pub(crate) fn record_province_exclusive(&mut self, elapsed_ns: u64) {
        if let Some(profile) = self.profile_mut() {
            profile.province_exclusive_ns =
                profile.province_exclusive_ns.saturating_add(elapsed_ns);
        }
    }

    pub(crate) fn record_hierarchy_exclusive(&mut self, elapsed_ns: u64) {
        if let Some(profile) = self.profile_mut() {
            profile.hierarchy_exclusive_ns =
                profile.hierarchy_exclusive_ns.saturating_add(elapsed_ns);
        }
    }

    pub(crate) fn record_candidate_discoveries(&mut self, count: u64) {
        self.stats.candidate_discoveries = self.stats.candidate_discoveries.saturating_add(count);
    }

    /// Evaluate through the bounded per-build cache. The generator borrow
    /// prevents accidentally reusing entries with another seed/configuration.
    pub fn evaluate_point(
        &mut self,
        location: SurfaceLocation,
    ) -> Result<SurfaceSample, TerrainError> {
        self.generator.evaluate_point_with_context(location, self)
    }

    /// Evaluate a whole sample batch through the same persistent worker context.
    /// Prepared pages are acquired per spatial window and reused across adjacent
    /// positions; output order always matches input order.
    pub fn evaluate_batch(
        &mut self,
        locations: &[SurfaceLocation],
        output: &mut [SurfaceSample],
    ) -> Result<(), TerrainError> {
        if locations.len() != output.len() {
            return Err(TerrainError::LengthMismatch);
        }
        for (location, sample) in locations.iter().zip(output) {
            *sample = self.evaluate_point(*location)?;
        }
        Ok(())
    }

    pub fn stats(&self) -> SurfaceQueryCacheStats {
        self.stats
    }

    pub(super) fn feature<F>(&mut self, key: CellKey, create: F) -> Option<CachedFeature>
    where
        F: FnOnce() -> Option<CachedFeature>,
    {
        let index = cell_hash(key) as usize & (self.entries.len() - 1);
        if !self.local_cache_enabled {
            self.stats.feature_misses = self.stats.feature_misses.saturating_add(1);
            return create();
        }
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
        key: CellKey,
        create: F,
    ) -> Result<GeologicalControls, TerrainError>
    where
        F: FnOnce() -> Result<GeologicalControls, TerrainError>,
    {
        let index = cell_hash(key) as usize & (self.entries.len() - 1);
        if !self.local_cache_enabled {
            self.stats.controls_misses = self.stats.controls_misses.saturating_add(1);
            return create();
        }
        if let Some(entry) = self.entries[index]
            .as_ref()
            .filter(|entry| entry.key == key)
            && let Some(controls) = entry.controls
        {
            self.stats.controls_hits = self.stats.controls_hits.saturating_add(1);
            return Ok(controls);
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
