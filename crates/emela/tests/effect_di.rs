//! End-to-end tests for compile-time effect DI (spec 0049), default path: a
//! *derived* effect whose operation has an inline default body over a primitive
//! effect. Using the derived effect (`uses { Log }`) discharges to the
//! primitive leaf, so the program runs through the primitive and the capability
//! manifest lists only the leaf. Built to `wasm-wasi` and run in-process.
//!
//! Named handlers and `with`/`provide` injection (the override path) are a
//! later change; this covers the default (inline) path that unblocks HTTP.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn write_main(source: &str) -> (PathBuf, PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-effect-di-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    (dir, input)
}

fn run_source(source: &str) -> Output {
    let (dir, input) = write_main(source);
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn emit_text(source: &str) -> String {
    let (dir, input) = write_main(source);
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .args(["build", "--emit", "text", "--backend", "wasm"])
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(
        output.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

const LOG_OVER_IO: &str = "\
import std.io

effect Log {
    pub fn info(msg: String) -> Unit uses { Io } { Io.print(msg) }
}

fn greet(name: String) -> Unit uses { Log } {
    Log.info(\"hello, \" ++ name ++ \"\\n\")
}

fn main() -> Unit uses { Log } {
    greet(\"emela\")
}
";

/// A derived effect `Log` with an inline default over `Io` runs *through* the
/// primitive: `main` and `greet` declare `uses { Log }`, yet the program prints
/// via `Io` and exits cleanly (spec 0049 default path).
#[test]
fn derived_effect_runs_through_its_primitive_leaf() {
    let output = run_source(LOG_OVER_IO);
    assert!(
        output.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "unexpected stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello, emela\n");
}

/// The capability manifest (spec 0025) lists the discharged *leaf* only: the
/// derived effect `Log` never appears as a capability; `io` (its inline
/// default's dependency) does. `Log` remaining out of the manifest is spec
/// 0049 D5 — a derived effect is a pure abstraction, not a host permission.
#[test]
fn manifest_shows_leaf_not_the_derived_effect() {
    let wat = emit_text(LOG_OVER_IO);
    let manifest = wat
        .lines()
        .find(|line| line.contains("emela:capabilities"))
        .expect("no capability manifest custom section");
    // The manifest bytes are hex-escaped in the `(@custom ...)` string; decode
    // the printable ASCII so we can assert on `io` / `Log` by name.
    let decoded = decode_wat_bytes(manifest);
    assert!(
        decoded.contains("\"capabilities\":[\"io\"]"),
        "expected the leaf `io` capability, got: {decoded}"
    );
    assert!(
        decoded.contains("io.write_stdout"),
        "expected the leaf platform call, got: {decoded}"
    );
    assert!(
        !decoded.contains("Log"),
        "derived effect `Log` must not appear in the manifest, got: {decoded}"
    );
}

/// Decodes the `\NN`-hex-escaped bytes inside a WAT `(@custom ... "...")` line
/// to a `String`, dropping the WAT structure around the quoted payload.
fn decode_wat_bytes(line: &str) -> String {
    // The payload is the quoted string after `(after last) `, up to the final
    // quote on the line.
    let marker = "(after last) \"";
    let (Some(begin), Some(end)) = (line.find(marker).map(|i| i + marker.len()), line.rfind('"'))
    else {
        return String::new();
    };
    if end <= begin {
        return String::new();
    }
    let mut out = Vec::new();
    let mut chars = line[begin..end].chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c as u8);
            continue;
        }
        let (Some(hi), Some(lo)) = (chars.next(), chars.next()) else {
            continue;
        };
        if let Ok(byte) = u8::from_str_radix(&format!("{hi}{lo}"), 16) {
            out.push(byte);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
