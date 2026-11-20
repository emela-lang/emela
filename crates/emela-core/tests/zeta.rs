//! Tests for copy propagation and dead-binding elimination (`emela_core::opt::reduce::zeta`).
//!
//! The rule removes the `let` chains that lowering and inlining leave (`let x = y`, and a binding
//! nothing reads whose value is trivial) and keeps every binding whose value could have an effect,
//! fail or raise a defect. The keeping half is the interesting one: a division by zero, an
//! `assert`, an effectful call and a variable whose type carries a finalizer must survive.
//!
//! Every test lowers with the Core IR optimisations off and then runs the pipeline with only the
//! `zeta` rule on ([`support::lower_files_with_rules`]), because the per-rule flags are off by
//! default until A10/PR-019. The observable behaviour of the rewrites is checked by the mutation
//! pair `zeta_fused.emel` / `zeta_expanded.emel` in `emela-driver/tests/opt_diff.rs`; the tests
//! here fix the shape the rule leaves behind.

mod support;

use emela_core::opt::{CoreFns, Options, Rules, run_with};
use emela_core::{BinOp, EffectDef, Expr, Lit, Local, Module, OpTy, TypeId, dump};
use support::lower_files_with_rules;

/// Only ζ is on: no other reduction rule may do part of what a test asserts.
const ZETA: Rules = Rules {
    zeta: true,
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

/// The number of `let` bindings in the body of `name`.
fn lets_in(m: &Module, name: &str) -> usize {
    let ir = dump(m);
    let header = if name == "main" {
        "export fn main".to_owned()
    } else {
        format!("fn {name}")
    };
    function(&ir, &header).matches("let ").count()
}

/// A `let` of `value` into `var`, whose body is `body`.
fn let_(var: Local, value: Expr, body: Expr) -> Expr {
    Expr::Let {
        var,
        value: Box::new(value),
        body: Box::new(body),
    }
}

/// The `let` chains that inlining leaves in front of the work are gone: the argument, the copy the
/// callee made of it, and the callee's own copy are all reads of the value they were bound to.
#[test]
fn an_inlined_let_chain_disappears() {
    let files = [(
        "main.emel",
        "\
fn twice(x: Int) -> Int {
  copy = x
  copy + copy
}

fn main() {
  dbg(twice(3))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert!(stats.zeta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(
        !main.contains("let x"),
        "the argument is copied in:\n{main}"
    );
    assert!(
        !main.contains("let copy"),
        "the callee's copy is copied in:\n{main}"
    );
    assert!(
        !ir.contains("let copy"),
        "the callee's own body lost its copy too:\n{ir}"
    );
    assert_eq!(lets_in(&m, "main"), 0, "{}", dump(&m));
}

/// A record and a closure that nothing reads are allocations only, so the bindings go.
#[test]
fn dead_record_and_closure_bindings_are_removed() {
    let files = [(
        "main.emel",
        "\
type Point(x: Int, y: Int)

fn main() {
  unused_point = Point(x: 1, y: 2)
  unused_closure = |z: Int| z + 1
  dbg(3)
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert!(stats.zeta >= 2, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(!main.contains("let unused_point"), "{main}");
    assert!(!main.contains("let unused_closure"), "{main}");
}

/// The dead-let rule also removes a constructor binding when the constructor is written at the
/// binding: making it allocates and nothing reads it.
#[test]
fn a_dead_trivial_constructor_binding_is_removed() {
    let files = [(
        "main.emel",
        "\
type Point(x: Int, y: Int)

fn main() {
  unused = Point(x: 1, y: 2)
  dbg(3)
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert_eq!(stats.zeta, 1, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(!main.contains("let unused"), "{main}");
    assert!(!main.contains("Point"), "the allocation stayed:\n{main}");
}

/// A lambda that captures nothing is a closure allocation only, so its dead binding goes too.
#[test]
fn a_dead_allocation_free_lambda_binding_is_removed() {
    let files = [(
        "main.emel",
        "\
fn main() {
  unused = |z: Int| z + 1
  dbg(3)
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert_eq!(stats.zeta, 1, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(!main.contains("let unused"), "{main}");
    assert!(!main.contains("|"), "the closure stayed:\n{main}");
}

/// A division may raise a defect when it is evaluated, whether or not its result is read: the
/// binding stays.
#[test]
fn a_division_that_may_fail_is_not_dropped() {
    let files = [(
        "main.emel",
        "\
fn main() {
  x = 1 / 0
  dbg(2)
}
",
    )];
    let (m, _) = lower_files_with_rules(&files, ZETA);
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(main.contains("let x"), "{main}");
    assert!(main.contains("/[Int]"), "{main}");
}

/// A call that uses an effect is not trivial: the binding stays even though nothing reads it, so
/// the operation runs.
#[test]
fn a_binding_of_an_effectful_call_stays() {
    let files = [(
        "main.emel",
        "\
effect Log {
  fn log(msg: String) -> ()
}

handler Ignore: Log {
  fn log(msg) { () }
}

fn logging(x: Int) -> Int uses { Log } {
  (use Log).log(\"at #{x}\")
  x
}

fn main() {
  with Ignore {
    unused = logging(1)
    dbg(2)
  }
}
",
    )];
    let (m, _) = lower_files_with_rules(&files, ZETA);
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(main.contains("let unused"), "{main}");
}

/// A copy inside a loop: the pattern binds `x` afresh each iteration, the body copies it into `y`,
/// and every read of `y` is the loop's `x`.
#[test]
fn a_copy_inside_a_loop_is_propagated() {
    let files = [(
        "main.emel",
        "\
fn sum(xs: List[Int], acc: Int) -> Int {
  xs match {
    [] -> acc
    [x, ..rest] -> {
      y = x
      sum(rest, acc + y)
    }
  }
}

fn main() {
  dbg(sum([1, 2, 3], 0))
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert!(stats.zeta > 0, "{stats:?}");
    let ir = dump(&m);
    assert!(!ir.contains("let y"), "{ir}");
    assert!(ir.contains("loop("), "{ir}");
}

/// A copy inside a `with` is propagated into the body, and the `with` and its release stay where
/// they were: the copy is a read of a literal, so no release can move.
#[test]
fn a_copy_inside_a_with_leaves_the_release_in_place() {
    let files = [(
        "main.emel",
        "\
effect Log {
  fn log(msg: String) -> ()
}

handler Session: Log {
  release {
    dbg(\"close\")
    ()
  }
  fn log(msg) {
    dbg(msg)
    ()
  }
}

fn main() {
  with Session {
    n = 2
    dbg(n)
  }
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, ZETA);
    assert!(stats.zeta > 0, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(!main.contains("let n"), "{main}");
    assert!(main.contains("@dbg[Int](2)"), "{main}");
    assert!(main.contains("release"), "{main}");
}

/// A string literal is not a copy value: replacing every read of `s` by the literal would repeat
/// its text, which is the code growth the scalar-only rule exists to avoid.
#[test]
fn a_string_copy_is_not_propagated() {
    let files = [(
        "main.emel",
        "\
fn main() {
  s = \"text\"
  dbg(s)
}
",
    )];
    let (m, _) = lower_files_with_rules(&files, ZETA);
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(main.contains("let s"), "{main}");
}

/// A variable whose type carries a finalizer is not copied: replacing the reads of `x` by `h`
/// would move the handler's last drop, and with it when the handler is released (plan §3.1).
///
/// Source-level capabilities come from `use` inside a handler, which makes the IR harder to shape
/// than the rule is about; the case is built directly, with a local of `Type::Handler`.
#[test]
fn a_finalizer_typed_var_is_not_propagated() {
    let mut m = Module::new();
    let effect = m.add_effect(EffectDef::new("Log", Vec::new()));
    let handler = m.handler(effect);
    let h = m.local("h", handler);
    let x = m.local("x", handler);
    let main = m.declare("main", true);
    m.define(
        main,
        Vec::new(),
        handler,
        let_(x, Expr::Var(h), Expr::Var(x)),
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(stats.zeta, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "the refused copy is a deopt: {stats:?}");
    let ir = dump(&m);
    let body = function(&ir, "export fn main");
    assert!(body.contains("let x"), "the copy was propagated:\n{body}");
}

/// A copy inside a lambda of a variable bound outside it is refused: the value is a free variable
/// of the lambda, so propagating it would turn every read of the binding into a read of a free
/// variable. This is the shape the effects benchmark has (`let hi = n` inside the lambda
/// `State.run` receives), where the JavaScript target loads the loop bound from the closure
/// context on every iteration. The `let` stays; the program is unchanged.
#[test]
fn a_capture_copy_inside_a_lambda_is_not_propagated() {
    let mut m = Module::new();
    let n = m.local("n", TypeId::INT);
    let hi = m.local("hi", TypeId::INT);
    let ty = m.fn_type(Vec::new(), TypeId::INT);
    let main = m.declare("main", true);
    m.define(
        main,
        vec![n],
        ty,
        Expr::Lambda {
            params: Vec::new(),
            ty,
            body: Box::new(let_(hi, Expr::Var(n), Expr::Var(hi))),
            suspends: false,
        },
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(stats.zeta, 0, "{stats:?}");
    assert_eq!(stats.deopt, 1, "the refused copy is a deopt: {stats:?}");
    let ir = dump(&m);
    let body = function(&ir, "export fn main");
    assert!(
        body.contains("let hi"),
        "the capture was propagated:\n{body}"
    );
}

/// A copy of a lambda's own parameter is not a capture: the value and the `let` are inside the
/// same lambda, so every read stays a read of a local. The effects benchmark has this shape too
/// (`let _ = _` in the per-element lambda), and it must still go.
#[test]
fn a_copy_of_a_lambda_parameter_is_propagated() {
    let mut m = Module::new();
    let p = m.local("p", TypeId::INT);
    let x = m.local("x", TypeId::INT);
    let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
    let main = m.declare("main", true);
    m.define(
        main,
        Vec::new(),
        ty,
        Expr::Lambda {
            params: vec![p],
            ty,
            body: Box::new(let_(x, Expr::Var(p), Expr::Var(x))),
            suspends: false,
        },
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(stats.zeta, 1, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    let ir = dump(&m);
    assert!(!ir.contains("let x"), "{ir}");
}

/// The capture restriction applies to a `Var` value only: a scalar literal inside a lambda still
/// propagates, because a literal read is not a free-variable read.
#[test]
fn a_scalar_literal_inside_a_lambda_is_propagated() {
    let mut m = Module::new();
    let n = m.local("n", TypeId::INT);
    let x = m.local("x", TypeId::INT);
    let hi = m.local("hi", TypeId::INT);
    let ty = m.fn_type(Vec::new(), TypeId::INT);
    let main = m.declare("main", true);
    // `fn main(n) = || { let x = 0; let hi = n; x + hi }`: the literal copy goes, the capture stays.
    m.define(
        main,
        vec![n],
        ty,
        Expr::Lambda {
            params: Vec::new(),
            ty,
            body: Box::new(let_(
                x,
                Expr::Lit(Lit::Int(0)),
                let_(
                    hi,
                    Expr::Var(n),
                    Expr::Binary {
                        op: BinOp::Add,
                        ty: OpTy::Int,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Var(hi)),
                    },
                ),
            )),
            suspends: false,
        },
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(stats.zeta, 1, "only the literal copy goes: {stats:?}");
    // A refused site is counted in every sweep the pass runs, as a finalizer refusal is.
    assert!(stats.deopt >= 1, "{stats:?}");
    let ir = dump(&m);
    assert!(!ir.contains("let x"), "the literal copy stayed:\n{ir}");
    assert!(ir.contains("let hi"), "the capture was propagated:\n{ir}");
}

/// Outside every lambda, a variable copy is not a capture: every `let` is in the same function as
/// the variable it copies. This is matmul's essential ζ (the argument bindings δ writes when it
/// inlines `get`, inside `dot`), which the loop unswitch needs.
#[test]
fn a_matmul_style_copy_outside_a_lambda_is_propagated() {
    let mut m = Module::new();
    let a = m.local("a", TypeId::INT);
    let i = m.local("i", TypeId::INT);
    let m_local = m.local("m", TypeId::INT);
    let k = m.local("k", TypeId::INT);
    let main = m.declare("main", true);
    m.define(
        main,
        vec![a, i],
        TypeId::INT,
        let_(
            m_local,
            Expr::Var(a),
            let_(
                k,
                Expr::Var(i),
                Expr::Binary {
                    op: BinOp::Add,
                    ty: OpTy::Int,
                    lhs: Box::new(Expr::Var(m_local)),
                    rhs: Box::new(Expr::Var(k)),
                },
            ),
        ),
    );

    let stats = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(stats.zeta, 2, "{stats:?}");
    assert_eq!(stats.deopt, 0, "{stats:?}");
    let ir = dump(&m);
    let body = function(&ir, "export fn main");
    assert!(!body.contains("let "), "a copy stayed:\n{body}");
}

/// The capture restriction does not make the pass oscillate: a refused copy is refused again and
/// the IR is the same, so a second run changes nothing.
#[test]
fn a_capture_copy_is_stable_across_runs() {
    let mut m = Module::new();
    let n = m.local("n", TypeId::INT);
    let hi = m.local("hi", TypeId::INT);
    let x = m.local("x", TypeId::INT);
    let ty = m.fn_type(vec![TypeId::INT], TypeId::INT);
    let main = m.declare("main", true);
    // `|p| { let x = p; let hi = n; x + hi }`: one same-owner copy and one capture copy.
    let p = m.local("p", TypeId::INT);
    m.define(
        main,
        vec![n],
        ty,
        Expr::Lambda {
            params: vec![p],
            ty,
            body: Box::new(let_(
                x,
                Expr::Var(p),
                let_(
                    hi,
                    Expr::Var(n),
                    Expr::Binary {
                        op: BinOp::Add,
                        ty: OpTy::Int,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Var(hi)),
                    },
                ),
            )),
            suspends: false,
        },
    );

    let first = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(first.zeta, 1, "the same-owner copy goes: {first:?}");
    let once = dump(&m);
    let second = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(second.zeta, 0, "{second:?}");
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(dump(&m), once);
}

#[test]
fn a_second_run_changes_nothing() {
    let files = [(
        "main.emel",
        "\
fn twice(x: Int) -> Int {
  copy = x
  copy + copy
}

fn main() {
  dbg(twice(3))
}
",
    )];
    let (mut m, first) = lower_files_with_rules(&files, ZETA);
    assert!(first.zeta > 0, "{first:?}");
    let once = dump(&m);
    let second = run_with(
        &mut m,
        &CoreFns::default(),
        Options {
            rules: ZETA,
            ..Options::default()
        },
    );
    assert_eq!(second.zeta, 0, "{second:?}");
    assert!(second.is_empty(), "{second:?}");
    assert_eq!(dump(&m), once);
}

/// With the rule off, the chain stays: the per-rule flags are how a test isolates one rewrite.
#[test]
fn rules_none_leaves_the_chain() {
    let files = [(
        "main.emel",
        "\
fn main() {
  x = 1
  y = x
  dbg(y)
}
",
    )];
    let (m, stats) = lower_files_with_rules(&files, Rules::none());
    assert_eq!(stats.zeta, 0, "{stats:?}");
    let ir = dump(&m);
    let main = function(&ir, "export fn main");
    assert!(main.contains("let y"), "{main}");
}
