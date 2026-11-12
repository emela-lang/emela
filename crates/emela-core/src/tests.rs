use crate::build::*;
use crate::*;

/// `fn fact(n, acc) = if n == 0 then acc else fact(n - 1, acc * n)`
fn fact_module() -> (Module, FnId) {
    let mut m = Module::new();
    let fact = m.declare("fact", true);
    let n = m.local("n", TypeId::INT);
    let acc = m.local("acc", TypeId::INT);
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
    m.define(fact, vec![n, acc], TypeId::INT, body);
    (m, fact)
}

#[test]
fn loopify_rewrites_tail_self_call() {
    let (mut m, fact) = fact_module();
    assert!(tail::loopify_function(&mut m, fact));
    let f = &m.functions[fact];
    let Expr::Loop { vars, inits, body } = &f.body else {
        panic!("not a Loop: {:?}", f.body);
    };
    // Loop variables are fresh, distinct from the parameters, and start from the arguments.
    assert_eq!(vars.len(), 2);
    assert!(vars.iter().all(|v| !f.params.contains(v)));
    assert_eq!(inits, &vec![var(f.params[0]), var(f.params[1])]);
    // Occurrences of the parameters in the body become loop variables, and the tail call becomes
    // Recur.
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
fn loopify_does_not_rewrite_const_initializers() {
    let mut m = Module::new();
    let constant = m.declare("A", false);
    m.functions[constant].constant = true;
    m.define(constant, vec![], TypeId::INT, call(constant, vec![]));

    assert!(!tail::loopify_function(&mut m, constant));
    assert_eq!(m.functions[constant].body, call(constant, vec![]));
}

#[test]
fn loopify_leaves_non_tail_calls() {
    // fn sum(n) = if n == 0 then 0 else n + sum(n - 1)
    let mut m = Module::new();
    let sum = m.declare("sum", false);
    let n = m.local("n", TypeId::INT);
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
    m.define(sum, vec![n], TypeId::INT, body.clone());
    tail::loopify(&mut m);
    assert_eq!(m.functions[sum].body, body);
}

#[test]
fn loopify_ignores_calls_inside_lambda_and_other_functions() {
    // fn f(n) = (fn() => f(n))()   a call inside a lambda is not in f's tail position
    // fn g(n) = f(n)               tail calls to other functions are out of scope
    let mut m = Module::new();
    let f = m.declare("f", false);
    let g = m.declare("g", false);
    let n = m.local("n", TypeId::INT);
    let n2 = m.local("n", TypeId::INT);
    let thunk = m.fn_type(vec![], TypeId::INT);
    let fb = call_value(lambda(vec![], thunk, call(f, vec![var(n)])), vec![]);
    m.define(f, vec![n], TypeId::INT, fb.clone());
    m.define(g, vec![n2], TypeId::INT, call(f, vec![var(n2)]));
    assert_eq!(verify(&m), Ok(()));
    tail::loopify(&mut m);
    assert_eq!(m.functions[f].body, fb);
    assert_eq!(m.functions[g].body, call(f, vec![var(n2)]));
}

#[test]
fn loopify_through_let_and_match() {
    // fn len(xs, acc) = let ys = xs; match ys { [] => acc, [_, ..rest] => len(rest, acc + 1) }
    let mut m = Module::new();
    let len = m.declare("len", false);
    let ints = m.list(TypeId::INT);
    let xs = m.local("xs", ints);
    let acc = m.local("acc", TypeId::INT);
    let ys = m.local("ys", ints);
    let rest = m.local("rest", ints);
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
    m.define(len, vec![xs, acc], TypeId::INT, body);
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
    let e = named::in_written_order(
        &mut m,
        vec![(0, int(1), TypeId::INT), (1, int(2), TypeId::INT)],
        |args| call(f, args),
    );
    assert_eq!(e, call(f, vec![int(1), int(2)]));
}

#[test]
fn named_args_keep_written_evaluation_order() {
    // f(b: g(), a: 1, c: h()) → let arg0 = g(); let arg1 = h(); f(1, arg0, arg1)
    let mut m = Module::new();
    let f = m.declare("f", false);
    let g = m.declare("g", false);
    let h_ty = m.func(FnType {
        params: vec![],
        ret: TypeId::INT,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    });
    let h = m.local("h", h_ty);
    let e = named::in_written_order(
        &mut m,
        vec![
            (1, call(g, vec![]), TypeId::INT),
            (0, int(1), TypeId::INT),
            (2, call_value(var(h), vec![]), TypeId::INT),
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
        let ints = m.list(TypeId::INT);
        let xs = m.local("xs", ints);
        let x = m.local("x", TypeId::INT);
        let rest = m.local("rest", ints);
        let test = bin(BinOp::Gt, OpTy::Int, var(x), int(0));
        m.define(
            f,
            vec![xs],
            TypeId::BOOL,
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

fn prelude_cons() -> (emela_types::TyCons, crate::SigCons) {
    use emela_types::{TyConData, TyConKind, TyCons, TyParam};
    let mut cons = TyCons::new();
    let option = cons.alloc(TyConData {
        name: "Option".into(),
        kind: TyConKind::Enum,
        params: vec![TyParam::new("A")],
    });
    let bytes = cons.alloc(TyConData {
        name: "Bytes".into(),
        kind: TyConKind::Type,
        params: Vec::new(),
    });
    let array = cons.alloc(TyConData {
        name: "Array".into(),
        kind: TyConKind::Type,
        params: vec![TyParam::new("A")],
    });
    (
        cons,
        crate::SigCons {
            option,
            bytes,
            array,
        },
    )
}

#[test]
fn builtin_names_are_unique_and_found_by_lookup() {
    for &op in Builtin::ALL {
        let i = op.info();
        assert_eq!(Builtin::lookup(i.module, i.name), Some(op), "{}", i.path());
    }
    assert_eq!(
        Builtin::lookup(Some("String"), "length"),
        Some(Builtin::StringLength)
    );
    assert_eq!(Builtin::lookup(None, "dbg"), Some(Builtin::Dbg));
    assert_eq!(Builtin::lookup(None, "inspect"), Some(Builtin::Inspect));
    // Prelude functions are not qualified.
    assert_eq!(Builtin::lookup(Some("Prelude"), "panic"), None);
    assert_eq!(Builtin::lookup(Some("List"), "map"), None);
}

#[test]
fn builtin_schemes() {
    let (cons, sig_cons) = prelude_cons();
    let show = |op: Builtin| format!("{}", op.info().scheme(sig_cons).display(&cons));
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
    assert_eq!(show(Builtin::Inspect), "∀ A. fn(A) -> String");
    assert_eq!(Builtin::Int64FromInt.info().path(), "Int64.from_int");
    assert_eq!(show(Builtin::BytesSize), "fn(Bytes) -> Int");
    assert_eq!(show(Builtin::ArrayFromList), "∀ A. fn(List[A]) -> Array[A]");
    assert_eq!(
        show(Builtin::ArraySet),
        "∀ A. fn(Array[A], Int, A) -> Array[A]"
    );
    assert_eq!(
        show(Builtin::ArrayConcat),
        "∀ A. fn(List[Array[A]]) -> Array[A]"
    );
    assert_eq!(Builtin::ArrayGet.info().path(), "Std.Array.get");
    // No builtin has two type parameters yet; the second is `B`.
    let two = BuiltinInfo {
        ty_params: 2,
        params: &[Sig::Param(0), Sig::Param(1)],
        ret: Sig::Param(1),
        ..Builtin::Dbg.info()
    };
    assert_eq!(
        format!("{}", two.scheme(sig_cons).display(&cons)),
        "∀ A, B. fn(A, B) -> B"
    );
    assert_eq!(Builtin::Todo.arity(), 0);
    assert_eq!(Builtin::Dbg.arity(), 1);
    assert_eq!(Builtin::StringSplit.arity(), 2);
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
    let a0 = m.type_param(0);
    assert_eq!(m.enums[a].variants[OPTION_SOME].tys, vec![a0]);
}

#[test]
fn type_subst_replaces_params() {
    let mut m = Module::new();
    let opt = m.option_enum();
    let a = m.type_param(0);
    let opt_a = m.enum_type(opt, vec![a]);
    let t = m.list(opt_a);
    let pair = m.tuple(vec![TypeId::INT, TypeId::STRING]);
    let opt_pair = m.enum_type(opt, vec![pair]);
    let expected = m.list(opt_pair);
    assert_eq!(m.types.subst(t, &[pair]), expected);
    // A type without parameters is returned as is, and equal types are one ID.
    assert_eq!(m.types.subst(pair, &[TypeId::BOOL]), pair);
    assert_eq!(m.tuple(vec![TypeId::INT, TypeId::STRING]), pair);
    assert!(m.types.has_param(t));
    assert!(!m.types.has_param(expected));
}

#[test]
fn types_are_interned() {
    let mut m = Module::new();
    assert_eq!(m.intern(Type::Int), TypeId::INT);
    assert_eq!(m.intern(Type::Any), TypeId::ANY);
    assert_eq!(m.ty(TypeId::FIBER_SCOPE), &Type::FiberScope);
    let f = FnType {
        params: vec![TypeId::INT],
        ret: TypeId::BOOL,
        fails: false,
        suspends: false,
        ev: false,
        sets: None,
    };
    let a = m.func(f.clone());
    let b = m.func(f.clone());
    assert_eq!(a, b);
    let c = m.func(FnType { fails: true, ..f });
    assert_ne!(a, c);
    let n = m.types.len();
    m.list(a);
    m.list(b);
    assert_eq!(m.types.len(), n + 1);
}

/// `fn id[A](x: A) -> A { x }`.
fn id_fn(m: &mut Module) -> FnId {
    let id = m.declare("id", false);
    let a = m.type_param(0);
    let x = m.local("x", a);
    m.define(id, vec![x], a, var(x));
    m.functions[id].type_params = 1;
    id
}

#[test]
fn type_of_follows_type_arguments() {
    let mut m = Module::new();
    let id = id_fn(&mut m);
    let none = m.none_ctor();
    let some = m.some_ctor();
    let opt_int = m.enum_type(none.enum_id, vec![TypeId::INT]);
    let ints = m.list(TypeId::INT);
    let id_str = m.fn_type(vec![TypeId::STRING], TypeId::STRING);
    let exprs = [
        (call_ty(id, vec![TypeId::INT], vec![int(1)]), TypeId::INT),
        (fn_ref_ty(id, vec![TypeId::STRING]), id_str),
        (ctor_ty(none, vec![TypeId::INT], vec![]), opt_int),
        (ctor_ty(some, vec![TypeId::INT], vec![int(1)]), opt_int),
        (list(TypeId::INT, vec![]), ints),
        // The first branch that is not `Never` decides.
        (if_(bool(true), fail(unit()), string("s")), TypeId::STRING),
        (builtin(Builtin::IntParse, vec![string("1")]), opt_int),
        (builtin(Builtin::Todo, vec![]), TypeId::NEVER),
    ];
    let mut t = Typer::new(&m);
    for (e, ty) in &exprs {
        assert_eq!(t.type_of(e), *ty, "{e:?}");
    }
}

#[test]
fn type_of_remembers_let_chains() {
    // let x0 = 0; let x1 = 1; ...; "end"
    let mut m = Module::new();
    let xs: Vec<Local> = (0..2000).map(|_| m.local("x", TypeId::INT)).collect();
    let chain = xs
        .iter()
        .enumerate()
        .rev()
        .fold(string("end"), |body, (i, &x)| let_(x, int(i as i32), body));
    let mut t = Typer::new(&m);
    // Asking at every node walks the chain once.
    let mut e = &chain;
    while let Expr::Let { value, body, .. } = e {
        assert_eq!(t.type_of(e), TypeId::STRING);
        assert_eq!(t.type_of(value), TypeId::INT);
        e = body;
    }
}

#[test]
fn verify_accepts_well_typed_ir() {
    let mut m = Module::new();
    let id = id_fn(&mut m);
    let main = m.declare("main", true);
    let none = m.none_ctor();
    let opt_int = m.enum_type(none.enum_id, vec![TypeId::INT]);
    let id_str = m.fn_type(vec![TypeId::STRING], TypeId::STRING);
    let (a, b, c) = (
        m.local("a", TypeId::INT),
        m.local("b", id_str),
        m.local("c", opt_int),
    );
    let body = let_(
        a,
        call_ty(id, vec![TypeId::INT], vec![int(1)]),
        let_(
            b,
            fn_ref_ty(id, vec![TypeId::STRING]),
            let_(
                c,
                ctor_ty(none, vec![TypeId::INT], vec![]),
                call_value(var(b), vec![string("s")]),
            ),
        ),
    );
    m.define(main, vec![], TypeId::STRING, body);
    assert_eq!(verify(&m), Ok(()));
}

#[test]
fn verify_reports_every_ill_typed_place() {
    let mut m = Module::new();
    let id = id_fn(&mut m);
    let main = m.declare("main", true);
    let some = m.some_ctor();
    let ints = m.list(TypeId::INT);
    let int_fn = m.fn_type(vec![], TypeId::INT);
    let (a, b) = (m.local("a", TypeId::INT), m.local("b", ints));
    let body = let_(
        a,
        // An argument of the wrong type, then a missing type argument.
        call_ty(id, vec![TypeId::INT], vec![string("s")]),
        let_(
            b,
            list(TypeId::INT, vec![call(id, vec![int(1)])]),
            tuple(vec![
                ctor_ty(some, vec![TypeId::INT], vec![string("s")]),
                if_(bool(true), int(1), string("s")),
                lambda(vec![], int_fn, string("s")),
                Expr::Recur(vec![]),
            ]),
        ),
    );
    m.define(main, vec![], TypeId::UNIT, body);
    let errors: Vec<String> = verify(&m)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        errors,
        [
            "main: let a%1 > arg 0: value: expected Int, found String",
            // Without type arguments, `A` is left as is.
            "main: let b%2 > elem 0: 0 type arguments for id with 1",
            "main: let b%2 > elem 0 > arg 0: value: expected A, found Int",
            "main: let b%2 > elem 0: value: expected Int, found A",
            "main: elem 0 > arg 0: value: expected Int, found String",
            "main: elem 1: branches of types Int and String",
            "main: elem 2 > lambda: body: expected Int, found String",
            "main: elem 3: recur outside a loop",
            "main: body: expected (), found (Option[Int], Int, fn() -> Int, Never)",
        ]
    );
}

/// Operation calls, handler implementations and evidence bindings are checked against `OpDef`s
/// and what each `EffectDef` instantiates.
#[test]
fn verify_checks_operations() {
    let mut m = Module::new();
    let e = m.add_effect(EffectDef::new(
        "E",
        vec![
            OpDef::new("get", vec![TypeId::INT], TypeId::STRING),
            OpDef::suspend("wait", vec![], TypeId::INT),
        ],
    ));
    // `G[Int]`, `G[$f.0]` (the key in a generic `f`) and an unrelated `H`.
    let a = m.type_param(0);
    let generic = |m: &mut Module, name: &str, decl: &str, args: Vec<TypeId>| {
        let ret = args.first().copied().unwrap_or(TypeId::UNIT);
        m.add_effect(EffectDef {
            name: name.to_owned(),
            decl: decl.to_owned(),
            args,
            ops: vec![OpDef::new("get", vec![], ret)],
            label_of: None,
        })
    };
    let g_int = generic(&mut m, "G[Int]", "G", vec![TypeId::INT]);
    let g_a = generic(&mut m, "G[$f.0]", "G", vec![a]);
    let h = generic(&mut m, "H", "H", vec![]);
    let main = m.declare("main", true);
    let str_fn = m.fn_type(vec![TypeId::STRING], TypeId::STRING);
    let unit_fn = m.fn_type(vec![], TypeId::UNIT);
    let int_fn = m.fn_type(vec![], TypeId::INT);
    let e_ty = m.handler(e);
    let (x, hv, r) = (
        m.local("x", TypeId::STRING),
        m.local("h", e_ty),
        m.local("r", TypeId::INT),
    );
    let handler = |effect, ops| Expr::Handler {
        name: "X".into(),
        effect,
        ops,
    };
    let body = let_(
        hv,
        // `get` takes an Int, and `wait`'s implementation lacks resume.
        handler(
            e,
            vec![
                lambda(vec![x], str_fn, var(x)),
                lambda(vec![], unit_fn, unit()),
            ],
        ),
        let_(
            r,
            call_op(var(hv), e, 0, vec![string("s")], false, false),
            Expr::Provide {
                base: None,
                bindings: vec![
                    (g_a, handler(g_int, vec![lambda(vec![], int_fn, int(1))])),
                    (g_int, handler(h, vec![lambda(vec![], unit_fn, unit())])),
                ],
            },
        ),
    );
    m.define(main, vec![], TypeId::EVIDENCE, body);
    let errors: Vec<String> = verify(&m)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        errors,
        [
            "main: let h%1 > op 0: parameter: expected String, found Int",
            "main: let h%1 > op 1: operation wait with 1 parameters implemented with 0",
            "main: let r%2 > arg 0: value: expected Int, found String",
            "main: let r%2: value: expected Int, found String",
            "main: binding 1: expected handler G[Int], found handler H",
        ]
    );
}

fn verify_errors(m: &Module) -> Vec<String> {
    match verify(m) {
        Ok(()) => Vec::new(),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    }
}

/// An enum with one constructor without fields, named after the enum.
fn unit_enum(m: &mut Module, name: &str, error: bool) -> CtorRef {
    let enum_id = m.add_enum(EnumDef {
        name: name.to_owned(),
        params: 0,
        variants: vec![VariantDef {
            name: name.to_owned(),
            fields: VariantFields::Unit,
            tys: Vec::new(),
        }],
        error,
        external: false,
    });
    CtorRef {
        enum_id,
        variant: 0,
    }
}

/// A value whose type has `Never` in place of a component flows into a slot of the type with the
/// component: lists, tuples, enum applications and function results.
#[test]
fn verify_accepts_never_inside_types() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let none = m.none_ctor();
    let n = m.local("n", TypeId::NEVER);
    let ints = m.list(TypeId::INT);
    let pair = m.tuple(vec![TypeId::INT, TypeId::INT]);
    let opt_int = m.enum_type(none.enum_id, vec![TypeId::INT]);
    let int_to_int = m.fn_type(vec![TypeId::INT], TypeId::INT);
    let int_to_never = m.fn_type(vec![TypeId::INT], TypeId::NEVER);
    let x = m.local("x", TypeId::INT);
    let (a, b, c, d) = (
        m.local("a", ints),
        m.local("b", pair),
        m.local("c", opt_int),
        m.local("d", int_to_int),
    );
    let body = let_(
        a,
        list(TypeId::NEVER, vec![]),
        let_(
            b,
            tuple(vec![var(n), int(1)]),
            let_(
                c,
                ctor_ty(none, vec![TypeId::NEVER], vec![]),
                let_(d, lambda(vec![x], int_to_never, var(n)), unit()),
            ),
        ),
    );
    m.define(main, vec![n], TypeId::UNIT, body);
    assert_eq!(verify_errors(&m), Vec::<String>::new());
}

/// Values that may not flow into a slot: `()` where a value is expected, function types whose
/// parameter counts or results differ, and different enums.
#[test]
fn verify_rejects_incompatible_values() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let one = unit_enum(&mut m, "One", false);
    let other = unit_enum(&mut m, "Other", false);
    let one_ty = m.enum_type(one.enum_id, vec![]);
    let x = m.local("x", TypeId::INT);
    let int_to_int = m.fn_type(vec![TypeId::INT], TypeId::INT);
    let int_to_str = m.fn_type(vec![TypeId::INT], TypeId::STRING);
    let two_ints = m.fn_type(vec![TypeId::INT, TypeId::INT], TypeId::INT);
    let no_params = m.fn_type(vec![], TypeId::INT);
    let (a, b, c, d, e) = (
        m.local("a", TypeId::INT),
        m.local("b", two_ints),
        m.local("c", int_to_int),
        m.local("d", one_ty),
        m.local("e", no_params),
    );
    let body = let_(
        a,
        unit(),
        let_(
            b,
            lambda(vec![x], int_to_int, int(1)),
            let_(
                c,
                lambda(vec![x], int_to_str, string("s")),
                let_(
                    d,
                    ctor(other, vec![]),
                    let_(e, lambda(vec![x], no_params, int(1)), unit()),
                ),
            ),
        ),
    );
    m.define(main, vec![], TypeId::UNIT, body);
    assert_eq!(
        verify_errors(&m),
        [
            "main: let a%1: value: expected Int, found ()",
            "main: let b%2: value: expected fn(Int, Int) -> Int, found fn(Int) -> Int",
            "main: let c%3: value: expected fn(Int) -> Int, found fn(Int) -> String",
            "main: let d%4: value: expected One, found Other",
            "main: let e%5: a lambda with 1 parameters of type fn() -> Int",
        ]
    );
}

/// An escape arm matches an error constructor, or is `_` without a guard.
#[test]
fn verify_checks_escape_arms() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let oops = unit_enum(&mut m, "Oops", true);
    let plain = unit_enum(&mut m, "Plain", false);
    let body = escape(
        int(1),
        vec![
            arm(p_ctor(oops, vec![]), int(2)),
            arm(Pat::Wild, int(3)),
            arm_if(Pat::Wild, bool(true), int(4)),
            arm(p_ctor(plain, vec![]), int(5)),
        ],
    );
    m.define(main, vec![], TypeId::INT, body);
    assert_eq!(
        verify_errors(&m),
        [
            "main: arm 2: an escape arm `_` with a guard",
            "main: arm 3: an escape arm not matching an error",
        ]
    );
}

/// `==` compares values of which one may flow into the other; a shown assert names its sides by
/// variables; an evidence table is not `()`; only functions are called.
#[test]
fn verify_checks_comparisons_tables_and_callees() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let e = m.add_effect(EffectDef::new("E", vec![]));
    let n = m.local("n", TypeId::NEVER);
    let a = m.local("a", TypeId::INT);
    let eq = |l, r| bin(BinOp::Eq, OpTy::Structural, l, r);
    let shown = Expr::Assert(Box::new(Assert {
        cond: AssertCond::Shown {
            cond: bool(true),
            op: BinOp::Eq,
            ty: TypeId::INT,
            lhs: int(1),
            rhs: var(a),
        },
        text: String::new(),
        location: None,
    }));
    let body = tuple(vec![
        eq(var(n), int(1)),
        eq(int(1), var(n)),
        eq(int(1), string("s")),
        let_(a, int(1), shown),
        Expr::Use {
            ctx: Box::new(int(1)),
            effect: e,
        },
        Expr::Provide {
            base: Some(Box::new(unit())),
            bindings: vec![],
        },
        call_value(int(1), vec![]),
    ]);
    m.define(main, vec![n], TypeId::ANY, body);
    assert_eq!(
        verify_errors(&m),
        [
            "main: elem 2: comparing Int with String",
            "main: elem 3: an assert's shown lhs is not a variable",
            "main: elem 4: expected evidence, found Int",
            "main: elem 5: expected evidence, found ()",
            "main: elem 6: calling a Int",
        ]
    );
}

/// Calling a `Never` value gives `Never`, which the other branch of an `if` decides the type of.
#[test]
fn verify_calling_never_gives_never() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let n = m.local("n", TypeId::NEVER);
    let body = if_(bool(true), call_value(var(n), vec![]), string("s"));
    m.define(main, vec![n], TypeId::INT, body);
    assert_eq!(
        verify_errors(&m),
        ["main: body: expected Int, found String"]
    );
}

/// Patterns against values of other shapes. Under a `Never` scrutinee nothing is known, so no
/// pattern is wrong.
#[test]
fn verify_checks_patterns() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let some = m.some_ctor();
    let one = unit_enum(&mut m, "One", false);
    let n = m.local("n", TypeId::NEVER);
    let (x, y) = (m.local("x", TypeId::INT), m.local("y", TypeId::INT));
    let pair = || Pat::Tuple(vec![Pat::Bind(x), Pat::Bind(y)]);
    let match_all = |scrutinee: Expr, pats: Vec<Pat>| {
        match_(
            scrutinee,
            pats.into_iter().map(|p| arm(p, unit())).collect(),
        )
    };
    let body = tuple(vec![
        match_all(
            var(n),
            vec![pair(), p_list(vec![], None), p_ctor(some, vec![Pat::Wild])],
        ),
        match_all(
            int(1),
            vec![pair(), p_list(vec![], None), p_ctor(some, vec![Pat::Wild])],
        ),
        match_all(tuple(vec![int(1), int(2), int(3)]), vec![pair()]),
        match_all(ctor(one, vec![]), vec![p_ctor(some, vec![Pat::Wild])]),
    ]);
    m.define(main, vec![n], TypeId::ANY, body);
    assert_eq!(
        verify_errors(&m),
        [
            "main: elem 1 > arm 0: a 2-tuple pattern against Int",
            "main: elem 1 > arm 1: a list pattern against Int",
            "main: elem 1 > arm 2: pattern Option.Some against Int",
            "main: elem 2 > arm 0: a 2-tuple pattern against (Int, Int, Int)",
            "main: elem 3 > arm 0: pattern Option.Some against One",
        ]
    );
}

/// Each concurrency operation's arguments against its signature.
#[test]
fn verify_checks_fiber_operations() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let some = m.some_ctor();
    let x = m.local("x", TypeId::INT);
    let int_to_int = m.fn_type(vec![TypeId::INT], TypeId::INT);
    let body = tuple(vec![
        fiber(FiberOp::Scope { fails: false }, vec![int(1)]),
        fiber(
            FiberOp::Fork,
            vec![int(1), lambda(vec![x], int_to_int, var(x))],
        ),
        fiber(FiberOp::Deferred, vec![string("s")]),
        fiber(
            FiberOp::Complete,
            vec![ctor_ty(some, vec![TypeId::INT], vec![int(1)]), string("s")],
        ),
        fiber(
            FiberOp::Attempt {
                exit: some.enum_id,
                suspends: false,
            },
            vec![lambda(vec![x], int_to_int, var(x))],
        ),
    ]);
    m.define(main, vec![], TypeId::ANY, body);
    assert_eq!(
        verify_errors(&m),
        [
            "main: elem 0: fiber#scope of a Int",
            "main: elem 1: scope: expected fiber_scope, found Int",
            "main: elem 1: fiber#fork of a fn(Int) -> Int",
            "main: elem 2: scope: expected fiber_scope, found String",
            "main: elem 3: value: expected Int, found String",
            "main: elem 4: fiber#attempt of a fn(Int) -> Int",
        ]
    );
}

/// A handler provided under the key of another instantiation of the same effect: the key's type
/// parameters stand for one type each, throughout the arguments.
#[test]
fn verify_checks_bindings_by_instance() {
    let mut m = Module::new();
    let main = m.declare("main", true);
    let none = m.none_ctor();
    let a = m.type_param(0);
    let boxed = m.add_enum(EnumDef {
        name: "Box".to_owned(),
        params: 1,
        variants: vec![],
        error: false,
        external: false,
    });
    let effect = |m: &mut Module, decl: &str, args: Vec<TypeId>| {
        let names: Vec<String> = args
            .iter()
            .map(|&t| crate::dump::type_name(m, &m.types, t))
            .collect();
        let name = format!("{decl}[{}]", names.join(", "));
        if let Some((id, _)) = m.effects.iter().find(|(_, e)| e.name == name) {
            return id;
        }
        m.add_effect(EffectDef {
            name,
            decl: decl.to_owned(),
            args,
            ops: vec![],
            label_of: None,
        })
    };
    let ty = |m: &mut Module, t: Type| m.types.intern(t);
    let a_int = ty(&mut m, Type::Tuple(vec![a, TypeId::INT]));
    let str_int = ty(&mut m, Type::Tuple(vec![TypeId::STRING, TypeId::INT]));
    let ints3 = ty(&mut m, Type::Tuple(vec![TypeId::INT; 3]));
    let list_a = m.list(a);
    let list_int = m.list(TypeId::INT);
    let opt_a = m.enum_type(none.enum_id, vec![a]);
    let opt_int = m.enum_type(none.enum_id, vec![TypeId::INT]);
    let box_int = m.enum_type(boxed, vec![TypeId::INT]);
    let a_to_int = m.fn_type(vec![a], TypeId::INT);
    let str_to_int = m.fn_type(vec![TypeId::STRING], TypeId::INT);
    let str_to_bool = m.fn_type(vec![TypeId::STRING], TypeId::BOOL);
    // (key, handler)
    let cases = [
        (vec![a_int], vec![str_int]),
        (vec![list_a], vec![list_int]),
        (vec![opt_a], vec![opt_int]),
        (vec![a_to_int], vec![str_to_int]),
        (vec![TypeId::INT], vec![TypeId::STRING]),
        (vec![a_int], vec![ints3]),
        (vec![opt_a], vec![box_int]),
        (vec![a_to_int], vec![str_to_bool]),
        (vec![a, a], vec![a, TypeId::INT]),
    ];
    let mut bindings = Vec::new();
    for (key, handler) in cases {
        let key = effect(&mut m, "G", key);
        let handler = effect(&mut m, "G", handler);
        bindings.push((
            key,
            Expr::Handler {
                name: "X".into(),
                effect: handler,
                ops: vec![],
            },
        ));
    }
    let h = effect(&mut m, "H", vec![]);
    let k = effect(&mut m, "K", vec![]);
    bindings.push((
        h,
        Expr::Handler {
            name: "X".into(),
            effect: k,
            ops: vec![],
        },
    ));
    m.define(
        main,
        vec![],
        TypeId::EVIDENCE,
        Expr::Provide {
            base: None,
            bindings,
        },
    );
    assert_eq!(
        verify_errors(&m),
        [
            "main: binding 4: expected handler G[Int], found handler G[String]",
            "main: binding 5: expected handler G[(A, Int)], found handler G[(Int, Int, Int)]",
            "main: binding 6: expected handler G[Option[A]], found handler G[Box[Int]]",
            "main: binding 7: expected handler G[fn(A) -> Int], found handler G[fn(String) -> Bool]",
            "main: binding 8: expected handler G[A, A], found handler G[A, Int]",
            "main: binding 9: expected handler H[], found handler K[]",
        ]
    );
}

/// After monomorphization, nothing is generic, function values agree on taking the evidence table,
/// `()` is not passed for one, and calls to fallible functions are marked.
#[test]
fn verify_monomorphic_checks_more() {
    let mut m = Module::new();
    let id = id_fn(&mut m);
    let a = m.type_param(0);
    let main = m.declare("main", true);
    let takes_ev = m.declare("takes_ev", false);
    let ev = m.local("ev", TypeId::EVIDENCE);
    m.define(takes_ev, vec![ev], TypeId::UNIT, unit());
    let fallible = m.declare("fallible", false);
    m.define(fallible, vec![], TypeId::UNIT, unit());
    m.functions[fallible].fails = true;
    let plain = m.fn_type(vec![], TypeId::INT);
    let with_ev = m.func(FnType {
        ev: true,
        ..FnType::pure(vec![], TypeId::INT)
    });
    let (f, e) = (m.local("f", plain), m.local("e", TypeId::EVIDENCE));
    let n = m.local("n", TypeId::NEVER);
    let body = let_(
        f,
        lambda(vec![e], with_ev, int(1)),
        tuple(vec![
            call_ty(id, vec![a], vec![var(n)]),
            call(takes_ev, vec![unit()]),
            call(fallible, vec![]),
        ]),
    );
    m.define(main, vec![n], TypeId::ANY, body);
    assert_eq!(verify_errors(&m), Vec::<String>::new());
    let strict: Vec<String> = verify_monomorphic(&m)
        .unwrap_err()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        strict,
        [
            "id: a generic function after monomorphization",
            "id: parameter of type A after monomorphization",
            "id: return type of type A after monomorphization",
            "main: let f%2: value: expected fn() -> Int, found fn() -> Int ev",
            "main: elem 0: type argument of type A after monomorphization",
            "main: elem 1 > arg 0: value: expected evidence, found ()",
            "main: elem 2: a call to fallible fallible not marked fails",
            // Locals belong to no function.
            ": local of type A after monomorphization",
        ]
    );
}

#[test]
fn external_binding_picks_the_target() {
    let binding = |target: &str, name: &str| ExternalBinding {
        target: target.to_owned(),
        host: HostRef::Global(name.to_owned()),
    };
    let ext = External {
        bindings: vec![binding("js", "a"), binding("wasm", "b"), binding("js", "c")],
    };
    assert_eq!(ext.binding("wasm"), Some(&binding("wasm", "b")));
    assert_eq!(ext.binding("js"), Some(&binding("js", "a")));
    assert_eq!(ext.binding("c"), None);
}

#[test]
fn op_def_failing_fails() {
    let op = OpDef::new("get", vec![], TypeId::INT);
    assert!(!op.fails);
    assert!(op.failing().fails);
}

#[test]
fn fiber_ops_that_fail() {
    let exit = Module::new().option_enum();
    assert!(FiberOp::Scope { fails: true }.fails());
    assert!(!FiberOp::Scope { fails: false }.fails());
    assert!(FiberOp::CauseRaise { fails: true }.fails());
    assert!(!FiberOp::CauseRaise { fails: false }.fails());
    assert!(
        !FiberOp::Attempt {
            exit,
            suspends: true
        }
        .fails()
    );
    assert!(!FiberOp::Join.fails());
}

#[test]
fn type_table_lookups() {
    let mut t = TypeTable::default();
    assert_eq!(format!("{:?}", TypeId::INT), "TypeId(0)");
    assert!(!t.is_empty());
    let ints = Type::List(TypeId::INT);
    assert_eq!(t.get(&ints), None);
    let id = t.list(TypeId::INT);
    assert_eq!(t.get(&ints), Some(id));
}

/// Every `Type::Set` inside a type becomes `()`, and function types lose their `sets`.
#[test]
fn erase_sets_reaches_every_component() {
    let mut m = Module::new();
    let some = m.some_ctor();
    let t = &mut m.types;
    let set = t.intern(Type::Set(SetSummary::unknown()));
    let fn_with = |params, ret| FnType {
        sets: Some(FnSets {
            errors: set,
            effects: set,
        }),
        ..FnType::pure(params, ret)
    };
    let cases = [
        (
            Type::Tuple(vec![set, TypeId::INT]),
            Type::Tuple(vec![TypeId::UNIT, TypeId::INT]),
        ),
        (Type::List(set), Type::List(TypeId::UNIT)),
        (Type::Array(set), Type::Array(TypeId::UNIT)),
        (
            Type::Enum(some.enum_id, vec![set]),
            Type::Enum(some.enum_id, vec![TypeId::UNIT]),
        ),
        (Type::Cell(set), Type::Cell(TypeId::UNIT)),
        (
            Type::Fn(fn_with(vec![set], set)),
            Type::Fn(FnType::pure(vec![TypeId::UNIT], TypeId::UNIT)),
        ),
    ];
    for (before, after) in cases {
        let before = t.intern(before);
        let after = t.intern(after);
        assert_eq!(t.erase_sets(before), after);
    }
    assert_eq!(t.erase_sets(set), TypeId::UNIT);
    assert_eq!(t.erase_sets(TypeId::INT), TypeId::INT);
}

#[test]
fn subst_reaches_cells_and_set_parameters() {
    let mut t = TypeTable::default();
    let a = t.type_param(0);
    let cell_a = t.cell(a);
    let cell_int = t.cell(TypeId::INT);
    assert_eq!(t.subst(cell_a, &[TypeId::INT]), cell_int);
    // A set naming parameter 0, instantiated with a failing set.
    let with_a = t.intern(Type::Set(SetSummary {
        params: vec![a],
        ..SetSummary::default()
    }));
    let failing = t.intern(Type::Set(SetSummary {
        fails: true,
        ..SetSummary::default()
    }));
    assert_eq!(t.subst(with_a, &[failing]), failing);
    // Instantiated with another set type parameter (a self reference passing its own), or with
    // none, the set still names one.
    let b = t.type_param(1);
    let with_b = t.intern(Type::Set(SetSummary {
        params: vec![b],
        ..SetSummary::default()
    }));
    assert_eq!(t.subst(with_a, &[b]), with_b);
    assert_eq!(t.subst(with_a, &[]), with_a);
}

/// While a set type parameter is left, the old flags stay, raised by the known parts; once none is
/// left, the flags are the sets'.
#[test]
fn fn_with_sets_keeps_flags_while_parameters_remain() {
    let mut t = TypeTable::default();
    let a = t.type_param(0);
    let open = t.intern(Type::Set(SetSummary {
        params: vec![a],
        ..SetSummary::default()
    }));
    let empty = t.intern(Type::Set(SetSummary::default()));
    let old = FnType {
        fails: true,
        suspends: true,
        ev: true,
        ..FnType::pure(vec![], TypeId::INT)
    };
    assert_eq!(
        fn_with_sets(&t, vec![], TypeId::INT, &old, open, empty),
        FnType {
            sets: Some(FnSets {
                errors: open,
                effects: empty,
            }),
            ..old.clone()
        }
    );
    assert_eq!(
        fn_with_sets(&t, vec![], TypeId::INT, &old, empty, open),
        FnType {
            sets: Some(FnSets {
                errors: empty,
                effects: open,
            }),
            ..old.clone()
        }
    );
    assert_eq!(
        fn_with_sets(&t, vec![], TypeId::INT, &old, empty, empty),
        FnType::pure(vec![], TypeId::INT)
    );
}

#[test]
fn assert_operands_in_order_and_location() {
    let mut m = Module::new();
    let (a, b) = (m.local("a", TypeId::INT), m.local("b", TypeId::INT));
    let shown = Assert::shown(
        bool(true),
        BinOp::Eq,
        TypeId::INT,
        (a, int(1)),
        (b, int(2)),
        "a == b",
    );
    let mut e = Assert::at(shown, Some("main.emel:1:1".to_owned()));
    // `let a = 1; let b = 2; assert`.
    let mut inner = &mut e;
    while let Expr::Let { body, .. } = inner {
        inner = body;
    }
    let Expr::Assert(assert) = inner else {
        panic!("{inner:?}")
    };
    assert_eq!(assert.location.as_deref(), Some("main.emel:1:1"));
    let mut seen = Vec::new();
    assert.for_each_operand_mut(&mut |e| seen.push(e.clone()));
    assert_eq!(seen, [bool(true), var(a), var(b)]);
    let Expr::Assert(mut compare) =
        Assert::compare(BinOp::Lt, OpTy::Int, TypeId::INT, int(1), int(2), "1 < 2")
    else {
        unreachable!()
    };
    seen.clear();
    compare.for_each_operand_mut(&mut |e| seen.push(e.clone()));
    assert_eq!(seen, [int(1), int(2)]);
}

#[test]
fn type_of_matches_calls_and_fibers() {
    let mut m = Module::new();
    let some = m.some_ctor();
    let int_fn = m.fn_type(vec![], TypeId::INT);
    let (n, f) = (m.local("n", TypeId::NEVER), m.local("f", int_fn));
    // `fn with_ev(x: String, ev) -> Int`: its value type leaves out the evidence table.
    let with_ev = m.declare("with_ev", false);
    let (x, ev) = (
        m.local("x", TypeId::STRING),
        m.local("ev", TypeId::EVIDENCE),
    );
    m.define(with_ev, vec![x, ev], TypeId::INT, int(1));
    let with_ev_ty = m.func(FnType {
        ev: true,
        ..FnType::pure(vec![TypeId::STRING], TypeId::INT)
    });
    let some_int = || ctor_ty(some, vec![TypeId::INT], vec![int(1)]);
    let exprs = [
        (
            match_(int(1), vec![arm(Pat::Wild, string("s"))]),
            TypeId::STRING,
        ),
        (
            match_(
                int(1),
                vec![arm(Pat::Wild, var(n)), arm(Pat::Wild, string("s"))],
            ),
            TypeId::STRING,
        ),
        // The body decides unless it is `Never`.
        (
            escape(int(1), vec![arm(Pat::Wild, string("s"))]),
            TypeId::INT,
        ),
        (
            escape(var(n), vec![arm(Pat::Wild, string("s"))]),
            TypeId::STRING,
        ),
        (call_value(var(f), vec![]), TypeId::INT),
        (call_value(var(n), vec![]), TypeId::NEVER),
        (call_value(int(1), vec![]), TypeId::ANY),
        (fn_ref(with_ev), with_ev_ty),
        (fiber(FiberOp::Join, vec![some_int()]), TypeId::INT),
        (fiber(FiberOp::DeferredAwait, vec![var(n)]), TypeId::NEVER),
        (fiber(FiberOp::Join, vec![int(1)]), TypeId::ANY),
    ];
    let mut t = Typer::new(&m);
    for (e, ty) in &exprs {
        assert_eq!(t.type_of(e), *ty, "{e:?}");
    }
}

/// A typer returns its table only when it added types.
#[test]
fn typer_returns_the_types_it_added() {
    let m = Module::new();
    let ints = list(TypeId::INT, vec![]);
    let mut t = Typer::new(&m);
    t.type_of(&ints);
    assert!(Typer::new(&m).into_types().is_none());
    let types = t.into_types().expect("List[Int] is new");
    assert_eq!(types.len(), m.types.len() + 1);
    assert!(types.get(&Type::List(TypeId::INT)).is_some());
}

fn tuple(es: Vec<Expr>) -> Expr {
    Expr::Tuple(es)
}
