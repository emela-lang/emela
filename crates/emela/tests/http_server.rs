//! End-to-end tests for the `HttpServer` capability (spec 0046). Each starts a
//! compiled Emela server as a child `emela run` process, drives it over a raw
//! TCP connection, and asserts on the response.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("emela-server-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An unused loopback port (bind, read the port, drop the listener).
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A running `emela run` child that is killed and reaped on drop.
struct Server {
    child: Child,
    dir: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Writes `source` (a `{PORT}` placeholder is substituted), starts it under
/// `emela run`, and waits until the port accepts connections.
fn start_server(label: &str, port: u16, source: &str) -> Server {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    fs::write(&input, source.replace("{PORT}", &port.to_string())).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Server { child, dir };
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not start listening on port {port}");
}

/// Sends a raw request and reads the whole response (server closes the
/// connection, so read to EOF).
fn round_trip(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

const ECHO_SERVER: &str = r#"import std.io
import std.http

fn handle(req: Request) -> Response uses {} {
    if req.url == "/" {
        Response {
            status: 200
            headers: []
            body: "hello from Emela\n"
        }
    } else {
        Response {
            status: 404
            headers: []
            body: "not found\n"
        }
    }
}

fn serve_loop(server: Server) -> Unit uses { HttpServer } {
    try {
        let inc = HttpServer.accept(server)
        HttpServer.respond(inc, handle(inc.request))
    } catch {
        e -> ()
    }
    serve_loop(server)
}

fn main() -> Unit uses { Io, HttpServer } {
    try {
        serve_loop(HttpServer.bind({PORT}))
    } catch {
        e -> Io.eprint("bind failed\n")
    }
}
"#;

/// The server routes on the request URL and serves many requests in a row
/// (the tail-recursive accept loop, spec 0045/0046).
#[test]
fn serves_routed_requests_in_a_loop() {
    let port = free_port();
    let server = start_server("routed", port, ECHO_SERVER);

    let ok = round_trip(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(ok.starts_with("HTTP/1.1 200 OK"), "{ok}");
    assert!(ok.ends_with("hello from Emela\n"), "{ok}");

    let missing = round_trip(port, "GET /missing HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(missing.starts_with("HTTP/1.1 404 Not Found"), "{missing}");
    assert!(missing.ends_with("not found\n"), "{missing}");

    // A second request on the root proves the loop kept accepting.
    let again = round_trip(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(again.ends_with("hello from Emela\n"), "{again}");

    drop(server);
}

/// A method outside `Method` is answered 501 by the host without reaching the
/// handler (spec 0046 S3); the loop then serves the next, valid request.
#[test]
fn unknown_method_is_answered_501_by_the_host() {
    let port = free_port();
    let server = start_server("unknown-method", port, ECHO_SERVER);

    let response = round_trip(port, "BREW / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(response.starts_with("HTTP/1.1 501"), "{response}");

    // The server is still alive and serving after rejecting the odd method.
    let ok = round_trip(port, "GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(ok.ends_with("hello from Emela\n"), "{ok}");

    drop(server);
}

/// The handler can read the request body and echo it back (POST round trip).
#[test]
fn handler_reads_request_body() {
    let port = free_port();
    let source = r#"import std.io
import std.http

fn serve_loop(server: Server) -> Unit uses { HttpServer } {
    try {
        let inc = HttpServer.accept(server)
        let reply = Response {
            status: 200
            headers: []
            body: inc.request.body
        }
        HttpServer.respond(inc, reply)
    } catch {
        e -> ()
    }
    serve_loop(server)
}

fn main() -> Unit uses { Io, HttpServer } {
    try {
        serve_loop(HttpServer.bind({PORT}))
    } catch {
        e -> Io.eprint("bind failed\n")
    }
}
"#;
    let server = start_server("echo-body", port, source);

    let response = round_trip(
        port,
        "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello",
    );
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(response.ends_with("hello"), "{response}");

    drop(server);
}

/// `HttpServer` is a distinct capability from the client `Http` (spec 0046
/// S1): a serve-only program's capability manifest shows `HttpServer` and not
/// `Http`.
#[test]
fn serve_only_program_does_not_require_the_client_capability() {
    let dir = temp_dir("manifest");
    let input = dir.join("main.emel");
    let out = dir.join("server.wasm");
    fs::write(&input, ECHO_SERVER.replace("{PORT}", "8080")).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&out)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let bytes = fs::read(&out).unwrap();
    let _ = fs::remove_dir_all(&dir);
    // Import module/field names are stored as UTF-8 in the binary. A serve-only
    // program imports the server host functions but not the client `request`.
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("server_accept"), "server import missing");
    assert!(
        !text.contains("request"),
        "serve-only program must not import the client `request`"
    );
}
