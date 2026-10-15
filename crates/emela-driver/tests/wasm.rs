//! The WASM target (spec chapter 16) through the `emela build` and `run` paths: the module is run
//! with `wasmtime` (`EMELA_WASMTIME` or `wasmtime`). Tests that need wasmtime are skipped, with a
//! message, when it cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, RunOutput, Target,
    WASM_ENTRY_FILE, build, run, wasmtime_program,
};

/// Whether wasmtime starts; prints why a test is skipped otherwise.
fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start `{}`", wasmtime.display());
    }
    ok
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-wasm-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project(main: &str) -> (MemoryFiles, Input) {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    (fs, input)
}

/// Builds and runs `main` on the WASM target.
fn run_wasm(name: &str, main: &str) -> (Analysis, Option<RunOutput>) {
    let (fs, input) = project(main);
    let options = RunOptions {
        out_dir: out_dir(name),
        node: wasmtime_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let result = run(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &options,
        |_| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn builds_the_module() {
    let (fs, input) = project("fn main() { dbg(1) }\n");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Wasm),
        &mut CoreWasm,
        &out_dir("build"),
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let entry = entry.unwrap();
    assert!(entry.ends_with(WASM_ENTRY_FILE));
    assert!(std::fs::metadata(&entry).unwrap().len() > 8);
    // The WASM magic.
    let bytes = std::fs::read(&entry).unwrap();
    assert_eq!(&bytes[..4], b"\0asm");
    let _ = std::fs::remove_dir_all(entry.parent().unwrap());
}

#[test]
fn runs_under_wasmtime() {
    if !have_wasmtime() {
        return;
    }
    let (analysis, output) = run_wasm(
        "run",
        "fn main() {\n  dbg(2 + 3)\n  dbg(\"hi\")\n}\n",
    );
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(text(&output.stdout), "");
    assert_eq!(text(&output.stderr), "5\n\"hi\"\n");
}

#[test]
fn unsupported_features_are_diagnostics() {
    let (analysis, output) = run_wasm("unsupported", "fn main() { dbg(1.5) }\n");
    assert!(analysis.has_errors());
    assert!(output.is_none());
    let messages: Vec<&str> = analysis
        .diagnostics
        .iter()
        .map(|d| d.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Float")),
        "{messages:?}"
    );
}
