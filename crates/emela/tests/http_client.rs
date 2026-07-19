//! End-to-end tests for the `Http` client capability (specs 0043/0044). Each
//! spins up a throwaway TCP server on the loopback interface, then runs a
//! compiled Emela program through `emela run` (the wasmi host) against it.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, thread};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-http-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_source(label: &str, source: &str) -> Output {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

/// Serves one connection with a fixed raw HTTP response, then returns the bytes
/// the client sent. Runs on a background thread so the client can connect.
fn serve_once(response: &'static [u8]) -> (u16, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // Read the whole request: the head, then the declared body so the
        // client's write completes before we respond and close.
        let mut received = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let head_end = received.windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(end) = head_end {
                let head = String::from_utf8_lossy(&received[..end]).to_ascii_lowercase();
                let content_length = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                if received.len() >= end + 4 + content_length {
                    break;
                }
            }
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            received.extend_from_slice(&buf[..n]);
        }
        stream.write_all(response).unwrap();
        stream.flush().unwrap();
        // Drop closes the connection, letting the client read to EOF.
        drop(stream);
        String::from_utf8_lossy(&received).into_owned()
    });
    (port, handle)
}

/// A GET returns the body and status; the request the host sent carries the
/// method, an origin-form target, and a `host` header (spec 0044).
#[test]
fn get_returns_body_and_sends_a_well_formed_request() {
    let (port, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\nhello\n",
    );
    let source = format!(
        "import std.io\nimport std.http\n\nfn main() -> Unit uses {{ Io, Http }} {{\n    let res = try {{\n        Http.get(\"http://127.0.0.1:{port}/hi\")\n    }} catch {{\n        e -> Response {{ status: 0  headers: []  body: \"error\\n\" }}\n    }}\n    Io.print(res.body)\n}}\n"
    );
    let output = run_source("get", &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
    let request = server.join().unwrap();
    assert!(request.starts_with("GET /hi HTTP/1.1"), "{request}");
    assert!(
        request.to_ascii_lowercase().contains("host: 127.0.0.1"),
        "{request}"
    );
}

/// A non-2xx status is a successful `Response`, not an `HttpError` (spec 0044
/// H4): the program reads the 404 status without entering `catch`.
#[test]
fn non_2xx_status_is_a_successful_response() {
    let (port, server) = serve_once(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    let source = format!(
        "import std.io\nimport std.http\n\nfn describe(res: Response) -> String uses {{}} {{\n    if res.status == 404 {{\n        \"missing\\n\"\n    }} else {{\n        \"other\\n\"\n    }}\n}}\n\nfn main() -> Unit uses {{ Io, Http }} {{\n    let out = try {{\n        describe(Http.get(\"http://127.0.0.1:{port}/\"))\n    }} catch {{\n        e -> \"threw\\n\"\n    }}\n    Io.print(out)\n}}\n"
    );
    let output = run_source("status", &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "missing\n");
    let _ = server.join();
}

/// A POST sends the body with a matching `content-length` (spec 0044 H5).
#[test]
fn post_sends_the_body() {
    let (port, server) = serve_once(b"HTTP/1.1 201 Created\r\nContent-Length: 3\r\n\r\nok!");
    let source = format!(
        "import std.io\nimport std.http\n\nfn main() -> Unit uses {{ Io, Http }} {{\n    let res = try {{\n        Http.post(\"http://127.0.0.1:{port}/submit\", \"payload\")\n    }} catch {{\n        e -> Response {{ status: 0  headers: []  body: \"error\\n\" }}\n    }}\n    Io.print(res.body)\n}}\n"
    );
    let output = run_source("post", &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "ok!");
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /submit HTTP/1.1"), "{request}");
    assert!(
        request.to_ascii_lowercase().contains("content-length: 7"),
        "{request}"
    );
    assert!(request.ends_with("payload"), "{request}");
}

/// Connecting to a closed port surfaces as `HttpError::ConnectFailed` on the
/// throws channel, caught by the program.
#[test]
fn connect_failure_is_an_http_error() {
    // Bind then drop to get a port nothing is listening on.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    // Make sure the port is really free before the client tries it.
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    let source = format!(
        "import std.io\nimport std.http\n\nfn main() -> Unit uses {{ Io, Http }} {{\n    let out = try {{\n        Http.get(\"http://127.0.0.1:{port}/\").body\n    }} catch {{\n        HttpError::ConnectFailed(msg) -> \"connect failed\\n\"\n        e -> \"other\\n\"\n    }}\n    Io.print(out)\n}}\n"
    );
    let output = run_source("connect-fail", &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "connect failed\n");
}

/// An `https` URL is rejected by the built-in runner (no TLS, spec 0044 H8).
#[test]
fn https_is_rejected_without_tls() {
    let source = "import std.io\nimport std.http\n\nfn main() -> Unit uses { Io, Http } {\n    let out = try {\n        Http.get(\"https://example.com/\").body\n    } catch {\n        HttpError::ConnectFailed(msg) -> \"no tls\\n\"\n        e -> \"other\\n\"\n    }\n    Io.print(out)\n}\n";
    let output = run_source("https", source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "no tls\n");
}

/// The JS backend does not supply `http.*`, so a program requiring `Http` is
/// rejected at build time (spec 0013 coverage check) — the system working as
/// designed for a target without the capability.
#[test]
fn js_backend_rejects_http() {
    let dir = temp_dir("js-reject");
    let input = dir.join("main.emel");
    fs::write(
        &input,
        "import std.http\n\nfn main() -> Unit uses { Http } {\n    let res = try { Http.get(\"http://x/\") } catch { e -> Response { status: 0  headers: []  body: \"\" } }\n    ()\n}\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("js-node")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not provide"), "{stderr}");
    assert!(stderr.contains("http.request"), "{stderr}");
}
