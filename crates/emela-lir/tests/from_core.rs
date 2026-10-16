//! Tests for the conversion from monomorphized Core IR to the Flow IR (`emela_lir::from_core`):
//! programs are lowered, monomorphized and converted, and the result must pass the Flow IR
//! verifier.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::{Module, dump_function, from_core, verify};
use support::try_lower;

/// Lowers (from `roots`), monomorphizes, converts and verifies.
fn convert(files: &[(&str, &str)], roots: Roots) -> Module {
    let m = try_lower(files, roots).unwrap_or_else(|e| panic!("{e:#?}"));
    let fn_roots: Vec<_> = match roots {
        Roots::All => m.functions.iter().map(|(id, _)| id).collect(),
        _ => m
            .functions
            .iter()
            .filter(|(_, f)| f.exported)
            .map(|(id, _)| id)
            .collect(),
    };
    let (mono, _) = monomorphize_from(&m, &fn_roots);
    if let Err(errors) = verify_monomorphic(&mono) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("not monomorphic:\n{}", lines.join("\n"));
    }
    let lir = from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    });
    if let Err(errors) = verify(&lir) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("Flow IR verifier errors:\n{}", lines.join("\n"));
    }
    lir
}

/// Converts one file from its exported functions.
fn program(src: &str) -> Module {
    convert(&[("main.emel", src)], Roots::Exported)
}

/// The dumps of the functions whose names start with one of `prefixes`, in module order.
fn parts(m: &Module, prefixes: &[&str]) -> String {
    m.functions
        .iter()
        .filter(|(_, f)| prefixes.iter().any(|p| f.name.starts_with(p)))
        .map(|(_, f)| dump_function(m, f))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every function of the bundled core and std converts to Flow IR the verifier accepts.
#[test]
fn every_core_function_converts() {
    let m = convert(&[("main.emel", "fn main() { 0 }\n")], Roots::All);
    assert!(m.functions.len() > 100);
    if let Some(path) = std::env::var_os("EMELA_FLOW_DUMP") {
        std::fs::write(path, emela_lir::dump(&m)).unwrap();
    }
}

/// Every function of each example project (`examples/*/src`) converts.
#[test]
fn every_example_converts() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut projects: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().path().join("src"))
        .filter(|p| p.join("main.emel").exists())
        .collect();
    projects.sort();
    assert!(!projects.is_empty());
    for src in projects {
        let mut files: Vec<(String, String)> = std::fs::read_dir(&src)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "emel"))
            .map(|p| {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read_to_string(&p).unwrap())
            })
            .collect();
        // The entry first.
        files.sort_by_key(|(name, _)| (name != "main.emel", name.clone()));
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(n, s)| (n.as_str(), s.as_str()))
            .collect();
        let m = convert(&files, Roots::All);
        if let Some(dir) = std::env::var_os("EMELA_FLOW_DUMP_DIR") {
            let name = src.parent().unwrap().file_name().unwrap();
            let path = std::path::Path::new(&dir).join(name).with_extension("flow");
            std::fs::write(path, emela_lir::dump(&m)).unwrap();
        }
    }
}

#[test]
fn arithmetic_and_lets() {
    let m = program(
        "\
fn calc(a: Int, b: Int) -> Int {
  x = a + b * 2
  y = x / 3
  if y > 10 && a != b { y - 1 } else { -y }
}

fn floats(x: Float, n: Int64) -> Float {
  m = n * 2
  x * 1.5 - Float.from_int(Int64.to_int(m))
}

fn main() {
  dbg(calc(1, 2))
  dbg(floats(1.0, 3))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["calc", "floats"]));
}

#[test]
fn match_with_nested_patterns_and_guards() {
    let m = program(
        "\
enum Shape {
  Circle(radius: Float),
  Rect(Float, Float),
  Group(List[Shape]),
}

fn describe(s: Shape, n: Int) -> String {
  (s, n) match {
    (Circle(radius:), _) if radius > 10.0 -> \"big circle\"
    (Circle(_), 0) -> \"circle zero\"
    (Group([Rect(w, _), ..rest]), k) if k > 1 -> \"group of rect\"
    (Group([]), _) -> \"empty group\"
    (_, 1) -> \"one\"
    _ -> \"other\"
  }
}

fn word(s: String) -> Int {
  s match {
    \"a\" -> 1
    \"b\" -> 2
    _ -> 0
  }
}

fn units(xs: List[()]) -> Int {
  xs match {
    [(), ..rest] -> List.length(rest)
    [] -> 0
  }
}

fn main() {
  dbg(describe(Circle(radius: 1.0), 0))
  dbg(word(\"a\"))
  dbg(units([(), ()]))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["describe", "word", "units"]));
}

#[test]
fn escape_and_fail() {
    let m = program(
        "\
error NotFound(id: Int)
error Invalid(reason: String)

fn find(id: Int) -> String fails NotFound | Invalid {
  if id < 0 { fail Invalid(reason: \"negative\") }
  if id == 0 { fail NotFound(id:) }
  \"user\"
}

fn lookup(id: Int) -> String fails Invalid {
  find(id) escape {
    NotFound(id: 7) -> \"seven\"
    NotFound(_) -> \"guest\"
  }
}

fn main() {
  dbg(lookup(1)) escape {
    Invalid(reason:) -> reason
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["find", "lookup", "main"]));
}

#[test]
fn with_and_release() {
    let m = program(
        "\
effect Pool {
  fn get() -> Int
}

handler Counted(n: Int) implements Pool {
  init { Counted(n: 1) }
  release { dbg(self.n) }
  fn get() { self.n }
}

fn work() -> Int use Pool {
  (use Pool).get() + 1
}

fn main() {
  with Counted {
    dbg(work())
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "work", "Counted"]));
}

#[test]
fn effects_and_handlers() {
    let m = program(
        "\
effect Log {
  fn info(msg: String) -> ()
  fn count() -> Int
}

handler Quiet implements Log {
  fn info(msg) { () }
  fn count() { 0 }
}

fn report(x: Int) -> Int use Log {
  log = use Log
  log.info(\"x\")
  x + log.count()
}

fn main() {
  with Quiet {
    dbg(report(1))
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "report", "Quiet"]));
}

#[test]
fn suspend_marks() {
    let m = program(
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock implements Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int) -> Int use Clock {
  (use Clock).sleep(ms) + 1
}

fn main() {
  with InstantClock { dbg(nap(1)) }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "nap", "InstantClock"]));
}

#[test]
fn fibers() {
    let m = program(
        "\
import Std.Fiber.{Fiber, Scope, Exit}
import Std.Deferred.{Deferred}

fn main() -> Int use { Async } {
  Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { [1] })
    d = s.deferred()
    Deferred.complete(d, ())
    e = Fiber.attempt(fn() { 3 })
    List.length(Fiber.join(a))
  })
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main"]));
}

/// fork's type parameter is boxed (`any`): a child whose value is not a reference is wrapped in a
/// closure that boxes it. A release that may suspend is held (spec 11.4).
#[test]
fn fork_boxes_and_suspending_releases_hold() {
    let m = program(
        "\
import Std.Fiber.{Fiber, Scope}

effect Wait {
  suspend fn wait() -> ()
}

handler Later implements Wait {
  init { Later }
  release { (use Wait).wait() }
  suspend fn wait(resume) { resume(()) }
}

fn main() -> Int use { Async, Wait } {
  with Later { () }
  Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { 1 })
    Fiber.join(a)
  })
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main"]));
}

#[test]
fn closures_with_captures() {
    let m = program(
        "\
fn adder(base: Int) -> fn(Int) -> (fn(Int) -> Int) {
  fn(x) {
    y = x * 2
    fn(z) { base + y + z }
  }
}

fn main() {
  f = adder(1)
  g = f(2)
  dbg(g(3))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["adder", "main"]));
}

#[test]
fn interpolation_with_nested_show() {
    let m = program(
        "\
enum Shape {
  Circle(radius: Float),
  Named(String, List[Option[Int]]),
  Empty,
}

fn main() {
  s = Named(\"a\\\"b\", [Some(1), None])
  t = (True, 2.5, \"x\")
  dbg(\"shape #{s} and #{t} and #{[(), ()]}: #{\"top\"} #{3}\")
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "show$"]));
}

#[test]
fn structural_equality() {
    let m = program(
        "\
type Point(x: Int, y: Int)

enum Tree {
  Leaf,
  Node(Tree, Point, Tree),
}

fn same(a: Tree, b: Tree) -> Bool {
  a == b
}

fn main() {
  t = Node(Leaf, Point(x: 1, y: 2), Leaf)
  dbg(same(t, t))
  dbg([(1, \"a\")] != [(1, \"b\")])
  assert Point(x: 1, y: 2) == Point(x: 1, y: 3)
}
",
    );
    insta::assert_snapshot!(parts(&m, &["same", "main", "eq$", "show$"]));
}
