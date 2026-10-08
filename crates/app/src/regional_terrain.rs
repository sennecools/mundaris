//! Bounded regional terrain selection, CPU tile reuse, and asynchronous builds.
//!
//! This layer deliberately keeps desired patches, CPU payloads, GPU-resident
//! patches, and the published draw cover as separate sets. Projected error is
//! an explicitly approximate selector: it combines a grid-cell spherical
//! sagitta term with one quarter of the authored geological relief fraction,
//! scaled by patch angular width. Proximity uses the closest point on the
//! angular patch footprint and its local relief envelope instead of the global
//! height bound, which would make distant tiles appear uniformly close. This
//! local residual proxy is deterministic but does not certify unsampled error.

mod coverage;

pub use coverage::{
    CoverageAddressWitness, CoverageCubeFace, CoverageDrawableAddress, ScreenCoverageCell,
    ScreenCoverageDiagnostic,
};

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use glam::{DMat3, DVec3};
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_renderer::CelestialProjection;
use mundaris_renderer::planet_surface::{PatchMetadata, SurfaceExtent, SurfaceTopology};
use mundaris_world::terrain::{SurfaceGenerator, SurfacePreparationStoreStats};
use serde::Serialize;

const BUILD_CANDIDATE_STARVATION_AFTER: Duration = Duration::from_secs(2);
const USEFUL_DETAIL_FLOOR_ERROR_PX: f64 = 4.0;
const PUBLICATION_STARVATION_AGE_TICKS: u64 = 32;
const MAX_SUPPORTED_TILE_CELLS: usize = 128;
const TILE_FILTER_TAPS_PER_ROW_SAMPLE: usize = 7;
const MAX_TILE_ROW_SAMPLES: usize =
    (MAX_SUPPORTED_TILE_CELLS + 3) * TILE_FILTER_TAPS_PER_ROW_SAMPLE;

use crate::resident_terrain::{
    ResidentTileBuilder, SharedDerivedField, TileBuildDiagnostics, TileBuildIdentity, TileData,
    TileKey,
};
use crate::terrain_trace::{BlockReason, Stage, TerrainTrace};

/// Prototype limits for one regional selector/scheduler instance.
#[derive(Debug, Clone)]
pub struct RegionalConfig {
    pub roots: Vec<CubePatchAddress>,
    pub max_level: u8,
    pub cells: u32,
    pub cpu_tile_cap: usize,
    pub cpu_byte_cap: usize,
    pub worker_count: usize,
    pub worker_delay: Duration,
    pub queue_cap: usize,
    pub completion_cap: usize,
    pub admission_cap_per_tick: usize,
    pub max_desired_patches: usize,
    pub upload_tile_cap: usize,
    pub upload_byte_cap: usize,
    pub publication_cap_per_tick: usize,
    pub transition_cap: usize,
    pub split_threshold_px: f64,
    pub merge_threshold_px: f64,
    pub prediction_seconds: f64,
    pub high_speed_mps: f64,
}

impl Default for RegionalConfig {
    fn default() -> Self {
        Self {
            roots: vec![CubePatchAddress::root(CubeFace::PositiveZ)],
            max_level: 8,
            cells: 32,
            cpu_tile_cap: 96,
            cpu_byte_cap: 64 * 1024 * 1024,
            worker_count: 4,
            worker_delay: Duration::ZERO,
            queue_cap: 32,
            completion_cap: 8,
            admission_cap_per_tick: 4,
            max_desired_patches: 512,
            upload_tile_cap: 4,
            upload_byte_cap: 8 * 1024 * 1024,
            publication_cap_per_tick: 4,
            transition_cap: 8,
            split_threshold_px: 0.15,
            merge_threshold_px: 0.075,
            prediction_seconds: 0.75,
            high_speed_mps: 250.0,
        }
    }
}

/// Body-fixed observer state consumed by regional selection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalView {
    pub body_position_m: DVec3,
    pub body_velocity_mps: DVec3,
    /// Focal/projection scale in pixels.
    pub projection_scale_px: f64,
}

#[derive(Clone, Copy)]
struct PlanetaryView {
    body_to_view: DMat3,
    projection: CelestialProjection,
}

impl PlanetaryView {
    fn matches(self, other: Option<Self>) -> bool {
        let Some(other) = other else { return false };
        // Re-expression of a stationary inspection basis can accumulate only
        // f64 roundoff. This tolerance is below 1e-10 pixels at ordinary display
        // scales; actual draw transforms retain their original precision.
        self.body_to_view
            .to_cols_array()
            .iter()
            .zip(other.body_to_view.to_cols_array())
            .all(|(a, b)| (*a - b).abs() <= 1.0e-14)
            && self.projection.viewport() == other.projection.viewport()
            && self.projection.origin() == other.projection.origin()
            && self.projection.near_m() == other.projection.near_m()
            && self.projection.vertical_fov_rad() == other.projection.vertical_fov_rad()
    }
}

/// One immutable CPU tile admitted to the app/runtime upload queue.
#[derive(Debug, Clone)]
pub struct RegionalUpload {
    pub address: CubePatchAddress,
    pub tile: Arc<TileData>,
    pub bytes: usize,
}

/// A local resident replacement that preserves a complete balanced cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionalPublication {
    Split {
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        cover: Vec<CubePatchAddress>,
    },
    Merge {
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        cover: Vec<CubePatchAddress>,
    },
}

impl RegionalPublication {
    pub fn cover(&self) -> &[CubePatchAddress] {
        match self {
            Self::Split { cover, .. } | Self::Merge { cover, .. } => cover,
        }
    }
}

/// Typed diagnostics; address strings retain stable `Debug` spelling without
/// adding serialization to the shared math address type.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalSnapshot {
    pub tick: u64,
    pub configuration: RegionalConfigSnapshot,
    pub desired: Vec<RegionalPatchSnapshot>,
    pub resident: Vec<String>,
    pub drawable: Vec<String>,
    pub upload_queue: Vec<RegionalQueueSnapshot>,
    pub build_requests: Vec<RegionalRequestSnapshot>,
    pub desired_count: usize,
    pub resident_count: usize,
    pub drawable_count: usize,
    pub cpu_cached_tiles: usize,
    pub cpu_cached_bytes: usize,
    pub worker_queued: usize,
    pub worker_outstanding: usize,
    pub worker_running: usize,
    pub completion_backlog: usize,
    pub completion_drain_micros: u64,
    pub selection_time_micros: u64,
    /// Number of selector ticks whose measured wall time exceeded 2 ms.
    pub selection_budget_overruns: u64,
    /// True when planetary adaptive selection stopped at its 2 ms deadline.
    pub selection_budget_exhausted: bool,
    pub scheduler_time_micros: u64,
    pub publication_discovery_micros: u64,
    pub frontier_discovery_micros: u64,
    pub cpu_tile_cap: usize,
    pub cpu_byte_cap: usize,
    pub max_desired_patches: usize,
    pub worker_queue_capacity: usize,
    pub completion_capacity: usize,
    pub upload_tile_capacity: usize,
    pub upload_byte_capacity: usize,
    /// Configured capacity estimate, not live RSS. Includes bounded worker
    /// query/builder buffers and shared prepared/derived caches; excludes allocator overhead.
    pub estimated_worker_scratch_bytes: usize,
    /// Aggregate maximum row buffers: 917 locations plus the active exact or
    /// derived sample type, multiplied by the configured worker count.
    pub tile_builder_row_workspace_bytes: usize,
    /// Aggregate builder-reported fixed stack estimate per worker; it is an
    /// estimate, not a platform stack high-water measurement.
    pub tile_builder_stack_scratch_bytes: usize,
    pub derived_generation_workspace_bytes: usize,
    /// Fixed worker query contexts plus the shared prepared-page store bound.
    pub prepared_generation_workspace_bytes: usize,
    pub prepared_store_capacity_bytes: usize,
    pub prepared_store_retained_bytes: usize,
    pub prepared_store_high_water_bytes: usize,
    pub prepared_store_transient_build_reservation_bytes: usize,
    pub derived_field_cache_capacity_bytes: usize,
    pub derived_field_cache_retained_bytes: usize,
    pub derived_batch_scratch_bound_bytes: usize,
    /// Sample evaluations spent in build attempts that ended by cancellation.
    pub cancelled_attempt_sample_evaluations: u64,
    /// Preparation bytes built during canceled attempts; pages remain reusable.
    pub cancelled_attempt_preparation_bytes: u64,
    /// Current desired-ancestor priority entries retained for frontier discovery.
    pub frontier_priority_index_entries: usize,
    /// Conservative configured upper bound for the index and exact-key signature.
    pub frontier_priority_cache_bytes_upper_bound: usize,
    /// Current missing eligible generation candidates tracked for fairness.
    pub build_candidate_wait_entries: usize,
    /// Config/dependency-derived upper bound for tracked candidate entries.
    pub build_candidate_wait_entries_upper_bound: usize,
    /// Conservative allocation bound, including exact-key definition words.
    pub build_candidate_wait_bytes_upper_bound: usize,
    pub upload_backlog_tiles: usize,
    pub upload_backlog_bytes: usize,
    /// Estimate of completed desired CPU tiles awaiting upload or residency.
    /// This is derived from the bounded CPU cache and excludes queued uploads.
    pub estimated_completed_unpublished_bytes: usize,
    pub publication_candidates: usize,
    /// Oldest currently eligible publication proposal, in selector ticks.
    pub publication_backlog_age_ticks: u64,
    pub active_transitions_reported: usize,
    /// Root-area-normalized area-weighted mean projected error, in pixels.
    pub desired_projected_error_px: f64,
    /// Root-area-normalized area-weighted mean projected error of drawable cover.
    pub drawable_projected_error_px: f64,
    /// Positive excess drawable mean error over desired mean error, in pixels.
    pub refinement_debt: f64,
    pub queue_pressure: bool,
    pub cpu_cache_pressure: bool,
    pub upload_pressure: bool,
    pub publication_pressure: bool,
    pub desired_capacity_pressure: bool,
    /// True only when every visible desired patch is below its refinement
    /// threshold and the selector is not constrained by the desired cap.
    pub target_quality_reached: bool,
    /// Drawable patches with an available approximate proxy score.
    pub visible_drawable_proxy_count: usize,
    /// Drawable patches missing an approximate proxy score.
    pub missing_drawable_proxy_score_count: usize,
    pub visible_desired_counterpart_count: usize,
    /// Existing unweighted summaries use positive-error scores from all drawable patches.
    pub visible_drawable_proxy_error_max_px: f64,
    pub visible_drawable_proxy_error_p95_px: f64,
    pub visible_drawable_proxy_error_over_1px: usize,
    pub visible_drawable_proxy_error_over_split_threshold: usize,
    /// Largest scored positive-error proxy among all drawable patches; visibility is
    /// not applied, and missing drawable scores can make this incomplete.
    pub worst_proxy: Option<RegionalProxyErrorSnapshot>,
    /// Largest visible drawable proxy above the split threshold. Missing visible
    /// proxy scores keep this unset because the worst unresolved patch is unknown.
    pub worst_visible_unresolved: Option<RegionalProxyErrorSnapshot>,
    /// At most eight visible drawable patches, sorted by error descending then address.
    pub visible_drawable_proxy_hotspots: Vec<RegionalDrawableProxyHotspotSnapshot>,
    /// Projected-area-weighted selector convergence ratio in [0, 1], where 1
    /// means every visible drawable proxy meets `convergence_target_error_px`.
    /// This selector measure remains uncertified.
    pub visible_convergence: Option<f64>,
    /// Center-screen drawable proxy convergence ratio in [0, 1]; None when the
    /// center ray misses the body or drawable evidence is missing. Uncertified.
    pub center_screen_convergence: Option<f64>,
    /// Center-screen drawable projected proxy error in pixels.
    pub center_screen_error_px: Option<f64>,
    /// Split threshold used to normalize both convergence ratios.
    pub convergence_target_error_px: f64,
    /// Worst visible drawable proxy error above the target. None when evidence is missing.
    pub worst_visible_unresolved_error_px: Option<f64>,
    pub useful_detail_reached: bool,
    pub useful_detail_proxy_error_threshold_px: f64,
    /// Cached 3 × 3 reference-sphere mask for the exact current view and cover.
    pub screen_coverage: Option<ScreenCoverageDiagnostic>,
    /// Three diagnostic instances during cache/snapshot/transient overlap,
    /// including inline vector headers and bounded witness payloads. Shared
    /// sampled-area map storage is excluded.
    pub screen_coverage_diagnostic_overlap_bytes_upper_bound: usize,
    pub visible_proxy_error_certified: bool,
    pub visible_proxy_error_model: &'static str,
    /// Whole-cover split/merge operations applied by the planetary selector
    /// during the last tick. Finite-region fixtures report zero.
    pub planetary_topology_operations: usize,
    /// Cover leaves scored for planetary refinement during the most recent tick.
    pub planetary_split_candidates_scanned: usize,
    pub selector_fixed_point_reused: bool,
    /// No scheduler admission was needed for unchanged, fully resident coverage.
    pub scheduler_fixed_point_reused: bool,
    pub requests_issued: u64,
    pub jobs_started: u64,
    /// Successful CPU tile builder completions, including results discarded as stale.
    pub completed_build_count: u64,
    pub cancelled_before_start: u64,
    pub cancelled_during_work: u64,
    pub stale_completions: u64,
    pub completed_unused_tiles: u64,
    /// Bytes from a CPU build instance discarded before GPU-residency
    /// acknowledgement. Cached instances still pending publication are excluded.
    pub bytes_built_but_unused: u64,
    /// Builder-reported kernel time for CPU build instances discarded before
    /// any successful GPU-residency acknowledgement. Excludes artificial delay.
    pub build_time_discarded_micros: u64,
    pub build_time_micros: u64,
    pub worker_elapsed_micros: u64,
    /// Bounded rolling sample of completed worker elapsed time, in microseconds.
    pub recent_build_elapsed_micros: Vec<u64>,
    pub build_failures: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_evictions: u64,
    pub rebuilds: u64,
    pub reuploads: u64,
    pub last_admitted_priorities: Vec<RegionalPrioritySnapshot>,
    pub selector_error_model: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalProxyErrorSnapshot {
    pub address: String,
    pub error_px: f64,
    /// Approximate projected pixel-area weight; not a certified screen-space bound.
    pub projected_footprint_weight: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalTileKeySnapshot {
    pub body_identity: u64,
    pub definition_words: Vec<u64>,
    pub radius_bits: u64,
    pub surface_revision: u64,
    pub material_revision: u64,
    pub format_version: u32,
    pub filter_version: u32,
    pub cells: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalDrawableProxyHotspotSnapshot {
    pub address: String,
    pub error_px: f64,
    /// Approximate projected pixel-area weight; not a certified screen-space bound.
    pub projected_footprint_weight: f64,
    pub key: Option<RegionalTileKeySnapshot>,
    pub parent: Option<String>,
    pub desired: bool,
    pub resident: bool,
    pub cpu_cached: bool,
    pub upload_queued: bool,
    pub in_flight: bool,
    pub in_flight_cancelled: bool,
    pub publication_parent_blocked: bool,
    pub upload_allowlisted: Option<bool>,
    /// Local state only; "none_observed" does not rule out blockers elsewhere.
    pub blocker_status: &'static str,
}

/// Settings that materially determine a regional terrain snapshot. Runtime
/// captures include these values so worker-pressure comparisons are explicit.
#[derive(Debug, Clone, Serialize)]
pub struct RegionalConfigSnapshot {
    pub roots: Vec<String>,
    pub max_level: u8,
    pub cells: u32,
    pub max_desired_patches: usize,
    pub worker_count: usize,
    pub worker_delay_millis: u64,
    pub queue_cap: usize,
    pub completion_cap: usize,
    pub admission_cap_per_tick: usize,
    pub upload_tile_cap: usize,
    pub upload_byte_cap: usize,
    pub publication_cap_per_tick: usize,
    pub transition_cap: usize,
    pub split_threshold_px: f64,
    pub merge_threshold_px: f64,
    pub prediction_seconds: f64,
    pub high_speed_mps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalPatchSnapshot {
    pub address: String,
    pub level: u8,
    pub projected_error_px: f64,
    pub priority: f64,
    /// absent, requested, building, built_cpu, queued_upload, resident, drawable
    pub state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalQueueSnapshot {
    pub address: String,
    pub bytes: usize,
    pub priority: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalRequestSnapshot {
    pub address: String,
    pub priority: f64,
    pub state: &'static str,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionalSplitFrontier {
    pub parent: CubePatchAddress,
    pub children: [CubePatchAddress; 4],
    pub parent_resident: bool,
    pub ready_children: usize,
    pub aggregate_priority: f64,
    /// Shared whole-view completion rank consumed by CPU and native GPU admission.
    pub completion_priority: f64,
    /// Time since this drawable parent first became an eligible refinement group.
    pub age: Duration,
    /// Current drawable error above the useful coarse-detail floor, in pixels.
    pub useful_floor_deficit_px: f64,
    pub resident_children: usize,
    /// Children which have not reached the CPU cache, upload queue, or residency.
    pub missing_dependencies: usize,
    pub blocker: RegionalCompletionBlocker,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompletionRank {
    pub(crate) priority: f64,
    pub(crate) under_floor: bool,
    pub(crate) age: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScreenCoverageKey {
    view_projection_bits: [u64; 20],
    drawable_cover_revision: u64,
    score_revision: u64,
    max_level: u8,
}

struct ScreenCoverageCacheEntry {
    key: ScreenCoverageKey,
    diagnostic: ScreenCoverageDiagnostic,
}

impl CompletionRank {
    pub(crate) fn compare(self, other: Self) -> std::cmp::Ordering {
        let self_starved = self.age >= BUILD_CANDIDATE_STARVATION_AFTER;
        let other_starved = other.age >= BUILD_CANDIDATE_STARVATION_AFTER;
        other_starved
            .cmp(&self_starved)
            .then_with(|| {
                if self_starved && other_starved {
                    other.age.cmp(&self.age)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| other.under_floor.cmp(&self.under_floor))
            .then_with(|| other.priority.total_cmp(&self.priority))
    }
}

fn completion_group_rank(
    error_px: f64,
    target_error_px: f64,
    projected_area_px: f64,
    age: Duration,
) -> CompletionRank {
    let floor_deficit = (error_px - USEFUL_DETAIL_FLOOR_ERROR_PX).max(0.0);
    let under_floor = floor_deficit > 0.0;
    CompletionRank {
        priority: projected_area_px.max(1.0e-9)
            * if under_floor {
                floor_deficit
            } else {
                (error_px - target_error_px).max(0.0)
            },
        under_floor,
        age,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalCompletionBlocker {
    Admission,
    CpuPreparation,
    Upload,
    Residency,
    Publication,
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegionalPrioritySnapshot {
    pub address: String,
    pub projected_error_px: f64,
    pub approach_multiplier: f64,
    pub high_speed_multiplier: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalError {
    InvalidConfig,
    InvalidView,
    InvalidTile,
    CpuCapacity,
    TileBuild,
    InvalidCover,
    NotResident,
    PublicationBudget,
}

impl std::fmt::Display for RegionalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfig => "regional terrain configuration is invalid",
            Self::InvalidView => "regional view contains a non-finite or negative projection value",
            Self::InvalidTile => "regional tile does not match this surface identity",
            Self::CpuCapacity => "regional CPU tile cache has no capacity for the seeded tile",
            Self::TileBuild => "regional tile content key could not be constructed",
            Self::InvalidCover => "regional cover is incomplete, overlapping, or unbalanced",
            Self::NotResident => "regional publication references a nonresident tile",
            Self::PublicationBudget => "regional publication budget is exhausted",
        })
    }
}

impl std::error::Error for RegionalError {}

#[derive(Debug)]
struct CacheEntry {
    tile: Arc<TileData>,
    bytes: usize,
    last_use: u64,
    build_elapsed_micros: Option<u64>,
    ever_resident: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheInsertResult {
    Inserted,
    AlreadyCached,
    Rejected,
}

#[derive(Debug)]
struct BuildToken {
    cancelled: AtomicBool,
    started: AtomicBool,
}

#[derive(Debug)]
struct BuildJob {
    address: CubePatchAddress,
    key: TileKey,
    token: Arc<BuildToken>,
}

#[derive(Debug)]
struct BuildResult {
    address: CubePatchAddress,
    key: TileKey,
    token: Arc<BuildToken>,
    result: Option<(Arc<TileData>, TileBuildDiagnostics)>,
    elapsed: Duration,
    cancelled_sample_evaluations: u64,
    cancelled_preparation_bytes: u64,
    query_workspace_bound_bytes: usize,
    prepared_store_stats: SurfacePreparationStoreStats,
}

struct WorkerPool {
    sender: SyncSender<BuildJob>,
    _worker_count: usize,
}

#[derive(Default)]
struct DesiredPriorityIndex {
    /// Exact ordered key and priority-bit signature; avoids hash-collision reuse.
    signature: Vec<(CubePatchAddress, u64)>,
    priorities: HashMap<CubePatchAddress, f64>,
    /// Parent, rank bits, and the two-second promotion state for native caches.
    completion_signature: Vec<(CubePatchAddress, u64, bool, bool)>,
    completion_summary_inputs: Option<(u64, u64, u64, bool)>,
    next_completion_promotion: Option<Instant>,
    #[cfg(test)]
    completion_frontier_scans: usize,
    revision: u64,
}

#[derive(Debug, Clone, Copy)]
struct BuildCandidateWait {
    first_eligible: Instant,
    last_seen_epoch: u64,
}

/// Regional adaptive selector and bounded asynchronous CPU tile builder.
///
/// GPU allocation and submission safety remain owned by the renderer. The app
/// acknowledges residency only after upload publication succeeds, and only a
/// complete resident 2:1 cover may become drawable.
pub struct RegionalTerrain {
    generator: Arc<SurfaceGenerator>,
    derived_field: Option<SharedDerivedField>,
    identity: TileBuildIdentity,
    config: RegionalConfig,
    trace: Arc<TerrainTrace>,
    planetary_view: Option<PlanetaryView>,
    last_selection_planetary_view: Option<PlanetaryView>,
    surface_topology: SurfaceTopology,
    patch_metadata: RefCell<HashMap<CubePatchAddress, PatchMetadata>>,
    planetary_scores: RefCell<BTreeMap<CubePatchAddress, (f64, f64, f64, f64)>>,
    planetary_footprint_weights: RefCell<HashMap<CubePatchAddress, f64>>,
    screen_coverage_cache: RefCell<Option<ScreenCoverageCacheEntry>>,
    drawable_cover_revision: u64,
    coverage_score_revision: u64,
    workers: WorkerPool,
    completions: Receiver<BuildResult>,
    completion_backlog: Arc<AtomicUsize>,
    worker_running: Arc<AtomicUsize>,
    tick: u64,
    view: RegionalView,
    desired: BTreeMap<CubePatchAddress, RegionalPatchSnapshot>,
    drawable: Vec<CubePatchAddress>,
    resident: BTreeSet<CubePatchAddress>,
    external_pins: BTreeSet<CubePatchAddress>,
    cache: HashMap<TileKey, CacheEntry>,
    cached_tiles_by_address: HashMap<CubePatchAddress, Arc<TileData>>,
    blocked_publication_parents: HashSet<CubePatchAddress>,
    cache_bytes: usize,
    cache_clock: u64,
    in_flight: HashMap<TileKey, Arc<BuildToken>>,
    uploads: Vec<RegionalUpload>,
    upload_bytes: usize,
    upload_allowlist: Option<BTreeSet<CubePatchAddress>>,
    publications: Vec<RegionalPublication>,
    publications_dirty: bool,
    last_priorities: Vec<RegionalPrioritySnapshot>,
    publications_this_tick: usize,
    desired_addresses: Vec<CubePatchAddress>,
    last_selection_view: Option<RegionalView>,
    selector_can_reuse_fixed_point: bool,
    selector_last_capacity_pressure: bool,
    selector_fixed_point_reused: bool,
    scheduler_fixed_point_reused: bool,
    planetary_topology_operations: usize,
    planetary_split_candidates_scanned: usize,
    planetary_candidate_cursor: usize,
    planetary_score_cursor: usize,
    planetary_merge_index_cover: Vec<CubePatchAddress>,
    planetary_merge_index_parents: Vec<CubePatchAddress>,
    planetary_merge_index_leaves: HashSet<CubePatchAddress>,
    desired_dependency_cache: RefCell<(Vec<CubePatchAddress>, BTreeSet<CubePatchAddress>)>,
    desired_priority_index: RefCell<DesiredPriorityIndex>,
    build_candidate_waits: HashMap<TileKey, BuildCandidateWait>,
    build_candidate_wait_epoch: u64,
    completion_group_first_eligible: RefCell<HashMap<CubePatchAddress, Instant>>,
    completion_gpu_reservation: Option<CubePatchAddress>,
    tile_key_definition_word_count: usize,
    selector_budget_exhausted: bool,
    merge_scan_cursor: Option<CubePatchAddress>,
    cull_scan_cursor: Option<CubePatchAddress>,
    selector_merge_scan_pending: bool,
    reserved_split_parent: Option<CubePatchAddress>,
    resident_history: HashSet<CubePatchAddress>,
    seen_cache_requests: HashSet<TileKey>,
    stats: Counters,
    frontier_discovery_micros: Cell<u64>,
    publication_candidate_ages: HashMap<CubePatchAddress, u64>,
    pressure: Pressure,
}

#[derive(Debug, Default)]
struct Counters {
    requests_issued: u64,
    jobs_started: u64,
    completed_build_count: u64,
    cancelled_before_start: u64,
    cancelled_during_work: u64,
    stale_completions: u64,
    completed_unused_tiles: u64,
    bytes_built_but_unused: u64,
    build_time_discarded_micros: u64,
    build_time_micros: u64,
    worker_elapsed_micros: u64,
    cancelled_attempt_sample_evaluations: u64,
    cancelled_attempt_preparation_bytes: u64,
    worker_query_workspace_bound_bytes: usize,
    prepared_store_stats: SurfacePreparationStoreStats,
    recent_build_elapsed_micros: VecDeque<u64>,
    build_failures: u64,
    completion_drain_micros: u64,
    selection_time_micros: u64,
    selection_budget_overruns: u64,
    scheduler_time_micros: u64,
    publication_discovery_micros: u64,
    cache_hits: u64,
    cache_misses: u64,
    cache_evictions: u64,
    rebuilds: u64,
    reuploads: u64,
}

#[derive(Debug, Default)]
struct Pressure {
    queue: bool,
    cpu: bool,
    upload: bool,
    publication: bool,
    desired_capacity: bool,
}

impl RegionalTerrain {
    pub fn new(
        generator: SurfaceGenerator,
        identity: TileBuildIdentity,
        config: RegionalConfig,
    ) -> Result<Self, RegionalError> {
        Self::new_with_derived_field(generator, identity, config, None)
    }

    /// Create a planetary regional scheduler with the shared bounded derived
    /// field used by production tile workers. Finite/reference fixtures retain
    /// the exact filtered tile representation through `new`.
    pub fn new_derived(
        generator: SurfaceGenerator,
        identity: TileBuildIdentity,
        config: RegionalConfig,
    ) -> Result<Self, RegionalError> {
        validate_config(&config)?;
        let field = SharedDerivedField::new(&generator).map_err(|_| RegionalError::TileBuild)?;
        Self::new_with_derived_field(generator, identity, config, Some(field))
    }

    fn new_with_derived_field(
        generator: SurfaceGenerator,
        identity: TileBuildIdentity,
        config: RegionalConfig,
        derived_field: Option<SharedDerivedField>,
    ) -> Result<Self, RegionalError> {
        validate_config(&config)?;
        let generator = Arc::new(generator);
        let first_key = if derived_field.is_some() {
            ResidentTileBuilder::derived_tile_key(
                &generator,
                identity,
                config.roots[0],
                config.cells,
            )
        } else {
            ResidentTileBuilder::tile_key(&generator, identity, config.roots[0], config.cells)
        };
        let tile_key_definition_word_count =
            first_key.map(|key| key.definition_words.len()).unwrap_or(0);
        let worker_count = config.worker_count;
        let (job_sender, job_receiver) = mpsc::sync_channel::<BuildJob>(config.queue_cap);
        let (completion_sender, completion_receiver) =
            mpsc::sync_channel::<BuildResult>(config.completion_cap);
        let job_receiver = Arc::new(Mutex::new(job_receiver));
        let completion_backlog = Arc::new(AtomicUsize::new(0));
        let worker_running = Arc::new(AtomicUsize::new(0));
        let trace = Arc::new(TerrainTrace::new());

        for worker_id in 0..worker_count {
            let receiver = Arc::clone(&job_receiver);
            let sender = completion_sender.clone();
            let generator = Arc::clone(&generator);
            let delay = config.worker_delay;
            let running = Arc::clone(&worker_running);
            let backlog = Arc::clone(&completion_backlog);
            let trace = Arc::clone(&trace);
            let worker_derived_field = derived_field.clone();
            thread::Builder::new()
                .name("regional-terrain-builder".into())
                .stack_size(4 * 1024 * 1024)
                .spawn(move || {
                    const WORKER_NAMES: [&str; 16] = [
                        "Terrain Worker 0",
                        "Terrain Worker 1",
                        "Terrain Worker 2",
                        "Terrain Worker 3",
                        "Terrain Worker 4",
                        "Terrain Worker 5",
                        "Terrain Worker 6",
                        "Terrain Worker 7",
                        "Terrain Worker 8",
                        "Terrain Worker 9",
                        "Terrain Worker 10",
                        "Terrain Worker 11",
                        "Terrain Worker 12",
                        "Terrain Worker 13",
                        "Terrain Worker 14",
                        "Terrain Worker 15",
                    ];
                    let _worker = WORKER_NAMES.get(worker_id).map_or_else(
                        || {
                            crate::engine_profile::worker_scope(
                                0x5445_5252_4149_4e00u64.saturating_add(worker_id as u64),
                            )
                        },
                        |name| crate::engine_profile::worker_scope_named(name),
                    );
                    // Prepared query pages are immutable and shared by the
                    // generator; each worker keeps only its bounded leases and
                    // batch scratch so neighboring jobs can reuse preparation.
                    let mut query_context = generator.prepared_query_context();
                    let derived_field = worker_derived_field;
                    loop {
                        let job = {
                            let receiver = receiver
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            receiver.recv()
                        };
                        let Ok(job) = job else { break };
                        if job.token.cancelled.load(AtomicOrdering::Acquire) {
                            trace.event_key(&job.key, Stage::GenerationCancelled);
                            backlog.fetch_add(1, AtomicOrdering::AcqRel);
                            if sender
                                .send(BuildResult {
                                    address: job.address,
                                    key: job.key,
                                    token: job.token,
                                    result: None,
                                    elapsed: Duration::ZERO,
                                    cancelled_sample_evaluations: 0,
                                    cancelled_preparation_bytes: 0,
                                    query_workspace_bound_bytes: query_context
                                        .total_workspace_bound_bytes(),
                                    prepared_store_stats: query_context.store_stats(),
                                })
                                .is_err()
                            {
                                backlog.fetch_sub(1, AtomicOrdering::AcqRel);
                                break;
                            }
                            continue;
                        }
                        job.token.started.store(true, AtomicOrdering::Release);
                        trace.event_key(&job.key, Stage::GenerationStarted);
                        running.fetch_add(1, AtomicOrdering::AcqRel);
                        let started = std::time::Instant::now();
                        let query_stats_before = query_context.stats();
                        let generation_span =
                            crate::engine_profile::span("terrain tile generation");
                        let mut remaining = delay;
                        while !remaining.is_zero()
                            && !job.token.cancelled.load(AtomicOrdering::Acquire)
                        {
                            let slice = remaining.min(Duration::from_millis(5));
                            thread::sleep(slice);
                            remaining = remaining.saturating_sub(slice);
                        }
                        let result = if job.token.cancelled.load(AtomicOrdering::Acquire) {
                            trace.event_key(&job.key, Stage::GenerationCancelled);
                            None
                        } else {
                            let build_span = crate::engine_profile::span("terrain tile sampling");
                            let cancelled = || job.token.cancelled.load(AtomicOrdering::Acquire);
                            let built = if let Some(field) = derived_field.as_ref() {
                                ResidentTileBuilder::build_derived(
                                    &generator,
                                    identity,
                                    job.address,
                                    job.key.cells,
                                    &mut query_context,
                                    field,
                                    cancelled,
                                )
                            } else {
                                ResidentTileBuilder::build_prepared(
                                    &generator,
                                    identity,
                                    job.address,
                                    job.key.cells,
                                    &mut query_context,
                                    cancelled,
                                )
                            };
                            drop(build_span);
                            match built {
                                Ok((tile, diagnostics)) => {
                                    trace.event_key(&job.key, Stage::GenerationCompleted);
                                    Some((Arc::new(tile), diagnostics))
                                }
                                Err(_) => {
                                    if job.token.cancelled.load(AtomicOrdering::Acquire) {
                                        trace.event_key(&job.key, Stage::GenerationCancelled);
                                    } else {
                                        trace.event_key(&job.key, Stage::GenerationFailed);
                                    }
                                    None
                                }
                            }
                        };
                        drop(generation_span);
                        let elapsed = started.elapsed();
                        let query_stats_after = query_context.stats();
                        let cancelled_work =
                            result.is_none() && job.token.cancelled.load(AtomicOrdering::Acquire);
                        running.fetch_sub(1, AtomicOrdering::AcqRel);
                        backlog.fetch_add(1, AtomicOrdering::AcqRel);
                        if sender
                            .send(BuildResult {
                                address: job.address,
                                key: job.key,
                                token: job.token,
                                result,
                                elapsed,
                                cancelled_sample_evaluations: if cancelled_work {
                                    query_stats_after
                                        .sample_evaluations
                                        .saturating_sub(query_stats_before.sample_evaluations)
                                } else {
                                    0
                                },
                                cancelled_preparation_bytes: if cancelled_work {
                                    query_stats_after
                                        .preparation_bytes
                                        .saturating_sub(query_stats_before.preparation_bytes)
                                } else {
                                    0
                                },
                                query_workspace_bound_bytes: query_context
                                    .total_workspace_bound_bytes(),
                                prepared_store_stats: query_context.store_stats(),
                            })
                            .is_err()
                        {
                            backlog.fetch_sub(1, AtomicOrdering::AcqRel);
                            break;
                        }
                    }
                })
                .map_err(|_| RegionalError::InvalidConfig)?;
        }
        drop(completion_sender);

        Ok(Self {
            generator,
            derived_field,
            identity,
            config,
            trace,
            planetary_view: None,
            last_selection_planetary_view: None,
            surface_topology: SurfaceTopology::new(),
            patch_metadata: RefCell::new(HashMap::new()),
            planetary_scores: RefCell::new(BTreeMap::new()),
            planetary_footprint_weights: RefCell::new(HashMap::new()),
            screen_coverage_cache: RefCell::new(None),
            drawable_cover_revision: 0,
            coverage_score_revision: 0,
            workers: WorkerPool {
                sender: job_sender,
                _worker_count: worker_count,
            },
            completions: completion_receiver,
            completion_backlog,
            worker_running,
            tick: 0,
            view: RegionalView {
                body_position_m: DVec3::ZERO,
                body_velocity_mps: DVec3::ZERO,
                projection_scale_px: 1.0,
            },
            desired: BTreeMap::new(),
            drawable: Vec::new(),
            resident: BTreeSet::new(),
            external_pins: BTreeSet::new(),
            cache: HashMap::new(),
            cached_tiles_by_address: HashMap::new(),
            blocked_publication_parents: HashSet::new(),
            cache_bytes: 0,
            cache_clock: 0,
            in_flight: HashMap::new(),
            uploads: Vec::new(),
            upload_bytes: 0,
            upload_allowlist: None,
            publications: Vec::new(),
            publications_dirty: false,
            last_priorities: Vec::new(),
            publications_this_tick: 0,
            desired_addresses: Vec::new(),
            last_selection_view: None,
            selector_can_reuse_fixed_point: false,
            selector_last_capacity_pressure: false,
            selector_fixed_point_reused: false,
            scheduler_fixed_point_reused: false,
            planetary_topology_operations: 0,
            planetary_split_candidates_scanned: 0,
            planetary_candidate_cursor: 0,
            planetary_score_cursor: 0,
            planetary_merge_index_cover: Vec::new(),
            planetary_merge_index_parents: Vec::new(),
            planetary_merge_index_leaves: HashSet::new(),
            desired_dependency_cache: RefCell::new((Vec::new(), BTreeSet::new())),
            desired_priority_index: RefCell::new(DesiredPriorityIndex::default()),
            build_candidate_waits: HashMap::new(),
            build_candidate_wait_epoch: 0,
            completion_group_first_eligible: RefCell::new(HashMap::new()),
            completion_gpu_reservation: None,
            tile_key_definition_word_count,
            selector_budget_exhausted: false,
            merge_scan_cursor: None,
            cull_scan_cursor: None,
            selector_merge_scan_pending: false,
            reserved_split_parent: None,
            resident_history: HashSet::new(),
            seen_cache_requests: HashSet::new(),
            stats: Counters::default(),
            frontier_discovery_micros: Cell::new(0),
            publication_candidate_ages: HashMap::new(),
            pressure: Pressure::default(),
        })
    }

    /// Update selection and admit bounded work. This method never waits for a
    /// worker, tile build, upload, GPU completion, or transition.
    pub fn tick(&mut self, view: RegionalView, _dt: Duration) -> Result<(), RegionalError> {
        if !view.body_position_m.is_finite()
            || !view.body_velocity_mps.is_finite()
            || !view.projection_scale_px.is_finite()
            || view.projection_scale_px < 0.0
        {
            return Err(RegionalError::InvalidView);
        }
        self.tick = self.tick.saturating_add(1);
        for age in self.publication_candidate_ages.values_mut() {
            *age = age.saturating_add(1);
        }
        self.publications_this_tick = 0;
        self.stats.publication_discovery_micros = 0;
        self.scheduler_fixed_point_reused = false;
        self.pressure = Pressure::default();
        self.view = view;
        let phase_started = std::time::Instant::now();
        let previous_desired = self.desired_addresses.clone();
        let selector_span = crate::engine_profile::span("terrain desired cover");
        self.select_desired();
        self.prune_completion_ages();
        drop(selector_span);
        if crate::engine_profile::is_enabled() && previous_desired != self.desired_addresses {
            let previous: HashSet<_> = previous_desired.iter().copied().collect();
            let current: HashSet<_> = self.desired_addresses.iter().copied().collect();
            for &address in &self.desired_addresses {
                if !previous.contains(&address)
                    && let Ok(key) = self.key_for(address)
                {
                    self.trace.set_desired(&key, true);
                }
            }
            for address in previous.difference(&current).copied() {
                self.trace.clear_desired(address);
            }
        }
        self.stats.selection_time_micros = phase_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        if self.planetary_view.is_some() && self.stats.selection_time_micros > 2_000 {
            self.stats.selection_budget_overruns =
                self.stats.selection_budget_overruns.saturating_add(1);
        }
        let completion_span = crate::engine_profile::span("terrain completion drain");
        self.drain_completions();
        drop(completion_span);
        self.touch_pinned_cache_entries();
        let scheduler_started = std::time::Instant::now();
        // An unchanged fixed point with complete resident coverage has no work
        // to admit. Keep LRU touches and completion draining above; a view,
        // topology, residency, publication or external-pin change restores the
        // normal scheduler before any requested dependency can be skipped.
        if self.planetary_view.is_some()
            && self.selector_fixed_point_reused
            && self.desired_addresses == self.drawable
            && self.is_idle()
            && self.uploads.is_empty()
            && self.publications.is_empty()
            && !self.publications_dirty
            && self
                .config
                .roots
                .iter()
                .chain(&self.drawable)
                .chain(&self.external_pins)
                .all(|address| self.resident.contains(address))
        {
            self.trace
                .sync_generation_candidates(std::iter::empty::<&TileKey>());
            self.trace
                .sync_upload_candidates(std::iter::empty::<&TileKey>());
            self.reserved_split_parent = None;
            self.scheduler_fixed_point_reused = true;
            self.stats.scheduler_time_micros = scheduler_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            return Ok(());
        }
        self.update_split_reservation();
        self.cancel_obsolete_work();
        self.admit_builds();
        self.admit_uploads();
        if self.desired_addresses != previous_desired || self.publications_dirty {
            self.refresh_publications();
        }
        self.stats.scheduler_time_micros = scheduler_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        Ok(())
    }

    pub fn desired(&self) -> &[CubePatchAddress] {
        // BTreeMap keeps the public cover deterministic. The scratch vector is
        // held in a stable field so the slice borrow cannot escape mutation.
        &self.desired_addresses
    }

    /// Measured time spent selecting the desired planetary/finite-region cover
    /// during the most recent tick, in microseconds.
    pub fn selection_time_micros(&self) -> u64 {
        self.stats.selection_time_micros
    }

    pub(crate) fn completion_drain_time_micros(&self) -> u64 {
        self.stats.completion_drain_micros
    }

    pub(crate) fn publication_discovery_time_micros(&self) -> u64 {
        self.stats.publication_discovery_micros
    }

    pub(crate) fn frontier_discovery_time_micros(&self) -> u64 {
        self.frontier_discovery_micros.get()
    }

    /// Number of successful CPU tile builds completed by workers so far.
    /// This excludes renderer uploads and includes results later rejected as stale.
    pub fn completed_build_count(&self) -> u64 {
        self.stats.completed_build_count
    }

    pub fn drawable(&self) -> &[CubePatchAddress] {
        &self.drawable
    }

    /// Shared bounded pipeline trace. Native diagnostics sample this separately
    /// from the hot regional scheduler state.
    pub(crate) fn trace(&self) -> Arc<TerrainTrace> {
        Arc::clone(&self.trace)
    }

    pub fn config(&self) -> &RegionalConfig {
        &self.config
    }

    /// Attach an optional body-fixed-to-view orientation and projection for
    /// conservative planetary demand culling. `RegionalView` remains the
    /// observer-position contract used by finite-region fixtures.
    pub fn set_planetary_view(
        &mut self,
        body_to_view: DMat3,
        projection: CelestialProjection,
    ) -> Result<(), RegionalError> {
        let columns = body_to_view.to_cols_array();
        if columns.iter().any(|component| !component.is_finite())
            || (body_to_view.transpose() * body_to_view - DMat3::IDENTITY)
                .to_cols_array()
                .iter()
                .any(|component| component.abs() > 1.0e-6)
            || (body_to_view.determinant() - 1.0).abs() > 1.0e-6
        {
            return Err(RegionalError::InvalidView);
        }
        let view = PlanetaryView {
            body_to_view,
            projection,
        };
        if !view.matches(self.planetary_view) {
            self.coverage_score_revision = self.coverage_score_revision.wrapping_add(1);
        }
        self.planetary_view = Some(view);
        Ok(())
    }

    /// Return to the original finite-region selection policy.
    pub fn clear_planetary_view(&mut self) {
        if self.planetary_view.take().is_some() {
            self.coverage_score_revision = self.coverage_score_revision.wrapping_add(1);
        }
    }

    /// Whether a patch's conservative displaced bounds intersect the current
    /// frustum and remain above the conservative planetary horizon.
    pub fn patch_visible(&self, address: CubePatchAddress) -> bool {
        self.patch_visible_with(address, self.planetary_view)
    }

    pub fn tile(&self, address: CubePatchAddress) -> Option<Arc<TileData>> {
        // Each core has immutable world authority and one exact key per address.
        // Admission verifies that key; eviction removes both indexes together.
        self.cached_tiles_by_address.get(&address).map(Arc::clone)
    }

    /// Insert a caller-provided prebuilt tile into the exact CPU cache.
    pub fn seed_tile(&mut self, tile: Arc<TileData>) -> Result<(), RegionalError> {
        if tile.validate().is_err() || self.key_for(tile.key.address).as_ref() != Ok(&tile.key) {
            return Err(RegionalError::InvalidTile);
        }
        if let Some(token) = self.in_flight.get(&tile.key) {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
        if self.cache_insert(tile, false, None) == CacheInsertResult::Rejected {
            return Err(RegionalError::CpuCapacity);
        }
        self.admit_uploads();
        Ok(())
    }

    pub fn queued_uploads(&self) -> &[RegionalUpload] {
        &self.uploads
    }

    /// Restrict queued and future upload admission to the supplied addresses.
    /// CPU cache entries, desired state, and worker builds are unaffected. Pass
    /// `None` to restore the default unrestricted upload frontier.
    pub fn set_upload_allowlist(&mut self, allowlist: Option<&[CubePatchAddress]>) {
        self.upload_allowlist = allowlist.map(|addresses| addresses.iter().copied().collect());
        let mut retained_bytes = 0usize;
        self.uploads.retain(|upload| {
            let retain = self
                .upload_allowlist
                .as_ref()
                .is_none_or(|allowed| allowed.contains(&upload.address));
            if retain {
                retained_bytes = retained_bytes.saturating_add(upload.bytes);
            }
            retain
        });
        self.upload_bytes = retained_bytes;
        self.admit_uploads();
    }

    /// Acknowledge a completed renderer upload. The caller must invoke this
    /// only after the renderer accepts the exact keyed tile.
    pub fn ack_resident(&mut self, address: CubePatchAddress) {
        let newly_resident = self.resident.insert(address);
        let queued = self
            .uploads
            .iter()
            .position(|upload| upload.address == address);
        if !newly_resident && queued.is_none() {
            return;
        }
        self.trace.set_resident(address, true);
        if let Ok(key) = self.key_for(address)
            && let Some(entry) = self.cache.get_mut(&key)
        {
            entry.ever_resident = true;
        }
        if let Some(index) = queued {
            let upload = self.uploads.remove(index);
            self.upload_bytes = self.upload_bytes.saturating_sub(upload.bytes);
        }
        if newly_resident && !self.resident_history.insert(address) {
            self.stats.reuploads = self.stats.reuploads.saturating_add(1);
        }
        if self.drawable.is_empty()
            && self
                .config
                .roots
                .iter()
                .all(|root| self.resident.contains(root))
        {
            self.drawable.clone_from(&self.config.roots);
            self.drawable.sort();
            self.drawable_cover_revision = self.drawable_cover_revision.wrapping_add(1);
            for &root in &self.drawable {
                self.trace.set_drawable(root, true);
            }
        }
        if self.planetary_view.is_some() {
            self.publications.clear();
            self.publications_dirty = true;
        } else {
            self.refresh_publications();
        }
    }

    /// Reflect renderer eviction so future upload admission can restore this tile.
    pub fn mark_not_resident(&mut self, address: CubePatchAddress) {
        if !self.resident.remove(&address) {
            return;
        }
        if self.drawable.contains(&address) {
            // Keep the last acknowledged cover until the runtime supplies a
            // replacement; publication validation prevents adding holes.
            self.pressure.publication = true;
        }
        if self.planetary_view.is_some() {
            self.publications.clear();
            self.publications_dirty = true;
        } else {
            self.refresh_publications();
        }
    }

    /// Cancel queued/delayed requests without waiting for a worker.
    pub fn request_cancel_all(&mut self) {
        for token in self.in_flight.values() {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
    }

    /// Drain worker outcomes without changing view selection or admitting new
    /// requests. Disabled fixtures can keep pumping this until `is_idle()`.
    pub fn drain_cancelled(&mut self) {
        self.drain_completions();
    }

    /// True after every request result was drained and workers left their build
    /// section. Runtime reconfiguration can drop the core safely at this point.
    pub fn is_idle(&self) -> bool {
        self.in_flight.is_empty()
            && self.worker_running.load(AtomicOrdering::Acquire) == 0
            && self.completion_backlog.load(AtomicOrdering::Acquire) == 0
    }

    /// Return local resident replacements. The returned vector is capped by
    /// both the per-tick publication and transition limits.
    pub fn publication_candidates(&self) -> Vec<RegionalPublication> {
        self.publications
            .iter()
            .filter_map(|candidate| self.materialize_publication(candidate))
            .collect()
    }

    /// Return compact local proposals for the native runtime. Covers are empty;
    /// the caller applies each local delta to its own transaction target.
    pub(crate) fn publication_local_candidates(&self) -> Vec<RegionalPublication> {
        self.publications.clone()
    }

    pub(crate) fn publication_locally_current(&self, candidate: &RegionalPublication) -> bool {
        self.local_publication_valid(candidate, true)
    }

    /// Current visual priority used to reprioritize prepared local work without
    /// rescoring tiles. Missing cached scores fall back to stored desired scores.
    pub(crate) fn publication_priority(&self, candidate: &RegionalPublication) -> f64 {
        if self.planetary_view.is_some() {
            self.publication_completion_rank(candidate, Instant::now())
                .priority
        } else {
            self.cached_publication_priority(candidate)
        }
    }

    /// Rank one already prepared publication without rediscovering whole-cover
    /// frontiers. The caller supplies one frozen instant for its bounded batch.
    pub(crate) fn publication_completion_rank(
        &self,
        candidate: &RegionalPublication,
        now: Instant,
    ) -> CompletionRank {
        let parent = publication_parent(candidate);
        let priority = || {
            self.planetary_scores
                .borrow()
                .get(&parent)
                .map(|score| score.1)
                .or_else(|| self.desired.get(&parent).map(|patch| patch.priority))
                .unwrap_or_else(|| match candidate {
                    RegionalPublication::Split { children, .. }
                    | RegionalPublication::Merge { children, .. } => children
                        .iter()
                        .filter_map(|child| self.desired.get(child).map(|patch| patch.priority))
                        .fold(0.0, f64::max),
                })
        };
        if self.planetary_view.is_none() {
            return CompletionRank {
                priority: priority(),
                under_floor: false,
                age: Duration::ZERO,
            };
        }
        if matches!(candidate, RegionalPublication::Split { .. }) {
            let error = self.score(parent).0;
            let area = self.projected_footprint_weight(parent);
            let age = if error > self.config.split_threshold_px && area > 0.0 {
                self.completion_group_first_eligible
                    .borrow()
                    .get(&parent)
                    .map(|first| now.saturating_duration_since(*first))
                    .unwrap_or_default()
            } else {
                Duration::ZERO
            };
            return completion_group_rank(error, self.config.split_threshold_px, area, age);
        }
        let ticks = self
            .publication_candidate_ages
            .get(&parent)
            .copied()
            .unwrap_or(0);
        CompletionRank {
            priority: priority(),
            under_floor: false,
            age: if ticks >= PUBLICATION_STARVATION_AGE_TICKS {
                Duration::from_secs(2).saturating_add(Duration::from_millis(ticks))
            } else {
                Duration::ZERO
            },
        }
    }

    pub(crate) fn publication_candidate_age_ticks(&self, candidate: &RegionalPublication) -> u64 {
        self.publication_candidate_ages
            .get(&publication_parent(candidate))
            .copied()
            .unwrap_or(0)
    }

    /// Commit a compact split/merge after local boundary preparation succeeds.
    /// Independent adoptions do not stale the proposal; its local delta is
    /// checked against the current drawable cover at the commit point.
    pub(crate) fn ack_local_publication(
        &mut self,
        candidate: &RegionalPublication,
    ) -> Result<(), RegionalError> {
        if !self.local_publication_valid(candidate, true) {
            return Err(RegionalError::InvalidCover);
        }
        self.apply_local_publication(candidate)
    }

    /// Finish a prepared merge after its accepted morph. The current demand may
    /// have reversed while that morph was displayed; topology and residency are
    /// still checked before replacing the four drawable children.
    pub(crate) fn ack_prepared_merge(
        &mut self,
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
    ) -> Result<(), RegionalError> {
        let candidate = RegionalPublication::Merge {
            parent,
            children,
            cover: Vec::new(),
        };
        if !self.local_publication_valid(&candidate, false) {
            return Err(RegionalError::InvalidCover);
        }
        self.apply_local_publication(&candidate)
    }

    /// Runtime transition ownership can temporarily exclude conflicting parents
    /// before bounded proposal selection. Publication and transition caps remain
    /// unchanged; eligible independent regions can proceed while a morph runs.
    pub(crate) fn set_publication_blocked_parents(&mut self, parents: &[CubePatchAddress]) {
        let blocked: HashSet<_> = parents.iter().copied().collect();
        if blocked != self.blocked_publication_parents {
            self.blocked_publication_parents = blocked;
            self.publications.clear();
            self.publications_dirty = true;
        }
    }

    pub(crate) fn is_current_publication(&self, candidate: &RegionalPublication) -> bool {
        self.publication_candidates().contains(candidate)
    }

    /// Acknowledge a publication previously produced by this core. Exact cache
    /// membership proves its root identity and complete balanced cover, so the
    /// trusted path avoids rebuilding the whole-cover validation index.
    pub(crate) fn ack_publication(
        &mut self,
        candidate: &RegionalPublication,
    ) -> Result<(), RegionalError> {
        if !self.is_current_publication(candidate) {
            return Err(RegionalError::InvalidCover);
        }
        let local = empty_cover_publication(candidate);
        self.apply_local_publication(&local)
    }

    /// Return locally desired sibling groups whose trial split preserves a
    /// complete balanced cover. Residency is reported but is not required, so
    /// callers can choose upload work before the group is GPU-ready.
    pub fn split_frontiers(&self) -> Vec<RegionalSplitFrontier> {
        let started = std::time::Instant::now();
        let result = self.split_frontier_groups();
        self.frontier_discovery_micros
            .set(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
        result
    }

    /// Pin the GPU adapter's currently feasible completion group so CPU cache
    /// reservation and GPU slot reservation advance the same sibling quartet.
    pub(crate) fn set_completion_gpu_reservation(&mut self, parent: Option<CubePatchAddress>) {
        self.completion_gpu_reservation = parent.filter(|parent| {
            self.drawable.contains(parent)
                && parent.children().is_ok_and(|children| {
                    local_replacement_is_balanced(*parent, true, &self.drawable, &self.config.roots)
                        && children.iter().all(|child| {
                            self.desired.keys().any(|desired| child.contains(*desired))
                        })
                })
        });
    }

    /// Balanced coarsening groups whose parent is required by the desired cover.
    pub fn merge_frontiers(&self) -> Vec<CubePatchAddress> {
        self.drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|parent| {
                let mut ancestor = Some(*parent);
                while let Some(address) = ancestor {
                    if self.desired.contains_key(&address) {
                        return local_replacement_is_balanced(
                            *parent,
                            false,
                            &self.drawable,
                            &self.config.roots,
                        );
                    }
                    ancestor = address.parent();
                }
                false
            })
            .collect()
    }

    /// Complete one resident sibling merge against the current valid cover.
    /// Exact source/replacement membership and outer-edge checks establish the
    /// same geometry invariant without validating every unaffected leaf again.
    pub(crate) fn ack_local_merge(
        &mut self,
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        cover: &[CubePatchAddress],
    ) -> Result<(), RegionalError> {
        if parent.children().ok() != Some(children)
            || !local_replacement_is_balanced(parent, false, &self.drawable, &self.config.roots)
        {
            return Err(RegionalError::InvalidCover);
        }
        let mut expected: Vec<_> = self
            .drawable
            .iter()
            .copied()
            .filter(|p| !children.contains(p))
            .chain([parent])
            .collect();
        expected.sort();
        if expected != cover {
            return Err(RegionalError::InvalidCover);
        }
        self.ack_drawable_inner(cover, true)
    }

    fn materialize_publication(
        &self,
        candidate: &RegionalPublication,
    ) -> Option<RegionalPublication> {
        let mut cover = self.drawable.clone();
        match candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => {
                let index = cover.binary_search(parent).ok()?;
                cover.remove(index);
                cover.extend(children);
                cover.sort_unstable();
            }
            RegionalPublication::Merge {
                parent, children, ..
            } => {
                for child in children {
                    let index = cover.binary_search(child).ok()?;
                    cover.remove(index);
                }
                let index = cover.binary_search(parent).err()?;
                cover.insert(index, *parent);
            }
        }
        Some(match candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => RegionalPublication::Split {
                parent: *parent,
                children: *children,
                cover,
            },
            RegionalPublication::Merge {
                parent, children, ..
            } => RegionalPublication::Merge {
                parent: *parent,
                children: *children,
                cover,
            },
        })
    }

    fn local_publication_valid(
        &self,
        candidate: &RegionalPublication,
        require_current_demand: bool,
    ) -> bool {
        if !candidate.cover().is_empty() {
            return false;
        }
        let (parent, children, split) = match candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => (*parent, *children, true),
            RegionalPublication::Merge {
                parent, children, ..
            } => (*parent, *children, false),
        };
        if parent.level() >= self.config.max_level || parent.children().ok() != Some(children) {
            return false;
        }
        let expected_sources_present = if split {
            self.drawable.binary_search(&parent).is_ok()
        } else {
            children
                .iter()
                .all(|child| self.drawable.binary_search(child).is_ok())
                && self.drawable.binary_search(&parent).is_err()
        };
        if !expected_sources_present {
            return false;
        }
        if require_current_demand {
            let demand_matches = if split {
                self.desired
                    .keys()
                    .any(|address| parent.contains(*address) && *address != parent)
            } else {
                std::iter::successors(Some(parent), |address| address.parent())
                    .any(|address| self.desired.contains_key(&address))
            };
            if !demand_matches {
                return false;
            }
        }
        if !local_replacement_is_balanced(parent, split, &self.drawable, &self.config.roots) {
            return false;
        }
        true
    }

    fn apply_local_publication(
        &mut self,
        candidate: &RegionalPublication,
    ) -> Result<(), RegionalError> {
        let split = matches!(candidate, RegionalPublication::Split { .. });
        let (parent, children) = match candidate {
            RegionalPublication::Split {
                parent, children, ..
            }
            | RegionalPublication::Merge {
                parent, children, ..
            } => (*parent, *children),
        };
        if self.publications_this_tick >= self.config.publication_cap_per_tick {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        if 1usize > self.config.transition_cap {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        if children
            .iter()
            .any(|address| !self.resident.contains(address))
            || !self.resident.contains(&parent)
        {
            return Err(RegionalError::NotResident);
        }
        if split {
            let index = self
                .drawable
                .binary_search(&parent)
                .map_err(|_| RegionalError::InvalidCover)?;
            self.drawable.remove(index);
            for child in children {
                let index = self
                    .drawable
                    .binary_search(&child)
                    .unwrap_or_else(|index| index);
                self.drawable.insert(index, child);
            }
        } else {
            for child in children {
                let index = self
                    .drawable
                    .binary_search(&child)
                    .map_err(|_| RegionalError::InvalidCover)?;
                self.drawable.remove(index);
            }
            let index = self
                .drawable
                .binary_search(&parent)
                .unwrap_or_else(|index| index);
            self.drawable.insert(index, parent);
        }
        self.drawable_cover_revision = self.drawable_cover_revision.wrapping_add(1);
        // This local replacement changes exactly one parent and its four
        // children. Recording that delta avoids rescanning the whole cover.
        self.trace.set_drawable(parent, !split);
        for child in children {
            self.trace.set_drawable(child, split);
        }
        if self.reserved_split_parent == Some(parent) {
            self.reserved_split_parent = None;
        }
        self.publications_this_tick += 1;
        self.publications.clear();
        self.publications_dirty = true;
        self.pressure.publication = false;
        Ok(())
    }

    /// Validate that `cover` completely and non-overlappingly covers the
    /// configured roots and satisfies the configured 2:1 level constraint.
    /// This is a pure geometry check; it does not inspect tile residency.
    pub fn cover_is_valid(&self, cover: &[CubePatchAddress]) -> bool {
        valid_cover(&self.config.roots, cover, self.config.max_level)
    }

    /// Publish a complete resident 2:1 cover. No partial or hole-bearing cover
    /// can replace the current drawable set.
    pub fn ack_drawable(&mut self, cover: &[CubePatchAddress]) -> Result<(), RegionalError> {
        self.ack_drawable_inner(cover, false)
    }

    fn ack_drawable_inner(
        &mut self,
        cover: &[CubePatchAddress],
        validated_publication: bool,
    ) -> Result<(), RegionalError> {
        if self.publications_this_tick >= self.config.publication_cap_per_tick {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        if !validated_publication && !valid_cover(&self.config.roots, cover, self.config.max_level)
        {
            return Err(RegionalError::InvalidCover);
        }
        let cover_set: HashSet<_> = cover.iter().copied().collect();
        if cover.iter().any(|address| !self.resident.contains(address)) {
            return Err(RegionalError::NotResident);
        }
        let drawable_set: HashSet<_> = self.drawable.iter().copied().collect();
        let changed = cover
            .iter()
            .filter(|address| !drawable_set.contains(address))
            .count()
            + self
                .drawable
                .iter()
                .filter(|address| !cover_set.contains(address))
                .count();
        if changed.div_ceil(5) > self.config.transition_cap {
            self.pressure.publication = true;
            return Err(RegionalError::PublicationBudget);
        }
        for address in self.drawable.iter().copied() {
            if !cover_set.contains(&address) {
                self.trace.set_drawable(address, false);
            }
        }
        self.drawable = cover.to_vec();
        self.drawable.sort();
        self.drawable_cover_revision = self.drawable_cover_revision.wrapping_add(1);
        for &address in &self.drawable {
            if !drawable_set.contains(&address) {
                self.trace.set_drawable(address, true);
            }
        }
        if self.reserved_split_parent.is_some_and(|parent| {
            !cover_set.contains(&parent)
                && parent
                    .children()
                    .is_ok_and(|children| children.iter().all(|child| cover_set.contains(child)))
        }) {
            self.reserved_split_parent = None;
        }
        self.publications_this_tick += 1;
        if validated_publication {
            self.publications.clear();
            self.publications_dirty = true;
        } else {
            self.refresh_publications();
        }
        Ok(())
    }

    /// Additional caller-owned pins protect transition/fallback dependencies.
    pub fn set_external_pins(&mut self, addresses: &[CubePatchAddress]) {
        self.external_pins.clear();
        self.external_pins.extend(addresses.iter().copied());
    }

    pub fn snapshot(&self) -> RegionalSnapshot {
        self.snapshot_inner(true)
    }

    /// Current scalar evidence without formatting every cover member per frame.
    /// The ordinary planetary host explicitly labels the omitted detail arrays.
    pub(crate) fn snapshot_summary(&self) -> RegionalSnapshot {
        self.snapshot_inner(false)
    }

    fn snapshot_inner(&self, include_details: bool) -> RegionalSnapshot {
        // Diagnostics share the current planetary view's score cache. Repeating
        // projected scoring for every drawable leaf would make evidence itself
        // a per-frame terrain evaluation workload.
        let mut diagnostic_scores = HashMap::new();
        let derived_stats = self
            .derived_field
            .as_ref()
            .and_then(|field| field.stats().ok())
            .unwrap_or_default();
        let drawable_addresses: HashSet<_> = self
            .drawable
            .iter()
            .filter(|_| include_details)
            .copied()
            .collect();
        let queued_upload_addresses: HashSet<_> =
            self.uploads.iter().map(|upload| upload.address).collect();
        let desired: Vec<_> = self
            .desired
            .iter()
            .filter(|_| include_details)
            .map(|(address, patch)| {
                let state = if drawable_addresses.contains(address) {
                    "drawable"
                } else if self.resident.contains(address) {
                    "resident"
                } else if queued_upload_addresses.contains(address) {
                    "queued_upload"
                } else if self.cached_tiles_by_address.contains_key(address) {
                    "built_cpu"
                } else if let Some(token) = self
                    .in_flight
                    .iter()
                    .find_map(|(key, token)| (key.address == *address).then_some(token))
                {
                    if token.cancelled.load(AtomicOrdering::Acquire) {
                        "absent"
                    } else if token.started.load(AtomicOrdering::Acquire) {
                        "building"
                    } else {
                        "requested"
                    }
                } else {
                    "absent"
                };
                let mut snapshot = patch.clone();
                snapshot.state = state;
                snapshot
            })
            .collect();
        // Normalize by this regional cover's own configured-root area so a
        // deep root reports pixels, not its tiny fraction of an entire face.
        let root_area = self
            .config
            .roots
            .iter()
            .map(|root| patch_area(*root))
            .sum::<f64>()
            .max(f64::MIN_POSITIVE);
        let desired_error = self
            .desired
            .iter()
            .map(|(address, patch)| patch_area(*address) * patch.projected_error_px)
            .sum::<f64>()
            / root_area;
        let drawable_error = self
            .drawable
            .iter()
            .map(|address| {
                patch_area(*address) * self.selection_score(*address, &mut diagnostic_scores).0
            })
            .sum::<f64>()
            / root_area;
        let area_debt = (drawable_error - desired_error).max(0.0);
        let running = self.worker_running.load(AtomicOrdering::Acquire);
        let in_flight = self.in_flight.len();
        let upload_queue = self
            .uploads
            .iter()
            .map(|upload| RegionalQueueSnapshot {
                address: format!("{:?}", upload.address),
                bytes: upload.bytes,
                priority: self
                    .desired
                    .get(&upload.address)
                    .map(|patch| patch.priority)
                    .unwrap_or_else(|| {
                        self.selection_score(upload.address, &mut diagnostic_scores)
                            .1
                    }),
            })
            .collect();
        let mut build_requests: Vec<_> = self
            .in_flight
            .iter()
            .map(|(key, token)| RegionalRequestSnapshot {
                address: format!("{:?}", key.address),
                priority: self
                    .desired
                    .get(&key.address)
                    .map(|patch| patch.priority)
                    .unwrap_or_else(|| self.selection_score(key.address, &mut diagnostic_scores).1),
                state: if token.cancelled.load(AtomicOrdering::Acquire) {
                    "cancelled"
                } else if token.started.load(AtomicOrdering::Acquire) {
                    "building"
                } else {
                    "requested"
                },
                cancelled: token.cancelled.load(AtomicOrdering::Acquire),
            })
            .collect();
        build_requests.sort_by(|a, b| {
            b.priority
                .total_cmp(&a.priority)
                .then(a.address.cmp(&b.address))
        });
        let upload_backlog_addresses: HashSet<_> =
            self.uploads.iter().map(|upload| upload.address).collect();
        let estimated_completed_unpublished_bytes = self
            .desired
            .keys()
            .filter(|address| {
                !self.resident.contains(address) && !upload_backlog_addresses.contains(address)
            })
            .filter_map(|address| self.cached_tiles_by_address.get(address))
            .map(|tile| tile_bytes(tile))
            .sum();
        let resident: Vec<_> = self
            .resident
            .iter()
            .filter(|_| include_details)
            .map(|a| format!("{a:?}"))
            .collect();
        let drawable: Vec<_> = self
            .drawable
            .iter()
            .filter(|_| include_details)
            .map(|a| format!("{a:?}"))
            .collect();
        let screen_coverage = self.screen_coverage_diagnostic();
        let target_quality_reached = !self.pressure.desired_capacity
            && (self.planetary_view.is_none() || self.selector_can_reuse_fixed_point)
            && !self.selector_budget_exhausted
            && self
                .desired
                .values()
                .all(|patch| patch.projected_error_px <= self.config.split_threshold_px);
        let mut visible_drawable_errors = Vec::new();
        let mut visible_drawable_count = 0usize;
        let mut visible_desired_counterparts = 0usize;
        let mut missing_drawable_proxy_scores = 0usize;
        let mut missing_visible_proxy_scores = 0usize;
        let mut visible_weight = 0.0;
        let mut converged_weight = 0.0;
        let mut worst_unresolved = 0.0f64;
        let mut worst_proxy: Option<(CubePatchAddress, f64)> = None;
        let mut worst_visible_unresolved: Option<(CubePatchAddress, f64, f64)> = None;
        let mut visible_proxy_hotspots = Vec::with_capacity(8);
        for address in &self.drawable {
            let visible = self.patch_visible(*address);
            let Some((error, _, _, _)) = diagnostic_scores.get(address).copied() else {
                missing_drawable_proxy_scores += 1;
                if visible {
                    missing_visible_proxy_scores += 1;
                }
                continue;
            };
            if error > 0.0 {
                visible_drawable_count += 1;
                visible_drawable_errors.push(error);
                if worst_proxy
                    .as_ref()
                    .is_none_or(|(current_address, current_error)| {
                        error.total_cmp(current_error).is_gt()
                            || (error.total_cmp(current_error).is_eq() && address < current_address)
                    })
                {
                    worst_proxy = Some((*address, error));
                }
            }
            if visible {
                let weight = self.projected_footprint_weight(*address);
                if weight.is_finite() && weight > 0.0 {
                    visible_weight += weight;
                    converged_weight +=
                        weight * convergence_ratio(error, self.config.split_threshold_px);
                }
                if error > self.config.split_threshold_px {
                    worst_unresolved = worst_unresolved.max(error);
                    if worst_visible_unresolved.as_ref().is_none_or(
                        |(current_address, current_error, _)| {
                            error.total_cmp(current_error).is_gt()
                                || (error.total_cmp(current_error).is_eq()
                                    && address < current_address)
                        },
                    ) {
                        worst_visible_unresolved = Some((
                            *address,
                            error,
                            if weight.is_finite() && weight > 0.0 {
                                weight
                            } else {
                                0.0
                            },
                        ));
                    }
                }
                visible_proxy_hotspots.push((
                    *address,
                    error,
                    if weight.is_finite() && weight > 0.0 {
                        weight
                    } else {
                        0.0
                    },
                ));
                visible_proxy_hotspots
                    .sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                visible_proxy_hotspots.truncate(8);
            }
        }
        let cached_planetary_scores = self.planetary_scores.borrow();
        for address in self.desired.keys() {
            if diagnostic_scores
                .get(address)
                .is_some_and(|score| score.0 > 0.0)
                || cached_planetary_scores
                    .get(address)
                    .is_some_and(|score| score.0 > 0.0)
            {
                visible_desired_counterparts += 1;
            }
        }
        drop(cached_planetary_scores);
        visible_drawable_errors.sort_by(f64::total_cmp);
        let proxy_max = visible_drawable_errors.last().copied().unwrap_or(0.0);
        let proxy_p95 = visible_drawable_errors
            .len()
            .checked_sub(1)
            .map(|last| visible_drawable_errors[((last + 1) * 95).div_ceil(100) - 1])
            .unwrap_or(0.0);
        let proxy_over_one = visible_drawable_errors
            .iter()
            .filter(|error| **error > 1.0)
            .count();
        let proxy_over_split = visible_drawable_errors
            .iter()
            .filter(|error| **error > self.config.split_threshold_px)
            .count();
        let useful_detail_reached = useful_detail_reached(
            self.planetary_view.is_some(),
            visible_drawable_count,
            missing_drawable_proxy_scores,
            proxy_max,
        );
        let visible_convergence = if visible_weight > 0.0 && missing_visible_proxy_scores == 0 {
            Some((converged_weight / visible_weight).clamp(0.0, 1.0))
        } else {
            None
        };
        let center_screen_error_px = self.center_screen_drawable_error(&mut diagnostic_scores);
        let center_screen_convergence = center_screen_error_px
            .map(|error| convergence_ratio(error, self.config.split_threshold_px));
        let worst_visible_unresolved_error_px = if missing_visible_proxy_scores == 0 {
            Some(worst_unresolved)
        } else {
            None
        };
        let worst_proxy = worst_proxy.map(|(address, error_px)| RegionalProxyErrorSnapshot {
            address: format!("{address:?}"),
            error_px,
            projected_footprint_weight: self.projected_footprint_weight(address),
        });
        let worst_visible_unresolved = if missing_visible_proxy_scores == 0 {
            worst_visible_unresolved.map(|(address, error_px, projected_footprint_weight)| {
                RegionalProxyErrorSnapshot {
                    address: format!("{address:?}"),
                    error_px,
                    projected_footprint_weight,
                }
            })
        } else {
            None
        };
        let visible_drawable_proxy_hotspots = visible_proxy_hotspots
            .into_iter()
            .map(|(address, error_px, projected_footprint_weight)| {
                self.drawable_proxy_hotspot(address, error_px, projected_footprint_weight)
            })
            .collect();
        let priority_index = self.desired_priority_index.borrow();
        let frontier_priority_index_entries = priority_index
            .priorities
            .len()
            .saturating_add(priority_index.completion_signature.len());
        drop(priority_index);
        let frontier_priority_cache_bytes_upper_bound =
            desired_priority_cache_bytes_upper_bound(&self.config);
        let build_candidate_wait_entries = self.build_candidate_waits.len();
        let build_candidate_wait_entries_upper_bound =
            self.build_candidate_wait_entries_upper_bound();
        let build_candidate_wait_bytes_upper_bound =
            self.build_candidate_wait_bytes_upper_bound(build_candidate_wait_entries_upper_bound);
        let builder_row_sample_size =
            std::mem::size_of::<mundaris_math::surface::SurfaceLocation>()
                + if self.derived_field.is_some() {
                    std::mem::size_of::<crate::resident_terrain::DerivedSample>()
                } else {
                    std::mem::size_of::<mundaris_world::terrain::SurfaceSample>()
                };
        let tile_builder_row_workspace_bytes = MAX_TILE_ROW_SAMPLES
            .saturating_mul(builder_row_sample_size)
            .saturating_mul(self.config.worker_count);
        let tile_builder_stack_scratch_bytes =
            (std::mem::size_of::<[mundaris_math::Direction3; 7]>()
                + std::mem::size_of::<[f64; 4]>()
                + std::mem::size_of::<mundaris_world::terrain::SurfaceSample>())
            .saturating_mul(self.config.worker_count);
        RegionalSnapshot {
            tick: self.tick,
            configuration: RegionalConfigSnapshot {
                roots: self
                    .config
                    .roots
                    .iter()
                    .map(|root| format!("{root:?}"))
                    .collect(),
                max_level: self.config.max_level,
                cells: self.config.cells,
                max_desired_patches: self.config.max_desired_patches,
                worker_count: self.config.worker_count,
                worker_delay_millis: self
                    .config
                    .worker_delay
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                queue_cap: self.config.queue_cap,
                completion_cap: self.config.completion_cap,
                admission_cap_per_tick: self.config.admission_cap_per_tick,
                upload_tile_cap: self.config.upload_tile_cap,
                upload_byte_cap: self.config.upload_byte_cap,
                publication_cap_per_tick: self.config.publication_cap_per_tick,
                transition_cap: self.config.transition_cap,
                split_threshold_px: self.config.split_threshold_px,
                merge_threshold_px: self.config.merge_threshold_px,
                prediction_seconds: self.config.prediction_seconds,
                high_speed_mps: self.config.high_speed_mps,
            },
            desired_count: self.desired.len(),
            desired,
            resident,
            drawable,
            upload_queue,
            build_requests,
            resident_count: self.resident.len(),
            drawable_count: self.drawable.len(),
            cpu_cached_tiles: self.cache.len(),
            cpu_cached_bytes: self.cache_bytes,
            worker_queued: self
                .in_flight
                .values()
                .filter(|token| !token.started.load(AtomicOrdering::Acquire))
                .count(),
            worker_outstanding: in_flight,
            worker_running: running,
            completion_backlog: self.completion_backlog.load(AtomicOrdering::Acquire),
            completion_drain_micros: self.stats.completion_drain_micros,
            selection_time_micros: self.stats.selection_time_micros,
            selection_budget_overruns: self.stats.selection_budget_overruns,
            selection_budget_exhausted: self.selector_budget_exhausted,
            scheduler_time_micros: self.stats.scheduler_time_micros,
            publication_discovery_micros: self.stats.publication_discovery_micros,
            frontier_discovery_micros: self.frontier_discovery_micros.get(),
            cpu_tile_cap: self.config.cpu_tile_cap,
            cpu_byte_cap: self.config.cpu_byte_cap,
            max_desired_patches: self.config.max_desired_patches,
            worker_queue_capacity: self.config.queue_cap,
            completion_capacity: self.config.completion_cap,
            upload_tile_capacity: self.config.upload_tile_cap,
            upload_byte_capacity: self.config.upload_byte_cap,
            estimated_worker_scratch_bytes: self
                .stats
                .worker_query_workspace_bound_bytes
                .saturating_mul(self.config.worker_count)
                .saturating_add(self.stats.prepared_store_stats.capacity_bytes)
                .saturating_add(derived_stats.cache_capacity_bytes)
                .saturating_add(
                    derived_stats
                        .batch_scratch_bound_bytes
                        .saturating_mul(self.config.worker_count),
                )
                .saturating_add(tile_builder_row_workspace_bytes)
                .saturating_add(tile_builder_stack_scratch_bytes),
            tile_builder_row_workspace_bytes,
            tile_builder_stack_scratch_bytes,
            derived_generation_workspace_bytes: derived_stats
                .batch_scratch_bound_bytes
                .saturating_mul(self.config.worker_count),
            prepared_generation_workspace_bytes: self
                .stats
                .worker_query_workspace_bound_bytes
                .saturating_mul(self.config.worker_count),
            prepared_store_capacity_bytes: self.stats.prepared_store_stats.capacity_bytes,
            prepared_store_retained_bytes: self.stats.prepared_store_stats.retained_bytes,
            prepared_store_high_water_bytes: self.stats.prepared_store_stats.high_water_bytes,
            prepared_store_transient_build_reservation_bytes: self
                .stats
                .prepared_store_stats
                .transient_build_reservation_bytes,
            derived_field_cache_capacity_bytes: derived_stats.cache_capacity_bytes,
            derived_field_cache_retained_bytes: derived_stats.retained_cache_bytes,
            derived_batch_scratch_bound_bytes: derived_stats.batch_scratch_bound_bytes,
            cancelled_attempt_sample_evaluations: self.stats.cancelled_attempt_sample_evaluations,
            cancelled_attempt_preparation_bytes: self.stats.cancelled_attempt_preparation_bytes,
            frontier_priority_index_entries,
            frontier_priority_cache_bytes_upper_bound,
            build_candidate_wait_entries,
            build_candidate_wait_entries_upper_bound,
            build_candidate_wait_bytes_upper_bound,
            upload_backlog_tiles: self.uploads.len(),
            upload_backlog_bytes: self.upload_bytes,
            estimated_completed_unpublished_bytes,
            publication_candidates: self.publications.len(),
            publication_backlog_age_ticks: self
                .publication_candidate_ages
                .values()
                .copied()
                .max()
                .unwrap_or(0),
            active_transitions_reported: 0,
            desired_projected_error_px: desired_error,
            drawable_projected_error_px: drawable_error,
            refinement_debt: area_debt,
            queue_pressure: self.pressure.queue,
            cpu_cache_pressure: self.pressure.cpu,
            upload_pressure: self.pressure.upload,
            publication_pressure: self.pressure.publication,
            desired_capacity_pressure: self.pressure.desired_capacity,
            target_quality_reached,
            visible_drawable_proxy_count: visible_drawable_count,
            missing_drawable_proxy_score_count: missing_drawable_proxy_scores,
            visible_desired_counterpart_count: visible_desired_counterparts,
            visible_drawable_proxy_error_max_px: proxy_max,
            visible_drawable_proxy_error_p95_px: proxy_p95,
            visible_drawable_proxy_error_over_1px: proxy_over_one,
            visible_drawable_proxy_error_over_split_threshold: proxy_over_split,
            worst_proxy,
            worst_visible_unresolved,
            visible_drawable_proxy_hotspots,
            visible_convergence,
            center_screen_convergence,
            center_screen_error_px,
            convergence_target_error_px: self.config.split_threshold_px,
            worst_visible_unresolved_error_px,
            useful_detail_reached,
            useful_detail_proxy_error_threshold_px: 1.0,
            screen_coverage,
            screen_coverage_diagnostic_overlap_bytes_upper_bound: std::mem::size_of::<
                ScreenCoverageDiagnostic,
            >()
            .saturating_add(coverage::coverage_witness_heap_capacity_bytes())
            .saturating_mul(3),
            visible_proxy_error_certified: false,
            visible_proxy_error_model: "approximate selector proxy; uncertified",
            planetary_topology_operations: self.planetary_topology_operations,
            planetary_split_candidates_scanned: self.planetary_split_candidates_scanned,
            selector_fixed_point_reused: self.selector_fixed_point_reused,
            scheduler_fixed_point_reused: self.scheduler_fixed_point_reused,
            requests_issued: self.stats.requests_issued,
            jobs_started: self.stats.jobs_started,
            completed_build_count: self.stats.completed_build_count,
            cancelled_before_start: self.stats.cancelled_before_start,
            cancelled_during_work: self.stats.cancelled_during_work,
            stale_completions: self.stats.stale_completions,
            completed_unused_tiles: self.stats.completed_unused_tiles,
            bytes_built_but_unused: self.stats.bytes_built_but_unused,
            build_time_discarded_micros: self.stats.build_time_discarded_micros,
            build_time_micros: self.stats.build_time_micros,
            worker_elapsed_micros: self.stats.worker_elapsed_micros,
            recent_build_elapsed_micros: self
                .stats
                .recent_build_elapsed_micros
                .iter()
                .copied()
                .collect(),
            build_failures: self.stats.build_failures,
            cache_hits: self.stats.cache_hits,
            cache_misses: self.stats.cache_misses,
            cache_evictions: self.stats.cache_evictions,
            rebuilds: self.stats.rebuilds,
            reuploads: self.stats.reuploads,
            last_admitted_priorities: self.last_priorities.clone(),
            selector_error_model: "spherical grid sagitta plus local relief residual; closest angular patch footprint for distance; heuristic, not sampled-error certificate",
        }
    }

    fn key_for(&self, address: CubePatchAddress) -> Result<TileKey, RegionalError> {
        let key = if self.derived_field.is_some() {
            ResidentTileBuilder::derived_tile_key(
                &self.generator,
                self.identity,
                address,
                self.config.cells,
            )
        } else {
            ResidentTileBuilder::tile_key(
                &self.generator,
                self.identity,
                address,
                self.config.cells,
            )
        };
        key.map_err(|_| RegionalError::TileBuild)
    }

    fn drawable_proxy_hotspot(
        &self,
        address: CubePatchAddress,
        error_px: f64,
        projected_footprint_weight: f64,
    ) -> RegionalDrawableProxyHotspotSnapshot {
        let key = self.key_for(address).ok();
        let token = key.as_ref().and_then(|key| self.in_flight.get(key));
        let in_flight = token.is_some();
        let in_flight_cancelled =
            token.is_some_and(|token| token.cancelled.load(AtomicOrdering::Acquire));
        let cpu_cached = key.as_ref().is_some_and(|key| self.cache.contains_key(key));
        let upload_queued = key.as_ref().is_some_and(|key| {
            self.uploads
                .iter()
                .any(|upload| upload.address == address && &upload.tile.key == key)
        });
        let parent = address.parent();
        let publication_parent_blocked =
            parent.is_some_and(|parent| self.blocked_publication_parents.contains(&parent));
        let upload_allowlisted = self
            .upload_allowlist
            .as_ref()
            .map(|allowlist| allowlist.contains(&address));
        let blocker_status = if key.is_none() {
            "tile_key_unavailable"
        } else if publication_parent_blocked {
            "publication_parent_blocked"
        } else if upload_allowlisted == Some(false) {
            "upload_not_allowlisted"
        } else if in_flight_cancelled {
            "build_cancelled"
        } else {
            "none_observed"
        };
        let key = key.map(|key| RegionalTileKeySnapshot {
            body_identity: key.body_identity,
            definition_words: key.definition_words,
            radius_bits: key.radius_bits,
            surface_revision: key.surface_revision,
            material_revision: key.material_revision,
            format_version: key.format_version,
            filter_version: key.filter_version,
            cells: key.cells,
        });
        RegionalDrawableProxyHotspotSnapshot {
            address: format!("{address:?}"),
            error_px,
            projected_footprint_weight,
            key,
            parent: parent.map(|parent| format!("{parent:?}")),
            desired: self.desired.contains_key(&address),
            resident: self.resident.contains(&address),
            cpu_cached,
            upload_queued,
            in_flight,
            in_flight_cancelled,
            publication_parent_blocked,
            upload_allowlisted,
            blocker_status,
        }
    }

    fn projected_footprint_weight(&self, address: CubePatchAddress) -> f64 {
        let Some(view) = self.planetary_view else {
            return 0.0;
        };
        if let Some(sampled_area) = self.cached_sampled_drawable_areas() {
            if let Some(area) = sampled_area.get(&address).copied() {
                return area.max(0.0);
            }
            // Desired children often lie below a coarser drawable fallback.
            // Assign them their conservative quarter-area share per level so
            // selection does not erase real on-screen coverage before split.
            let mut ancestor = address;
            let mut depth = 0u32;
            while let Some(parent) = ancestor.parent() {
                depth += 1;
                if let Some(area) = sampled_area.get(&parent).copied() {
                    return (area / 4_f64.powi(depth as i32)).max(0.0);
                }
                ancestor = parent;
            }
            // The reference sphere omits displaced-only silhouette pixels. Keep
            // conservative geometry candidates alive with negligible area so
            // broad culling bounds cannot monopolize the useful-floor tier.
            return if self.patch_visible(address) {
                1.0e-6
            } else {
                0.0
            };
        }
        if let Some(weight) = self
            .planetary_footprint_weights
            .borrow()
            .get(&address)
            .copied()
        {
            return weight;
        }
        let corner = |i, j| address.sample_direction(i, j, 1).ok().map(|d| d.unit());
        let Some([a, b, c, d]) =
            (|| Some([corner(0, 0)?, corner(1, 0)?, corner(1, 1)?, corner(0, 1)?]))()
        else {
            return 0.0;
        };
        let spherical_triangle = |a: DVec3, b: DVec3, c: DVec3| {
            2.0 * a
                .dot(b.cross(c))
                .abs()
                .atan2(1.0 + a.dot(b) + b.dot(c) + c.dot(a))
        };
        let solid_angle = spherical_triangle(a, b, c) + spherical_triangle(a, c, d);
        let center = (a + b + c + d).normalize_or_zero();
        let to_camera = self.view.body_position_m - center * self.generator.radius_m();
        let distance = to_camera.length().max(self.generator.radius_m() * 1.0e-9);
        let projected = solid_angle
            * self.generator.radius_m().powi(2)
            * center.dot(to_camera).max(0.0)
            * view.projection.focal_pixels().powi(2)
            / distance.powi(3);
        let weight = if projected.is_finite() {
            projected
        } else {
            0.0
        };
        let mut weights = self.planetary_footprint_weights.borrow_mut();
        let limit = self.config.max_desired_patches.saturating_mul(2).max(8192);
        if weights.len() >= limit
            && !weights.contains_key(&address)
            && let Some(evicted) = weights
                .keys()
                .find(|candidate| !self.desired.contains_key(candidate))
                .copied()
        {
            weights.remove(&evicted);
        }
        if weights.len() < limit || weights.contains_key(&address) {
            weights.insert(address, weight);
        }
        weight
    }

    fn screen_coverage_key(&self) -> Option<ScreenCoverageKey> {
        let view = self.planetary_view?;
        let mut view_projection_bits = [0u64; 20];
        let body_position_bits = self.view.body_position_m.to_array().map(f64::to_bits);
        let transform_bits = view.body_to_view.to_cols_array().map(f64::to_bits);
        view_projection_bits[..3].copy_from_slice(&body_position_bits);
        view_projection_bits[3..12].copy_from_slice(&transform_bits);
        let [width, height] = view.projection.viewport();
        let [origin_x, origin_y] = view.projection.origin();
        view_projection_bits[12..].copy_from_slice(&[
            u64::from(width),
            u64::from(height),
            u64::from(origin_x),
            u64::from(origin_y),
            view.projection.near_m().to_bits(),
            view.projection.vertical_fov_rad().to_bits(),
            self.generator.radius_m().to_bits(),
            self.config.split_threshold_px.to_bits(),
        ]);
        Some(ScreenCoverageKey {
            view_projection_bits,
            drawable_cover_revision: self.drawable_cover_revision,
            score_revision: self.coverage_score_revision,
            max_level: self.config.max_level,
        })
    }

    fn cached_sampled_drawable_areas(&self) -> Option<Arc<HashMap<CubePatchAddress, f64>>> {
        let key = self.screen_coverage_key()?;
        self.screen_coverage_cache
            .borrow()
            .as_ref()
            .filter(|entry| entry.key == key)
            .map(|entry| Arc::clone(&entry.diagnostic.sampled_drawable_area_pixels))
    }

    fn has_current_screen_coverage_cache(&self) -> bool {
        let Some(key) = self.screen_coverage_key() else {
            return false;
        };
        self.screen_coverage_cache
            .borrow()
            .as_ref()
            .is_some_and(|entry| entry.key == key)
    }

    fn screen_coverage_diagnostic(&self) -> Option<ScreenCoverageDiagnostic> {
        let view = self.planetary_view?;
        let key = self.screen_coverage_key()?;
        if let Some(entry) = self.screen_coverage_cache.borrow().as_ref()
            && entry.key == key
        {
            return Some(entry.diagnostic.clone());
        }
        let mut scores = HashMap::with_capacity(self.drawable.len());
        let mut score_cache = HashMap::new();
        for &address in &self.drawable {
            let error = self.selection_score(address, &mut score_cache).0;
            scores.insert(address, error.is_finite().then_some(error));
        }
        let diagnostic = coverage::screen_coverage_diagnostic(
            self.view.body_position_m,
            view.body_to_view,
            view.projection,
            self.generator.radius_m(),
            self.config.max_level,
            &scores,
            self.config.split_threshold_px,
        );
        *self.screen_coverage_cache.borrow_mut() = Some(ScreenCoverageCacheEntry {
            key,
            diagnostic: diagnostic.clone(),
        });
        Some(diagnostic)
    }

    fn center_screen_drawable_error(
        &self,
        score_cache: &mut HashMap<CubePatchAddress, (f64, f64, f64, f64)>,
    ) -> Option<f64> {
        let view = self.planetary_view?;
        let origin = self.view.body_position_m;
        let direction = view.body_to_view.transpose() * DVec3::NEG_Z;
        let radius = self.generator.radius_m();
        let half_b = origin.dot(direction);
        let discriminant = half_b * half_b - (origin.length_squared() - radius * radius);
        if discriminant < 0.0 {
            return None;
        }
        let distance = -half_b - discriminant.sqrt();
        if distance <= 0.0 {
            return None;
        }
        let body_direction = (origin + direction * distance).normalize_or_zero();
        let face = cube_face_for_direction(body_direction);
        let [normal, u, v] = face.basis();
        let denominator = body_direction.dot(normal);
        if denominator <= 0.0 {
            return None;
        }
        let uv = [
            body_direction.dot(u) / denominator,
            body_direction.dot(v) / denominator,
        ];
        let level = self.config.max_level.min(30);
        let scale = 1u64 << level;
        let coordinate = |value: f64| {
            (((value + 1.0) * 0.5 * scale as f64).floor() as i64).clamp(0, scale as i64 - 1) as u32
        };
        let target =
            CubePatchAddress::try_new(face, level, coordinate(uv[0]), coordinate(uv[1])).ok()?;
        let drawable = self
            .drawable
            .iter()
            .copied()
            .find(|patch| patch.contains(target))?;
        let error = self.selection_score(drawable, score_cache).0;
        error.is_finite().then_some(error)
    }

    fn touch_pinned_cache_entries(&mut self) {
        let pinned = self.pinned_addresses();
        for entry in self.cache.values_mut() {
            if pinned.contains(&entry.tile.key.address) {
                self.cache_clock = self.cache_clock.saturating_add(1);
                entry.last_use = self.cache_clock;
            }
        }
    }

    fn select_desired(&mut self) {
        self.selector_fixed_point_reused = false;
        self.selector_budget_exhausted = false;
        self.planetary_split_candidates_scanned = 0;
        let same_planetary_view = match (self.planetary_view, self.last_selection_planetary_view) {
            (None, None) => true,
            (Some(current), Some(previous)) => current.matches(Some(previous)),
            _ => false,
        };
        if self.last_selection_view != Some(self.view) || !same_planetary_view {
            self.merge_scan_cursor = None;
            self.cull_scan_cursor = None;
            self.planetary_scores.borrow_mut().clear();
            self.planetary_footprint_weights.borrow_mut().clear();
            self.coverage_score_revision = self.coverage_score_revision.wrapping_add(1);
        }
        if self.selector_can_reuse_fixed_point
            && self.last_selection_view == Some(self.view)
            && same_planetary_view
        {
            self.pressure.desired_capacity = self.selector_last_capacity_pressure;
            self.selector_fixed_point_reused = true;
            return;
        }

        if self.planetary_view.is_some() {
            self.select_planetary_desired();
            return;
        }

        let mut cover = self.config.roots.clone();
        cover.sort();
        let previous: BTreeSet<_> = self.desired.keys().copied().collect();
        let previous_cover: Vec<_> = previous.iter().copied().collect();
        let mut previously_refined = HashSet::with_capacity(previous.len());
        for &old in &previous {
            let mut ancestor = old.parent();
            while let Some(address) = ancestor {
                previously_refined.insert(address);
                ancestor = address.parent();
            }
        }
        let mut score_cache = HashMap::new();
        let mut selected_priorities = Vec::new();

        loop {
            let mut candidates = Vec::new();
            for address in &cover {
                let (error, priority, approach, speed) =
                    self.selection_score(*address, &mut score_cache);
                let was_refined = previously_refined.contains(address);
                let threshold = if was_refined {
                    self.config.merge_threshold_px
                } else {
                    self.config.split_threshold_px
                };
                if address.level() >= self.config.max_level || error <= threshold {
                    continue;
                }
                if self.too_transient(*address) {
                    continue;
                }
                let Ok(children) = address.children() else {
                    continue;
                };
                let total = priority;
                candidates.push((total, error, *address, children, approach, speed));
            }
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
            let mut accepted = None;
            let had_split_candidate = !candidates.is_empty();
            for (total, error, parent, children, approach, speed) in candidates {
                let mut proposed: Vec<_> = cover
                    .iter()
                    .copied()
                    .filter(|item| *item != parent)
                    .collect();
                proposed.extend(children);
                if let Some(balanced) = balance_cover(
                    &self.config.roots,
                    proposed,
                    self.config.max_level,
                    self.config.max_desired_patches,
                    None,
                ) && balanced.len() > cover.len()
                {
                    accepted = Some((balanced, total, error, parent, approach, speed));
                    break;
                }
            }
            let Some((next, total, error, parent, approach, speed)) = accepted else {
                self.pressure.desired_capacity |= had_split_candidate;
                break;
            };
            selected_priorities.push(RegionalPrioritySnapshot {
                address: format!("{parent:?}"),
                projected_error_px: error,
                approach_multiplier: approach,
                high_speed_multiplier: speed,
                total,
            });
            cover = next;
        }

        self.desired.clear();
        for address in &cover {
            let (error, priority, approach, speed) =
                self.selection_score(*address, &mut score_cache);
            self.desired.insert(
                *address,
                RegionalPatchSnapshot {
                    address: format!("{address:?}"),
                    level: address.level(),
                    projected_error_px: error,
                    priority,
                    state: "absent",
                },
            );
            if self.last_priorities.len() < self.config.admission_cap_per_tick {
                let record = RegionalPrioritySnapshot {
                    address: format!("{address:?}"),
                    projected_error_px: error,
                    approach_multiplier: approach,
                    high_speed_multiplier: speed,
                    total: priority,
                };
                if !selected_priorities
                    .iter()
                    .any(|entry| entry.address == record.address)
                {
                    selected_priorities.push(record);
                }
            }
        }
        selected_priorities
            .sort_by(|a, b| b.total.total_cmp(&a.total).then(a.address.cmp(&b.address)));
        self.last_priorities = selected_priorities;
        self.desired_addresses = self.desired.keys().copied().collect();
        let active_addresses: HashSet<_> = self.desired.keys().copied().collect();
        self.seen_cache_requests
            .retain(|key| active_addresses.contains(&key.address));
        self.selector_can_reuse_fixed_point = self.desired_addresses == previous_cover;
        self.selector_last_capacity_pressure = self.pressure.desired_capacity;
        self.last_selection_view = Some(self.view);
        self.last_selection_planetary_view = self.planetary_view;
    }

    /// Planetary topology moves toward its current target in a small number of
    /// complete, balanced cover transactions per tick. The desired cover stays
    /// complete across all six roots while invisible leaves simply have zero
    /// refinement priority.
    fn select_planetary_desired(&mut self) {
        const SELECTOR_BUDGET: Duration = Duration::from_millis(2);
        const MAX_TOPOLOGY_OPERATIONS_PER_TICK: usize = 8;
        const MAX_CANDIDATES_PER_TICK: usize = 8;
        const MAX_SCORES_PER_TICK: usize = 256;
        let selection_started = std::time::Instant::now();
        self.selector_budget_exhausted = false;
        self.planetary_topology_operations = 0;
        self.planetary_split_candidates_scanned = 0;
        self.selector_merge_scan_pending = false;

        let previous: Vec<_> = self.desired.keys().copied().collect();
        let mut cover = if previous.is_empty() {
            let mut roots = self.config.roots.clone();
            roots.sort();
            roots
        } else {
            previous.clone()
        };
        let mut score_cache = HashMap::new();
        let mut selected_priorities = Vec::new();
        let mut changed = false;
        let mut operations = 0;

        // Release a fully culled sibling group before spending the frame
        // budget on refinement scoring. This makes camera retreat responsive.
        if cover.len() > self.config.roots.len()
            && selection_started.elapsed() < SELECTOR_BUDGET
            && let Some((next, priority, error, parent, approach, speed)) = self
                .best_planetary_merge(&cover, &mut score_cache, selection_started, SELECTOR_BUDGET)
        {
            cover = next;
            changed = true;
            operations += 1;
            self.planetary_topology_operations = operations;
            selected_priorities.push(RegionalPrioritySnapshot {
                address: format!("{parent:?}"),
                projected_error_px: error,
                approach_multiplier: approach,
                high_speed_multiplier: speed,
                total: priority,
            });
        }
        let mut candidates = Vec::new();
        let now = Instant::now();
        // Score a bounded slice before the full settling bookkeeping below.
        // A whole-cover "nothing to refine" scan here can consume the deadline
        // on every tick and permanently starve the last few required splits.
        if !cover.is_empty() {
            let start = self.planetary_score_cursor % cover.len();
            let mut scanned = 0usize;
            while scanned < cover.len().min(MAX_SCORES_PER_TICK) {
                if selection_started.elapsed() >= SELECTOR_BUDGET {
                    self.selector_budget_exhausted = true;
                    break;
                }
                let address = cover[(start + scanned) % cover.len()];
                scanned += 1;
                let (error, priority, approach, speed) =
                    self.selection_score(address, &mut score_cache);
                if address.level() >= self.config.max_level
                    || error <= self.config.split_threshold_px
                    || self.too_transient(address)
                {
                    continue;
                }
                if let Ok(children) = address.children() {
                    let rank = self.selection_completion_rank(address, error, priority, now);
                    candidates.push((rank, priority, error, address, children, approach, speed));
                }
            }
            self.planetary_score_cursor = (start + scanned) % cover.len();
            self.planetary_split_candidates_scanned = scanned;
        }
        candidates.sort_by(|a, b| a.0.compare(b.0).then_with(|| a.3.cmp(&b.3)));

        let had_split_candidate = !candidates.is_empty();
        let mut accepted = None;
        let mut all_candidates_attempted = false;
        if had_split_candidate {
            // A split adds three leaves before balancing. Near capacity no
            // candidate can fit, so avoid copying and balancing the full cover.
            if cover.len().saturating_add(3) > self.config.max_desired_patches {
                self.pressure.desired_capacity = !candidates.is_empty();
            } else if !candidates.is_empty() {
                let start = self.planetary_candidate_cursor % candidates.len();
                let attempts = candidates.len().min(MAX_CANDIDATES_PER_TICK);
                all_candidates_attempted = attempts == candidates.len();
                for offset in 0..attempts {
                    if selection_started.elapsed() >= SELECTOR_BUDGET {
                        self.selector_budget_exhausted = true;
                        break;
                    }
                    let (_rank, priority, error, parent, _children, approach, speed) =
                        candidates[(start + offset) % candidates.len()];
                    if let Some(balanced) = balance_planetary_split(
                        &cover,
                        parent,
                        self.config.max_level,
                        self.config.max_desired_patches,
                        MAX_TOPOLOGY_OPERATIONS_PER_TICK.saturating_sub(operations),
                        selection_started,
                        SELECTOR_BUDGET,
                    ) && balanced.len() > cover.len()
                    {
                        let split_cost = (balanced.len() - cover.len()) / 3;
                        if split_cost <= MAX_TOPOLOGY_OPERATIONS_PER_TICK {
                            accepted = Some((
                                balanced, split_cost, priority, error, parent, approach, speed,
                            ));
                            // A successful change invalidates the old ranking.
                            self.planetary_candidate_cursor = 0;
                            break;
                        }
                    }
                    if selection_started.elapsed() >= SELECTOR_BUDGET {
                        self.selector_budget_exhausted = true;
                        break;
                    }
                    self.planetary_candidate_cursor = (start + offset + 1) % candidates.len();
                }
                if candidates.len() > attempts && accepted.is_none() {
                    self.planetary_candidate_cursor = (start + attempts) % candidates.len();
                }
            }
        }

        if let Some((next, split_cost, priority, error, parent, approach, speed)) = accepted {
            changed = true;
            operations += split_cost;
            self.planetary_topology_operations = operations;
            selected_priorities.push(RegionalPrioritySnapshot {
                address: format!("{parent:?}"),
                projected_error_px: error,
                approach_multiplier: approach,
                high_speed_multiplier: speed,
                total: priority,
            });
            cover = next;
        } else {
            self.pressure.desired_capacity |=
                had_split_candidate && all_candidates_attempted && !self.selector_budget_exhausted;
        }

        // Advance the merge cursor once per tick. A second pass can restart a
        // just-completed scan and erase its completion before settling checks.
        if selection_started.elapsed() >= SELECTOR_BUDGET {
            self.selector_budget_exhausted = true;
        }

        if cover == previous {
            for (address, (error, priority, _, _)) in &score_cache {
                if let Some(patch) = self.desired.get_mut(address) {
                    patch.projected_error_px = *error;
                    patch.priority = *priority;
                }
            }
        } else {
            let mut next_desired = BTreeMap::new();
            for &address in &cover {
                let (error, priority, _, _) = if let Some(score) =
                    score_cache.get(&address).copied()
                {
                    score
                } else if let Some(score) = self.planetary_scores.borrow().get(&address).copied() {
                    score
                } else if selection_started.elapsed() < SELECTOR_BUDGET {
                    self.selection_score(address, &mut score_cache)
                } else {
                    self.selector_budget_exhausted = true;
                    self.desired
                        .get(&address)
                        .map(|patch| (patch.projected_error_px, patch.priority, 1.0, 1.0))
                        .or_else(|| {
                            address
                                .parent()
                                .and_then(|parent| self.desired.get(&parent))
                                .map(|patch| (patch.projected_error_px, patch.priority, 1.0, 1.0))
                        })
                        .unwrap_or((self.config.split_threshold_px + 1.0, 0.0, 1.0, 1.0))
                };
                next_desired.insert(
                    address,
                    RegionalPatchSnapshot {
                        address: format!("{address:?}"),
                        level: address.level(),
                        projected_error_px: error,
                        priority,
                        state: "absent",
                    },
                );
            }
            self.desired = next_desired;
            self.desired_addresses = cover;
        }
        selected_priorities.sort_by(|a, b| {
            b.total
                .total_cmp(&a.total)
                .then_with(|| a.address.cmp(&b.address))
        });
        selected_priorities.truncate(self.config.admission_cap_per_tick);
        self.last_priorities = selected_priorities;
        let active_addresses: HashSet<_> = self.desired.keys().copied().collect();
        self.seen_cache_requests
            .retain(|key| active_addresses.contains(&key.address));
        if changed {
            // A topology edit changes the set of possible sibling groups. A
            // complete subsequent merge scan is required before reuse/settling.
            self.merge_scan_cursor = None;
            // Keep the independent culling cursor moving across topology edits;
            // repeated refinement must not starve obsolete regions later in order.
            self.selector_merge_scan_pending = true;
        }
        let has_more_work = self.desired.iter().any(|(address, patch)| {
            address.level() < self.config.max_level
                && patch.projected_error_px > self.config.split_threshold_px
                && !self.too_transient(*address)
        });
        self.selector_can_reuse_fixed_point = !changed
            && self
                .desired
                .keys()
                .all(|address| self.planetary_scores.borrow().contains_key(address))
            && (!has_more_work || self.pressure.desired_capacity)
            && !self.selector_merge_scan_pending;
        // A completed score/merge pass can be reused even if final bookkeeping
        // exceeded the deadline. This frame still reports exhaustion; the next
        // unchanged tick needs no selection work. Incomplete passes cannot reuse.
        self.selector_last_capacity_pressure = self.pressure.desired_capacity;
        self.selector_fixed_point_reused = false;
        self.last_selection_view = Some(self.view);
        self.last_selection_planetary_view = self.planetary_view;
        self.prune_completion_ages();
    }

    fn best_planetary_merge(
        &mut self,
        cover: &[CubePatchAddress],
        score_cache: &mut HashMap<CubePatchAddress, (f64, f64, f64, f64)>,
        selection_started: std::time::Instant,
        selection_budget: Duration,
    ) -> Option<(Vec<CubePatchAddress>, f64, f64, CubePatchAddress, f64, f64)> {
        const MAX_MERGE_CANDIDATES_PER_TICK: usize = 8;
        const MAX_CULLED_GROUPS_PER_TICK: usize = 8;
        if self.planetary_merge_index_cover != cover {
            self.planetary_merge_index_cover = cover.to_vec();
            self.planetary_merge_index_parents = cover
                .iter()
                .filter_map(|child| child.parent())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            self.planetary_merge_index_leaves = cover.iter().copied().collect();
        }
        let parents = &self.planetary_merge_index_parents;
        if parents.is_empty() {
            self.selector_merge_scan_pending = false;
            self.merge_scan_cursor = None;
            return None;
        }
        let cover_set = &self.planetary_merge_index_leaves;
        // The current cover's score pass already classified these children.
        // Retire any fully culled sibling group before spending the bounded
        // candidate budget on projected-error checks for visible regions.
        let cull_start = self.cull_scan_cursor.map_or(0, |cursor| {
            parents.partition_point(|parent| *parent <= cursor)
        }) % parents.len();
        let cull_count = parents.len().min(MAX_CULLED_GROUPS_PER_TICK);
        for offset in 0..cull_count {
            if selection_started.elapsed() >= selection_budget {
                self.selector_budget_exhausted = true;
                break;
            }
            let parent = parents[(cull_start + offset) % parents.len()];
            self.cull_scan_cursor = Some(parent);
            let Ok(children) = parent.children() else {
                continue;
            };
            if !children.iter().all(|child| cover_set.contains(child))
                || !children.iter().all(|child| !self.patch_visible(*child))
                || self.patch_visible(parent)
                || !planetary_merge_is_balanced(parent, cover_set)
            {
                continue;
            }
            let mut next: Vec<_> = cover
                .iter()
                .copied()
                .filter(|patch| !children.contains(patch))
                .chain([parent])
                .collect();
            next.sort();
            self.merge_scan_cursor = Some(parent);
            self.selector_merge_scan_pending = false;
            return Some((next, 0.0, 0.0, parent, 1.0, 1.0));
        }
        if selection_started.elapsed() >= selection_budget {
            self.selector_budget_exhausted = true;
            self.selector_merge_scan_pending = true;
            return None;
        }
        let mut start = self.merge_scan_cursor.map_or(0, |cursor| {
            parents.partition_point(|parent| *parent <= cursor)
        });
        if start == parents.len() {
            start = 0;
        }
        let count = parents.len().min(MAX_MERGE_CANDIDATES_PER_TICK);
        let scan_completed = start + count >= parents.len();
        self.selector_merge_scan_pending = !scan_completed;
        let scanned: Vec<_> = (0..count)
            .map(|offset| parents[(start + offset) % parents.len()])
            .collect();
        self.merge_scan_cursor = scanned.last().copied();
        for parent in scanned {
            let Ok(children) = parent.children() else {
                continue;
            };
            if !children.iter().all(|child| cover_set.contains(child)) {
                continue;
            }
            let (error, priority, approach, speed) = self.selection_score(parent, score_cache);
            if error >= self.config.merge_threshold_px
                || !planetary_merge_is_balanced(parent, cover_set)
            {
                continue;
            }
            let mut next: Vec<_> = cover
                .iter()
                .copied()
                .filter(|patch| !children.contains(patch))
                .chain([parent])
                .collect();
            next.sort();
            return Some((next, priority, error, parent, approach, speed));
        }
        None
    }

    fn patch_visible_with(
        &self,
        address: CubePatchAddress,
        planetary_view: Option<PlanetaryView>,
    ) -> bool {
        let Some(planetary_view) = planetary_view else {
            return true;
        };
        let metadata = {
            let mut cache = self.patch_metadata.borrow_mut();
            if cache.len() >= self.config.max_desired_patches.saturating_mul(2).max(8192)
                && !cache.contains_key(&address)
                && let Some(evicted) = cache.keys().next().copied()
            {
                cache.remove(&evicted);
            }
            if let Some(metadata) = cache.get(&address).copied() {
                Some(metadata)
            } else {
                PatchMetadata::build(address, &self.surface_topology)
                    .ok()
                    .inspect(|metadata| {
                        cache.insert(address, *metadata);
                    })
            }
        };
        let Some(metadata) = metadata else {
            return true;
        };
        let radius = self.generator.radius_m();
        let amplitude = self.generator.conservative_absolute_height_bound_m();
        let extent = SurfaceExtent {
            min_height_m: -amplitude,
            max_height_m: amplitude,
            guaranteed_opaque_radius_m: 0.0,
        };
        let Ok((center_body, bound_radius)) = metadata.ball(radius, extent) else {
            return true;
        };
        let camera_view = planetary_view.body_to_view * self.view.body_position_m;
        let center_view = planetary_view.body_to_view * center_body - camera_view;
        if planetary_view
            .projection
            .rejects_ball(center_view, bound_radius)
            .unwrap_or(false)
        {
            return false;
        }

        // The lower radial envelope forms a conservative inscribed occluder for
        // the closed displaced body. If it cannot reject the patch, retain it.
        let inner_radius = radius - amplitude;
        if inner_radius > 0.0
            && metadata.horizon_reject(
                self.view.body_position_m,
                inner_radius,
                SurfaceExtent::smooth(inner_radius),
            )
        {
            return false;
        }
        true
    }

    fn selection_score(
        &self,
        address: CubePatchAddress,
        cache: &mut HashMap<CubePatchAddress, (f64, f64, f64, f64)>,
    ) -> (f64, f64, f64, f64) {
        if let Some(score) = cache.get(&address).copied() {
            return score;
        }
        if self.planetary_view.is_some()
            && let Some(score) = self.planetary_scores.borrow().get(&address).copied()
        {
            cache.insert(address, score);
            return score;
        }
        let score = self.score(address);
        cache.insert(address, score);
        if self.planetary_view.is_some() {
            let mut scores = self.planetary_scores.borrow_mut();
            if scores.len() >= self.config.max_desired_patches.saturating_mul(2).max(8192)
                && !scores.contains_key(&address)
                && let Some(evicted) = scores
                    .keys()
                    .find(|candidate| !self.desired.contains_key(candidate))
                    .copied()
            {
                scores.remove(&evicted);
            }
            scores.insert(address, score);
        }
        score
    }

    fn selection_completion_rank(
        &self,
        address: CubePatchAddress,
        error_px: f64,
        legacy_priority: f64,
        now: Instant,
    ) -> CompletionRank {
        if self.planetary_view.is_none() {
            return CompletionRank {
                priority: legacy_priority,
                under_floor: false,
                age: Duration::ZERO,
            };
        }
        let area = self.projected_footprint_weight(address);
        let age = if self.drawable.contains(&address) && area > 0.0 {
            self.begin_completion_eligibility(address, now)
        } else {
            Duration::ZERO
        };
        completion_group_rank(error_px, self.config.split_threshold_px, area, age)
    }

    fn completion_rank_map(&self) -> HashMap<CubePatchAddress, CompletionRank> {
        let mut ranks = HashMap::new();
        if self.planetary_view.is_none() {
            return ranks;
        }
        for group in self.split_frontier_groups() {
            let rank = CompletionRank {
                priority: group.completion_priority,
                under_floor: group.useful_floor_deficit_px > 0.0,
                age: group.age,
            };
            for child in group.children {
                ranks.insert(child, rank);
            }
        }
        ranks
    }

    fn completion_rank_for_address(
        &self,
        address: CubePatchAddress,
        group_ranks: &HashMap<CubePatchAddress, CompletionRank>,
    ) -> CompletionRank {
        if let Some(rank) = group_ranks.get(&address).copied() {
            return rank;
        }
        let (error, legacy_priority, _, _) = self
            .desired
            .get(&address)
            .map(|patch| (patch.projected_error_px, patch.priority, 1.0, 1.0))
            .unwrap_or_else(|| self.score(address));
        self.selection_completion_rank(address, error, legacy_priority, Instant::now())
    }

    fn prune_completion_ages(&self) {
        if self.planetary_view.is_none() {
            self.completion_group_first_eligible.borrow_mut().clear();
            return;
        }
        let mut ages = self.completion_group_first_eligible.borrow_mut();
        ages.retain(|parent, _| {
            self.drawable.contains(parent)
                && parent.level() < self.config.max_level
                && !self.too_transient(*parent)
                && self.score(*parent).0 > self.config.split_threshold_px
        });
    }

    fn score(&self, address: CubePatchAddress) -> (f64, f64, f64, f64) {
        if !self.patch_visible(address) {
            return (0.0, 0.0, 1.0, 1.0);
        }
        let [normal, u, v] = address.face().basis();
        let [x, y] = address.coordinates();
        let scale = (1u64 << address.level()) as f64;
        let center = (normal
            + u * (2.0 * (f64::from(x) + 0.5) / scale - 1.0)
            + v * (2.0 * (f64::from(y) + 0.5) / scale - 1.0))
            .normalize();
        let radius = self.generator.radius_m();
        let center_m = center * radius;
        let to_patch = center_m - self.view.body_position_m;
        let distance = to_patch.length().max(radius * 1.0e-9);
        let angular_width = 2.0 / scale;
        let grid_step = angular_width / f64::from(self.config.cells);
        let half_cell_diagonal = (2.0f64).sqrt() * grid_step * 0.5;
        let patch_half_diagonal = (2.0f64).sqrt() * angular_width * 0.5;
        let curvature_m = radius * (1.0 - half_cell_diagonal.cos());
        // Relief is a patch-scale envelope proxy, not a derivative bound; it
        // decreases as subdivision narrows the region. Cell spacing controls
        // the separate spherical curvature term above.
        let authored_relief_m = self.generator.radius_m()
            * self
                .generator
                .definition()
                .terrain()
                .parameters()
                .relief_fraction
            * 0.25;
        let relief_envelope_m =
            authored_relief_m.min(self.generator.conservative_absolute_height_bound_m());
        let relief_m = relief_envelope_m * patch_half_diagonal.sin().abs();
        let geometric_error_m = curvature_m + relief_m;
        let approach_speed = self
            .view
            .body_velocity_mps
            .dot(to_patch / distance)
            .max(0.0);
        // Approximate the closest point on this patch's angular footprint. A
        // global absolute-height bound here overwhelms the footprint distance
        // for every patch at low altitude and destroys spatial LOD variation.
        let camera_radius = self.view.body_position_m.length();
        let clearance = (camera_radius - radius).max(1.0);
        let camera_direction = self.view.body_position_m.normalize_or_zero();
        let patch_angle = camera_direction.dot(center).clamp(-1.0, 1.0).acos();
        let nearest_angle = (patch_angle - patch_half_diagonal).max(0.0);
        let local_surface_radius = radius + relief_m;
        let closest_surface_distance = (camera_radius * camera_radius
            + local_surface_radius * local_surface_radius
            - 2.0 * camera_radius * local_surface_radius * nearest_angle.cos())
        .max(0.0)
        .sqrt();
        let nearest_patch_distance = closest_surface_distance.max(clearance);
        let predicted_distance = (nearest_patch_distance
            - approach_speed * self.config.prediction_seconds)
            .max(clearance);
        let projection_scale_px = self
            .planetary_view
            .map(|view| view.projection.focal_pixels())
            .unwrap_or(self.view.projection_scale_px);
        let error_px = geometric_error_m * projection_scale_px / predicted_distance;
        let approach_multiplier = 1.0
            + (approach_speed * self.config.prediction_seconds / nearest_patch_distance.max(1.0))
                .clamp(0.0, 2.0);
        let speed = self.view.body_velocity_mps.length();
        let high_speed_multiplier = if speed > self.config.high_speed_mps && address.level() > 0 {
            (self.config.high_speed_mps / speed).clamp(0.15, 1.0)
        } else {
            1.0
        };
        let priority = error_px * approach_multiplier * high_speed_multiplier;
        (
            error_px,
            priority,
            approach_multiplier,
            high_speed_multiplier,
        )
    }

    fn too_transient(&self, address: CubePatchAddress) -> bool {
        let speed = self.view.body_velocity_mps.length();
        if speed <= self.config.high_speed_mps || address.level() == 0 {
            return false;
        }
        let angular_width = 2.0 / (1u64 << address.level()) as f64;
        let patch_width_m = self.generator.radius_m() * angular_width;
        patch_width_m / speed < self.config.prediction_seconds * 0.25
    }

    fn cancel_obsolete_work(&mut self) {
        let mut needed = self.scheduled_dependencies();
        needed.extend(self.external_pins.iter().copied());
        for (key, token) in &self.in_flight {
            if !needed.contains(&key.address) && !token.cancelled.swap(true, AtomicOrdering::AcqRel)
            {
                self.trace.event_key(key, Stage::Superseded);
                if token.started.load(AtomicOrdering::Acquire) {
                    self.stats.cancelled_during_work =
                        self.stats.cancelled_during_work.saturating_add(1);
                } else {
                    self.stats.cancelled_before_start =
                        self.stats.cancelled_before_start.saturating_add(1);
                }
            }
        }
    }

    fn admit_builds(&mut self) {
        let drawable: HashSet<_> = self.drawable.iter().copied().collect();
        let merge_frontier = self.merge_parent_dependencies();
        let group_ranks = self.completion_rank_map();
        let addresses: Vec<_> = self.scheduled_dependencies().into_iter().collect();
        let mut candidates = Vec::new();
        for address in addresses {
            if self.resident.contains(&address) {
                continue;
            }
            if self.reserved_split_parent.is_some_and(|parent| {
                !drawable.contains(&address)
                    && !merge_frontier.contains(&address)
                    && parent
                        .children()
                        .is_ok_and(|children| !children.contains(&address))
            }) {
                continue;
            }
            let in_current_frontier = if self.drawable.is_empty() {
                self.config.roots.contains(&address)
            } else {
                drawable.contains(&address)
                    || merge_frontier.contains(&address)
                    || address
                        .parent()
                        .is_some_and(|parent| drawable.contains(&parent))
            };
            if !in_current_frontier {
                continue;
            }
            let Ok(key) = self.key_for(address) else {
                continue;
            };
            if self.cache.contains_key(&key) {
                if self.seen_cache_requests.insert(key) {
                    self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
                }
                continue;
            }
            if self.in_flight.contains_key(&key)
                || self.uploads.iter().any(|upload| upload.address == address)
            {
                continue;
            }
            let rank = self.completion_rank_for_address(address, &group_ranks);
            candidates.push((rank, address, key));
        }
        candidates.sort_by(|a, b| a.0.compare(b.0).then(a.1.cmp(&b.1)));
        let now = Instant::now();
        if self.planetary_view.is_none() {
            let wait_candidates: Vec<_> = candidates
                .iter()
                .map(|(rank, address, key)| (rank.priority, *address, key.clone()))
                .collect();
            self.refresh_build_candidate_waits(&wait_candidates, now);
            if let Some(index) = oldest_starved_candidate_index(
                candidates.iter().map(|(_, _, key)| key),
                &self.build_candidate_waits,
                now,
            ) && index > 0
            {
                candidates[..=index].rotate_right(1);
            }
        } else {
            self.build_candidate_waits.clear();
        }
        self.trace
            .sync_generation_candidates(candidates.iter().map(|(_, _, key)| key));
        if !candidates.is_empty() && !self.cpu_cache_can_admit_tile() {
            self.pressure.cpu = true;
            for (_, address, key) in candidates {
                self.trace.admit(&key);
                self.trace.block(address, BlockReason::CpuCapacity);
            }
            return;
        }
        self.pressure.queue = self.in_flight.len() >= self.config.queue_cap;
        let mut admitted = 0;
        self.last_priorities.clear();
        for (rank, address, key) in candidates {
            if admitted >= self.config.admission_cap_per_tick {
                break;
            }
            let token = Arc::new(BuildToken {
                cancelled: AtomicBool::new(false),
                started: AtomicBool::new(false),
            });
            let job = BuildJob {
                address,
                key: key.clone(),
                token: Arc::clone(&token),
            };
            self.trace.admit(&key);
            match self.workers.sender.try_send(job) {
                Ok(()) => {
                    self.trace.event_key(&key, Stage::GenerationQueued);
                    self.trace.unblock(address);
                    self.build_candidate_waits.remove(&key);
                    self.in_flight.insert(key, token);
                    self.stats.cache_misses = self.stats.cache_misses.saturating_add(1);
                    self.stats.requests_issued = self.stats.requests_issued.saturating_add(1);
                    admitted += 1;
                    let (error, _, approach, speed) = self.score(address);
                    self.last_priorities.push(RegionalPrioritySnapshot {
                        address: format!("{address:?}"),
                        projected_error_px: error,
                        approach_multiplier: approach,
                        high_speed_multiplier: speed,
                        total: rank.priority,
                    });
                }
                Err(TrySendError::Full(_)) => {
                    self.trace.block(address, BlockReason::WorkerQueueFull);
                    self.pressure.queue = true;
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.trace.block(address, BlockReason::WorkerQueueFull);
                    self.pressure.queue = true;
                    break;
                }
            }
        }
        if candidates_remain(&self.desired, &self.cache, &self.in_flight, &self.resident) {
            self.pressure.queue |= admitted == self.config.admission_cap_per_tick;
        }
    }

    fn refresh_build_candidate_waits(
        &mut self,
        candidates: &[(f64, CubePatchAddress, TileKey)],
        now: Instant,
    ) {
        self.build_candidate_wait_epoch = self.build_candidate_wait_epoch.wrapping_add(1).max(1);
        let epoch = self.build_candidate_wait_epoch;
        for (_, _, key) in candidates {
            let wait =
                self.build_candidate_waits
                    .entry(key.clone())
                    .or_insert(BuildCandidateWait {
                        first_eligible: now,
                        last_seen_epoch: epoch,
                    });
            wait.last_seen_epoch = epoch;
        }
        self.build_candidate_waits
            .retain(|_, wait| wait.last_seen_epoch == epoch);
    }

    fn drain_completions(&mut self) {
        let drain_started = std::time::Instant::now();
        let mut needed = None;
        while let Ok(result) = self.completions.try_recv() {
            let needed = needed.get_or_insert_with(|| self.scheduled_dependencies());
            self.completion_backlog.fetch_sub(1, AtomicOrdering::AcqRel);
            self.in_flight.remove(&result.key);
            self.stats.worker_elapsed_micros = self
                .stats
                .worker_elapsed_micros
                .saturating_add(result.elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
            self.stats.cancelled_attempt_sample_evaluations = self
                .stats
                .cancelled_attempt_sample_evaluations
                .saturating_add(result.cancelled_sample_evaluations);
            self.stats.cancelled_attempt_preparation_bytes = self
                .stats
                .cancelled_attempt_preparation_bytes
                .saturating_add(result.cancelled_preparation_bytes);
            self.stats.worker_query_workspace_bound_bytes = self
                .stats
                .worker_query_workspace_bound_bytes
                .max(result.query_workspace_bound_bytes);
            self.stats.prepared_store_stats = result.prepared_store_stats;
            self.stats
                .recent_build_elapsed_micros
                .push_back(result.elapsed.as_micros().min(u128::from(u64::MAX)) as u64);
            if self.stats.recent_build_elapsed_micros.len() > 256 {
                self.stats.recent_build_elapsed_micros.pop_front();
            }
            let still_needed =
                needed.contains(&result.address) || self.external_pins.contains(&result.address);
            if result.token.started.load(AtomicOrdering::Acquire) {
                self.stats.jobs_started = self.stats.jobs_started.saturating_add(1);
            }
            if result.token.cancelled.load(AtomicOrdering::Acquire) && !still_needed {
                self.stats.stale_completions = self.stats.stale_completions.saturating_add(1);
                self.trace.event_key(&result.key, Stage::StaleCompletion);
                self.trace.event_key(&result.key, Stage::DiscardedStale);
            }
            if let Some((tile, diagnostics)) = result.result {
                self.stats.completed_build_count =
                    self.stats.completed_build_count.saturating_add(1);
                self.stats.rebuilds = self.stats.rebuilds.saturating_add(1);
                self.stats.build_time_micros = self.stats.build_time_micros.saturating_add(
                    diagnostics.elapsed.as_micros().min(u128::from(u64::MAX)) as u64,
                );
                if !still_needed {
                    self.stats.completed_unused_tiles =
                        self.stats.completed_unused_tiles.saturating_add(1);
                }
                let bytes = tile_bytes(&tile);
                let build_elapsed_micros =
                    diagnostics.elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
                match self.cache_insert(tile, !still_needed, Some(build_elapsed_micros)) {
                    CacheInsertResult::Inserted => {}
                    CacheInsertResult::AlreadyCached | CacheInsertResult::Rejected => {
                        self.record_discarded_build(bytes, build_elapsed_micros);
                    }
                }
            } else if !result.token.cancelled.load(AtomicOrdering::Acquire) {
                self.stats.build_failures = self.stats.build_failures.saturating_add(1);
            }
        }
        self.stats.completion_drain_micros = drain_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
    }

    fn admit_uploads(&mut self) {
        let candidates = self.upload_candidates();
        self.trace.sync_upload_candidates(
            self.uploads
                .iter()
                .map(|upload| &upload.tile.key)
                .chain(candidates.iter().map(|(_, _, tile)| &tile.key)),
        );
        for (_, address, tile) in candidates {
            if self.uploads.len() >= self.config.upload_tile_cap {
                self.trace.block(address, BlockReason::UploadCapacity);
                self.pressure.upload = true;
                break;
            }
            let bytes = tile_bytes(&tile);
            if self.upload_bytes.saturating_add(bytes) > self.config.upload_byte_cap {
                self.trace.block(address, BlockReason::UploadCapacity);
                self.pressure.upload = true;
                continue;
            }
            self.trace.unblock(address);
            self.uploads.push(RegionalUpload {
                address,
                tile,
                bytes,
            });
            self.upload_bytes += bytes;
        }
    }

    fn upload_candidates(&self) -> Vec<(CompletionRank, CubePatchAddress, Arc<TileData>)> {
        let _span = crate::engine_profile::span("Upload candidate indexing");
        let drawable: HashSet<_> = self.drawable.iter().copied().collect();
        let merge_frontier = self.merge_parent_dependencies();
        let group_ranks = self.completion_rank_map();
        let desired_ancestor_index = self
            .reserved_split_parent
            .is_none()
            .then(|| self.desired_priority_by_ancestor());
        let reserved_children = self
            .reserved_split_parent
            .and_then(|parent| parent.children().ok());
        let mut candidates = Vec::new();
        for (&address, tile) in &self.cached_tiles_by_address {
            if self.resident.contains(&address)
                || self.uploads.iter().any(|upload| upload.address == address)
            {
                continue;
            }

            let is_root = self.config.roots.contains(&address);
            let is_merge_parent = merge_frontier.contains(&address);
            let is_desired_dependency = if self.reserved_split_parent.is_some() {
                // scheduled_dependencies() retains exactly these bounded anchors
                // when a split reservation is active. The caller's original
                // wanted set also independently includes desired addresses.
                self.desired.contains_key(&address)
                    || is_root
                    || drawable.contains(&address)
                    || self.external_pins.contains(&address)
                    || is_merge_parent
                    || reserved_children.is_some_and(|children| children.contains(&address))
            } else {
                // dependency_addresses() includes the desired nodes and their
                // ancestors only down to the configured root. The ancestor index
                // is shared with frontier discovery; root clipping preserves the
                // original finite-region dependency contract.
                self.desired.contains_key(&address)
                    || is_root
                    || is_merge_parent
                    || (self.config.roots.iter().any(|root| root.contains(address))
                        && desired_ancestor_index
                            .as_ref()
                            .is_some_and(|index| index.contains_key(&address)))
            };
            if !is_desired_dependency {
                continue;
            }
            if self.drawable.is_empty() {
                if !is_root {
                    continue;
                }
            } else if !drawable.contains(&address)
                && !is_merge_parent
                && !address
                    .parent()
                    .is_some_and(|parent| drawable.contains(&parent))
            {
                continue;
            }
            if self.reserved_split_parent.is_some()
                && !drawable.contains(&address)
                && !is_merge_parent
                && !reserved_children.is_some_and(|children| children.contains(&address))
            {
                continue;
            }
            if self
                .upload_allowlist
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&address))
            {
                continue;
            }
            let rank = self.completion_rank_for_address(address, &group_ranks);
            candidates.push((rank, address, Arc::clone(tile)));
        }
        drop(desired_ancestor_index);
        candidates.sort_by(|a, b| a.0.compare(b.0).then(a.1.cmp(&b.1)));
        candidates
    }

    fn refresh_publications(&mut self) {
        let started = std::time::Instant::now();
        self.refresh_publications_inner();
        self.stats.publication_discovery_micros = self
            .stats
            .publication_discovery_micros
            .saturating_add(started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
    }

    fn refresh_publications_inner(&mut self) {
        self.publications.clear();
        self.publications_dirty = false;
        const MAX_CANDIDATES: usize = 32;
        let publication_limit = if self.planetary_view.is_some() {
            MAX_CANDIDATES
        } else {
            self.config
                .publication_cap_per_tick
                .min(self.config.transition_cap)
                .min(MAX_CANDIDATES)
        };
        if self.drawable.is_empty() || publication_limit == 0 {
            return;
        }
        {
            let mut cached = self.desired_dependency_cache.borrow_mut();
            if !self.desired.keys().eq(cached.0.iter()) {
                cached.0 = self.desired.keys().copied().collect();
                cached.1.clear();
                for &desired in self.desired.keys() {
                    let mut cursor = desired;
                    loop {
                        if !self.config.roots.iter().any(|root| root.contains(cursor))
                            || !cached.1.insert(cursor)
                        {
                            break;
                        }
                        if self.config.roots.contains(&cursor) {
                            break;
                        }
                        let Some(parent) = cursor.parent() else {
                            break;
                        };
                        cursor = parent;
                    }
                }
            }
        }
        let desired_ancestors = self.desired_dependency_cache.borrow();
        let mut candidates = Vec::new();
        let mut current_candidate_parents = HashSet::new();
        for &parent in &self.drawable {
            let has_desired_descendant =
                desired_ancestors.1.contains(&parent) && !self.desired.contains_key(&parent);
            let Ok(children) = parent.children() else {
                continue;
            };
            if has_desired_descendant
                && self.resident.contains(&parent)
                && children.iter().all(|child| self.resident.contains(child))
                && local_replacement_is_balanced(parent, true, &self.drawable, &self.config.roots)
            {
                current_candidate_parents.insert(parent);
                candidates.push(RegionalPublication::Split {
                    parent,
                    children,
                    cover: Vec::new(),
                });
            }
        }
        let parents: BTreeSet<_> = self
            .drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect();
        for parent in parents {
            let mut ancestor = Some(parent);
            let desired_contains_parent = std::iter::from_fn(|| {
                let current = ancestor?;
                ancestor = current.parent();
                Some(current)
            })
            .any(|address| self.desired.contains_key(&address));
            let Ok(children) = parent.children() else {
                continue;
            };
            if desired_contains_parent
                && self.resident.contains(&parent)
                && children
                    .iter()
                    .all(|child| self.drawable.binary_search(child).is_ok())
                && local_replacement_is_balanced(parent, false, &self.drawable, &self.config.roots)
            {
                current_candidate_parents.insert(parent);
                candidates.push(RegionalPublication::Merge {
                    parent,
                    children,
                    cover: Vec::new(),
                });
            }
        }
        candidates.retain(|candidate| {
            let parent = publication_parent(candidate);
            !self.blocked_publication_parents.contains(&parent)
        });
        for parent in &current_candidate_parents {
            self.publication_candidate_ages.entry(*parent).or_default();
        }
        self.publication_candidate_ages
            .retain(|parent, _| current_candidate_parents.contains(parent));
        if self.planetary_view.is_some() {
            // Freeze every rank before sorting. Recomputing group ages inside
            // the comparator lets a sort cross the two-second promotion
            // boundary halfway through and violates its total-order contract.
            let now = Instant::now();
            let ranked: Vec<_> = candidates
                .into_iter()
                .map(|candidate| {
                    let rank = self.publication_completion_rank(&candidate, now);
                    (candidate, rank)
                })
                .collect();
            candidates = sort_planetary_publication_candidates(ranked);
        } else {
            candidates.sort_by(|a, b| {
                let parent_a = publication_parent(a);
                let parent_b = publication_parent(b);
                let age_a = self
                    .publication_candidate_ages
                    .get(&parent_a)
                    .copied()
                    .unwrap_or(0);
                let age_b = self
                    .publication_candidate_ages
                    .get(&parent_b)
                    .copied()
                    .unwrap_or(0);
                let starved_a = age_a >= PUBLICATION_STARVATION_AGE_TICKS;
                let starved_b = age_b >= PUBLICATION_STARVATION_AGE_TICKS;
                starved_b
                    .cmp(&starved_a)
                    .then_with(|| {
                        if starved_a && starved_b {
                            age_b.cmp(&age_a)
                        } else {
                            self.cached_publication_priority(b)
                                .total_cmp(&self.cached_publication_priority(a))
                        }
                    })
                    .then_with(|| parent_a.cmp(&parent_b))
            });
        }
        candidates.truncate(publication_limit);
        self.publications = candidates;
    }

    fn cached_publication_priority(&self, candidate: &RegionalPublication) -> f64 {
        let parent = publication_parent(candidate);
        self.planetary_scores
            .borrow()
            .get(&parent)
            .map(|score| score.1)
            .or_else(|| self.desired.get(&parent).map(|patch| patch.priority))
            .unwrap_or_else(|| match candidate {
                RegionalPublication::Split { children, .. }
                | RegionalPublication::Merge { children, .. } => children
                    .iter()
                    .filter_map(|child| self.desired.get(child).map(|patch| patch.priority))
                    .fold(0.0, f64::max),
            })
    }

    fn cache_insert(
        &mut self,
        tile: Arc<TileData>,
        count_hit: bool,
        build_elapsed_micros: Option<u64>,
    ) -> CacheInsertResult {
        let key = tile.key.clone();
        if self.cache.contains_key(&key) {
            if count_hit {
                self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
            }
            self.cache_clock = self.cache_clock.saturating_add(1);
            if let Some(entry) = self.cache.get_mut(&key) {
                entry.last_use = self.cache_clock;
            }
            return CacheInsertResult::AlreadyCached;
        }
        let bytes = tile_bytes(&tile);
        if bytes > self.config.cpu_byte_cap || self.config.cpu_tile_cap == 0 {
            self.pressure.cpu = true;
            return CacheInsertResult::Rejected;
        }
        while self.cache.len() >= self.config.cpu_tile_cap
            || self.cache_bytes.saturating_add(bytes) > self.config.cpu_byte_cap
        {
            let pins = self.pinned_addresses();
            let victim = self
                .cache
                .iter()
                .filter(|(_, entry)| !pins.contains(&entry.tile.key.address))
                .min_by_key(|(_, entry)| entry.last_use)
                .map(|(key, _)| key.clone());
            let Some(victim) = victim else {
                self.pressure.cpu = true;
                return CacheInsertResult::Rejected;
            };
            if let Some(removed) = self.cache.remove(&victim) {
                self.trace.event_key(&victim, Stage::CpuEvicted);
                self.trace.event_key(&victim, Stage::Evicted);
                self.cached_tiles_by_address
                    .remove(&removed.tile.key.address);
                self.cache_bytes = self.cache_bytes.saturating_sub(removed.bytes);
                self.seen_cache_requests.remove(&victim);
                self.stats.cache_evictions = self.stats.cache_evictions.saturating_add(1);
                if self.resident.remove(&removed.tile.key.address) {
                    self.trace.set_resident(removed.tile.key.address, false);
                    if self.drawable.contains(&removed.tile.key.address) {
                        self.pressure.publication = true;
                    }
                    self.refresh_publications();
                }
                if !removed.ever_resident {
                    self.record_discarded_build(
                        removed.bytes,
                        removed.build_elapsed_micros.unwrap_or(0),
                    );
                }
            }
        }
        self.cache_clock = self.cache_clock.saturating_add(1);
        self.cache_bytes += bytes;
        self.cached_tiles_by_address
            .insert(tile.key.address, Arc::clone(&tile));
        self.cache.insert(
            key,
            CacheEntry {
                tile,
                bytes,
                last_use: self.cache_clock,
                build_elapsed_micros,
                ever_resident: false,
            },
        );
        CacheInsertResult::Inserted
    }

    fn record_discarded_build(&mut self, bytes: usize, build_elapsed_micros: u64) {
        self.stats.bytes_built_but_unused = self
            .stats
            .bytes_built_but_unused
            .saturating_add(bytes as u64);
        self.stats.build_time_discarded_micros = self
            .stats
            .build_time_discarded_micros
            .saturating_add(build_elapsed_micros);
    }

    fn cpu_cache_can_admit_tile(&self) -> bool {
        let Some(tile_bytes) = expected_tile_bytes(self.config.cells) else {
            return false;
        };
        if tile_bytes > self.config.cpu_byte_cap || self.config.cpu_tile_cap == 0 {
            return false;
        }
        if self.cache.len() < self.config.cpu_tile_cap
            && self.cache_bytes.saturating_add(tile_bytes) <= self.config.cpu_byte_cap
        {
            return true;
        }

        // LRU can make room only by evicting unpinned entries. If the pinned
        // remainder plus one exact-size tile exceeds either cap, further jobs
        // cannot be retained; stop admitting duplicates of known failures.
        let pinned = self.pinned_addresses();
        let (pinned_tiles, pinned_bytes) = self
            .cache
            .values()
            .filter(|entry| pinned.contains(&entry.tile.key.address))
            .fold((0usize, 0usize), |(count, bytes), entry| {
                (count.saturating_add(1), bytes.saturating_add(entry.bytes))
            });
        pinned_tiles.saturating_add(1) <= self.config.cpu_tile_cap
            && pinned_bytes.saturating_add(tile_bytes) <= self.config.cpu_byte_cap
    }

    fn pinned_addresses(&self) -> HashSet<CubePatchAddress> {
        if self.reserved_split_parent.is_some() {
            let mut pins: HashSet<_> = self
                .config
                .roots
                .iter()
                .chain(self.drawable.iter())
                .chain(self.external_pins.iter())
                .chain(self.uploads.iter().map(|upload| &upload.address))
                .copied()
                .collect();
            pins.extend(self.merge_parent_dependencies());
            if let Some(parent) = self.reserved_split_parent
                && let Ok(children) = parent.children()
            {
                pins.extend(children);
            }
            return pins;
        }
        let mut pins: HashSet<_> = self
            .desired
            .keys()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .chain(self.uploads.iter().map(|upload| &upload.address))
            .copied()
            .collect();
        pins.extend(self.dependency_addresses());
        pins
    }

    fn scheduled_dependencies(&self) -> BTreeSet<CubePatchAddress> {
        let mut dependencies = self.dependency_addresses();
        let Some(parent) = self.reserved_split_parent else {
            return dependencies;
        };
        let mut retained: BTreeSet<_> = self
            .config
            .roots
            .iter()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .copied()
            .collect();
        retained.extend(self.merge_parent_dependencies());
        if let Ok(children) = parent.children() {
            retained.extend(children);
        }
        dependencies.retain(|address| retained.contains(address));
        dependencies.extend(retained);
        dependencies
    }

    /// When the CPU cache cannot hold every sibling frontier, serialize split
    /// work around one complete local group. This prevents aggregate per-tile
    /// priority from leaving several parents with two cached children each.
    fn update_split_reservation(&mut self) {
        let groups = self.split_frontier_groups();
        if groups.is_empty() {
            self.reserved_split_parent = None;
            return;
        }

        let mut anchors: BTreeSet<_> = self
            .config
            .roots
            .iter()
            .chain(self.drawable.iter())
            .chain(self.external_pins.iter())
            .chain(self.uploads.iter().map(|upload| &upload.address))
            .copied()
            .collect();
        anchors.extend(self.merge_parent_dependencies());
        let all_children: BTreeSet<_> = groups.iter().flat_map(|group| group.children).collect();
        let Some(tile_bytes) = expected_tile_bytes(self.config.cells) else {
            self.reserved_split_parent = None;
            return;
        };
        let required = anchors.union(&all_children).count();
        let constrained = required > self.config.cpu_tile_cap
            || required.saturating_mul(tile_bytes) > self.config.cpu_byte_cap;
        if !constrained {
            self.reserved_split_parent = None;
            return;
        }

        if let Some(parent) = self.completion_gpu_reservation
            && groups.iter().any(|group| group.parent == parent)
        {
            self.reserved_split_parent = Some(parent);
            return;
        }

        if let Some(parent) = self.reserved_split_parent
            && groups.iter().any(|group| group.parent == parent)
        {
            return;
        }

        self.reserved_split_parent = groups
            .into_iter()
            .find(|group| {
                let needed = anchors.len()
                    + group
                        .children
                        .iter()
                        .filter(|child| !anchors.contains(child))
                        .count();
                needed <= self.config.cpu_tile_cap
                    && needed.saturating_mul(tile_bytes) <= self.config.cpu_byte_cap
            })
            .map(|group| group.parent);
    }

    fn split_frontier_groups(&self) -> Vec<RegionalSplitFrontier> {
        let mut groups = Vec::new();
        let desired_priority_index = self.desired_priority_by_ancestor();
        let now = Instant::now();
        let mut next_completion_promotion = None;
        for &parent in &self.drawable {
            let Ok(children) = parent.children() else {
                continue;
            };
            let child_priorities: Vec<_> = children
                .iter()
                .map(|child| desired_priority_index.get(child).copied())
                .collect();
            if child_priorities.iter().any(Option::is_none) {
                continue;
            }
            if !local_replacement_is_balanced(parent, true, &self.drawable, &self.config.roots) {
                continue;
            }
            let aggregate_priority = child_priorities.iter().flatten().copied().sum::<f64>();
            let ready_children = children
                .iter()
                .filter(|child| {
                    self.resident.contains(child)
                        || self.cached_tiles_by_address.contains_key(child)
                        || self.uploads.iter().any(|upload| upload.address == **child)
                        || self
                            .key_for(**child)
                            .is_ok_and(|key| self.in_flight.contains_key(&key))
                })
                .count();
            let resident_children = children
                .iter()
                .filter(|child| self.resident.contains(child))
                .count();
            let (error, _, _, _) = self.score(parent);
            let useful_floor_deficit_px = (error - USEFUL_DETAIL_FLOOR_ERROR_PX).max(0.0);
            let raw_footprint = self.projected_footprint_weight(parent);
            let age = if self.planetary_view.is_some()
                && error > self.config.split_threshold_px
                && raw_footprint > 0.0
            {
                self.completion_age(parent, now)
            } else {
                Duration::ZERO
            };
            if self.planetary_view.is_some()
                && age < BUILD_CANDIDATE_STARVATION_AFTER
                && error > self.config.split_threshold_px
                && raw_footprint > 0.0
                && let Some(promotion) = now.checked_add(BUILD_CANDIDATE_STARVATION_AFTER - age)
            {
                next_completion_promotion = Some(
                    next_completion_promotion
                        .map_or(promotion, |current: Instant| current.min(promotion)),
                );
            }
            let rank = if self.planetary_view.is_some() {
                completion_group_rank(error, self.config.split_threshold_px, raw_footprint, age)
            } else {
                CompletionRank {
                    priority: aggregate_priority,
                    under_floor: useful_floor_deficit_px > 0.0,
                    age,
                }
            };
            let missing_dependencies = children.len().saturating_sub(ready_children);
            let blocker = if resident_children == children.len() {
                RegionalCompletionBlocker::Publication
            } else if ready_children < children.len() {
                if self
                    .in_flight
                    .keys()
                    .any(|key| children.contains(&key.address))
                {
                    RegionalCompletionBlocker::CpuPreparation
                } else if children
                    .iter()
                    .any(|child| self.cached_tiles_by_address.contains_key(child))
                {
                    RegionalCompletionBlocker::Upload
                } else {
                    RegionalCompletionBlocker::Admission
                }
            } else {
                RegionalCompletionBlocker::Residency
            };
            groups.push(RegionalSplitFrontier {
                parent,
                children,
                parent_resident: self.resident.contains(&parent),
                ready_children,
                aggregate_priority,
                completion_priority: rank.priority,
                age,
                useful_floor_deficit_px,
                resident_children,
                missing_dependencies,
                blocker,
            });
        }
        drop(desired_priority_index);
        sort_split_frontier_groups(&mut groups, self.planetary_view.is_some());
        let signature = split_frontier_completion_signature(&groups, self.planetary_view.is_some());
        let mut index = self.desired_priority_index.borrow_mut();
        if index.completion_signature != signature {
            index.completion_signature = signature;
            index.revision = index.revision.wrapping_add(1);
        }
        index.completion_summary_inputs = Some((
            index.revision,
            self.drawable_cover_revision,
            self.coverage_score_revision,
            self.has_current_screen_coverage_cache(),
        ));
        index.next_completion_promotion = next_completion_promotion;
        #[cfg(test)]
        {
            index.completion_frontier_scans = index.completion_frontier_scans.saturating_add(1);
        }
        groups
    }

    fn completion_age(&self, parent: CubePatchAddress, now: Instant) -> Duration {
        let mut ages = self.completion_group_first_eligible.borrow_mut();
        let first = ages.entry(parent).or_insert(now);
        now.saturating_duration_since(*first)
    }

    fn begin_completion_eligibility(&self, parent: CubePatchAddress, now: Instant) -> Duration {
        self.completion_age(parent, now)
    }

    fn desired_priority_by_ancestor(&self) -> std::cell::Ref<'_, HashMap<CubePatchAddress, f64>> {
        let _priority_index_span = crate::engine_profile::span("GPU frontier priority index");
        {
            let mut index = self.desired_priority_index.borrow_mut();
            let signature_matches = index.signature.len() == self.desired.len()
                && index.signature.iter().zip(self.desired.iter()).all(
                    |((cached_address, cached_priority_bits), (address, patch))| {
                        *cached_address == *address
                            && *cached_priority_bits == patch.priority.to_bits()
                    },
                );
            if !signature_matches {
                index.signature.clear();
                index.priorities.clear();
                for (&address, patch) in &self.desired {
                    index.signature.push((address, patch.priority.to_bits()));
                    let mut ancestor = Some(address);
                    while let Some(current) = ancestor {
                        index
                            .priorities
                            .entry(current)
                            .and_modify(|priority| {
                                if patch.priority.total_cmp(priority).is_gt() {
                                    *priority = patch.priority;
                                }
                            })
                            .or_insert(patch.priority);
                        ancestor = current.parent();
                    }
                }
                index.revision = index.revision.wrapping_add(1);
            }
        }
        std::cell::Ref::map(self.desired_priority_index.borrow(), |index| {
            &index.priorities
        })
    }

    /// Monotonic revision for consumers caching summaries of desired priorities.
    pub(crate) fn desired_priority_revision(&self) -> u64 {
        drop(self.desired_priority_by_ancestor());
        let now = Instant::now();
        let needs_refresh = {
            let index = self.desired_priority_index.borrow();
            index.completion_summary_inputs
                != Some((
                    index.revision,
                    self.drawable_cover_revision,
                    self.coverage_score_revision,
                    self.has_current_screen_coverage_cache(),
                ))
                || index
                    .next_completion_promotion
                    .is_some_and(|promotion| now >= promotion)
        };
        if needs_refresh {
            let _ = self.split_frontier_groups();
        }
        self.desired_priority_index.borrow().revision
    }

    fn build_candidate_wait_entries_upper_bound(&self) -> usize {
        self.config
            .max_desired_patches
            .saturating_mul(usize::from(self.config.max_level) + 1)
            .saturating_add(self.config.roots.len())
            .saturating_add(self.external_pins.len())
            .saturating_add(self.drawable.len().saturating_mul(2))
            .saturating_add(4)
            .max(self.build_candidate_waits.capacity())
    }

    fn build_candidate_wait_bytes_upper_bound(&self, entries_upper_bound: usize) -> usize {
        // HashMap bucket/control overhead is covered by the factor of two.
        // Every tracked exact TileKey owns the same immutable definition words.
        let per_entry = std::mem::size_of::<(TileKey, BuildCandidateWait)>()
            .saturating_add(self.tile_key_definition_word_count.saturating_mul(8))
            .saturating_add(16);
        entries_upper_bound
            .saturating_mul(per_entry)
            .saturating_mul(2)
    }

    /// Desired descendants cannot publish directly: every ancestor level must
    /// become resident first so split proposals can advance from the current
    /// drawable cover without holes. Keep that bounded path schedulable and
    /// cancellable as one dependency closure.
    fn dependency_addresses(&self) -> BTreeSet<CubePatchAddress> {
        let mut cached = self.desired_dependency_cache.borrow_mut();
        if !self.desired.keys().eq(cached.0.iter()) {
            cached.0 = self.desired.keys().copied().collect();
            cached.1.clear();
            for &desired in self.desired.keys() {
                let mut cursor = desired;
                loop {
                    if !self.config.roots.iter().any(|root| root.contains(cursor))
                        || !cached.1.insert(cursor)
                    {
                        break;
                    }
                    if self.config.roots.contains(&cursor) {
                        break;
                    }
                    let Some(parent) = cursor.parent() else {
                        break;
                    };
                    cursor = parent;
                }
            }
        }
        let mut dependencies = cached.1.clone();
        drop(cached);
        dependencies.extend(self.merge_parent_dependencies());
        dependencies
    }

    /// Coarsening an already drawable four-sibling group needs its parent tile
    /// resident even though that parent is not a child of the drawable cover.
    /// Include only complete local groups with a desired ancestor so retreat
    /// repair stays bounded to actual merge candidates.
    fn merge_parent_dependencies(&self) -> BTreeSet<CubePatchAddress> {
        let drawable: HashSet<_> = self.drawable.iter().copied().collect();
        self.drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|parent| {
                let mut ancestor = Some(*parent);
                while let Some(address) = ancestor {
                    if self.desired.contains_key(&address) {
                        return true;
                    }
                    ancestor = address.parent();
                }
                false
            })
            .filter(|parent| {
                parent
                    .children()
                    .is_ok_and(|children| children.iter().all(|child| drawable.contains(child)))
            })
            .collect()
    }
}

impl Drop for RegionalTerrain {
    fn drop(&mut self) {
        // Queued jobs are cheap to abandon. Running workers observe cancellation
        // before build and the dropped result receiver releases bounded senders.
        for token in self.in_flight.values() {
            token.cancelled.store(true, AtomicOrdering::Release);
        }
    }
}

fn sort_split_frontier_groups(groups: &mut [RegionalSplitFrontier], planetary: bool) {
    if planetary {
        groups.sort_by(|a, b| {
            CompletionRank {
                priority: a.completion_priority,
                under_floor: a.useful_floor_deficit_px > 0.0,
                age: a.age,
            }
            .compare(CompletionRank {
                priority: b.completion_priority,
                under_floor: b.useful_floor_deficit_px > 0.0,
                age: b.age,
            })
            .then_with(|| a.parent.cmp(&b.parent))
        });
    } else {
        groups.sort_by(|a, b| {
            b.aggregate_priority
                .total_cmp(&a.aggregate_priority)
                .then_with(|| a.parent.cmp(&b.parent))
        });
    }
}

fn split_frontier_completion_signature(
    groups: &[RegionalSplitFrontier],
    planetary: bool,
) -> Vec<(CubePatchAddress, u64, bool, bool)> {
    groups
        .iter()
        .map(|group| {
            (
                group.parent,
                group.completion_priority.to_bits(),
                planetary && group.useful_floor_deficit_px > 0.0,
                group.age >= BUILD_CANDIDATE_STARVATION_AFTER,
            )
        })
        .collect()
}

fn validate_config(config: &RegionalConfig) -> Result<(), RegionalError> {
    if config.roots.is_empty()
        || config.max_level > 30
        || config
            .roots
            .iter()
            .any(|root| root.level() > config.max_level)
        || config.cells == 0
        || !config.cells.is_power_of_two()
        || config.cells > TileData::MAX_CELLS
        || config.cpu_tile_cap == 0
        || config.cpu_byte_cap == 0
        || config.worker_count == 0
        || config.worker_count > 32
        || config.queue_cap == 0
        || config.completion_cap == 0
        || config.max_desired_patches < config.roots.len()
        || config.upload_tile_cap == 0
        || config.upload_byte_cap == 0
        || config.publication_cap_per_tick == 0
        || config.transition_cap == 0
        || !config.split_threshold_px.is_finite()
        || config.split_threshold_px <= 0.0
        || !config.merge_threshold_px.is_finite()
        || config.merge_threshold_px < 0.0
        || config.merge_threshold_px >= config.split_threshold_px
        || !config.prediction_seconds.is_finite()
        || config.prediction_seconds < 0.0
        || !config.high_speed_mps.is_finite()
        || config.high_speed_mps < 0.0
        || !valid_cover(config.roots.as_slice(), &config.roots, config.max_level)
    {
        return Err(RegionalError::InvalidConfig);
    }
    Ok(())
}

fn tile_bytes(tile: &TileData) -> usize {
    tile.texels
        .len()
        .saturating_mul(std::mem::size_of::<crate::resident_terrain::TileTexel>())
}

fn patch_area(address: CubePatchAddress) -> f64 {
    4.0f64.powi(-i32::from(address.level()))
}

fn convergence_ratio(error_px: f64, target_error_px: f64) -> f64 {
    if error_px <= 0.0 {
        1.0
    } else if error_px.is_finite() {
        (target_error_px / error_px).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn cube_face_for_direction(direction: DVec3) -> CubeFace {
    let absolute = direction.abs();
    if absolute.x >= absolute.y && absolute.x >= absolute.z {
        if direction.x >= 0.0 {
            CubeFace::PositiveX
        } else {
            CubeFace::NegativeX
        }
    } else if absolute.y >= absolute.z {
        if direction.y >= 0.0 {
            CubeFace::PositiveY
        } else {
            CubeFace::NegativeY
        }
    } else if direction.z >= 0.0 {
        CubeFace::PositiveZ
    } else {
        CubeFace::NegativeZ
    }
}

fn expected_tile_bytes(cells: u32) -> Option<usize> {
    let side = usize::try_from(cells.checked_add(3)?).ok()?;
    side.checked_mul(side)?
        .checked_mul(std::mem::size_of::<crate::resident_terrain::TileTexel>())
}

fn candidates_remain(
    desired: &BTreeMap<CubePatchAddress, RegionalPatchSnapshot>,
    cache: &HashMap<TileKey, CacheEntry>,
    in_flight: &HashMap<TileKey, Arc<BuildToken>>,
    resident: &BTreeSet<CubePatchAddress>,
) -> bool {
    let cached: HashSet<_> = cache.keys().map(|key| key.address).collect();
    let pending: HashSet<_> = in_flight.keys().map(|key| key.address).collect();
    desired.keys().any(|address| {
        !resident.contains(address) && !cached.contains(address) && !pending.contains(address)
    })
}

fn oldest_starved_candidate_index<'a>(
    ordered_keys: impl IntoIterator<Item = &'a TileKey>,
    waits: &HashMap<TileKey, BuildCandidateWait>,
    now: Instant,
) -> Option<usize> {
    let mut oldest: Option<(usize, Instant)> = None;
    for (index, key) in ordered_keys.into_iter().enumerate() {
        let Some(wait) = waits.get(key) else {
            continue;
        };
        if now.saturating_duration_since(wait.first_eligible) < BUILD_CANDIDATE_STARVATION_AFTER {
            continue;
        }
        // Keep the earlier score/address candidate on exact timestamp ties.
        if oldest.is_none_or(|(_, first_eligible)| wait.first_eligible < first_eligible) {
            oldest = Some((index, wait.first_eligible));
        }
    }
    oldest.map(|(index, _)| index)
}

fn planetary_merge_is_balanced(
    parent: CubePatchAddress,
    cover: &HashSet<CubePatchAddress>,
) -> bool {
    // On a complete balanced six-face cover, the parent has four leaf children.
    // Across each outer edge, a same-level/coarser neighbor is safe. Otherwise
    // both touching neighbor children must be leaves: deeper descendants would
    // differ by at least two levels after the merge. No global index is needed.
    parent
        .children()
        .is_ok_and(|children| children.iter().all(|child| cover.contains(child)))
        && PatchEdge::ALL.into_iter().all(|edge| {
            let relation = parent.neighbor(edge);
            let mut ancestor = Some(relation.address);
            while let Some(address) = ancestor {
                if cover.contains(&address) {
                    return true;
                }
                ancestor = address.parent();
            }
            relation.address.children().is_ok_and(|children| {
                children
                    .into_iter()
                    .filter(|child| {
                        let [x, y] = child.coordinates();
                        match relation.edge {
                            PatchEdge::UMin => x % 2 == 0,
                            PatchEdge::UMax => x % 2 == 1,
                            PatchEdge::VMin => y % 2 == 0,
                            PatchEdge::VMax => y % 2 == 1,
                        }
                    })
                    .all(|child| cover.contains(&child))
            })
        })
}

// A valid cover can become unbalanced only along the newly split leaf's outer
// edges. Recursively split any coarser neighbor, with a bounded closure. Keeping
// the current leaves in one ordered set avoids rebuilding global adjacency for
// every trial and preserves cross-face neighbor transforms from CubePatchAddress.
fn balance_planetary_split(
    cover: &[CubePatchAddress],
    parent: CubePatchAddress,
    max_level: u8,
    max_leaves: usize,
    max_splits: usize,
    started: std::time::Instant,
    budget: Duration,
) -> Option<Vec<CubePatchAddress>> {
    // The input is sorted. Test the bounded closure against a small overlay
    // rather than cloning/indexing the entire cover before any useful work.
    // The full output copy happens only after the local closure is validated.
    let mut removed = HashSet::new();
    let mut added = BTreeSet::new();
    let mut pending = vec![parent];
    let mut splits = 0;
    while let Some(address) = pending.pop() {
        if started.elapsed() >= budget {
            return None;
        }
        if !added.contains(&address)
            && (removed.contains(&address) || cover.binary_search(&address).is_err())
        {
            continue;
        }
        if address.level() >= max_level
            || splits == max_splits
            || cover.len().saturating_add((splits + 1) * 3) > max_leaves
        {
            return None;
        }
        for edge in PatchEdge::ALL {
            let mut neighbor = Some(address.neighbor(edge).address);
            while let Some(candidate) = neighbor {
                if added.contains(&candidate)
                    || (!removed.contains(&candidate) && cover.binary_search(&candidate).is_ok())
                {
                    if candidate.level() < address.level() {
                        pending.push(candidate);
                    }
                    break;
                }
                neighbor = candidate.parent();
            }
        }
        if !added.remove(&address) {
            removed.insert(address);
        }
        added.extend(address.children().ok()?);
        splits += 1;
    }
    // Do not discard completed closure work if the unavoidable output copy
    // exceeds the deadline. The caller records actual elapsed-time overruns.
    let mut next: Vec<_> = cover
        .iter()
        .copied()
        .filter(|address| !removed.contains(address))
        .chain(added)
        .collect();
    next.sort_unstable();
    Some(next)
}

fn balance_cover(
    roots: &[CubePatchAddress],
    mut cover: Vec<CubePatchAddress>,
    max_level: u8,
    patch_cap: usize,
    deadline: Option<(std::time::Instant, Duration)>,
) -> Option<Vec<CubePatchAddress>> {
    cover.sort();
    cover.dedup();
    loop {
        if selector_deadline_expired(deadline) {
            return None;
        }
        if cover.len() > patch_cap {
            return None;
        }
        let (complete, indexed_cover) = match deadline {
            Some((started, budget)) => {
                complete_nonoverlapping_cover_until(roots, &cover, max_level, started, budget)?
            }
            None => (
                complete_nonoverlapping_cover(roots, &cover, max_level),
                None,
            ),
        };
        if !complete {
            return None;
        }
        let (cover_set, descendants) = match indexed_cover {
            Some(index) => index,
            None => cover_address_index(&cover),
        };
        let mut too_coarse = BTreeSet::new();
        for &patch in &cover {
            if selector_deadline_expired(deadline) {
                return None;
            }
            for edge in PatchEdge::ALL {
                for neighbor in adjacent_cover_patches(patch, edge, &cover_set, &descendants) {
                    if patch.level() > neighbor.level() + 1 {
                        too_coarse.insert(neighbor);
                    }
                }
            }
        }
        if too_coarse.is_empty() {
            return Some(cover);
        }
        let needed = too_coarse.len().saturating_mul(3);
        if cover.len().saturating_add(needed) > patch_cap
            || too_coarse.iter().any(|patch| patch.level() >= max_level)
        {
            return None;
        }
        let mut next = Vec::with_capacity(cover.len() + needed);
        for patch in cover {
            if too_coarse.contains(&patch) {
                next.extend(patch.children().ok()?);
            } else {
                next.push(patch);
            }
        }
        next.sort();
        cover = next;
    }
}

fn selector_deadline_expired(deadline: Option<(std::time::Instant, Duration)>) -> bool {
    deadline.is_some_and(|(started, budget)| started.elapsed() >= budget)
}

type CoverAddressIndex = (
    HashSet<CubePatchAddress>,
    HashMap<CubePatchAddress, Vec<CubePatchAddress>>,
);

fn cover_address_index_until(
    cover: &[CubePatchAddress],
    started: std::time::Instant,
    budget: Duration,
) -> Option<CoverAddressIndex> {
    let mut cover_set = HashSet::with_capacity(cover.len());
    let mut descendants = HashMap::<CubePatchAddress, Vec<CubePatchAddress>>::new();
    for &leaf in cover {
        if started.elapsed() >= budget {
            return None;
        }
        cover_set.insert(leaf);
        let mut ancestor = Some(leaf);
        while let Some(address) = ancestor {
            descendants.entry(address).or_default().push(leaf);
            ancestor = address.parent();
        }
    }
    Some((cover_set, descendants))
}

fn complete_nonoverlapping_cover_until(
    roots: &[CubePatchAddress],
    cover: &[CubePatchAddress],
    max_level: u8,
    started: std::time::Instant,
    budget: Duration,
) -> Option<(bool, Option<CoverAddressIndex>)> {
    if roots.is_empty() || cover.is_empty() {
        return Some((false, None));
    }
    for patch in cover {
        if started.elapsed() >= budget {
            return None;
        }
        if patch.level() > max_level {
            return Some((false, None));
        }
    }
    for (index, root) in roots.iter().enumerate() {
        if started.elapsed() >= budget {
            return None;
        }
        for other in roots.iter().skip(index + 1) {
            if root.contains(*other) || other.contains(*root) {
                return Some((false, None));
            }
        }
    }
    let (cover_set, descendants) = cover_address_index_until(cover, started, budget)?;
    if cover_set.len() != cover.len()
        || cover.iter().any(|patch| {
            descendants
                .get(patch)
                .is_some_and(|leaves| leaves.len() > 1)
        })
    {
        return Some((false, None));
    }
    let mut areas = vec![0u128; roots.len()];
    for patch in cover {
        if started.elapsed() >= budget {
            return None;
        }
        let Some(root_index) = roots.iter().position(|root| root.contains(*patch)) else {
            return Some((false, None));
        };
        let mut ancestor = patch.parent();
        while let Some(parent) = ancestor {
            if cover_set.contains(&parent) {
                return Some((false, None));
            }
            if parent == roots[root_index] {
                break;
            }
            ancestor = parent.parent();
        }
        let depth = u32::from(max_level - patch.level());
        areas[root_index] = areas[root_index].saturating_add(1u128 << (depth * 2));
    }
    for (root, area) in roots.iter().zip(areas) {
        let depth = u32::from(max_level - root.level());
        if area != 1u128 << (depth * 2) {
            return Some((false, None));
        }
    }
    Some((true, Some((cover_set, descendants))))
}

fn valid_cover(roots: &[CubePatchAddress], cover: &[CubePatchAddress], max_level: u8) -> bool {
    complete_nonoverlapping_cover(roots, cover, max_level) && cover_is_balanced(cover)
}

fn complete_nonoverlapping_cover(
    roots: &[CubePatchAddress],
    cover: &[CubePatchAddress],
    max_level: u8,
) -> bool {
    if roots.is_empty() || cover.is_empty() || cover.iter().any(|patch| patch.level() > max_level) {
        return false;
    }
    if roots.iter().enumerate().any(|(index, root)| {
        roots
            .iter()
            .skip(index + 1)
            .any(|other| root.contains(*other) || other.contains(*root))
    }) {
        return false;
    }
    let cover_set: HashSet<_> = cover.iter().copied().collect();
    if cover_set.len() != cover.len() {
        return false;
    }
    let (_, descendants) = cover_address_index(cover);
    if cover.iter().any(|patch| {
        descendants
            .get(patch)
            .is_some_and(|leaves| leaves.len() > 1)
    }) {
        return false;
    }
    let mut areas = vec![0u128; roots.len()];
    for patch in cover {
        let Some(root_index) = roots.iter().position(|root| root.contains(*patch)) else {
            return false;
        };
        let mut ancestor = patch.parent();
        while let Some(parent) = ancestor {
            if cover_set.contains(&parent) {
                return false;
            }
            if parent == roots[root_index] {
                break;
            }
            ancestor = parent.parent();
        }
        let depth = u32::from(max_level - patch.level());
        areas[root_index] = areas[root_index].saturating_add(1u128 << (depth * 2));
    }
    for (root, area) in roots.iter().zip(areas) {
        let depth = u32::from(max_level - root.level());
        if area != 1u128 << (depth * 2) {
            return false;
        }
    }
    true
}

// Replacing one leaf by its four children, or the reverse, preserves complete
// nonoverlapping coverage. Only the replacement's outer edges can change balance.
// Callers supply the current valid cover and reuse its adjacency index.
#[cfg(test)]
fn replacement_is_balanced(
    parent: CubePatchAddress,
    split: bool,
    cover: &HashSet<CubePatchAddress>,
    descendants: &HashMap<CubePatchAddress, Vec<CubePatchAddress>>,
) -> bool {
    let Ok(children) = parent.children() else {
        return false;
    };
    if (split && !cover.contains(&parent))
        || (!split && !children.iter().all(|child| cover.contains(child)))
    {
        return false;
    }
    PatchEdge::ALL.into_iter().all(|edge| {
        adjacent_cover_patches(parent, edge, cover, descendants)
            .into_iter()
            .all(|neighbor| {
                if split {
                    neighbor.level() >= parent.level()
                } else {
                    neighbor.level() <= parent.level() + 1
                }
            })
    })
}

fn publication_parent(publication: &RegionalPublication) -> CubePatchAddress {
    match publication {
        RegionalPublication::Split { parent, .. } | RegionalPublication::Merge { parent, .. } => {
            *parent
        }
    }
}

fn sort_planetary_publication_candidates(
    mut ranked: Vec<(RegionalPublication, CompletionRank)>,
) -> Vec<RegionalPublication> {
    ranked.sort_by(|a, b| {
        a.1.compare(b.1)
            .then_with(|| publication_parent(&a.0).cmp(&publication_parent(&b.0)))
    });
    ranked.into_iter().map(|(candidate, _)| candidate).collect()
}

fn useful_detail_reached(
    has_planetary_view: bool,
    visible_drawable_count: usize,
    missing_drawable_scores: usize,
    maximum_proxy_error_px: f64,
) -> bool {
    has_planetary_view
        && visible_drawable_count > 0
        && missing_drawable_scores == 0
        && maximum_proxy_error_px <= 1.0
}

fn empty_cover_publication(publication: &RegionalPublication) -> RegionalPublication {
    match publication {
        RegionalPublication::Split {
            parent, children, ..
        } => RegionalPublication::Split {
            parent: *parent,
            children: *children,
            cover: Vec::new(),
        },
        RegionalPublication::Merge {
            parent, children, ..
        } => RegionalPublication::Merge {
            parent: *parent,
            children: *children,
            cover: Vec::new(),
        },
    }
}

/// Check only the replacement boundary against a sorted complete balanced
/// cover. A split can fail only when an outer neighbor is coarser than parent.
/// A merge can fail only when a touching neighbor is finer than parent+1.
fn local_replacement_is_balanced(
    parent: CubePatchAddress,
    split: bool,
    cover: &[CubePatchAddress],
    roots: &[CubePatchAddress],
) -> bool {
    if split {
        if cover.binary_search(&parent).is_err() {
            return false;
        }
    } else {
        let Some(children) = parent.children().ok() else {
            return false;
        };
        if !children
            .iter()
            .all(|child| cover.binary_search(child).is_ok())
        {
            return false;
        }
    }
    PatchEdge::ALL.into_iter().all(|edge| {
        let relation = parent.neighbor(edge);
        // A local regional cover may end at its configured root boundary. Such
        // an edge has no terrain neighbor to constrain the replacement. The
        // relation still participates when it overlaps any configured root,
        // including a coarser/finer root that contains it or is contained by it.
        if !roots
            .iter()
            .any(|root| root.contains(relation.address) || relation.address.contains(*root))
        {
            return true;
        }
        let mut ancestor = Some(relation.address);
        while let Some(address) = ancestor {
            if cover.binary_search(&address).is_ok() {
                return if split {
                    address.level() >= parent.level()
                } else {
                    address.level() <= parent.level().saturating_add(1)
                };
            }
            ancestor = address.parent();
        }
        if split {
            // A complete cover has finer leaves under this neighbor address.
            // None can be coarser than the address itself.
            relation.address.level() >= parent.level()
        } else {
            // Current 2:1 balance bounds edge neighbors to at most two leaves
            // at parent+1. A deeper touching descendant is detected by absence
            // of one of these required boundary children.
            relation.address.children().is_ok_and(|children| {
                children
                    .into_iter()
                    .filter(|child| {
                        let [x, y] = child.coordinates();
                        match relation.edge {
                            PatchEdge::UMin => x % 2 == 0,
                            PatchEdge::UMax => x % 2 == 1,
                            PatchEdge::VMin => y % 2 == 0,
                            PatchEdge::VMax => y % 2 == 1,
                        }
                    })
                    .all(|child| cover.binary_search(&child).is_ok())
            })
        }
    })
}

fn cover_is_balanced(cover: &[CubePatchAddress]) -> bool {
    let (cover_set, descendants) = cover_address_index(cover);
    for &patch in cover {
        for edge in PatchEdge::ALL {
            for neighbor in adjacent_cover_patches(patch, edge, &cover_set, &descendants) {
                if patch.level().abs_diff(neighbor.level()) > 1 {
                    return false;
                }
            }
        }
    }
    true
}

fn cover_address_index(
    cover: &[CubePatchAddress],
) -> (
    HashSet<CubePatchAddress>,
    HashMap<CubePatchAddress, Vec<CubePatchAddress>>,
) {
    let cover_set: HashSet<_> = cover.iter().copied().collect();
    let mut descendants = HashMap::<CubePatchAddress, Vec<CubePatchAddress>>::new();
    for &leaf in cover {
        let mut ancestor = Some(leaf);
        while let Some(address) = ancestor {
            descendants.entry(address).or_default().push(leaf);
            ancestor = address.parent();
        }
    }
    (cover_set, descendants)
}

/// Preserve the old descendant maximum for split priority without retaining a
/// vector of every desired descendant for every ancestor. A child's entry is
/// the `total_cmp` maximum priority of that child and all desired patches below
/// it, matching `max_by(f64::total_cmp)` while storing one scalar per ancestor.
fn desired_priority_cache_bytes_upper_bound(config: &RegionalConfig) -> usize {
    // Each desired address contributes at most one entry per ancestor level.
    // Account for spare Vec/hash buckets and hash control bytes conservatively
    // so diagnostics don't present entry payload size as the whole allocation.
    let desired_capacity = config.max_desired_patches;
    let index_entries = desired_capacity.saturating_mul(usize::from(config.max_level) + 1);
    std::mem::size_of::<DesiredPriorityIndex>()
        .saturating_add(
            desired_capacity
                .saturating_mul(std::mem::size_of::<(CubePatchAddress, u64)>())
                .saturating_mul(2),
        )
        .saturating_add(
            index_entries
                .saturating_mul(std::mem::size_of::<(CubePatchAddress, f64)>())
                .saturating_mul(3),
        )
        .saturating_add(
            desired_capacity
                .saturating_mul(std::mem::size_of::<(CubePatchAddress, u64, bool, bool)>())
                .saturating_mul(2),
        )
}

fn adjacent_cover_patches(
    patch: CubePatchAddress,
    edge: PatchEdge,
    cover: &HashSet<CubePatchAddress>,
    descendants: &HashMap<CubePatchAddress, Vec<CubePatchAddress>>,
) -> Vec<CubePatchAddress> {
    let relation = patch.neighbor(edge);
    let mut ancestor = Some(relation.address);
    while let Some(address) = ancestor {
        if cover.contains(&address) {
            return vec![address];
        }
        ancestor = address.parent();
    }

    descendants
        .get(&relation.address)
        .into_iter()
        .flatten()
        .copied()
        .filter(|candidate| candidate.level() > patch.level())
        .filter(|candidate| {
            let depth = candidate.level() - relation.address.level();
            let side = 1u32 << depth;
            let [x, y] = candidate.coordinates();
            match relation.edge {
                PatchEdge::UMin => x % side == 0,
                PatchEdge::UMax => x % side == side - 1,
                PatchEdge::VMin => y % side == 0,
                PatchEdge::VMax => y % side == side - 1,
            }
        })
        .collect()
}

#[cfg(test)]
mod frontier_tests {
    use super::*;

    #[test]
    fn convergence_ratio_normalizes_error_to_the_configured_pixel_target() {
        let target = 0.15;
        assert_eq!(convergence_ratio(0.0, target), 1.0);
        assert_eq!(convergence_ratio(target * 0.5, target), 1.0);
        assert_eq!(convergence_ratio(target, target), 1.0);
        assert_eq!(convergence_ratio(target * 2.0, target), 0.5);
        assert_eq!(convergence_ratio(f64::INFINITY, target), 0.0);
        assert_eq!(convergence_ratio(f64::NAN, target), 0.0);
    }

    fn test_generator() -> SurfaceGenerator {
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
        };
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x2d76),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        SurfaceGenerator::new(&definition, 80_000.0).unwrap()
    }

    fn test_terrain(roots: Vec<CubePatchAddress>) -> RegionalTerrain {
        RegionalTerrain::new(
            test_generator(),
            TileBuildIdentity {
                body_identity: 1,
                surface_revision: 1,
                material_revision: 1,
            },
            RegionalConfig {
                roots,
                cells: 4,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn starved_candidate_selection_uses_synthetic_age_and_stable_ties() {
        let terrain = test_terrain(vec![CubePatchAddress::root(CubeFace::PositiveX)]);
        let addresses = CubePatchAddress::root(CubeFace::PositiveX)
            .children()
            .unwrap();
        let keys: Vec<_> = addresses
            .iter()
            .map(|&address| terrain.key_for(address).unwrap())
            .collect();
        let now = Instant::now();
        let mut waits = HashMap::new();
        waits.insert(
            keys[0].clone(),
            BuildCandidateWait {
                first_eligible: now - Duration::from_secs(3),
                last_seen_epoch: 1,
            },
        );
        waits.insert(
            keys[1].clone(),
            BuildCandidateWait {
                first_eligible: now - Duration::from_secs(4),
                last_seen_epoch: 1,
            },
        );
        waits.insert(
            keys[2].clone(),
            BuildCandidateWait {
                first_eligible: now - Duration::from_secs(4),
                last_seen_epoch: 1,
            },
        );
        waits.insert(
            keys[3].clone(),
            BuildCandidateWait {
                first_eligible: now - Duration::from_millis(1_999),
                last_seen_epoch: 1,
            },
        );

        // The oldest timestamp wins, with the score/address order deciding an
        // exact age tie; a candidate just below the threshold is not promoted.
        assert_eq!(oldest_starved_candidate_index(&keys, &waits, now), Some(1));
        assert_eq!(
            oldest_starved_candidate_index(&keys[..1], &waits, now),
            Some(0)
        );
        assert_eq!(
            oldest_starved_candidate_index(&keys[3..], &waits, now),
            None
        );
    }

    #[test]
    fn completion_rank_finishes_useful_floor_before_optional_detail_then_promotes_age() {
        let useful_edge = CompletionRank {
            priority: 2.0,
            under_floor: true,
            age: Duration::from_millis(1_999),
        };
        let optional_center = CompletionRank {
            priority: 1_000_000.0,
            under_floor: false,
            age: Duration::ZERO,
        };
        assert!(useful_edge.compare(optional_center).is_lt());

        let promoted_optional = CompletionRank {
            age: Duration::from_secs(2),
            ..optional_center
        };
        assert!(promoted_optional.compare(useful_edge).is_lt());
    }

    #[test]
    fn completion_rank_is_transitive_around_the_age_promotion_boundary() {
        let ranks = [
            CompletionRank {
                priority: 1_000.0,
                under_floor: false,
                age: Duration::from_millis(1_999),
            },
            CompletionRank {
                priority: 2.0,
                under_floor: true,
                age: Duration::from_millis(2_000),
            },
            CompletionRank {
                priority: 0.5,
                under_floor: false,
                age: Duration::from_millis(2_001),
            },
            CompletionRank {
                priority: 10.0,
                under_floor: false,
                age: Duration::ZERO,
            },
        ];
        for a in ranks {
            for b in ranks {
                assert_eq!(a.compare(b), b.compare(a).reverse());
                for c in ranks {
                    if a.compare(b).is_le() && b.compare(c).is_le() {
                        assert!(a.compare(c).is_le());
                    }
                }
            }
        }
        let mut sorted = ranks.to_vec();
        sorted.sort_by(|a, b| a.compare(*b));
        assert!(
            sorted
                .windows(2)
                .all(|pair| pair[0].compare(pair[1]).is_le())
        );
    }

    #[test]
    fn completion_signature_tracks_useful_floor_tier_at_equal_priority() {
        let parents = [
            CubePatchAddress::root(CubeFace::PositiveX),
            CubePatchAddress::root(CubeFace::NegativeX),
        ];
        let make_group = |parent, floor_deficit| RegionalSplitFrontier {
            parent,
            children: parent.children().unwrap(),
            parent_resident: true,
            ready_children: 0,
            aggregate_priority: 12.0,
            completion_priority: 12.0,
            age: Duration::ZERO,
            useful_floor_deficit_px: floor_deficit,
            resident_children: 0,
            missing_dependencies: 4,
            blocker: RegionalCompletionBlocker::Admission,
        };
        let groups = vec![make_group(parents[0], 2.0), make_group(parents[1], 0.0)];
        let with_floor_tier = split_frontier_completion_signature(&groups, true);
        let without_floor_tier = split_frontier_completion_signature(&groups, false);
        assert_ne!(with_floor_tier, without_floor_tier);
        assert!(with_floor_tier[0].2);
        assert!(!with_floor_tier[1].2);
        assert!(with_floor_tier.iter().all(|entry| !entry.3));
    }

    #[test]
    fn finite_split_frontiers_keep_aggregate_priority_ahead_of_floor_deficit() {
        let floor_parent = CubePatchAddress::root(CubeFace::PositiveX);
        let optional_parent = CubePatchAddress::root(CubeFace::NegativeX);
        let make_group = |parent, aggregate_priority, floor_deficit| RegionalSplitFrontier {
            parent,
            children: parent.children().unwrap(),
            parent_resident: true,
            ready_children: 0,
            aggregate_priority,
            completion_priority: aggregate_priority,
            age: Duration::ZERO,
            useful_floor_deficit_px: floor_deficit,
            resident_children: 0,
            missing_dependencies: 4,
            blocker: RegionalCompletionBlocker::Admission,
        };
        let mut groups = vec![
            make_group(floor_parent, 1.0, 8.0),
            make_group(optional_parent, 100.0, 0.0),
        ];

        sort_split_frontier_groups(&mut groups, false);
        assert_eq!(groups[0].parent, optional_parent);
        assert_eq!(groups[1].parent, floor_parent);

        sort_split_frontier_groups(&mut groups, true);
        assert_eq!(groups[0].parent, floor_parent);
        assert_eq!(groups[1].parent, optional_parent);
    }

    #[test]
    fn mixed_split_merge_publications_use_one_frozen_ordering_key() {
        let useful_parent = CubePatchAddress::root(CubeFace::PositiveX);
        let optional_parent = CubePatchAddress::root(CubeFace::NegativeX);
        let merge_parent = CubePatchAddress::root(CubeFace::PositiveY);
        let split = |parent| RegionalPublication::Split {
            parent,
            children: parent.children().unwrap(),
            cover: Vec::new(),
        };
        let merge = RegionalPublication::Merge {
            parent: merge_parent,
            children: merge_parent.children().unwrap(),
            cover: Vec::new(),
        };
        let rank = |priority, under_floor| CompletionRank {
            priority,
            under_floor,
            age: Duration::ZERO,
        };
        // The old comparator switched to raw priority for mixed split/merge
        // pairs, which can reverse an already-ranked split pair.
        let sorted = sort_planetary_publication_candidates(vec![
            (merge, rank(500.0, false)),
            (split(optional_parent), rank(1_000.0, false)),
            (split(useful_parent), rank(1.0, true)),
        ]);
        let parents: Vec<_> = sorted.iter().map(publication_parent).collect();
        assert_eq!(parents, vec![useful_parent, optional_parent, merge_parent]);
    }

    #[test]
    fn completion_group_age_survives_cpu_upload_residency_and_publication_stages() {
        use crate::resident_terrain::ResidentTileBuilder;

        let parent = CubePatchAddress::root(CubeFace::PositiveZ);
        let children = parent.children().unwrap();
        let mut terrain = test_terrain(vec![parent]);
        terrain.drawable = vec![parent];
        terrain.resident.insert(parent);
        let projection =
            CelestialProjection::try_new(800, 600, 70.0_f64.to_radians(), 0.1).unwrap();
        terrain
            .set_planetary_view(DMat3::IDENTITY, projection)
            .unwrap();
        terrain.view = RegionalView {
            body_position_m: DVec3::Z * (terrain.generator.radius_m() + 1_000.0),
            body_velocity_mps: DVec3::ZERO,
            projection_scale_px: projection.focal_pixels(),
        };
        for child in children {
            let (error, priority, _, _) = terrain.score(child);
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: error,
                    priority,
                    state: "desired",
                },
            );
        }
        terrain.split_frontiers();
        let first_eligible = Instant::now() - Duration::from_secs(3);
        terrain
            .completion_group_first_eligible
            .borrow_mut()
            .insert(parent, first_eligible);

        let frontier = |terrain: &RegionalTerrain| {
            terrain
                .split_frontiers()
                .into_iter()
                .find(|group| group.parent == parent)
                .unwrap()
        };
        let initial = frontier(&terrain);
        assert!(initial.age >= Duration::from_secs(3));
        assert_eq!(initial.missing_dependencies, 4);
        assert_eq!(initial.blocker, RegionalCompletionBlocker::Admission);
        let first_eligible = terrain
            .completion_group_first_eligible
            .borrow()
            .get(&parent)
            .copied()
            .unwrap();
        let candidate = RegionalPublication::Split {
            parent,
            children,
            cover: Vec::new(),
        };
        let prepared_rank = terrain.publication_completion_rank(&candidate, Instant::now());
        assert_eq!(
            prepared_rank.priority.to_bits(),
            initial.completion_priority.to_bits()
        );
        assert_eq!(
            prepared_rank.under_floor,
            initial.useful_floor_deficit_px > 0.0
        );
        assert!(prepared_rank.age >= initial.age);
        assert_eq!(
            terrain
                .completion_group_first_eligible
                .borrow()
                .get(&parent),
            Some(&first_eligible),
            "ranking a prepared product must not create or reset group age"
        );

        let key = terrain.key_for(children[0]).unwrap();
        terrain.in_flight.insert(
            key,
            Arc::new(BuildToken {
                cancelled: AtomicBool::new(false),
                started: AtomicBool::new(true),
            }),
        );
        let preparing = frontier(&terrain);
        assert_eq!(preparing.blocker, RegionalCompletionBlocker::CpuPreparation);
        assert!(preparing.age >= initial.age);
        terrain.in_flight.clear();

        let generator = terrain.generator.clone();
        let tile_identity = terrain.identity;
        for child in children {
            let tile =
                ResidentTileBuilder::build(&generator, tile_identity, child, terrain.config.cells)
                    .unwrap()
                    .0;
            assert_eq!(
                terrain.cache_insert(Arc::new(tile), false, None),
                CacheInsertResult::Inserted
            );
        }
        let cached = frontier(&terrain);
        assert_eq!(cached.blocker, RegionalCompletionBlocker::Residency);
        assert!(cached.age >= initial.age);
        terrain.resident.extend(children);
        let resident = frontier(&terrain);
        assert_eq!(resident.blocker, RegionalCompletionBlocker::Publication);
        assert_eq!(resident.missing_dependencies, 0);
        assert!(resident.age >= initial.age);

        let now = Instant::now();
        terrain
            .completion_group_first_eligible
            .borrow_mut()
            .insert(parent, now);
        let unpromoted = frontier(&terrain);
        assert!(unpromoted.age < BUILD_CANDIDATE_STARVATION_AFTER);
        let unpromoted_revision = terrain.desired_priority_revision();
        terrain.completion_group_first_eligible.borrow_mut().insert(
            parent,
            now - BUILD_CANDIDATE_STARVATION_AFTER - Duration::from_millis(1),
        );
        terrain
            .desired_priority_index
            .borrow_mut()
            .next_completion_promotion = Some(now - Duration::from_millis(1));
        let promoted_revision = terrain.desired_priority_revision();
        assert!(promoted_revision > unpromoted_revision);
    }

    #[test]
    fn constrained_cpu_reservation_uses_oldest_ranked_visible_group() {
        use glam::DQuat;
        let parents = [
            CubePatchAddress::root(CubeFace::PositiveZ),
            CubePatchAddress::root(CubeFace::PositiveX),
        ];
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        terrain.drawable = roots.clone();
        terrain.resident.extend(roots);
        terrain.config.cpu_tile_cap = 10;
        terrain.config.cpu_byte_cap = usize::MAX;
        let projection =
            CelestialProjection::try_new(800, 600, 120.0_f64.to_radians(), 0.1).unwrap();
        let camera_direction = DVec3::new(1.0, 0.0, 1.0).normalize();
        let body_to_view = DMat3::from_quat(DQuat::from_rotation_arc(camera_direction, DVec3::Z));
        terrain
            .set_planetary_view(body_to_view, projection)
            .unwrap();
        terrain.view = RegionalView {
            body_position_m: camera_direction * (terrain.generator.radius_m() + 1_000.0),
            body_velocity_mps: DVec3::ZERO,
            projection_scale_px: projection.focal_pixels(),
        };
        let mut sampled_areas = HashMap::new();
        sampled_areas.insert(parents[0], 12.0);
        sampled_areas.insert(parents[1], 8.0);
        terrain
            .screen_coverage_cache
            .replace(Some(ScreenCoverageCacheEntry {
                key: terrain.screen_coverage_key().unwrap(),
                diagnostic: ScreenCoverageDiagnostic {
                    method: "test sampled cover",
                    certified: false,
                    limitation: "unit-test fixture",
                    sampling_valid: true,
                    samples_per_cell_axis: 1,
                    useful_proxy_error_threshold_px: USEFUL_DETAIL_FLOOR_ERROR_PX,
                    target_proxy_error_threshold_px: terrain.config.split_threshold_px,
                    all_view: ScreenCoverageCell::default(),
                    cells: std::array::from_fn(|_| ScreenCoverageCell::default()),
                    sampled_drawable_area_pixels: Arc::new(sampled_areas),
                },
            }));
        for parent in parents {
            for child in parent.children().unwrap() {
                let (error, priority, _, _) = terrain.score(child);
                terrain.desired.insert(
                    child,
                    RegionalPatchSnapshot {
                        address: format!("{child:?}"),
                        level: child.level(),
                        projected_error_px: error,
                        priority,
                        state: "desired",
                    },
                );
            }
        }
        let now = Instant::now();
        terrain
            .completion_group_first_eligible
            .borrow_mut()
            .insert(parents[0], now - Duration::from_millis(2_500));
        terrain
            .completion_group_first_eligible
            .borrow_mut()
            .insert(parents[1], now - Duration::from_millis(3_000));
        let groups = terrain.split_frontiers();
        assert_eq!(groups.len(), 2);
        let chosen = groups.first().unwrap().parent;
        assert_eq!(chosen, parents[1]);
        terrain.update_split_reservation();
        assert_eq!(terrain.reserved_split_parent, Some(chosen));
    }

    #[test]
    fn admission_wait_age_tracks_only_currently_eligible_missing_candidates() {
        let root = CubePatchAddress::root(CubeFace::PositiveX);
        let mut terrain = test_terrain(vec![root]);
        terrain.drawable = vec![root];
        terrain.resident.insert(root);
        terrain.config.admission_cap_per_tick = 1;
        let children = root.children().unwrap();
        for (index, address) in children.iter().copied().enumerate() {
            let priority = (children.len() - index) as f64;
            terrain.desired.insert(
                address,
                RegionalPatchSnapshot {
                    address: format!("{address:?}"),
                    level: address.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }

        let now = Instant::now();
        let starved_address = children[3];
        let starved_key = terrain.key_for(starved_address).unwrap();
        terrain.build_candidate_waits.insert(
            starved_key.clone(),
            BuildCandidateWait {
                first_eligible: now - Duration::from_secs(3),
                last_seen_epoch: terrain.build_candidate_wait_epoch,
            },
        );
        terrain.admit_builds();

        assert!(terrain.in_flight.contains_key(&starved_key));
        assert!(!terrain.build_candidate_waits.contains_key(&starved_key));
        let retired_address = children[2];
        let retired_key = terrain.key_for(retired_address).unwrap();
        assert!(terrain.build_candidate_waits.contains_key(&retired_key));

        // Removing a candidate from the desired frontier retires its age on the
        // next eligibility rebuild, even when admission itself is paused.
        terrain.desired.remove(&retired_address);
        terrain.config.admission_cap_per_tick = 0;
        terrain.admit_builds();
        assert!(!terrain.build_candidate_waits.contains_key(&retired_key));
        assert!(
            terrain
                .build_candidate_waits
                .keys()
                .all(|key| terrain.desired.contains_key(&key.address))
        );
    }

    fn original_merge_parent_dependencies(terrain: &RegionalTerrain) -> BTreeSet<CubePatchAddress> {
        let drawable: HashSet<_> = terrain.drawable.iter().copied().collect();
        let desired: HashSet<_> = terrain.desired.keys().copied().collect();
        terrain
            .drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|parent| {
                let mut ancestor = Some(*parent);
                while let Some(address) = ancestor {
                    if desired.contains(&address) {
                        return true;
                    }
                    ancestor = address.parent();
                }
                false
            })
            .filter(|parent| {
                parent
                    .children()
                    .is_ok_and(|children| children.iter().all(|child| drawable.contains(child)))
            })
            .collect()
    }

    fn original_dependency_addresses(terrain: &RegionalTerrain) -> BTreeSet<CubePatchAddress> {
        let mut dependencies = BTreeSet::new();
        for &desired in terrain.desired.keys() {
            let mut cursor = desired;
            loop {
                if !terrain
                    .config
                    .roots
                    .iter()
                    .any(|root| root.contains(cursor))
                    || !dependencies.insert(cursor)
                {
                    break;
                }
                if terrain.config.roots.contains(&cursor) {
                    break;
                }
                let Some(parent) = cursor.parent() else {
                    break;
                };
                cursor = parent;
            }
        }
        dependencies.extend(original_merge_parent_dependencies(terrain));
        dependencies
    }

    fn original_scheduled_dependencies(terrain: &RegionalTerrain) -> BTreeSet<CubePatchAddress> {
        let mut dependencies = original_dependency_addresses(terrain);
        let Some(parent) = terrain.reserved_split_parent else {
            return dependencies;
        };
        let mut retained: BTreeSet<_> = terrain
            .config
            .roots
            .iter()
            .chain(terrain.drawable.iter())
            .chain(terrain.external_pins.iter())
            .copied()
            .collect();
        retained.extend(original_merge_parent_dependencies(terrain));
        if let Ok(children) = parent.children() {
            retained.extend(children);
        }
        dependencies.retain(|address| retained.contains(address));
        dependencies.extend(retained);
        dependencies
    }

    fn original_upload_candidate_oracle(terrain: &RegionalTerrain) -> Vec<(f64, CubePatchAddress)> {
        let drawable: HashSet<_> = terrain.drawable.iter().copied().collect();
        let merge_frontier = original_merge_parent_dependencies(terrain);
        let queued_addresses: HashSet<_> = terrain
            .uploads
            .iter()
            .map(|upload| upload.address)
            .collect();
        let mut wanted: BTreeSet<_> = terrain.desired.keys().copied().collect();
        wanted.extend(terrain.config.roots.iter().copied());
        wanted.extend(original_scheduled_dependencies(terrain));
        let priorities: BTreeMap<_, _> = terrain
            .desired
            .iter()
            .map(|(address, patch)| (*address, patch.priority))
            .collect();
        let mut candidates: Vec<_> = wanted
            .into_iter()
            .filter(|address| {
                !terrain.resident.contains(address) && !queued_addresses.contains(address)
            })
            .filter(|address| {
                if terrain.drawable.is_empty() {
                    terrain.config.roots.contains(address)
                } else {
                    drawable.contains(address)
                        || merge_frontier.contains(address)
                        || address
                            .parent()
                            .is_some_and(|parent| drawable.contains(&parent))
                }
            })
            .filter(|address| {
                terrain.reserved_split_parent.is_none_or(|parent| {
                    drawable.contains(address)
                        || merge_frontier.contains(address)
                        || parent
                            .children()
                            .is_ok_and(|children| children.contains(address))
                })
            })
            .filter(|address| {
                terrain
                    .upload_allowlist
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(address))
            })
            .filter_map(|address| {
                terrain.cached_tiles_by_address.get(&address)?;
                let priority = priorities
                    .get(&address)
                    .copied()
                    .unwrap_or_else(|| terrain.score(address).1);
                Some((priority, address))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        candidates
    }

    fn cache_test_tiles(terrain: &mut RegionalTerrain, addresses: &[CubePatchAddress]) {
        for &address in addresses {
            let (tile, _) = ResidentTileBuilder::build(
                &terrain.generator,
                terrain.identity,
                address,
                terrain.config.cells,
            )
            .unwrap();
            assert!(matches!(
                terrain.cache_insert(Arc::new(tile), false, None),
                CacheInsertResult::Inserted | CacheInsertResult::AlreadyCached
            ));
        }
    }

    #[test]
    fn cached_upload_frontier_matches_original_admission_oracle() {
        let all_roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        for scenario in 0..4 {
            let finite_root = all_roots[4].children().unwrap()[scenario % 4];
            let roots = if scenario >= 2 {
                vec![finite_root]
            } else {
                all_roots.clone()
            };
            let mut terrain = test_terrain(roots.clone());
            let drawable = if scenario >= 2 {
                vec![finite_root]
            } else {
                let split_root = all_roots[scenario];
                all_roots
                    .iter()
                    .copied()
                    .filter(|root| *root != split_root)
                    .chain(split_root.children().unwrap())
                    .collect()
            };
            terrain.drawable = drawable.clone();
            for (parent_index, parent) in drawable.iter().copied().enumerate().take(5) {
                let Ok(children) = parent.children() else {
                    continue;
                };
                for (child_index, child) in children.into_iter().enumerate() {
                    let priority = (parent_index * 4 + child_index) as f64 + 0.25;
                    terrain.desired.insert(
                        child,
                        RegionalPatchSnapshot {
                            address: format!("{child:?}"),
                            level: child.level(),
                            projected_error_px: priority,
                            priority,
                            state: "desired",
                        },
                    );
                }
            }

            let mut possible = BTreeSet::new();
            possible.extend(roots.iter().copied());
            possible.extend(drawable.iter().copied());
            possible.extend(terrain.desired.keys().copied());
            for &address in terrain.desired.keys() {
                let mut ancestor = address.parent();
                while let Some(current) = ancestor {
                    possible.insert(current);
                    ancestor = current.parent();
                }
            }
            // Include an ancestor outside a finite root to verify root clipping,
            // plus unrelated cached leaves which should not become candidates.
            possible.extend(all_roots.iter().copied());
            possible.extend(all_roots[5].children().unwrap());
            let possible: Vec<_> = possible.into_iter().collect();
            let cached: Vec<_> = possible
                .iter()
                .enumerate()
                .filter_map(|(index, address)| ((index + scenario) % 3 != 1).then_some(*address))
                .collect();
            cache_test_tiles(&mut terrain, &cached);

            terrain.resident.extend(
                cached
                    .iter()
                    .copied()
                    .filter(|address| address.level() % 3 == 0)
                    .take(2),
            );
            terrain.external_pins.extend(cached.iter().copied().take(1));
            if scenario % 2 == 1 {
                terrain.reserved_split_parent = Some(roots[0]);
                terrain.upload_allowlist = Some(
                    cached
                        .iter()
                        .copied()
                        .filter(|address| address.coordinates()[0] % 2 == 0)
                        .collect(),
                );
            }

            let before_queue = original_upload_candidate_oracle(&terrain);
            if scenario % 2 == 1
                && let Some((_, address)) = before_queue.first().copied()
                && let Some(tile) = terrain.cached_tiles_by_address.get(&address).cloned()
            {
                let bytes = tile_bytes(&tile);
                terrain.uploads.push(RegionalUpload {
                    address,
                    tile,
                    bytes,
                });
                terrain.upload_bytes = bytes;
            }

            let expected = original_upload_candidate_oracle(&terrain);
            let actual = terrain
                .upload_candidates()
                .into_iter()
                .map(|(rank, address, _)| (rank.priority, address))
                .collect::<Vec<_>>();
            assert_eq!(actual.len(), expected.len(), "scenario {scenario}");
            for ((actual_priority, actual_address), (expected_priority, expected_address)) in
                actual.iter().zip(&expected)
            {
                assert_eq!(actual_address, expected_address, "scenario {scenario}");
                assert_eq!(
                    actual_priority.to_bits(),
                    expected_priority.to_bits(),
                    "scenario {scenario}, address {actual_address:?}"
                );
            }

            // Admission order and queue-cap pressure must remain equivalent too.
            let initial_uploads = terrain.uploads.len();
            let initial_addresses: Vec<_> = terrain
                .uploads
                .iter()
                .map(|upload| upload.address)
                .collect();
            if let Some((_, address)) = expected.first().copied() {
                let first_tile_bytes = tile_bytes(&terrain.cached_tiles_by_address[&address]);
                terrain.config.upload_tile_cap = initial_uploads + expected.len() + 1;
                terrain.config.upload_byte_cap = terrain
                    .upload_bytes
                    .saturating_add(first_tile_bytes.saturating_sub(1));
                terrain.admit_uploads();
                assert_eq!(
                    terrain
                        .uploads
                        .iter()
                        .map(|upload| upload.address)
                        .collect::<Vec<_>>(),
                    initial_addresses,
                    "byte pressure scenario {scenario}"
                );
                assert!(terrain.pressure.upload, "byte pressure scenario {scenario}");
                terrain.pressure.upload = false;
            }
            let remaining = usize::from(!expected.is_empty());
            terrain.config.upload_tile_cap = initial_uploads + remaining;
            terrain.config.upload_byte_cap = usize::MAX;
            terrain.admit_uploads();
            let actual_addresses: Vec<_> = terrain
                .uploads
                .iter()
                .map(|upload| upload.address)
                .collect();
            let mut expected_addresses = initial_addresses;
            expected_addresses.extend(expected.iter().take(remaining).map(|(_, address)| *address));
            assert_eq!(actual_addresses, expected_addresses, "scenario {scenario}");
            assert_eq!(
                terrain.pressure.upload,
                expected.len() > remaining,
                "scenario {scenario}"
            );
        }
    }

    #[test]
    fn drawable_proxy_hotspots_are_bounded_stable_and_carry_exact_tile_identity() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        let drawable: Vec<_> = roots
            .iter()
            .flat_map(|root| root.children().unwrap())
            .collect();
        terrain.drawable = drawable.clone();

        let mut scores = HashMap::new();
        let mut expected: Vec<_> = drawable
            .iter()
            .copied()
            .map(|address| (address, terrain.selection_score(address, &mut scores).0))
            .collect();
        expected.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let top = expected
            .iter()
            .take(8)
            .map(|(address, _)| *address)
            .collect::<Vec<_>>();
        let desired_address = top[0];
        let resident_address = top[1];
        let desired_priority = 4.0;
        terrain.desired.insert(
            desired_address,
            RegionalPatchSnapshot {
                address: format!("{desired_address:?}"),
                level: desired_address.level(),
                projected_error_px: desired_priority,
                priority: desired_priority,
                state: "desired",
            },
        );
        terrain.resident.insert(resident_address);
        let blocked_parent = desired_address.parent().unwrap();
        terrain.blocked_publication_parents.insert(blocked_parent);
        terrain.upload_allowlist = Some(BTreeSet::from([desired_address, resident_address]));

        let snapshot = terrain.snapshot();
        let repeated = terrain.snapshot();
        assert_eq!(snapshot.visible_drawable_proxy_hotspots.len(), 8);
        assert_eq!(
            snapshot
                .visible_drawable_proxy_hotspots
                .iter()
                .map(|hotspot| hotspot.address.as_str())
                .collect::<Vec<_>>(),
            repeated
                .visible_drawable_proxy_hotspots
                .iter()
                .map(|hotspot| hotspot.address.as_str())
                .collect::<Vec<_>>()
        );
        for ((expected_address, expected_error), hotspot) in expected
            .iter()
            .take(8)
            .zip(&snapshot.visible_drawable_proxy_hotspots)
        {
            assert_eq!(hotspot.address, format!("{expected_address:?}"));
            assert_eq!(hotspot.error_px.to_bits(), expected_error.to_bits());
            let key = terrain.key_for(*expected_address).unwrap();
            let actual_key = hotspot.key.as_ref().unwrap();
            assert_eq!(actual_key.body_identity, key.body_identity);
            assert_eq!(actual_key.definition_words, key.definition_words);
            assert_eq!(actual_key.radius_bits, key.radius_bits);
            assert_eq!(actual_key.surface_revision, key.surface_revision);
            assert_eq!(actual_key.material_revision, key.material_revision);
            assert_eq!(actual_key.format_version, key.format_version);
            assert_eq!(actual_key.filter_version, key.filter_version);
            assert_eq!(actual_key.cells, key.cells);
            assert_eq!(
                hotspot.parent,
                expected_address.parent().map(|p| format!("{p:?}"))
            );
        }

        let desired_hotspot = snapshot
            .visible_drawable_proxy_hotspots
            .iter()
            .find(|hotspot| hotspot.address == format!("{desired_address:?}"))
            .unwrap();
        assert!(desired_hotspot.desired);
        assert!(desired_hotspot.publication_parent_blocked);
        assert_eq!(desired_hotspot.upload_allowlisted, Some(true));
        assert_eq!(desired_hotspot.blocker_status, "publication_parent_blocked");
        let resident_hotspot = snapshot
            .visible_drawable_proxy_hotspots
            .iter()
            .find(|hotspot| hotspot.address == format!("{resident_address:?}"))
            .unwrap();
        assert!(resident_hotspot.resident);
        assert_eq!(resident_hotspot.upload_allowlisted, Some(true));

        if let Some(worst) = &snapshot.worst_proxy {
            assert_eq!(
                worst.address,
                snapshot.visible_drawable_proxy_hotspots[0].address
            );
            assert_eq!(
                worst.error_px.to_bits(),
                snapshot.visible_drawable_proxy_hotspots[0]
                    .error_px
                    .to_bits()
            );
            assert_eq!(
                worst.projected_footprint_weight.to_bits(),
                snapshot.visible_drawable_proxy_hotspots[0]
                    .projected_footprint_weight
                    .to_bits()
            );
        }
        if let Some(worst) = &snapshot.worst_visible_unresolved {
            assert!(worst.error_px > snapshot.convergence_target_error_px);
            assert!(
                snapshot
                    .visible_drawable_proxy_hotspots
                    .iter()
                    .any(|hotspot| hotspot.address == worst.address)
            );
        }
    }

    fn indexed_split_frontier_groups(terrain: &RegionalTerrain) -> Vec<RegionalSplitFrontier> {
        let desired_addresses: Vec<_> = terrain.desired.keys().copied().collect();
        let (_, desired_descendants) = cover_address_index(&desired_addresses);
        let (drawable_set, drawable_descendants) = cover_address_index(&terrain.drawable);
        let mut groups = Vec::new();
        for &parent in &terrain.drawable {
            let Ok(children) = parent.children() else {
                continue;
            };
            let child_priorities: Vec<_> = children
                .iter()
                .map(|child| {
                    desired_descendants
                        .get(child)
                        .into_iter()
                        .flatten()
                        .filter_map(|address| terrain.desired.get(address))
                        .map(|patch| patch.priority)
                        .max_by(f64::total_cmp)
                })
                .collect();
            if child_priorities.iter().any(Option::is_none)
                || !replacement_is_balanced(parent, true, &drawable_set, &drawable_descendants)
            {
                continue;
            }
            let ready_children = children
                .iter()
                .filter(|child| {
                    terrain.resident.contains(child)
                        || terrain
                            .key_for(**child)
                            .is_ok_and(|key| terrain.cache.contains_key(&key))
                        || terrain
                            .uploads
                            .iter()
                            .any(|upload| upload.address == **child)
                        || terrain
                            .key_for(**child)
                            .is_ok_and(|key| terrain.in_flight.contains_key(&key))
                })
                .count();
            let resident_children = children
                .iter()
                .filter(|child| terrain.resident.contains(child))
                .count();
            let aggregate_priority = child_priorities.iter().flatten().copied().sum();
            groups.push(RegionalSplitFrontier {
                parent,
                children,
                parent_resident: terrain.resident.contains(&parent),
                ready_children,
                aggregate_priority,
                completion_priority: aggregate_priority,
                age: Duration::ZERO,
                useful_floor_deficit_px: 0.0,
                resident_children,
                missing_dependencies: children.len().saturating_sub(ready_children),
                blocker: RegionalCompletionBlocker::Admission,
            });
        }
        if terrain.planetary_view.is_none() {
            groups.sort_by(|a, b| {
                b.aggregate_priority
                    .total_cmp(&a.aggregate_priority)
                    .then_with(|| a.parent.cmp(&b.parent))
            });
        }
        groups
    }

    fn indexed_merge_frontiers(terrain: &RegionalTerrain) -> Vec<CubePatchAddress> {
        let (cover, descendants) = cover_address_index(&terrain.drawable);
        let desired: HashSet<_> = terrain.desired.keys().copied().collect();
        terrain
            .drawable
            .iter()
            .filter_map(|child| child.parent())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter(|parent| {
                let mut ancestor = Some(*parent);
                while let Some(address) = ancestor {
                    if desired.contains(&address) {
                        return replacement_is_balanced(*parent, false, &cover, &descendants);
                    }
                    ancestor = address.parent();
                }
                false
            })
            .collect()
    }

    #[test]
    fn allocation_light_frontiers_match_indexed_oracle_on_balanced_cube_covers() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut cover = roots.clone();
        let priorities = [
            -1.0e20,
            1.0e20,
            -3.25,
            -0.0,
            0.0,
            f64::from_bits(0x3fd5_5555_5555_5555),
            1.0e-100,
        ];

        for sample in 0..16 {
            if sample > 0 {
                let splittable: Vec<_> = cover
                    .iter()
                    .copied()
                    .filter(|address| address.level() < 7)
                    .collect();
                let parent = splittable[(sample * 13) % splittable.len()];
                let mut proposed: Vec<_> = cover
                    .iter()
                    .copied()
                    .filter(|address| *address != parent)
                    .collect();
                proposed.extend(parent.children().unwrap());
                cover = balance_cover(&roots, proposed, 8, 2_048, None).unwrap();
            }

            let mut terrain = test_terrain(roots.clone());
            terrain.drawable = cover.clone();
            terrain.resident.extend(
                cover
                    .iter()
                    .copied()
                    .filter(|address| (usize::from(address.level()) + sample) % 3 == 0),
            );
            terrain.desired.clear();
            for (parent_index, parent) in cover.iter().copied().enumerate() {
                let Ok(children) = parent.children() else {
                    continue;
                };
                for (child_index, child) in children.into_iter().enumerate() {
                    if (parent_index + child_index + sample) % 5 == 0 {
                        continue;
                    }
                    let priority =
                        priorities[(parent_index + child_index + sample) % priorities.len()];
                    terrain.desired.insert(
                        child,
                        RegionalPatchSnapshot {
                            address: format!("{child:?}"),
                            level: child.level(),
                            projected_error_px: priority,
                            priority,
                            state: "desired",
                        },
                    );
                    if let Ok(grandchildren) = child.children()
                        && (parent_index + child_index + sample) % 2 == 0
                    {
                        let grandchild = grandchildren[(sample + child_index) % 4];
                        let nested_priority = priorities
                            [(parent_index + child_index + sample + 3) % priorities.len()];
                        terrain.desired.insert(
                            grandchild,
                            RegionalPatchSnapshot {
                                address: format!("{grandchild:?}"),
                                level: grandchild.level(),
                                projected_error_px: nested_priority,
                                priority: nested_priority,
                                state: "desired",
                            },
                        );
                    }
                    if (parent_index + child_index + sample) % 4 == 0 {
                        terrain.resident.insert(child);
                    }
                }
            }

            let actual = terrain.split_frontier_groups();
            let oracle = indexed_split_frontier_groups(&terrain);
            assert_eq!(actual.len(), oracle.len(), "sample {sample}");
            for (actual, oracle) in actual.iter().zip(&oracle) {
                assert_eq!(actual.parent, oracle.parent, "sample {sample}");
                assert_eq!(actual.children, oracle.children, "sample {sample}");
                assert_eq!(actual.parent_resident, oracle.parent_resident);
                assert_eq!(actual.ready_children, oracle.ready_children);
                assert_eq!(
                    actual.aggregate_priority.to_bits(),
                    oracle.aggregate_priority.to_bits(),
                    "sample {sample}, parent {:?}",
                    actual.parent
                );
            }
            assert_eq!(terrain.merge_frontiers(), indexed_merge_frontiers(&terrain));
        }
    }

    #[test]
    fn desired_priority_index_invalidates_on_exact_priority_and_address_changes() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        terrain.drawable = roots.clone();
        let parent = roots[0];
        let children = parent.children().unwrap();
        for (index, child) in children.iter().copied().enumerate() {
            let priority = (index + 1) as f64;
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }

        let initial = terrain.split_frontier_groups();
        assert_eq!(initial, indexed_split_frontier_groups(&terrain));
        let first_revision = terrain.desired_priority_revision();
        let initial_priority = initial
            .iter()
            .find(|group| group.parent == parent)
            .unwrap()
            .aggregate_priority;

        // The address set is unchanged; the exact priority bits must invalidate
        // the cached ancestor maxima and preserve the indexed oracle's result.
        let changed_child = children[0];
        let changed_patch = terrain.desired.get_mut(&changed_child).unwrap();
        changed_patch.priority = 12.125;
        let changed_priority = terrain.split_frontier_groups();
        assert_eq!(changed_priority, indexed_split_frontier_groups(&terrain));
        assert!(terrain.desired_priority_revision() > first_revision);
        let changed_aggregate = changed_priority
            .iter()
            .find(|group| group.parent == parent)
            .unwrap()
            .aggregate_priority;
        assert_ne!(changed_aggregate.to_bits(), initial_priority.to_bits());

        // Replacing one desired address with descendant addresses changes the
        // exact signature and must roll the same ancestor index forward.
        terrain.desired.remove(&changed_child);
        for (index, descendant) in changed_child.children().unwrap().into_iter().enumerate() {
            let priority = 8.0 + index as f64;
            terrain.desired.insert(
                descendant,
                RegionalPatchSnapshot {
                    address: format!("{descendant:?}"),
                    level: descendant.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }
        let changed_address = terrain.split_frontier_groups();
        assert_eq!(changed_address, indexed_split_frontier_groups(&terrain));
        assert!(terrain.desired_priority_revision() > first_revision + 1);
    }

    #[test]
    fn desired_priority_revision_reuses_rank_summary_across_readiness_churn() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        terrain.drawable = roots.clone();
        let parent = roots[0];
        let children = parent.children().unwrap();
        for (index, child) in children.iter().copied().enumerate() {
            let priority = (index + 1) as f64;
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }

        let initial_ready_children = terrain
            .split_frontiers()
            .iter()
            .find(|group| group.parent == parent)
            .unwrap()
            .ready_children;
        assert_eq!(initial_ready_children, 0);
        let revision = terrain.desired_priority_revision();
        let scans = terrain
            .desired_priority_index
            .borrow()
            .completion_frontier_scans;
        for _ in 0..4 {
            assert_eq!(terrain.desired_priority_revision(), revision);
        }
        assert_eq!(
            terrain
                .desired_priority_index
                .borrow()
                .completion_frontier_scans,
            scans,
            "an unchanged policy query must not enumerate drawable parents"
        );

        terrain.resident.extend(children);
        let ready = terrain.split_frontiers();
        assert_eq!(
            ready
                .iter()
                .find(|group| group.parent == parent)
                .unwrap()
                .ready_children,
            4
        );
        assert_eq!(terrain.desired_priority_revision(), revision);
        assert_eq!(
            terrain
                .desired_priority_index
                .borrow()
                .completion_frontier_scans,
            scans + 1,
            "readiness diagnostics may refresh without invalidating rank policy"
        );
    }

    #[test]
    fn desired_priority_revision_refreshes_for_topology_view_and_age_inputs() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        terrain.drawable = roots;
        let parent = CubePatchAddress::root(CubeFace::PositiveZ);
        for (index, child) in parent.children().unwrap().into_iter().enumerate() {
            let priority = (index + 1) as f64;
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }
        let initial_revision = terrain.desired_priority_revision();
        let mut scans = terrain
            .desired_priority_index
            .borrow()
            .completion_frontier_scans;

        let projection =
            CelestialProjection::try_new(800, 600, 70.0_f64.to_radians(), 0.1).unwrap();
        terrain
            .set_planetary_view(DMat3::IDENTITY, projection)
            .unwrap();
        let after_view = terrain.desired_priority_revision();
        assert!(after_view >= initial_revision);
        scans += 1;
        assert_eq!(
            terrain
                .desired_priority_index
                .borrow()
                .completion_frontier_scans,
            scans,
            "changed view weights refresh the cached rank summary"
        );
        terrain.drawable_cover_revision = terrain.drawable_cover_revision.wrapping_add(1);
        let after_topology = terrain.desired_priority_revision();
        assert!(after_topology >= after_view);
        scans += 1;
        assert_eq!(
            terrain
                .desired_priority_index
                .borrow()
                .completion_frontier_scans,
            scans,
            "changed drawable topology refreshes the cached rank summary"
        );
        let expired = Instant::now() - Duration::from_millis(1);
        terrain
            .desired_priority_index
            .borrow_mut()
            .next_completion_promotion = Some(expired);
        let after_age = terrain.desired_priority_revision();
        scans += 1;
        assert_eq!(
            terrain
                .desired_priority_index
                .borrow()
                .completion_frontier_scans,
            scans,
            "an elapsed age-promotion deadline refreshes the rank summary"
        );
        assert!(after_age >= after_topology);
    }

    #[test]
    fn sampled_coverage_availability_invalidates_same_view_frontier_priority() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        terrain.drawable = roots.clone();
        terrain.resident.extend(roots);
        let parent = CubePatchAddress::root(CubeFace::PositiveZ);
        let projection =
            CelestialProjection::try_new(800, 600, 70.0_f64.to_radians(), 0.1).unwrap();
        terrain
            .set_planetary_view(DMat3::IDENTITY, projection)
            .unwrap();
        terrain.view = RegionalView {
            body_position_m: DVec3::Z * (terrain.generator.radius_m() + 1_000.0),
            body_velocity_mps: DVec3::ZERO,
            projection_scale_px: projection.focal_pixels(),
        };
        terrain
            .planetary_footprint_weights
            .borrow_mut()
            .insert(parent, 1.0);
        for (index, child) in parent.children().unwrap().into_iter().enumerate() {
            let priority = (index + 1) as f64;
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: priority,
                    priority,
                    state: "desired",
                },
            );
        }
        let without_mask = terrain.desired_priority_revision();
        let old_rank = terrain
            .split_frontiers()
            .into_iter()
            .find(|group| group.parent == parent)
            .unwrap()
            .completion_priority;

        let mut sampled_areas = HashMap::new();
        sampled_areas.insert(parent, 100.0);
        terrain
            .screen_coverage_cache
            .replace(Some(ScreenCoverageCacheEntry {
                key: terrain.screen_coverage_key().unwrap(),
                diagnostic: ScreenCoverageDiagnostic {
                    method: "test sampled cover",
                    certified: false,
                    limitation: "unit-test fixture",
                    sampling_valid: true,
                    samples_per_cell_axis: 1,
                    useful_proxy_error_threshold_px: USEFUL_DETAIL_FLOOR_ERROR_PX,
                    target_proxy_error_threshold_px: terrain.config.split_threshold_px,
                    all_view: ScreenCoverageCell::default(),
                    cells: std::array::from_fn(|_| ScreenCoverageCell::default()),
                    sampled_drawable_area_pixels: Arc::new(sampled_areas),
                },
            }));

        let with_mask = terrain.desired_priority_revision();
        let new_rank = terrain
            .split_frontiers()
            .into_iter()
            .find(|group| group.parent == parent)
            .unwrap()
            .completion_priority;
        assert_ne!(old_rank.to_bits(), new_rank.to_bits());
        assert!(with_mask > without_mask);
    }

    #[test]
    fn local_publication_descriptor_acks_one_sorted_delta_and_public_api_keeps_full_cover() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        let parent = roots[0];
        let children = parent.children().unwrap();
        let independent_parent = roots[1];
        let independent_children = independent_parent.children().unwrap();
        terrain.drawable = roots.clone();
        terrain.drawable.sort_unstable();
        terrain.resident.extend(roots.iter().copied());
        terrain.resident.extend(children);
        terrain.resident.extend(independent_children);
        for child in children.into_iter().chain(independent_children) {
            terrain.desired.insert(
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: 2.0,
                    priority: 2.0,
                    state: "resident",
                },
            );
        }
        terrain.refresh_publications();
        let local = terrain.publication_local_candidates();
        assert_eq!(local.len(), 2);
        let first = local
            .iter()
            .find(|candidate| publication_parent(candidate) == parent)
            .unwrap();
        let second = local
            .iter()
            .find(|candidate| publication_parent(candidate) == independent_parent)
            .unwrap();
        assert!(first.cover().is_empty());
        assert!(terrain.publication_locally_current(first));
        let full = terrain.publication_candidates();
        assert_eq!(full.len(), 2);
        assert!(
            full.iter()
                .all(|candidate| terrain.cover_is_valid(candidate.cover()))
        );

        terrain.resident.remove(&children[0]);
        assert!(terrain.publication_locally_current(first));
        let before_rejected_ack = terrain.drawable_cover_revision;
        assert_eq!(
            terrain.ack_local_publication(first),
            Err(RegionalError::NotResident)
        );
        assert_eq!(terrain.drawable_cover_revision, before_rejected_ack);
        terrain.resident.insert(children[0]);
        let before_local_ack = terrain.drawable_cover_revision;
        terrain.ack_local_publication(first).unwrap();
        assert_eq!(terrain.drawable_cover_revision, before_local_ack + 1);
        let before_stale_ack = terrain.drawable_cover_revision;
        assert_eq!(
            terrain.ack_local_publication(first),
            Err(RegionalError::InvalidCover)
        );
        assert_eq!(terrain.drawable_cover_revision, before_stale_ack);
        assert!(terrain.publication_locally_current(second));
        let before_second_ack = terrain.drawable_cover_revision;
        terrain.ack_local_publication(second).unwrap();
        assert_eq!(terrain.drawable_cover_revision, before_second_ack + 1);
        assert!(terrain.drawable.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!terrain.drawable.contains(&parent));
        assert!(!terrain.drawable.contains(&independent_parent));
        assert!(
            children
                .iter()
                .all(|child| terrain.drawable.contains(child))
        );
        assert!(
            independent_children
                .iter()
                .all(|child| terrain.drawable.contains(child))
        );
        assert!(terrain.cover_is_valid(&terrain.drawable));
    }

    #[test]
    fn prepared_merge_can_finish_after_demand_reverses() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = test_terrain(roots.clone());
        let parent = roots[0];
        let children = parent.children().unwrap();
        terrain.drawable = roots
            .iter()
            .copied()
            .filter(|root| *root != parent)
            .chain(children)
            .collect();
        terrain.drawable.sort_unstable();
        terrain.resident.extend(terrain.drawable.iter().copied());
        terrain.resident.insert(parent);
        // A reversed selector target no longer contains the merge parent.
        terrain.desired.extend(children.into_iter().map(|child| {
            (
                child,
                RegionalPatchSnapshot {
                    address: format!("{child:?}"),
                    level: child.level(),
                    projected_error_px: 0.0,
                    priority: 0.0,
                    state: "resident",
                },
            )
        }));
        terrain.ack_prepared_merge(parent, children).unwrap();
        assert_eq!(terrain.drawable, roots);
        assert!(terrain.cover_is_valid(&terrain.drawable));
    }

    #[test]
    fn compact_snapshots_preserve_scalars_and_stationary_scheduling_reacts_to_changes() {
        use crate::resident_terrain::ResidentTileBuilder;
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
        };
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x2d75),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let identity = TileBuildIdentity {
            body_identity: 1,
            surface_revision: 1,
            material_revision: 1,
        };
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut terrain = RegionalTerrain::new(
            generator.clone(),
            identity,
            RegionalConfig {
                roots: roots.clone(),
                cells: 2,
                worker_delay: Duration::from_secs(30),
                ..Default::default()
            },
        )
        .unwrap();
        for address in &roots {
            let tile = ResidentTileBuilder::build(&generator, identity, *address, 2)
                .unwrap()
                .0;
            terrain.seed_tile(Arc::new(tile)).unwrap();
            terrain.ack_resident(*address);
        }
        let projection =
            CelestialProjection::try_new(800, 600, 70.0_f64.to_radians(), 0.1).unwrap();
        terrain
            .set_planetary_view(DMat3::IDENTITY, projection)
            .unwrap();
        let far = RegionalView {
            body_position_m: DVec3::Z * 80_000_000_000.0,
            body_velocity_mps: DVec3::ZERO,
            projection_scale_px: projection.focal_pixels(),
        };
        for _ in 0..20 {
            terrain.tick(far, Duration::from_millis(16)).unwrap();
        }
        assert!(terrain.scheduler_fixed_point_reused);
        assert!(terrain.is_idle());
        let mut detailed = serde_json::to_value(terrain.snapshot()).unwrap();
        let mut compact = serde_json::to_value(terrain.snapshot_summary()).unwrap();
        assert_eq!(detailed["desired"].as_array().unwrap().len(), 6);
        for field in ["desired", "resident", "drawable"] {
            assert!(compact[field].as_array().unwrap().is_empty());
            detailed.as_object_mut().unwrap().remove(field);
            compact.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(compact, detailed);
        terrain.set_external_pins(&[roots[0].children().unwrap()[0]]);
        terrain.tick(far, Duration::from_millis(16)).unwrap();
        assert!(!terrain.scheduler_fixed_point_reused);
        terrain.set_external_pins(&[]);
        terrain.mark_not_resident(roots[0]);
        terrain.tick(far, Duration::from_millis(16)).unwrap();
        assert!(!terrain.scheduler_fixed_point_reused);
        assert!(
            terrain
                .queued_uploads()
                .iter()
                .any(|upload| upload.address == roots[0])
        );
        terrain.ack_resident(roots[0]);
        for _ in 0..3 {
            terrain.tick(far, Duration::from_millis(16)).unwrap();
        }
        assert!(terrain.scheduler_fixed_point_reused);
        terrain
            .tick(
                RegionalView {
                    body_position_m: DVec3::Z * 80_250.0,
                    ..far
                },
                Duration::from_millis(16),
            )
            .unwrap();
        assert!(!terrain.scheduler_fixed_point_reused);
        assert!(terrain.desired().len() > roots.len());
    }

    #[test]
    fn local_merge_acknowledgement_rejects_wrong_cover_and_missing_residency() {
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
        };
        let mut roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        roots.sort();
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x2d72),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let mut terrain = RegionalTerrain::new(
            generator,
            TileBuildIdentity {
                body_identity: 1,
                surface_revision: 1,
                material_revision: 1,
            },
            RegionalConfig {
                roots: roots.clone(),
                cells: 4,
                ..Default::default()
            },
        )
        .unwrap();
        let parent = roots[0];
        let children = parent.children().unwrap();
        terrain.drawable = roots
            .iter()
            .copied()
            .filter(|p| *p != parent)
            .chain(children)
            .collect();
        terrain.drawable.sort();
        terrain.resident.extend(terrain.drawable.iter().copied());
        let original = terrain.drawable.clone();
        let before_rejected_merge = terrain.drawable_cover_revision;
        assert_eq!(
            terrain.ack_local_merge(parent, children, &roots[..5]),
            Err(RegionalError::InvalidCover)
        );
        assert_eq!(
            terrain.ack_local_merge(parent, children, &roots),
            Err(RegionalError::NotResident)
        );
        assert_eq!(terrain.drawable_cover_revision, before_rejected_merge);
        assert_eq!(terrain.drawable, original);
        terrain.resident.insert(parent);
        let before_merge = terrain.drawable_cover_revision;
        terrain.ack_local_merge(parent, children, &roots).unwrap();
        assert_eq!(terrain.drawable_cover_revision, before_merge + 1);
        assert_eq!(terrain.drawable, roots);
        assert!(terrain.cover_is_valid(&terrain.drawable));
    }

    #[test]
    fn publication_limit_counts_valid_proposals_instead_of_blocking_behind_invalid_first() {
        use mundaris_world::terrain::{
            SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
        };
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut cover = roots.clone();
        for step in 0..48 {
            let (set, descendants) = cover_address_index(&cover);
            if !replacement_is_balanced(cover[0], true, &set, &descendants)
                && let Some(expected) = cover
                    .iter()
                    .copied()
                    .find(|parent| replacement_is_balanced(*parent, true, &set, &descendants))
            {
                let definition = SurfaceDefinition::generated(
                    TerrainIdentity(0x2d71),
                    TerrainSeed(0),
                    SurfaceAlgorithm::RockyV5,
                );
                let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
                let mut terrain = RegionalTerrain::new(
                    generator,
                    TileBuildIdentity {
                        body_identity: 1,
                        surface_revision: 1,
                        material_revision: 1,
                    },
                    RegionalConfig {
                        roots: roots.clone(),
                        cells: 4,
                        max_level: 24,
                        max_desired_patches: 2048,
                        publication_cap_per_tick: 1,
                        ..Default::default()
                    },
                )
                .unwrap();
                terrain.drawable = cover.clone();
                terrain.desired.clear();
                for parent in &cover {
                    terrain.resident.insert(*parent);
                    for child in parent.children().unwrap() {
                        terrain.resident.insert(child);
                        terrain.desired.insert(
                            child,
                            RegionalPatchSnapshot {
                                address: format!("{child:?}"),
                                level: child.level(),
                                projected_error_px: 0.0,
                                priority: 1.0,
                                state: "resident",
                            },
                        );
                    }
                }
                terrain.refresh_publications();
                let proposals = terrain.publication_candidates();
                assert_eq!(proposals.len(), 1);
                assert!(
                    matches!(proposals[0], RegionalPublication::Split { parent, .. } if parent == expected)
                );
                assert!(terrain.cover_is_valid(proposals[0].cover()));
                terrain.set_publication_blocked_parents(&[expected]);
                terrain.refresh_publications();
                let proposals = terrain.publication_candidates();
                assert_eq!(proposals.len(), 1);
                assert!(
                    matches!(proposals[0], RegionalPublication::Split { parent, .. } if parent != expected)
                );
                assert!(terrain.cover_is_valid(proposals[0].cover()));
                terrain.set_publication_blocked_parents(&[]);
                terrain.refresh_publications();
                let proposals = terrain.publication_candidates();
                assert!(
                    matches!(proposals[0], RegionalPublication::Split { parent, .. } if parent == expected)
                );
                return;
            }
            let parent = cover[(step * 17) % cover.len()];
            let mut next: Vec<_> = cover.iter().copied().filter(|p| *p != parent).collect();
            next.extend(parent.children().unwrap());
            cover = balance_cover(&roots, next, 24, 1024, None).unwrap();
        }
        panic!("fixture did not exercise an invalid first proposal");
    }

    #[test]
    fn indexed_local_replacements_match_full_cross_face_cover_validation() {
        let roots: Vec<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        let mut cover = roots.clone();
        let mut rejected_splits = 0;
        let mut rejected_merges = 0;
        for step in 0..48 {
            let (set, descendants) = cover_address_index(&cover);
            for &parent in &cover {
                let mut trial: Vec<_> = cover.iter().copied().filter(|p| *p != parent).collect();
                trial.extend(parent.children().unwrap());
                let full = valid_cover(&roots, &trial, 24);
                let globally_balanced =
                    balance_cover(&roots, trial.clone(), 24, 1024, None).unwrap();
                let locally_balanced = balance_planetary_split(
                    &cover,
                    parent,
                    24,
                    1024,
                    1024,
                    std::time::Instant::now(),
                    Duration::from_secs(1),
                )
                .unwrap();
                assert_eq!(locally_balanced, globally_balanced);
                assert_eq!(
                    replacement_is_balanced(parent, true, &set, &descendants),
                    full
                );
                assert_eq!(
                    local_replacement_is_balanced(parent, true, &cover, &roots),
                    full
                );
                rejected_splits += usize::from(!full);
            }
            for parent in cover
                .iter()
                .filter_map(|p| p.parent())
                .collect::<BTreeSet<_>>()
            {
                let children = parent.children().unwrap();
                if children.iter().all(|p| set.contains(p)) {
                    let mut trial: Vec<_> = cover
                        .iter()
                        .copied()
                        .filter(|p| !children.contains(p))
                        .collect();
                    trial.push(parent);
                    let full = valid_cover(&roots, &trial, 24);
                    assert_eq!(planetary_merge_is_balanced(parent, &set), full);
                    assert_eq!(
                        replacement_is_balanced(parent, false, &set, &descendants),
                        full
                    );
                    assert_eq!(
                        local_replacement_is_balanced(parent, false, &cover, &roots),
                        full
                    );
                    rejected_merges += usize::from(!full);
                }
            }
            let parent = cover[(step * 17) % cover.len()];
            let mut next: Vec<_> = cover.iter().copied().filter(|p| *p != parent).collect();
            next.extend(parent.children().unwrap());
            cover = balance_cover(&roots, next, 24, 1024, None).unwrap();
        }
        assert!(rejected_splits > 0 && rejected_merges > 0);
    }
}
