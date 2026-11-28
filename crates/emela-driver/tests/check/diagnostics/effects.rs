//! Diagnostics of effects and handlers: the uses clause, suspension, stateful handlers, capability
//! types, handler members, effect aliases, tasks and external effects.

use emela_driver::{Lower, Resolve, Typecheck, render};

use super::{check_files, check_files_with_deprecated};

/// The diagnostics of an escape arm `_` and of a variable arm (E3203, W3101, W3201,
/// `[error.escape]`).
#[test]
fn escape_wildcard_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Boom

fn steady() -> Int { 1 }
fn boom() -> Int fails Boom { fail Boom }

fn main() {
  a = steady() escape { _ -> 0 }
  b = boom() escape {
    _ -> 0
    Boom -> 1
  }
  c = boom() escape { _ where a > 0 -> 0 }
  d = boom() escape { e -> 0 }
  dbg((a, b, c, d))
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of effect diagnostics (E4xxx). The driver also passes main and test functions to
/// inference to check that their R is empty (spec 8.3, 13.3).
#[test]
fn effect_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
  fn warn(msg: String) -> ()
}

effect Clock {
  fn now() -> Int
}

handler Half: Logger {
  fn info(msg) { () }
  fn info(msg) { () }
}

handler Stamped: Logger {
  fn info(a, b) { (use Clock).now()
  () }
  fn warn(msg) { () }
}

handler Ticking: Clock {
  fn now() { (use Logger).info("tick")
  0 }
}

@external(js, "console.log")
fn console_log(text: String) -> ()

fn only(f: fn() -> () uses {}) -> () uses {} {
  (use Logger).infoo("x")
  f()
}

fn misuse() {
  log = use Logger
  only(|| log.info("x"))
  console_log("x")
  xs = [log]
  Some(log)
  with Stamped, Ticking { () }
  log
}

fn main() {
  (use Clock).now()
}

@test
fn logs() {
  (use Logger).info("t")
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The diagnostics about the uses clause (`[grammar.decl]`, `[diag.list]`): the old clause
/// (E1404) in a signature, in a function type, on its own line and on an effect operation, read
/// on as a uses clause; and the sets of E4301, E4301 and E4008 shown in the canonical
/// form `uses { ... }`.
#[test]
fn uses_clause_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
}

effect Db {
  fn get() -> Int use Logger
}

effect State[S] {
  fn get() -> S
}

fn old() -> Int use Db {
  (use Db).get()
}

fn own_line() -> Int
  use { Db, Logger } {
  (use Db).get()
}

fn old_type(f: fn() -> () use {}) -> () uses {} {
  f()
}

fn undeclared() -> () uses {} {
  (use Logger).info("x")
}

fn two() -> () uses {} {
  (use Logger).info("x")
  (use Db).get()
  ()
}

fn mismatch() {
  old_type(|| (use Logger).info("x"))
}

fn untyped() -> Int uses State {
  1
}

@external(js, "console.log")
fn console_log(text: String) -> ()

fn outside() {
  console_log("x")
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of suspend, Async, Immediate and declaration type argument set diagnostics
/// (E2110, E4304 to E3008, E4003 to E4302). Only Async may remain in main's R (spec 8.3).
#[test]
fn suspend_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "./ffi.mjs", "fetchText")
suspend fn fetch_text(url: String) -> String

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, || resume(()))
  }
  suspend fn now(resume) { resume(0) }
}

handler Runtime: Async {
}

effect Http {
  fn get(url: String) -> String
}

handler JsHttp: Http {
  fn get(url) { fetch_text(url) }
}

effect Ticks {
  fn tick() -> Int
}

fn stamp() -> Int uses { Clock } { (use Clock).now() }

handler Stamped: Ticks {
  fn tick() { stamp() + (use Clock).now() }
}

fn update[A, R: Immediate](x: A, f: fn(A) -> A uses { R }) -> A uses { R } { f(x) }

fn not_effect[A: Immediate](x: A) -> A { x }

type Handler[E, R](run: fn() -> () fails E uses { R })

fn handlers(a: Handler[Int, Clock], b: Handler[Clock, Clock], c: List[Clock], d: List[{}]) { () }

fn misuse(g: fn(Int) -> Int) {
  use Async
  fetch_text("x")
  update(1, |n| {
    (use Clock).sleep(n)
    n
  })
  update(1, g)
}

fn main() {
  with SystemClock { (use Clock).sleep(1) }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of stateful handler and layer diagnostics (E4101, E4005 to E4103; spec 8.4, 8.5).
#[test]
fn stateful_handler_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Boom

effect Logger {
  fn info(msg: String) -> ()
}

effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

fn risky() -> () fails Boom { fail Boom }

handler Counter(n: Int): Logger {
  init { Counter(n: 0) }
  release { risky() }
  fn info(msg) { () }
}

handler NoInit(n: Int): Logger {
  fn info(msg) { () }
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler HPing: Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong: Pong {
  fn pong() { (use Ping).ping() }
}

layer Loud {
  Counter
  Quiet
}

layer Twice { Quiet, Quiet }

layer LoopA { LoopB }

layer LoopB { LoopA }

layer PingPong {
  HPing
  HPong
}

fn make() { Counter(n: 1) }
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of the diagnostics of handlers of several effects (E4012, E4012, E4403, and E2102,
/// E4401, E4001, E4102, E4203 with several effects; `[effect.handler]`, `[effect.use]`,
/// `[effect.layer]`, `[effect.label]`).
#[test]
fn multi_effect_handler_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Query {
  fn query(sql: String) -> Int
}

effect Execute {
  fn execute(sql: String) -> ()
}

effect Reader {
  fn query(sql: String) -> Int
}

effect Replica of Query

handler Clash: Query + Reader {
  fn query(sql) { 0 }
}

handler Repeated: Execute + Execute {
  fn execute(sql) { () }
}

handler Half: Query + Execute {
  fn exec(sql) { () }
}

handler Plain: Query + Execute {
  fn query(sql) { 0 }
  fn execute(sql) { () }
}

handler Only: Execute {
  fn execute(sql) { () }
}

layer Both {
  Plain
  Only
}

fn read_write(rw: use Query + Execute) -> () { rw.execute("a") }

fn widen(q: use Query) -> () {
  read_write(q)
  q.execute("b")
}

fn wrong() -> Int {
  with Replica by Only { (use Replica).query("a") }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `Self` as an expression where it cannot construct a handler (`[effect.handler]`,
/// `[grammar.expr]`): inside a handler outside its init (E4005), outside a handler (E2108, also in
/// an impl), and as the target of a record update (E2111).
#[test]
fn handler_self_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
}

trait Make {
  fn make() -> Self
}

type Box(n: Int)

impl Box: Make {
  fn make() { Self(n: 1) }
}

handler Counter(n: Int): Logger {
  init { Self(n: 0, ..Self(n: 1)) }
  fn info(msg) {
    next = Self(n: self.n + 1)
    ()
  }
}

fn make() { Self(n: 1) }
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The messages of effect type parameter diagnostics (E4008, E4009, E3101, E4301; spec 8.1).
#[test]
fn effect_type_param_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Cell[S] {
  fn get() -> S
}

effect Raise[E] {
  fn guard(f: fn() -> () fails E) -> ()
}

handler Generic: Cell {
  fn get() { 0 }
}

handler Zero: Cell[Int] {
  fn get() { 0 }
}

fn omitted() { (use Cell).get() }

fn too_many() { (use Cell[Int, Int]).get() }

handler One: Cell[Int] {
  fn get() { 1 }
}

handler Empty: Cell[String] {
  fn get() { "" }
}

fn leftover() -> Int uses {} {
  with Zero {
    (use Cell[String]).get()
    (use Cell[Int]).get()
  }
}

layer Both {
  Zero
  Empty
}

layer Twice {
  Zero
  One
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn bundled_console_checks_cleanly() {
    let main = "import Std.Console.{Console, SystemConsole}\n\nfn main() {\n  with SystemConsole {\n    (use Console).print_line(\"hi\")\n  }\n}\n";
    let files = [("home/main.emel", main)];
    for analysis in [
        check_files(&files, "home/main.emel", &mut Resolve::default()),
        check_files(&files, "home/main.emel", &mut Typecheck::default()),
        check_files(&files, "home/main.emel", &mut Lower::default()),
    ] {
        assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    }
}

/// The messages of the diagnostics on the arguments of handler init (E4104, E4104, E3006, and
/// E3201 and E4301 in a layer; `[effect.handler]`, `[effect.layer]`).
#[test]
fn handler_init_argument_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"error Down

effect Config {
  fn url() -> String
}

effect Kv {
  fn get(key: String) -> String
}

handler WorkerKv(binding: String): Kv {
  init(binding: String, retries: Int) { WorkerKv(binding:) }
  fn get(key) { self.binding }
}

handler FakeKv: Kv {
  fn get(key) { "" }
}

handler Untyped(binding: String): Kv {
  init(binding) { Untyped(binding:) }
  fn get(key) { self.binding }
}

fn name() -> String fails Down { fail Down }

layer Failing {
  WorkerKv(binding: name(), retries: 1)
}

layer Reading {
  WorkerKv(binding: (use Config).url(), retries: 1)
}

layer Unconfigured {
  WorkerKv
}

fn main() {
  with WorkerKv { (use Kv).get("a") }
  with FakeKv() { (use Kv).get("a") }
  with WorkerKv(binding: "a") { (use Kv).get("a") }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `[effect.use]`, `[diag.list]`: the type `use E` names an effect; another kind of name is
/// E2106. An operation that E does not have, called on a value of type `use E`, is E4401. A
/// capability value passed, stored and returned is not an error,
/// and one that leaves the `with` that made it is E4405.
#[test]
fn capability_types() {
    let main = "\
effect Db {
  fn query(q: String) -> Int
}

handler PrimaryDb: Db {
  fn query(q) { 1 }
}

fn not_an_effect(a: use Int, b: use PrimaryDb) -> () { () }

fn wrong_op(db: use Db) -> () {
  db.info(\"x\")
}

fn values(db: use Db) -> List[use Db] {
  [db]
}

fn leaks() -> use Db {
  with PrimaryDb {
    db = use Db
    db
  }
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The diagnostics of capabilities told apart in types (`[effect.escape]`, `[effect.handler]`,
/// `[type.rules]`, `[ext.boundary]`, `[diag.list]`): a capability that leaves the `with` that
/// made it (E4405), a capability type in the result of an operation (E4406), `@borrowed` on a
/// parameter that is not a callback of an external function (E5004), a value of a
/// capability-bound type made outside a handler of its effect (E4407), and a capability in the R
/// of a callback (E5003).
#[test]
fn capability_identity() {
    let main = "\
effect Db {
  fn query(q: String) -> Int
  fn conn() -> use Db
}

type Ticket(n: Int) uses Db

handler PrimaryDb: Db {
  fn query(q) { 1 }
  fn conn() { panic(\"no\") }
}

@external(js, \"setTimeout\")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

fn plain(@borrowed f: fn() -> ()) { f() }

fn leaks() {
  with db = PrimaryDb {
    || db.query(\"x\")
  }
}

fn made() -> Ticket uses { Db } { Ticket(1) }

handler Late: Db {
  fn query(q) {
    with db = PrimaryDb {
      set_timeout(1, || {
        db.query(q)
        ()
      })
    }
    1
  }
  fn conn() { panic(\"no\") }
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// Handler members (`[effect.decl]`, `[effect.handler]`, `[effect.with]`): an item without
/// `x =`, a member in a layer and a bound layer (E4105), a member opened outside `with` and an
/// init (E4106), an inner handler that does not fit its member (E4011), a missing member
/// (E4001), and an init of a member without `suspend` that may suspend (E4303).
#[test]
fn handler_members() {
    let main = "\
effect ReadFile {
  fn read(max: Int) -> Int
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

effect Files {
  handler Open(path: String): ReadFile
}

handler Ticks: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
}

layer App {
  Ticks
}

layer Bad {
  Files.Open
}

handler NoOpen: Files {
}

handler WrongParams: Files {
  handler Open(path: Int): ReadFile {
    init(path: Int) { Self(path:) }
    fn read(max) { max }
  }
}

handler Sleepy: Files {
  handler Open(path: String): ReadFile {
    init(path: String) {
      (use Clock).sleep(1)
      Self(path:)
    }
    fn read(max) { max }
  }
}

fn f() -> () uses { Files } {
  x = Files.Open(\"a\")
  with Files.Open(\"b\") { () }
  with y = App { () }
  ()
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// A handler member whose init takes parameters, written as an item of `with` without them, is
/// E4104, as a handler is (`[effect.with]`, issue #700). A member without parameters is not.
#[test]
fn member_items_need_init_arguments() {
    let main = "effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
  handler Temporary(): ReadFile
}

handler MemoryFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }

  handler Temporary: ReadFile {
    fn read(max) { max }
  }
}

fn main() {
  with MemoryFiles, f = Files.Open, t = Files.Temporary {
    _ = f.read(1) + t.read(1)
  }
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    let output = render(&analysis.diagnostics, &analysis.sources, false);
    assert!(
        output.contains("error[E4104]: handler member `Open` needs arguments"),
        "{output}"
    );
    assert!(
        output.contains("help: write `Files.Open(path: ...)`"),
        "{output}"
    );
    assert_eq!(output.matches("error[").count(), 1, "{output}");
}

/// A handler member named through a label without its arguments is E4104, and the help names
/// the label as written (`[effect.label]`, issue #700).
#[test]
fn member_items_through_a_label_need_init_arguments() {
    let main = "effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
}

effect Audit of Files

handler MemoryFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }
}

fn main() {
  with MemoryFiles, Audit by Files, a = Audit.Open {
    _ = a.read(1)
  }
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    let output = render(&analysis.diagnostics, &analysis.sources, false);
    assert!(
        output.contains("error[E4104]: handler member `Open` needs arguments"),
        "{output}"
    );
    assert!(
        output.contains("help: write `Audit.Open(path: ...)`"),
        "{output}"
    );
    assert_eq!(output.matches("error[").count(), 1, "{output}");
}

/// The messages of host effects (`[effect.host]`): E4003 and E4402 for `ProcessHost`, E5002 at an
/// effect in the uses of an external function that is not a host effect, E4010 at an effect
/// without operations, and E4301 for a host effect with a note that names the entry and what it
/// provides, without the help to provide it with `with`. A process handler reached through
/// `with Platform` gets the help to use `Process` (`[lib.layers]`).
#[test]
fn host_effect_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"import Std.FileSystem.{FileSystem}
import Std.Platform.{Platform}

effect Marker {}

effect Files {
  fn title() -> String
}

@external(js, "./x.mjs", "write")
fn write_host(text: String) -> () uses { Files }

@external(js, "./dom.mjs", "title")
fn title_host() -> String uses { PageHost }

handler PageFiles: Files {
  fn title() { title_host() }
}

handler Host: ProcessHost {
}

fn grab() { use ProcessHost }

@export(js, "f")
pub fn f() -> String { with PageFiles { (use Files).title() } }

fn main() {
  with PageFiles { (use Files).title() }
  with Platform { (use FileSystem).exists("x") }
  ()
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// E1113 (`[grammar.lex]`): `<-` is one token. In an expression it is an error that shows the
/// comparison; at the head of a statement, a right side that is not an effect gets the comparison
/// built from what was written as a help.
#[test]
fn left_arrow_diagnostics() {
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"const LIMIT: Int = 3

fn compare(a: Int, b: Int) -> Bool {
  x = a<-1
  a < -b
}

fn head(x: Int) {
  x <- 1
  x <- LIMIT
  x <- Int.abs(3)
}
"#,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// E4404 (`[effect.use]`): an effect alias is not a value, and a handler member cannot be opened
/// through one. `x <- E` where E is not an effect is the usual error of a name of the wrong kind.
#[test]
fn effect_alias_diagnostics() {
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
}

effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
}

handler Plain: Logger {
  fn info(msg) { () }
}

fn keep(log: use Logger) -> () { log.info("kept") }

fn values() {
  log <- Logger
  keep(log)
  log
}

fn members() -> Int {
  fs <- Files
  with t = fs.Open("a") { t.read(1) }
}

fn not_an_effect() {
  h <- Plain
  ()
}
"#,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// E5003 (`[ext.boundary]`): the R of a callback cannot contain an effect, since the host calls it
/// without handlers; a callback that captures the capability of a named item can be passed.
#[test]
fn callback_effect_diagnostics() {
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Greeter {
  fn greet(s: String) -> String
}

handler Plain: Greeter {
  fn greet(s) { "plain #{s}" }
}

@external(js, "./host.mjs", "later")
fn later(f: fn() -> ()) -> () uses {}

@external(js, "./host.mjs", "declared")
fn declared(f: fn() -> () uses { Greeter }) -> () uses {}

fn main() {
  with Plain {
    later(|| {
      g <- Greeter
      _ = g.greet("alias")
    })
  }
  with g = Plain {
    later(|| {
      _ = g.greet("named")
    })
  }
}
"#,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// W4401 (`[effect.use]`): `use E` written as an expression is deprecated, in either form. The
/// help names the effect alias that means the same where there is one.
#[test]
fn deprecated_use_diagnostics() {
    let analysis = check_files_with_deprecated(
        &[(
            "home/main.emel",
            r#"effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

fn keep(log: use Logger) -> () { log.info("kept") }

fn forms() {
  (use Logger).info("in place")
  log = use Logger
  log.info("receiver")
  outer = use Logger
  with Quiet { outer.info("inner with") }
  keep(use Logger)
}
"#,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The diagnostics of handlers with type parameters (`[effect.handler]`, `[effect.with]`,
/// `[effect.layer]`): E4009, E4107, E1213, E4013, E3101 on an item, and the help of E4008 in the
/// head of a handler.
#[test]
fn generic_handler_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"effect Cell[S] {
  fn get() -> S
}

effect Logger {
  fn info(msg: String) -> ()
}

effect ReadFile {
  fn read() -> String
}

effect Files {
  fn exists(path: String) -> Bool
  handler Open(path: String): ReadFile
}

handler Generic: Cell {
  fn get() { 0 }
}

handler AsSet[E]: Cell[fn() -> () fails E] {
  fn get() { || () }
}

handler Nowhere[A]: Logger {
  fn info(msg) { () }
}

handler Outer: Files {
  fn exists(path) { True }
  handler Open[T](path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read() { self.path }
  }
}

handler Fixed: Cell[Int] {
  fn get() { 0 }
}

handler Pass[S]: Cell[S] {
  fn get() { (use Cell[S]).get() }
}

handler Empty[A]: Cell[List[A]] {
  fn get() { [] }
}

handler Blank[A](xs: List[A]): Logger {
  init() { Blank(xs: []) }
  fn info(msg) { () }
}

fn ints() -> List[Int] uses { Cell[List[Int]] } { (use Cell[List[Int]]).get() }

fn strings() -> List[String] uses { Cell[List[String]] } { (use Cell[List[String]]).get() }

fn unknown() { with Blank { () } }

fn two_fit() { with Empty { (ints(), strings()) } }

fn may_override() { with Fixed, Pass { (use Cell[Int]).get() } }

fn not_generic() { with Fixed[Int] { () } }

layer Missing {
  Blank
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// A call that may suspend after resume in a suspend fn implementation (E4305,
/// `[effect.handler]`): at the call, with a label at the resume it follows, which is a direct call
/// or another use of resume (`[effect.async]`).
#[test]
fn suspend_after_resume() {
    let main = "\
effect Logger {
  suspend fn info(msg: String) -> ()
}

effect Net {
  suspend fn send(msg: String) -> ()
}

@external(js, \"setTimeout\")
fn set_timeout(f: fn() -> () uses {}, ms: Int) -> () uses {}

handler RemoteLog: Logger {
  suspend fn info(msg, resume) {
    net <- Net
    resume(())
    net.send(msg)
  }
}

handler LaterLog: Logger {
  suspend fn info(msg, resume) {
    net <- Net
    set_timeout(|| resume(()), 0)
    net.send(msg)
  }
}
";
    let analysis = check_files(
        &[("app/Pome.toml", ""), ("app/src/main.emel", main)],
        "app",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// The diagnostics of tasks (`[task]`, `[diag.list]`): importing `Std.Fiber` is E2004 with a help
/// that names `Std.Task`; an effect that a task uses and that a with inside the body of its group
/// provides is E4301 at the call of start, with the note and the help to provide it outside the
/// group; a handler for TaskGroup or Spawn is E4003 naming the built-in handler that provides it.
#[test]
fn task_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"import Std.Fiber.{Fiber}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    let fiber = render(&analysis.diagnostics, &analysis.sources, false);
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[(
            "home/main.emel",
            r#"import Std.Task.{Task, TaskGroup, Spawn}

type User(name: String)

effect Users {
  fn find(id: Int) -> User
}

handler PostgresUsers: Users {
  fn find(id) { User(name: "alice") }
}

handler MyGroup: TaskGroup[{}, {}] {
  fn start(f) { todo() }
}

handler MySpawn: Spawn[{}, {}] {
  fn start(f) { () }
}

fn main() {
  with tasks = Task.Group {
    with PostgresUsers {
      tasks.start(|| (use Users).find(1))
    }
    ()
  }
}

fn declared() -> () uses { Async } {
  with tasks = Task.Group {
    with PostgresUsers {
      tasks.start(|| (use Users).find(1))
    }
    ()
  }
}
"#,
        )],
        "home/main.emel",
        &mut frontend,
    );
    let tasks = render(&analysis.diagnostics, &analysis.sources, false);
    insta::assert_snapshot!(format!("{fiber}\n{tasks}"));
}

/// `[effect.with]`: a bound item provides nothing by name. E4301 names the bound item on the way
/// to the use by name; a label item that makes no handler cannot be bound (E4105). The helps
/// that suggest a bound item (W4401, E4404, E5003) are left out when the body of the `with` that
/// provides the effect also uses it by name, since a bound item would send those uses elsewhere.
#[test]
fn bound_item_diagnostics() {
    let analysis = check_files_with_deprecated(
        &[(
            "home/main.emel",
            r##"effect Db {
  fn name() -> String
}

effect Side of Db

handler PrimaryDb: Db {
  fn name() { "primary" }
}

handler ReplicaDb: Db {
  fn name() { "replica" }
}

@external(js, "./host.mjs", "later")
fn later(f: fn() -> ()) -> () uses {}

fn write_side() -> String uses { Db } {
  db <- Db
  db.name()
}

fn show(d: use Db) -> String { d.name() }

fn main() {
  with replica = ReplicaDb {
    _ = "#{replica.name()} #{write_side()}"
  }
}

fn bound_labels() {
  with PrimaryDb {
    with side = Side by Db, db = Side { "#{side.name()} #{db.name()}" }
  }
}

fn use_with_uses_by_name() -> String {
  with PrimaryDb {
    x = use Db
    "#{show(x)} #{write_side()}"
  }
}

fn use_alone() -> String {
  with PrimaryDb {
    x = use Db
    show(x)
  }
}

fn alias_with_uses_by_name() -> String {
  with PrimaryDb {
    d <- Db
    "#{show(d)} #{write_side()}"
  }
}

fn alias_alone() -> String {
  with PrimaryDb {
    d <- Db
    "#{show(d)} #{d.name()}"
  }
}

fn callback_with_uses_by_name() {
  with PrimaryDb {
    _ = write_side()
    later(|| {
      _ = write_side()
    })
  }
}

fn callback_alone() {
  with PrimaryDb {
    later(|| {
      _ = write_side()
    })
  }
}
"##,
        )],
        "home/main.emel",
        &mut Typecheck::default(),
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `[effect.external]`: the ways out of a handler that a value calling an external function had
/// before External, and the diagnostics about External. Each value that calls an external
/// function has the External of its module in its type: put where a pure function type is
/// expected (a State, a parameter, the field of an error, the result of an operation) it is
/// E4301; called outside the handlers of its module it is E4301. A handler for External is
/// E4003, `use External` is E4402, External in the uses of an external function is E5002, and
/// the External of another module in a callback is E5003.
#[test]
fn external_effect_diagnostics() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/log.emel",
                r#"@external(js, "./h.mjs", "hostLog")
fn host_log(s: String) -> ()

## A factory of callbacks: its R is empty, and its result has the External of Log.
pub fn ping() -> fn() -> () uses { External } { || host_log("ping") }
"#,
            ),
            (
                "app/src/main.emel",
                r#"import Log
import Std.State.{State}
import Std.Task.{Task, TaskGroup}

@external(js, "./h.mjs", "hostLog")
fn host_log(s: String) -> ()

@external(js, "./h.mjs", "takeBack")
fn take_back() -> fn(String) -> ()

@external(js, "./h.mjs", "register")
fn register(f: fn() -> ()) -> ()

@external(js, "./h.mjs", "marked")
fn marked() -> () uses { External }

error Carry(f: fn(String) -> ())

effect Leak {
  fn stash() -> ()
  fn kept() -> ()
  fn carry() -> () fails Carry
  fn hand() -> fn(String) -> () fails {} uses {}
  fn give() -> ()
}

fn keep(f: fn(String) -> () fails {} uses {}) {
  st <- State[fn(String) -> ()]
  st.set(f)
}

handler Leaker: Leak {
  fn stash() {
    st <- State[fn(String) -> ()]
    st.set(|s| host_log(s))
  }
  fn kept() { keep(|s| host_log(s)) }
  fn carry() { fail Carry(f: |s| host_log(s)) }
  fn hand() { take_back() }
  fn give() { register(Log.ping()) }
}

effect Spawn {
  fn spawn() -> ()
}

## The task runs in the group of `main`, outside the handlers of Main.
handler Spawner: Spawn {
  fn spawn() {
    tasks <- TaskGroup
    _ = tasks.start(|| host_log("task"))
  }
}

handler Host: External {
}

fn uses_external() {
  _ = use External
  ()
}

fn returned() -> () uses {} {
  f = take_back()
  f("outside")
}

fn from_factory() -> () uses {} {
  g = Log.ping()
  g()
}

fn main() -> () uses { Async } {
  with Task.Group {
    with Spawner {
      s <- Spawn
      s.spawn()
    }
  }
}
"#,
            ),
        ],
        "app",
        &mut frontend,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `[effect.external]`, `[ext.call]`, `[ext.decl]`: the forms that stay silent. A private fn
/// calls an external function and a handler of its module calls it; a pub
/// `@external` fn declared with `uses {}` is called from another module; a pub `@external` fn
/// that adds External is referred to as a value by another module and passed to a handler of its
/// module, as is the result of a factory, to an operation and to an init; an effect and a handler
/// of a module that call external functions are used by the handlers of another module; inside a
/// handler, a closure passed to `List.map`, kept in a field, passed to another handler of the
/// same module through a private effect, started as a task in a group inside the body, and
/// passed to an external function of the module as a callback; a function returned by an
/// external function, called in a handler; External written in the uses clause of a private fn.
#[test]
fn external_effect_allowed() {
    let mut frontend = Typecheck::default();
    let analysis = check_files(
        &[
            ("app/Pome.toml", ""),
            (
                "app/src/log.emel",
                r##"import Std.Task.{Task}

@external(js, "./h.mjs", "hostLog")
pub fn host_log(s: String) -> ()

@external(js, "./h.mjs", "fold")
pub fn fold_case(s: String) -> String uses {}

@external(js, "./h.mjs", "later")
fn later(f: fn() -> ()) -> ()

@external(js, "./h.mjs", "takeBack")
fn take_back() -> fn(String) -> ()

fn tagged(tag: String, msg: String) { host_log("[#{tag}] #{msg}") }

fn written(msg: String) -> () uses { External } { host_log(msg) }

pub fn printer() -> fn(String) -> () uses { External } { |s| host_log(s) }

pub effect Log {
  fn info(msg: String) -> ()
}

pub handler FileLog: Log {
  init {
    tagged("init", "open")
    FileLog
  }
  fn info(msg) {
    tagged("info", msg)
    written(msg)
    _ = List.map(["a", "b"], |s| host_log(s))
    later(|| host_log("later"))
    f = take_back()
    f(msg)
  }
  release { tagged("release", "close") }
}

pub effect Run {
  fn run(f: fn(String) -> () uses { External }) -> ()
}

pub handler Runner: Run {
  fn run(f) { f("ran") }
}

pub handler Kept(f: fn(String) -> () uses { External }): Log {
  init(f: fn(String) -> () uses { External }) { Self(f:) }
  fn info(msg) { self.f(msg) }
}

effect Pass {
  fn pass(f: fn(String) -> () uses { External }) -> ()
}

handler Passer: Pass {
  fn pass(f) { f("passed") }
}

pub effect Relay {
  fn relay(msg: String) -> ()
}

pub handler Relayer: Relay {
  fn relay(msg) {
    with Passer {
      p <- Pass
      p.pass(|s| host_log("#{msg} #{s}"))
    }
  }
}

pub effect Spawn {
  suspend fn spawn() -> ()
}

pub handler Spawner: Spawn {
  suspend fn spawn(resume) {
    with tasks = Task.Group {
      _ = tasks.start(|| host_log("task"))
    }
    resume(())
  }
}
"##,
            ),
            (
                "app/src/main.emel",
                r#"import Log
import Log.{FileLog, Kept, Log, Relay, Relayer, Run, Runner, Spawn, Spawner}

effect Greet {
  fn greet(name: String) -> ()
}

handler Greeter: Greet {
  fn greet(name) {
    log <- Log
    log.info(Log.fold_case(name))
  }
}

fn main() -> () uses { Async } {
  _ = Log.fold_case("pure")
  with FileLog, Greeter, Runner, Relayer, Spawner {
    g <- Greet
    g.greet("you")
    run <- Run
    run.run(Log.host_log)
    run.run(Log.printer())
    r <- Relay
    r.relay("msg")
    s <- Spawn
    s.spawn()
  }
  with Kept(Log.printer()) {
    log <- Log
    log.info("kept")
  }
}
"#,
            ),
        ],
        "app",
        &mut frontend,
    );
    assert_eq!(
        render(&analysis.diagnostics, &analysis.sources, false),
        "",
        "{:#?}",
        analysis.diagnostics
    );
}
