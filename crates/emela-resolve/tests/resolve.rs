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
  doubled = List.map(xs, |x| x * 2)
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

fn update[A, R: Immediate](x: A, f: fn(A) -> A uses { R }) -> A uses { R } {
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
  z = Lisst.map([], |a| a)
  w = List.mapp([], |a| a)
  v = Show.shw(1)
  u = Undefined
  x match {
    Nothing -> 1
    MISSING -> 2
  }
}

fn g[A](a: B) -> A fails NotFonud uses { Loger } { a }
"##,
    )]));
}

/// The top-level pattern of an escape arm is limited to errors (E0217); `_` and a variable pass
/// name resolution (`[module.resolution]`), and only the constructor that is not an error is
/// reported.
#[test]
fn escape_arm_patterns() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"type User(id: Int)
error NotFound(id: Int)

fn f() -> Int fails NotFound { fail NotFound(id: 1) }

fn g() {
  a = f() escape { _ -> 0 }
  b = f() escape { e -> 0 }
  c = f() escape { User(_) -> 0 }
}
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
  List.fold(xs, from: 0, f: |acc, (a, _): (Int, Int)| acc + a)
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
  g = |x, x| x
  h = |x, (x, y)| y
  k = |(a, b), [a, ..]| b
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

fn f(x: Logger) -> Int fails User uses { Console } {
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
pub opaque type Raw

@intrinsic
fn with_body() -> Int { 1 }
"##,
    )]));
}

/// `[kw.annotation]`: in the bundled sources, `@intrinsic` may be put on a type without fields (a
/// built-in type such as `Bytes`), not on one with fields (E0225).
#[test]
fn intrinsic_types_in_core() {
    insta::assert_snapshot!(run_with_core(
        &[("main.emel", "fn size(b: Raw.Raw) -> Raw.Raw { b }\n",)],
        &[(
            "raw.emel",
            r##"@intrinsic
pub opaque type Raw

@intrinsic
pub type Cooked(n: Int)
"##,
        )],
    ));
}

/// `[kw.annotation]`, `[api.std.array]`: a built-in type of the bundled sources may take type
/// parameters (`Array[A]`).
#[test]
fn intrinsic_types_with_parameters_in_core() {
    insta::assert_snapshot!(run_with_core(
        &[(
            "main.emel",
            "fn first(s: Seq.Seq[Int]) -> Seq.Seq[Int] { s }\n",
        )],
        &[(
            "seq.emel",
            r##"@intrinsic
pub opaque type Seq[A]
"##,
        )],
    ));
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

/// In the module of a type, an fn whose first parameter has that type at its head cannot have
/// the name of one of its fields (E0334, `[data.type]`), so that a method call never finds both.
/// A fn of another module, or whose first parameter has another type, may.
#[test]
fn fn_named_like_a_field() {
    insta::assert_snapshot!(diagnostics(&[
        (
            "main.emel",
            r#"pub type Request(headers: List[String], url: String)

pub error NotFound(id: Int)

pub type Pair[A](left: A, right: A)

pub fn headers(req: Request, name: String) -> List[String] { req.headers }

fn url(n: Int) -> Int { n }

fn other(name: String, req: Request) -> Int { 0 }

fn id(e: NotFound) -> Int { e.id }

fn left[A](p: Pair[A]) -> A { p.left }
"#,
        ),
        (
            "other.emel",
            r#"import Main.{Request}

fn headers(req: Request) -> List[String] { req.headers }
"#,
        ),
    ]));
}

/// Every expression and pattern form lowers to its own HIR node: each operator, literal, string
/// shape, and each form a binding's left side takes as a pattern (spec 6.5, 17.6).
#[test]
fn lowering_forms() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"type Point(x: Int, y: Int)
const MAX: Int = 3

fn ops(a: Int, b: Bool, f: Float) -> Bool {
  c = a + a - a * a / a % a
  d = a == a && a != a || a < a && a <= a && a > a && a >= a
  e = !b
  g = -f
  h = a |> ops
  assert b
  i = 1_000 + 2
  j = 2.5
  k = ""
  l = "plain"
  m = "x #{a} y #{f}"
  b
}

fn binding_patterns(p: Point, xs: List[Int], s: String, n: Int, o: Option[Int]) {
  _ = p
  Option.None = o
  MAX = n
  "s" = s
  -1 = n
  () = ()
  Point(x:, y: _) = p
  [first, ..rest] = xs
  [only] = xs
}

fn match_patterns(n: Int, xs: List[Int], u: (), f: Float, p: Point, s: String) -> Int {
  n match {
    -1 -> 0
    0 -> 1
    MAX -> 2
    _ -> 3
  }
  xs match {
    [a, ..] -> a
    [] -> 0
  }
  f match {
    -1.5 -> 0
    _ -> 1
  }
  u match {
    () -> 0
  }
  p match {
    Point(x:, ..) -> x
  }
  s match {
    "a" -> 1
    _ -> 2
  }
}

fn update(p: Point) -> Point {
  Point(x: 1, ..p)
}
"##,
    )]));
}

/// Resolves one `main.emel` with no syntax or resolution errors.
fn program(src: &str) -> emela_resolve::hir::Program {
    let fs = MemoryFs::new(["src/main.emel"]);
    let (modules, _) = collect_modules(&fs, Path::new("src"));
    let mut trees = ArenaMap::default();
    for (id, _) in modules.iter() {
        let parse = emela_syntax::parse(src);
        assert!(parse.diagnostics().is_empty(), "{:?}", parse.diagnostics());
        trees.insert(id, parse.tree());
    }
    let (program, diagnostics) = resolve(&modules, &trees, &builtins());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    program
}

/// The source definition named `name` of kind `kind`.
fn def_named(
    program: &emela_resolve::hir::Program,
    name: &str,
    kind: emela_resolve::hir::DefKind,
) -> emela_resolve::hir::DefId {
    program
        .defs
        .iter()
        .find(|(_, d)| d.name == name && d.kind == kind)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no {kind:?} `{name}`"))
}

/// The queries type inference makes of the HIR: prelude names, the impl of an impl fn, and the
/// operations of an effect and of a label (spec 8.6).
#[test]
fn program_queries() {
    use emela_resolve::hir::{DefKind, Item};
    let program = program(
        r##"type User(id: Int)
impl User: Show {
  fn show(self) { "" }
}
fn show(u: User) -> String { "" }
effect Console {
  fn print(s: String) -> ()
}
effect Audit of Console
trait Named {}
fn make() -> User { User(id: 1) }
"##,
    );
    // Prelude names, constructors included; a source name is not one.
    let option = program.prelude_def("Option").unwrap();
    let some = program.prelude_def("Some").unwrap();
    assert_eq!(program.defs[option].name, "Option");
    assert!(matches!(program.item(some), Some(Item::Ctor(c)) if c.owner == option));
    assert_eq!(program.prelude_def("User"), None);
    // A variant without payload is a constructor of its enum too.
    let none = program.prelude_def("None").unwrap();
    assert!(matches!(program.item(none), Some(Item::Ctor(c)) if c.owner == option));

    let impl_show = def_named(&program, "show", DefKind::ImplFn);
    let (owner, f) = program.impl_fn(impl_show).unwrap();
    assert_eq!(program.defs[owner].kind, DefKind::Impl);
    assert_eq!(f.def, impl_show);
    assert_eq!(
        program.impl_fn(def_named(&program, "show", DefKind::Fn)),
        None
    );

    let console = def_named(&program, "Console", DefKind::Effect);
    let audit = def_named(&program, "Audit", DefKind::Effect);
    let print = def_named(&program, "print", DefKind::Op);
    assert_eq!(program.label_base(audit), Some(console));
    assert_eq!(program.label_base(console), None);
    assert_eq!(program.effect_ops(console), [print]);
    assert_eq!(program.effect_ops(audit), [print]);

    // The module sees its own trait and the Prelude's.
    let main = program.modules.iter().next().unwrap().1;
    let named = def_named(&program, "Named", DefKind::Trait);
    let show = program.prelude_def("Show").unwrap();
    assert!(main.traits.contains(&named) && main.traits.contains(&show));

    // Each written name records where it points: in a signature and in a body.
    let user = def_named(&program, "User", DefKind::Type);
    let user_ctor = def_named(&program, "User", DefKind::Ctor);
    let refs_to = |target| {
        program
            .refs
            .iter()
            .filter(|r| r.target == emela_resolve::hir::RefTarget::Def(target))
            .count()
    };
    // `impl User`, `u: User`, `-> User`.
    assert_eq!(refs_to(user), 3);
    // `User(id: 1)`.
    assert_eq!(refs_to(user_ctor), 1);
}

/// A name of the wrong kind in a with item, a label of `by`, the right of `by`, a trait position,
/// a type argument, and `use` of a type parameter.
#[test]
fn wrong_kinds_in_effect_and_trait_positions() {
    insta::assert_snapshot!(diagnostics(&[(
        "main.emel",
        r##"type User(id: Int)
effect Log {
  fn info(msg: String) -> ()
}
effect Audit of Log
handler Quiet: Log {
  fn info(msg) { () }
}
fn helper() { 1 }

fn f[Eff]() -> Int uses { Eff } {
  with User { 1 }
  with User by Quiet { 1 }
  with Audit by User { 1 }
  with Audit by Quiet { 1 }
  use Eff
  xs: List[Quiet] = []
  1
}

trait Named: User {}
impl User: User {}
fn g[A: User](a: A) -> Int { 1 }
"##,
    )]));
}

/// Types in every position: a function type with an error set and an effect set, set type
/// arguments (spec 9.2), and type arguments of an effect in uses.
#[test]
fn type_forms() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        r##"error Timeout
error Refused
effect Log {
  fn info(msg: String) -> ()
}
effect State[S] {
  fn get() -> S
}
type Box[A](value: A)

fn run[E, R](
  f: fn(Int) -> Box[Int] fails E | Timeout uses { R, Log },
  h: Handler[Timeout | Refused, { Log, State[Int] }],
) -> Int fails E uses { R, State[Box[Int]] } {
  1
}
"##,
    )]));
}

/// "Did you mean" for a name imported from a module looks at the module's own definitions, not at
/// what it imports; for the left of `.`, traits are candidates too.
#[test]
fn suggestions_for_imports_and_qualified_names() {
    insta::assert_snapshot!(diagnostics(&[
        ("lib.emel", "import Util.{opener}\npub fn open() { 1 }\n",),
        ("util.emel", "pub fn opener() { 1 }\n"),
        (
            "main.emel",
            "import Lib.{opne, openr}\n\nfn f() {\n  Shw.show(1)\n}\n",
        ),
    ]));
}

/// A built-in module with a type of its own name gives that type (`Map[K, V]`); one without
/// does not.
#[test]
fn eponymous_types_of_builtin_modules() {
    insta::assert_snapshot!(run(&[(
        "main.emel",
        "fn f(m: Map[Int, Int], t: Time) -> Int { 1 }\n",
    )]));
}

/// The pub definitions of a bundled `Prelude` replace the built-in Prelude's names; private ones
/// and the ones it imports do not.
#[test]
fn core_prelude_overrides_the_builtin_prelude() {
    insta::assert_snapshot!(run_with_core(
        &[(
            "main.emel",
            "fn f(o: Option[Int], h: Helper, s: Secret) -> Ordering { Less }\n",
        )],
        &[
            (
                "prelude.emel",
                "import Extra.{Helper}\npub enum Option[A] {\n  Some(A)\n  None\n}\ntype Secret(x: Int)\n",
            ),
            ("extra.emel", "pub type Helper(x: Int)\n"),
        ],
    ));
}

/// A number keeps its kind (an integer or a float), and the last expression of a block is its
/// value, not a statement (spec 6.3).
#[test]
fn literals_keep_their_kind_and_a_block_ends_in_its_value() {
    use emela_resolve::hir::{DefKind, ExprKind, Item, Literal};
    let program = program(
        "fn f() -> Float {\n  n = 1_000\n  2.5\n}\nfn g(x: Float) -> Int {\n  x match {\n    -1.5 -> 1\n    _ -> 2\n  }\n}\n",
    );
    let f = def_named(&program, "f", DefKind::Fn);
    let Some(Item::Fn(item)) = program.item(f) else {
        panic!("expected an fn")
    };
    let ExprKind::Block(block) = &program.exprs[item.body.unwrap()].kind else {
        panic!("expected a block")
    };
    assert_eq!(block.stmts.len(), 1);
    let tail = block.tail.expect("the last expression is the value");
    assert_eq!(
        program.exprs[tail].kind,
        ExprKind::Lit(Literal::Float("2.5".into()))
    );
    let ints: Vec<_> = program
        .exprs
        .iter()
        .filter_map(|(_, e)| match &e.kind {
            ExprKind::Lit(Literal::Int(n)) => Some(n.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(ints, ["1000", "1", "2"]);
    let float_pats: Vec<_> = program
        .pats
        .iter()
        .filter_map(|(_, p)| match &p.kind {
            emela_resolve::hir::PatKind::Lit(Literal::Float(n)) => Some(n.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(float_pats, ["-1.5"]);
}

/// Scopes and declarations at their edges: type parameters do not outlive their declaration, a
/// handler's field and an fn of the same name clash (E0334), `self` in a lambda inside an impl, a
/// duplicate operation keeps the first signature, a handler of a label does not look up its
/// operations, qualified traits, and the type arguments of `use` and of `by`.
#[test]
fn scopes_and_declarations() {
    insta::assert_snapshot!(run(&[
        ("lib.emel", "pub trait Named {}\npub type Thing(x: Int)\n",),
        (
            "main.emel",
            r##"import Lib
type User(id: Int)
effect Log {
  fn info(msg: String) -> ()
  fn info(code: Int) -> Int
}
effect Audit of Log
effect State[S] {
  fn get() -> S
}
handler Quiet: Audit {
  fn info(msg) { () }
}
handler Pg(conn: Int): Log {
  fn info(msg) { () }
}
enum Shape {
  Dot
}
effect Spawn {
  fn fork[T](x: T) -> T
  fn join(x: T) -> Int
}
@intrinsic
type Raw
@intrinsic
effect Hidden {}

@entry(Lib.Thing.make)
fn serve() { 1 }

fn first[A](a: A) -> A { a }
fn second(x: A) -> Int { 1 }
fn conn(p: Pg) -> Int { 1 }

impl User: Lib.Named {
  fn name(self) {
    f = |self| self
    1
  }
}
impl User: Lib.Thing {}

fn effects(o: Option[Int], p: User) -> Int uses { State[Int], Audit } {
  s = use State[Int]
  Some(v) = o
  q = Nope(id: 1, ..p)
  r = Dot(id: 1, ..p)
  with Audit by State[Int] { 1 }
  1
}
"##,
        ),
    ]));
}

/// The import paths of a file, for the import graph: one per `import`, with its segments.
#[test]
fn imports_lists_each_import_path() {
    let parse = emela_syntax::parse("import Http.Client\nimport Json.{decode}\n");
    let paths: Vec<String> = imports(&parse.tree())
        .iter()
        .map(|i| i.path.to_string())
        .collect();
    assert_eq!(paths, ["Http.Client", "Json"]);
}

/// Two imports that give the same name to different modules clash; importing one module twice
/// does not.
#[test]
fn import_aliases() {
    insta::assert_snapshot!(diagnostics(&[
        ("aa/item.emel", "pub fn f() { 1 }\n"),
        ("bb/item.emel", "pub fn g() { 1 }\n"),
        ("cc.emel", "pub fn h() { 1 }\n"),
        (
            "main.emel",
            "import Aa.Item\nimport Bb.Item\nimport Cc\nimport Cc\n"
        ),
    ]));
}

/// `@intrinsic` is allowed on effects and types without fields of the bundled sources; a core
/// module's type of its own name is its eponymous type only if it is pub and its own.
#[test]
fn core_intrinsics_and_eponymous_types() {
    insta::assert_snapshot!(run_with_core(
        &[("main.emel", "fn f(s: Sec[Int], b: Box) -> Int { 1 }\n",)],
        &[
            (
                "scope.emel",
                "@intrinsic\npub effect Scope {}\n@intrinsic\npub type Bytes\n"
            ),
            ("sec.emel", "type Sec[A](a: A)\n"),
            ("box.emel", "import Other.{Box}\n"),
            ("other.emel", "pub type Box(x: Int)\n"),
        ],
    ));
}

/// Resolves the files under `src/` with `core` bundled (as in [`run_with_core`]), and returns the
/// program and the module map.
fn program_with_core(
    files: &[(&str, &str)],
    core: &[(&str, &str)],
) -> (emela_resolve::hir::Program, emela_resolve::ModuleMap) {
    let fs = MemoryFs::new(files.iter().map(|(path, _)| format!("src/{path}")));
    let (mut modules, _) = collect_modules(&fs, Path::new("src"));
    for (file, _) in core {
        let stem = file.strip_suffix(".emel").unwrap();
        let name = ModuleName::new([to_type_name(stem).unwrap()]);
        modules.add_core(name, Path::new("<core>").join(file));
    }
    let mut trees = ArenaMap::default();
    for (id, data) in modules.iter() {
        let name = data.file.file_name().unwrap().to_str().unwrap();
        let text = files
            .iter()
            .chain(core)
            .find(|(p, _)| p.ends_with(name))
            .unwrap()
            .1;
        trees.insert(id, emela_syntax::parse(text).tree());
    }
    let (program, diagnostics) = resolve(&modules, &trees, &builtins());
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    (program, modules)
}

/// A bundled `Prelude` that defines a Prelude name replaces the built-in definition in
/// [`hir::Prelude::defs`], so the name is looked up there once.
#[test]
fn a_core_prelude_definition_replaces_the_builtin_one() {
    let (program, _) = program_with_core(
        &[("main.emel", "fn f(o: Option[Int]) -> Int { 1 }\n")],
        &[(
            "prelude.emel",
            "pub enum Option[A] {\n  Some(A)\n  None\n}\n",
        )],
    );
    let options: Vec<_> = program
        .prelude
        .defs
        .iter()
        .filter(|&&d| program.defs[d].name == "Option")
        .collect();
    assert_eq!(options.len(), 1);
    let option = program.prelude_def("Option").unwrap();
    assert!(matches!(
        program.defs[option].module,
        emela_resolve::hir::DefModule::Source(_)
    ));
}

/// Each segment of an import path records the module it names.
#[test]
fn import_paths_refer_to_their_modules() {
    use emela_resolve::hir::RefTarget;
    let (program, modules) = program_with_core(
        &[
            ("http/client.emel", "pub fn get() { 1 }\n"),
            ("http.emel", "pub fn h() { 1 }\n"),
            ("main.emel", "import Http.Client\n"),
        ],
        &[],
    );
    let named: Vec<String> = program
        .refs
        .iter()
        .filter_map(|r| match r.target {
            RefTarget::Module(m) => Some(modules[m].name.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(named, ["Http", "Http.Client"]);
}
