use crate::build::*;
use crate::*;

/// `fn fact(n, acc) = if n == 0 then acc else fact(n - 1, acc * n)`
fn fact_module() -> (Module, FnId) {
    let mut m = Module::new();
    let fact = m.declare("fact", true);
    let n = m.local("n");
    let acc = m.local("acc");
    let body = if_(
        bin(BinOp::Eq, OpTy::Int, var(n), int(0)),
        var(acc),
        call(
            fact,
            vec![
                bin(BinOp::Sub, OpTy::Int, var(n), int(1)),
                bin(BinOp::Mul, OpTy::Int, var(acc), var(n)),
            ],
        ),
    );
    m.define(fact, vec![n, acc], body);
    (m, fact)
}

#[test]
fn loopify_rewrites_tail_self_call() {
    let (mut m, fact) = fact_module();
    assert!(tail::loopify_function(&mut m, fact));
    let f = &m.functions[fact];
    let Expr::Loop { vars, inits, body } = &f.body else {
        panic!("Loop になっていない: {:?}", f.body);
    };
    // ループの変数は引数と別の新しい変数で，初期値は引数．
    assert_eq!(vars.len(), 2);
    assert!(vars.iter().all(|v| !f.params.contains(v)));
    assert_eq!(inits, &vec![var(f.params[0]), var(f.params[1])]);
    // 本体の引数の出現はループの変数に置き換わり，末尾の呼び出しは Recur になる．
    let Expr::If { cond, then, else_ } = &**body else {
        panic!()
    };
    assert_eq!(**cond, bin(BinOp::Eq, OpTy::Int, var(vars[0]), int(0)));
    assert_eq!(**then, var(vars[1]));
    assert_eq!(
        **else_,
        Expr::Recur(vec![
            bin(BinOp::Sub, OpTy::Int, var(vars[0]), int(1)),
            bin(BinOp::Mul, OpTy::Int, var(vars[1]), var(vars[0])),
        ])
    );
}

#[test]
fn loopify_leaves_non_tail_calls() {
    // fn sum(n) = if n == 0 then 0 else n + sum(n - 1)
    let mut m = Module::new();
    let sum = m.declare("sum", false);
    let n = m.local("n");
    let body = if_(
        bin(BinOp::Eq, OpTy::Int, var(n), int(0)),
        int(0),
        bin(
            BinOp::Add,
            OpTy::Int,
            var(n),
            call(sum, vec![bin(BinOp::Sub, OpTy::Int, var(n), int(1))]),
        ),
    );
    m.define(sum, vec![n], body.clone());
    tail::loopify(&mut m);
    assert_eq!(m.functions[sum].body, body);
}

#[test]
fn loopify_ignores_calls_inside_lambda_and_other_functions() {
    // fn f(n) = (fn() => f(n))   ラムダの中の呼び出しは f の末尾ではない
    // fn g(n) = f(n)             他の関数への末尾呼び出しは対象外
    let mut m = Module::new();
    let f = m.declare("f", false);
    let g = m.declare("g", false);
    let n = m.local("n");
    let n2 = m.local("n");
    let fb = lambda(vec![], call(f, vec![var(n)]));
    m.define(f, vec![n], fb.clone());
    m.define(g, vec![n2], call(f, vec![var(n2)]));
    tail::loopify(&mut m);
    assert_eq!(m.functions[f].body, fb);
    assert_eq!(m.functions[g].body, call(f, vec![var(n2)]));
}

#[test]
fn loopify_through_let_and_match() {
    // fn len(xs, acc) = let ys = xs; match ys { [] => acc, [_, ..rest] => len(rest, acc + 1) }
    let mut m = Module::new();
    let len = m.declare("len", false);
    let xs = m.local("xs");
    let acc = m.local("acc");
    let ys = m.local("ys");
    let rest = m.local("rest");
    let body = let_(
        ys,
        var(xs),
        match_(
            var(ys),
            vec![
                arm(p_list(vec![], None), var(acc)),
                arm(
                    p_list(vec![Pat::Wild], Some(Pat::Bind(rest))),
                    call(
                        len,
                        vec![var(rest), bin(BinOp::Add, OpTy::Int, var(acc), int(1))],
                    ),
                ),
            ],
        ),
    );
    m.define(len, vec![xs, acc], body);
    assert!(tail::loopify_function(&mut m, len));
    let Expr::Loop { vars, body, .. } = &m.functions[len].body else {
        panic!()
    };
    let Expr::Let { value, body, .. } = &**body else {
        panic!()
    };
    assert_eq!(**value, var(vars[0]));
    let Expr::Match { arms, .. } = &**body else {
        panic!()
    };
    assert_eq!(
        arms[1].body,
        Expr::Recur(vec![
            var(rest),
            bin(BinOp::Add, OpTy::Int, var(vars[1]), int(1))
        ])
    );
}

#[test]
fn named_args_in_positional_order_are_untouched() {
    let mut m = Module::new();
    let f = m.declare("f", false);
    let e = named::in_written_order(&mut m, vec![(0, int(1)), (1, int(2))], |args| call(f, args));
    assert_eq!(e, call(f, vec![int(1), int(2)]));
}

#[test]
fn named_args_keep_written_evaluation_order() {
    // f(b: g(), a: 1, c: h()) → let arg0 = g(); let arg1 = h(); f(1, arg0, arg1)
    let mut m = Module::new();
    let f = m.declare("f", false);
    let g = m.declare("g", false);
    let h = m.local("h");
    let e = named::in_written_order(
        &mut m,
        vec![
            (1, call(g, vec![])),
            (0, int(1)),
            (2, call_value(var(h), vec![])),
        ],
        |args| call(f, args),
    );
    let Expr::Let {
        var: a0,
        value: v0,
        body,
    } = e
    else {
        panic!()
    };
    assert_eq!(*v0, call(g, vec![]));
    let Expr::Let {
        var: a1,
        value: v1,
        body,
    } = *body
    else {
        panic!()
    };
    assert_eq!(*v1, call_value(var(h), vec![]));
    assert_eq!(*body, call(f, vec![int(1), var(a0), var(a1)]));
}

#[test]
fn loopify_through_short_circuit() {
    // fn all_pos(xs) = match xs { [] => true, [x, ..rest] => x > 0 && all_pos(rest) }
    // fn any_zero(xs) = match xs { [] => false, [x, ..rest] => x == 0 || any_zero(rest) }
    let mut m = Module::new();
    for (name, op) in [("all_pos", BinOp::And), ("any_zero", BinOp::Or)] {
        let f = m.declare(name, false);
        let xs = m.local("xs");
        let x = m.local("x");
        let rest = m.local("rest");
        let test = bin(BinOp::Gt, OpTy::Int, var(x), int(0));
        m.define(
            f,
            vec![xs],
            match_(
                var(xs),
                vec![
                    arm(p_list(vec![], None), bool(op == BinOp::And)),
                    arm(
                        p_list(vec![Pat::Bind(x)], Some(Pat::Bind(rest))),
                        bin(op, OpTy::Bool, test.clone(), call(f, vec![var(rest)])),
                    ),
                ],
            ),
        );
        assert!(tail::loopify_function(&mut m, f));
        let Expr::Loop { body, .. } = &m.functions[f].body else {
            panic!()
        };
        let Expr::Match { arms, .. } = &**body else {
            panic!()
        };
        let recur = Expr::Recur(vec![var(rest)]);
        let expected = if op == BinOp::And {
            if_(test, recur, bool(false))
        } else {
            if_(test, bool(true), recur)
        };
        assert_eq!(arms[1].body, expected);
    }
}

// ---- 組み込み関数の表 ----

fn prelude_cons() -> (emela_types::TyCons, emela_types::TyConId) {
    use emela_types::{TyConData, TyConKind, TyCons, TyParam};
    let mut cons = TyCons::new();
    let option = cons.alloc(TyConData {
        name: "Option".into(),
        kind: TyConKind::Enum,
        params: vec![TyParam::new("A")],
    });
    (cons, option)
}

#[test]
fn builtin_names_are_unique_and_found_by_lookup() {
    for &op in Builtin::ALL {
        let i = op.info();
        assert_eq!(Builtin::lookup(i.module, i.name), Some(op), "{}", i.path());
        assert!(i.pure, "{} が純粋でない", i.path());
        assert!(i.js.starts_with('$'), "{} の実装名", i.path());
    }
    assert_eq!(
        Builtin::lookup(Some("String"), "length"),
        Some(Builtin::StringLength)
    );
    assert_eq!(Builtin::lookup(None, "dbg"), Some(Builtin::Dbg));
    // Prelude の関数は修飾しない．
    assert_eq!(Builtin::lookup(Some("Prelude"), "panic"), None);
    assert_eq!(Builtin::lookup(Some("List"), "map"), None);
}

#[test]
fn builtin_schemes() {
    let (cons, option) = prelude_cons();
    let show = |op: Builtin| format!("{}", op.info().scheme(option).display(&cons));
    assert_eq!(show(Builtin::StringLength), "fn(String) -> Int");
    assert_eq!(
        show(Builtin::StringSplit),
        "fn(String, String) -> List[String]"
    );
    assert_eq!(
        show(Builtin::StringJoin),
        "fn(List[String], String) -> String"
    );
    assert_eq!(show(Builtin::IntCheckedAdd), "fn(Int, Int) -> Option[Int]");
    assert_eq!(show(Builtin::FloatRound), "fn(Float) -> Int");
    assert_eq!(show(Builtin::Panic), "fn(String) -> Never");
    assert_eq!(show(Builtin::Todo), "fn() -> Never");
    assert_eq!(show(Builtin::Dbg), "∀ A. fn(A) -> A");
    assert_eq!(Builtin::Dbg.info().path(), "dbg");
    assert_eq!(Builtin::Int64FromInt.info().path(), "Int64.from_int");
}

#[test]
fn option_enum_is_shared() {
    let mut m = Module::new();
    let a = m.option_enum();
    let b = m.option_enum();
    assert_eq!(a, b);
    assert_eq!(m.enums.len(), 1);
    assert_eq!(m.some_ctor().variant, OPTION_SOME);
    let none = m.none_ctor();
    assert_eq!(m.variant(none).name, "None");
    assert_eq!(m.enums[a].variants[OPTION_SOME].tys, vec![Type::Param(0)]);
}

#[test]
fn type_subst_replaces_params() {
    let mut m = Module::new();
    let opt = m.option_enum();
    let t = Type::list(Type::Enum(opt, vec![Type::Param(0)]));
    assert_eq!(
        t.subst(&[Type::Tuple(vec![Type::Int, Type::String])]),
        Type::list(Type::Enum(
            opt,
            vec![Type::Tuple(vec![Type::Int, Type::String])]
        ))
    );
}
