//! End-to-end tests for `effect` declarations and effect-qualified operations
//! (spec 0036): importing an effect as a whole, qualified-only operation calls,
//! effect gating via `uses`, and the rejection diagnostics. Effects are backed
//! by platform functions (spec 0013); the positive cases import the embedded
//! `std.io` (spec 0038), which resolves with no `--package`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-effect-1f-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let input = dir.join("main.emel");
    fs::write(&input, source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg(&input)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    output
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// `import std.io` brings in the whole effect, and both operations are callable
/// in qualified form inside a `uses { io }` function (spec 0036).
#[test]
fn imports_effect_and_calls_operations_qualified() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { io } {\n\
             let a = io.print(\"hi\\n\")\n\
             io.eprint(\"bye\\n\")\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// A bare operation name (`print`) must not resolve to an imported effect
/// operation; the diagnostic points at the qualified spelling.
#[test]
fn bare_effect_operation_is_rejected() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { io } { print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("operation of effect `io`") && err.contains("io.print"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A per-operation import of an effect operation is rejected; the diagnostic
/// tells the user to import the effect instead (spec 0036).
#[test]
fn per_operation_effect_import_is_rejected() {
    let output = check_single(
        "import std.io.print\n\
         fn main() -> Unit uses { io } { io.print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Effect operation import") && err.contains("import std.io"),
        "unexpected diagnostic:\n{err}"
    );
}

/// Calling an effect operation requires the effect in `uses`; the existing
/// subset check (spec 0023) gates it.
#[test]
fn calling_operation_without_uses_is_rejected() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit { io.print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Unhandled effects") || err.contains("uses"),
        "unexpected diagnostic:\n{err}"
    );
}

/// An `effect` block parses standalone and its operations carry the effect
/// implicitly: a `uses { log }` function may use them (bare, since a same-file
/// effect has no import qualifier). This exercises the parser desugar path.
#[test]
fn single_file_effect_declaration_parses() {
    let output = check_single(
        "effect log {\n\
             pub fn info(s: String) -> Unit { () }\n\
         }\n\
         fn main() -> Unit uses { log } { info(\"hi\\n\") }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// An explicit `uses` clause on an operation inside an `effect` block is
/// redundant and rejected (spec 0036): the effect is implicit.
#[test]
fn explicit_uses_inside_effect_is_rejected() {
    let output = check_single(
        "effect log {\n\
             pub fn info(s: String) -> Unit uses { log } { () }\n\
         }\n\
         fn main() -> Unit {}\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Redundant effect on operation") || err.contains("remove the `uses`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// An `intrinsic fn` cannot be an effect operation (it must be pure); the
/// parser rejects it inside an `effect` block.
#[test]
fn intrinsic_inside_effect_is_rejected() {
    let output = check_single(
        "effect log {\n\
             intrinsic fn emit(s: String) -> Unit\n\
         }\n\
         fn main() -> Unit {}\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Intrinsic inside effect"),
        "unexpected diagnostic:\n{err}"
    );
}
