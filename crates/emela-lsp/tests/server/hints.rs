//! Inlay hints and semantic tokens.

use emela_driver::{ParseOnly, Typecheck};
use lsp_types as lsp;

use super::{Client, MAIN, Token, codes, range};

/// Shows the unwritten types, E and R inline (spec 9.3).
#[test]
fn inlay_hints_show_types_errors_and_effects() {
    let src = "error NotFound(id: Int)

fn find(id: Int) -> Int fails NotFound {
  if id > 0 { id } else { fail NotFound(id:) }
}

effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn greet() {
  log = use Logger
  log.info(\"hi\")
  n = find(1)
  List.map([n], |x| x)
}

fn main() {
  with Quiet { greet() }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().inlay_hint_provider,
        Some(lsp::OneOf::Left(true))
    );
    let hint = |line, col, label: &str, padding| (line, col, label.to_owned(), padding);
    assert_eq!(
        client.inlay_hints(MAIN).unwrap(),
        [
            // Parameter types of a handler operation.
            hint(11, 13, ": String", false),
            // An omitted annotation, including invisible sets.
            hint(14, 10, "-> List[Int] fails NotFound uses { Logger }", true),
            // E and R of a call in statement position.
            hint(16, 16, "uses { Logger }", true),
            // The type of a binding (not shown for the capability `log`).
            hint(17, 3, ": Int", false),
            hint(17, 13, "fails NotFound", true),
            // Lambda parameters, after the name inside the bars (`|x: Int|`). Calls with neither
            // E nor R get no hint.
            hint(18, 18, ": Int", false),
            hint(21, 9, "-> List[Int] fails NotFound", true),
            // A call inside `with` shows R.
            hint(22, 22, "fails NotFound uses { Logger }", true),
        ]
    );
    // Written annotations get no hints. Frontends without types give none.
    client.shutdown();
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.inlay_hints(MAIN), Some(vec![]));
    client.shutdown();
}

const CAPABILITY_MAIN: &str = "effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Loud: Logger {
  fn info(msg) { () }
}

fn greet(names: List[String]) {
  log = use Logger
  List.map(names, |n| log.info(\"hi #{n}\"))
  (use Logger).info(\"bye\")
}

fn main() {
  with Quiet {
    outer = use Logger
    with Loud {
      inner = use Logger
      inner.info(\"a\")
      outer.info(\"b\")
    }
    greet([\"x\"])
  }
}
";

/// The variable type with the `capability` modifier (bit 0).
fn cap(line: u32, col: u32, len: u32) -> Token {
    (line, col, len, 0, 1)
}

/// Capability values (spec 8.2) are marked where bound and where used, including inside closures
/// and nested `with`. Nothing else is marked.
#[test]
fn semantic_tokens_mark_capability_values() {
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/main.emel", CAPABILITY_MAIN),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let Some(lsp::SemanticTokensServerCapabilities::SemanticTokensOptions(options)) =
        emela_lsp::capabilities().semantic_tokens_provider
    else {
        panic!("no semantic tokens options");
    };
    assert_eq!(
        options.legend.token_types,
        [lsp::SemanticTokenType::VARIABLE]
    );
    assert_eq!(
        options.legend.token_modifiers,
        [lsp::SemanticTokenModifier::new("capability")]
    );
    assert_eq!(options.range, Some(true));
    assert_eq!(
        client.semantic_tokens(MAIN, None).unwrap(),
        [
            // The binding and its use in a closure.
            cap(13, 2, 3),
            cap(14, 22, 3),
            // Nested `with`: both the outer and the inner value.
            cap(20, 4, 5),
            cap(22, 6, 5),
            cap(23, 6, 5),
            cap(24, 6, 5),
        ]
    );
    // A range gets only the tokens inside it.
    assert_eq!(
        client
            .semantic_tokens(MAIN, Some(range((22, 0), (24, 0))))
            .unwrap(),
        [cap(22, 6, 5), cap(23, 6, 5)]
    );
    client.shutdown();
    // Frontends without types give none.
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.semantic_tokens(MAIN, None), Some(vec![]));
    client.shutdown();
}

/// Errors elsewhere in the file do not hide the capability values that can still be told.
#[test]
fn semantic_tokens_survive_errors() {
    let src = "effect Logger {
  fn info(msg: String) -> ()
}

fn greet() {
  log = use Logger
  n: Int = \"not an int\"
  log.info(missing)
  bad = use Nope
  bad.info(\"x\")
}

fn broken( {
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    // Syntax, name and type errors.
    let published = codes(&client.diagnostics());
    for code in ["E1101", "E2101", "E3001"] {
        assert!(published.iter().any(|c| c == code), "{published:?}");
    }
    assert_eq!(
        client.semantic_tokens(MAIN, None).unwrap(),
        [
            cap(5, 2, 3),
            cap(7, 2, 3),
            // `use` of an unknown effect is still a capability value.
            cap(8, 2, 3),
            cap(9, 2, 3),
        ]
    );
    client.shutdown();
}

/// An effect alias (`[effect.use]`) is marked as a capability where it is declared and where it
/// is the receiver of an operation, inside closures too.
#[test]
fn semantic_tokens_mark_effect_aliases() {
    let src = "effect Logger {
  fn info(msg: String) -> ()
}

fn greet() {
  log <- Logger
  log.info(\"a\")
  List.each([1], |n| log.info(\"#{n}\"))
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        client.semantic_tokens(MAIN, None).unwrap(),
        [cap(5, 2, 3), cap(6, 2, 3), cap(7, 21, 3)]
    );
    client.shutdown();
}
