//! Function pruning by reachability end to end (E3): with `LirRules::prune` on, native C and
//! WASM remove the functions no reachable code can name, keep the index (and a stub body) of a
//! handler operation that a reachable handler record names but no code performs, and still produce
//! the same stdout, stderr and exit code as the default build. The flag is off by default, so the
//! default output and the WASM byte pins are unchanged.
//!
//! The program uses `SystemConsole.print_line` and `Ping.ping`; `SystemConsole.print_error_line`
//! and `Ping.pong` are built by their handlers but never performed. The pruned C source contains
//! neither the host of the unused console operation nor the literal of the unused `pong` body, and
//! the referenced closure functions remain as stubs.
//!
//! Needs a C compiler (`EMELA_CC` or `cc`) and wasmtime (`EMELA_WASMTIME` or `wasmtime`); a test is
//! skipped with a message when its toolchain is missing.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreWasm, Input, Lower, OsFs, Output, RunOptions, RunOutput, Target,
    cc_command, run, wasmtime_program,
};
use emela_lir::opt::{LirRules, Options};

/// A two-operation effect and a `SystemConsole` handler, with one operation of each unused.
const PROGRAM: &str = "\
import Std.Console.{Console, SystemConsole}

effect Ping {
  fn ping() -> Int
  fn pong() -> Int
}

handler Pinging: Ping {
  fn ping() { 1 }
  fn pong() { 1234567 }
}

fn main() {
  with SystemConsole {
    console = use Console
    console.print_line(\"start\")
    with Pinging {
      n = (use Ping).ping()
      console.print_line(\"#{n}\")
    }
  }
}
";

/// The default LIR options with reachability pruning on or explicitly off.
fn options(prune: bool) -> Options {
    Options {
        rules: LirRules {
            prune,
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

fn source(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-prune-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.emel");
    std::fs::write(&path, PROGRAM).unwrap();
    path
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("emela-prune-out-{}-{name}", std::process::id()))
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
            .map(|d| format!("{:?} {}", d.code, d.message))
            .collect()),
    }
}

fn run_native(path: &Path, options: Options, optimize: bool, name: &str) -> (Outcome, String) {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    let mut backend = CoreC::with_cc(cc, &flags)
        .with_lir_options(options)
        .with_optimize(optimize);
    let input = Input::resolve(&OsFs, path).unwrap();
    let dir = out_dir(name);
    let run_options = RunOptions {
        out_dir: dir.clone(),
        js_host: wasmtime_program(),
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

fn run_wasm(path: &Path, options: Options, optimize: bool, name: &str) -> (Outcome, Vec<u8>) {
    let mut backend = CoreWasm::default()
        .with_lir_options(options)
        .with_optimize(optimize);
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

/// The pruned C source drops the host only the unperformed console operation would call and the
/// literal of the unperformed `pong` body; the default source has both. The program's output is
/// the same either way.
#[test]
fn pruning_keeps_native_behaviour_and_drops_the_unused_bodies() {
    if !have_cc() {
        return;
    }
    let path = source("native");
    let (default, default_c) = run_native(&path, options(false), true, "native-default");
    let (pruned, pruned_c) = run_native(&path, options(true), true, "native-pruned");

    assert_eq!(default, pruned, "stdout, stderr and exit code");
    assert_eq!(pruned.0, 0, "{}", pruned.2);
    assert_eq!(pruned.1, "start\n1\n");
    assert!(
        default_c.contains("emela_console_print_error_line"),
        "the default build emits the unused console operation's host call"
    );
    assert!(
        default_c.contains("1234567"),
        "the default build emits the unused `pong` body"
    );
    assert!(
        !pruned_c.contains("emela_console_print_error_line"),
        "the pruned build removes the unreachable host"
    );
    assert!(
        !pruned_c.contains("1234567"),
        "the pruned build stubs the unperformed operation body"
    );

    // `--no-opt`: with the LIR optimisation passes off, pruning is the only difference and it
    // still changes nothing the program observes.
    let (no_opt, _) = run_native(&path, options(false), false, "native-no-opt");
    let (no_opt_pruned, _) = run_native(&path, options(true), false, "native-no-opt-pruned");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(no_opt, no_opt_pruned, "--no-opt with pruning");
}

/// The pruned WASM component runs the same and is smaller: the unreachable host function's index
/// (and its body) is gone, while the referenced operation functions keep their index and a stub.
#[test]
fn pruning_keeps_wasm_behaviour() {
    if !have_wasmtime() {
        return;
    }
    let path = source("wasm");
    let (default, default_wasm) = run_wasm(&path, options(false), true, "wasm-default");
    let (pruned, pruned_wasm) = run_wasm(&path, options(true), true, "wasm-pruned");

    assert_eq!(default, pruned, "stdout, stderr and exit code");
    assert_eq!(pruned.0, 0, "{}", pruned.2);
    assert_eq!(pruned.1, "start\n1\n");
    assert!(
        pruned_wasm.len() < default_wasm.len(),
        "pruning removes an unreachable function: {} vs {} bytes",
        pruned_wasm.len(),
        default_wasm.len()
    );

    let (no_opt, _) = run_wasm(&path, options(false), false, "wasm-no-opt");
    let (no_opt_pruned, _) = run_wasm(&path, options(true), false, "wasm-no-opt-pruned");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    assert_eq!(no_opt, no_opt_pruned, "--no-opt with pruning");
}
