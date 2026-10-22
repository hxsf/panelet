// SPDX-License-Identifier: MPL-2.0

//! Small, bounded loopback HTTP transport; rendering is isolated per request.
use panelet_management::{prepare_widget_candidate, WidgetCandidate};
use panelet_widget_package::{decode_pnw, encode_pnw};
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MAX_BODY: usize = 65_536;
const MAX_HEADERS: usize = 8192;
const MAX_OUTPUT: usize = 262_144;
const WORKER_TIMEOUT: Duration = Duration::from_secs(10);

pub fn error(code: &str, message: &str) -> Value {
    json!({"ok":false,"pixels":null,"width":400,"height":300,"logs":[],
        "error":{"code":code,"message":message}})
}

pub fn serve(port: u16, assets_root: Option<PathBuf>) -> io::Result<()> {
    let assets = assets_root
        .map(crate::assets::LocalAssets::open)
        .transpose()?
        .map(std::sync::Arc::new);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    let port = listener.local_addr()?.port();
    println!("Panelet Studio 小组件编辑器：http://127.0.0.1:{port}");
    if assets.is_some() {
        println!("浏览器 WASM 预览；本机 assets 目录持久化，容量模拟 8 MiB。开发 token：preview-token。Ctrl-C 退出。");
    } else {
        println!("真实 Rune / Canvas；网络与存储使用本次预览的模拟数据。Ctrl-C 退出。");
    }
    // Bounded workers also permit a browser's speculative idle connection.
    let listener = std::sync::Arc::new(listener);
    let mut workers = Vec::new();
    for _ in 0..2 {
        let listener = listener.clone();
        let assets = assets.clone();
        workers.push(std::thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut stream) = connection else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
                let reply = match read_request(&mut BufReader::new(&mut stream), port) {
                    Ok(request) => match &assets {
                        Some(assets) => assets.route(request),
                        None => route(request),
                    },
                    Err((status, message)) => Reply::json(status, error("http_request", message)),
                };
                let rejected = reply.status >= 400;
                let _ = reply.write(&mut stream);
                // Send the response FIN before closing a rejected upload. Closing
                // with unread input can otherwise reset TCP and hide the 413.
                let _ = stream.shutdown(std::net::Shutdown::Write);
                if rejected {
                    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                    let _ = io::copy(
                        &mut (&mut stream).take((MAX_BODY * 2) as u64),
                        &mut io::sink(),
                    );
                }
            }
        }));
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

pub(super) struct Request {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) body: Vec<u8>,
    pub(super) headers: std::collections::BTreeMap<String, String>,
}
type Rejection = (u16, &'static str);

fn read_request(reader: &mut impl BufRead, port: u16) -> Result<Request, Rejection> {
    let mut headers = Vec::new();
    loop {
        let start = headers.len();
        let remaining = MAX_HEADERS + 1 - start;
        let count = (&mut *reader)
            .take(remaining as u64)
            .read_until(b'\n', &mut headers)
            .map_err(|_| (400, "Incomplete request headers"))?;
        if count == 0 || headers.len() > MAX_HEADERS {
            return Err((400, "Invalid or oversized request headers"));
        }
        if &headers[start..] == b"\r\n" {
            break;
        }
    }
    let headers = std::str::from_utf8(&headers).map_err(|_| (400, "Invalid headers"))?;
    let mut lines = headers.split("\r\n");
    let parts: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if parts.len() != 3 || parts[2] != "HTTP/1.1" || !matches!(parts[0], "GET" | "POST") {
        return Err((400, "Expected an HTTP/1.1 GET or POST"));
    }
    let mut fields = std::collections::BTreeMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or((400, "Invalid header"))?;
        let name = name.to_ascii_lowercase();
        if fields.insert(name, value.trim()).is_some() {
            return Err((400, "Duplicate header"));
        }
    }
    let host = fields
        .get("host")
        .copied()
        .ok_or((400, "Host is required"))?;
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err((403, "Only the editor loopback host is allowed"));
    }
    if fields
        .get("origin")
        .is_some_and(|origin| *origin != format!("http://{host}"))
        || (fields
            .get("sec-fetch-site")
            .is_some_and(|site| *site == "cross-site")
            && !(parts[0] == "GET" && parts[1] == "/"))
    {
        return Err((403, "Cross-origin editor requests are not allowed"));
    }
    if fields.contains_key("transfer-encoding") {
        return Err((400, "Transfer-Encoding is not supported"));
    }
    let length = match fields.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| (400, "Invalid Content-Length"))?,
        None if parts[0] == "POST" => return Err((411, "Content-Length is required")),
        None => 0,
    };
    if length > MAX_BODY {
        return Err((413, "Request exceeds 64 KiB"));
    }
    if parts[0] == "POST" {
        let content_type = fields
            .get("content-type")
            .copied()
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let expected = if parts[1] == "/package/decode"
            || parts[1].split('?').next() == Some("/web/v1/chunk")
        {
            "application/octet-stream"
        } else {
            "application/json"
        };
        if !content_type.eq_ignore_ascii_case(expected) {
            return Err((415, "Unsupported Content-Type"));
        }
    } else if length != 0 {
        return Err((400, "GET body is not supported"));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|_| (400, "Incomplete request body"))?;
    Ok(Request {
        method: parts[0].to_owned(),
        path: parts[1].to_owned(),
        body,
        headers: fields
            .into_iter()
            .map(|(name, value)| (name, value.to_owned()))
            .collect(),
    })
}

pub(super) struct Reply {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    headers: Vec<(&'static str, String)>,
}
impl Reply {
    pub(super) fn bytes(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.into(),
            body,
            headers: Vec::new(),
        }
    }
    pub(super) fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
    pub(super) fn json(status: u16, value: Value) -> Self {
        Self::bytes(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(&value).unwrap(),
        )
    }
    fn write(self, stream: &mut impl Write) -> io::Result<()> {
        write!(stream, "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n",
            self.status, match self.status {200 => "OK", 304 => "Not Modified", _ => "Error"}, self.content_type, self.body.len())?;
        if !self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("Content-Security-Policy"))
        {
            stream.write_all(b"Content-Security-Policy: default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; worker-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'\r\n")?;
        }
        if !self
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("Cache-Control"))
        {
            stream.write_all(b"Cache-Control: no-store\r\n")?;
        }
        for (name, value) in self.headers {
            write!(stream, "{name}: {value}\r\n")?;
        }
        stream.write_all(b"\r\n")?;
        stream.write_all(&self.body)
    }
}

fn route(request: Request) -> Reply {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => Reply::bytes(
            200,
            "text/html; charset=utf-8",
            include_bytes!("editor.html").to_vec(),
        ),
        ("GET", "/editor.css") => editor_css(),
        ("GET", "/examples") => Reply::json(200, examples()),
        ("POST", "/preview/v1") => match render_worker(&request.body) {
            Ok(value) => Reply::json(200, value),
            Err(message) => Reply::json(500, error("preview_worker", &message)),
        },
        ("POST", "/package/decode") => {
            match decode_pnw(&request.body)
                .map_err(|e| e.to_string())
                .and_then(validate_package)
            {
                Ok(package) => Reply::json(200, serde_json::to_value(package).unwrap()),
                Err(message) => Reply::json(400, error("invalid_package", &message)),
            }
        }
        ("POST", "/package/encode") => {
            match serde_json::from_slice::<WidgetCandidate>(&request.body)
                .map_err(|e| e.to_string())
                .and_then(validate_package)
            {
                Ok(package) => Reply::bytes(
                    200,
                    "application/vnd.note4c.widget+zip",
                    encode_pnw(&package),
                ),
                Err(message) => Reply::json(400, error("invalid_package", &message)),
            }
        }
        _ => Reply::json(404, error("not_found", "Unknown editor endpoint")),
    }
}

pub(super) fn editor_css() -> Reply {
    let css = include_str!("../server.html")
        .split_once("<style>")
        .unwrap()
        .1
        .split_once("</style>")
        .unwrap()
        .0;
    Reply::bytes(
        200,
        "text/css; charset=utf-8",
        format!("/* SPDX-License-Identifier: MPL-2.0 */\n{css}").into_bytes(),
    )
}

fn validate_package(package: WidgetCandidate) -> Result<WidgetCandidate, String> {
    let package = prepare_widget_candidate(package).map_err(|e| e.message)?;
    decode_pnw(&encode_pnw(&package)).map_err(|e| e.to_string())
}

fn render_worker(body: &[u8]) -> Result<Value, String> {
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--render-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let input = child.stdin.take().unwrap().write_all(body);
    if input.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("Preview worker could not read the request".into());
    }
    let stdout = child.stdout.take().unwrap();
    // Drain concurrently so a full pipe cannot stall a 30,000-byte frame.
    let output = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take((MAX_OUTPUT + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + WORKER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("Preview exceeded the 10 second process deadline".to_owned());
            }
        }
    };
    let bytes = output
        .join()
        .map_err(|_| "Preview output reader failed")?
        .map_err(|_| "Preview output read failed")?;
    if !status?.success() {
        return Err("Preview worker exited unexpectedly".into());
    }
    if bytes.len() > MAX_OUTPUT {
        return Err("Preview output exceeded its limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Preview returned invalid JSON".into())
}

fn examples() -> Value {
    macro_rules! package {
        ($name:literal) => {{
            json!({"manifest":serde_json::from_str::<Value>(include_str!(concat!("../../../../examples/widgets/", $name, "/manifest.json"))).unwrap(),
                "source":include_str!(concat!("../../../../examples/widgets/", $name, "/main.rn")),
                "resources":[{"path":"LICENSE.txt","text":include_str!(concat!("../../../../examples/widgets/", $name, "/resources/LICENSE.txt"))}]})
        }};
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let mut album = package!("photo-album");
    album["resources"] = json!([
        {"path":"LICENSE.txt","text":include_str!("../../../../examples/widgets/photo-album/resources/LICENSE.txt")},
        {"path":"album.json","text":include_str!("../../../../examples/widgets/photo-album/resources/album.json")},
        {"path":"captions/morning.txt","text":include_str!("../../../../examples/widgets/photo-album/resources/captions/morning.txt")},
        {"path":"captions/market.txt","text":include_str!("../../../../examples/widgets/photo-album/resources/captions/market.txt")},
        {"path":"captions/harbor.txt","text":include_str!("../../../../examples/widgets/photo-album/resources/captions/harbor.txt")}
    ]);
    let environment = json!({"unix_ms":now,"utc_offset_minutes":480,"http":[],"shared_files":{}});
    let mut todo_env = environment.clone();
    let url = "https://preview.example.test/todo.json";
    todo_env["http"] = json!([{"url":url,"status":200,"body":include_str!("../../../../examples/widgets/todo/feed.example.json")}]);
    json!([
        {"id":"counter","name":"按键计数器","package":package!("counter"),"config":{},"host_permissions":null,"environment":environment},
        {"id":"calendar","name":"月历","package":package!("calendar"),"config":{},"host_permissions":null,"environment":environment},
        {"id":"todo","name":"待办事项","package":package!("todo"),"config":{"url":url,"title":"待办事项","auth_type":"none"},"host_permissions":[url],"environment":todo_env},
        {"id":"photo-album","name":"相册","package":album,"config":{},"host_permissions":null,"environment":environment}
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Request, Rejection> {
        read_request(&mut io::Cursor::new(text.as_bytes()), 18745)
    }

    #[test]
    fn transport_rejects_cross_origin_rebinding_and_unbounded_bodies() {
        for headers in [
            "Host: attacker.test:18745\r\nContent-Length: 2",
            "Host: localhost:18745\r\nOrigin: https://attacker.test\r\nContent-Length: 2",
            "Host: localhost:18745\r\nContent-Length: 65537",
            "Host: localhost:18745\r\nContent-Length: 2\r\nContent-Length: 2",
            "Host: localhost:18745\r\nTransfer-Encoding: chunked",
        ] {
            assert!(parse(&format!("POST /preview/v1 HTTP/1.1\r\n{headers}\r\nContent-Type: application/json\r\n\r\n{{}}")).is_err());
        }
        assert!(parse("POST /preview/v1 HTTP/1.1\r\nHost: 127.0.0.1:18745\r\nOrigin: http://127.0.0.1:18745\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").is_ok());
        // The device management page links to the desktop editor on another origin.
        assert!(parse(
            "GET / HTTP/1.1\r\nHost: 127.0.0.1:18745\r\nSec-Fetch-Site: cross-site\r\n\r\n"
        )
        .is_ok());
        assert!(parse("POST /preview/v1 HTTP/1.1\r\nHost: 127.0.0.1:18745\r\nSec-Fetch-Site: cross-site\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").is_err());
    }

    #[test]
    fn packages_roundtrip_and_examples_render_with_real_runtime() {
        for example in examples().as_array().unwrap() {
            let package =
                validate_package(serde_json::from_value(example["package"].clone()).unwrap())
                    .unwrap();
            assert!(package
                .source
                .starts_with("// SPDX-License-Identifier: MIT\n"));
            assert!(package
                .source
                .contains("Copyright (c) 2026 Panelet contributors"));
            let license = package
                .resources
                .iter()
                .find(|resource| resource.path == "LICENSE.txt")
                .expect("exported examples retain the MIT terms");
            let full_notice = include_str!("../../../../examples/widgets/LICENSE");
            assert_eq!(
                license.text,
                &full_notice[full_notice.find("Permission").unwrap()..]
            );
            assert_eq!(decode_pnw(&encode_pnw(&package)).unwrap(), package);
            let result = crate::runtime::render(
                json!({"package":example["package"],"config":example["config"],"environment":example["environment"],"host_permissions":example["host_permissions"]}),
            );
            assert_eq!(result["ok"], true, "{}: {}", example["id"], result["error"]);
            assert_eq!(result["pixels"].as_array().unwrap().len(), 30_000);
        }
    }
}
