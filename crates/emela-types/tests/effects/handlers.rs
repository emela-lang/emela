//! Handler checks and stateful handlers.

use super::{render_effects, render_handlers};

/// Handler checks (spec 8.4): every operation implemented exactly once, parameter counts from the
/// effect, operation body E within the declared fails. A handler with fields needs init.
#[test]
fn handler_checks() {
    insta::assert_snapshot!(render_effects(
        r#"
error Boom

effect Logger {
  fn info(msg: String) -> ()
  fn warn(msg: String) -> () fails Boom
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

handler Half: Logger {
  fn info(msg) { () }
}

handler Twice: Logger {
  fn info(msg) { () }
  fn info(msg) { () }
  fn warn(a, b) { () }
}

handler Fails: Logger {
  fn info(msg) { fail Boom }
  fn warn(msg) { fail Boom }
}

handler Typed: Logger {
  fn info(msg) { msg + 1 }
  fn warn(msg) { self }
}

handler Stateful(n: Int): Logger {
  fn info(msg) { () }
  fn warn(msg) { () }
}

handler Sleepy: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
}
"#
    ));
}

/// Stateful handlers (spec 8.4): field types and `self`; init's value is the handler value; `use`
/// inside init and release are dependencies; init's E goes into the E of `with` (spec 9.4).
/// release may call suspend operations. The initialization order of `with` follows dependencies
/// (spec 14.2).
#[test]
fn stateful_handlers() {
    insta::assert_snapshot!(render_handlers(
        r#"
error DbDown

type Connection(url: String)

effect Config {
  fn db_url() -> String
}

effect Users {
  fn find(id: Int) -> String
}

effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

fn connect(url: String) -> Connection fails DbDown {
  if url == "" { fail DbDown } else { Connection(url:) }
}

handler PostgresUsers(conn: Connection): Users {
  init {
    config = use Config
    PostgresUsers(conn: connect(config.db_url()))
  }

  release {
    (use Logger).info(self.conn.url)
    (use Clock).sleep(10)
  }

  fn find(id) { self.conn.url }
}

handler EnvConfig: Config {
  fn db_url() { "postgres://" }
}

handler Quiet: Logger {
  init { Quiet }
  release { () }
  fn info(msg) { () }
}

handler Unit(): Logger {
  init { Unit() }
  fn info(msg) { () }
}

fn lookup() {
  with EnvConfig, PostgresUsers { (use Users).find(1) }
}

fn reversed() {
  with PostgresUsers, EnvConfig { (use Users).find(1) }
}

fn logged() {
  with Quiet, EnvConfig, PostgresUsers { (use Users).find(1) }
}
"#
    ));
}

/// Stateful handler errors: building the handler value outside init (E4005), fields without init
/// (E4006), a release that can fail (E4007), the type of init's value, and unknown fields.
#[test]
fn stateful_handler_errors() {
    insta::assert_snapshot!(render_handlers(
        r#"
error Boom

effect Logger {
  fn info(msg: String) -> ()
}

fn risky() -> () fails Boom { fail Boom }

handler Counter(n: Int): Logger {
  init { Counter(n: 0) }
  release { risky() }
  fn info(msg) {
    next = Counter(n: self.n + 1)
    ()
  }
}

handler Careful(n: Int): Logger {
  init { Careful(n: 1) }
  release { risky() escape { Boom -> () } }
  fn info(msg) { () }
}

handler NoInit(n: Int): Logger {
  fn info(msg) { () }
}

handler WrongInit(n: Int): Logger {
  init { 1 }
  fn info(msg) { self.m }
}

handler Plain: Logger {
  fn info(msg) {
    me = self
    ()
  }
}

fn make() { Counter(n: 1) }

fn lambda() { || Counter(n: 1) }
"#
    ));
}

/// `[effect.handler]`, `[grammar.expr]`: inside the init of a handler, `Self(...)` constructs its
/// value as the handler's name does: with fields, alone for a handler without parentheses, `Self()`
/// for `handler H()`, and inside an anonymous function inside init. Its type is the handler's, and
/// its fields are checked as for `H(...)`. Inside the handler but outside init it is E4005; outside
/// a handler (in a function, a trait or an impl) it is E2108. It is not a record update target.
#[test]
fn self_constructs_in_init() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Logger {
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
  init(start: Int) {
    value: Int = Self(n: start)
    Self(n: start)
  }
  release { Self(n: 0) }
  fn info(msg) {
    next = Self(n: self.n + 1)
    ()
  }
}

handler Plain: Logger {
  init { Self }
  fn info(msg) { () }
}

handler Empty(): Logger {
  init { Self() }
  fn info(msg) { () }
}

handler Lazy(n: Int): Logger {
  init {
    make = || Self(n: 1)
    make()
  }
  fn info(msg) { () }
}

handler Wrong(n: Int): Logger {
  init { Self(m: 1) }
  fn info(msg) { () }
}

handler Typed(n: Int): Logger {
  init { Self(n: "a") }
  fn info(msg) { () }
}

handler Updated(n: Int): Logger {
  init { Self(n: 1, ..Self(n: 2)) }
  fn info(msg) { () }
}

fn outside() { Self(n: 1) }

fn main() {
  with Counter(start: 1) { (use Logger).info("a") }
}
"#
    ));
}

/// An item that opens a handler member requires the effect that declares it, so it is
/// initialized after a provider of that effect in the same `with`; otherwise the items keep the
/// order they are written in (`[dyn.with]`). A bound item is never overridden, so it is
/// initialized even when a later item provides the same effect (`[effect.with]`).
#[test]
fn handler_member_items_in_initialization_order() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect ReadFile {
  fn read(max: Int) -> Int
}

effect Files {
  handler Open(path: String): ReadFile
}

effect Logger {
  fn info(msg: String) -> ()
}

handler MemFiles: Files {
  handler Open(path: String): ReadFile {
    init(path: String) { Self(path:) }
    fn read(max) { max }
  }
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Loud: Logger {
  fn info(msg) { () }
}

fn order() {
  with a = Files.Open("a"), MemFiles, quiet = Quiet, b = Files.Open("b"), Loud {
    quiet.info("x")
    (use Logger).info("y")
    a.read(1) + b.read(2)
  }
}

fn outside() {
  with a = Files.Open("a"), Quiet {
    a.read(1)
  }
}
"#
    ));
}
