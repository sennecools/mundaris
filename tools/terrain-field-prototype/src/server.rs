//! Small single-threaded loopback authoring API; browser requests never choose paths.
use crate::{
    Error,
    graph::{self, CompiledGraph, Graph, MAX_GRAPH_BYTES, VariationLocks},
    graph_bundle,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_BODY: usize = MAX_GRAPH_BYTES + 64 * 1024;
const MAX_HEADER: usize = 16 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewRequest {
    graph: Graph,
    resolution: u32,
    face: usize,
    inspect_node: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphRequest {
    graph: Graph,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportRequest {
    graph: Graph,
    resolution: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VariationRequest {
    graph: Graph,
    seed: i64,
    locks: VariationLocks,
}

pub fn serve(output_root: &Path, port: u16) -> Result<(), Error> {
    fs::create_dir_all(output_root)?;
    let root = output_root.canonicalize()?;
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))?;
    println!(
        "terrain authoring server listening at http://127.0.0.1:{}",
        listener.local_addr()?.port()
    );
    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                let actual_port = listener.local_addr()?.port();
                if let Err(e) = handle(&mut stream, actual_port, &root) {
                    let body =
                        json!({"ok":false,"error":format!("request failed: {e}")}).to_string();
                    let _ = respond(&mut stream, 400, "application/json; charset=utf-8", &body);
                }
            }
            Err(e) => eprintln!("loopback accept failed: {e}"),
        }
    }
    Ok(())
}

fn handle(stream: &mut TcpStream, port: u16, output_root: &Path) -> Result<(), Error> {
    let request = match read_request(stream)? {
        Some(r) => r,
        None => {
            return respond(
                stream,
                400,
                "application/json; charset=utf-8",
                "{\"ok\":false,\"error\":\"malformed request\"}",
            );
        }
    };
    if !valid_host(&request.host, port) {
        return respond(
            stream,
            403,
            "application/json; charset=utf-8",
            "{\"ok\":false,\"error\":\"Host must target this loopback service\"}",
        );
    }
    if let Some(origin) = request.origin.as_deref()
        && !valid_origin(origin, port)
    {
        return respond(
            stream,
            403,
            "application/json; charset=utf-8",
            "{\"ok\":false,\"error\":\"Origin must be this loopback service\"}",
        );
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/catalog") => respond(
            stream,
            200,
            "application/json; charset=utf-8",
            &graph::catalog().to_string(),
        ),
        ("GET", "/") => static_file(stream, "index.html", "text/html; charset=utf-8"),
        ("GET", "/app.js") => static_file(stream, "app.js", "text/javascript; charset=utf-8"),
        ("GET", "/style.css") => static_file(stream, "style.css", "text/css; charset=utf-8"),
        ("POST", "/api/preview") => with_json::<PreviewRequest>(stream, &request.body, |r| {
            if r.inspect_node.as_deref().is_some_and(|id| id.len() > 64) {
                return Err(Error::Invalid("inspect_node is too long".into()));
            };
            let compiled = CompiledGraph::compile(r.graph.clone())?;
            let map = compiled.sample_map(r.resolution, r.face, r.inspect_node.as_deref())?;
            let node_values = if let Some(id) = r.inspect_node.as_deref() {
                let mut desc = compiled.inspect_descriptor(id)?;
                desc["values"] = serde_json::to_value(map.node_values.clone().unwrap_or_default())?;
                desc
            } else {
                Value::Null
            };
            let maps = json!({"height":map.height_m,"humidity":map.humidity,"temperature":map.temperature_k,"material":map.material,"weight_0":map.weight_0,"weight_1":map.weight_1,"weight_2":map.weight_2,"support":map.support,"normal":map.normal});
            Ok(
                json!({"ok":true,"identities":compiled.identities(),"stats":map.stats,"width":map.width,"height":map.height,"maps":maps,"node_values":node_values,"ranges":compiled.ranges()}),
            )
        }),
        ("POST", "/api/validate") => with_json::<GraphRequest>(stream, &request.body, |r| {
            let c = CompiledGraph::compile(r.graph)?;
            Ok(json!({"ok":true,"identities":c.identities()}))
        }),
        ("POST", "/api/variation") => with_json::<VariationRequest>(stream, &request.body, |r| {
            if r.seed < 0 || r.seed as u64 > graph::MAX_JS_SAFE_SEED {
                return Err(Error::Invalid(
                    "variation seed must be a JavaScript-safe nonnegative integer".into(),
                ));
            }
            let g = graph::vary(&r.graph, r.seed as u64, &r.locks)?;
            Ok(json!({"ok":true,"graph":g}))
        }),
        ("POST", "/api/export") => with_json::<ExportRequest>(stream, &request.body, |r| {
            let (path, sha, count) = graph_bundle::publish(&r.graph, r.resolution, output_root)?;
            Ok(
                json!({"ok":true,"bundle_path":path.to_string_lossy(),"manifest_sha256":sha,"files":count}),
            )
        }),
        _ => respond(
            stream,
            404,
            "application/json; charset=utf-8",
            "{\"ok\":false,\"error\":\"route not found\"}",
        ),
    }
}

struct Request {
    method: String,
    path: String,
    host: String,
    origin: Option<String>,
    body: Vec<u8>,
}
fn read_request(stream: &mut TcpStream) -> Result<Option<Request>, Error> {
    let mut data = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];
    let end = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(None);
        };
        data.extend_from_slice(&chunk[..n]);
        if data.len() > MAX_HEADER + MAX_BODY {
            return Err(Error::Invalid("request exceeds size limit".into()));
        };
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if data.len() > MAX_HEADER {
            return Err(Error::Invalid("request headers exceed size limit".into()));
        }
    };
    if end > MAX_HEADER {
        return Err(Error::Invalid("request headers exceed size limit".into()));
    }
    let headers = std::str::from_utf8(&data[..end])
        .map_err(|_| Error::Invalid("HTTP headers must be UTF-8".into()))?;
    let mut lines = headers.split("\r\n");
    let first = lines
        .next()
        .ok_or_else(|| Error::Invalid("missing request line".into()))?;
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts
        .next()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("")
        .to_owned();
    if parts.next().is_none() || !matches!(method.as_str(), "GET" | "POST") {
        return Err(Error::Invalid("unsupported HTTP request line".into()));
    };
    let mut host = None;
    let mut origin = None;
    let mut length = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host" => {
                    if host.replace(v.trim().to_owned()).is_some() {
                        return Err(Error::Invalid("duplicate Host header".into()));
                    }
                }
                "origin" => {
                    if origin.replace(v.trim().to_owned()).is_some() {
                        return Err(Error::Invalid("duplicate Origin header".into()));
                    }
                }
                "content-length" => {
                    let parsed = v
                        .trim()
                        .parse()
                        .map_err(|_| Error::Invalid("invalid Content-Length".into()))?;
                    if length.replace(parsed).is_some() {
                        return Err(Error::Invalid("duplicate Content-Length header".into()));
                    }
                }
                "transfer-encoding" => {
                    return Err(Error::Invalid(
                        "chunked request bodies are not accepted".into(),
                    ));
                }
                _ => (),
            }
        }
    }
    if method == "POST" && length.is_none() {
        return Err(Error::Invalid("POST requires Content-Length".into()));
    }
    let length = length.unwrap_or(0);
    if length > MAX_BODY {
        return Err(Error::Invalid("request body exceeds size limit".into()));
    };
    let mut body = data[end..].to_vec();
    if body.len() > length {
        return Err(Error::Invalid(
            "request contains unexpected trailing body data".into(),
        ));
    };
    while body.len() < length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(Error::Invalid("request body ended early".into()));
        };
        body.extend_from_slice(&chunk[..n]);
        if body.len() > MAX_BODY {
            return Err(Error::Invalid("request body exceeds size limit".into()));
        }
    }
    Ok(Some(Request {
        method,
        path,
        host: host.ok_or_else(|| Error::Invalid("Host header is required".into()))?,
        origin,
        body,
    }))
}
fn valid_host(host: &str, port: u16) -> bool {
    host == format!("127.0.0.1:{port}") || host == format!("localhost:{port}")
}
fn valid_origin(origin: &str, port: u16) -> bool {
    origin == format!("http://127.0.0.1:{port}") || origin == format!("http://localhost:{port}")
}
fn with_json<T: for<'de> Deserialize<'de>>(
    stream: &mut TcpStream,
    bytes: &[u8],
    f: impl FnOnce(T) -> Result<Value, Error>,
) -> Result<(), Error> {
    if bytes.len() > MAX_BODY {
        return respond_status(
            stream,
            413,
            "application/json; charset=utf-8",
            "{\"ok\":false,\"error\":\"request body exceeds size limit\"}",
        );
    };
    let request = match serde_json::from_slice::<T>(bytes) {
        Ok(r) => r,
        Err(e) => {
            return respond_status(
                stream,
                400,
                "application/json; charset=utf-8",
                &json!({"ok":false,"error":format!("invalid request JSON: {e}")}).to_string(),
            );
        }
    };
    match f(request) {
        Ok(value) => respond(
            stream,
            200,
            "application/json; charset=utf-8",
            &value.to_string(),
        ),
        Err(e) => respond_status(
            stream,
            400,
            "application/json; charset=utf-8",
            &json!({"ok":false,"error":e.to_string()}).to_string(),
        ),
    }
}
fn static_file(stream: &mut TcpStream, name: &str, content_type: &str) -> Result<(), Error> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("web")
        .join(name);
    let bytes = fs::read(&path)
        .map_err(|e| Error::Invalid(format!("web asset {} is unavailable: {e}", path.display())))?;
    let text =
        String::from_utf8(bytes).map_err(|_| Error::Invalid("web asset is not UTF-8".into()))?;
    respond(stream, 200, content_type, &text)
}
fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), Error> {
    respond_status(stream, status, content_type, body)
}
fn respond_status(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> Result<(), Error> {
    let label = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {label}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;
    Ok(())
}
