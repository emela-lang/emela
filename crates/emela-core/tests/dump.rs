//! IR の dump（`emela_core::dump`）の見た目のテスト．IR は `build` のヘルパで手で組む．

use emela_core::build::*;
use emela_core::{
    Assert, BinOp, Builtin, CtorRef, EnumDef, Module, OpTy, Pat, Type, UnOp, VariantDef, dump,
};

#[test]
fn every_kind_of_expression() {
    let mut m = Module::new();
    let shape = m.add_enum(EnumDef {
        name: "Shapes.Shape".into(),
        params: 0,
        variants: vec![
            VariantDef::positional("Circle", vec![Type::Float]),
            VariantDef::named(
                "Rect",
                vec![("width", Type::Float), ("height", Type::Float)],
            ),
        ],
    });
    let pair = m.add_enum(EnumDef {
        name: "Pair".into(),
        params: 2,
        variants: vec![VariantDef::named(
            "Pair",
            vec![("first", Type::Param(0)), ("second", Type::Param(1))],
        )],
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
    let s = m.local("s");
    let r = m.local("r");
    let w = m.local("w");
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
    m.define(area, vec![s], body);

    // fn sum(xs, acc) { loop(...) { match xs { [] -> acc, [x, ..rest] -> recur(rest, acc + x) } } }
    let sum = m.declare("sum", false);
    let (xs, acc) = (m.local("xs"), m.local("acc"));
    let (lxs, lacc, x, rest) = (m.local("xs"), m.local("acc"), m.local("x"), m.local("rest"));
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
    m.define(sum, vec![xs, acc], body);

    let main = m.declare("main", true);
    let (p, f, y, name) = (m.local("p"), m.local("f"), m.local("y"), m.local("name"));
    let z = m.local("z");
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
                    builtin_ty(
                        dbg,
                        vec![Type::Enum(pair, vec![Type::Int, Type::String])],
                        vec![var(p)],
                    ),
                    builtin(length, vec![var(name)]),
                    emela_core::Expr::Concat(vec![
                        lit_part("n = "),
                        value_part(var(name), Type::String),
                    ]),
                    ctor(some, vec![int(1)]),
                    ctor(none, vec![]),
                    Assert::compare(
                        BinOp::Eq,
                        OpTy::Structural,
                        Type::Enum(pair, vec![Type::Int, Type::String]),
                        var(p),
                        var(p),
                        "p == p",
                    ),
                    Assert::bool(bool(true), "True"),
                ]),
            ),
        ),
    );
    m.define(main, vec![], body);

    insta::assert_snapshot!(dump(&m));
}
