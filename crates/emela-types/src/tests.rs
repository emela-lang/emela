use smol_str::SmolStr;

use crate::*;

fn cons() -> (TyCons, TyConId, TyConId) {
    let mut cons = TyCons::new();
    let pair = cons.alloc(TyConData {
        name: "Pair".into(),
        kind: TyConKind::Type,
        params: vec![TyParam::new("A"), TyParam::new("B")],
    });
    let option = cons.alloc(TyConData {
        name: "Option".into(),
        kind: TyConKind::Enum,
        params: vec![TyParam::new("A")],
    });
    (cons, pair, option)
}

fn show(cx: &mut InferCtx, cons: &TyCons, ty: &Ty) -> String {
    cx.resolve(ty).display(cons).to_string()
}

// 単一化の成功

#[test]
fn unify_var_with_concrete() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    let ty = Ty::list(Ty::INT);
    cx.unify(&a, &ty).unwrap();
    assert_eq!(show(&mut cx, &cons, &a), "List[Int]");
}

#[test]
fn unify_structural_binds_inner_vars() {
    let (cons, pair, _) = cons();
    let mut cx = InferCtx::new();
    let (a, b, c) = (cx.new_var(), cx.new_var(), cx.new_var());
    // fn(?a, Pair[?b, String]) -> ?c  ~  fn(Int, Pair[Bool, String]) -> (Int, ?a)
    let lhs = Ty::func(
        [a.clone(), Ty::named(pair, [b.clone(), Ty::STRING])],
        c.clone(),
    );
    let rhs = Ty::func(
        [Ty::INT, Ty::named(pair, [Ty::BOOL, Ty::STRING])],
        Ty::tuple([Ty::INT, a.clone()]),
    );
    cx.unify(&lhs, &rhs).unwrap();
    assert_eq!(show(&mut cx, &cons, &a), "Int");
    assert_eq!(show(&mut cx, &cons, &b), "Bool");
    assert_eq!(show(&mut cx, &cons, &c), "(Int, Int)");
}

#[test]
fn unify_var_chain() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let (a, b, c) = (cx.new_var(), cx.new_var(), cx.new_var());
    cx.unify(&a, &b).unwrap();
    cx.unify(&b, &c).unwrap();
    cx.unify(&c, &Ty::FLOAT).unwrap();
    assert_eq!(show(&mut cx, &cons, &a), "Float");
}

// 単一化の失敗

#[test]
fn mismatch_reports_expected_and_actual() {
    let mut cx = InferCtx::new();
    let err = cx.unify(&Ty::INT, &Ty::STRING).unwrap_err();
    assert_eq!(err.expected, Ty::INT);
    assert_eq!(err.actual, Ty::STRING);
    assert_eq!(
        *err.kind,
        TypeErrorKind::Mismatch {
            expected: Ty::INT,
            actual: Ty::STRING
        }
    );
}

#[test]
fn mismatch_reports_inner_part() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let err = cx
        .unify(&Ty::list(Ty::INT), &Ty::list(Ty::BOOL))
        .unwrap_err();
    assert_eq!(err.expected.display(&cons).to_string(), "List[Int]");
    assert_eq!(err.actual.display(&cons).to_string(), "List[Bool]");
    let TypeErrorKind::Mismatch { expected, actual } = *err.kind else {
        panic!("Mismatch でない")
    };
    assert_eq!((expected, actual), (Ty::INT, Ty::BOOL));
}

#[test]
fn nominal_types_differ_by_name() {
    let (_, pair, option) = cons();
    let mut cx = InferCtx::new();
    let p = Ty::named(pair, [Ty::INT, Ty::INT]);
    let o = Ty::named(option, [Ty::INT]);
    assert!(cx.unify(&p, &o).is_err());
    // タプルと同じ形でも名前付き型とは別．
    assert!(cx.unify(&p, &Ty::tuple([Ty::INT, Ty::INT])).is_err());
}

#[test]
fn arity_mismatch_is_error() {
    let mut cx = InferCtx::new();
    let f1 = Ty::func([Ty::INT], Ty::INT);
    let f2 = Ty::func([Ty::INT, Ty::INT], Ty::INT);
    assert!(cx.unify(&f1, &f2).is_err());
    let t2 = Ty::tuple([Ty::INT, Ty::INT]);
    let t3 = Ty::tuple([Ty::INT, Ty::INT, Ty::INT]);
    assert!(cx.unify(&t2, &t3).is_err());
}

#[test]
fn failed_unify_rolls_back_bindings() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    // 先頭の要素で ?a = Int まで進んでから，2つ目で失敗する．
    let lhs = Ty::tuple([a.clone(), Ty::INT]);
    let rhs = Ty::tuple([Ty::INT, Ty::BOOL]);
    let err = cx.unify(&lhs, &rhs).unwrap_err();
    assert_eq!(show(&mut cx, &cons, &a), "?0");
    assert_eq!(err.expected.display(&cons).to_string(), "(?0, Int)");
}

#[test]
fn errors_are_collected() {
    let mut cx = InferCtx::new();
    assert!(!cx.expect(&Ty::INT, &Ty::BOOL));
    assert!(cx.expect(&Ty::INT, &Ty::INT));
    assert!(!cx.expect(&Ty::STRING, &Ty::FLOAT));
    assert_eq!(cx.errors().len(), 2);
    let errors = cx.take_errors();
    assert_eq!(errors[1].expected, Ty::STRING);
    assert_eq!(errors[1].actual, Ty::FLOAT);
    assert!(cx.errors().is_empty());
}

// 出現検査

#[test]
fn occurs_check_rejects_infinite_type() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    let err = cx.unify(&a, &Ty::list(a.clone())).unwrap_err();
    let TypeErrorKind::InfiniteType { var, ty } = &*err.kind else {
        panic!("InfiniteType でない: {err:?}")
    };
    assert_eq!(Ty::Var(*var), a);
    assert_eq!(ty.display(&cons).to_string(), "List[?0]");
    // 失敗の後も ?0 は未束縛のまま．
    assert_eq!(show(&mut cx, &cons, &a), "?0");
}

#[test]
fn occurs_check_through_bound_var() {
    let mut cx = InferCtx::new();
    let (a, b) = (cx.new_var(), cx.new_var());
    cx.unify(&b, &Ty::func([a.clone()], Ty::INT)).unwrap();
    // ?a ~ (?b, Int) は ?b = fn(?a) -> Int を辿ると ?a が現れる．
    let err = cx.unify(&a, &Ty::tuple([b, Ty::INT])).unwrap_err();
    assert!(matches!(*err.kind, TypeErrorKind::InfiniteType { .. }));
}

#[test]
fn unify_var_with_itself_is_ok() {
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    cx.unify(&a, &a).unwrap();
}

// Never

#[test]
fn join_with_never_takes_other_side() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    assert_eq!(cx.join(&Ty::Never, &Ty::INT).unwrap(), Ty::INT);
    assert_eq!(cx.join(&Ty::STRING, &Ty::Never).unwrap(), Ty::STRING);
    assert_eq!(cx.join(&Ty::Never, &Ty::Never).unwrap(), Ty::Never);
    // 型変数は Never に束縛しない（後で分かる型を残す）．
    let a = cx.new_var();
    let j = cx.join(&a, &Ty::Never).unwrap();
    assert_eq!(show(&mut cx, &cons, &j), "?0");
    assert_eq!(show(&mut cx, &cons, &a), "?0");
}

#[test]
fn join_unifies_both_branches() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    let j = cx.join(&a, &Ty::list(Ty::BOOL)).unwrap();
    assert_eq!(show(&mut cx, &cons, &j), "List[Bool]");
    assert_eq!(show(&mut cx, &cons, &a), "List[Bool]");

    let err = cx.join(&Ty::INT, &Ty::STRING).unwrap_err();
    assert_eq!((err.expected, err.actual), (Ty::INT, Ty::STRING));
    assert_eq!(cx.join_or_report(&Ty::INT, &Ty::BOOL), Ty::INT);
    assert_eq!(cx.errors().len(), 1);
}

#[test]
fn never_is_not_equal_to_other_types() {
    let mut cx = InferCtx::new();
    // 部分型はないので，単一化では Never と Int は別の型．
    assert!(cx.unify(&Ty::INT, &Ty::Never).is_err());
    // 期待した型の位置に置くときだけ，Never はどの型にもなれる．
    assert!(cx.coerce(&Ty::INT, &Ty::Never).is_ok());
    assert!(cx.coerce(&Ty::Never, &Ty::INT).is_err());
}

// 汎化と具体化

#[test]
fn generalize_identity() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    let id = Ty::func([a.clone()], a);
    let scheme = cx.generalize(&id, &[]);
    assert_eq!(scheme.params.len(), 1);
    assert_eq!(scheme.display(&cons).to_string(), "∀ A. fn(A) -> A");
}

#[test]
fn generalize_skips_env_vars() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let (a, b) = (cx.new_var(), cx.new_var());
    let Ty::Var(av) = a else { unreachable!() };
    let ty = Ty::func([a, b.clone()], b);
    let scheme = cx.generalize(&ty, &[av]);
    assert_eq!(scheme.display(&cons).to_string(), "∀ A. fn(?0, A) -> A");
}

#[test]
fn generalize_follows_bindings() {
    let (cons, pair, _) = cons();
    let mut cx = InferCtx::new();
    let (a, b, c) = (cx.new_var(), cx.new_var(), cx.new_var());
    cx.unify(&a, &Ty::named(pair, [b.clone(), c.clone()]))
        .unwrap();
    cx.unify(&b, &c).unwrap();
    let scheme = cx.generalize(&Ty::func([a], Ty::INT), &[]);
    assert_eq!(
        scheme.display(&cons).to_string(),
        "∀ A. fn(Pair[A, A]) -> Int"
    );
}

#[test]
fn instantiate_gives_fresh_vars() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let scheme = Scheme::new(
        vec![TyParam::new("A"), TyParam::new("B")],
        Ty::func(
            [Ty::Bound(0), Ty::Bound(1)],
            Ty::tuple([Ty::Bound(1), Ty::Bound(0)]),
        ),
    );
    let t1 = cx.instantiate(&scheme);
    let t2 = cx.instantiate(&scheme);
    assert_eq!(show(&mut cx, &cons, &t1), "fn(?0, ?1) -> (?1, ?0)");
    assert_eq!(show(&mut cx, &cons, &t2), "fn(?2, ?3) -> (?3, ?2)");
    // 具体化した型は互いに独立に単一化できる．
    cx.unify(
        &t1,
        &Ty::func([Ty::INT, Ty::BOOL], Ty::tuple([Ty::BOOL, Ty::INT])),
    )
    .unwrap();
    cx.unify(
        &t2,
        &Ty::func([Ty::STRING, Ty::FLOAT], Ty::tuple([Ty::FLOAT, Ty::STRING])),
    )
    .unwrap();
    assert_eq!(show(&mut cx, &cons, &t1), "fn(Int, Bool) -> (Bool, Int)");
}

#[test]
fn generalize_instantiate_round_trip() {
    let (_, _, option) = cons();
    let mut cx = InferCtx::new();
    let (a, b) = (cx.new_var(), cx.new_var());
    let ty = Ty::func(
        [Ty::func([a.clone()], b.clone()), Ty::named(option, [a])],
        Ty::named(option, [b]),
    );
    let scheme = cx.generalize(&ty, &[]);
    let inst = cx.instantiate(&scheme);
    let again = cx.generalize(&inst, &[]);
    assert_eq!(scheme, again);
}

#[test]
fn mono_scheme_instantiates_to_itself() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let scheme = Scheme::mono(Ty::list(Ty::INT));
    assert!(scheme.is_mono());
    assert_eq!(cx.instantiate(&scheme), Ty::list(Ty::INT));
    assert_eq!(scheme.display(&cons).to_string(), "List[Int]");
}

// 表示

#[test]
fn display_types() {
    let (cons, pair, option) = cons();
    let d = |ty: Ty| ty.display(&cons).to_string();
    assert_eq!(d(Ty::INT), "Int");
    assert_eq!(d(Ty::INT64), "Int64");
    assert_eq!(d(Ty::FLOAT), "Float");
    assert_eq!(d(Ty::BOOL), "Bool");
    assert_eq!(d(Ty::STRING), "String");
    assert_eq!(d(Ty::UNIT), "()");
    assert_eq!(d(Ty::Never), "Never");
    assert_eq!(d(Ty::list(Ty::INT)), "List[Int]");
    assert_eq!(d(Ty::func([Ty::INT], Ty::BOOL)), "fn(Int) -> Bool");
    assert_eq!(d(Ty::func([], Ty::UNIT)), "fn() -> ()");
    assert_eq!(d(Ty::tuple([Ty::INT, Ty::STRING])), "(Int, String)");
    assert_eq!(d(Ty::named(pair, [Ty::INT, Ty::BOOL])), "Pair[Int, Bool]");
    assert_eq!(
        d(Ty::named(option, [Ty::list(Ty::named(option, [Ty::INT]))])),
        "Option[List[Option[Int]]]"
    );
    assert_eq!(
        d(Ty::func(
            [Ty::func([Ty::INT], Ty::INT)],
            Ty::func([Ty::INT], Ty::INT)
        )),
        "fn(fn(Int) -> Int) -> fn(Int) -> Int"
    );
    assert_eq!(d(Ty::Bound(0)), "'0");
}

#[test]
fn display_vars() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let _ = cx.new_var();
    let b = cx.new_var();
    assert_eq!(b.display(&cons).to_string(), "?1");
}

#[test]
fn display_scheme_with_bounds() {
    let (cons, _, _) = cons();
    let scheme = Scheme::new(
        vec![
            TyParam::with_bounds("A", [SmolStr::new("Ord"), SmolStr::new("Show")]),
            TyParam::new("B"),
        ],
        Ty::func([Ty::list(Ty::Bound(0))], Ty::Bound(1)),
    );
    assert_eq!(
        scheme.display(&cons).to_string(),
        "∀ A: Ord + Show, B. fn(List[A]) -> B"
    );
}
