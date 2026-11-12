//! Snapshot tests for method calls (`[type.rules]`): `x.f(a)` is typed as `M.f(x, a)` of the
//! module that defines the type of x, or as `Trait.f(x, a)` of a visible trait. The output is the
//! inferred types of the fns (with their E and R) and the diagnostics.

mod common;

use common::render_bodies;

/// The type, E and R of a method call are those of the call `M.f(x, a)`, with the same named
/// arguments. Functions without a declared type can be called before their definition.
#[test]
fn method_calls_have_the_type_of_the_call() {
    insta::assert_snapshot!(render_bodies(
        r#"
error Bad

effect Log {
  fn write(s: String) -> ()
}

type User(name: String, age: Int)

fn chained(u: User) {
  u.rename("a").check().logged()
}

fn named(u: User) {
  u.rename(name: "b").name
}

fn before(u: User) {
  u.later()
}

fn rename(u: User, name: String) -> User {
  User(name:, ..u)
}

fn check(u: User) -> User fails Bad {
  if u.age < 0 { fail Bad } else { u }
}

fn logged(u: User) -> User uses { Log } {
  (use Log).write(u.name)
  u
}

fn later(u: User) {
  u.age + 1
}

fn private_here(u: User) {
  u.helper()
}

fn helper(u: User) -> Int {
  u.age
}
"#
    ));
}

/// The functions of the core modules are found from the head of the type: List, String, Option,
/// Int. Only the head is needed: `List.join` is found for a `List[Int]`, and the argument does
/// not match.
#[test]
fn core_functions_are_methods() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn lengths(xs: List[String]) {
  xs.map(|s| s.length()).fold(from: 0, f: |a, b| a + b)
}

fn first_line(s: String) {
  s.split("\n").head().or("")
}

fn joined(xs: List[String]) {
  xs.join(", ")
}

fn digits(n: Int) {
  n.to_string().length()
}

fn not_strings(xs: List[Int]) -> String {
  xs.join(", ")
}
"#
    ));
}

/// A field is looked up first, and a field of function type is called. Trait functions that
/// take `self` are found for the traits of the Prelude and the traits of the module, and for a
/// type parameter, the traits of its bounds and their supertraits.
#[test]
fn fields_and_trait_functions() {
    insta::assert_snapshot!(render_bodies(
        r##"
trait Area {
  fn area(self) -> Int
}

trait Shape: Area {
  fn name(self) -> String
}

type Square(side: Int)
  derive Show

type Greeter(greet: fn(String) -> String)

impl Square: Area {
  fn area(self) { self.side * self.side }
}

impl Square: Shape {
  fn name(self) { "square" }
}

fn call_field(g: Greeter) {
  g.greet("hi")
}

fn traits(s: Square) {
  "#{s.show()} #{s.area()} #{s.name()}"
}

fn total[A: Area](xs: List[A]) -> Int {
  xs.fold(from: 0, f: |n, x| n + x.area())
}

fn through_super[A: Shape](x: A) -> Int {
  x.area()
}

fn unbounded[A](x: A) -> Int {
  x.area()
}
"##
    ));
}

/// A method call is ordered before only the fns that can be its callee: those whose first
/// parameter has a type of their own module. `get` here takes a String, defined by the core
/// module String, so `[f()].get(0)` (`List.get`) does not put it in a group with `run`, and its
/// type is generalized before `main` uses it.
#[test]
fn a_method_call_does_not_order_before_fns_of_other_modules() {
    insta::assert_snapshot!(render_bodies(
        r#"
fn run(f: fn() -> Int) -> Int {
  [f()].get(0).or(0)
}

fn get(url: String) -> Int {
  run(|| url.length())
}

fn main() -> Int {
  get("x")
}
"#
    ));
}

/// Errors at a method call: a wrong number of arguments (E0305), a function written without a
/// call (E0312), no field or function (E0312), a type without fields (E0313), and an undetermined
/// type (E0314).
#[test]
fn method_call_errors() {
    insta::assert_snapshot!(render_bodies(
        r#"
type User(name: String)

fn rename(u: User, name: String) -> User {
  User(name:, ..u)
}

fn too_few(u: User) {
  u.rename()
}

fn not_called(u: User) {
  u.rename
}

fn misspelled(u: User) {
  u.renmae("a")
}

fn tuple(p: (Int, Int)) {
  p.first()
}

fn unknown() {
  |x| x.rename("a")
}
"#
    ));
}

/// The names [`emela_types::methods_of`] gives for a value of the type `ty_src` (the type of the
/// parameter `x` of `probe`) seen from `main.emel`.
fn methods(src: &str) -> Vec<String> {
    use emela_resolve::hir::{DefKind, DefModule, Item};
    let (program, main, _) = common::resolve_main(src);
    let types = emela_types::check_program(&program);
    let probe = program
        .defs
        .iter()
        .find(|(_, d)| d.name == "probe" && d.module == DefModule::Source(main))
        .map(|(id, _)| id)
        .unwrap();
    let Some(Item::Fn(f)) = program.item(probe) else {
        unreachable!()
    };
    let x = f.sig.params[0].local;
    let ty = types.locals[x].clone();
    emela_types::methods_of(&program, &types, main, &ty, &[])
        .into_iter()
        .map(|def| {
            let data = &program.defs[def];
            match (data.kind, data.parent) {
                (DefKind::TraitFn, Some(t)) => format!("{}.{}", program.defs[t].name, data.name),
                _ => data.name.to_string(),
            }
        })
        .collect()
}

/// The fns of the type's module whose first parameter has the type at its head, private ones
/// included from that module, then the `self` functions of the traits it implements.
#[test]
fn methods_of_a_type_of_this_module() {
    let found = methods(
        r#"
trait Named {
  fn name(self) -> String
  fn make() -> Self
}
type User(id: Int)
  derive Eq
impl User: Named {
  fn name(self) { "" }
  fn make() { User(id: 1) }
}
type Other(id: Int)

pub fn rename(u: User) -> User { u }
fn private(u: User) -> Int { 1 }
fn second(n: Int, u: User) -> Int { 1 }
fn other(o: Other) -> Int { 1 }
const ME: User = User(id: 1)

fn probe(x: User) { () }
"#,
    );
    assert_eq!(found, ["rename", "private", "probe", "Eq.eq", "Named.name"]);
}

/// A type of the core has the pub fns of its core module; a trait is found for a type whatever
/// its type arguments (a `List[fn() -> Int]` finds `Show.show`). Tuples and functions have none.
#[test]
fn methods_of_core_types() {
    let found = methods("fn probe(x: List[fn() -> Int]) { () }\n");
    assert!(found.contains(&"map".to_string()), "{found:?}");
    assert!(found.contains(&"Show.show".to_string()), "{found:?}");
    assert!(!found.iter().any(|f| f.starts_with('_')), "{found:?}");
    let boxed =
        methods("type Box[A](value: A)\n  derive Show\nfn probe(x: Box[fn() -> Int]) { () }\n");
    assert_eq!(boxed, ["probe", "Show.show"]);
    // Types without a head have no methods to complete.
    assert_eq!(
        methods("fn probe(x: (Int, Int)) { () }\n"),
        Vec::<String>::new()
    );
    assert_eq!(
        methods("fn probe(x: fn() -> Int) { () }\n"),
        Vec::<String>::new()
    );
}

/// Method calls at their edges: a callee inferred later is inferred first, a name in both the
/// type's module and a trait is ambiguous, a value that never returns has every method, a type
/// parameter in the first parameter does not make a method, and an opaque type's fields are
/// hidden outside its module.
#[test]
fn method_calls_at_their_edges() {
    insta::assert_snapshot!(render_bodies(
        r#"
import Std.Duration.{Duration}

type User(id: Int)
  derive Show

fn caller(u: User) -> Int { u.later() }
fn later(u: User) { 1 }

fn show(u: User) -> String { "" }
fn ambiguous(u: User) -> String { u.show() }

fn never() -> Int {
  x = todo()
  x.anything()
}

fn generic[A](a: A) -> Int { 1 }
fn not_a_method(u: User) -> Int { u.generic() }

fn hidden(d: Duration) -> Int64 { d.milliseconds }

fn tuple_method(t: (Int, fn() -> Int)) -> String { t.show() }

fn without_call(u: User) { u.untyped }
fn without_call_generic(u: User) { u.generic_untyped }
fn untyped(u: User) { 1 }
fn generic_untyped[A](a: A) { 1 }
"#
    ));
}
