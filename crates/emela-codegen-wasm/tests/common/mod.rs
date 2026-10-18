//! Compiles Emela sources to WASM and validates the module; runs it with `wasmtime` when present.

#![allow(dead_code)]

#[path = "../../../emela-core/tests/support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use emela_codegen_wasm::{Entry, WasmError, WasmOutput, compile};
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

/// Compiles `src`, validating the module with `wasmparser`.
pub fn compile_checked(src: &str) -> Vec<u8> {
    let bytes = to_wasm(src).unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    bytes
}

/// The `wasmtime` binary (`EMELA_WASMTIME` or `wasmtime`), or `None` if it cannot be started.
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
    let wt = wasmtime().expect("checked by the caller");
    std::fs::write(dir.join("main.wasm"), bytes).unwrap();
    let mut command = Command::new(wt);
    command.arg("run").arg("main.wasm").current_dir(dir);
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
