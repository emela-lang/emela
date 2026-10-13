//! Collecting API documentation (`emela doc`). The collected docs are snapshotted as text.

use std::fmt::Write;
use std::path::Path;

use emela_driver::{
    DocModule, Input, MemoryFiles, Typecheck, check, collect_docs, render_markdown,
};

/// Type checks a project with these files under `app/src` and collects its docs.
fn docs(files: &[(&str, &str)], core: bool) -> Vec<DocModule> {
    let mut sources = vec![("app/Pome.toml".to_string(), String::new())];
    sources.extend(
        files
            .iter()
            .map(|(path, text)| (format!("app/src/{path}"), text.to_string())),
    );
    let fs = MemoryFiles::new(sources.iter().map(|(p, t)| (p.as_str(), t.as_str())));
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut Typecheck::default());
    assert!(
        !analysis.has_errors(),
        "{}",
        emela_driver::render(&analysis.diagnostics, &analysis.sources, false)
    );
    collect_docs(&program.unwrap(), &analysis, core)
}

/// One line per item and child, with the doc indented under it.
fn dump(modules: &[DocModule]) -> String {
    let mut out = String::new();
    for module in modules {
        writeln!(out, "== {}", module.name).unwrap();
        for item in &module.items {
            writeln!(out, "[{:?}] {}", item.kind, item.signature).unwrap();
            for line in item.doc.lines() {
                writeln!(out, "{}", format!("  | {line}").trim_end()).unwrap();
            }
            for child in &item.children {
                writeln!(out, "  - {}", child.signature).unwrap();
                for line in child.doc.lines() {
                    writeln!(out, "{}", format!("    | {line}").trim_end()).unwrap();
                }
            }
        }
    }
    out
}

const SHAPES: &str = r#"## A point on the plane.
pub type Point(
  ## Distance from the left.
  x: Int,
  y: Int,
)

## Hidden fields: only the name is shown.
pub opaque type Secret(value: String)

pub enum Shape {
  ## A circle around the origin.
  Circle(radius: Int)
  Rect(Int, Int)
  Empty
}

## Not public, so not documented.
type Hidden(n: Int)

pub error NotFound(id: Int)

pub error Timeout

## Writes log lines.
pub effect Log {
  ## One line at the info level.
  fn info(msg: String) -> ()
}

## Prefixes each line.
pub handler Prefixed(prefix: String) implements Log {
  init { Prefixed(prefix: "> ") }
  fn info(msg) { () }
}

pub handler Quiet implements Log {
  fn info(msg) { () }
}

pub layer Logging {
  Quiet
}

pub const LIMIT: Int = 3

## Finds a point, or fails.
##
## Indented lines stay indented:
##
##     find(1)
pub fn find(id: Int) -> Point fails NotFound | Timeout use { Log } {
  (use Log).info("find")
  if id < 0 { fail NotFound(id:) }
  Point(x: id, y: id)
}

pub fn apply[A, B](x: A, f: fn(A) -> B) -> B {
  f(x)
}

fn helper() -> Int { 1 }
"#;

#[test]
fn project_docs() {
    let modules = docs(
        &[
            ("main.emel", "import Shapes\n\nfn main() -> Int { 0 }\n"),
            ("shapes.emel", SHAPES),
        ],
        false,
    );
    insta::assert_snapshot!(dump(&modules));
}

#[test]
fn project_docs_leave_out_core() {
    let modules = docs(&[("main.emel", "fn main() -> Int { 0 }\n")], false);
    let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Main"]);
    assert!(modules[0].items.is_empty());
}

#[test]
fn core_docs_cover_core_and_std() {
    let modules = docs(&[("main.emel", "fn main() -> Int { 0 }\n")], true);
    let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    // Sorted by name, core and std together.
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    for name in [
        "List",
        "Prelude",
        "String",
        "Std.Clock",
        "Std.Fiber",
        "Std.Http",
    ] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    assert!(!names.contains(&"Main"));

    let signature = |module: &str, item: &str| {
        modules
            .iter()
            .find(|m| m.name == module)
            .and_then(|m| m.items.iter().find(|i| i.name == item))
            .map(|i| i.signature.clone())
            .unwrap_or_else(|| panic!("{module}.{item} missing"))
    };
    // The invisible sets of a function-typed parameter are shown as in hover (spec 9.2).
    assert_eq!(
        signature("List", "map"),
        "fn map[A, B](xs: List[A], f: fn(A) -> B fails E(f) use R(f)) -> List[B] fails E(f) use R(f)"
    );
    // Opaque types show no fields (spec 4.4).
    assert_eq!(
        signature("Std.Duration", "Duration"),
        "opaque type Duration"
    );

    // An operation lists only its own type parameters, not the effect's.
    let scope = modules
        .iter()
        .find(|m| m.name == "Std.Fiber")
        .and_then(|m| m.items.iter().find(|i| i.name == "Scope"))
        .unwrap();
    assert_eq!(scope.signature, "effect Scope[E, R]");
    assert!(
        scope.children[0].signature.starts_with("fn fork[A]("),
        "{}",
        scope.children[0].signature
    );
}

#[test]
fn markdown_pages() {
    let modules = docs(
        &[
            ("main.emel", "import Shapes\n\nfn main() -> Int { 0 }\n"),
            ("shapes.emel", SHAPES),
        ],
        false,
    );
    let files = render_markdown("app", &modules);
    let paths: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert_eq!(paths, ["index.md", "Main.md", "Shapes.md"]);
    insta::assert_snapshot!("markdown_index", files[0].contents);
    // A module with nothing public gets a page with only its name.
    assert_eq!(files[1].contents, "# Main\n");
    insta::assert_snapshot!("markdown_shapes", files[2].contents);
}
