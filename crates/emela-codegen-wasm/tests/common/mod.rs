//! Compiles Emela sources to WASM and validates the module; runs it with `wasmtime` when present.

#![allow(dead_code)]

#[path = "../../../emela-core/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use emela_codegen_wasm::{Entry, TestEntry, WasmError, WasmOutput, compile};
use emela_core::Roots;

/// Lowers `src` and compiles it to WASM.
pub fn to_wasm(src: &str) -> Result<Vec<u8>, Vec<WasmError>> {
    let m = support::try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    compile(&m, &Entry::Main).map(|o| o.module)
}

/// Lowers `src` and compiles it to WASM, returning the module together with its
/// `Layout` so tests can cross-check the emitted memory layout.
pub fn to_output(src: &str) -> WasmOutput {
    let m = support::try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    compile(&m, &Entry::Main).unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"))
}

/// Lowers `src` with every function as a root and compiles the named `@test` functions with
/// [`Entry::Tests`].
pub fn to_wasm_tests(src: &str, names: &[&str]) -> Result<Vec<u8>, Vec<WasmError>> {
    let m = support::try_lower(&[("main.emel", src)], Roots::All)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    let tests: Vec<emela_codegen_wasm::TestEntry> = names
        .iter()
        .map(|&name| {
            let (id, _) = m
                .functions
                .iter()
                .find(|(_, f)| f.name == name)
                .unwrap_or_else(|| panic!("no test function `{name}`"));
            emela_codegen_wasm::TestEntry {
                name: name.to_owned(),
                function: id,
            }
        })
        .collect();
    compile(&m, &emela_codegen_wasm::Entry::Tests(tests)).map(|o| o.module)
}

/// Compiles `src`, validating the module with `wasmparser`.
pub fn compile_checked(src: &str) -> Vec<u8> {
    let bytes = to_wasm(src).unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    bytes
}

/// Compiles the functions `tests` of `src` as test functions ([`Entry::Tests`]), validating the
/// module with `wasmparser`.
pub fn compile_tests_checked(src: &str, tests: &[&str]) -> Vec<u8> {
    let m = support::try_lower(&[("main.emel", src)], Roots::All)
        .unwrap_or_else(|e| panic!("lowering failed: {e:#?}"));
    let tests = tests
        .iter()
        .map(|&name| TestEntry {
            name: name.to_owned(),
            function: m
                .functions
                .iter()
                .find(|(_, f)| f.name == name)
                .map(|(id, _)| id)
                .unwrap_or_else(|| panic!("no function `{name}`")),
        })
        .collect();
    let bytes = compile(&m, &Entry::Tests(tests))
        .unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"))
        .module;
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    bytes
}

/// The `wasmtime` binary (`EMELA_WASMTIME` or `wasmtime`), or `None` if it cannot be started.
///
/// Locally the run tests are skipped without it. Under CI (`CI` is set) a missing wasmtime is a
/// failure, so the checks of the emitted code, such as the whole `GraphemeBreakTest.txt` in
/// `tests/unicode.rs`, cannot pass without running.
pub fn wasmtime() -> Option<&'static str> {
    static WT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    WT.get_or_init(|| {
        let wt = std::env::var("EMELA_WASMTIME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "wasmtime".to_owned());
        let ok = Command::new(&wt)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            assert!(
                std::env::var_os("CI").is_none(),
                "cannot start `{wt}` under CI (set EMELA_WASMTIME)"
            );
            eprintln!("skipping: cannot start `{wt}` (set EMELA_WASMTIME)");
        }
        ok.then_some(wt)
    })
    .as_deref()
}

pub fn out_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("codegen-wasm")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Writes `bytes` as `main.wasm` under `dir` and runs it with `wasmtime`.
pub fn run_bytes(dir: &Path, bytes: &[u8]) -> Run {
    run_bytes_with_command(dir, bytes, |_| {})
}

/// Like [`run_bytes`], but lets the caller adjust the `wasmtime` command (environment variables,
/// extra arguments) before it starts.
pub fn run_bytes_with_command(dir: &Path, bytes: &[u8], adjust: impl FnOnce(&mut Command)) -> Run {
    let wt = wasmtime().expect("checked by the caller");
    std::fs::write(dir.join("main.wasm"), bytes).unwrap();
    let mut command = Command::new(wt);
    command.arg("run").arg("-S").arg("p3=y");
    adjust(&mut command);
    command.arg("main.wasm").current_dir(dir);
    let ran = run_with_timeout(command, Duration::from_secs(60)).expect("the program timed out");
    Run {
        stdout: String::from_utf8_lossy(&ran.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&ran.stderr).into_owned(),
        code: ran.status.code().unwrap_or(-1),
    }
}

pub fn run(name: &str, src: &str) -> Option<Run> {
    wasmtime()?;
    let bytes = compile_checked(src);
    Some(run_bytes(&out_dir(name), &bytes))
}

pub fn run_with_timeout(mut command: Command, limit: Duration) -> Option<Output> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
