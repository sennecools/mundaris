//! Bounded, opt-in CPU timeline profiling for app-owned engine work.
//!
//! Timestamps are nanoseconds from a process-local [`Instant`] origin and are
//! comparable across threads. A span captures the frame number when it starts;
//! callers should publish a frame before dispatching that frame's worker work.
//! The default lane is `Main`. Worker scopes select a stable named or numeric
//! lane for the scope's thread and restore the previous lane when dropped.
//!
//! Event and rolling-stat rings are fixed-size and allocated when a lane is
//! first registered. Stable named worker lanes can be reused by short-lived
//! threads; each event keeps its originating thread sequence. A completed
//! span takes only its lane lock. When disabled,
//! creating a span returns an inert guard after one relaxed atomic load.

use mundaris_renderer::resident_tile::TileKey;
use serde::Serialize;
use std::{
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};

#[cfg(feature = "surface-profile")]
use mundaris_renderer::planet_surface::CpuStageTimer;

const MAX_LANES: usize = 16;
const MAIN_EVENT_CAPACITY: usize = 8_192;
const WORKER_EVENT_CAPACITY: usize = 512;
const MAX_NAMES_PER_LANE: usize = 64;
const STAT_SAMPLE_CAPACITY: usize = 128;
const MAX_BUDGETS: usize = 64;
const MAX_NESTING: usize = 64;

/// Stable identity assigned to a profile lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum WorkerIdentity {
    Main,
    Named(&'static str),
    Numeric(u64),
}

/// Stable identity carried by profiler-only worker envelopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProfileJobIdentity {
    Tile {
        job_id: u64,
        capture_id: u64,
        origin_frame_id: u64,
        dispatch_frame_id: u64,
        body_identity: u64,
        surface_revision: u64,
        material_revision: u64,
        radius_bits: u64,
        face: u8,
        level: u8,
        x: u32,
        y: u32,
        cells: u32,
        format_version: u32,
        filter_version: u32,
    },
    BoundaryBatch {
        job_id: u64,
        capture_id: u64,
        origin_frame_id: u64,
        dispatch_frame_id: u64,
        topology_revision: u64,
        tile_count: u32,
        upstream_job_ids: [u64; 8],
        upstream_job_count: u8,
        upstream_jobs_omitted: u32,
    },
}

impl ProfileJobIdentity {
    pub fn capture_id(self) -> u64 {
        match self {
            Self::Tile { capture_id, .. } | Self::BoundaryBatch { capture_id, .. } => capture_id,
        }
    }

    pub fn tile(key: &TileKey, job_id: u64, origin_frame_id: u64, dispatch_frame_id: u64) -> Self {
        let [x, y] = key.address.coordinates();
        Self::Tile {
            job_id,
            capture_id: capture_id(),
            origin_frame_id,
            dispatch_frame_id,
            body_identity: key.body_identity,
            surface_revision: key.surface_revision,
            material_revision: key.material_revision,
            radius_bits: key.radius_bits,
            face: key.address.face() as u8,
            level: key.address.level(),
            x,
            y,
            cells: key.cells,
            format_version: key.format_version,
            filter_version: key.filter_version,
        }
    }

    pub fn boundary_batch(
        job_id: u64,
        origin_frame_id: u64,
        dispatch_frame_id: u64,
        topology_revision: u64,
        tile_count: u32,
        upstream: impl IntoIterator<Item = u64>,
    ) -> Self {
        let mut upstream_job_ids = [0; 8];
        let mut upstream_job_count = 0;
        let mut upstream_jobs_omitted = 0u32;
        for id in upstream {
            if usize::from(upstream_job_count) < upstream_job_ids.len() {
                upstream_job_ids[usize::from(upstream_job_count)] = id;
                upstream_job_count += 1;
            } else {
                upstream_jobs_omitted = upstream_jobs_omitted.saturating_add(1);
            }
        }
        Self::BoundaryBatch {
            job_id,
            capture_id: capture_id(),
            origin_frame_id,
            dispatch_frame_id,
            topology_revision,
            tile_count,
            upstream_job_ids,
            upstream_job_count,
            upstream_jobs_omitted,
        }
    }
}

/// Current recording generation. Old queued context is never reused after clear.
pub fn capture_id() -> u64 {
    profiler().capture_id.load(Ordering::Relaxed)
}

/// Copy scalar job context for an explicitly spawned child task.
pub fn current_job_identity() -> Option<ProfileJobIdentity> {
    if !is_enabled() {
        return None;
    }
    THREAD_STATE
        .with(|state| state.borrow().job_identity)
        .filter(|identity| identity.capture_id() == capture_id())
}

/// Bounded completed-job preference for the off-thread terrain snapshot. This
/// retains exact ID links for completed work as well as the pending frontier.
pub fn recent_job_ids() -> Vec<u64> {
    let snapshot = snapshot();
    let mut events = snapshot
        .lanes
        .iter()
        .flat_map(|lane| &lane.events)
        .filter(|event| event.job_identity.is_some())
        .collect::<Vec<_>>();
    events.sort_unstable_by_key(|event| std::cmp::Reverse(event.end_ns));
    let mut ids = Vec::with_capacity(64);
    for event in events {
        let Some(identity) = event.job_identity else {
            continue;
        };
        let candidates: &[u64] = match &identity {
            ProfileJobIdentity::Tile { job_id, .. } => std::slice::from_ref(job_id),
            ProfileJobIdentity::BoundaryBatch {
                upstream_job_ids,
                upstream_job_count,
                ..
            } => &upstream_job_ids[..usize::from(*upstream_job_count)],
        };
        for id in candidates {
            if !ids.contains(id) {
                ids.push(*id);
            }
            if ids.len() == 64 {
                return ids;
            }
        }
    }
    ids
}

/// Monotonic process-local identity for one admitted asynchronous task.
pub fn next_job_id() -> u64 {
    profiler().next_job_id.fetch_add(1, Ordering::Relaxed)
}

/// Typed reason for an instrumented blocking wait. Uninstrumented blank time
/// remains unknown and is not assigned an inferred cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileWaitReason {
    TerrainWorkerQueue,
    BoundaryPreparationQueue,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileEvent {
    pub event_id: u64,
    pub capture_id: u64,
    pub nesting_depth: usize,
    pub parent_event_id: Option<u64>,
    /// Identifies the originating OS thread when a stable lane is reused by
    /// short-lived threads across batches.
    pub thread_sequence: u64,
    pub name: &'static str,
    pub job_identity: Option<ProfileJobIdentity>,
    pub wait_reason: Option<ProfileWaitReason>,
    pub frame_id: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    pub duration_ns: u64,
    /// Optional time scheduled on the calling OS thread. `None` means the
    /// diagnostic was disabled or the platform clock could not report it.
    /// Short spans may report `Some(0)` because OS accounting is coarse.
    pub thread_cpu_ns: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileLaneSnapshot {
    pub lane_id: usize,
    pub worker: WorkerIdentity,
    /// Thread sequence that first registered the lane. Individual events
    /// retain their own sequence when a named lane is reused.
    pub registration_thread_sequence: u64,
    pub events: Vec<ProfileEvent>,
    /// Number of old events overwritten by this lane's bounded ring.
    pub dropped_events: u64,
    /// Completed events omitted from name-based rolling statistics after that
    /// lane encountered more distinct names than its fixed table supports.
    pub dropped_stat_names: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileStat {
    pub name: &'static str,
    /// Most recently completed sample included by this snapshot.
    pub current_ns: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    /// Maximum among the rolling samples included by this snapshot.
    pub max_ns: u64,
    pub sample_count: u64,
    pub budget_ns: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProfileSnapshot {
    pub schema_version: u32,
    pub enabled: bool,
    /// True only when the process was launched with
    /// `MUNDARIS_PROFILE_THREAD_CPU=1` and the `surface-profile` feature.
    pub thread_cpu_timing_enabled: bool,
    pub capture_id: u64,
    pub dropped_nesting: u64,
    pub snapshot_events_omitted: usize,
    pub generated_at_ns: u64,
    pub frame_id: u64,
    /// If present, timeline events and rolling samples ending before this
    /// timestamp are omitted.
    pub since_ns: Option<u64>,
    pub lanes: Vec<ProfileLaneSnapshot>,
    pub stats: Vec<ProfileStat>,
    /// Worker lanes refused after the bounded lane table filled.
    pub dropped_lane_registrations: u64,
}

/// Result of measuring the disabled span fast path in the current process.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileOverheadMeasurement {
    pub iterations: usize,
    pub total_ns: u64,
    pub ns_per_span: f64,
}

#[derive(Clone, Copy)]
struct ThreadState {
    lane_id: Option<usize>,
    thread_sequence: u64,
    worker_scoped: bool,
    job_identity: Option<ProfileJobIdentity>,
    stack: [u64; MAX_NESTING],
    depth: usize,
}

impl ThreadState {
    const fn new() -> Self {
        Self {
            lane_id: None,
            thread_sequence: 0,
            worker_scoped: false,
            job_identity: None,
            stack: [0; MAX_NESTING],
            depth: 0,
        }
    }

    fn begin_span(&mut self, event_id: u64) -> Option<(usize, Option<u64>)> {
        if self.depth == MAX_NESTING {
            return None;
        }
        let depth = self.depth;
        let parent = (depth > 0).then(|| self.stack[depth - 1]);
        self.stack[depth] = event_id;
        self.depth += 1;
        Some((depth, parent))
    }

    fn finish_span(&mut self, depth: usize, event_id: u64) {
        if self.depth == depth + 1 && self.stack[depth] == event_id {
            self.depth = depth;
        } else if self.depth > depth {
            // Recover the nesting stack after an unusual non-LIFO drop.
            self.depth = depth;
        }
    }
}

thread_local! {
    static THREAD_STATE: RefCell<ThreadState> = const { RefCell::new(ThreadState::new()) };
}

struct EventRing {
    entries: Vec<Option<ProfileEvent>>,
    next: usize,
    len: usize,
    dropped: u64,
}

impl EventRing {
    fn new(capacity: usize) -> Self {
        Self {
            entries: vec![None; capacity],
            next: 0,
            len: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, event: ProfileEvent) {
        if self.len == self.entries.len() {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.len += 1;
        }
        self.entries[self.next] = Some(event);
        self.next = (self.next + 1) % self.entries.len();
    }

    fn copy_since(&self, since_ns: Option<u64>, output: &mut Vec<ProfileEvent>) {
        let start = if self.len == self.entries.len() {
            self.next
        } else {
            0
        };
        for offset in 0..self.len {
            let index = (start + offset) % self.entries.len();
            if let Some(event) = &self.entries[index]
                && since_ns.is_none_or(|since| event.end_ns >= since)
            {
                output.push(event.clone());
            }
        }
    }

    fn clear(&mut self) {
        self.entries.fill(None);
        self.next = 0;
        self.len = 0;
        self.dropped = 0;
    }
}

#[derive(Clone, Copy, Default)]
struct StatSample {
    duration_ns: u64,
    end_ns: u64,
}

struct StatSlot {
    name: Option<&'static str>,
    samples: [StatSample; STAT_SAMPLE_CAPACITY],
    next: usize,
    len: usize,
}

impl StatSlot {
    fn empty() -> Self {
        Self {
            name: None,
            samples: [StatSample::default(); STAT_SAMPLE_CAPACITY],
            next: 0,
            len: 0,
        }
    }

    fn push(&mut self, name: &'static str, duration_ns: u64, end_ns: u64) {
        if self.name.is_none() {
            self.name = Some(name);
        }
        self.samples[self.next] = StatSample {
            duration_ns,
            end_ns,
        };
        self.next = (self.next + 1) % self.samples.len();
        self.len = (self.len + 1).min(self.samples.len());
    }

    fn clear(&mut self) {
        *self = Self::empty();
    }
}

struct LaneData {
    events: EventRing,
    stats: Vec<StatSlot>,
    dropped_stat_names: u64,
}

struct Lane {
    id: usize,
    worker: WorkerIdentity,
    registration_thread_sequence: u64,
    data: Mutex<LaneData>,
}

impl Lane {
    fn new(id: usize, worker: WorkerIdentity, registration_thread_sequence: u64) -> Self {
        let event_capacity = if matches!(worker, WorkerIdentity::Main) {
            MAIN_EVENT_CAPACITY
        } else {
            WORKER_EVENT_CAPACITY
        };
        Self {
            id,
            worker,
            registration_thread_sequence,
            data: Mutex::new(LaneData {
                events: EventRing::new(event_capacity),
                stats: (0..MAX_NAMES_PER_LANE).map(|_| StatSlot::empty()).collect(),
                dropped_stat_names: 0,
            }),
        }
    }

    fn record(&self, event: ProfileEvent) {
        let mut data = lock(&self.data);
        data.events.push(event.clone());
        let existing = data
            .stats
            .iter()
            .position(|slot| slot.name == Some(event.name));
        let index = existing.or_else(|| data.stats.iter().position(|slot| slot.name.is_none()));
        let slot = index.map(|index| &mut data.stats[index]);
        if let Some(slot) = slot {
            slot.push(event.name, event.duration_ns, event.end_ns);
        } else {
            data.dropped_stat_names = data.dropped_stat_names.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy)]
struct Budget {
    name: &'static str,
    threshold_ns: u64,
}

struct Profiler {
    enabled: AtomicBool,
    frame_id: AtomicU64,
    capture_id: AtomicU64,
    dropped_nesting: AtomicU64,
    next_event_id: AtomicU64,
    next_job_id: AtomicU64,
    next_thread_sequence: AtomicU64,
    dropped_lane_registrations: AtomicU64,
    lanes: Mutex<Vec<Arc<Lane>>>,
    lane_slots: [OnceLock<Arc<Lane>>; MAX_LANES],
    budgets: Mutex<Vec<Budget>>,
    thread_cpu_timing_enabled: bool,
}

impl Profiler {
    fn new() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            frame_id: AtomicU64::new(0),
            capture_id: AtomicU64::new(1),
            dropped_nesting: AtomicU64::new(0),
            next_event_id: AtomicU64::new(1),
            next_job_id: AtomicU64::new(1),
            next_thread_sequence: AtomicU64::new(1),
            dropped_lane_registrations: AtomicU64::new(0),
            lanes: Mutex::new(Vec::with_capacity(MAX_LANES)),
            lane_slots: std::array::from_fn(|_| OnceLock::new()),
            budgets: Mutex::new(Vec::with_capacity(MAX_BUDGETS)),
            thread_cpu_timing_enabled: thread_cpu_timing_requested(),
        }
    }

    fn lane_for(&self, worker: WorkerIdentity, thread_sequence: u64) -> Option<usize> {
        let mut lanes = lock(&self.lanes);
        if let Some(lane) = lanes.iter().find(|lane| lane.worker == worker) {
            return Some(lane.id);
        }
        let main_lane_reserved = worker != WorkerIdentity::Main;
        if lanes.len() == MAX_LANES || (main_lane_reserved && lanes.len() == MAX_LANES - 1) {
            self.dropped_lane_registrations
                .fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let id = lanes.len();
        let lane = Arc::new(Lane::new(id, worker, thread_sequence));
        if self.lane_slots[id].set(Arc::clone(&lane)).is_err() {
            return None;
        }
        lanes.push(lane);
        Some(id)
    }

    fn lane(&self, lane_id: usize) -> Option<Arc<Lane>> {
        self.lane_slots.get(lane_id)?.get().cloned()
    }

    fn budget_for(&self, name: &'static str) -> Option<u64> {
        lock(&self.budgets)
            .iter()
            .find(|budget| budget.name == name)
            .map(|budget| budget.threshold_ns)
    }
}

#[cfg(feature = "surface-profile")]
fn thread_cpu_timing_requested() -> bool {
    std::env::var_os("MUNDARIS_PROFILE_THREAD_CPU").is_some_and(|value| value == "1")
}

#[cfg(not(feature = "surface-profile"))]
const fn thread_cpu_timing_requested() -> bool {
    false
}

fn profiler() -> &'static Profiler {
    static PROFILER: OnceLock<Profiler> = OnceLock::new();
    PROFILER.get_or_init(Profiler::new)
}

fn clock_origin() -> &'static Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now)
}

fn now_ns() -> u64 {
    clock_origin().elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Enables or disables profiling. Disabled spans do not register lanes or read
/// the clock; in-flight spans still complete and are recorded.
pub fn set_enabled(enabled: bool) {
    profiler().enabled.store(enabled, Ordering::Relaxed);
}

/// Measures the disabled span fast path without writing events or clearing
/// existing profile data. The process-wide enabled flag is restored on return,
/// including unwinding. Run this in an isolated profiling process because spans
/// on other threads are also temporarily disabled during the measurement.
pub fn measure_overhead(iterations: usize) -> ProfileOverheadMeasurement {
    if iterations == 0 {
        return ProfileOverheadMeasurement {
            iterations,
            total_ns: 0,
            ns_per_span: 0.0,
        };
    }

    struct RestoreEnabled(bool);
    impl Drop for RestoreEnabled {
        fn drop(&mut self) {
            profiler().enabled.store(self.0, Ordering::Relaxed);
        }
    }

    let profiler = profiler();
    let previous = profiler.enabled.swap(false, Ordering::Relaxed);
    let _restore_enabled = RestoreEnabled(previous);
    let started = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(span("engine_profile.overhead_probe"));
    }
    let total_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
    ProfileOverheadMeasurement {
        iterations,
        total_ns,
        ns_per_span: total_ns as f64 / iterations as f64,
    }
}

/// Returns whether new spans currently record.
pub fn is_enabled() -> bool {
    profiler().enabled.load(Ordering::Relaxed)
}

/// Associates subsequently started spans with `frame_id`.
pub fn begin_frame(frame_id: u64) {
    profiler().frame_id.store(frame_id, Ordering::Relaxed);
}

/// Returns the current frame ID for asynchronous trace correlation. This
/// performs one relaxed atomic load and allocates nothing.
pub fn current_frame_id() -> u64 {
    profiler().frame_id.load(Ordering::Relaxed)
}

/// Installs or updates a static-name duration budget in nanoseconds. Returns
/// false only when the bounded budget table is full.
pub fn set_budget_ns(name: &'static str, threshold_ns: Option<u64>) -> bool {
    let mut budgets = lock(&profiler().budgets);
    if let Some(index) = budgets.iter().position(|budget| budget.name == name) {
        if let Some(threshold_ns) = threshold_ns {
            budgets[index].threshold_ns = threshold_ns;
        } else {
            budgets.swap_remove(index);
        }
        return true;
    }
    let Some(threshold_ns) = threshold_ns else {
        return true;
    };
    if budgets.len() == MAX_BUDGETS {
        return false;
    }
    budgets.push(Budget { name, threshold_ns });
    true
}

/// Starts a named CPU span. Names must be compile-time static strings. The
/// returned RAII guard records when dropped.
pub fn span(name: &'static str) -> ProfileSpan {
    span_with_metadata(name, None)
}

/// Starts a measured wait span and labels only the explicitly known blocker.
pub fn wait_span(name: &'static str, reason: ProfileWaitReason) -> ProfileSpan {
    let mut span = span_with_metadata(name, None);
    if span.active {
        span.wait_reason = Some(reason);
    }
    span
}

fn span_with_metadata(name: &'static str, wait_reason: Option<ProfileWaitReason>) -> ProfileSpan {
    let profiler = profiler();
    if !profiler.enabled.load(Ordering::Relaxed) {
        return ProfileSpan::disabled();
    }

    let mut lane_id = None;
    let mut event_id = 0;
    let mut parent_event_id = None;
    let mut thread_sequence = 0;
    let mut depth = 0;
    let mut job_identity = None;
    THREAD_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.thread_sequence == 0 {
            state.thread_sequence = profiler
                .next_thread_sequence
                .fetch_add(1, Ordering::Relaxed);
        }
        if state.lane_id.is_none() && !state.worker_scoped {
            state.lane_id = profiler.lane_for(WorkerIdentity::Main, state.thread_sequence);
        }
        lane_id = state.lane_id;
        thread_sequence = state.thread_sequence;
        job_identity = state
            .job_identity
            .filter(|identity| identity.capture_id() == capture_id());
        depth = state.depth;
        if lane_id.is_some() && depth < MAX_NESTING {
            event_id = profiler.next_event_id.fetch_add(1, Ordering::Relaxed);
            if let Some((span_depth, parent_id)) = state.begin_span(event_id) {
                depth = span_depth;
                parent_event_id = parent_id;
            }
        }
    });
    let Some(lane_id) = lane_id else {
        return ProfileSpan::disabled();
    };
    if depth >= MAX_NESTING {
        profiler.dropped_nesting.fetch_add(1, Ordering::Relaxed);
        return ProfileSpan::disabled();
    }
    let start = now_ns();
    ProfileSpan {
        name,
        lane_id: Some(lane_id),
        event_id,
        capture_id: capture_id(),
        parent_event_id,
        thread_sequence,
        job_identity,
        wait_reason,
        depth,
        frame_id: profiler.frame_id.load(Ordering::Relaxed),
        start_ns: start,
        #[cfg(feature = "surface-profile")]
        thread_cpu_started: profiler.thread_cpu_timing_enabled.then(CpuStageTimer::new),
        active: true,
        _not_send: PhantomData,
    }
}

/// Selects a stable numeric worker lane for this thread until the returned
/// scope is dropped. Reusing an identity reuses its bounded lane.
pub fn worker_scope(worker_id: u64) -> WorkerScope {
    worker_scope_identity(WorkerIdentity::Numeric(worker_id))
}

/// Selects a stable static-name worker lane for this thread until dropped.
pub fn worker_scope_named(name: &'static str) -> WorkerScope {
    worker_scope_identity(WorkerIdentity::Named(name))
}

/// Installs a stable job identity on this thread until dropped. Callers must
/// carry identities explicitly with queued jobs so execution may occur on a
/// different thread or frame from the originating request.
pub fn job_scope(identity: ProfileJobIdentity) -> JobScope {
    let active = is_enabled() && identity.capture_id() == capture_id();
    let previous = THREAD_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let previous = state.job_identity;
        if active {
            state.job_identity = Some(identity);
        }
        previous
    });
    JobScope {
        previous,
        active,
        _not_send: PhantomData,
    }
}

pub struct JobScope {
    previous: Option<ProfileJobIdentity>,
    active: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for JobScope {
    fn drop(&mut self) {
        if self.active {
            THREAD_STATE.with(|state| state.borrow_mut().job_identity = self.previous);
        }
    }
}

fn worker_scope_identity(identity: WorkerIdentity) -> WorkerScope {
    let previous = THREAD_STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.thread_sequence == 0 {
            state.thread_sequence = profiler()
                .next_thread_sequence
                .fetch_add(1, Ordering::Relaxed);
        }
        let previous = *state;
        state.lane_id = if profiler().enabled.load(Ordering::Relaxed) {
            profiler().lane_for(identity, state.thread_sequence)
        } else {
            None
        };
        state.depth = 0;
        state.worker_scoped = true;
        previous
    });
    WorkerScope {
        previous,
        _not_send: PhantomData,
    }
}

/// RAII worker identity scope. The worker lane is registered only while
/// profiling is enabled.
pub struct WorkerScope {
    previous: ThreadState,
    _not_send: PhantomData<Rc<()>>,
}

impl Drop for WorkerScope {
    fn drop(&mut self) {
        THREAD_STATE.with(|state| *state.borrow_mut() = self.previous);
    }
}

/// RAII profile span. Dropping it publishes its event to its lane.
pub struct ProfileSpan {
    name: &'static str,
    lane_id: Option<usize>,
    event_id: u64,
    capture_id: u64,
    parent_event_id: Option<u64>,
    job_identity: Option<ProfileJobIdentity>,
    wait_reason: Option<ProfileWaitReason>,
    thread_sequence: u64,
    depth: usize,
    frame_id: u64,
    start_ns: u64,
    #[cfg(feature = "surface-profile")]
    thread_cpu_started: Option<CpuStageTimer>,
    active: bool,
    _not_send: PhantomData<Rc<()>>,
}

impl ProfileSpan {
    fn disabled() -> Self {
        Self {
            name: "",
            lane_id: None,
            event_id: 0,
            capture_id: 0,
            parent_event_id: None,
            job_identity: None,
            wait_reason: None,
            thread_sequence: 0,
            depth: 0,
            frame_id: 0,
            start_ns: 0,
            #[cfg(feature = "surface-profile")]
            thread_cpu_started: None,
            active: false,
            _not_send: PhantomData,
        }
    }
}

impl Drop for ProfileSpan {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let end_ns = now_ns();
        THREAD_STATE.with(|state| {
            let mut state = state.borrow_mut();
            if state.lane_id == self.lane_id {
                state.finish_span(self.depth, self.event_id);
            }
        });
        if self.capture_id != capture_id() {
            return;
        }
        let event = ProfileEvent {
            event_id: self.event_id,
            capture_id: self.capture_id,
            nesting_depth: self.depth,
            parent_event_id: self.parent_event_id,
            thread_sequence: self.thread_sequence,
            name: self.name,
            job_identity: self.job_identity,
            wait_reason: self.wait_reason,
            frame_id: self.frame_id,
            start_ns: self.start_ns,
            end_ns,
            duration_ns: end_ns.saturating_sub(self.start_ns),
            #[cfg(feature = "surface-profile")]
            thread_cpu_ns: self.thread_cpu_started.as_ref().and_then(|timer| {
                timer
                    .thread_elapsed()
                    .map(|duration| duration.as_nanos().min(u64::MAX as u128) as u64)
            }),
            #[cfg(not(feature = "surface-profile"))]
            thread_cpu_ns: None,
        };
        if let Some(lane) = self.lane_id.and_then(|id| profiler().lane(id)) {
            lane.record(event);
        }
        self.active = false;
    }
}

/// Copies a bounded timeline and aggregates rolling statistics.
pub fn snapshot() -> ProfileSnapshot {
    snapshot_impl(None)
}

/// Copies only events and rolling samples ending at or after `timestamp_ns`.
/// Timestamps use the same process-local monotonic origin as event timestamps.
pub fn snapshot_since(timestamp_ns: u64) -> ProfileSnapshot {
    snapshot_impl(Some(timestamp_ns))
}

fn snapshot_impl(since_ns: Option<u64>) -> ProfileSnapshot {
    let profiler = profiler();
    let now = now_ns();
    let lanes = lock(&profiler.lanes).clone();
    let mut lane_snapshots = Vec::with_capacity(lanes.len());
    let mut aggregates: Vec<Aggregate> = Vec::new();
    for lane in lanes {
        let data = lock(&lane.data);
        let mut events = Vec::with_capacity(data.events.len);
        data.events.copy_since(since_ns, &mut events);
        events.retain(|event| event.capture_id == capture_id());
        events.sort_unstable_by_key(|event| event.start_ns);
        lane_snapshots.push(ProfileLaneSnapshot {
            lane_id: lane.id,
            worker: lane.worker.clone(),
            registration_thread_sequence: lane.registration_thread_sequence,
            events,
            dropped_events: data.events.dropped,
            dropped_stat_names: data.dropped_stat_names,
        });
        for slot in &data.stats {
            let Some(name) = slot.name else { continue };
            let aggregate_index = if let Some(index) = aggregates
                .iter()
                .position(|aggregate| aggregate.name == name)
            {
                index
            } else {
                aggregates.push(Aggregate::new(name));
                aggregates.len() - 1
            };
            let aggregate = &mut aggregates[aggregate_index];
            for sample in slot.samples.iter().take(slot.len) {
                if since_ns.is_none_or(|since| sample.end_ns >= since) {
                    aggregate.push(sample.duration_ns, sample.end_ns);
                }
            }
        }
    }

    let stats = aggregates
        .into_iter()
        .filter_map(|aggregate| {
            let budget_ns = profiler.budget_for(aggregate.name);
            aggregate.finish(budget_ns)
        })
        .collect();
    ProfileSnapshot {
        schema_version: 2,
        snapshot_events_omitted: 0,
        capture_id: capture_id(),
        dropped_nesting: profiler.dropped_nesting.load(Ordering::Relaxed),
        enabled: is_enabled(),
        thread_cpu_timing_enabled: profiler.thread_cpu_timing_enabled,
        generated_at_ns: now,
        frame_id: profiler.frame_id.load(Ordering::Relaxed),
        since_ns,
        lanes: lane_snapshots,
        stats,
        dropped_lane_registrations: profiler.dropped_lane_registrations.load(Ordering::Relaxed),
    }
}

/// Clears event rings and rolling samples while keeping lane and budget
/// registration stable for already-running workers.
pub fn clear() {
    let profiler = profiler();
    profiler.capture_id.fetch_add(1, Ordering::Relaxed);
    profiler.dropped_nesting.store(0, Ordering::Relaxed);
    for lane in lock(&profiler.lanes).iter() {
        let mut data = lock(&lane.data);
        data.events.clear();
        data.dropped_stat_names = 0;
        for slot in &mut data.stats {
            slot.clear();
        }
    }
    profiler
        .dropped_lane_registrations
        .store(0, Ordering::Relaxed);
}

struct Aggregate {
    name: &'static str,
    samples: Vec<StatSample>,
}

impl Aggregate {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            samples: Vec::new(),
        }
    }

    fn push(&mut self, duration_ns: u64, end_ns: u64) {
        self.samples.push(StatSample {
            duration_ns,
            end_ns,
        });
    }

    fn finish(mut self, budget_ns: Option<u64>) -> Option<ProfileStat> {
        if self.samples.is_empty() {
            return None;
        }
        self.samples
            .sort_unstable_by_key(|sample| sample.duration_ns);
        let max_ns = self.samples.last().map_or(0, |sample| sample.duration_ns);
        let p50_ns = percentile_nearest_rank(&self.samples, 50);
        let p95_ns = percentile_nearest_rank(&self.samples, 95);
        let current_ns = self
            .samples
            .iter()
            .max_by_key(|sample| sample.end_ns)
            .map_or(0, |sample| sample.duration_ns);
        Some(ProfileStat {
            name: self.name,
            current_ns,
            p50_ns,
            p95_ns,
            max_ns,
            sample_count: self.samples.len() as u64,
            budget_ns,
        })
    }
}

fn percentile_nearest_rank(samples: &[StatSample], percentile: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let rank = (samples.len() * percentile).div_ceil(100).max(1);
    samples[rank - 1].duration_ns
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_lane() -> Lane {
        Lane::new(0, WorkerIdentity::Main, 1)
    }

    #[test]
    fn records_nested_spans_with_parent_and_frame() {
        let lane = local_lane();
        let parent = ProfileEvent {
            event_id: 10,
            capture_id: 1,
            nesting_depth: 0,
            parent_event_id: None,
            thread_sequence: 1,
            name: "parent",
            job_identity: None,
            wait_reason: None,
            frame_id: 42,
            start_ns: 1,
            end_ns: 9,
            duration_ns: 8,
            thread_cpu_ns: None,
        };
        let child = ProfileEvent {
            event_id: 11,
            capture_id: 1,
            nesting_depth: 0,
            parent_event_id: Some(10),
            thread_sequence: 1,
            name: "child",
            job_identity: None,
            wait_reason: None,
            frame_id: 42,
            start_ns: 3,
            end_ns: 6,
            duration_ns: 3,
            thread_cpu_ns: None,
        };
        lane.record(parent);
        lane.record(child);
        let data = lock(&lane.data);
        let mut events = Vec::new();
        data.events.copy_since(None, &mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].parent_event_id, Some(events[0].event_id));
        assert_eq!(events[1].frame_id, 42);
    }

    #[test]
    fn thread_state_links_real_nested_span_entries() {
        let mut state = ThreadState::new();
        assert_eq!(state.begin_span(101), Some((0, None)));
        assert_eq!(state.begin_span(102), Some((1, Some(101))));
        state.finish_span(1, 102);
        assert_eq!(state.depth, 1);
        state.finish_span(0, 101);
        assert_eq!(state.depth, 0);
    }

    #[test]
    fn event_ring_overwrites_oldest_and_counts_drops() {
        let mut ring = EventRing {
            entries: vec![None; 2],
            next: 0,
            len: 0,
            dropped: 0,
        };
        for event_id in 1..=3 {
            ring.push(ProfileEvent {
                event_id,
                capture_id: 1,
                nesting_depth: 0,
                parent_event_id: None,
                thread_sequence: 1,
                name: "bounded",
                job_identity: None,
                wait_reason: None,
                frame_id: 0,
                start_ns: event_id,
                end_ns: event_id,
                duration_ns: 0,
                thread_cpu_ns: None,
            });
        }
        let mut events = Vec::new();
        ring.copy_since(None, &mut events);
        assert_eq!(
            events
                .iter()
                .map(|event| event.event_id)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        assert_eq!(ring.dropped, 1);
    }

    #[test]
    fn main_lane_retains_more_timeline_events_than_worker_lanes() {
        let main = local_lane();
        let worker = Lane::new(1, WorkerIdentity::Named("worker"), 2);
        assert_eq!(lock(&main.data).events.entries.len(), MAIN_EVENT_CAPACITY);
        assert_eq!(
            lock(&worker.data).events.entries.len(),
            WORKER_EVENT_CAPACITY
        );
    }

    #[test]
    fn named_lane_is_reused_by_transient_threads_and_main_keeps_a_slot() {
        let profiler = Profiler::new();
        let first = profiler
            .lane_for(WorkerIdentity::Named("endpoint0"), 10)
            .unwrap();
        let next_batch = profiler
            .lane_for(WorkerIdentity::Named("endpoint0"), 11)
            .unwrap();
        assert_eq!(first, next_batch);

        for worker_id in 0..(MAX_LANES as u64) {
            profiler.lane_for(WorkerIdentity::Numeric(worker_id), worker_id + 20);
        }
        let main = profiler.lane_for(WorkerIdentity::Main, 99);
        assert!(main.is_some());
        assert_eq!(lock(&profiler.lanes).len(), MAX_LANES);
    }

    #[test]
    fn percentile_uses_nearest_rank_over_rolling_samples() {
        let mut aggregate = Aggregate::new("latency");
        for duration_ns in [1, 2, 3, 4, 100] {
            aggregate.push(duration_ns, duration_ns);
        }
        let stat = aggregate.finish(Some(90)).unwrap();
        assert_eq!(stat.current_ns, 100);
        assert_eq!(stat.p50_ns, 3);
        assert_eq!(stat.p95_ns, 100);
        assert_eq!(stat.max_ns, 100);
        assert_eq!(stat.sample_count, 5);
        assert_eq!(stat.budget_ns, Some(90));
    }

    #[test]
    fn disabled_span_is_inert() {
        let span = ProfileSpan::disabled();
        assert!(!span.active);
    }

    #[test]
    fn stable_job_identity_can_cross_threads_and_frames() {
        let identity = ProfileJobIdentity::Tile {
            job_id: 9,
            capture_id: 1,
            origin_frame_id: 11,
            dispatch_frame_id: 12,
            body_identity: 7,
            surface_revision: 3,
            material_revision: 4,
            radius_bits: 99,
            face: 0,
            level: 5,
            x: 8,
            y: 11,
            cells: 64,
            format_version: 1,
            filter_version: 1,
        };
        let worker_identity = std::thread::spawn(move || {
            let mut worker_state = ThreadState::new();
            worker_state.job_identity = Some(identity);
            worker_state.job_identity
        })
        .join()
        .unwrap();
        assert_eq!(worker_identity, Some(identity));

        let encoded = serde_json::to_value(identity).unwrap();
        assert_eq!(encoded["kind"], "tile");
        assert_eq!(encoded["body_identity"], 7);
        assert_eq!(encoded["surface_revision"], 3);
        assert_eq!(encoded["level"], 5);
    }

    #[test]
    fn wait_reason_is_a_typed_export_value() {
        let encoded = serde_json::to_value(ProfileWaitReason::TerrainWorkerQueue).unwrap();
        assert_eq!(encoded, "terrain_worker_queue");
    }

    #[test]
    fn real_worker_scope_restores_job_on_unwind_and_rejects_old_capture_context() {
        let previous_enabled = is_enabled();
        set_enabled(true);
        let identity = ProfileJobIdentity::boundary_batch(next_job_id(), 31, 32, 7, 3, [5, 6]);
        let captured = std::thread::spawn(move || {
            let _lane = worker_scope_named("test scoped worker");
            let _job = job_scope(identity);
            let _ = std::panic::catch_unwind(|| {
                let other = ProfileJobIdentity::boundary_batch(next_job_id(), 41, 42, 8, 1, []);
                let _nested_job = job_scope(other);
                let _span = span("test nested worker");
                panic!("synthetic unwind");
            });
            assert_eq!(current_job_identity(), Some(identity));
            begin_frame(99);
            let _span = span("test scoped execution");
            identity
        })
        .join()
        .unwrap();
        let captured_id = match captured {
            ProfileJobIdentity::BoundaryBatch { job_id, .. } => job_id,
            _ => unreachable!(),
        };
        let snap = snapshot();
        assert!(
            snap.lanes
                .iter()
                .flat_map(|lane| &lane.events)
                .any(|event| event.name == "test scoped execution"
                    && event.frame_id == 99
                    && event.job_identity == Some(captured))
        );
        clear();
        assert!(next_job_id() > captured_id);
        let _old = job_scope(captured);
        assert_eq!(current_job_identity(), None);
        set_enabled(previous_enabled);
    }
}
