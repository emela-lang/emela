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
    // The check is the verifier inside `convert`; the count only guards against converting
    // nothing (core and std have hundreds of functions).
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
    // Guards against finding no project (a moved `examples`), which would check nothing.
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

/// Escape arms: an arm that takes every value of its error (no guard, irrefutable fields: a
/// binding, `()`, a tuple or a single constructor of such) ends the tries for that error, so a
/// later arm for it is not tried; `_` takes the rest (`[error.escape]`).
#[test]
fn escape_arms_that_take_every_value() {
    let m = program(
        "\
type Wrapper(n: Int)

error Bad(code: Int, pair: (Int, ()), rest: List[Int], w: Wrapper)

error Other

fn risky(n: Int) -> Int fails Bad | Other {
  if n == 0 { fail Bad(code: 1, pair: (1, ()), rest: [], w: Wrapper(n: 1)) }
  fail Other
}

fn main() -> Int {
  risky(1) escape {
    Bad(code: 1, ..) -> 1
    Bad(code: c, ..) if c > 5 -> 2
    Bad(w: Wrapper(n: 2), ..) -> 6
    Bad(code: c, pair: (_, ()), rest: _, w: Wrapper(n: _)) -> 3
    Bad(code: _, ..) -> 4
    _ -> 5
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main"]));
}

/// An assert without source text (hand-built IR) names the comparison in its message (spec
/// 13.2).
#[test]
fn asserts_without_source_text() {
    let mut m = try_lower(
        &[(
            "main.emel",
            "\
fn main() {
  a = 1
  assert a == 2
  assert a != 1
  assert a < 0
  assert a <= 0
  assert a > 2
  assert a >= 2
  assert a == 1 && True
}
",
        )],
        Roots::Exported,
    )
    .unwrap_or_else(|e| panic!("{e:#?}"));
    fn strip(e: &mut emela_core::Expr) {
        if let emela_core::Expr::Assert(a) = e {
            a.text.clear();
        }
        emela_core::tail::for_each_child_mut(e, &mut strip);
    }
    for (_, f) in m.functions.iter_mut() {
        strip(&mut f.body);
    }
    let roots: Vec<_> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect();
    let (mono, _) = monomorphize_from(&m, &roots);
    let lir = from_core(mono).unwrap_or_else(|e| panic!("{e:?}"));
    let out = parts(&lir, &["main"]);
    let heads: Vec<&str> = out
        .lines()
        .filter_map(|l| l.split("assertion failed").nth(1))
        .map(|rest| rest.split("\\n").next().unwrap_or_default())
        .collect();
    assert_eq!(
        heads,
        [
            ": left == right",
            ": left != right",
            ": left < right",
            ": left <= right",
            ": left > right",
            ": left >= right",
            // A plain condition says only that it failed.
            "\") at \"src/main.emel:9:3\"",
        ]
    );
}

/// Builtins the conversion handles itself: `dbg` through a written Show (a debug print, with a
/// String part quoted), `inspect` and the show of a String, `todo` and a failed assert (defects,
/// the assert's with its location), and the cells of `State.run` (their `()` contents keep a
/// slot).
#[test]
fn builtins_and_cells() {
    let m = program(
        "\
import Std.State.{State}

type Name(value: String)

impl Name: Show {
  fn show(self) { self.value }
}

impl Name: Eq {
  fn eq(self, other: Name) { True }
}

type Tagged(name: Name, label: String)

fn main() -> Int {
  n = Name(value: \"a\")
  dbg(n)
  dbg(Tagged(name: n, label: \"l\"))
  s = inspect(5)
  t = \"#{\"\"}#{n}\"
  assert n == n
  (r, _) = State.run(1, || (use State[Int]).get())
  (u, _) = State.run((), || (use State[()]).get())
  if r > 5 { todo() }
  r
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "Tagged: debug"]));
}

/// The structure's Eq and Show that the conversion makes (spec 10.6): fields compared by their
/// operators (`Int64`, `Float`, an enum of constructors without fields by its tag), a function
/// shown as `<fn>`, a String quoted, and an external type, whose values the program cannot look
/// into, shown by a defect.
#[test]
fn structural_eq_and_show_of_fields() {
    let m = program(
        "\
enum Color {
  Red
  Green
}
  derive Eq

@external
type Conn

@external(js, \"conn\")
fn conn() -> Conn uses {}

type Rec(big: Int64, ratio: Float, color: Color) derive Eq

type Shown(f: fn() -> Int, text: String, c: Conn)

fn main() -> Bool {
  a = Rec(big: Int64.from_int(1), ratio: 1.0, color: Red)
  dbg(Shown(f: || 1, text: \"t\", c: conn()))
  a == a
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "eq$", "show$"]));
}

/// The function that shows each error a failure reaching the entry may carry (`error_shows`,
/// `[dyn.error]`): its Show impl, the structure for a derived Show, or only its name.
#[test]
fn error_shows() {
    let m = program(
        "\
error Bare

error Shown(code: Int) derive Show

error Named(code: Int)

impl Named: Show {
  fn show(self) { \"named\" }
}

type Plain(n: Int)

fn pick(n: Int) -> Int fails Bare | Shown | Named {
  p = Plain(n:)
  if n == 0 { fail Bare } else if n == 1 { fail Shown(code: 1) } else { fail Named(code: 2) }
}

fn main() -> Int fails Bare | Shown | Named { pick(1) }
",
    );
    let shows: Vec<String> = m
        .error_shows
        .iter()
        .map(|&(id, f)| format!("{}: {}", m.core.enums[id].name, m.functions[f].name))
        .collect();
    assert_eq!(
        shows,
        [
            "Bare: name$Bare",
            "Shown: show$Shown",
            "Named: Named: Show.show"
        ]
    );
}

/// A scope whose failure monomorphization over-approximates (a set type argument made empty) does
/// not fail in a body that cannot fail, and fails where the errors are real; so does raising a
/// cause again (`Cause.raise` of an `Exit[A, {}]` and of an `Exit[A, Oops]`).
#[test]
fn scopes_that_cannot_fail() {
    let m = program(
        "\
import Std.Fiber.{Fiber}
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause}

error Oops

fn run[E](f: fn() -> Int fails E uses { Async }) -> Int fails E uses { Async } {
  Fiber.scope(|| f())
}

fn reraise[E](f: fn() -> Int fails E uses { Async }) -> Int fails E uses { Async } {
  Exit.attempt(f) match {
    Success(x) -> x
    Failure(c) -> Cause.raise(c)
  }
}

fn awaited() -> Int uses { Async } {
  Fiber.scope(|| {
    f = Fiber.fork(|| 1)
    Fiber.await(f) match {
      Success(x) -> x
      Failure(c) -> Cause.raise(c)
    }
  })
}

fn attempted() -> Int fails Oops uses { Async } {
  Exit.attempt(|| fail Oops) match {
    Success(x) -> x
    Failure(c) -> Cause.raise(c)
  }
}

fn main() -> Int fails Oops uses { Async } {
  a = run(|| 1)
  b = run(|| fail Oops)
  c = awaited() + attempted() + reraise(|| 1)
  d = Fiber.scope(|| panic(\"no\"))
  a
}
",
    );
    insta::assert_snapshot!(parts(&m, &["run", "reraise", "awaited", "attempted"]));
}

/// Malformed Core IR is reported, with the function it is in, instead of converted.
#[test]
fn conversion_errors_name_the_function() {
    let mut core = emela_core::Module::new();
    let main = core.declare("main", true);
    let x = core.local("x", emela_core::TypeId::INT);
    core.define(
        main,
        vec![],
        emela_core::TypeId::INT,
        emela_core::build::var(x),
    );
    let Err(errors) = from_core(core) else {
        panic!("an unbound local is converted");
    };
    let errors: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(errors, ["fn main: local x%0 is used unbound"]);
}

#[test]
fn with_and_release() {
    let m = program(
        "\
effect Pool {
  fn get() -> Int
}

handler Counted(n: Int): Pool {
  init { Counted(n: 1) }
  release { dbg(self.n) }
  fn get() { self.n }
}

fn work() -> Int uses { Pool } {
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

handler Quiet: Log {
  fn info(msg) { () }
  fn count() { 0 }
}

fn report(x: Int) -> Int uses { Log } {
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

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int) -> Int uses { Clock } {
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
import Std.Fiber.{Fiber, Scope}
import Std.Exit.{Exit}
import Std.Deferred.{Deferred}

fn main() -> Int uses { Async } {
  Fiber.scope(|| {
    s = use Scope
    a = s.fork(|| [1])
    d = s.deferred()
    Deferred.complete(d, ())
    e = Exit.attempt(|| 3)
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

handler Later: Wait {
  init { Later }
  release { (use Wait).wait() }
  suspend fn wait(resume) { resume(()) }
}

fn main() -> Int uses { Async, Wait } {
  with Later { () }
  Fiber.scope(|| {
    s = use Scope
    a = s.fork(|| 1)
    Fiber.join(a)
  })
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main"]));
}

/// A release that may suspend holds off interruption until it ends (spec 11.4): one that calls a
/// suspend fn directly or nested in its body, or waits for a scope; not one that only makes a
/// lambda that would suspend, nor one that does nothing.
#[test]
fn releases_hold_only_when_they_may_suspend() {
    let m = program(
        "\
import Std.Fiber.{Fiber}

effect Wait {
  suspend fn wait() -> ()
}

effect Ka {
  fn a() -> ()
}

effect Kb {
  fn b() -> ()
}

effect Kc {
  fn c() -> ()
}

effect Kd {
  fn d() -> ()
}

effect Ke {
  fn e() -> ()
}

handler Direct: Ka {
  init { Direct }
  release { (use Wait).wait() }
  fn a() { () }
}

handler Nested: Kb {
  init { Nested }
  release {
    x = 1
    (use Wait).wait()
  }
  fn b() { () }
}

handler Lambda: Kc {
  init { Lambda }
  release {
    f = || (use Wait).wait()
    ()
  }
  fn c() { () }
}

handler Scoped: Kd {
  init { Scoped }
  release { Fiber.scope(|| ()) }
  fn d() { () }
}

handler Plain: Ke {
  init { Plain }
  release { () }
  fn e() { () }
}

fn main() -> () uses { Async, Wait } {
  with Direct { () }
  with Nested { () }
  with Lambda { () }
  with Scoped { () }
  with Plain { () }
}
",
    );
    let out = parts(&m, &["main"]);
    let holds: Vec<usize> = out
        .split("handler ")
        .skip(1)
        .map(|part| part.matches("runtime hold").count())
        .collect();
    assert_eq!(holds, [1, 1, 0, 1, 0], "{out}");
}

#[test]
fn closures_with_captures() {
    let m = program(
        "\
fn adder(base: Int) -> fn(Int) -> (fn(Int) -> Int) {
  |x| {
    y = x * 2
    |z| base + y + z
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
  derive Show

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
pub type Point(x: Int, y: Int)
  derive Eq, Show

enum Tree {
  Leaf,
  Node(Tree, Point, Tree),
}
  derive Eq

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
