//! End-to-end tests for first-class effects and module-unit imports (spec
//! 0037): importing the module that owns an effect, `Effect.op(...)` calls
//! gated by `uses { Effect }`, the layered diagnostics, and the visibility of
//! private/backing operations. Effects are backed by platform functions (spec
//! 0013); the positive cases import the embedded `std.io` (spec 0038), which
//! resolves with no `--package`.

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

/// `import std.io` brings the module in, its effect `Io` into scope, and both
/// public operations are callable as `Io.op(...)` inside a `uses { Io }`
/// function (spec 0037).
#[test]
fn imports_module_and_calls_operations_effect_qualified() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { Io } {\n\
             let a = Io.print(\"hi\\n\")\n\
             Io.eprint(\"bye\\n\")\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// A bare operation name (`print`) must not resolve to an effect operation;
/// the diagnostic points at the `Io.print` spelling (spec 0037).
#[test]
fn bare_effect_operation_is_rejected() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { Io } { print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("operation of effect `Io`") && err.contains("Io.print"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A per-item import (`import std.io.print`) is rejected — imports are
/// module-unit (spec 0037) — and the diagnostic guides to `import std.io`.
#[test]
fn per_item_import_is_rejected_with_guidance() {
    let output = check_single(
        "import std.io.print\n\
         fn main() -> Unit uses {} { () }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Module-unit import") && err.contains("import std.io"),
        "unexpected diagnostic:\n{err}"
    );
}

/// Calling an operation without declaring the effect fails at the reference
/// with the `uses`-gate diagnostic (spec 0037), naming the effect.
#[test]
fn calling_operation_without_uses_is_rejected() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit { Io.print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Effect not declared in `uses`") && err.contains("`Io`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// `uses { Io }` without the import that brings `Io` into scope is an unknown
/// effect, and the diagnostic names the missing import (spec 0037).
#[test]
fn uses_without_import_is_unknown_effect() {
    let output = check_single("fn main() -> Unit uses { Io } { () }\n");
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Unknown effect") && err.contains("import std.io"),
        "unexpected diagnostic:\n{err}"
    );
}

/// The old lowercase spelling gets a capitalization hint (spec 0037).
#[test]
fn lowercase_uses_row_gets_capitalization_hint() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { io } { Io.print(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Unknown effect") && err.contains("did you mean `Io`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A private backing extern must not leak to the importer by bare name
/// (the v0.4.0 visibility hole; spec 0037 R5).
#[test]
fn private_backing_extern_is_invisible_bare() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { Io } { write_stdout(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Unknown name") && err.contains("write_stdout"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A private backing extern is not a public operation either: `Io.write_stdout`
/// is rejected as private (spec 0037).
#[test]
fn private_backing_extern_is_rejected_qualified() {
    let output = check_single(
        "import std.io\n\
         fn main() -> Unit uses { Io } { Io.write_stdout(\"hi\\n\") }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("private operation of effect `Io`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// A `--package` addressed as `std` may not provide a module reserved by the
/// embedded core (spec 0038): the embedded `std.io` always wins resolution,
/// so a conflicting file is a hard, eager error — even if never imported.
#[test]
fn std_package_shadowing_an_embedded_module_is_rejected() {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-std-shadow-{}-{id}", std::process::id()));
    let package = dir.join("std");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("emela-package.json"),
        r#"{"name":"std","source":"src"}"#,
    )
    .unwrap();
    fs::write(
        package.join("src").join("io.emel"),
        "module io\n\neffect Io {\nextern fn write_stdout(s: String) -> Unit\npub fn print(s: String) -> Unit { write_stdout(s) }\n}\n",
    )
    .unwrap();
    let app = dir.join("main.emel");
    // The app never imports std.io: the conflict is still rejected.
    fs::write(&app, "fn main() -> Int uses {} { 0 }\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("check")
        .arg("--package")
        .arg(&package)
        .arg(&app)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("embedded in the compiler") && err.contains("io.emel"),
        "unexpected diagnostic:\n{err}"
    );
}

/// An `effect` declared in the compilation root works exactly like an imported
/// one (spec 0037): operations are `Effect.op(...)`, gated by `uses`.
#[test]
fn same_file_effect_is_qualified_and_gated() {
    let output = check_single(
        "effect Log {\n\
             pub fn info(s: String) -> Unit { () }\n\
         }\n\
         fn main() -> Unit uses { Log } { Log.info(\"hi\\n\") }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// Sibling operations of one effect call each other by bare name (spec 0037);
/// from outside the effect the bare name stays an error.
#[test]
fn sibling_operations_call_bare_but_outsiders_do_not() {
    let ok = check_single(
        "effect Log {\n\
             fn fmt(s: String) -> String { s }\n\
             pub fn info(s: String) -> Unit { let x = fmt(s)\n() }\n\
         }\n\
         fn main() -> Unit uses { Log } { Log.info(\"hi\\n\") }\n",
    );
    assert!(
        ok.status.success(),
        "expected check to pass:\n{}",
        stderr(&ok)
    );
    let bad = check_single(
        "effect Log {\n\
             pub fn info(s: String) -> Unit { () }\n\
         }\n\
         fn main() -> Unit uses { Log } { info(\"hi\\n\") }\n",
    );
    assert!(!bad.status.success(), "expected check to fail");
    let err = stderr(&bad);
    assert!(
        err.contains("operation of effect `Log`") && err.contains("Log.info"),
        "unexpected diagnostic:\n{err}"
    );
}

/// An effect name must start with an uppercase letter (spec 0037).
#[test]
fn lowercase_effect_name_is_rejected() {
    let output = check_single(
        "effect log {\n\
             pub fn info(s: String) -> Unit { () }\n\
         }\n\
         fn main() -> Unit uses {} { () }\n",
    );
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(
        err.contains("Invalid effect name") && err.contains("`effect Log`"),
        "unexpected diagnostic:\n{err}"
    );
}

/// An explicit `uses` clause on an operation inside an `effect` block is
/// redundant and rejected (specs 0036/0037): the effect is implicit.
#[test]
fn explicit_uses_inside_effect_is_rejected() {
    let output = check_single(
        "effect Log {\n\
             pub fn info(s: String) -> Unit uses { Log } { () }\n\
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
        "effect Log {\n\
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
