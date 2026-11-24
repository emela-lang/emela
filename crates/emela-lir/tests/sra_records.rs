//! Tests for the scalar replacement of records on lowered programs (C5/PR-043): a non-escaping
//! record of scalars loses its allocation behind `LirRules::sra_records`, while a record that
//! leaves the function, has a reference field or is nested stays. The flag is off by default, so
//! the same program keeps its allocation when it is not set.
//!
//! A tag switch on a single-variant record, a record whose construction has a defect or suspension
//! edge and an escape analysis that answers `Unknown` cannot be produced by lowering: they are
//! tested on hand-built IR in the unit tests of `opt::sra`.

use crate::support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::opt::{self, LirRules, Options};
use emela_lir::{FnId, Module, convert_closures, dump_function, from_core, verify};
use support::try_lower_with_opt;

/// Lowers, monomorphizes and converts to the Flow IR, then lifts the closures (the stage of the
/// closure-level optimisations).
///
/// The Core IR optimisations are off: these tests are about what the LIR passes do to the shapes
/// lowering produces, and Core inlining would move those shapes into the caller.
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

/// SRA with records on or off; the other LIR passes are off so a test sees exactly SRA.
fn sra(records: bool) -> Options {
    Options {
        rules: LirRules {
            sra: true,
            sra_records: records,
            devirt: false,
            sra_closures: false,
            rc_cleanup: false,
            rc_borrow: false,
            dce: false,
            drop_spec: false,
            borrow: false,
            reuse: false,
            switch: false,
        },
    }
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

/// A program with a local record read field by field, returned from `add` as an Int.
const POINT: &str = "\
pub type Point(x: Int, y: Int)

fn add(a: Int) -> Int {
  p = Point(x: a * 2, y: a + 1)
  p.x + p.y
}

fn main() { dbg(add(3)) }
";

#[test]
fn a_non_escaping_record_loses_its_allocation() {
    let mut m = mid(&[("main.emel", POINT)], Roots::Exported);
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert!(stats.sra_records > 0, "record SRA did not fire");
    verify(&m).expect("SRA left invalid LIR");
    let add = text(&m, "add");
    assert!(!add.contains("make"), "{add}");
    assert!(!add.contains("field "), "{add}");
}

#[test]
fn the_flag_off_leaves_the_record_alone() {
    let mut m = mid(&[("main.emel", POINT)], Roots::Exported);
    let before = emela_lir::dump(&m);
    let stats = opt::run_mid_with(&mut m, sra(false));
    assert_eq!(stats.sra_records, 0);
    assert_eq!(stats.sra_deopts, 0);
    assert_eq!(emela_lir::dump(&m), before);
    assert!(text(&m, "add").contains("make"), "{}", text(&m, "add"));
}

#[test]
fn a_returned_record_keeps_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
pub type Point(x: Int, y: Int)

fn make(a: Int) -> Point {
  Point(x: a, y: a + 1)
}

fn main() {
  p = make(1)
  dbg(p.x)
}
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(stats.sra_records, 0);
    assert!(stats.sra_deopts > 0);
    verify(&m).expect("SRA left invalid LIR");
    assert!(text(&m, "make").contains("make"), "{}", text(&m, "make"));
}

#[test]
fn a_record_stored_in_a_list_keeps_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
pub type Point(x: Int, y: Int)

fn store(a: Int) -> Int {
  p = Point(x: a, y: a + 1)
  xs = [p]
  List.length(xs)
}

fn main() { dbg(store(1)) }
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(stats.sra_records, 0);
    verify(&m).expect("SRA left invalid LIR");
    assert!(text(&m, "store").contains("make"), "{}", text(&m, "store"));
}

#[test]
fn a_record_captured_by_a_closure_keeps_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
pub type Point(x: Int, y: Int)

fn capture(a: Int) -> Int {
  p = Point(x: a, y: a + 1)
  f = || p.x + p.y
  f()
}

fn main() { dbg(capture(1)) }
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(stats.sra_records, 0);
    verify(&m).expect("SRA left invalid LIR");
    assert!(
        text(&m, "capture").contains("make"),
        "{}",
        text(&m, "capture")
    );
}

#[test]
fn a_record_with_a_reference_field_keeps_its_allocation() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
pub type Named(name: String, n: Int)

fn len(s: String) -> Int {
  r = Named(name: s, n: 1)
  String.length(r.name) + r.n
}

fn main() { dbg(len(\"abc\")) }
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(stats.sra_records, 0);
    assert!(stats.sra_deopts > 0);
    verify(&m).expect("SRA left invalid LIR");
    assert!(text(&m, "len").contains("make"), "{}", text(&m, "len"));
}

#[test]
fn a_nested_record_keeps_both_allocations() {
    let mut m = mid(
        &[(
            "main.emel",
            "\
pub type Inner(n: Int)

pub type Outer(inner: Inner, k: Int)

fn read(a: Int) -> Int {
  o = Outer(inner: Inner(n: a), k: 2)
  o.k
}

fn main() { dbg(read(1)) }
",
        )],
        Roots::Exported,
    );
    let stats = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(stats.sra_records, 0, "a nested record was decomposed");
    assert_eq!(stats.sra_deopts, 2);
    verify(&m).expect("SRA left invalid LIR");
    let read = text(&m, "read");
    assert_eq!(read.matches("make").count(), 2, "{read}");
}

#[test]
fn the_record_pass_is_idempotent_on_a_lowered_program() {
    let mut m = mid(&[("main.emel", POINT)], Roots::Exported);
    let first = opt::run_mid_with(&mut m, sra(true));
    assert!(first.sra_records > 0);
    let once = emela_lir::dump(&m);
    verify(&m).expect("SRA left invalid LIR");
    let second = opt::run_mid_with(&mut m, sra(true));
    assert_eq!(second.sra_records, 0, "record SRA fired again");
    assert_eq!(second.sra_deopts, 0);
    assert_eq!(emela_lir::dump(&m), once);
}
