//! End-to-end tests for effect-row subsumption on function values (spec 0023):
//! a function whose `uses` row is a subset is acceptable where a wider row is
//! wanted — as a call argument or a record field. The reverse (a wider row
//! where a narrower one is wanted) is rejected. Effects are backed by the
//! embedded `std.io` (spec 0038), which resolves with no `--package`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-subsume-{}-{id}", std::process::id()));
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

/// A pure function value (`uses {}`) is acceptable where a `uses { Io }`
/// parameter is wanted: `{} ⊆ { Io }` (spec 0023 subsumption).
#[test]
fn pure_handler_accepted_as_argument() {
    let output = check_single(
        "import std.io\n\
         fn apply(f: (Int) -> Int uses { Io }, x: Int) -> Int uses { Io } { f(x) }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply(pure_inc, 5)\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// The same subsumption applies when storing a handler into a record field
/// whose declared row is wider — the shape quince's Router relies on.
#[test]
fn pure_handler_accepted_in_record_field() {
    let output = check_single(
        "import std.io\n\
         record Handler { run: (Int) -> Int uses { Io } }\n\
         fn pure_inc(x: Int) -> Int uses {} { x + 1 }\n\
         fn main() -> Unit uses { Io } {\n\
             let h = Handler { run: pure_inc }\n\
             let f = h.run\n\
             let g = f(5)\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// The reverse is rejected: an effectful function (`uses { Io }`) is not
/// acceptable where a pure `uses {}` parameter is wanted, since it would let
/// `apply_pure` perform an effect it never declared.
#[test]
fn effectful_handler_rejected_where_pure_expected() {
    let output = check_single(
        "import std.io\n\
         fn apply_pure(f: (Int) -> Int uses {}, x: Int) -> Int uses {} { f(x) }\n\
         fn io_inc(x: Int) -> Int uses { Io } {\n\
             let p = Io.print(\"x\\n\")\n\
             x + 1\n\
         }\n\
         fn main() -> Unit uses { Io } {\n\
             let r = apply_pure(io_inc, 5)\n\
             ()\n\
         }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Type mismatch"),
        "unexpected diagnostic:\n{err}"
    );
}
