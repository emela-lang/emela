//! Syntax-only structure and navigation through partial checking results.

use std::path::Path;

use emela_driver::{
    Analysis, Dependencies, DependencyPome, DependencyRef, FoldKind, Frontend, Input, MemoryFiles,
    NavTarget, TypedProgram, check, folding_ranges, selection_ranges, workspace_symbols,
};
use line_index::{TextRange, TextSize};

type Typecheck = emela_driver::Typecheck;

fn checked(files: &[(&str, &str)]) -> (Analysis, TypedProgram) {
    let fs = MemoryFiles::new(files.iter().copied());
    let input = Input::resolve(&fs, Path::new("/app")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut Typecheck::default());
    (analysis, program.unwrap())
}

fn at(text: &str, needle: &str) -> TextSize {
    TextSize::try_from(text.find(needle).unwrap()).unwrap()
}

fn spelling<'a>(analysis: &'a Analysis, target: &NavTarget) -> &'a str {
    &analysis.sources[target.file].text()[std::ops::Range::<usize>::from(target.range)]
}

#[test]
fn highlights_respect_binding_identity_and_file() {
    let text = "import Data.{identity}\nfn use_it(value: Int) {\n  identity(value)\n  value = value + 1\n  value\n}\n";
    let (analysis, program) = checked(&[
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", text),
        (
            "/app/src/data.emel",
            "pub fn identity(value: Int) { value }\n",
        ),
    ]);
    let file = analysis
        .sources
        .file_id(Path::new("/app/src/main.emel"))
        .unwrap();
    let ranges = Typecheck::document_highlights(&program, &analysis, file, at(text, "value: Int"));
    assert_eq!(ranges.len(), 3);
    assert!(
        ranges
            .iter()
            .all(|r| &text[std::ops::Range::<usize>::from(*r)] == "value")
    );
    let rebound = at(text, "value =");
    let ranges = Typecheck::document_highlights(&program, &analysis, file, rebound);
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0].start(), rebound);
    assert_eq!(
        Typecheck::document_highlights(&program, &analysis, file, at(text, "identity(value)"))
            .len(),
        2
    );
}

#[test]
fn nominal_types_and_trait_implementations_survive_errors() {
    let text = "import Data.{Boxed, Choice, One, Speak}\nfn use_it(item: Boxed, choice: Choice) {\n  copy = item\n  copy\n  choice\n}\nfn broken() -> Int { missing }\nfn call(item: Boxed) { Speak.speak(item) }\n";
    let data = "pub type Boxed(value: Int)\npub enum Choice { One }\npub trait Speak {\n  fn speak(self) -> String\n  fn fallback(self) -> String { \"default\" }\n}\nimpl Boxed: Speak {\n  fn speak(self) { \"boxed\" }\n}\nimpl Choice: Speak {\n  fn speak(self) { \"choice\" }\n}\n";
    let (analysis, program) = checked(&[
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", text),
        ("/app/src/data.emel", data),
    ]);
    assert!(analysis.has_errors());
    let file = analysis
        .sources
        .file_id(Path::new("/app/src/main.emel"))
        .unwrap();
    for (needle, expected) in [
        ("item: Boxed", "Boxed"),
        ("copy\n", "Boxed"),
        ("choice\n", "Choice"),
    ] {
        let target =
            Typecheck::type_definition(&program, &analysis, file, at(text, needle)).unwrap();
        assert_eq!(spelling(&analysis, &target), expected);
    }
    assert!(Typecheck::type_definition(&program, &analysis, file, at(text, "Int")).is_none());
    let targets = Typecheck::implementations(&program, &analysis, file, at(text, "Speak.speak"));
    assert_eq!(targets.len(), 2);
    assert!(
        targets
            .iter()
            .all(|t| spelling(&analysis, t).contains(": Speak"))
    );
    let targets = Typecheck::implementations(&program, &analysis, file, at(text, "speak(item)"));
    assert_eq!(targets.len(), 2);
    assert!(targets.iter().all(|t| spelling(&analysis, t) == "speak"));
    let data_file = analysis
        .sources
        .file_id(Path::new("/app/src/data.emel"))
        .unwrap();
    let self_at = at(data, "self) { \"boxed\"");
    let target = Typecheck::type_definition(&program, &analysis, data_file, self_at).unwrap();
    assert_eq!(spelling(&analysis, &target), "Boxed");
    let ranges = Typecheck::document_highlights(&program, &analysis, data_file, self_at);
    assert_eq!(ranges, [TextRange::at(self_at, TextSize::from(4))]);
    let body = at(data, "\"boxed\"");
    assert!(Typecheck::document_highlights(&program, &analysis, data_file, body).is_empty());
    assert_eq!(
        Typecheck::implementations(&program, &analysis, data_file, at(data, "fallback(self)"))
            .len(),
        2
    );
}

#[test]
fn expression_type_definition_uses_the_outer_nominal_type() {
    let text = "type Boxed[A](value: A)\nfn make() { Boxed(value: 1) }\nfn use_it() { make() }\n";
    let (analysis, program) = checked(&[("/app/Pome.toml", ""), ("/app/src/main.emel", text)]);
    assert!(!analysis.has_errors());
    let file = analysis
        .sources
        .file_id(Path::new("/app/src/main.emel"))
        .unwrap();
    let offset = at(text, "make() }") + TextSize::from(5);
    let target = Typecheck::type_definition(&program, &analysis, file, offset).unwrap();
    assert_eq!(spelling(&analysis, &target), "Boxed");
}

#[test]
fn workspace_symbols_exclude_dependencies_and_core() {
    let fs = MemoryFiles::new([
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", "fn main() { () }\n"),
        (
            "/app/src/unopened.emel",
            "type Boxed(value: Int)\nfn broken() { missing }\n",
        ),
        ("/deps/lib/src/lib.emel", "pub type Hidden(value: Int)\n"),
    ]);
    let mut input = Input::resolve(&fs, Path::new("/app")).unwrap();
    input.dependencies = Dependencies {
        pomes: vec![DependencyPome {
            name: "example.com/lib".into(),
            dir: "/deps/lib".into(),
            last_element: "lib".into(),
            module: None,
            emela: None,
            targets: None,
            version: "v1.0.0".into(),
            dependencies: Vec::new(),
        }],
        project: vec![DependencyRef {
            pome: 0,
            module: None,
        }],
        ..Dependencies::default()
    };
    let (analysis, _) = check(&fs, &input, &mut Typecheck::default());
    let symbols = workspace_symbols(&analysis, "");
    assert_eq!(
        symbols.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["main", "Boxed", "value", "broken"]
    );
    assert_eq!(workspace_symbols(&analysis, "oXe")[0].name, "Boxed");
    assert!(workspace_symbols(&analysis, "Hidden").is_empty());
    assert!(workspace_symbols(&analysis, "no_such_symbol").is_empty());
}

#[test]
fn folds_bodies_records_imports_and_comments_without_checking() {
    let text = "import First\nimport Second\n\n# first\n## second\ntype User(\n  id: Int,\n  name: String,\n)\nenum Choice {\n  One\n  Two\n}\nfn broken() {\n  missing match {\n    _ -> {\n      unknown\n    }\n  }\n}\nfn unfinished(\n";
    let folds = folding_ranges(text);
    assert_eq!(
        folds.iter().filter(|f| f.kind == FoldKind::Imports).count(),
        1
    );
    assert_eq!(
        folds.iter().filter(|f| f.kind == FoldKind::Comment).count(),
        1
    );
    let regions: Vec<_> = folds
        .iter()
        .filter(|f| f.kind == FoldKind::Region)
        .map(|f| &text[std::ops::Range::<usize>::from(f.range)])
        .collect();
    assert_eq!(regions.len(), 5, "{regions:?}");
    assert!(regions.iter().any(|r| r.starts_with("(\n  id:")));
    assert!(folding_ranges("# one\n\n# two\nfn empty() {}\n").is_empty());
    assert!(folding_ranges("const TEXT = \"# not a comment\"\n").is_empty());
}

#[test]
fn selections_are_strictly_nested_and_handle_empty_or_broken_files() {
    let text = "fn broken() {\n  # comment\n  (unknown + 1)\n";
    let offsets = [
        at(text, "unknown"),
        TextSize::of(text),
        at(text, "# comment"),
    ];
    let chains = selection_ranges(text, &offsets);
    assert_eq!(chains.len(), offsets.len());
    for (chain, offset) in chains.iter().zip(offsets) {
        assert!(!chain.is_empty());
        assert!(chain.iter().all(|r| r.contains_inclusive(offset)));
        assert!(
            chain
                .windows(2)
                .all(|w| w[0] != w[1] && w[1].contains_range(w[0]))
        );
        assert_eq!(chain.last(), Some(&TextRange::up_to(TextSize::of(text))));
    }
    assert_eq!(
        selection_ranges("", &[0.into()]),
        [vec![TextRange::empty(0.into())]]
    );
}

/// A pattern in place of a lambda parameter (spec 6.1): its names are hovered and completed like
/// bound names, the hidden parameter is not completed, and the pattern gets one type hint.
#[test]
fn lambda_parameter_patterns_in_ide_queries() {
    let text = "fn main() -> Int {\n  List.fold([(1, True)], from: 0, f: fn(acc, (count, _)) {\n    acc + count\n  })\n}\n";
    let (analysis, program) = checked(&[("/app/Pome.toml", ""), ("/app/src/main.emel", text)]);
    let file = analysis
        .sources
        .file_id(Path::new("/app/src/main.emel"))
        .unwrap();
    let hover = |needle: &str| {
        Typecheck::hover(&program, &analysis, file, at(text, needle))
            .unwrap()
            .contents
    };
    assert!(
        hover("count, _").contains("count: Int"),
        "{}",
        hover("count, _")
    );
    assert!(
        hover("(count").contains("(count, _): (Int, Bool)"),
        "{}",
        hover("(count")
    );

    let labels: Vec<String> =
        Typecheck::complete(&program, &analysis, file, at(text, "acc + count"))
            .into_iter()
            .map(|c| c.label)
            .collect();
    assert!(labels.iter().any(|l| l == "count"), "{labels:?}");
    assert!(labels.iter().any(|l| l == "acc"), "{labels:?}");
    assert!(!labels.iter().any(|l| l.starts_with('(')), "{labels:?}");

    let range = TextRange::up_to(TextSize::of(text));
    let hints: Vec<(TextSize, String)> = Typecheck::inlay_hints(&program, &analysis, file, range)
        .into_iter()
        .map(|h| (h.offset, h.label))
        .collect();
    let after_acc = at(text, ", (count");
    let after_pattern = at(text, ")) {") + TextSize::from(1);
    assert_eq!(
        hints,
        [
            (after_acc, ": Int".to_owned()),
            (after_pattern, ": (Int, Bool)".to_owned())
        ]
    );
}
