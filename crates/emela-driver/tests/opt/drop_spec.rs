//! Drop specialisation end to end (C3/PR-027): with `LirRules::drop_spec` on, native C and WASM
//! build and run the same program with the same observable result as the generic-drop build, and
//! the flag reaches the backends (the emitted code differs). The flag is on by default since M3;
//! the baseline here turns it explicitly off.
//!
//! The native build compiles the C runtime with its leak check (`-DEMELA_LEAK_CHECK`), so a block
//! released without walking a reference would show up as a leak (exit code 99) or a double
//! release. The program drops a String, an array of scalars, an all-scalar record, an all-scalar
//! enum and a list (which stays generic). The Core IR optimisations are off in these tests, so the
//! values are not folded away before the LIR stage and the drop sites exist.
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

/// A program with a drop of each kind the classification distinguishes: two block drops (a
/// String and an array of Int) and every other value used, so reference counting has drop sites.
const PROGRAM: &str = "\
import Std.Array.{Array}

type Point(x: Int, y: Bool)

enum Tag { Small(n: Int), Large(n: Int64) }

fn main() {
  s = \"hello\"
  a = Array.from_list([1, 2, 3])
  p = Point(x: 1, y: True)
  t = Small(n: 2)
  l = [1, 2, 3]
  u = Large(n: Int64.from_int(3))
  n = t match {
    Small(n:) -> n
    Large(n:) -> Int64.to_int(n)
  }
  m = u match {
    Small(n:) -> n
    Large(n:) -> Int64.to_int(n)
  }
  dbg(s)
  dbg(Array.size(a))
  dbg(p.x)
  dbg(n + m + List.length(l))
  ()
}
";

/// The default LIR options with drop specialisation on.
fn drop_options() -> Options {
    Options {
        rules: LirRules {
            drop_spec: true,
            ..LirRules::default()
        },
    }
}

/// The default LIR options with drop specialisation explicitly off (every drop generic).
fn generic_drop_options() -> Options {
    Options {
        rules: LirRules {
            drop_spec: false,
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
    let dir = std::env::temp_dir().join(format!("emela-drop-spec-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.emel");
    std::fs::write(&path, PROGRAM).unwrap();
    path
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("emela-drop-spec-out-{}-{name}", std::process::id()))
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

/// Builds and runs the program on WASM, and returns its outcome and the emitted component (so a
/// test can see that the flag reached the backend).
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
    let wasm = std::fs::read(dir.join("main.wasm"))
        .unwrap_or_else(|e| panic!("the emitted component is missing: {e}"));
    let _ = std::fs::remove_dir_all(&dir);
    let outcome = outcome(&analysis, output)
        .unwrap_or_else(|errors| panic!("the WASM build failed:\n{}", errors.join("\n")));
    (outcome, wasm)
}

/// The block drop keeps stdout, stderr and the exit code, does not leak (the C runtime's leak
/// check exits 99), and only the specialising build calls `em_drop_block`.
#[test]
fn the_drop_flag_keeps_native_behaviour_and_leaks_nothing() {
    if !have_cc() {
        return;
    }
    let path = source("native");
    let (default, default_c) = run_native(&path, generic_drop_options(), "native-generic");
    let (specialised, specialised_c) = run_native(&path, drop_options(), "native-specialised");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(
        default.0, 0,
        "the generic-drop build leaks or fails: {default:?}"
    );
    assert_eq!(
        specialised.0, 0,
        "the specialising build leaks or fails: {specialised:?}"
    );
    assert_eq!(
        default, specialised,
        "the block drop changed what the program observes"
    );
    assert!(
        !default_c.contains("em_drop_block"),
        "the generic-drop build emits a block drop"
    );
    assert!(
        specialised_c.contains("em_drop_block"),
        "the flag did not reach the C backend"
    );
    // The String and the array drops are block drops; the list keeps the generic one, so both
    // calls are still there.
    assert!(
        specialised_c.contains("em_drop("),
        "the generic drop disappeared from the specialising build"
    );
}

/// The same program on WASM: the block drop keeps the observable result and the emitted component
/// differs from the generic-drop build's. The default bytes are pinned by `bytes_stable`, which
/// the M3 defaults move; the specialising build's bytes differ because it gains the `drop_block`
/// runtime function and calls it.
#[test]
fn the_drop_flag_keeps_wasm_behaviour() {
    if !have_wasmtime() {
        return;
    }
    let path = source("wasm");
    let (default, default_wasm) = run_wasm(&path, generic_drop_options(), "wasm-generic");
    let (specialised, specialised_wasm) = run_wasm(&path, drop_options(), "wasm-specialised");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(default.0, 0, "the default build fails: {default:?}");
    assert_eq!(
        specialised.0, 0,
        "the specialising build fails: {specialised:?}"
    );
    assert_eq!(
        default, specialised,
        "the block drop changed what the program observes"
    );
    assert_ne!(
        default_wasm, specialised_wasm,
        "the drop flag did not reach the WASM backend"
    );
}
