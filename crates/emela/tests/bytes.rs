//! End-to-end tests for the `Bytes` type (spec 0051): construction from a
//! `String` (UTF-8 encode via `to_bytes`), byte-unit `length` / `byte_at` /
//! `slice`, `++` (Concat impl) and `==` (Eq impl). Built to `wasm-wasi` and run
//! in-process; `main`'s `Int` result is the process exit code, so each test
//! encodes its check as a returned integer.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn run_source(label: &str, source: &str) -> Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-bytes-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input: PathBuf = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("run")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn run_int(label: &str, source: &str) -> i32 {
    let output = run_source(label, source);
    assert!(
        output.stderr.is_empty(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.code().expect("process had no exit code")
}

/// `to_bytes` UTF-8 encodes, and `length` counts *bytes*: "héllo" is 5 scalars
/// but 6 bytes ("é" is two UTF-8 bytes) — unlike `String` length (spec 0051 T2).
#[test]
fn length_counts_bytes_not_scalars() {
    let code = run_int(
        "length",
        "import std.bytes\nfn main() -> Int { bytes.length(bytes.to_bytes(\"héllo\")) }\n",
    );
    assert_eq!(code, 6);
}

/// `byte_at` returns the raw byte value (0–255) at a byte index. "A" is 65.
#[test]
fn byte_at_returns_byte_value() {
    let code = run_int(
        "byte-at",
        "import std.bytes\nfn main() -> Int {\n  match bytes.byte_at(bytes.to_bytes(\"A\"), 0) {\n    Some(v) -> v\n    None -> 0\n  }\n}\n",
    );
    assert_eq!(code, 65);
}

/// `byte_at` past the end is `None`, not a panic (spec 0011).
#[test]
fn byte_at_out_of_range_is_none() {
    let code = run_int(
        "byte-at-oob",
        "import std.bytes\nfn main() -> Int {\n  match bytes.byte_at(bytes.to_bytes(\"A\"), 5) {\n    Some(_v) -> 1\n    None -> 0\n  }\n}\n",
    );
    assert_eq!(code, 0);
}

/// `++` concatenates `Bytes` (the Concat impl, spec 0051 B5); the result length
/// is the sum of the byte lengths.
#[test]
fn concat_operator_joins_bytes() {
    let code = run_int(
        "concat",
        "import std.bytes\nfn main() -> Int {\n  bytes.length(bytes.to_bytes(\"ab\") ++ bytes.to_bytes(\"cde\"))\n}\n",
    );
    assert_eq!(code, 5);
}

/// `slice` cuts a byte-unit half-open range (spec 0051 B4); `[1,4)` of 6 is 3.
#[test]
fn slice_is_byte_unit() {
    let code = run_int(
        "slice",
        "import std.bytes\nfn main() -> Int {\n  bytes.length(bytes.slice(bytes.to_bytes(\"abcdef\"), 1, 4))\n}\n",
    );
    assert_eq!(code, 3);
}

/// The first byte of `slice("abcdef", 1, 4)` is 'b' = 98 — the slice really
/// starts at byte 1.
#[test]
fn slice_starts_at_offset() {
    let code = run_int(
        "slice-offset",
        "import std.bytes\nfn main() -> Int {\n  match bytes.byte_at(bytes.slice(bytes.to_bytes(\"abcdef\"), 1, 4), 0) {\n    Some(v) -> v\n    None -> 0\n  }\n}\n",
    );
    assert_eq!(code, 98);
}

/// `==` (the Eq impl, spec 0051 B9) is byte-wise: equal → 1, differing → 0.
#[test]
fn eq_operator_is_bytewise() {
    let eq = run_int(
        "eq-true",
        "import std.bytes\nfn main() -> Int { if bytes.to_bytes(\"xy\") == bytes.to_bytes(\"xy\") { 1 } else { 0 } }\n",
    );
    assert_eq!(eq, 1);
    let ne = run_int(
        "eq-false",
        "import std.bytes\nfn main() -> Int { if bytes.to_bytes(\"xy\") == bytes.to_bytes(\"xz\") { 1 } else { 0 } }\n",
    );
    assert_eq!(ne, 0);
}
