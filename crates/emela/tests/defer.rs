//! Tests for `defer` — deterministic resource cleanup (spec 0056).
//!
//! The action must run exactly once on each of the three exits from its scope
//! (D4): the value path, the error channel, and a self tail call. The last one
//! is where the backends disagree internally — wasm jumps past the value path
//! while JavaScript returns a trampoline marker *through* it — so every runtime
//! test here runs on both and asserts they agree (spec 0052 parity).

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-defer-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn emela() -> Command {
    Command::new(env!("CARGO_BIN_EXE_emela"))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Runs `emela check` against a single self-contained file.
fn check_single(source: &str) -> Output {
    let dir = temp_dir("check");
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = emela().arg("check").arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

/// `emela run` (wasm-wasi under the embedded runtime) — stdout.
fn run_wasm(label: &str, source: &str) -> String {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = emela().arg("run").arg(&input).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(output.status.success(), "{}", stderr(&output));
    String::from_utf8(output.stdout).unwrap()
}

/// The same source through the js-node backend, executed with `node` — stdout.
fn run_node(label: &str, source: &str) -> String {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    let js = dir.join("out.js");
    fs::write(&input, source).unwrap();
    let build = emela()
        .args(["build", "--backend", "js-node", "-o"])
        .arg(&js)
        .arg(&input)
        .output()
        .unwrap();
    assert!(build.status.success(), "{}", stderr(&build));
    let node = Command::new("node").arg(&js).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(node.status.success(), "{}", stderr(&node));
    String::from_utf8(node.stdout).unwrap()
}

/// Runs on both backends and asserts they agree, then returns the output.
fn both(label: &str, source: &str) -> String {
    let wasm = run_wasm(label, source);
    let node = run_node(label, source);
    assert_eq!(wasm, node, "backends disagree (spec 0052 parity)");
    wasm
}

// ===========================================================================
// D4/D6 — the three exits, and reverse order
// ===========================================================================

/// The value path (D4.1): actions run after the block's value, latest first,
/// and an inner block's `defer` is scoped to that block (D2/D6).
#[test]
fn actions_run_in_reverse_order_and_scope_to_their_block() {
    let out = both(
        "order",
        "import std.io\n\
         \n\
         fn main() -> Unit uses { Io } {\n\
         \x20   defer Io.print(\"a\")\n\
         \x20   defer Io.print(\"b\")\n\
         \x20   {\n\
         \x20       defer Io.print(\"c\")\n\
         \x20       Io.print(\"inner\")\n\
         \x20   }\n\
         \x20   Io.print(\"outer\")\n\
         }\n",
    );
    assert_eq!(out, "innercouterba");
}

/// The error channel (D4.2): the action runs on the way out and the error keeps
/// propagating to the caller's `catch`.
#[test]
fn an_error_runs_the_action_on_its_way_out() {
    let out = both(
        "throw",
        "import std.io\n\
         \n\
         enum Boom { Bang }\n\
         \n\
         fn explode() -> Int throws Boom uses {} { throw Boom::Bang }\n\
         \n\
         fn guarded() -> Int throws Boom uses { Io } {\n\
         \x20   defer Io.print(\"cleanup \")\n\
         \x20   Io.print(\"before \")\n\
         \x20   explode()?\n\
         }\n\
         \n\
         fn main() -> Unit uses { Io } {\n\
         \x20   try {\n\
         \x20       let _n = guarded()\n\
         \x20       Io.print(\"unreachable\")\n\
         \x20   } catch {\n\
         \x20       Bang -> Io.print(\"caught\")\n\
         \x20   }\n\
         }\n",
    );
    assert_eq!(out, "before cleanup caught");
}

/// The self tail call (D4.3). Emela has no loop construct, so this is the only
/// way to iterate: the action must run once per iteration — not zero times
/// (the jump skipping the value path) and not twice (a marker passing through
/// it as well as a per-jump copy).
#[test]
fn a_self_tail_call_runs_the_action_once_per_iteration() {
    let out = both(
        "loop",
        "import std.io\n\
         \n\
         fn count(n: Int) -> Unit uses { Io } {\n\
         \x20   if n == 0 {\n\
         \x20       Io.print(\"|\")\n\
         \x20   } else {\n\
         \x20       defer Io.print(\"x\")\n\
         \x20       count(n - 1)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() -> Unit uses { Io } { count(3) }\n",
    );
    // Each iteration's action fires at its jump, before the next iteration
    // begins — so the three `x`s precede the base case's `|`.
    assert_eq!(out, "xxx|");
}

/// A `defer` in the loop body must not cost stack: the self tail call stays a
/// jump (spec 0045 T2).
#[test]
fn a_deferred_loop_does_not_grow_the_stack() {
    let out = run_wasm(
        "deep",
        "import std.io\n\
         \n\
         fn count(n: Int, acc: Int) -> Int uses { Io } {\n\
         \x20   if n == 0 {\n\
         \x20       acc\n\
         \x20   } else {\n\
         \x20       defer Io.print(\"\")\n\
         \x20       count(n - 1, acc + 1)\n\
         \x20   }\n\
         }\n\
         \n\
         fn main() -> Unit uses { Io } { Io.print(count(200000, 0)) }\n",
    );
    assert_eq!(out, "200000");
}

// ===========================================================================
// D3/D8/D9 — the frontend rules
// ===========================================================================

/// D8: the action must not throw. It runs while an error may already be
/// propagating, and the error channel carries one error (spec 0011).
#[test]
fn a_throwing_action_is_rejected() {
    let output = check_single(
        "enum Boom { Bang }\n\
         \n\
         fn explode() -> Unit throws Boom uses {} { throw Boom::Bang }\n\
         \n\
         fn main() -> Unit uses {} {\n\
         \x20   defer explode()\n\
         \x20   ()\n\
         }\n",
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("Throwing `defer` action"), "{text}");
    assert!(text.contains("catch"), "the help names the fix: {text}");
}

/// D8: `?` inside the action is the same violation — the action would raise.
#[test]
fn a_question_in_the_action_is_rejected() {
    let output = check_single(
        "enum Boom { Bang }\n\
         \n\
         fn explode() -> Int throws Boom uses {} { throw Boom::Bang }\n\
         \n\
         fn main() -> Unit throws Boom uses {} {\n\
         \x20   defer explode()?\n\
         \x20   ()\n\
         }\n",
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Throwing `defer` action"),
        "{}",
        stderr(&output)
    );
}

/// D8: the action runs for its effect, so a leftover value is a mistake.
#[test]
fn a_non_unit_action_is_rejected() {
    let output = check_single(
        "fn main() -> Unit uses {} {\n\
         \x20   defer 1 + 1\n\
         \x20   ()\n\
         }\n",
    );
    assert!(!output.status.success());
    let text = stderr(&output);
    assert!(text.contains("`defer` action must be `Unit`"), "{text}");
    assert!(text.contains("`Int`"), "the label names the type: {text}");
}

/// D3: a trailing `defer` guards nothing.
#[test]
fn a_trailing_defer_is_rejected() {
    let output = check_single(
        "import std.io\n\
         \n\
         fn main() -> Unit uses { Io } {\n\
         \x20   Io.print(\"x\")\n\
         \x20   defer Io.print(\"y\")\n\
         }\n",
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("Trailing `defer`"),
        "{}",
        stderr(&output)
    );
}

/// D9: the action's capabilities are the enclosing function's, so an undeclared
/// one is caught at the `uses` gate like any other reference.
#[test]
fn the_action_needs_the_enclosing_uses_row() {
    let output = check_single(
        "import std.io\n\
         \n\
         fn main() -> Unit uses {} {\n\
         \x20   defer Io.print(\"x\")\n\
         \x20   ()\n\
         }\n",
    );
    assert!(!output.status.success(), "{}", stderr(&output));
}

// ===========================================================================
// std — the leak the mechanism exists to close
// ===========================================================================

/// `std.fs`'s `read_file` closes the file on the error path too. Reading a
/// *directory* opens fine and then fails, which is exactly the shape that used
/// to skip `raw_close` and leak one descriptor per call.
///
/// The js backend hands out the raw OS descriptor as `File.id`, and POSIX
/// allocates the lowest free one — so a fresh open receives the same descriptor
/// after 50 failed reads precisely when none of them leaked. (Against the old
/// body this prints 50, one leak per iteration.)
#[test]
fn std_read_file_closes_on_the_error_path() {
    let dir = temp_dir("fdleak");
    let probe = dir.join("probe");
    fs::create_dir_all(&probe).unwrap();
    let path = probe.display().to_string();
    let source = format!(
        "import std.fs\n\
         import std.io\n\
         \n\
         fn churn(path: String, n: Int) -> Unit uses {{ Fs }} {{\n\
         \x20   if n == 0 {{\n\
         \x20       ()\n\
         \x20   }} else {{\n\
         \x20       try {{\n\
         \x20           let _d = Fs.read_file(path)\n\
         \x20           ()\n\
         \x20       }} catch {{ e -> () }}\n\
         \x20       churn(path, n - 1)\n\
         \x20   }}\n\
         }}\n\
         \n\
         fn next_fd(path: String) -> Int throws FsError uses {{ Fs }} {{\n\
         \x20   let f = Fs.open_read(path)?\n\
         \x20   defer Fs.close(f.id)\n\
         \x20   f.id\n\
         }}\n\
         \n\
         fn main() -> Unit uses {{ Fs, Io }} {{\n\
         \x20   let dir = {path:?}\n\
         \x20   let base = try {{ next_fd(dir) }} catch {{ e -> 0 - 1 }}\n\
         \x20   churn(dir, 50)\n\
         \x20   let after = try {{ next_fd(dir) }} catch {{ e -> 0 - 1 }}\n\
         \x20   Io.print(base)\n\
         \x20   Io.print(\" \")\n\
         \x20   Io.print(after - base)\n\
         }}\n"
    );
    let out = run_node("fdleak", &source);
    let _ = fs::remove_dir_all(&dir);
    let (base, drift) = out.split_once(' ').expect("two numbers");
    assert!(
        base.parse::<i32>().unwrap() >= 0,
        "the probe opens must succeed: {out}"
    );
    assert_eq!(drift, "0", "read_file leaked a descriptor per failed read");
}

/// The other side of D9: declaring the capability is enough — a `defer` is the
/// only thing that needs it here.
#[test]
fn the_action_alone_justifies_the_uses_row() {
    let output = check_single(
        "import std.io\n\
         \n\
         fn main() -> Unit uses { Io } {\n\
         \x20   defer Io.print(\"x\")\n\
         \x20   ()\n\
         }\n",
    );
    assert!(output.status.success(), "{}", stderr(&output));
}
