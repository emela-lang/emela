//! Effect type parameters (spec 8.1) and State (spec 8.8). Checks that effects with different type
//! arguments are distinct in R, that `with` and `State.run` remove only effects with the same type
//! arguments, and errors for omitted or miscounted type arguments.
//!
//! State is a bundled core module (`emela-core/lib/state.emel`). The output shows declared types,
//! local variable types, handler dependencies, diagnostics of `main.emel`, and expressions with a
//! non-empty R.

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, check_program_with};

use common::{describe, resolve_main};

fn render(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let tests = program
        .defs
        .iter()
        .filter(|(def, data)| in_main(*def) && data.name.starts_with("test_"))
        .map(|(def, _)| def)
        .collect();
    let types = check_program_with(
        &program,
        &CheckOptions {
            entry: None,
            main: None,
            tests,
        },
    );
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) == Some(&def) {
                let name = &program.locals[local].name;
                writeln!(out, "  {name}: {}", ty.display(cons)).unwrap();
            }
        }
    }
    for (def, row) in types.handler_uses.iter() {
        if in_main(def) {
            let name = &program.defs[def].name;
            writeln!(
                out,
                "handler {name}: use {}",
                row.display_effects(cons, &[])
            )
            .unwrap();
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }

    let mut rows: Vec<_> = types
        .effects
        .iter()
        .filter(|(_, row)| !row.is_empty())
        .filter_map(|(expr, row)| {
            let span = program.exprs[expr].span;
            (span.module == module).then_some((span.range, row))
        })
        .collect();
    rows.sort_by_key(|(range, _)| (range.start(), std::cmp::Reverse(range.end())));
    out.push_str("--- R\n");
    for (range, row) in rows {
        let text = &src[usize::from(range.start())..usize::from(range.end())];
        let text = text.replace('\n', " ");
        writeln!(out, "{text}: {}", row.display_effects(cons, &[])).unwrap();
    }
    out
}

/// The bundled State source type-checks with core without errors, and `run` has the type of
/// spec 8.8.
#[test]
fn state_source_typechecks() {
    let (program, _, resolve_diagnostics) = resolve_main("fn unused() { () }\n");
    assert_eq!(resolve_diagnostics, []);
    let types = check_program_with(&program, &CheckOptions::default());
    assert_eq!(types.diags, []);
    let cons = &types.ty_table.cons;
    let state = program
        .defs
        .iter()
        .find(|(_, d)| d.kind == DefKind::Effect && d.name == "State")
        .map(|(def, d)| (def, d.module.clone()))
        .expect("the State effect exists");
    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        let in_state = data.module == state.1 && data.kind == DefKind::Fn;
        if !(in_state || data.parent == Some(state.0)) {
            continue;
        }
        if let Some(sig) = types.sigs.get(def) {
            writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap();
        }
    }
    insta::assert_snapshot!(out);
}

/// The example of spec 8.8. TestConsole is a handler depending on State[List[String]], provided by
/// `State.run`. The `S` of `State.run` is not determined by the initial `[]` alone but by the
/// body's R.
#[test]
fn spec_test_console() {
    insta::assert_snapshot!(render(
        r#"
import Std.Console.{Console}
import Std.State.{State}

fn greet(id: Int) -> () uses { Console } {
  (use Console).print_line("Hello, alice!")
}

handler TestConsole: Console {
  fn print_line(text) {
    st = use State[List[String]]
    st.set([text, ..st.get()])
  }
  fn print_error_line(text) { () }
}

fn test_greets() {
  (_, lines) = State.run([], || with TestConsole { greet(1) })
  assert lines == ["Hello, alice!"]
}

fn captured() {
  State.run([], || with TestConsole { greet(1) })
}
"#
    ));
}

/// A TestClock that advances time. Depends on State[Instant]; sleep resumes immediately.
#[test]
fn test_clock() {
    insta::assert_snapshot!(render(
        r#"
import Std.Clock.{Clock}
import Std.State.{State}
import Std.Instant.{Instant}
import Std.Duration.{Duration}

handler TestClock: Clock {
  suspend fn sleep(ms, resume) {
    (use State[Instant]).update(|t| Instant.add(t, Duration.milliseconds(Int64.from_int(ms))))
    resume(())
  }
  fn now() { (use State[Instant]).get() }
}

fn elapsed() -> Duration uses { Clock } {
  clock = use Clock
  start = clock.now()
  clock.sleep(1500)
  clock.now() |> Instant.since(start)
}

fn test_elapsed() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (d, last) = State.run(start, || with TestClock { elapsed() })
  assert Duration.to_milliseconds(d) == Int64.from_int(1500)
}
"#
    ));
}

/// Effects with different type arguments are different effects; `with` removes only those with
/// the same type arguments.
#[test]
fn distinct_by_type_args() {
    insta::assert_snapshot!(render(
        r#"
import Std.State.{State}

handler Zero: State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler Empty: State[String] {
  fn get() { "" }
  fn set(value) { () }
  fn update(f) { () }
}

fn both() -> () uses { State[Int], State[String] } {
  n = (use State[Int]).get()
  s = (use State[String]).get()
  ()
}

fn inferred() {
  (use State[Int]).update(|n| n + 1)
  (use State[String]).get()
}

fn only_int() { with Zero { both() } }

fn both_provided() { with Zero, Empty { both() } }

fn counted() {
  State.run(0, || {
    (use State[Int]).update(|n| n + 1)
    (use State[String]).get()
  })
}

fn nested() {
  State.run(0, || {
    State.run("", || inferred())
  })
}

fn test_leftover() { with Zero { both() } }
"#
    ));
}

/// A fn with type parameters can write them in `use State[S]`; the caller decides them.
#[test]
fn generic_state() {
    insta::assert_snapshot!(render(
        r#"
import Std.State.{State}

fn modify[S](f: fn(S) -> S fails {} uses {}) -> S uses { State[S] } {
  st = use State[S]
  st.update(f)
  st.get()
}

fn bump() { modify(|n| n + 1) }

fn run_bump() { State.run(41, || bump()) }

fn shout() { State.run("a", || modify(|s| String.append(s, "!"))) }

fn peek() { modify(|x| x) }

fn peek_int() { State.run(1, || peek()) }
"#
    ));
}

/// Omitted type arguments (E0423), wrong counts (E0304), type parameters used as sets (E0424),
/// and operations with mismatched types.
#[test]
fn errors() {
    insta::assert_snapshot!(render(
        r#"
import Std.State.{State}

effect Logger {
  fn info(msg: String) -> ()
}

effect Raise[E] {
  fn guard(f: fn() -> () fails E) -> ()
}

handler Generic: State {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler TooMany: State[Int, String] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

fn omitted() { (use State).get() }

fn clause() -> () uses { State } { () }

fn logger() { (use Logger[Int]).info("a") }

fn wrong_value() { (use State[Int]).set("a") }

fn undeclared() -> () uses { State[Int] } {
  (use State[String]).set("a")
}

fn as_type(x: State[Int]) { () }
"#
    ));
}

/// State handlers with different type arguments in layers and `with`. Only those with the same
/// type arguments are duplicates (E0421), and the later one wins. Handler operation
/// implementations are checked against the types in the handler head.
#[test]
fn handlers_and_layers() {
    insta::assert_snapshot!(render(
        r#"
import Std.Console.{Console}
import Std.State.{State}

handler Zero: State[Int] {
  fn get() { 0 }
  fn set(value) { () }
  fn update(f) { () }
}

handler One: State[Int] {
  fn get() { 1 }
  fn set(value) { () }
  fn update(f) { () }
}

handler Empty: State[String] {
  fn get() { "" }
  fn set(value) { () }
  fn update(f) { () }
}

handler Wrong: State[Int] {
  fn get() { "x" }
  fn set(value) { value + 1 }
  fn update(f) { () }
}

handler Reads: Console {
  fn print_line(text) {
    n = (use State[Int]).get()
    ()
  }
  fn print_error_line(text) { () }
}

layer Both {
  Zero
  Empty
}

layer Twice {
  Zero
  One
}

fn later_wins() { with Zero, One { (use State[Int]).get() } }

fn layered() { with Both { (use State[String]).get() } }

fn same_with() { with Zero, Reads { (use Console).print_line("x") } }

fn outside() { with Reads { (use Console).print_line("x") } }
"#
    ));
}

/// Set type arguments of a user type (spec 9.2): an error, a union of errors, an effect set, an
/// effect with type arguments, a set type parameter; and the wrong kind of set, or a type, where a
/// set is expected.
#[test]
fn set_type_arguments_of_user_types() {
    insta::assert_snapshot!(common::render_bodies(
        r##"
import Std.State.{State}

error Bad
error Worse
effect Log {
  fn info(m: String) -> ()
}
type Holder[E](f: fn() -> Int fails E uses {})
type Runner[R](f: fn() -> Int fails {} uses { R })

fn union(h: Holder[Bad | Worse]) -> Int { 1 }
fn single(h: Holder[Bad]) -> Int { 1 }
fn effects(h: Runner[{ Log }]) -> Int { 1 }
fn bare_effect(h: Runner[Log]) -> Int { 1 }
fn with_args(h: Runner[State[Int]]) -> Int { 1 }
fn param[E](h: Holder[E]) -> Int { 1 }
fn effect_for_errors(h: Holder[{ Log }]) -> Int { 1 }
fn error_for_effects(h: Runner[Bad]) -> Int { 1 }
fn type_for_set(h: Holder[List[Int]]) -> Int { 1 }
fn named_type_for_set(h: Holder[Int]) -> Int { 1 }
fn effect_with_args_for_errors(h: Holder[State[Int]]) -> Int { 1 }
fn effect_for_error(h: Holder[Log]) -> Int { 1 }
"##
    ));
}

/// A type parameter given as a set type argument of another type is a set parameter too, in
/// every form of set, and through a chain of types declared in any order: it takes a union.
#[test]
fn set_parameters_through_other_types() {
    insta::assert_snapshot!(common::render_bodies(
        r##"
error Bad
error Worse
effect Log {
  fn info(m: String) -> ()
}
type Outer[E](w: Wrap[E])
type Wrap[E](h: Holder[E])
type WrapUnion[E](h: Holder[E | Bad])
type WrapEffects[R](r: Runner[{ R, Log }])
type Holder[E](f: fn() -> Int fails E uses {})
type Runner[R](f: fn() -> Int fails {} uses { R })

fn outer(o: Outer[Bad | Worse]) -> Int { 1 }
fn union(w: WrapUnion[Bad | Worse]) -> Int { 1 }
fn effects(w: WrapEffects[{ Log }]) -> Int { 1 }
"##
    ));
}
