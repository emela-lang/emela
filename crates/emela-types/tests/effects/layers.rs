//! Layers, labels and bound items.

use super::render_handlers;

/// Layers (spec 8.5): contents are handlers or other layers. Initialization order follows
/// dependencies, and unmet dependencies remain in the R of `with`. Two handlers providing the same
/// effect (E4102), mutual inclusion (E4103) and dependency cycles (E4101) are reported at the
/// layer declaration. In `with AppLive, FixedClock` the later one wins.
#[test]
fn layers() {
    insta::assert_snapshot!(render_handlers(
        r#"
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
  fn now() -> Int
}

effect Ping {
  fn ping() -> ()
}

effect Pong {
  fn pong() -> ()
}

handler EnvConfig: Config {
  fn db_url() { "x" }
}

handler ConsoleLogger: Logger {
  fn info(msg) { () }
}

handler SilentLogger: Logger {
  fn info(msg) { () }
}

handler FixedClock: Clock {
  fn now() { 0 }
}

handler RealClock: Clock {
  fn now() {
    (use Logger).info("now")
    1
  }
}

handler DbUsers(url: String): Users {
  init { DbUsers(url: (use Config).db_url()) }
  fn find(id) {
    (use Logger).info(self.url)
    self.url
  }
}

handler HPing: Ping {
  fn ping() { (use Pong).pong() }
}

handler HPong: Pong {
  fn pong() { (use Ping).ping() }
}

layer Base {
  ConsoleLogger
  RealClock
}

layer AppLive {
  DbUsers
  Base
  EnvConfig
}

layer Partial {
  DbUsers
  ConsoleLogger
}

layer Dup {
  ConsoleLogger
  SilentLogger
}

layer DupNested {
  Dup
  Base
}

layer Twice { ConsoleLogger, ConsoleLogger }

layer LoopA { LoopB }

layer LoopB { LoopA }

layer PingPong {
  HPing
  HPong
}

fn main() {
  with AppLive { (use Users).find(1) }
}

fn partial() {
  with Partial { (use Users).find(1) }
}

fn test_override() {
  with AppLive, FixedClock { (use Clock).now() }
}

fn later_wins() {
  with ConsoleLogger, SilentLogger { (use Logger).info("x") }
}

fn ping_pong() {
  with PingPong { (use Ping).ping() }
}

fn loops() {
  with LoopA { () }
}
"#
    ));
}

/// Labels (issue #255). `with L` provides the base from `L`; `L by P` provides `L`; a provider
/// of the base also provides the labels declared `default` that the body uses. A label's R holds
/// the label until something provides it.
#[test]
fn labels() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Log {
  fn log(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

effect Audit of Log
effect Debug of Log default
effect Other of Log
effect Ticks of Clock

handler PrintLog: Log {
  fn log(msg) { () }
}

handler Tagged: Log {
  fn log(msg) { (use Other).log(msg) }
}

fn a() { (use Log).log("a") }

fn c() {
  with Audit { a() }
  with Debug { a() }
}

fn direct() { (use Audit).log("x") }

fn run_hidden(f: fn() -> ()) { with PrintLog { f() } }

fn main() {
  with PrintLog, Audit by Log { c() }
  with PrintLog, Audit by PrintLog, Debug by Tagged, Other by Log { c() }
  with Audit by PrintLog { direct() }
  with PrintLog, Audit by Log { with Other by Audit { run_hidden(|| (use Other).log("y")) } }
}

fn ticks[R: Immediate](f: fn() -> () uses { R }) { f() }

fn sleeps() { ticks(|| (use Ticks).sleep(1)) }
"#
    ));
}

/// Misused labels (issue #255): bases that cannot be used (E4201), effects that are not labels
/// (E4202), providers of another effect (E4203), two providers for a label (E4204), and handlers
/// of labels (E4003).
#[test]
fn label_errors() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Log {
  fn log(msg: String) -> ()
}

effect Other {
  fn other() -> ()
}

effect Audit of Log
effect Nested of Audit
effect Bad of Async

handler PrintLog: Log {
  fn log(msg) { () }
}

handler OtherH: Other {
  fn other() { () }
}

handler AuditH: Audit {
  fn log(msg) { () }
}

fn a() { (use Log).log("a") }

fn uses_audit() { (use Audit).log("x") }

fn main() {
  with Log { a() }
  with Log by PrintLog { a() }
  with PrintLog, Audit by OtherH { uses_audit() }
  with PrintLog, Audit by PrintLog, Audit by Log { uses_audit() }
}

fn undefault() {
  with PrintLog { uses_audit() }
}

fn unused(f: fn() -> ()) {
  with PrintLog, Audit by PrintLog { a() }
  with PrintLog, Audit by PrintLog { f() }
}
"#
    ));
}

/// Labels of an effect with type arguments (issue #255): the label's operations are typed with
/// the base's arguments, and the right of `by` may carry arguments too.
#[test]
fn labels_of_generic_effects() {
    insta::assert_snapshot!(render_handlers(
        r#"
effect Box[T] {
  fn get() -> T
}

effect Hits of Box[Int]
effect Names of Box[String] default
effect Bare of Box

handler IntBox: Box[Int] {
  fn get() { 1 }
}

handler StrBox: Box[String] {
  fn get() { "s" }
}

fn hits() -> Int { (use Hits).get() + 1 }

fn name() -> String { (use Names).get() }

fn main() {
  with Hits by IntBox { hits() }
  with IntBox, Hits by Box[Int] { hits() }
  with StrBox { name() }
  with IntBox { name() }
  with Hits by StrBox { hits() }
  with IntBox, Hits by Box[String] { hits() }
  with IntBox, Hits by Box { hits() }
}
"#
    ));
}

/// `[effect.with]`: a bound item makes its capability and provides nothing by name, whether it
/// is a handler, a handler of several effects or a label plugged into a handler. The body's uses
/// by name are left to the withs around it, and a requirement of another item of the same `with`
/// is not met by it. It provides no label declared `default` (`[effect.label]`). It is never
/// overridden and does not override an unbound item, so both are initialized, and the E
/// of its init and of its arguments are in the E of the `with` (`[type.rules]`).
#[test]
fn bound_items_provide_nothing_by_name() {
    insta::assert_snapshot!(render_handlers(
        r##"
effect Db {
  fn name() -> String
}

effect Clock {
  fn now() -> Int
}

effect Side of Db

effect Names of Db default

error Down

handler PrimaryDb: Db {
  fn name() { "primary" }
}

fn probe(n: Int) -> () fails Down { if n > 0 { fail Down } else { () } }

handler ReplicaDb: Db {
  init {
    probe(0)
    ReplicaDb
  }

  fn name() { "replica" }
}

handler Both: Db + Clock {
  fn name() { "both" }
  fn now() { 0 }
}

handler NeedsDb: Clock {
  fn now() {
    _ = (use Db).name()
    0
  }
}

fn write_side() -> String uses { Db } { (use Db).name() }

fn names() -> String uses { Names } { (use Names).name() }

fn show(side: use Side) -> String { side.name() }

fn now() -> Int uses { Clock } { (use Clock).now() }

fn left_to_caller() {
  with replica = ReplicaDb { "#{replica.name()} #{write_side()}" }
}

fn outer() {
  with PrimaryDb {
    with replica = ReplicaDb { "#{replica.name()} #{write_side()}" }
  }
}

fn several() {
  with both = Both { "#{both.name()} #{now()}" }
}

fn label() {
  with side = Side by ReplicaDb { "#{show(side)} #{write_side()}" }
}

fn no_default() {
  with replica = ReplicaDb { names() }
}

fn requirement() {
  with replica = ReplicaDb, NeedsDb { now() }
}

fn never_overridden() {
  with PrimaryDb, a = ReplicaDb, b = PrimaryDb {
    "#{a.name()} #{b.name()} #{write_side()}"
  }
}
"##
    ));
}

/// `[effect.with]`, `[effect.label]`: a label item that makes no handler (`L`, `L by P` where P
/// is an effect or a label) cannot be bound to a name (E4105); an unbound one is
/// unchanged. Two bound items of one label are not E4204, and a bound label item is never
/// W4201: it provides nothing by name, so the label is used through its capability.
#[test]
fn bound_label_items() {
    insta::assert_snapshot!(render_handlers(
        r##"
effect Db {
  fn name() -> String
}

effect Side of Db

effect Other of Db

handler PrimaryDb: Db {
  fn name() { "primary" }
}

handler ReplicaDb: Db {
  fn name() { "replica" }
}

fn bound_to_effect() -> String uses { Db } {
  with side = Side by Db { side.name() }
}

fn bound_to_label() -> String uses { Other } {
  with side = Side by Other { side.name() }
}

fn bound_alone() -> String uses { Side } {
  with db = Side { db.name() }
}

fn unbound() -> String uses { Db } {
  with Side by Db { (use Side).name() }
}

fn two_bound() -> String {
  with a = Side by PrimaryDb, b = Side by ReplicaDb { "#{a.name()} #{b.name()}" }
}

fn two_unbound() {
  with Side by PrimaryDb, Side by ReplicaDb { (use Side).name() }
}

fn bound_unused() {
  with side = Side by PrimaryDb { () }
}
"##
    ));
}
