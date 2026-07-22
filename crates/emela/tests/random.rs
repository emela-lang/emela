//! End-to-end tests for the `Random` capability (spec 0054 Part A). The effect
//! is non-deterministic, so it is checked structurally — the wasip2 lowering
//! imports `wasi:random`, and `Random.bytes(n)` yields `n` bytes under
//! `emela run` (the host-backed parity path).

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir() -> std::path::PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-rand-test-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], source: &str) -> std::process::Output {
    let dir = temp_dir();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
    for arg in args {
        command.arg(arg);
    }
    let output = command.arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

/// Runs a `main` printing `expr` under `emela run` and returns trimmed stdout.
fn run_stdout(program: &str) -> String {
    let output = run(&["run"], program);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// The core-module WAT the `wasm-wasip2` backend emits (spec 0052), used to check
/// which WASI 0.2 interfaces the program lowers to.
fn wasip2_wat(program: &str) -> String {
    let output = run(
        &["build", "--backend", "wasm-wasip2", "--emit", "text"],
        program,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// A `main` that prints one `Int` expression (needs `Io`, plus `uses` for `expr`).
fn print_prog(uses: &str, expr: &str) -> String {
    format!(
        "import std.random\nimport std.io\n\
         fn main() -> Unit uses {{ {uses} }} {{\n    Io.print({expr})\n}}\n"
    )
}

#[test]
fn random_bytes_has_requested_length() {
    // Spec 0054 P4: `Random.bytes(n)` returns `n` bytes. The length is
    // deterministic even though the bytes are not.
    let program = "import std.random\nimport std.io\nimport std.bytes\n\
        fn main() -> Unit uses { Io, Random } {\n\
        \x20   Io.print(bytes.length(Random.bytes(24)))\n\
        }\n";
    assert_eq!(run_stdout(program), "24");
}

#[test]
fn random_int_runs_and_prints() {
    // Spec 0054 P3 smoke test: `Random.int()` executes under `emela run` and
    // prints an integer (the value itself is non-deterministic).
    let program = print_prog("Io, Random", "Random.int()");
    let out = run_stdout(&program);
    assert!(
        out.parse::<i64>().is_ok(),
        "expected an integer, got {out:?}"
    );
}

#[test]
fn random_effect_lowers_to_wasi_random_on_wasip2() {
    // Spec 0054 Compilation Notes: `Random.*` lowers to `wasi:random` on the
    // component backend, and to nothing else WASI-wise beyond `wasi:cli`.
    let program = "import std.random\n\
        fn main() -> Int uses { Random } { Random.int() & 255 }\n";
    let wat = wasip2_wat(program);
    assert!(
        wat.contains("wasi:random/random@0.2.0"),
        "Random.int() should import wasi:random:\n{wat}"
    );
    assert!(
        !wat.contains("wasi:sockets"),
        "a Random-only program must not import wasi:sockets:\n{wat}"
    );
}
