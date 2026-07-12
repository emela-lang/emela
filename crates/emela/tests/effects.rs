//! End-to-end tests for `effect` declarations and effect-qualified operations
//! (spec 0036): importing an effect as a whole, qualified-only operation calls,
//! effect gating via `uses`, and the rejection diagnostics. Effects are backed
//! by platform functions (spec 0013), so the positive cases lay out a `std`
//! package whose `effect io` wraps `io.write_stdout`/`io.write_stderr`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Lays out a `std` package containing `effect io { ... }` and writes `app` as
/// the compilation-root source. Returns (package dir, app file).
fn io_project(app: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-effects-test-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("io.emel"),
        "effect io {\n\
         extern fn write_stdout(s: String) -> Unit\n\
         extern fn write_stderr(s: String) -> Unit\n\
         pub fn print(s: String) -> Unit { write_stdout(s) }\n\
         pub fn eprint(s: String) -> Unit { write_stderr(s) }\n\
         }\n",
    )
    .unwrap();
    let app_file = dir.join("main.emel");
    fs::write(&app_file, app).unwrap();
    (package, app_file)
}

/// Runs `emela check` against a `std`-package program and returns the output.
fn check_with_io(app: &str) -> std::process::Output {
    let (package, app_file) = io_project(app);
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--package")
        .arg(&package)
        .arg(&app_file)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(app_file.parent().unwrap());
    output
}

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
    let output = check_with_io(
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
    let output = check_with_io(
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
    let output = check_with_io(
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
    let output = check_with_io(
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
