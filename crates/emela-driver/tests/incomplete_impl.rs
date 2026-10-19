//! Required trait methods are checked even when an impl is never used.

use std::path::Path;

use emela_driver::{Analysis, Input, Location, MemoryFiles, Typecheck, check, code, render};

fn checked(main: &str, extra: &[(&str, &str)]) -> Analysis {
    let mut files = vec![("/app/Pome.toml", ""), ("/app/src/main.emel", main)];
    files.extend_from_slice(extra);
    let fs = MemoryFiles::new(files);
    let input = Input::resolve(&fs, Path::new("/app")).unwrap();
    check(&fs, &input, &mut Typecheck::default()).0
}

#[test]
fn reports_every_required_method_in_declaration_order_in_the_impl_file() {
    let main = "import Labels.{Label}\ntype User(id: Int)\nimpl User: Label {}\n";
    let analysis = checked(
        main,
        &[(
            "/app/src/labels.emel",
            "pub trait Label {\n  fn zebra(self) -> String\n  fn default(self) -> Int { 1 }\n  fn alpha(self, other: Self) -> Self\n  fn create() -> Self\n}\n",
        )],
    );
    assert_eq!(analysis.error_count(), 1, "{analysis:?}");
    let diagnostic = &analysis.diagnostics[0];
    assert_eq!(diagnostic.code, Some(code::INCOMPLETE_IMPL));
    assert_eq!(
        diagnostic.message,
        "incomplete impl: missing methods `zebra`, `alpha`, `create`"
    );
    let Some(Location::Span(span)) = diagnostic.location else {
        panic!("impl diagnostic must have a source span")
    };
    assert_eq!(
        analysis.sources[span.file].path(),
        Path::new("/app/src/main.emel")
    );
    assert_eq!(&main[span.range], "impl User: Label");
}

#[test]
fn diagnostic_excludes_the_body_and_trailing_header_whitespace() {
    let main = "type Box[A](value: A)\ntrait Keep {\n  fn keep(self) -> Self\n  fn create() -> Int\n}\nimpl Box[A]: Keep \t {\n  fn keep(self) { self }\n}\n";
    let analysis = checked(main, &[]);
    assert_eq!(analysis.error_count(), 1, "{analysis:?}");
    let Some(Location::Span(span)) = analysis.diagnostics[0].location else {
        panic!("impl diagnostic must have a source span")
    };
    assert_eq!(&main[span.range], "impl Box[A]: Keep");
}

#[test]
fn complete_and_default_only_impls_are_valid() {
    let analysis = checked(
        "type Box[A](value: A)\ntrait Keep {\n  fn keep(self) -> Self\n  fn create() -> Int\n  fn default(self) -> Int { 1 }\n}\nimpl Box[A]: Keep {\n  fn keep(self) { self }\n  fn create() { 1 }\n}\ntrait DefaultOnly { fn value(self) -> Int { 1 } }\nimpl Int: DefaultOnly {}\n",
        &[],
    );
    assert!(
        !analysis.has_errors(),
        "{}",
        render(&analysis.diagnostics, &analysis.sources, false)
    );
}

#[test]
fn derives_and_builtin_implementations_do_not_require_source_methods() {
    let analysis = checked(
        "type User(id: Int) derive Eq, Ord, Show\nenum Choice { First, Second(Int) } derive Eq, Ord, Show\nfn main() {\n  assert 1 == 1\n  assert 1 < 2\n  assert True == True\n  assert Some(1) == Some(1)\n  dbg(\"#{User(id: 1)} #{First}\")\n}\n",
        &[],
    );
    assert!(
        !analysis.has_errors(),
        "{}",
        render(&analysis.diagnostics, &analysis.sources, false)
    );
}

/// Hash is not a Prelude trait (spec 10.6, 15.1).
#[test]
fn hash_is_not_a_prelude_trait() {
    let analysis = checked("type User(id: Int) derive Hash\n", &[]);
    let found: Vec<_> = analysis
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.as_str()))
        .collect();
    assert_eq!(
        found,
        [(Some("E0211"), "cannot find trait `Hash` in this scope")]
    );
}

#[test]
fn unresolved_traits_do_not_produce_incomplete_impl_cascades() {
    let analysis = checked("type User(id: Int)\nimpl User: Missing {}\n", &[]);
    assert!(analysis.has_errors());
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| d.code != Some(code::INCOMPLETE_IMPL))
    );
}
