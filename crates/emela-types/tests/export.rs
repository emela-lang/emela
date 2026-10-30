//! `@export` and `@entry` (spec 12.7, 12.8): boundary types and the effects of an exported fn.

mod common;

use common::resolve_main;
use emela_types::{CheckOptions, DiagnosticKind, add_entry_exports, check_program_with};

fn diagnostics(src: &str) -> Vec<DiagnosticKind> {
    let (mut program, entry, _) = resolve_main(src);
    add_entry_exports(&mut program, Some(entry));
    let options = CheckOptions {
        entry: Some(entry),
        ..CheckOptions::default()
    };
    check_program_with(&program, &options)
        .diags
        .into_iter()
        .map(|d| d.kind)
        .collect()
}

/// Every parameter and the return value of an `@export` fn are boundary types (E0436).
#[test]
fn export_parameters_and_return_are_boundary_types() {
    let found = diagnostics(
        r#"
@export(js, "f")
pub fn f(a: Int, b: List[Int]) -> List[Int] { b }
"#,
    );
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .any(|d| matches!(d, DiagnosticKind::NotBoundary { ty, .. } if ty == "List[Int]")),
        "{found:?}"
    );
}

/// Types made of boundary types (an `@external` type, a callback) are boundary types.
#[test]
fn boundary_types_include_external_types_and_callbacks() {
    let found = diagnostics(
        r#"
@external
type Handle

@export(js, "f")
pub fn f(a: Handle, g: fn(Handle) -> Handle fails {} use {}) -> Handle { g(a) }
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

/// The R of an `@export` fn is empty except for Async (E0402), with the export-specific context.
#[test]
fn export_effects_are_checked() {
    let found = diagnostics(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

@export(js, "f")
pub fn f() { (use Logger).info("x") }
"#,
    );
    assert!(
        found.iter().any(|d| matches!(
            d,
            DiagnosticKind::UnhandledEffects { context: emela_types::EffectContext::Export { name }, .. }
                if name == "f"
        )),
        "{found:?}"
    );
}

/// The E of an `@export` fn is empty, even when a `fails` clause declares it (E0402).
#[test]
fn export_errors_are_checked() {
    let found = diagnostics(
        r#"
error Timeout

@export(js, "f")
pub fn f() -> Int fails Timeout { fail Timeout }
"#,
    );
    assert!(
        found.iter().any(|d| matches!(
            d,
            DiagnosticKind::ExportFails { function, errors }
                if function == "f" && errors.as_str() == "Timeout"
        )),
        "{found:?}"
    );
}

/// `@entry` requires the fn to fit the adapter's first parameter, including its E and R (E0440).
#[test]
fn entry_checks_the_effects_of_the_function() {
    let adapter = r#"
error Timeout

@export(js, "default", "serve")
pub fn serve(app: fn(Int) -> Int fails {} use {}) -> Int { app(1) }
"#;
    let ok = diagnostics(&format!(
        "{adapter}\n@entry(serve)\npub fn app(n: Int) -> Int {{ n }}\n"
    ));
    assert!(ok.is_empty(), "{ok:?}");

    let found = diagnostics(&format!(
        "{adapter}\n@entry(serve)\npub fn app(n: Int) -> Int fails Timeout {{ fail Timeout }}\n"
    ));
    assert!(
        found
            .iter()
            .any(|d| matches!(d, DiagnosticKind::EntryMismatch { .. })),
        "{found:?}"
    );
}

/// A type parameter is not a boundary type, and is shown by its name, not `'0`.
#[test]
fn export_boundary_types_show_type_parameter_names() {
    let found = diagnostics(
        r#"
@export(js, "f")
pub fn f[A](x: A) -> A { x }
"#,
    );
    assert!(
        found
            .iter()
            .any(|d| matches!(d, DiagnosticKind::NotBoundary { ty, .. } if ty == "A")),
        "{found:?}"
    );
}

/// An `@export` fn whose first parameter is a function type is an entry adapter: its first
/// parameter need not be a boundary type, but the others and the return value are (E0436).
#[test]
fn entry_adapter_skips_the_first_parameter() {
    let found = diagnostics(
        r#"
@export(js, "default", "serve")
pub fn serve(app: fn(List[Int]) -> Int fails {} use {}, n: List[Int]) -> Int { app(n) }
"#,
    );
    // The first parameter's `List[Int]` is allowed; the second is not.
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found
            .iter()
            .any(|d| matches!(d, DiagnosticKind::NotBoundary { ty, .. } if ty == "List[Int]")),
        "{found:?}"
    );
}
