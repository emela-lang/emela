//! Syntax-level tests for effect-row parameters (spec 0022, revised): the
//! lowercase names of a function's `<...>` list, referenced from `uses`
//! positions as the bare form `uses e` or the tail form `uses { Io, ..e }`.
//! These exercise the parser's accept/reject rules; row unification and
//! subsumption are covered by `effect_subsumption.rs`.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

/// Runs `emela check` against a single self-contained file (no package).
fn check_single(source: &str) -> std::process::Output {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-rowsyn-{}-{id}", std::process::id()));
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

fn assert_rejected(source: &str, diagnostic: &str) {
    let output = check_single(source);
    assert!(!output.status.success(), "expected check to fail");
    let err = stderr(&output);
    assert!(err.contains(diagnostic), "unexpected diagnostic:\n{err}");
}

/// The bare form: `<T, e>` declares a row parameter, `uses e` references it in
/// a parameter's function type and the function's own row, and the `{` after
/// the bare `uses e` is unambiguously the body.
#[test]
fn accepts_bare_row_variable() {
    let output = check_single(
        "fn apply<T, e>(x: T, f: (T) -> T uses e) -> T uses e {\n\
             f(x)\n\
         }\n\
         fn main() -> Unit uses {} {\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// The extended form `uses { Io, ..e }`: the function performs its own `Io`
/// (gated on the concrete part) on top of whatever the callback needs.
#[test]
fn accepts_row_extension() {
    let output = check_single(
        "import std.io\n\
         fn traced<T, e>(x: T, f: (T) -> T uses e) -> T uses { Io, ..e } {\n\
             let p = Io.print(\"call\\n\")\n\
             f(x)\n\
         }\n\
         fn main() -> Unit uses {} {\n\
             ()\n\
         }\n",
    );
    assert!(
        output.status.success(),
        "expected check to pass:\n{}",
        stderr(&output)
    );
}

/// `uses e` without a matching `<..., e>` declaration is rejected (spec 0022:
/// row variables are explicitly quantified).
#[test]
fn rejects_undeclared_bare_row_variable() {
    assert_rejected(
        "fn apply<T>(x: T, f: (T) -> T uses e) -> T uses {} {\n\
             x\n\
         }\n",
        "Unknown effect-row variable",
    );
}

/// `..e` inside braces also requires the declaration.
#[test]
fn rejects_undeclared_tail() {
    assert_rejected(
        "import std.io\n\
         fn f() -> Unit uses { Io, ..e } {\n\
             ()\n\
         }\n",
        "Unknown effect-row variable",
    );
}

/// A row parameter written bare inside braces is a near-miss for the tail
/// syntax and gets a targeted diagnostic.
#[test]
fn rejects_row_variable_without_dots_in_braces() {
    assert_rejected(
        "fn f<T, e>(f: (T) -> T uses { e }) -> Unit uses {} {\n\
             ()\n\
         }\n",
        "Effect-row variable without `..`",
    );
}

/// Type parameters must start uppercase (spec 0014); on a data type a
/// lowercase name cannot be a row parameter either (spec 0022).
#[test]
fn rejects_lowercase_type_parameter_on_enum() {
    assert_rejected(
        "enum Box<t> {\n\
             Value(t)\n\
         }\n",
        "Lowercase type parameter",
    );
}

/// Row parameters carry no trait bounds (spec 0022).
#[test]
fn rejects_bound_on_row_parameter() {
    assert_rejected(
        "fn f<e: Show>(g: () -> Unit uses e) -> Unit uses e {\n\
             g()\n\
         }\n",
        "Bound on an effect-row parameter",
    );
}

/// `<e, e>` and `<T, T>` share one duplicate check across both kinds.
#[test]
fn rejects_duplicate_row_parameter() {
    assert_rejected(
        "fn f<e, e>(g: () -> Unit uses e) -> Unit uses e {\n\
             g()\n\
         }\n",
        "Duplicate type parameter",
    );
}

/// A row variable is not a type (spec 0022): it cannot appear in a type
/// position.
#[test]
fn rejects_row_variable_in_type_position() {
    assert_rejected(
        "fn f<e>(x: e, g: () -> Unit uses e) -> Unit uses e {\n\
             g()\n\
         }\n",
        "Effect-row variable in type position",
    );
}

/// Trait method signatures have no row parameters in v1, so a tail there is
/// undeclared by construction.
#[test]
fn rejects_row_variable_in_trait_method() {
    assert_rejected(
        "trait Runner {\n\
             fn run(self: Self) -> Unit uses e\n\
         }\n",
        "Unknown effect-row variable",
    );
}

/// Effect operations cannot be row-polymorphic in v1 (spec 0022 Open
/// Questions).
#[test]
fn rejects_row_parameter_on_effect_operation() {
    assert_rejected(
        "effect Log {\n\
             pub fn tap<e>(f: () -> Unit uses e) -> Unit uses e {\n\
                 f()\n\
             }\n\
         }\n",
        "Effect-row parameter on an effect operation",
    );
}
