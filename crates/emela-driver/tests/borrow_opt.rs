//! Borrow inference end to end (C2b/PR-026): with `LirRules::borrow` on, native C and WASM build
//! and run the same program with the same observable result as the default build, and the flag
//! reaches the backends (the emitted code differs).
//!
//! The native build compiles the C runtime with its leak check (`-DEMELA_LEAK_CHECK`), so a leaked
//! or double-counted reference is an observable failure (exit code 99) rather than a silent
//! difference. The program calls a read-only function twice with the same heap list, so the
//! default build `dup`s the argument before the first call and the borrow build does not. The Core
//! IR optimisations are off in these tests: otherwise the δ reduction may inline the read-only
//! function before the LIR stage, and there is no call left for the flag to change. The LIR
//! switch under test is the one in `LirRules::borrow`.
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

/// A read-only function over a heap list, called twice with the same argument. `first` only reads
/// its parameter (`ListIsEmpty`, `ListHead`), so borrow inference marks it borrowed; the list is a
/// reference on both backends, so the call-site `dup` is visible in the emitted code.
const PROGRAM: &str = "\
fn first(xs: List[Int]) -> Int {
  xs match {
    [] -> -1
    [x, ..] -> x
  }
}

fn main() {
  xs = [1, 2, 3]
  a = first(xs)
  b = first(xs)
  dbg(a + b)
}
";

/// A program the C2 rule treats differently in each function: `insert` stores every view of the
/// node and `apply` only calls its parameter, so both keep the owned convention; `depth` and
/// `check` read views, so they stay borrowed. `first` is passed as a value and is owned.
const NARROWED: &str = "\
enum Tree {
  Leaf
  Node(left: Tree, value: Int, right: Tree)
}

fn insert(t: Tree, x: Int) -> Tree {
  t match {
    Leaf -> Node(left: Leaf, value: x, right: Leaf)
    Node(left:, value: here, right:) -> if x < here {
      Node(left: insert(left, x), value: here, right:)
    } else {
      Node(left:, value: here, right: insert(right, x))
    }
  }
}

fn depth(t: Tree) -> Int {
  t match {
    Leaf -> 0
    Node(left:, right:, ..) -> 1 + depth(left) + depth(right)
  }
}

fn check(xs: List[Int]) -> Int {
  xs match {
    [] -> 0
    [x, ..rest] -> x + check(rest)
  }
}

fn first(xs: List[Int]) -> Int {
  xs match {
    [] -> -1
    [x, ..] -> x
  }
}

fn apply(f: fn(List[Int]) -> Int, xs: List[Int]) -> Int { f(xs) }

fn main() {
  xs = [1, 2, 3]
  t = Node(left: Leaf, value: 2, right: Leaf)
  u = insert(t, 1)
  v = insert(u, 3)
  dbg(check(xs) + depth(v) + apply(first, xs))
}
";

/// The default LIR options with borrow inference off (the owned convention).
fn owned_options() -> Options {
    Options {
        rules: LirRules {
            borrow: false,
            ..LirRules::default()
        },
    }
}

/// The default LIR options with borrow inference on.
fn borrow_options() -> Options {
    Options {
        rules: LirRules {
            borrow: true,
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
    source_with(name, PROGRAM)
}

/// Writes `program` to a fresh directory and returns the entry file.
fn source_with(name: &str, program: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-borrow-opt-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("main.emel");
    std::fs::write(&path, program).unwrap();
    path
}

fn out_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "emela-borrow-opt-out-{}-{name}",
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

/// The borrow build keeps stdout, stderr and the exit code, does not leak (the C runtime's leak
/// check exits 99), and its emitted C differs from the default build's (the flag reaches the
/// backend and removes the call-site `dup`).
#[test]
fn the_borrow_flag_keeps_native_behaviour_and_leaks_nothing() {
    if !have_cc() {
        return;
    }
    let path = source("native");
    let (default, default_c) = run_native(&path, Options::default(), "native-default");
    let (borrow, borrow_c) = run_native(&path, borrow_options(), "native-borrow");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(
        default.0, 0,
        "the default build leaks or fails: {default:?}"
    );
    assert_eq!(borrow.0, 0, "the borrow build leaks or fails: {borrow:?}");
    assert_eq!(
        default, borrow,
        "the borrow build changed what the program observes"
    );
    assert_ne!(
        default_c, borrow_c,
        "the borrow flag did not reach the C backend"
    );
    // The borrowed call no longer dups the argument, and the callee no longer drops the
    // parameter: both runtime calls are gone from the emitted C.
    let count = |source: &str, call: &str| source.matches(call).count();
    let (default_dups, borrow_dups) = (count(&default_c, "em_dup"), count(&borrow_c, "em_dup"));
    let (default_drops, borrow_drops) = (count(&default_c, "em_drop"), count(&borrow_c, "em_drop"));
    assert!(
        borrow_dups < default_dups,
        "the borrow build keeps the caller's dup: default {default_dups}, borrow {borrow_dups}"
    );
    assert!(
        borrow_drops < default_drops,
        "the borrow build keeps the callee's drop: default {default_drops}, borrow {borrow_drops}"
    );
}

/// The narrowed signature (C2) keeps the program's observable result and leaks nothing: the
/// `insert` and `apply` parameters are owned under both conventions, while `check` and `depth`
/// keep their borrow. The native build runs the runtime's leak check, so an unbalanced reference
/// in either shape exits 99.
#[test]
fn the_narrowed_signature_keeps_native_behaviour_and_leaks_nothing() {
    if !have_cc() {
        return;
    }
    let path = source_with("narrowed", NARROWED);
    let (owned, _) = run_native(&path, owned_options(), "narrowed-owned");
    let (borrow, _) = run_native(&path, borrow_options(), "narrowed-borrow");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(owned.0, 0, "the owned build leaks or fails: {owned:?}");
    assert_eq!(borrow.0, 0, "the borrow build leaks or fails: {borrow:?}");
    assert_eq!(
        owned, borrow,
        "the narrowed signature changed what the program observes"
    );
}

/// The same program on WASM: the borrow build keeps the observable result and the emitted
/// component differs from the default build's.
#[test]
fn the_borrow_flag_keeps_wasm_behaviour() {
    if !have_wasmtime() {
        return;
    }
    let path = source("wasm");
    let (default, default_wasm) = run_wasm(&path, Options::default(), "wasm-default");
    let (borrow, borrow_wasm) = run_wasm(&path, borrow_options(), "wasm-borrow");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());

    assert_eq!(default.0, 0, "the default build fails: {default:?}");
    assert_eq!(borrow.0, 0, "the borrow build fails: {borrow:?}");
    assert_eq!(
        default, borrow,
        "the borrow build changed what the program observes"
    );
    assert_ne!(
        default_wasm, borrow_wasm,
        "the borrow flag did not reach the WASM backend"
    );
}
