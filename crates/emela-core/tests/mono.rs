//! Tests for monomorphization (`emela_core::monomorphize`).

mod support;

use emela_core::{Module, Roots, Type, dump, monomorphize, monomorphize_from, verify_monomorphic};
use support::{lower_files, try_lower};

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
  dbg(List.map([1, 2], fn(x) { x + 1 }))
  dbg(List.map([\"a\"], String.length))
  with Fixed {
    dbg(List.map([1], fn(x) { x + (use Clock).now() }))
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

fn modify[S](f: fn(S) -> S fails {} use {}) -> S use State[S] {
  (use State[S]).update(f)
  (use State[S]).get()
}

fn twice[T](f: fn(T) -> T fails {} use {}) -> T use State[T] {
  modify(f)
  modify(f)
}

fn both() -> String use { State[Int], State[String] } {
  n = modify(fn(n) { n + 1 })
  (use State[String]).set(\"n=#{n}\")
  twice(fn(s) { String.append(s, \"!\") })
}

fn main() {
  dbg(State.run(10, fn() { State.run(\"\", fn() { both() }) }))
  g = modify
  dbg(State.run(1, fn() { g(fn(x) { x * 2 }) }))
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

fn modify[S](f: fn(S) -> S fails {} use {}) -> S use State[S] {
  (use State[S]).update(f)
  (use State[S]).get()
}

pub fn keep[A](x: A) -> A {
  x
}

fn main() {
  dbg(State.run(10, fn() { modify(fn(n) { n + 1 }) }))
  dbg(List.map([\"a\"], fn(s) { keep(s) }))
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

fn naps(xs: List[Int]) -> List[Int] use Clock {
  clock = use Clock
  List.map(xs, fn(ms) { clock.sleep(ms) })
}

fn main() {
  with InstantClock { dbg(naps([1, 2])) }
  dbg(List.map([1], fn(x) { x }))
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
import Std.Fiber.{Fiber, Scope, Exit}
import Std.Deferred.{Deferred}

fn main() -> Int use { Async } {
  Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { [1] })
    b = Fiber.fork(fn() { 2 })
    d = s.deferred()
    Deferred.complete(d, \"x\")
    e = Fiber.attempt(fn() { 3 })
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
  fn(x) { g(f(x)) }
}

fn main() {
  h = compose(fn(x) { x + 1 }, fn(y) { y * 2 })
  dbg(h(1))
  with Quiet {
    k = compose(fn(x) { (use Log).info(\"x\")
      x }, fn(y) { y })
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
               Fiber.scope(fn() {\n\
                 h = Fiber.fork(fn() { panic(\"child\") })\n\
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
