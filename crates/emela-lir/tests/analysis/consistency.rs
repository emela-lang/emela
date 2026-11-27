//! B7 analysis properties of the LIR analyses: monotonicity, the soundness fixtures, the
//! `Unknown` contract and the consistency between escape, borrow, ownership and liveness.
//!
//! The per-analysis edge cases are in `escape.rs`, `borrow.rs`, `liveness.rs` and `ownership.rs`
//! beside this file. This file tests the parts a consumer relies on when it combines them:
//!
//! - adding a source only adds facts (an escape, a use, a live variable), and a "unique" or
//!   "borrowed" fact only shrinks when a new use appears;
//! - the known-good and known-bad fixtures of each analysis;
//! - an `Unknown` fact means "no change", checked through the pass that consumes it where one
//!   exists today;
//! - a borrowable parameter is not in the escape summary, a borrowed argument is still unique at
//!   the call, and a saved variable is live before its suspension;
//! - two runs and two arena orders give the same facts, and the module is visited in arena order.

use crate::support;

use std::collections::{BTreeMap, BTreeSet};

use emela_core::{Module as CoreModule, Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::Fact;
use emela_lir::opt::analysis::borrow::{self, BorrowFacts};
use emela_lir::opt::analysis::escape::{self, EscapeSets, ParameterSummary};
use emela_lir::opt::analysis::liveness::{self, LiveSets};
use emela_lir::opt::analysis::ownership::{self, UniqueRefs};
use emela_lir::opt::{self, LirRules, Options};
use emela_lir::{
    Callee, Const, FnBuilder, FnId, Function, Module, OnFail, Operand, Stage, Stmt, StmtId,
    Suspend, Term, Value, VarId, convert_closures, dump, explicit_defects, from_core,
    state_machines, to_heap_with,
};
use support::try_lower_with_opt;

// ---------------------------------------------------------------------------------------------
// Lowering helpers
// ---------------------------------------------------------------------------------------------

/// Lowers, monomorphizes and converts to the Flow IR, with the Core optimisations off.
fn flow(source: &str) -> Module {
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
    from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    })
}

/// The Flow IR after closure conversion: the stage of `run_mid` and of SRA.
fn mid(source: &str) -> Module {
    let mut m = flow(source);
    convert_closures(&mut m);
    m
}

/// The Heap IR with explicit defects and suspension frames, before reference counting.
fn heap(source: &str) -> Module {
    let mut m = mid(source);
    explicit_defects(&mut m);
    state_machines(&mut m);
    m
}

/// The Heap IR after reference counting, with every LIR optimisation off.
fn refcounted(source: &str) -> Module {
    let mut m = flow(source);
    to_heap_with(&mut m, false);
    assert_eq!(m.stage, Stage::RefCounted);
    m
}

fn function(m: &Module, name: &str) -> FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}` in\n{}", dump(m)))
}

fn variable(m: &Module, name: &str, hint: &str) -> VarId {
    let f = &m.functions[function(m, name)];
    f.vars
        .iter()
        .find(|(_, v)| v.hint == hint)
        .map(|(v, _)| v)
        .unwrap_or_else(|| panic!("no variable `{hint}` in `{name}`"))
}

/// The `let` whose destination is `v`.
fn let_stmt(f: &Function, v: VarId) -> StmtId {
    f.stmts
        .iter()
        .find(|(_, s)| matches!(s, Stmt::Let { dst, .. } if *dst == Some(v)))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no let defines `{}`", f.vars[v].hint))
}

fn defining_let(m: &Module, name: &str, hint: &str) -> StmtId {
    let f = &m.functions[function(m, name)];
    let_stmt(f, variable(m, name, hint))
}

fn escape_sets(m: &Module) -> EscapeSets {
    escape::analyze(m)
        .known()
        .expect("a module built by lowering has escape sets")
}

fn parameter_summary(m: &Module) -> ParameterSummary {
    escape::parameter_summary(m)
        .known()
        .expect("a module built by lowering has a parameter summary")
}

fn borrow_facts(m: &Module) -> BorrowFacts {
    borrow::analyze(m)
        .known()
        .expect("a module built by lowering has borrow facts")
}

fn live_sets(m: &Module) -> LiveSets {
    liveness::analyze(m)
        .known()
        .expect("the Heap IR has liveness")
}

fn ownership_facts(m: &Module) -> UniqueRefs {
    ownership::analyze(m)
        .known()
        .expect("the Heap IR has ownership facts")
}

/// The hints of an escape or live set, so two builds can be compared.
fn hint_set(f: &Function, set: &rustc_hash::FxHashSet<VarId>) -> BTreeSet<String> {
    set.iter().map(|v| f.vars[*v].hint.clone()).collect()
}

fn unique_after(m: &Module, name: &str, stmt: &str, var: &str) -> Fact<bool> {
    let refs = ownership_facts(m);
    let f = function(m, name);
    ownership::unique_after(
        &refs,
        f,
        defining_let(m, name, stmt),
        variable(m, name, var),
    )
}

fn int(n: i32) -> Operand {
    Operand::Const(Const::Int(n))
}

/// `value + 0`, a value that reads `value`.
fn read(value: VarId) -> Value {
    Value::Prim {
        op: emela_lir::PrimOp::Add,
        ty: emela_lir::PrimTy::I32,
        args: vec![value.into(), int(0)],
    }
}

fn let_(dst: Option<VarId>, value: Value) -> Stmt {
    Stmt::Let {
        dst,
        value,
        on_defect: None,
        on_suspend: None,
    }
}

/// A Core module with the types the hand-built fixtures use, plus a builder that adds functions.
struct Fixture {
    m: Module,
    pair: TypeId,
    array_pair: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = CoreModule::new();
        let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
        let array_pair = core.intern(emela_core::Type::Array(pair));
        Fixture {
            m: Module::new(core),
            pair,
            array_pair,
        }
    }

    fn build<R>(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>) -> R) -> (FnId, R) {
        let mut b = FnBuilder::new(&self.m.core, name);
        let r = f(&mut b);
        let function = b.finish();
        (self.m.add(function), r)
    }

    fn summary(&self) -> ParameterSummary {
        parameter_summary(&self.m)
    }

    fn params(&self, id: FnId) -> Vec<bool> {
        self.summary()[&id].clone()
    }
}

// ---------------------------------------------------------------------------------------------
// Monotonicity
// ---------------------------------------------------------------------------------------------

/// Builds `f` with a fresh tuple that is either only read (`stored = false`) or also put into a
/// list (`stored = true`).
fn escape_program(stored: bool) -> String {
    let store = if stored {
        "  xs = [t]\n  dbg(xs)\n"
    } else {
        ""
    };
    format!(
        "\
fn f() -> Int {{
  t = (1, 2)
{store}  (a, _) = t
  a
}}

fn main() {{ dbg(f()) }}
"
    )
}

/// Storing a value in a container adds it to the escape set; reading a field does not. The case
/// is a pair of modules, so the fact is compared by the variable's hint.
#[test]
fn escape_adds_a_fact_when_a_value_is_stored() {
    let local = mid(&escape_program(false));
    let escaped = mid(&escape_program(true));
    let local_f = function(&local, "f");
    let escaped_f = function(&escaped, "f");
    let t_local = variable(&local, "f", "t");
    let t_escaped = variable(&escaped, "f", "t");

    let local_set = &escape_sets(&local)[&local_f];
    let escaped_set = &escape_sets(&escaped)[&escaped_f];
    assert!(
        !local_set.contains(&t_local),
        "a tuple that is only read does not escape"
    );
    assert!(
        escaped_set.contains(&t_escaped),
        "a tuple stored in a list escapes"
    );
    let local_hints = hint_set(&local.functions[local_f], local_set);
    let escaped_hints = hint_set(&escaped.functions[escaped_f], escaped_set);
    assert!(
        local_hints.is_subset(&escaped_hints),
        "adding a store removed an escape fact: {local_hints:?} -> {escaped_hints:?}"
    );
}

/// A read-only parameter is borrowable. Adding a use that returns the value takes the reference,
/// so the fact only shrinks: `true` becomes `false`, never the other way round.
#[test]
fn borrow_true_only_shrinks_when_a_use_is_added() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (read_only, _) = fx.build("read_only", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    let (with_return, _) = fx.build("with_return", |b| {
        b.set_ret(pair);
        let x = b.param("x", pair);
        b.term(Term::Return(Some(x.into())));
    });

    let facts = borrow_facts(&fx.m);
    assert_eq!(facts.borrowed_params[&read_only], vec![true]);
    assert_eq!(facts.borrowed_params[&with_return], vec![false]);
    for (old, new) in facts.borrowed_params[&read_only]
        .iter()
        .zip(&facts.borrowed_params[&with_return])
    {
        assert!(
            !new || *old,
            "the extra use added a borrow fact: {old} -> {new}"
        );
    }
}

/// A fresh tuple is unique after its definition; storing it in a list takes the reference, and the
/// fact never comes back to `true` later in a straight-line body.
#[test]
fn ownership_true_only_shrinks_when_a_new_use_appears() {
    let m = heap(
        "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn main() {
  t = (1, 2)
  n = first(t)
  xs = [t]
  dbg(xs)
  dbg(n)
}
",
    );
    let f = &m.functions[function(&m, "main")];
    let t = variable(&m, "main", "t");
    let s_t = let_stmt(f, t);
    let s_xs = defining_let(&m, "main", "xs");
    let refs = ownership_facts(&m);
    let fid = function(&m, "main");

    assert_eq!(
        ownership::unique_after(&refs, fid, s_t, t),
        Fact::Known(true),
        "the tuple is fresh"
    );
    assert_eq!(
        ownership::unique_after(&refs, fid, s_xs, t),
        Fact::Known(false),
        "the list holds a second reference"
    );

    // The body is straight-line, so statement order is execution order: once the fact is false it
    // never turns true again for `t`.
    let body = f.body.expect("main has a body");
    let mut saw_false = false;
    for &s in &f.seqs[body].stmts {
        if !matches!(f.stmts[s], Stmt::Let { .. }) {
            continue;
        }
        match ownership::unique_after(&refs, fid, s, t) {
            Fact::Known(false) => saw_false = true,
            Fact::Known(true) => {
                assert!(!saw_false, "a unique fact came back after it was disproved");
            }
            Fact::Unknown => {}
        }
    }
    assert!(saw_false, "the test did not reach the storing use");
}

/// Builds a function whose `x` is returned (`extra_use = false`) or additionally read by the
/// marker statement (`extra_use = true`), and not returned.
fn liveness_module(extra_use: bool) -> (Module, FnId, Vec<StmtId>) {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "f");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    let y = b.var("y", TypeId::INT);
    let s_x = b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    let s_marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
    // The extra read comes after the marker, so `x` is live after the marker in the extended
    // build and dead there in the base build. (`live_after` excludes a statement's own
    // destination, so the read cannot be observed at `s_x`.)
    let used = if extra_use {
        Value::Prim {
            op: emela_lir::PrimOp::Add,
            ty: emela_lir::PrimTy::I32,
            args: vec![x.into(), int(0)],
        }
    } else {
        Value::Const(Const::Int(2))
    };
    let s_y = b.stmt(let_(Some(y), used));
    b.term(Term::Return(Some(y.into())));
    let f = b.finish();
    let mut m = Module::new(core);
    let id = m.add(f);
    m.stage = Stage::RefCounted;
    (m, id, vec![s_x, s_marker, s_y])
}

/// A new read of a variable only adds it to the live sets; it never makes a variable less live.
/// The two builds have the same statements in the same order, so the sets are compared position by
/// position.
#[test]
fn liveness_a_new_read_never_removes_a_live_variable() {
    let (base, base_f, statements) = liveness_module(false);
    let (extended, extended_f, _) = liveness_module(true);
    let (base_sets, extended_sets) = (live_sets(&base), live_sets(&extended));
    let base_function = &base.functions[base_f];
    let extended_function = &extended.functions[extended_f];

    for &s in &statements {
        let before = liveness::live_after(&base_sets, base_f, s)
            .map(|set| hint_set(base_function, set))
            .unwrap_or_default();
        let after = liveness::live_after(&extended_sets, extended_f, s)
            .map(|set| hint_set(extended_function, set))
            .unwrap_or_default();
        assert!(
            before.is_subset(&after),
            "a new read removed a live variable: {before:?} -> {after:?}"
        );
    }
    let marker = statements[1];
    assert!(
        !liveness::live_after(&base_sets, base_f, marker)
            .unwrap()
            .contains(&variable(&base, "f", "x")),
        "the base does not read `x` after the marker"
    );
    assert!(
        liveness::live_after(&extended_sets, extended_f, marker)
            .unwrap()
            .contains(&variable(&extended, "f", "x")),
        "the later read keeps `x` live after the marker"
    );
}

// ---------------------------------------------------------------------------------------------
// Soundness fixtures
// ---------------------------------------------------------------------------------------------

/// The known-good and known-bad escape fixtures in one lowered program: a parameter read field by
/// field and returned as a scalar does not escape; a returned, stored or captured parameter does.
#[test]
fn escape_marks_the_escaping_uses_and_not_a_scalar_read() {
    let m = mid("\
fn scalar(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn returned(x: (Int, Int)) -> (Int, Int) { x }

fn store(xs: List[(Int, Int)], x: (Int, Int)) -> List[(Int, Int)] {
  [x, ..xs]
}

fn capture(x: (Int, Int)) -> fn() -> Int {
  || {
    (a, _) = x
    a
  }
}

fn main() {
  dbg(scalar((1, 2)))
  u = returned((3, 4))
  dbg(u)
  xs = [(0, 0)]
  dbg(store(xs, (5, 6)))
  f = capture((7, 8))
  dbg(f())
}
");
    let summary = parameter_summary(&m);
    assert_eq!(summary[&function(&m, "scalar")], vec![false]);
    assert_eq!(summary[&function(&m, "returned")], vec![true]);
    assert_eq!(summary[&function(&m, "store")], vec![true, true]);
    assert_eq!(summary[&function(&m, "capture")], vec![true]);
}

/// A parameter that is live across a suspension point is saved into the heap frame, so it escapes
/// even though every other use is a read.
#[test]
fn escape_marks_a_parameter_saved_by_a_suspension() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (suspend, _) = fx.build("suspend_once", |b| {
        b.set_ret(TypeId::INT);
        b.term(Term::Return(Some(int(0))));
    });
    let (id, (x, suspension)) = fx.build("live", |b| {
        b.set_suspends();
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let _marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
        let suspension = b.stmt(Stmt::Let {
            dst: None,
            value: Value::Call {
                callee: Callee::Direct(suspend),
                args: Vec::new(),
                suspends: true,
                on_fail: None,
            },
            on_defect: None,
            on_suspend: Some(Suspend {
                state: 0,
                saved: vec![x],
            }),
        });
        // A read after the suspension, so `x` is live before it as well.
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
        (x, suspension)
    });

    assert_eq!(fx.params(id), vec![true]);
    assert!(
        escape_sets(&fx.m)[&id].contains(&x),
        "the frame saves the parameter"
    );
    // The liveness half of the cross-analysis rule: the saved variable is live before the
    // suspension point. Liveness is defined from `DefectsExplicit` on.
    fx.m.stage = Stage::StateMachines;
    let sets = live_sets(&fx.m);
    assert!(
        liveness::live_after(&sets, id, suspension)
            .expect("the suspension is a keyed let")
            .contains(&x),
        "the continuation reads `x` after the suspension"
    );
}

/// A read-only parameter is borrowable; one that is returned is not. The array and the value of
/// `Array.set` are not borrowable, while its index is.
#[test]
fn borrow_marks_only_the_reads_as_borrowable() {
    let mut fx = Fixture::new();
    let (pair, array) = (fx.pair, fx.array_pair);
    let (read_only, _) = fx.build("read_only", |b| {
        b.set_ret(TypeId::INT);
        let x = b.param("x", pair);
        let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
        b.term(Term::Return(Some(a.into())));
    });
    let (returned, _) = fx.build("returned", |b| {
        b.set_ret(pair);
        let x = b.param("x", pair);
        b.term(Term::Return(Some(x.into())));
    });
    let (set, _) = fx.build("set", |b| {
        b.set_ret(array);
        let a = b.param("a", array);
        let i = b.param("i", TypeId::INT);
        let v = b.param("v", pair);
        let r = b.assign(
            "r",
            array,
            Value::Builtin {
                op: emela_core::Builtin::ArraySet,
                args: vec![a.into(), i.into(), v.into()],
            },
        );
        b.term(Term::Return(Some(r.into())));
    });

    let facts = borrow_facts(&fx.m);
    assert_eq!(facts.borrowed_params[&read_only], vec![true]);
    assert_eq!(facts.borrowed_params[&returned], vec![false]);
    assert_eq!(
        facts.borrowed_params[&set],
        vec![false, true, false],
        "the array is written in place and the value is stored; only the index is read"
    );
}

/// A fresh tuple is unique until a consuming use; a call that borrows its argument leaves the
/// caller's reference alone.
#[test]
fn ownership_a_fresh_value_is_unique_until_a_consuming_use() {
    let m = heap(
        "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn take(x: (Int, Int)) -> (Int, Int) { x }

fn main() {
  t = (1, 2)
  n = first(t)
  u = take(t)
  dbg(n)
  dbg(u)
}
",
    );
    assert_eq!(unique_after(&m, "main", "t", "t"), Fact::Known(true));
    assert_eq!(
        unique_after(&m, "main", "n", "t"),
        Fact::Known(true),
        "`first` borrows its parameter"
    );
    assert_eq!(
        unique_after(&m, "main", "u", "t"),
        Fact::Known(false),
        "`take` consumes the value"
    );
}

/// A variable read only by a failure landing is live before the statement that may fail.
#[test]
fn liveness_a_failure_landing_keeps_its_variables_live() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "failure");
    b.set_ret(TypeId::INT);
    let callee = b.param("callee", TypeId::INT);
    let p = b.param("p", TypeId::INT);
    let error = b.param("error", TypeId::INT);
    let marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
    let landing = b.seq(|b| {
        b.stmt(let_(None, read(p)));
        b.term(Term::Unwind);
    });
    let call = b.stmt(Stmt::Let {
        dst: None,
        value: Value::Call {
            callee: Callee::Indirect(callee),
            args: Vec::new(),
            suspends: false,
            on_fail: Some(OnFail::Handle {
                error,
                body: landing,
            }),
        },
        on_defect: None,
        on_suspend: None,
    });
    b.term(Term::Return(Some(int(0))));
    let f = b.finish();
    let mut m = Module::new(core);
    let id = m.add(f);
    m.stage = Stage::RefCounted;

    let sets = live_sets(&m);
    for statement in [marker, call] {
        assert!(
            liveness::live_after(&sets, id, statement)
                .expect("a keyed let")
                .contains(&p),
            "the failure landing reads `p`"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// `Unknown` and the deopt of a consumer
// ---------------------------------------------------------------------------------------------

/// A module at [`Stage::Flow`] for the `Unknown` tests.
fn flow_module_with_dead_constant() -> Module {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "f");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    b.term(Term::Return(Some(int(0))));
    let f = b.finish();
    let mut m = Module::new(core);
    m.add(f);
    m.stage = Stage::Flow;
    m
}

/// Liveness answers `Unknown` before defects are explicit. DCE then leaves every function alone
/// and counts one deopt per function; it does not read `Unknown` as "not live".
#[test]
fn dce_deopts_and_changes_nothing_when_liveness_is_unknown() {
    let mut m = flow_module_with_dead_constant();
    assert!(
        liveness::analyze(&m).is_unknown(),
        "the test needs the Unknown stage"
    );
    let before = dump(&m);
    let stats = opt::dce::run(&mut m);
    assert_eq!(stats.dce, 0, "no dead statement may be removed");
    assert_eq!(
        stats.dce_deopts, 1,
        "the function the pass left alone is counted"
    );
    assert_eq!(dump(&m), before, "the module is unchanged");
}

/// `live_after` answers `None` for a statement that is not a keyed `let`; `None` is "no answer",
/// not an empty set. DCE is the consumer that has to check it.
#[test]
fn liveness_none_is_no_answer_not_an_empty_set() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "f");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    let let_x = b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    let dup = b.stmt(Stmt::Dup(x));
    b.term(Term::Return(Some(x.into())));
    let f = b.finish();
    let mut m = Module::new(core);
    let id = m.add(f);
    m.stage = Stage::RefCounted;

    let sets = live_sets(&m);
    assert!(
        liveness::live_after(&sets, id, let_x).is_some(),
        "a let is keyed"
    );
    assert!(
        liveness::live_after(&sets, id, dup).is_none(),
        "a dup is not a keyed statement: None, not an empty set"
    );
}

/// SRA runs at `Stage::ClosuresConverted`, where liveness and ownership are `Unknown`. It consumes
/// only the escape sets, and those are always `Known`; there is no `Unknown` path for this pass to
/// deopt on today. The test pins the current behaviour and the facts the pass does read, so a
/// later consumer (C4 reuse, C5 records) has to add the deopt.
#[test]
fn sra_runs_with_unknown_liveness_and_ownership_because_it_only_reads_escape() {
    let mut m = mid("\
fn f(a: Int) -> Int {
  t = (a, a + 1)
  (x, y) = t
  x + y
}

fn main() { dbg(f(1)) }
");
    assert!(
        liveness::analyze(&m).is_unknown(),
        "liveness is not defined at ClosuresConverted"
    );
    assert!(
        ownership::analyze(&m).is_unknown(),
        "ownership is not defined at ClosuresConverted"
    );
    assert!(
        escape::analyze(&m).is_known(),
        "the escape sets SRA reads are always known"
    );
    assert!(
        escape::parameter_summary(&m).is_known(),
        "the parameter summary is known here"
    );

    let before = dump(&m);
    let stats = opt::run_mid(&mut m, true);
    assert!(stats.sra_tuples > 0, "SRA replaces the non-escaping tuple");
    assert_ne!(dump(&m), before, "SRA changed the tuple it replaced");
    emela_lir::verify(&m).expect("SRA left valid LIR");
}

/// The Heap IR passes run at `Stage::RefCounted`, where all four analyses answer: a pass that
/// needs one of them does not start from `Unknown` in the real pipeline. This is the complement of
/// the deopt test above.
#[test]
fn the_heap_stage_has_every_fact_the_heap_passes_could_read() {
    let m = refcounted(
        "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn main() {
  t = (1, 2)
  n = first(t)
  dbg(n)
}
",
    );
    assert!(escape::analyze(&m).is_known());
    assert!(escape::parameter_summary(&m).is_known());
    assert!(liveness::analyze(&m).is_known());
    assert!(borrow::analyze(&m).is_known());
    assert!(ownership::analyze(&m).is_known());
}

/// A query that is not keyed is `Unknown`, and the ownership consumers treat it as "no answer": no
/// fact is "not unique", and no entry appears for a call result whose uniqueness was not proved.
#[test]
fn an_unkeyed_ownership_query_is_unknown_not_not_unique() {
    let m = heap(
        "\
fn make() -> (Int, Int) { (1, 2) }

fn main() {
  t = make()
  dbg(t)
}
",
    );
    let refs = ownership_facts(&m);
    let fid = function(&m, "main");
    let t = variable(&m, "main", "t");
    let s_t = let_stmt(&m.functions[fid], t);
    assert_eq!(
        ownership::unique_after(&refs, fid, s_t, t),
        Fact::Unknown,
        "a call result is owned but not proved unique"
    );
    let marker = m.functions[fid]
        .stmts
        .iter()
        .find(|(_, s)| matches!(s, Stmt::Dup(_) | Stmt::Drop(_)))
        .map(|(id, _)| id);
    if let Some(marker) = marker {
        assert_eq!(
            ownership::unique_after(&refs, fid, marker, t),
            Fact::Unknown,
            "a statement that is not a keyed let has no fact"
        );
    }
}

/// A malformed module (a function that is declared but neither defined nor external) makes the
/// analyses that resolve calls `Unknown`, while liveness and the escape sets stay `Known`. The
/// existing consumers do not read `Unknown` as a fact: DCE still uses liveness and does not count
/// a deopt, and neither SRA nor the `rc` cleanup consumes ownership or borrow.
#[test]
fn a_malformed_module_only_makes_the_call_resolving_analyses_unknown() {
    let mut m = Module::new(CoreModule::new());
    m.declare("declared_but_never_defined");
    m.stage = Stage::RefCounted;

    assert!(liveness::analyze(&m).is_known());
    assert!(escape::analyze(&m).is_known());
    assert!(escape::parameter_summary(&m).is_unknown());
    assert!(borrow::analyze(&m).is_unknown());
    assert!(ownership::analyze(&m).is_unknown());

    let before = dump(&m);
    let stats = opt::dce::run(&mut m);
    assert_eq!(stats.dce, 0);
    assert_eq!(
        stats.dce_deopts, 0,
        "DCE reads liveness, which is known, so it does not deopt here"
    );
    assert_eq!(dump(&m), before);
}

// ---------------------------------------------------------------------------------------------
// Consistency between the analyses
// ---------------------------------------------------------------------------------------------

/// Every parameter the borrow analysis proves borrowable is absent from the escape summary (the
/// environment parameter is skipped: it is owned by the caller's closure record, not by this
/// rule).
#[test]
fn a_borrowable_parameter_is_not_in_the_escape_summary() {
    let m = mid("\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn twice(x: (Int, Int)) -> Int {
  a = first(x)
  a
}

fn ignore(x: (Int, Int)) -> Int { 0 }

fn main() {
  dbg(twice((1, 2)))
  dbg(ignore((3, 4)))
}
");
    let summary = parameter_summary(&m);
    let borrows = borrow_facts(&m);
    let mut checked = 0;
    for (id, f) in m.functions.iter() {
        let borrowed = &borrows.borrowed_params[&id];
        let escaping = &summary[&id];
        for index in 0..f.params.len() {
            if index == 0 && f.env.is_some() {
                continue;
            }
            if borrowed[index] {
                assert!(
                    !escaping[index],
                    "`{}` parameter {index} is borrowable but escapes",
                    f.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "the fixture proved no parameter borrowable");
}

/// The documented contracts diverge on `Value::Coerce`: borrow lists it as a read, escape as a
/// use that may box a value into `Any` and cross the frame. The current behaviour is that a
/// coerced parameter is both borrowable and in the escape summary. This is a discrepancy against
/// the cross-analysis rule ("a borrowable parameter is not in the escape set"), reported with the
/// PR; the test pins it so a later change is visible.
#[test]
fn a_coerced_parameter_is_borrowable_and_escaping_today() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("coerce", |b| {
        b.set_ret(TypeId::UNIT);
        let x = b.param("x", pair);
        let _y = b.assign("y", TypeId::ANY, Value::Coerce(x));
        b.term(Term::Return(None));
    });
    assert_eq!(
        borrow_facts(&fx.m).borrowed_params[&id],
        vec![true],
        "the borrow contract lists `Coerce` as a read"
    );
    assert_eq!(
        fx.params(id),
        vec![true],
        "the escape summary treats `Coerce` as a use that may box the value"
    );
}

/// A borrowed argument stays uniquely owned at the call. The callee's parameter is proved
/// borrowable, and the caller's fresh tuple is `Known(true)` after the call; a consuming call
/// disproves it.
#[test]
fn a_borrowed_argument_is_still_uniquely_owned_at_the_call() {
    let m = heap(
        "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn take(x: (Int, Int)) -> (Int, Int) { x }

fn main() {
  t = (1, 2)
  n = first(t)
  u = take(t)
  dbg(n)
  dbg(u)
}
",
    );
    let facts = borrow_facts(&m);
    assert_eq!(
        facts.borrowed(first_fn(&m), 0),
        Fact::Known(true),
        "`first` only reads its parameter"
    );
    assert_eq!(
        facts.borrowed(take_fn(&m), 0),
        Fact::Known(false),
        "`take` returns its parameter"
    );
    assert_eq!(
        unique_after(&m, "main", "n", "t"),
        Fact::Known(true),
        "the borrowed argument is still the only owner"
    );
    assert_eq!(
        unique_after(&m, "main", "u", "t"),
        Fact::Known(false),
        "the consuming call takes the reference"
    );
}

fn first_fn(m: &Module) -> FnId {
    function(m, "first")
}

fn take_fn(m: &Module) -> FnId {
    function(m, "take")
}

// ---------------------------------------------------------------------------------------------
// Determinism and module order
// ---------------------------------------------------------------------------------------------

/// Each LIR analysis gives the same result on two runs.
#[test]
fn each_lir_analysis_gives_the_same_result_twice() {
    let m = refcounted(
        "\
fn first(x: (Int, Int)) -> Int {
  (a, _) = x
  a
}

fn make() -> (Int, Int) { (1, 2) }

fn main() {
  t = make()
  n = first(t)
  xs = [t]
  dbg(xs)
  dbg(n)
}
",
    );
    assert_eq!(escape::analyze(&m), escape::analyze(&m));
    assert_eq!(escape::parameter_summary(&m), escape::parameter_summary(&m));
    assert_eq!(liveness::analyze(&m), liveness::analyze(&m));
    assert_eq!(borrow::analyze(&m), borrow::analyze(&m));
    assert_eq!(ownership::analyze(&m), ownership::analyze(&m));
}

/// Builds two functions in the order given by `reverse`, at `Stage::StateMachines` (liveness and
/// ownership are defined, and no `dup`/`drop` complicates the facts). The functions are the same;
/// only their arena order differs.
fn ordered_module(reverse: bool) -> (Module, FnId, FnId) {
    let mut core = CoreModule::new();
    let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);

    let mut b = FnBuilder::new(&core, "read");
    b.set_ret(TypeId::INT);
    let x = b.param("x", pair);
    let a = b.assign("a", TypeId::INT, Value::TupleGet { base: x, index: 0 });
    b.term(Term::Return(Some(a.into())));
    let read = b.finish();

    let mut b = FnBuilder::new(&core, "make");
    b.set_ret(pair);
    let x = b.param("x", pair);
    b.term(Term::Return(Some(x.into())));
    let make = b.finish();

    let mut m = Module::new(core);
    let (read, make) = if reverse {
        let make = m.add(make);
        let read = m.add(read);
        (read, make)
    } else {
        let read = m.add(read);
        let make = m.add(make);
        (read, make)
    };
    m.stage = Stage::StateMachines;
    (m, read, make)
}

/// The live sets of a body, keyed by the statement's position, so two builds of the same function
/// can be compared without their `VarId`s.
fn live_by_position(m: &Module, f: FnId) -> BTreeMap<usize, BTreeSet<String>> {
    let sets = live_sets(m);
    let function = &m.functions[f];
    let body = function.body.expect("a body");
    function.seqs[body]
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(index, &statement)| {
            liveness::live_after(&sets, f, statement).map(|set| (index, hint_set(function, set)))
        })
        .collect()
}

/// The ownership facts of a body, keyed by statement position and variable hint.
fn ownership_by_position(m: &Module, f: FnId) -> BTreeMap<usize, BTreeMap<String, Fact<bool>>> {
    let refs = ownership_facts(m);
    let function = &m.functions[f];
    let body = function.body.expect("a body");
    function.seqs[body]
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(index, &statement)| {
            ownership::facts_after(&refs, f, statement).map(|facts| {
                let by_hint = facts
                    .iter()
                    .map(|(&v, &fact)| (function.vars[v].hint.clone(), fact))
                    .collect();
                (index, by_hint)
            })
        })
        .collect()
}

/// Two modules built with the functions in another arena order have the same per-name facts, and
/// every analysis keys the module in `m.functions` order: the order is the arena's, not a hash
/// map's.
#[test]
fn the_lir_analyses_follow_arena_order_not_hash_order() {
    let (forward, forward_read, forward_make) = ordered_module(false);
    let (backward, backward_read, backward_make) = ordered_module(true);
    let names =
        |m: &Module| -> Vec<String> { m.functions.iter().map(|(_, f)| f.name.clone()).collect() };
    assert_eq!(names(&forward), ["read", "make"]);
    assert_eq!(names(&backward), ["make", "read"]);
    assert_ne!(names(&forward), names(&backward));

    // Escape sets, by name, are the same.
    let escape_names = |m: &Module| -> BTreeMap<String, BTreeSet<String>> {
        let sets = escape_sets(m);
        m.functions
            .iter()
            .map(|(id, f)| (f.name.clone(), hint_set(f, &sets[&id])))
            .collect()
    };
    assert_eq!(escape_names(&forward), escape_names(&backward));

    // Borrow facts, by name.
    let borrow_names = |m: &Module| -> BTreeMap<String, Vec<bool>> {
        let facts = borrow_facts(m);
        m.functions
            .iter()
            .map(|(id, f)| (f.name.clone(), facts.borrowed_params[&id].clone()))
            .collect()
    };
    assert_eq!(borrow_names(&forward), borrow_names(&backward));

    // Parameter summaries, by name.
    let summary_names = |m: &Module| -> BTreeMap<String, Vec<bool>> {
        let summary = parameter_summary(m);
        m.functions
            .iter()
            .map(|(id, f)| (f.name.clone(), summary[&id].clone()))
            .collect()
    };
    assert_eq!(summary_names(&forward), summary_names(&backward));

    // Liveness and ownership, by name and statement position (the functions have the same shape).
    let live_names = |m: &Module, read: FnId, make: FnId| {
        BTreeMap::from([
            ("read".to_owned(), live_by_position(m, read)),
            ("make".to_owned(), live_by_position(m, make)),
        ])
    };
    assert_eq!(
        live_names(&forward, forward_read, forward_make),
        live_names(&backward, backward_read, backward_make)
    );
    let ownership_names = |m: &Module, read: FnId, make: FnId| {
        BTreeMap::from([
            ("read".to_owned(), ownership_by_position(m, read)),
            ("make".to_owned(), ownership_by_position(m, make)),
        ])
    };
    assert_eq!(
        ownership_names(&forward, forward_read, forward_make),
        ownership_names(&backward, backward_read, backward_make)
    );

    // Reading the outputs in arena order visits every function exactly once.
    for m in [&forward, &backward] {
        let sets = escape_sets(m);
        let summary = parameter_summary(m);
        let liveness = live_sets(m);
        let facts = borrow_facts(m);
        let refs = ownership_facts(m);
        for (id, f) in m.functions.iter() {
            assert!(sets.contains_key(&id));
            assert!(summary.contains_key(&id));
            assert!(liveness.contains_key(&id));
            assert!(facts.borrowed_params.contains_key(&id));
            assert_eq!(
                refs.contains_key(&id),
                f.body.is_some(),
                "ownership has facts exactly for the functions with a body"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Corpus smoke
// ---------------------------------------------------------------------------------------------

const CORPUS: &[(&str, &str)] = &[
    (
        "array",
        include_str!("../../../emela-codegen-c/tests/programs/array.emel"),
    ),
    (
        "closures",
        include_str!("../../../emela-codegen-c/tests/programs/closures.emel"),
    ),
    (
        "data",
        include_str!("../../../emela-codegen-c/tests/programs/data.emel"),
    ),
];

fn check_escape(m: &Module, sets: &EscapeSets, name: &str) {
    assert_eq!(
        sets.len(),
        m.functions.len(),
        "{name}: every function has an escape entry"
    );
    for (id, f) in m.functions.iter() {
        let set = &sets[&id];
        for v in set {
            assert!(
                (u32::from(v.into_raw()) as usize) < f.vars.len(),
                "{name}: the escape set names a variable outside its function"
            );
        }
    }
}

fn check_parameter_summary(m: &Module, summary: &ParameterSummary, name: &str) {
    assert_eq!(
        summary.len(),
        m.functions.len(),
        "{name}: every function has a parameter summary"
    );
    for (id, f) in m.functions.iter() {
        assert_eq!(
            summary[&id].len(),
            f.params.len(),
            "{name}: one flag per parameter of `{}`",
            f.name
        );
    }
}

fn check_liveness(m: &Module, sets: &LiveSets, name: &str) {
    assert_eq!(
        sets.len(),
        m.functions.len(),
        "{name}: every function has a live-set entry"
    );
    for (id, live) in sets {
        let f = &m.functions[*id];
        for (&statement, vars) in live {
            assert!(
                matches!(f.stmts[statement], Stmt::Let { .. }),
                "{name}: liveness is keyed by a non-let statement"
            );
            for v in vars {
                assert!(
                    (u32::from(v.into_raw()) as usize) < f.vars.len(),
                    "{name}: a live set names a variable outside its function"
                );
            }
        }
    }
}

fn check_borrow(m: &Module, facts: &BorrowFacts, name: &str) {
    assert_eq!(
        facts.borrowed_params.len(),
        m.functions.len(),
        "{name}: every function has borrow facts"
    );
    for (id, f) in m.functions.iter() {
        assert_eq!(
            facts.borrowed_params[&id].len(),
            f.params.len(),
            "{name}: one flag per parameter of `{}`",
            f.name
        );
    }
}

fn check_ownership(m: &Module, refs: &UniqueRefs, name: &str) {
    for (id, f) in m.functions.iter() {
        if f.body.is_none() {
            assert!(
                !refs.contains_key(&id),
                "{name}: an external function has no ownership facts"
            );
            continue;
        }
        let facts = &refs[&id];
        for (&statement, by_var) in facts {
            assert!(
                matches!(f.stmts[statement], Stmt::Let { .. }),
                "{name}: ownership is keyed by a non-let statement"
            );
            for v in by_var.keys() {
                assert!(
                    (u32::from(v.into_raw()) as usize) < f.vars.len(),
                    "{name}: an ownership fact names a variable outside its function"
                );
            }
        }
    }
}

/// Runs every LIR analysis on a handful of lowered codegen programs and checks that each map is
/// well formed: every key names a function, a `let` or a variable of the module, and no analysis
/// panics.
#[test]
fn the_lir_analyses_smoke_the_codegen_corpus() {
    for (name, source) in CORPUS {
        let m = refcounted(source);
        let sets = escape::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the escape sets are known"));
        check_escape(&m, &sets, name);
        let summary = escape::parameter_summary(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: the parameter summary is known"));
        check_parameter_summary(&m, &summary, name);
        let live = liveness::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: liveness is known at RefCounted"));
        check_liveness(&m, &live, name);
        let borrows = borrow::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: borrow facts are known"));
        check_borrow(&m, &borrows, name);
        let refs = ownership::analyze(&m)
            .known()
            .unwrap_or_else(|| panic!("{name}: ownership facts are known"));
        check_ownership(&m, &refs, name);
    }
}

/// The flags of the passes a test turns on explicitly, so a change of the default is visible.
#[test]
fn the_lir_rule_flags_are_the_ones_the_tests_assume() {
    let all = LirRules::all();
    assert!(all.sra && all.rc_cleanup && all.dce);
    let none = LirRules::none();
    assert!(!none.sra && !none.rc_cleanup && !none.dce);
    let default = Options::default();
    assert!(default.rules.sra && default.rules.rc_cleanup);
    assert!(default.rules.rc_borrow && default.rules.dce);
    assert!(default.rules.borrow && default.rules.switch);
    assert!(default.rules.devirt && default.rules.sra_closures);
    assert!(!default.rules.sra_records && !default.rules.drop_spec);
}

/// The fixture module compiles a plain function at a chosen stage; this pins that the helper the
/// hand-built tests use still produces a body (a regression here would make many tests vacuous).
#[test]
fn the_hand_built_fixture_has_a_body_and_a_keyed_let() {
    let mut fx = Fixture::new();
    let pair = fx.pair;
    let (id, _) = fx.build("f", |b| {
        b.set_ret(pair);
        let t = b.assign("t", pair, Value::Tuple(vec![int(1), int(2)]));
        b.term(Term::Return(Some(t.into())));
    });
    let f = &fx.m.functions[id];
    assert!(f.body.is_some());
    assert!(
        f.stmts.iter().any(|(_, s)| matches!(s, Stmt::Let { .. })),
        "the fixture has a keyed let"
    );
}
