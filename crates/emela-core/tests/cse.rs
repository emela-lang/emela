//! Tests for common subexpression elimination (`emela_core::opt::reduce::cse`).
//!
//! The rule merges two occurrences of a pure computation in one straight-line `let` chain. The
//! interesting half is what it must leave alone: a call, a division, a value whose type carries a
//! finalizer, and an occurrence on the other side of a branch, loop or lambda. The rule is off in
//! `Rules::default` for now, so every test turns on only `cse` with
//! [`support::lower_files_with_rules`] or a hand-built module.
//!
//! The observable behaviour is checked by the differential harness; the tests here fix the shape
//! the rule leaves behind and the deopt decisions.

mod support;

use emela_core::opt::{CoreFns, Options, Rules, Stats, fusion::Laws, run_with};
use emela_core::{Expr, Lit, Local, Module, TypeId, dump};
use support::lower_files_with_rules;

/// Only CSE is on: no other reduction rule may do part of what a test asserts.
const CSE: Rules = Rules {
    cse: true,
    ..Rules::none()
};

/// The part of the dump for the function whose header starts with `header`.
///
/// The dump writes the entry as `export fn main(…)`, so the header includes `export `.
fn function<'a>(ir: &'a str, header: &str) -> &'a str {
    let start = ir
        .find(header)
        .unwrap_or_else(|| panic!("the dump has no `{header}`:\n{ir}"));
    let rest = &ir[start..];
    let end = rest
        .find("\nfn ")
        .into_iter()
        .chain(rest.find("\nexport fn "))
        .min()
        .unwrap_or(rest.len());
    &rest[..end]
}

/// How often `needle` occurs in the body of `header`.
fn count_in(ir: &str, header: &str, needle: &str) -> usize {
    function(ir, header).matches(needle).count()
}

/// Lowers `source` with only CSE on.
fn with_cse(source: &str) -> (Module, Stats) {
    lower_files_with_rules(&[("main.emel", source)], CSE)
}

/// Lowers `source` with every reduction rule off.
fn without_cse(source: &str) -> (Module, Stats) {
    lower_files_with_rules(&[("main.emel", source)], Rules::none())
}

/// A `let` of `value` into `var`, whose body is `body`.
fn let_(var: Local, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        var,
        value: Box::new(value),
        body: Box::new(body),
    }
}

/// Runs only CSE on a hand-built module.
fn run_cse(m: &mut Module) -> Stats {
    run_with(
        m,
        &CoreFns::default(),
        Options {
            rules: CSE,
            laws: Laws::none(),
        },
    )
}

/// `x * 2` computed twice in a straight line is computed once; the second binding reads the
/// first. With the flag off both computations stay.
#[test]
fn a_repeated_pure_expression_is_computed_once() {
    let source = "\
fn main() {
  x = 3
  a = x * 2
  b = x * 2
  dbg(a + b)
}
";
    let (m, stats) = with_cse(source);
    assert!(stats.cse >= 1, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert_eq!(count_in(&ir, "export fn main", "*[Int]"), 1, "{main}");
    assert!(
        main.contains("= a") || main.contains("= a%"),
        "the second computation did not become a read of the first:\n{main}"
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");

    let (off, stats) = without_cse(source);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert_eq!(
        count_in(&dump(&off), "export fn main", "*[Int]"),
        2,
        "the flag-off run lost a computation:\n{}",
        dump(&off)
    );
}

/// The tail of the chain is a later occurrence too: the final expression reads the local the
/// earlier computation was bound to.
#[test]
fn an_occurrence_in_the_tail_is_merged() {
    let source = "\
fn main() -> Int {
  x = 3
  a = x * 2
  a + x * 2
}
";
    let (m, stats) = with_cse(source);
    assert!(stats.cse >= 1, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "*[Int]"),
        1,
        "the tail recomputed the expression:\n{}",
        dump(&m)
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// A second run of the rule finds nothing to remove.
#[test]
fn a_second_run_removes_nothing() {
    let source = "\
fn main() {
  x = 3
  a = x * 2
  b = x * 2
  dbg(a + b)
}
";
    let (mut m, first) = with_cse(source);
    assert!(first.cse >= 1, "{first:?}");
    let second = run_cse(&mut m);
    assert_eq!(second.cse, 0, "{second:?}");
    assert_eq!(second.deopt, 0, "{second:?}");
    assert_eq!(second.passes, 1, "{second:?}");
}

/// Two calls of a function that may print are not merged: the call may have an effect, and the
/// site is a deopt.
#[test]
fn an_effectful_call_is_not_merged() {
    let source = "\
fn shout(x: Int) -> Int {
  dbg(x)
  x
}

fn main() {
  a = shout(1)
  b = shout(1)
  dbg(a + b)
}
";
    let (m, stats) = with_cse(source);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "shout("),
        2,
        "CSE merged an effectful call:\n{}",
        dump(&m)
    );
}

/// A division may raise a defect on a zero divisor, so two divisions are not merged.
#[test]
fn a_division_is_not_merged() {
    let source = "\
fn main() {
  x = 3
  a = x / 2
  b = x / 2
  dbg(a + b)
}
";
    let (m, stats) = with_cse(source);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "/[Int]"),
        2,
        "CSE removed a division's defect:\n{}",
        dump(&m)
    );
}

/// A repeated tuple of finalizer-free values is one allocation.
#[test]
fn a_repeated_finalizer_free_tuple_is_merged() {
    let source = "\
fn main() {
  a = (1, 2)
  b = (1, 2)
  dbg(a)
  dbg(b)
}
";
    let (m, stats) = with_cse(source);
    assert!(stats.cse >= 1, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "(1, 2)"),
        1,
        "the tuple was built twice:\n{}",
        dump(&m)
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// A tuple that contains a finalizer-bearing value is a deopt: merging the two tuples would move
/// the field's last drop. The drop-point rule is ζ's `has_finalizer`, read recursively here.
#[test]
fn a_finalizer_bearing_tuple_is_not_merged() {
    let mut m = Module::new();
    let cell = m.cell(TypeId::INT);
    let param = m.local("p", cell);
    let pair = m.tuple(vec![cell, TypeId::INT]);
    let a = m.local("a", pair);
    let b = m.local("b", pair);
    let result = m.local("result", TypeId::INT);
    let f = m.declare("held", false);
    let tuple = || Expr::Tuple(vec![Expr::Var(param), Expr::Lit(Lit::Int(1))]);
    m.define(
        f,
        vec![param],
        TypeId::INT,
        let_(
            a,
            tuple(),
            let_(
                b,
                tuple(),
                let_(result, Expr::Lit(Lit::Int(0)), Expr::Var(result)),
            ),
        ),
    );
    let stats = run_cse(&mut m);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert_eq!(
        dump(&m).matches("(p%0, 1)").count(),
        2,
        "a finalizer-bearing tuple was merged:\n{}",
        dump(&m)
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// A field read whose result is plain is still a deopt when the constructor it reads holds a
/// finalizer-bearing value: sharing the reads would also merge the constructor allocations.
#[test]
fn a_plain_field_read_of_a_finalizer_bearing_constructor_is_not_merged() {
    let mut m = Module::new();
    let cell = m.cell(TypeId::INT);
    let def = emela_core::EnumDef {
        name: "Record".to_owned(),
        params: 0,
        variants: vec![emela_core::VariantDef::named(
            "Record",
            vec![("cell", cell), ("value", TypeId::INT)],
        )],
        error: false,
        external: false,
    };
    let enum_id = m.add_enum(def);
    let ctor = emela_core::CtorRef {
        enum_id,
        variant: 0,
    };
    let param = m.local("p", cell);
    let a = m.local("a", TypeId::INT);
    let b = m.local("b", TypeId::INT);
    let f = m.declare("held", false);
    let field = || Expr::Field {
        base: Box::new(Expr::Ctor {
            ctor,
            ty_args: Vec::new(),
            args: vec![Expr::Var(param), Expr::Lit(Lit::Int(1))],
        }),
        ctor,
        index: 1,
    };
    m.define(
        f,
        vec![param],
        TypeId::INT,
        let_(a, field(), let_(b, field(), Expr::Lit(Lit::Int(0)))),
    );
    let stats = run_cse(&mut m);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "fn held", ".value#1"),
        2,
        "the constructor behind the field reads was merged:\n{}",
        dump(&m)
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// An occurrence in an `if` arm is on another path: it is not merged with the value before the
/// branch.
#[test]
fn an_occurrence_across_an_if_arm_is_not_merged() {
    let source = "\
fn main() {
  x = 3
  a = x * 2
  if x > 0 {
    dbg(x * 2)
  } else {
    dbg(a)
  }
}
";
    let (m, stats) = with_cse(source);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "*[Int]"),
        2,
        "CSE merged across an if arm:\n{}",
        dump(&m)
    );
}

/// An occurrence inside a lambda body is not merged with one outside it.
#[test]
fn an_occurrence_inside_a_lambda_is_not_merged() {
    let source = "\
fn main() {
  x = 3
  a = x * 2
  f = |y: Int| x * 2 + y
  dbg(f(a))
}
";
    let (m, stats) = with_cse(source);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert_eq!(
        count_in(&dump(&m), "export fn main", "*[Int]"),
        2,
        "CSE merged across a lambda boundary:\n{}",
        dump(&m)
    );
}

/// An occurrence inside a loop body is not merged with one before the loop.
#[test]
fn an_occurrence_across_a_loop_is_not_merged() {
    let mut m = Module::new();
    let x = m.local("x", TypeId::INT);
    let a = m.local("a", TypeId::INT);
    let i = m.local("i", TypeId::INT);
    let b = m.local("b", TypeId::INT);
    let f = m.declare("held", false);
    let times_two = || Expr::Binary {
        op: emela_core::BinOp::Mul,
        ty: emela_core::OpTy::Int,
        lhs: Box::new(Expr::Var(x)),
        rhs: Box::new(Expr::Lit(Lit::Int(2))),
    };
    let step = Expr::Binary {
        op: emela_core::BinOp::Add,
        ty: emela_core::OpTy::Int,
        lhs: Box::new(Expr::Var(i)),
        rhs: Box::new(Expr::Lit(Lit::Int(1))),
    };
    let body = Expr::Loop {
        vars: vec![i],
        inits: vec![Expr::Lit(Lit::Int(0))],
        body: Box::new(Expr::If {
            cond: Box::new(Expr::Binary {
                op: emela_core::BinOp::Lt,
                ty: emela_core::OpTy::Int,
                lhs: Box::new(Expr::Var(i)),
                rhs: Box::new(Expr::Lit(Lit::Int(1))),
            }),
            then: Box::new(let_(b, times_two(), Expr::Recur(vec![step]))),
            else_: Box::new(Expr::Var(a)),
        }),
    };
    m.define(f, vec![x], TypeId::INT, let_(a, times_two(), body));
    let stats = run_cse(&mut m);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert_eq!(
        dump(&m).matches("*[Int]").count(),
        2,
        "CSE merged across a loop:\n{}",
        dump(&m)
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// When the first occurrence is a subexpression that no local binds, the rule inserts a `let` at
/// that point; the later occurrence reads it and the expression is still evaluated once, in
/// order.
#[test]
fn an_unbound_first_occurrence_gets_a_let() {
    let source = "\
fn main() {
  x = 3
  c = x * 2 + 1
  d = x * 2
  dbg(c + d)
}
";
    let (m, stats) = with_cse(source);
    assert!(stats.cse >= 1, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert_eq!(count_in(&ir, "export fn main", "*[Int]"), 1, "{main}");
    let inserted = main
        .find("cse%")
        .unwrap_or_else(|| panic!("no inserted let:\n{main}"));
    let c = main
        .find("let c%")
        .unwrap_or_else(|| panic!("no binding for c:\n{main}"));
    let d = main
        .find("let d%")
        .unwrap_or_else(|| panic!("no binding for d:\n{main}"));
    assert!(
        inserted < c && c < d,
        "the inserted let is out of order:\n{main}"
    );
    emela_core::verify(&m).expect("CSE left well-typed IR");
}

/// A call through a callee whose type gives no summary is a deopt, not a pure expression.
#[test]
fn an_unknown_effect_summary_deopts() {
    let mut m = Module::new();
    let callee = m.local("f", TypeId::ANY);
    let a = m.local("a", TypeId::INT);
    let b = m.local("b", TypeId::INT);
    let f = m.declare("held", false);
    let call = || Expr::Call {
        callee: emela_core::Callee::Indirect(Box::new(Expr::Var(callee))),
        args: vec![Expr::Lit(Lit::Int(1))],
        fails: false,
        suspends: false,
    };
    m.define(
        f,
        vec![callee],
        TypeId::INT,
        let_(a, call(), let_(b, call(), Expr::Lit(Lit::Int(0)))),
    );
    let stats = run_cse(&mut m);
    assert_eq!(stats.cse, 0, "{stats:?}");
    assert!(stats.deopt >= 1, "{stats:?}");
    assert_eq!(
        dump(&m).matches("(f%0)(1)").count(),
        2,
        "CSE merged a call with an unknown summary:\n{}",
        dump(&m)
    );
}
