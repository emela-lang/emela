//! Tests for how the IR dump (`emela_core::dump`) looks. The IR is built by hand with the `build`
//! helpers.

use emela_core::build::*;
use emela_core::{
    Assert, BinOp, Builtin, CtorRef, EnumDef, FnType, Module, OpTy, Pat, TypeId, UnOp, VariantDef,
    dump,
};

#[test]
fn every_kind_of_expression() {
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shapes.Shape".into(),
        params: 0,
        variants: vec![
            VariantDef::positional("Circle", vec![TypeId::FLOAT]),
            VariantDef::named(
                "Rect",
                vec![("width", TypeId::FLOAT), ("height", TypeId::FLOAT)],
            ),
        ],
        error: false,
    });
    let (a, b) = (m.type_param(0), m.type_param(1));
    let pair = m.add_enum(EnumDef {
        name: "Pair".into(),
        params: 2,
        variants: vec![VariantDef::named("Pair", vec![("first", a), ("second", b)])],
        error: false,
    });
    let circle = CtorRef {
        enum_id: shape,
        variant: 0,
    };
    let rect = CtorRef {
        enum_id: shape,
        variant: 1,
    };
    let pair_ctor = CtorRef {
        enum_id: pair,
        variant: 0,
    };
    let some = m.some_ctor();
    let none = m.none_ctor();
    let dbg = Builtin::lookup(None, "dbg").unwrap();
    let length = Builtin::lookup(Some("String"), "length").unwrap();

    // fn area(s) { match s { Circle(r) -> r * r * 2.5, Rect(width: w, height: _) if w > 0.0 -> w, _ -> 0.0 } }
    let area = m.declare("Shapes.area", false);
    let shape_ty = m.enum_type(shape, vec![]);
    let s = m.local("s", shape_ty);
    let r = m.local("r", TypeId::FLOAT);
    let w = m.local("w", TypeId::FLOAT);
    let body = match_(
        var(s),
        vec![
            arm(
                p_ctor(circle, vec![Pat::Bind(r)]),
                bin(
                    BinOp::Mul,
                    OpTy::Float,
                    bin(BinOp::Mul, OpTy::Float, var(r), var(r)),
                    float(2.5),
                ),
            ),
            arm_if(
                p_ctor(rect, vec![Pat::Bind(w), Pat::Wild]),
                bin(BinOp::Gt, OpTy::Float, var(w), float(0.0)),
                var(w),
            ),
            arm(Pat::Wild, float(0.0)),
        ],
    );
    m.define(area, vec![s], TypeId::FLOAT, body);

    // fn sum(xs, acc) { loop(...) { match xs { [] -> acc, [x, ..rest] -> recur(rest, acc + x) } } }
    let sum = m.declare("sum", false);
    let ints = m.list(TypeId::INT);
    let (xs, acc) = (m.local("xs", ints), m.local("acc", TypeId::INT));
    let (lxs, lacc) = (m.local("xs", ints), m.local("acc", TypeId::INT));
    let (x, rest) = (m.local("x", TypeId::INT), m.local("rest", ints));
    let body = emela_core::Expr::Loop {
        vars: vec![lxs, lacc],
        inits: vec![var(xs), var(acc)],
        body: Box::new(match_(
            var(lxs),
            vec![
                arm(p_list(vec![], None), var(lacc)),
                arm(
                    p_list(vec![Pat::Bind(x)], Some(Pat::Bind(rest))),
                    emela_core::Expr::Recur(vec![
                        var(rest),
                        bin(BinOp::Add, OpTy::Int, var(lacc), var(x)),
                    ]),
                ),
            ],
        )),
    };
    m.define(sum, vec![xs, acc], TypeId::INT, body);

    let main = m.declare("main", true);
    let pair_ty = m.enum_type(pair, vec![TypeId::INT, TypeId::STRING]);
    let inc_ty = m.func(FnType {
        params: vec![TypeId::INT64],
        ret: TypeId::INT64,
        fails: false,
        suspends: false,
        ev: false,
    });
    let area_ty = m.func(FnType {
        params: vec![shape_ty],
        ret: TypeId::FLOAT,
        fails: false,
        suspends: false,
        ev: false,
    });
    let p = m.local("p", pair_ty);
    let f = m.local("f", inc_ty);
    let y = m.local("y", TypeId::INT64);
    let name = m.local("name", TypeId::STRING);
    let z = m.local("z", TypeId::UNIT);
    let option_int = m.enum_type(some.enum_id, vec![TypeId::INT]);
    let main_ret = m.tuple(vec![
        TypeId::INT,
        TypeId::INT64,
        TypeId::INT,
        TypeId::FLOAT,
        area_ty,
        pair_ty,
        TypeId::INT,
        TypeId::STRING,
        option_int,
        option_int,
        TypeId::UNIT,
        TypeId::UNIT,
    ]);
    let body = let_(
        p,
        ctor(pair_ctor, vec![int(1), string("a")]),
        let_(
            f,
            lambda(vec![y], bin(BinOp::Add, OpTy::Int64, var(y), int64(1))),
            let_(
                name,
                if_(
                    bin(
                        BinOp::And,
                        OpTy::Bool,
                        bool(true),
                        unary(UnOp::Not, OpTy::Bool, bool(false)),
                    ),
                    string("yes"),
                    let_(z, unit(), string("no")),
                ),
                emela_core::Expr::Tuple(vec![
                    field(var(p), pair_ctor, 0),
                    call_value(var(f), vec![int64(41)]),
                    call(
                        sum,
                        vec![list_with_tail(vec![int(1), int(2)], list(vec![])), int(0)],
                    ),
                    call(
                        area,
                        vec![ctor(
                            circle,
                            vec![unary(UnOp::Neg, OpTy::Float, float(1.5))],
                        )],
                    ),
                    emela_core::Expr::Fn(area),
                    builtin_ty(dbg, vec![pair_ty], vec![var(p)]),
                    builtin(length, vec![var(name)]),
                    emela_core::Expr::Concat(vec![
                        lit_part("n = "),
                        value_part(var(name), TypeId::STRING),
                    ]),
                    ctor(some, vec![int(1)]),
                    ctor(none, vec![]),
                    Assert::compare(
                        BinOp::Eq,
                        OpTy::Structural,
                        pair_ty,
                        var(p),
                        var(p),
                        "p == p",
                    ),
                    Assert::bool(bool(true), "True"),
                ]),
            ),
        ),
    );
    m.define(main, vec![], main_ret, body);

    insta::assert_snapshot!(dump(&m));
}
