//! Bounded, revision-keyed terrain pipeline trace.
//!
//! This is diagnostic state only. It records monotonic milestones for immutable
//! tile identities and keeps both its job table and event ring bounded.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, TryLockError};
use std::time::Instant;

use mundaris_math::surface::CubePatchAddress;
use mundaris_math::surface::PatchEdge;
use serde::Serialize;

use crate::resident_terrain::TileKey;

const MAX_JOBS: usize = 32_768;
const MAX_EVENTS: usize = 8_192;
const SNAPSHOT_JOBS: usize = 128;
const SNAPSHOT_EVENTS: usize = 256;
const RATE_WINDOW_US: u64 = 10_000_000;

/// Stable pipeline milestones. Variants are static so hot-path recording does
/// not allocate labels or format addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Requested,
    Desired,
    GenerationQueued,
    GenerationStarted,
    GenerationCompleted,
    GenerationCancelled,
    GenerationFailed,
    StaleCompletion,
    CpuEvicted,
    Resident,
    Drawable,
    BoundaryQueued,
    BoundaryStarted,
    BoundaryFinished,
    BoundaryFailed,
    FullyPrepared,
    Publishable,
    PublicationStarted,
    PublicationFinished,
    PublicationAdopted,
    TransitionStarted,
    TransitionCompleted,
    Drawn,
    Superseded,
    DiscardedStale,
    Evicted,
}

impl Stage {
    const ALL: [Self; 26] = [
        Self::Requested,
        Self::Desired,
        Self::GenerationQueued,
        Self::GenerationStarted,
        Self::GenerationCompleted,
        Self::GenerationCancelled,
        Self::GenerationFailed,
        Self::StaleCompletion,
        Self::CpuEvicted,
        Self::Resident,
        Self::Drawable,
        Self::BoundaryQueued,
        Self::BoundaryStarted,
        Self::BoundaryFinished,
        Self::BoundaryFailed,
        Self::FullyPrepared,
        Self::Publishable,
        Self::PublicationStarted,
        Self::PublicationFinished,
        Self::PublicationAdopted,
        Self::TransitionStarted,
        Self::TransitionCompleted,
        Self::Drawn,
        Self::Superseded,
        Self::DiscardedStale,
        Self::Evicted,
    ];
    const COUNT: usize = Self::ALL.len();

    const fn index(self) -> usize {
        self as usize
    }
}

/// A typed reason that a tile or local publication is waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    WorkerQueueFull,
    CpuCapacity,
    UploadCapacity,
    PublicationBudget,
    MissingDependency,
    NeighborConflict,
    TransitionCapacity,
    NotDesired,
    NotResident,
    ActiveSplitOverlap,
    ActiveMergeOverlap,
    ParentDependency,
    ChildDependency,
    NeighborBoundaryDependency,
    ResidentSlotUnavailable,
    GpuResourcePressure,
    RevisionMismatch,
    StaleResult,
    PublicationBudgetExhausted,
    BoundaryWorkerBusy,
}

impl BlockReason {
    const ALL: [Self; 20] = [
        Self::WorkerQueueFull,
        Self::CpuCapacity,
        Self::UploadCapacity,
        Self::PublicationBudget,
        Self::MissingDependency,
        Self::NeighborConflict,
        Self::TransitionCapacity,
        Self::NotDesired,
        Self::NotResident,
        Self::ActiveSplitOverlap,
        Self::ActiveMergeOverlap,
        Self::ParentDependency,
        Self::ChildDependency,
        Self::NeighborBoundaryDependency,
        Self::ResidentSlotUnavailable,
        Self::GpuResourcePressure,
        Self::RevisionMismatch,
        Self::StaleResult,
        Self::PublicationBudgetExhausted,
        Self::BoundaryWorkerBusy,
    ];
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTraceEvent {
    pub time_us: u64,
    pub address: TraceAddress,
    pub stage: Option<Stage>,
    pub block: Option<BlockReason>,
    pub unblocked: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTraceJob {
    pub address: TraceAddress,
    pub body_identity: u64,
    pub definition_words: Vec<u64>,
    pub radius_bits: u64,
    pub surface_revision: u64,
    pub material_revision: u64,
    pub format_version: u32,
    pub filter_version: u32,
    pub cells: u32,
    /// First profiled request frame for the current generation cycle.
    pub origin_frame_id: Option<u64>,
    /// Bounded exact-key links to individual worker admissions/retries.
    pub profile_jobs: Vec<ProfileJobLink>,
    pub profile_jobs_omitted: u64,
    pub blocker_intervals: Vec<TraceBlockInterval>,
    pub blocker_intervals_omitted: u64,
    pub prepared_to_drawable_us: Option<u64>,
    pub requested_us: Option<u64>,
    pub milestones: Vec<TerrainTraceMilestone>,
    pub blocked_by: Vec<BlockReason>,
    pub desired: bool,
    pub resident: bool,
    pub drawable: bool,
    pub state: TerrainTraceJobState,
    pub latest_stage: Option<Stage>,
    pub age_ms: u64,
    pub requested_to_generation_start_us: Option<u64>,
    pub generation_elapsed_us: Option<u64>,
    pub generation_to_boundary_start_us: Option<u64>,
    pub boundary_elapsed_us: Option<u64>,
    pub boundary_to_publishable_us: Option<u64>,
    pub publication_wait_us: Option<u64>,
    pub publication_elapsed_us: Option<u64>,
    pub requested_to_drawable_us: Option<u64>,
    pub requested_to_transition_completed_us: Option<u64>,
    /// Sum of the currently open per-reason blocker intervals. Concurrent
    /// reasons overlap and are counted independently.
    pub active_blocked_elapsed_us: u64,
    /// Closed plus active per-reason blocker intervals for this exact tile key.
    pub total_blocked_elapsed_us: u64,
    pub parent: Option<TraceAddress>,
    pub children: Vec<TraceAddress>,
    pub neighbors: Vec<TraceAddress>,
}

/// Per-exact-key recorded blockers, bounded independently of the recent event ring.
#[derive(Debug, Clone, Serialize)]
pub struct TraceBlockInterval {
    pub reason: BlockReason,
    pub start_us: u64,
    pub end_us: u64,
    pub open: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ProfileJobLink {
    pub job_id: u64,
    pub capture_id: u64,
    pub origin_frame_id: u64,
    pub dispatch_frame_id: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainTraceJobState {
    Desired,
    GenerationQueued,
    GenerationRunning,
    GeneratedAwaitingUpload,
    ParkedCache,
    GeneratedAwaitingBoundary,
    BoundaryQueued,
    BoundaryRunning,
    Prepared,
    Publishable,
    PublicationRunning,
    PublicationFinished,
    PublicationBlocked,
    GenerationBlocked,
    UploadBlocked,
    BoundaryFailed,
    Superseded,
    Evicted,
    Retired,
    Resident,
    Drawable,
    Cancelled,
    Stale,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTraceMilestone {
    pub stage: Stage,
    pub time_us: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct TraceAddress {
    pub face: u8,
    pub level: u8,
    pub x: u32,
    pub y: u32,
}

impl From<CubePatchAddress> for TraceAddress {
    fn from(address: CubePatchAddress) -> Self {
        let face = match address.face() {
            mundaris_math::surface::CubeFace::PositiveX => 0,
            mundaris_math::surface::CubeFace::NegativeX => 1,
            mundaris_math::surface::CubeFace::PositiveY => 2,
            mundaris_math::surface::CubeFace::NegativeY => 3,
            mundaris_math::surface::CubeFace::PositiveZ => 4,
            mundaris_math::surface::CubeFace::NegativeZ => 5,
        };
        let [x, y] = address.coordinates();
        Self {
            face,
            level: address.level(),
            x,
            y,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct TerrainTraceSnapshot {
    pub schema_version: u32,
    pub enabled: bool,
    /// Monotonic microseconds since this terrain trace's creation.
    pub generated_at_us: u64,
    /// Engine frame observed while this snapshot was sampled, independent of
    /// the frame that requested an asynchronous capture.
    pub snapshot_frame_id: u64,
    pub jobs: Vec<TerrainTraceJob>,
    pub events: Vec<TerrainTraceEvent>,
    pub snapshot_jobs_omitted: usize,
    pub snapshot_events_omitted: usize,
    pub tracked_jobs: usize,
    pub requested: usize,
    pub generation_queued: usize,
    pub generation_running: usize,
    pub blocked: usize,
    pub completed: u64,
    pub completed_rate_per_second: f64,
    /// Validity for `completed_rate_per_second` under bounded event retention.
    pub completed_rate_window_complete: bool,
    pub oldest_pending_us: Option<u64>,
    pub generation_p50_us: Option<u64>,
    pub generation_p95_us: Option<u64>,
    pub dropped_jobs: u64,
    pub dropped_events: u64,
    pub retry_count: u64,
    pub queues: Vec<TerrainTraceQueueSnapshot>,
    pub block_reasons: Vec<TerrainTraceBlockSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainTraceQueue {
    GenerationQueued,
    GenerationBlocked,
    GenerationActive,
    GeneratedAwaitingUpload,
    ParkedCache,
    GeneratedAwaitingBoundary,
    UploadBlocked,
    BoundaryQueued,
    BoundaryActive,
    Prepared,
    Publishable,
    PublicationActive,
    PublicationBlocked,
    ResidentAwaitingDrawable,
    /// Terminal or otherwise not assignable to a live pipeline queue.
    Unattributed,
}

impl TerrainTraceQueue {
    const ALL: [Self; 15] = [
        Self::GenerationQueued,
        Self::GenerationBlocked,
        Self::GenerationActive,
        Self::GeneratedAwaitingUpload,
        Self::ParkedCache,
        Self::GeneratedAwaitingBoundary,
        Self::UploadBlocked,
        Self::BoundaryQueued,
        Self::BoundaryActive,
        Self::Prepared,
        Self::Publishable,
        Self::PublicationActive,
        Self::PublicationBlocked,
        Self::ResidentAwaitingDrawable,
        Self::Unattributed,
    ];

    const COUNT: usize = Self::ALL.len();

    const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTraceQueueSnapshot {
    pub queue: TerrainTraceQueue,
    /// Pipeline domain represented by this row, including whether cache rows
    /// are active work or parked resources.
    pub queue_scope: &'static str,
    pub count: usize,
    pub oldest_age_ms: Option<u64>,
    pub p50_age_ms: Option<u64>,
    pub p95_age_ms: Option<u64>,
    pub completed_per_second: f64,
    /// False when event-ring eviction may have removed events inside the rate window.
    pub rate_window_complete: bool,
    /// Jobs evicted from the bounded job table while they were in this queue.
    pub dropped: u64,
    pub cancelled: u64,
    pub superseded: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainTraceBlockSnapshot {
    pub reason: BlockReason,
    pub count: u64,
    pub active_count: usize,
    pub oldest_age_us: Option<u64>,
    pub p95_block_us: Option<u64>,
    pub total_block_time_us: u64,
    pub rechecks: u64,
}

#[derive(Clone)]
struct Job {
    key: TileKey,
    milestones: [Option<u64>; Stage::COUNT],
    requested_us: Option<u64>,
    blocked_since: Vec<(BlockReason, u64)>,
    desired: bool,
    resident: bool,
    drawable: bool,
    upload_candidate: bool,
    upload_candidate_since_us: Option<u64>,
    generation_cancelled_queue: Option<TerrainTraceQueue>,
    superseded_queue: Option<TerrainTraceQueue>,
    total_block_time_us: u64,
    blocker_intervals: VecDeque<TraceBlockInterval>,
    blocker_intervals_omitted: u64,
}

#[derive(Default)]
struct ReasonTotals {
    count: u64,
    rechecks: u64,
    total_block_time_us: u64,
    completed_samples: VecDeque<u64>,
}

struct Inner {
    jobs: HashMap<TileKey, Job>,
    profile_origins: HashMap<TileKey, u64>,
    profile_jobs: HashMap<TileKey, VecDeque<ProfileJobLink>>,
    profile_jobs_omitted: HashMap<TileKey, u64>,
    order: VecDeque<TileKey>,
    current_by_address: HashMap<CubePatchAddress, TileKey>,
    generation_candidates: HashMap<CubePatchAddress, (TileKey, u64)>,
    generation_candidate_epoch: u64,
    upload_candidates: HashMap<CubePatchAddress, (TileKey, u64)>,
    upload_candidate_epoch: u64,
    events: VecDeque<TerrainTraceEvent>,
    reason_totals: HashMap<BlockReason, ReasonTotals>,
    dropped_jobs: u64,
    dropped_jobs_by_queue: [u64; TerrainTraceQueue::COUNT],
    dropped_events: u64,
    retry_count: u64,
}

/// Shared trace attached to a `RegionalTerrain` and its generation workers.
pub struct TerrainTrace {
    origin: Instant,
    inner: Mutex<Inner>,
}

impl Default for TerrainTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl TerrainTrace {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            inner: Mutex::new(Inner {
                jobs: HashMap::new(),
                profile_origins: HashMap::new(),
                profile_jobs: HashMap::new(),
                profile_jobs_omitted: HashMap::new(),
                order: VecDeque::new(),
                current_by_address: HashMap::new(),
                generation_candidates: HashMap::new(),
                generation_candidate_epoch: 0,
                upload_candidates: HashMap::new(),
                upload_candidate_epoch: 0,
                events: VecDeque::new(),
                reason_totals: HashMap::new(),
                dropped_jobs: 0,
                dropped_jobs_by_queue: [0; TerrainTraceQueue::COUNT],
                dropped_events: 0,
                retry_count: 0,
            }),
        }
    }

    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }

    /// Begin tracking the exact immutable tile key before queue admission.
    pub fn admit(&self, key: &TileKey) -> Option<u64> {
        if !crate::engine_profile::is_enabled() {
            return None;
        }
        let now = self.now_us();
        let current_frame = crate::engine_profile::current_frame_id();
        let mut inner = self.lock();
        if inner.jobs.contains_key(key) {
            let (first_request, completed_blocks, origin_reset) = {
                let existing = inner.jobs.get_mut(key).expect("job checked above");
                let generation_finished = existing.milestones[Stage::GenerationCompleted.index()]
                    .or(existing.milestones[Stage::GenerationCancelled.index()])
                    .or(existing.milestones[Stage::GenerationFailed.index()])
                    .is_some();
                let completed_blocks = if generation_finished {
                    let completed_blocks = std::mem::take(&mut existing.blocked_since);
                    reset_generation_cycle(existing, now);
                    completed_blocks
                } else {
                    Vec::new()
                };
                let requested = &mut existing.milestones[Stage::Requested.index()];
                let first_request = requested.is_none();
                if first_request {
                    *requested = Some(now);
                    existing.requested_us = Some(now);
                }
                (
                    first_request,
                    completed_blocks,
                    generation_finished || first_request,
                )
            };
            if origin_reset {
                inner.profile_origins.insert(key.clone(), current_frame);
            }
            for (reason, started) in completed_blocks {
                Self::record_unblocked_for_job(&mut inner, key, now, reason, started);
            }
            inner.retry_count = inner.retry_count.saturating_add(1);
            if first_request {
                Self::push_event(
                    &mut inner,
                    TerrainTraceEvent {
                        time_us: now,
                        address: key.address.into(),
                        stage: Some(Stage::Requested),
                        block: None,
                        unblocked: false,
                    },
                );
            }
            return inner.profile_origins.get(key).copied();
        }
        while inner.jobs.len() >= MAX_JOBS {
            if !evict_oldest_job(&mut inner, now) {
                break;
            }
        }
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Requested.index()] = Some(now);
        inner.order.push_back(key.clone());
        inner.current_by_address.insert(key.address, key.clone());
        inner.profile_origins.insert(key.clone(), current_frame);
        inner.jobs.insert(
            key.clone(),
            Job {
                key: key.clone(),
                milestones,
                requested_us: Some(now),
                blocked_since: Vec::new(),
                desired: false,
                resident: false,
                drawable: false,
                upload_candidate: false,
                upload_candidate_since_us: None,
                generation_cancelled_queue: None,
                superseded_queue: None,
                total_block_time_us: 0,
                blocker_intervals: VecDeque::new(),
                blocker_intervals_omitted: 0,
            },
        );
        Self::push_event(
            &mut inner,
            TerrainTraceEvent {
                time_us: now,
                address: key.address.into(),
                stage: Some(Stage::Requested),
                block: None,
                unblocked: false,
            },
        );
        Some(current_frame)
    }

    pub fn profile_job_dispatched(
        &self,
        key: &TileKey,
        job_id: u64,
        origin_frame_id: u64,
        dispatch_frame_id: u64,
    ) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let mut inner = self.lock();
        if inner.jobs.contains_key(key) {
            let omitted = {
                let links = inner.profile_jobs.entry(key.clone()).or_default();
                if links.iter().any(|link| link.job_id == job_id) {
                    return;
                }
                let omitted = links.len() == 8;
                if omitted {
                    links.pop_front();
                }
                links.push_back(ProfileJobLink {
                    job_id,
                    capture_id: crate::engine_profile::capture_id(),
                    origin_frame_id,
                    dispatch_frame_id,
                });
                omitted
            };
            if omitted {
                let count = inner.profile_jobs_omitted.entry(key.clone()).or_default();
                *count = count.saturating_add(1);
            }
        }
    }

    pub fn latest_profile_job_ids<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a TileKey>,
    ) -> Vec<u64> {
        if !crate::engine_profile::is_enabled() {
            return Vec::new();
        }
        let inner = self.lock();
        keys.into_iter()
            .filter_map(|key| inner.profile_jobs.get(key)?.back())
            .filter(|link| link.capture_id == crate::engine_profile::capture_id())
            .map(|link| link.job_id)
            .collect()
    }

    /// Synchronize the exact keys still eligible for CPU generation admission.
    /// A generation-capacity block stops being active when its key leaves this
    /// frontier, even if that key never receives a later successful admission.
    pub fn sync_generation_candidates<'a>(&self, keys: impl IntoIterator<Item = &'a TileKey>) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        let epoch = inner.generation_candidate_epoch.wrapping_add(1).max(1);
        inner.generation_candidate_epoch = epoch;
        let mut retired = Vec::new();
        for key in keys {
            let address = key.address;
            if let Some((tracked, seen_epoch)) = inner.generation_candidates.get_mut(&address) {
                if tracked == key {
                    *seen_epoch = epoch;
                } else {
                    retired.push(std::mem::replace(tracked, key.clone()));
                    *seen_epoch = epoch;
                }
            } else if inner.generation_candidates.len() < MAX_JOBS {
                inner
                    .generation_candidates
                    .insert(address, (key.clone(), epoch));
            }
        }
        inner.generation_candidates.retain(|_, (key, seen_epoch)| {
            if *seen_epoch == epoch {
                true
            } else {
                retired.push(key.clone());
                false
            }
        });
        close_candidate_blocks(&mut inner, retired, now, is_generation_blocker);
    }

    /// Synchronize the exact CPU tile keys that currently have an upload
    /// candidate or queued upload. Completed but unused cache entries remain
    /// explicitly parked instead of looking like active GPU-upload work.
    pub fn sync_upload_candidates<'a>(&self, keys: impl IntoIterator<Item = &'a TileKey>) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        let epoch = inner.upload_candidate_epoch.wrapping_add(1).max(1);
        inner.upload_candidate_epoch = epoch;
        let mut retired = Vec::new();
        for key in keys {
            let address = key.address;
            let tracked_candidate =
                if let Some((tracked, seen_epoch)) = inner.upload_candidates.get_mut(&address) {
                    if tracked == key {
                        *seen_epoch = epoch;
                    } else {
                        retired.push(std::mem::replace(tracked, key.clone()));
                        *seen_epoch = epoch;
                    }
                    true
                } else if inner.upload_candidates.len() < MAX_JOBS {
                    inner
                        .upload_candidates
                        .insert(address, (key.clone(), epoch));
                    true
                } else {
                    false
                };
            if tracked_candidate && let Some(job) = inner.jobs.get_mut(key) {
                job.upload_candidate = true;
                job.upload_candidate_since_us.get_or_insert(now);
            }
        }
        inner.upload_candidates.retain(|_, (key, seen_epoch)| {
            if *seen_epoch == epoch {
                true
            } else {
                retired.push(key.clone());
                false
            }
        });
        for key in &retired {
            if let Some(job) = inner.jobs.get_mut(key) {
                job.upload_candidate = false;
                job.upload_candidate_since_us = None;
            }
        }
        close_candidate_blocks(&mut inner, retired, now, is_upload_blocker);
    }

    /// Record a milestone for the current exact key associated with an address.
    /// The core should use `event_key` whenever it already has the immutable key.
    pub fn event(&self, address: CubePatchAddress, stage: Stage) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        let Some(key) = inner.current_by_address.get(&address).cloned() else {
            Self::push_event(
                &mut inner,
                TerrainTraceEvent {
                    time_us: now,
                    address: address.into(),
                    stage: Some(stage),
                    block: None,
                    unblocked: false,
                },
            );
            return;
        };
        Self::event_locked(&mut inner, &key, now, stage);
    }

    pub fn event_key(&self, key: &TileKey, stage: Stage) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        if !inner.jobs.contains_key(key) {
            Self::insert_minimal_job(&mut inner, key, now);
        }
        Self::event_locked(&mut inner, key, now, stage);
    }

    pub fn set_desired(&self, key: &TileKey, desired: bool) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        if desired {
            self.event_key(key, Stage::Desired);
        }
        let mut inner = self.lock();
        if desired && !inner.jobs.contains_key(key) {
            Self::insert_minimal_job(&mut inner, key, self.now_us());
        }
        if let Some(job) = inner.jobs.get_mut(key) {
            job.desired = desired;
        }
    }

    pub fn clear_desired(&self, address: CubePatchAddress) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let mut inner = self.lock();
        let key = inner.current_by_address.get(&address).cloned();
        if let Some(key) = key
            && let Some(job) = inner.jobs.get_mut(&key)
        {
            job.desired = false;
        }
    }

    pub fn set_resident(&self, address: CubePatchAddress, resident: bool) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        if resident {
            self.event(address, Stage::Resident);
        }
        let mut inner = self.lock();
        let key = inner.current_by_address.get(&address).cloned();
        if let Some(key) = key
            && let Some(job) = inner.jobs.get_mut(&key)
        {
            job.resident = resident;
        }
    }

    pub fn set_drawable(&self, address: CubePatchAddress, drawable: bool) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        if drawable {
            self.event(address, Stage::Drawable);
        }
        let mut inner = self.lock();
        let key = inner.current_by_address.get(&address).cloned();
        if let Some(key) = key
            && let Some(job) = inner.jobs.get_mut(&key)
        {
            job.drawable = drawable;
        }
    }

    pub fn block(&self, address: CubePatchAddress, reason: BlockReason) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        let key = inner.current_by_address.get(&address).cloned();
        let changed = key
            .as_ref()
            .and_then(|key| inner.jobs.get_mut(key))
            .is_some_and(|job| {
                if job
                    .blocked_since
                    .iter()
                    .any(|(active, _)| *active == reason)
                {
                    false
                } else {
                    job.blocked_since.push((reason, now));
                    true
                }
            });
        if changed {
            let totals = inner.reason_totals.entry(reason).or_default();
            totals.count = totals.count.saturating_add(1);
            Self::push_event(
                &mut inner,
                TerrainTraceEvent {
                    time_us: now,
                    address: address.into(),
                    stage: None,
                    block: Some(reason),
                    unblocked: false,
                },
            );
        } else if key.is_some() {
            inner.retry_count = inner.retry_count.saturating_add(1);
            let totals = inner.reason_totals.entry(reason).or_default();
            totals.rechecks = totals.rechecks.saturating_add(1);
        }
    }

    pub fn unblock(&self, address: CubePatchAddress) {
        if !crate::engine_profile::is_enabled() {
            return;
        }
        let now = self.now_us();
        let mut inner = self.lock();
        let key = inner.current_by_address.get(&address).cloned();
        let removed = key
            .as_ref()
            .and_then(|key| inner.jobs.get_mut(key))
            .map(|job| std::mem::take(&mut job.blocked_since))
            .unwrap_or_default();
        for (reason, started) in removed {
            if let Some(key) = key.as_ref() {
                Self::record_unblocked_for_job(&mut inner, key, now, reason, started);
            } else {
                Self::record_unblocked(&mut inner, address, now, reason, started);
            }
        }
    }

    pub fn snapshot(&self) -> TerrainTraceSnapshot {
        self.snapshot_for_profile_jobs(&[])
    }

    pub fn snapshot_for_profile_jobs(&self, preferred_job_ids: &[u64]) -> TerrainTraceSnapshot {
        if !crate::engine_profile::is_enabled() {
            return TerrainTraceSnapshot {
                schema_version: 1,
                ..TerrainTraceSnapshot::default()
            };
        }
        let now = self.now_us();
        let snapshot_frame_id = crate::engine_profile::current_frame_id();
        let (
            tracked_jobs,
            requested,
            queued,
            running,
            blocked,
            oldest_pending_us,
            completed,
            completed_in_window,
            completed_rate_window_complete,
            dropped_jobs,
            dropped_events,
            retry_count,
            mut generation_times,
            queue_accumulators,
            block_accumulators,
            row_jobs,
            events,
        ) = {
            let inner = self.lock();
            let mut generation_times = Vec::new();
            let mut requested = 0usize;
            let mut queued = 0usize;
            let mut running = 0usize;
            let mut blocked = 0usize;
            let mut oldest_pending_us = None;
            let mut completed = 0u64;
            for job in inner.jobs.values() {
                let started = job.milestones[Stage::GenerationStarted.index()];
                let ended = job.milestones[Stage::GenerationCompleted.index()];
                let cancelled = job.milestones[Stage::GenerationCancelled.index()];
                let failed = job.milestones[Stage::GenerationFailed.index()].is_some();
                let is_finished = ended.is_some() || cancelled.is_some() || failed;
                requested += usize::from(job.requested_us.is_some());
                queued += usize::from(
                    job.milestones[Stage::GenerationQueued.index()].is_some()
                        && started.is_none()
                        && !is_finished,
                );
                running += usize::from(started.is_some() && !is_finished);
                blocked += usize::from(!job.blocked_since.is_empty());
                if let (Some(start), Some(end)) = (started, ended) {
                    generation_times.push(end.saturating_sub(start));
                    completed += 1;
                }
                if !is_finished
                    && let Some(started) =
                        job.requested_us.or(job.milestones[Stage::Desired.index()])
                {
                    let age = now.saturating_sub(started);
                    oldest_pending_us =
                        Some(oldest_pending_us.map_or(age, |old: u64| old.max(age)));
                }
            }
            let completed_in_window = inner
                .events
                .iter()
                .filter(|event| {
                    event.stage == Some(Stage::GenerationCompleted)
                        && event.time_us.saturating_add(RATE_WINDOW_US) >= now
                })
                .count() as u64;
            let completed_rate_window_complete = rate_window_complete(&inner, now);
            let mut selected_keys = Vec::with_capacity(SNAPSHOT_JOBS);
            let preferred = preferred_job_ids
                .iter()
                .take(64)
                .copied()
                .collect::<HashSet<_>>();
            for (key, links) in &inner.profile_jobs {
                if selected_keys.len() == SNAPSHOT_JOBS {
                    break;
                }
                if links.iter().any(|link| {
                    preferred.contains(&link.job_id)
                        && link.capture_id == crate::engine_profile::capture_id()
                }) && inner.jobs.contains_key(key)
                {
                    selected_keys.push(key.clone());
                }
            }
            let mut selected_set = selected_keys.iter().cloned().collect::<HashSet<_>>();
            for key in snapshot_job_keys(&inner.order, &inner.jobs) {
                if selected_keys.len() == SNAPSHOT_JOBS {
                    break;
                }
                if selected_set.insert(key.clone()) {
                    selected_keys.push(key);
                }
            }
            let row_jobs = selected_keys
                .iter()
                .filter_map(|key| {
                    Some((
                        inner.jobs.get(key)?.clone(),
                        inner.profile_origins.get(key).copied(),
                        inner
                            .profile_jobs
                            .get(key)
                            .map(|links| links.iter().copied().collect::<Vec<_>>())
                            .unwrap_or_default(),
                        inner.profile_jobs_omitted.get(key).copied().unwrap_or(0),
                    ))
                })
                .collect::<Vec<_>>();
            let events = inner
                .events
                .iter()
                .rev()
                .take(SNAPSHOT_EVENTS)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            (
                inner.jobs.len(),
                requested,
                queued,
                running,
                blocked,
                oldest_pending_us,
                completed,
                completed_in_window,
                completed_rate_window_complete,
                inner.dropped_jobs,
                inner.dropped_events,
                inner.retry_count,
                generation_times,
                queue_accumulators(&inner, now),
                block_accumulators(&inner, now),
                row_jobs,
                events,
            )
        };
        generation_times.sort_unstable();
        let queues =
            finish_queue_snapshots(queue_accumulators, now, completed_rate_window_complete);
        let block_reasons = finish_block_snapshots(block_accumulators);
        let jobs = row_jobs
            .iter()
            .map(|(job, origin, profile_jobs, omitted)| {
                snapshot_job_with_profile(job, now, *origin, profile_jobs, *omitted)
            })
            .collect();
        let span_us = now.clamp(1, RATE_WINDOW_US);
        TerrainTraceSnapshot {
            schema_version: 1,
            enabled: true,
            generated_at_us: now,
            snapshot_frame_id,
            snapshot_jobs_omitted: tracked_jobs.saturating_sub(row_jobs.len()),
            snapshot_events_omitted: {
                let inner = self.lock();
                inner.events.len().saturating_sub(SNAPSHOT_EVENTS)
            },
            jobs,
            events,
            tracked_jobs,
            requested,
            generation_queued: queued,
            generation_running: running,
            blocked,
            completed,
            completed_rate_per_second: completed_in_window as f64 * 1_000_000.0 / span_us as f64,
            completed_rate_window_complete,
            oldest_pending_us,
            generation_p50_us: percentile(&generation_times, 50),
            generation_p95_us: percentile(&generation_times, 95),
            dropped_jobs,
            dropped_events,
            retry_count,
            queues,
            block_reasons,
        }
    }

    fn event_locked(inner: &mut Inner, key: &TileKey, now: u64, stage: Stage) {
        let mut inserted = false;
        let mut completed_blocks = Vec::new();
        if let Some(job) = inner.jobs.get_mut(key) {
            let queue_before_stage =
                if matches!(stage, Stage::GenerationCancelled | Stage::Superseded) {
                    Some(
                        queue_for_job(job)
                            .map_or(TerrainTraceQueue::Unattributed, |(queue, _, _)| queue),
                    )
                } else {
                    None
                };
            if stage == Stage::BoundaryQueued {
                reset_boundary_cycle(job);
            }
            let milestone = &mut job.milestones[stage.index()];
            if milestone.is_none() {
                *milestone = Some(now);
                inserted = true;
                match stage {
                    Stage::GenerationCancelled => {
                        job.generation_cancelled_queue = queue_before_stage;
                    }
                    Stage::Superseded => job.superseded_queue = queue_before_stage,
                    _ => {}
                }
            }
            match stage {
                Stage::Desired => job.desired = true,
                Stage::Resident => job.resident = true,
                Stage::Drawable => job.drawable = true,
                Stage::CpuEvicted => job.resident = false,
                Stage::Evicted => {
                    job.resident = false;
                    job.drawable = false;
                }
                _ => {}
            }
            if stage_ends_pipeline(stage) {
                completed_blocks = std::mem::take(&mut job.blocked_since);
            }
        }
        for (reason, started) in completed_blocks {
            Self::record_unblocked_for_job(inner, key, now, reason, started);
        }
        if inserted {
            Self::push_event(
                inner,
                TerrainTraceEvent {
                    time_us: now,
                    address: key.address.into(),
                    stage: Some(stage),
                    block: None,
                    unblocked: false,
                },
            );
        }
    }

    fn record_unblocked(
        inner: &mut Inner,
        address: CubePatchAddress,
        now: u64,
        reason: BlockReason,
        started: u64,
    ) {
        let elapsed = now.saturating_sub(started);
        let totals = inner.reason_totals.entry(reason).or_default();
        totals.total_block_time_us = totals.total_block_time_us.saturating_add(elapsed);
        if totals.completed_samples.len() >= 256 {
            totals.completed_samples.pop_front();
        }
        totals.completed_samples.push_back(elapsed);
        Self::push_event(
            inner,
            TerrainTraceEvent {
                time_us: now,
                address: address.into(),
                stage: None,
                block: Some(reason),
                unblocked: true,
            },
        );
    }

    fn record_unblocked_for_job(
        inner: &mut Inner,
        key: &TileKey,
        now: u64,
        reason: BlockReason,
        started: u64,
    ) {
        let elapsed = now.saturating_sub(started);
        if let Some(job) = inner.jobs.get_mut(key) {
            job.total_block_time_us = job.total_block_time_us.saturating_add(elapsed);
            if job.blocker_intervals.len() == 32 {
                job.blocker_intervals.pop_front();
                job.blocker_intervals_omitted = job.blocker_intervals_omitted.saturating_add(1);
            }
            job.blocker_intervals.push_back(TraceBlockInterval {
                reason,
                start_us: started,
                end_us: now,
                open: false,
            });
        }
        Self::record_unblocked(inner, key.address, now, reason, started);
    }

    fn insert_minimal_job(inner: &mut Inner, key: &TileKey, now: u64) {
        if inner.jobs.len() >= MAX_JOBS {
            evict_oldest_job(inner, now);
        }
        let milestones = [None; Stage::COUNT];
        inner.order.push_back(key.clone());
        inner.current_by_address.insert(key.address, key.clone());
        inner.jobs.insert(
            key.clone(),
            Job {
                key: key.clone(),
                milestones,
                requested_us: None,
                blocked_since: Vec::new(),
                desired: false,
                resident: false,
                drawable: false,
                upload_candidate: false,
                upload_candidate_since_us: None,
                generation_cancelled_queue: None,
                superseded_queue: None,
                total_block_time_us: 0,
                blocker_intervals: VecDeque::new(),
                blocker_intervals_omitted: 0,
            },
        );
    }

    fn push_event(inner: &mut Inner, event: TerrainTraceEvent) {
        if inner.events.len() >= MAX_EVENTS {
            inner.events.pop_front();
            inner.dropped_events = inner.dropped_events.saturating_add(1);
        }
        inner.events.push_back(event);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        match self.inner.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                let contention = crate::engine_profile::span("Terrain trace contention");
                let guard = self
                    .inner
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                drop(contention);
                guard
            }
        }
    }
}

fn evict_oldest_job(inner: &mut Inner, now: u64) -> bool {
    let Some(oldest) = inner.order.pop_front() else {
        return false;
    };
    if let Some(job) = inner.jobs.remove(&oldest) {
        inner.profile_origins.remove(&oldest);
        inner.profile_jobs.remove(&oldest);
        inner.profile_jobs_omitted.remove(&oldest);
        let queue =
            queue_for_job(&job).map_or(TerrainTraceQueue::Unattributed, |(queue, _, _)| queue);
        inner.dropped_jobs_by_queue[queue.index()] =
            inner.dropped_jobs_by_queue[queue.index()].saturating_add(1);
        for (reason, started) in job.blocked_since {
            let duration = now.saturating_sub(started);
            let totals = inner.reason_totals.entry(reason).or_default();
            totals.total_block_time_us = totals.total_block_time_us.saturating_add(duration);
            if totals.completed_samples.len() >= 256 {
                totals.completed_samples.pop_front();
            }
            totals.completed_samples.push_back(duration);
        }
        if inner.current_by_address.get(&oldest.address) == Some(&oldest) {
            inner.current_by_address.remove(&oldest.address);
        }
        inner.dropped_jobs = inner.dropped_jobs.saturating_add(1);
    }
    true
}

fn close_candidate_blocks(
    inner: &mut Inner,
    keys: Vec<TileKey>,
    now: u64,
    is_candidate_block: fn(BlockReason) -> bool,
) {
    let mut closed = Vec::new();
    for key in keys {
        if let Some(job) = inner.jobs.get_mut(&key) {
            job.blocked_since.retain(|(reason, started)| {
                if is_candidate_block(*reason) {
                    closed.push((key.clone(), *reason, *started));
                    false
                } else {
                    true
                }
            });
        }
    }
    for (key, reason, started) in closed {
        TerrainTrace::record_unblocked_for_job(inner, &key, now, reason, started);
    }
}

fn reset_boundary_cycle(job: &mut Job) {
    for stage in [
        Stage::BoundaryQueued,
        Stage::BoundaryStarted,
        Stage::BoundaryFinished,
        Stage::BoundaryFailed,
        Stage::FullyPrepared,
        Stage::Publishable,
        Stage::PublicationStarted,
        Stage::PublicationFinished,
        Stage::PublicationAdopted,
        Stage::TransitionStarted,
        Stage::TransitionCompleted,
        Stage::Drawn,
        Stage::Superseded,
        Stage::DiscardedStale,
    ] {
        job.milestones[stage.index()] = None;
    }
    job.superseded_queue = None;
}

fn reset_generation_cycle(job: &mut Job, now: u64) {
    job.requested_us = None;
    job.blocked_since.clear();
    for stage in [
        Stage::Requested,
        Stage::GenerationQueued,
        Stage::GenerationStarted,
        Stage::GenerationCompleted,
        Stage::GenerationCancelled,
        Stage::GenerationFailed,
        Stage::StaleCompletion,
        Stage::Resident,
        Stage::Drawable,
        Stage::CpuEvicted,
        Stage::Evicted,
        Stage::BoundaryQueued,
        Stage::BoundaryStarted,
        Stage::BoundaryFinished,
        Stage::BoundaryFailed,
        Stage::FullyPrepared,
        Stage::Publishable,
        Stage::PublicationStarted,
        Stage::PublicationFinished,
        Stage::PublicationAdopted,
        Stage::TransitionStarted,
        Stage::TransitionCompleted,
    ] {
        job.milestones[stage.index()] = None;
    }
    job.generation_cancelled_queue = None;
    // Residency/drawability are current resource state, independent of a new
    // CPU generation retry. Re-seed their milestones after clearing the old
    // cycle so snapshots remain consistent with the live flags.
    if job.resident {
        job.milestones[Stage::Resident.index()] = Some(now);
    }
    if job.drawable {
        job.milestones[Stage::Drawable.index()] = Some(now);
    }
}

fn stage_ends_pipeline(stage: Stage) -> bool {
    matches!(
        stage,
        Stage::GenerationCancelled
            | Stage::GenerationFailed
            | Stage::BoundaryFailed
            | Stage::StaleCompletion
            | Stage::CpuEvicted
            | Stage::Evicted
            | Stage::Drawable
            | Stage::Drawn
            | Stage::TransitionCompleted
            | Stage::Superseded
            | Stage::DiscardedStale
    )
}

fn percentile(values: &[u64], percentile: usize) -> Option<u64> {
    values
        .len()
        .checked_sub(1)
        .map(|last| values[((last + 1) * percentile).div_ceil(100) - 1])
}

fn rate_window_complete(inner: &Inner, now: u64) -> bool {
    inner.dropped_events == 0
        || inner
            .events
            .front()
            .is_some_and(|oldest| oldest.time_us <= now.saturating_sub(RATE_WINDOW_US))
}

#[derive(Default)]
struct QueueAccumulator {
    ages_ms: Vec<u64>,
    completed: u64,
    dropped: u64,
    cancelled: u64,
    superseded: u64,
}

#[derive(Default)]
struct BlockAccumulator {
    active_ages_us: Vec<u64>,
    completed_samples_us: Vec<u64>,
    count: u64,
    rechecks: u64,
    completed_total_us: u64,
}

fn queue_accumulators(inner: &Inner, now: u64) -> HashMap<TerrainTraceQueue, QueueAccumulator> {
    let mut accumulators: HashMap<TerrainTraceQueue, QueueAccumulator> = TerrainTraceQueue::ALL
        .into_iter()
        .map(|queue| (queue, QueueAccumulator::default()))
        .collect();
    for queue in TerrainTraceQueue::ALL {
        accumulators
            .get_mut(&queue)
            .expect("all queues are registered")
            .dropped = inner.dropped_jobs_by_queue[queue.index()];
    }
    for job in inner.jobs.values() {
        if let Some(queue) = job.generation_cancelled_queue {
            let accumulator = accumulators
                .get_mut(&queue)
                .expect("all queues are registered");
            accumulator.cancelled = accumulator.cancelled.saturating_add(1);
        }
        if let Some(queue) = job.superseded_queue {
            let accumulator = accumulators
                .get_mut(&queue)
                .expect("all queues are registered");
            accumulator.superseded = accumulator.superseded.saturating_add(1);
        }
        if let Some((queue, started_at, _)) = queue_for_job(job) {
            let accumulator = accumulators
                .get_mut(&queue)
                .expect("all queues are registered");
            accumulator
                .ages_ms
                .push(now.saturating_sub(started_at) / 1000);
        }
    }
    for event in &inner.events {
        if event.time_us.saturating_add(RATE_WINDOW_US) < now {
            continue;
        }
        let completed_queue = if event.unblocked {
            event.block.map(|reason| {
                if is_generation_blocker(reason) {
                    TerrainTraceQueue::GenerationBlocked
                } else if is_upload_blocker(reason) {
                    TerrainTraceQueue::UploadBlocked
                } else {
                    TerrainTraceQueue::PublicationBlocked
                }
            })
        } else {
            match event.stage {
                Some(Stage::GenerationStarted) => Some(TerrainTraceQueue::GenerationQueued),
                Some(Stage::GenerationCompleted) => Some(TerrainTraceQueue::GenerationActive),
                Some(Stage::Resident) => Some(TerrainTraceQueue::GeneratedAwaitingUpload),
                Some(Stage::BoundaryQueued) => Some(TerrainTraceQueue::GeneratedAwaitingBoundary),
                Some(Stage::BoundaryStarted) => Some(TerrainTraceQueue::BoundaryQueued),
                Some(Stage::FullyPrepared) => Some(TerrainTraceQueue::BoundaryActive),
                Some(Stage::BoundaryFailed) => Some(TerrainTraceQueue::BoundaryActive),
                Some(Stage::Publishable) => Some(TerrainTraceQueue::Prepared),
                Some(Stage::PublicationStarted) => Some(TerrainTraceQueue::Publishable),
                Some(Stage::PublicationFinished) => Some(TerrainTraceQueue::PublicationActive),
                Some(Stage::Drawable) => Some(TerrainTraceQueue::ResidentAwaitingDrawable),
                _ => None,
            }
        };
        if let Some(queue) = completed_queue {
            let accumulator = accumulators
                .get_mut(&queue)
                .expect("all queues are registered");
            accumulator.completed = accumulator.completed.saturating_add(1);
        }
    }
    accumulators
}

fn finish_queue_snapshots(
    mut accumulators: HashMap<TerrainTraceQueue, QueueAccumulator>,
    now: u64,
    rate_window_complete: bool,
) -> Vec<TerrainTraceQueueSnapshot> {
    let rate_window = now.clamp(1, RATE_WINDOW_US) as f64;
    TerrainTraceQueue::ALL
        .into_iter()
        .map(|queue| {
            let accumulator = accumulators.remove(&queue).unwrap_or_default();
            let mut ages = accumulator.ages_ms;
            ages.sort_unstable();
            TerrainTraceQueueSnapshot {
                queue,
                queue_scope: queue_scope(queue),
                count: ages.len(),
                oldest_age_ms: ages.last().copied(),
                p50_age_ms: percentile(&ages, 50),
                p95_age_ms: percentile(&ages, 95),
                completed_per_second: accumulator.completed as f64 * 1_000_000.0 / rate_window,
                rate_window_complete,
                dropped: accumulator.dropped,
                cancelled: accumulator.cancelled,
                superseded: accumulator.superseded,
            }
        })
        .collect()
}

fn queue_for_job(job: &Job) -> Option<(TerrainTraceQueue, u64, Option<u64>)> {
    let get = |stage: Stage| job.milestones[stage.index()];
    let terminal = [
        Stage::GenerationCancelled,
        Stage::GenerationFailed,
        Stage::BoundaryFailed,
        Stage::StaleCompletion,
        Stage::DiscardedStale,
        Stage::Superseded,
        Stage::CpuEvicted,
        Stage::Evicted,
    ]
    .into_iter()
    .any(|stage| get(stage).is_some());
    if terminal
        || job.drawable
        || get(Stage::Drawable).is_some()
        || get(Stage::Drawn).is_some()
        || get(Stage::TransitionCompleted).is_some()
    {
        return None;
    }
    if let Some((reason, started)) = job.blocked_since.iter().min_by_key(|(_, started)| *started) {
        let queue = if is_generation_blocker(*reason) {
            TerrainTraceQueue::GenerationBlocked
        } else if is_upload_blocker(*reason) {
            TerrainTraceQueue::UploadBlocked
        } else {
            TerrainTraceQueue::PublicationBlocked
        };
        return Some((queue, *started, get(Stage::PublicationStarted)));
    }
    if let Some(started) = get(Stage::PublicationStarted)
        && get(Stage::PublicationFinished).is_none()
    {
        return Some((
            TerrainTraceQueue::PublicationActive,
            started,
            get(Stage::PublicationFinished),
        ));
    }
    if let Some(publishable) = get(Stage::Publishable)
        && get(Stage::PublicationStarted).is_none()
    {
        return Some((
            TerrainTraceQueue::Publishable,
            publishable,
            get(Stage::PublicationStarted),
        ));
    }
    if let Some(prepared) = get(Stage::FullyPrepared)
        && get(Stage::Publishable).is_none()
    {
        return Some((
            TerrainTraceQueue::Prepared,
            prepared,
            get(Stage::Publishable),
        ));
    }
    if let Some(started) = get(Stage::BoundaryStarted)
        && get(Stage::BoundaryFinished).is_none()
        && get(Stage::BoundaryFailed).is_none()
    {
        return Some((
            TerrainTraceQueue::BoundaryActive,
            started,
            get(Stage::BoundaryFinished),
        ));
    }
    if let Some(queued) = get(Stage::BoundaryQueued)
        && get(Stage::BoundaryStarted).is_none()
    {
        return Some((
            TerrainTraceQueue::BoundaryQueued,
            queued,
            get(Stage::BoundaryStarted),
        ));
    }
    // Residency alone does not make a tile eligible for boundary preparation:
    // an entire local replacement must be ready. The publication pipeline owns
    // that queue; unqueued resident tiles remain in resident_drawability.
    if let Some(completed) = get(Stage::GenerationCompleted)
        && get(Stage::BoundaryQueued).is_none()
        && !job.resident
        && get(Stage::Drawable).is_none()
        && get(Stage::Drawn).is_none()
        && get(Stage::TransitionCompleted).is_none()
    {
        return Some((
            if job.upload_candidate {
                TerrainTraceQueue::GeneratedAwaitingUpload
            } else {
                TerrainTraceQueue::ParkedCache
            },
            job.upload_candidate_since_us.unwrap_or(completed),
            None,
        ));
    }
    if let Some(started) = get(Stage::GenerationStarted)
        && get(Stage::GenerationCompleted).is_none()
        && get(Stage::GenerationCancelled).is_none()
        && get(Stage::GenerationFailed).is_none()
    {
        return Some((
            TerrainTraceQueue::GenerationActive,
            started,
            get(Stage::GenerationCompleted),
        ));
    }
    if let Some(queued) = get(Stage::GenerationQueued)
        && get(Stage::GenerationStarted).is_none()
        && get(Stage::GenerationCancelled).is_none()
        && get(Stage::GenerationFailed).is_none()
    {
        return Some((
            TerrainTraceQueue::GenerationQueued,
            queued,
            get(Stage::GenerationStarted),
        ));
    }
    if job.resident {
        let resident = get(Stage::Resident).or(job.requested_us).unwrap_or(0);
        return Some((
            TerrainTraceQueue::ResidentAwaitingDrawable,
            resident,
            get(Stage::Drawable),
        ));
    }
    None
}

fn is_generation_blocker(reason: BlockReason) -> bool {
    matches!(
        reason,
        BlockReason::WorkerQueueFull | BlockReason::CpuCapacity
    )
}

fn queue_scope(queue: TerrainTraceQueue) -> &'static str {
    match queue {
        TerrainTraceQueue::GenerationQueued
        | TerrainTraceQueue::GenerationBlocked
        | TerrainTraceQueue::GenerationActive => "generation_admission",
        TerrainTraceQueue::GeneratedAwaitingUpload | TerrainTraceQueue::UploadBlocked => {
            "gpu_upload_admission"
        }
        TerrainTraceQueue::ParkedCache => "inactive_cpu_cache",
        TerrainTraceQueue::GeneratedAwaitingBoundary
        | TerrainTraceQueue::BoundaryQueued
        | TerrainTraceQueue::BoundaryActive => "boundary_preparation",
        TerrainTraceQueue::Prepared
        | TerrainTraceQueue::Publishable
        | TerrainTraceQueue::PublicationActive
        | TerrainTraceQueue::PublicationBlocked => "publication",
        TerrainTraceQueue::ResidentAwaitingDrawable => "resident_drawability",
        TerrainTraceQueue::Unattributed => "unattributed_terminal",
    }
}

fn is_upload_blocker(reason: BlockReason) -> bool {
    matches!(
        reason,
        BlockReason::UploadCapacity
            | BlockReason::GpuResourcePressure
            | BlockReason::ResidentSlotUnavailable
    )
}

#[cfg(test)]
fn snapshot_job(job: &Job, now: u64) -> TerrainTraceJob {
    snapshot_job_with_profile(job, now, None, &[], 0)
}

fn snapshot_job_with_profile(
    job: &Job,
    now: u64,
    origin_frame_id: Option<u64>,
    profile_jobs: &[ProfileJobLink],
    profile_jobs_omitted: u64,
) -> TerrainTraceJob {
    let get = |stage: Stage| job.milestones[stage.index()];
    let latest = job
        .milestones
        .iter()
        .enumerate()
        .filter_map(|(index, time)| time.map(|time| (stage_from_index(index), time)))
        .max_by_key(|(_, time)| *time);
    let is_blocked = !job.blocked_since.is_empty();
    let state = if get(Stage::GenerationFailed).is_some() {
        TerrainTraceJobState::Failed
    } else if get(Stage::BoundaryFailed).is_some() {
        TerrainTraceJobState::BoundaryFailed
    } else if get(Stage::StaleCompletion).is_some() || get(Stage::DiscardedStale).is_some() {
        TerrainTraceJobState::Stale
    } else if get(Stage::GenerationCancelled).is_some() {
        TerrainTraceJobState::Cancelled
    } else if get(Stage::Superseded).is_some() {
        TerrainTraceJobState::Superseded
    } else if get(Stage::Evicted).is_some() || get(Stage::CpuEvicted).is_some() {
        TerrainTraceJobState::Evicted
    } else if job.drawable {
        TerrainTraceJobState::Drawable
    } else if get(Stage::Drawable).is_some()
        || get(Stage::Drawn).is_some()
        || get(Stage::TransitionCompleted).is_some()
    {
        if job.resident {
            TerrainTraceJobState::Resident
        } else {
            TerrainTraceJobState::Retired
        }
    } else if is_blocked {
        let reason = job
            .blocked_since
            .iter()
            .min_by_key(|(_, started)| *started)
            .map(|(reason, _)| *reason);
        if reason.is_some_and(is_generation_blocker) {
            TerrainTraceJobState::GenerationBlocked
        } else if reason.is_some_and(is_upload_blocker) {
            TerrainTraceJobState::UploadBlocked
        } else {
            TerrainTraceJobState::PublicationBlocked
        }
    } else if get(Stage::PublicationStarted).is_some() && get(Stage::PublicationFinished).is_none()
    {
        TerrainTraceJobState::PublicationRunning
    } else if get(Stage::PublicationFinished).is_some() {
        TerrainTraceJobState::PublicationFinished
    } else if get(Stage::Publishable).is_some() {
        TerrainTraceJobState::Publishable
    } else if get(Stage::FullyPrepared).is_some() {
        TerrainTraceJobState::Prepared
    } else if get(Stage::BoundaryStarted).is_some() && get(Stage::BoundaryFinished).is_none() {
        TerrainTraceJobState::BoundaryRunning
    } else if get(Stage::BoundaryQueued).is_some() && get(Stage::BoundaryStarted).is_none() {
        TerrainTraceJobState::BoundaryQueued
    } else if get(Stage::GenerationCompleted).is_some() && get(Stage::BoundaryQueued).is_none() {
        if job.resident {
            TerrainTraceJobState::Resident
        } else if job.upload_candidate {
            TerrainTraceJobState::GeneratedAwaitingUpload
        } else {
            TerrainTraceJobState::ParkedCache
        }
    } else if get(Stage::GenerationStarted).is_some() && get(Stage::GenerationCompleted).is_none() {
        TerrainTraceJobState::GenerationRunning
    } else if get(Stage::GenerationQueued).is_some() {
        TerrainTraceJobState::GenerationQueued
    } else if job.resident {
        TerrainTraceJobState::Resident
    } else {
        TerrainTraceJobState::Desired
    };
    let address = job.key.address;
    let parent = address.parent().map(Into::into);
    let children = address
        .children()
        .map(|children| children.into_iter().map(Into::into).collect())
        .unwrap_or_default();
    let neighbors = PatchEdge::ALL
        .into_iter()
        .map(|edge| address.neighbor(edge).address.into())
        .collect();
    TerrainTraceJob {
        address: address.into(),
        body_identity: job.key.body_identity,
        definition_words: job.key.definition_words.clone(),
        radius_bits: job.key.radius_bits,
        surface_revision: job.key.surface_revision,
        material_revision: job.key.material_revision,
        format_version: job.key.format_version,
        filter_version: job.key.filter_version,
        cells: job.key.cells,
        origin_frame_id,
        profile_jobs: profile_jobs.to_vec(),
        profile_jobs_omitted,
        blocker_intervals: job
            .blocker_intervals
            .iter()
            .cloned()
            .chain(
                job.blocked_since
                    .iter()
                    .map(|(reason, start_us)| TraceBlockInterval {
                        reason: *reason,
                        start_us: *start_us,
                        end_us: now,
                        open: true,
                    }),
            )
            .collect(),
        blocker_intervals_omitted: job.blocker_intervals_omitted,
        prepared_to_drawable_us: get(Stage::FullyPrepared)
            .zip(get(Stage::Drawable))
            .and_then(|(start, end)| end.checked_sub(start)),
        requested_us: job.requested_us,
        milestones: job
            .milestones
            .iter()
            .enumerate()
            .filter_map(|(index, time_us)| {
                time_us.map(|time_us| TerrainTraceMilestone {
                    stage: stage_from_index(index),
                    time_us,
                })
            })
            .collect(),
        blocked_by: job
            .blocked_since
            .iter()
            .map(|(reason, _)| *reason)
            .collect(),
        desired: job.desired,
        resident: job.resident,
        drawable: job.drawable,
        state,
        latest_stage: latest.map(|(stage, _)| stage),
        age_ms: now.saturating_sub(job.requested_us.or(get(Stage::Desired)).unwrap_or(now)) / 1000,
        requested_to_generation_start_us: job
            .requested_us
            .zip(get(Stage::GenerationStarted))
            .map(|(requested, start)| start.saturating_sub(requested)),
        generation_elapsed_us: get(Stage::GenerationStarted)
            .zip(get(Stage::GenerationCompleted))
            .map(|(start, end)| end.saturating_sub(start)),
        generation_to_boundary_start_us: get(Stage::GenerationCompleted)
            .zip(get(Stage::BoundaryStarted))
            .map(|(start, end)| end.saturating_sub(start)),
        boundary_elapsed_us: get(Stage::BoundaryStarted)
            .zip(get(Stage::BoundaryFinished))
            .map(|(start, end)| end.saturating_sub(start)),
        boundary_to_publishable_us: get(Stage::BoundaryFinished)
            .zip(get(Stage::Publishable))
            .map(|(finished, publishable)| publishable.saturating_sub(finished)),
        publication_wait_us: get(Stage::Publishable)
            .zip(get(Stage::PublicationStarted))
            .map(|(publishable, started)| started.saturating_sub(publishable)),
        publication_elapsed_us: get(Stage::PublicationStarted)
            .zip(get(Stage::PublicationFinished))
            .map(|(start, end)| end.saturating_sub(start)),
        requested_to_drawable_us: job
            .requested_us
            .zip(get(Stage::Drawable))
            .map(|(requested, end)| end.saturating_sub(requested)),
        requested_to_transition_completed_us: job
            .requested_us
            .zip(get(Stage::TransitionCompleted))
            .map(|(requested, completed)| completed.saturating_sub(requested)),
        active_blocked_elapsed_us: job
            .blocked_since
            .iter()
            .map(|(_, started)| now.saturating_sub(*started))
            .fold(0u64, u64::saturating_add),
        total_blocked_elapsed_us: job.total_block_time_us.saturating_add(
            job.blocked_since
                .iter()
                .map(|(_, started)| now.saturating_sub(*started))
                .fold(0u64, u64::saturating_add),
        ),
        parent,
        children,
        neighbors,
    }
}

fn block_accumulators(inner: &Inner, now: u64) -> HashMap<BlockReason, BlockAccumulator> {
    let mut accumulators: HashMap<BlockReason, BlockAccumulator> = BlockReason::ALL
        .into_iter()
        .map(|reason| (reason, BlockAccumulator::default()))
        .collect();
    for (reason, totals) in &inner.reason_totals {
        if let Some(accumulator) = accumulators.get_mut(reason) {
            accumulator.count = totals.count;
            accumulator.rechecks = totals.rechecks;
            accumulator.completed_total_us = totals.total_block_time_us;
            accumulator
                .completed_samples_us
                .extend(totals.completed_samples.iter().copied());
        }
    }
    for job in inner.jobs.values() {
        for (reason, started) in &job.blocked_since {
            if let Some(accumulator) = accumulators.get_mut(reason) {
                accumulator
                    .active_ages_us
                    .push(now.saturating_sub(*started));
            }
        }
    }
    accumulators
}

fn finish_block_snapshots(
    mut accumulators: HashMap<BlockReason, BlockAccumulator>,
) -> Vec<TerrainTraceBlockSnapshot> {
    BlockReason::ALL
        .into_iter()
        .map(|reason| {
            let accumulator = accumulators.remove(&reason).unwrap_or_default();
            let mut samples = accumulator.completed_samples_us;
            samples.extend(accumulator.active_ages_us.iter().copied());
            samples.sort_unstable();
            let active_total = accumulator.active_ages_us.iter().copied().sum::<u64>();
            TerrainTraceBlockSnapshot {
                reason,
                count: accumulator.count,
                active_count: accumulator.active_ages_us.len(),
                oldest_age_us: accumulator.active_ages_us.iter().copied().max(),
                p95_block_us: percentile(&samples, 95),
                total_block_time_us: accumulator.completed_total_us.saturating_add(active_total),
                rechecks: accumulator.rechecks,
            }
        })
        .collect()
}

fn stage_from_index(index: usize) -> Stage {
    Stage::ALL[index]
}

fn snapshot_job_keys(order: &VecDeque<TileKey>, jobs: &HashMap<TileKey, Job>) -> Vec<TileKey> {
    let mut selected = Vec::with_capacity(SNAPSHOT_JOBS);
    let mut selected_set = HashSet::with_capacity(SNAPSHOT_JOBS);
    for key in order {
        if jobs
            .get(key)
            .is_some_and(|job| queue_for_job(job).is_some() || !job.blocked_since.is_empty())
        {
            selected.push(key.clone());
            selected_set.insert(key.clone());
            if selected.len() == SNAPSHOT_JOBS / 2 {
                break;
            }
        }
    }
    for key in order.iter().rev() {
        if selected.len() == SNAPSHOT_JOBS {
            break;
        }
        if jobs.contains_key(key) && selected_set.insert(key.clone()) {
            selected.push(key.clone());
        }
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::{
        BlockReason, Inner, Job, ProfileJobLink, Stage, TerrainTrace, TerrainTraceJobState,
        TerrainTraceQueue, evict_oldest_job, queue_accumulators, rate_window_complete,
        reset_boundary_cycle, reset_generation_cycle, snapshot_job,
    };
    use crate::resident_terrain::TileKey;
    use mundaris_math::surface::{CubeFace, CubePatchAddress};
    use std::collections::{HashMap, VecDeque};

    fn test_key(identity: u32) -> TileKey {
        TileKey {
            body_identity: 1,
            definition_words: vec![u64::from(identity)],
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        }
    }

    fn test_job(key: TileKey, queued: bool, running: bool, terminal: bool) -> Job {
        let mut milestones = [None; Stage::COUNT];
        if queued {
            milestones[Stage::GenerationQueued.index()] = Some(1);
        }
        if running {
            milestones[Stage::GenerationStarted.index()] = Some(2);
        }
        if terminal {
            milestones[Stage::GenerationFailed.index()] = Some(3);
        }
        Job {
            key,
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        }
    }

    fn test_inner(jobs: Vec<Job>) -> Inner {
        let mut inner_jobs = HashMap::new();
        let mut order = VecDeque::new();
        for job in jobs {
            order.push_back(job.key.clone());
            inner_jobs.insert(job.key.clone(), job);
        }
        Inner {
            jobs: inner_jobs,
            profile_origins: HashMap::new(),
            profile_jobs: HashMap::new(),
            profile_jobs_omitted: HashMap::new(),
            order,
            current_by_address: HashMap::new(),
            generation_candidates: HashMap::new(),
            generation_candidate_epoch: 0,
            upload_candidates: HashMap::new(),
            upload_candidate_epoch: 0,
            events: VecDeque::new(),
            reason_totals: HashMap::new(),
            dropped_jobs: 0,
            dropped_jobs_by_queue: [0; TerrainTraceQueue::COUNT],
            dropped_events: 0,
            retry_count: 0,
        }
    }

    #[test]
    fn every_stage_has_one_dense_snapshot_index() {
        for (index, stage) in Stage::ALL.iter().copied().enumerate() {
            assert_eq!(stage.index(), index);
        }
        assert_eq!(Stage::COUNT, Stage::ALL.len());
        assert_eq!(Stage::Evicted.index() + 1, Stage::COUNT);
    }

    #[test]
    fn exact_job_blocker_intervals_are_bounded_and_preserve_open_reasons() {
        let key = test_key(1);
        let mut inner = test_inner(vec![test_job(key.clone(), true, false, false)]);
        for index in 0..40 {
            TerrainTrace::record_unblocked_for_job(
                &mut inner,
                &key,
                index * 10 + 5,
                BlockReason::WorkerQueueFull,
                index * 10,
            );
        }
        inner
            .jobs
            .get_mut(&key)
            .unwrap()
            .blocked_since
            .push((BlockReason::CpuCapacity, 395));
        let snapshot = snapshot_job(&inner.jobs[&key], 400);
        assert_eq!(snapshot.blocker_intervals.len(), 33);
        assert_eq!(snapshot.blocker_intervals_omitted, 8);
        assert_eq!(snapshot.blocker_intervals[0].start_us, 80);
        assert!(snapshot.blocker_intervals.last().unwrap().open);
        assert_eq!(snapshot.blocker_intervals.last().unwrap().end_us, 400);
    }

    #[test]
    fn preferred_completed_job_retains_exact_identity_outside_pending_snapshot_frontier() {
        use std::{sync::Mutex, time::Instant};
        let previous = crate::engine_profile::is_enabled();
        crate::engine_profile::set_enabled(true);
        let keys = (1..=150).map(test_key).collect::<Vec<_>>();
        let mut inner = test_inner(
            keys.iter()
                .cloned()
                .map(|key| test_job(key, true, false, false))
                .collect(),
        );
        let selected = keys[74].clone();
        let completed = inner.jobs.get_mut(&selected).unwrap();
        completed.milestones[Stage::GenerationQueued.index()] = None;
        completed.milestones[Stage::GenerationStarted.index()] = Some(3);
        completed.milestones[Stage::GenerationCompleted.index()] = Some(7);
        inner.profile_jobs.insert(
            selected.clone(),
            VecDeque::from([ProfileJobLink {
                job_id: 999,
                capture_id: crate::engine_profile::capture_id(),
                origin_frame_id: 7,
                dispatch_frame_id: 8,
            }]),
        );
        let trace = TerrainTrace {
            origin: Instant::now(),
            inner: Mutex::new(inner),
        };
        assert!(
            !trace
                .snapshot()
                .jobs
                .iter()
                .any(|job| job.definition_words == selected.definition_words)
        );
        let snapshot = trace.snapshot_for_profile_jobs(&[999]);
        let matched = snapshot
            .jobs
            .iter()
            .find(|job| job.profile_jobs.iter().any(|link| link.job_id == 999))
            .unwrap();
        assert_eq!(matched.definition_words, selected.definition_words);
        assert_eq!(matched.body_identity, selected.body_identity);
        assert_eq!(snapshot.jobs.len(), 128);
        assert_eq!(snapshot.snapshot_jobs_omitted, 22);
        crate::engine_profile::set_enabled(previous);
    }

    #[test]
    fn cancellation_and_superseding_counts_use_the_queue_at_terminal_time() {
        let queued_cancel = test_key(1);
        let active_cancel = test_key(2);
        let queued_supersede = test_key(3);
        let active_supersede = test_key(4);
        let mut inner = test_inner(vec![
            test_job(queued_cancel.clone(), true, false, false),
            test_job(active_cancel.clone(), true, true, false),
            test_job(queued_supersede.clone(), true, false, false),
            test_job(active_supersede.clone(), true, true, false),
        ]);

        TerrainTrace::event_locked(&mut inner, &queued_cancel, 10, Stage::GenerationCancelled);
        TerrainTrace::event_locked(&mut inner, &active_cancel, 11, Stage::GenerationCancelled);
        TerrainTrace::event_locked(&mut inner, &queued_supersede, 12, Stage::Superseded);
        TerrainTrace::event_locked(&mut inner, &active_supersede, 13, Stage::Superseded);

        // Repeated terminal notifications preserve event deduplication and do
        // not move the original attribution to Unattributed.
        let event_count = inner.events.len();
        TerrainTrace::event_locked(&mut inner, &queued_cancel, 20, Stage::GenerationCancelled);
        TerrainTrace::event_locked(&mut inner, &active_supersede, 21, Stage::Superseded);
        assert_eq!(inner.events.len(), event_count);

        let accumulators = queue_accumulators(&inner, 30);
        for queue in [
            TerrainTraceQueue::GenerationQueued,
            TerrainTraceQueue::GenerationActive,
        ] {
            let accumulator = &accumulators[&queue];
            assert_eq!(accumulator.cancelled, 1);
            assert_eq!(accumulator.superseded, 1);
        }
        assert_eq!(
            inner.jobs[&queued_cancel].generation_cancelled_queue,
            Some(TerrainTraceQueue::GenerationQueued)
        );
        assert_eq!(
            inner.jobs[&active_cancel].generation_cancelled_queue,
            Some(TerrainTraceQueue::GenerationActive)
        );
    }

    #[test]
    fn job_evictions_are_attributed_to_live_queue_or_unattributed() {
        let queued = test_key(1);
        let active = test_key(2);
        let terminal = test_key(3);
        let mut inner = test_inner(vec![
            test_job(queued, true, false, false),
            test_job(active, true, true, false),
            test_job(terminal, true, false, true),
        ]);

        assert!(evict_oldest_job(&mut inner, 5));
        assert!(evict_oldest_job(&mut inner, 6));
        assert!(evict_oldest_job(&mut inner, 7));

        assert_eq!(inner.dropped_jobs, 3);
        assert_eq!(
            inner.dropped_jobs_by_queue[TerrainTraceQueue::GenerationQueued.index()],
            1
        );
        assert_eq!(
            inner.dropped_jobs_by_queue[TerrainTraceQueue::GenerationActive.index()],
            1
        );
        assert_eq!(
            inner.dropped_jobs_by_queue[TerrainTraceQueue::Unattributed.index()],
            1
        );
        let accumulators = queue_accumulators(&inner, 8);
        assert_eq!(
            accumulators[&TerrainTraceQueue::GenerationQueued].dropped,
            1
        );
        assert_eq!(
            accumulators[&TerrainTraceQueue::GenerationActive].dropped,
            1
        );
        assert_eq!(accumulators[&TerrainTraceQueue::Unattributed].dropped, 1);
    }

    #[test]
    fn rate_window_reports_when_the_event_ring_covers_the_full_interval() {
        let now = super::RATE_WINDOW_US + 100;
        let mut inner = test_inner(Vec::new());
        inner.dropped_events = 1;
        inner.events.push_back(super::TerrainTraceEvent {
            time_us: now - super::RATE_WINDOW_US + 1,
            address: test_key(1).address.into(),
            stage: Some(Stage::GenerationCompleted),
            block: None,
            unblocked: false,
        });
        assert!(!rate_window_complete(&inner, now));

        inner.events.front_mut().expect("event inserted").time_us = now - super::RATE_WINDOW_US;
        assert!(rate_window_complete(&inner, now));

        inner.events.clear();
        assert!(!rate_window_complete(&inner, now));
        inner.dropped_events = 0;
        assert!(rate_window_complete(&inner, now));
    }

    #[test]
    fn boundary_requeue_resets_only_current_publication_cycle() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Requested.index()] = Some(2);
        milestones[Stage::GenerationStarted.index()] = Some(5);
        milestones[Stage::GenerationCompleted.index()] = Some(12);
        milestones[Stage::BoundaryQueued.index()] = Some(13);
        milestones[Stage::BoundaryStarted.index()] = Some(14);
        milestones[Stage::BoundaryFinished.index()] = Some(18);
        milestones[Stage::FullyPrepared.index()] = Some(20);
        milestones[Stage::Publishable.index()] = Some(21);
        milestones[Stage::PublicationStarted.index()] = Some(22);
        milestones[Stage::PublicationFinished.index()] = Some(25);
        let mut job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(2),
            blocked_since: Vec::new(),
            desired: true,
            resident: true,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        reset_boundary_cycle(&mut job);

        assert_eq!(job.milestones[Stage::Requested.index()], Some(2));
        assert_eq!(job.milestones[Stage::GenerationCompleted.index()], Some(12));
        for stage in [
            Stage::BoundaryQueued,
            Stage::BoundaryStarted,
            Stage::BoundaryFinished,
            Stage::FullyPrepared,
            Stage::Publishable,
            Stage::PublicationStarted,
            Stage::PublicationFinished,
        ] {
            assert_eq!(job.milestones[stage.index()], None);
        }
        assert!(job.desired && job.resident);
    }

    #[test]
    fn publication_lifecycle_states_follow_the_latest_active_stage() {
        let key = TileKey {
            body_identity: 1,
            definition_words: Vec::new(),
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        };
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Publishable.index()] = Some(10);
        let mut job = Job {
            key,
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(matches!(
            snapshot_job(&job, 11).state,
            TerrainTraceJobState::Publishable
        ));

        job.milestones[Stage::PublicationStarted.index()] = Some(12);
        assert!(matches!(
            snapshot_job(&job, 13).state,
            TerrainTraceJobState::PublicationRunning
        ));

        job.milestones[Stage::PublicationFinished.index()] = Some(14);
        assert!(matches!(
            snapshot_job(&job, 15).state,
            TerrainTraceJobState::PublicationFinished
        ));
    }

    #[test]
    fn per_tile_latency_timeline_and_block_totals_use_monotonic_milestones() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::GenerationStarted.index()] = Some(110);
        milestones[Stage::GenerationCompleted.index()] = Some(130);
        milestones[Stage::BoundaryQueued.index()] = Some(140);
        milestones[Stage::BoundaryStarted.index()] = Some(150);
        milestones[Stage::BoundaryFinished.index()] = Some(170);
        milestones[Stage::FullyPrepared.index()] = Some(180);
        milestones[Stage::Publishable.index()] = Some(200);
        milestones[Stage::PublicationStarted.index()] = Some(230);
        milestones[Stage::PublicationFinished.index()] = Some(250);
        milestones[Stage::TransitionCompleted.index()] = Some(300);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(100),
            blocked_since: vec![
                (BlockReason::CpuCapacity, 260),
                (BlockReason::UploadCapacity, 270),
            ],
            desired: true,
            resident: true,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 40,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        let row = snapshot_job(&job, 310);
        assert_eq!(row.requested_to_generation_start_us, Some(10));
        assert_eq!(row.generation_elapsed_us, Some(20));
        assert_eq!(row.generation_to_boundary_start_us, Some(20));
        assert_eq!(row.boundary_elapsed_us, Some(20));
        assert_eq!(row.boundary_to_publishable_us, Some(30));
        assert_eq!(row.publication_wait_us, Some(30));
        assert_eq!(row.publication_elapsed_us, Some(20));
        assert_eq!(row.requested_to_transition_completed_us, Some(200));
        assert_eq!(row.active_blocked_elapsed_us, 90);
        assert_eq!(row.total_blocked_elapsed_us, 130);
    }

    #[test]
    fn failed_boundary_is_terminal_and_not_left_in_the_active_queue() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::BoundaryQueued.index()] = Some(1);
        milestones[Stage::BoundaryStarted.index()] = Some(2);
        milestones[Stage::BoundaryFailed.index()] = Some(3);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(matches!(
            snapshot_job(&job, 4).state,
            TerrainTraceJobState::BoundaryFailed
        ));
        assert!(super::queue_for_job(&job).is_none());
    }

    #[test]
    fn generation_retry_keeps_current_residency_and_drawability_milestones() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Requested.index()] = Some(2);
        milestones[Stage::GenerationFailed.index()] = Some(9);
        milestones[Stage::Resident.index()] = Some(3);
        milestones[Stage::Drawable.index()] = Some(4);
        let mut job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(2),
            blocked_since: Vec::new(),
            desired: true,
            resident: true,
            drawable: true,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        reset_generation_cycle(&mut job, 12);

        assert_eq!(job.milestones[Stage::GenerationFailed.index()], None);
        assert_eq!(job.milestones[Stage::Resident.index()], Some(12));
        assert_eq!(job.milestones[Stage::Drawable.index()], Some(12));
        assert!(job.resident && job.drawable);
    }

    #[test]
    fn terminal_blocked_generation_is_not_reported_as_an_active_queue() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::GenerationQueued.index()] = Some(1);
        milestones[Stage::GenerationCancelled.index()] = Some(4);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(1),
            blocked_since: vec![(BlockReason::WorkerQueueFull, 2)],
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(super::queue_for_job(&job).is_none());
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::Cancelled
        ));
    }

    #[test]
    fn terminal_stage_closes_blocker_time_and_emits_reasoned_unblock() {
        use super::{Inner, ReasonTotals, TerrainTrace};
        use std::collections::{HashMap, VecDeque};

        let key = TileKey {
            body_identity: 1,
            definition_words: Vec::new(),
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        };
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::GenerationQueued.index()] = Some(1);
        let job = Job {
            key: key.clone(),
            milestones,
            requested_us: Some(1),
            blocked_since: vec![(BlockReason::WorkerQueueFull, 2)],
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };
        let mut inner = Inner {
            jobs: HashMap::from([(key.clone(), job)]),
            profile_origins: HashMap::new(),
            profile_jobs: HashMap::new(),
            profile_jobs_omitted: HashMap::new(),
            order: VecDeque::from([key.clone()]),
            current_by_address: HashMap::from([(key.address, key.clone())]),
            generation_candidates: HashMap::new(),
            generation_candidate_epoch: 0,
            upload_candidates: HashMap::new(),
            upload_candidate_epoch: 0,
            events: VecDeque::new(),
            reason_totals: HashMap::from([(BlockReason::WorkerQueueFull, ReasonTotals::default())]),
            dropped_jobs: 0,
            dropped_jobs_by_queue: [0; TerrainTraceQueue::COUNT],
            dropped_events: 0,
            retry_count: 0,
        };

        TerrainTrace::event_locked(&mut inner, &key, 7, Stage::GenerationCancelled);

        assert!(inner.jobs[&key].blocked_since.is_empty());
        assert_eq!(inner.jobs[&key].total_block_time_us, 5);
        assert_eq!(
            inner.reason_totals[&BlockReason::WorkerQueueFull].total_block_time_us,
            5
        );
        assert_eq!(inner.jobs[&key].total_block_time_us, 5);
        assert!(
            inner.events.iter().any(|event| {
                event.unblocked && event.block == Some(BlockReason::WorkerQueueFull)
            })
        );
    }

    #[test]
    fn generation_admission_block_has_its_own_queue() {
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones: [None; Stage::COUNT],
            requested_us: Some(1),
            blocked_since: vec![(BlockReason::CpuCapacity, 2)],
            desired: false,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::GenerationBlocked, 2, _))
        ));
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::GenerationBlocked
        ));
    }

    #[test]
    fn completed_jobs_separate_upload_boundary_and_parked_cache_waits() {
        let key = TileKey {
            body_identity: 1,
            definition_words: Vec::new(),
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        };
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::GenerationCompleted.index()] = Some(3);
        milestones[Stage::Resident.index()] = Some(5);
        let mut job = Job {
            key,
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: true,
            resident: false,
            drawable: false,
            upload_candidate: true,
            upload_candidate_since_us: Some(4),
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::GeneratedAwaitingUpload, 4, None))
        ));
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::GeneratedAwaitingUpload
        ));

        job.upload_candidate = false;
        job.upload_candidate_since_us = None;
        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::ParkedCache, 3, None))
        ));
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::ParkedCache
        ));

        job.resident = true;
        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::ResidentAwaitingDrawable, 5, _))
        ));
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::Resident
        ));
        job.milestones[Stage::BoundaryQueued.index()] = Some(6);
        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::BoundaryQueued, 6, _))
        ));
        assert_eq!(
            super::queue_scope(TerrainTraceQueue::ParkedCache),
            "inactive_cpu_cache"
        );
        assert_eq!(
            super::queue_scope(TerrainTraceQueue::GeneratedAwaitingBoundary),
            "boundary_preparation"
        );
    }

    #[test]
    fn retiring_generation_candidate_closes_only_its_admission_block() {
        struct RestoreProfile(bool);
        impl Drop for RestoreProfile {
            fn drop(&mut self) {
                crate::engine_profile::set_enabled(self.0);
            }
        }

        let previous = crate::engine_profile::is_enabled();
        let _restore_profile = RestoreProfile(previous);
        crate::engine_profile::set_enabled(true);
        let trace = super::TerrainTrace::new();
        let key = TileKey {
            body_identity: 1,
            definition_words: Vec::new(),
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        };
        trace.admit(&key);
        trace.block(key.address, BlockReason::CpuCapacity);
        trace.block(key.address, BlockReason::UploadCapacity);
        trace.sync_generation_candidates(std::iter::once(&key));
        trace.sync_generation_candidates(std::iter::empty::<&TileKey>());

        let snapshot = trace.snapshot();
        assert_eq!(snapshot.blocked, 1);
        let job = snapshot
            .jobs
            .iter()
            .find(|job| job.address.level == 0)
            .unwrap();
        assert_eq!(job.blocked_by, vec![BlockReason::UploadCapacity]);
        let generation = snapshot
            .block_reasons
            .iter()
            .find(|reason| reason.reason == BlockReason::CpuCapacity)
            .unwrap();
        assert_eq!(generation.active_count, 0);
        assert_eq!(generation.count, 1);
        assert!(
            snapshot
                .events
                .iter()
                .any(|event| { event.unblocked && event.block == Some(BlockReason::CpuCapacity) })
        );
    }

    #[test]
    fn upload_candidate_retirement_closes_upload_capacity_block() {
        struct RestoreProfile(bool);
        impl Drop for RestoreProfile {
            fn drop(&mut self) {
                crate::engine_profile::set_enabled(self.0);
            }
        }

        let previous = crate::engine_profile::is_enabled();
        let _restore_profile = RestoreProfile(previous);
        crate::engine_profile::set_enabled(true);
        let trace = super::TerrainTrace::new();
        let key = TileKey {
            body_identity: 1,
            definition_words: Vec::new(),
            radius_bits: 1,
            surface_revision: 1,
            material_revision: 1,
            format_version: 1,
            filter_version: 1,
            address: CubePatchAddress::root(CubeFace::PositiveZ),
            cells: 1,
        };
        trace.admit(&key);
        trace.event_key(&key, Stage::GenerationCompleted);
        trace.block(key.address, BlockReason::UploadCapacity);
        trace.sync_upload_candidates(std::iter::once(&key));
        assert!(matches!(
            trace.snapshot().jobs[0].state,
            TerrainTraceJobState::UploadBlocked
        ));
        trace.sync_upload_candidates(std::iter::empty::<&TileKey>());

        let snapshot = trace.snapshot();
        assert!(
            snapshot
                .jobs
                .iter()
                .all(|job| !matches!(job.state, TerrainTraceJobState::UploadBlocked))
        );
        let upload = snapshot
            .block_reasons
            .iter()
            .find(|reason| reason.reason == BlockReason::UploadCapacity)
            .unwrap();
        assert_eq!(upload.active_count, 0);
        assert!(
            snapshot.events.iter().any(|event| {
                event.unblocked && event.block == Some(BlockReason::UploadCapacity)
            })
        );
    }

    #[test]
    fn resident_ancestor_needed_for_topology_remains_queued_when_undesired() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::GenerationQueued.index()] = Some(3);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: None,
            blocked_since: Vec::new(),
            desired: false,
            resident: false,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(matches!(
            super::queue_for_job(&job),
            Some((TerrainTraceQueue::GenerationQueued, 3, _))
        ));
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::GenerationQueued
        ));
    }

    #[test]
    fn previously_drawable_retired_resident_is_not_awaiting_first_draw() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Drawable.index()] = Some(3);
        milestones[Stage::TransitionCompleted.index()] = Some(4);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: false,
            resident: true,
            drawable: false,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(super::queue_for_job(&job).is_none());
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::Resident
        ));
    }

    #[test]
    fn evicted_previously_drawable_job_reports_terminal_state() {
        let mut milestones = [None; Stage::COUNT];
        milestones[Stage::Drawable.index()] = Some(3);
        milestones[Stage::Evicted.index()] = Some(4);
        let job = Job {
            key: TileKey {
                body_identity: 1,
                definition_words: Vec::new(),
                radius_bits: 1,
                surface_revision: 1,
                material_revision: 1,
                format_version: 1,
                filter_version: 1,
                address: CubePatchAddress::root(CubeFace::PositiveZ),
                cells: 1,
            },
            milestones,
            requested_us: Some(1),
            blocked_since: Vec::new(),
            desired: false,
            resident: false,
            drawable: true,
            upload_candidate: false,
            upload_candidate_since_us: None,
            generation_cancelled_queue: None,
            superseded_queue: None,
            total_block_time_us: 0,
            blocker_intervals: VecDeque::new(),
            blocker_intervals_omitted: 0,
        };

        assert!(super::queue_for_job(&job).is_none());
        assert!(matches!(
            snapshot_job(&job, 5).state,
            TerrainTraceJobState::Evicted
        ));
    }
}
