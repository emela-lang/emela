//! Snapshot tests for body inference (`check_program`).
//!
//! For each fn in `main.emel`, prints the declared type (or the inferred, generalized type if none
//! is written) and the types of parameters and bound variables, followed by the type diagnostics
//! of `main.emel` in source order.

mod common;

use std::fmt::Write;

use emela_types::check_program;

use common::{render_bodies, resolve_main};

fn run(src: &str) -> String {
    render_bodies(src)
}

/// The bundled Prelude and core sources type-check without diagnostics.
#[test]
fn core_sources_typecheck() {
    let (program, module, _) = resolve_main("");
    let types = check_program(&program);
    let cons = &types.ty_table.cons;
    let mut out = String::new();
    for d in &types.diags {
        if d.span.is_some_and(|s| s.module == module) {
            continue;
        }
        writeln!(out, "{:?} {:?}", d.span.map(|s| s.module), d.kind).unwrap();
    }
    let _ = cons;
    assert_eq!(out, "");
}

#[test]
fn literals_and_bindings() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int) -> (Int, String) {
  a = 1
  b = 2.5
  c = "s"
  d = (a, c)
  e = ()
  (n, c)
}
"#));
}

#[test]
fn inferred_return() {
    insta::assert_snapshot!(run(r#"
fn g(x: Int, s: String) {
  y = x
  (y, s, 1.0)
}
fn unit() {
  x = 1
}
"#));
}

#[test]
fn annotations() {
    insta::assert_snapshot!(run(r#"
fn f() -> Int {
  x: Int = 1
  y: String = 2
  x
}
"#));
}

#[test]
fn return_mismatch() {
    insta::assert_snapshot!(run(r#"
fn f(x: Int) -> String {
  x
}
fn id[A](x: A) -> A {
  1
}
fn ok[A](x: A) -> A {
  y = x
  y
}
"#));
}

#[test]
fn rebinding() {
    insta::assert_snapshot!(run(r#"
fn f() -> String {
  x = 1
  x = "now a string"
  x
}
"#));
}

#[test]
fn calls() {
    insta::assert_snapshot!(run(r#"
fn add(x: Int, y: Int) -> Int { x }
fn pair[A, B](a: A, b: B) -> (A, B) { (a, b) }

fn f() -> Int {
  a = add(1, 2)
  p = pair(1, "s")
  q = pair("t", 2.0)
  a
}
"#));
}

#[test]
fn ctor_calls() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)

fn f() -> Option[User] {
  u = User(1, "a")
  s = Some(1)
  t = Some("x")
  n = None
  Some(u)
}
"#));
}

#[test]
fn never_from_prelude() {
    insta::assert_snapshot!(run(r#"
fn f(x: Int) -> String {
  y = dbg(x)
  todo()
}
"#));
}

#[test]
fn call_errors() {
    insta::assert_snapshot!(run(r#"
fn add(x: Int, y: Int) -> Int { x }

fn f(n: Int) -> Int {
  a = add(1)
  b = add(1, 2, 3)
  c = add("one", 2)
  d = n(1)
  a
}
"#));
}

#[test]
fn calling_a_local_function() {
    insta::assert_snapshot!(run(r#"
fn apply(f: fn(Int) -> String, x: Int) -> String {
  f(x)
}
"#));
}

#[test]
fn named_args() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
fn connect(host: String, port: Int) -> Bool { todo() }

fn f(id: Int) -> User {
  a = connect("localhost", 5432)
  b = connect("localhost", port: 5432)
  c = connect(port: 5432, host: "localhost")
  u = User(name: "a", id:)
  u
}
"#));
}

#[test]
fn named_arg_errors() {
    insta::assert_snapshot!(run(r#"
fn connect(host: String, port: Int) -> Bool { todo() }

fn f(g: fn(Int) -> Int) -> Int {
  a = connect("x", prot: 1)
  b = connect("x", host: "y")
  c = connect(port: "no")
  d = Some(value: 1)
  e = g(x: 1)
  1
}
"#));
}

#[test]
fn simple_patterns() {
    insta::assert_snapshot!(run(r#"
const ORIGIN: (Int, Int) = (0, 0)

fn f(p: (Int, String)) -> Int {
  (n, s) = p
  ((a, b), c) = ((1, "x"), 2.0)
  () = ()
  (_, t) = p
  n
}
"#));
}

#[test]
fn pattern_mismatch() {
    insta::assert_snapshot!(run(r#"
fn f(p: (Int, String)) -> Int {
  (a, b, c) = p
  (x, y) = 1
  () = 1
  1
}
"#));
}

#[test]
fn ctor_and_list_patterns() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)

fn f(u: User, p: Pair[Int, String], o: Option[Bool], xs: List[Int]) -> Int {
  User(id:, name:) = u
  User(name: n, ..) = u
  User(i, s) = u
  Pair(a, b) = p
  Some(flag) = o
  [x, y, ..rest] = xs
  [z, ..] = xs
  [] = xs
  id
}
"#));
}

#[test]
fn ctor_pattern_errors() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn f(u: User, s: Shape) -> Int {
  User(name:) = u
  Rect(w) = s
  Rect(w2, h2, d2) = s
  Circle(r: r) = s
  Some(v) = u
  [q] = u
  1
}
"#));
}

#[test]
fn missing_pattern_fields() {
    insta::assert_snapshot!(run(r#"
type Account(id: Int, name: String, email: String)

fn f(a: Account) -> Int {
  Account(name:) = a
  Account(name:, ..) = a
  1
}
"#));
}

#[test]
fn if_expressions() {
    insta::assert_snapshot!(run(r#"
fn f(c: Bool, n: Int) -> String {
  a = if c { 1 } else { 2 }
  b = if c { "x" } else { panic("no") }
  d = if c { todo() } else { (n, "y") }
  e = if c { () }
  g = if c { panic("stop") }
  "ok"
}
"#));
}

#[test]
fn if_errors() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int) -> Int {
  a = if n { 1 } else { 2 }
  b = if True { 1 } else { "a" }
  c = if True { 1 }
  n
}
"#));
}

#[test]
fn match_expressions() {
    insta::assert_snapshot!(run(r#"
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn area(s: Shape) -> Float {
  match s {
    Circle(radius:) if True -> radius
    Rect(w, h) -> w
    Empty -> panic("empty")
  }
}

fn first(xs: List[Int]) -> Option[Int] {
  match xs {
    [] -> None
    [x, ..] -> Some(x)
  }
}

fn label(n: Int) -> String {
  r = match n {
    0 -> "zero"
    _ -> "many"
  }
  r
}
"#));
}

#[test]
fn match_errors() {
    insta::assert_snapshot!(run(r#"
fn f(o: Option[Int]) -> Int {
  a = match o {
    Some(x) -> x
    None -> "none"
  }
  b = match o {
    Some(x) if x -> 1
    "s" -> 2
  }
  1
}
"#));
}

#[test]
fn expected_type_reaches_branches() {
    insta::assert_snapshot!(run(r#"
fn by_if(c: Bool) -> Int {
  if c { 1 } else { "a" }
}
fn by_match(o: Option[Int]) -> Int {
  match o {
    Some(x) -> x
    None -> "none"
  }
}
fn nested(c: Bool) -> Int {
  {
    y = 1
    if c { y } else { { "deep" } }
  }
}
fn no_tail() -> Int {
  x = 1
}
fn annotated(c: Bool) -> Int {
  v: String = if c { "s" } else { 2 }
  1
}
fn never_branch(c: Bool) -> Int {
  if c { panic("x") } else { 1 }
}
"#));
}

#[test]
fn int_literals() {
    insta::assert_snapshot!(run(r#"
fn take64(n: Int64) -> Int64 { n }

fn f() -> Int {
  a = 1
  b: Int64 = 3_000_000_000
  c = take64(42)
  d: Int = 2147483647
  e: Int = 2147483648
  g: Int = 0xFFFF_FFFF
  h: Int64 = 0xFFFF_FFFF
  i: Int = 0b1010
  j: Float = 1
  k: String = 2
  a
}
"#));
}

#[test]
fn operators() {
    insta::assert_snapshot!(run(r#"
fn add(x: Int, y: Int) -> Int { x }

fn f(n: Int, big: Int64, r: Float, c: Bool) -> Bool {
  a = n + 1
  b = big * 2
  d = r / 2.0
  e = -n
  g = n % 3 == 0
  h = !c && n < 10 || r >= 1.0
  i = n |> add(1)
  j = 5 |> add(n)
  k = 1 + 2
  h
}
"#));
}

#[test]
fn operator_errors() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int, big: Int64, s: String, c: Bool) -> Int {
  a = n + big
  b = s + s
  d = n + 1.5
  e = -c
  g = !n
  h = c && 1
  i = n == "x"
  n
}
"#));
}

#[test]
fn lambdas() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)

fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }
fn names(users: List[User], f: fn(User) -> String) -> List[String] { todo() }

fn f(users: List[User]) -> Int {
  inc = fn(x) { x + 1 }
  typed = fn(x: Int) { x * 2 }
  a = apply(fn(n) { n * 3 }, 1)
  b = names(users, fn(u) { u.name })
  c = inc(1)
  a
}
"#));
}

#[test]
fn lambda_errors() {
    insta::assert_snapshot!(run(r#"
fn apply(f: fn(Int) -> Int, x: Int) -> Int { f(x) }

fn f() -> Int {
  a = apply(fn(n) { "s" }, 1)
  b = apply(fn(n, m) { n }, 1)
  c = apply(fn(n: String) { 1 }, 1)
  g = fn(u) { u.name }
  1
}
"#));
}

#[test]
fn list_literals() {
    insta::assert_snapshot!(run(r#"
fn f(xs: List[Int]) -> List[Int] {
  a = [1, 2, 3]
  b = []
  c = [0, ..xs]
  d: List[Int64] = [1, 2]
  e = [[1], []]
  [1, "a"]
}
"#));
}

#[test]
fn fields() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)
error NotFound(id: Int)
enum Shape {
  Circle(radius: Float)
}

fn f(u: User, p: Pair[Int, String], e: NotFound, s: Shape, n: Int) -> String {
  a = u.id
  b = p.second
  c = e.id
  d = u.nmae
  g = s.radius
  h = n.name
  u.name
}
"#));
}

#[test]
fn interpolation_and_assert() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int, name: String) -> String {
  assert n > 0
  assert n
  "hello #{name}, #{n + 1}"
}
"#));
}

#[test]
fn record_update() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)
type Pair[A, B](first: A, second: B)

fn f(u: User, p: Pair[Int, Bool]) -> Int {
  a = User(name: "b", ..u)
  b = Pair(first: "s", ..p)
  c = User(name: 1, ..u)
  d = User(nmae: "x", ..u)
  g = User(name: "x", ..p)
  1
}
"#));
}

#[test]
fn inferred_functions_are_generalized() {
    insta::assert_snapshot!(run(r#"
fn use_them() -> Int {
  a = wrap(1)
  b = wrap("s")
  c = empty(0)
  d: List[String] = empty(1)
  e = one()
  a_len()
}

fn wrap[A](x: A) { [x] }
fn untyped(x) { x }
fn empty(n: Int) { [] }
fn one() { 1 }
fn keep[A](x: A, n: Int) { (x, n) }
const LIMIT = 10
fn a_len() { LIMIT }
"#));
}

#[test]
fn recursion() {
    insta::assert_snapshot!(run(r#"
fn count(n: Int) {
  if n == 0 { 0 } else { count(n - 1) }
}

fn is_even(n: Int) {
  if n == 0 { True } else { is_odd(n - 1) }
}
fn is_odd(n: Int) {
  if n == 0 { False } else { is_even(n - 1) }
}

fn length[A](xs: List[A]) -> Int {
  match xs {
    [] -> 0
    [_, ..rest] -> 1 + length(rest)
  }
}
"#));
}

#[test]
fn recursion_is_monomorphic_without_annotation() {
    insta::assert_snapshot!(run(r#"
fn f(n: Int) {
  a = g(1)
  b = g(2)
  c = h(n)
  d = h("s")
  e = uses_k_twice(n)
  n
}
fn g[A](x: A) {
  f(1)
  x
}
fn h(x: Int) {
  f(x)
}
fn k(n: Int) {
  f(n)
  []
}
fn uses_k_twice(n: Int) {
  xs: List[Int] = k(1)
  ys: List[String] = k(2)
  f(n)
}
"#));
}

#[test]
fn exhaustiveness() {
    insta::assert_snapshot!(run(r#"
enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn area(s: Shape) -> Float {
  match s {
    Circle(radius:) -> radius
    Rect(w, h) -> w * h
  }
}

fn guarded(o: Option[Int]) -> Int {
  match o {
    Some(x) if x > 0 -> x
    None -> 0
  }
}

fn nested(o: Option[Option[Bool]]) -> Int {
  match o {
    Some(Some(True)) -> 1
    None -> 0
  }
}

fn lists(xs: List[Int]) -> Int {
  match xs {
    [] -> 0
    [x] -> x
  }
}

fn bools(a: Bool, b: Bool) -> Int {
  match (a, b) {
    (True, _) -> 1
    (False, True) -> 2
    (False, False) -> 3
  }
}

fn literals(n: Int) -> String {
  match n {
    0 -> "zero"
    1 -> "one"
  }
}

fn named(u: Option[Shape]) -> Int {
  match u {
    Some(Circle(radius: _)) -> 1
    Some(Rect(..)) -> 2
    Some(Empty) -> 3
    None -> 4
  }
}
"#));
}

#[test]
fn unreachable_arms() {
    insta::assert_snapshot!(run(r#"
const MAX = 10

fn f(o: Option[Int], n: Int) -> Int {
  a = match o {
    _ -> 0
    Some(x) -> x
  }
  b = match n {
    MAX -> 1
    MAX -> 2
    _ -> 3
  }
  c = match o {
    Some(x) if x > 0 -> x
    Some(x) -> x
    None -> 0
  }
  a
}
"#));
}

#[test]
fn refutable_bindings() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)

fn f(o: Option[Int], xs: List[Int], u: User, p: (Int, Bool)) -> Int {
  Some(x) = o
  [y, ..rest] = xs
  0 = x
  User(id:, ..) = u
  (n, True) = p
  (m, b) = p
  id
}
"#));
}
