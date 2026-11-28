//! Navigation: definitions, references, highlights, symbols, document links and rename.

use std::time::Duration;

use emela_driver::{ParseOnly, Typecheck};
use lsp_server::Message;
use lsp_types as lsp;
use serde_json::json;

use super::{Client, MAIN, NAV_LOG, NAV_MAIN, NAV_SHAPES, labels, location, nav_files, range, uri};

fn navigation<T: serde::de::DeserializeOwned>(
    client: &mut Client,
    method: &str,
    params: serde_json::Value,
) -> T {
    let id = client.request(method, params);
    let response = client.response(&id);
    serde_json::from_value(response.response_result.unwrap()).unwrap()
}

#[test]
fn navigation_capabilities() {
    let caps = emela_lsp::capabilities();
    assert_eq!(
        caps.document_highlight_provider,
        Some(lsp::OneOf::Left(true))
    );
    assert_eq!(caps.workspace_symbol_provider, Some(lsp::OneOf::Left(true)));
    assert_eq!(
        caps.folding_range_provider,
        Some(lsp::FoldingRangeProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.selection_range_provider,
        Some(lsp::SelectionRangeProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.type_definition_provider,
        Some(lsp::TypeDefinitionProviderCapability::Simple(true))
    );
    assert_eq!(
        caps.implementation_provider,
        Some(lsp::ImplementationProviderCapability::Simple(true))
    );
}

#[test]
fn highlights_are_same_file_and_include_the_binding() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let highlights: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 17, 7),
    );
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((1, 15), (1, 20)), range((17, 6), (17, 11))]
    );
    assert!(
        highlights
            .iter()
            .all(|h| h.kind == Some(lsp::DocumentHighlightKind::TEXT))
    );
    let highlights: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 12, 3),
    );
    assert_eq!(
        highlights.iter().map(|h| h.range).collect::<Vec<_>>(),
        [range((12, 2), (12, 5)), range((13, 2), (13, 5))]
    );
    let empty: Vec<lsp::DocumentHighlight> = navigation(
        &mut client,
        "textDocument/documentHighlight",
        Client::position(MAIN, 2, 0),
    );
    assert!(empty.is_empty());
    client.shutdown();
}

#[test]
fn workspace_symbols_search_unopened_modules_and_unsaved_contents() {
    let mut files = nav_files().to_vec();
    files.push(("/other/Pome.toml", ""));
    files.push(("/other/src/lib.emel", "pub type OtherShape(id: Int)\n"));
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let symbols: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "sHaP" }));
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "Shape");
    assert_eq!(
        symbols[0].location,
        location(SHAPES, (0, 9), (0, 14)).unwrap()
    );
    assert_eq!(symbols[0].container_name.as_deref(), Some("Shapes"));
    client.open(
        SHAPES,
        1,
        "pub type NewShape(id: Int)\nfn broken() { missing }\n",
    );
    let symbols: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "shape" }));
    assert_eq!(symbols.len(), 1);
    assert_eq!(symbols[0].name, "NewShape");
    let all: Vec<lsp::SymbolInformation> =
        navigation(&mut client, "workspace/symbol", json!({ "query": "" }));
    assert!(
        all.iter()
            .all(|s| s.location.uri.as_str().starts_with("file:///app/"))
    );
    let empty: Vec<lsp::SymbolInformation> = navigation(
        &mut client,
        "workspace/symbol",
        json!({ "query": "no_such_symbol" }),
    );
    assert!(empty.is_empty());
    client.shutdown();
}

#[test]
fn syntax_navigation_works_with_parse_only_and_utf16() {
    let path = "/tmp/main.emel";
    let text = "# 😀 one\r\n## two\r\nimport First\r\nimport Second\r\nfn broken() {\r\n  \"😀\"\r\n  unknown match {\r\n    _ -> 1\r\n  }\r\n}\r\nfn unfinished(\r\n";
    let mut client = Client::initialize(&[(path, "")], None, ParseOnly::new);
    client.open(path, 1, text);
    let folds: Vec<lsp::FoldingRange> = navigation(
        &mut client,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(path) } }),
    );
    assert_eq!(
        folds
            .iter()
            .map(|f| (f.start_line, f.end_line, f.kind.clone()))
            .collect::<Vec<_>>(),
        [
            (0, 1, Some(lsp::FoldingRangeKind::Comment)),
            (2, 3, Some(lsp::FoldingRangeKind::Imports)),
            (4, 9, Some(lsp::FoldingRangeKind::Region)),
            (6, 8, Some(lsp::FoldingRangeKind::Region)),
        ]
    );
    let selections: Vec<lsp::SelectionRange> = navigation(
        &mut client,
        "textDocument/selectionRange",
        json!({
            "textDocument": { "uri": uri(path) }, "positions": [
                { "line": 0, "character": 4 }, { "line": 5, "character": 3 }, { "line": 6, "character": 3 }
            ]
        }),
    );
    assert_eq!(selections.len(), 3);
    assert_eq!(selections[0].range, range((0, 0), (0, 8)));
    assert_eq!(selections[1].range, range((5, 3), (5, 5)));
    assert_eq!(selections[2].range, range((6, 2), (6, 9)));
    for selection in &selections {
        let mut current = selection;
        while let Some(parent) = current.parent.as_deref() {
            assert!(
                parent.range.start <= current.range.start && parent.range.end >= current.range.end
            );
            assert_ne!(parent.range, current.range);
            current = parent;
        }
    }
    client.change(path, 2, "fn empty() {}\n");
    let folds: Vec<lsp::FoldingRange> = navigation(
        &mut client,
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(path) } }),
    );
    assert!(folds.is_empty());
    client.shutdown();
}

#[test]
fn document_links_to_external_files() {
    assert_eq!(
        emela_lsp::capabilities().document_link_provider,
        Some(lsp::DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
        })
    );
    // Only the unsaved contents have the annotations, and the file has a type error.
    let text = r#"@external(js, "./http_ffi.mjs", "fetchText")
@external(c, "../native/foo.c", "f")
fn fetch_text(url: String) -> String

@external(js, "./missing.mjs", "g")
@external(c, "sin")
fn g() -> Int

@external(js, """
  ./multi.mjs
  """, "h")
fn h() -> Int

@external(js, "./#{h()}.mjs", "i")
@external(wasm, "http_ffi.mjs", "i")
@external(c, "./http\u{66}fi.mjs", "i")
fn i() -> Int

@export(wasm, "./http_ffi.mjs", "run")
pub fn run() -> Int { "not an int" }
"#;
    let mut client = Client::initialize(
        &[
            ("/app/Pome.toml", ""),
            (MAIN, ""),
            ("/app/src/http_ffi.mjs", ""),
            ("/app/src/multi.mjs", ""),
            ("/app/native/foo.c", ""),
        ],
        Some("/app"),
        Typecheck::default,
    );
    client.open(MAIN, 1, text);
    let links: Vec<lsp::DocumentLink> = navigation(
        &mut client,
        "textDocument/documentLink",
        json!({ "textDocument": { "uri": uri(MAIN) } }),
    );
    assert_eq!(
        links
            .into_iter()
            .map(|link| (link.range, link.target))
            .collect::<Vec<_>>(),
        [
            (range((0, 15), (0, 29)), Some(uri("/app/src/http_ffi.mjs"))),
            (range((1, 14), (1, 29)), Some(uri("/app/native/foo.c"))),
            (range((9, 2), (9, 13)), Some(uri("/app/src/multi.mjs"))),
        ]
    );
    client.shutdown();
}

#[test]
fn type_and_implementation_navigation_with_type_errors() {
    let data = "pub type Boxed(value: Int)\npub enum Choice { One }\npub trait Speak {\n  fn speak(self) -> String\n}\nimpl Boxed: Speak {\n  fn speak(self) { \"boxed\" }\n}\nimpl Choice: Speak {\n  fn speak(self) { \"choice\" }\n}\n";
    let text = "import Data.{Boxed, Speak}\nfn call(item: Boxed) {\n  copy = item\n  Speak.speak(copy)\n}\nfn broken() -> Int { missing }\n";
    let mut client = Client::initialize(
        &[
            ("/app/Pome.toml", ""),
            (MAIN, text),
            ("/app/src/data.emel", data),
        ],
        Some("/app"),
        Typecheck::default,
    );
    let found: Option<lsp::Location> = navigation(
        &mut client,
        "textDocument/typeDefinition",
        Client::position(MAIN, 3, 15),
    );
    assert_eq!(found, location("/app/src/data.emel", (0, 9), (0, 14)));
    let found: Option<lsp::Location> = navigation(
        &mut client,
        "textDocument/typeDefinition",
        Client::position(MAIN, 5, 16),
    );
    assert_eq!(found, None);
    let impls: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 3, 4),
    );
    assert_eq!(
        impls,
        [
            location("/app/src/data.emel", (5, 0), (7, 1)).unwrap(),
            location("/app/src/data.emel", (8, 0), (10, 1)).unwrap()
        ]
    );
    let methods: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 3, 9),
    );
    assert_eq!(
        methods,
        [
            location("/app/src/data.emel", (6, 5), (6, 10)).unwrap(),
            location("/app/src/data.emel", (9, 5), (9, 10)).unwrap()
        ]
    );
    let empty: Vec<lsp::Location> = navigation(
        &mut client,
        "textDocument/implementation",
        Client::position(MAIN, 1, 9),
    );
    assert!(empty.is_empty());
    client.shutdown();
}

const SHAPES: &str = "/app/src/shapes.emel";

const LOG: &str = "/app/src/util/log.emel";

#[test]
fn definition_of_names() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().definition_provider,
        Some(lsp::OneOf::Left(true))
    );
    // An effect operation (the `info` of `log.info`).
    assert_eq!(
        client.definition(MAIN, 13, 7),
        location(MAIN, (4, 5), (4, 9))
    );
    // From a handler's operation name to the effect's operation.
    assert_eq!(
        client.definition(MAIN, 8, 6),
        location(MAIN, (4, 5), (4, 9))
    );
    // A local variable.
    assert_eq!(
        client.definition(MAIN, 13, 3),
        location(MAIN, (12, 2), (12, 5))
    );
    assert_eq!(
        client.definition(MAIN, 13, 23),
        location(MAIN, (11, 9), (11, 13))
    );
    // The module left of `.` goes to the top of its file; the function right of it to its
    // definition.
    assert_eq!(
        client.definition(MAIN, 13, 12),
        location(LOG, (0, 0), (0, 0))
    );
    assert_eq!(
        client.definition(MAIN, 13, 17),
        location(LOG, (0, 7), (0, 13))
    );
    // The import path and an imported name. `Util` is not a module, so null.
    assert_eq!(
        client.definition(MAIN, 0, 13),
        location(LOG, (0, 0), (0, 0))
    );
    assert_eq!(client.definition(MAIN, 0, 8), None);
    assert_eq!(
        client.definition(MAIN, 1, 16),
        location(SHAPES, (0, 9), (0, 14))
    );
    assert_eq!(
        client.definition(MAIN, 1, 23),
        location(SHAPES, (2, 7), (2, 11))
    );
    // Effect, handler, constructor, function. The position right after a name counts as the name.
    assert_eq!(
        client.definition(MAIN, 12, 12),
        location(MAIN, (3, 7), (3, 13))
    );
    assert_eq!(
        client.definition(MAIN, 7, 17),
        location(MAIN, (3, 7), (3, 13))
    );
    assert_eq!(
        client.definition(MAIN, 18, 9),
        location(MAIN, (7, 8), (7, 13))
    );
    assert_eq!(
        client.definition(MAIN, 17, 6),
        location(SHAPES, (0, 9), (0, 14))
    );
    assert_eq!(
        client.definition(MAIN, 18, 20),
        location(MAIN, (11, 3), (11, 8))
    );
    assert_eq!(
        client.definition(MAIN, 19, 6),
        location(SHAPES, (2, 7), (2, 11))
    );
    // A definition's name points to itself.
    assert_eq!(
        client.definition(MAIN, 11, 4),
        location(MAIN, (11, 3), (11, 8))
    );
    // A type position.
    assert_eq!(
        client.definition(SHAPES, 2, 16),
        location(SHAPES, (0, 9), (0, 14))
    );
    // A position with nothing there.
    assert_eq!(client.definition(MAIN, 2, 0), None);
    client.shutdown();
}

/// The argument of `@entry` (`[ext.entry]`) goes to the entry adapter, and its module to the top
/// of the module's file.
#[test]
fn definition_of_an_entry_adapter() {
    let main = "import Http\n\n@entry(Http.serve)\nfn app(x: Int) -> Int { x }\n\nfn main() {}\n";
    let http = "@export(js, \"default\", \"fetch\")\npub fn serve(app: fn(Int) -> Int fails {} uses {}, x: Int) -> Int { app(x) }\n";
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", main),
        ("/app/src/http.emel", http),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let http_path = "/app/src/http.emel";
    assert_eq!(
        client.definition(MAIN, 2, 13),
        location(http_path, (1, 7), (1, 12))
    );
    assert_eq!(
        client.definition(MAIN, 2, 8),
        location(http_path, (0, 0), (0, 0))
    );
    // The fn that `@entry` makes has no source: hover and inlay hints do not show it.
    let hover = format!("{:?}", client.hover(MAIN, 2, 2));
    assert!(!hover.contains("$entry"), "{hover}");
    let hints = client.inlay_hints(MAIN).unwrap_or_default();
    assert!(hints.iter().all(|(line, ..)| *line != 2), "{hints:?}");
    client.shutdown();
}

/// Definitions in the bundled core have no location to go to.
#[test]
fn definition_of_core_is_null() {
    let src = "fn main() {\n  List.map([1], |x| x)\n}\n";
    let files = [("/tmp/main.emel", src)];
    let mut client = Client::initialize(&files, None, Typecheck::default);
    assert_eq!(client.definition("/tmp/main.emel", 1, 3), None);
    assert_eq!(client.definition("/tmp/main.emel", 1, 8), None);
    // A local variable in a standalone file can be navigated.
    assert_eq!(
        client.definition("/tmp/main.emel", 1, 20),
        location("/tmp/main.emel", (1, 17), (1, 18))
    );
    // null for frontends that do not resolve names.
    client.shutdown();
    let mut client = Client::initialize(&files, None, ParseOnly::new);
    assert_eq!(client.definition("/tmp/main.emel", 1, 24), None);
    client.shutdown();
}

#[test]
fn references_in_the_project() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().references_provider,
        Some(lsp::OneOf::Left(true))
    );
    let main = |s, e| (MAIN.to_owned(), range(s, e));
    let shapes = |s, e| (SHAPES.to_owned(), range(s, e));
    // A constructor with the same name as its type counts as the same thing. Includes the
    // declaration.
    assert_eq!(
        client.references(SHAPES, 0, 10, true),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((0, 9), (0, 14)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // Same result from a reference position. Excludes the declaration.
    assert_eq!(
        client.references(MAIN, 17, 7, false),
        Some(vec![
            main((1, 15), (1, 20)),
            main((17, 6), (17, 11)),
            shapes((2, 15), (2, 20)),
        ])
    );
    // An effect operation: the handler's implementation and calls on the capability.
    assert_eq!(
        client.references(MAIN, 4, 6, false),
        Some(vec![main((8, 5), (8, 9)), main((13, 6), (13, 10))])
    );
    // A local variable.
    assert_eq!(
        client.references(MAIN, 12, 3, true),
        Some(vec![main((12, 2), (12, 5)), main((13, 2), (13, 5))])
    );
    // A module: the import path and the left of `.`.
    assert_eq!(
        client.references(MAIN, 13, 12, true),
        Some(vec![main((0, 12), (0, 15)), main((13, 11), (13, 14))])
    );
    // A position with nothing there.
    assert_eq!(client.references(MAIN, 2, 0, true), None);
    client.shutdown();
}

#[test]
fn document_symbols_list_declarations() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), ParseOnly::new);
    assert_eq!(
        emela_lsp::capabilities().document_symbol_provider,
        Some(lsp::OneOf::Left(true))
    );
    fn outline(symbols: &[lsp::DocumentSymbol]) -> Vec<String> {
        symbols
            .iter()
            .map(|s| {
                let children = outline(s.children.as_deref().unwrap_or_default());
                let kind = format!("{:?}", s.kind);
                if children.is_empty() {
                    format!("{} {kind}", s.name)
                } else {
                    format!("{} {kind} [{}]", s.name, children.join(", "))
                }
            })
            .collect()
    }
    let symbols = client.document_symbols(MAIN).unwrap();
    assert_eq!(
        outline(&symbols),
        [
            "Logger Interface [info Method]",
            "Quiet Class [info Method]",
            "greet Function",
            "main Function",
        ]
    );
    // The range is the whole declaration; the selection range is the name.
    assert_eq!(symbols[0].range, range((3, 0), (5, 1)));
    assert_eq!(symbols[0].selection_range, range((3, 7), (3, 13)));
    let symbols = client.document_symbols(SHAPES).unwrap();
    assert_eq!(
        outline(&symbols),
        ["Shape Struct [w Field, h Field]", "area Function"]
    );
    // An open document uses unsaved contents. Declarations that parsed are listed despite syntax
    // errors.
    let src = "enum Color { Red\n Green }\n\nconst MAX: Int = 3\n\ntrait Named {\n  fn name(self) -> String\n}\n\nimpl Color: Named {\n  fn name(self) { \"c\" }\n}\n\nfn broken( {\n";
    client.open("/app/src/other.emel", 1, src);
    assert_eq!(
        outline(&client.document_symbols("/app/src/other.emel").unwrap()),
        [
            "Color Enum [Red EnumMember, Green EnumMember]",
            "MAX Constant",
            "Named Interface [name Method]",
            "impl Color: Named Namespace [name Method]",
            "broken Function",
        ]
    );
    // null for unreadable files.
    assert_eq!(client.document_symbols("/app/src/nowhere.emel"), None);
    while let Ok(message) = client
        .connection
        .receiver
        .recv_timeout(Duration::from_millis(200))
    {
        assert!(matches!(message, Message::Notification(_)), "{message:?}");
    }
    client.shutdown();
}

#[test]
fn rename_across_files() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert!(matches!(
        emela_lsp::capabilities().rename_provider,
        Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            ..
        }))
    ));
    let edit = |path: &str, s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    // A type and its constructor are renamed together, including import names.
    assert_eq!(
        client.rename(MAIN, 17, 7, "Figure"),
        Ok(vec![
            edit(MAIN, (1, 15), (1, 20), "Figure"),
            edit(MAIN, (17, 6), (17, 11), "Figure"),
            edit(SHAPES, (0, 9), (0, 14), "Figure"),
            edit(SHAPES, (2, 15), (2, 20), "Figure"),
        ])
    );
    // An effect operation: the declaration, the handler's implementation and the calls.
    assert_eq!(
        client.rename(MAIN, 4, 6, "log_info"),
        Ok(vec![
            edit(MAIN, (4, 5), (4, 9), "log_info"),
            edit(MAIN, (8, 5), (8, 9), "log_info"),
            edit(MAIN, (13, 6), (13, 10), "log_info"),
        ])
    );
    // A local variable.
    assert_eq!(
        client.rename(MAIN, 11, 10, "who"),
        Ok(vec![
            edit(MAIN, (11, 9), (11, 13), "who"),
            edit(MAIN, (13, 22), (13, 26), "who"),
        ])
    );
    // Names of the wrong lexical class, and reserved words.
    assert_eq!(
        client.rename(MAIN, 17, 7, "figure"),
        Err("`figure` is not a valid type name".to_owned())
    );
    assert_eq!(
        client.rename(MAIN, 11, 10, "match"),
        Err("`match` is not a valid lowercase name".to_owned())
    );
    // Modules cannot be renamed.
    assert_eq!(
        client.rename(MAIN, 13, 12, "Logs"),
        Err("this name cannot be renamed".to_owned())
    );
    // prepareRename returns the name's range, or null if it cannot be renamed.
    assert_eq!(
        client.prepare_rename(MAIN, 13, 7),
        Some(range((13, 6), (13, 10)))
    );
    assert_eq!(client.prepare_rename(MAIN, 13, 12), None);
    assert_eq!(client.prepare_rename(MAIN, 2, 0), None);
    client.shutdown();
}

/// `[effect.handler]`: `Self` in an init names the handler. Definition goes to the handler; renaming
/// the handler keeps `Self`, and `Self` itself cannot be renamed.
#[test]
fn self_in_init_refers_to_the_handler() {
    let src = "effect Log {\n  fn log(msg: String) -> ()\n}\n\nhandler Tagged(tag: String): Log {\n  init { Self(tag: \"a\") }\n  fn log(msg) { () }\n}\n\nfn main() {\n  with Tagged { (use Log).log(\"x\") }\n}\n";
    let path = "/tmp/main.emel";
    let mut client = Client::initialize(&[(path, src)], None, Typecheck::default);
    assert_eq!(
        client.definition(path, 5, 9),
        location(path, (4, 8), (4, 14))
    );
    let edit = |s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    assert_eq!(
        client.rename(path, 4, 9, "Labelled"),
        Ok(vec![
            edit((4, 8), (4, 14), "Labelled"),
            edit((10, 7), (10, 13), "Labelled"),
        ])
    );
    assert_eq!(client.prepare_rename(path, 5, 9), None);
    assert_eq!(
        client.rename(path, 5, 9, "Labelled"),
        Err("this name cannot be renamed".to_owned())
    );
    client.shutdown();
}

/// Shorthand named arguments and field patterns keep their labels. Names in the bundled core
/// cannot be renamed.
#[test]
fn rename_keeps_punned_labels() {
    let src = "error NotFound(id: Int)\n\ntype User(name: String)\n\nfn find(id: Int) -> Int fails NotFound {\n  fail NotFound(id:)\n}\n\nfn greet(u: User) -> String {\n  u match {\n    User(name:) -> name\n  }\n}\n\nfn main() {\n  List.map([1], |x| x)\n}\n";
    let path = "/tmp/main.emel";
    let mut client = Client::initialize(&[(path, src)], None, Typecheck::default);
    let edit = |s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    assert_eq!(
        client.rename(path, 4, 8, "key"),
        Ok(vec![
            edit((4, 8), (4, 10), "key"),
            edit((5, 16), (5, 18), "id: key")
        ])
    );
    assert_eq!(
        client.rename(path, 10, 20, "who"),
        Ok(vec![
            edit((10, 9), (10, 13), "name: who"),
            edit((10, 19), (10, 23), "who")
        ])
    );
    assert_eq!(client.prepare_rename(path, 15, 8), None);
    assert_eq!(client.prepare_rename(path, 15, 3), None);
    client.shutdown();
}

/// A method call is a reference of the function it calls (`[lsp.navigation]`), and after `.` the
/// functions that can be called as methods follow the fields (`[lsp.completion]`).
#[test]
fn method_calls_in_navigation_and_completion() {
    let src = NAV_MAIN.replace("  area(s)\n", "  s.area()\n");
    let files = [
        ("/app/Pome.toml", ""),
        (MAIN, src.as_str()),
        (SHAPES, NAV_SHAPES),
        (LOG, NAV_LOG),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        client.definition(MAIN, 19, 5),
        location(SHAPES, (2, 7), (2, 11))
    );
    let main = |s, e| (MAIN.to_owned(), range(s, e));
    let shapes = |s, e| (SHAPES.to_owned(), range(s, e));
    assert_eq!(
        client.references(SHAPES, 2, 8, true),
        Some(vec![
            main((1, 22), (1, 26)),
            main((19, 4), (19, 8)),
            shapes((2, 7), (2, 11)),
        ])
    );
    let edit = |path: &str, s, e, text: &str| (path.to_owned(), range(s, e), text.to_owned());
    assert_eq!(
        client.rename(MAIN, 19, 5, "size"),
        Ok(vec![
            edit(MAIN, (1, 22), (1, 26), "size"),
            edit(MAIN, (19, 4), (19, 8), "size"),
            edit(SHAPES, (2, 7), (2, 11), "size"),
        ])
    );
    client.change(MAIN, 2, &NAV_MAIN.replace("  area(s)\n", "  s.\n"));
    assert_eq!(
        client.complete(MAIN, 19, 4).unwrap(),
        [
            (
                "w".to_owned(),
                lsp::CompletionItemKind::FIELD,
                Some("Int".to_owned())
            ),
            (
                "h".to_owned(),
                lsp::CompletionItemKind::FIELD,
                Some("Int".to_owned())
            ),
            (
                "area".to_owned(),
                lsp::CompletionItemKind::METHOD,
                Some("fn() -> Int".to_owned())
            ),
        ]
    );
    // The return type of a method in a chain: `s.w.to_string()` is a String.
    client.change(
        MAIN,
        3,
        &NAV_MAIN.replace("  area(s)\n", "  s.w.to_string().\n"),
    );
    let items = client.complete(MAIN, 19, 18).unwrap();
    assert!(labels(&items).contains(&"length"), "{items:?}");
    client.shutdown();
}
