//! Bounded pre/post-event telemetry, serialized by a disposable background writer.
use crate::developer_snapshot::{DeveloperSnapshot, PerformanceSnapshot};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufWriter, Write},
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, SyncSender},
    time::{Duration, Instant},
};

#[cfg(feature = "developer-tools")]
use sha2::{Digest, Sha256};
#[cfg(feature = "developer-tools")]
use std::io::Read;

/// One outstanding sampled timeline export. Sorting and JSON construction stay
/// on the diagnostic worker; the frame owner never waits for its result.
#[derive(Default)]
pub struct ProfileSampler {
    sender: Option<SyncSender<()>>,
    receiver: Option<mpsc::Receiver<ProfileSample>>,
    outstanding: bool,
}

#[derive(Debug)]
enum ProfileSample {
    Ready(Value),
    SerializationFailed(String),
    WorkerUnavailable,
}

impl ProfileSampler {
    pub fn poll_value(&mut self) -> Option<Value> {
        match self.poll()? {
            ProfileSample::Ready(value) => Some(value),
            _ => None,
        }
    }
    /// Queue a profile snapshot after the triggering frame has finished.
    /// The bounded channel keeps this request nonblocking on the frame thread.
    pub fn request(&mut self) -> Result<(), &'static str> {
        if self.outstanding {
            return Err("busy");
        }
        if self.sender.is_none() {
            let (tx, rx) = mpsc::sync_channel::<()>(1);
            let (result_tx, result_rx) = mpsc::sync_channel::<ProfileSample>(1);
            if std::thread::Builder::new()
                .name("profile-export".into())
                .spawn(move || {
                    while rx.recv().is_ok() {
                        let _lane = crate::engine_profile::worker_scope_named("Profile snapshot");
                        let _span = crate::engine_profile::span("Profile snapshot serialization");
                        let mut snapshot = crate::engine_profile::snapshot();
                        // Bound transport payload separately from the underlying event rings.
                        let mut retained = snapshot
                            .lanes
                            .iter()
                            .flat_map(|lane| lane.events.iter())
                            .map(|event| (event.end_ns, event.event_id))
                            .collect::<Vec<_>>();
                        retained.sort_unstable();
                        let omitted = retained.len().saturating_sub(4096);
                        if let Some(cutoff) = retained.get(omitted).copied() {
                            for lane in &mut snapshot.lanes {
                                lane.events
                                    .retain(|event| (event.end_ns, event.event_id) >= cutoff);
                            }
                        }
                        snapshot.snapshot_events_omitted = omitted;
                        let result = match serde_json::to_value(snapshot) {
                            Ok(value) => ProfileSample::Ready(value),
                            Err(error) => ProfileSample::SerializationFailed(error.to_string()),
                        };
                        if result_tx.send(result).is_err() {
                            break;
                        }
                    }
                })
                .is_ok()
            {
                self.sender = Some(tx);
                self.receiver = Some(result_rx);
            } else {
                return Err("worker_unavailable");
            }
        }
        match self.sender.as_ref().expect("worker started").try_send(()) {
            Ok(()) => {
                self.outstanding = true;
                Ok(())
            }
            Err(mpsc::TrySendError::Full(())) => {
                self.outstanding = true;
                Err("busy")
            }
            Err(mpsc::TrySendError::Disconnected(())) => {
                *self = Self::default();
                Err("worker_unavailable")
            }
        }
    }

    fn poll(&mut self) -> Option<ProfileSample> {
        let receiver = self.receiver.as_ref()?;
        let sample = match receiver.try_recv() {
            Ok(sample) => Some(sample),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(ProfileSample::WorkerUnavailable),
        };
        if sample.is_some() {
            self.outstanding = false;
        }
        sample
    }
}

const PRE_FRAMES: usize = 512;
const POST_FRAMES: usize = 64;
const MAX_CAPTURES: usize = 4;
const PROFILE_RESULT_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Clone, Copy)]
struct CaptureThresholds {
    frame_cpu_ms: f64,
    publication_ms: f64,
    terrain_update_ms: f64,
    queue_age_ms: f64,
    convergence_stall: Duration,
}

impl CaptureThresholds {
    fn from_environment() -> Self {
        Self {
            frame_cpu_ms: env_threshold("ASTRUM_CAPTURE_FRAME_CPU_MS", 100.0, 60_000.0),
            publication_ms: env_threshold("ASTRUM_CAPTURE_PUBLICATION_MS", 2.0, 60_000.0),
            terrain_update_ms: env_threshold("ASTRUM_CAPTURE_TERRAIN_UPDATE_MS", 12.0, 60_000.0),
            queue_age_ms: env_threshold("ASTRUM_CAPTURE_QUEUE_AGE_MS", 5_000.0, 3_600_000.0),
            convergence_stall: Duration::from_secs_f64(
                env_threshold(
                    "ASTRUM_CAPTURE_CONVERGENCE_STALL_MS",
                    10_000.0,
                    3_600_000.0,
                ) / 1000.0,
            ),
        }
    }
}

fn parse_threshold(value: Option<&str>, default: f64, maximum: f64) -> f64 {
    value
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map_or(default, |value| value.min(maximum))
}

fn env_threshold(name: &str, default: f64, maximum: f64) -> f64 {
    parse_threshold(std::env::var(name).ok().as_deref(), default, maximum)
}

#[derive(Clone, Serialize)]
struct Frame {
    frame: u64,
    elapsed_ms: f64,
    performance: PerformanceSnapshot,
    publication_ms: Option<f64>,
    terrain_ms: Option<f64>,
    queue_age_ms: Option<f64>,
    visible_convergence: Option<f64>,
    center_convergence: Option<f64>,
    desired: Option<u64>,
    drawable: Option<u64>,
    generation: Option<u64>,
    upload_bytes: Option<u64>,
    target_quality_reached: Option<bool>,
    quality_pending: Option<bool>,
}

struct Bundle {
    name: String,
    reason: &'static str,
    trigger_frame: u64,
    frames: Vec<Frame>,
    resident: Value,
    profile: Value,
    profile_status: &'static str,
    profile_error: Option<String>,
    profile_wait_ms: Option<u64>,
    profile_requested_at: Option<Instant>,
    post_remaining: usize,
}

#[derive(Default)]
struct ConvergenceWatch {
    desired: Option<u64>,
    drawable: Option<u64>,
    best_visible_ratio: Option<f64>,
    last_progress: Option<Instant>,
}

impl ConvergenceWatch {
    fn stalled(&mut self, frame: &Frame, now: Instant, threshold: Duration) -> bool {
        let pending =
            frame.quality_pending == Some(true) || frame.target_quality_reached == Some(false);
        let Some(desired) = frame.desired.filter(|desired| *desired > 0) else {
            self.reset();
            return false;
        };
        if !pending {
            self.reset();
            return false;
        }

        let target_changed = self.desired != Some(desired);
        let drawable = frame.drawable.unwrap_or(0);
        let ratio_progress = frame.visible_convergence.is_some_and(|ratio| {
            ratio.is_finite()
                && ratio >= 0.0
                && self
                    .best_visible_ratio
                    .is_none_or(|best| ratio > best + 0.001)
        });
        let progress = target_changed
            || self.drawable.is_some_and(|previous| drawable > previous)
            || ratio_progress;

        if progress || self.last_progress.is_none() {
            self.last_progress = Some(now);
            self.best_visible_ratio = if target_changed || ratio_progress {
                frame.visible_convergence
            } else {
                self.best_visible_ratio.or(frame.visible_convergence)
            };
        }
        self.desired = Some(desired);
        self.drawable = Some(drawable);
        self.last_progress
            .is_some_and(|at| now.saturating_duration_since(at) >= threshold)
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Captures are opt-in via `ASTRUM_CAPTURE_DIR`. No file I/O occurs in observe.
pub struct PerformanceCapture {
    writer: Option<SyncSender<Bundle>>,
    output_root: PathBuf,
    active: bool,
    history: VecDeque<Frame>,
    pending: Option<Bundle>,
    started: Instant,
    last_trigger: Option<Instant>,
    count: usize,
    requested: bool,
    thresholds: CaptureThresholds,
    convergence: ConvergenceWatch,
    profile_sampler: Option<ProfileSampler>,
    pub dropped: u64,
}

impl PerformanceCapture {
    pub fn from_environment() -> Self {
        let configured_root = std::env::var_os("ASTRUM_CAPTURE_DIR").map(PathBuf::from);
        let output_root = configured_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("target/terrain-captures"))
            .join(format!("process-{}", std::process::id()));
        let writer = configured_root.and_then(|_| spawn_writer(output_root.clone()));
        let active = writer.is_some();
        Self {
            writer,
            output_root,
            active,
            history: VecDeque::with_capacity(PRE_FRAMES),
            pending: None,
            started: Instant::now(),
            last_trigger: None,
            count: 0,
            requested: false,
            thresholds: CaptureThresholds::from_environment(),
            convergence: ConvergenceWatch::default(),
            profile_sampler: None,
            dropped: 0,
        }
    }

    pub fn request(&mut self) {
        if !self.active {
            self.writer = spawn_writer(self.output_root.clone());
            self.active = self.writer.is_some();
        }
        if !self.active {
            return;
        }
        self.requested = true;
    }

    /// Stop observing captures and queue any partial bundle for background
    /// writing. `post_frames_missing` records frames omitted by this early stop.
    pub fn stop(&mut self) {
        self.active = false;
        self.requested = false;
        self.history.clear();
        if let Some(mut bundle) = self.pending.take() {
            if bundle.profile_status == "pending" {
                bundle.profile_status = "stopped_pending";
                bundle.profile_wait_ms = bundle
                    .profile_requested_at
                    .map(|requested| requested.elapsed().as_millis() as u64);
            }
            if self
                .writer
                .as_ref()
                .is_none_or(|writer| writer.try_send(bundle).is_err())
            {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
        self.writer = None;
        self.profile_sampler = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns a correlated native screenshot request name at trigger time.
    pub fn observe(&mut self, snapshot: &DeveloperSnapshot) -> Option<String> {
        if !self.active {
            return None;
        }
        self.writer.as_ref()?;
        // Atlas terrain reports drawn nodes and completion, not a desired count,
        // so the convergence-stall trigger stays inactive for it.
        let frame = Frame {
            frame: snapshot.general.frame_number,
            elapsed_ms: self.started.elapsed().as_secs_f64() * 1000.0,
            performance: snapshot.performance.clone(),
            publication_ms: None,
            terrain_ms: snapshot.performance.terrain_update_ms,
            queue_age_ms: None,
            visible_convergence: None,
            center_convergence: None,
            desired: None,
            drawable: Some(snapshot.terrain.visible_leaf_count as u64),
            generation: None,
            upload_bytes: snapshot.performance.upload_bytes,
            target_quality_reached: snapshot.terrain.settled,
            quality_pending: snapshot.terrain.quality_pending,
        };
        let now = Instant::now();
        self.poll_profile_result(now);
        let convergence_stalled =
            self.convergence
                .stalled(&frame, now, self.thresholds.convergence_stall);
        if let Some(bundle) = &mut self.pending
            && bundle.post_remaining > 0
        {
            bundle.frames.push(frame.clone());
            bundle.post_remaining -= 1;
        }
        self.finish_pending_if_ready();
        if self.history.len() == PRE_FRAMES {
            self.history.pop_front();
        }
        self.history.push_back(frame.clone());
        if self.pending.is_some()
            || self.count == MAX_CAPTURES
            || self
                .last_trigger
                .is_some_and(|at| at.elapsed() < Duration::from_secs(10))
        {
            return None;
        }
        let reason = trigger_reason(&frame, self.requested, self.thresholds, convergence_stalled)?;
        self.requested = false;
        self.count += 1;
        self.last_trigger = Some(Instant::now());
        let name = format!("bad-frame-{:04}-{}", self.count, frame.frame);
        // The native finished-frame guard has already run before observe; queue
        // this snapshot now, before assembling any capture artifacts.
        let profile_requested_at = Instant::now();
        let profile_sampler = self
            .profile_sampler
            .get_or_insert_with(ProfileSampler::default);
        let (profile_status, profile_error, profile_requested_at) = match profile_sampler.request()
        {
            Ok(()) => ("pending", None, Some(profile_requested_at)),
            Err(status) => (status, Some(status.to_owned()), None),
        };
        // Never clone the existing 8192-frame diagnostic history into a bundle.
        let mut state = serde_json::Map::new();
        state.insert(
            "camera".into(),
            serde_json::to_value(&snapshot.camera).unwrap_or(Value::Null),
        );
        state.insert(
            "terrain".into(),
            serde_json::to_value(&snapshot.terrain).unwrap_or(Value::Null),
        );
        state.insert(
            "rendering".into(),
            serde_json::to_value(&snapshot.rendering).unwrap_or(Value::Null),
        );
        if let Some(atlas) = &snapshot.terrain_atlas {
            state.insert("terrain_atlas".into(), atlas.clone());
        }
        self.pending = Some(Bundle {
            name: name.clone(),
            reason,
            trigger_frame: frame.frame,
            frames: self.history.iter().cloned().collect(),
            resident: Value::Object(state),
            // Snapshot independently after the triggering frame so a cached
            // developer snapshot cannot omit the frame that caused this capture.
            profile: Value::Null,
            profile_status,
            profile_error,
            profile_wait_ms: None,
            profile_requested_at,
            post_remaining: POST_FRAMES,
        });
        Some(name)
    }

    fn poll_profile_result(&mut self, now: Instant) {
        let is_pending = self
            .pending
            .as_ref()
            .is_some_and(|bundle| bundle.profile_status == "pending");
        let result = self.profile_sampler.as_mut().and_then(ProfileSampler::poll);
        let Some(bundle) = self.pending.as_mut() else {
            // Discard late results from a timed-out or already written capture.
            return;
        };
        if !is_pending {
            return;
        }
        if let Some(sample) = result {
            bundle.profile_wait_ms = bundle
                .profile_requested_at
                .map(|requested| now.saturating_duration_since(requested).as_millis() as u64);
            match sample {
                ProfileSample::Ready(value) => {
                    let enabled = value
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    bundle.profile = value;
                    bundle.profile_status = if enabled {
                        "ready"
                    } else {
                        "profiling_disabled"
                    };
                }
                ProfileSample::SerializationFailed(error) => {
                    bundle.profile_status = "serialization_failed";
                    bundle.profile_error = Some(error);
                }
                ProfileSample::WorkerUnavailable => {
                    bundle.profile_status = "worker_unavailable";
                    bundle.profile_error =
                        Some("profile worker stopped before returning a snapshot".into());
                }
            }
        } else if bundle.profile_requested_at.is_some_and(|requested| {
            now.saturating_duration_since(requested) >= PROFILE_RESULT_TIMEOUT
        }) {
            bundle.profile_status = "timeout";
            bundle.profile_wait_ms = Some(PROFILE_RESULT_TIMEOUT.as_millis() as u64);
            bundle.profile_error = Some("profile snapshot exceeded the bounded wait".into());
        }
    }

    fn finish_pending_if_ready(&mut self) {
        let ready = self
            .pending
            .as_ref()
            .is_some_and(|bundle| bundle.post_remaining == 0 && bundle.profile_status != "pending");
        if !ready {
            return;
        }
        let Some(bundle) = self.pending.take() else {
            return;
        };
        if self
            .writer
            .as_ref()
            .is_none_or(|writer| writer.try_send(bundle).is_err())
        {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

impl Drop for PerformanceCapture {
    fn drop(&mut self) {
        if let (Some(writer), Some(bundle)) = (&self.writer, self.pending.take()) {
            let mut bundle = bundle;
            if bundle.profile_status == "pending" {
                bundle.profile_status = "shutdown_pending";
                bundle.profile_wait_ms = bundle
                    .profile_requested_at
                    .map(|requested| requested.elapsed().as_millis() as u64);
            }
            let _ = writer.try_send(bundle);
        }
    }
}

fn trigger_reason(
    frame: &Frame,
    requested: bool,
    thresholds: CaptureThresholds,
    convergence_stalled: bool,
) -> Option<&'static str> {
    if requested {
        Some("manual")
    } else if frame
        .performance
        .frame_cpu_ms
        .or(frame.performance.host_frame_ms)
        .is_some_and(|ms| ms > thresholds.frame_cpu_ms)
    {
        Some("frame_cpu_budget")
    } else if frame
        .publication_ms
        .is_some_and(|ms| ms > thresholds.publication_ms)
    {
        Some("publication_budget")
    } else if frame
        .terrain_ms
        .is_some_and(|ms| ms > thresholds.terrain_update_ms)
    {
        Some("terrain_update_budget")
    } else if frame
        .queue_age_ms
        .is_some_and(|ms| ms > thresholds.queue_age_ms)
    {
        Some("queue_age")
    } else if convergence_stalled {
        Some("convergence_stall")
    } else {
        None
    }
}

fn spawn_writer(root: PathBuf) -> Option<SyncSender<Bundle>> {
    let (tx, rx) = mpsc::sync_channel::<Bundle>(2);
    std::thread::Builder::new()
        .name("performance-artifacts".into())
        .spawn(move || {
            while let Ok(bundle) = rx.recv() {
                if let Err(error) = write_bundle(&root, bundle) {
                    tracing::warn!(%error, "performance artifact could not be written");
                }
            }
        })
        .ok()
        .map(|_| tx)
}

fn write_json(path: PathBuf, value: &impl Serialize) -> std::io::Result<()> {
    let mut output = BufWriter::new(
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?,
    );
    serde_json::to_writer(&mut output, value)?;
    output.flush()
}

fn write_bundle(root: &std::path::Path, bundle: Bundle) -> std::io::Result<()> {
    fs::create_dir_all(root)?;
    let output = root.join(&bundle.name);
    fs::create_dir_all(&output)?;
    let identity = collect_identity();
    write_json(
        output.join("summary.json"),
        &json!({
            "schema_version":1,"kind":"engine_bad_frame","trigger":bundle.reason,
            "trigger_frame":bundle.trigger_frame,"screenshot_capture_name":bundle.name,
            "screenshot_source":"correlated native developer bundle; next submitted frame",
            "platform":std::env::consts::OS,"frames":bundle.frames.len(),
            "pre_frame_capacity":PRE_FRAMES,"post_frame_capacity":POST_FRAMES,
            "post_frames_missing":bundle.post_remaining,"capture_limit":MAX_CAPTURES,
            "profile_status":bundle.profile_status,
            "profile_error":bundle.profile_error,
            "profile_wait_ms":bundle.profile_wait_ms,
            "thresholds":{
                "frame_cpu_ms":env_threshold("ASTRUM_CAPTURE_FRAME_CPU_MS",100.0,60_000.0),
                "publication_ms":env_threshold("ASTRUM_CAPTURE_PUBLICATION_MS",2.0,60_000.0),
                "terrain_update_ms":env_threshold("ASTRUM_CAPTURE_TERRAIN_UPDATE_MS",12.0,60_000.0),
                "queue_age_ms":env_threshold("ASTRUM_CAPTURE_QUEUE_AGE_MS",5_000.0,3_600_000.0),
                "convergence_stall_ms":env_threshold("ASTRUM_CAPTURE_CONVERGENCE_STALL_MS",10_000.0,3_600_000.0),
            },
            "identity":identity,
        }),
    )?;
    let mut frames = BufWriter::new(
        File::options()
            .write(true)
            .create_new(true)
            .open(output.join("frames.jsonl"))?,
    );
    for frame in &bundle.frames {
        serde_json::to_writer(&mut frames, frame)?;
        writeln!(frames)?;
    }
    frames.flush()?;
    write_json(output.join("resident_state.json"), &bundle.resident)?;
    write_json(output.join("spans.json"), &bundle.profile)?;
    crate::profile_export::write_profile_trace(&output.join("timeline.json"), &bundle.profile, None)
}

fn collect_identity() -> Value {
    let executable = std::env::current_exe().ok();
    let executable_metadata = executable.as_ref().and_then(|path| fs::metadata(path).ok());
    let modified_unix_seconds = executable_metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let (git_head, git_dirty, git_changes, git_changes_truncated) = git_identity();
    json!({
        "build": {
            "package_version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "developer_tools": cfg!(feature="developer-tools"),
        },
        "executable": {
            "path": executable.as_ref().map(|path| path.display().to_string()),
            "bytes": executable_metadata.as_ref().map(|metadata| metadata.len()),
            "modified_unix_seconds": modified_unix_seconds,
            "sha256": executable.as_ref().and_then(|path| sha256_file(path)),
        },
        "git": {
            "head": git_head,
            "dirty": git_dirty,
            "status_entries": git_changes,
            "status_truncated": git_changes_truncated,
        },
    })
}

fn git_identity() -> (Option<String>, Option<bool>, Vec<String>, bool) {
    let preferred_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fallback_root = std::env::current_dir().ok();
    let roots = [Some(preferred_root), fallback_root];
    for root in roots.into_iter().flatten() {
        let head = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output();
        let Ok(head) = head else { continue };
        if !head.status.success() {
            continue;
        }
        let head = String::from_utf8_lossy(&head.stdout).trim().to_owned();
        let status = Command::new("git")
            .args(["status", "--short", "--untracked-files=all"])
            .current_dir(&root)
            .output();
        let Ok(status) = status else {
            return (Some(head), None, Vec::new(), false);
        };
        if !status.status.success() {
            return (Some(head), None, Vec::new(), false);
        }
        let all: Vec<_> = String::from_utf8_lossy(&status.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        let dirty = !all.is_empty();
        let truncated = all.len() > 64;
        return (
            Some(head),
            Some(dirty),
            all.into_iter().take(64).collect(),
            truncated,
        );
    }
    (None, None, Vec::new(), false)
}

fn sha256_file(path: &std::path::Path) -> Option<String> {
    #[cfg(feature = "developer-tools")]
    {
        let mut input = File::open(path).ok()?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer).ok()?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Some(format!("{:x}", hasher.finalize()))
    }
    #[cfg(not(feature = "developer-tools"))]
    {
        let _ = path;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> CaptureThresholds {
        CaptureThresholds {
            frame_cpu_ms: 100.0,
            publication_ms: 2.0,
            terrain_update_ms: 12.0,
            queue_age_ms: 5_000.0,
            convergence_stall: Duration::from_secs(10),
        }
    }

    #[test]
    fn missing_measurements_never_trigger_and_publication_threshold_is_strict() {
        let mut frame = Frame {
            frame: 1,
            elapsed_ms: 0.0,
            performance: PerformanceSnapshot::default(),
            publication_ms: None,
            terrain_ms: None,
            queue_age_ms: None,
            visible_convergence: None,
            center_convergence: None,
            desired: None,
            drawable: None,
            generation: None,
            upload_bytes: None,
            target_quality_reached: None,
            quality_pending: None,
        };
        assert_eq!(trigger_reason(&frame, false, thresholds(), false), None);
        frame.publication_ms = Some(2.0);
        assert_eq!(trigger_reason(&frame, false, thresholds(), false), None);
        frame.publication_ms = Some(2.01);
        assert_eq!(
            trigger_reason(&frame, false, thresholds(), false),
            Some("publication_budget")
        );
    }

    #[test]
    fn capture_thresholds_accept_finite_nonnegative_values_and_cap_extremes() {
        assert_eq!(parse_threshold(Some("14.5"), 100.0, 60_000.0), 14.5);
        assert_eq!(parse_threshold(Some("-1"), 100.0, 60_000.0), 100.0);
        assert_eq!(parse_threshold(Some("NaN"), 100.0, 60_000.0), 100.0);
        assert_eq!(parse_threshold(Some("70000"), 100.0, 60_000.0), 60_000.0);
    }

    #[test]
    fn convergence_stall_requires_pending_stable_target_and_expires_after_threshold() {
        let start = Instant::now();
        let mut watch = ConvergenceWatch::default();
        let frame = Frame {
            frame: 1,
            elapsed_ms: 0.0,
            performance: PerformanceSnapshot::default(),
            publication_ms: None,
            terrain_ms: None,
            queue_age_ms: None,
            visible_convergence: Some(0.5),
            center_convergence: None,
            desired: Some(100),
            drawable: Some(80),
            generation: Some(10),
            upload_bytes: None,
            target_quality_reached: Some(false),
            quality_pending: Some(true),
        };
        assert!(!watch.stalled(&frame, start, Duration::from_secs(10)));
        assert!(!watch.stalled(
            &frame,
            start + Duration::from_secs(9),
            Duration::from_secs(10)
        ));
        assert!(watch.stalled(
            &frame,
            start + Duration::from_secs(10),
            Duration::from_secs(10)
        ));

        let mut progress = frame.clone();
        progress.visible_convergence = Some(0.6);
        assert!(!watch.stalled(
            &progress,
            start + Duration::from_secs(11),
            Duration::from_secs(10)
        ));
        let mut complete = progress;
        complete.target_quality_reached = Some(true);
        complete.quality_pending = Some(false);
        assert!(!watch.stalled(
            &complete,
            start + Duration::from_secs(30),
            Duration::from_secs(10)
        ));
    }

    #[test]
    fn profile_sampler_returns_a_fresh_background_snapshot() {
        let mut sampler = ProfileSampler::default();
        assert_eq!(sampler.request(), Ok(()));
        assert_eq!(sampler.request(), Err("busy"));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(sample) = sampler.poll() {
                let ProfileSample::Ready(snapshot) = sample else {
                    panic!("profile snapshot unavailable: {sample:?}");
                };
                assert!(
                    snapshot
                        .get("generated_at_ns")
                        .and_then(Value::as_u64)
                        .is_some()
                );
                assert!(snapshot.get("lanes").and_then(Value::as_array).is_some());
                break;
            }
            assert!(Instant::now() < deadline, "profile worker did not finish");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
