//! End-to-end tests for the `Random` capability and the seedable PRNG
//! (spec 0054). The PRNG (Part B) is pure and deterministic, so its sequence is
//! asserted exactly and checked to agree across backends. The `Random` effect
//! (Part A) is non-deterministic, so it is checked structurally — the wasip2
//! lowering imports `wasi:random`, and `Random.bytes(n)` yields `n` bytes under
//! `emela run`.

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

#[test]
fn prng_is_deterministic_and_seed_sensitive() {
    // Part B (spec 0054 Q1): the same seed yields the same sequence; different
    // seeds (almost surely) differ. Pure, so no capability is used.
    let third = "{\n\
        let a = random.next(random.seed(SEED))\n\
        let b = random.next(a.next)\n\
        random.next(b.next).value\n\
    }";
    let with_seed = |s: &str| print_prog("Io", &third.replace("SEED", s));
    let a = run_stdout(&with_seed("42"));
    let b = run_stdout(&with_seed("42"));
    let c = run_stdout(&with_seed("43"));
    assert_eq!(a, b, "same seed must reproduce the sequence");
    assert_ne!(a, c, "different seeds must (almost surely) differ");
}

#[test]
fn prng_agrees_across_backends() {
    // The PRNG is pure Emela, so wasm-wasi and js-node produce the same value
    // (spec 0054 Q1, spec 0000).
    let program = print_prog("Io", "random.next(random.seed(12345)).value");
    let wasm = run_stdout(&program);

    let dir = temp_dir();
    let input = dir.join("main.emel");
    let js_path = dir.join("out.js");
    fs::write(&input, &program).unwrap();
    let build = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(["build", "--backend", "js-node", "-o"])
        .arg(&js_path)
        .arg(&input)
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let node = Command::new("node").arg(&js_path).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    let js = String::from_utf8(node.stdout).unwrap().trim().to_owned();
    assert_eq!(wasm, js, "PRNG must agree across backends");
}

#[test]
fn prng_seed_zero_is_remapped() {
    // Part B (spec 0054 Q2): a zero seed cannot leave xorshift state 0, so it is
    // remapped; `next` on `seed(0)` produces a non-zero draw.
    let program = print_prog("Io", "random.next(random.seed(0)).value");
    assert_ne!(
        run_stdout(&program),
        "0",
        "seed(0) must not yield a zero draw"
    );
}
