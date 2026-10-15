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
