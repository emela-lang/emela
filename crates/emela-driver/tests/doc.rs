//! Collecting API documentation (`emela doc`). The collected docs are snapshotted as text.

use std::fmt::Write;
use std::path::Path;

use emela_driver::{
    DocKind, DocModule, Input, MemoryFiles, Typecheck, check, collect_docs, render_html,
    render_markdown,
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
pub handler Prefixed(prefix: String): Log {
  init { Prefixed(prefix: "> ") }
  fn info(msg) { () }
}

pub handler Quiet: Log {
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
pub fn find(id: Int) -> Point fails NotFound | Timeout uses { Log } {
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
        "Std.Http",
        "Std.Task",
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
        "fn map[A, B](xs: List[A], f: fn(A) -> B fails E(f) uses { R(f) }) -> List[B] fails E(f) uses { R(f) }"
    );
    // Std.Set.filter is inferred in one group with Std.Map.filter, which its body calls as a
    // method; its R names its own parameter, not one of Map.filter (issue #684).
    assert_eq!(
        signature("Std.Set", "filter"),
        "fn filter[A](s: Set[A], f: fn(A) -> Bool fails E(f) uses { R(f) }) -> Set[A] fails E(f) uses { R(f) }"
    );
    // Opaque types show no fields (spec 4.4).
    assert_eq!(
        signature("Std.Duration", "Duration"),
        "opaque type Duration"
    );

    // An operation lists only its own type parameters, not the effect's.
    let group = modules
        .iter()
        .find(|m| m.name == "Std.Task")
        .and_then(|m| m.items.iter().find(|i| i.name == "TaskGroup"))
        .unwrap();
    assert_eq!(group.signature, "effect TaskGroup[E, R]");
    assert!(
        group.children[0].signature.starts_with("fn start[A]("),
        "{}",
        group.children[0].signature
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
    insta::assert_snapshot!("markdown_index", files[0].text());
    // A module with nothing public gets a page with only its name.
    assert_eq!(files[1].text(), "# Main\n");
    insta::assert_snapshot!("markdown_shapes", files[2].text());
}

#[test]
fn html_pages() {
    let modules = docs(
        &[
            ("main.emel", "import Shapes\n\nfn main() -> Int { 0 }\n"),
            ("shapes.emel", SHAPES),
        ],
        false,
    );
    let files = render_html("app", &modules);
    let paths: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
    assert_eq!(
        paths,
        ["index.html", "style.css", "Main.html", "Shapes.html"]
    );
    let shapes = files[3].text();
    // Type names in signatures link to their item on the same page.
    assert!(
        shapes.contains(r##"-&gt; <a class="ty" href="#Point">Point</a>"##),
        "{shapes}"
    );
    // Children can be linked to.
    assert!(shapes.contains(r#"<li id="Shape.Circle">"#), "{shapes}");
    insta::assert_snapshot!("html_shapes", shapes);
}

#[test]
fn html_links_across_modules_and_escapes_raw_html() {
    let modules = docs(
        &[
            ("main.emel", "import Box\n\nfn main() -> Int { 0 }\n"),
            (
                "box.emel",
                "import Std.Duration.{Duration}\n\n\
                 ## Waits. <script>alert(1)</script>\n\
                 pub fn wait(d: Duration) -> Option[Int] { None }\n",
            ),
        ],
        false,
    );
    let core = docs(&[("main.emel", "fn main() -> Int { 0 }\n")], true);
    let mut all = core;
    all.extend(modules);
    all.sort_by(|a, b| a.name.cmp(&b.name));
    let files = render_html("app", &all);
    let page = &files
        .iter()
        .find(|f| f.path.to_str() == Some("Box.html"))
        .unwrap()
        .text();
    // Defined in exactly one other module: linked to that page.
    assert!(
        page.contains(r#"<a class="ty" href="Std.Duration.html#Duration">Duration</a>"#),
        "{page}"
    );
    assert!(
        page.contains(r#"<a class="ty" href="Prelude.html#Option">Option</a>"#),
        "{page}"
    );
    // Raw HTML in a doc stays text.
    assert!(
        page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{page}"
    );
    assert!(!page.contains("<script>"), "{page}");
}

/// The bundled modules cite the specification by section name (`[effect.host]`), not by number
/// (#686). Numbers change when sections move. Covers `##` and `#` comments and the host files.
#[test]
fn core_sources_cite_sections_by_name() {
    fn visit(dir: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, found);
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (n, line) in text.lines().enumerate() {
                let numbered = line.match_indices("spec ").any(|(at, _)| {
                    let rest = &line[at + "spec ".len()..];
                    rest.starts_with(|c: char| c.is_ascii_digit()) || rest.starts_with("chapter")
                });
                if numbered {
                    found.push(format!("{}:{}: {line}", path.display(), n + 1));
                }
            }
        }
    }
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../emela-core/lib");
    let mut found = Vec::new();
    visit(&lib, &mut found);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Every `pub` declaration of core and std, and every operation of an effect and function of a
/// trait, has a `##` doc comment, so that its page in `emela doc --std` says what it is (#686).
#[test]
fn core_docs_describe_every_public_item() {
    let modules = docs(&[("main.emel", "fn main() -> Int { 0 }\n")], true);
    let mut missing = Vec::new();
    for module in &modules {
        for item in &module.items {
            if item.doc.trim().is_empty() {
                missing.push(format!("{}.{}", module.name, item.name));
            }
            if matches!(item.kind, DocKind::Effect | DocKind::Trait) {
                for child in &item.children {
                    if child.doc.trim().is_empty() {
                        missing.push(format!("{}.{}.{}", module.name, item.name, child.name));
                    }
                }
            }
        }
    }
    assert!(missing.is_empty(), "no doc comment: {missing:?}");
}

/// Notes on how core and std are implemented are `#` comments, not `##` doc comments, so they do
/// not show in `emela doc --std` or in hover (#686).
#[test]
fn core_docs_leave_out_implementation_notes() {
    let modules = docs(&[("main.emel", "fn main() -> Int { 0 }\n")], true);
    let mut found = Vec::new();
    for module in &modules {
        for item in &module.items {
            let docs = std::iter::once(&item.doc).chain(item.children.iter().map(|c| &c.doc));
            for doc in docs {
                if doc.contains(".emel") || doc.contains("external function boundary") {
                    found.push(format!("{}.{}: {doc}", module.name, item.name));
                }
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}
