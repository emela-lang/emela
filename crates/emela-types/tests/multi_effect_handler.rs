//! Handlers that implement several effects (`[effect.handler]`, `[effect.with]`,
//! `[effect.layer]`, `[effect.label]`, `[effect.use]`, `[type.rules]`).
//!
//! The output shows each fn's type and its local variables' types, then the diagnostics.

mod common;

use common::render_bodies;

const TX: &str = r#"
effect Query {
  fn query(sql: String) -> Int
}

effect Execute {
  fn execute(sql: String) -> ()
}

effect Audit {
  fn note(msg: String) -> ()
}

effect Store[S] {
  fn load() -> S
}

handler SystemTx(conn: Int): Query + Execute {
  init(url: String) { SystemTx(conn: 1) }
  release { () }
  fn query(sql) { self.conn }
  fn execute(sql) { () }
}
"#;

fn run(src: &str) -> String {
    render_bodies(&format!("{TX}{src}"))
}

/// A handler implements each operation of every effect it lists, with the type arguments
/// written for each; all of them share `self` (`[effect.handler]`). Written in a with, it
/// provides all of them (`[effect.with]`, provides(H) in `[type.rules]`).
#[test]
fn providing_several_effects() {
    insta::assert_snapshot!(run(r#"
handler Counter(n: Int): Store[Int] + Audit {
  init { Counter(n: 0) }
  fn load() { self.n }
  fn note(msg) { () }
}

fn both() -> Int uses { Query, Execute } {
  (use Execute).execute("x")
  (use Query).query("y")
}

fn run_both() -> Int {
  with SystemTx(url: "db") { both() }
}

fn counted() -> Int {
  with Counter {
    (use Audit).note("a")
    (use Store[Int]).load()
  }
}
"#));
}

/// A name bound to a handler of several effects is the capability of all of them; the order the
/// effects are written in does not matter (`[effect.with]`, `[effect.use]`). Calling an operation
/// on it adds that effect, through the capability (`[type.rules]`).
#[test]
fn binding_a_handler_of_several_effects() {
    insta::assert_snapshot!(run(r#"
fn use_both(tx: use Execute + Query) -> Int {
  tx.execute("a")
  tx.query("b")
}

fn bound() -> Int {
  with tx = SystemTx(url: "db") {
    use_both(tx)
  }
}

fn typed(tx: use Query + Execute) -> use Execute + Query { tx }
"#));
}

/// A capability of a set of effects is passed as the type of fewer of them; only the operations
/// of those can be called through the parameter (`[effect.use]`).
#[test]
fn narrowing_a_capability() {
    insta::assert_snapshot!(run(r#"
handler Everything: Query + Execute + Audit {
  fn query(sql) { 0 }
  fn execute(sql) { () }
  fn note(msg) { () }
}

fn read_only(q: use Query) -> Int { q.query("a") }

fn read_write(rw: use Query + Execute) -> Int {
  rw.execute("b")
  read_only(rw)
}

fn audited(c: use Audit + Query + Execute) -> Int {
  c.note("c")
  read_write(c) + read_only(c)
}

fn all() -> Int {
  with e = Everything { audited(e) }
}
"#));
}

/// A capability cannot be passed as the type of an effect it does not have, and the operations of
/// the original effects cannot be called on a narrowed one (`[effect.use]`).
#[test]
fn widening_is_an_error() {
    insta::assert_snapshot!(run(r#"
fn read_write(rw: use Query + Execute) -> () { rw.execute("a") }

fn widen(q: use Query) -> () { read_write(q) }

fn other(q: use Query + Audit) -> () { read_write(q) }

fn narrowed(q: use Query) -> () { q.execute("a") }

fn back(rw: use Query + Execute) -> use Query + Execute + Audit { rw }
"#));
}

/// A missing operation of any effect is E0404, listing all of them; the same operation twice is
/// E0405. Two effects with an operation of the same name, and the same effect listed twice, are
/// compile errors (`[effect.handler]`). A label among them is E0433, and a host effect such as Async is a compile error too.
#[test]
fn handler_errors() {
    insta::assert_snapshot!(run(r#"
effect Reader {
  fn query(sql: String) -> Int
}

handler Missing: Query + Execute + Audit {
  fn execute(sql) { () }
}

handler Twice: Query + Execute {
  fn query(sql) { 0 }
  fn execute(sql) { () }
  fn execute(sql) { () }
}

handler Clash: Query + Reader {
  fn query(sql) { 0 }
}

handler Repeated: Audit + Query + Audit {
  fn note(msg) { () }
  fn query(sql) { 0 }
}

handler Arity: Query + Execute {
  fn query(sql) { 0 }
  fn execute() { () }
}

effect Replica of Query

handler OfLabel: Replica + Audit {
  fn note(msg) { () }
}

handler WithAsync: Audit + Async {
  fn note(msg) { () }
}
"#));
}

/// The operations of one effect can use another effect the handler implements only through a
/// with: the requirement is provided outside, as for a requirement on its own effect, even when
/// an earlier item of the same with provides it, since the handler replaces that item for the
/// effect (`[effect.handler]`, `[effect.with]`).
#[test]
fn requirements_on_its_own_effects() {
    insta::assert_snapshot!(run(r#"
handler Logged: Query + Audit {
  fn query(sql) {
    (use Audit).note(sql)
    0
  }
  fn note(msg) { () }
}

handler Notes: Audit {
  fn note(msg) { () }
}

fn outer() -> Int uses { Audit } {
  with Logged { (use Query).query("a") }
}

fn wired() -> Int {
  with Notes, Logged { (use Query).query("a") }
}
"#));
}

/// A handler of several effects counts as providing each of them for E0421 (`[effect.layer]`).
/// In a with, a later item replaces it only for the effects it provides (`[effect.with]`).
#[test]
fn layers_and_overriding() {
    insta::assert_snapshot!(run(r#"
handler FixedQuery: Query {
  fn query(sql) { 7 }
}

layer Both {
  SystemTx(url: "db")
  FixedQuery
}

handler Plain: Query + Execute {
  fn query(sql) { 0 }
  fn execute(sql) { () }
}

layer Fine {
  Plain
}

fn overridden() -> Int {
  with Plain, FixedQuery {
    (use Execute).execute("a")
    (use Query).query("b")
  }
}

fn from_layer() -> Int {
  with Fine { (use Query).query("c") }
}
"#));
}

/// In `L by H`, H serves L through the effect it implements that is the underlying effect of L;
/// if none is, E0431 (`[effect.label]`).
#[test]
fn labels_by_a_handler_of_several_effects() {
    insta::assert_snapshot!(run(r#"
effect Replica of Query

handler Plain: Query + Execute {
  fn query(sql) { 0 }
  fn execute(sql) { () }
}

handler Notes: Audit + Execute {
  fn note(msg) { () }
  fn execute(sql) { () }
}

fn replica() -> Int {
  with Replica by Plain { (use Replica).query("a") }
}

fn wrong() -> Int {
  with Replica by Notes { (use Replica).query("a") }
}
"#));
}

/// A handler member can implement several effects; the capability it opens has all of them, and
/// the inner handler implements the same effects (`[effect.decl]`, `[effect.handler]`,
/// `[grammar.decl]`).
#[test]
fn handler_members_of_several_effects() {
    insta::assert_snapshot!(run(r#"
effect Files {
  handler Lock(path: String): Query + Execute
}

handler MemFiles: Files {
  handler Lock(path: String): Execute + Query {
    init(path: String) { Self(path:) }
    fn query(sql) { 0 }
    fn execute(sql) { () }
  }
}

handler BadFiles: Files {
  handler Lock(path: String): Query {
    init(path: String) { Self(path:) }
    fn query(sql) { 0 }
  }
}

fn read_only(q: use Query) -> Int { q.query("a") }

fn locked() -> Int {
  with MemFiles, l = Files.Lock("a") {
    l.execute("b")
    read_only(l)
  }
}
"#));
}
