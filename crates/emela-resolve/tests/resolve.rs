//! 名前解決の結果（HIR のダンプと診断）をスナップショットで確かめる．
//! 例は仕様の 4.4，5章，6章，7.3，8.2〜8.5，10章から取る．

use std::fmt::Write;
use std::path::Path;

use emela_resolve::{
    BuiltinKind, BuiltinTable, MemoryFs, build_import_graph, collect_modules, dump_module, imports,
    resolve,
};
use la_arena::ArenaMap;

/// 仮の組み込みのモジュール．core の表（#122）が入るまでのもの．
fn builtins() -> BuiltinTable {
    BuiltinTable::new()
        .functions("String", &["contains", "concat", "length"])
        .functions(
            "List",
            &["map", "filter", "fold", "partition", "get", "tail", "each"],
        )
        .functions("Option", &["or_fail", "unwrap_or"])
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

/// `src/` の下のファイルを解決し，モジュールごとのダンプと診断を返す．構文の誤りは許さない．
fn run(files: &[(&str, &str)]) -> String {
    let fs = MemoryFs::new(files.iter().map(|(path, _)| format!("src/{path}")));
    let (modules, diagnostics) = collect_modules(&fs, Path::new("src"));
    assert_eq!(diagnostics, []);
    let mut trees = ArenaMap::default();
    let mut import_lists = ArenaMap::default();
    for (id, data) in modules.iter() {
        let path = data.file.strip_prefix("src").unwrap().to_str().unwrap();
        let text = files.iter().find(|(p, _)| *p == path).unwrap().1;
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
    for (id, data) in modules.iter() {
        writeln!(out, "== {} ({})", data.name, data.file.display()).unwrap();
        out.push_str(&dump_module(&program, &modules, id));
    }
    if !diagnostics.is_empty() {
        out.push_str("== diagnostics\n");
    }
    for d in &diagnostics {
        let path = d.file.strip_prefix("src").unwrap().to_str().unwrap();
        let text = files.iter().find(|(p, _)| *p == path).unwrap().1;
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

/// 診断だけを返す．
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
  match n {
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
  match xs {
    []           -> []
    [x, ..rest]  -> [f(x), ..map(rest, f)]
  }
}

fn connect(host: String, port: Int) -> () { () }

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
    |> List.fold(init: 0, f: add)
  doubled = List.map(xs, fn(x) { x * 2 })
  "#{label}: #{total}"
}

enum Shape {
  Circle(radius: Float)
  Rect(Float, Float)
  Empty
}

fn area(shape: Shape) -> Float {
  match shape {
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

pub handler ConsoleLogger implements Logger {
  fn info(msg) { console_log(msg) }
}

@external(js, "console.log")
fn console_log(msg: String) -> ()

@external(js, "pg.connect")
fn pg_connect(url: String) -> Connection

@external(js, "pg.close")
fn pg_close(conn: Connection) -> ()

@external(js, "Connection")
type Connection

pub handler PostgresUsers(conn: Connection) implements Users {
  init {
    config = use Config
    PostgresUsers(conn: pg_connect(config.db_url()))
  }

  release {
    pg_close(self.conn)
  }

  fn find(id) { todo() }
}

pub handler EnvConfig implements Config {
  fn db_url() { "postgres://" }
}

pub handler JsClock implements Clock {
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
    match Compare.compare(self, other) {
      Less -> other
      _    -> self
    }
  }
}

type User(id: Int, name: String)
  derive Eq, Show

impl Show for User {
  fn show(self) { "User(#{self.name})" }
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
  match x {
    Nothing -> 1
    MISSING -> 2
  }
}

fn g[A](a: B) -> A fails NotFonud use Loger { a }
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
  match (1, 2) {
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
            "import Email\nimport Email.{Email, Raw}\n\nfn f(e: Email) {\n  a = Email(value: \"x\")\n  b = Email.Email(value: \"y\")\n  c = Raw(\"z\")\n  Email(value:) = e\n  match Email.make() {\n    Email(value: v) -> v\n  }\n}\n",
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
handler Console implements User {
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

impl Show for User {
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
    // 全ノードがモジュールと範囲を持ち，範囲はソースの中にある．
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
