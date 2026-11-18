//! Tests for the local-base borrow elision of the `rc` cleanup
//! ([`LirRules::rc_borrow`]).
//!
//! Each test builds the Heap IR the reference counting pass emits (`dup` and `drop` explicit), runs
//! the Heap IR passes with the flag on or off, and checks the output with the verifier. With the
//! flag off nothing in this file may change: that is the default build's output.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Builtin, Roots, Type, TypeId, monomorphize_from, verify_monomorphic};
use emela_lir::opt::analysis::{liveness, ownership};
use emela_lir::opt::{LirRules, Options, Stats, run_heap_with};
use emela_lir::{
    Const, FnBuilder, FnId, Module, Stage, Stmt, Term, Value, dump_function, from_core,
    to_heap_with, verify,
};
use support::try_lower_with_opt;

/// A module with the types the tests build.
struct Fixture {
    m: Module,
    /// `List[Int]`.
    list: TypeId,
}

impl Fixture {
    fn new() -> Self {
        let mut core = emela_core::Module::new();
        let list = core.list(TypeId::INT);
        Fixture {
            m: Module::new(core),
            list,
        }
    }

    fn array(&mut self) -> TypeId {
        self.m.core.intern(Type::Array(TypeId::INT))
    }

    fn tuple(&mut self, elems: Vec<TypeId>) -> TypeId {
        self.m.core.tuple(elems)
    }

    fn build(&mut self, name: &str, f: impl FnOnce(&mut FnBuilder<'_>)) -> FnId {
        let mut b = FnBuilder::new(&self.m.core, name);
        f(&mut b);
        self.m.add(b.finish())
    }
}

/// The Heap IR passes with only the borrow elision switched on or off (SRA and the rest of the `rc`
/// cleanup as in a default build).
fn options(borrow: bool) -> Options {
    Options {
        rules: LirRules {
            rc_borrow: borrow,
            ..LirRules::default()
        },
    }
}

/// Runs the Heap IR passes at their precondition stage and verifies the result.
fn run(m: &mut Module, borrow: bool) -> Stats {
    m.stage = Stage::RefCounted;
    let stats = run_heap_with(m, options(borrow));
    if let Err(errors) = verify(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!(
            "the Heap IR passes produced invalid IR:\n{}",
            lines.join("\n")
        );
    }
    stats
}

fn text(m: &Module, id: FnId) -> String {
    dump_function(m, &m.functions[id])
}

/// The dump before the pass, at the pass's stage (the dump marks a function's `defects` and
/// `suspends` only from the stages where they are explicit).
fn snapshot(m: &mut Module, id: FnId) -> String {
    m.stage = Stage::RefCounted;
    text(m, id)
}

/// `root = [1, 2]; t = tail(root); dup t; h = head(t); drop t; e = is_empty(root); drop root`
///
/// The view `t` is used once and its root is used again after the view's last use, so reference
/// counting gives `t` a reference of its own and drops the root after the region. The root is a
/// fresh, uniquely owned local.
fn local_view(fx: &mut Fixture) -> FnId {
    let list = fx.list;
    fx.build("local", |b| {
        b.set_ret(TypeId::INT);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into(), Const::Int(2).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        let h = b.assign("h", TypeId::INT, Value::ListHead(t));
        b.stmt(Stmt::Drop(t));
        let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
        b.stmt(Stmt::Drop(root));
        b.term(Term::Return(Some(h.into())));
    })
}

/// The pair is removed with the flag on, and kept with it off.
#[test]
fn a_view_of_a_live_unique_local_is_elided_under_the_flag() {
    let mut fx = Fixture::new();
    let id = local_view(&mut fx);
    let before = snapshot(&mut fx.m, id);

    let with = run(&mut fx.m, true);
    assert_eq!(with.rc_borrow_pairs, 1, "{with:?}");
    assert_eq!(with.rc_pairs, 1, "{with:?}");
    let after = text(&fx.m, id);
    assert_eq!(after.matches("dup").count(), 0, "{after}");
    assert!(!after.contains("drop t"), "{after}");
    assert!(after.contains("drop root"), "{after}");
    assert_eq!(before.matches("dup").count(), 1, "{before}");
    assert!(before.contains("drop t"), "{before}");

    // The default rules keep the reference: the flag is off by default.
    let mut fx = Fixture::new();
    let id = local_view(&mut fx);
    let before = snapshot(&mut fx.m, id);
    let without = run(&mut fx.m, false);
    assert_eq!(without.rc_borrow_pairs, 0, "{without:?}");
    assert_eq!(without.rc_pairs, 0, "{without:?}");
    assert_eq!(text(&fx.m, id), before);
    assert!(text(&fx.m, id).contains("dup"));
}

/// The root is released before the view's last use: the reference the `dup` takes is the only thing
/// keeping the value alive, so the pair stays.
#[test]
fn a_root_dropped_before_the_paired_drop_deopts() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let id = fx.build("dropped", |b| {
        b.set_ret(TypeId::INT);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        // The root is dead after its view, which is exactly what reference counting emits.
        b.stmt(Stmt::Drop(root));
        let h = b.assign("h", TypeId::INT, Value::ListHead(t));
        b.stmt(Stmt::Drop(t));
        b.term(Term::Return(Some(h.into())));
    });
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 0, "{stats:?}");
    assert_eq!(text(&fx.m, id), before);
}

/// The drop is reached on one arm only: the arms disagree, so the pair stays.
#[test]
fn a_view_reached_on_one_branch_only_deopts() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let id = fx.build("branch", |b| {
        b.set_ret(TypeId::INT);
        let c = b.param("c", TypeId::BOOL);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        let r = b.var("r", TypeId::INT);
        b.if_(
            Some(r),
            c,
            |b| {
                // This arm gives the reference back and leaves.
                let h = b.assign("h", TypeId::INT, Value::ListHead(t));
                b.stmt(Stmt::Drop(t));
                b.stmt(Stmt::Drop(root));
                b.term(Term::Return(Some(h.into())));
            },
            // This one flows on to the drop after the `if`.
            |b| b.term(Term::Yield(Some(Const::Int(0).into()))),
        );
        b.stmt(Stmt::Drop(t));
        let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
        b.stmt(Stmt::Drop(root));
        b.term(Term::Return(Some(r.into())));
    });
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 0, "{stats:?}");
    assert_eq!(text(&fx.m, id), before);
}

/// A frame saves the base (and the view) across a suspension inside the region: the frame owns the
/// base for the caller, so the pair stays.
#[test]
fn a_frame_that_saves_the_root_deopts() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let id = fx.build("saved", |b| {
        b.set_suspends();
        b.set_ret(TypeId::INT);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        // The suspension edge of reference counting saves exactly the owned set: the view and its
        // root.
        let landing = b.seq(|b| {
            b.stmt(Stmt::Drop(root));
            b.stmt(Stmt::Drop(t));
            b.term(Term::Unwind);
        });
        b.stmt(Stmt::Let {
            dst: None,
            value: Value::Runtime {
                op: emela_lir::RuntimeOp::Yield,
                args: Vec::new(),
            },
            on_defect: Some(landing),
            on_suspend: Some(emela_lir::Suspend {
                state: 0,
                saved: vec![root, t],
            }),
        });
        let h = b.assign("h", TypeId::INT, Value::ListHead(t));
        b.stmt(Stmt::Drop(t));
        let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
        b.stmt(Stmt::Drop(root));
        b.term(Term::Return(Some(h.into())));
    });
    // `unwind` is a path of a function that may raise defects.
    fx.m.functions[id].defects = true;
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 0, "{stats:?}");
    assert_eq!(text(&fx.m, id), before);
}

/// A call consumes the view: the callee owns its argument under the calling convention, so
/// removing the `dup` would underflow it. Call-site borrow elision is C2, not this rule.
#[test]
fn a_call_that_consumes_the_view_deopts() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let g = fx.build("g", |b| {
        b.set_ret(TypeId::INT);
        let p = b.param("p", list);
        let n = b.assign("n", TypeId::INT, Value::ListIsEmpty(p));
        b.stmt(Stmt::Drop(p));
        b.term(Term::Return(Some(n.into())));
    });
    let id = fx.build("call", |b| {
        b.set_ret(TypeId::INT);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        let n = b.assign(
            "n",
            TypeId::INT,
            Value::Call {
                callee: emela_lir::Callee::Direct(g),
                args: vec![t.into()],
                suspends: false,
                on_fail: None,
            },
        );
        let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
        b.stmt(Stmt::Drop(root));
        b.term(Term::Return(Some(n.into())));
    });
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 0, "{stats:?}");
    assert_eq!(text(&fx.m, id), before);
}

/// `Array.set` consumes its array argument. The view is that argument, so the pair stays.
#[test]
fn array_set_on_the_view_deopts() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let array = fx.array();
    let pair = fx.tuple(vec![array, TypeId::INT]);
    let mut b = FnBuilder::new(&fx.m.core, "array");
    b.set_ret(TypeId::INT);
    let elems = b.assign(
        "elems",
        list,
        Value::List {
            elems: vec![Const::Int(1).into()],
            tail: None,
        },
    );
    let a = b.assign(
        "a",
        array,
        Value::Builtin {
            op: Builtin::ArrayFromList,
            args: vec![elems.into()],
        },
    );
    // A builtin borrows its arguments, so the list `elems` is still owned and released here.
    b.stmt(Stmt::Drop(elems));
    // A uniquely owned local base: an array field of a fresh tuple.
    let root = b.assign(
        "root",
        pair,
        Value::Tuple(vec![a.into(), Const::Int(0).into()]),
    );
    let v = b.assign(
        "v",
        array,
        Value::TupleGet {
            base: root,
            index: 0,
        },
    );
    b.stmt(Stmt::Dup(v));
    // `set` consumes the view; out of range raises a defect, so it has a landing.
    let landing = b.seq(|b| {
        b.stmt(Stmt::Drop(root));
        b.term(Term::Unwind);
    });
    let set = b.var("set", array);
    b.stmt(Stmt::Let {
        dst: Some(set),
        value: Value::Builtin {
            op: Builtin::ArraySet,
            args: vec![v.into(), Const::Int(0).into(), Const::Int(3).into()],
        },
        on_defect: Some(landing),
        on_suspend: None,
    });
    b.stmt(Stmt::Drop(set));
    let i = b.assign(
        "i",
        TypeId::INT,
        Value::TupleGet {
            base: root,
            index: 1,
        },
    );
    b.stmt(Stmt::Drop(root));
    b.term(Term::Return(Some(i.into())));
    let mut f = b.finish();
    f.defects = true;
    let id = fx.m.add(f);
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 0, "{stats:?}");
    assert_eq!(text(&fx.m, id), before);
}

/// The releases the pass does not remove keep their order; only the pair's drop goes.
#[test]
fn the_remaining_drops_keep_their_order() {
    let mut fx = Fixture::new();
    let list = fx.list;
    let id = fx.build("order", |b| {
        b.set_ret(TypeId::INT);
        let q = b.param("q", TypeId::STRING);
        let r = b.param("r", TypeId::STRING);
        let root = b.assign(
            "root",
            list,
            Value::List {
                elems: vec![Const::Int(1).into()],
                tail: None,
            },
        );
        let t = b.assign("t", list, Value::ListTail(root));
        b.stmt(Stmt::Dup(t));
        let h = b.assign("h", TypeId::INT, Value::ListHead(t));
        b.stmt(Stmt::Drop(t));
        b.stmt(Stmt::Drop(q));
        b.stmt(Stmt::Drop(r));
        let _e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(root));
        b.stmt(Stmt::Drop(root));
        b.term(Term::Return(Some(h.into())));
    });
    let order = |text: &str| {
        text.lines()
            .filter(|l| l.trim_start().starts_with("drop"))
            .map(|l| l.trim().split('%').next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    let before = snapshot(&mut fx.m, id);
    let stats = run(&mut fx.m, true);
    assert_eq!(stats.rc_borrow_pairs, 1, "{stats:?}");
    let after = text(&fx.m, id);
    assert_eq!(
        order(&before),
        ["drop t", "drop q", "drop r", "drop root"],
        "{before}"
    );
    assert_eq!(order(&after), ["drop q", "drop r", "drop root"], "{after}");
}

/// A second run removes nothing.
#[test]
fn the_borrow_elision_is_idempotent() {
    let mut fx = Fixture::new();
    let id = local_view(&mut fx);
    assert_eq!(run(&mut fx.m, true).rc_borrow_pairs, 1);
    let after_first = text(&fx.m, id);
    let second = run(&mut fx.m, true);
    assert_eq!(second.rc_borrow_pairs, 0, "{second:?}");
    assert_eq!(second.rc_pairs, 0, "{second:?}");
    assert_eq!(text(&fx.m, id), after_first);
}

/// The rule needs the liveness and ownership facts, which do not exist before the Heap IR stage.
#[test]
fn the_facts_the_rule_needs_are_unknown_at_the_flow_stage() {
    let mut fx = Fixture::new();
    local_view(&mut fx);
    assert_eq!(fx.m.stage, Stage::Flow);
    assert!(
        liveness::analyze(&fx.m).is_unknown(),
        "liveness answered before the Heap IR stage"
    );
    assert!(
        ownership::analyze(&fx.m).is_unknown(),
        "ownership answered before the Heap IR stage"
    );
    // At the stage of the pass both answer for the function, which is what the proof reads. A
    // missing fact deopts the pair; `opt::rc`'s unit tests cover that case.
    fx.m.stage = Stage::RefCounted;
    assert!(liveness::analyze(&fx.m).is_known());
    assert!(ownership::analyze(&fx.m).is_known());
}

/// Lowers a program and takes it to the Heap IR with the default passes (the borrow elision off).
fn lowered(src: &str) -> Module {
    let m = try_lower_with_opt(
        &[("main.emel", src)],
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
    to_heap_with(&mut lir, true);
    lir
}

/// The folded program reaches the shape: a pattern match binds the tail of a fresh local list as a
/// view, uses it twice (so reference counting gives it a reference), and reads the list again
/// after the view's last use. The view's root is unique and alive, so the flag removes the pair in
/// the function the verifier accepts.
#[test]
fn a_lowered_match_elides_the_view_of_its_local_list() {
    let mut m = lowered(
        "\
fn main() {
  xs = [1, 2, 3]
  n = xs match {
    [_, ..rest] -> {
      rest match {
        [] -> 0
        [_, ..] -> 1
      }
    }
    [] -> 0
  }
  dbg(n + List.length(xs))
}
",
    );
    let id = m
        .functions
        .iter()
        .find(|(_, f)| f.name == "main")
        .map(|(id, _)| id)
        .expect("main");
    let before = text(&m, id);
    assert!(before.contains("dup"), "{before}");
    let stats = run(&mut m, true);
    assert!(stats.rc_borrow_pairs >= 1, "{stats:?}");
    let after = text(&m, id);
    assert!(!after.contains("dup"), "{after}");
    assert!(after.contains("drop"), "{after}");
}
