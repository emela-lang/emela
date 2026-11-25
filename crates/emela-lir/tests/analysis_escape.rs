//! Tests for the escape analysis on lowered programs: the intraprocedural sets SRA reads and the
//! interprocedural parameter summary that borrow inference (C2/PR-026), reuse (C4/PR-042) and the
//! records and closures of SRA (C5/PR-043) consume.

use crate::support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::escape;
use emela_lir::{FnId, Module, convert_closures, from_core};
use support::try_lower_with_opt;

/// Lowers a program to the stage the closure-level analyses run at: monomorphic, in the Flow IR,
/// closures lifted. The Core IR optimisations are off, so the shapes are the ones lowering makes.
fn mid(source: &str) -> Module {
    let m = try_lower_with_opt(
        &[("main.emel", source)],
        Roots::Exported,
        emela_core::core_sources(),
        emela_core::bundled_files(),
        false,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    let roots: Vec<_> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect();
    let (mono, _) = monomorphize_from(&m, &roots);
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
        .find(|(_, f)| {
            f.name == name
                || f.name
                    .strip_prefix(name)
                    .is_some_and(|rest| rest.starts_with('['))
        })
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

/// The parameter summary of `name`.
fn summary(m: &Module, name: &str) -> Vec<bool> {
    let summaries = escape::parameter_summary(m)
        .known()
        .expect("a well-formed module has a parameter summary");
    summaries[&function(m, name)].clone()
}

/// The escaping variables of `name`.
fn escaping(m: &Module, name: &str) -> rustc_hash::FxHashSet<emela_lir::VarId> {
    let sets = escape::analyze(m)
        .known()
        .expect("a well-formed module has escape sets");
    sets[&function(m, name)].clone()
}

fn variable(m: &Module, name: &str, hint: &str) -> emela_lir::VarId {
    let id = function(m, name);
    m.functions[id]
        .vars
        .iter()
        .find(|(_, v)| v.hint == hint)
        .map(|(v, _)| v)
        .unwrap_or_else(|| panic!("no variable `{hint}` in `{name}`"))
}

#[test]
fn a_returned_parameter_escapes() {
    let m = mid("\
fn id(x: (Int, Int)) -> (Int, Int) { x }

fn main() {
  t = (1, 2)
  u = id(t)
  dbg(u)
}
");
    assert_eq!(summary(&m, "id"), vec![true]);
}

#[test]
fn a_parameter_only_read_and_returned_as_a_scalar_does_not_escape() {
    let m = mid("\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn main() { dbg(first((1, 2))) }
");
    assert_eq!(summary(&m, "first"), vec![false]);
}

#[test]
fn parameters_stored_in_a_list_escape() {
    let m = mid("\
fn store(xs: List[(Int, Int)], x: (Int, Int)) -> List[(Int, Int)] {
  [x, ..xs]
}

fn main() {
  xs = [(0, 0)]
  dbg(store(xs, (1, 2)))
}
");
    assert_eq!(summary(&m, "store"), vec![true, true]);
}

#[test]
fn a_local_passed_to_an_escaping_callee_escapes_where_it_is_passed() {
    let m = mid("\
fn id(x: (Int, Int)) -> (Int, Int) { x }

fn caller() {
  t = (1, 2)
  u = id(t)
  ()
}

fn main() { caller() }
");
    // `id` returns its parameter, so the value the caller passes escapes through the call even
    // though the caller ignores the result.
    assert_eq!(summary(&m, "id"), vec![true]);
    assert!(escaping(&m, "caller").contains(&variable(&m, "caller", "t")));
}

#[test]
fn a_callee_that_only_reads_a_parameter_does_not_make_it_escape() {
    let m = mid("\
fn read(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn caller(t: (Int, Int)) -> Int {
  read(t)
}

fn main() {
  t = (1, 2)
  dbg(caller(t))
}
");
    assert_eq!(summary(&m, "read"), vec![false]);
    // The call passes the parameter to a callee that only reads a field of it.
    assert_eq!(summary(&m, "caller"), vec![false]);
}

#[test]
fn mutual_recursion_is_summarised_to_a_fixpoint() {
    let m = mid("\
fn ping(x: (Int, Int)) -> (Int, Int) { pong(x) }
fn pong(y: (Int, Int)) -> (Int, Int) { y }

fn main() { dbg(ping((1, 2))) }
");
    assert_eq!(summary(&m, "pong"), vec![true]);
    assert_eq!(summary(&m, "ping"), vec![true]);
}

#[test]
fn a_cycle_that_never_observes_a_value_keeps_it_local() {
    let m = mid("\
fn a(x: (Int, Int)) { b(x) }
fn b(y: (Int, Int)) { a(y) }

fn main() { a((1, 2)) }
");
    assert_eq!(summary(&m, "a"), vec![false]);
    assert_eq!(summary(&m, "b"), vec![false]);
}

#[test]
fn an_indirect_call_makes_every_argument_escape() {
    let m = mid("\
fn apply(f: fn((Int, Int)) -> Int, x: (Int, Int)) -> Int { f(x) }

fn main() {
  double = |p| {
    (a, b) = p
    a + b
  }
  dbg(apply(double, (1, 2)))
}
");
    assert_eq!(summary(&m, "apply"), vec![false, true]);
}

#[test]
fn only_a_malformed_module_has_no_summary() {
    let m = mid("fn main() { () }");
    assert!(escape::parameter_summary(&m).is_known());

    // A declared function that is never defined has no body to summarise, and no host to be
    // conservative about; `verify` reports it as `MissingBody`.
    let mut malformed = Module::new(emela_core::Module::new());
    malformed.declare("declared_but_never_defined");
    assert!(escape::parameter_summary(&malformed).is_unknown());
}

#[test]
fn the_summary_does_not_change_the_intraprocedural_sets() {
    // The two analyses share the walk; running the summary first must not feed back into the
    // escape sets SRA reads.
    let m = mid("\
fn id(x: (Int, Int)) -> (Int, Int) { x }

fn main() {
  t = (1, 2)
  dbg(id(t))
}
");
    let before = escape::analyze(&m).known().expect("known");
    let _ = escape::parameter_summary(&m);
    let after = escape::analyze(&m).known().expect("known");
    assert_eq!(before, after);
}
