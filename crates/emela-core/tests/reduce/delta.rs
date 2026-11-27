//! Tests for δ reduction (`emela_core::opt::reduce::delta`), the direct-call inlining.
//!
//! δ is the inliner that ran before the reduction kernel; A10/PR-019 put it behind
//! `emela_core::opt::Rules::delta`, which is on by default. These tests lower with the
//! optimisations on for the shape tests and call [`opt::run`] directly where they need a
//! hand-built module. A test that asserts the exact δ count runs with only δ on, so another rule
//! (β, ζ or folding) cannot remove the shape or change the count.
//!
//! The interesting half is what must not change: a const (whose entry point is a runtime cell), an
//! indirect call, a compiler-generated `$` helper and a trait or derived impl stay calls, and a
//! site over the expansion budget is left whole rather than copied in part.

use crate::support;

use emela_core::opt::{CoreFns, Options, Rules, Stats, run_with};
use emela_core::{BinOp, Callee, Expr, FnId, Lit, Module, OpTy, TypeId, dump, opt, verify};
use support::{lower_files, lower_files_with, lower_files_with_stats};

/// Only δ is on: another rule may not change the shape or the count a test asserts.
const DELTA: Rules = Rules {
    delta: true,
    ..Rules::none()
};

/// Runs only δ on `m`.
fn delta_only(m: &mut Module) -> Stats {
    run_with(
        m,
        &CoreFns::default(),
        Options {
            rules: DELTA,
            ..Options::default()
        },
    )
}

/// The one function called `name`.
fn function(m: &Module, name: &str) -> FnId {
    let found: Vec<FnId> = m
        .functions
        .iter()
        .filter(|(_, f)| f.name == name)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(found.len(), 1, "functions named `{name}`: {found:?}");
    found[0]
}

/// The number of direct calls to `target` in `e`.
fn direct_calls(e: &Expr, target: FnId) -> usize {
    let mut count = match e {
        Expr::Call {
            callee: Callee::Direct { f, .. },
            ..
        } if *f == target => 1,
        _ => 0,
    };
    e.for_each_child(&mut |c| count += direct_calls(c, target));
    count
}

/// The number of direct calls to `target` in the body of `owner`.
fn calls_in(m: &Module, owner: FnId, target: FnId) -> usize {
    direct_calls(&m.functions[owner].body, target)
}

/// The number of direct calls to `target` anywhere in the module.
fn calls_anywhere(m: &Module, target: FnId) -> usize {
    m.functions
        .iter()
        .map(|(_, f)| direct_calls(&f.body, target))
        .sum()
}

/// Whether `e` contains an indirect call.
fn has_indirect_call(e: &Expr) -> bool {
    let mut found = matches!(
        e,
        Expr::Call {
            callee: Callee::Indirect(_),
            ..
        }
    );
    e.for_each_child(&mut |c| found |= has_indirect_call(c));
    found
}

/// A direct call to a small pure function is expanded.
#[test]
fn a_direct_call_to_a_small_pure_function_is_expanded() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn main() {
  dbg(double(21))
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    assert_eq!(stats.delta, 1, "{stats:?}");
    let double = function(&m, "double");
    assert_eq!(calls_anywhere(&m, double), 0, "{}", dump(&m));
    assert!(dump(&m).contains("*[Int] 2"), "{}", dump(&m));
}

/// A const is not expanded: its value is evaluated once, through a zero-argument entry point that
/// guards the runtime cell. Copying the initializer to each reference would evaluate it again.
#[test]
fn a_const_is_not_expanded() {
    let files = [(
        "main.emel",
        "\
const LIMIT: Int = 3

fn main() {
  dbg(LIMIT)
}
",
    )];
    let (m, stats) = lower_files_with_stats(&files, true);
    let limit = function(&m, "LIMIT");
    assert!(m.functions[limit].constant, "{}", dump(&m));
    assert_eq!(calls_anywhere(&m, limit), 1, "{}", dump(&m));
    assert_eq!(stats.delta, 0, "{stats:?}");
    assert!(
        stats.deopt > 0,
        "the call to the const is a deopt: {stats:?}"
    );
}

/// An indirect call is not expanded, whichever function contains it. `apply` itself is direct and
/// is copied in; the call through its parameter stays indirect.
#[test]
fn an_indirect_call_is_left_alone() {
    let files = [(
        "main.emel",
        "\
fn apply(x: Int, f: fn(Int) -> Int fails {} uses {}) -> Int {
  f(x)
}

fn main() {
  dbg(apply(1, |y| y + 1))
}
",
    )];
    let mut m = lower_files(&files);
    let stats = delta_only(&mut m);
    let apply = function(&m, "apply");
    assert_eq!(calls_anywhere(&m, apply), 0, "{}", dump(&m));
    let main = function(&m, "main");
    assert!(has_indirect_call(&m.functions[main].body), "{}", dump(&m));
    assert!(stats.delta >= 1, "{stats:?}");
}

/// A compiler-generated helper (a name with `$`) is not copied.
#[test]
fn a_compiler_generated_helper_is_left_alone() {
    let files = [(
        "main.emel",
        "\
fn helper(x: Int) -> Int {
  x + 1
}

fn main() {
  dbg(helper(1))
}
",
    )];
    let mut m = lower_files(&files);
    let helper = function(&m, "helper");
    m.functions[helper].name = "helper$init".to_string();
    let stats = delta_only(&mut m);
    assert_eq!(calls_anywhere(&m, helper), 1, "{}", dump(&m));
    assert_eq!(stats.delta, 0, "{stats:?}");
    assert!(stats.deopt > 0, "{stats:?}");
}

/// A trait function stays a call: monomorphization, not δ, chooses the impl. That includes a
/// derived impl such as `Show.show` for a user type.
#[test]
fn a_derived_impl_is_left_alone() {
    let files = [(
        "main.emel",
        "\
type User(name: String) derive Show

fn main() {
  dbg(Show.show(User(name: \"a\")))
}
",
    )];
    let mut m = lower_files(&files);
    let show = function(&m, "Show.show");
    let main = function(&m, "main");
    let stats = delta_only(&mut m);
    assert_eq!(calls_in(&m, main, show), 1, "{}", dump(&m));
    assert!(stats.deopt > 0, "{stats:?}");
}

/// A call whose expansion would exceed the budget is abandoned whole. The callee stays a call and
/// the cheap pre-check allocates no fresh local for the doomed copy.
#[test]
fn an_expansion_over_the_budget_is_left_whole() {
    let mut m = Module::new();
    // `h` has 19 nodes, small enough to be a candidate.
    let h = m.declare("h", false);
    m.define(h, Vec::new(), TypeId::INT, sum_of(10));
    // `top` calls `h` eight times: 15 nodes before δ, 8 * 19 + 7 = 159 nodes after the calls to
    // `h` are expanded. Eight calls of 20 nodes each do not fit the 128-node budget either.
    let top = m.declare("top", false);
    let mut body = call(h);
    for _ in 1..8 {
        body = Expr::Binary {
            op: BinOp::Add,
            ty: OpTy::Int,
            lhs: Box::new(body),
            rhs: Box::new(call(h)),
        };
    }
    m.define(top, Vec::new(), TypeId::INT, body);
    let main = m.declare("main", true);
    m.define(main, Vec::new(), TypeId::INT, call(top));

    // Expanding `h` needs no local, and the pre-check rejects `top` before it allocates one, so a
    // deopt leaves the local arena as it was.
    let locals = m.locals.len();
    let stats = delta_only(&mut m);
    assert_eq!(calls_anywhere(&m, top), 1, "{}", dump(&m));
    assert_eq!(m.locals.len(), locals, "{}", dump(&m));
    assert_eq!(
        stats.delta, 8,
        "only the calls to `h` are expanded: {stats:?}"
    );
    verify(&m).unwrap();
}

/// The same source lowered twice gives the same module, and a second run of the pipeline changes
/// nothing: the budget and the traversal are deterministic.
#[test]
fn the_same_module_optimized_twice_gives_the_same_dump() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  dbg(double(shout(21)))
}
",
    )];
    let (mut first, stats) = lower_files_with_stats(&files, true);
    let (second, _) = lower_files_with_stats(&files, true);
    assert!(stats.delta > 0, "{stats:?}");
    assert_eq!(dump(&first), dump(&second));
    let once = dump(&first);
    let again = opt::run(&mut first, &CoreFns::default(), true);
    assert!(again.is_empty(), "a second run changed the IR: {again:?}");
    assert_eq!(dump(&first), once);
    verify(&first).unwrap();
}

/// `lower_files_with` with the optimisations off is the shape lowering produces; used to check
/// that δ is what changes a call.
#[test]
fn optimize_off_keeps_the_call() {
    let files = [(
        "main.emel",
        "\
fn double(x: Int) -> Int {
  x * 2
}

fn main() {
  dbg(double(21))
}
",
    )];
    let m = lower_files_with(&files, false);
    assert_eq!(calls_anywhere(&m, function(&m, "double")), 1);
}

/// The sum of `n` ones, as a left-nested tree of binary additions.
fn sum_of(n: usize) -> Expr {
    let mut e = Expr::Lit(Lit::Int(1));
    for _ in 1..n {
        e = Expr::Binary {
            op: BinOp::Add,
            ty: OpTy::Int,
            lhs: Box::new(e),
            rhs: Box::new(Expr::Lit(Lit::Int(1))),
        };
    }
    e
}

/// A zero-argument direct call to `f`.
fn call(f: FnId) -> Expr {
    Expr::Call {
        callee: Callee::Direct {
            f,
            ty_args: Vec::new(),
        },
        args: Vec::new(),
        fails: false,
        suspends: false,
    }
}
