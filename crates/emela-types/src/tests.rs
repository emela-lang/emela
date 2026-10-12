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
        panic!("expected Mismatch")
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
    // A named type differs from a tuple of the same shape.
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
    // Gets as far as ?a = Int on the first element, then fails on the second.
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

#[test]
fn occurs_check_rejects_infinite_type() {
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let a = cx.new_var();
    let err = cx.unify(&a, &Ty::list(a.clone())).unwrap_err();
    let TypeErrorKind::InfiniteType { var, ty } = &*err.kind else {
        panic!("expected InfiniteType: {err:?}")
    };
    assert_eq!(Ty::Var(*var), a);
    assert_eq!(ty.display(&cons).to_string(), "List[?0]");
    // ?0 stays unbound after the failure.
    assert_eq!(show(&mut cx, &cons, &a), "?0");
}

#[test]
fn occurs_check_through_bound_var() {
    let mut cx = InferCtx::new();
    let (a, b) = (cx.new_var(), cx.new_var());
    cx.unify(&b, &Ty::func([a.clone()], Ty::INT)).unwrap();
    // For ?a ~ (?b, Int), following ?b = fn(?a) -> Int reaches ?a.
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
    // Type variables are not bound to Never, so a type found later is kept.
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
    // There is no subtyping, so unification treats Never and Int as different types.
    assert!(cx.unify(&Ty::INT, &Ty::Never).is_err());
    // Only when placed where a type is expected can Never become any type.
    assert!(cx.coerce(&Ty::INT, &Ty::Never).is_ok());
    assert!(cx.coerce(&Ty::Never, &Ty::INT).is_err());
}

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
fn generalize_skips_vars_reached_from_bound_env_vars() {
    // If the environment's ?0 is bound to List[?1], ?1 is an environment variable too and is not
    // quantified.
    let (cons, _, _) = cons();
    let mut cx = InferCtx::new();
    let (a, b) = (cx.new_var(), cx.new_var());
    let Ty::Var(av) = a else { unreachable!() };
    cx.unify(&a, &Ty::list(b.clone())).unwrap();
    let ty = Ty::func([b.clone()], b);
    let scheme = cx.generalize(&ty, &[av]);
    assert_eq!(scheme.display(&cons).to_string(), "fn(?1) -> ?1");
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
    // Instantiated types unify independently of each other.
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
        "fn(fn(Int) -> Int fails {} use {}) -> fn(Int) -> Int"
    );
    assert_eq!(d(Ty::Bound(0)), "'0");
}

/// Makes a set tag (an error definition), registering its name for display.
fn tag(cons: &mut TyCons, i: u32, name: &str) -> emela_resolve::hir::DefId {
    let def = la_arena::Idx::from_raw(la_arena::RawIdx::from_u32(i));
    cons.set_tag_name(def, name.into());
    def
}

#[test]
fn display_fails() {
    let (mut cons, _, _) = cons();
    let not_found = tag(&mut cons, 0, "NotFound");
    let db = tag(&mut cons, 1, "DbError");
    let row = Row::from_elems([
        RowElem::Tag(Tag::new(db)),
        RowElem::Tag(Tag::new(not_found)),
    ]);
    let d = |ty: Ty, cons: &TyCons| ty.display(cons).to_string();
    assert_eq!(
        d(Ty::func_fails([Ty::INT], Ty::BOOL, row.clone()), &cons),
        "fn(Int) -> Bool fails NotFound | DbError"
    );
    // A function return type is parenthesized if the outer type has a fails clause.
    assert_eq!(
        d(
            Ty::func_fails(
                [],
                Ty::func_fails([], Ty::INT, Row::tag(db)),
                Row::tag(not_found)
            ),
            &cons
        ),
        "fn() -> (fn() -> Int fails DbError) fails NotFound"
    );
    assert_eq!(
        d(
            Ty::func([], Ty::func_fails([], Ty::INT, Row::tag(db))),
            &cons
        ),
        "fn() -> fn() -> Int fails DbError"
    );
    // Set type parameters and error types.
    let scheme = Scheme::new(
        vec![TyParam::new("A"), TyParam::set("E")],
        Ty::func_fails(
            [Ty::func_fails(
                [],
                Ty::Bound(0),
                Row::from_elems([RowElem::Tag(Tag::new(db)), RowElem::Bound(1)]),
            )],
            Ty::Bound(0),
            Row::from_elems([RowElem::Bound(1), RowElem::Of(Ty::Bound(0))]),
        ),
    );
    assert_eq!(
        scheme.display(&cons).to_string(),
        "∀ A, E. fn(fn() -> A fails DbError | E use {}) -> A fails E | A"
    );
    assert_eq!(Row::empty().display(&cons).to_string(), "{}");
}

#[test]
fn display_uses() {
    let (mut cons, _, _) = cons();
    let logger = tag(&mut cons, 0, "Logger");
    let clock = tag(&mut cons, 1, "Clock");
    let boom = tag(&mut cons, 2, "Boom");
    let d = |ty: Ty, cons: &TyCons| ty.display(cons).to_string();
    // One effect is shown bare, two or more in braces (spec 17.4), after fails.
    assert_eq!(
        d(
            Ty::func_sets([], Ty::UNIT, Row::empty(), Row::tag(logger)),
            &cons
        ),
        "fn() -> () use Logger"
    );
    assert_eq!(
        d(
            Ty::func_sets(
                [],
                Ty::UNIT,
                Row::tag(boom),
                Row::from_elems([
                    RowElem::Tag(Tag::new(logger)),
                    RowElem::Tag(Tag::new(clock))
                ])
            ),
            &cons
        ),
        "fn() -> () fails Boom use { Logger, Clock }"
    );
    // A function return type is parenthesized if the outer type has a use clause.
    assert_eq!(
        d(
            Ty::func_sets([], Ty::func([], Ty::INT), Row::empty(), Row::tag(logger)),
            &cons
        ),
        "fn() -> (fn() -> Int) use Logger"
    );
    // Set type parameters.
    let scheme = Scheme::new(
        vec![TyParam::set("R")],
        Ty::func_sets(
            [Ty::func_sets(
                [],
                Ty::INT,
                Row::empty(),
                Row::from_elems([RowElem::Bound(0)]),
            )],
            Ty::INT,
            Row::empty(),
            Row::from_elems([RowElem::Tag(Tag::new(clock)), RowElem::Bound(0)]),
        ),
    );
    assert_eq!(
        scheme.display(&cons).to_string(),
        "∀ R. fn(fn() -> Int fails {} use R) -> Int use { Clock, R }"
    );
    // A capability value's type is shown as the expression that creates it.
    assert_eq!(d(Ty::Cap(Tag::new(logger)), &cons), "use Logger");
}

/// Function types' E is not equated by unification but turned into inclusion constraints,
/// reversed inside parameters.
#[test]
fn unify_relates_fails_by_variance() {
    let (mut cons, _, _) = cons();
    let x = tag(&mut cons, 0, "X");
    let mut cx = InferCtx::new();
    let v = cx.new_row_var();
    let w = cx.new_row_var();
    let expected = Ty::func_fails(
        [Ty::func_fails([], Ty::INT, Row::var(w))],
        Ty::INT,
        Row::var(v),
    );
    let actual = Ty::func_fails([Ty::func([], Ty::INT)], Ty::INT, Row::tag(x));
    cx.unify(&expected, &actual).unwrap();
    let shown: Vec<String> = cx
        .subsets()
        .iter()
        .map(|c| format!("{} <= {}", c.sub.display(&cons), c.sup.display(&cons)))
        .collect();
    assert_eq!(shown, ["?e1 <= {}", "X <= ?e0"]);
}

/// When unification fails, the constraints created along the way are dropped.
#[test]
fn failed_unify_drops_row_constraints() {
    let (mut cons, _, _) = cons();
    let x = tag(&mut cons, 0, "X");
    let mut cx = InferCtx::new();
    let v = cx.new_row_var();
    let expected = Ty::tuple([Ty::func_fails([], Ty::INT, Row::var(v)), Ty::INT]);
    let actual = Ty::tuple([Ty::func_fails([], Ty::INT, Row::tag(x)), Ty::STRING]);
    assert!(cx.unify(&expected, &actual).is_err());
    assert!(cx.subsets().is_empty());
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
