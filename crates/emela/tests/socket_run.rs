//! End-to-end tests for the `Socket` capability (spec 0050): a compiled Emela
//! raw-TCP echo server is started as a child `emela run` process and driven
//! over a real TCP connection. This exercises the whole path — the wasm backend
//! lowering `socket.raw_*` to `emela_socket` imports and the wasmi host backing
//! them with `std::net` — not just the frontend.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{fs, thread};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// The spec 0050 echo server: accept, read up to 4096 bytes, write them back,
/// close, and loop (self-tail-call, spec 0045). `{PORT}` is substituted.
const ECHO_SERVER: &str = r#"import std.socket

fn serve(listener: Listener) -> Unit uses { Socket } {
    try {
        let conn = Socket.accept(listener)
        let data = Socket.read(conn, 4096)
        Socket.write(conn, data)
        Socket.close(conn.id)
    } catch { e -> () }
    serve(listener)
}

fn main() -> Unit uses { Socket } {
    try {
        serve(Socket.listen({PORT}))
    } catch { e -> () }
}
"#;

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "emela-socket-run-{label}-{}-{id}",
        std::process::id()
    ));
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
    let mut child = child;
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(&dir);
    panic!("server did not start listening on port {port}");
}

/// Connects, sends `payload`, and reads the echo to EOF (the server closes the
/// connection after echoing).
fn echo_round_trip(port: u16, payload: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(payload).unwrap();
    stream.flush().unwrap();
    let mut received = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => received.extend_from_slice(&buf[..n]),
            // The server closes after echoing; a reset while tearing down is
            // equivalent to EOF here (the echo is already buffered).
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("read failed: {e}"),
        }
    }
    received
}

/// A raw TCP echo server built from `Socket` primitives returns exactly the
/// bytes it was sent.
#[test]
fn echo_server_round_trips_ascii() {
    let port = free_port();
    let server = start_server("ascii", port, ECHO_SERVER);
    let echoed = echo_round_trip(port, b"hello, socket\n");
    drop(server);
    assert_eq!(echoed, b"hello, socket\n");
}

/// `Socket.read`/`write` are byte-exact (spec 0050 B1): a non-ASCII UTF-8
/// payload round-trips unchanged, byte for byte.
#[test]
fn echo_server_round_trips_utf8_bytes() {
    let port = free_port();
    let server = start_server("utf8", port, ECHO_SERVER);
    // "héllo" is 6 bytes in UTF-8 ("é" = 0xC3 0xA9).
    let payload = "héllo".as_bytes();
    let echoed = echo_round_trip(port, payload);
    drop(server);
    assert_eq!(echoed, payload);
}
