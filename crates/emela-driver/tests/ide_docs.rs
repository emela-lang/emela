//! Doc comments in IDE queries, including private definitions, dependencies and std.

use std::path::Path;

use emela_driver::{
    Analysis, Dependencies, DependencyPome, DependencyRef, Frontend, Input, MemoryFiles, Typecheck,
    TypedProgram, check,
};
use line_index::TextSize;

const MAIN: &str = "/app/src/main.emel";
type Ide = Typecheck;

fn checked(fs: &MemoryFiles, dependencies: Dependencies) -> (Analysis, TypedProgram) {
    let mut input = Input::resolve(fs, Path::new("/app")).unwrap();
    input.dependencies = dependencies;
    let (analysis, program) = check(fs, &input, &mut Typecheck::default());
    (analysis, program.unwrap())
}

fn at(text: &str, needle: &str, within: usize) -> TextSize {
    ((text.find(needle).unwrap() + within) as u32).into()
}

fn hover_doc(
    analysis: &Analysis,
    program: &TypedProgram,
    text: &str,
    needle: &str,
    within: usize,
) -> String {
    let file = analysis.sources.file_id(Path::new(MAIN)).unwrap();
    let hover = Ide::hover(program, analysis, file, at(text, needle, within))
        .unwrap_or_else(|| panic!("no hover: {needle}"));
    hover
        .contents
        .split_once("\n\n---\n\n")
        .unwrap_or_else(|| panic!("{needle}: {hover:?}"))
        .1
        .to_owned()
}

const PRIVATE: &str = r#"## A private function.
##
##     twice(2)
fn twice(x: Int) -> Int { x + x }

## A private record.
type Point(
  ## Horizontal distance.
  x: Int,
)

## A private enum.
enum Shape {
  ## A point variant.
  Dot(
    ## Variant payload.
    point: Point,
  )
}

## A private error.
error Missing(
  ## Missing key.
  key: Int,
)

## A private effect.
effect Log {
  ## Logs a value.
  fn write(value: Int) -> ()
}

## A private handler.
handler Silent(prefix: Int): Log {
  ## Initializes logging.
  init { Silent(prefix: 0) }
  ## Releases logging.
  release { () }
  ## Implements logging.
  fn write(value) { () }
}

## A private layer.
layer Logging { Silent }

## A private trait.
trait Measure {
  ## Measures the value.
  fn measure(self) -> Int
}

## Measures points.
impl Point: Measure {
  ## A concrete measure.
  fn measure(self) { self.x }
}

## A private constant.
const LIMIT = 4

fn main() -> Int {
  p = Point(x: LIMIT)
  with Logging {
    log = use Log
    log.write(p.x)
    twice(Measure.measure(p))
  }
}
"#;

#[test]
fn private_declarations_children_and_references() {
    let fs = MemoryFiles::new([("/app/Pome.toml", ""), (MAIN, PRIVATE)]);
    let (analysis, program) = checked(&fs, Dependencies::default());
    assert_eq!(
        analysis
            .diagnostics
            .iter()
            .filter(|d| d.severity == emela_driver::Severity::Error)
            .count(),
        0
    );
    for (needle, within, expected) in [
        ("fn twice", 3, "A private function.\n\n    twice(2)"),
        ("twice(Measure", 0, "A private function.\n\n    twice(2)"),
        ("type Point", 5, "A private record."),
        ("Point(x: LIMIT)", 0, "A private record."),
        ("\n  x: Int", 3, "Horizontal distance."),
        ("p.x)", 2, "Horizontal distance."),
        ("enum Shape", 5, "A private enum."),
        ("Dot(", 0, "A point variant."),
        ("point: Point", 0, "Variant payload."),
        ("error Missing", 6, "A private error."),
        ("key: Int", 0, "Missing key."),
        ("effect Log", 7, "A private effect."),
        ("fn write(value: Int)", 3, "Logs a value."),
        ("log.write", 4, "Logs a value."),
        ("handler Silent", 8, "A private handler."),
        ("fn write(value)", 3, "Implements logging."),
        ("init {", 0, "Initializes logging."),
        ("release {", 0, "Releases logging."),
        ("impl Point: Measure", 0, "Measures points."),
        ("layer Logging", 6, "A private layer."),
        ("trait Measure", 6, "A private trait."),
        ("fn measure(self) ->", 3, "Measures the value."),
        ("Measure.measure(p)", 8, "Measures the value."),
        ("fn measure(self) {", 3, "A concrete measure."),
        ("const LIMIT", 6, "A private constant."),
    ] {
        assert_eq!(
            hover_doc(&analysis, &program, PRIVATE, needle, within),
            expected,
            "{needle}"
        );
    }
    let file = analysis.sources.file_id(Path::new(MAIN)).unwrap();
    let items = Ide::complete(&program, &analysis, file, at(PRIVATE, "twice(Measure", 0));
    for (name, doc) in [
        ("twice", "A private function.\n\n    twice(2)"),
        ("Point", "A private record."),
        ("Dot", "A point variant."),
        ("LIMIT", "A private constant."),
    ] {
        assert_eq!(
            items
                .iter()
                .find(|i| i.label == name)
                .unwrap()
                .documentation
                .as_deref(),
            Some(doc)
        );
    }
    let labels = Ide::complete(&program, &analysis, file, at(PRIVATE, "Point(x: LIMIT)", 6));
    assert_eq!(labels[0].label, "x:");
    assert_eq!(
        labels[0].documentation.as_deref(),
        Some("Horizontal distance.")
    );
    let fields = Ide::complete(&program, &analysis, file, at(PRIVATE, "p.x)", 2));
    assert_eq!(fields[0].label, "x");
    assert_eq!(
        fields[0].documentation.as_deref(),
        Some("Horizontal distance.")
    );
    let methods = Ide::complete(
        &program,
        &analysis,
        file,
        at(PRIVATE, "Measure.measure(p)", 8),
    );
    assert_eq!(
        methods[0].documentation.as_deref(),
        Some("Measures the value.")
    );
    let sig =
        Ide::signature_help(&program, &analysis, file, at(PRIVATE, "log.write(p.x)", 10)).unwrap();
    assert_eq!(sig.documentation.as_deref(), Some("Logs a value."));
}

#[test]
fn pome_and_std_docs_in_all_three_queries() {
    let text = "import Quince\nimport Std.Duration\n\nfn main() -> Int {\n  Quince.answer(1)\n  Duration.to_seconds(Duration.milliseconds(1))\n  0\n}\n";
    let fs = MemoryFiles::new([
        ("/app/Pome.toml", ""),
        (MAIN, text),
        ("/deps/quince/Pome.toml", ""),
        (
            "/deps/quince/src/lib.emel",
            "## From a dependency.\npub fn answer(n: Int) -> Int { n }\n",
        ),
    ]);
    let dependencies = Dependencies {
        pomes: vec![DependencyPome {
            name: "github.com/acme/quince".into(),
            dir: "/deps/quince".into(),
            last_element: "quince".into(),
            module: None,
            emela: Some(env!("CARGO_PKG_VERSION").into()),
            targets: None,
            version: "v1.0.0".into(),
            dependencies: vec![],
        }],
        project: vec![DependencyRef {
            pome: 0,
            module: None,
        }],
        dev: Vec::new(),
        manifest: "/app/Pome.toml".into(),
        allow_external: vec![],
    };
    let (analysis, program) = checked(&fs, dependencies);
    assert!(!analysis.has_errors(), "{:?}", analysis.diagnostics);
    let file = analysis.sources.file_id(Path::new(MAIN)).unwrap();
    for (needle, name, within, doc) in [
        ("Quince.answer(1)", "answer", 7, "From a dependency."),
        (
            "Duration.to_seconds(",
            "to_seconds",
            9,
            "Truncates toward zero.",
        ),
    ] {
        assert_eq!(hover_doc(&analysis, &program, text, needle, within), doc);
        let sig = Ide::signature_help(
            &program,
            &analysis,
            file,
            at(text, needle, within + name.len() + 1),
        )
        .unwrap();
        assert_eq!(sig.documentation.as_deref(), Some(doc));
        let items = Ide::complete(&program, &analysis, file, at(text, needle, within));
        assert_eq!(
            items
                .iter()
                .find(|i| i.label == name)
                .unwrap()
                .documentation
                .as_deref(),
            Some(doc)
        );
    }
}

#[test]
fn empty_comments_and_shadowing_do_not_inherit_docs() {
    let text = "## Function docs.\nfn same() -> Int { 1 }\n##   \nfn blank() -> Int { 0 }\nfn main() -> Int {\n  same = blank()\n  same\n}\n";
    let fs = MemoryFiles::new([("/app/Pome.toml", ""), (MAIN, text)]);
    let (analysis, program) = checked(&fs, Dependencies::default());
    assert!(!analysis.has_errors());
    let file = analysis.sources.file_id(Path::new(MAIN)).unwrap();
    let offset = at(text, "  same\n", 2);
    let hover = Ide::hover(&program, &analysis, file, offset).unwrap();
    assert_eq!(hover.contents, "```emela\nsame: Int\n```");
    let items = Ide::complete(&program, &analysis, file, offset);
    for name in ["same", "blank", "fn"] {
        assert_eq!(
            items
                .iter()
                .find(|i| i.label == name)
                .unwrap()
                .documentation,
            None,
            "{name}"
        );
    }
    let sig = Ide::signature_help(&program, &analysis, file, at(text, "blank()\n", 6)).unwrap();
    assert_eq!(sig.documentation, None);
}
