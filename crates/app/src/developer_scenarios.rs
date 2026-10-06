//! Declarative development scenarios and owned-process replay.
//!
//! This module orchestrates the production application through its development
//! protocol. It does not construct an alternate native engine.

use crate::developer_protocol::{
    BodyInventory, DevCommand, DevOperation, DevResponse, LEASE_SECONDS, SessionDescriptor,
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, OnceLock, atomic::AtomicBool},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_STEPS: usize = 256;
const MAX_WAIT_SECONDS: f64 = 300.0;
const MAX_ACTION_SECONDS: f64 = 60.0;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_BUILD_DURATION: Duration = Duration::from_secs(30 * 60);
const DEFAULT_WIDTH: u32 = 960;
const DEFAULT_HEIGHT: u32 = 640;
const OUTPUT_CLAIM_FILE: &str = ".mundaris-scenario-claim";

static OWNED_CHILDREN: OnceLock<Mutex<HashMap<u32, Child>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema: u32,
    pub preset: String,
    #[serde(default)]
    pub initial_settings: Vec<DevCommand>,
    #[serde(default)]
    pub deterministic: bool,
    pub steps: Vec<ScenarioStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenarioStep {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub action: Option<DevCommand>,
    #[serde(default)]
    pub wait: Option<WaitStep>,
    #[serde(default)]
    pub capture: Option<CaptureStep>,
    #[serde(default)]
    pub checkpoint: Option<CheckpointStep>,
    #[serde(default)]
    pub duration_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaitStep {
    pub predicate: Predicate,
    #[serde(default = "default_timeout")]
    pub timeout_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStep {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointStep {
    pub name: String,
    pub predicate: Predicate,
    #[serde(default = "default_timeout")]
    pub timeout_s: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    /// Dot-separated path into the latest developer snapshot.
    pub path: String,
    #[serde(default)]
    pub equals: Option<Value>,
    #[serde(default)]
    pub approximately: Option<f64>,
    #[serde(default)]
    pub tolerance: f64,
    #[serde(default)]
    pub at_least: Option<f64>,
    #[serde(default)]
    pub at_most: Option<f64>,
}

fn default_timeout() -> f64 {
    30.0
}

impl Scenario {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("reading scenario {}", path.display()))?;
        let scenario: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing scenario {}", path.display()))?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1,
            "unsupported developer scenario schema {}",
            self.schema
        );
        ensure!(
            matches!(
                self.preset.as_str(),
                "solar-system" | "real-solar-system" | "gravity-orbits" | "gravity-hierarchy"
            ),
            "unsupported preset {:?}",
            self.preset
        );
        ensure!(!self.steps.is_empty(), "scenario has no steps");
        ensure!(
            self.initial_settings.len() <= 64,
            "scenario exceeds 64 initial settings"
        );
        ensure!(
            self.steps.len() <= MAX_STEPS,
            "scenario exceeds {MAX_STEPS} steps"
        );
        let mut artifacts = HashSet::new();
        for (index, step) in self.steps.iter().enumerate() {
            let variants = usize::from(step.action.is_some())
                + usize::from(step.wait.is_some())
                + usize::from(step.capture.is_some())
                + usize::from(step.checkpoint.is_some());
            ensure!(
                variants == 1,
                "step {index} must contain exactly one operation"
            );
            ensure!(
                step.duration_s.is_finite()
                    && (0.0..=MAX_ACTION_SECONDS).contains(&step.duration_s),
                "step {index} duration_s must be finite and in 0..={MAX_ACTION_SECONDS}"
            );
            if let Some(wait) = &step.wait {
                wait.validate(index)?;
            }
            if let Some(checkpoint) = &step.checkpoint {
                checkpoint.validate(index)?;
            }
            if let Some(capture) = &step.capture {
                validate_artifact_name(&capture.name)
                    .with_context(|| format!("capture in step {index}"))?;
                ensure!(
                    artifacts.insert(capture.name.to_lowercase()),
                    "duplicate scenario artifact name {:?}",
                    capture.name
                );
            }
            if let Some(checkpoint) = &step.checkpoint {
                ensure!(
                    artifacts.insert(checkpoint.name.to_lowercase()),
                    "duplicate scenario artifact name {:?}",
                    checkpoint.name
                );
            }
            if let Some(name) = &step.name {
                validate_name(name).with_context(|| format!("name in step {index}"))?;
            }
        }
        Ok(())
    }
}

impl WaitStep {
    fn validate(&self, index: usize) -> Result<()> {
        validate_timeout(self.timeout_s, index)?;
        self.predicate.validate(index)
    }
}

impl CheckpointStep {
    fn validate(&self, index: usize) -> Result<()> {
        validate_artifact_name(&self.name)
            .with_context(|| format!("checkpoint in step {index}"))?;
        validate_timeout(self.timeout_s, index)?;
        self.predicate.validate(index)
    }
}

impl Predicate {
    fn validate(&self, index: usize) -> Result<()> {
        ensure!(
            !self.path.is_empty(),
            "predicate in step {index} has empty path"
        );
        let selectors = usize::from(self.equals.is_some())
            + usize::from(self.approximately.is_some())
            + usize::from(self.at_least.is_some())
            + usize::from(self.at_most.is_some());
        ensure!(
            selectors > 0,
            "predicate in step {index} needs an expected value or numeric bound"
        );
        ensure!(
            !(self.equals.is_some()
                && (self.approximately.is_some()
                    || self.at_least.is_some()
                    || self.at_most.is_some())),
            "predicate in step {index} cannot combine equals with numeric bounds"
        );
        ensure!(
            self.approximately.is_none_or(f64::is_finite)
                && self.at_least.is_none_or(f64::is_finite)
                && self.at_most.is_none_or(f64::is_finite)
                && self.tolerance.is_finite()
                && self.tolerance >= 0.0,
            "predicate in step {index} has invalid numeric tolerance"
        );
        ensure!(
            self.at_least
                .zip(self.at_most)
                .is_none_or(|(minimum, maximum)| minimum <= maximum),
            "predicate in step {index} has an inverted numeric range"
        );
        Ok(())
    }
}

fn validate_timeout(timeout: f64, index: usize) -> Result<()> {
    ensure!(
        timeout.is_finite() && timeout > 0.0 && timeout <= MAX_WAIT_SECONDS,
        "step {index} timeout_s must be finite and in (0, {MAX_WAIT_SECONDS}]"
    );
    Ok(())
}

fn validate_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 80
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "invalid scenario artifact name {value:?}"
    );
    Ok(())
}

fn validate_artifact_name(value: &str) -> Result<()> {
    validate_name(value)?;
    ensure!(
        ![
            "progress",
            "scenario",
            "scenario-result",
            "session",
            "owned-session",
            "build-manifest"
        ]
        .contains(&value.to_ascii_lowercase().as_str()),
        "scenario artifact name {value:?} is reserved"
    );
    Ok(())
}

fn assert_owned_session(session: &SessionDescriptor) -> Result<()> {
    let ownership_path = Path::new(&session.output_directory).join("owned-session.json");
    let ownership: Value =
        serde_json::from_slice(&fs::read(&ownership_path).with_context(|| {
            format!(
                "session is not owned by this launcher: {}",
                ownership_path.display()
            )
        })?)?;
    ensure!(
        ownership.get("session_id").and_then(Value::as_str) == Some(session.session_id.as_str())
            && ownership.get("pid").and_then(Value::as_u64) == Some(session.pid as u64)
            && ownership.get("binary_sha256").and_then(Value::as_str)
                == Some(session.binary_sha256.as_str()),
        "ownership marker does not match the selected process"
    );
    Ok(())
}

pub fn launch_owned(preset: &str, registry: &Path, output: &Path) -> Result<SessionDescriptor> {
    launch_owned_cancellable(preset, registry, output, None)
}

pub fn launch_owned_cancellable(
    preset: &str,
    registry: &Path,
    output: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<SessionDescriptor> {
    check_cancelled(cancelled)?;
    ensure!(
        matches!(
            preset,
            "solar-system" | "real-solar-system" | "gravity-orbits" | "gravity-hierarchy"
        ),
        "unsupported preset {preset:?}"
    );
    let registry = absolute_from_cwd(registry)?;
    let output = absolute_from_cwd(output)?;
    create_fresh_dir(&output)?;
    let repo = repository_root()?;
    check_cancelled(cancelled)?;
    let source_before = source_attribution(&repo)?;
    let build_started = Instant::now();
    let mut build_command = Command::new("cargo");
    build_command.current_dir(&repo).args([
        "build",
        "--locked",
        "--release",
        "-p",
        "mundaris_app",
        "--bin",
        "mundaris_app",
        "--features",
        "developer-tools",
    ]);
    // The manifest and copy must refer to this build, regardless of an inherited
    // Cargo output-directory or target override from the caller's shell.
    build_command
        .arg("--target-dir")
        .arg(repo.join("target"))
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET");
    let build_status = run_build_cancellable(
        build_command,
        &output.join("build.stdout.txt"),
        &output.join("build.stderr.txt"),
        cancelled,
    )
    .context("running locked release developer-tools build")?;
    ensure!(
        build_status.success(),
        "locked release build failed with {build_status}"
    );
    check_cancelled(cancelled)?;
    let source_after = source_attribution(&repo)?;
    ensure!(
        source_before == source_after,
        "repository inputs changed during build; refusing to attribute executable to an unstable source tree"
    );
    let mut build_manifest = source_after;
    build_manifest["build_elapsed_s"] = json!(build_started.elapsed().as_secs_f64());
    build_manifest["command"] = json!(format!(
        "cargo build --locked --release -p mundaris_app --bin mundaris_app --features developer-tools --target-dir {}",
        repo.join("target").display()
    ));
    for (key, program, args) in [
        ("rustc_version", "rustc", vec!["-vV"]),
        ("cargo_version", "cargo", vec!["--version"]),
    ] {
        let version = Command::new(program)
            .args(args)
            .output()
            .with_context(|| format!("recording {program} version"))?;
        ensure!(
            version.status.success(),
            "could not record {program} version"
        );
        build_manifest[key] = json!(String::from_utf8(version.stdout)?);
    }
    let built = executable_path(&repo);
    ensure!(
        built.is_file(),
        "built executable missing: {}",
        built.display()
    );
    let immutable_exe = output.join(executable_name());
    fs::copy(&built, &immutable_exe).context("copying immutable executable for owned run")?;
    let binary_sha256 = sha256_file(&immutable_exe)?;
    build_manifest["binary_sha256"] = json!(binary_sha256);
    build_manifest["immutable_executable"] = json!(immutable_exe.display().to_string());
    let build_manifest_path = output.join("build-manifest.json");
    write_json(build_manifest_path.clone(), &build_manifest)?;
    let command_arg = match preset {
        "gravity-hierarchy" | "gravity-orbits" => "--gravity-orbits",
        "real-solar-system" => "--real-solar-system",
        _ => "--solar-system",
    };
    let mut command = Command::new(&immutable_exe);
    command
        .current_dir(&repo)
        .arg(command_arg)
        .arg("--dev-interface")
        .env("MUNDARIS_DEV_REGISTRY", &registry)
        .env("MUNDARIS_DEV_OUTPUT", &output)
        .env("MUNDARIS_DEV_BUILD_MANIFEST", &build_manifest_path)
        .stdout(Stdio::from(fs::File::create(output.join("stdout.txt"))?))
        .stderr(Stdio::from(fs::File::create(output.join("stderr.txt"))?));
    let mut child = command
        .spawn()
        .context("starting owned native application")?;
    let descriptor = match wait_for_session(
        &registry,
        &mut child,
        preset,
        &immutable_exe,
        &binary_sha256,
        cancelled,
    ) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            if let Ok(sessions) = crate::developer_bridge::discover(&registry) {
                for session in sessions
                    .into_iter()
                    .filter(|session| session.pid == child.id())
                {
                    remove_owned_descriptor(&registry, &session);
                }
            }
            if let Err(cleanup) = terminate_owned_child(&mut child) {
                return Err(error.context(format!("owned child cleanup failed: {cleanup:#}")));
            }
            return Err(error);
        }
    };
    let launch_result = (|| -> Result<()> {
        check_cancelled(cancelled)?;
        let capabilities = checked_response(
            crate::developer_bridge::Client::new(descriptor.clone())
                .request(DevOperation::Capabilities)?,
        )?;
        check_cancelled(cancelled)?;
        write_json(
            output.join("launch-manifest.json"),
            &json!({"session":descriptor,"build":build_manifest,"runtime_capabilities":capabilities}),
        )?;
        write_json(output.join("session.json"), &descriptor)?;
        write_json(
            output.join("owned-session.json"),
            &json!({
                "session_id":descriptor.session_id,
                "pid":descriptor.pid,
                "executable":descriptor.executable,
                "binary_sha256":descriptor.binary_sha256,
                "output_directory":descriptor.output_directory
            }),
        )?;
        Ok(())
    })();
    if let Err(error) = launch_result {
        remove_owned_descriptor(&registry, &descriptor);
        if let Err(cleanup) = terminate_owned_child(&mut child) {
            return Err(error.context(format!("owned child cleanup failed: {cleanup:#}")));
        }
        return Err(error);
    }
    OWNED_CHILDREN
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("owned process registry lock poisoned"))?
        .insert(descriptor.pid, child);
    Ok(descriptor)
}

fn run_build_cancellable(
    mut command: Command,
    stdout_path: &Path,
    stderr_path: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<std::process::ExitStatus> {
    check_cancelled(cancelled)?;
    let stdout = fs::File::create(stdout_path)?;
    let stderr = fs::File::create(stderr_path)?;
    let mut child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .context("spawning locked Cargo build")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if is_cancelled(cancelled) {
            terminate_owned_child(&mut child)?;
            bail!("scenario launch cancelled during Cargo build")
        }
        if started.elapsed() >= MAX_BUILD_DURATION {
            terminate_owned_child(&mut child)?;
            bail!("locked Cargo build exceeded 30-minute limit")
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn stop_owned(session: &SessionDescriptor, registry: &Path) -> Result<Value> {
    stop_owned_cancellable(session, registry, None)
}

pub fn stop_owned_cancellable(
    session: &SessionDescriptor,
    registry: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    check_cancelled(cancelled)?;
    assert_owned_session(session)?;
    let client = crate::developer_bridge::Client::new(session.clone());
    let mut lease = acquire_lease(&client, "scenario-runner")?;
    lease.renew_if_due(&client)?;
    check_cancelled(cancelled)?;
    let response = checked_response(client.request(DevOperation::Shutdown {
        lease: lease.token.clone(),
    })?)?;
    let timeout = Duration::from_secs(30);
    let start = Instant::now();
    let mut cancellation_observed_after_shutdown = false;
    loop {
        cancellation_observed_after_shutdown |= is_cancelled(cancelled);
        let owned = crate::developer_bridge::discover(registry)?
            .into_iter()
            .any(|found| found.session_id == session.session_id && found.pid == session.pid);
        if let Some(status) = wait_owned_child(session.pid)? {
            ensure!(
                status.success(),
                "owned application exited unsuccessfully: {status}"
            );
            return Ok(
                json!({"shutdown":response,"process_exited":true,"exit_status":status.to_string(),"cancellation_observed_after_shutdown":cancellation_observed_after_shutdown}),
            );
        }
        if !owned && !process_exists(session.pid)? {
            return Ok(
                json!({"shutdown":response,"process_exited":true,"cancellation_observed_after_shutdown":cancellation_observed_after_shutdown}),
            );
        }
        ensure!(
            start.elapsed() < timeout,
            "owned application did not exit within 30 seconds"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn run_native(
    session: &SessionDescriptor,
    scenario: &Scenario,
    output: &Path,
) -> Result<Value> {
    run_native_cancellable(session, scenario, output, None)
}

pub fn run_native_cancellable(
    session: &SessionDescriptor,
    scenario: &Scenario,
    output: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    check_cancelled(cancelled)?;
    scenario.validate()?;
    ensure!(
        normalize_preset(&session.preset) == normalize_preset(&scenario.preset),
        "scenario preset does not match owned session"
    );
    let output = absolute_from_cwd(output)?;
    create_fresh_dir(&output)?;
    let mut host = NativeHost::new(session, cancelled)?;
    execute_scenario(&mut host, scenario, &output, cancelled)
}

pub fn run_offscreen(scenario: &Scenario, output: &Path) -> Result<Value> {
    run_offscreen_cancellable(scenario, output, None)
}

pub fn run_offscreen_cancellable(
    scenario: &Scenario,
    output: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    check_cancelled(cancelled)?;
    scenario.validate()?;
    ensure!(
        scenario.deterministic,
        "offscreen repeat requires a scenario declared deterministic"
    );
    let output = absolute_from_cwd(output)?;
    create_fresh_dir(&output)?;
    check_cancelled(cancelled)?;
    let mut host = OffscreenHost::new(&scenario.preset)?;
    execute_scenario(&mut host, scenario, &output, cancelled)
}

trait ScenarioHost {
    fn resolve_command(&self, command: DevCommand) -> Result<DevCommand>;
    fn action(
        &mut self,
        command: DevCommand,
        duration_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value>;
    fn observe(
        &mut self,
        predicate: &Predicate,
        timeout_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value>;
    fn capture(
        &mut self,
        name: &str,
        output: &Path,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value>;
    fn elapsed_metadata(&self) -> Value;
    fn result_metadata(&self) -> Value;
    fn finish(&mut self) -> Result<()>;
}

fn execute_scenario(
    host: &mut impl ScenarioHost,
    scenario: &Scenario,
    output: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    let mut scenario_sha256 = String::new();
    let mut records = Vec::new();
    let run_result = (|| -> Result<Value> {
        let scenario_bytes = serde_json::to_vec_pretty(scenario)?;
        scenario_sha256 = sha256_bytes(&scenario_bytes);
        write_json(output.join("scenario.json"), scenario)?;
        for (index, command) in scenario.initial_settings.iter().enumerate() {
            check_cancelled(cancelled)?;
            let command = host.resolve_command(command.clone())?;
            let mut record =
                json!({"kind":"initial_setting","setting_index":index,"command":command});
            record["result"] = host.action(command, 0.0, cancelled)?;
            record["elapsed"] = host.elapsed_metadata();
            records.push(record);
            write_progress(
                output.join("progress.json"),
                &json!({"completed_steps":records}),
            )?;
        }
        for (index, step) in scenario.steps.iter().enumerate() {
            check_cancelled(cancelled)?;
            let mut record = if let Some(command) = &step.action {
                let command = host.resolve_command(command.clone())?;
                json!({"index":index,"kind":"action","command":command,"result":host.action(command,step.duration_s,cancelled)?})
            } else if let Some(wait) = &step.wait {
                json!({"index":index,"kind":"wait","timeout_s":wait.timeout_s,"snapshot":host.observe(&wait.predicate,wait.timeout_s,cancelled)?})
            } else if let Some(capture) = &step.capture {
                json!({"index":index,"kind":"capture","name":capture.name,"result":host.capture(&capture.name,output,cancelled)?})
            } else if let Some(checkpoint) = &step.checkpoint {
                let snapshot =
                    host.observe(&checkpoint.predicate, checkpoint.timeout_s, cancelled)?;
                write_json(output.join(format!("{}.json", checkpoint.name)), &snapshot)?;
                json!({"index":index,"kind":"checkpoint","name":checkpoint.name,"timeout_s":checkpoint.timeout_s,"snapshot":snapshot})
            } else {
                bail!("validated scenario step lost its operation")
            };
            record["elapsed"] = host.elapsed_metadata();
            records.push(record);
            write_progress(
                output.join("progress.json"),
                &json!({"completed_steps":records}),
            )?;
        }
        Ok(
            json!({"schema":1,"status":"completed","scenario_preset":scenario.preset,"scenario_sha256":scenario_sha256,"declared_deterministic":scenario.deterministic,"host":host.result_metadata(),"steps":records}),
        )
    })();
    let finish_result = host.finish();
    match run_result {
        Ok(result) => match finish_result {
            Ok(()) => {
                write_json(output.join("scenario-result.json"), &result)?;
                Ok(result)
            }
            Err(error) => {
                let failure = json!({"schema":1,"status":"failed","error":format!("{error:#}"),"completed_steps":records});
                write_json(output.join("scenario-result.json"), &failure)?;
                Err(error.context("scenario completed but host finalization failed"))
            }
        },
        Err(error) => {
            let mut failure = json!({"schema":1,"status":"failed","error":format!("{error:#}"),"completed_steps":records});
            if let Err(finish_error) = finish_result {
                failure["finish_error"] = json!(format!("{finish_error:#}"));
            }
            write_json(output.join("scenario-result.json"), &failure)?;
            Err(error.context("scenario failed; host finalization was attempted"))
        }
    }
}

struct NativeHost {
    session: SessionDescriptor,
    client: crate::developer_bridge::Client,
    lease: ControlLease,
    handles: HashMap<String, String>,
    minimum_sequence: u64,
    started: Instant,
    lease_released: bool,
}

impl NativeHost {
    fn new(session: &SessionDescriptor, cancelled: Option<&AtomicBool>) -> Result<Self> {
        check_cancelled(cancelled)?;
        let client = crate::developer_bridge::Client::new(session.clone());
        let lease = acquire_lease(&client, "scenario-runner")?;
        if let Err(error) = check_cancelled(cancelled) {
            let _ = client.request(DevOperation::ReleaseControl { lease: lease.token });
            return Err(error);
        }
        let inventory = match inspect_inventory(&client) {
            Ok(inventory) => inventory,
            Err(error) => {
                let _ = client.request(DevOperation::ReleaseControl { lease: lease.token });
                return Err(error.context("inspecting native scenario inventory"));
            }
        };
        let handles = match body_handles(&inventory) {
            Ok(handles) => handles,
            Err(error) => {
                let _ = client.request(DevOperation::ReleaseControl { lease: lease.token });
                return Err(error.context("building native scenario body handles"));
            }
        };
        Ok(Self {
            session: session.clone(),
            client,
            lease,
            handles,
            minimum_sequence: 0,
            started: Instant::now(),
            lease_released: false,
        })
    }

    fn update_sequence(&mut self, receipt: &Value) -> Result<()> {
        self.minimum_sequence = receipt
            .get("sequence")
            .and_then(Value::as_u64)
            .context("receipt lacks sequence")?;
        Ok(())
    }
}

impl ScenarioHost for NativeHost {
    fn resolve_command(&self, command: DevCommand) -> Result<DevCommand> {
        resolve_command(command, &self.handles)
    }

    fn action(
        &mut self,
        command: DevCommand,
        duration_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        check_cancelled(cancelled)?;
        let accepted = checked_response(self.client.request(DevOperation::Action {
            lease: self.lease.token.clone(),
            command,
        })?)?;
        let command_id = accepted
            .get("command_id")
            .and_then(Value::as_str)
            .context("action response lacks command_id")?;
        let receipt = await_receipt(
            &self.client,
            &mut self.lease,
            command_id,
            Duration::from_secs(20),
            cancelled,
        )?;
        self.update_sequence(&receipt)?;
        if duration_s > 0.0 {
            sleep_with_renewal(
                &self.client,
                &mut self.lease,
                Duration::from_secs_f64(duration_s),
                cancelled,
            )?;
        }
        Ok(json!({"accepted":accepted,"receipt":receipt,"duration_s":duration_s}))
    }

    fn observe(
        &mut self,
        predicate: &Predicate,
        timeout_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout_s);
        loop {
            check_cancelled(cancelled)?;
            self.lease.renew_if_due(&self.client)?;
            let snapshot = self.read_snapshot()?;
            if fresh_at(&snapshot, self.minimum_sequence)
                && predicate_matches(&snapshot, predicate)?
            {
                return Ok(snapshot);
            }
            ensure!(
                Instant::now() < deadline,
                "fresh observation predicate timed out: {}",
                predicate.path
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn capture(
        &mut self,
        name: &str,
        _output: &Path,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        self.wait_fresh(Duration::from_secs(30), cancelled)?;
        check_cancelled(cancelled)?;
        let accepted = checked_response(self.client.request(DevOperation::Capture {
            lease: self.lease.token.clone(),
            name: name.to_owned(),
        })?)?;
        let command_id = accepted
            .get("command_id")
            .and_then(Value::as_str)
            .context("capture response lacks command_id")?;
        let receipt = match await_receipt(
            &self.client,
            &mut self.lease,
            command_id,
            Duration::from_secs(30),
            cancelled,
        ) {
            Ok(receipt) => receipt,
            Err(error) if is_cancelled(cancelled) => {
                let cancellation = self.client.request(DevOperation::Cancel {
                    lease: self.lease.token.clone(),
                });
                self.lease_released = cancellation.and_then(checked_response).is_ok();
                return Err(
                    error.context("capture wait cancelled; command cancellation was requested")
                );
            }
            Err(error) => return Err(error),
        };
        self.update_sequence(&receipt)?;
        Ok(json!({"accepted":accepted,"receipt":receipt}))
    }

    fn elapsed_metadata(&self) -> Value {
        json!({"elapsed_s":self.started.elapsed().as_secs_f64()})
    }

    fn result_metadata(&self) -> Value {
        json!({"mode":"native","clock_mode":"wall-paced","deterministic":false,"session_id":self.session.session_id,"pid":self.session.pid,"binary_sha256":self.session.binary_sha256,"build_manifest":self.session.build_manifest,"elapsed_s":self.started.elapsed().as_secs_f64()})
    }

    fn finish(&mut self) -> Result<()> {
        if self.lease_released {
            return Ok(());
        }
        checked_response(self.client.request(DevOperation::ReleaseControl {
            lease: self.lease.token.clone(),
        })?)?;
        Ok(())
    }
}

impl NativeHost {
    fn read_snapshot(&self) -> Result<Value> {
        let inspect = checked_response(self.client.request(DevOperation::Inspect)?)?;
        Ok(inspect.get("snapshot").unwrap_or(&inspect).clone())
    }

    fn wait_fresh(&mut self, timeout: Duration, cancelled: Option<&AtomicBool>) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            check_cancelled(cancelled)?;
            self.lease.renew_if_due(&self.client)?;
            let snapshot = self.read_snapshot()?;
            if fresh_at(&snapshot, self.minimum_sequence) {
                return Ok(snapshot);
            }
            ensure!(
                Instant::now() < deadline,
                "fresh native observation timed out"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }
}

fn fresh_at(snapshot: &Value, minimum_sequence: u64) -> bool {
    let development = snapshot.get("development");
    development
        .and_then(|d| d.get("stale"))
        .and_then(Value::as_bool)
        == Some(false)
        && development
            .and_then(|d| d.get("command_sequence"))
            .and_then(Value::as_u64)
            .is_some_and(|sequence| sequence >= minimum_sequence)
}

struct OffscreenHost {
    demo: crate::GravityOrbitsDemo,
    latest: crate::developer_capture::DeveloperCapture,
    handles: HashMap<String, String>,
    session_id: String,
    elapsed_frames: u64,
}

impl OffscreenHost {
    fn new(preset: &str) -> Result<Self> {
        let mut demo = make_demo(preset)?;
        let session_id = format!("offscreen-{}-{}", std::process::id(), timestamp_nonce());
        demo.developer_set_session(&session_id);
        let inventory = demo.developer_inventory(&session_id)?;
        let latest =
            demo.developer_offscreen_frame(Duration::ZERO, DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
        Ok(Self {
            demo,
            latest,
            handles: body_handles(&inventory)?,
            session_id,
            elapsed_frames: 0,
        })
    }

    fn advance(&mut self, frames: u64, cancelled: Option<&AtomicBool>) -> Result<()> {
        for _ in 0..frames {
            check_cancelled(cancelled)?;
            self.latest = self.demo.developer_offscreen_frame(
                Duration::from_nanos(16_666_667),
                DEFAULT_WIDTH,
                DEFAULT_HEIGHT,
            )?;
            self.elapsed_frames += 1;
        }
        Ok(())
    }
}

impl ScenarioHost for OffscreenHost {
    fn resolve_command(&self, command: DevCommand) -> Result<DevCommand> {
        resolve_command(command, &self.handles)
    }

    fn action(
        &mut self,
        command: DevCommand,
        duration_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        check_cancelled(cancelled)?;
        self.demo.developer_apply_command(&command)?;
        let frames = ((duration_s * 60.0).ceil() as u64).max(1);
        self.advance(frames, cancelled)?;
        Ok(
            json!({"command":command,"fixed_steps":frames,"fixed_step_elapsed_s":self.elapsed_frames as f64 / 60.0}),
        )
    }

    fn observe(
        &mut self,
        predicate: &Predicate,
        timeout_s: f64,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        let deadline = (timeout_s * 60.0).ceil() as u64;
        for checked in 0..=deadline {
            check_cancelled(cancelled)?;
            let snapshot = serde_json::to_value(&self.latest.snapshot)?;
            if predicate_matches(&snapshot, predicate)? {
                return Ok(snapshot);
            }
            if checked < deadline {
                self.advance(1, cancelled)?;
            }
        }
        bail!("offscreen wait predicate timed out: {}", predicate.path)
    }

    fn capture(
        &mut self,
        name: &str,
        output: &Path,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Value> {
        check_cancelled(cancelled)?;
        let metadata = self
            .latest
            .snapshot
            .capture
            .as_mut()
            .context("offscreen frame lacks capture metadata")?;
        metadata.scene = name.to_owned();
        metadata.image = format!("{name}.png");
        crate::developer_capture::write_pair(output, &self.latest)?;
        check_cancelled(cancelled)?;
        self.latest =
            self.demo
                .developer_offscreen_frame(Duration::ZERO, DEFAULT_WIDTH, DEFAULT_HEIGHT)?;
        Ok(
            json!({"image":format!("{name}.png"),"fixed_step_elapsed_s":self.elapsed_frames as f64 / 60.0}),
        )
    }

    fn elapsed_metadata(&self) -> Value {
        json!({"fixed_step_elapsed_s":self.elapsed_frames as f64 / 60.0})
    }

    fn result_metadata(&self) -> Value {
        json!({"mode":"offscreen","clock_mode":"fixed-step","deterministic":true,"fixed_step_hz":60,"fixed_step_elapsed_s":self.elapsed_frames as f64 / 60.0,"session_id":self.session_id})
    }

    fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

pub fn rebuild_and_replay(
    session: &SessionDescriptor,
    scenario: &Scenario,
    registry: &Path,
    output: &Path,
) -> Result<Value> {
    rebuild_and_replay_cancellable(session, scenario, registry, output, None)
}

pub fn rebuild_and_replay_cancellable(
    session: &SessionDescriptor,
    scenario: &Scenario,
    registry: &Path,
    output: &Path,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    check_cancelled(cancelled)?;
    scenario.validate()?;
    create_fresh_dir(output)?;
    assert_owned_session(session)?;
    let previous = session.clone();
    let previous_result =
        run_native_cancellable(&previous, scenario, &output.join("previous"), cancelled)?;
    if let Err(error) = check_cancelled(cancelled) {
        write_json(
            output.join("rebuild-replay.json"),
            &json!({"schema":1,"status":"cancelled_before_shutdown","previous_session_id":previous.session_id,"previous_result":previous_result}),
        )?;
        return Err(error);
    }
    let shutdown = stop_owned_cancellable(&previous, registry, cancelled)?;
    if let Err(error) = check_cancelled(cancelled) {
        write_json(
            output.join("rebuild-replay.json"),
            &json!({"schema":1,"status":"cancelled_after_shutdown","previous_session_id":previous.session_id,"previous_result":previous_result,"shutdown":shutdown}),
        )?;
        return Err(error.context(
            "previous session exited; rebuild/replay was cancelled before launching a replacement",
        ));
    }
    let replay_directory = output.join("replay");
    let relaunched = match launch_owned_cancellable(
        &scenario.preset,
        registry,
        &replay_directory,
        cancelled,
    ) {
        Ok(session) => session,
        Err(error) => {
            write_json(
                output.join("rebuild-replay.json"),
                &json!({"schema":1,"status":"replacement_launch_failed","error":format!("{error:#}"),"previous_session_id":previous.session_id,"shutdown":shutdown}),
            )?;
            return Err(error.context("previous session exited; replacement launch failed"));
        }
    };
    let native_result = match run_native_cancellable(
        &relaunched,
        scenario,
        &output.join("replayed"),
        cancelled,
    ) {
        Ok(result) => result,
        Err(error) if is_cancelled(cancelled) => {
            let cleanup = stop_owned_cancellable(&relaunched, registry, None);
            write_json(
                output.join("rebuild-replay.json"),
                &json!({"schema":1,"status":"cancelled_during_replay","error":format!("{error:#}"),"previous_session_id":previous.session_id,"shutdown":shutdown,"replacement_session_id":relaunched.session_id,"replacement_cleanup":cleanup.map_err(|error| format!("{error:#}"))}),
            )?;
            return Err(error
                .context("replay cancelled; newly launched owned session cleanup was attempted"));
        }
        Err(error) => return Err(error),
    };
    if let Err(error) = check_cancelled(cancelled) {
        let cleanup = stop_owned_cancellable(&relaunched, registry, None);
        write_json(
            output.join("rebuild-replay.json"),
            &json!({"schema":1,"status":"cancelled_after_replay","previous_session_id":previous.session_id,"shutdown":shutdown,"replacement_session_id":relaunched.session_id,"replacement_result":native_result,"replacement_cleanup":cleanup.map_err(|error| format!("{error:#}"))}),
        )?;
        return Err(
            error.context("replay completed; newly launched owned session cleanup was attempted")
        );
    }
    let comparisons = compare_checkpoint_results(scenario, &previous_result, &native_result)?;
    let result = json!({
        "schema":1,"scenario_preset":scenario.preset,
        "previous_session":{"session_id":previous.session_id,"pid":previous.pid,"binary_sha256":previous.binary_sha256,"scenario_result":previous_result,"shutdown":shutdown},
        "replayed_session":{"session_id":relaunched.session_id,"pid":relaunched.pid,"binary_sha256":relaunched.binary_sha256},
        "native_result":native_result,"checkpoint_comparisons":comparisons
    });
    write_json(output.join("rebuild-replay.json"), &result)?;
    Ok(result)
}

fn compare_checkpoint_results(
    scenario: &Scenario,
    previous: &Value,
    replayed: &Value,
) -> Result<Value> {
    let previous_steps = previous
        .get("steps")
        .and_then(Value::as_array)
        .context("previous scenario result has no steps")?;
    let replayed_steps = replayed
        .get("steps")
        .and_then(Value::as_array)
        .context("replayed scenario result has no steps")?;
    let mut comparisons = Vec::new();
    for (index, step) in scenario.steps.iter().enumerate() {
        let Some(checkpoint) = &step.checkpoint else {
            continue;
        };
        let prior_snapshot = result_checkpoint_snapshot(previous_steps, index, &checkpoint.name)?;
        let replay_snapshot = result_checkpoint_snapshot(replayed_steps, index, &checkpoint.name)?;
        let prior_value = predicate_value(prior_snapshot, &checkpoint.predicate)?;
        let replay_value = predicate_value(replay_snapshot, &checkpoint.predicate)?;
        ensure!(
            predicate_matches(prior_snapshot, &checkpoint.predicate)?,
            "previous checkpoint {:?} no longer satisfies its predicate",
            checkpoint.name
        );
        ensure!(
            predicate_matches(replay_snapshot, &checkpoint.predicate)?,
            "replayed checkpoint {:?} fails its predicate",
            checkpoint.name
        );
        let equivalent = if checkpoint.predicate.equals.is_some() {
            prior_value == replay_value
        } else if let (Some(a), Some(b)) = (prior_value.as_f64(), replay_value.as_f64()) {
            let tolerance = if checkpoint.predicate.tolerance > 0.0 {
                checkpoint.predicate.tolerance * 2.0
            } else {
                a.abs().max(b.abs()) * 0.05
            };
            (a - b).abs() <= tolerance
        } else {
            prior_value == replay_value
        };
        ensure!(
            equivalent,
            "semantic checkpoint {:?} changed on replay: {prior_value} vs {replay_value}",
            checkpoint.name
        );
        comparisons.push(json!({"name":checkpoint.name,"path":checkpoint.predicate.path,"previous":prior_value,"replayed":replay_value,"equivalent":true}));
    }
    Ok(Value::Array(comparisons))
}

fn result_checkpoint_snapshot<'a>(
    steps: &'a [Value],
    index: usize,
    name: &str,
) -> Result<&'a Value> {
    steps
        .iter()
        .find(|record| record.get("index").and_then(Value::as_u64) == Some(index as u64))
        .and_then(|record| record.get("snapshot"))
        .with_context(|| format!("checkpoint {name:?} missing from replay result"))
}

fn predicate_value<'a>(snapshot: &'a Value, predicate: &Predicate) -> Result<&'a Value> {
    let mut value = snapshot;
    for component in predicate.path.split('.') {
        value = value.get(component).with_context(|| {
            format!(
                "snapshot path {:?} missing component {component:?}",
                predicate.path
            )
        })?;
    }
    Ok(value)
}

fn await_receipt(
    client: &crate::developer_bridge::Client,
    lease: &mut ControlLease,
    command_id: &str,
    timeout: Duration,
    cancelled: Option<&AtomicBool>,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        check_cancelled(cancelled)?;
        lease.renew_if_due(client)?;
        let receipt = checked_response(client.request(DevOperation::Receipt {
            command_id: command_id.into(),
        })?)?;
        let status = receipt.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "applied" || status == "completed" {
            return Ok(receipt);
        }
        ensure!(
            status != "failed"
                && status != "rejected"
                && status != "cancelled"
                && status != "not_found",
            "command {command_id} failed: {receipt}"
        );
        ensure!(
            Instant::now() < deadline,
            "command {command_id} receipt timed out"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

struct ControlLease {
    token: String,
    last_renewed: Instant,
}

impl ControlLease {
    fn renew_if_due(&mut self, client: &crate::developer_bridge::Client) -> Result<()> {
        if self.last_renewed.elapsed() >= Duration::from_secs((LEASE_SECONDS / 3).max(1)) {
            checked_response(client.request(DevOperation::RenewControl {
                lease: self.token.clone(),
            })?)?;
            self.last_renewed = Instant::now();
        }
        Ok(())
    }
}

fn sleep_with_renewal(
    client: &crate::developer_bridge::Client,
    lease: &mut ControlLease,
    duration: Duration,
    cancelled: Option<&AtomicBool>,
) -> Result<()> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        check_cancelled(cancelled)?;
        lease.renew_if_due(client)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        thread::sleep(remaining.min(POLL_INTERVAL));
    }
    Ok(())
}

fn is_cancelled(cancelled: Option<&AtomicBool>) -> bool {
    cancelled.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
}

fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<()> {
    ensure!(!is_cancelled(cancelled), "developer scenario cancelled");
    Ok(())
}

fn acquire_lease(client: &crate::developer_bridge::Client, owner: &str) -> Result<ControlLease> {
    let response = checked_response(client.request(DevOperation::AcquireControl {
        owner: owner.into(),
    })?)?;
    let lease = response
        .get("lease")
        .and_then(Value::as_str)
        .context("lease response missing lease token")?
        .to_owned();
    Ok(ControlLease {
        token: lease,
        last_renewed: Instant::now(),
    })
}

fn inspect_inventory(client: &crate::developer_bridge::Client) -> Result<Vec<BodyInventory>> {
    let response = checked_response(client.request(DevOperation::Inspect)?)?;
    let bodies = response
        .get("bodies")
        .context("inspect response lacks bodies")?;
    Ok(serde_json::from_value(bodies.clone())?)
}

fn body_handles(inventory: &[BodyInventory]) -> Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for body in inventory {
        out.insert(body.name.to_lowercase(), body.handle.clone());
        if let Some(identity) = &body.semantic_identity {
            out.insert(identity.to_lowercase(), body.handle.clone());
        }
    }
    Ok(out)
}

fn resolve_command(
    mut command: DevCommand,
    handles: &HashMap<String, String>,
) -> Result<DevCommand> {
    let resolve = |body: &mut String| -> Result<()> {
        *body = handles
            .get(&body.to_lowercase())
            .with_context(|| format!("body identity {body:?} absent from current inventory"))?
            .clone();
        Ok(())
    };
    match &mut command {
        DevCommand::Select { body }
        | DevCommand::Focus { body, .. }
        | DevCommand::LookAt { body } => resolve(body)?,
        _ => {}
    }
    Ok(command)
}

fn checked_response(response: DevResponse) -> Result<Value> {
    ensure!(
        response.status == "ok" || response.status == "accepted",
        "development operation failed: {}",
        response.data
    );
    Ok(response.data)
}

fn predicate_matches(snapshot: &Value, predicate: &Predicate) -> Result<bool> {
    let mut value = snapshot;
    for key in predicate.path.split('.') {
        let Some(next) = value.get(key) else {
            return Ok(false);
        };
        value = next;
    }
    if let Some(expected) = &predicate.equals {
        return Ok(value == expected);
    }
    let Some(actual) = value.as_f64() else {
        return Ok(false);
    };
    Ok(predicate
        .approximately
        .is_none_or(|expected| (actual - expected).abs() <= predicate.tolerance)
        && predicate.at_least.is_none_or(|minimum| actual >= minimum)
        && predicate.at_most.is_none_or(|maximum| actual <= maximum))
}

fn create_fresh_dir(path: &Path) -> Result<()> {
    if path.exists() {
        ensure!(
            path.is_dir(),
            "output path is not a directory: {}",
            path.display()
        );
        ensure!(
            fs::read_dir(path)?.next().is_none(),
            "output directory must be empty: {}",
            path.display()
        );
    } else {
        fs::create_dir_all(path)?;
    }
    let claim_path = path.join(OUTPUT_CLAIM_FILE);
    let mut marker = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&claim_path)
        .with_context(|| {
            format!(
                "claiming fresh output directory {} (already claimed or concurrently used)",
                path.display()
            )
        })?;
    writeln!(
        marker,
        "pid={} claimed_unix_ms={}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis()
    )?;
    Ok(())
}

fn absolute_from_cwd(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn write_json(path: PathBuf, value: &impl Serialize) -> Result<()> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer
        .flush()
        .with_context(|| format!("flushing {}", path.display()))?;
    Ok(())
}

fn write_progress(path: PathBuf, value: &impl Serialize) -> Result<()> {
    ensure!(
        path.file_name().and_then(|name| name.to_str()) == Some("progress.json"),
        "only progress.json may be updated in place"
    );
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("updating mutable progress file {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer
        .flush()
        .with_context(|| format!("flushing mutable progress file {}", path.display()))?;
    Ok(())
}

fn repository_root() -> Result<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .context("resolving workspace root")?
        .to_path_buf();
    Ok(root)
}

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "mundaris_app.exe"
    } else {
        "mundaris_app"
    }
}

fn executable_path(root: &Path) -> PathBuf {
    root.join("target").join("release").join(executable_name())
}

fn normalize_preset(preset: &str) -> &str {
    if preset == "gravity-hierarchy" {
        "gravity-orbits"
    } else {
        preset
    }
}

fn wait_for_session(
    registry: &Path,
    child: &mut Child,
    preset: &str,
    executable: &Path,
    hash: &str,
    cancelled: Option<&AtomicBool>,
) -> Result<SessionDescriptor> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        check_cancelled(cancelled)?;
        if let Some(found) = crate::developer_bridge::discover(registry)?
            .into_iter()
            .find(|s| {
                s.pid == child.id() && normalize_preset(&s.preset) == normalize_preset(preset)
            })
        {
            ensure!(
                found.binary_sha256 == hash,
                "registered process binary hash mismatch"
            );
            let expected = executable
                .canonicalize()
                .unwrap_or_else(|_| executable.to_path_buf());
            let reported = PathBuf::from(&found.executable)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&found.executable));
            ensure!(
                reported == expected,
                "registered process executable does not match owned copy"
            );
            return Ok(found);
        }
        if let Some(status) = child.try_wait()? {
            bail!("owned app exited before registering (status {status})");
        }
        ensure!(
            Instant::now() < deadline,
            "owned app did not register within 60 seconds"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn remove_owned_descriptor(registry: &Path, session: &SessionDescriptor) {
    let path = registry.join(format!("{}.json", session.session_id));
    let Ok(bytes) = fs::read(&path) else {
        return;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    if value.get("session_id").and_then(Value::as_str) == Some(session.session_id.as_str())
        && value.get("pid").and_then(Value::as_u64) == Some(session.pid as u64)
    {
        let _ = fs::remove_file(path);
    }
}

fn wait_owned_child(pid: u32) -> Result<Option<std::process::ExitStatus>> {
    let Some(children) = OWNED_CHILDREN.get() else {
        return Ok(None);
    };
    let mut children = children
        .lock()
        .map_err(|_| anyhow::anyhow!("owned process registry lock poisoned"))?;
    let status = match children.get_mut(&pid) {
        Some(child) => child.try_wait()?,
        None => return Ok(None),
    };
    if status.is_some() {
        children.remove(&pid);
    }
    Ok(status)
}

fn terminate_owned_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_none() {
        child.kill().context("terminating failed owned launch")?;
    }
    child.wait().context("reaping failed owned launch")?;
    Ok(())
}

fn timestamp_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(windows)]
fn process_exists(pid: u32) -> Result<bool> {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .context("checking owned process exit with tasklist")?;
    ensure!(
        output.status.success(),
        "tasklist failed while checking owned process"
    );
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!(",\"{pid}\"")))
}

#[cfg(unix)]
fn process_exists(pid: u32) -> Result<bool> {
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .context("checking owned process exit")?;
    Ok(status.success())
}

#[cfg(not(any(windows, unix)))]
fn process_exists(_pid: u32) -> Result<bool> {
    bail!("cannot confirm process exit on this platform")
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

fn source_attribution(root: &Path) -> Result<Value> {
    let head = Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "HEAD"])
        .output()?;
    ensure!(head.status.success(), "git rev-parse HEAD failed");
    let status = Command::new("git")
        .current_dir(root)
        .args(["status", "--short"])
        .output()?;
    ensure!(status.status.success(), "git status failed");
    let files = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            ".cargo",
            "crates",
        ])
        .output()?;
    ensure!(
        files.status.success(),
        "git source fingerprint discovery failed"
    );
    let mut fingerprints = Vec::new();
    for relative in String::from_utf8(files.stdout)?.lines() {
        let path = root.join(relative);
        if path.is_file() {
            fingerprints.push(json!({"path":relative,"sha256":sha256_file(&path)?}));
        }
    }
    Ok(json!({
        "head":String::from_utf8_lossy(&head.stdout).trim(),
        "status":String::from_utf8_lossy(&status.stdout),
        "source_files":fingerprints
    }))
}

#[cfg(feature = "terrain-capture")]
fn make_demo(preset: &str) -> Result<crate::GravityOrbitsDemo> {
    match preset {
        "solar-system" => crate::GravityOrbitsDemo::solar_system(false),
        "real-solar-system" => crate::GravityOrbitsDemo::solar_system(true),
        "gravity-orbits" | "gravity-hierarchy" => crate::GravityOrbitsDemo::new(),
        _ => bail!("unsupported preset {preset:?}"),
    }
}

#[cfg(not(feature = "terrain-capture"))]
fn make_demo(_preset: &str) -> Result<crate::GravityOrbitsDemo> {
    bail!("offscreen scenario execution requires terrain-capture")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_reject_invalid_schema_steps_and_timeouts() {
        let invalid = Scenario {
            schema: 2,
            preset: "solar-system".into(),
            initial_settings: Vec::new(),
            deterministic: false,
            steps: vec![ScenarioStep {
                name: None,
                action: None,
                wait: None,
                capture: None,
                checkpoint: None,
                duration_s: 0.0,
            }],
        };
        assert!(invalid.validate().is_err());
        let reserved_checkpoint = Scenario {
            schema: 1,
            preset: "solar-system".into(),
            initial_settings: Vec::new(),
            deterministic: false,
            steps: vec![ScenarioStep {
                name: None,
                action: None,
                wait: None,
                capture: None,
                checkpoint: Some(CheckpointStep {
                    name: "progress".into(),
                    predicate: Predicate {
                        path: "general.frame_number".into(),
                        equals: Some(json!(1)),
                        approximately: None,
                        tolerance: 0.0,
                        at_least: None,
                        at_most: None,
                    },
                    timeout_s: 1.0,
                }),
                duration_s: 0.0,
            }],
        };
        assert!(reserved_checkpoint.validate().is_err());
        let invalid = Scenario {
            schema: 1,
            preset: "solar-system".into(),
            initial_settings: Vec::new(),
            deterministic: false,
            steps: vec![ScenarioStep {
                name: None,
                action: None,
                wait: Some(WaitStep {
                    predicate: Predicate {
                        path: "general.frame_number".into(),
                        equals: Some(json!(1)),
                        approximately: None,
                        tolerance: 0.0,
                        at_least: None,
                        at_most: None,
                    },
                    timeout_s: f64::INFINITY,
                }),
                capture: None,
                checkpoint: None,
                duration_s: 0.0,
            }],
        };
        assert!(invalid.validate().is_err());
        let inverted_range = Predicate {
            path: "motion.published_time_s".into(),
            equals: None,
            approximately: None,
            tolerance: 0.0,
            at_least: Some(2.0),
            at_most: Some(1.0),
        };
        assert!(inverted_range.validate(0).is_err());
    }

    #[test]
    fn predicates_support_exact_and_tolerant_values() {
        assert!(
            predicate_matches(
                &json!({"camera":{"clearance":1.01}}),
                &Predicate {
                    path: "camera.clearance".into(),
                    equals: None,
                    approximately: Some(1.0),
                    tolerance: 0.02,
                    at_least: None,
                    at_most: None
                }
            )
            .unwrap()
        );
        assert!(
            !predicate_matches(
                &json!({"camera":{"clearance":1.1}}),
                &Predicate {
                    path: "camera.clearance".into(),
                    equals: None,
                    approximately: Some(1.0),
                    tolerance: 0.02,
                    at_least: None,
                    at_most: None
                }
            )
            .unwrap()
        );
        assert!(
            !predicate_matches(
                &json!({}),
                &Predicate {
                    path: "camera.clearance".into(),
                    equals: Some(json!(1)),
                    approximately: None,
                    tolerance: 0.0,
                    at_least: None,
                    at_most: None
                }
            )
            .unwrap()
        );
    }

    #[test]
    fn cancelled_offscreen_run_stops_before_creating_artifacts_or_engine() {
        let scenario = Scenario {
            schema: 1,
            preset: "solar-system".into(),
            initial_settings: Vec::new(),
            deterministic: true,
            steps: Vec::new(),
        };
        let output = std::env::temp_dir().join(format!(
            "mundaris-cancelled-scenario-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cancelled = AtomicBool::new(true);
        let result = run_offscreen_cancellable(&scenario, &output, Some(&cancelled));
        assert!(result.is_err());
        assert!(!output.exists());
    }

    #[test]
    fn output_claim_is_atomic_and_static_json_cannot_be_replaced() {
        let root = std::env::temp_dir().join(format!(
            "mundaris-output-claim-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let output = root.join("caller-provided-empty-output");
        fs::create_dir(&output).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let claimers = (0..2)
            .map(|_| {
                let barrier = barrier.clone();
                let output = output.clone();
                thread::spawn(move || {
                    barrier.wait();
                    create_fresh_dir(&output)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = claimers
            .into_iter()
            .map(|claimer| claimer.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        assert!(output.join(OUTPUT_CLAIM_FILE).is_file());

        let static_path = output.join("scenario.json");
        write_json(static_path.clone(), &json!({"revision": 1})).unwrap();
        let original = fs::read(&static_path).unwrap();
        assert!(write_json(static_path.clone(), &json!({"revision": 2})).is_err());
        assert_eq!(fs::read(static_path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }
}
