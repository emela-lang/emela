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
  s match {
    Circle(radius:) if True -> radius
    Rect(w, h) -> w
    Empty -> panic("empty")
  }
}

fn first(xs: List[Int]) -> Option[Int] {
  xs match {
    [] -> None
    [x, ..] -> Some(x)
  }
}

fn label(n: Int) -> String {
  r = n match {
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
  a = o match {
    Some(x) -> x
    None -> "none"
  }
  b = o match {
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
  o match {
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
  inc = |x| x + 1
  typed = |x: Int| x * 2
  a = apply(|n| n * 3, 1)
  b = names(users, |u| u.name)
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
  a = apply(|n| "s", 1)
  b = apply(|n, m| n, 1)
  c = apply(|n: String| 1, 1)
  g = |u| u.name
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
  xs match {
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
  s match {
    Circle(radius:) -> radius
    Rect(w, h) -> w * h
  }
}

fn guarded(o: Option[Int]) -> Int {
  o match {
    Some(x) if x > 0 -> x
    None -> 0
  }
}

fn nested(o: Option[Option[Bool]]) -> Int {
  o match {
    Some(Some(True)) -> 1
    None -> 0
  }
}

fn lists(xs: List[Int]) -> Int {
  xs match {
    [] -> 0
    [x] -> x
  }
}

fn bools(a: Bool, b: Bool) -> Int {
  (a, b) match {
    (True, _) -> 1
    (False, True) -> 2
    (False, False) -> 3
  }
}

fn literals(n: Int) -> String {
  n match {
    0 -> "zero"
    1 -> "one"
  }
}

fn named(u: Option[Shape]) -> Int {
  u match {
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
  a = o match {
    _ -> 0
    Some(x) -> x
  }
  b = n match {
    MAX -> 1
    MAX -> 2
    _ -> 3
  }
  c = o match {
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

/// Patterns of lambda parameters must always match, as bindings do (E0317, spec 6.1). Their names
/// get types from the parameter.
#[test]
fn lambda_param_patterns() {
    insta::assert_snapshot!(run(r#"
type User(id: Int, name: String)

fn f(us: List[User], ps: List[(Int, Bool)], os: List[Option[Int]]) -> List[Int] {
  a = List.map(us, |User(id:, ..)| id)
  b = List.map(ps, |(n, _)| n)
  c = List.map(os, |Some(x)| x)
  d = List.map(ps, |(n, True)| n)
  e = List.fold(ps, from: 0, f: |acc, (n, _): (Int, Bool)| acc + n)
  List.flatten([a, b, c, d, [e]])
}
"#));
}

/// Consts whose values certainly evaluate each other are an error (E0231, spec 5.6). References
/// in a branch, on the right of `&&`, in a lambda, or through a function are left to the defect
/// at run time.
#[test]
fn const_cycles() {
    insta::assert_snapshot!(run(r#"
const A = B + 1
const B = {
  x = C
  x * A
}
const C = 1
const SELF = SELF
const P = (Q, 1)
const Q = [P]
const BRANCH = if True { OTHER } else { 0 }
const OTHER = BRANCH
const LAZY = False && LAZY2
const LAZY2 = LAZY
const LAMBDA = || LAMBDA2()
const LAMBDA2 = LAMBDA
const CALL = f()

fn f() -> Int { CALL }
"#));
}

/// A const in a pattern stands for its value in the exhaustiveness and redundancy checks when the
/// value is made only of literals, negated numeric literals, constructors, tuples, lists and other
/// such consts. Otherwise its arm counts like a guarded arm (spec 5.6, 6.5).
#[test]
fn const_patterns() {
    insta::assert_snapshot!(run(r#"
enum Shape {
  Circle(Int)
  Rect(w: Int, h: Int)
  Empty
}

const NOTHING = Empty
const ORIGIN = Circle(0)
const SQUARE = Rect(h: 1, w: 1)
const PAIR = (NOTHING, -1)
const MINUS_ONE = -1
const ALSO_MINUS_ONE = MINUS_ONE
const DIGITS = [1, 2]
const MORE = [0, ..DIGITS]
const COMPUTED = Circle(zero())
const LIMIT = zero() + 10

fn zero() -> Int { 0 }

fn known(s: Shape) -> Int {
  s match {
    NOTHING -> 0
    Circle(_) -> 1
    Rect(..) -> 2
  }
}

fn known_redundant(s: Shape) -> Int {
  s match {
    Circle(_) -> 1
    Rect(w: 1, h: 1) -> 2
    SQUARE -> 3
    ORIGIN -> 4
    _ -> 5
  }
}

fn tuples(p: (Shape, Int)) -> Int {
  p match {
    (Circle(_), _) -> 0
    (Rect(..), _) -> 1
    (Empty, 0) -> 2
    PAIR -> 3
    (Empty, _) -> 4
    (NOTHING, _) -> 5
  }
}

fn negatives(n: Int) -> Int {
  n match {
    MINUS_ONE -> 0
    ALSO_MINUS_ONE -> 1
    _ -> 2
  }
}

fn lists(xs: List[Int]) -> Int {
  xs match {
    MORE -> 0
    [0, 1, 2] -> 1
    _ -> 2
  }
}

fn computed_not_counted(s: Shape) -> Int {
  s match {
    COMPUTED -> 0
    Rect(..) -> 1
  }
}

fn computed_covers_nothing(s: Shape) -> Int {
  s match {
    COMPUTED -> 0
    Circle(_) -> 1
    Rect(..) -> 2
    Empty -> 3
  }
}

fn computed_redundant(n: Int) -> Int {
  n match {
    _ -> 0
    LIMIT -> 1
  }
}

fn computed_binding(s: Shape) -> Int {
  COMPUTED = s
  0
}
"#));
}

/// Coverage of matches and bindings (spec 6.5): `()` and tuples are covered by their single
/// shape, a type's constructor by its fields given positionally or by name, and a negative float
/// is its own value.
#[test]
fn coverage_of_units_tuples_records_and_floats() {
    insta::assert_snapshot!(common::render_diagnostics(
        r##"
type Point(x: Int, y: Int)

fn unit(u: ()) -> Int {
  u match {
    () -> 1
  }
}

fn tuple(p: (Int, Bool)) -> Int {
  p match {
    (_, True) -> 1
    (_, False) -> 2
  }
}

fn positional(p: Point) -> Int {
  p match {
    Point(0, _) -> 1
    Point(_, 0) -> 2
  }
}

fn named(p: Point) -> Int {
  p match {
    Point(y: 0, x: _) -> 1
    Point(x: 0, y: _) -> 2
    Point(x: 0, y: 1) -> 3
  }
}

fn floats(f: Float) -> Int {
  f match {
    -1.5 -> 1
    1.5 -> 2
    -1.5 -> 3
    _ -> 4
  }
}

fn bindings(p: (Int, Int), u: ()) -> Int {
  (a, 1) = p
  () = u
  (b, c) = (1, 2)
  a
}
"##
    ));
}

/// The names [`emela_types::body_refs`] gives for the definition `name` of `main.emel`.
fn refs_of(src: &str, name: &str) -> Vec<String> {
    use emela_resolve::hir::DefModule;
    let (program, main, _) = common::resolve_main(src);
    let def = program
        .defs
        .iter()
        .find(|(_, d)| d.name == name && d.module == DefModule::Source(main))
        .map(|(id, _)| id)
        .unwrap();
    emela_types::body_refs(&program, def)
        .into_iter()
        .map(|(d, span)| format!("{}@{}", program.defs[d].name, &src[span.range]))
        .collect()
}

/// A body refers to the fns it calls and to the consts in its patterns, inside constructors,
/// tuples and lists; a layer refers to what the arguments of its items call.
#[test]
fn body_refs_include_consts_in_patterns_and_layer_arguments() {
    let src = r##"
const MAX: Int = 3
const MIN: Int = 0
type Box(v: Int)
fn helper() -> Int { 1 }
fn f(xs: List[Int], p: (Int, Int), b: Box) -> Int {
  xs match {
    [MAX, ..] -> 1
    _ -> 2
  }
  p match {
    (MIN, _) -> 1
    _ -> 2
  }
  b match {
    Box(v: MAX) -> 1
    _ -> 2
  }
  helper()
}
effect Log {
  fn info(msg: String) -> ()
}
handler Quiet(level: Int): Log {
  init(level: Int) { Quiet(level:) }
  fn info(msg) { () }
}
layer App { Quiet(level: helper()) }
"##;
    assert_eq!(
        refs_of(src, "f"),
        ["MAX@MAX", "MIN@MIN", "MAX@MAX", "helper@helper"]
    );
    assert_eq!(refs_of(src, "App"), ["helper@helper"]);
}

/// A const in a pattern stands for its value: an arm after one with the same value is
/// unreachable, whether the value is a tuple, a negative float, or a record built positionally or
/// by name (spec 6.5).
#[test]
fn consts_in_patterns_are_compared_by_value() {
    insta::assert_snapshot!(common::render_diagnostics(
        r##"
type Point(x: Int, y: Int)
const ORIGIN: (Int, Int) = (0, 0)
const NEG: Float = -1.5
const P: Point = Point(0, 1)
const Q: Point = Point(y: 1, x: 0)
const UNIT: () = ()

fn tuple(t: (Int, Int)) -> Int {
  t match {
    ORIGIN -> 1
    (0, 0) -> 2
    _ -> 3
  }
}

fn float(f: Float) -> Int {
  f match {
    NEG -> 1
    -1.5 -> 2
    1.5 -> 3
    _ -> 4
  }
}

fn record(p: Point) -> Int {
  p match {
    P -> 1
    Point(0, 1) -> 2
    Q -> 3
    Point(1, 0) -> 4
    _ -> 5
  }
}

fn unit(u: ()) -> Int {
  u match {
    UNIT -> 1
    () -> 2
  }
}
"##
    ));
}

/// A reference to a generic definition records the types that instantiate it, and one with set
/// parameters the sets; a reference to a monomorphic one records neither.
#[test]
fn references_record_their_instantiations() {
    use emela_resolve::hir::{DefModule, ExprKind, Res};
    let (program, main, _) = common::resolve_main(
        r##"
fn id[A](a: A) -> A { a }
fn apply[E](f: fn() -> Int fails E) -> Int fails E { f() }
fn plain(n: Int) -> Int { n }
fn use_them() -> Int { plain(id(1)) + apply(|| 2) }
"##,
    );
    let types = emela_types::check_program(&program);
    let cons = &types.ty_table.cons;
    let mut found: Vec<String> = Vec::new();
    for (expr, e) in program.exprs.iter() {
        let ExprKind::Path(Res::Def(def)) = e.kind else {
            continue;
        };
        if program.defs[def].module != DefModule::Source(main) {
            continue;
        }
        let tys: Vec<String> = types
            .type_args
            .get(expr)
            .map(|args| args.iter().map(|t| t.display(cons).to_string()).collect())
            .unwrap_or_default();
        let rows = types.type_rows.get(expr).map_or(0, Vec::len);
        found.push(format!(
            "{} [{}] rows={rows}",
            program.defs[def].name,
            tys.join(", ")
        ));
    }
    assert_eq!(found[0], "plain [] rows=0");
    assert!(found[1].starts_with("id [Int] "), "{found:?}");
    assert!(!found[2].ends_with("rows=0"), "{found:?}");
}

/// Labels written twice in a partial update are E0305 (spec 5.3).
#[test]
fn a_partial_update_names_each_field_once() {
    insta::assert_snapshot!(common::render_diagnostics(
        r##"
type User(id: Int, name: String)
fn f(u: User) -> User { User(id: 1, id: 2, ..u) }
"##
    ));
}

/// A cycle among consts is E0336, reported along one cycle even when the consts branch.
#[test]
fn const_cycles_with_branches() {
    insta::assert_snapshot!(common::render_diagnostics(
        r##"
const AA: Int = BB + CC
const BB: Int = CC
const CC: Int = AA
"##
    ));
}

/// A method call to a generic fn records its instantiation on the callee `x.f`, as a reference
/// by name does.
#[test]
fn method_calls_record_their_instantiations() {
    use emela_resolve::hir::ExprKind;
    let (program, main, _) = common::resolve_main(
        r##"
type Box[A](value: A)
fn wrap[A](b: Box[A]) -> Box[A] { b }
fn apply[E](b: Box[Int], f: fn() -> Int fails E) -> Int fails E { f() }
fn plain(b: Box[Int]) -> Int { 1 }
fn use_them(b: Box[Int]) -> Int {
  c = b.wrap()
  d = b.apply(|| 1)
  b.plain()
}
"##,
    );
    let types = emela_types::check_program(&program);
    let found: Vec<(String, usize, usize)> = program
        .exprs
        .iter()
        .filter(|(_, e)| e.span.module == main)
        .filter_map(|(expr, e)| match &e.kind {
            ExprKind::Field { name, .. } => Some((
                name.name.to_string(),
                types.type_args.get(expr).map_or(0, Vec::len),
                types.type_rows.get(expr).map_or(0, Vec::len),
            )),
            _ => None,
        })
        .collect();
    let s = |n: &str, a, r| (n.to_string(), a, r);
    assert_eq!(found[0], s("wrap", 1, 1), "{found:?}");
    assert!(found[1].0 == "apply" && found[1].2 > 0, "{found:?}");
    assert_eq!(found[2], s("plain", 0, 0), "{found:?}");
}

/// Matching a handler value covers it by its fields, as a type with fields (spec 6.5); a type
/// with set arguments in a mismatch is shown without unsolved set variables.
#[test]
fn handler_values_in_matches_and_set_arguments_in_mismatches() {
    insta::assert_snapshot!(common::render_diagnostics(
        r##"
effect Log {
  fn info(m: String) -> ()
}
handler Leveled(level: Int): Log {
  init(level: Int) {
    x = Leveled(level:)
    x match {
      Leveled(level: 0) -> x
    }
  }
  fn info(m) { () }
}
type Holder[E](f: fn() -> Int fails E uses {})
fn f() -> Int {
  x: Int = Holder(f: || 1)
  x
}
"##
    ));
}
