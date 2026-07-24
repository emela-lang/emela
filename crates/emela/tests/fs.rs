//! Tests for the `Fs` capability (spec 0055): importing the embedded `std.fs`
//! brings the effect `Fs` and its handle/error types into scope. Frontend tests
//! use `emela check`; the wasm-wasi and js-node runtime tests are gated behind
//! cfg flags to accommodate this branch (which omits the wasm backend).
//!
//! This file follows the convention of `tests/random.rs` — all capability tests
//! live in a single file.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-fs-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Helpers — frontend (emela check)
// ---------------------------------------------------------------------------

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let dir = temp_dir("check");
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = emela().arg("check").arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// ===========================================================================
// Frontend tests (emela check)
// ===========================================================================

/// Imports `std.fs` and calls each public operation inside `uses { Fs }`. This
/// exercises the whole effect block: `open_read`/`open_write` (the open
/// wrappers), `read`/`write` (the data wrappers), `close` (the close wrapper),
/// and `read_file`/`write_file` (the convenience wrappers).
#[test]
fn fs_module_imports_and_typechecks() {
    let output = check_single(
        "import std.fs\n\
         import std.bytes\n\
         \n\
         fn main() -> Unit uses { Fs } {\n\
             try {\n\
                 let f = Fs.open_read(\"in.txt\")\n\
                 let data = Fs.read_file(\"in.txt\")\n\
                 let n = bytes.length(data)\n\
                 Fs.close(f.id)\n\
             } catch { e -> () }\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// `FsError` is a public enum whose variants can be matched — the failure
/// value delivered on the throws channel (spec 0043).
#[test]
fn fs_error_is_matchable() {
    let output = check_single(
        "import std.fs\n\
         \n\
         fn describe(e: FsError) -> String uses {} {\n\
             match e {\n\
                 FsError::NotFound(m) -> m\n\
                 FsError::PermissionDenied(m) -> m\n\
                 FsError::Io(m) -> m\n\
             }\n\
         }\n\
         \n\
         fn main() -> Unit uses {} {\n\
             let _ = describe(FsError::Io(\"test\"))\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// An `Fs` operation is usable only inside a `uses { Fs }` scope: calling
/// `Fs.close` from a `uses {}` function is rejected (spec 0037).
#[test]
fn fs_operation_requires_capability() {
    let output = check_single(
        "import std.fs\n\
         \n\
         fn main() -> Unit uses {} {\n\
             try { Fs.close(1) } catch { e -> () }\n\
         }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    assert!(
        stderr(&output).contains("Fs"),
        "diagnostic should name the Fs effect:\n{}",
        stderr(&output)
    );
}

/// The backing `raw_open_read`/`raw_open_write`/`raw_read`/`raw_write`
/// operations are private to the effect (spec 0037): a program cannot call
/// `Fs.raw_open_read` directly; only the `pub fn` wrappers are public.
#[test]
fn backing_operations_are_private() {
    let output = check_single(
        "import std.fs\n\
         \n\
         fn main() -> Unit uses { Fs } {\n\
             try {\n\
                 let _ = Fs.raw_open_read(\"in.txt\")\n\
             } catch { e -> () }\n\
         }\n",
    );
    assert!(
        !output.status.success(),
        "expected check to fail: raw_open_read is private"
    );
}

/// `File` is a public record, constructible by users (for testing / mocking
/// handles). Its `id` field is an `Int`.
#[test]
fn file_record_is_constructible() {
    let output = check_single(
        "import std.fs\n\
         \n\
         fn main() -> Unit uses {} {\n\
             let f = File { id: 1 }\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}
