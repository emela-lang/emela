//! Tests for the Heap IR passes (`emela_lir::to_heap`): programs are lowered, monomorphized,
//! converted to the Flow IR and taken to the Heap IR, and the result must pass the verifier at
//! the Heap IR's stage.

#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use emela_core::{Roots, monomorphize_from, verify_monomorphic};
use emela_lir::{Module, Stage, dump_function, from_core, to_heap, verify};
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
    assert!(m.functions.len() > 100);
    if let Some(path) = std::env::var_os("EMELA_HEAP_DUMP") {
        std::fs::write(path, emela_lir::dump(&m)).unwrap();
    }
}

/// Every function of each example project (`examples/*/src`) goes through the passes. Suspending
/// functions are only closure-converted (see `Stage`).
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
  fn(x) {
    y = x * 2
    fn(z) { base + y + z }
  }
}

fn twice(f: fn(Int) -> Int, x: Int) -> Int { f(f(x)) }

fn inc(x: Int) -> Int { x + 1 }

fn main() {
  f = adder(1)
  g = f(2)
  dbg(g(3))
  dbg(twice(inc, 1))
  dbg(List.map([1, 2], fn(x) { x * 10 }))
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

handler Counted(n: Int) implements Pool {
  init { Counted(n: 1) }
  release { dbg(self.n) }
  fn get() { self.n }
}

error Empty

fn work(k: Int) -> Int fails Empty use Pool {
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

handler Strict implements Pool {
  release { panic(\"release failed\") }
  fn get() { 1 }
}

fn pick(xs: List[Int]) -> Int use Pool {
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

#[test]
fn effects_and_handlers() {
    let m = program(
        "\
effect Log {
  fn info(msg: String) -> ()
  fn count() -> Int
}

handler Prefixed(prefix: String) implements Log {
  init { Prefixed(prefix: \"> \") }
  fn info(msg) {
    dbg(\"#{self.prefix}#{msg}\")
    ()
  }
  fn count() { 0 }
}

fn report(x: Int) -> Int use Log {
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

/// Suspending functions are only closure-converted (the state machine comes later); the verifier
/// checks them at `Stage::ClosuresConverted`.
#[test]
fn suspending_functions_are_left() {
    let m = program(
        "\
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler InstantClock implements Clock {
  suspend fn sleep(ms, resume) { resume(ms) }
}

fn nap(ms: Int, label: String) -> Int use Clock {
  with InstantClock { dbg(label) }
  (use Clock).sleep(ms) + 1
}

fn main() {
  with InstantClock { dbg(nap(1, \"a\")) }
}
",
    );
    assert!(m.functions.iter().any(|(_, f)| f.suspends));
    insta::assert_snapshot!(parts(&m, &["nap", "InstantClock"]));
}

#[test]
fn interpolation() {
    let m = program(
        "\
enum Shape {
  Circle(radius: Float),
  Named(String, List[Option[Int]]),
}

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
