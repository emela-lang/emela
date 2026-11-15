//! Tests for the scalar replacement of tuples on lowered programs: a non-escaping tuple of
//! scalars loses its allocation, while a tuple that leaves the function stays.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::{FnId, Module, convert_closures, dump_function, from_core, opt, verify};
use support::try_lower_with_opt;

/// Lowers, monomorphizes and converts to the Flow IR, then lifts the closures (the stage of the
/// closure-level optimisations).
///
/// The Core IR optimisations are off: these tests are about what the LIR passes do to the shapes
/// lowering produces, and Core inlining would move those shapes into the caller. The combination
/// (inline, then SRA across the call) is covered by the backend tests and `opt_diff`.
fn mid(files: &[(&str, &str)], roots: Roots) -> Module {
    let m = try_lower_with_opt(
        files,
        roots,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        false,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let fn_roots: Vec<_> = match roots {
        Roots::All => m.functions.iter().map(|(id, _)| id).collect(),
        _ => m
            .functions
            .iter()
            .filter(|(_, f)| f.exported)
            .map(|(id, _)| id)
            .collect(),
    };
    let (mono, _) = monomorphize_from(&m, &fn_roots);
    if let Err(errors) = verify_monomorphic(&mono) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("not monomorphic:\n{}", lines.join("\n"));
    }
    let mut lir = from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    });
    convert_closures(&mut lir);
    lir
}

fn function(m: &Module, name: &str) -> FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

fn text(m: &Module, name: &str) -> String {
    dump_function(m, &m.functions[function(m, name)])
}

#[test]
fn a_non_escaping_tuple_loses_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
fn add(a: Int, b: Int) -> Int {
  t = (a * 2, b + 1)
  (x, y) = t
  x + y
}

fn main() { dbg(add(3, 4)) }
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid(&mut m, true);
    assert!(stats.sra_tuples > 0, "SRA did not fire");
    verify(&m).expect("SRA left invalid LIR");
    let add = text(&m, "add");
    assert!(!add.contains("tuple("), "{add}");
    assert!(!add.contains("tuple_get"), "{add}");
}

#[test]
fn the_pass_is_idempotent_on_a_lowered_program() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
fn f(a: Int) -> Int {
  t = (a, a + 1)
  (x, y) = t
  x + y
}

fn main() { dbg(f(1)) }
",
        )],
        Roots::Exported,
    );
    let first = opt::run_mid(&mut m, true);
    assert!(first.sra_tuples > 0);
    let once = emela_lir::dump(&m);
    let second = opt::run_mid(&mut m, true);
    assert_eq!(second.sra_tuples, 0, "SRA fired again");
    assert_eq!(emela_lir::dump(&m), once);
}

#[test]
fn a_returned_tuple_keeps_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
fn pair() -> (Int, Int) {
  (1, 2)
}

fn main() {
  t = pair()
  (x, y) = t
  dbg(x)
}
",
        )],
        Roots::Exported,
    );
    opt::run_mid(&mut m, true);
    verify(&m).expect("SRA left invalid LIR");
    let pair = text(&m, "pair");
    assert!(pair.contains("tuple("), "{pair}");
}

#[test]
fn the_optimisation_switch_turns_sra_off() {
    let mut off = mid(
        &[(
            "main.emel",
            "\
fn f(a: Int) -> Int {
  t = (a, a + 1)
  (x, y) = t
  x + y
}

fn main() { dbg(f(1)) }
",
        )],
        Roots::Exported,
    );
    let before = emela_lir::dump(&off);
    let stats = opt::run_mid(&mut off, false);
    assert_eq!(stats.sra_tuples, 0);
    assert_eq!(emela_lir::dump(&off), before);
}
