//! Completion and signature help.

use emela_driver::Typecheck;
use lsp_types as lsp;
use lsp_types::request::{Completion, Request as _};

use super::{Client, MAIN, NAV_MAIN, labels, nav_files};

#[test]
fn completion_after_dot() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities()
            .completion_provider
            .and_then(|c| c.trigger_characters),
        Some(vec![".".to_owned(), "@".to_owned()])
    );
    // Operations of a capability. Same candidates with or without a partially typed name.
    let src = NAV_MAIN.replace("  log.info(Log.prefix(name))\n", "  log.\n  log.in\n");
    client.open(MAIN, 1, &src);
    let items = client.complete(MAIN, 13, 6).unwrap();
    assert_eq!(
        items,
        [(
            "info".to_owned(),
            lsp::CompletionItemKind::METHOD,
            Some("fn(String) -> ()".to_owned())
        )]
    );
    assert_eq!(labels(&client.complete(MAIN, 14, 8).unwrap()), ["info"]);
    // A module's pub definitions.
    client.change(MAIN, 2, &NAV_MAIN.replace("Log.prefix(name)", "Log."));
    assert_eq!(labels(&client.complete(MAIN, 13, 15).unwrap()), ["prefix"]);
    // A type's fields, with the declared type as detail, then its methods.
    client.change(MAIN, 3, &NAV_MAIN.replace("  area(s)\n", "  s.\n"));
    assert_eq!(
        labels(&client.complete(MAIN, 19, 4).unwrap()),
        ["w", "h", "area"]
    );
    // core modules usable without an import.
    client.change(MAIN, 4, &NAV_MAIN.replace("  area(s)\n", "  List.\n"));
    let items = client.complete(MAIN, 19, 7).unwrap();
    assert!(labels(&items).contains(&"map"), "{items:?}");
    // Nothing after an expression of unknown type.
    client.change(MAIN, 5, &NAV_MAIN.replace("  area(s)\n", "  1.\n"));
    assert_eq!(client.complete(MAIN, 19, 4), Some(vec![]));
    client.shutdown();
}

#[test]
fn completion_in_imports() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let src = format!("import \n{NAV_MAIN}");
    client.open(MAIN, 1, &src);
    // The first segment of a module path (the current module is excluded).
    let items = client.complete(MAIN, 0, 7).unwrap();
    assert!(labels(&items).contains(&"Shapes"), "{items:?}");
    assert!(labels(&items).contains(&"Util"), "{items:?}");
    assert!(!labels(&items).contains(&"Main"), "{items:?}");
    assert!(items.iter().all(|i| i.1 == lsp::CompletionItemKind::MODULE));
    // Later segments offer only module names (no function names).
    client.change(MAIN, 2, &format!("import Util.\n{NAV_MAIN}"));
    assert_eq!(
        client.complete(MAIN, 0, 12).unwrap(),
        [(
            "Log".to_owned(),
            lsp::CompletionItemKind::MODULE,
            Some("Util.Log".to_owned())
        )]
    );
    // Inside braces, the module's pub names.
    client.change(MAIN, 3, &format!("import Shapes.{{a}}\n{NAV_MAIN}"));
    assert_eq!(
        labels(&client.complete(MAIN, 0, 16).unwrap()),
        ["Shape", "area"]
    );
    client.shutdown();
}

#[test]
fn completion_of_names_in_scope() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    // The body of greet: locals (innermost first), module definitions, imported names, modules,
    // keywords.
    let src = NAV_MAIN.replace(
        "  log.info(Log.prefix(name))\n",
        "  log.info(Log.prefix(name))\n  n\n",
    );
    client.open(MAIN, 1, &src);
    let items = client.complete(MAIN, 14, 3).unwrap();
    let names = labels(&items);
    assert_eq!(names[..2], ["log", "name"]);
    assert_eq!(items[0].2.as_deref(), Some("use Logger"));
    assert_eq!(items[1].2.as_deref(), Some("String"));
    assert!(!names.contains(&"Prelude"), "{names:?}");
    for name in [
        "greet", "main", "Logger", "Quiet", "Shape", "area", "Log", "List", "Some", "match",
    ] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    // No locals of other functions and no names that were not imported.
    assert!(!names.contains(&"s"), "{names:?}");
    assert!(!names.contains(&"prefix"), "{names:?}");
    // A variable is not visible before its binding or inside its right-hand side.
    let items = client.complete(MAIN, 12, 2).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    let items = client.complete(MAIN, 12, 12).unwrap();
    assert!(!labels(&items).contains(&"log"), "{items:?}");
    // Only effects after `use`; only handlers and layers after `with`.
    let items = client.complete(MAIN, 12, 12).unwrap();
    assert!(labels(&items).contains(&"Logger"), "{items:?}");
    assert!(
        items
            .iter()
            .all(|i| i.1 == lsp::CompletionItemKind::INTERFACE),
        "{items:?}"
    );
    let items = client.complete(MAIN, 19, 7).unwrap();
    assert_eq!(labels(&items), ["Quiet"]);
    // Only effects right after `uses` too (`[lsp.completion]`).
    let src = format!("{NAV_MAIN}\nfn later() -> () uses L\n");
    client.open(MAIN, 2, &src);
    let items = client.complete(MAIN, 22, 23).unwrap();
    assert!(labels(&items).contains(&"Logger"), "{items:?}");
    assert!(
        items
            .iter()
            .all(|i| i.1 == lsp::CompletionItemKind::INTERFACE),
        "{items:?}"
    );
    // In a type, the imported modules that have a pub type (`Shapes`), not the others (`Log`).
    let src = format!("{NAV_MAIN}\nfn later(s: S) -> ()\n");
    client.change(MAIN, 3, &src);
    let items = client.complete(MAIN, 22, 12).unwrap();
    let names = labels(&items);
    assert!(names.contains(&"Shapes"), "{names:?}");
    assert!(names.contains(&"Shape"), "{names:?}");
    assert!(!names.contains(&"Log"), "{names:?}");
    client.shutdown();
}

#[test]
fn signature_help_in_calls() {
    let mut client = Client::initialize(&nav_files(), Some("/app"), Typecheck::default);
    let caps = emela_lsp::capabilities();
    assert_eq!(
        caps.signature_help_provider
            .and_then(|s| s.trigger_characters),
        Some(vec!["(".to_owned(), ",".to_owned()])
    );
    let s = |label: &str, params: &[&str], active: Option<u32>| {
        Some((
            label.to_owned(),
            params.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            active,
        ))
    };
    // The innermost call.
    assert_eq!(
        client.signature(MAIN, 13, 22),
        s("prefix(s: String) -> String", &["s: String"], Some(0))
    );
    // An effect operation.
    assert_eq!(
        client.signature(MAIN, 13, 11),
        s("info(msg: String) -> ()", &["msg: String"], Some(0))
    );
    // A constructor. The active parameter is decided by the number of commas.
    let shape = "Shape(w: Int, h: Int) -> Shape";
    assert_eq!(
        client.signature(MAIN, 17, 12),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    assert_eq!(
        client.signature(MAIN, 17, 18),
        s(shape, &["w: Int", "h: Int"], Some(1))
    );
    // A partially written call, and a parameter decided by its label.
    client.open(
        MAIN,
        1,
        &NAV_MAIN.replace("Shape(w: 1, h: 2)", "Shape(h: 2, w:"),
    );
    assert_eq!(
        client.signature(MAIN, 17, 20),
        s(shape, &["w: Int", "h: Int"], Some(0))
    );
    // Outside a call.
    client.change(MAIN, 2, NAV_MAIN);
    assert_eq!(client.signature(MAIN, 12, 2), None);
    assert_eq!(client.signature(MAIN, 17, 23), None);
    client.shutdown();
}

const PIPE_MAIN: &str = "effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn add(a: Int, b: Int) -> Int { a + b }

fn greet(s: String) {
  log = use Logger
  s |> log.info
  s |> log.info()
}

fn main() {
  xs = [1, 2, 3]
  ys = xs |> List.map(|x| x |> add(1))
  n = xs |> List.fold(from: 0, f: add)
  m = 1 |> add(2) |> add(3)
  k = add(1, 2) |> add(3)
  with Quiet { greet(\"a\") }
}
";

#[test]
fn signature_help_on_the_right_of_a_pipe() {
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", PIPE_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let s = |label: &str, params: &[&str], active: Option<u32>| {
        Some((
            label.to_owned(),
            params.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            active,
        ))
    };
    // The left side fills the first parameter, so it is left out.
    let add_b = s("add(b: Int) -> Int", &["b: Int"], Some(0));
    let f = "f: fn(A) -> B fails E(f) uses { R(f) }";
    assert_eq!(
        client.signature(MAIN, 18, 22),
        s(
            &format!("map({f}) -> List[B] fails E(f) uses {{ R(f) }}"),
            &[f],
            Some(0)
        )
    );
    // Nested in a lambda on the right of another pipe.
    assert_eq!(client.signature(MAIN, 18, 35), add_b);
    // The other parameters given by name.
    let f = "f: fn(B, A) -> B fails E(f) uses { R(f) }";
    let fold = format!("fold(from: B, {f}) -> B fails E(f) uses {{ R(f) }}");
    for (col, active) in [(22, 0), (32, 1), (34, 1)] {
        assert_eq!(
            client.signature(MAIN, 19, col),
            s(&fold, &["from: B", f], Some(active))
        );
    }
    // A chain of pipes.
    assert_eq!(client.signature(MAIN, 20, 15), add_b);
    assert_eq!(client.signature(MAIN, 20, 25), add_b);
    // A call on the left of a pipe keeps all its parameters.
    assert_eq!(
        client.signature(MAIN, 21, 10),
        s("add(a: Int, b: Int) -> Int", &["a: Int", "b: Int"], Some(0))
    );
    assert_eq!(client.signature(MAIN, 21, 23), add_b);
    // An operation of a capability value.
    assert_eq!(client.signature(MAIN, 13, 16), s("info() -> ()", &[], None));
    // The first parameter's label is not offered.
    let items = client.complete(MAIN, 19, 22).unwrap();
    let labels: Vec<&str> = labels(&items)
        .into_iter()
        .filter(|l| l.ends_with(':'))
        .collect();
    assert_eq!(labels, ["from:", "f:"]);
    // Naming the first parameter is an error, so no parameter is active.
    client.open(
        MAIN,
        1,
        &PIPE_MAIN.replace("List.fold(from: 0, f: add)", "List.fold(xs: "),
    );
    assert_eq!(
        client.signature(MAIN, 19, 26),
        s(&fold, &["from: B", f], None)
    );
    client.shutdown();
}

const CHAIN_MAIN: &str = "type Point(x: Int, y: Int)
type Line(from: Point, to: Point)
type Box[A](value: A)

effect Clock {
  fn now() -> Point
}

fn main() {
  l = Line(from: Point(x: 1, y: 2), to: Point(x: 3, y: 4))
  b = Box(value: l)
  c = use Clock
  0
}
";

/// `a.b.` chains. The left of a nameless `.` never reaches type inference, so its type is found
/// by walking from the left.
#[test]
fn completion_after_dot_chains() {
    let files = [("/app/Pome.toml", ""), (MAIN, CHAIN_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let with = |line: &str| CHAIN_MAIN.replace("  0\n", &format!("  {line}\n"));
    // A field of a field.
    client.open(MAIN, 1, &with("l.from."));
    let items = client.complete(MAIN, 12, 9).unwrap();
    assert_eq!(labels(&items), ["x", "y"]);
    // Fields of a generic type substitute the type arguments (`value` of `Box[Line]` is `Line`).
    client.change(MAIN, 2, &with("b.value.to."));
    assert_eq!(labels(&client.complete(MAIN, 12, 13).unwrap()), ["x", "y"]);
    // The return value of a capability's operation.
    client.change(MAIN, 3, &with("c.now()."));
    assert_eq!(labels(&client.complete(MAIN, 12, 10).unwrap()), ["x", "y"]);
    // Same with a partially typed name.
    client.change(MAIN, 4, &with("l.to.x"));
    assert_eq!(labels(&client.complete(MAIN, 12, 8).unwrap()), ["x", "y"]);
    client.shutdown();
}

/// After a handler value (`self.`), its fields are offered (spec 8.4).
#[test]
fn completion_of_handler_fields() {
    let src = "effect Users {\n  fn find(id: Int) -> String\n}\n\nhandler DbUsers(url: String, port: Int): Users {\n  init { Self(url: \"x\", port: 1) }\n  fn find(id) { self.url }\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let items = client.complete(MAIN, 6, 21).unwrap();
    assert_eq!(labels(&items), ["url", "port"]);
    client.shutdown();
}

/// At the start of a call argument, labels not yet written come first.
#[test]
fn completion_of_argument_labels() {
    let files = [("/app/Pome.toml", ""), (MAIN, CHAIN_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let with = |line: &str| CHAIN_MAIN.replace("  0\n", &format!("  {line}\n"));
    let raw = |client: &mut Client, line: u32, character: u32| -> Vec<lsp::CompletionItem> {
        let id = client.request(Completion::METHOD, Client::position(MAIN, line, character));
        let result: Option<lsp::CompletionResponse> =
            serde_json::from_value(client.response(&id).response_result.unwrap()).unwrap();
        let Some(lsp::CompletionResponse::Array(mut items)) = result else {
            panic!("not an array");
        };
        items.sort_by(|a, b| a.sort_text.cmp(&b.sort_text));
        items
    };
    client.open(MAIN, 1, &with("Point("));
    let items = raw(&mut client, 12, 8);
    let first: Vec<_> = items[..2]
        .iter()
        .map(|i| {
            (
                i.label.as_str(),
                i.kind,
                i.detail.as_deref(),
                i.insert_text.as_deref(),
            )
        })
        .collect();
    let property = Some(lsp::CompletionItemKind::PROPERTY);
    assert_eq!(
        first,
        [
            ("x:", property, Some("Int"), Some("x: ")),
            ("y:", property, Some("Int"), Some("y: ")),
        ]
    );
    // Value candidates follow.
    assert!(items.iter().any(|i| i.label == "l"));
    // Written labels are not offered; still offered with a partially typed name.
    client.change(MAIN, 2, &with("Line(from: Point(x: 1, y: 2), t"));
    let items = client.complete(MAIN, 12, 33).unwrap();
    assert_eq!(
        items[0],
        (
            "to:".to_owned(),
            lsp::CompletionItemKind::PROPERTY,
            Some("Point".to_owned())
        )
    );
    assert!(!labels(&items).contains(&"from:"));
    // Not offered after a label (in value position).
    client.change(MAIN, 3, &with("Point(x: "));
    let items = client.complete(MAIN, 12, 11).unwrap();
    assert!(
        !labels(&items).iter().any(|l| l.ends_with(':')),
        "{items:?}"
    );
    // A nested call offers the inner call's labels.
    client.change(MAIN, 4, &with("Line(from: Point("));
    let items = client.complete(MAIN, 12, 19).unwrap();
    assert_eq!(labels(&items)[..2], ["x:", "y:"]);
    client.shutdown();
}

/// After `@`, the attributes a user can write; in the first argument of `@external` and
/// `@export`, the target names (`[lsp.completion]`).
#[test]
fn completion_of_attributes_and_targets() {
    let files = [("/app/Pome.toml", ""), (MAIN, "fn main() { () }\n")];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let attributes = [
        "external",
        "export",
        "entry",
        "test",
        "deprecated",
        "key",
        "tag",
    ];
    client.open(MAIN, 1, "@\nfn main() { () }\n");
    let items = client.complete(MAIN, 0, 1).unwrap();
    assert_eq!(labels(&items), attributes);
    assert!(
        items
            .iter()
            .all(|i| i.1 == lsp::CompletionItemKind::KEYWORD),
        "{items:?}"
    );
    // The same with a partially typed name; `intrinsic` and `internal` only in core.
    client.change(MAIN, 2, "@te\nfn main() { () }\n");
    assert_eq!(labels(&client.complete(MAIN, 0, 3).unwrap()), attributes);
    let targets = ["js", "wasm", "c"];
    client.change(MAIN, 3, "@external(\nfn f() -> ()\nfn main() { () }\n");
    let items = client.complete(MAIN, 0, 10).unwrap();
    assert_eq!(labels(&items), targets);
    assert!(
        items
            .iter()
            .all(|i| i.1 == lsp::CompletionItemKind::KEYWORD),
        "{items:?}"
    );
    client.change(
        MAIN,
        4,
        "@export( w, \"a\")\npub fn f() -> () { () }\nfn main() { () }\n",
    );
    assert_eq!(labels(&client.complete(MAIN, 0, 10).unwrap()), targets);
    // Not in the later arguments, nor in the argument of `@entry`.
    client.change(MAIN, 5, "@external(js, \nfn f() -> ()\nfn main() { () }\n");
    let items = client.complete(MAIN, 0, 14).unwrap();
    assert!(!labels(&items).contains(&"wasm"), "{items:?}");
    client.change(MAIN, 6, "@entry(\nfn main() { () }\n");
    let items = client.complete(MAIN, 0, 7).unwrap();
    assert!(!labels(&items).contains(&"wasm"), "{items:?}");
    client.shutdown();
}

/// Nothing inside a comment or a string; the inside of an interpolation is code.
#[test]
fn completion_not_in_comments_or_strings() {
    let src = "## doc nam @\nfn main() {\n  name = \"a\"\n  # nam @\n  s = \"nam #{nam} @\"\n  t = \"abc\n  ()\n}\n";
    let files = [("/app/Pome.toml", ""), (MAIN, src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    client.open(MAIN, 1, src);
    for (line, character) in [(0, 10), (0, 12), (3, 7), (3, 9), (4, 10), (4, 19), (5, 10)] {
        assert_eq!(
            client.complete(MAIN, line, character),
            Some(vec![]),
            "{line}:{character}"
        );
    }
    // Before the `#` of a comment, completion is as usual.
    let items = client.complete(MAIN, 3, 2).unwrap();
    assert!(labels(&items).contains(&"name"), "{items:?}");
    // Inside `#{ }`.
    let items = client.complete(MAIN, 4, 16).unwrap();
    assert!(labels(&items).contains(&"name"), "{items:?}");
    // After the closing quote.
    let items = client.complete(MAIN, 4, 20).unwrap();
    assert!(labels(&items).contains(&"name"), "{items:?}");
    client.shutdown();
}

const CLAUSE_MAIN: &str = "type Point(x: Int, y: Int)
enum Color { Red, Green }
error NotFound
error Timeout
trait Named {
  fn name(self) -> String
}
effect Db {
  fn get() -> Int
}
effect Log {
  fn info(msg: String) -> ()
}

fn main() {
  p = Point(x: 1, y: 2)
  0
}
";

/// In clauses and types, only the names of one kind (`[lsp.completion]`). `$` in a line marks
/// the cursor; an indented line goes before the last expression of main, and any other line is
/// appended after the program.
#[test]
fn completion_in_clauses_and_types() {
    let files = [("/app/Pome.toml", ""), (MAIN, CLAUSE_MAIN)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    client.open(MAIN, 1, CLAUSE_MAIN);
    let mut version = 1;
    let mut at = |line: &str| {
        let src = if line.starts_with(' ') {
            CLAUSE_MAIN.replace("  0\n", &format!("{line}\n  0\n"))
        } else {
            format!("{CLAUSE_MAIN}\n{line}\n")
        };
        let cursor = src.find('$').unwrap();
        let before = &src[..cursor];
        let row = u32::try_from(before.matches('\n').count()).unwrap();
        let column = u32::try_from(cursor - before.rfind('\n').map_or(0, |i| i + 1)).unwrap();
        version += 1;
        client.change(MAIN, version, &src.replacen('$', "", 1));
        (line.to_owned(), client.complete(MAIN, row, column).unwrap())
    };
    type Items = (
        String,
        Vec<(String, lsp::CompletionItemKind, Option<String>)>,
    );
    let check = |(line, items): Items,
                 kinds: &[lsp::CompletionItemKind],
                 present: &[&str],
                 absent: &[&str]| {
        let names = labels(&items);
        for name in present {
            assert!(names.contains(name), "{line}: {name}: {names:?}");
        }
        for name in absent {
            assert!(!names.contains(name), "{line}: {name}: {names:?}");
        }
        assert!(
            items.iter().all(|i| kinds.contains(&i.1)),
            "{line}: {items:?}"
        );
    };
    let effects = ["Db", "Log"];
    let errors = ["NotFound", "Timeout"];
    let traits = ["Named", "Show"];
    let types = ["Point", "Color", "NotFound", "Int", "String", "List"];
    let general = ["main", "match", "p"];
    let interface = [lsp::CompletionItemKind::INTERFACE];
    let structs = [lsp::CompletionItemKind::STRUCT];
    let type_kinds = [
        lsp::CompletionItemKind::STRUCT,
        lsp::CompletionItemKind::ENUM,
        lsp::CompletionItemKind::MODULE,
    ];
    let not_effects: Vec<&str> = [&general[..], &errors, &["Named"]].concat();
    let not_errors: Vec<&str> = [&general[..], &effects, &["Point", "Color", "Named"]].concat();
    let not_traits: Vec<&str> = [&general[..], &effects, &["Point"]].concat();
    let not_types: Vec<&str> = [&general[..], &effects, &["Named", "Red"]].concat();
    // Inside the braces of `uses`, at any item.
    for line in [
        "fn f() -> Int uses { $",
        "fn f() -> Int uses { $ }",
        "fn f() -> Int uses { Db, $ }",
        "fn f() -> Int uses {\n  Db,\n  $\n}",
        "fn f(g: fn() -> Int uses { Db, $ }) -> Int",
    ] {
        check(at(line), &interface, &effects, &not_effects);
    }
    // An effect set as a type argument, and the effect of a handler.
    check(
        at("fn f(x: TaskGroup[E, { $"),
        &interface,
        &effects,
        &not_effects,
    );
    check(at("handler Quiet: $"), &interface, &effects, &not_effects);
    check(
        at("handler Quiet(n: Int): $"),
        &interface,
        &effects,
        &not_effects,
    );
    // The effect of a handler with type parameters (`[effect.handler]`).
    check(
        at("handler Quiet[A]: $"),
        &interface,
        &effects,
        &not_effects,
    );
    check(
        at("handler Quiet[A](n: A): $"),
        &interface,
        &effects,
        &not_effects,
    );
    // A bound of a type parameter of a handler.
    check(at("handler Quiet[A: $"), &interface, &traits, &not_traits);
    // After `fails`, and after `|` in its error set.
    for line in [
        "fn f() -> Int fails $",
        "fn f() -> Int fails NotFound | $",
        "fn f() -> Int fails NotFound | $ uses { Db }",
        "fn f() -> Int fails NotFound\n  | $",
        "fn f(g: fn() -> Int fails $) -> Int",
    ] {
        check(at(line), &structs, &errors, &not_errors);
    }
    // The trait of an impl, supertraits, bounds and derive.
    for line in [
        "impl Point: $",
        "impl Point: $ {\n}",
        "trait Tagged: $",
        "trait Tagged: Named + $",
        "fn f[A: $](a: A) -> Int",
        "fn f[A: Named + $](a: A) -> Int",
        "type Tag(a: Int) derive $",
        "type Tag(a: Int) derive Eq, $",
    ] {
        check(at(line), &interface, &traits, &not_traits);
    }
    // Types: parameters, fields, return types, bindings, consts and type arguments.
    for line in [
        "fn f(a: $",
        "fn f(a: Int, b: $) -> Int",
        "fn f[A](a: $",
        "fn f() -> $",
        "fn f(a: Int) -> $ {\n}",
        "fn f(g: fn($",
        "fn f(g: fn(Int) -> $",
        "fn f(a: List[$",
        "fn f(a: Map[Int, $",
        "type Tag(a: $",
        "error Bad(a: Int, b: $",
        "enum Shape { Dot(a: $",
        "enum Shape { Dot($",
        "const C: $",
        "  q: $",
        // The type arguments of an item of `with` and of the right of `by` (`[effect.with]`).
        "  with Quiet[$",
        "  with Quiet[Int, $",
        "  with Audit by Quiet[$",
    ] {
        check(at(line), &type_kinds, &types, &not_types);
    }
    // Value positions stay unrestricted: a named argument, a match arm, a list, a call argument.
    for line in [
        "  Point(x: $",
        "  match p { Point(x:, y:) -> $ }",
        "  [$",
        "  other($",
    ] {
        let (_, items) = at(line);
        let names = labels(&items);
        for name in ["p", "main", "match"] {
            assert!(names.contains(&name), "{line}: {name}: {names:?}");
        }
    }
    client.shutdown();
}

/// After `derive`, the derivable traits of std are offered once they are imported
/// (`[trait.derive]`, `[lsp.completion]`).
#[test]
fn completion_of_derive_offers_the_imported_traits_of_std() {
    let imported = "import Std.Dynamic.{Decode, Encode, ObjectKey}\nimport Std.Shape.{Shaped}\n\ntype Tag(a: Int) derive Eq, \n";
    let files = [("/app/Pome.toml", ""), (MAIN, imported)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    client.open(MAIN, 1, imported);
    let items = client.complete(MAIN, 3, 28).unwrap();
    let names = labels(&items);
    for name in ["Encode", "Decode", "ObjectKey", "Shaped", "Show"] {
        assert!(names.contains(&name), "{name}: {names:?}");
    }
    let plain = "type Tag(a: Int) derive Eq, \n";
    client.change(MAIN, 2, plain);
    let items = client.complete(MAIN, 0, 28).unwrap();
    let names = labels(&items);
    assert!(!names.contains(&"Encode"), "{names:?}");
    assert!(names.contains(&"Show"), "{names:?}");
    client.shutdown();
}
