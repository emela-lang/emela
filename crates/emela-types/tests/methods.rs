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

fn logged(u: User) -> User use Log {
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
  xs.map(fn(s) { s.length() }).fold(from: 0, f: fn(a, b) { a + b })
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
  xs.fold(from: 0, f: fn(n, x) { n + x.area() })
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
  fn(x) { x.rename("a") }
}
"#
    ));
}
