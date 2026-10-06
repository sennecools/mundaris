//! Local command-line and MCP clients for the app-owned development protocol.
use crate::developer_protocol::{
    DevCommand, DevOperation, DevRequest, DevResponse, MAX_MESSAGE_BYTES, MAX_RESPONSE_BYTES,
    PROTOCOL_VERSION, SessionDescriptor,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

/// One request/response client. Connections are short-lived so every response is
/// attributable to one request and a stalled server cannot pin the caller forever.
pub struct Client {
    session: SessionDescriptor,
}

impl Client {
    pub fn new(session: SessionDescriptor) -> Self {
        Self { session }
    }

    pub fn request(&self, operation: DevOperation) -> Result<DevResponse> {
        let request_id = format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
        );
        let request = DevRequest {
            protocol_version: PROTOCOL_VERSION,
            session_id: self.session.session_id.clone(),
            request_id: request_id.clone(),
            operation,
        };
        let bytes = serde_json::to_vec(&request).context("encoding development request")?;
        ensure!(
            bytes.len() <= MAX_MESSAGE_BYTES,
            "request exceeds protocol message cap"
        );
        let mut stream = connect_endpoint(&self.session.endpoint)?;
        stream
            .write_all(&bytes)
            .context("writing development request")?;
        stream
            .write_all(b"\n")
            .context("terminating development request")?;
        stream.flush().context("flushing development request")?;
        let reader = BufReader::new(stream);
        let mut line = Vec::new();
        reader
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)
            .context("reading development response")?;
        ensure!(
            !line.is_empty(),
            "development endpoint closed without a response"
        );
        ensure!(
            line.len() <= MAX_RESPONSE_BYTES,
            "response exceeds protocol message cap"
        );
        let text = std::str::from_utf8(&line).context("response is not UTF-8")?;
        let response: DevResponse = serde_json::from_str(text.trim_end_matches(['\r', '\n']))
            .context("decoding development response")?;
        ensure!(
            response.protocol_version == PROTOCOL_VERSION,
            "protocol version mismatch"
        );
        ensure!(
            response.session_id == self.session.session_id,
            "response belongs to a different session"
        );
        ensure!(
            response.request_id == request_id
                || (response.request_id.is_empty()
                    && response.status == "failed"
                    && response.data["error"] == "connection_overflow"),
            "response request id mismatch"
        );
        Ok(response)
    }
}

fn connect_endpoint(endpoint: &str) -> Result<TcpStream> {
    let address = endpoint;
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .context("resolving development endpoint")?
        .collect();
    ensure!(
        !addresses.is_empty(),
        "development endpoint resolved to no addresses"
    );
    ensure!(
        addresses.iter().all(|socket| socket.ip().is_loopback()),
        "development endpoint must be loopback"
    );
    let mut last_error = None;
    for socket in addresses {
        match TcpStream::connect_timeout(&socket, Duration::from_secs(2)) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(35)))?;
                stream.set_write_timeout(Some(Duration::from_secs(35)))?;
                return Ok(stream);
            }
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow!("development endpoint resolved to no addresses")))
}

/// Read descriptors from a local registry and retain only endpoints that answer
/// a session-bound capabilities request.
pub fn discover(registry: &Path) -> Result<Vec<SessionDescriptor>> {
    let entries = match fs::read_dir(registry) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading session registry {}", registry.display()));
        }
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if bytes.len() > MAX_RESPONSE_BYTES || std::str::from_utf8(&bytes).is_err() {
            continue;
        }
        let Ok(session) = serde_json::from_slice::<SessionDescriptor>(&bytes) else {
            continue;
        };
        if !descriptor_valid(&session) {
            continue;
        }
        let client = Client::new(session.clone());
        if let Ok(response) = client.request(DevOperation::Capabilities)
            && response.status == "ok"
            && response
                .data
                .get("session_id")
                .and_then(Value::as_str)
                .is_none_or(|actual| actual == session.session_id)
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    Ok(sessions)
}

fn descriptor_valid(s: &SessionDescriptor) -> bool {
    s.protocol_version == PROTOCOL_VERSION
        && s.pid != 0
        && !s.session_id.is_empty()
        && !s.endpoint.is_empty()
        && !s.preset.is_empty()
        && !s.executable.is_empty()
        && !s.output_directory.is_empty()
        && !s.binary_sha256.is_empty()
        && [
            s.session_id.as_str(),
            s.endpoint.as_str(),
            s.preset.as_str(),
            s.executable.as_str(),
            s.output_directory.as_str(),
            s.binary_sha256.as_str(),
        ]
        .iter()
        .all(|v| v.len() <= MAX_MESSAGE_BYTES)
}

fn select_session(registry: &Path, explicit: Option<&str>) -> Result<SessionDescriptor> {
    let sessions = discover(registry)?;
    if let Some(id) = explicit {
        let matches: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.session_id == id)
            .collect();
        return match matches.as_slice() {
            [only] => Ok(only.clone()),
            [] => bail!("no live session {id:?} in {}", registry.display()),
            many => bail!(
                "session id {id:?} matches {} live descriptors; remove the duplicate registry entries",
                many.len()
            ),
        };
    }
    match sessions.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!(
            "no live Mundaris development sessions found in {}",
            registry.display()
        ),
        many => bail!(
            "multiple live sessions found; select one with --session ({} sessions)",
            many.len()
        ),
    }
}

fn registry_default() -> PathBuf {
    std::env::var_os("MUNDARIS_DEV_REGISTRY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/developer-sessions"))
}

fn fresh_output(parent: &str) -> PathBuf {
    PathBuf::from(parent).join(format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT_REQUEST.fetch_add(1, Ordering::Relaxed)
    ))
}

fn emit(value: &Value) -> Result<()> {
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, value)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn read_json_arg(value: Option<&str>) -> Result<Value> {
    let mut bytes = Vec::new();
    if value == Some("-") || value.is_none() {
        std::io::stdin()
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .context("reading JSON from stdin")?;
    } else if let Some(value) = value {
        if let Some(path) = value.strip_prefix('@') {
            bytes = fs::read(path).with_context(|| format!("reading JSON input {}", path))?;
        } else {
            bytes.extend_from_slice(value.as_bytes());
        }
    }
    ensure!(
        bytes.len() <= MAX_MESSAGE_BYTES,
        "input exceeds protocol message cap"
    );
    let text = std::str::from_utf8(&bytes).context("input is not UTF-8")?;
    serde_json::from_str(text).context("parsing JSON input")
}

fn parse_cli(args: &[String]) -> Result<Value> {
    if args.is_empty() || args[0] == "--help" || args[0] == "help" {
        return Ok(json!({"usage": CLI_HELP}));
    }
    let command = args[0].as_str();
    let mut registry = registry_default();
    let mut session_id = None;
    let mut offscreen = false;
    let mut wait_timeout_s = 30.0;
    let mut wait_predicate = None;
    let mut positional = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--registry" => {
                i += 1;
                registry = PathBuf::from(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--registry needs a path"))?,
                );
            }
            "--session" => {
                i += 1;
                session_id = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow!("--session needs an id"))?
                        .clone(),
                );
            }
            "--offscreen" => offscreen = true,
            "--timeout" => {
                i += 1;
                wait_timeout_s = args
                    .get(i)
                    .ok_or_else(|| anyhow!("--timeout needs seconds"))?
                    .parse()?;
            }
            "--predicate" => {
                i += 1;
                wait_predicate = Some(read_json_arg(args.get(i).map(String::as_str))?);
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }
    if command == "mcp" {
        return Ok(json!({"mcp": true}));
    }
    if command == "sessions" {
        let live = discover(&registry)?;
        return Ok(json!({"status":"ok","sessions":live}));
    }
    if command == "launch" {
        let preset = positional
            .first()
            .ok_or_else(|| anyhow!("launch needs a preset"))?;
        let output = PathBuf::from(
            positional
                .get(1)
                .ok_or_else(|| anyhow!("launch needs an output directory"))?,
        );
        let session = crate::developer_scenarios::launch_owned(preset, &registry, &output)?;
        return Ok(json!({"status":"ok","session":session}));
    }
    if command == "scenario" || command == "rebuild-replay" {
        let scenario_value = read_json_arg(positional.first().map(String::as_str))?;
        let scenario: crate::developer_scenarios::Scenario =
            serde_json::from_value(scenario_value)?;
        let output = positional
            .get(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| fresh_output("target/developer-scenarios"));
        let result = if command == "scenario" {
            if offscreen {
                crate::developer_scenarios::run_offscreen(&scenario, &output)?
            } else {
                let session = select_session(&registry, session_id.as_deref())?;
                crate::developer_scenarios::run_native(&session, &scenario, &output)?
            }
        } else {
            let session = select_session(&registry, session_id.as_deref())?;
            crate::developer_scenarios::rebuild_and_replay(&session, &scenario, &registry, &output)?
        };
        return Ok(result);
    }
    if command == "stop" {
        let session = select_session(&registry, session_id.as_deref())?;
        return crate::developer_scenarios::stop_owned(&session, &registry);
    }
    let session = select_session(&registry, session_id.as_deref())?;
    let client = Client::new(session);
    let response = match command {
        "request" => client.request(serde_json::from_value(read_json_arg(
            positional.first().map(String::as_str),
        )?)?)?,
        "capabilities" => client.request(DevOperation::Capabilities)?,
        "inspect" => client.request(DevOperation::Inspect)?,
        "diagnostics" => client.request(DevOperation::Diagnostics {
            scope: positional
                .first()
                .cloned()
                .unwrap_or_else(|| "errors".into()),
        })?,
        "control" => match positional.first().map(String::as_str) {
            Some("acquire") => client.request(DevOperation::AcquireControl {
                owner: positional
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| "mundaris-dev".into()),
            })?,
            Some("renew") => client.request(DevOperation::RenewControl {
                lease: required(&positional, 1, "lease")?,
            })?,
            Some("release") => client.request(DevOperation::ReleaseControl {
                lease: required(&positional, 1, "lease")?,
            })?,
            _ => bail!("control requires acquire [owner], renew <lease>, or release <lease>"),
        },
        "action" => {
            let lease = required(&positional, 0, "lease")?;
            let value = read_json_arg(positional.get(1).map(String::as_str))?;
            let command: DevCommand = serde_json::from_value(value)?;
            client.request(DevOperation::Action { lease, command })?
        }
        "receipt" => client.request(DevOperation::Receipt {
            command_id: required(&positional, 0, "command id")?,
        })?,
        "events" => client.request(DevOperation::Events {
            after: positional
                .first()
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(0),
        })?,
        "capture" => {
            let accepted = client.request(DevOperation::Capture {
                lease: required(&positional, 0, "lease")?,
                name: required(&positional, 1, "capture name")?,
            })?;
            if accepted.status != "accepted" && accepted.status != "ok" {
                return Ok(serde_json::to_value(accepted)?);
            }
            let Some(command_id) = accepted.data.get("command_id").and_then(Value::as_str) else {
                return Ok(serde_json::to_value(accepted)?);
            };
            let waited = wait_for_receipt(&client, Some(command_id.to_string()), 30.0, None, None)?;
            return Ok(json!({"accepted":accepted,"result":waited}));
        }
        "cancel" => client.request(DevOperation::Cancel {
            lease: required(&positional, 0, "lease")?,
        })?,
        "wait" => {
            let command_id = positional.first().cloned();
            ensure!(
                command_id.is_some() || wait_predicate.is_some(),
                "wait needs a command id or --predicate"
            );
            return wait_for_receipt(&client, command_id, wait_timeout_s, wait_predicate, None);
        }
        other => bail!("unknown command {other:?}; run mundaris_dev --help"),
    };
    Ok(serde_json::to_value(response)?)
}

fn required(values: &[String], index: usize, label: &str) -> Result<String> {
    values
        .get(index)
        .cloned()
        .ok_or_else(|| anyhow!("missing {label}"))
}

const CLI_HELP: &str = "mundaris_dev [--registry DIR] [--session ID] COMMAND\n\
sessions | inspect | control acquire [OWNER] | control renew LEASE | control release LEASE\n\
capabilities | diagnostics [terrain|performance|errors]\n\
request [JSON|-|@FILE]\n\
action LEASE [JSON|-] | receipt COMMAND_ID | events [AFTER_SEQUENCE] | capture LEASE NAME\n\
wait [COMMAND_ID] [--predicate JSON] [--timeout SECONDS] | cancel LEASE | launch PRESET OUTPUT_DIR | stop | scenario [--offscreen] [JSON|-|@FILE] [OUTPUT_DIR]\n\
rebuild-replay [JSON|-|@FILE] [OUTPUT_DIR] | mcp\n\
JSON action payloads use the tagged DevCommand schema; '-' reads stdin and @FILE reads a file.";

fn path_value<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = value;
    for component in dotted.split('.') {
        current = current.get(component)?;
    }
    Some(current)
}

fn predicate_matches(snapshot: &Value, predicate: &Value) -> bool {
    let Some(path) = predicate.get("path").and_then(Value::as_str) else {
        return false;
    };
    let Some(actual) = path_value(snapshot, path) else {
        return false;
    };
    predicate
        .get("equals")
        .is_none_or(|expected| actual == expected)
        && predicate
            .get("not_equals")
            .is_none_or(|expected| actual != expected)
        && predicate
            .get("at_least")
            .and_then(Value::as_f64)
            .is_none_or(|expected| actual.as_f64().is_some_and(|value| value >= expected))
        && predicate
            .get("at_most")
            .and_then(Value::as_f64)
            .is_none_or(|expected| actual.as_f64().is_some_and(|value| value <= expected))
}

fn validate_predicate(predicate: &Value) -> Result<()> {
    let schema = mcp_tools()
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == "mundaris_wait"))
        .and_then(|tool| {
            tool["inputSchema"]["properties"]["predicate"]
                .as_object()
                .cloned()
        })
        .ok_or_else(|| anyhow!("wait predicate schema is unavailable"))?;
    validate_json_schema(predicate, &Value::Object(schema), "predicate")?;
    let object = predicate
        .as_object()
        .ok_or_else(|| anyhow!("wait predicate must be an object"))?;
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("wait predicate path must be a string"))?;
    ensure!(
        !path.is_empty() && path.split('.').all(|component| !component.is_empty()),
        "wait predicate path must be a nonempty dotted field path"
    );
    let has_equals = object.contains_key("equals");
    let has_not_equals = object.contains_key("not_equals");
    let at_least = object.get("at_least").and_then(Value::as_f64);
    let at_most = object.get("at_most").and_then(Value::as_f64);
    ensure!(
        has_equals || has_not_equals || at_least.is_some() || at_most.is_some(),
        "wait predicate needs at least one comparison"
    );
    ensure!(
        !(has_equals && has_not_equals),
        "wait predicate cannot combine equals and not_equals"
    );
    ensure!(
        !(has_equals || has_not_equals) || (at_least.is_none() && at_most.is_none()),
        "wait predicate cannot combine equality and numeric bounds"
    );
    if let (Some(lower), Some(upper)) = (at_least, at_most) {
        ensure!(lower <= upper, "wait predicate has inverted numeric bounds");
    }
    Ok(())
}

fn extract_snapshot(response: &DevResponse) -> Option<Value> {
    response.data.get("snapshot").cloned().or_else(|| {
        response
            .data
            .get("data")
            .and_then(|v| v.get("snapshot"))
            .cloned()
    })
}

fn find_string(value: &Value, key: &str) -> Option<String> {
    if let Some(found) = value.get(key).and_then(Value::as_str) {
        return Some(found.to_owned());
    }
    match value {
        Value::Object(map) => map.values().find_map(|v| find_string(v, key)),
        Value::Array(values) => values.iter().find_map(|v| find_string(v, key)),
        _ => None,
    }
}

fn capture_images(value: &Value) -> Result<Vec<Value>> {
    let mut images = Vec::new();
    for key in ["full_image", "viewport_image"] {
        if let Some(path) = find_string(value, key) {
            let bytes =
                fs::read(&path).with_context(|| format!("reading capture image {}", path))?;
            ensure!(
                bytes.len() <= 24 * 1024 * 1024,
                "capture image exceeds MCP image cap"
            );
            use base64::Engine;
            images.push(json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(bytes),"mimeType":"image/png"}));
        }
    }
    Ok(images)
}

fn wait_for_receipt(
    client: &Client,
    command_id: Option<String>,
    timeout_s: f64,
    predicate: Option<Value>,
    cancel: Option<&AtomicBool>,
) -> Result<Value> {
    ensure!(
        timeout_s.is_finite() && (0.0..=30.0).contains(&timeout_s),
        "wait timeout must be between 0 and 30 seconds"
    );
    ensure!(
        command_id.is_some() || predicate.is_some(),
        "wait needs a command id or a snapshot predicate"
    );
    if let Some(predicate) = &predicate {
        validate_predicate(predicate)?;
    }
    let start = Instant::now();
    let timeout = Duration::from_secs_f64(timeout_s);
    let mut latest_receipt = Value::Null;
    let mut latest_snapshot = Value::Null;
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            bail!("wait cancelled");
        }
        if let Some(command_id) = &command_id {
            let response = client.request(DevOperation::Receipt {
                command_id: command_id.clone(),
            })?;
            let receipt_status = response
                .data
                .get("receipt")
                .and_then(|r| r.get("status"))
                .and_then(Value::as_str)
                .or_else(|| response.data.get("status").and_then(Value::as_str))
                .unwrap_or(response.status.as_str());
            if response.status != "ok" {
                return Ok(serde_json::to_value(response)?);
            }
            let terminal = matches!(
                receipt_status,
                "completed" | "applied" | "failed" | "cancelled" | "expired" | "not_found"
            );
            latest_receipt = serde_json::to_value(&response)?;
            if matches!(
                receipt_status,
                "failed" | "cancelled" | "expired" | "not_found"
            ) || (terminal && predicate.is_none())
            {
                return Ok(latest_receipt);
            }
        }
        if let Some(predicate) = &predicate {
            let inspected = client.request(DevOperation::Inspect)?;
            if let Some(snapshot) = extract_snapshot(&inspected) {
                latest_snapshot = snapshot.clone();
                if snapshot
                    .pointer("/development/stale")
                    .and_then(Value::as_bool)
                    == Some(false)
                    && predicate_matches(&snapshot, predicate)
                {
                    return Ok(
                        json!({"status":"predicate_met","receipt":latest_receipt,"snapshot":snapshot}),
                    );
                }
            }
        }
        if start.elapsed() >= timeout {
            return Ok(
                json!({"status":"timeout","receipt":latest_receipt,"snapshot":latest_snapshot}),
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn validate_json_schema(value: &Value, schema: &Value, path: &str) -> Result<()> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.as_f64().is_some(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            _ => false,
        };
        ensure!(matches, "{path} must be {expected}");
    }
    if let Some(expected) = schema.get("const") {
        ensure!(value == expected, "{path} must equal {expected}");
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        ensure!(choices.contains(value), "{path} is not an allowed value");
    }
    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                ensure!(object.contains_key(key), "{path}.{key} is required");
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in object {
                if let Some(property_schema) = properties.get(key) {
                    validate_json_schema(value, property_schema, &format!("{path}.{key}"))?;
                } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                    bail!("{path}.{key} is not allowed");
                }
            }
        } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            ensure!(object.is_empty(), "{path} does not allow properties");
        }
    }
    if let Some(array) = value.as_array() {
        if let Some(minimum) = schema.get("minItems").and_then(Value::as_u64) {
            ensure!(array.len() as u64 >= minimum, "{path} has too few items");
        }
        if let Some(maximum) = schema.get("maxItems").and_then(Value::as_u64) {
            ensure!(array.len() as u64 <= maximum, "{path} has too many items");
        }
        if let Some(item_schema) = schema.get("items") {
            for (index, item) in array.iter().enumerate() {
                validate_json_schema(item, item_schema, &format!("{path}[{index}]"))?;
            }
        }
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            ensure!(number >= minimum, "{path} must be at least {minimum}");
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            ensure!(number <= maximum, "{path} must be at most {maximum}");
        }
        if let Some(minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
            ensure!(number > minimum, "{path} must be greater than {minimum}");
        }
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        let matched = branches
            .iter()
            .filter(|branch| validate_json_schema(value, branch, path).is_ok())
            .count();
        ensure!(matched == 1, "{path} must match exactly one schema variant");
    }
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
        ensure!(
            branches
                .iter()
                .any(|branch| validate_json_schema(value, branch, path).is_ok()),
            "{path} does not match an allowed schema variant"
        );
    }
    Ok(())
}

fn validate_mcp_arguments(name: &str, args: &Value) -> Result<()> {
    let tools = mcp_tools();
    let tool = tools
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["name"] == name))
        .ok_or_else(|| anyhow!("unknown MCP tool {name:?}"))?;
    validate_json_schema(args, &tool["inputSchema"], "arguments")?;
    Ok(())
}

fn mcp_tools() -> Value {
    let body_handle_description = "Opaque session-scoped handle copied from bodies[].handle in mundaris_inspect; do not pass semantic_identity or name.";
    let action_schema = json!({
        "oneOf": [
            {"type":"object","properties":{"action":{"const":"select"},"body":{"type":"string","description":body_handle_description}},"required":["action","body"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"focus"},"body":{"type":"string","description":body_handle_description},"body_fixed":{"type":"boolean"}},"required":["action","body"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"overview"}},"required":["action"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"look_at"},"body":{"type":"string","description":body_handle_description}},"required":["action","body"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"navigation_mode"},"mode":{"enum":["system_orbit","overview","body_orbit","surface_horizon","surface_inspection","free_flight"]}},"required":["action","mode"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"navigation"},"drag":{"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2},"scroll_notches":{"type":"number"},"translation":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"speed_multiplier":{"type":"number","exclusiveMinimum":0},"boost_multiplier":{"type":"number","exclusiveMinimum":0},"duration_s":{"type":"number","minimum":0}},"required":["action"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"clearance"},"meters":{"type":"number","exclusiveMinimum":0}},"required":["action","meters"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"pause"},"paused":{"type":"boolean"}},"required":["action","paused"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"rate"},"multiplier":{"type":"number"}},"required":["action","multiplier"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"seek"},"seconds":{"type":"number"}},"required":["action","seconds"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"single_step"},"forward":{"type":"boolean"}},"required":["action"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"reset"}},"required":["action"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"render_mode"},"mode":{"type":"string"}},"required":["action","mode"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"layer"},"layer":{"enum":["terrain","ocean","clouds","atmosphere","sky","markers","labels","trails","guides","patch_borders","lod_colors"]},"enabled":{"type":"boolean"}},"required":["action","layer","enabled"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"sky_setting"},"setting":{"enum":["intensity","star_intensity","background_intensity","halo_strength","galactic_yaw_rad","galactic_roll_rad"]},"value":{"type":"number"}},"required":["action","setting","value"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"gpu_tile"},"enabled":{"type":"boolean"},"family":{"enum":["rocky_v5","icy_v3","volcanic_v3"]},"seed":{"type":"integer","minimum":0},"radius_m":{"type":"number","exclusiveMinimum":0},"face":{"enum":["positive_x","negative_x","positive_y","negative_y","positive_z","negative_z"]},"level":{"type":"integer","minimum":0,"maximum":30},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"cells":{"type":"integer","minimum":1,"maximum":128},"revision":{"type":"integer","minimum":0}},"required":["action","enabled"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"gpu_tile_view"},"mode":{"type":"integer","minimum":0,"maximum":10},"camera_offset_m":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"sun_direction_body":{"type":"array","items":{"type":"number"},"minItems":3,"maxItems":3},"reference_cpu":{"type":"boolean"}},"required":["action"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"gpu_hierarchy"},"enabled":{"type":"boolean"},"refine":{"type":"boolean"},"morph_duration_ms":{"type":"integer","minimum":0,"maximum":10000},"child_delays_ms":{"type":"array","items":{"type":"integer","minimum":0,"maximum":5000},"minItems":4,"maxItems":4},"request_mask":{"type":"integer","minimum":0,"maximum":15},"cancel_pending":{"type":"boolean"},"diagnostic_validate":{"type":"boolean"}},"required":["action","enabled"],"additionalProperties":false},
            {"type":"object","properties":{"action":{"const":"gpu_regional"},"enabled":{"type":"boolean"},"max_depth":{"type":"integer","minimum":1,"maximum":4},"gpu_slots":{"type":"integer","minimum":5,"maximum":512},"cpu_tiles":{"type":"integer","minimum":5,"maximum":1024},"worker_count":{"type":"integer","minimum":1,"maximum":8},"worker_delay_ms":{"type":"integer","minimum":0,"maximum":5000},"upload_tiles_per_frame":{"type":"integer","minimum":1,"maximum":16},"upload_bytes_per_frame":{"type":"integer","minimum":1,"maximum":67108864},"publication_groups_per_frame":{"type":"integer","minimum":1,"maximum":8},"transition_limit":{"type":"integer","minimum":1,"maximum":16},"morph_duration_ms":{"type":"integer","minimum":0,"maximum":10000},"split_error_px":{"type":"number","exclusiveMinimum":0},"merge_error_px":{"type":"number","exclusiveMinimum":0}},"required":["action","enabled"],"additionalProperties":false}
        ]
    });
    let mut tools = json!([
      {"name":"mundaris_sessions","description":"List live Mundaris development sessions.","inputSchema":{"type":"object","properties":{"registry":{"type":"string"}},"additionalProperties":false}},
      {"name":"mundaris_capabilities","description":"Read protocol and action capabilities for a live session.","inputSchema":{"type":"object","properties":{"session":{"type":"string"}},"additionalProperties":false}},
      {"name":"mundaris_inspect","description":"Read the current coherent developer snapshot.","inputSchema":{"type":"object","properties":{"session":{"type":"string"}},"additionalProperties":false}},
      {"name":"mundaris_diagnostics","description":"Read bounded terrain, performance, or error diagnostics.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"scope":{"enum":["terrain","performance","errors"]}},"additionalProperties":false}},
      {"name":"mundaris_control","description":"Acquire, renew, release, or cancel the session control lease. A lease lasts 30 seconds; renew it before expiry to keep control.","inputSchema":{"type":"object","oneOf":[{"properties":{"session":{"type":"string"},"operation":{"const":"acquire"},"owner":{"type":"string"}},"required":["operation"],"additionalProperties":false},{"properties":{"session":{"type":"string"},"operation":{"enum":["renew","release","cancel"]},"lease":{"type":"string"}},"required":["operation","lease"],"additionalProperties":false}]}},
      {"name":"mundaris_action","description":"Submit one typed engine action under a control lease.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"lease":{"type":"string"},"command":action_schema},"required":["lease","command"],"additionalProperties":false}},
      {"name":"mundaris_receipt","description":"Read one command receipt by its command id.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"command_id":{"type":"string"}},"required":["command_id"],"additionalProperties":false}},
      {"name":"mundaris_capture","description":"Request a paired native capture and return its image and evidence paths.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"lease":{"type":"string"},"name":{"type":"string"}},"required":["lease","name"],"additionalProperties":false}},
      {"name":"mundaris_wait","description":"Wait up to 30 seconds for a command receipt and/or snapshot predicate.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"command_id":{"type":"string"},"timeout_s":{"type":"number","minimum":0,"maximum":30,"description":"Maximum wait duration in seconds (0 to 30)."},"predicate":{"type":"object","description":"Snapshot condition. Example: {\"path\":\"general.frame_number\",\"at_least\":3}.","properties":{"path":{"type":"string","description":"Nonempty dotted snapshot path, such as general.frame_number."},"equals":{},"not_equals":{},"at_least":{"type":"number"},"at_most":{"type":"number"}},"required":["path"],"additionalProperties":false}},"anyOf":[{"required":["command_id"]},{"required":["predicate"]}],"additionalProperties":false}},
      {"name":"mundaris_events","description":"Read bounded recent command and diagnostic events.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"after":{"type":"integer","minimum":0}},"additionalProperties":false}},
      {"name":"mundaris_scenarios","description":"Run a serialized scenario natively or offscreen, or rebuild and replay it.","inputSchema":{"type":"object","properties":{"session":{"type":"string"},"scenario":{"type":"object"},"output_directory":{"type":"string"},"rebuild_replay":{"type":"boolean"},"offscreen":{"type":"boolean"}},"required":["scenario"],"additionalProperties":false}},
      {"name":"mundaris_ownedlifecycle","description":"Launch an owned native session or stop a selected owned session.","inputSchema":{"type":"object","properties":{"operation":{"enum":["launch","stop"]},"session":{"type":"string"},"preset":{"enum":["solar-system","real-solar-system","gravity-orbits"]},"output_directory":{"type":"string"},"registry":{"type":"string"}},"required":["operation"],"additionalProperties":false}}
    ]);
    for tool in tools.as_array_mut().expect("tool list") {
        let schema = &mut tool["inputSchema"];
        if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("registry".into(), json!({"type":"string"}));
        }
        if let Some(branches) = schema.get_mut("oneOf").and_then(Value::as_array_mut) {
            for branch in branches {
                branch["properties"]["registry"] = json!({"type":"string"});
            }
        }
    }
    tools
}

fn mcp_tool_call(
    name: &str,
    args: &Value,
    cancelled: Option<&AtomicBool>,
) -> Result<(Value, Vec<Value>)> {
    ensure!(
        !cancelled.is_some_and(|token| token.load(Ordering::Relaxed)),
        "request cancelled"
    );
    validate_mcp_arguments(name, args)?;
    let registry = args
        .get("registry")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(registry_default);
    let explicit = args.get("session").and_then(Value::as_str);
    if name == "mundaris_sessions" {
        let sessions = discover(&registry)?;
        return Ok((json!({"sessions":sessions}), Vec::new()));
    }
    if name == "mundaris_ownedlifecycle" {
        let operation = args
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing operation"))?;
        let output = args
            .get("output_directory")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| fresh_output("target/developer-owned"));
        let result = if operation == "launch" {
            crate::developer_scenarios::launch_owned_cancellable(
                args.get("preset")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing preset"))?,
                &registry,
                &output,
                cancelled,
            )
            .map(|v| json!({"session":v}))?
        } else if operation == "stop" {
            let session = select_session(&registry, explicit)?;
            crate::developer_scenarios::stop_owned_cancellable(&session, &registry, cancelled)?
        } else {
            bail!("operation must be launch or stop")
        };
        return Ok((result, Vec::new()));
    }
    if name == "mundaris_scenarios" {
        let scenario: crate::developer_scenarios::Scenario = serde_json::from_value(
            args.get("scenario")
                .cloned()
                .ok_or_else(|| anyhow!("missing scenario"))?,
        )?;
        let output = args
            .get("output_directory")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or_else(|| fresh_output("target/developer-scenarios"));
        let result = if args
            .get("offscreen")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            ensure!(
                !args
                    .get("rebuild_replay")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "offscreen and rebuild_replay cannot be combined"
            );
            crate::developer_scenarios::run_offscreen_cancellable(&scenario, &output, cancelled)?
        } else if args
            .get("rebuild_replay")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let session = select_session(&registry, explicit)?;
            crate::developer_scenarios::rebuild_and_replay_cancellable(
                &session, &scenario, &registry, &output, cancelled,
            )?
        } else {
            let session = select_session(&registry, explicit)?;
            crate::developer_scenarios::run_native_cancellable(
                &session, &scenario, &output, cancelled,
            )?
        };
        return Ok((result, Vec::new()));
    }
    let session = select_session(&registry, explicit)?;
    let client = Client::new(session);
    if name == "mundaris_capabilities" {
        return Ok((
            serde_json::to_value(client.request(DevOperation::Capabilities)?)?,
            Vec::new(),
        ));
    }
    if name == "mundaris_diagnostics" {
        let scope = args
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or("errors")
            .to_owned();
        return Ok((
            serde_json::to_value(client.request(DevOperation::Diagnostics { scope })?)?,
            Vec::new(),
        ));
    }
    if name == "mundaris_capture" {
        let lease = args
            .get("lease")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing lease"))?
            .to_owned();
        let accepted = client.request(DevOperation::Capture {
            lease: lease.clone(),
            name: args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing name"))?
                .into(),
        })?;
        if accepted.status != "accepted" && accepted.status != "ok" {
            let value = serde_json::to_value(accepted)?;
            return Ok((value, Vec::new()));
        }
        let Some(command_id) = accepted.data.get("command_id").and_then(Value::as_str) else {
            return Ok((serde_json::to_value(accepted)?, Vec::new()));
        };
        let result = match wait_for_receipt(
            &client,
            Some(command_id.to_owned()),
            30.0,
            None,
            cancelled,
        ) {
            Ok(result) if result["status"] == "timeout" => {
                let cancellation = client.request(DevOperation::Cancel { lease });
                return Ok((
                    json!({"accepted":accepted,"result":result,"cancellation":cancellation.map(|r|serde_json::to_value(r).unwrap_or(Value::Null)).map_err(|e|e.to_string())}),
                    Vec::new(),
                ));
            }
            Ok(result) => result,
            Err(error) => {
                let cancellation = client.request(DevOperation::Cancel { lease });
                bail!("{error}; capture cancellation: {cancellation:?}");
            }
        };
        let structured = json!({"accepted":accepted,"result":result});
        let images = capture_images(&structured)?;
        return Ok((structured, images));
    }
    let response = match name {
        "mundaris_inspect" => client.request(DevOperation::Inspect)?,
        "mundaris_receipt" => client.request(DevOperation::Receipt {
            command_id: args
                .get("command_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing command_id"))?
                .into(),
        })?,
        "mundaris_diagnostics" => client.request(DevOperation::Diagnostics {
            scope: args
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("errors")
                .into(),
        })?,
        "mundaris_control" => match args
            .get("operation")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("missing operation"))?
        {
            "acquire" => client.request(DevOperation::AcquireControl {
                owner: args
                    .get("owner")
                    .and_then(Value::as_str)
                    .unwrap_or("mcp")
                    .into(),
            })?,
            "renew" => client.request(DevOperation::RenewControl {
                lease: args
                    .get("lease")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing lease"))?
                    .into(),
            })?,
            "release" => client.request(DevOperation::ReleaseControl {
                lease: args
                    .get("lease")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing lease"))?
                    .into(),
            })?,
            "cancel" => client.request(DevOperation::Cancel {
                lease: args
                    .get("lease")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("missing lease"))?
                    .into(),
            })?,
            _ => bail!("operation must be acquire, renew, release, or cancel"),
        },
        "mundaris_action" => client.request(DevOperation::Action {
            lease: args
                .get("lease")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("missing lease"))?
                .into(),
            command: serde_json::from_value(
                args.get("command")
                    .cloned()
                    .ok_or_else(|| anyhow!("missing command"))?,
            )?,
        })?,
        "mundaris_capture" => unreachable!("capture handled above"),
        "mundaris_wait" => {
            let id = args
                .get("command_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let timeout = args
                .get("timeout_s")
                .and_then(Value::as_f64)
                .unwrap_or(30.0);
            let predicate = args.get("predicate").cloned();
            let value = wait_for_receipt(&client, id, timeout, predicate, cancelled)?;
            return Ok((value, Vec::new()));
        }
        "mundaris_events" => client.request(DevOperation::Events {
            after: args.get("after").and_then(Value::as_u64).unwrap_or(0),
        })?,
        other => bail!("unknown MCP tool {other:?}"),
    };
    Ok((serde_json::to_value(response)?, Vec::new()))
}

fn mcp_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn mcp_error(id: Value, code: i64, message: impl ToString) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message.to_string()}})
}

fn rpc_id_key(value: &Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn read_capped_stdio_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if bytes.is_empty() {
                Ok(None)
            } else {
                Ok(Some(decode_stdio_line(bytes)))
            };
        }
        let count = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        let done = available[count - 1] == b'\n';
        if bytes.len().saturating_add(count) > MAX_MESSAGE_BYTES {
            reader.consume(count);
            return Ok(Some(" ".repeat(MAX_MESSAGE_BYTES + 1)));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if done {
            return Ok(Some(decode_stdio_line(bytes)));
        }
    }
}

fn decode_stdio_line(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|_| "\0invalid-utf8".into())
}

fn contains_failure(value: &Value) -> bool {
    if value
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| {
            matches!(
                status,
                "failed" | "expired" | "cancelled" | "timeout" | "not_found" | "error"
            )
        })
    {
        return true;
    }
    match value {
        Value::Object(map) => map.values().any(contains_failure),
        Value::Array(values) => values.iter().any(contains_failure),
        _ => false,
    }
}

fn handle_mcp_message(
    line: &str,
    cancels: &Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
) -> Option<Value> {
    if line.len() > MAX_MESSAGE_BYTES {
        return Some(mcp_error(Value::Null, -32600, "message exceeds size cap"));
    }
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(mcp_error(Value::Null, -32700, e)),
    };
    let id = value.get("id").cloned();
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(mcp_error(
            id.unwrap_or(Value::Null),
            -32600,
            "jsonrpc must be 2.0",
        ));
    }
    let method = value.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return Some(mcp_error(
            id.unwrap_or(Value::Null),
            -32600,
            "missing method",
        ));
    };
    if method == "notifications/initialized" {
        return None;
    }
    if method == "notifications/cancelled" || method == "$/cancelRequest" {
        if let Some(request_value) = value.pointer("/params/requestId")
            && let Ok(map) = cancels.lock()
            && let Some(token) = map.get(rpc_id_key(request_value).as_str())
        {
            token.store(true, Ordering::Relaxed);
        }
        return None;
    }
    let id = id?;
    let params = value.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => Some(mcp_response(
            id,
            json!({"protocolVersion":"2025-11-25","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"mundaris-dev","version":env!("CARGO_PKG_VERSION")}}),
        )),
        "ping" => Some(mcp_response(id, json!({}))),
        "tools/list" => Some(mcp_response(id, json!({"tools":mcp_tools()}))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let token = cancels
                .lock()
                .ok()
                .and_then(|map| map.get(&rpc_id_key(&id)).cloned())
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            if let Ok(mut map) = cancels.lock() {
                map.insert(rpc_id_key(&id), token.clone());
            }
            let result = mcp_tool_call(name, &args, Some(&token));
            if let Ok(mut map) = cancels.lock() {
                map.remove(rpc_id_key(&id).as_str());
            }
            match result {
                Ok((structured, images)) => {
                    let mut content = vec![json!({"type":"text","text":structured.to_string()})];
                    content.extend(images);
                    let is_error = contains_failure(&structured);
                    Some(mcp_response(
                        id,
                        json!({"content":content,"structuredContent":structured,"isError":is_error}),
                    ))
                }
                Err(error) => Some(mcp_response(
                    id,
                    json!({"content":[{"type":"text","text":error.to_string()}],"isError":true}),
                )),
            }
        }
        _ => Some(mcp_error(id, -32601, format!("method not found: {method}"))),
    }
}

/// Run a JSON-RPC MCP stdio server. Input and output are one JSON object per line.
pub fn run_mcp_stdio() -> Result<()> {
    let cancels = Arc::new(Mutex::new(HashMap::new()));
    let (input_tx, input_rx) = mpsc::sync_channel::<Option<String>>(8);
    let (output_tx, output_rx) = mpsc::sync_channel::<Option<Value>>(8);
    thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        while let Ok(Some(line)) = read_capped_stdio_line(&mut reader) {
            let oversized = line.len() > MAX_MESSAGE_BYTES;
            if input_tx.send(Some(line)).is_err() {
                return;
            }
            if oversized {
                break;
            }
        }
        let _ = input_tx.send(None);
    });
    let mut active = 0usize;
    let initialize_seen = AtomicBool::new(false);
    let initialized = AtomicBool::new(false);
    loop {
        while let Ok(message) = output_rx.try_recv() {
            if let Some(message) = message {
                emit(&message)?;
            }
            active = active.saturating_sub(1);
        }
        match input_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(Some(line)) => {
                let value = serde_json::from_str::<Value>(&line).ok();
                let method = value
                    .as_ref()
                    .and_then(|value| value.get("method"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if method == "notifications/initialized" {
                    if initialize_seen.load(Ordering::Acquire) {
                        initialized.store(true, Ordering::Release);
                    }
                    continue;
                }
                if method == "notifications/cancelled" || method == "$/cancelRequest" {
                    let _ = handle_mcp_message(&line, &cancels);
                    continue;
                }
                if method == "initialize" {
                    initialize_seen.store(true, Ordering::Release);
                    if let Some(response) = handle_mcp_message(&line, &cancels) {
                        emit(&response)?;
                    }
                    continue;
                }
                if matches!(method, "tools/list" | "tools/call")
                    && !initialized.load(Ordering::Acquire)
                {
                    if let Some(id) = value.as_ref().and_then(|value| value.get("id")) {
                        emit(&mcp_error(
                            id.clone(),
                            -32002,
                            "initialize and initialized notification are required before tool use",
                        ))?;
                    }
                    continue;
                }
                if active >= 8 {
                    if let Some(id) = value.as_ref().and_then(|value| value.get("id")) {
                        emit(&mcp_error(
                            id.clone(),
                            -32000,
                            "MCP server is busy; retry after a request completes",
                        ))?;
                    }
                    continue;
                }
                if method == "tools/call"
                    && let Some(id) = value.as_ref().and_then(|value| value.get("id"))
                    && let Ok(mut map) = cancels.lock()
                {
                    map.insert(rpc_id_key(id), Arc::new(AtomicBool::new(false)));
                }
                let tx = output_tx.clone();
                let tokens = cancels.clone();
                active = active.saturating_add(1);
                thread::spawn(move || {
                    let response = handle_mcp_message(&line, &tokens);
                    let _ = tx.send(response);
                });
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    if let Ok(tokens) = cancels.lock() {
        for token in tokens.values() {
            token.store(true, Ordering::Relaxed);
        }
    }
    while active > 0 {
        match output_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(message) => {
                if let Some(message) = message {
                    emit(&message)?;
                }
                active = active.saturating_sub(1);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

pub fn cli_main() -> Result<()> {
    let args = normalize_cli_arguments(&std::env::args().skip(1).collect::<Vec<_>>())?;
    if args.first().is_some_and(|s| s == "mcp") {
        return run_mcp_stdio();
    }
    match parse_cli(&args) {
        Ok(value) => emit(&value),
        Err(error) => {
            let _ = emit(&json!({"status":"error","error":error.to_string()}));
            Err(error)
        }
    }
}

fn normalize_cli_arguments(args: &[String]) -> Result<Vec<String>> {
    let mut index = 0;
    while args
        .get(index)
        .is_some_and(|arg| matches!(arg.as_str(), "--registry" | "--session"))
    {
        ensure!(
            args.get(index + 1).is_some(),
            "{} needs a value",
            args[index]
        );
        index += 2;
    }
    if index == 0 {
        return Ok(args.to_vec());
    }
    let command = args.get(index).ok_or_else(|| anyhow!("missing command"))?;
    let mut normalized = vec![command.clone()];
    normalized.extend_from_slice(&args[..index]);
    normalized.extend_from_slice(&args[index + 1..]);
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn global_options_work_before_or_after_the_cli_command() {
        let args = ["--registry", "custom", "--session", "selected", "inspect"].map(str::to_string);
        assert_eq!(
            normalize_cli_arguments(&args).unwrap(),
            ["inspect", "--registry", "custom", "--session", "selected"]
        );
        assert!(normalize_cli_arguments(&["--session".into()]).is_err());
    }
    #[test]
    fn dotted_predicates_are_typed_and_bounded() {
        let snapshot = json!({"terrain":{"settled":true,"lod":12},"general":{"frame_number":9}});
        assert!(predicate_matches(
            &snapshot,
            &json!({"path":"terrain.settled","equals":true})
        ));
        assert!(predicate_matches(
            &snapshot,
            &json!({"path":"terrain.lod","at_least":10})
        ));
        assert!(!predicate_matches(
            &snapshot,
            &json!({"path":"general.frame_number","at_most":8})
        ));
        assert!(!predicate_matches(
            &snapshot,
            &json!({"path":"missing.field","equals":0})
        ));
        let bounded = json!({"path":"general.frame_number","at_least":3,"at_most":9});
        assert!(validate_predicate(&bounded).is_ok());
        assert!(predicate_matches(&snapshot, &bounded));
        assert!(!predicate_matches(
            &snapshot,
            &json!({"path":"general.frame_number","at_least":3,"at_most":7})
        ));
        assert!(
            validate_predicate(&json!({"path":"general.frame_number","at_least":8,"at_most":3}))
                .is_err()
        );
        assert!(
            validate_predicate(&json!({"path":"general.frame_number","equals":3,"at_least":3}))
                .is_err()
        );
    }

    #[test]
    fn mcp_schema_validation_rejects_bad_waits_and_commands_before_io() {
        assert!(validate_mcp_arguments(
            "mundaris_wait",
            &json!({"timeout_ms":30,"predicate":{"field":"general.frame_number","operator":"at_least","value":3}})
        )
        .is_err());
        assert!(
            validate_mcp_arguments(
                "mundaris_wait",
                &json!({"session":123,"command_id":"command"})
            )
            .is_err()
        );
        assert!(
            validate_mcp_arguments(
                "mundaris_wait",
                &json!({"predicate":{"path":"general.frame_number","at_least":3,"at_most":"bad"}})
            )
            .is_err()
        );
        assert!(
            validate_mcp_arguments(
                "mundaris_action",
                &json!({"lease":"lease","command":{"action":"navigation_mode","mode":"invalid"}})
            )
            .is_err()
        );
        assert!(
            validate_mcp_arguments(
                "mundaris_control",
                &json!({"operation":"acquire","owner":"test"})
            )
            .is_ok()
        );
        assert!(
            validate_mcp_arguments(
                "mundaris_control",
                &json!({"operation":"release","lease":"lease"})
            )
            .is_ok()
        );
    }

    #[test]
    fn descriptor_requires_protocol_and_bounded_utf8_fields() {
        let descriptor = SessionDescriptor {
            protocol_version: PROTOCOL_VERSION,
            session_id: "s".into(),
            endpoint: "127.0.0.1:1".into(),
            pid: 1,
            preset: "p".into(),
            executable: "x".into(),
            output_directory: "o".into(),
            binary_sha256: "h".into(),
            build_manifest: None,
        };
        assert!(descriptor_valid(&descriptor));
        let mut invalid = descriptor;
        invalid.protocol_version += 1;
        assert!(!descriptor_valid(&invalid));
    }

    #[test]
    fn tools_are_named_and_have_explicit_object_schemas() {
        let tools = mcp_tools().as_array().unwrap().clone();
        assert_eq!(tools.len(), 12);
        assert!(
            tools
                .iter()
                .all(|t| t["inputSchema"]["type"] == "object"
                    || t["inputSchema"]["oneOf"].is_array())
        );
        assert!(tools.iter().any(|t| t["name"] == "mundaris_ownedlifecycle"));
        for tool in &tools {
            let schema = &tool["inputSchema"];
            if let Some(branches) = schema["oneOf"].as_array() {
                assert!(
                    branches
                        .iter()
                        .all(|branch| branch["properties"]["registry"]["type"] == "string")
                );
            } else {
                assert_eq!(schema["properties"]["registry"]["type"], "string");
            }
        }
    }

    #[test]
    fn initialize_negotiates_supported_version_and_failure_status_is_detected() {
        let cancellations = Arc::new(Mutex::new(HashMap::new()));
        let response = handle_mcp_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
            &cancellations,
        )
        .unwrap();
        assert_eq!(response["result"]["protocolVersion"], "2025-11-25");
        assert!(contains_failure(
            &json!({"accepted":{"status":"accepted"},"result":{"data":{"status":"failed"}}})
        ));
        assert!(!contains_failure(&json!({"status":"accepted"})));
    }

    #[test]
    fn stdio_line_reader_caps_without_draining_unterminated_oversized_input() {
        let input = format!("{}\n{{}}\n", "x".repeat(MAX_MESSAGE_BYTES * 2));
        let input_length = input.len();
        let mut reader = BufReader::new(std::io::Cursor::new(input.into_bytes()));
        let oversized = read_capped_stdio_line(&mut reader).unwrap().unwrap();
        assert_eq!(oversized.len(), MAX_MESSAGE_BYTES + 1);
        assert!(
            reader.get_ref().position() < input_length as u64,
            "oversized input returns without draining the untrusted remainder"
        );
    }
}
