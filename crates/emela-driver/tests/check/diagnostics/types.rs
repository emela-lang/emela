//! Diagnostics of type checking: method calls, names that stay free, closures, local functions
//! and type parameters.

use emela_driver::{Typecheck, render};

use super::check_files;

#[test]
fn type_checking_reports_in_source_order() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"type User(id: Int, name: String)

enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn connect(host: String, port: Int) -> Bool { todo() }

fn area(s: Shape) -> Float {
  s match {
    Circle(radius:) -> radius
    Rect(w, h) -> w * h
    _ -> 0.0
    Empty -> 1.0
  }
}

fn main() {
  u = User(id: 1, name: "a")
  a: Int = u.name
  b = connect("x", prot: 1)
  c: Int = 3000000000
  Some(x) = Some(1)
  User(name:) = u
  d = u.nmae
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The diagnostics of method calls (`[type.rules]`, `[data.type]`): no field or function (E3106,
/// also for a function written without a call), a type without fields (E3107), an undetermined
/// type (E3005), a fn named like a field (E3109) and more than one candidate (E3110). A trait
/// reached only as `Traits.Named` is not looked up, and a private fn is not found outside its
/// module.
#[test]
fn method_call_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/traits.emel",
                r#"pub trait Seq {
  fn size(self) -> Int
}

pub trait Named {
  fn label(self) -> String
}
"#,
            ),
            (
                "app/src/shapes.emel",
                r#"import Traits.{Seq, Named}

pub type Square(side: Int)

pub type Box(width: Int)

pub fn area(s: Square) -> Int {
  s.side * s.side
}

fn secret(s: Square) -> Int {
  s.side
}

pub fn width(b: Box) -> Int {
  b.width
}

impl Square: Seq {
  fn size(self) { 4 }
}

impl Square: Named {
  fn label(self) { "square" }
}
"#,
            ),
            (
                "app/src/main.emel",
                r#"import Shapes
import Traits
import Traits.{Seq}

trait Big {
  fn size(self) -> Int
}

type Point(x: Int)

type Rec(size: Int)

impl Point: Seq {
  fn size(self) { 1 }
}

impl Rec: Seq {
  fn size(self) { 2 }
}

impl Shapes.Square: Big {
  fn size(self) { 3 }
}

fn size(p: Point) -> Int {
  p.x
}

fn private(s: Shapes.Square) -> Int {
  s.secret()
}

fn qualified_only(s: Shapes.Square) -> String {
  s.label()
}

fn not_called(s: Shapes.Square) -> Int {
  f = s.area
  0
}

fn misspelled(s: Shapes.Square) -> Int {
  s.aera()
}

fn no_fields(n: Int) -> Int {
  n.nmae()
}

fn unknown() -> Int {
  f = |x| x.area()
  0
}

fn module_and_trait(p: Point) -> Int {
  p.size()
}

fn field_and_trait(r: Rec) -> Int {
  r.size()
}

fn two_traits(s: Shapes.Square) -> Int {
  s.size()
}
"#,
            ),
        ],
        "app",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// A method call needs no import of the module that defines the type (`[module.resolution]`),
/// and finds the traits imported by name, defined in the module, and of the Prelude.
#[test]
fn method_calls_without_imports_check_cleanly() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/shapes.emel",
                r#"pub type Square(side: Int)
  derive Show

pub fn area(s: Square) -> Int {
  s.side * s.side
}

pub fn grow(s: Square, by: Int) -> Square {
  Square(side: s.side + by)
}
"#,
            ),
            (
                "app/src/factory.emel",
                r#"import Shapes.{Square}

pub fn unit() -> Square {
  Square(side: 1)
}
"#,
            ),
            (
                "app/src/main.emel",
                r#"import Factory

trait Doubled {
  fn doubled(self) -> Int
}

impl Int: Doubled {
  fn doubled(self) { self * 2 }
}

fn main() {
  s = Factory.unit()
  t = s.grow(by: 2)
  label = t.show()
  label.length() + t.area().doubled() + ["a", "b"].join(", ").length()
  ()
}
"#,
            ),
        ],
        "app",
        &mut frontend,
    );
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

/// `[api.std.bytes]`, compatibility: `Bytes` is reached only through `import Std.Bytes`, so a
/// module `bytes.emel` and a user type named `Bytes` keep working. Outside `Std.Bytes`, a Bytes is
/// made only by its functions: it has no constructor to call or match.
#[test]
fn bytes_names_stay_free() {
    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/bytes.emel",
            "pub type Bytes(value: Int)\n\npub fn count(b: Bytes) -> Int { b.value }\n",
        ),
        (
            "app/src/main.emel",
            "import Bytes\n\nfn main() {\n  dbg(Bytes.count(Bytes.Bytes(value: 1)))\n  ()\n}\n",
        ),
    ];
    let analysis = check_files(&files, "app", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");

    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            r#"import Std.Bytes.{Bytes}

fn size(b: Bytes) -> Int {
  b match {
    Bytes() -> 0
  }
}

fn main() {
  dbg(Bytes())
}
"#,
        ),
    ];
    let analysis = check_files(&files, "app", &mut Typecheck::default());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `[api.std.array]`, compatibility: `Array` is reached only through `import Std.Array`, so a
/// module `array.emel` and a user type named `Array` keep working. Outside `Std.Array`, an Array
/// is made only by its functions: it has no constructor to call or match.
#[test]
fn array_names_stay_free() {
    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/array.emel",
            "pub type Array(n: Int)\n\npub fn count(a: Array) -> Int { a.n }\n",
        ),
        (
            "app/src/main.emel",
            "import Array\n\nfn main() {\n  dbg(Array.count(Array.Array(n: 1)))\n  ()\n}\n",
        ),
    ];
    let analysis = check_files(&files, "app", &mut Typecheck::default());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");

    let files = [
        ("app/Pome.toml", ""),
        (
            "app/src/main.emel",
            r#"import Std.Array.{Array}

fn size(a: Array[Int]) -> Int {
  a match {
    Array() -> 0
  }
}

fn main() {
  dbg(Array())
}
"#,
        ),
    ];
    let analysis = check_files(&files, "app", &mut Typecheck::default());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// A guard written after `if` is W1401 at `if`, in a match arm and in an escape arm; the program
/// keeps its meaning and has no error. A guard after `where` has no diagnostic, and `where` is
/// not an identifier (`[grammar.expr]`, `[kw.reserved]`, `[diag.list]`).
#[test]
fn guard_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Boom(code: Int)

fn f(o: Option[Int]) -> Int {
  o match {
    Some(x) if x > 0 -> x
    Some(x) where x < 0 -> -x
    _ -> 0
  }
}

fn g() -> Int fails Boom {
  fail Boom(code: 1)
}

fn h() -> Int {
  g() escape {
    Boom(code:) if code > 0 -> code
    Boom(_) -> 0
  }
}

fn main() {
  dbg(f(Some(1)) + h())
  ()
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    assert!(!analysis.has_errors());
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));

    let analysis = check_files(
        &[(
            "home/main.emel",
            "fn where() -> Int {\n  1\n}\n\nfn main() -> Int {\n  where = 1\n  0\n}\n",
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    let codes: Vec<_> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| d.code)
        .collect();
    assert_eq!(codes, [Some("E1101"), Some("E1102"), Some("E1109")]);
}

/// The old form of an anonymous function (E1405) shows its rewrite; an anonymous function before
/// the last statement of a block is E1112; a line that starts with `||` gets a help on E1112 and
/// on E3001 (`[fn.decl]`, `[fn.block]`, `[diag.list]`).
#[test]
fn closure_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"fn old(xs: List[Int]) -> List[Int] {
  ys = List.map(xs, fn(x) { x + 1 })
  List.map(ys, fn(y) {
    z = y * 2
    z
  })
}

fn unused() -> Int {
  |x| x
  1
}

fn continued(a: Bool, b: Bool) -> Bool {
  x = a
  || b
  x
}

fn last(a: Bool, b: Bool) -> Bool {
  x = a
  || b
}

fn main() -> Int {
  0
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// What a local fn cannot have (E1212, E1213), a local fn as the last statement of its block (E1214),
/// a parameter without a type (E3006), and its written `fails` and `uses` (E3201, E4301), as the
/// user sees them (`[fn.local]`, `[diag.list]`).
#[test]
fn local_fn_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Boom

effect Log {
  fn info(msg: String) -> ()
}

fn modifiers() -> Int {
  fn generic[A](x: A) -> A { x }
  @test
  fn annotated() -> Int { 1 }
  pub fn public() -> Int { 1 }
  suspend fn later() -> Int { 1 }
  1
}

fn last() {
  fn go(n: Int) -> Int { n }
}

fn untyped() -> Int {
  fn go(n) -> Int { 1 }
  go(1)
}

fn unhandled() -> Int {
  fn go() -> Int { fail Boom }
  go()
}

fn undeclared() {
  fn say() -> () uses {} {
    log = use Log
    log.info("x")
  }
  say()
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// W3002 (`[fn.decl]`, `[trait.decl]`, `[ext.decl]`): a type parameter of a top-level fn, of a
/// trait function or of an `@external` fn that does not appear in the type of the fn. One that
/// appears only in a bound or in type annotations inside the body is warned about too, and set
/// type parameters are treated as ordinary ones. The warning points at the type parameter in the
/// list, once for a trait function and not at its impls.
#[test]
fn unused_type_parameters() {
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"fn f[A]() -> Int { 1 }

fn g[A](x: Int) -> Int {
  y: Option[A] = None
  x
}

fn h[A: Show]() -> Int { 1 }

fn e[E, R](x: Int) -> Int { x }

fn kept[A, B](x: A) -> A { x }

trait Describe {
  fn describe[A](self) -> String
}

type User(name: String)

impl User: Describe {
  fn describe(self) { self.name }
}

@external(js, "./x.mjs", "f")
fn ext[A](x: Int) -> Int uses {}

fn main() {
  dbg(f())
  dbg(g(1))
  dbg(h())
  dbg(e(1))
  dbg(kept(1))
  dbg(User("a").describe())
  ()
}
"#,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// No W3002 for a type parameter that appears in the type of the fn (`[fn.decl]`): in a
/// parameter, the return type, fails or uses, inside a function type or a type argument, or in a
/// return type, fails or uses inferred from the body. Nor for type parameters of types and enums
/// (phantom types), of an impl head, the `Self` of a trait function, or a fn whose signature
/// could not be read (the error is reported instead).
#[test]
fn used_type_parameters_without_warning() {
    let analysis = check_files(
        &[(
            "home/main.emel",
            r##"error Oops

effect Log {
  fn info(msg: String) -> ()
}

fn param[A](x: A) -> Int { 1 }

fn ret[A]() -> Option[A] { None }

fn fails_[E](f: fn() -> Int fails E) -> Int fails E { f() }

fn uses_[R](f: fn() -> Int uses { R }) -> Int uses { R } { f() }

fn inside[A, B](f: fn(A) -> B) -> List[fn(A) -> B] { [f] }

fn inferred[A](x: Int) {
  none: Option[A] = None
  none
}

fn bounded[A: Show](x: A) -> String { "#{x}" }

type Tagged[A](id: Int)

enum Phantom[A] {
  Mark
}

trait Describe {
  fn describe(self) -> String
  fn tag[A](self, x: A) -> A { x }
}

type Box[A](value: A)

impl Box[A]: Describe {
  fn describe(self) { "box" }
}

@external(js, "./x.mjs", "f")
fn ext[A](x: A) -> A uses {}

fn main() {
  dbg(param(1))
  o: Option[String] = inferred(1)
  dbg(o)
  t: Tagged[Int] = Tagged(1)
  dbg(t.id)
  ()
}
"##,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    let warnings: Vec<_> = analysis
        .diagnostics
        .iter()
        .filter(|d| d.code == Some("W3002"))
        .collect();
    let rendered = render(&analysis.diagnostics, &analysis.sources, false);
    assert!(warnings.is_empty(), "{rendered}");
    assert!(!analysis.has_errors(), "{rendered}");
}

/// No W3002 when a type in the signature could not be read: the error is reported instead.
#[test]
fn unused_type_parameter_not_reported_for_a_broken_signature() {
    let analysis = check_files(
        &[("home/main.emel", "fn f[A](x: Lst[A]) -> Int { 1 }\n")],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    let codes: Vec<_> = analysis.diagnostics.iter().map(|d| d.code).collect();
    assert!(!codes.contains(&Some("W3002")), "{codes:?}");
    assert!(codes.contains(&Some("E2101")), "{codes:?}");
}

/// `match` binds more loosely than `&&` and `||` (`[fn.operator]`), so `a && k match { ... }`
/// matches on `a && k`. When the right operand is not Bool, one E3001 is reported at it, with a
/// note naming the scrutinee and a help that encloses the intended `match` in parentheses. The arm
/// patterns are checked against the right operand, so they report no further E3001 and no W3101
/// (`[diag.list]`).
#[test]
fn match_on_logical_operator() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"enum Kind { Aa, Bb }

type Item(cap: Option[Int], res: Option[Int])

fn g(a: Bool, k: Kind) -> Bool {
  a && k match {
    Aa -> True
    Bb -> False
  }
}

fn h(item: Item) -> Bool {
  item.cap.is_some() || item.res match {
    Some(n) -> n > 0
    None -> False
  }
}

fn both(a: Bool, b: Bool) -> Bool {
  a && b match {
    True -> False
    False -> True
  }
}

fn main() {}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}
