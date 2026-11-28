//! Code actions: inferred annotations, quickfixes and the rewriting of old forms.

use emela_driver::Typecheck;
use lsp_types as lsp;
use lsp_types::request::{CodeActionRequest, Request as _};
use serde_json::json;

use super::{Client, MAIN, codes, range, uri};

/// Inserts inferred results into omitted annotations (spec 4.5).
#[test]
fn code_action_inserts_inferred_annotations() {
    let main = "import Errs.{NotFound}
import Other

effect Logger {
  fn info(msg: String) -> ()
}

fn find(id: Int) {
  if id > 0 { id } else { fail NotFound(id:) }
}

fn greet() {
  log = use Logger
  log.info(\"hi\")
}

fn both(id: Int) -> Int {
  (use Logger).info(\"x\")
  id
}

fn other() {
  fail Other.Hidden(n: 1)
}

fn apply[A](f: fn() -> A) {
  f()
}
";
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", main),
        ("/app/src/errs.emel", "pub error NotFound(id: Int)\n"),
        ("/app/src/other.emel", "pub error Hidden(n: Int)\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert!(emela_lsp::capabilities().code_action_provider.is_some());
    let action = |title: &str, at: (u32, u32), text: &str| {
        Some(vec![(
            title.to_owned(),
            vec![(range(at, at), text.to_owned())],
        )])
    };
    // The return type and the inferred E are added together.
    assert_eq!(
        client.code_actions(MAIN, (7, 4), (7, 4)),
        action(
            "Add inferred `-> Int fails NotFound`",
            (7, 16),
            " -> Int fails NotFound"
        )
    );
    // Inferred R.
    assert_eq!(
        client.code_actions(MAIN, (11, 0), (11, 0)),
        action(
            "Add inferred `-> () uses { Logger }`",
            (11, 10),
            " -> () uses { Logger }"
        )
    );
    // With a return type present, only `use` is added, after the return type.
    assert_eq!(
        client.code_actions(MAIN, (16, 5), (16, 5)),
        action(
            "Add inferred `uses { Logger }`",
            (16, 23),
            " uses { Logger }"
        )
    );
    // Errors that cannot be written unqualified are not added.
    assert_eq!(client.code_actions(MAIN, (21, 4), (21, 4)), Some(vec![]));
    // The invisible sets of function arguments are not written (they are implicitly added to
    // written clauses).
    assert_eq!(
        client.code_actions(MAIN, (25, 4), (25, 4)),
        action("Add inferred `-> A`", (25, 25), " -> A")
    );
    // Not offered inside a body.
    assert_eq!(client.code_actions(MAIN, (8, 4), (8, 4)), Some(vec![]));
    client.shutdown();
}

#[test]
fn quickfixes_link_only_the_diagnostic_at_the_requested_range() {
    let main = "enum Color { Red, Blue }\nfn color(x: Color) -> Int { x match { Red -> 1 } }\nfn first(count: Int) -> Int { cout }\nfn second(total: Int) -> Int { tota }\ntrait Show { fn show(self) -> String }\nimpl Color: Show {}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(
        codes(&published),
        ["E2101", "E2101", "W2102", "E3102", "E3301"]
    );
    let capabilities = emela_lsp::capabilities();
    let Some(lsp::CodeActionProviderCapability::Options(options)) =
        capabilities.code_action_provider
    else {
        panic!("code action options")
    };
    assert!(
        options
            .code_action_kinds
            .unwrap()
            .contains(&lsp::CodeActionKind::QUICKFIX)
    );

    for (at, code, inserted) in [
        ((1, 28), "E3102", "Blue -> todo()"),
        ((2, 30), "E2101", "count"),
        ((3, 31), "E2101", "total"),
        ((5, 1), "E3301", "fn show(self) { todo() }"),
    ] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1, "at {at:?}: {actions:?}");
        let action = &actions[0];
        let diagnostic = &action.diagnostics.as_ref().unwrap()[0];
        assert_eq!(
            diagnostic.code,
            Some(lsp::NumberOrString::String(code.to_owned()))
        );
        assert!(diagnostic.range.start <= lsp::Position::new(at.0, at.1));
        assert!(diagnostic.range.end > lsp::Position::new(at.0, at.1));
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits.len(), 1);
        assert!(edits[0].new_text.contains(inserted));
    }
    assert!(client.quickfixes(MAIN, (1, 0), (1, 0)).is_empty());
    assert_eq!(client.quickfixes(MAIN, (1, 0), (6, 0)).len(), 4);
    // The snapshot is rechecked even when the client echoes no diagnostics.
    client.open(MAIN, 1, main);
    client.change(MAIN, 2, &main.replace("cout", "count"));
    assert!(client.quickfixes(MAIN, (2, 30), (2, 30)).is_empty());
    client.shutdown();
}

#[test]
fn impl_stubs_are_quickfixes_linked_to_published_diagnostics() {
    let main = "type User(id: Int)\ntrait Label {\n  fn show(self) -> String\n  fn merge(self, other: Self) -> Self\n  fn default(self) -> Int { 1 }\n}\nimpl User: Label {\n  fn show(self) { \"user\" }\n}\nimpl Int: Label {}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E3301", "E3301"]);
    assert_eq!(published.diagnostics[0].range, range((6, 0), (6, 16)));
    assert_eq!(published.diagnostics[1].range, range((9, 0), (9, 15)));
    let id = client.request(
        CodeActionRequest::METHOD,
        json!({
            "textDocument": { "uri": uri(MAIN) },
            "range": range((6, 0), (6, 0)),
            "context": { "diagnostics": [] },
        }),
    );
    let actions: Vec<lsp::CodeAction> =
        serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].kind, Some(lsp::CodeActionKind::QUICKFIX));
    // The old header refactoring is not offered alongside the quickfix.
    let id = client.request(
        CodeActionRequest::METHOD,
        json!({
            "textDocument": { "uri": uri(MAIN) },
            "range": range((6, 0), (6, 0)),
            "context": { "diagnostics": [], "only": ["refactor.rewrite"] },
        }),
    );
    let actions: Vec<lsp::CodeAction> =
        serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
    assert!(actions.is_empty());
    assert_eq!(
        published.diagnostics[0].message,
        "incomplete impl: missing methods `merge`"
    );
    assert_eq!(
        published.diagnostics[1].message,
        "incomplete impl: missing methods `show`, `merge`"
    );
    for at in [(6, 0), (6, 5), (6, 15)] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].title, "Implement missing trait methods");
        assert_eq!(
            actions[0].diagnostics.as_ref().unwrap(),
            &vec![published.diagnostics[0].clone()]
        );
        let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "\n  fn merge(self, other) { todo() }");
    }
    assert_eq!(client.quickfixes(MAIN, (6, 5), (6, 18)).len(), 1);
    for at in [(6, 19), (6, 20), (6, 21), (7, 2), (7, 18), (8, 0), (8, 1)] {
        assert!(client.quickfixes(MAIN, at, at).is_empty(), "at {at:?}");
    }
    assert!(client.quickfixes(MAIN, (6, 20), (8, 1)).is_empty());
    let actions = client.quickfixes(MAIN, (9, 0), (9, 0));
    assert_eq!(actions.len(), 1);
    let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert!(edits[0].new_text.contains("fn show(self) { todo() }"));
    assert!(
        edits[0]
            .new_text
            .contains("fn merge(self, other) { todo() }")
    );
    assert!(!edits[0].new_text.contains("fn default"));
    assert_eq!(client.quickfixes(MAIN, (6, 0), (10, 0)).len(), 2);
    client.open(MAIN, 1, main);
    client.change(
        MAIN,
        2,
        &main.replace(
            "fn merge(self, other: Self) -> Self",
            "fn merge(self, other: Self) -> Self { self }",
        ),
    );
    assert!(client.quickfixes(MAIN, (6, 0), (6, 0)).is_empty());
    client.shutdown();
}

#[test]
fn quickfixes_insert_imports_and_extend_effect_and_error_annotations() {
    let main = "error Missing\neffect Log { fn log() -> () }\nfn io() -> () { Io.print(\"hi\") }\nfn fails() -> () uses {} { fail Missing }\nfn effects() -> () uses {} {\n  log <- Log\n  log.log()\n}\n";
    let files = [
        ("/app/Pome.toml", ""),
        (MAIN, main),
        ("/app/src/io.emel", "pub fn print(s: String) -> () {}\n"),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E2101", "E3201", "E4301"]);
    for (at, code, inserted) in [
        ((2, 16), "E2101", "import Io\n"),
        ((3, 27), "E3201", " fails Missing"),
        ((6, 4), "E4301", "uses { Log }"),
    ] {
        let actions = client.quickfixes(MAIN, at, at);
        assert_eq!(actions.len(), 1, "at {at:?}: {actions:?}");
        let action = &actions[0];
        assert_eq!(
            action.diagnostics.as_ref().unwrap()[0].code,
            Some(lsp::NumberOrString::String(code.to_owned()))
        );
        let edits = &action.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits[0].new_text, inserted);
    }
    client.shutdown();
}

/// The old impl and handler heads come with quick fixes that rewrite them (E1402, E1403).
#[test]
fn old_forms_have_rewriting_quickfixes() {
    let main = "trait Label { fn label(self) -> String }\nimpl Label for Int {\n  fn label(self) { \"n\" }\n}\neffect Log { fn log() -> () }\nhandler Quiet implements Log {\n  fn log() { () }\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E1402", "E1403"]);
    for ((line, character), expected) in [((1, 0), "impl Int: Label"), ((5, 14), ":")] {
        let actions = client.quickfixes(MAIN, (line, character), (line, character));
        assert_eq!(actions.len(), 1, "at {line}:{character}");
        let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
        assert_eq!(edits[0].new_text, expected);
    }
    client.shutdown();
}

/// The old form of an anonymous function comes with a quick fix that rewrites it (E1405,
/// `[fn.decl]`), and the inlay hint of a parameter type stays on the new form (`|x: Int|`).
#[test]
fn old_lambda_has_a_rewriting_quickfix() {
    let main = "fn f(xs: List[Int]) -> List[Int] {\n  List.map(xs, fn(x) { x + 1 })\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["E1405"]);
    let actions = client.quickfixes(MAIN, (1, 15), (1, 15));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Rewrite as `|x| ...`");
    let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert_eq!(edits[0].new_text, "|x| x + 1");
    client.shutdown();
}

/// A guard written after `if` is published as a warning (W1401) with a quick fix that replaces
/// `if` with `where` (`[lsp.code-action]`).
#[test]
fn guard_after_if_has_a_rewriting_quickfix() {
    let main = "fn f(o: Option[Int]) -> Int {\n  o match {\n    Some(x) if x > 0 -> x\n    _                -> 0\n  }\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, main)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let published = client.diagnostics();
    assert_eq!(codes(&published), ["W1401"]);
    assert_eq!(
        published.diagnostics[0].severity,
        Some(lsp::DiagnosticSeverity::WARNING)
    );
    let actions = client.quickfixes(MAIN, (2, 12), (2, 12));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].title, "Replace `if` with `where`");
    let edits = &actions[0].edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri(MAIN)];
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].new_text, "where");
    assert_eq!(edits[0].range.start, lsp::Position::new(2, 12));
    assert_eq!(edits[0].range.end, lsp::Position::new(2, 14));
    client.shutdown();
}
