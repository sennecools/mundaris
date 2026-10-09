//! Loopback transport, owner-thread commands and immutable evidence publication.
use crate::{
    GravityOrbitsDemo,
    developer_protocol::*,
    developer_snapshot::{DeveloperSnapshot, DevelopmentSnapshot},
};
use anyhow::{Context, Result, ensure};
use astrum_renderer::{RenderOutcome, Renderer, native_capture::NativeCaptureFrame};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Pending {
    request: DevRequest,
    reply: SyncSender<DevResponse>,
    deadline: Instant,
}
struct Lease {
    token: String,
    owner: String,
    expires: Instant,
}
struct Capture {
    id: u64,
    command: String,
    name: String,
    snapshot: Option<DeveloperSnapshot>,
    deadline: Instant,
    publishing: bool,
    cancelled: Arc<AtomicBool>,
}
struct Publish {
    frame: NativeCaptureFrame,
    snapshot: DeveloperSnapshot,
    name: String,
    command: String,
    output: PathBuf,
    cancelled: Arc<AtomicBool>,
}
struct Published {
    command: String,
    result: std::result::Result<Value, String>,
}
/// Only the native event-loop owner accesses this state. IO workers cannot mutate it.
pub struct DeveloperService {
    pub descriptor: SessionDescriptor,
    requests: Receiver<Pending>,
    stopped: Arc<AtomicBool>,
    registry_file: PathBuf,
    lease: Option<Lease>,
    sequence: u64,
    receipts: VecDeque<DevReceipt>,
    events: VecDeque<Value>,
    event_sequence: u64,
    observation: Option<(DeveloperSnapshot, Instant)>,
    capture: Option<Capture>,
    publish: SyncSender<Publish>,
    published: Receiver<Published>,
    pub shutdown_requested: bool,
}

fn refresh_freshness(
    development: &mut DevelopmentSnapshot,
    observation_age: Duration,
    drawable: bool,
    observed_world_revision: u64,
    current_world_revision: u64,
    current_command_sequence: u64,
) {
    development.observation_age_ms = observation_age.as_secs_f64() * 1000.0;
    development.stale = !drawable
        || observation_age > Duration::from_millis(250)
        || observed_world_revision != current_world_revision
        || development.command_sequence != current_command_sequence;
    development.drawable = drawable;
}

impl DeveloperService {
    pub fn start(preset: &str, wake: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        let registry = std::env::var_os("ASTRUM_DEV_REGISTRY")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/developer-sessions"));
        let output = std::env::var_os("ASTRUM_DEV_OUTPUT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/developer-evidence"));
        fs::create_dir_all(&registry)?;
        fs::create_dir_all(&output)?;
        let executable = std::env::current_exe()?;
        let session_id = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let binary_sha256 = hash_file(&executable)?;
        let build_manifest = std::env::var_os("ASTRUM_DEV_BUILD_MANIFEST")
            .map(|p| -> Result<Value> { Ok(serde_json::from_slice(&fs::read(p)?)?) })
            .transpose()?
            .filter(|manifest| {
                manifest.get("binary_sha256").and_then(Value::as_str)
                    == Some(binary_sha256.as_str())
            });
        let descriptor = SessionDescriptor {
            protocol_version: PROTOCOL_VERSION,
            session_id: session_id.clone(),
            endpoint: listener.local_addr()?.to_string(),
            pid: std::process::id(),
            preset: preset.into(),
            executable: executable.display().to_string(),
            output_directory: fs::canonicalize(output)?.display().to_string(),
            binary_sha256,
            build_manifest,
        };
        let registry_file = registry.join(format!("{session_id}.json"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&registry_file)?;
        file.write_all(&serde_json::to_vec_pretty(&descriptor)?)?;
        let (send, requests) = mpsc::sync_channel(REQUEST_CAPACITY);
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopped);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let active = Arc::new(AtomicUsize::new(0));
        let desc = descriptor.clone();
        let net_wake = Arc::clone(&wake);
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        if active.fetch_add(1, Ordering::Relaxed) >= REQUEST_CAPACITY {
                            active.fetch_sub(1, Ordering::Relaxed);
                            let _ = socket.set_write_timeout(Some(Duration::from_millis(10)));
                            if let Ok(bytes) = serde_json::to_vec(&DevResponse::new(
                                &desc.session_id,
                                "",
                                "failed",
                                json!({"error":"connection_overflow"}),
                            )) {
                                let _ = socket.write_all(&bytes);
                                let _ = socket.write_all(b"\n");
                            }
                            continue;
                        }
                        let send = send.clone();
                        let wake = Arc::clone(&net_wake);
                        let desc = desc.clone();
                        let active = Arc::clone(&active);
                        thread::spawn(move || {
                            let _ = serve(socket, &desc, &send, &*wake);
                            active.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        let (publish, jobs) = mpsc::sync_channel::<Publish>(1);
        let (done, published) = mpsc::sync_channel(1);
        thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let command = job.command.clone();
                let result = publish_bundle(job).map_err(|e| e.to_string());
                if done.send(Published { command, result }).is_err() {
                    break;
                }
                wake();
            }
        });
        Ok(Self {
            descriptor,
            requests,
            stopped,
            registry_file,
            lease: None,
            sequence: 0,
            receipts: VecDeque::new(),
            events: VecDeque::new(),
            event_sequence: 0,
            observation: None,
            capture: None,
            publish,
            published,
            shutdown_requested: false,
        })
    }
    pub fn observe(&mut self, demo: &GravityOrbitsDemo, renderer: &Renderer, drawable: bool) {
        let Some(snapshot) = demo.developer_last_snapshot() else {
            return;
        };
        let mut snapshot = snapshot.clone();
        let submitted = match renderer.last_render_outcome() {
            RenderOutcome::Submitted { submission_id, .. } => Some(submission_id),
            _ => None,
        };
        snapshot.development = Some(DevelopmentSnapshot {
            session_id: self.descriptor.session_id.clone(),
            observation_age_ms: 0.0,
            stale: !drawable,
            drawable,
            command_sequence: self.sequence,
            prepared_frame: snapshot.general.frame_number,
            submitted_frame: submitted,
            presentation_requested: submitted.is_some(),
            capture_id: None,
            current_errors: self
                .receipts
                .iter()
                .filter(|r| r.status == "failed")
                .map(|r| r.data.to_string())
                .collect(),
            asynchronous_measurement_source: "latest_completed".into(),
            gpu_measurement_source_frame: snapshot.performance.gpu_source_frame,
        });
        if let Some(capture) = &mut self.capture
            && !capture.publishing
            && capture.snapshot.is_none()
            && submitted.is_some()
        {
            let mut captured = snapshot.clone();
            captured.capture = Some(crate::developer_snapshot::CaptureMetadata {
                scene: capture.name.clone(),
                image: "client.png".into(),
                width: 0,
                height: 0,
                terrain_updates: 0,
                worker_count: snapshot.work.worker_count,
                step_ms: 0,
                morph_duration_ms: demo.developer_morph_duration_ms(),
                adapter: String::new(),
                backend: String::new(),
            });
            if let Some(d) = &mut captured.development {
                d.capture_id = Some(capture.command.clone());
            }
            capture.snapshot = Some(captured);
        }
        self.observation = Some((snapshot, Instant::now()));
        for receipt in &mut self.receipts {
            if receipt.status == "applied" && receipt.prepared_frame.is_none() {
                receipt.prepared_frame = Some(
                    demo.developer_last_snapshot()
                        .map_or(0, |s| s.general.frame_number),
                );
                receipt.submitted_frame = submitted;
            }
        }
    }
    fn current_observation(
        &self,
        drawable: bool,
        current_world_revision: u64,
    ) -> Option<DeveloperSnapshot> {
        self.observation.as_ref().map(|(snapshot, observed_at)| {
            let mut snapshot = snapshot.clone();
            if let Some(development) = &mut snapshot.development {
                refresh_freshness(
                    development,
                    observed_at.elapsed(),
                    drawable,
                    snapshot.general.world_revision,
                    current_world_revision,
                    self.sequence,
                );
            }
            snapshot
        })
    }
    fn event(&mut self, kind: &str, data: Value) {
        self.event_sequence += 1;
        self.events
            .push_back(json!({"sequence":self.event_sequence,"kind":kind,"data":data}));
        if self.events.len() > HISTORY_CAPACITY {
            self.events.pop_front();
        }
    }
    fn receipt(&mut self, status: &str, revision: u64, data: Value) -> String {
        self.sequence += 1;
        let id = format!("{}-{}", self.descriptor.session_id, self.sequence);
        self.receipts.push_back(DevReceipt {
            command_id: id.clone(),
            sequence: self.sequence,
            status: status.into(),
            world_revision: revision,
            prepared_frame: None,
            submitted_frame: None,
            data,
        });
        if self.receipts.len() > HISTORY_CAPACITY {
            let active_command = self
                .capture
                .as_ref()
                .map(|capture| capture.command.as_str());
            let eviction_index = active_command
                .and_then(|active| {
                    self.receipts
                        .iter()
                        .position(|receipt| receipt.command_id != active)
                })
                .unwrap_or(0);
            self.receipts.remove(eviction_index);
        }
        id
    }
    fn complete(&mut self, id: &str, status: &str, data: Value) {
        let Some(receipt) = self
            .receipts
            .iter_mut()
            .find(|r| r.command_id == id && r.status == "accepted")
        else {
            return;
        };
        receipt.status = status.into();
        receipt.submitted_frame = data["source_frame"].as_u64();
        receipt.prepared_frame = data["prepared_frame"].as_u64();
        receipt.data = data;
        self.event(status, json!({"command_id":id}));
    }
    fn owns(&self, token: &str) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|l| l.token == token && l.expires > Instant::now())
    }
    pub fn interrupt(
        &mut self,
        demo: &mut GravityOrbitsDemo,
        renderer: &mut Renderer,
        reason: &str,
    ) {
        if self.lease.take().is_some() {
            let _ = demo.developer_cancel();
            demo.developer_set_automation(None);
            if let Some(capture) = &self.capture {
                capture.cancelled.store(true, Ordering::Release);
                let command = capture.command.clone();
                if capture.publishing {
                    self.complete(&command, "cancelled", json!({"reason":reason}));
                } else if let Some(capture) = self.capture.take() {
                    renderer.cancel_native_capture();
                    let _ = renderer.poll_native_capture();
                    self.complete(&capture.command, "cancelled", json!({"reason":reason}));
                }
            }
            self.event("control_lost", json!({"reason":reason}));
        }
    }
    pub fn turn(&mut self, demo: &mut GravityOrbitsDemo, renderer: &mut Renderer, drawable: bool) {
        if self
            .lease
            .as_ref()
            .is_some_and(|l| l.expires <= Instant::now())
        {
            self.interrupt(demo, renderer, "lease_expired");
        }
        if demo.developer_take_stop() {
            self.interrupt(demo, renderer, "human_stop");
        }
        let capture_expired = self
            .capture
            .as_ref()
            .is_some_and(|c| !c.publishing && (c.deadline <= Instant::now() || !drawable));
        let publication_expired = self
            .capture
            .as_ref()
            .is_some_and(|c| c.publishing && (c.deadline <= Instant::now() || !drawable));
        if publication_expired && let Some(capture) = &self.capture {
            capture.cancelled.store(true, Ordering::Release);
            let command = capture.command.clone();
            self.complete(
                &command,
                "failed",
                json!({"error":if drawable{"capture_timeout"}else{"not_drawable"}}),
            );
        }
        if capture_expired {
            let capture = self.capture.take();
            if let Some(capture) = capture {
                capture.cancelled.store(true, Ordering::Release);
                renderer.cancel_native_capture();
                let _ = renderer.poll_native_capture();
                self.complete(
                    &capture.command,
                    "failed",
                    json!({"error":if drawable{"capture_timeout"}else{"not_drawable"}}),
                );
            }
        }
        while let Ok(p) = self.published.try_recv() {
            if self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.command == p.command && capture.publishing)
            {
                self.capture = None;
            }
            match p.result {
                Ok(data) => self.complete(&p.command, "applied", data),
                Err(error) => self.complete(&p.command, "failed", json!({"error":error})),
            }
        }
        if self
            .capture
            .as_ref()
            .is_some_and(|capture| !capture.publishing)
        {
            match renderer.poll_native_capture() {
                Ok(Some(frame)) => {
                    if let Some(c) = &mut self.capture {
                        if frame.capture_id != c.id {
                            let command = c.command.clone();
                            c.cancelled.store(true, Ordering::Release);
                            self.capture = None;
                            self.complete(
                                &command,
                                "failed",
                                json!({"error":"capture_identity_mismatch"}),
                            );
                        } else if let Some(snapshot) = c.snapshot.take() {
                            let job = Publish {
                                frame,
                                snapshot,
                                name: c.name.clone(),
                                command: c.command.clone(),
                                output: PathBuf::from(&self.descriptor.output_directory),
                                cancelled: Arc::clone(&c.cancelled),
                            };
                            match self.publish.try_send(job) {
                                Ok(()) => c.publishing = true,
                                Err(_) => {
                                    let command = c.command.clone();
                                    c.cancelled.store(true, Ordering::Release);
                                    self.capture = None;
                                    self.complete(
                                        &command,
                                        "failed",
                                        json!({"error":"publication_busy"}),
                                    );
                                }
                            }
                        } else {
                            let command = c.command.clone();
                            c.cancelled.store(true, Ordering::Release);
                            self.capture = None;
                            self.complete(
                                &command,
                                "failed",
                                json!({"error":"source_observation_unavailable"}),
                            );
                        }
                    }
                }
                Err(e) => {
                    if let Some(c) = self.capture.take() {
                        c.cancelled.store(true, Ordering::Release);
                        self.complete(&c.command, "failed", json!({"error":e}));
                    }
                }
                Ok(None) => {}
            }
        }
        for _ in 0..COMMANDS_PER_TURN {
            let Ok(p) = self.requests.try_recv() else {
                break;
            };
            let response = if p.deadline <= Instant::now() {
                DevResponse::new(
                    &self.descriptor.session_id,
                    &p.request.request_id,
                    "expired",
                    json!({"error":"request_timeout"}),
                )
            } else {
                self.handle(&p.request, demo, renderer, drawable)
            };
            let _ = p.reply.try_send(response);
        }
        if drawable
            && self.capture.is_none()
            && let Some(name) = demo.take_diagnostic_capture_request()
        {
            let command = self.receipt(
                "accepted",
                demo.world().revision(),
                json!({"source":"studio"}),
            );
            let id = self.sequence;
            match renderer
                .enable_native_capture()
                .and_then(|()| renderer.request_native_capture(id))
            {
                Ok(()) => {
                    self.capture = Some(Capture {
                        id,
                        command,
                        name,
                        snapshot: None,
                        deadline: Instant::now() + Duration::from_secs(30),
                        publishing: false,
                        cancelled: Arc::new(AtomicBool::new(false)),
                    })
                }
                Err(error) => self.complete(&command, "failed", json!({"error":error})),
            }
        }
    }
    fn handle(
        &mut self,
        request: &DevRequest,
        demo: &mut GravityOrbitsDemo,
        renderer: &mut Renderer,
        drawable: bool,
    ) -> DevResponse {
        let response = |status: &str, data: Value| {
            DevResponse::new(
                &self.descriptor.session_id,
                &request.request_id,
                status,
                data,
            )
        };
        if request.protocol_version != PROTOCOL_VERSION
            || request.session_id != self.descriptor.session_id
        {
            return response("failed", json!({"error":"protocol_or_session_mismatch"}));
        }
        let (status, data) = match &request.operation {
            DevOperation::Capabilities => (
                "ok",
                json!({"protocol_version":PROTOCOL_VERSION,"queue_capacity":REQUEST_CAPACITY,"applications_per_turn":COMMANDS_PER_TURN,"history_capacity":HISTORY_CAPACITY,"lease_seconds":LEASE_SECONDS,"native_capture":"on_demand_surface_copy","presets":["test-solar-system"],"actions":["profiler","select","focus","overview","look_at","navigation_mode","navigation","clearance","surface_pose","pause","rate","seek","single_step","reset","render_mode","setting","reset_render_settings","layer","resident_cover_hold"]}),
            ),
            DevOperation::Inspect => {
                let mut snapshot = self.current_observation(drawable, demo.world().revision());
                if snapshot.is_none() {
                    snapshot = demo.developer_last_snapshot().cloned();
                }
                (
                    "ok",
                    json!({"session":self.descriptor,"snapshot":snapshot,"bodies":demo.developer_inventory(&self.descriptor.session_id).unwrap_or_default(),"control_owner":self.lease.as_ref().map(|l|&l.owner),"source_attribution":if self.descriptor.build_manifest.is_some(){"manifest_available"}else{"unavailable"}}),
                )
            }
            DevOperation::Diagnostics { scope } => {
                let observed = self.current_observation(drawable, demo.world().revision());
                let s = observed.as_ref();
                match scope.as_str() {
                    "terrain" => (
                        "ok",
                        json!({"terrain":s.map(|s|&s.terrain),"terrain_atlas":s.and_then(|s|s.terrain_atlas.as_ref()),"work":s.map(|s|&s.work),"memory":s.map(|s|&s.memory),"freshness":s.and_then(|s|s.development.as_ref())}),
                    ),
                    "performance" => {
                        let requested = renderer.request_developer_gpu_timing();
                        (
                            "ok",
                            json!({"performance":s.map(|s|&s.performance),"freshness":s.and_then(|s|s.development.as_ref()),"gpu_timing_requested":requested}),
                        )
                    }
                    "errors" => (
                        "ok",
                        json!({"failures":self.receipts.iter().filter(|r|r.status=="failed").collect::<Vec<_>>()}),
                    ),
                    _ => ("failed", json!({"error":"unsupported_diagnostic_scope"})),
                }
            }
            DevOperation::AcquireControl { owner } => {
                if self.lease.is_some() {
                    ("failed", json!({"error":"control_owned"}))
                } else if owner.is_empty() || owner.len() > 128 {
                    ("failed", json!({"error":"invalid_owner"}))
                } else {
                    let token = format!(
                        "lease-{}-{}",
                        self.descriptor.session_id, self.event_sequence
                    );
                    self.lease = Some(Lease {
                        token: token.clone(),
                        owner: owner.clone(),
                        expires: Instant::now() + Duration::from_secs(LEASE_SECONDS),
                    });
                    demo.developer_set_automation(Some(owner));
                    self.event("control_acquired", json!({"owner":owner}));
                    ("ok", json!({"lease":token,"lease_seconds":LEASE_SECONDS}))
                }
            }
            DevOperation::RenewControl { lease } => {
                if self.owns(lease) {
                    if let Some(l) = &mut self.lease {
                        l.expires = Instant::now() + Duration::from_secs(LEASE_SECONDS);
                    }
                    ("ok", json!({"lease":lease,"lease_seconds":LEASE_SECONDS}))
                } else {
                    ("failed", json!({"error":"lost_ownership"}))
                }
            }
            DevOperation::ReleaseControl { lease } | DevOperation::Cancel { lease } => {
                if self.owns(lease) {
                    self.interrupt(demo, renderer, "explicit_release");
                    ("ok", json!({"released":true}))
                } else {
                    ("failed", json!({"error":"lost_ownership"}))
                }
            }
            DevOperation::Action { lease, command } => {
                if !self.owns(lease) {
                    ("failed", json!({"error":"lost_ownership"}))
                } else {
                    let result = demo.developer_apply_command(command);
                    let revision = demo.world().revision();
                    match result {
                        Ok(()) => {
                            let id = self.receipt("applied", revision, json!({}));
                            self.event("applied", json!({"command_id":id}));
                            ("accepted", json!({"command_id":id}))
                        }
                        Err(e) => {
                            let id =
                                self.receipt("failed", revision, json!({"error":e.to_string()}));
                            self.event("failed", json!({"command_id":id}));
                            ("failed", json!({"command_id":id,"error":e.to_string()}))
                        }
                    }
                }
            }
            DevOperation::Receipt { command_id } => self
                .receipts
                .iter()
                .find(|r| &r.command_id == command_id)
                .map_or(
                    ("expired", json!({"error":"receipt_unavailable_or_evicted"})),
                    |r| ("ok", json!(r)),
                ),
            DevOperation::Events { after } => {
                let first = self
                    .events
                    .front()
                    .and_then(|e| e["sequence"].as_u64())
                    .unwrap_or(self.event_sequence + 1);
                (
                    "ok",
                    json!({"events":self.events.iter().filter(|e|e["sequence"].as_u64().is_some_and(|s|s>*after)).collect::<Vec<_>>(),"history_lost":after.saturating_add(1)<first,"latest_sequence":self.event_sequence}),
                )
            }
            DevOperation::Capture { lease, name } => {
                if !self.owns(lease) {
                    ("failed", json!({"error":"lost_ownership"}))
                } else if !drawable {
                    ("failed", json!({"error":"not_drawable"}))
                } else if self.capture.is_some() {
                    ("failed", json!({"error":"busy"}))
                } else if name.is_empty()
                    || name.len() > 80
                    || !name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    ("failed", json!({"error":"invalid_capture_name"}))
                } else {
                    let id = self.receipt("accepted", demo.world().revision(), json!({}));
                    let capture_id = self.sequence;
                    let result = renderer
                        .enable_native_capture()
                        .and_then(|()| renderer.request_native_capture(capture_id));
                    match result {
                        Ok(()) => {
                            self.capture = Some(Capture {
                                id: capture_id,
                                command: id.clone(),
                                name: name.clone(),
                                snapshot: None,
                                deadline: Instant::now() + Duration::from_secs(30),
                                publishing: false,
                                cancelled: Arc::new(AtomicBool::new(false)),
                            });
                            ("accepted", json!({"command_id":id}))
                        }
                        Err(error) => {
                            self.complete(&id, "failed", json!({"error":error}));
                            ("failed", json!({"command_id":id,"error":error}))
                        }
                    }
                }
            }
            DevOperation::Shutdown { lease } => {
                if self.owns(lease) {
                    self.shutdown_requested = true;
                    ("ok", json!({"closing":true}))
                } else {
                    ("failed", json!({"error":"lost_ownership"}))
                }
            }
        };
        DevResponse::new(
            &self.descriptor.session_id,
            &request.request_id,
            status,
            data,
        )
    }
}
impl Drop for DeveloperService {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        let _ = fs::remove_file(&self.registry_file);
    }
}
fn serve(
    socket: TcpStream,
    descriptor: &SessionDescriptor,
    send: &SyncSender<Pending>,
    wake: &dyn Fn(),
) -> Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let mut reader = BufReader::new(socket);
    reader
        .by_ref()
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > MAX_MESSAGE_BYTES && bytes.last() != Some(&b'\n') {
        // A bounded drain avoids resetting a Windows socket while the small
        // error response is in flight. Unbounded/unterminated senders cannot
        // hold this worker beyond the short deadline or this additional cap.
        reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut trailing = Vec::new();
        let _ = reader
            .by_ref()
            .take(MAX_MESSAGE_BYTES as u64)
            .read_until(b'\n', &mut trailing);
    }
    let mut socket = reader.into_inner();
    let response = if bytes.len() > MAX_MESSAGE_BYTES || bytes.last() != Some(&b'\n') {
        DevResponse::new(
            &descriptor.session_id,
            "",
            "failed",
            json!({"error":"malformed_or_oversized_message"}),
        )
    } else {
        match serde_json::from_slice::<DevRequest>(&bytes) {
            Ok(request) => {
                let request_id = request.request_id.clone();
                let (reply, result) = mpsc::sync_channel(1);
                let pending = Pending {
                    request,
                    reply,
                    deadline: Instant::now() + Duration::from_secs(30),
                };
                match send.try_send(pending) {
                    Ok(()) => {
                        wake();
                        result
                            .recv_timeout(Duration::from_secs(31))
                            .unwrap_or_else(|_| {
                                DevResponse::new(
                                    &descriptor.session_id,
                                    &request_id,
                                    "expired",
                                    json!({"error":"owner_thread_timeout"}),
                                )
                            })
                    }
                    Err(error) => DevResponse::new(
                        &descriptor.session_id,
                        &request_id,
                        "failed",
                        json!({"error":match error {mpsc::TrySendError::Full(_)=>"queue_overflow",mpsc::TrySendError::Disconnected(_)=>"session_closed"}}),
                    ),
                }
            }
            Err(e) => DevResponse::new(
                &descriptor.session_id,
                "",
                "failed",
                json!({"error":"invalid_request","detail":e.to_string()}),
            ),
        }
    };
    let response_bytes = encode_response_line(&response, MAX_RESPONSE_BYTES)?;
    socket.write_all(&response_bytes)?;
    socket.shutdown(Shutdown::Write)?;
    if bytes.len() > MAX_MESSAGE_BYTES {
        // Allow the peer to consume the error and close its request side before
        // dropping the socket. This is bounded even for a sender that continues.
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut trailing = Vec::new();
        let _ = socket
            .take(MAX_MESSAGE_BYTES as u64)
            .read_to_end(&mut trailing);
    }
    Ok(())
}
fn encode_response_line(response: &DevResponse, max_bytes: usize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    if bytes.len() <= max_bytes {
        return Ok(bytes);
    }

    let fallback = DevResponse::new(
        &response.session_id,
        &response.request_id,
        "failed",
        json!({"error":"response_too_large"}),
    );
    let mut bytes = serde_json::to_vec(&fallback)?;
    bytes.push(b'\n');
    ensure!(
        bytes.len() <= max_bytes,
        "response limit is too small for an error response"
    );
    Ok(bytes)
}
fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 65536];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(())
}
fn ensure_publication_active(cancelled: &AtomicBool) -> Result<()> {
    ensure!(!cancelled.load(Ordering::Acquire), "publication_cancelled");
    Ok(())
}
fn publish_bundle(mut job: Publish) -> Result<Value> {
    ensure_publication_active(&job.cancelled)?;
    let output = job.output.join(format!("{}-{}", job.command, job.name));
    fs::create_dir(&output).context("refusing to replace evidence bundle")?;
    let full = output.join("client.png");
    let viewport = output.join("viewport.png");
    let snapshot = output.join("snapshot.json");
    let manifest = output.join("complete.json");
    let f = &job.frame;
    ensure!(
        job.snapshot
            .development
            .as_ref()
            .and_then(|d| d.submitted_frame)
            == Some(f.submission_id),
        "snapshot/submission mismatch"
    );
    job.snapshot.capture = Some(crate::developer_snapshot::CaptureMetadata {
        scene: job.name.clone(),
        image: "client.png".into(),
        width: f.width,
        height: f.height,
        terrain_updates: 0,
        worker_count: job.snapshot.work.worker_count,
        step_ms: 0,
        morph_duration_ms: job
            .snapshot
            .capture
            .as_ref()
            .map_or(0, |c| c.morph_duration_ms),
        adapter: f.adapter.clone(),
        backend: f.backend.clone(),
    });
    write_png(&full, f.width, f.height, &f.rgba)?;
    ensure_publication_active(&job.cancelled)?;
    let [x, y] = job.snapshot.camera.viewport_origin_pixels;
    let [width, height] = job.snapshot.camera.viewport_size_pixels;
    ensure!(
        x.checked_add(width).is_some_and(|v| v <= f.width)
            && y.checked_add(height).is_some_and(|v| v <= f.height),
        "capture viewport changed"
    );
    let mut crop = Vec::with_capacity(width as usize * height as usize * 4);
    for row in y..y + height {
        let start = (row as usize * f.width as usize + x as usize) * 4;
        crop.extend_from_slice(&f.rgba[start..start + width as usize * 4]);
    }
    write_png(&viewport, width, height, &crop)?;
    ensure_publication_active(&job.cancelled)?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&snapshot)?;
    file.write_all(&serde_json::to_vec_pretty(&job.snapshot)?)?;
    ensure_publication_active(&job.cancelled)?;
    let data = json!({"capture_id":job.command,"source":"native_surface_after_scene_and_ui","source_frame":f.submission_id,"prepared_frame":job.snapshot.general.frame_number,"world_revision":job.snapshot.general.world_revision,"width":f.width,"height":f.height,"adapter":f.adapter,"backend":f.backend,"full_image":full,"viewport_image":viewport,"snapshot":snapshot,"manifest":manifest});
    ensure_publication_active(&job.cancelled)?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&manifest)?;
    file.write_all(&serde_json::to_vec_pretty(&data)?)?;
    file.sync_all()?;
    if job.name.starts_with("bad-frame-") {
        let root = std::env::var_os("ASTRUM_CAPTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/terrain-captures"));
        let destination = root
            .join(format!("process-{}", std::process::id()))
            .join(&job.name);
        fs::create_dir_all(&destination)?;
        fs::copy(&viewport, destination.join("screenshot.png"))?;
        fs::copy(&full, destination.join("client.png"))?;
        fs::copy(&snapshot, destination.join("screenshot-snapshot.json"))?;
        fs::copy(&manifest, destination.join("screenshot-complete.json"))?;
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> DeveloperService {
        let (_, requests) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (publish, _) = mpsc::sync_channel(1);
        let (_, published) = mpsc::sync_channel(1);
        DeveloperService {
            descriptor: SessionDescriptor {
                protocol_version: 1,
                session_id: "test".into(),
                endpoint: "127.0.0.1:1".into(),
                pid: 1,
                preset: "solar-system".into(),
                executable: "test".into(),
                output_directory: "test".into(),
                binary_sha256: "test".into(),
                build_manifest: None,
            },
            requests,
            stopped: Arc::new(AtomicBool::new(false)),
            registry_file: std::env::temp_dir()
                .join(format!("astrum-test-{}-absent", std::process::id())),
            lease: None,
            sequence: 0,
            receipts: VecDeque::new(),
            events: VecDeque::new(),
            event_sequence: 0,
            observation: None,
            capture: None,
            publish,
            published,
            shutdown_requested: false,
        }
    }
    #[test]
    fn retained_failures_survive_success_and_history_is_bounded() {
        let mut service = state();
        let failure = service.receipt("failed", 3, json!({"error":"invalid_handle"}));
        for _ in 0..HISTORY_CAPACITY - 1 {
            service.receipt("applied", 4, json!({}));
            service.event("applied", json!({}));
        }
        assert_eq!(service.receipts.front().unwrap().command_id, failure);
        assert_eq!(service.receipts.front().unwrap().status, "failed");
        service.receipt("applied", 5, json!({}));
        assert_eq!(service.receipts.len(), HISTORY_CAPACITY);
        assert!(!service.receipts.iter().any(|r| r.command_id == failure));
        service.event("applied", json!({}));
        service.event("applied", json!({}));
        assert_eq!(service.events.len(), HISTORY_CAPACITY);
    }
    #[test]
    fn active_capture_receipt_is_pinned_within_bounded_history() {
        let mut service = state();
        let command = service.receipt("accepted", 3, json!({}));
        service.capture = Some(Capture {
            id: 1,
            command: command.clone(),
            name: "capture".into(),
            snapshot: None,
            deadline: Instant::now() + Duration::from_secs(30),
            publishing: true,
            cancelled: Arc::new(AtomicBool::new(false)),
        });

        for revision in 0..HISTORY_CAPACITY {
            service.receipt("applied", revision as u64, json!({}));
        }

        assert_eq!(service.receipts.len(), HISTORY_CAPACITY);
        let active = service
            .receipts
            .iter()
            .find(|receipt| receipt.command_id == command)
            .expect("in-flight capture receipt remains queryable");
        assert_eq!(active.status, "accepted");
    }

    #[test]
    fn oversized_response_falls_back_with_original_identity() {
        let response = DevResponse::new(
            "session-1",
            "request-2",
            "ok",
            json!({"data":"x".repeat(1024)}),
        );

        let line = encode_response_line(&response, 256).unwrap();
        assert!(line.len() <= 256);
        assert_eq!(line.last(), Some(&b'\n'));
        let fallback: DevResponse = serde_json::from_slice(&line).unwrap();
        assert_eq!(fallback.protocol_version, PROTOCOL_VERSION);
        assert_eq!(fallback.session_id, "session-1");
        assert_eq!(fallback.request_id, "request-2");
        assert_eq!(fallback.status, "failed");
        assert_eq!(fallback.data["error"], "response_too_large");
    }

    #[test]
    fn late_publication_cannot_replace_a_terminal_receipt() {
        let mut service = state();
        let cancelled_command = service.receipt("accepted", 3, json!({}));
        service.complete(
            &cancelled_command,
            "cancelled",
            json!({"reason":"human_stop"}),
        );
        let failed_command = service.receipt("accepted", 3, json!({}));
        service.complete(&failed_command, "failed", json!({"error":"write failed"}));
        let applied_command = service.receipt("applied", 3, json!({"source_frame":7}));
        let event_count = service.events.len();

        service.complete(&cancelled_command, "applied", json!({"source_frame":42}));
        service.complete(&failed_command, "applied", json!({"source_frame":43}));
        service.complete(
            &applied_command,
            "failed",
            json!({"error":"late publisher error"}),
        );

        let cancelled = service
            .receipts
            .iter()
            .find(|r| r.command_id == cancelled_command)
            .unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert_eq!(cancelled.data, json!({"reason":"human_stop"}));
        let failed = service
            .receipts
            .iter()
            .find(|r| r.command_id == failed_command)
            .unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.data, json!({"error":"write failed"}));
        let applied = service
            .receipts
            .iter()
            .find(|r| r.command_id == applied_command)
            .unwrap();
        assert_eq!(applied.status, "applied");
        assert_eq!(applied.data, json!({"source_frame":7}));
        assert_eq!(service.events.len(), event_count);
    }

    #[test]
    fn cancellation_flag_stops_publication_before_completion() {
        let cancelled = Arc::new(AtomicBool::new(false));
        ensure_publication_active(&cancelled).unwrap();
        cancelled.store(true, Ordering::Release);
        let error = ensure_publication_active(&cancelled).unwrap_err();
        assert!(error.to_string().contains("publication_cancelled"));
    }

    #[test]
    fn lease_is_exclusive_and_deadline_checked() {
        let mut service = state();
        service.lease = Some(Lease {
            token: "one".into(),
            owner: "test".into(),
            expires: Instant::now() + Duration::from_secs(1),
        });
        assert!(service.owns("one"));
        assert!(!service.owns("other"));
        service.lease.as_mut().unwrap().expires = Instant::now();
        assert!(!service.owns("one"));
    }

    #[test]
    fn observation_freshness_tracks_age_drawability_revision_and_sequence() {
        let mut development = DevelopmentSnapshot {
            session_id: "test".into(),
            observation_age_ms: 0.0,
            stale: false,
            drawable: true,
            command_sequence: 7,
            prepared_frame: 12,
            submitted_frame: Some(12),
            presentation_requested: true,
            capture_id: None,
            current_errors: Vec::new(),
            asynchronous_measurement_source: "latest_completed".into(),
            gpu_measurement_source_frame: Some(11),
        };

        refresh_freshness(&mut development, Duration::from_millis(250), true, 9, 9, 7);
        assert!(!development.stale, "the 250 ms boundary is still fresh");
        assert_eq!(development.observation_age_ms, 250.0);

        refresh_freshness(&mut development, Duration::from_millis(251), true, 9, 9, 7);
        assert!(development.stale, "older observations must be stale");

        refresh_freshness(&mut development, Duration::ZERO, false, 9, 9, 7);
        assert!(
            development.stale,
            "a minimized/non-drawable observation is stale"
        );
        assert!(!development.drawable);

        refresh_freshness(&mut development, Duration::ZERO, true, 9, 10, 7);
        assert!(
            development.stale,
            "world revision changes stale the observation"
        );

        refresh_freshness(&mut development, Duration::ZERO, true, 9, 9, 8);
        assert!(
            development.stale,
            "a newer command sequence stales the observation"
        );
    }

    #[test]
    fn transport_bounds_bad_oversized_disconnected_and_saturated_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let descriptor = state().descriptor.clone();
        let (send, _queue) = mpsc::sync_channel(0);
        let (disconnected_send, disconnected_queue) = mpsc::sync_channel(REQUEST_CAPACITY);
        drop(disconnected_queue);
        let worker = thread::spawn(move || {
            for index in 0..4 {
                let (socket, _) = listener.accept().unwrap();
                let sender = if index == 3 {
                    disconnected_send.clone()
                } else {
                    send.clone()
                };
                serve(socket, &descriptor, &sender, &|| {}).unwrap();
            }
        });
        let valid = b"{\"protocol_version\":1,\"session_id\":\"test\",\"request_id\":\"request\",\"operation\":{\"op\":\"inspect\"}}\n";
        let mut oversized = vec![b'a'; MAX_MESSAGE_BYTES + 1];
        oversized.push(b'\n');
        for (bytes, expected) in [
            (valid.as_slice(), "queue_overflow"),
            (b"\xff\n".as_slice(), "invalid_request"),
            (oversized.as_slice(), "malformed_or_oversized_message"),
            (valid.as_slice(), "session_closed"),
        ] {
            let mut socket = TcpStream::connect(address).unwrap();
            socket.write_all(bytes).unwrap();
            let mut line = String::new();
            BufReader::new(socket).read_line(&mut line).unwrap();
            let response: DevResponse = serde_json::from_str(&line).unwrap();
            assert_eq!(response.status, "failed");
            assert_eq!(response.data["error"], expected);
        }
        worker.join().unwrap();
    }
}
