#![cfg(feature = "developer-tools")]

use mundaris_app::{
    developer_bridge::{Client, discover},
    developer_protocol::{
        DevOperation, DevRequest, DevResponse, PROTOCOL_VERSION, SessionDescriptor,
    },
};
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "mundaris-dev-bridge-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn descriptor(session_id: &str, endpoint: String) -> SessionDescriptor {
    SessionDescriptor {
        protocol_version: PROTOCOL_VERSION,
        session_id: session_id.into(),
        endpoint,
        pid: std::process::id(),
        preset: "gameplay".into(),
        executable: "mundaris_app".into(),
        output_directory: "target/dev".into(),
        binary_sha256: "test-hash".into(),
        build_manifest: None,
    }
}

#[test]
fn client_uses_one_bounded_newline_json_exchange_and_checks_session_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let session = descriptor("session-a", endpoint);
    let thread = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let request: DevRequest = serde_json::from_str(&line).unwrap();
        assert!(matches!(&request.operation, DevOperation::Inspect));
        let response = DevResponse::new(
            &request.session_id,
            &request.request_id,
            "ok",
            json!({"frame":7}),
        );
        let mut stream = reader.into_inner();
        serde_json::to_writer(&mut stream, &response).unwrap();
        stream.write_all(b"\n").unwrap();
    });
    let response = Client::new(session).request(DevOperation::Inspect).unwrap();
    assert_eq!(response.status, "ok");
    assert_eq!(response.data["frame"], 7);
    thread.join().unwrap();
}

#[test]
fn discover_returns_only_sessions_confirmed_by_the_live_endpoint() {
    let scratch = Scratch::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    let live = descriptor("live", endpoint);
    let live_copy = live.clone();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let request: DevRequest = serde_json::from_str(&line).unwrap();
        assert!(matches!(&request.operation, DevOperation::Capabilities));
        let response = DevResponse::new(
            &request.session_id,
            &request.request_id,
            "ok",
            json!({"session_id": live_copy.session_id}),
        );
        let mut stream = reader.into_inner();
        serde_json::to_writer(&mut stream, &response).unwrap();
        stream.write_all(b"\n").unwrap();
    });
    fs::write(
        scratch.0.join("live.json"),
        serde_json::to_vec(&live).unwrap(),
    )
    .unwrap();
    fs::write(
        scratch.0.join("dead.json"),
        serde_json::to_vec(&descriptor("dead", "127.0.0.1:1".into())).unwrap(),
    )
    .unwrap();
    assert_eq!(
        discover(&scratch.0)
            .unwrap()
            .iter()
            .map(|s| s.session_id.as_str())
            .collect::<Vec<_>>(),
        ["live"]
    );
    server.join().unwrap();
}

#[test]
fn discovery_ignores_malformed_or_non_utf8_registry_entries() {
    let scratch = Scratch::new();
    fs::write(scratch.0.join("bad.json"), [0xff, 0xfe]).unwrap();
    fs::write(scratch.0.join("ignored.txt"), b"not a descriptor").unwrap();
    assert!(discover(&scratch.0).unwrap().is_empty());
}

#[test]
fn missing_registry_is_an_empty_session_list() {
    let scratch = Scratch::new();
    let missing = scratch.0.join("not-created");
    assert!(discover(&missing).unwrap().is_empty());
}

fn bounded_responder(
    listener: TcpListener,
    session_id: String,
    expected_requests: usize,
) -> thread::JoinHandle<usize> {
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut handled = 0;
        while handled < expected_requests && Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let request: DevRequest = serde_json::from_str(&line).unwrap();
                    let data = match &request.operation {
                        DevOperation::Capabilities => json!({"session_id":session_id}),
                        DevOperation::Inspect => json!({"selected_session":session_id}),
                        _ => panic!("unexpected mock operation"),
                    };
                    let response = DevResponse::new(&session_id, &request.request_id, "ok", data);
                    let mut stream = reader.into_inner();
                    serde_json::to_writer(&mut stream, &response).unwrap();
                    stream.write_all(b"\n").unwrap();
                    handled += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("mock listener failed: {error}"),
            }
        }
        handled
    })
}

#[test]
fn session_selection_rejects_ambiguity_and_honors_explicit_live_id() {
    let scratch = Scratch::new();
    let first_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let second_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let first_id = "session-a";
    let chosen_id = "session-b";
    let first = descriptor(first_id, first_listener.local_addr().unwrap().to_string());
    let chosen = descriptor(chosen_id, second_listener.local_addr().unwrap().to_string());
    fs::write(
        scratch.0.join("session-a.json"),
        serde_json::to_vec(&first).unwrap(),
    )
    .unwrap();
    fs::write(
        scratch.0.join("session-b.json"),
        serde_json::to_vec(&chosen).unwrap(),
    )
    .unwrap();

    // The first CLI discovery probes both endpoints; the explicit selection
    // probes both again and then inspects only session-b.
    let first_server = bounded_responder(first_listener, first_id.into(), 2);
    let chosen_server = bounded_responder(second_listener, chosen_id.into(), 3);
    let binary = env!("CARGO_BIN_EXE_mundaris_dev");
    let ambiguous = Command::new(binary)
        .args(["inspect", "--registry"])
        .arg(&scratch.0)
        .output()
        .unwrap();
    let explicit = Command::new(binary)
        .args(["--registry"])
        .arg(&scratch.0)
        .args(["--session", chosen_id, "inspect"])
        .output()
        .unwrap();
    let first_handled = first_server.join().unwrap();
    let chosen_handled = chosen_server.join().unwrap();

    assert_eq!(first_handled, 2);
    assert_eq!(chosen_handled, 3);
    assert!(!ambiguous.status.success());
    let ambiguous_json: serde_json::Value = serde_json::from_slice(&ambiguous.stdout).unwrap();
    assert_eq!(ambiguous_json["status"], "error");
    assert!(
        ambiguous_json["error"]
            .as_str()
            .unwrap()
            .contains("multiple live sessions")
    );

    assert!(explicit.status.success());
    let explicit_json: serde_json::Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(explicit_json["session_id"], chosen_id);
    assert_eq!(explicit_json["data"]["selected_session"], chosen_id);
}

#[test]
fn client_preserves_connection_overflow_and_rejects_cross_session_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let session = descriptor("selected", listener.local_addr().unwrap().to_string());
    let worker = thread::spawn(move || {
        for wrong_session in [false, true] {
            let (socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: DevRequest = serde_json::from_str(&line).unwrap();
            let response = if wrong_session {
                DevResponse::new("unrelated", &request.request_id, "ok", json!({}))
            } else {
                DevResponse::new(
                    "selected",
                    "",
                    "failed",
                    json!({"error":"connection_overflow"}),
                )
            };
            let mut socket = reader.into_inner();
            serde_json::to_writer(&mut socket, &response).unwrap();
            socket.write_all(b"\n").unwrap();
        }
    });
    let client = Client::new(session);
    let overflow = client.request(DevOperation::Inspect).unwrap();
    assert_eq!(overflow.data["error"], "connection_overflow");
    assert!(client.request(DevOperation::Inspect).is_err());
    worker.join().unwrap();
}
