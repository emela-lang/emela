//! Effect sets R: `use`, `with`, declared `uses` and closed definitions.

use emela_resolve::hir::DefModule;

use super::render_effects;
use crate::common::resolve_main;

/// `use X` puts X in R, even inside a branch, for the whole function's R (spec 8.2). Calls inherit
/// the callee's R.
#[test]
fn use_adds_to_r() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

fn log(msg: String) {
  (use Logger).info(msg)
}

fn maybe(c: Bool) -> Int {
  if c { (use Clock).now() } else { 0 }
}

fn both() {
  log("a")
  maybe(True)
}
"#
    ));
}

/// `with` removes the provided effects from R and adds the handlers' dependencies (spec 8.3, 9.4).
/// Handler dependencies may be met by handlers for other effects in the same `with` (spec 14.2).
#[test]
fn with_provides_effects() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Stamped: Logger {
  fn info(msg) {
    t = (use Clock).now()
    ()
  }
}

handler Fixed: Clock {
  fn now() { 0 }
}

fn greet() {
  (use Logger).info("hi")
  (use Clock).now()
}

fn partly() {
  with Quiet { greet() }
}

fn needs_clock() {
  with Stamped { (use Logger).info("x") }
}

fn sibling() {
  with Stamped, Fixed { greet() }
}

fn main() {
  with Quiet, Fixed { greet() }
}
"#
    ));
}

/// Handlers in the same `with` depending on each other's effects form a cycle (E4101).
#[test]
fn handler_cycles() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

handler HPing: Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong: Pong {
  fn pong() { (use Ping).ping() }
}

fn f() {
  with HPing, HPong { (use Ping).ping() }
}
"#
    ));
}

/// Capability operation types come from the effect declaration, and an operation's fails adds to
/// the call's E (spec 8.1). Named arguments use the operation's parameter names. Unknown
/// operations are E4401.
#[test]
fn operations() {
    insta::assert_snapshot!(render_effects(
        r#"
error NotFound(id: Int)

effect Users {
  fn find(id: Int) -> String fails NotFound
}

fn by_label() {
  users = use Users
  users.find(id: 1)
}

fn wrong() {
  users = use Users
  a = users.find("x")
  users.fnd(1)
}
"#
    ));
}

/// A written `use` clause is exactly that set (spec 9.3); a larger body R is E4301. A function
/// type with `use {}` accepts only functions without effects (spec 9.2).
#[test]
fn declared_uses() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

fn declared() -> () uses { Logger } {
  (use Logger).info("a")
}

fn too_many() -> () uses { Logger } {
  (use Clock).now()
  ()
}

fn pure_only() -> () uses {} {
  (use Logger).info("a")
}

fn run_pure(f: fn() -> () uses {}) { f() }

fn callers() {
  run_pure(|| ())
  run_pure(|| (use Logger).info("x"))
  run_pure(declared)
}
"#
    ));
}

/// Omitting `use` on a function-typed parameter gives an invisible set variable that joins the
/// caller's R (spec 9.2). Lambdas enclose their body's R. Type parameters in `use R` (spec 9.5).
#[test]
fn hidden_and_named_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }

fn update[A, R](x: A, f: fn(A) -> A uses { R }) -> A uses { R } { f(x) }

fn make() {
  || (use Logger).info("later")
}

fn callers() {
  a = apply(|n| { (use Logger).info("n")
  n }, 1)
  b = update(1, |n| n + 1)
  c = List.map([1], |n| (use Logger).info("m"))
  f = make()
}

fn caller() {
  |g| g()
}
"#
    ));
}

/// Capability values can be passed as arguments, stored in data structures, and returned, as any
/// other value (`[effect.use]`).
#[test]
fn capability_values_as_values() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

type Box(log: Int)

fn f() {
  log = use Logger
  xs = [log]
  p = (log, 1)
  s = Some(log)
  pipe = log |> Some
  id = |x| x
  id(log)
  log
}

fn g() {
  log = use Logger
  || log
}

fn h() {
  log = use Logger
  || log.info("closure")
}
"#
    ));
}

/// The R of `main` and test functions must be empty (spec 8.3, 13.3). A const's value cannot have
/// effects (spec 5.6).
#[test]
fn closed_definitions() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

const C: () = (use Logger).info("c")

fn main() {
  (use Logger).info("main")
}

fn test_inside() {
  with Quiet { (use Logger).info("ok") }
}

fn test_outside() -> () uses { Logger } {
  (use Logger).info("t")
}
"#
    ));
}

/// `use { Quiet, R }` mixing an effect and a set type parameter (spec 8.3, 17.4), handled like the
/// error case `fails Timeout | E` (spec 7.4). The listed effect is removed by `with` and the set
/// type parameter remains. R gets the R of the passed function minus Quiet.
#[test]
fn mixed_effect_sets() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Quiet {
  fn hush() -> ()
}

effect Logger {
  fn info(msg: String) -> ()
}

handler Silent: Quiet {
  fn hush() { () }
}

fn run[R](f: fn() -> () uses { Quiet, R }) -> () uses { R } {
  with Silent { f() }
}

fn run_rev[R](f: fn() -> () uses { R, Quiet }) -> () uses { R } {
  with Silent { f() }
}

fn run_inferred[R](f: fn() -> () uses { Quiet, R }) {
  with Silent { f() }
}

fn run_hidden(f: fn() -> ()) {
  with Silent { f() }
}

fn not_provided[R](f: fn() -> () uses { Quiet, R }) -> () uses { R } {
  f()
}

fn wrap[R](f: fn() -> () uses { R }) -> fn() -> () uses { Quiet, R } {
  || {
    (use Quiet).hush()
    f()
  }
}

fn forward(g: fn() -> ()) {
  run(g)
}

fn callers() {
  a = run(|| {
    (use Quiet).hush()
    (use Logger).info("x")
  })
  b = run(|| (use Quiet).hush())
  c = run(|| ())
  d = run_hidden(|| (use Quiet).hush())
  e = run(wrap(|| (use Logger).info("y")))
}
"#
    ));
}

/// A local fn encloses its body's R in its type, and the R surfaces where it is called. A written
/// `uses` is enforced as for a top-level fn (E4301). A local fn can return a capability value as
/// a fn can (`[fn.local]`, `[effect.use]`).
#[test]
fn local_fn_effects() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Log {
  fn info(msg: String) -> ()
}

fn inferred() {
  fn say(m: String) {
    log = use Log
    log.info(m)
  }
  say("a")
}

fn never_called() -> Int {
  fn say(m: String) {
    log = use Log
    log.info(m)
  }
  1
}

fn declared() uses { Log } {
  fn say(m: String) -> () uses { Log } {
    log = use Log
    log.info(m)
  }
  say("a")
}

fn undeclared() {
  fn say(m: String) -> () uses {} {
    log = use Log
    log.info(m)
  }
  say("a")
}

fn returns_capability() {
  fn get() {
    use Log
  }
  get()
}
"#
    ));
}

/// An effect declared in source has at least one operation (E4010, `[effect.decl]`). A label has
/// the operations of the effect it stands for.
#[test]
fn effect_without_operations() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Marker {}

effect Console {
  fn print(text: String) -> ()
}

effect Audit of Console
"#
    ));
}

/// A reference from another definition of the same group to a fn whose E or R is inferred places
/// the referenced fn's type parameters and invisible sets in that E and R with the arguments of
/// the reference (spec 9.3, issue #684). `b` and `d` list their type parameters in a different
/// order from `a` and `c`, so an index of `a` or `c` taken as is would name another parameter of
/// `b` or `d`, or none.
#[test]
fn group_references_place_inferred_sets() {
    let src = r#"
fn a[A](x: A, f: fn(A) -> Bool, n: Int) -> Bool {
  if n == 0 { f(x) } else { b(0, x, f, n - 1) }
}

fn b[B, A](y: B, x: A, f: fn(A) -> Bool, n: Int) -> Bool {
  a(x, f, n)
}

fn c[A](x: A, f: fn(A) -> Bool, n: Int) {
  if n == 0 { f(x) } else { d(0, x, f, n - 1) }
}

fn d[B, A](y: B, x: A, f: fn(A) -> Bool, n: Int) {
  c(x, f, n)
}
"#;
    let (program, module, resolve_diagnostics) = resolve_main(src);
    assert_eq!(resolve_diagnostics, []);
    let types = emela_types::check_program(&program);
    let cons = &types.ty_table.cons;
    let sig = |name: &str| {
        let (def, _) = program
            .defs
            .iter()
            .find(|(_, d)| d.module == DefModule::Source(module) && d.name == name)
            .unwrap();
        types
            .sigs
            .get(def)
            .unwrap()
            .scheme
            .display(cons)
            .to_string()
    };
    assert!(types.diags.is_empty(), "{:?}", types.diags);
    assert_eq!(
        sig("b"),
        "∀ B, A, E(f), R(f). fn(B, A, fn(A) -> Bool fails E(f) uses { R(f) }, Int) -> Bool fails E(f) uses { R(f) }"
    );
    assert_eq!(
        sig("d"),
        "∀ B, A, E(f), R(f). fn(B, A, fn(A) -> Bool fails E(f) uses { R(f) }, Int) -> Bool fails E(f) uses { R(f) }"
    );
}

/// `[effect.use]`, `[type.rules]`: a call through an effect alias or `(use E).op(...)` adds E to
/// the R of the innermost function that contains it, a closure or a local fn included. A `with`
/// around the closure removes E only from the R of its own body; the closure's type keeps it.
/// `x.op` without a call is a function value whose R contains E.
#[test]
fn effect_aliases_in_closures() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Greeter {
  fn greet(s: String) -> String
}

handler Plain: Greeter {
  fn greet(s) { "plain #{s}" }
}

fn made_inside() -> fn(String) -> String uses { Greeter } {
  with Plain {
    g <- Greeter
    |s| g.greet(s)
  }
}

fn in_place() -> fn(String) -> String uses { Greeter } {
  with Plain { |s| (use Greeter).greet(s) }
}

fn op_value() -> fn(String) -> String uses { Greeter } {
  with Plain {
    g <- Greeter
    g.greet
  }
}

fn local() -> String {
  with Plain {
    fn go(s: String) -> String {
      g <- Greeter
      g.greet(s)
    }
    go("x")
  }
}
"#
    ));
}
