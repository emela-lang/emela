//! Snapshot tests of name resolution (HIR dumps and diagnostics).
//! Examples are taken from spec 4.4, chapters 5 and 6, 7.3, 8.2 to 8.5, and chapter 10.

use std::fmt::Write;
use std::path::Path;

use emela_resolve::{
    BuiltinKind, BuiltinTable, MemoryFs, ModuleName, build_import_graph, collect_modules,
    dump_module, imports, resolve, to_type_name,
};
use la_arena::ArenaMap;

/// A stand-in table of built-in modules, until the core table (#122) lands.
fn builtins() -> BuiltinTable {
    BuiltinTable::new()
        .functions("String", &["contains", "concat", "length"])
        .functions(
            "List",
            &["map", "filter", "fold", "partition", "get", "tail", "each"],
        )
        .functions("Option", &["or_fail", "or"])
        .module(
            "Map",
            &[("Map", BuiltinKind::Type), ("to_list", BuiltinKind::Fn)],
        )
        .functions("Int", &["to_string"])
        .module(
            "Time",
            &[("Instant", BuiltinKind::Type), ("now", BuiltinKind::Fn)],
        )
}

/// Resolves the files under `src/` and returns the dump and diagnostics per module. Syntax errors
/// are not allowed.
fn run(files: &[(&str, &str)]) -> String {
    run_with_core(files, &[])
}

/// `core` is the bundled core's sources (file name and contents), placed under `<core>/`.
/// Only the `src/` modules are dumped.
fn run_with_core(files: &[(&str, &str)], core: &[(&str, &str)]) -> String {
    let fs = MemoryFs::new(files.iter().map(|(path, _)| format!("src/{path}")));
    let (mut modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    for (file, _) in core {
        let stem = file.strip_suffix(".emel").unwrap();
        let name = ModuleName::new([to_type_name(stem).unwrap()]);
        let (_, shadowed) = modules.add_core(name, Path::new("<core>").join(file));
        assert_eq!(shadowed, None);
    }
    let text_of = |file: &Path| -> &str {
        if let Ok(path) = file.strip_prefix("<core>") {
            let path = path.to_str().unwrap();
            return core.iter().find(|(p, _)| *p == path).unwrap().1;
        }
        let path = file.strip_prefix("src").unwrap().to_str().unwrap();
        files.iter().find(|(p, _)| *p == path).unwrap().1
    };
    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    for (id, data) in modules.iter() {
        let path = data.file.to_str().unwrap();
        let text = text_of(&data.file);
        let parse = emela_syntax::parse(text);
        assert!(
            parse.diagnostics().is_empty(),
            "{path}: {:?}",
            parse.diagnostics()
        );
        import_lists.insert(id, imports(&parse.tree()));
        trees.insert(id, parse.tree());
    }
    let (_, graph_diagnostics) = build_import_graph(&modules, &import_lists);
    assert_eq!(graph_diagnostics, []);
    let (program, diagnostics) = resolve(&modules, &trees, &builtins());

    let mut out = String::new();
    for (id, data) in modules.iter().filter(|(id, _)| !modules.is_core(*id)) {
        writeln!(out, "== {} ({})", data.name, data.file.display()).unwrap();
        out.push_str(&dump_module(&program, &modules, id));
    }
    if !diagnostics.is_empty() {
        out.push_str("== diagnostics\n");
    }
    for d in &diagnostics {
        let text = text_of(&d.file);
        let range = d.range.unwrap();
        let level = if d.kind.is_warning() {
            "warning"
        } else {
            "error"
        };
        writeln!(
            out,
            "{level} {}@{:?} `{}`: {d}",
            d.file.display(),
            range,
            &text[range]
        )
        .unwrap();
    }
    out
}

fn diagnostics(files: &[(&str, &str)]) -> String {
    let out = run(files);
    match out.find("== diagnostics\n") {
        Some(i) => out[i + "== diagnostics\n".len()..].to_owned(),
        None => String::new(),
    }
}

#[test]
fn email_module_4_4() {
    insta::assert_snapshot!(run(&[
        (
            "email.emel",
            r##"pub opaque type Email(value: String)

pub error InvalidEmail(input: String)

pub fn parse(input: String) -> Email fails InvalidEmail {
  if String.contains(input, "@") {
    Email(value: input)
  } else {
    fail InvalidEmail(input:)
  }
}

pub fn to_string(email: Email) -> String {
  email.value
}
"##,
        ),
        (
            "main.emel",
            r##"import Email
import Email.{Email, InvalidEmail}

fn register(input: String) -> Email.Email fails InvalidEmail {
  Email.parse(input)
}

fn show(email: Email) -> String {
  Email.to_string(email)
}

fn safe(input: String) -> Option[Email] {
  Some(Email.parse(input)) escape {
    InvalidEmail(input: _) -> None
  }
}
"##,
        ),
    ]));
}

#[test]
fn data_chapter_5() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"type User(id: Int, name: String)

type Pair[A, B](first: A, second: B)

type Todo(
  id:        Int,
  title:     String,
  completed: Bool,
)

enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

enum Tree[Item] {
  Leaf
  Node(Tree[Item], Item, Tree[Item])
}

type Config(port: Int)

const MAX_RETRY = 3
const DEFAULT_CONFIG = Config(port: 8080)

fn make() -> (User, Pair[Int, String], Shape, ()) {
  (User(id: 1, name: "a"), Pair(1, second: "b"), Circle(1.0), ())
}

fn retry(n: Int) -> Bool {
  n match {
    MAX_RETRY -> False
    _         -> True
  }
}
"##,
    )]));
}

#[test]
fn functions_chapter_6() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"fn add(x: Int, y: Int) -> Int {
  x + y
}

fn map[A, B](xs: List[A], f: fn(A) -> B) -> List[B] {
  xs match {
    []           -> []
    [x, ..rest]  -> [f(x), ..map(rest, f)]
  }
}

fn connect(host: String, port: Int) -> () { () }

fn apply(f: (fn(Int) -> Int), x: Int) -> Int { f(x) }

fn is_even(n: Int) -> Bool { n % 2 == 0 }

fn calls(xs: List[Int], scores: Map[String, Int]) {
  connect("localhost", 5432)
  connect("localhost", port: 5432)
  port = 1
  connect(host: "localhost", port:)
  count = 0
  count = count + 1
  (evens, odds) = List.partition(xs, is_even)
  pairs: List[(String, Int)] = Map.to_list(scores)
  label = if count > 0 { "あり" } else { "なし" }
  total = xs
    |> List.filter(is_even)
    |> List.fold(from: 0, f: add)
  doubled = List.map(xs, fn(x) { x * 2 })
  "#{label}: #{total}"
}

enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn area(shape: Shape) -> Float {
  shape match {
    Circle(radius:) if radius > 0.0 -> 3.14 * radius * radius
    Circle(_)                       -> 0.0
    Rect(w, h)                      -> w * h
    Empty                           -> 0.0
  }
}

type User(id: Int, name: String)

fn name_of(user: User) -> String {
  User(name:, ..) = user
  name
}
"##,
    )]));
}

#[test]
fn errors_chapter_7() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"type User(id: Int)
type Data(id: Int)

error NotFound(id: Int)
error DbError(message: String)
error Unavailable(reason: String)

fn find_user(id: Int) -> User fails NotFound { fail NotFound(id:) }
fn guest_user() -> User { User(id: 0) }
fn load(id: Int) -> Data fails NotFound | DbError { fail DbError(message: "x") }
fn default_data() -> Data { Data(id: 0) }

fn rethrow(e: NotFound) -> User fails NotFound {
  fail e
}

fn main(id: Int) {
  user = find_user(id) escape {
    NotFound(_) -> guest_user()
  }
  data = load(id) escape {
    NotFound(_) -> default_data()
  }
  other = load(id) escape {
    DbError(message:) -> fail Unavailable(reason: message)
  }
  found = Some(find_user(id)) escape {
    NotFound(_) -> None
  }
  user
}
"##,
    )]));
}

#[test]
fn effects_chapter_8() {
    insta::assert_snapshot!(run(&[
        (
            "app.emel",
            r##"import Effects.{Logger, Users, Clock, Config, User, NotFound}

pub handler ConsoleLogger: Logger {
  fn info(msg) { console_log(msg) }
}

@external(js, "console.log")
fn console_log(msg: String) -> ()

@external(js, "pg.connect")
fn pg_connect(url: String) -> Connection

@external(js, "pg.close")
fn pg_close(conn: Connection) -> ()

@external
type Connection

pub handler PostgresUsers(conn: Connection): Users {
  init {
    config = use Config
    PostgresUsers(conn: pg_connect(config.db_url()))
  }

  release {
    pg_close(self.conn)
  }

  fn find(id) { todo() }
}

pub handler EnvConfig: Config {
  fn db_url() { "postgres://" }
}

pub handler JsClock: Clock {
  suspend fn sleep(ms, resume) {
    resume(())
  }
  fn now() { todo() }
}

pub layer AppLive {
  EnvConfig
  ConsoleLogger
  JsClock
  PostgresUsers
}

fn greet(id: Int) {
  log   = use Logger
  users = use Users

  user = users.find(id) |> Option.or_fail(NotFound(id:))
  log.info("hello #{user.name}")

  (use Clock).sleep(1000)
  user
}

fn main() {
  with AppLive { greet(1) }
  with AppLive, ConsoleLogger {
    primary = use Logger
    with ConsoleLogger {
      primary.info("x")
    }
  }
}
"##,
        ),
        (
            "effects.emel",
            r##"pub type User(id: Int, name: String)

pub error NotFound(id: Int)

pub effect Logger {
  fn info(msg: String) -> ()
}

pub effect Users {
  fn find(id: Int) -> Option[User]
}

pub effect Config {
  fn db_url() -> String
}

pub effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Time.Instant
}

fn update[A, R: Immediate](x: A, f: fn(A) -> A use R) -> A use R {
  f(x)
}
"##,
        ),
    ]));
}

#[test]
fn traits_chapter_10() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"pub trait Decode {
  fn decode(json: String) -> Self fails DecodeError
}

pub error DecodeError(message: String)

pub trait Compare: Eq {
  fn compare(self, other: Self) -> Ordering

  fn max(self, other: Self) -> Self {
    Compare.compare(self, other) match {
      Less -> other
      _    -> self
    }
  }
}

type User(id: Int, name: String)
  derive Eq, Show

impl User: Show {
  fn show(self) { "User(#{self.name})" }
}

type Pair[A, B](first: A, second: B)

impl Pair[A: Eq, B: Eq]: Eq {
  fn eq(self, other) { Eq.eq(self.first, other.first) }
}

fn largest[A: Ord + Show](xs: List[A]) -> Option[A] {
  todo()
}

fn describe(user: User) -> String {
  user |> Show.show
}
"##,
    )]));
}

#[test]
fn undefined_names() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"type User(id: Int)
error NotFound(id: Int)

fn f(count: Int) -> Strng {
  x = cout + 1
  y = Usr(id: 1)
  z = Lisst.map([], fn(a) { a })
  w = List.mapp([], fn(a) { a })
  v = Show.shw(1)
  u = Undefined
  x match {
    Nothing -> 1
    MISSING -> 2
  }
}

fn g[A](a: B) -> A fails NotFonud use Loger { a }
"##,
    )]));
}

/// A pattern in place of a lambda parameter name becomes a hidden parameter and a binding at the
/// head of the body (spec 6.1).
#[test]
fn lambda_param_patterns() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"fn f(xs: List[(Int, Int)]) -> Int {
  List.fold(xs, from: 0, f: fn(acc, (a, _): (Int, Int)) { acc + a })
}
"##,
    )]));
}

/// Names bound by different parameters of a lambda clash too (E0220, spec 6.1).
#[test]
fn lambda_param_duplicates() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"fn f() {
  g = fn(x, x) { x }
  h = fn(x, (x, y)) { y }
  k = fn((a, b), [a, ..]) { b }
}
"##,
    )]));
}

#[test]
fn duplicates() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"type User(id: Int, id: Int)
enum User { Admin }
fn f() { 1 }
fn f() { 2 }
effect Log {
  fn info() -> ()
  fn info() -> ()
}
fn g[A, A](x: Int, x: Int) {
  (y, y) = (1, 2)
  (1, 2) match {
    (z, z) -> z
  }
}
"##,
    )]));
}

#[test]
fn private_items() {
    insta::assert_snapshot!(diagnostics(&[
        (
            "lib.emel",
            "pub fn open() { 1 }\nfn secret() { 2 }\ntype Hidden(x: Int)\npub type Shown(x: Int)\n",
        ),
        (
            "main.emel",
            "import Lib\nimport Lib.{open, secret, Hidden, Shown, Missing}\n\nfn f() {\n  Lib.secret()\n  Lib.open()\n  Lib.nothing()\n}\n",
        ),
    ]));
}

#[test]
fn opaque_outside_module() {
    insta::assert_snapshot!(diagnostics(&[
        (
            "email.emel",
            "pub opaque type Email(value: String)\npub opaque enum Token {\n  Raw(String)\n}\npub fn make() -> Email { Email(value: \"\") }\nfn inside(e: Email) -> String {\n  Email(value:) = e\n  value\n}\n",
        ),
        (
            "main.emel",
            "import Email\nimport Email.{Email, Raw}\n\nfn f(e: Email) {\n  a = Email(value: \"x\")\n  b = Email.Email(value: \"y\")\n  c = Raw(\"z\")\n  Email(value:) = e\n  Email.make() match {\n    Email(value: v) -> v\n  }\n}\n",
        ),
    ]));
}

#[test]
fn constructor_collisions() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"enum Color {
  Red
  Green
}

enum Light {
  Red
  Yellow
}

type Green(x: Int)

enum Maybe[A] {
  Some(A)
  None
}

fn f() -> Maybe[Int] {
  Some(1)
}
"##,
    )]));
}

#[test]
fn type_param_shadowing() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"type Item(id: Int)

fn first[Item, A](xs: List[Item]) -> Option[Item] { todo() }

type Box[String](value: String)
"##,
    )]));
}

#[test]
fn wrong_kinds() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"type User(id: Int)
enum Shape {
  Empty
}
error NotFound(id: Int)
effect Logger {
  fn info(msg: String) -> ()
}
handler Console: User {
  fn info(msg) { self }
}
layer App { Logger, Console }

fn f(x: Logger) -> Int fails User use Console {
  log = use User
  with Logger { 1 }
  fail Empty
  y = 1 escape {
    Empty -> 2
  }
  Logger.info("x")
  z = Int
  s = self
  1
}

fn g(self) -> Self { 1 }

impl User: Show {
  fn display(self) { "" }
}
"##,
    )]));
}

#[test]
fn ambiguous_module_and_trait() {
    insta::assert_snapshot!(diagnostics(&[
        ("codec.emel", "pub fn encode() { 1 }\n"),
        (
            "main.emel",
            "import Codec\n\ntrait Codec {\n  fn encode(self) -> String\n}\n\nfn f() { Codec.encode() }\n",
        ),
    ]));
}

#[test]
fn spans_point_at_source() {
    let text = "fn f(x: Int) -> Int { x + 1 }\n";
    let fs = MemoryFs::new(["src/main.emel"]);
    let (modules, _) = collect_modules(&fs, Path::new("src"));
    let (id, _) = modules.iter().next().unwrap();
    let mut trees = ArenaMap::default();
    trees.insert(id, emela_syntax::parse(text).tree());
    let (program, diagnostics) = resolve(&modules, &trees, &builtins());
    assert_eq!(diagnostics, []);
    // Every node has a module and a range, and the range lies within the source.
    for (_, expr) in program.exprs.iter() {
        assert_eq!(expr.span.module, id);
        assert!(usize::from(expr.span.range.end()) <= text.len());
    }
    let texts: Vec<&str> = program
        .exprs
        .iter()
        .map(|(_, e)| &text[e.span.range])
        .collect();
    assert_eq!(texts, ["x", "1", "x + 1", "{ x + 1 }"]);
    let local = program.locals.iter().next().unwrap().1;
    assert_eq!(&text[local.span.range], "x");
}

#[test]
fn record_update_5_3() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"type User(id: Int, name: String)
error NotFound(id: Int, detail: String)
enum Shape {
  Circle(radius: Float)
}
effect Counter {
  fn get() -> Int
}
handler Fixed(value: Int): Counter {
  init { Fixed(value: 0) }
  fn get() {
    next = Fixed(value: 1, ..self)
    next.value
  }
}

fn rename(user: User) -> User {
  User(name: "b", ..user)
}

fn retag(e: NotFound) -> NotFound {
  NotFound(detail: "x", ..e)
}

fn bad(user: User, shape: Shape) {
  a = Circle(radius: 1.0, ..shape)
  b = rename(name: "c", ..user)
  c = User("c", ..user)
  d = user(name: "c", ..user)
  d
}
"##,
    )]));
}

/// A small core standing in for the bundled core sources.
const CORE: &[(&str, &str)] = &[
    (
        "option.emel",
        r##"pub fn or[A](o: Option[A], default: A) -> A {
  o match {
    Some(x) -> x
    None    -> default
  }
}

@intrinsic
pub fn is_some[A](o: Option[A]) -> Bool
"##,
    ),
    (
        "map.emel",
        r##"pub opaque type Map[K, V](entries: List[(K, V)])

@intrinsic
pub fn get[K, V](m: Map[K, V], key: K) -> Option[V]
"##,
    ),
    (
        "prelude.emel",
        r##"pub enum Ordering {
  Less
  Equal
  Greater
}

pub fn identity[A](x: A) -> A { x }
"##,
    ),
];

#[test]
fn core_modules_are_implicit() {
    insta::assert_snapshot!(run_with_core(
        &[
            ("util.emel", "pub fn twice(x: Int) -> Int { x + x }\n"),
            (
                "main.emel",
                r##"fn lookup(m: Map[String, Int]) -> Int {
  Option.or(Map.get(m, "a"), 0)
}

fn order(x: Int) -> Ordering {
  if Option.is_some(Some(x)) { Less } else { identity(Greater) }
}

fn not_imported() -> Int {
  Util.twice(1)
}
"##,
            ),
        ],
        CORE,
    ));
}

#[test]
fn intrinsic_only_in_core() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"@intrinsic
fn length(s: String) -> Int

@intrinsic
type Handle(id: Int)

@intrinsic
fn with_body() -> Int { 1 }
"##,
    )]));
}

/// `[effect.handler]`: the parameters of init are in scope in its body only. The arguments of
/// `with`, `layer` and the right of `by` are resolved where the item is written.
#[test]
fn handler_init_arguments() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"pub effect Kv {
  fn get(key: String) -> String
}

effect AuditKv of Kv

const NAME: String = "KV"

handler WorkerKv(binding: String): Kv {
  init(binding: String, retries: Int) { WorkerKv(binding:) }
  fn get(key) { self.binding }
}

layer AppLive {
  WorkerKv(binding: NAME, retries: 3)
}

fn main() {
  name = "KV"
  with WorkerKv(binding: name, retries: 1) { (use Kv).get("a") }
  with AppLive, AuditKv by WorkerKv(name, 2) { (use AuditKv).get("a") }
}
"##,
    )]));
}

/// `H(...)` at a `with` is a call of init, not a construction of the handler value, so it is not
/// a record update (E0222). The parameters of init are not seen outside it.
#[test]
fn handler_init_argument_errors() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"pub effect Kv {
  fn get(key: String) -> String
}

handler WorkerKv(binding: String): Kv {
  init(binding: String) { WorkerKv(binding:) }
  fn get(key) { binding }
}

fn main(base: Int) {
  with WorkerKv(binding: "a", ..base) { (use Kv).get("a") }
  with WorkerKv(binding: missing) { (use Kv).get("a") }
}
"##,
    )]));
}

/// `@export` and `@entry` (spec 12.7, 12.8): the export names are kept, and the argument of
/// `@entry` is looked up as a value, in the module or through a module path.
#[test]
fn export_and_entry() {
    insta::assert_snapshot!(run(&[
        (
            "main.emel",
            r##"import Http

@export(js, "add")
@export(js, "default", "add")
@export(c, "add")
pub fn add(a: Int, b: Int) -> Int { a + b }

@export(js, "default", "run")
fn local(app: fn(Int) -> Int, x: Int) -> Int { app(x) }

@entry(local)
@entry(Http.serve)
fn app(x: Int) -> Int { x }

@entry(missing)
fn unknown(x: Int) -> Int { x }

@entry(Http.missing)
fn unknown_member(x: Int) -> Int { x }

@entry
fn no_arg(x: Int) -> Int { x }

@entry("serve")
fn string_arg(x: Int) -> Int { x }
"##,
        ),
        (
            "http.emel",
            r##"@export(js, "default", "fetch")
pub fn serve(app: fn(Int) -> Int, x: Int) -> Int { app(x) }
"##,
        ),
    ]));
}
