//! Closure scalar replacement end to end (C5/PR-043, closure half): with `LirRules::devirt` and
//! `LirRules::sra_closures` on, native C and WASM build and run the same program with the same
//! observable result as the default build, the C build allocates the closure record no more, and
//! the emitted code differs (the specialisation reached the backends).
//!
//! The program creates and calls a closure over a scalar in every iteration of a loop. Without
//! the flags the call dispatches through a record allocated per iteration; with them E5 makes the
//! call direct and closure SRA removes the record. The native build compiles the runtime with its
//! leak check (`-DEMELA_LEAK_CHECK`), so a block kept alive or released twice shows up as an
//! observable failure, and with the allocation report (`-DEMELA_ALLOC_STATS`), so the number of
//! allocations is measured, not guessed.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); a test is
//! skipped with a message when its toolchain is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreWasm, Input, Lower, OsFs, Output, RunOptions, RunOutput, Target,
    cc_command, js_host_program, run, wasmtime_program,
};
use emela_lir::opt::{LirRules, Options};

/// A loop that makes a closure over a scalar and calls it once per iteration. The recursive call
/// is lowered to a loop, so the record is made and used in the same sequence on every iteration.
const PROGRAM: &str = "\
fn step(i: Int, limit: Int, total: Int) -> Int {
  if limit <= i {
    total
  } else {
    base = i * 2
    f = |x: Int| x + base + i
    step(i + 1, limit, total + f(1))
  }
}

fn main() {
  dbg(step(0, 1000, 0))
  ()
}
";

/// The default LIR options with E5 and closure SRA on.
fn closure_sra_options() -> Options {
    Options {
        rules: LirRules {
            devirt: true,
            sra_closures: true,
            ..LirRules::default()
        },
    }
}

/// The default LIR options with both flags explicitly off.
fn no_closure_sra_options() -> Options {
    Options {
        rules: LirRules {
            devirt: false,
            sra_closures: false,
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
        eprintln!(
            "skipping: cannot start wasmtime `{}` (set EMELA_WASMTIME)",
            wasmtime.display()
        );
    }
    ok
}

/// Writes [`PROGRAM`] to a fresh directory and returns the entry file.
fn source(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-closure-sra-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.emel");
    std::fs::write(&path, PROGRAM).unwrap();
    path
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "emela-closure-sra-out-{}-{name}",
        std::process::id()
    ))
}

type Outcome = (i32, String, String);

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
            .map(|d| format!("{:?}", d))
            .collect()),
    }
}

/// Builds and runs the program on native C with the leak check and the allocation report, and
/// returns its outcome and the emitted C source.
fn run_native(path: &Path, options: Options, name: &str) -> (Outcome, String) {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    flags.push("-DEMELA_ALLOC_STATS".to_owned());
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
    let outcome = outcome(&analysis, output)
        .unwrap_or_else(|errors| panic!("the native build failed:\n{}", errors.join("\n")));
    let source = std::fs::read_to_string(dir.join("main.c"))
        .unwrap_or_else(|e| panic!("the emitted C source is missing: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    (outcome, source)
}

/// Builds and runs the program on WASM, and returns its outcome and the emitted component.
fn run_wasm(path: &Path, options: Options, name: &str) -> (Outcome, Vec<u8>) {
    let mut backend = CoreWasm::default().with_lir_options(options);
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
    let outcome = outcome(&analysis, output)
        .unwrap_or_else(|errors| panic!("the WASM build failed:\n{}", errors.join("\n")));
    let wasm = std::fs::read(dir.join("main.wasm"))
        .unwrap_or_else(|e| panic!("the emitted component is missing: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    (outcome, wasm)
}

/// The total the allocation report printed on stderr (`emela-alloc-total N`).
fn allocations(stderr: &str) -> u64 {
    stderr
        .lines()
        .find_map(|line| line.strip_prefix("emela-alloc-total "))
        .unwrap_or_else(|| panic!("no allocation report in stderr:\n{stderr}"))
        .parse()
        .expect("the allocation total is a number")
}

/// The calls of `em_alloc` in the emitted C source.
fn alloc_sites(source: &str) -> usize {
    source.matches("em_alloc(").count()
}

/// Closure SRA keeps stdout, stderr and the exit code, leaks nothing, removes one allocation site
/// from the C source and allocates one closure record less per iteration.
#[test]
fn closure_sra_keeps_native_behaviour_and_allocates_less() {
    if !have_cc() {
        return;
    }
    let path = source("native-c");
    let (off, off_c) = run_native(&path, no_closure_sra_options(), "native-off");
    let (on, on_c) = run_native(&path, closure_sra_options(), "native-on");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(off.0, 0, "the build without the flags fails: {off:?}");
    assert_eq!(on.0, 0, "the build with the flags fails: {on:?}");
    assert_eq!(off.1, on.1, "closure SRA changed stdout");
    // stderr differs only in the allocation report; the leak check (exit 99) and any defect would
    // have failed the run above.
    for (name, out) in [("off", &off), ("on", &on)] {
        assert!(
            !out.2.contains("leak check"),
            "the {name} build leaks:\n{}",
            out.2
        );
    }
    assert_ne!(off_c, on_c, "closure SRA did not reach the C backend");
    assert_eq!(
        alloc_sites(&off_c),
        alloc_sites(&on_c) + 1,
        "closure SRA should remove exactly one `em_alloc` site"
    );
    let off_total = allocations(&off.2);
    let on_total = allocations(&on.2);
    assert_eq!(
        off_total,
        on_total + 1000,
        "closure SRA should save one allocation per iteration (1000)\
         \n off: {}\n  on: {}",
        off.2,
        on.2
    );
}

/// The same program on WASM: closure SRA keeps the observable result and changes the component.
#[test]
fn closure_sra_keeps_wasm_behaviour() {
    if !have_wasmtime() {
        return;
    }
    let path = source("wasm-c");
    let (off, off_wasm) = run_wasm(&path, no_closure_sra_options(), "wasm-off");
    let (on, on_wasm) = run_wasm(&path, closure_sra_options(), "wasm-on");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(off.0, 0, "the build without the flags fails: {off:?}");
    assert_eq!(on.0, 0, "the build with the flags fails: {on:?}");
    assert_eq!(off, on, "closure SRA changed what the program observes");
    assert_ne!(
        off_wasm, on_wasm,
        "closure SRA did not reach the WASM backend"
    );
}
