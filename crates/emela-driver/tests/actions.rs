//! Code actions use the current snapshot, not client-supplied diagnostics.

use std::path::Path;

use emela_driver::{
    Analysis, CodeAction, CodeActionKind, FileId, Frontend, Input, MemoryFiles, TypedProgram,
    check, code,
};
use line_index::{TextRange, TextSize};

type Typecheck = emela_driver::Typecheck;

fn checked(text: &str, extra: &[(&str, &str)]) -> (Analysis, TypedProgram, FileId) {
    let mut files = vec![("/app/Pome.toml", ""), ("/app/src/main.emel", text)];
    files.extend_from_slice(extra);
    let fs = MemoryFiles::new(files);
    let input = Input::resolve(&fs, Path::new("/app")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut Typecheck::default());
    let file = analysis
        .sources
        .file_id(Path::new("/app/src/main.emel"))
        .unwrap();
    (analysis, program.unwrap(), file)
}

fn actions(
    text: &str,
    needle: &str,
    extra: &[(&str, &str)],
    kind: CodeActionKind,
) -> Vec<CodeAction> {
    let (analysis, program, file) = checked(text, extra);
    let at = TextSize::try_from(text.find(needle).unwrap()).unwrap();
    Typecheck::code_actions(&program, &analysis, file, TextRange::empty(at))
        .into_iter()
        .filter(|a| a.kind == kind)
        .collect()
}

fn fixes(text: &str, needle: &str, extra: &[(&str, &str)]) -> Vec<CodeAction> {
    actions(text, needle, extra, CodeActionKind::QuickFix)
}

fn refactors(text: &str, needle: &str) -> Vec<CodeAction> {
    actions(text, needle, &[], CodeActionKind::RefactorRewrite)
}

fn apply(text: &str, action: &CodeAction) -> String {
    let mut edits = action.edits.clone();
    edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start()));
    let mut result = text.to_owned();
    for (range, inserted) in edits {
        result.replace_range(std::ops::Range::<usize>::from(range), &inserted);
    }
    result
}

fn assert_clean(text: &str, extra: &[(&str, &str)]) {
    let (analysis, _, _) = checked(text, extra);
    assert!(
        !analysis.has_errors(),
        "{}",
        emela_driver::render(&analysis.diagnostics, &analysis.sources, false)
    );
}

#[test]
fn match_arms_follow_coverage_including_guards_and_nested_patterns() {
    let text = "enum Shape { Circle(Int), Rect(Int, Int), Empty }\nfn area(s: Shape) -> Int {\n  s match {\n    Circle(_) if False -> 1\n    Rect(_, _) -> 2\n  }\n}\n";
    let actions = fixes(text, "s match", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0].diagnostics[0].code,
        Some(code::NON_EXHAUSTIVE_MATCH)
    );
    assert_eq!(
        actions[0].edits[0].1,
        "\n    Circle(_) -> todo()\n    Empty -> todo()"
    );
    assert_clean(&apply(text, &actions[0]), &[]);

    let text = "fn f(x: Option[Option[Int]]) -> Int { x match { Some(Some(_)) -> 1 } }\n";
    let actions = fixes(text, "x match", &[]);
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edits[0].1.contains("Some(None) -> todo()"));
    assert_clean(&apply(text, &actions[0]), &[]);
}

#[test]
fn qualified_witnesses_and_trailing_comments() {
    let extra = [("/app/src/colors.emel", "pub enum Color { Red, Blue }\n")];
    let text = "import Colors\nfn f(x: Colors.Color) -> Int {\n  x match {\n    Colors.Red -> 1 # keep this comment\n  }\n}\n";
    let actions = fixes(text, "x match", &extra);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].edits[0].1, "\n    Colors.Blue -> todo()");
    let edited = apply(text, &actions[0]);
    assert!(edited.contains("Colors.Red -> 1 # keep this comment\n    Colors.Blue"));
    assert_clean(&edited, &extra);

    let text = text.replace('\n', "\r\n");
    let actions = fixes(&text, "x match", &extra);
    assert!(actions[0].edits[0].1.starts_with("\r\n"));
    assert_clean(&apply(&text, &actions[0]), &extra);
}

#[test]
fn witness_limit_and_ambiguous_constructor_names_use_a_fallback() {
    let cases = (0..70)
        .map(|i| format!("Case{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!("enum Many {{ {cases} }}\nfn f(x: Many) -> Int {{ x match {{}} }}\n");
    let actions = fixes(&text, "x match", &[]);
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edits[0].1.contains("_ -> todo()"));
    assert_clean(&apply(&text, &actions[0]), &[]);

    let extra = [
        ("/app/src/colors.emel", "pub enum Color { Red, Blue }\n"),
        ("/app/src/shades.emel", "pub enum Shade { Blue, Dark }\n"),
    ];
    let text = "import Colors\nimport Shades\nfn f(x: (Colors.Color, Shades.Shade)) -> Int { x match { (Colors.Red, Shades.Dark) -> 1 } }\n";
    let actions = fixes(text, "x match", &extra);
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edits[0].1.contains("_ -> todo()"));
    assert_clean(&apply(text, &actions[0]), &extra);
}

#[test]
fn empty_match_and_catch_all() {
    let text = "enum Color { Red, Blue }\nfn f(x: Color) -> Int { x match {} }\n";
    let actions = fixes(text, "x match", &[]);
    assert_eq!(actions.len(), 1);
    assert_clean(&apply(text, &actions[0]), &[]);
    assert!(fixes("fn f(x: Int) -> Int { x match { _ -> 1 } }", "x match", &[]).is_empty());
}

#[test]
fn impl_stubs_omit_types_and_default_methods() {
    let text = "type User(id: Int)\ntrait Show {\n  fn show(self) -> String\n  fn merge(self, other: Self) -> Self\n  fn default(self) -> Int { 1 }\n}\nimpl User: Show {}\n";
    let (analysis, _, _) = checked(text, &[]);
    assert_eq!(analysis.error_count(), 1);
    assert_eq!(
        analysis
            .diagnostics
            .iter()
            .find(|d| d.code == Some(code::INCOMPLETE_IMPL))
            .unwrap()
            .message,
        "incomplete impl: missing methods `show`, `merge`"
    );
    assert!(refactors(text, "impl User: Show").is_empty());
    let actions = fixes(text, "impl User: Show", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].diagnostics[0].code, Some(code::INCOMPLETE_IMPL));
    let edited = apply(text, &actions[0]);
    assert!(edited.contains("fn show(self) { todo() }"));
    assert!(edited.contains("fn merge(self, other) { todo() }"));
    assert_clean(&edited, &[]);
    assert!(fixes(&edited, "impl User: Show", &[]).is_empty());

    let text = text.replace(
        "impl User: Show {}",
        "impl User: Show {\n  fn show(self) { \"user\" }\n}",
    );
    let actions = fixes(&text, "impl User: Show", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0].edits[0].1,
        "\n  fn merge(self, other) { todo() }"
    );
    assert_clean(&apply(&text, &actions[0]), &[]);
}

#[test]
fn impl_quickfixes_follow_diagnostic_ranges_and_missing_non_default_methods() {
    let text = "type User(id: Int)\ntrait Show {\n  fn show(self) -> String\n  fn merge(self, other: Self) -> Self\n}\nimpl User: Show {\n  fn show(self) { \"user\" }\n}\nimpl Int: Show {}\n";
    let (analysis, program, file) = checked(text, &[]);
    assert_eq!(analysis.error_count(), 2);
    let start = text.find("impl User: Show").unwrap();
    let open = start + text[start..].find('{').unwrap();
    let close = open + text[open..].find("\n}\nimpl").unwrap() + 1;
    for at in [start, start + 5, open - 2] {
        let at = TextSize::try_from(at).unwrap();
        let actions = Typecheck::code_actions(&program, &analysis, file, TextRange::empty(at));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, CodeActionKind::QuickFix);
        assert_eq!(actions[0].diagnostics[0].code, Some(code::INCOMPLETE_IMPL));
        assert_eq!(
            actions[0].edits[0].1,
            "\n  fn merge(self, other) { todo() }"
        );
    }
    let header_range = TextRange::new(
        TextSize::try_from(start + 5).unwrap(),
        TextSize::try_from(open).unwrap(),
    );
    assert_eq!(
        Typecheck::code_actions(&program, &analysis, file, header_range).len(),
        1
    );
    for at in [
        open - 1,
        open,
        open + 1,
        text.find("fn show(self) { \"user\"").unwrap(),
        close,
    ] {
        let at = TextSize::try_from(at).unwrap();
        assert!(
            Typecheck::code_actions(&program, &analysis, file, TextRange::empty(at)).is_empty()
        );
    }
    let body_range = TextRange::new(
        TextSize::try_from(open).unwrap(),
        TextSize::try_from(close + 1).unwrap(),
    );
    assert!(Typecheck::code_actions(&program, &analysis, file, body_range).is_empty());
    let end = TextSize::try_from(close + 1).unwrap();
    assert!(Typecheck::code_actions(&program, &analysis, file, TextRange::empty(end)).is_empty());
    let all = TextRange::up_to(TextSize::try_from(text.len()).unwrap());
    let actions = Typecheck::code_actions(&program, &analysis, file, all);
    assert_eq!(actions.len(), 2);
    assert!(actions[1].edits[0].1.contains("fn show(self) { todo() }"));
    assert!(
        actions[1].edits[0]
            .1
            .contains("fn merge(self, other) { todo() }")
    );
    assert_clean(
        "trait Show { fn show(self) -> String { \"ok\" } }\nimpl Int: Show {}\n",
        &[],
    );
    assert!(
        refactors(
            "trait Show { fn show(self) -> String { \"ok\" } }\nimpl Int: Show {}\n",
            "impl Int: Show"
        )
        .is_empty()
    );
}

#[test]
fn adds_module_and_effect_imports() {
    let extra = [("/app/src/io.emel", "pub fn print(s: String) -> () {}\n")];
    let text = "fn f() -> () { Io.print(\"hi\") }\n";
    let actions = fixes(text, "Io.print", &extra);
    let action = actions.iter().find(|a| a.title == "Import `Io`").unwrap();
    assert_eq!(action.edits[0].1, "import Io\n");
    assert_clean(&apply(text, action), &extra);

    let text = "fn f() { (use Console).print_line(\"hi\") }\n";
    let actions = fixes(text, "Console", &[]);
    let action = actions
        .iter()
        .find(|a| a.title == "Import `Std.Console.{Console}`")
        .unwrap();
    assert_clean(&apply(text, action), &[]);

    let extra = [("/app/src/log.emel", "pub effect Log { fn log() -> () }\n")];
    let text = "fn f() -> () { (use Log).log() }\n";
    let actions = fixes(text, "Log", &extra);
    let action = actions
        .iter()
        .find(|a| a.title == "Import `Log.{Log}`")
        .unwrap();
    assert_clean(&apply(text, action), &extra);
}

#[test]
fn extends_written_annotations_without_losing_existing_members() {
    let text = "error Alpha\nerror Beta\nfn f() -> () fails Alpha use {} { fail Beta }\n";
    let actions = fixes(text, "fail Beta", &[]);
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edits[0].1.contains("fails Alpha | Beta"));
    assert_clean(&apply(text, &actions[0]), &[]);

    let text = "error Beta\nfn f() -> () use {} { fail Beta }\n";
    let actions = fixes(text, "fail Beta", &[]);
    assert_eq!(actions.len(), 1);
    assert_clean(&apply(text, &actions[0]), &[]);

    let text = "effect Log { fn log() -> () }\neffect Clock { fn now() -> Int }\nfn f() -> Int use Log { (use Clock).now() }\n";
    let actions = fixes(text, "use Clock", &[]);
    assert_eq!(actions.len(), 1);
    assert!(actions[0].edits[0].1.contains("Log"));
    assert!(actions[0].edits[0].1.contains("Clock"));
    assert_clean(&apply(text, &actions[0]), &[]);
}

#[test]
fn effect_fixes_do_not_propagate_from_main_tests_or_constants() {
    let text = "effect Log { fn log() -> () }\nfn main() -> () { (use Log).log() }\n";
    assert!(fixes(text, "use Log", &[]).is_empty());
    let text = "effect Log { fn log() -> () }\n@test fn test_log() -> () { (use Log).log() }\n";
    assert!(fixes(text, "use Log", &[]).is_empty());
    let text = "effect Log { fn log() -> () }\nconst X = (use Log).log()\n";
    assert!(fixes(text, "use Log", &[]).is_empty());
}

#[test]
fn replacement_uses_edit_distance_and_lexical_scope() {
    let text = "fn f(count: Int) -> Int { cout }\nfn g() -> Int { cout }\n";
    let actions = fixes(text, "cout }", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].edits[0].1, "count");
    assert_eq!(&text[actions[0].edits[0].0], "cout");
    let (analysis, program, file) = checked(text, &[]);
    let at = TextSize::try_from(text.rfind("cout").unwrap()).unwrap();
    assert!(Typecheck::code_actions(&program, &analysis, file, TextRange::empty(at)).is_empty());
}

#[test]
fn diagnostics_from_other_files_do_not_leak_into_actions() {
    let text = "import Other\nfn f(count: Int) -> Int { cout }\n";
    let extra = [(
        "/app/src/other.emel",
        "pub fn g(total: Int) -> Int { tota }\n",
    )];
    let actions = fixes(text, "cout", &extra);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].edits[0].1, "count");
    let edited = apply(text, &actions[0]);
    assert!(fixes(&edited, "count }", &extra).is_empty());
}

#[test]
fn multiple_errors_are_filtered_by_file_range_and_code() {
    let text = "fn f(count: Int) -> Int { cout }\nfn g(total: Int) -> Int { tota }\n";
    let (mut analysis, program, file) = checked(text, &[]);
    assert_eq!(
        analysis
            .diagnostics
            .iter()
            .filter(|d| d.code == Some(code::UNDEFINED_NAME))
            .count(),
        2
    );
    let first = TextSize::try_from(text.find("cout").unwrap()).unwrap();
    let second = TextSize::try_from(text.find("tota }").unwrap()).unwrap();
    for (at, replacement) in [(first, "count"), (second, "total")] {
        let actions = Typecheck::code_actions(&program, &analysis, file, TextRange::empty(at));
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].diagnostics.len(), 1);
        assert_eq!(actions[0].edits[0].0.start(), at);
        assert_eq!(actions[0].edits[0].1, replacement);
    }
    let all = Typecheck::code_actions(
        &program,
        &analysis,
        file,
        TextRange::up_to(TextSize::try_from(text.len()).unwrap()),
    );
    assert_eq!(
        all.iter()
            .filter(|a| a.kind == CodeActionKind::QuickFix)
            .count(),
        2
    );
    for d in &mut analysis.diagnostics {
        d.code = None;
    }
    assert!(Typecheck::code_actions(&program, &analysis, file, TextRange::empty(first)).is_empty());
}

#[test]
fn old_impl_and_handler_heads_are_rewritten() {
    let text = "type User(id: Int)\ntrait Label {\n  fn label(self) -> String\n}\nimpl Label for User {\n  fn label(self) { \"user\" }\n}\n";
    let actions = fixes(text, "for User", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Rewrite as `impl User: Label`");
    assert_eq!(actions[0].diagnostics[0].code, Some("E0144"));
    assert_clean(&apply(text, &actions[0]), &[]);

    let text = "type Box[A](value: A)\ntrait Label {\n  fn label(self) -> String\n}\nimpl[A: Label] Box[A]: Label {\n  fn label(self) { \"box\" }\n}\n";
    let actions = fixes(text, "impl[", &[]);
    assert_eq!(actions.len(), 1);
    let edited = apply(text, &actions[0]);
    assert!(edited.contains("impl Box[A: Label]: Label {"));
    assert_clean(&edited, &[]);

    let text = "effect Logger {\n  fn info(msg: String) -> ()\n}\nhandler Quiet(level: Int) implements Logger {\n  init { Quiet(level: 0) }\n  fn info(msg) { () }\n}\n";
    let actions = fixes(text, "implements", &[]);
    assert_eq!(actions.len(), 1);
    assert_eq!(
        actions[0].title,
        "Rewrite as `handler Quiet(level: Int): Logger`"
    );
    assert_eq!(actions[0].diagnostics[0].code, Some("E0145"));
    let edited = apply(text, &actions[0]);
    assert!(edited.contains("handler Quiet(level: Int): Logger {"));
    assert_clean(&edited, &[]);
}
