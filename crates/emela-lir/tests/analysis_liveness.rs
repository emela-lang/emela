//! Tests for the shared liveness analysis (`opt::analysis::liveness`).
//!
//! The functions are built by hand so that a test can name the exact statement whose live set it
//! asks about. They use the shapes the Heap IR has: explicit defect landings, suspension edges and
//! `dup`/`drop` are handled by the same walk.

use emela_core::{Module as CoreModule, TypeId};
use emela_lir::opt::analysis::Fact;
use emela_lir::opt::analysis::liveness::{self, LiveSets};
use emela_lir::{
    CaseKey, Const, FnBuilder, FnId, Function, Module, PrimOp, PrimTy, Stage, Stmt, Suspend,
    SwitchOn, Term, Value, VarId,
};

/// A module with one function at `stage`.
fn module(f: Function, stage: Stage) -> (Module, FnId) {
    let mut m = Module::new(CoreModule::new());
    let id = m.add(f);
    m.stage = stage;
    (m, id)
}

/// The live sets of `m`, which must be `Known`.
fn known(m: &Module) -> LiveSets {
    match liveness::analyze(m) {
        Fact::Known(sets) => sets,
        Fact::Unknown => panic!("liveness is unknown at {:?}", m.stage),
    }
}

/// `value + 0`, a value that reads `value`.
fn read(value: VarId) -> Value {
    Value::Prim {
        op: PrimOp::Add,
        ty: PrimTy::I32,
        args: vec![value.into(), Const::Int(0).into()],
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

/// A variable read at the end is live after every earlier `let`; a variable defined and never read
/// is not live after its own definition.
#[test]
fn straight_line_liveness_reaches_the_end_and_kills_unused_definitions() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "straight");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    let dead = b.var("dead", TypeId::INT);
    let _sx = b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    let marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
    let sdead = b.stmt(let_(Some(dead), Value::Const(Const::Int(2))));
    b.term(Term::Return(Some(x.into())));
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    assert!(
        liveness::live_after(&sets, f, marker).unwrap().contains(&x),
        "a value returned at the end is live after the statements in between"
    );
    assert!(
        liveness::live_after(&sets, f, sdead).unwrap().contains(&x),
        "a value returned at the end is live after the later statements"
    );
    assert!(
        !liveness::live_after(&sets, f, sdead)
            .unwrap()
            .contains(&dead),
        "a value that is never read is not live after its only definition"
    );
}

/// A variable live in one arm of an `if` is live before the branch, and the join takes the union.
#[test]
fn an_if_join_takes_the_union_of_its_arms() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "branch");
    b.set_ret(TypeId::INT);
    let cond = b.param("cond", TypeId::BOOL);
    let then_only = b.var("then_only", TypeId::INT);
    let else_only = b.var("else_only", TypeId::INT);
    let result = b.var("result", TypeId::INT);
    let s_then = b.stmt(let_(Some(then_only), Value::Const(Const::Int(1))));
    let s_else = b.stmt(let_(Some(else_only), Value::Const(Const::Int(2))));
    let join = b.stmt(let_(None, Value::Const(Const::Int(0))));
    b.if_(
        Some(result),
        cond,
        |b| {
            let t = b.assign("t", TypeId::INT, read(then_only));
            b.term(Term::Yield(Some(t.into())));
        },
        |b| {
            let e = b.assign("e", TypeId::INT, read(else_only));
            b.term(Term::Yield(Some(e.into())));
        },
    );
    b.term(Term::Return(Some(result.into())));
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    let before = liveness::live_after(&sets, f, join).unwrap();
    assert!(before.contains(&then_only), "live in the then arm");
    assert!(before.contains(&else_only), "live in the else arm");
    assert!(
        before.contains(&cond),
        "the condition is read by the branch"
    );

    let after_then = liveness::live_after(&sets, f, s_then).unwrap();
    assert!(
        !after_then.contains(&else_only),
        "a variable defined in the other arm is not live before its definition"
    );
    assert!(
        liveness::live_after(&sets, f, s_else)
            .unwrap()
            .contains(&then_only),
        "the then arm's variable is still live at the else arm's definition"
    );
}

/// A variable live in any case of a `switch` (or in its default) is live before the switch.
#[test]
fn a_switch_join_takes_the_union_of_its_cases() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "switch");
    b.set_ret(TypeId::INT);
    let scrutinee = b.var("scrutinee", TypeId::INT);
    let case0 = b.var("case0", TypeId::INT);
    let case1 = b.var("case1", TypeId::INT);
    let result = b.var("result", TypeId::INT);
    let _s_scrutinee = b.stmt(let_(Some(scrutinee), Value::Const(Const::Int(0))));
    let _s_case0 = b.stmt(let_(Some(case0), Value::Const(Const::Int(1))));
    let s_case1 = b.stmt(let_(Some(case1), Value::Const(Const::Int(2))));
    let join = b.stmt(let_(None, Value::Const(Const::Int(0))));
    let body0 = b.seq(|b| {
        let v = b.assign("v0", TypeId::INT, read(case0));
        b.term(Term::Yield(Some(v.into())));
    });
    let body1 = b.seq(|b| {
        let v = b.assign("v1", TypeId::INT, read(case1));
        b.term(Term::Yield(Some(v.into())));
    });
    let default = b.seq(|b| b.term(Term::Yield(Some(case0.into()))));
    b.switch(
        Some(result),
        SwitchOn::Lit(scrutinee),
        vec![
            (CaseKey::Lit(Const::Int(0)), body0),
            (CaseKey::Lit(Const::Int(1)), body1),
        ],
        Some(default),
    );
    b.term(Term::Return(Some(result.into())));
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    let before = liveness::live_after(&sets, f, join).unwrap();
    assert!(before.contains(&case0), "live in a case and in the default");
    assert!(before.contains(&case1), "live in another case");
    assert!(before.contains(&scrutinee), "read by the switch");

    let after_case1 = liveness::live_after(&sets, f, s_case1).unwrap();
    assert!(
        after_case1.contains(&case0),
        "live in a case and the default"
    );
    assert!(
        !after_case1.contains(&case1),
        "a definition's destination is not live after it"
    );
}

/// A variable used again on the next iteration is live after the statements of the body that come
/// before the back edge; the loop fixpoint terminates. The loop variable is bound at the head, so
/// only an outer variable carried around the back edge is live before the loop.
#[test]
fn a_loop_carries_a_variable_around_its_back_edge_to_a_fixpoint() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "loop");
    b.set_ret(TypeId::UNIT);
    let x = b.var("x", TypeId::INT);
    let i = b.var("i", TypeId::INT);
    let n = b.var("n", TypeId::INT);
    let _sx = b.stmt(let_(Some(x), Value::Const(Const::Int(10))));
    let before_loop = b.stmt(let_(None, Value::Const(Const::Int(0))));
    let mut s_body = None;
    b.loop_(None, vec![(i, Const::Int(0).into())], |b| {
        s_body = Some(b.stmt(let_(Some(n), read(x))));
        b.term(Term::Recur(vec![i.into()]));
    });
    let s_body = s_body.expect("the loop body has a statement");
    b.term(Term::Return(None));
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    assert!(
        liveness::live_after(&sets, f, before_loop)
            .unwrap()
            .contains(&x),
        "the loop head needs `x` again"
    );
    let after_body = liveness::live_after(&sets, f, s_body).unwrap();
    assert!(
        after_body.contains(&x),
        "the next iteration reads `x` before defining it"
    );
    assert!(
        after_body.contains(&i),
        "the back edge reads the loop variable"
    );

    // The body below the first statement is also keyed: the fixpoint reached every statement.
    assert!(
        sets[&f].contains_key(&s_body),
        "the loop walk keyed the body statement"
    );
}

/// A variable read only by a defect landing is live before the statement that may defect.
#[test]
fn a_defect_landing_keeps_its_variables_live_before_the_statement() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "defect");
    b.set_ret(TypeId::INT);
    let landing_use = b.param("landing_use", TypeId::INT);
    let x = b.var("x", TypeId::INT);
    let y = b.var("y", TypeId::INT);
    let d = b.var("d", TypeId::INT);
    let before = b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    let landing = b.seq(|b| {
        b.stmt(let_(Some(y), read(landing_use)));
        b.term(Term::Unwind);
    });
    let may_defect = b.stmt(Stmt::Let {
        dst: Some(d),
        value: Value::Const(Const::Int(2)),
        on_defect: Some(landing),
        on_suspend: None,
    });
    b.term(Term::Return(Some(d.into())));
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    assert!(
        liveness::live_after(&sets, f, may_defect)
            .unwrap()
            .contains(&landing_use),
        "the landing reads the parameter"
    );
    assert!(
        liveness::live_after(&sets, f, before)
            .unwrap()
            .contains(&landing_use),
        "the landing use is live before the statement that may defect"
    );
    assert!(
        !liveness::live_after(&sets, f, may_defect)
            .unwrap()
            .contains(&d),
        "the destination is not live after its definition"
    );
}

/// A variable saved by a `Suspend` and read after it is live before the suspension point.
#[test]
fn a_suspension_keeps_a_saved_variable_live_before_the_suspension_point() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "suspend");
    b.set_ret(TypeId::INT);
    let v = b.var("v", TypeId::INT);
    let d = b.var("d", TypeId::INT);
    let u = b.var("u", TypeId::INT);
    let _before = b.stmt(let_(Some(v), Value::Const(Const::Int(1))));
    let before_suspension = b.stmt(let_(None, Value::Const(Const::Int(0))));
    let suspension = b.stmt(Stmt::Let {
        dst: Some(d),
        value: Value::Const(Const::Int(2)),
        on_defect: None,
        on_suspend: Some(Suspend {
            state: 0,
            saved: vec![v],
        }),
    });
    b.stmt(let_(Some(u), read(v)));
    b.term(Term::Return(Some(u.into())));
    let (m, f) = module(b.finish(), Stage::StateMachines);

    let sets = known(&m);
    assert!(
        liveness::live_after(&sets, f, suspension)
            .unwrap()
            .contains(&v),
        "the resumed continuation reads the saved variable"
    );
    assert!(
        liveness::live_after(&sets, f, before_suspension)
            .unwrap()
            .contains(&v),
        "the saved variable is live before the suspension point"
    );
}

/// Unreachable terminators do not panic the walk, and a use on the reachable arm still counts.
#[test]
fn unreachable_statements_are_handled_without_panicking() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "unreachable");
    b.set_ret(TypeId::INT);
    let cond = b.param("cond", TypeId::BOOL);
    let x = b.var("x", TypeId::INT);
    let _sx = b.stmt(let_(Some(x), Value::Const(Const::Int(3))));
    let marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
    b.if_(
        None,
        cond,
        |b| b.term(Term::Unreachable),
        |b| b.term(Term::Return(Some(x.into()))),
    );
    b.term(Term::Unreachable);
    let (m, f) = module(b.finish(), Stage::RefCounted);

    let sets = known(&m);
    assert!(
        liveness::live_after(&sets, f, marker).unwrap().contains(&x),
        "the reachable arm reads `x`"
    );
}

/// Before defects are explicit a `protect` cleanup can be reached through an exit the walk does
/// not model, so the analysis answers `Unknown` instead of guessing.
#[test]
fn flow_and_closure_stages_answer_unknown() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "flow");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    b.term(Term::Return(Some(x.into())));
    let f = b.finish();

    for stage in [Stage::Flow, Stage::ClosuresConverted] {
        let (m, _) = module(f.clone(), stage);
        assert!(
            liveness::analyze(&m).is_unknown(),
            "no liveness before defects are explicit ({stage:?})"
        );
    }
}

/// From `DefectsExplicit` on the statements are the Heap IR ones the walk handles, so the sets are
/// known even before `insert_rc` adds `dup`/`drop`.
#[test]
fn heap_stages_answer_known() {
    let core = CoreModule::new();
    let mut b = FnBuilder::new(&core, "heap");
    b.set_ret(TypeId::INT);
    let x = b.var("x", TypeId::INT);
    let _sx = b.stmt(let_(Some(x), Value::Const(Const::Int(1))));
    let marker = b.stmt(let_(None, Value::Const(Const::Int(0))));
    b.term(Term::Return(Some(x.into())));
    let f = b.finish();

    for stage in [
        Stage::DefectsExplicit,
        Stage::StateMachines,
        Stage::RefCounted,
    ] {
        let (m, id) = module(f.clone(), stage);
        let sets = known(&m);
        assert!(
            liveness::live_after(&sets, id, marker)
                .unwrap()
                .contains(&x),
            "the statement is keyed at {stage:?}"
        );
    }
}
