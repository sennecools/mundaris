//! Optional timestamp query capability detection and measured scope names.

/// Whether adapter features permit timestamp-query measurements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampAvailability {
    Unavailable,
    WholePassOnly,
    InsidePasses,
}

/// CPU-side surface upload work; durations are host wall time, never GPU time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuUploadProfile {
    pub bytes_uploaded: u64,
    pub resident_capacity_bytes: u64,
    pub buffer_growth_events: u32,
    pub growth_wait_events: u32,
    pub growth_wait: std::time::Duration,
    pub upload_api_duration: std::time::Duration,
}

/// GPU-only elapsed times from timestamp queries. Per-draw categories require
/// `TIMESTAMP_QUERY_INSIDE_PASSES`; otherwise only pass totals are populated.
/// `None` means the scope is unavailable or has not completed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuProfile {
    /// Complete encoded native GPU frame body, including debug, celestial,
    /// and editor-UI rendering. Surface acquisition, CPU work, present, and
    /// timestamp readback are outside this interval.
    pub frame: Option<std::time::Duration>,
    /// Complete main celestial render pass.
    pub celestial_pass: Option<std::time::Duration>,
    /// Complete guides/overlay render pass.
    pub overlay_pass: Option<std::time::Duration>,
    /// Mesh and resident surface draws, including the regional Moon terrain path.
    pub terrain: Option<std::time::Duration>,
    /// Transition fallback surface draws inside the celestial scene pass.
    pub transition_fallback: Option<std::time::Duration>,
    pub remaining_celestial: Option<std::time::Duration>,
    /// Distant background and finite-star draws only, excluding upload/readback.
    pub sky: Option<std::time::Duration>,
    /// All sun shadow cascade passes.
    pub shadows: Option<std::time::Duration>,
    /// Ground-truth ambient occlusion pass.
    pub ao: Option<std::time::Duration>,
    /// Ambient × AO composite into the HDR target.
    pub ao_composite: Option<std::time::Duration>,
    /// Luminance histogram and adaptation.
    pub exposure: Option<std::time::Duration>,
    /// Bloom down/upsample chain.
    pub bloom: Option<std::time::Duration>,
    /// Exposure, tonemap and dither into the scene texture.
    pub tonemap: Option<std::time::Duration>,
    /// Measured scopes placed relative to the start of the sampled GPU frame
    /// (the frame scope when present, else the earliest scope), in nesting order.
    pub scopes: [Option<GpuScopeSpan>; GPU_SCOPE_SLOTS],
}

/// One timestamped GPU scope on the sampled frame's own GPU clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpuScopeSpan {
    pub name: &'static str,
    /// 0 frame, 1 render pass, 2 scope inside a pass.
    pub depth: u8,
    pub start: std::time::Duration,
    pub end: std::time::Duration,
}

pub const GPU_SCOPE_SLOTS: usize = 12;

/// Query pair (and mask bit) of each timed pass or scope. Pairs 1, 5 and 6 are
/// unassigned; 4 is the retired transition-fallback scope.
pub(crate) mod pair {
    pub const SCENE: usize = 0;
    pub const OVERLAY: usize = 2;
    pub const TERRAIN: usize = 3;
    pub const SPHERES: usize = 7;
    pub const SKY: usize = 8;
    pub const SHADOWS: usize = 10;
    pub const AO: usize = 11;
    pub const AO_COMPOSITE: usize = 12;
    pub const EXPOSURE: usize = 13;
    pub const BLOOM: usize = 14;
    pub const TONEMAP: usize = 15;
}

/// (mask bit and query pair, name, depth) of every scope the timeline shows,
/// in submission order.
const TIMELINE_SCOPES: [(usize, &str, u8); GPU_SCOPE_SLOTS] = [
    (FRAME_QUERY_PAIR, "GPU frame", 0),
    (pair::SHADOWS, "Shadows", 1),
    (pair::SCENE, "Scene pass", 1),
    (pair::SKY, "Sky", 2),
    (pair::SPHERES, "Body spheres", 2),
    (pair::TERRAIN, "Terrain", 2),
    (pair::AO, "GTAO", 1),
    (pair::AO_COMPOSITE, "AO composite", 1),
    (pair::EXPOSURE, "Exposure", 1),
    (pair::BLOOM, "Bloom", 1),
    (pair::TONEMAP, "Tonemap", 1),
    (pair::OVERLAY, "Overlay pass", 1),
];

fn scope_spans(
    ticks: &[u64],
    period_nanoseconds: f32,
    scope_mask: u32,
) -> [Option<GpuScopeSpan>; GPU_SCOPE_SLOTS] {
    let pair = |pair: usize| {
        let start = *ticks.get(pair * 2)?;
        let end = *ticks.get(pair * 2 + 1)?;
        (scope_mask & (1u32 << pair) != 0 && end >= start).then_some((start, end))
    };
    let origin = pair(FRAME_QUERY_PAIR).map(|(start, _)| start).or_else(|| {
        TIMELINE_SCOPES
            .iter()
            .filter_map(|&(index, ..)| pair(index).map(|(start, _)| start))
            .min()
    });
    let to_duration = |ticks: u64| {
        let ns = ticks as f64 * f64::from(period_nanoseconds);
        (ns.is_finite() && ns >= 0.0).then(|| std::time::Duration::from_nanos(ns.round() as u64))
    };
    TIMELINE_SCOPES.map(|(index, name, depth)| {
        let origin = origin?;
        let (start, end) = pair(index)?;
        if index == 8 && start == end {
            return None; // Cold-query sky sentinel, as in `decode`.
        }
        // A scope before the frame origin is not comparable on this clock.
        Some(GpuScopeSpan {
            name,
            depth,
            start: to_duration(start.checked_sub(origin)?)?,
            end: to_duration(end.checked_sub(origin)?)?,
        })
    })
}

/// Low-cost counters describing timestamp query requests and asynchronous
/// readback. Submission IDs identify actual query sources; a busy skip records
/// the candidate frame ID and the still-pending source without attributing any
/// application work to that skipped frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimestampProfilingMetrics {
    pub explicit_requests: u64,
    pub eligible_frames: u64,
    pub accepted_reservations: u64,
    pub actual_submissions: u64,
    pub valid_completions: u64,
    pub busy_skips: u64,
    pub map_failures: u64,
    pub decode_failures: u64,
    pub invalid_samples: u64,
    pub last_busy_skip_candidate_submission_id: Option<u64>,
    pub last_busy_skip_source_submission_id: Option<u64>,
    pub last_submitted_source_submission_id: Option<u64>,
    pub last_completed_source_submission_id: Option<u64>,
    pub last_map_failure_source_submission_id: Option<u64>,
    pub last_decode_failure_source_submission_id: Option<u64>,
    pub last_invalid_sample_source_submission_id: Option<u64>,
}

impl TimestampProfilingMetrics {
    #[cfg(any(feature = "developer-tools", test))]
    fn explicit_request(&mut self) {
        self.explicit_requests = self.explicit_requests.saturating_add(1);
    }

    fn eligible_frame(&mut self) {
        self.eligible_frames = self.eligible_frames.saturating_add(1);
    }

    fn accepted_reservation(&mut self) {
        self.accepted_reservations = self.accepted_reservations.saturating_add(1);
    }

    fn busy_skip(&mut self, candidate: Option<u64>, source: Option<u64>) {
        self.busy_skips = self.busy_skips.saturating_add(1);
        self.last_busy_skip_candidate_submission_id = candidate;
        self.last_busy_skip_source_submission_id = source;
    }

    fn submitted(&mut self, source: u64) {
        self.actual_submissions = self.actual_submissions.saturating_add(1);
        self.last_submitted_source_submission_id = Some(source);
    }

    fn mapped(&mut self, source: u64, has_valid_scope: bool, has_invalid_scope: bool) {
        self.last_completed_source_submission_id = Some(source);
        if has_valid_scope && !has_invalid_scope {
            self.valid_completions = self.valid_completions.saturating_add(1);
        }
        if has_invalid_scope {
            self.invalid_samples = self.invalid_samples.saturating_add(1);
            self.last_invalid_sample_source_submission_id = Some(source);
        }
        if !has_valid_scope || has_invalid_scope {
            self.decode_failures = self.decode_failures.saturating_add(1);
            self.last_decode_failure_source_submission_id = Some(source);
        }
    }

    fn map_failed(&mut self, source: Option<u64>) {
        self.map_failures = self.map_failures.saturating_add(1);
        self.invalid_samples = self.invalid_samples.saturating_add(1);
        self.last_map_failure_source_submission_id = source;
        self.last_invalid_sample_source_submission_id = source;
    }
}

fn scope_decode_status(ticks: &[u64], period_nanoseconds: f32, scope_mask: u32) -> (bool, bool) {
    let mut has_valid_scope = false;
    let mut has_invalid_scope = false;
    for pair in 0..QUERY_PAIRS {
        if scope_mask & (1 << pair) == 0 {
            continue;
        }
        let Some(start) = ticks.get(pair * 2).copied() else {
            has_invalid_scope = true;
            continue;
        };
        let Some(end) = ticks.get(pair * 2 + 1).copied() else {
            has_invalid_scope = true;
            continue;
        };
        let Some(elapsed) = end.checked_sub(start) else {
            has_invalid_scope = true;
            continue;
        };
        let elapsed_ns = elapsed as f64 * f64::from(period_nanoseconds);
        if !elapsed_ns.is_finite() || elapsed_ns < 0.0 {
            has_invalid_scope = true;
        } else if pair == 8 && elapsed_ns == 0.0 {
            // Zero sky time is the existing cold-query sentinel, not a bad sample.
        } else {
            has_valid_scope = true;
        }
    }
    (has_valid_scope, has_invalid_scope)
}

pub(crate) fn decode(ticks: &[u64], period_nanoseconds: f32, scope_mask: u32) -> GpuProfile {
    let duration = |pair: usize| {
        let start = *ticks.get(pair * 2)?;
        let end = *ticks.get(pair * 2 + 1)?;
        let elapsed = end.checked_sub(start)? as f64 * f64::from(period_nanoseconds);
        (elapsed.is_finite() && elapsed >= 0.0)
            .then(|| std::time::Duration::from_nanos(elapsed.round() as u64))
    };
    let scoped = |pair: usize| {
        (scope_mask & (1u32 << pair) != 0)
            .then(|| duration(pair))
            .flatten()
    };
    GpuProfile {
        frame: scoped(FRAME_QUERY_PAIR),
        celestial_pass: scoped(pair::SCENE),
        overlay_pass: scoped(pair::OVERLAY),
        terrain: scoped(pair::TERRAIN),
        transition_fallback: scoped(4),
        remaining_celestial: scoped(pair::SPHERES),
        // A zero elapsed interval cannot establish sky work on the cold query;
        // retain unavailable semantics rather than publishing a fabricated win.
        sky: scoped(pair::SKY).filter(|elapsed| !elapsed.is_zero()),
        shadows: scoped(pair::SHADOWS),
        ao: scoped(pair::AO),
        ao_composite: scoped(pair::AO_COMPOSITE),
        exposure: scoped(pair::EXPOSURE),
        bloom: scoped(pair::BLOOM),
        tonemap: scoped(pair::TONEMAP),
        scopes: scope_spans(ticks, period_nanoseconds, scope_mask),
    }
}

/// Timestamp-query capability detected before device creation.
pub(crate) fn available_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let supported = adapter.features();
    let mut requested = wgpu::Features::empty();
    if supported.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested |= wgpu::Features::TIMESTAMP_QUERY;
        if supported.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
            requested |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        }
        if supported.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES) {
            requested |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES;
        }
    }
    requested
}

pub(crate) fn availability(features: wgpu::Features) -> TimestampAvailability {
    if !features.contains(wgpu::Features::TIMESTAMP_QUERY) {
        TimestampAvailability::Unavailable
    } else if features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES) {
        TimestampAvailability::InsidePasses
    } else {
        TimestampAvailability::WholePassOnly
    }
}

/// Queries for three pass totals, six scopes inside the main pass, and the
/// complete encoded native frame.
pub(crate) struct CelestialQueries {
    pub set: wgpu::QuerySet,
    inside_passes: bool,
    inside_encoders: bool,
}

impl CelestialQueries {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| Self {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("Mundaris native frame and celestial timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: QUERY_COUNT,
                }),
                inside_passes: device
                    .features()
                    .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
                inside_encoders: device
                    .features()
                    .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
            })
    }

    pub fn pass_writes(&self, pass: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some((pass * 2) as u32),
            end_of_pass_write_index: Some((pass * 2 + 1) as u32),
        }
    }

    /// Begin-only or end-only writes let one scope span several passes.
    pub fn pass_writes_partial(
        &self,
        pass: usize,
        begin: bool,
        end: bool,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        (begin || end).then(|| wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: begin.then_some((pass * 2) as u32),
            end_of_pass_write_index: end.then_some((pass * 2 + 1) as u32),
        })
    }

    pub fn compute_writes(
        &self,
        pass: usize,
        begin: bool,
        end: bool,
    ) -> Option<wgpu::ComputePassTimestampWrites<'_>> {
        (begin || end).then(|| wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: begin.then_some((pass * 2) as u32),
            end_of_pass_write_index: end.then_some((pass * 2 + 1) as u32),
        })
    }

    pub fn inside_passes(&self) -> bool {
        self.inside_passes
    }

    pub fn inside_encoders(&self) -> bool {
        self.inside_encoders
    }

    pub fn write_scope(&self, pass: &mut wgpu::RenderPass<'_>, pair: usize) {
        if self.inside_passes {
            pass.write_timestamp(&self.set, (pair * 2) as u32);
        }
    }

    pub fn end_scope(&self, pass: &mut wgpu::RenderPass<'_>, pair: usize) {
        if self.inside_passes {
            pass.write_timestamp(&self.set, (pair * 2 + 1) as u32);
        }
    }
}

pub(crate) const FRAME_QUERY_PAIR: usize = 9;
pub(crate) const FRAME_SCOPE_BIT: u32 = 1 << FRAME_QUERY_PAIR;
const QUERY_PAIRS: usize = 16;
pub(crate) const QUERY_COUNT: u32 = (QUERY_PAIRS * 2) as u32;

/// Controls automatic timestamp sampling while developer observation is active.
/// Ordinary renderer use remains continuously sampled as before.
#[cfg(feature = "developer-tools")]
#[derive(Debug, Default)]
pub(crate) struct DeveloperTimestampGate {
    observation_mode: bool,
    requested: bool,
}

#[cfg(feature = "developer-tools")]
impl DeveloperTimestampGate {
    pub fn set_observation_mode(&mut self, enabled: bool) {
        self.observation_mode = enabled;
    }

    pub fn observation_mode(&self) -> bool {
        self.observation_mode
    }

    pub fn request(&mut self) {
        self.requested = true;
    }

    pub fn wants_sample(&self, has_celestial_frame: bool) -> bool {
        has_celestial_frame && (!self.observation_mode || self.requested)
    }

    /// True only when a celestial frame can start in an idle query slot.
    pub fn begin_if_idle(&self, has_celestial_frame: bool, slot_idle: bool) -> bool {
        self.wants_sample(has_celestial_frame) && slot_idle
    }

    /// Retain the request through skips and preparation errors; consume it only
    /// after a timestamp-bearing frame has actually been submitted.
    pub fn submitted(&mut self) {
        self.requested = false;
    }
}

/// One bounded native readback slot. A pending mapping makes the next frame skip
/// query use rather than waiting for or overwriting the in-flight result.
pub(crate) struct AsyncTimestampSlot {
    pub queries: CelestialQueries,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    receiver: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
    pub latest: GpuProfile,
    latest_submission_id: Option<u64>,
    pending_submission_id: Option<u64>,
    period_nanoseconds: f32,
    scope_mask: u32,
    metrics: TimestampProfilingMetrics,
}

impl AsyncTimestampSlot {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        let queries = CelestialQueries::new(device)?;
        let size = u64::from(QUERY_COUNT) * 8;
        Some(Self {
            queries,
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mundaris timestamp resolve"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mundaris timestamp readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            receiver: None,
            latest: GpuProfile::default(),
            latest_submission_id: None,
            pending_submission_id: None,
            period_nanoseconds: 1.0,
            scope_mask: 0,
            metrics: TimestampProfilingMetrics::default(),
        })
    }

    pub fn available(&mut self) -> bool {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    let Ok(mapped) = self.readback.get_mapped_range(..) else {
                        self.pending_submission_id = None;
                        self.receiver = None;
                        return false;
                    };
                    let ticks: Vec<u64> = mapped
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|chunk| u64::from_le_bytes(*chunk))
                        .collect();
                    self.latest = decode(&ticks, self.period_nanoseconds, self.scope_mask);
                    let (has_valid_scope, has_invalid_scope) =
                        scope_decode_status(&ticks, self.period_nanoseconds, self.scope_mask);
                    let source_submission_id = self.pending_submission_id.take();
                    self.latest_submission_id = source_submission_id;
                    if let Some(source_submission_id) = source_submission_id {
                        self.metrics.mapped(
                            source_submission_id,
                            has_valid_scope,
                            has_invalid_scope,
                        );
                    } else {
                        self.metrics.decode_failures =
                            self.metrics.decode_failures.saturating_add(1);
                        self.metrics.invalid_samples =
                            self.metrics.invalid_samples.saturating_add(1);
                    }
                    drop(mapped);
                    self.readback.unmap();
                    self.receiver = None;
                }
                Ok(Err(_)) => {
                    self.metrics.map_failed(self.pending_submission_id);
                    self.pending_submission_id = None;
                    self.receiver = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.metrics.map_failed(self.pending_submission_id);
                    self.pending_submission_id = None;
                    self.receiver = None;
                }
            }
        }
        self.receiver.is_none()
    }

    pub fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.resolve_query_set(&self.queries.set, 0..QUERY_COUNT, &self.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.readback,
            0,
            u64::from(QUERY_COUNT) * 8,
        );
    }

    pub fn map(&mut self, period_nanoseconds: f32, scope_mask: u32, submission_id: u64) {
        self.period_nanoseconds = period_nanoseconds;
        self.scope_mask = scope_mask;
        self.pending_submission_id = Some(submission_id);
        self.metrics.submitted(submission_id);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        self.receiver = Some(receiver);
    }

    pub fn latest_submission_id(&self) -> Option<u64> {
        self.latest_submission_id
    }

    pub fn metrics(&self) -> TimestampProfilingMetrics {
        self.metrics
    }

    #[cfg(feature = "developer-tools")]
    pub fn record_explicit_request(&mut self) {
        self.metrics.explicit_request();
    }

    pub fn record_eligible_frame(&mut self) {
        self.metrics.eligible_frame();
    }

    pub fn record_accepted_reservation(&mut self) {
        self.metrics.accepted_reservation();
    }

    pub fn record_busy_skip(&mut self, candidate: Option<u64>) {
        self.metrics
            .busy_skip(candidate, self.pending_submission_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_exposes_available_celestial_and_inside_pass_scopes() {
        let profile = decode(&(0..20).collect::<Vec<u64>>(), 2.0, 0b11_1111_1111);
        let two_ns = Some(std::time::Duration::from_nanos(2));
        assert_eq!(profile.frame, two_ns);
        assert_eq!(profile.celestial_pass, two_ns);
        assert_eq!(profile.terrain, two_ns);
        assert_eq!(profile.sky, two_ns);
    }

    #[test]
    fn scopes_are_placed_relative_to_the_frame_start() {
        let mut ticks = vec![0u64; QUERY_COUNT as usize];
        let mut set = |pair: usize, start: u64, end: u64| {
            ticks[pair * 2] = start;
            ticks[pair * 2 + 1] = end;
        };
        set(FRAME_QUERY_PAIR, 1000, 1400);
        set(0, 1010, 1300); // scene pass
        set(3, 1100, 1250); // terrain inside it
        set(2, 1300, 1390); // overlay pass
        set(7, 900, 950); // sphere scope, masked out below
        let mask = FRAME_SCOPE_BIT | 1 | (1 << 2) | (1 << 3);
        let scopes = decode(&ticks, 2.0, mask).scopes;
        let get = |name| scopes.iter().flatten().find(|s| s.name == name).copied();
        let ns = std::time::Duration::from_nanos;
        assert_eq!(
            get("GPU frame").map(|s| (s.start, s.end)),
            Some((ns(0), ns(800)))
        );
        assert_eq!(
            get("Terrain").map(|s| (s.start, s.end, s.depth)),
            Some((ns(200), ns(500), 2))
        );
        assert_eq!(get("Overlay pass").map(|s| s.start), Some(ns(600)));
        assert!(get("Body spheres").is_none(), "unmasked scope ignored");
        assert!(get("Sky").is_none());

        // Without the frame scope the earliest scope is the origin.
        let scopes = decode(&ticks, 1.0, 1 | (1 << 3)).scopes;
        let get = |name| scopes.iter().flatten().find(|s| s.name == name).copied();
        assert_eq!(get("Scene pass").map(|s| s.start), Some(ns(0)));
        assert_eq!(get("Terrain").map(|s| s.start), Some(ns(90)));
    }

    #[test]
    fn absent_scope_mask_ignores_stale_query_values() {
        let profile = decode(&(0..20).collect::<Vec<u64>>(), 1.0, 1);
        assert_eq!(
            profile.celestial_pass,
            Some(std::time::Duration::from_nanos(1))
        );
        assert_eq!(profile.terrain, None);
        assert_eq!(profile.transition_fallback, None);
        assert_eq!(profile.sky, None);
        assert_eq!(profile.frame, None);
    }

    #[test]
    fn reversed_or_missing_timestamp_pair_is_unavailable() {
        assert_eq!(decode(&[20, 10], 1.0, 1).celestial_pass, None);
        assert_eq!(decode(&[], 1.0, 1).celestial_pass, None);
    }

    #[test]
    fn frame_requires_its_mask_bit_and_a_valid_complete_pair() {
        let mut ticks = (0..20).collect::<Vec<u64>>();
        assert_eq!(decode(&ticks, 1.0, 0).frame, None);
        assert_eq!(
            decode(&ticks, 2.0, FRAME_SCOPE_BIT).frame,
            Some(std::time::Duration::from_nanos(2))
        );

        ticks[FRAME_QUERY_PAIR * 2 + 1] = ticks[FRAME_QUERY_PAIR * 2];
        assert_eq!(
            decode(&ticks, 1.0, FRAME_SCOPE_BIT).frame,
            Some(std::time::Duration::ZERO)
        );

        ticks[FRAME_QUERY_PAIR * 2 + 1] = ticks[FRAME_QUERY_PAIR * 2] - 1;
        assert_eq!(decode(&ticks, 1.0, FRAME_SCOPE_BIT).frame, None);
        assert_eq!(
            decode(&ticks[..ticks.len() - 1], 1.0, FRAME_SCOPE_BIT).frame,
            None
        );
    }

    #[test]
    fn profiling_metrics_keep_requests_reservations_and_submissions_distinct() {
        let mut metrics = TimestampProfilingMetrics::default();
        metrics.explicit_request();
        metrics.eligible_frame();
        metrics.accepted_reservation();
        metrics.busy_skip(Some(13), Some(12));
        metrics.submitted(13);

        assert_eq!(metrics.explicit_requests, 1);
        assert_eq!(metrics.eligible_frames, 1);
        assert_eq!(metrics.accepted_reservations, 1);
        assert_eq!(metrics.actual_submissions, 1);
        assert_eq!(metrics.busy_skips, 1);
        assert_eq!(metrics.last_busy_skip_candidate_submission_id, Some(13));
        assert_eq!(metrics.last_busy_skip_source_submission_id, Some(12));
        assert_eq!(metrics.last_submitted_source_submission_id, Some(13));
    }

    #[test]
    fn partially_invalid_active_scopes_do_not_count_as_valid_completion() {
        let mut ticks = (0..QUERY_COUNT as u64).collect::<Vec<_>>();
        ticks[3] = ticks[2] - 1;
        let (has_valid_scope, has_invalid_scope) = scope_decode_status(&ticks, 1.0, 0b11);
        assert!(has_valid_scope);
        assert!(has_invalid_scope);

        let mut metrics = TimestampProfilingMetrics::default();
        metrics.mapped(24, has_valid_scope, has_invalid_scope);
        assert_eq!(metrics.valid_completions, 0);
        assert_eq!(metrics.decode_failures, 1);
        assert_eq!(metrics.invalid_samples, 1);
        assert_eq!(metrics.last_decode_failure_source_submission_id, Some(24));
        assert_eq!(metrics.last_invalid_sample_source_submission_id, Some(24));
    }

    #[test]
    fn zero_timestamp_values_are_valid_except_for_sky_scope() {
        let zero_ticks = [0; QUERY_COUNT as usize];
        let frame = decode(&zero_ticks, 1.0, FRAME_SCOPE_BIT);
        assert_eq!(frame.frame, Some(std::time::Duration::ZERO));
        assert_eq!(
            scope_decode_status(&zero_ticks, 1.0, FRAME_SCOPE_BIT),
            (true, false)
        );

        let sky = decode(&zero_ticks, 1.0, 1 << 8);
        assert_eq!(sky.sky, None);
        assert_eq!(
            scope_decode_status(&zero_ticks, 1.0, 1 << 8),
            (false, false)
        );

        let mut reversed_ticks = zero_ticks;
        reversed_ticks[0] = 1;
        assert_eq!(scope_decode_status(&reversed_ticks, 1.0, 1), (false, true));
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn observation_gate_waits_for_celestial_idle_slot_and_submitted_sample() {
        let mut gate = DeveloperTimestampGate::default();
        assert!(gate.begin_if_idle(true, true));

        gate.set_observation_mode(true);
        assert!(!gate.wants_sample(true));
        assert!(!gate.wants_sample(false));
        assert!(!gate.begin_if_idle(true, true));
        gate.request();
        assert!(gate.wants_sample(true));
        assert!(!gate.begin_if_idle(false, true));
        assert!(!gate.begin_if_idle(true, false));
        assert!(gate.begin_if_idle(true, true));

        // Merely being eligible does not consume it: surface acquisition or
        // frame preparation may still skip before submission.
        assert!(gate.begin_if_idle(true, true));
        gate.submitted();
        assert!(!gate.begin_if_idle(true, true));
    }
}
