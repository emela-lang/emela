//! Hover: types, errors, effects, handlers and what values hold.

use emela_driver::{ParseOnly, Typecheck};
use lsp_types as lsp;

use super::{Client, MAIN, range};

/// Hover shows the type and E (spec 9.3).
#[test]
fn hover_shows_types_and_errors() {
    let src = "error NotFound(id: Int)\n\nfn find(id: Int) -> Int fails NotFound {\n  if id > 0 { id } else { fail NotFound(id:) }\n}\n\nfn main() {\n  n = find(1)\n  List.map([n], |x| x)\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    assert_eq!(
        emela_lsp::capabilities().hover_provider,
        Some(lsp::HoverProviderCapability::Simple(true))
    );
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    // A definition's name shows the declared type.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 4)),
        "```emela\nfind: fn(Int) -> Int fails NotFound\n```"
    );
    // Inferred E.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 4)),
        "```emela\nmain: fn() -> List[Int] fails NotFound\n```"
    );
    // A call expression shows its type and E; the range is the expression.
    let hover = client.hover("/app/src/main.emel", 7, 12).unwrap();
    assert_eq!(hover.range, Some(range((7, 6), (7, 13))));
    assert_eq!(text(Some(hover)), "```emela\nInt\nfails NotFound\n```");
    // A local variable.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 2)),
        "```emela\nn: Int\n```"
    );
    // A reference to a generic function shows the generalized type, including the invisible sets
    // of function arguments. Its documentation follows.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 8)),
        "```emela\nmap: ∀ A, B, E(f), R(f). fn(List[A], fn(A) -> B fails E(f) uses { R(f) }) -> List[B] fails E(f) uses { R(f) }\n```\n\n---\n\nApplies `f` to each element."
    );
    // A position with nothing there.
    assert_eq!(client.hover("/app/src/main.emel", 1, 0), None);
    // null for frontends without types.
    client.shutdown();
    let mut client = Client::initialize(&files, Some("/app"), ParseOnly::new);
    assert_eq!(client.hover("/app/src/main.emel", 2, 4), None);
    client.shutdown();
}

/// A label shows its declaration, at the declaration and at references (issue #255). Types keep
/// showing the label itself.
#[test]
fn hover_on_labels() {
    let src = "effect Log {\n  fn log(msg: String) -> ()\n}\n\neffect Debug of Log default\n\nfn b() -> () uses { Debug } {\n  with Debug { (use Log).log(\"b\") }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    let decl = "```emela\neffect Debug of Log default\n```";
    assert_eq!(text(client.hover("/app/src/main.emel", 4, 8)), decl);
    // In the `uses` clause, and in `with`.
    assert_eq!(text(client.hover("/app/src/main.emel", 6, 20)), decl);
    assert_eq!(text(client.hover("/app/src/main.emel", 7, 8)), decl);
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 3)),
        "```emela\nb: fn() -> () uses { Debug }\n```"
    );
    client.shutdown();
}

/// Hover over a negative number pattern (spec 6.5) and the arm around it.
#[test]
fn hover_on_negative_number_pattern() {
    let src = "fn sign(n: Int) -> Int {\n  n match {\n    -1 -> n\n    1 -> 1\n    _ -> 0\n  }\n}\n\nfn main() {\n  sign(-1)\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| {
        hover.map(|h| match h.contents {
            lsp::HoverContents::Markup(m) => (m.value, h.range),
            other => panic!("{other:?}"),
        })
    };
    // On the `-` and on the digits, the same as a positive literal pattern (`1`).
    let positive = text(client.hover("/app/src/main.emel", 3, 4));
    assert_eq!(
        positive,
        Some(("```emela\nInt\n```".to_owned(), Some(range((1, 2), (5, 3)))))
    );
    assert_eq!(text(client.hover("/app/src/main.emel", 2, 4)), positive);
    assert_eq!(text(client.hover("/app/src/main.emel", 2, 5)), positive);
    // The arm's body after the pattern.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 10)),
        Some((
            "```emela\nn: Int\n```".to_owned(),
            Some(range((2, 10), (2, 11)))
        ))
    );
    client.shutdown();
}

#[test]
fn hover_shows_effects() {
    let src = "effect Logger {\n  fn info(msg: String) -> ()\n}\n\nhandler Quiet: Logger {\n  fn info(msg) { () }\n}\n\nfn greet() {\n  log = use Logger\n  log.info(\"hi\")\n}\n\nfn main() {\n  with Quiet { greet() }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    // Inferred R.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 4)),
        "```emela\ngreet: fn() -> () uses { Logger }\n```"
    );
    // A local variable holding a capability, provided by the caller (`[lsp.hover]`).
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 2)),
        "```emela\nlog: use Logger\ncapabilities: Logger\n```"
    );
    // An operation call expression shows its type and R.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 10, 16)),
        "```emela\n()\nuses { Logger }\n```"
    );
    // A call inside `with` shows R; outside it does not.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 21)),
        "```emela\n()\nuses { Logger }\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 3)),
        "```emela\n()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 13, 4)),
        "```emela\nmain: fn() -> ()\n```"
    );
    client.shutdown();
}

/// `[lsp.hover]`, `[effect.external]`: External is shown with its module, in the module that
/// declares the external function and in another.
#[test]
fn hover_shows_external_with_its_module() {
    let log = "@external(js, \"./h.mjs\", \"hostLog\")\npub fn host_log(s: String) -> ()\n\nfn tagged(s: String) { host_log(s) }\n";
    let main = "import Log\n\nfn later() { Log.host_log }\n";
    let files = [
        ("/app/Pome.toml", ""),
        ("/app/src/log.emel", log),
        ("/app/src/main.emel", main),
    ];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/log.emel", 3, 4)),
        "```emela\ntagged: fn(String) -> () uses { Log.External }\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 2, 4)),
        "```emela\nlater: fn() -> fn(String) -> () uses { Log.External }\n```"
    );
    client.shutdown();
}

/// `[lsp.hover]`, `[type.rules]`: the capabilities the type of a value records, as a line
/// `capabilities: ...`: the variables of named items, the handler of an item without a name; and
/// the capabilities in the R of a closure by the same names.
#[test]
fn hover_shows_what_values_hold() {
    let src = "\
effect Db {
  fn query(q: String) -> Int
}

handler PrimaryDb: Db {
  fn query(q) { 1 }
}

handler ReplicaDb: Db {
  fn query(q) { 2 }
}

fn main() {
  with x = PrimaryDb, y = ReplicaDb {
    dbs = [x, y]
    f = || x.query(\"x\")
    n = f()
    ()
  }
  with PrimaryDb {
    p = use Db
    ()
  }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 4)),
        "```emela\ndbs: List[use Db]\ncapabilities: x, y\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 15, 4)),
        "```emela\nf: fn() -> Int uses { x }\n```"
    );
    // A value whose type cannot contain a capability records none.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 16, 4)),
        "```emela\nn: Int\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 20, 4)),
        "```emela\np: use Db\ncapabilities: PrimaryDb\n```"
    );
    client.shutdown();
}

/// `[lsp.hover]`, `[effect.use]`: the type of a capability value is shown as written, `use E`, and
/// the capabilities of the parameters in the R of a function by their names.
#[test]
fn hover_shows_capability_types() {
    let src = "\
effect Db {
  fn query(q: String) -> Int
}

fn sync_users(from: use Db, to: use Db) -> Int {
  from.query(\"a\") + to.query(\"b\")
}

fn pick(dbs: List[use Db]) -> Option[use Db] {
  dbs match {
    [db, ..] -> Some(db)
    _ -> None
  }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 4, 4)),
        "```emela\nsync_users: fn(use Db, use Db) -> Int uses { from, to }\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 4, 15)),
        "```emela\nfrom: use Db\ncapabilities: from\n```"
    );
    let pick = text(client.hover("/app/src/main.emel", 8, 4));
    assert!(
        pick.starts_with("```emela\npick: fn(List[use Db]) -> Option[use Db]\n"),
        "{pick}"
    );
    client.shutdown();
}

/// Handler members (`[effect.decl]`, `[effect.with]`): a name bound by `with` is the capability
/// of the handler opened, `Files.Open` goes to the member's declaration, and the outline lists
/// handler members and inner handlers.
#[test]
fn handler_members_in_the_editor() {
    let src = "\
effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
}

handler MemFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }
}

fn count(path: String) -> Int {
  with f = Files.Open(path) {
    f.read(1)
  }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    let bound = text(client.hover("/app/src/main.emel", 16, 7));
    assert!(bound.starts_with("```emela\nf: use ReadFile"), "{bound}");
    let member = client
        .definition("/app/src/main.emel", 16, 18)
        .expect("a definition");
    assert_eq!(member.range.start, lsp::Position::new(5, 10));
    let symbols = client
        .document_symbols("/app/src/main.emel")
        .expect("symbols");
    let names = |s: &lsp::DocumentSymbol| -> Vec<String> {
        s.children
            .iter()
            .flatten()
            .map(|c| c.name.clone())
            .collect()
    };
    assert_eq!(names(&symbols[1]), ["Open"]);
    assert_eq!(names(&symbols[2]), ["Open"]);
    client.shutdown();
}

/// Suspending operations and handlers that leave Async (spec 8.4, 8.7). Operation types get
/// `suspend`, and handler names show their dependencies.
#[test]
fn hover_shows_suspend_and_async() {
    let src = "effect Clock {\n  suspend fn sleep(ms: Int) -> ()\n}\n\n@external(js, \"setTimeout\")\nfn set_timeout(ms: Int, f: fn() -> ()) -> ()\n\nhandler JsClock: Clock {\n  suspend fn sleep(ms, resume) {\n    set_timeout(ms, || resume(()))\n  }\n}\n\nfn main() {\n  with JsClock { (use Clock).sleep(1) }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 1, 14)),
        "```emela\nsleep: suspend fn(Int) -> ()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 10)),
        "```emela\nhandler JsClock: Clock\nuses { Async }\n```"
    );
    // `resume` takes the operation's return type and returns `()`.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 24)),
        "```emela\nresume: fn(()) -> ()\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 31)),
        "```emela\nsuspend fn(Int) -> () uses { Clock }\nuses { Clock }\n```"
    );
    // Async may remain in main's R (spec 8.3).
    assert_eq!(
        text(client.hover("/app/src/main.emel", 13, 4)),
        "```emela\nmain: fn() -> () uses { Async }\n```"
    );
    client.shutdown();
}

/// Stateful handlers and layers (spec 8.4, 8.5). A handler's name shows its fields, the E of
/// `init` and its dependencies; a layer's name shows its handlers in initialization order and
/// the unmet dependencies; `self` shows the handler value's type.
#[test]
fn hover_shows_stateful_handlers_and_layers() {
    let src = "error Down\n\neffect Config {\n  fn url() -> String\n}\n\neffect Users {\n  fn find(id: Int) -> String\n}\n\nfn connect(url: String) -> String fails Down { url }\n\nhandler DbUsers(conn: String): Users {\n  init { DbUsers(conn: connect((use Config).url())) }\n  fn find(id) { self.conn }\n}\n\nlayer App {\n  DbUsers\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 12, 10)),
        "```emela\nhandler DbUsers(conn: String): Users\ninit fails Down\nuses { Config }\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 14, 16)),
        "```emela\nself: DbUsers\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 17, 7)),
        "```emela\nlayer App { DbUsers }\nuses { Config }\n```"
    );
    client.shutdown();
}

/// `[effect.handler]`: the hover on a handler shows the parameters of its init, a parameter of
/// init has its type, and a name in
/// the arguments of a `with` item goes to its definition.
#[test]
fn handler_init_arguments() {
    let src = "effect Kv {\n  fn get(key: String) -> String\n}\n\nconst NAME: String = \"KV\"\n\nhandler WorkerKv(binding: String): Kv {\n  init(binding: String, retries: Int) { WorkerKv(binding:) }\n  fn get(key) { self.binding }\n}\n\nfn main() {\n  with WorkerKv(binding: NAME, retries: 1) { (use Kv).get(\"a\") }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 10)),
        "```emela\nhandler WorkerKv(binding: String): Kv\ninit(binding: String, retries: Int)\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 7, 8)),
        "```emela\nbinding: String\n```"
    );
    let def = client
        .definition("/app/src/main.emel", 12, 26)
        .expect("a definition");
    assert_eq!(def.range, range((4, 6), (4, 10)));
    client.shutdown();
}

/// Effects with type arguments (spec 8.1). The handler name shows `handler H: Cell[Int]`, a
/// capability shows `use Cell[Int]`, and an operation shows its type with the arguments substituted.
#[test]
fn hover_shows_effect_type_args() {
    let src = "effect Cell[S] {\n  fn get() -> S\n}\n\nhandler Zero: Cell[Int] {\n  fn get() { 0 }\n}\n\nfn read() -> Int uses { Cell[Int] } {\n  cell = use Cell[Int]\n  cell.get()\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 4, 9)),
        "```emela\nhandler Zero: Cell[Int]\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 3)),
        "```emela\ncell: use Cell[Int]\ncapabilities: Cell\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 10, 8)),
        "```emela\nfn() -> Int uses { Cell[Int] }\n```"
    );
    client.shutdown();
}

/// Handlers with type parameters (`[lsp.hover]`, `[lsp.inlay-hint]`): the declaration shows its
/// type parameters, an item of a `with`, of a layer or the right of `by` shows them substituted
/// with the type arguments of that item, `self` has the handler applied to its type parameters,
/// and type arguments that are not written are shown after the handler's name.
#[test]
fn generic_handlers_in_hover_and_inlay_hints() {
    let src = "effect Cell[S] {
  fn get() -> S
}

effect Hits of Cell[Int]

handler Memory[S](value: S): Cell[S] {
  init(value: S) { Memory(value:) }
  fn get() { self.value }
}

layer Both {
  Memory(value: \"a\")
}

fn read() -> Int uses { Cell[Int] } {
  c <- Cell[Int]
  c.get()
}

fn main() {
  with Memory(value: 1) { read() }
  with Memory[Int](value: 2) { read() }
  with Hits by Memory(value: 3) { () }
}
";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 6, 9)),
        "```emela\nhandler Memory[S](value: S): Cell[S]\ninit(value: S)\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 14)),
        "```emela\nself: Memory[S]\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 12, 3)),
        "```emela\nhandler Memory[String](value: String): Cell[String]\ninit(value: String)\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 21, 8)),
        "```emela\nhandler Memory[Int](value: Int): Cell[Int]\ninit(value: Int)\n```"
    );
    assert_eq!(
        text(client.hover("/app/src/main.emel", 23, 16)),
        "```emela\nhandler Memory[Int](value: Int): Cell[Int]\ninit(value: Int)\n```"
    );
    let hints: Vec<_> = client
        .inlay_hints(MAIN)
        .unwrap()
        .into_iter()
        .filter(|h| h.2.starts_with('['))
        .collect();
    let hint = |line, col, label: &str| (line, col, label.to_owned(), false);
    assert_eq!(
        hints,
        [
            hint(12, 8, "[String]"),
            hint(21, 13, "[Int]"),
            hint(23, 21, "[Int]"),
        ]
    );
    client.shutdown();
}

/// A handler of several effects is shown with its effects joined by `+`, and a capability of
/// several effects as `use A + B` (`[effect.handler]`, `[effect.use]`).
#[test]
fn hover_of_several_effects() {
    let src = "effect Cell[S] {\n  fn get() -> S\n}\n\neffect Log {\n  fn note(msg: String) -> ()\n}\n\nhandler Both: Log + Cell[Int] {\n  fn get() { 0 }\n  fn note(msg) { () }\n}\n\nfn read(tx: use Cell[Int] + Log) -> Int {\n  tx.note(\"a\")\n  tx.get()\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 9)),
        "```emela\nhandler Both: Log + Cell[Int]\n```"
    );
    assert!(text(client.hover("/app/src/main.emel", 13, 8)).contains("tx: use Cell[Int] + Log"),);
    client.shutdown();
}

/// A uses clause mixing effects and set type parameters (spec 8.3). Inside `with` the listed effect
/// is visible; outside it only the set type parameter remains.
#[test]
fn hover_shows_mixed_effect_sets() {
    let src = "effect Quiet {\n  fn hush() -> ()\n}\n\nhandler Silent: Quiet {\n  fn hush() { () }\n}\n\nfn run[R](f: fn() -> () uses { Quiet, R }) -> () uses { R } {\n  with Silent { f() }\n}\n";
    let files = [("/app/Pome.toml", ""), ("/app/src/main.emel", src)];
    let mut client = Client::initialize(&files, Some("/app"), Typecheck::default);
    let text = |hover: Option<lsp::Hover>| match hover.map(|h| h.contents) {
        Some(lsp::HoverContents::Markup(m)) => m.value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        text(client.hover("/app/src/main.emel", 8, 4)),
        "```emela\nrun: ∀ R, E(f). fn(fn() -> () fails E(f) uses { Quiet, R }) -> () fails E(f) uses { R }\n```"
    );
    // The call expression's R includes the set type parameter.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 17)),
        "```emela\n()\nfails E(f)\nuses { Quiet, R }\n```"
    );
    // Outside `with`, Quiet is gone.
    assert_eq!(
        text(client.hover("/app/src/main.emel", 9, 3)),
        "```emela\n()\nfails E(f)\nuses { R }\n```"
    );
    client.shutdown();
}
