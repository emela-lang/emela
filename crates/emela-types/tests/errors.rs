//! Snapshot tests for error set inference (spec chapter 7, 9.2, 9.4).
//!
//! Besides the `render_bodies` output (declared types and diagnostics), `render_errors` prints the
//! expressions of `main.emel` with a non-empty E, in source order, as `expr: E`.

mod common;

use std::fmt::Write;

use emela_resolve::hir::DefModule;
use emela_types::check_program;

use common::{render_bodies, resolve_main};

/// Expressions at the same span (such as the call on the right of a pipe) are not merged.
fn render_errors(src: &str) -> String {
    let (program, module, _) = resolve_main(src);
    let types = check_program(&program);
    let cons = &types.ty_table.cons;
    let mut rows: Vec<_> = types
        .errors
        .iter()
        .filter(|(_, row)| !row.is_empty())
        .filter_map(|(expr, row)| {
            let span = program.exprs[expr].span;
            (span.module == module).then_some((span.range, row))
        })
        .collect();
    rows.sort_by_key(|(range, _)| (range.start(), std::cmp::Reverse(range.end())));
    let mut out = render_bodies(src);
    out.push_str("--- E\n");
    for (range, row) in rows {
        let text = &src[usize::from(range.start())..usize::from(range.end())];
        let text = text.replace('\n', " ");
        writeln!(out, "{text}: {}", row.display(cons)).unwrap();
    }
    let _ = DefModule::Prelude;
    out
}

/// A lambda encloses its body's E in its function type; it surfaces when called.
#[test]
fn lambdas_close_over_errors() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom

fn make() {
  fn() { fail Boom }
}

fn call_local() {
  f = fn() { fail Boom }
  f()
}

fn call_made() {
  make()()
}
"#
    ));
}

/// Generalization also generalizes set variables. A parameter function's E shows up at the
/// caller.
#[test]
fn generalizes_set_variables() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom

fn caller() {
  fn(g) { g() }
}

fn twice() {
  fn(g) { g() + g() }
}

fn use_it() {
  call = caller()
  a = call(fn() { 1 })
  b = call(fn() { fail Boom })
  (a, b)
}
"#
    ));
}

/// Functions with different E can be joined in branches and lists.
#[test]
fn functions_with_different_errors_join() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom
error Bad

fn fx() -> Int fails Boom { fail Boom }
fn fy() -> Int fails Bad { fail Bad }

fn pick(c: Bool) {
  f = if c { fx } else { fy }
  f()
}

fn all() {
  fs = [fx, fy]
  fs
}

fn annotated() {
  h: fn() -> Int = fx
  h()
}
"#
    ));
}

/// E of mutually recursive definitions. `escape` also removes caught tags from the solutions of
/// set variables still being inferred.
#[test]
fn recursive_groups() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom

fn a(n: Int) {
  if n > 0 { b(n - 1) } else { fail Boom }
}

fn b(n: Int) {
  a(n) escape { Boom -> 0 }
}
"#
    ));
}

/// Declared E. Writing `fails {}` means the fn cannot fail. A fn calling a parameter function may
/// propagate its E without writing it (invisible sets cannot be written).
#[test]
fn declared_errors() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom

fn pure() -> Int fails {} { 1 }

fn impure() -> Int fails {} {
  fail Boom
}

fn apply(f: fn(Int) -> Int, x: Int) -> Int {
  f(x)
}

fn apply_x(f: fn(Int) -> Int, x: Int) -> Int fails Boom {
  if x > 0 { f(x) } else { fail Boom }
}

fn use_apply() {
  apply(fn(n) { if n > 0 { n } else { fail Boom } }, 1)
}
"#
    ));
}

/// The invisible set of a function received by a parameter function (a negative position) has
/// unknown contents from the body's point of view.
#[test]
fn callbacks_in_negative_position() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom

fn with_callback(f: fn(fn() -> Int) -> Int) -> Int {
  f(fn() { 1 })
}

fn bad_callback(f: fn(fn() -> Int) -> Int) -> Int {
  f(fn() { fail Boom })
}
"#
    ));
}

/// W0302 when an `escape` arm catches an error that cannot occur, unless a set with unknown
/// contents is present. Redundant arms for the same error are W0301.
#[test]
fn escape_warnings() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom
error Bad(n: Int)

fn steady() -> Int { 1 }
fn fy(n: Int) -> Int fails Bad { fail Bad(n:) }

fn never_raised() -> Int {
  steady() escape { Boom -> 0 }
}

fn unknown[A](f: fn() -> A) -> A {
  f() escape { Boom -> f() }
}

fn redundant() -> Int {
  fy(1) escape {
    Bad(_) -> 0
    Bad(n: 1) -> 1
  }
}

fn guarded() -> Int {
  fy(1) escape {
    Bad(n:) if n > 0 -> n
    Bad(_) -> 0
  }
}
"#
    ));
}

/// An arm `_` handles every error that reaches it, so the E of the escape is the E of its arms
/// alone, with every tag and every set variable of the original expression removed, opaque or not
/// (`[error.escape]`, `[type.rules]`). Arms above `_` are tried first; errors raised in arms stay.
/// The hidden set variable of a parameter function stays in the E of a declaration whatever its
/// body does (`[error.fails]`), so the caller of `hidden` still has the errors.
#[test]
fn escape_wildcard_removes_everything() {
    insta::assert_snapshot!(render_errors(
        r#"
error NotFound(id: Int)
error DbError
error Other

fn load(id: Int) -> Int fails NotFound | DbError { fail DbError }

fn all() {
  load(1) escape { _ -> 0 }
}

fn after_ctor() {
  load(1) escape {
    NotFound(id: 1) -> 1
    _ -> 0
  }
}

fn arm_fails() {
  load(1) escape { _ -> fail Other }
}

fn or_else[A, E](f: fn() -> A fails E, default: A) -> A {
  f() escape { _ -> default }
}

fn hidden[A](f: fn() -> A, default: A) -> A {
  f() escape { _ -> default }
}

fn set_param[E](f: fn() -> Int fails E) -> Int {
  f() escape { _ -> 0 }
}

fn error_param[E](err: E) -> Int {
  { fail err } escape { _ -> 0 }
}

fn inferred() {
  fn(g) { g() escape { _ -> 0 } }
}

fn local() {
  f = fn() { load(2) }
  f() escape { _ -> 0 }
}

fn caller() -> Int {
  or_else(fn() { load(3) }, 0) + set_param(fn() { load(4) }) + inferred()(fn() { load(5) })
}

fn hidden_caller() {
  hidden(fn() { load(6) }, 0)
}
"#
    ));
}

/// Warnings and errors around an arm `_` (`[error.escape]`, `[diag.list]`): W0302 on `_` when
/// the original expression raises nothing; W0301 on every arm below `_`, also when E has an opaque
/// set variable, and no W0302 in addition; E0320 for a variable or a guarded `_`, without E0318.
#[test]
fn escape_wildcard_diagnostics() {
    insta::assert_snapshot!(render_errors(
        r#"
error Boom
error Bad(n: Int)

fn steady() -> Int { 1 }
fn fy(n: Int) -> Int fails Bad { fail Bad(n:) }

fn nothing_raised() -> Int {
  steady() escape { _ -> 0 }
}

fn below_on_nothing() -> Int {
  steady() escape {
    _ -> 0
    Boom -> 1
  }
}

fn below() -> Int {
  fy(1) escape {
    _ -> 0
    Bad(_) -> 1
    _ -> 2
  }
}

fn below_opaque[A](f: fn() -> A) -> A {
  f() escape {
    _ -> f()
    Boom -> f()
  }
}

fn after_total() -> Int {
  fy(1) escape {
    Bad(_) -> 0
    _ -> 1
  }
}

fn guarded(c: Bool) -> Int {
  fy(1) escape { _ if c -> 0 }
}

fn variable() -> Int {
  fy(1) escape { e -> 0 }
}
"#
    ));
}

/// `escape` removes a tag from E only when the arms catch every value of that error (spec 7.3). If
/// the arms catch only some values (field values, guards), the tag stays in E.
#[test]
fn escape_partial_arms_keep_tags() {
    insta::assert_snapshot!(render_errors(
        r#"
error NotFound(id: Int)
error Flag(on: Bool)

fn find(id: Int) -> Int fails NotFound { fail NotFound(id:) }
fn flag(on: Bool) -> Int fails Flag { fail Flag(on:) }

fn by_value() -> Int {
  find(1) escape { NotFound(id: 1) -> 0 }
}

fn by_guard() -> Int {
  find(1) escape { NotFound(id:) if id > 0 -> id }
}

fn inferred() {
  find(1) escape { NotFound(id: 1) -> 0 }
}

fn total_after_partial() -> Int {
  find(1) escape {
    NotFound(id: 1) -> 0
    NotFound(id:) if id > 0 -> id
    NotFound(_) -> 2
  }
}

fn total_by_fields() -> Int {
  flag(True) escape {
    Flag(on: True) -> 1
    Flag(on: False) -> 0
  }
}

fn local() -> Int {
  f = fn() { find(2) }
  f() escape { NotFound(id: 2) -> 0 }
}
"#
    ));
}

/// E of calls on the right of a pipe, and error type parameters (`or_fail`).
#[test]
fn pipes_and_error_type_params() {
    insta::assert_snapshot!(render_errors(
        r#"
error NotFound(id: Int)

fn or_fail[A, E](opt: Option[A], err: E) -> A fails E {
  opt match {
    Some(a) -> a
    None -> fail err
  }
}

fn first(xs: List[Int]) {
  xs |> List.head |> or_fail(NotFound(id: 0))
}

fn raise_any[E](err: E) {
  fail err
}

fn use_raise() {
  raise_any(NotFound(id: 1))
}
"#
    ));
}

/// `[effect.with]`, `[type.rules]`: the E of the arguments of init goes into the E of the `with`,
/// except for a handler a later item overrides. In a layer, the arguments cannot fail (E0318,
/// `[effect.layer]`).
#[test]
fn init_argument_errors_join_the_with() {
    insta::assert_snapshot!(render_errors(
        r#"
error Bad

effect Kv {
  fn get(key: String) -> String
}

handler RemoteKv(url: String): Kv {
  init(url: String) { RemoteKv(url:) }
  fn get(key) { self.url }
}

handler FakeKv: Kv {
  fn get(key) { "" }
}

fn url() -> String fails Bad { fail Bad }

fn live() -> String fails Bad {
  with RemoteKv(url: url()) { (use Kv).get("a") }
}

fn overridden() -> String {
  with RemoteKv(url: url()), FakeKv { (use Kv).get("a") }
}

layer Failing {
  RemoteKv(url: url())
}
"#
    ));
}
