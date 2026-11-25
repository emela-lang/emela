//! Tests for bounds-check elimination (`opt::reduce::bounds`, A11/PR-040).
//!
//! The Option fusion turns `match Array.get(a, i) { … }` into a bounds test around
//! `ArrayLoadUnchecked(a, i)`; this rule removes the test when the index is provably in the array.
//! The tests pin the three sides of the contract: a provable index loses the check, an index that
//! may be out of range keeps it (the defect and the else branch survive), and an index the range
//! analysis cannot bound deopts. The flag is off by default, so `Rules::none()` must leave the test
//! exactly where the fusion built it.

use crate::support;

use emela_core::intrinsics::Builtin;
use emela_core::opt::{CoreFn, CoreFns, Options, Rules, Stats, run_with};
use emela_core::{BinOp, Expr, Lit, Local, Module, OpTy, Type, TypeId, dump, opt, verify};
use support::{lower_files_with, lower_files_with_rules};

/// A straight-line `match Array.get` on an array whose length is written in the source.
const LITERAL_ARRAY: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([10, 20, 30])
  dbg(Array.get(a, 1) match {
    Some(x) -> x
    None -> -1
  })
}
",
)];

/// The residual case: the range fusion builds the loop and the Option fusion builds the test, but
/// the unswitch cannot move the test out of the loop because its index is the loop counter.
const FUSED_LOOP: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([10, 20, 30])
  dbg(List.fold(List.range(0, 3), 0, |acc, i| acc + (Array.get(a, i) match {
    Some(x) -> x
    None -> 0
  })))
}
",
)];

/// The array's length comes from the range fusion's `ArrayMapRange`, not from a list literal.
const MAP_RANGE: &[(&str, &str)] = &[(
    "main.emel",
    "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list(List.map(List.range(0, 3), |x| x * 2))
  dbg(Array.get(a, 1) match {
    Some(x) -> x
    None -> -1
  })
}
",
)];

/// `bounds` with the rest of the reduction kernel off: the fusion laws stay on, so the checked
/// shape exists and only this rule can remove it.
const BOUNDS_ONLY: Rules = Rules {
    bounds: true,
    ..Rules::none()
};

/// An `Int` literal.
fn lit(value: i32) -> Expr {
    Expr::Lit(Lit::Int(value))
}

/// A read of `local`.
fn var(local: Local) -> Expr {
    Expr::Var(local)
}

/// An `Int` binary operation with an explicit operand type.
fn binary(op: BinOp, l: Expr, r: Expr) -> Expr {
    Expr::Binary {
        op,
        ty: OpTy::Int,
        lhs: Box::new(l),
        rhs: Box::new(r),
    }
}

/// `l + r` on `Int`.
fn add(l: Expr, r: Expr) -> Expr {
    binary(BinOp::Add, l, r)
}

/// `l < r` on `Int`.
fn less(l: Expr, r: Expr) -> Expr {
    binary(BinOp::Lt, l, r)
}

/// The bounds test and the load the Option fusion builds for `Array.get(a, index)`, with `0` for
/// the `None` arm and `x` bound to the element.
fn bounds_test(a: Local, index: Local, x: Local) -> Expr {
    let lower = Expr::Binary {
        op: BinOp::Ge,
        ty: OpTy::Int,
        lhs: Box::new(var(index)),
        rhs: Box::new(lit(0)),
    };
    let upper = Expr::Binary {
        op: BinOp::Lt,
        ty: OpTy::Int,
        lhs: Box::new(var(index)),
        rhs: Box::new(Expr::Builtin {
            op: Builtin::ArraySize,
            ty_args: vec![TypeId::INT],
            args: vec![var(a)],
        }),
    };
    let cond = Expr::Binary {
        op: BinOp::And,
        ty: OpTy::Bool,
        lhs: Box::new(lower),
        rhs: Box::new(upper),
    };
    let load = Expr::Builtin {
        op: Builtin::ArrayLoadUnchecked,
        ty_args: vec![TypeId::INT],
        args: vec![var(a), var(index)],
    };
    Expr::If {
        cond: Box::new(cond),
        then: Box::new(Expr::Let {
            var: x,
            value: Box::new(load),
            body: Box::new(var(x)),
        }),
        else_: Box::new(lit(0)),
    }
}

/// The body of `main` from the dump, without the definitions of the module.
fn main_ir(m: &Module) -> String {
    function_ir(m, "export fn main")
}

/// The body of the function the dump writes after `header`, without the next function's header.
fn function_ir(m: &Module, header: &str) -> String {
    let ir = dump(m);
    let part = ir
        .split(header)
        .nth(1)
        .unwrap_or_else(|| panic!("`{header}` is not in the dump:\n{ir}"));
    part.split("\nfn ").next().unwrap().to_string()
}

/// Rebuilds the `core_fns` map lowering passes to `run`, by IR name, as `tests/opt.rs` does.
///
/// [`lower_files_with_rules`] uses an empty map, which is what isolates a reduction rule from the
/// fusions; a test that needs the `List.fold`/`List.map` range fusion and `Array.from_list` has to
/// supply the map, because lowering does not expose the one it built.
fn core_fns(m: &Module) -> CoreFns {
    let mut ids = CoreFns::default();
    for (id, f) in m.functions.iter() {
        let name = f.name.split_once('$').map_or(f.name.as_str(), |(n, _)| n);
        let kind = match name {
            "Option.or" => CoreFn::OptionOr,
            "List.fold" => CoreFn::ListFold,
            "List.map" => CoreFn::ListMap,
            "List.range" => CoreFn::ListRange,
            "List.reverse" => CoreFn::ListReverse,
            _ if name.starts_with("List.") => CoreFn::Collection,
            _ => continue,
        };
        ids.insert(id, kind);
    }
    ids
}

/// Lowers with the passes off, then runs the pipeline with the core functions and `rules`, so a
/// fusion that matches a bundled `List` function can fire before the rule under test.
fn lower_and_run(files: &[(&str, &str)], rules: Rules) -> (Module, Stats) {
    let mut m = lower_files_with(files, false);
    let ids = core_fns(&m);
    let stats = run_with(
        &mut m,
        &ids,
        Options {
            rules,
            ..Options::default()
        },
    );
    (m, stats)
}

/// With the rule on, the test of a literal index into an array of a written length is gone; the
/// load and the value stay. With it off, the fusion's test is exactly where it was.
#[test]
fn a_provable_index_removes_the_check() {
    let (m, stats) = lower_files_with_rules(LITERAL_ARRAY, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 1, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    let main = main_ir(&m);
    assert!(!main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
    verify(&m).expect("the rewrite keeps the module well typed");
}

/// `Rules::none()` does not run the rule, so the empty-flag pipeline keeps the test and its size
/// read. The Option fusion still fires: the law has its own flag, not a [`Rules`] one.
#[test]
fn rules_none_leaves_the_check() {
    let (m, stats) = lower_files_with_rules(LITERAL_ARRAY, Rules::none());
    assert_eq!(stats.bounds, 0, "{stats:?}");
    let main = main_ir(&m);
    assert!(main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
}

/// The residual case the unswitch leaves: the test is inside the loop and its index is the loop
/// counter, so the unswitch cannot copy it out. The loop's guard and the array's length prove it,
/// and the rule removes it without removing the loop.
#[test]
fn a_fused_loop_removes_the_test_the_unswitch_cannot_move() {
    let (off, off_stats) = lower_and_run(FUSED_LOOP, Rules::none());
    assert_eq!(off_stats.range_fold, 1, "{off_stats:?}");
    assert_eq!(off_stats.array_get_match, 1, "{off_stats:?}");
    assert_eq!(off_stats.bounds, 0, "{off_stats:?}");
    let off_main = main_ir(&off);
    assert!(off_main.contains("loop("), "{off_main}");
    assert!(off_main.contains("@Std.Array.size"), "{off_main}");
    assert!(off_main.contains("@$Array.load_unchecked"), "{off_main}");

    let (on, stats) = lower_and_run(FUSED_LOOP, BOUNDS_ONLY);
    assert_eq!(stats.range_fold, 1, "{stats:?}");
    assert_eq!(stats.array_get_match, 1, "{stats:?}");
    assert_eq!(stats.bounds, 1, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    let main = main_ir(&on);
    assert!(main.contains("loop("), "{main}");
    assert!(!main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
    verify(&on).expect("the rewrite keeps the module well typed");
}

/// The length of an `ArrayMapRange` is its constant extent, so the test of an index into it is
/// removed too. This is the array the range fusion builds without a list.
#[test]
fn a_constant_map_range_has_a_known_length() {
    let (m, stats) = lower_and_run(MAP_RANGE, BOUNDS_ONLY);
    assert_eq!(stats.map_range, 1, "{stats:?}");
    assert_eq!(stats.bounds, 1, "{stats:?}");
    let main = main_ir(&m);
    assert!(!main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
    verify(&m).expect("the rewrite keeps the module well typed");
}

/// An index equal to the length is out of range. The rule must keep the test and the `None` arm;
/// the defect is the program's, not the optimiser's to remove.
#[test]
fn an_index_that_reaches_the_length_keeps_the_check() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([10, 20, 30])
  dbg(Array.get(a, 3) match {
    Some(x) -> x
    None -> -1
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    let main = main_ir(&m);
    assert!(main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
    assert!(
        main.contains("(-[Int] 1)"),
        "the `None` arm is gone: {main}"
    );
}

/// An index from a parameter has no interval (`Top`), so the rule deopts even though the array's
/// length is written in the same function.
#[test]
fn an_index_from_a_parameter_deopts() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn f(j: Int) -> Int {
  a = Array.from_list([10, 20, 30])
  Array.get(a, j) match {
    Some(x) -> x
    None -> -1
  }
}

fn main() {
  dbg(f(1))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    let f = function_ir(&m, "fn f(");
    assert!(f.contains("@Std.Array.size"), "{f}");
    assert!(f.contains("@$Array.load_unchecked"), "{f}");
    assert!(f.contains("(-[Int] 1)"), "the `None` arm is gone: {f}");
}

/// A negative index is out of range: the lower bound fails, so the rule deopts and the check and
/// the `None` arm stay.
#[test]
fn a_negative_index_deopts() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([10, 20, 30])
  dbg(Array.get(a, -1) match {
    Some(x) -> x
    None -> -1
  })
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    let main = main_ir(&m);
    assert!(main.contains("@Std.Array.size"), "{main}");
    assert!(main.contains("@$Array.load_unchecked"), "{main}");
    assert!(
        main.contains("(-[Int] 1)"),
        "the `None` arm is gone: {main}"
    );
}

/// The rewrite keeps evaluation order: the array's elements are evaluated before the test and the
/// load, and the taken arm's effects run after the load. The `None` arm's effect is gone only
/// because the interval proves that arm never runs; with the rule off, it is still there.
#[test]
fn evaluation_order_and_effects_are_kept() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([dbg(10), dbg(20), dbg(30)])
  dbg(Array.get(a, 1) match {
    Some(x) -> dbg(x)
    None -> dbg(-1)
  })
}
",
    )];
    let (on, stats) = lower_files_with_rules(&files, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 1, "{stats:?}");
    let main = main_ir(&on);
    let load = main.find("@$Array.load_unchecked").expect(&main);
    for element in ["@dbg[Int](10)", "@dbg[Int](20)", "@dbg[Int](30)"] {
        let at = main
            .find(element)
            .unwrap_or_else(|| panic!("{element} is gone: {main}"));
        assert!(at < load, "{element} was moved after the load: {main}");
    }
    let arm = main.rfind("@dbg[Int]").expect(&main);
    assert!(
        load < arm,
        "the taken arm was moved before the load: {main}"
    );
    assert!(
        !main.contains("(-[Int] 1)"),
        "the `None` arm survived: {main}"
    );
    verify(&on).expect("the rewrite keeps the module well typed");

    let (off, _) = lower_files_with_rules(&files, Rules::none());
    let off_main = main_ir(&off);
    assert!(
        off_main.contains("(-[Int] 1)"),
        "with the rule off the `None` arm is part of the program: {off_main}"
    );
}

/// The `Option.or` fusion has no unchecked form that keeps its default argument's evaluation, so
/// the rule leaves `ArrayGetOr` alone even when the index and the length are known. Removing it
/// would drop the `dbg(-1)` evaluation.
#[test]
fn an_array_get_or_is_left_alone() {
    let files = [(
        "main.emel",
        "\
import Std.Array.{Array}

fn main() {
  a = Array.from_list([10, 20, 30])
  dbg(Array.get(a, 1) |> Option.or(dbg(-1)))
}
",
    )];
    let (m, stats) = lower_and_run(&files, BOUNDS_ONLY);
    assert_eq!(stats.option_or, 1, "{stats:?}");
    assert_eq!(stats.bounds, 0, "{stats:?}");
    let main = main_ir(&m);
    assert!(main.contains("@$Array.get_or"), "{main}");
    assert!(
        main.contains("@dbg[Int]") && main.contains("(-[Int] 1)"),
        "the default argument is no longer evaluated: {main}"
    );
}

/// The nested loop's induction has one entry per round of the outer loop's fixed point. The first
/// round sees only `j = 0`, but the program reaches `j = 2`, so the rule must join the entries and
/// deopt for a one-element array rather than prove from the first round. The IR is hand-built:
/// the range fusion does not produce a `let` between the counter and the guard.
#[test]
fn an_early_loop_round_cannot_prove_bounds() {
    let mut m = Module::new();
    let array = m.intern(Type::Array(TypeId::INT));
    let a = m.local("a", array);
    let j = m.local("j", TypeId::INT);
    let i = m.local("i", TypeId::INT);
    let bound = m.local("m", TypeId::INT);
    let inner_result = m.local("r", TypeId::INT);
    let applied = m.local("t", TypeId::INT);
    let element = m.local("x", TypeId::INT);

    let from_list = Expr::Builtin {
        op: Builtin::ArrayFromList,
        ty_args: vec![TypeId::INT],
        args: vec![Expr::List {
            elem: TypeId::INT,
            elems: vec![lit(10)],
            tail: None,
        }],
    };
    let inner = Expr::Loop {
        vars: vec![i],
        inits: vec![lit(0)],
        body: Box::new(Expr::If {
            cond: Box::new(less(var(i), var(bound))),
            then: Box::new(Expr::Let {
                var: applied,
                value: Box::new(bounds_test(a, i, element)),
                body: Box::new(Expr::Recur(vec![add(var(i), lit(1))])),
            }),
            else_: Box::new(lit(0)),
        }),
    };
    let outer = Expr::Loop {
        vars: vec![j],
        inits: vec![lit(0)],
        body: Box::new(Expr::Let {
            var: bound,
            value: Box::new(var(j)),
            body: Box::new(Expr::If {
                cond: Box::new(less(var(j), lit(3))),
                then: Box::new(Expr::Let {
                    var: inner_result,
                    value: Box::new(inner),
                    body: Box::new(Expr::Recur(vec![add(var(j), lit(1))])),
                }),
                else_: Box::new(lit(0)),
            }),
        }),
    };
    let f = m.declare("f", false);
    m.define(
        f,
        Vec::new(),
        TypeId::INT,
        Expr::Let {
            var: a,
            value: Box::new(from_list),
            body: Box::new(outer),
        },
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: BOUNDS_ONLY,
            ..Options::default()
        },
    );
    assert_eq!(stats.bounds, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    let ir = dump(&m);
    assert!(ir.contains("@Std.Array.size"), "{ir}");
    verify(&m).expect("the hand-built module is well typed");
}

/// A second run changes nothing: the test it would remove is gone, and the rule has no other site.
#[test]
fn a_second_run_changes_nothing() {
    let (mut m, stats) = lower_files_with_rules(LITERAL_ARRAY, BOUNDS_ONLY);
    assert_eq!(stats.bounds, 1, "{stats:?}");
    let once = dump(&m);
    let again = opt::run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: BOUNDS_ONLY,
            ..Options::default()
        },
    );
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(again.bounds, 0, "{again:?}");
    assert_eq!(dump(&m), once);
}
