//! Frontend tests for the `Socket` capability (spec 0050): importing the
//! embedded `std.socket` brings the effect `Socket` and its handle/error types
//! into scope, `Socket.op(...)` calls are gated by `uses { Socket }`, and the
//! `raw_*` backing platform functions validate against the registry (spec
//! 0013/0043). These are `emela check` (frontend) tests — the wasm host that
//! actually runs `socket.*` lands with the wasi:sockets wiring (spec 0050
//! Compilation Notes), so nothing here is built or run.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-socket-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The spec 0050 echo server typechecks: `import std.socket` brings in the
/// effect `Socket` and the `Listener`/`Connection` types, each operation is
/// callable as `Socket.op(...)` inside `uses { Socket }`, `read` yields `Bytes`
/// that `write` accepts, and `close` takes an id. This also exercises the
/// `raw_*` backing externs validating against the registry (the wrappers call
/// them), so a signature drift in the registry would fail here.
#[test]
fn echo_server_typechecks() {
    let output = check_single(
        "import std.socket\n\
         \n\
         fn serve(listener: Listener) -> Unit uses { Socket } {\n\
             try {\n\
                 let conn = Socket.accept(listener)\n\
                 let data = Socket.read(conn, 4096)\n\
                 Socket.write(conn, data)\n\
                 Socket.close(conn.id)\n\
             } catch { e -> () }\n\
             serve(listener)\n\
         }\n\
         \n\
         fn main() -> Unit uses { Socket } {\n\
             try {\n\
                 serve(Socket.listen(8080))\n\
             } catch { e -> () }\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// `SocketError` is a public enum whose variants can be matched — the failure
/// value delivered on the throws channel (spec 0043).
#[test]
fn socket_error_is_matchable() {
    let output = check_single(
        "import std.socket\n\
         \n\
         fn describe(e: SocketError) -> String uses {} {\n\
             match e {\n\
                 SocketError::BindFailed(m) -> m\n\
                 SocketError::AcceptFailed(m) -> m\n\
                 SocketError::ConnectionClosed -> \"closed\"\n\
                 SocketError::Io(m) -> m\n\
             }\n\
         }\n\
         \n\
         fn main() -> Unit uses {} {\n\
             let _ = describe(SocketError::ConnectionClosed)\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// A `Socket` operation is usable only inside a `uses { Socket }` scope: calling
/// `Socket.close` from a `uses {}` function is rejected (spec 0037), which
/// confirms the capability is recognized and gated.
#[test]
fn socket_operation_requires_capability() {
    let output = check_single(
        "import std.socket\n\
         \n\
         fn main() -> Unit uses {} {\n\
             try { Socket.close(1) } catch { e -> () }\n\
         }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    assert!(
        stderr(&output).contains("Socket"),
        "diagnostic should name the Socket effect:\n{}",
        stderr(&output)
    );
}

/// The backing `raw_*` operations are private to the effect (spec 0037): a
/// program cannot call `Socket.raw_listen` directly; only the `pub fn` wrappers
/// are public.
#[test]
fn backing_operations_are_private() {
    let output = check_single(
        "import std.socket\n\
         \n\
         fn main() -> Unit uses { Socket } {\n\
             try {\n\
                 let _ = Socket.raw_listen(8080)\n\
             } catch { e -> () }\n\
         }\n",
    );
    assert!(
        !output.status.success(),
        "expected check to fail: raw_listen is private"
    );
}
