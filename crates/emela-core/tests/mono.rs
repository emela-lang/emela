//! Tests for monomorphization (`emela_core::monomorphize`).

mod support;

use emela_core::{
    ErrorDisplay, Expr, Module, Roots, Type, dump, monomorphize, monomorphize_from,
    verify_monomorphic,
};
use support::{Options, lower_files, lower_with, try_lower};

/// Monomorphizes from the exported functions and checks the result with the verifier.
fn mono(m: &Module) -> Module {
    let out = monomorphize(m);
    assert_monomorphic(&out);
    out
}

fn assert_monomorphic(m: &Module) {
    if let Err(errors) = verify_monomorphic(m) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("not monomorphic:\n{}", lines.join("\n"));
    }
}

/// The parts of the dump whose first line contains one of `keys` (functions and effects).
fn parts(m: &Module, keys: &[&str]) -> String {
    let out = dump(m);
    let mine: Vec<&str> = out
        .split("\n\n")
        .filter(|part| {
            let first = part.lines().next().unwrap_or_default();
            keys.iter().any(|k| first.contains(k))
        })
        .collect();
    mine.join("\n\n")
}

/// Pure and effectful lambdas passed to the same generic function get different copies: the pure
/// one is called without the evidence table, and the copy for it takes none.
#[test]
fn list_map_with_pure_and_effectful_lambdas() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Clock {
  fn now() -> Int
}

handler Fixed: Clock {
  fn now() { 42 }
}

fn main() {
  dbg(List.map([1, 2], |x| x + 1))
  dbg(List.map([\"a\"], String.length))
  with Fixed {
    dbg(List.map([1], |x| x + (use Clock).now()))
  }
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn List.map[", "fn List.fold["]));
}

/// A trait function call becomes a call of the impl function for the type, with the impl's type
/// parameters taken from the type (`Box[Point]` gives `A = Point`). A default implementation is
/// copied per type, and calls inside it pick the impl too (spec 10.3).
#[test]
fn trait_calls_pick_the_impl() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Point(x: Int)
type Box[A](value: A)

trait Size {
  fn size(self) -> Int

  fn double(self) -> Int { Size.size(self) * 2 }
}

impl Point: Size {
  fn size(self) { self.x }
}

impl Box[A: Size]: Size {
  fn size(self) { Size.size(self.value) + 1 }
}

fn main() {
  dbg(Size.size(Box(value: Point(x: 1))))
  dbg(Size.double(Point(x: 2)))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "Size."]));
}

/// A derived impl is generated per type (spec 10.5). Eq of a type that reaches no written impl
/// compares the structure as a whole; Show of a type with a field that has a written impl goes
/// field by field; Ord compares the constructors' indices, then the fields.
#[test]
fn derived_impls_are_generated_per_type() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

type User(id: Int, name: Name, nick: String) derive Show

type Point(x: Int, y: Int) derive Eq, Ord

enum Shape {
  Dot
  Line(Point, Point)
}
  derive Eq, Ord

fn main() {
  dbg(Show.show(User(id: 1, name: Name(value: \"a\"), nick: \"b\")))
  dbg(Eq.eq(Point(x: 1, y: 2), Point(x: 1, y: 2)))
  dbg(Ord.compare(Dot, Line(Point(x: 1, y: 2), Point(x: 3, y: 4))))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", ": Eq.", ": Ord.", ": Show."]));
}

/// `dbg` of a type without Show whose parts reach a written Show impl (spec 10.6): one function
/// per enum and list type writes the value as constructed, with `Name` through its Show, a String
/// quoted, an Int as the backend shows it, tuples in place, and an array of elements without Show
/// as `Array.from_list([...])`.
#[test]
fn debug_display_of_types_without_show() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.Array.{Array}

type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

type Inner(name: Name)

enum Plain {
  Empty
  Two(Name, Inner)
  Many(parts: (Name, String, Int, Inner), names: List[Name], more: List[Inner], inners: Array[Inner])
}

fn main() {
  dbg(Empty)
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", ": debug"]));
}

/// Derived Eq, Ord and Show of an enum with a field that has a written impl go field by field
/// (spec 10.5): `()` is always equal, basic types use their operators (`False < True`), tuples
/// go part by part, `Name` uses its impls, and a field that reaches no written impl (`List[Int]`)
/// is compared as a whole. A type that reaches no written impl compares and
/// shows as a whole.
#[test]
fn derived_impls_field_by_field() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

impl Name: Eq {
  fn eq(self, other: Name) { True }
}

impl Name: Ord {
  fn compare(self, other: Name) { Equal }
}

enum Mixed {
  Empty
  Full(unit: (), big: Int64, flag: Bool, text: String, pair: (Name, Int, Name), nums: List[Int])
}
  derive Eq, Ord, Show

type Ratio(ratio: Float, name: Name) derive Eq

type Plain(n: Int, s: String) derive Eq, Show

fn main() {
  dbg(Eq.eq(Empty, Empty))
  dbg(Ord.compare(Empty, Empty))
  dbg(Show.show(Empty))
  dbg(Eq.eq(Plain(n: 1, s: \"a\"), Plain(n: 1, s: \"a\")))
  dbg(Eq.eq(Ratio(ratio: 1.0, name: Name(value: \"a\")), Ratio(ratio: 1.0, name: Name(value: \"a\"))))
  dbg(Show.show(Plain(n: 1, s: \"a\")))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["Mixed: ", "Plain: ", "Ratio: "]));
}

/// The compiler's impls of Eq, Ord and Show for primitive types, `()` and tuples (spec 10.6).
/// Their names stay apart from those of a user type named `Int` in the entry, whose names are not
/// qualified.
#[test]
fn built_in_impls_of_primitives_and_tuples() {
    let m = mono(&lower_files(&[
        (
            "main.emel",
            "\
import Units

type Int(n: Float) derive Eq

impl Int: Show {
  fn show(self) { \"mine\" }
}

fn main() {
  dbg(Eq.eq(1, 2))
  dbg(Eq.eq(Int(n: 1.5), Int(n: 1.5)))
  dbg(Ord.compare(1, 2))
  dbg(Ord.compare(True, False))
  dbg(Ord.compare((), ()))
  dbg(Show.show(\"s\"))
  dbg(Show.show((1, \"a\", True)))
  dbg(Show.show(Int(n: 1.5)))
  dbg(Show.show(Units.Int(n: 1.5)))
}
",
        ),
        (
            "units.emel",
            "\
pub type Int(n: Float)

impl Int: Show {
  fn show(self) { \"units\" }
}
",
        ),
    ]));
    insta::assert_snapshot!(parts(
        &m,
        &[
            "Int: ",
            "Bool: Ord",
            "(): Ord",
            "String: Show",
            "(A, B, C): Show",
        ]
    ));
}

/// How a failure reaching the entry shows each error it can fail with (spec 13.3): through a
/// written Show impl, by the structure for a derived Show, and by name (not listed) without Show.
/// Monomorphizing again keeps what the first pass decided.
#[test]
fn errors_reaching_the_entry_are_displayed() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
error Bare

error Shown(code: Int) derive Show

error Named(code: Int)

impl Named: Show {
  fn show(self) { \"named\" }
}

fn pick(n: Int) -> Int fails Bare | Shown | Named {
  if n == 0 { fail Bare } else if n == 1 { fail Shown(code: 1) } else { fail Named(code: 2) }
}

fn main() -> Int fails Bare | Shown | Named { pick(1) }
",
    )]));
    let displays = |m: &Module| {
        let mut out: Vec<String> = m
            .error_display
            .iter()
            .map(|(&id, d)| match d {
                ErrorDisplay::Impl(f) => format!("{}: {}", m.enums[id].name, m.functions[*f].name),
                ErrorDisplay::Structure => format!("{}: structure", m.enums[id].name),
            })
            .collect();
        out.sort();
        out
    };
    assert_eq!(
        displays(&m),
        ["Named: Named: Show.show", "Shown: structure"]
    );
    assert_eq!(displays(&mono(&m)), displays(&m));
}

/// A comparison assert shows its sides when their type satisfies Show (spec 13.2): through the
/// written impl, bound first; by the backend when Show is derived, after comparing through Eq's
/// impl if it is written; not at all without Show.
#[test]
fn asserts_show_their_sides() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

impl Name: Eq {
  fn eq(self, other: Name) { self.value == other.value }
}

impl Name: Ord {
  fn compare(self, other: Name) { Equal }
}

type Point(x: Int) derive Eq

type Tagged(x: Int) derive Eq, Show

type Code(x: Int) derive Show

impl Code: Eq {
  fn eq(self, other: Code) { True }
}

fn main() {
  n = Name(value: \"a\")
  assert n == n
  assert n != n
  assert n < n
  assert n <= n
  assert n > n
  assert n >= n
  assert Point(x: 1) == Point(x: 1)
  assert Tagged(x: 1) == Tagged(x: 1)
  assert Code(x: 1) == Code(x: 2)
  assert 1 < 2
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main("]));
}

/// `inspect` and `dbg` of a type with a written Show go through it: `inspect` returns the text,
/// `dbg` prints it and returns the value. For a type that reaches no written Show, both stay
/// builtins, shown by the backend (spec 10.6).
#[test]
fn inspect_and_dbg_through_show() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

fn main() {
  n = Name(value: \"a\")
  a = inspect(n)
  b = dbg(n)
  c = inspect(1)
  dbg(2)
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main("]), @r#"
    export fn main() -> Int {
      let n%0: Name = Name.Name(value: "a")
      let a%1: String = Name: Show.show(n%0)
      let b%3: Name = {
        let value%4: Name = n%0
        let shown%5: () = @$Debug.print(Name: Show.show(value%4))
        value%4
      }
      let c%6: String = @inspect[Int](1)
      @dbg[Int](2)
    }
    "#);
}

/// A value whose type argument is a function is shown through a written Show impl that does not
/// need the argument's Show (spec 10.6). Lowering used to reject it ("showing a function").
#[test]
fn written_show_of_a_type_holding_a_function() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Box[A](value: A)

impl Box[A]: Show {
  fn show(self) { \"box\" }
}

fn twice(n: Int) -> Int { n * 2 }

fn main() -> String {
  b = Box(value: twice)
  \"#{b}\"
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main("]), @r"
    export fn main() -> String {
      let b%0: Box[fn(Int) -> Int] = Box.Box[fn(Int) -> Int](value: twice)
      concat(Box: Show.show[fn(Int) -> Int](b%0) as String)
    }
    ");
}

/// An assert without source text (hand-built IR) names the comparison in its message, and
/// without a location fails as a panic (spec 13.2).
#[test]
fn asserts_without_source_text() {
    let mut m = lower_files(&[(
        "main.emel",
        "\
type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

impl Name: Eq {
  fn eq(self, other: Name) { True }
}

impl Name: Ord {
  fn compare(self, other: Name) { Equal }
}

fn main() {
  n = Name(value: \"a\")
  assert n == n
  assert n != n
  assert n < n
  assert n <= n
  assert n > n
  assert n >= n
}
",
    )]);
    fn strip(e: &mut Expr) {
        if let Expr::Assert(a) = e {
            a.text.clear();
            a.location = None;
        }
        emela_core::tail::for_each_child_mut(e, &mut strip);
    }
    for (_, f) in m.functions.iter_mut() {
        strip(&mut f.body);
    }
    let out = parts(&mono(&m), &["fn main("]);
    let heads: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split("assertion failed: ").nth(1))
        .map(|rest| rest.split("\\n").next().unwrap_or_default())
        .collect();
    assert_eq!(
        heads,
        [
            "left == right",
            "left != right",
            "left < right",
            "left <= right",
            "left > right",
            "left >= right",
        ]
    );
    assert_eq!(out.matches("@panic(").count(), 6, "{out}");
}

/// A function value that does not take the evidence table, flowing into a slot whose type takes
/// one, gets an adapter that drops it: a fn, a local, a lambda and the branches of `if` and
/// `match`, and a function parameter of a function value (spec 16.5).
#[test]
fn adapters_for_function_values() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

fn pure() -> Int { 1 }

fn logs() -> Int uses { Log } {
  (use Log).info(\"x\")
  2
}

fn call_pure(f: fn() -> Int uses {}) -> Int uses { Log } { f() }

fn call_any(f: fn() -> Int uses { Log }) -> Int uses { Log } { f() }

fn main() -> Int {
  with Quiet {
    p = pure
    fs: List[fn() -> Int uses { Log }] = [pure, p, || 3, logs]
    g: fn() -> Int uses { Log } = if List.is_empty(fs) { pure } else { logs }
    h: fn() -> Int uses { Log } = 1 match {
      0 -> pure
      _ -> logs
    }
    k: fn(fn() -> Int uses {}) -> Int uses { Log } = call_pure
    m: fn(fn() -> Int uses {}) -> Int uses { Log } = call_any
    g() + h() + k(pure) + m(pure)
  }
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main("]));
}

/// Whether `dbg` goes through Show (spec 10.6): core's impls for `List` and `Option` give what the
/// structure gives, so a type holding only those is shown by the backend, while Duration's Show
/// in std is a written one. An impl whose type parameter needs Eq or Ord applies when the argument
/// has the compiler's impl (Int, Float and tuples for Eq), and not when it has none (Float is not
/// Ord).
#[test]
fn which_values_dbg_shows_through_show() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.Duration.{Duration}

type Bag(items: List[Int], maybe: Option[Int])

type Timed(d: Duration)

type ByEq[A](value: A)

impl ByEq[A: Eq]: Show {
  fn show(self) { \"eq\" }
}

type ByOrd[A](value: A)

impl ByOrd[A: Ord]: Show {
  fn show(self) { \"ord\" }
}

fn main() {
  dbg(Bag(items: [1], maybe: None))
  dbg(Timed(d: Duration.milliseconds(Int64.from_int(1))))
  dbg(ByEq(value: 1))
  dbg(ByEq(value: 1.5))
  dbg(ByEq(value: (1, \"a\")))
  dbg(ByOrd(value: 1))
  dbg(ByOrd(value: 1.5))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", ": debug"]));
}

/// A derived Eq of a type whose fields reach no written impl, through lists and tuples, compares
/// the whole value structurally; one with an array field goes field by field, through Array's
/// written Eq (spec 10.5).
#[test]
fn derived_eq_through_lists_tuples_and_arrays() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.Array.{Array}

type Ints(xs: List[Int], t: (Int, String)) derive Eq

type Arr(a: Array[Int]) derive Eq

fn main() {
  dbg(Eq.eq(Ints(xs: [1], t: (1, \"a\")), Ints(xs: [1], t: (1, \"a\"))))
  dbg(Eq.eq(Arr(a: Array.from_list([1])), Arr(a: Array.from_list([1]))))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["Ints: Eq", "Arr: Eq"]), @r"
    fn Ints: Eq.eq(self%1: Ints, other%2: Ints) -> Bool {
      (self%1 ==[Structural] other%2)
    }

    fn Arr: Eq.eq(self%3: Arr, other%4: Arr) -> Bool {
      match self%3 {
        Arr.Arr(a: x%5) -> match other%4 {
          Arr.Arr(a: y%6) -> Std.Array.Array: Eq.eq[Int](x%5, y%6)
        }
      }
    }
    ");
}

/// A const whose value is a function is one value, also when its type is generalized over the
/// function's R (spec 5.6): it takes no parameters and returns the function. Lowering used to read
/// its type as the signature of a function.
#[test]
fn consts_holding_functions() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

const DOUBLE: fn(Int) -> Int = |x| x * 2

const APPLY = |callback| callback()

fn main() -> Int {
  with Quiet {
    DOUBLE(2) + APPLY(|| 1) + APPLY(|| {
      (use Log).info(\"x\")
      2
    })
  }
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["DOUBLE", "APPLY", "fn main("]));
}

/// An effect with type arguments named only inside a generic function (`State[S]` of the
/// `State.run` in `wrap`) becomes one effect per instantiation, with its operations' types
/// instantiated too.
#[test]
fn effects_instantiated_inside_generic_functions() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}

fn wrap[S](init: S) -> S {
  (r, _) = State.run(init, || (use State[S]).get())
  r
}

fn main() -> String {
  n = wrap(1)
  wrap(\"a\")
}
",
    )]));
    let out = dump(&m);
    let effects: Vec<&str> = out
        .split("\n\n")
        .filter(|p| p.starts_with("effect Std.State.State["))
        .collect();
    insta::assert_snapshot!(effects.join("\n\n"), @r"
    effect Std.State.State[Int] = Std.State.State[Int] {
      fn get() -> Int
      fn set(Int) -> ()
      fn update(fn(Int) -> Int) -> ()
    }

    effect Std.State.State[String] = Std.State.State[String] {
      fn get() -> String
      fn set(String) -> ()
      fn update(fn(String) -> String) -> ()
    }
    ");
}

/// An exported generic function is instantiated with `Any` for its type parameters and, for a set
/// type parameter found anywhere in its signature (in a function type, a list, a tuple or an
/// enum), a set that fails and needs the evidence table.
#[test]
fn generic_exports_with_set_parameters() {
    let options = Options {
        roots: Roots::Public,
        ..Options::default()
    };
    let lowered = lower_with(
        &[(
            "main.emel",
            "\
pub fn direct[R](f: fn() -> Int uses R) -> Int uses R { f() }

pub fn listed[R](fs: List[fn() -> Int uses R]) -> Int { List.length(fs) }

pub fn paired[R](p: (fn() -> Int uses R, Int)) -> Int { 0 }

pub fn maybe[R](o: Option[fn() -> Int uses R]) -> Int { 0 }

fn main() { 0 }
",
        )],
        &options,
    )
    .unwrap_or_else(|errors| panic!("{errors:#?}"));
    let m = mono(&lowered.module);
    let heads: Vec<String> = ["direct", "listed", "paired", "maybe"]
        .iter()
        .map(|name| {
            let part = parts(&m, &[&format!("export fn {name}(")]);
            part.lines().next().unwrap_or_default().to_owned()
        })
        .collect();
    insta::assert_snapshot!(heads.join("\n"), @r"
    export fn direct(f%0: fn() -> Int fails ev, ev%1: evidence) -> Int fails {
    export fn listed(fs%2: List[fn() -> Int fails ev]) -> Int fails {
    export fn paired(p%3: (fn() -> Int fails ev, Int)) -> Int fails {
    export fn maybe(o%4: Option[fn() -> Int fails ev]) -> Int fails {
    ");
}

/// Inside a copy, a call of a function value is marked by its instantiated type: one that cannot
/// fail is not checked for failure. A lambda returning a function value adapts it to its return
/// type.
#[test]
fn calls_and_returns_follow_instantiated_types() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

fn pure() -> Int { 1 }

fn apply[E](f: fn() -> Int fails E uses {}) -> Int fails E uses {} { f() }

fn main() -> Int {
  r: fn() -> fn() -> Int uses { Log } = || pure
  with Quiet { apply(|| 1) + r()() }
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn apply["]));
}

/// Option, nested generic types and a generic function calling another with its own type
/// parameters.
#[test]
fn option_and_nested_generics() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
type Pair[A, B](first: A, second: B)

fn swap[A, B](p: Pair[A, B]) -> Pair[B, A] {
  Pair(first: p.second, second: p.first)
}

fn wrap[T](x: T) -> Option[Pair[T, List[T]]] {
  Some(swap(Pair(first: [x], second: x)))
}

fn main() {
  dbg(wrap(1))
  dbg(wrap(\"s\"))
  dbg(swap(Pair(first: Some(1.5), second: [None, Some(True)])))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn swap[", "fn wrap["]));
}

/// A generic function with State[S] in its R: the evidence table key `State[$modify.0]` becomes
/// the caller's `State[Int]` and `State[String]`, and State.run's handler is built per state type.
#[test]
fn state_run_and_effects_with_type_arguments() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}

fn modify[S](f: fn(S) -> S fails {} uses {}) -> S uses { State[S] } {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn twice[T](f: fn(T) -> T fails {} uses {}) -> T uses { State[T] } {
  modify(f)
  modify(f)
}

fn both() -> String uses { State[Int], State[String] } {
  n = modify(|n| n + 1)
  (use State[String]).set(\"n=#{n}\")
  twice(|s| String.append(s, \"!\"))
}

fn main() {
  dbg(State.run(10, || State.run(\"\", || both())))
  g = modify
  dbg(State.run(1, || g(|x| x * 2)))
}
",
    )]));
    insta::assert_snapshot!(parts(
        &m,
        &["fn main(", "fn modify[", "fn twice[", "fn both(", "effect "]
    ));
}

/// An exported fn with a type parameter that its boundary types do not use is instantiated at
/// `Any` under its own name.
#[test]
fn generic_exports_are_instantiated_at_any() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
@export(js, \"count\")
pub fn count[A](n: Int) -> Int {
  xs: List[A] = []
  n + List.length(xs)
}

fn main() {
  dbg(count(1))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn count"]));
}

/// Build, run and test hand monomorphized IR to the C and WASM pipelines, which monomorphize it
/// again; that changes nothing.
#[test]
fn monomorphizing_twice_changes_nothing() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.State.{State}

fn modify[S](f: fn(S) -> S fails {} uses {}) -> S uses { State[S] } {
  (use State[S]).update(f)
  (use State[S]).get()
}

pub fn keep[A](x: A) -> A {
  x
}

fn main() {
  dbg(State.run(10, || modify(|n| n + 1)))
  dbg(List.map([\"a\"], |s| keep(s)))
}
",
    )]));
    assert_eq!(dump(&mono(&m)), dump(&m));
}

/// Suspending copies (`List.map$suspend8`) are copied per instantiation too; the suspend bit of
/// the summary agrees with the copy.
#[test]
fn suspending_copies() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn naps(xs: List[Int]) -> List[Int] uses { Clock } {
  clock = use Clock
  List.map(xs, |ms| clock.sleep(ms))
}

fn main() {
  with InstantClock { dbg(naps([1, 2])) }
  dbg(List.map([1], |x| x))
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn naps(", "fn List.map"]));
}

/// Fibers: Scope's generic operations keep their own type parameters, and fork and attempt get
/// concrete types.
#[test]
fn fibers() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
import Std.Fiber.{Fiber, Scope}
import Std.Exit.{Exit}
import Std.Deferred.{Deferred}

fn main() -> Int uses { Async } {
  Fiber.scope(|| {
    s = use Scope
    a = s.fork(|| [1])
    b = Fiber.fork(|| 2)
    d = s.deferred()
    Deferred.complete(d, \"x\")
    e = Exit.attempt(|| 3)
    List.length(Fiber.join(a)) + Fiber.join(b)
  })
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn Std.Fiber.fork[", "effect "]));
}

/// A generic function returning a closure over its argument function: inside the copy for a pure
/// argument, the closure takes no evidence table.
#[test]
fn closures_from_generic_functions() {
    let m = mono(&lower_files(&[(
        "main.emel",
        "\
effect Log {
  fn info(msg: String) -> ()
}

handler Quiet: Log {
  fn info(msg) { () }
}

fn compose[A, B, C](f: fn(A) -> B, g: fn(B) -> C) -> fn(A) -> C {
  |x| g(f(x))
}

fn main() {
  h = compose(|x| x + 1, |y| y * 2)
  dbg(h(1))
  with Quiet {
    k = compose(|x| { (use Log).info(\"x\")
      x }, |y| y)
    dbg(k(2))
  }
}
",
    )]));
    insta::assert_snapshot!(parts(&m, &["fn main(", "fn compose["]));
}

/// Roots that take no arguments and whose inferred types have type parameters are instantiated,
/// not skipped: `Never` for an ordinary type parameter (a test that joins a child that only
/// panics). A generic root that takes arguments (`List.map`) has no instantiation.
#[test]
fn generic_roots_are_instantiated() {
    let m = try_lower(
        &[(
            "main.emel",
            "import Std.Fiber.{Fiber, Scope}\n\
             fn child_defect() {\n\
               Fiber.scope(|| {\n\
                 h = Fiber.fork(|| panic(\"child\"))\n\
                 Fiber.join(h)\n\
               })\n\
             }\n\
             fn main() { 0 }\n",
        )],
        Roots::All,
    )
    .unwrap_or_else(|errors| panic!("{errors:#?}"));
    let find = |name: &str| {
        m.functions
            .iter()
            .find(|(_, f)| f.name.ends_with(name))
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("{name} is lowered"))
    };
    let roots = [find("child_defect"), find("List.map")];
    for &r in &roots {
        let f = &m.functions[r];
        assert!(f.type_params > f.set_params.len(), "{} is generic", f.name);
    }
    let (out, copies) = monomorphize_from(&m, &roots);
    assert_monomorphic(&out);
    assert!(copies[1].is_none(), "List.map takes arguments");
    let child_defect = &out.functions[copies[0].unwrap()];
    assert!(child_defect.params.is_empty() && !child_defect.fails);
    assert_eq!(out.types[child_defect.ret], Type::Never);
}

/// Every function of the bundled core and std, as roots, monomorphizes to a module the verifier
/// accepts.
#[test]
fn every_core_function_monomorphizes() {
    let m = try_lower(&[("main.emel", "fn main() { 0 }\n")], Roots::All)
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
    let roots: Vec<_> = m.functions.iter().map(|(id, _)| id).collect();
    let (out, new_roots) = monomorphize_from(&m, &roots);
    assert_monomorphic(&out);
    // Generic roots that take arguments have no instantiation; the others (generic or not) get
    // one each.
    for (&old, new) in roots.iter().zip(&new_roots) {
        let f = &m.functions[old];
        let inputs = f.params.len() - usize::from(m.takes_ev(old));
        assert_eq!(new.is_none(), f.type_params > 0 && inputs > 0, "{}", f.name);
    }
    let distinct: std::collections::HashSet<_> = new_roots.iter().flatten().collect();
    assert_eq!(distinct.len(), new_roots.iter().flatten().count());
}

/// Prints function counts and times for lowering and monomorphizing the program at
/// `EMELA_MONO_BENCH` (a path to an `.emel` file). Not a timing test: run it by hand with
/// `cargo test --release -p emela-core --test mono measure -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run by hand"]
fn measure() {
    let Some(path) = std::env::var_os("EMELA_MONO_BENCH") else {
        return;
    };
    let src = std::fs::read_to_string(&path).expect("EMELA_MONO_BENCH names a readable file");
    for roots in [Roots::Exported, Roots::All] {
        let start = std::time::Instant::now();
        let m = try_lower(&[("main.emel", &src)], roots).unwrap_or_else(|e| panic!("{e:?}"));
        let lowered = start.elapsed();
        let fn_roots: Vec<_> = match roots {
            Roots::All => m.functions.iter().map(|(id, _)| id).collect(),
            _ => m
                .functions
                .iter()
                .filter(|(_, f)| f.exported)
                .map(|(id, _)| id)
                .collect(),
        };
        let start = std::time::Instant::now();
        let (out, _) = monomorphize_from(&m, &fn_roots);
        let monomorphized = start.elapsed();
        let start = std::time::Instant::now();
        assert_monomorphic(&out);
        let verified = start.elapsed();
        let generic = m
            .functions
            .iter()
            .filter(|(_, f)| f.type_params > 0)
            .count();
        println!(
            "{roots:?}: functions {} before ({generic} generic), {} after; locals {} before, {} \
             after; types {} before, {} after",
            m.functions.len(),
            out.functions.len(),
            m.locals.len(),
            out.locals.len(),
            m.types.len(),
            out.types.len(),
        );
        println!(
            "{roots:?}: check + lower + verify {lowered:?}, monomorphize {monomorphized:?}, \
             verify_monomorphic {verified:?}"
        );
    }
}
