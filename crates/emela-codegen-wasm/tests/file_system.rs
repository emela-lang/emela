//! The `Std.FileSystem` internal hosts on WASM (spec 15.2): the WASI file system and the opaque
//! `Outcome` handle.
//!
//! The bundled `host_*` functions are `suspend fn`s (spec 12.6), and the task attachment that
//! makes them callable lives in the scheduler/fiber track. So these tests drive the *same*
//! runtime functions through non-suspending `@external(wasm, "emela:fs", ...)` bindings: the
//! `(module, name)` registry, the runtime functions and the `Outcome` handle are the real ones,
//! only the wrapped `fn`s are ordinary (not `suspend`) so no scheduler is needed. The module is
//! validated with `wasmparser`; when `wasmtime` is present it is also run against a fresh
//! temporary directory (the WASI preopen).

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::{Run, compile_checked, wasmtime};

/// The runtime's file-system operations and the `Outcome` accessors, as ordinary externals. The
/// `(module, name)` pairs are exactly the ones `std/file_system.emel` binds to (`emela:fs`).
const PRELUDE: &str = r#"
@external
type Outcome

@external(wasm, "emela:fs", "read_text")
fn raw_read_text(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "write_text")
fn raw_write_text(path: String, text: String) -> Outcome use {}

@external(wasm, "emela:fs", "append_text")
fn raw_append_text(path: String, text: String) -> Outcome use {}

@external(wasm, "emela:fs", "exists")
fn raw_exists(path: String) -> Bool use {}

@external(wasm, "emela:fs", "is_directory")
fn raw_is_directory(path: String) -> Bool use {}

@external(wasm, "emela:fs", "list_directory")
fn raw_list_directory(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "create_directory")
fn raw_create_directory(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "remove")
fn raw_remove(path: String) -> Outcome use {}

@external(wasm, "emela:fs", "outcome_error")
fn outcome_error(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_message")
fn outcome_message(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_text")
fn outcome_text(o: Outcome) -> String use {}

@external(wasm, "emela:fs", "outcome_name_count")
fn outcome_name_count(o: Outcome) -> Int use {}

@external(wasm, "emela:fs", "outcome_name_at")
fn outcome_name_at(o: Outcome, i: Int) -> String use {}

@external(wasm, "emela:fs", "release_outcome")
fn release_outcome(o: Outcome) -> () use {}

fn outcome_code(o: Outcome) -> String {
  code = outcome_error(o)
  release_outcome(o)
  code
}

fn read(path: String) -> String {
  o = raw_read_text(path)
  code = outcome_error(o)
  text = outcome_text(o)
  release_outcome(o)
  if code == "" { text } else { "error:#{code}" }
}

fn names_onto(o: Outcome, i: Int, acc: List[String]) -> List[String] {
  if i < 0 { acc } else { names_onto(o, i - 1, [outcome_name_at(o, i), ..acc]) }
}

fn names(path: String) -> List[String] {
  o = raw_list_directory(path)
  n = outcome_name_count(o)
  ns = names_onto(o, n - 1, [])
  release_outcome(o)
  ns
}
"#;

fn source(body: &str) -> String {
    format!("{PRELUDE}\nfn main() {{\n{body}\n}}\n")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-wasm-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs `bytes` as `main.wasm` in `dir`, preopening it as the WASI current directory.
fn run_directed(dir: &Path, bytes: &[u8]) -> Run {
    let wt = wasmtime().expect("checked by the caller");
    std::fs::write(dir.join("main.wasm"), bytes).unwrap();
    let mut command = Command::new(wt);
    command
        .arg("run")
        .arg("--dir=.")
        .arg("main.wasm")
        .current_dir(dir);
    let ran = common::run_with_timeout(command, Duration::from_secs(60)).expect("timed out");
    Run {
        stdout: String::from_utf8_lossy(&ran.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
        code: ran.status.code().unwrap_or(-1),
    }
}

/// The module must be well formed even without `wasmtime`.
#[test]
fn the_module_is_valid() {
    let src = source("  dbg(raw_exists(\"x\"))");
    let bytes = compile_checked(&src);
    wasmparser::validate(&bytes).unwrap();
}

/// A program that does not use `Std.FileSystem` must not carry the fragment's WASI imports (the
/// fragment is on demand, so the modules of other programs keep their bytes).
#[test]
fn the_fragment_is_emitted_on_demand() {
    let plain = common::to_wasm("fn main() { dbg(1 + 1) }").unwrap();
    let text = String::from_utf8_lossy(&plain);
    assert!(
        !text.contains("path_open") && !text.contains("fd_readdir"),
        "the file-system imports leaked into a program that does not use them"
    );
}

const BODY: &str = r#"
  dbg(raw_exists("d/a.txt"))
  dbg(outcome_code(raw_create_directory("d")))
  dbg(raw_is_directory("d"))
  dbg(outcome_code(raw_write_text("d/a.txt", "hello")))
  dbg(read("d/a.txt"))
  dbg(outcome_code(raw_append_text("d/a.txt", "!")))
  dbg(read("d/a.txt"))
  dbg(outcome_code(raw_write_text("d/b.txt", "b")))
  dbg(names("d"))
  dbg(outcome_code(raw_remove("d/b.txt")))
  dbg(names("d"))
  dbg(outcome_code(raw_read_text("d")))
  dbg(outcome_code(raw_remove("d")))
  dbg(outcome_code(raw_remove("d/a.txt")))
  dbg(outcome_code(raw_remove("d")))
  dbg(raw_exists("d"))
  dbg(outcome_code(raw_read_text("missing.txt")))
"#;

#[test]
fn wasi_file_system_works() {
    if wasmtime().is_none() {
        return;
    }
    let bytes = compile_checked(&source(BODY));
    let dir = temp_dir("run");
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert!(
        got.stderr.contains("FileNotFound"),
        "the missing-file error is not reported:\n{}",
        got.stderr
    );
    // The generated output is compared in full by the driver parity tests once the suspending
    // host is wired; here the run must at least finish cleanly and produce the landmarks.
    for line in [
        "\"hello\"",
        "\"hello!\"",
        "[\"a.txt\", \"b.txt\"]",
        "[\"a.txt\"]",
        "\"IsADirectory\"",
        "\"DirectoryNotEmpty\"",
    ] {
        assert!(
            got.stderr.contains(line),
            "missing {line:?} in:\n{}",
            got.stderr
        );
    }
}

#[test]
fn read_text_rejects_invalid_utf8() {
    if wasmtime().is_none() {
        return;
    }
    let dir = temp_dir("utf8");
    std::fs::write(dir.join("bad.txt"), [0x66, 0xff, 0x6f]).unwrap();
    let bytes = compile_checked(&source("  dbg(outcome_code(raw_read_text(\"bad.txt\")))"));
    let got = run_directed(&dir, &bytes);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, 0, "stderr:\n{}", got.stderr);
    assert!(
        got.stderr.contains("InvalidText"),
        "invalid UTF-8 was not reported:\n{}",
        got.stderr
    );
}
