//! Scalar replacement of records end to end (C5/PR-043): with `LirRules::sra_records` on, native C
//! and WASM build and run the same program with the same observable result as the build with the
//! flag off, and the WASM component differs, which shows the pass reached the emitter.
//!
//! The program builds a non-escaping record of scalar fields in a tail-recursive function, reads
//! it through its fields and through a tag switch on its only variant, and adds the results to the
//! accumulator. `crates/emela-lir/tests/sra_records.rs` covers the rule itself; here it runs
//! through the driver and the backends.
//!
//! The native build compiles the runtime with its leak check (`-DEMELA_LEAK_CHECK`), so a block
//! kept alive after the pass removed its construction shows up as an observable failure. The WASM
//! comparison uses the boxed representation ([`WasmOptions::boxed`]): the default by-value
//! representation already holds an eligible record without an allocation, so only the boxed one
//! shows the pass in the emitted bytes.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); a test is
//! skipped with a message when its toolchain is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_codegen_wasm::WasmOptions;
use emela_driver::{
    Analysis, CoreC, CoreWasm, Input, Lower, OsFs, Output, RunOptions, RunOutput, Target,
    cc_command, js_host_program, run, wasmtime_program,
};
use emela_lir::opt::{LirRules, Options};

/// A tail-recursive function that builds a non-escaping record of scalars, reads it through its
/// fields and through a tag switch on its only variant, and never lets it escape.
const PROGRAM: &str = "\
type Point(x: Int, y: Int)

fn go(i: Int, acc: Int) -> Int {
  if 1000 <= i {
    acc
  } else {
    p = Point(x: i, y: i + 1)
    n = p match {
      Point(x:, y:) -> x * 2 + y
    }
    go(i + 1, acc + p.x + n)
  }
}

fn main() {
  dbg(go(0, 0))
  ()
}
";

/// The default LIR options with record scalar replacement on.
fn sra_options() -> Options {
    Options {
        rules: LirRules {
            sra_records: true,
            ..LirRules::default()
        },
    }
}

/// The default LIR options with record scalar replacement explicitly off.
fn no_sra_options() -> Options {
    Options {
        rules: LirRules {
            sra_records: false,
            ..LirRules::default()
        },
    }
}

fn have_cc() -> bool {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!(
            "skipping: cannot start the C compiler `{}` (set EMELA_CC)",
            cc.display()
        );
    }
    ok
}

fn have_wasmtime() -> bool {
    let wasmtime = wasmtime_program();
    let ok = Command::new(&wasmtime)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start wasmtime `{}`", wasmtime.display());
    }
    ok
}

/// Writes [`PROGRAM`] to a fresh directory and returns the entry file. The caller removes the
/// directory when it is done (the test runs both builds from it). `name` keeps the two tests from
/// sharing a directory.
fn source(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-record-sra-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.emel");
    std::fs::write(&path, PROGRAM).unwrap();
    path
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "emela-record-sra-out-{}-{name}",
        std::process::id()
    ))
}

type Outcome = (i32, String, String);

/// The observable result of a build and run, or the diagnostics that stopped it.
fn outcome(analysis: &Analysis, output: Option<RunOutput>) -> Result<Outcome, Vec<String>> {
    match output {
        Some(RunOutput {
            code,
            stdout,
            stderr,
        }) if !analysis.has_errors() => Ok((
            code,
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )),
        _ => Err(analysis
            .diagnostics
            .iter()
            .map(|d| format!("{:?} {}", d.code, d.message))
            .collect()),
    }
}

/// Builds and runs the program on native C with the runtime's leak check, and returns its outcome
/// and the emitted C source (so a test can see that the flag reached the backend).
fn run_native(path: &Path, options: Options, name: &str) -> (Outcome, String) {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let mut backend = CoreC::with_cc(cc, &flags).with_lir_options(options);
    let input = Input::resolve(&OsFs, path).unwrap();
    let dir = out_dir(name);
    let run_options = RunOptions {
        out_dir: dir.clone(),
        js_host: js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default()
        .with_target(Target::Native)
        .with_optimize(false);
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut frontend,
        &mut backend,
        &run_options,
        |_, _| {},
    );
    let source = std::fs::read_to_string(dir.join("main.c"))
        .unwrap_or_else(|e| panic!("the emitted C source is missing: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    let outcome = outcome(&analysis, output)
        .unwrap_or_else(|errors| panic!("the native build failed:\n{}", errors.join("\n")));
    (outcome, source)
}

/// Builds and runs the program on WASM with the boxed representation, and returns its outcome and
/// the emitted component (so a test can see that the flag reached the backend).
fn run_wasm(path: &Path, options: Options, name: &str) -> (Outcome, Vec<u8>) {
    let mut backend = CoreWasm::default()
        .with_wasm_options(WasmOptions::boxed())
        .with_lir_options(options);
    let input = Input::resolve(&OsFs, path).unwrap();
    let dir = out_dir(name);
    let run_options = RunOptions {
        out_dir: dir.clone(),
        js_host: wasmtime_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default()
        .with_target(Target::Wasm)
        .with_optimize(false);
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut frontend,
        &mut backend,
        &run_options,
        |_, _| {},
    );
    let wasm = std::fs::read(dir.join("main.wasm"))
        .unwrap_or_else(|e| panic!("the emitted component is missing: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    let outcome = outcome(&analysis, output)
        .unwrap_or_else(|errors| panic!("the WASM build failed:\n{}", errors.join("\n")));
    (outcome, wasm)
}

/// The record SRA keeps stdout, stderr and the exit code, and does not leak (the C runtime's leak
/// check exits 99). The C backend already holds an all-scalar record by value, so the emitted
/// source need not change; the WASM test below is the one that sees the pass in the bytes.
#[test]
fn record_sra_keeps_native_behaviour() {
    if !have_cc() {
        return;
    }
    let path = source("native");
    let (off, _off_c) = run_native(&path, no_sra_options(), "native-off");
    let (on, _on_c) = run_native(&path, sra_options(), "native-on");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(off.0, 0, "the build without record SRA fails: {off:?}");
    assert_eq!(on.0, 0, "the build with record SRA fails: {on:?}");
    assert_eq!(off, on, "record SRA changed what the program observes");
    assert!(
        !on.2.contains("leak check"),
        "the build with record SRA leaks:\n{}",
        on.2
    );
}

/// The same program on WASM with the boxed representation: record SRA keeps the observable result
/// and changes the component, which proves the pass reached the emitter.
#[test]
fn record_sra_keeps_wasm_behaviour() {
    if !have_wasmtime() {
        return;
    }
    let path = source("wasm");
    let (off, off_wasm) = run_wasm(&path, no_sra_options(), "wasm-off");
    let (on, on_wasm) = run_wasm(&path, sra_options(), "wasm-on");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(off.0, 0, "the build without record SRA fails: {off:?}");
    assert_eq!(on.0, 0, "the build with record SRA fails: {on:?}");
    assert_eq!(off, on, "record SRA changed what the program observes");
    assert_ne!(
        off_wasm, on_wasm,
        "record SRA did not reach the WASM backend"
    );
}
