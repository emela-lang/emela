//! Tests for how the LIR dump looks, on hand-built functions that use every construct. Each module
//! also passes the verifier.

mod common;

use common::{Fixture, int, string};
use emela_core::{Builtin, External, ExternalBinding, FiberOp, HostModule, HostRef, Type, TypeId};
use emela_lir::{
    CaseKey, Const, FnBuilder, Module, OnFail, Operand, PrimOp, PrimTy, Repr, SwitchOn, Term,
    Value, dump, repr_of, verify,
};

fn prim(op: PrimOp, ty: PrimTy, args: Vec<Operand>) -> Value {
    Value::Prim { op, ty, args }
}

fn checked_dump(m: &Module) -> String {
    if let Err(errors) = verify(m) {
        let messages: Vec<String> = errors.iter().map(|e| e.to_string()).collect();
        eprintln!("verifier errors:\n{}", messages.join("\n"));
        panic!("verifier errors");
    }
    dump(m)
}

/// Control: `if`, `switch` on tags and literals, `loop` / `recur`, `block` / `break`, `protect`,
/// and every terminator.
#[test]
fn control() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());

    // export fn fact(n: Int) -> Int, loopified.
    let mut b = FnBuilder::new(&m.core, "fact");
    b.set_exported().set_ret(TypeId::INT);
    let n = b.param("n", TypeId::INT);
    let i = b.var("i", TypeId::INT);
    let acc = b.var("acc", TypeId::INT);
    let r = b.var("r", TypeId::INT);
    b.loop_(Some(r), vec![(i, n.into()), (acc, int(1))], |b| {
        let z = b.assign(
            "z",
            TypeId::BOOL,
            prim(PrimOp::Eq, PrimTy::I32, vec![i.into(), int(0)]),
        );
        let v = b.var("v", TypeId::INT);
        b.if_(
            Some(v),
            z,
            |b| b.term(Term::Yield(Some(acc.into()))),
            |b| {
                let j = b.assign(
                    "j",
                    TypeId::INT,
                    prim(PrimOp::Sub, PrimTy::I32, vec![i.into(), int(1)]),
                );
                let a = b.assign(
                    "a",
                    TypeId::INT,
                    prim(PrimOp::Mul, PrimTy::I32, vec![acc.into(), i.into()]),
                );
                b.term(Term::Recur(vec![j.into(), a.into()]));
            },
        );
        b.term(Term::Yield(Some(v.into())));
    });
    b.term(Term::Return(Some(r.into())));
    m.add(b.finish());

    // fn area(s: Shape) -> Float: a tag switch without default, with fields.
    let mut b = FnBuilder::new(&m.core, "area");
    b.set_ret(TypeId::FLOAT);
    let s = b.param("s", fx.shape_ty);
    let circle = b.seq(|b| {
        let r = b.assign(
            "r",
            TypeId::FLOAT,
            Value::Field {
                base: s,
                ctor: fx.ctor(fx.shape, 0),
                index: 0,
            },
        );
        let a = b.assign(
            "a",
            TypeId::FLOAT,
            prim(PrimOp::Mul, PrimTy::F64, vec![r.into(), r.into()]),
        );
        b.term(Term::Return(Some(a.into())));
    });
    let rect = b.seq(|b| {
        let w = b.assign(
            "w",
            TypeId::FLOAT,
            Value::Field {
                base: s,
                ctor: fx.ctor(fx.shape, 1),
                index: 0,
            },
        );
        let h = b.assign(
            "h",
            TypeId::FLOAT,
            Value::Field {
                base: s,
                ctor: fx.ctor(fx.shape, 1),
                index: 1,
            },
        );
        let a = b.assign(
            "a",
            TypeId::FLOAT,
            prim(PrimOp::Mul, PrimTy::F64, vec![w.into(), h.into()]),
        );
        b.term(Term::Return(Some(a.into())));
    });
    b.switch(
        None,
        SwitchOn::Tag(s),
        vec![(CaseKey::Variant(0), circle), (CaseKey::Variant(1), rect)],
        None,
    );
    b.term(Term::Unreachable);
    m.add(b.finish());

    // fn code(n: Int) -> Color: a literal switch with a default defect, and nullary constructors.
    let mut b = FnBuilder::new(&m.core, "code");
    b.set_ret(fx.color_ty);
    let n = b.param("n", TypeId::INT);
    let c = b.var("c", fx.color_ty);
    let red = b.seq(|b| {
        let v = b.assign(
            "v",
            fx.color_ty,
            Value::Make {
                ctor: fx.ctor(fx.color, 0),
                args: vec![],
            },
        );
        b.term(Term::Yield(Some(v.into())));
    });
    let blue = b.seq(|b| {
        let v = b.assign(
            "v",
            fx.color_ty,
            Value::Make {
                ctor: fx.ctor(fx.color, 2),
                args: vec![],
            },
        );
        b.term(Term::Yield(Some(v.into())));
    });
    let other = b.seq(|b| b.term(Term::Defect(string("no color"), None)));
    b.switch(
        Some(c),
        SwitchOn::Lit(n),
        vec![
            (CaseKey::Lit(Const::Int(0)), red),
            (CaseKey::Lit(Const::Int(2)), blue),
        ],
        Some(other),
    );
    // A tag switch on an i32 enum, with a default.
    let t = b.var("t", fx.color_ty);
    let green = b.seq(|b| {
        let v = b.assign(
            "v",
            fx.color_ty,
            Value::Make {
                ctor: fx.ctor(fx.color, 1),
                args: vec![],
            },
        );
        b.term(Term::Yield(Some(v.into())));
    });
    let same = b.seq(|b| b.term(Term::Yield(Some(c.into()))));
    b.switch(
        Some(t),
        SwitchOn::Tag(c),
        vec![(CaseKey::Variant(0), green)],
        Some(same),
    );
    let k = b.assign("k", TypeId::INT, Value::Tag(t));
    let big = b.assign(
        "big",
        TypeId::BOOL,
        prim(PrimOp::Gt, PrimTy::I32, vec![k.into(), int(1)]),
    );
    b.if_(None, big, |b| b.term(Term::Return(Some(t.into()))), |_| {});
    b.term(Term::Return(Some(c.into())));
    m.add(b.finish());

    // fn first_positive(xs: List[Int]) -> Int: list operations, block / break, protect.
    let mut b = FnBuilder::new(&m.core, "first_positive");
    b.set_ret(TypeId::INT);
    let xs = b.param("xs", fx.ints);
    let found = b.var("found", TypeId::INT);
    b.block(Some(found), "found", |b, done| {
        let l = b.var("l", fx.ints);
        b.loop_(None, vec![(l, xs.into())], |b| {
            let e = b.assign("e", TypeId::BOOL, Value::ListIsEmpty(l));
            b.if_(
                None,
                e,
                |b| {
                    b.term(Term::Break {
                        label: done,
                        value: Some(int(0)),
                    })
                },
                |_| {},
            );
            let x = b.assign("x", TypeId::INT, Value::ListHead(l));
            let p = b.assign(
                "p",
                TypeId::BOOL,
                prim(PrimOp::Gt, PrimTy::I32, vec![x.into(), int(0)]),
            );
            b.if_(
                None,
                p,
                |b| {
                    b.term(Term::Break {
                        label: done,
                        value: Some(x.into()),
                    })
                },
                |_| {},
            );
            let rest = b.assign("rest", fx.ints, Value::ListTail(l));
            b.term(Term::Recur(vec![rest.into()]));
        });
        b.term(Term::Unreachable);
    });
    let out = b.var("out", TypeId::INT);
    b.protect(
        Some(out),
        |b| {
            let y = b.assign(
                "y",
                TypeId::INT,
                prim(PrimOp::Neg, PrimTy::I32, vec![found.into()]),
            );
            b.term(Term::Yield(Some(y.into())));
        },
        |b| {
            let msg = b.assign(
                "msg",
                TypeId::STRING,
                Value::Builtin {
                    op: Builtin::IntToString,
                    args: vec![found.into()],
                },
            );
            b.let_(
                None,
                Value::Builtin {
                    op: Builtin::Dbg,
                    args: vec![msg.into()],
                },
            );
        },
    );
    b.term(Term::Return(Some(out.into())));
    m.add(b.finish());

    insta::assert_snapshot!(checked_dump(&m));
}

/// Data and calls: constants, primitive operations, enum values, tuples, lists, failure edges,
/// external and const functions.
#[test]
fn data_and_calls() {
    let fx = Fixture::new();
    let mut m = Module::new(fx.core.clone());
    let parse = m.declare("parse");
    let checked = m.declare("checked");

    // external fn parse(s: String) -> Int fails
    let mut b = FnBuilder::new(&m.core, "parse");
    b.set_ret(TypeId::INT).set_fails();
    b.param("s", TypeId::STRING);
    m.define(
        parse,
        b.finish_external(External {
            bindings: vec![ExternalBinding {
                target: "js".into(),
                host: HostRef::Import {
                    module: HostModule::Bundled("std/parse.mjs".into()),
                    name: "parse".into(),
                },
            }],
        }),
    );

    // fn checked(n: Int) -> Int fails: `fail` with an error value.
    let mut b = FnBuilder::new(&m.core, "checked");
    b.set_ret(TypeId::INT).set_fails();
    let n = b.param("n", TypeId::INT);
    let neg = b.assign(
        "neg",
        TypeId::BOOL,
        prim(PrimOp::Lt, PrimTy::I32, vec![n.into(), int(0)]),
    );
    b.if_(
        None,
        neg,
        |b| {
            let e = b.assign(
                "e",
                fx.parse_error_ty,
                Value::Make {
                    ctor: fx.ctor(fx.parse_error, 0),
                    args: vec![string("negative")],
                },
            );
            b.term(Term::Fail(e.into()));
        },
        |_| {},
    );
    b.term(Term::Return(Some(n.into())));
    m.define(checked, b.finish());

    // fn parse_or(s: String, default: Int) -> Int: failure edges to a block and outward.
    let mut b = FnBuilder::new(&m.core, "parse_or");
    b.set_ret(TypeId::INT).set_fails();
    let s = b.param("s", TypeId::STRING);
    let default = b.param("default", TypeId::INT);
    let err = b.var("err", fx.parse_error_ty);
    b.block(Some(err), "catch", |b, catch| {
        let x = b.var("x", TypeId::INT);
        b.let_(
            Some(x),
            Value::Call {
                callee: emela_lir::Callee::Direct(parse),
                args: vec![s.into()],
                suspends: false,
                on_fail: Some(OnFail::Break(catch)),
            },
        );
        let y = b.var("y", TypeId::INT);
        b.let_(
            Some(y),
            Value::Call {
                callee: emela_lir::Callee::Direct(checked),
                args: vec![x.into()],
                suspends: false,
                on_fail: Some(OnFail::Propagate),
            },
        );
        b.term(Term::Return(Some(y.into())));
    });
    let input = b.assign(
        "input",
        TypeId::STRING,
        Value::Field {
            base: err,
            ctor: fx.ctor(fx.parse_error, 0),
            index: 0,
        },
    );
    let empty = b.assign(
        "empty",
        TypeId::BOOL,
        prim(PrimOp::Eq, PrimTy::String, vec![input.into(), string("")]),
    );
    let not_empty = b.assign(
        "not_empty",
        TypeId::BOOL,
        prim(PrimOp::Not, PrimTy::I32, vec![empty.into()]),
    );
    b.if_(
        None,
        not_empty,
        |b| b.term(Term::Return(Some(default.into()))),
        |_| {},
    );
    b.term(Term::Return(Some(int(0))));
    m.add(b.finish());

    // fn values() -> (Int, String): constants of every kind, tuples, lists, Option.
    let mut b = FnBuilder::new(&m.core, "values");
    b.set_ret(fx.pair);
    let big = b.assign(
        "big",
        TypeId::INT64,
        prim(
            PrimOp::Add,
            PrimTy::I64,
            vec![
                Operand::Const(Const::Int64(1 << 40)),
                Operand::Const(Const::Int64(-1)),
            ],
        ),
    );
    let half = b.assign("half", TypeId::FLOAT, Value::Const(Const::Float(0.5)));
    let neg = b.assign(
        "neg",
        TypeId::FLOAT,
        prim(PrimOp::Neg, PrimTy::F64, vec![half.into()]),
    );
    let yes = b.assign("yes", TypeId::BOOL, Value::Const(Const::Bool(true)));
    let some = b.assign(
        "some",
        fx.option_int,
        Value::Make {
            ctor: fx.some,
            args: vec![int(3)],
        },
    );
    let none = b.assign(
        "none",
        fx.option_int,
        Value::Make {
            ctor: fx.none,
            args: vec![],
        },
    );
    let tail = b.assign(
        "tail",
        fx.ints,
        Value::List {
            elems: vec![],
            tail: None,
        },
    );
    let xs = b.assign(
        "xs",
        fx.ints,
        Value::List {
            elems: vec![int(1), int(2)],
            tail: Some(tail.into()),
        },
    );
    let t = b.assign("t", fx.pair, Value::Tuple(vec![int(7), string("seven")]));
    let first = b.assign("first", TypeId::INT, Value::TupleGet { base: t, index: 0 });
    let joined = b.assign(
        "joined",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![string("n = "), string("7")],
        },
    );
    let u = b.assign(
        "u",
        fx.pair,
        Value::Tuple(vec![first.into(), joined.into()]),
    );
    let _ = (big, neg, yes, some, none, xs);
    b.term(Term::Return(Some(u.into())));
    m.add(b.finish());

    // const fn answer() -> Int
    let mut b = FnBuilder::new(&m.core, "answer");
    b.set_ret(TypeId::INT).set_constant();
    b.term(Term::Return(Some(int(42))));
    m.add(b.finish());

    insta::assert_snapshot!(checked_dump(&m));
}

/// Functions as values, effects and concurrency: closures with captures, function references,
/// indirect and operation calls, evidence tables, handler values, fiber operations.
#[test]
fn closures_effects_fibers() {
    let mut fx = Fixture::new();
    let handle_ty = fx.core.intern(Type::Any);
    let fork_body = fx.thunk;
    let suspending_thunk = fx.core.func(emela_core::FnType {
        params: vec![],
        ret: TypeId::INT,
        fails: false,
        suspends: true,
        ev: false,
        sets: None,
    });
    let mut m = Module::new(fx.core.clone());
    let greet = m.declare("greet");

    // fn greet(name: String, ev: evidence) -> (): evidence lookup and an operation call.
    let mut b = FnBuilder::new(&m.core, "greet");
    let name = b.param("name", TypeId::STRING);
    let ev = b.param("ev", TypeId::EVIDENCE);
    let console = b.assign(
        "console",
        fx.console_ty,
        Value::EvLookup {
            ev,
            effect: fx.console,
        },
    );
    let line = b.assign(
        "line",
        TypeId::STRING,
        Value::Builtin {
            op: Builtin::StringAppend,
            args: vec![string("hello, "), name.into()],
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: emela_lir::Callee::Op {
                handler: console,
                effect: fx.console,
                op: 0,
            },
            args: vec![line.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(None));
    m.define(greet, b.finish());

    // fn run(ev: evidence, prefix: String) -> Int: a handler value of closures, an extended
    // table, function values.
    let mut b = FnBuilder::new(&m.core, "run");
    b.set_ret(TypeId::INT);
    let ev = b.param("ev", TypeId::EVIDENCE);
    let prefix = b.param("prefix", TypeId::STRING);
    let print = b.var("print", fx.string_to_unit);
    let text = b.var("text", TypeId::STRING);
    b.closure(print, vec![text], vec![prefix], false, |b| {
        let s = b.assign(
            "s",
            TypeId::STRING,
            Value::Builtin {
                op: Builtin::StringAppend,
                args: vec![prefix.into(), text.into()],
            },
        );
        b.let_(
            None,
            Value::Builtin {
                op: Builtin::Dbg,
                args: vec![s.into()],
            },
        );
        b.term(Term::Return(None));
    });
    let print_error = b.var("print_error", fx.string_to_unit);
    let text2 = b.var("text", TypeId::STRING);
    b.closure(print_error, vec![text2], vec![], false, |b| {
        b.term(Term::Return(None))
    });
    let h = b.assign(
        "h",
        fx.console_ty,
        Value::Handler {
            name: "Prefixed".into(),
            effect: fx.console,
            ops: vec![print, print_error],
        },
    );
    let ev2 = b.assign(
        "ev2",
        TypeId::EVIDENCE,
        Value::EvExtend {
            base: Some(ev),
            bindings: vec![(fx.console, h)],
        },
    );
    let ev3 = b.assign(
        "ev3",
        TypeId::EVIDENCE,
        Value::EvExtend {
            base: None,
            bindings: vec![(fx.console, h)],
        },
    );
    b.let_(
        None,
        Value::Call {
            callee: emela_lir::Callee::Direct(greet),
            args: vec![string("world"), ev2.into()],
            suspends: false,
            on_fail: None,
        },
    );
    let g = b.assign("g", fx.string_to_unit, Value::FnRef(greet));
    let _ = (g, ev3);
    // A closure with the evidence table as its last parameter and a `()` parameter dropped.
    let add = b.var("add", fx.with_unit_ev);
    let x = b.var("x", TypeId::INT);
    let cev = b.var("ev", TypeId::EVIDENCE);
    b.closure(add, vec![x, cev], vec![], false, |b| {
        let y = b.assign(
            "y",
            TypeId::INT,
            prim(PrimOp::Add, PrimTy::I32, vec![x.into(), int(1)]),
        );
        b.term(Term::Return(Some(y.into())));
    });
    let r = b.var("r", TypeId::INT);
    b.let_(
        Some(r),
        Value::Call {
            callee: emela_lir::Callee::Indirect(add),
            args: vec![int(41), ev.into()],
            suspends: false,
            on_fail: None,
        },
    );
    b.term(Term::Return(Some(r.into())));
    m.add(b.finish());

    // suspend fn spawn(scope: fiber_scope, f: fn() -> Int) -> Int: fiber operations, a suspending
    // closure and a suspending direct call.
    let spawn = m.declare("spawn");
    let mut b = FnBuilder::new(&m.core, "spawn");
    b.set_ret(TypeId::INT).set_suspends();
    let scope = b.param("scope", TypeId::FIBER_SCOPE);
    let f = b.param("f", fork_body);
    let fiber = b.assign(
        "fiber",
        handle_ty,
        Value::Fiber {
            op: FiberOp::Fork,
            args: vec![scope.into(), f.into()],
            on_fail: None,
        },
    );
    let v = b.var("v", TypeId::INT);
    b.let_(
        Some(v),
        Value::Fiber {
            op: FiberOp::Join,
            args: vec![fiber.into()],
            on_fail: None,
        },
    );
    let again = b.var("again", suspending_thunk);
    b.closure(again, vec![], vec![scope, f], true, |b| {
        let w = b.var("w", TypeId::INT);
        b.let_(
            Some(w),
            Value::Call {
                callee: emela_lir::Callee::Direct(spawn),
                args: vec![scope.into(), f.into()],
                suspends: true,
                on_fail: None,
            },
        );
        b.term(Term::Return(Some(w.into())));
    });
    b.block(None, "skip", |b, skip| {
        b.term(Term::Break {
            label: skip,
            value: None,
        })
    });
    b.term(Term::Return(Some(v.into())));
    m.define(spawn, b.finish());

    // fn reraise(cause: any) -> Never fails: an operation that may fail, then unreachable.
    let mut b = FnBuilder::new(&m.core, "reraise");
    b.set_ret(TypeId::NEVER).set_fails();
    let cause = b.param("cause", handle_ty);
    b.let_(
        None,
        Value::Fiber {
            op: FiberOp::CauseRaise { fails: true },
            args: vec![cause.into()],
            on_fail: Some(OnFail::Propagate),
        },
    );
    b.term(Term::Unreachable);
    m.add(b.finish());

    insta::assert_snapshot!(checked_dump(&m));
}

/// The representation of each kind of Core type.
#[test]
fn representations() {
    let mut fx = Fixture::new();
    let cell = fx.core.cell(TypeId::INT);
    let param = fx.core.type_param(0);
    let any = fx.core.intern(Type::Any);
    let opaque = fx.core.add_enum(emela_core::EnumDef {
        name: "Handle".into(),
        params: 0,
        variants: vec![],
        error: false,
        external: true,
    });
    let opaque_ty = fx.core.enum_type(opaque, vec![]);
    let not_found = fx.core.add_enum(emela_core::EnumDef {
        name: "NotFound".into(),
        params: 0,
        variants: vec![emela_core::VariantDef::unit("NotFound")],
        error: true,
        external: false,
    });
    let not_found_ty = fx.core.enum_type(not_found, vec![]);
    let cases = [
        (TypeId::INT, Repr::I32),
        (TypeId::BOOL, Repr::I32),
        (TypeId::INT64, Repr::I64),
        (TypeId::FLOAT, Repr::F64),
        (TypeId::UNIT, Repr::Void),
        (TypeId::NEVER, Repr::Void),
        (TypeId::STRING, Repr::Ref),
        (fx.ints, Repr::Ref),
        (fx.pair, Repr::Ref),
        (fx.thunk, Repr::Ref),
        (fx.console_ty, Repr::Ref),
        (TypeId::EVIDENCE, Repr::Ref),
        (cell, Repr::Ref),
        (TypeId::FIBER_SCOPE, Repr::Ref),
        (any, Repr::Ref),
        (fx.color_ty, Repr::I32),
        (fx.shape_ty, Repr::Ref),
        (fx.option_int, Repr::Ref),
        (fx.parse_error_ty, Repr::Ref),
        // An error without fields is still a reference: errors must be told apart at run time.
        (not_found_ty, Repr::Ref),
        (opaque_ty, Repr::Ref),
        (param, Repr::Ref),
    ];
    for (ty, expected) in cases {
        assert_eq!(
            repr_of(&fx.core, ty),
            expected,
            "{}",
            emela_lir::core_ty(&fx.core, ty)
        );
    }
}
