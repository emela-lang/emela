//! Tests for the ownership analysis (`opt::analysis::ownership`): which references are provably
//! uniquely owned after a `let`, so that Perceus reuse (C4/PR-042) may reuse them.
//!
//! The first group lowers real programs to the Heap IR stage before reference counting; the second
//! builds functions by hand for the shapes lowering does not produce on demand (a suspension frame,
//! a second binding, a parameter).

use crate::support;

use emela_core::{Roots, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::Fact;
use emela_lir::opt::analysis::ownership::{self, UniqueRefs};
use emela_lir::{
    Const, FnBuilder, FnId, Module, Operand, Stage, Stmt, StmtId, Suspend, Term, Value, VarId,
    convert_closures, explicit_defects, from_core, state_machines,
};
use support::try_lower_with_opt;

/// Lowers a program to the Heap IR stage before reference counting: explicit defects and
/// suspension frames, no `dup`/`drop` yet.
fn heap(source: &str) -> Module {
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
    explicit_defects(&mut lir);
    state_machines(&mut lir);
    lir
}

fn function(m: &Module, name: &str) -> FnId {
    m.functions
        .iter()
        .find(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

fn facts(m: &Module) -> UniqueRefs {
    ownership::analyze(m)
        .known()
        .expect("a well-formed module at the Heap IR stage has ownership facts")
}

fn variable(m: &Module, name: &str, hint: &str) -> VarId {
    let id = function(m, name);
    m.functions[id]
        .vars
        .iter()
        .find(|(_, v)| v.hint == hint)
        .map(|(v, _)| v)
        .unwrap_or_else(|| panic!("no variable `{hint}` in `{name}`"))
}

/// The `let` whose destination is the variable named `hint`.
fn defining_let(m: &Module, name: &str, hint: &str) -> StmtId {
    let f = &m.functions[function(m, name)];
    let v = variable(m, name, hint);
    let_stmt(f, v)
}

fn let_stmt(f: &emela_lir::Function, v: VarId) -> StmtId {
    f.stmts
        .iter()
        .find(|(_, s)| matches!(s, Stmt::Let { dst: Some(d), .. } if *d == v))
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no let defines `{}`", f.vars[v].hint))
}

fn unique_after(m: &Module, name: &str, stmt: &str, var: &str) -> Fact<bool> {
    let refs = facts(m);
    let f = function(m, name);
    let s = defining_let(m, name, stmt);
    let v = variable(m, name, var);
    ownership::unique_after(&refs, f, s, v)
}

/// A module with the types the hand-built tests use.
struct Fixture {
    m: Module,
    pair: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = emela_core::Module::new();
        let pair = core.tuple(vec![TypeId::INT, TypeId::INT]);
        Fixture {
            m: Module::new(core),
            pair,
        }
    }

    /// A module holding `f`, with the fixture's Core types.
    fn module(&self, f: emela_lir::Function, stage: Stage) -> (Module, FnId) {
        let mut m = self.m.clone();
        let id = m.add(f);
        m.stage = stage;
        (m, id)
    }
}

fn tuple() -> Value {
    Value::Tuple(vec![
        Operand::Const(Const::Int(1)),
        Operand::Const(Const::Int(2)),
    ])
}

#[test]
fn a_fresh_tuple_is_unique_until_a_consuming_call() {
    let m = heap(
        "\
fn first(t: (Int, Int)) -> Int {
  (a, _) = t
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
        "`first` only reads its parameter, so the caller keeps its reference"
    );
    assert_eq!(
        unique_after(&m, "main", "u", "t"),
        Fact::Known(false),
        "`take` returns its parameter, so the call consumes the value"
    );
}

#[test]
fn a_fresh_value_stored_in_a_list_loses_its_uniqueness() {
    let m = heap(
        "\
fn main() {
  t = (1, 2)
  xs = [t]
  dbg(xs)
}
",
    );
    assert_eq!(unique_after(&m, "main", "t", "t"), Fact::Known(true));
    assert_eq!(
        unique_after(&m, "main", "xs", "t"),
        Fact::Known(false),
        "the list holds a reference to the tuple"
    );
    assert_eq!(
        unique_after(&m, "main", "xs", "xs"),
        Fact::Known(true),
        "the list itself is a fresh value"
    );
}

#[test]
fn a_captured_value_loses_its_uniqueness() {
    let m = heap(
        "\
fn main() {
  t = (1, 2)
  f = || {
    (a, b) = t
    a + b
  }
  dbg(f())
}
",
    );
    assert_eq!(unique_after(&m, "main", "t", "t"), Fact::Known(true));
    assert_eq!(
        unique_after(&m, "main", "f", "t"),
        Fact::Known(false),
        "the closure record holds the capture"
    );
    assert_eq!(
        unique_after(&m, "main", "f", "f"),
        Fact::Known(true),
        "the closure record itself is a fresh value"
    );
}

#[test]
fn a_view_of_a_local_is_not_unique() {
    let m = heap(
        "\
fn main() {
  t = (\"x\", 1)
  (s, n) = t
  dbg(s)
  dbg(n)
}
",
    );
    let f = &m.functions[function(&m, "main")];
    let refs = facts(&m);
    let fid = function(&m, "main");
    // The `TupleGet` that reads the String is a view; the one that reads the Int is not counted.
    let (view, _) = f
        .stmts
        .iter()
        .find(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    dst: Some(d),
                    value: Value::TupleGet { .. },
                    ..
                } if f.vars[*d].repr == emela_lir::Repr::Ref
            )
        })
        .expect("a String view");
    let Stmt::Let { dst: Some(d), .. } = f.stmts[view] else {
        unreachable!()
    };
    assert_eq!(
        ownership::unique_after(&refs, fid, view, d),
        Fact::Known(false),
        "a view shares its value with the base"
    );
    let t = variable(&m, "main", "t");
    let s_t = let_stmt(f, t);
    assert_eq!(
        ownership::unique_after(&refs, fid, s_t, t),
        Fact::Known(true),
        "the tuple itself is still uniquely owned"
    );
}

#[test]
fn an_env_view_is_borrowed() {
    let m = heap(
        "\
fn main() {
  t = (1, 2)
  f = || {
    (a, b) = t
    a + b
  }
  dbg(f())
}
",
    );
    let refs = facts(&m);
    let (id, f) = m
        .functions
        .iter()
        .find(|(_, f)| f.name.starts_with("main$"))
        .expect("the lifted closure");
    let (env_get, dst) = f
        .stmts
        .iter()
        .find_map(|(s, stmt)| match stmt {
            Stmt::Let {
                dst: Some(d),
                value: Value::EnvGet { .. },
                ..
            } => Some((s, *d)),
            _ => None,
        })
        .expect("the environment view");
    assert_eq!(
        ownership::unique_after(&refs, id, env_get, dst),
        Fact::Known(false),
        "the capture lives in the closure record the caller keeps alive"
    );
}

#[test]
fn a_branch_join_keeps_uniqueness_only_if_every_arm_does() {
    let m = heap(
        "\
fn main(c: Bool) {
  t = (1, 2)
  if c {
    xs = [t]
    dbg(xs)
    ()
  } else {
    dbg(t)
    ()
  }
  dbg(t)
}
",
    );
    assert_eq!(unique_after(&m, "main", "t", "t"), Fact::Known(true));
    // The last keyed `let` is after the branch (`t` is defined before it in the arena and the
    // statements of both arms are allocated before the `if` statement).
    let f = &m.functions[function(&m, "main")];
    let next = f
        .stmts
        .iter()
        .filter(|(_, s)| matches!(s, Stmt::Let { dst: Some(_), .. }))
        .max_by_key(|(id, _)| u32::from(id.into_raw()))
        .map(|(id, _)| id)
        .expect("a later let");
    let refs = facts(&m);
    assert_eq!(
        ownership::unique_after(&refs, function(&m, "main"), next, variable(&m, "main", "t")),
        Fact::Known(false),
        "one arm stores the tuple, so the join does not prove uniqueness"
    );
}

#[test]
fn a_parameter_and_a_call_result_are_unknown() {
    let m = heap(
        "\
fn make() -> (Int, Int) { (1, 2) }

fn main() {
  t = make()
  dbg(t)
}
",
    );
    assert_eq!(
        unique_after(&m, "main", "t", "t"),
        Fact::Unknown,
        "a call result is owned but its uniqueness is not proven"
    );
    let f = &m.functions[function(&m, "main")];
    let refs = facts(&m);
    let s_t = let_stmt(f, variable(&m, "main", "t"));
    assert!(
        ownership::facts_after(&refs, function(&m, "main"), s_t).is_none_or(|f| f.is_empty()),
        "no entry for the call result"
    );
}

#[test]
fn a_parameter_is_unknown_in_the_callee() {
    let fx = Fixture::new();
    let pair = fx.pair;
    let mut b = FnBuilder::new(&fx.m.core, "callee");
    b.set_ret(pair);
    let p = b.param("p", pair);
    let marker = b.var("marker", TypeId::INT);
    let marker_stmt = b.stmt(Stmt::Let {
        dst: Some(marker),
        value: Value::Const(Const::Int(0)),
        on_defect: None,
        on_suspend: None,
    });
    b.term(Term::Return(Some(p.into())));
    let (m, id) = fx.module(b.finish(), Stage::StateMachines);

    let refs = facts(&m);
    assert_eq!(
        ownership::unique_after(&refs, id, marker_stmt, p),
        Fact::Unknown,
        "an owned parameter may or may not have been duplicated at the call site"
    );
}

#[test]
fn a_value_saved_by_a_frame_is_not_unique() {
    let fx = Fixture::new();
    let pair = fx.pair;
    let mut b = FnBuilder::new(&fx.m.core, "suspender");
    b.set_suspends();
    b.set_ret(pair);
    let v = b.assign("v", pair, tuple());
    let s_v = let_stmt(b.function(), v);
    let suspend_stmt = b.stmt(Stmt::Let {
        dst: None,
        value: Value::Runtime {
            op: emela_lir::RuntimeOp::Yield,
            args: Vec::new(),
        },
        on_defect: None,
        on_suspend: Some(Suspend {
            state: 0,
            saved: vec![v],
        }),
    });
    b.term(Term::Return(Some(v.into())));
    let (m, id) = fx.module(b.finish(), Stage::StateMachines);

    let refs = facts(&m);
    assert_eq!(
        ownership::unique_after(&refs, id, s_v, v),
        Fact::Known(true),
        "the tuple is fresh before the suspension point"
    );
    assert_eq!(
        ownership::unique_after(&refs, id, suspend_stmt, v),
        Fact::Known(false),
        "the frame may keep the value after the function returned suspended"
    );
}

#[test]
fn a_value_aliased_by_a_second_binding_is_not_unique() {
    let fx = Fixture::new();
    let pair = fx.pair;
    let mut b = FnBuilder::new(&fx.m.core, "alias");
    b.set_ret(pair);
    let t = b.assign("t", pair, tuple());
    let s_t = let_stmt(b.function(), t);
    let y = b.assign("y", pair, Value::Coerce(t));
    let s_y = let_stmt(b.function(), y);
    b.term(Term::Return(Some(y.into())));
    let (m, id) = fx.module(b.finish(), Stage::StateMachines);

    let refs = facts(&m);
    assert_eq!(
        ownership::unique_after(&refs, id, s_t, t),
        Fact::Known(true)
    );
    assert_eq!(
        ownership::unique_after(&refs, id, s_y, t),
        Fact::Known(false),
        "the second binding shares the reference"
    );
    assert_eq!(
        ownership::unique_after(&refs, id, s_y, y),
        Fact::Known(false),
        "a view owns nothing of its own"
    );
}

#[test]
fn an_unkeyed_statement_is_unknown_not_not_unique() {
    let fx = Fixture::new();
    let pair = fx.pair;
    let mut b = FnBuilder::new(&fx.m.core, "f");
    b.set_ret(pair);
    let t = b.assign("t", pair, tuple());
    let marker = b.stmt(Stmt::Dup(t));
    b.term(Term::Return(Some(t.into())));
    let (m, id) = fx.module(b.finish(), Stage::StateMachines);

    let refs = facts(&m);
    assert_eq!(
        ownership::unique_after(&refs, id, marker, t),
        Fact::Unknown,
        "a statement without a destination is not keyed"
    );
}

#[test]
fn the_facts_are_deterministic() {
    let m = heap(
        "\
fn main() {
  t = (1, 2)
  xs = [t]
  dbg(xs)
}
",
    );
    assert_eq!(ownership::analyze(&m), ownership::analyze(&m));
}

#[test]
fn a_module_before_defects_are_explicit_has_no_facts() {
    let fx = Fixture::new();
    let pair = fx.pair;
    let mut b = FnBuilder::new(&fx.m.core, "f");
    b.set_ret(pair);
    let t = b.assign("t", pair, tuple());
    b.term(Term::Return(Some(t.into())));
    let (m, _) = fx.module(b.finish(), Stage::Flow);
    assert!(ownership::analyze(&m).is_unknown());
}
