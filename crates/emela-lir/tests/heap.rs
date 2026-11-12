//! Tests for the Heap IR passes (`emela_lir::to_heap`): programs are lowered, monomorphized,
//! converted to the Flow IR and taken to the Heap IR, and the result must pass the verifier at
//! the Heap IR's stage.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::{
    Callee, ErrorKind, Module, Stage, Stmt, Value, dump_function, from_core, to_heap, verify,
};
use support::try_lower;

/// Lowers (from `roots`), monomorphizes, converts, runs the Heap IR passes and verifies.
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
    let mut lir = from_core(mono).unwrap_or_else(|errors| {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("conversion errors:\n{}", lines.join("\n"));
    });
    to_heap(&mut lir);
    assert!(lir.stage > Stage::Flow);
    if let Err(errors) = verify(&lir) {
        let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
        panic!("Heap IR verifier errors:\n{}", lines.join("\n"));
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

/// Every function of the bundled core and std goes through the passes.
#[test]
fn every_core_function_converts() {
    let m = convert(&[("main.emel", "fn main() { 0 }\n")], Roots::All);
    // The check is the verifier inside `convert`; the count only guards against converting
    // nothing (core and std have hundreds of functions).
    assert!(m.functions.len() > 100);
    if let Some(path) = std::env::var_os("EMELA_HEAP_DUMP") {
        std::fs::write(path, emela_lir::dump(&m)).unwrap();
    }
}

/// Every function of each example project (`examples/*/src`) goes through the passes, the
/// suspending ones (fibers, clocks) included.
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
        if let Some(dir) = std::env::var_os("EMELA_HEAP_DUMP_DIR") {
            let name = src.parent().unwrap().file_name().unwrap();
            let path = std::path::Path::new(&dir).join(name).with_extension("heap");
            std::fs::write(path, emela_lir::dump(&m)).unwrap();
        }
    }
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

fn twice(f: fn(Int) -> Int, x: Int) -> Int { f(f(x)) }

fn inc(x: Int) -> Int { x + 1 }

fn main() {
  f = adder(1)
  g = f(2)
  dbg(g(3))
  dbg(twice(inc, 1))
  dbg(List.map([1, 2], |x| x * 10))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["adder", "main", "inc"]));
}

#[test]
fn matching_lists_and_enums() {
    let m = program(
        "\
enum Shape {
  Circle(radius: Float),
  Rect(Float, Float),
  Group(List[Shape]),
}

fn count(s: Shape) -> Int {
  s match {
    Circle(_) -> 1
    Rect(_, _) -> 1
    Group(items) -> sum(items, 0)
  }
}

fn sum(items: List[Shape], acc: Int) -> Int {
  items match {
    [] -> acc
    [first, ..rest] -> sum(rest, acc + count(first))
  }
}

fn main() {
  dbg(count(Group([Circle(radius: 1.0), Rect(1.0, 2.0)])))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["count", "sum"]));
}

#[test]
fn errors_with_escape() {
    let m = program(
        "\
error NotFound(id: Int)
error Invalid(reason: String)

fn find(id: Int) -> String fails NotFound | Invalid {
  if id < 0 { fail Invalid(reason: \"negative\") }
  if id == 0 { fail NotFound(id:) }
  \"user\"
}

fn lookup(id: Int, fallback: String) -> String fails Invalid {
  find(id) escape {
    NotFound(_) -> fallback
  }
}

fn ratio(a: Int, b: Int) -> Int { a / b + a % 2 }

fn main() {
  dbg(ratio(4, 2))
  dbg(lookup(1, \"guest\")) escape {
    Invalid(reason:) -> reason
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["find", "lookup", "ratio"]));
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

error Empty

fn work(k: Int) -> Int fails Empty uses { Pool } {
  if k == 0 { fail Empty }
  (use Pool).get() + k
}

fn main() {
  with Counted {
    dbg(work(1)) escape { Empty -> 0 }
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "work", "Counted"]));
}

#[test]
fn defects_through_releases() {
    let m = program(
        "\
effect Pool {
  fn get() -> Int
}

handler Strict: Pool {
  release { panic(\"release failed\") }
  fn get() { 1 }
}

fn pick(xs: List[Int]) -> Int uses { Pool } {
  xs match {
    [] -> panic(\"empty\")
    [x, ..] -> x / (use Pool).get()
  }
}

fn main() -> Int {
  with Strict {
    pick([1])
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "pick", "Strict$release"]));
}

/// `Cause.raise` of a `Cause[{}]` cannot fail, so a body that cannot fail may call it (spec
/// 11.5): the raise has no failure edge there.
#[test]
fn raising_a_cause_that_cannot_fail() {
    let m = program(
        "\
import Std.Exit.{Success, Failure}
import Std.Cause.{Cause}

fn again() -> Int {
  Exit.attempt(|| panic(\"bad\")) match {
    Success(v) -> v
    Failure(c) -> Cause.raise(c)
  }
}

fn main() -> Int { again() }
",
    );
    let f = m
        .functions
        .iter()
        .find(|(_, f)| f.name == "again")
        .map(|(_, f)| f)
        .unwrap();
    assert!(!f.fails);
}

#[test]
fn effects_and_handlers() {
    let m = program(
        "\
effect Log {
  fn info(msg: String) -> ()
  fn count() -> Int
}

handler Prefixed(prefix: String): Log {
  init { Prefixed(prefix: \"> \") }
  fn info(msg) {
    dbg(\"#{self.prefix}#{msg}\")
    ()
  }
  fn count() { 0 }
}

fn report(x: Int) -> Int uses { Log } {
  log = use Log
  log.info(\"x\")
  x + log.count()
}

fn main() {
  with Prefixed {
    dbg(report(1))
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "report", "Prefixed"]));
}

/// Suspending functions become state machines: a fairness yield before each suspending call, and
/// a suspension edge on each value that may suspend, saving what is needed after it (in a loop,
/// across a release that may suspend, and in a lifted closure, which owns its environment).
#[test]
fn suspending_functions_become_state_machines() {
    let m = program(
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

handler Napper: Clock {
  init { Napper }
  release { (use Clock).sleep(0) }
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int, label: String) -> Int uses { Clock } {
  with Napper { dbg(label) }
  (use Clock).sleep(ms) + 1
}

fn count(i: Int, acc: Int) -> Int uses { Clock } {
  if i == 0 { acc } else { count(i - 1, acc + (use Clock).sleep(i)) }
}

fn twice(f: fn() -> Int uses { Clock }) -> Int uses { Clock } {
  f() + f()
}

fn main() {
  label = \"a\"
  with InstantClock { dbg(twice(|| nap(count(2, 0), label))) }
}
",
    );
    assert!(m.functions.iter().any(|(_, f)| f.suspends));
    insta::assert_snapshot!(parts(&m, &["nap", "count", "main$closure0"]));
}

#[test]
fn interpolation() {
    let m = program(
        "\
enum Shape {
  Circle(radius: Float),
  Named(String, List[Option[Int]]),
}
  derive Show

fn label(s: Shape, n: Int) -> String {
  \"shape #{s} number #{n}\"
}

fn main() {
  dbg(label(Named(\"a\", [Some(1), None]), 2))
}
",
    );
    insta::assert_snapshot!(parts(&m, &["label", "show$Shape", "show$List"]));
}

const NAPS: &str = "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int, label: String) -> String uses { Clock } {
  n = (use Clock).sleep(ms)
  \"#{label} #{ms} #{n}\"
}

fn main() {
  with InstantClock { dbg(nap(1, \"a\")) }
}
";

/// The error kinds the verifier finds in `nap` of [`NAPS`] after `tamper` changes the suspension
/// edge of its operation call.
fn tampered(tamper: impl FnOnce(&mut emela_lir::Function, emela_lir::StmtId)) -> Vec<ErrorKind> {
    let mut m = program(NAPS);
    let (id, _) = m
        .functions
        .iter()
        .find(|(_, f)| f.name == "nap")
        .expect("nap");
    let f = &mut m.functions[id];
    let (call, _) = f
        .stmts
        .iter()
        .find(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Call {
                        callee: Callee::Op { .. },
                        ..
                    },
                    ..
                }
            )
        })
        .expect("the operation call");
    tamper(f, call);
    verify(&m)
        .expect_err("the tampered module must fail")
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

fn edge(f: &mut emela_lir::Function, s: emela_lir::StmtId) -> &mut Option<emela_lir::Suspend> {
    match &mut f.stmts[s] {
        Stmt::Let { on_suspend, .. } => on_suspend,
        _ => unreachable!(),
    }
}

/// The verifier checks the suspension edges: present exactly on the values that may suspend in a
/// suspending function, with distinct states, saving every variable needed after the value, and
/// (with reference counting) exactly the references owned there.
#[test]
fn the_verifier_checks_suspension_edges() {
    assert_eq!(
        tampered(|f, s| *edge(f, s) = None),
        [ErrorKind::SuspendEdge]
    );
    // The state of the fairness yield before the call.
    assert_eq!(
        tampered(|f, s| edge(f, s).as_mut().unwrap().state = 0),
        [ErrorKind::SuspendEdge]
    );
    // `ms` (an `i32`) is needed after the call.
    assert_eq!(
        tampered(|f, s| {
            let ms = f.params[0];
            edge(f, s).as_mut().unwrap().saved.retain(|&v| v != ms);
        }),
        [ErrorKind::SuspendEdge]
    );
    // Saving a reference twice moves two references into the frame.
    assert_eq!(
        tampered(|f, s| {
            let e = edge(f, s).as_mut().unwrap();
            let label = e.saved[1];
            e.saved.push(label);
        }),
        [ErrorKind::RcImbalance]
    );
    // A non-suspending function has no suspension edges.
    let mut m = program(NAPS);
    let (id, _) = m
        .functions
        .iter()
        .find(|(_, f)| f.name == "main")
        .expect("main");
    let f = &mut m.functions[id];
    let (_, stmt) = f
        .stmts
        .iter_mut()
        .find(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Call { suspends: true, .. },
                    ..
                }
            )
        })
        .expect("the call of nap");
    if let Stmt::Let { on_suspend, .. } = stmt {
        *on_suspend = Some(emela_lir::Suspend {
            state: 0,
            saved: Vec::new(),
        });
    }
    let kinds: Vec<ErrorKind> = verify(&m)
        .unwrap_err()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, [ErrorKind::SuspendEdge]);
}

/// More of the suspension edges: an edge on a value that cannot suspend; and with another error
/// in the function, the saved variables are not checked (the edges may be what is wrong).
#[test]
fn the_verifier_checks_where_edges_are() {
    assert_eq!(
        tampered(|f, call| {
            let (other, _) = f
                .stmts
                .iter()
                .find(|&(s, stmt)| {
                    s != call
                        && matches!(stmt, Stmt::Let { value, on_suspend: None, .. }
                            if !emela_lir::may_suspend(value))
                })
                .expect("a value that cannot suspend");
            *edge(f, other) = Some(emela_lir::Suspend {
                state: 9,
                saved: Vec::new(),
            });
        }),
        [ErrorKind::SuspendEdge]
    );
    assert_eq!(
        tampered(|f, s| {
            let ms = f.params[0];
            let e = edge(f, s).as_mut().unwrap();
            e.state = 0;
            e.saved.retain(|&v| v != ms);
        }),
        [ErrorKind::SuspendEdge]
    );
}

/// After the state machine pass, before reference counting, the suspension edges are where they
/// belong (spec 16.5).
#[test]
fn state_machines_stage_verifies() {
    let m = try_lower(&[("main.emel", NAPS)], Roots::Exported).unwrap_or_else(|e| panic!("{e:#?}"));
    let roots: Vec<_> = m
        .functions
        .iter()
        .filter(|(_, f)| f.exported)
        .map(|(id, _)| id)
        .collect();
    let (mono, _) = monomorphize_from(&m, &roots);
    let mut lir = from_core(mono).unwrap_or_else(|e| panic!("{e:?}"));
    emela_lir::convert_closures(&mut lir);
    emela_lir::explicit_defects(&mut lir);
    emela_lir::state_machines(&mut lir);
    assert_eq!(lir.stage, Stage::StateMachines);
    assert_eq!(verify(&lir), Ok(()));
}

/// Which functions may come back with a pending defect (`Function::defects`): a builtin that may
/// raise one (a shift by a negative amount, spec 7.5), a concurrency operation that waits (spec
/// 11.4), a defect inside a failure handler's body, a const's wrapper (spec 5.6), and the callers
/// of such functions. Calls of other builtins and `fork` do not.
#[test]
fn which_functions_may_defect() {
    let m = program(
        "\
import Std.Fiber.{Fiber}

error Oops

const VALUE: Int = 1

fn shifts(n: Int) -> Int { Int.shift_left(n, 1) }

fn calls_shifts(n: Int) -> Int { shifts(n) + 1 }

fn lengths(s: String) -> Int { String.length(s) }

fn calls_lengths(s: String) -> Int { lengths(s) + 1 }

fn risky(n: Int) -> Int fails Oops {
  if n == 0 { fail Oops }
  n
}

fn handled(n: Int) -> Int {
  risky(n) escape {
    Oops -> Int.shift_left(n, 1)
  }
}

fn joins() -> Int uses { Async } { Fiber.scope(|| Fiber.join(Fiber.fork(|| 1))) }

fn main() -> Int uses { Async } {
  calls_shifts(1) + calls_lengths(\"a\") + handled(1) + joins() + VALUE
}
",
    );
    let mut defects: Vec<&str> = m
        .functions
        .iter()
        .filter(|(_, f)| f.defects)
        .map(|(_, f)| f.name.as_str())
        .filter(|name| {
            [
                "VALUE",
                "shifts",
                "calls_shifts",
                "lengths",
                "risky",
                "handled",
                "joins",
                "main",
            ]
            .contains(name)
        })
        .collect();
    defects.sort_unstable();
    assert_eq!(
        defects,
        [
            "VALUE",
            "calls_shifts",
            "handled",
            "joins",
            "main",
            "shifts"
        ]
    );
}

/// Reference counting at its edges (Perceus): a value passed twice to one call gets a second
/// reference, an external function only borrows its arguments, a reference live at a loop head
/// stays owned around the loop, and two closures of one function are lifted under different
/// names.
#[test]
fn reference_counts_at_the_edges() {
    let m = program(
        "\
@external(js, \"host\")
fn host(s: String) -> Int uses {}

fn pair(a: String, b: String) -> Int { String.length(a) + String.length(b) }

fn count(n: Int, s: String) -> Int {
  if n == 0 { String.length(s) } else { count(n - 1, s) }
}

fn main() -> Int {
  t = String.append(\"a\", \"b\")
  f = || String.length(t)
  g = || String.length(t) + 1
  pair(t, t) + host(t) + count(3, t) + f() + g()
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main", "count"]));
}

/// A call that may raise a defect while references are owned gets a landing that drops them
/// (`on_defect`): a builtin that may raise one and a concurrency operation that waits (`fork`
/// too, used after its scope ended, spec 11.8); a builtin that cannot and showing a cause do
/// not.
#[test]
fn defect_landings() {
    let m = program(
        "\
import Std.Fiber.{Fiber}
import Std.Exit.{Success, Failure}
import Std.Cause.{Cause}

fn main() -> Int uses { Async } {
  s = String.append(\"a\", \"b\")
  a = Int.shift_left(String.length(s), 1)
  b = Fiber.scope(|| {
    f = Fiber.fork(|| 1)
    g = Fiber.fork(|| 2)
    Fiber.join(f) + (Fiber.await(g) match {
      Success(x) -> x
      Failure(c) -> String.length(Cause.to_string(c))
    })
  })
  String.length(s) + a + b
}
",
    );
    insta::assert_snapshot!(parts(&m, &["main"]));
}

/// A reference passed to a call and live on its failure landing keeps a reference for the landing
/// (the caught error is the landing's own), and a String taken back from `any` (a child's result in `Fiber.both`) is a view of it, not
/// a new reference.
#[test]
fn references_on_failure_landings_and_boxed_results() {
    let m = program(
        "\
import Std.Fiber.{Fiber}

error Oops

fn risky(n: Int) -> Int fails Oops {
  if n == 0 { fail Oops }
  n
}

fn risky_len(s: String) -> Int fails Oops {
  if String.length(s) == 0 { fail Oops }
  String.length(s)
}

fn handled(n: Int) -> Int {
  s = String.append(\"a\", \"b\")
  x = risky_len(s) escape {
    Oops -> String.length(s)
  }
  x
}

effect Wait {
  suspend fn wait() -> ()
}

handler Waits: Wait {
  suspend fn wait(resume) { resume(()) }
}

fn word(n: Int) -> String fails Oops uses { Wait } {
  (use Wait).wait()
  if n == 0 { fail Oops }
  \"x\"
}

fn main() -> Int fails Oops uses { Async } {
  with Waits {
    (a, b) = Fiber.both(|| word(1), || word(2))
    handled(1) + String.length(a) + String.length(b)
  }
}
",
    );
    insta::assert_snapshot!(parts(&m, &["handled", "Std.Fiber.both"]));
}

/// Whether the array argument of each `Array.set` in the function named `name` is still owned by
/// the function after the call: a `dup` right before the call gives `set` a reference of its own
/// while the function keeps one, so the array is shared and `set` copies.
fn set_keeps_array(m: &Module, name: &str) -> Vec<bool> {
    let (_, f) = m
        .functions
        .iter()
        .find(|(_, f)| f.name == name)
        .unwrap_or_else(|| panic!("no function {name}"));
    let mut out = Vec::new();
    for (_, seq) in f.seqs.iter() {
        for (i, &s) in seq.stmts.iter().enumerate() {
            let Stmt::Let {
                value:
                    Value::Builtin {
                        op: emela_core::Builtin::ArraySet,
                        args,
                    },
                ..
            } = &f.stmts[s]
            else {
                continue;
            };
            let Some(emela_lir::Operand::Var(a)) = args.first() else {
                panic!("the array of `set` is a variable");
            };
            let dup = i > 0 && f.stmts[seq.stmts[i - 1]] == Stmt::Dup(*a);
            out.push(dup);
        }
    }
    out
}

/// `[api.std.array]`: `set` consumes its array argument, so when the argument is a variable not
/// used after the call (a parameter or a local binding), the reference counting pass moves the last
/// reference into the call and the runtime sees a count of 1 (in place, O(1)). When the variable is
/// used after the call, `set` gets a second reference and copies. (When another value holds the
/// array, the count is above 1 at run time; the parity program `array` checks that case.)
#[test]
fn array_set_takes_the_last_reference() {
    let m = program(
        "\
import Std.Array.{Array}

fn histogram(bytes: List[Int], counts: Array[Int]) -> Array[Int] {
  bytes match {
    [] -> counts
    [b, ..rest] -> {
      n = Array.get(counts, b) |> Option.or(0)
      histogram(rest, Array.set(counts, b, n + 1))
    }
  }
}

fn bump_twice(counts: Array[Int], b: Int) -> (Array[Int], Array[Int]) {
  (Array.set(counts, b, 1), counts)
}

fn local(n: Int) -> Array[Int] {
  a = Array.repeat(0, n)
  b = Array.set(a, 0, 1)
  Array.set(b, 1, 2)
}

fn main() {
  dbg(histogram([1, 2, 1], Array.repeat(0, 3)))
  dbg(bump_twice(Array.repeat(0, 3), 1))
  dbg(local(2))
}
",
    );
    assert_eq!(set_keeps_array(&m, "histogram"), [false]);
    assert_eq!(set_keeps_array(&m, "local"), [false, false]);
    assert_eq!(set_keeps_array(&m, "bump_twice"), [true]);
}
