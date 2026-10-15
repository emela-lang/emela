//! Tests for the verifier: one hand-built function per kind of error.

mod common;

use common::{Fixture, int, string};
use emela_core::{TypeId, VariantDef};
use emela_lir::{
    Callee, CaseKey, Closure, Const, ErrorKind, FnBuilder, Function, Module, OnFail, Operand,
    PrimOp, PrimTy, Repr, Stage, Stmt, SwitchOn, Term, Value, VarData, verify, verify_function,
};

fn module() -> (Fixture, Module) {
    let fx = Fixture::new();
    let m = Module::new(fx.core.clone());
    (fx, m)
}

fn kinds(m: &Module, f: &Function) -> Vec<ErrorKind> {
    verify_function(m, f).into_iter().map(|e| e.kind).collect()
}

fn add(a: impl Into<Operand>, b: impl Into<Operand>) -> Value {
    Value::Prim {
        op: PrimOp::Add,
        ty: PrimTy::I32,
        args: vec![a.into(), b.into()],
    }
}

/// `fn name(n: Int) -> Int { return n }`, with `flags` applied.
fn identity(m: &mut Module, name: &str, flags: impl FnOnce(&mut FnBuilder)) -> emela_lir::FnId {
    let mut b = FnBuilder::new(&m.core, name);
    b.set_ret(TypeId::INT);
    flags(&mut b);
    let n = b.param("n", TypeId::INT);
    b.term(Term::Return(Some(n.into())));
    let f = b.finish();
    m.add(f)
}

#[test]
fn a_valid_module_passes() {
    let (_, mut m) = module();
    identity(&mut m, "id", |_| {});
    assert_eq!(verify(&m), Ok(()));
}

#[test]
fn use_before_definition_and_outside_scope() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    b.set_ret(TypeId::INT);
    let c = b.param("c", TypeId::BOOL);
    let later = b.var("later", TypeId::INT);
    b.assign("early", TypeId::INT, add(later, int(1)));
    b.let_(Some(later), Value::Const(emela_lir::Const::Int(1)));
    let inner = b.var("inner", TypeId::INT);
    b.if_(
        None,
        c,
        |b| b.let_(Some(inner), Value::Const(emela_lir::Const::Int(2))),
        |_| {},
    );
    b.term(Term::Return(Some(inner.into())));
    let f = b.finish();
    let errors = verify_function(&m, &f);
    let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        messages,
        [
            "fn f: variable later%1 is used before its definition",
            "fn f: variable inner%3 is used outside its scope",
        ]
    );
    assert!(errors.iter().all(|e| e.kind == ErrorKind::UseOutOfScope));
}

#[test]
fn redefined() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let x = b.var("x", TypeId::INT);
    b.let_(Some(x), Value::Const(emela_lir::Const::Int(1)));
    b.let_(Some(x), Value::Const(emela_lir::Const::Int(2)));
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::Redefined]);
}

#[test]
fn uncaptured() {
    let (fx, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let prefix = b.param("prefix", TypeId::STRING);
    let print = b.var("print", fx.string_to_unit);
    let text = b.var("text", TypeId::STRING);
    // Uses `prefix` without listing it.
    b.closure(print, vec![text], vec![], false, |b| {
        b.let_(
            None,
            Value::Builtin {
                op: emela_core::Builtin::StringAppend,
                args: vec![prefix.into(), text.into()],
            },
        );
        b.term(Term::Return(None));
    });
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::Uncaptured]);
}

#[test]
fn bad_variables() {
    let (_, mut m) = module();
    let param = m.core.type_param(0);
    let mut b = FnBuilder::new(&m.core, "f");
    b.var("generic", param);
    b.term(Term::Return(None));
    let mut f = b.finish();
    f.vars.alloc(VarData {
        hint: "unit".into(),
        repr: Repr::Void,
        ty: TypeId::UNIT,
    });
    f.vars.alloc(VarData {
        hint: "wide".into(),
        repr: Repr::I64,
        ty: TypeId::INT,
    });
    assert_eq!(
        kinds(&m, &f),
        [
            ErrorKind::Polymorphic,
            ErrorKind::VoidVariable,
            ErrorKind::VarRepr
        ]
    );
}

#[test]
fn repr_mismatch() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    b.set_ret(TypeId::INT);
    let x = b.param("x", TypeId::FLOAT);
    // An i32 operation on an f64, and a destination of the wrong representation.
    b.assign("y", TypeId::INT, add(x, int(1)));
    b.assign("z", TypeId::INT, Value::Const(emela_lir::Const::Float(1.0)));
    b.term(Term::Return(Some(x.into())));
    let errors = verify_function(&m, &b.finish());
    let messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
    assert_eq!(
        messages,
        [
            "+[i32]: argument 0: expected i32, found f64",
            "destination z%2 is i32, but the value is f64",
            "return: expected i32, found f64",
        ]
    );
    assert!(errors.iter().all(|e| e.kind == ErrorKind::ReprMismatch));
}

#[test]
fn arity() {
    let (_, mut m) = module();
    let id = identity(&mut m, "id", |_| {});
    let mut b = FnBuilder::new(&m.core, "f");
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![int(1), int(2)],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::Arity]);
}

#[test]
fn void_destination() {
    let (_, mut m) = module();
    let mut b = FnBuilder::new(&m.core, "unit");
    b.term(Term::Return(None));
    let f = b.finish();
    let unit = m.add(f);
    let mut b = FnBuilder::new(&m.core, "f");
    b.assign(
        "x",
        TypeId::INT,
        Value::Call {
            callee: Callee::Direct(unit),
            args: vec![],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::VoidDestination]);
}

#[test]
fn closure_without_destination() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let body = b.seq(|b| b.term(Term::Return(None)));
    b.let_(
        None,
        Value::Closure(Box::new(Closure {
            params: vec![],
            captures: vec![],
            body,
            suspends: false,
        })),
    );
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::MissingDestination]);
}

#[test]
fn wrong_type_and_bad_index() {
    let (fx, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let n = b.param("n", TypeId::INT);
    let s = b.param("s", fx.shape_ty);
    b.assign("t", TypeId::INT, Value::Tag(n));
    b.assign(
        "w",
        TypeId::FLOAT,
        Value::Field {
            base: s,
            ctor: fx.ctor(fx.shape, 0),
            index: 5,
        },
    );
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::WrongType, ErrorKind::BadIndex]
    );
}

#[test]
fn bad_prim() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    b.assign(
        "s",
        TypeId::STRING,
        Value::Prim {
            op: PrimOp::Add,
            ty: PrimTy::String,
            args: vec![string("a"), string("b")],
        },
    );
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::BadPrim]);
}

#[test]
fn switch_errors() {
    let (fx, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let s = b.param("s", fx.shape_ty);
    let n = b.param("n", TypeId::INT);
    // A literal case in a tag switch, a duplicate constructor, and Rect missing without default.
    let a = b.seq(|_| {});
    let c = b.seq(|_| {});
    let d = b.seq(|_| {});
    b.switch(
        None,
        SwitchOn::Tag(s),
        vec![
            (CaseKey::Lit(emela_lir::Const::Int(0)), a),
            (CaseKey::Variant(0), c),
            (CaseKey::Variant(0), d),
        ],
        None,
    );
    // A literal switch without default.
    let e = b.seq(|_| {});
    b.switch(
        None,
        SwitchOn::Lit(n),
        vec![(CaseKey::Lit(emela_lir::Const::Int(0)), e)],
        None,
    );
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [
            ErrorKind::BadCase,
            ErrorKind::DuplicateCase,
            ErrorKind::NonExhaustiveSwitch,
            ErrorKind::NonExhaustiveSwitch,
        ]
    );
}

#[test]
fn labels() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    // A break to a label whose block does not enclose it.
    let elsewhere = b.block(None, "elsewhere", |_, _| {});
    b.block(None, "here", |b, _| {
        b.term(Term::Break {
            label: elsewhere,
            value: None,
        })
    });
    // A second block with the same label.
    let body = b.seq(|_| {});
    b.stmt(Stmt::Block {
        dst: None,
        label: elsewhere,
        body,
    });
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::UnknownLabel, ErrorKind::LabelRedefined]
    );
}

#[test]
fn break_value_mismatch() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let x = b.var("x", TypeId::INT);
    b.block(Some(x), "done", |b, done| {
        b.term(Term::Break {
            label: done,
            value: Some(string("no")),
        })
    });
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::ReprMismatch]);
}

#[test]
fn recur_outside_loop_or_across_protect() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let i = b.var("i", TypeId::INT);
    b.loop_(None, vec![(i, int(0))], |b| {
        b.protect(None, |b| b.term(Term::Recur(vec![int(1)])), |_| {});
        // The right arity, the wrong representation.
        b.term(Term::Recur(vec![string("one")]));
    });
    b.term(Term::Recur(vec![]));
    assert_eq!(
        kinds(&m, &b.finish()),
        [
            ErrorKind::RecurOutsideLoop,
            ErrorKind::ReprMismatch,
            ErrorKind::RecurOutsideLoop
        ]
    );
}

#[test]
fn missing_terminator() {
    let (fx, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let print = b.var("print", fx.string_to_unit);
    let text = b.var("text", TypeId::STRING);
    // The closure body and the function body both reach their ends.
    b.closure(print, vec![text], vec![], false, |_| {});
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::MissingTerminator, ErrorKind::MissingTerminator]
    );
}

#[test]
fn failing() {
    let (fx, mut m) = module();
    let fallible = identity(&mut m, "fallible", |b| {
        b.set_fails();
    });
    let id = identity(&mut m, "id", |_| {});
    let mut b = FnBuilder::new(&m.core, "f");
    // A fallible call without failure edge, an infallible one with one (which also fails a body
    // that cannot fail), and `fail`.
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(fallible),
            args: vec![int(1)],
            suspends: false,
            on_fail: None,
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![int(1)],
            suspends: false,
            on_fail: Some(OnFail::Propagate),
        },
    );
    let e = b.assign(
        "e",
        fx.parse_error_ty,
        Value::Make {
            ctor: fx.ctor(fx.parse_error, 0),
            args: vec![string("x")],
        },
    );
    b.term(Term::Fail(e.into()));
    assert_eq!(
        kinds(&m, &b.finish()),
        [
            ErrorKind::FailEdge,
            ErrorKind::FailEdge,
            ErrorKind::FailInInfallible,
            ErrorKind::FailInInfallible,
        ]
    );
}

#[test]
fn failure_edge_to_a_block_of_another_type() {
    let (_, mut m) = module();
    let fallible = identity(&mut m, "fallible", |b| {
        b.set_fails();
    });
    let mut b = FnBuilder::new(&m.core, "f");
    let x = b.var("x", TypeId::INT);
    b.block(Some(x), "catch", |b, catch| {
        b.let_(
            None,
            Value::Call {
                callee: Callee::Direct(fallible),
                args: vec![int(1)],
                suspends: false,
                on_fail: Some(OnFail::Break(catch)),
            },
        );
        b.term(Term::Unreachable);
    });
    b.term(Term::Return(None));
    let errors = verify_function(&m, &b.finish());
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].kind, ErrorKind::ReprMismatch);
    assert_eq!(
        errors[0].message,
        "failure edge to 'catch%0: expected ref, found i32"
    );
}

#[test]
fn escape_from_cleanup() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    b.block(None, "out", |b, out| {
        b.protect(
            None,
            |_| {},
            |b| {
                b.term(Term::Break {
                    label: out,
                    value: None,
                })
            },
        );
        b.protect(None, |_| {}, |b| b.term(Term::Return(None)));
    });
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::EscapeFromCleanup, ErrorKind::EscapeFromCleanup]
    );
}

#[test]
fn suspend_mismatch() {
    let (_, mut m) = module();
    let id = identity(&mut m, "id", |_| {});
    let mut b = FnBuilder::new(&m.core, "f");
    b.set_suspends();
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![int(1)],
            suspends: true,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::SuspendMismatch]);
}

#[test]
fn missing_body() {
    let (_, m) = module();
    assert_eq!(
        kinds(&m, &Function::new("declared")),
        [ErrorKind::MissingBody]
    );
}

#[test]
fn shared_node() {
    let (_, m) = module();
    let mut b = FnBuilder::new(&m.core, "f");
    let c = b.param("c", TypeId::BOOL);
    let arm = b.seq(|_| {});
    b.stmt(Stmt::If {
        dst: None,
        cond: c.into(),
        then: arm,
        else_: arm,
    });
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::SharedNode]);
}

#[test]
fn make_checks_fields_with_type_arguments() {
    let (fx, mut m) = module();
    // `type Box[A](value: A, tag: ())`: the `()` field takes the placeholder `()`, the `A` field
    // a value of the type argument's representation.
    let a = m.core.type_param(0);
    let boxed = m.core.add_enum(emela_core::EnumDef {
        name: "Box".into(),
        params: 1,
        variants: vec![VariantDef::positional("Box", vec![a, TypeId::UNIT])],
        error: false,
        external: false,
    });
    let box_float = m.core.enum_type(boxed, vec![TypeId::FLOAT]);
    let mut b = FnBuilder::new(&m.core, "f");
    b.assign(
        "ok",
        box_float,
        Value::Make {
            ctor: fx.ctor(boxed, 0),
            args: vec![
                Operand::Const(emela_lir::Const::Float(1.0)),
                Operand::Const(emela_lir::Const::Unit),
            ],
        },
    );
    b.assign(
        "bad",
        box_float,
        Value::Make {
            ctor: fx.ctor(boxed, 0),
            args: vec![int(1), Operand::Const(emela_lir::Const::Unit)],
        },
    );
    // The `()` field still has its slot.
    b.assign(
        "short",
        box_float,
        Value::Make {
            ctor: fx.ctor(boxed, 0),
            args: vec![Operand::Const(emela_lir::Const::Float(1.0))],
        },
    );
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::ReprMismatch, ErrorKind::Arity]
    );
}

#[test]
fn all_errors_of_all_functions_are_reported() {
    let (_, mut m) = module();
    let mut b = FnBuilder::new(&m.core, "one");
    b.set_ret(TypeId::INT);
    b.term(Term::Return(None));
    let f = b.finish();
    m.add(f);
    let mut b = FnBuilder::new(&m.core, "two");
    b.term(Term::Recur(vec![]));
    let f = b.finish();
    m.add(f);
    let errors = verify(&m).unwrap_err();
    let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
    assert_eq!(
        messages,
        [
            "fn one: return: expected i32, found void",
            "fn two: recur outside a loop of the same body, or across protect",
        ]
    );
}

#[test]
fn closures_and_the_stage() {
    let (fx, mut m) = module();
    // A nested closure and a function reference after closure conversion.
    let target = identity(&mut m, "id", |_| {});
    let mut b = FnBuilder::new(&m.core, "nested");
    let body = b.seq(|b| b.term(Term::Return(Some(int(1)))));
    let c = b.var("c", fx.thunk);
    b.let_(
        Some(c),
        Value::Closure(Box::new(Closure {
            params: vec![],
            captures: vec![],
            body,
            suspends: false,
        })),
    );
    b.assign("r", fx.thunk, Value::FnRef(target));
    b.term(Term::Return(None));
    let nested = b.finish();
    // A closure record of a function that is not lifted.
    let mut b = FnBuilder::new(&m.core, "record");
    b.assign(
        "c",
        fx.thunk,
        Value::MakeClosure {
            function: target,
            captures: vec![],
        },
    );
    b.term(Term::Return(None));
    let record = b.finish();
    assert_eq!(
        kinds(&m, &record),
        [ErrorKind::StageViolation, ErrorKind::BadEnvironment]
    );
    m.stage = Stage::ClosuresConverted;
    assert_eq!(
        kinds(&m, &nested),
        [ErrorKind::StageViolation, ErrorKind::StageViolation]
    );
    assert_eq!(kinds(&m, &record), [ErrorKind::BadEnvironment]);
}

#[test]
fn defects_and_the_stage() {
    let (fx, mut m) = module();
    let id = identity(&mut m, "id", |_| {});
    m.functions[id].defects = true;
    m.stage = Stage::DefectsExplicit;
    // Flow IR constructs: protect, a failure edge without landing, defect.
    let mut b = FnBuilder::new(&m.core, "flow");
    b.set_fails();
    let g = b.param("g", fx.int_to_int_fails);
    b.protect(None, |b| b.term(Term::Yield(None)), |_| {});
    let landing = b.seq(|b| b.term(Term::Unwind));
    b.stmt(Stmt::Let {
        dst: None,
        value: Value::Call {
            callee: Callee::Indirect(g),
            args: vec![int(1)],
            suspends: false,
            on_fail: Some(OnFail::Propagate),
        },
        on_defect: Some(landing),
    });
    b.term(Term::Defect(string("boom")));
    let mut f = b.finish();
    f.defects = true;
    assert_eq!(
        kinds(&m, &f),
        [
            ErrorKind::StageViolation,
            ErrorKind::StageViolation,
            ErrorKind::StageViolation
        ]
    );
    // A call that may raise a defect without landing; unwind in a function that does not.
    let mut b = FnBuilder::new(&m.core, "edges");
    b.let_(
        None,
        Value::Call {
            callee: Callee::Direct(id),
            args: vec![int(1)],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Unwind);
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::DefectEdge, ErrorKind::DefectEdge]
    );
    // A landing that yields, and one on a value that cannot raise a defect.
    let mut b = FnBuilder::new(&m.core, "landings");
    let yields = b.seq(|_| {});
    let unwinds = b.seq(|b| b.term(Term::Unwind));
    b.stmt(Stmt::Let {
        dst: None,
        value: Value::Call {
            callee: Callee::Direct(id),
            args: vec![int(1)],
            suspends: false,
            on_fail: None,
        },
        on_defect: Some(yields),
    });
    b.stmt(Stmt::Let {
        dst: None,
        value: Value::Const(Const::Int(1)),
        on_defect: Some(unwinds),
    });
    b.term(Term::Return(None));
    let mut f = b.finish();
    f.defects = true;
    assert_eq!(
        kinds(&m, &f),
        [ErrorKind::MissingTerminator, ErrorKind::DefectEdge]
    );
    // Landings are not allowed in the Flow IR.
    m.stage = Stage::Flow;
    assert_eq!(
        kinds(&m, &f),
        [
            ErrorKind::StageViolation,
            ErrorKind::MissingTerminator,
            ErrorKind::StageViolation,
            ErrorKind::StageViolation,
        ]
    );
}

#[test]
fn reference_counts() {
    let (_, mut m) = module();
    m.stage = Stage::RefCounted;
    // fn keep(s: String) -> String { return s }
    let mut b = FnBuilder::new(&m.core, "keep");
    b.set_ret(TypeId::STRING);
    let s = b.param("s", TypeId::STRING);
    b.term(Term::Return(Some(s.into())));
    let keep = b.finish();
    let keep = m.add(keep);
    assert_eq!(verify(&m), Ok(()));
    let call_keep = |s: emela_lir::VarId| Value::Call {
        callee: Callee::Direct(keep),
        args: vec![s.into()],
        suspends: false,
        on_fail: None,
    };
    // A leak.
    let mut b = FnBuilder::new(&m.core, "leak");
    b.param("s", TypeId::STRING);
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::RcImbalance]);
    // A second consumption, and a borrowed use after the last reference.
    let mut b = FnBuilder::new(&m.core, "twice");
    b.set_ret(TypeId::STRING);
    let s = b.param("s", TypeId::STRING);
    let a = b.assign("a", TypeId::STRING, call_keep(s));
    b.stmt(Stmt::Drop(a));
    b.assign(
        "n",
        TypeId::INT,
        Value::Builtin {
            op: emela_core::Builtin::StringLength,
            args: vec![s.into()],
        },
    );
    b.term(Term::Return(Some(s.into())));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::RcImbalance, ErrorKind::RcImbalance]
    );
    // Different references on the two edges into the end of an `if`.
    let mut b = FnBuilder::new(&m.core, "join");
    let c = b.param("c", TypeId::BOOL);
    let s = b.param("s", TypeId::STRING);
    b.if_(None, c, |b| b.stmt_drop(s), |_| {});
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::RcImbalance]);
    // A dup and drop before the stage.
    m.stage = Stage::DefectsExplicit;
    let mut b = FnBuilder::new(&m.core, "early");
    let s = b.param("s", TypeId::STRING);
    b.stmt(Stmt::Drop(s));
    b.term(Term::Return(None));
    assert_eq!(kinds(&m, &b.finish()), [ErrorKind::StageViolation]);
}

trait DropStmt {
    fn stmt_drop(&mut self, v: emela_lir::VarId);
}

impl DropStmt for FnBuilder<'_> {
    fn stmt_drop(&mut self, v: emela_lir::VarId) {
        self.stmt(Stmt::Drop(v));
    }
}

#[test]
fn environments() {
    let (fx, mut m) = module();
    m.stage = Stage::ClosuresConverted;
    // fn lifted(env: fn() -> Int) -> Int env[Int] { x = env_get env#0; return x }
    let mut b = FnBuilder::new(&m.core, "lifted");
    b.set_ret(TypeId::INT);
    let env = b.param("env", fx.thunk);
    let x = b.assign("x", TypeId::INT, Value::EnvGet { env, index: 0 });
    b.term(Term::Return(Some(x.into())));
    let mut f = b.finish();
    f.env = Some(vec![TypeId::INT]);
    let lifted = m.add(f);
    assert_eq!(verify(&m), Ok(()));
    // Captures of another type, and env_get outside a lifted function.
    let mut b = FnBuilder::new(&m.core, "user");
    let p = b.param("p", fx.thunk);
    let s = b.assign("s", TypeId::STRING, Value::Const(Const::String("a".into())));
    b.assign(
        "c",
        fx.thunk,
        Value::MakeClosure {
            function: lifted,
            captures: vec![s],
        },
    );
    b.assign("y", TypeId::INT, Value::EnvGet { env: p, index: 0 });
    b.term(Term::Return(None));
    assert_eq!(
        kinds(&m, &b.finish()),
        [ErrorKind::BadEnvironment, ErrorKind::BadEnvironment]
    );
    // A capture out of range.
    let mut b = FnBuilder::new(&m.core, "far");
    let env = b.param("env", fx.thunk);
    b.assign("y", TypeId::INT, Value::EnvGet { env, index: 1 });
    b.term(Term::Return(None));
    let mut f = b.finish();
    f.env = Some(vec![TypeId::INT]);
    assert_eq!(kinds(&m, &f), [ErrorKind::BadIndex]);
}
