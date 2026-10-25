//! `@export` and `@entry` (spec 12.7, 12.8): boundary types and the effects of an exported fn.

mod common;

use common::resolve_main;
use emela_types::{CheckOptions, DiagnosticKind, check_program_with};

fn diagnostics(src: &str) -> Vec<DiagnosticKind> {
    let (program, entry, _) = resolve_main(src);
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
        found.iter().any(|d| matches!(
            d,
            DiagnosticKind::NotBoundaryType { ty, .. } if ty == "List[Int]"
        )),
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

/// The E of an `@export` fn is empty and its R is empty except for Async (E0402).
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
        found
            .iter()
            .any(|d| matches!(d, DiagnosticKind::UnhandledEffects { .. })),
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
            .any(|d| matches!(d, DiagnosticKind::NotBoundaryType { ty, .. } if ty == "List[Int]")),
        "{found:?}"
    );
}
