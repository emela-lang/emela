//! End-to-end tests for `emela run`: build to `wasm-wasi` and execute the
//! module in-process via the embedded wasmi runtime (the `run` feature, on by
//! default). Each test drives the compiled `emela` binary like a user would.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-run-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes `source` to a temp `main.emel` and runs `emela run` on it (no package
/// path). Returns the process output; the caller inspects exit code / streams.
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

/// `main`'s `Int` result becomes the process exit code (spec's `emit_start`).
#[test]
fn int_result_is_the_exit_code() {
    let output = run_source(
        "exit-code",
        "fn add(x: Int, y: Int) -> Int { x + y }\nfn main() -> Int { add(20, 22) }\n",
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(42));
}

/// Closures compile to `call_indirect`; make sure that path executes, not just
/// builds.
#[test]
fn closures_execute() {
    let output = run_source(
        "closures",
        "fn make_adder(n: Int) -> (Int) -> Int {\n  fn (x: Int) -> Int { x + n }\n}\nfn main() -> Int {\n  let add10 = make_adder(10)\n  add10(32)\n}\n",
    );
    assert_eq!(output.status.code(), Some(42));
}

/// `try` / `catch` resolves a thrown error to a value at run time.
#[test]
fn try_catch_executes() {
    let output = run_source(
        "try-catch",
        "enum E {\n  Bad\n}\nfn risky() -> Int throws E { throw E::Bad }\nfn main() -> Int {\n  try {\n    risky()\n  } catch {\n    Bad -> 42\n  }\n}\n",
    );
    assert_eq!(output.status.code(), Some(42));
}

/// A `panic` traps; `run` surfaces it as a runtime error and a non-zero exit,
/// not as a normal exit code.
#[test]
fn panic_is_reported_as_a_trap() {
    let output = run_source(
        "panic",
        "fn boom() -> Int { panic(\"nope\") }\nfn main() -> Int { boom() }\n",
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("wasm runtime error"), "{stderr}");
}

/// A program that does I/O prints to stdout through the WASI `fd_write` shim.
#[test]
fn writes_to_stdout() {
    let dir = temp_dir("stdout");
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("io.emel"),
        "effect io {\nextern fn write_stdout(s: String) -> Unit\npub fn print(s: String) -> Unit { write_stdout(s) }\n}\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    fs::write(
        &app,
        "import std.io\nfn main() -> Unit uses { io } { io.print(\"Hello, Emela!\\n\") }\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "Hello, Emela!\n");
}

/// `run` executes WebAssembly; asking for a non-wasm backend is rejected.
#[test]
fn rejects_non_wasm_backend() {
    let dir = temp_dir("backend");
    let input = dir.join("main.emel");
    fs::write(&input, "fn main() -> Int { 0 }\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg("--backend")
        .arg("js-node")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("executes WebAssembly"), "{stderr}");
}
