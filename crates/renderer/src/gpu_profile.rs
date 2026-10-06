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
    /// Complete main celestial render pass.
    pub celestial_pass: Option<std::time::Duration>,
    /// Complete atmosphere render pass.
    pub atmosphere_pass: Option<std::time::Duration>,
    /// Complete guides/overlay render pass.
    pub overlay_pass: Option<std::time::Duration>,
    pub terrain: Option<std::time::Duration>,
    pub transition_fallback: Option<std::time::Duration>,
    pub ocean: Option<std::time::Duration>,
    pub clouds: Option<std::time::Duration>,
    /// Same measurement as `atmosphere_pass`, also available under the layer name.
    pub atmosphere: Option<std::time::Duration>,
    pub remaining_celestial: Option<std::time::Duration>,
    /// Distant background and finite-star draws only, excluding upload/readback.
    pub sky: Option<std::time::Duration>,
}

pub(crate) fn decode(ticks: &[u64], period_nanoseconds: f32, scope_mask: u16) -> GpuProfile {
    let duration = |pair: usize| {
        let start = *ticks.get(pair * 2)?;
        let end = *ticks.get(pair * 2 + 1)?;
        let elapsed = end.checked_sub(start)? as f64 * f64::from(period_nanoseconds);
        (elapsed.is_finite() && elapsed >= 0.0)
            .then(|| std::time::Duration::from_nanos(elapsed.round() as u64))
    };
    let scoped = |bit, pair| {
        (scope_mask & (1u16 << bit) != 0u16)
            .then(|| duration(pair))
            .flatten()
    };
    GpuProfile {
        celestial_pass: scoped(0, 0),
        atmosphere_pass: scoped(1, 1),
        overlay_pass: scoped(2, 2),
        terrain: scoped(3, 3),
        transition_fallback: scoped(4, 4),
        ocean: scoped(5, 5),
        clouds: scoped(6, 6),
        atmosphere: scoped(1, 1),
        remaining_celestial: scoped(7, 7),
        // A zero elapsed interval cannot establish sky work on the cold query;
        // retain unavailable semantics rather than publishing a fabricated win.
        sky: scoped(8, 8).filter(|elapsed| !elapsed.is_zero()),
    }
}

/// Timestamp-query capability detected before device creation.
pub(crate) fn available_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let supported = adapter.features();
    let mut requested = wgpu::Features::empty();
    if supported.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested |= wgpu::Features::TIMESTAMP_QUERY;
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

/// Queries for three pass totals and six scopes inside the main pass.
pub(crate) struct CelestialQueries {
    pub set: wgpu::QuerySet,
    inside_passes: bool,
}

impl CelestialQueries {
    pub fn new(device: &wgpu::Device) -> Option<Self> {
        device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| Self {
                set: device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("Mundaris celestial timestamps"),
                    ty: wgpu::QueryType::Timestamp,
                    count: QUERY_COUNT,
                }),
                inside_passes: device
                    .features()
                    .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
            })
    }

    pub fn pass_writes(&self, pass: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some((pass * 2) as u32),
            end_of_pass_write_index: Some((pass * 2 + 1) as u32),
        }
    }

    pub fn inside_passes(&self) -> bool {
        self.inside_passes
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

pub(crate) const QUERY_COUNT: u32 = 18;

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

    /// True only when a celestial frame can start in an idle query slot.
    pub fn begin_if_idle(&self, has_celestial_frame: bool, slot_idle: bool) -> bool {
        has_celestial_frame && slot_idle && (!self.observation_mode || self.requested)
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
    scope_mask: u16,
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
        })
    }

    pub fn available(&mut self) -> bool {
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    let mapped = self.readback.get_mapped_range(..);
                    let ticks: Vec<u64> = mapped
                        .as_chunks::<8>()
                        .0
                        .iter()
                        .map(|chunk| u64::from_le_bytes(*chunk))
                        .collect();
                    self.latest = decode(&ticks, self.period_nanoseconds, self.scope_mask);
                    self.latest_submission_id = self.pending_submission_id.take();
                    drop(mapped);
                    self.readback.unmap();
                    self.receiver = None;
                }
                Ok(Err(_)) => {
                    self.pending_submission_id = None;
                    self.receiver = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
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

    pub fn map(&mut self, period_nanoseconds: f32, scope_mask: u16, submission_id: u64) {
        self.period_nanoseconds = period_nanoseconds;
        self.scope_mask = scope_mask;
        self.pending_submission_id = Some(submission_id);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_exposes_available_pass_and_inside_pass_scopes() {
        let profile = decode(&(0..18).collect::<Vec<u64>>(), 2.0, 0b1_1111_1111);
        let two_ns = Some(std::time::Duration::from_nanos(2));
        assert_eq!(profile.celestial_pass, two_ns);
        assert_eq!(profile.atmosphere_pass, two_ns);
        assert_eq!(profile.terrain, two_ns);
        assert_eq!(profile.ocean, two_ns);
        assert_eq!(profile.clouds, two_ns);
        assert_eq!(profile.sky, two_ns);
    }

    #[test]
    fn absent_scope_mask_ignores_stale_query_values() {
        let profile = decode(&(0..16).collect::<Vec<u64>>(), 1.0, 1);
        assert_eq!(
            profile.celestial_pass,
            Some(std::time::Duration::from_nanos(1))
        );
        assert_eq!(profile.atmosphere_pass, None);
        assert_eq!(profile.terrain, None);
        assert_eq!(profile.transition_fallback, None);
        assert_eq!(profile.sky, None);
    }

    #[test]
    fn reversed_or_missing_timestamp_pair_is_unavailable() {
        assert_eq!(decode(&[20, 10], 1.0, 1).celestial_pass, None);
        assert_eq!(decode(&[], 1.0, 1).celestial_pass, None);
    }

    #[cfg(feature = "developer-tools")]
    #[test]
    fn observation_gate_waits_for_celestial_idle_slot_and_submitted_sample() {
        let mut gate = DeveloperTimestampGate::default();
        assert!(gate.begin_if_idle(true, true));

        gate.set_observation_mode(true);
        assert!(!gate.begin_if_idle(true, true));
        gate.request();
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
