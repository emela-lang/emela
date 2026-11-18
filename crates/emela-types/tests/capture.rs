//! Capture sets (`[effect.escape]`, `[fiber.escape]`) and the capabilities in R (`[effect.use]`,
//! `[type.fn]`, `[type.rules]`).
//!
//! The output shows, for each fn of `main.emel`, its type and what its return value holds, the
//! types of its local variables and what they hold, and then the diagnostics: a value that leaves
//! the `with` or `Fiber.scope` providing a root it holds (W0402) as
//! `` `value` leaves: `root` (provider) ``, and the others as in the other tests.

mod common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, DiagnosticKind, Held, check_program_with};

use common::{describe, resolve_main};

fn held(list: &[Held]) -> String {
    list.iter()
        .map(|h| match &h.provider {
            Some(p) => format!("{} ({})", h.name, p.what),
            None => h.name.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let types = check_program_with(&program, &CheckOptions::default());
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
        if let Some(list) = types.captures.defs.get(def) {
            writeln!(out, "  holds: {}", held(list)).unwrap();
        }
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) != Some(&def) {
                continue;
            }
            let name = &program.locals[local].name;
            write!(out, "  {name}: {}", ty.display(cons)).unwrap();
            if let Some(list) = types.captures.locals.get(local) {
                write!(out, "  holds: {}", held(list)).unwrap();
            }
            writeln!(out).unwrap();
        }
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        match &d.kind {
            DiagnosticKind::CaptureEscape { root, scope, .. } => {
                let range = d.span.unwrap().range;
                let text = &src[usize::from(range.start())..usize::from(range.end())];
                let provider = scope.as_deref().unwrap_or("with");
                writeln!(out, "`{text}` leaves: `{root}` ({provider})").unwrap();
            }
            _ => writeln!(out, "{}", describe(src, cons, d)).unwrap(),
        }
    }
    out
}

const PRELUDE: &str = r#"
effect Logger {
  fn info(msg: String) -> ()
}

handler ConsoleLogger: Logger {
  fn info(msg) { () }
}

handler FileLogger: Logger {
  fn info(msg) { () }
}

effect Db {
  fn query(q: String) -> Int
}

handler PrimaryDb: Db {
  fn query(q) { 1 }
}

handler ReplicaDb: Db {
  fn query(q) { 2 }
}
"#;

fn with_prelude(src: &str) -> String {
    format!("{PRELUDE}\n{src}")
}

/// `[effect.escape]`: a closure that captures a capability and is the value of the `with` that
/// provides it is W0402. The examples of the proposal: a closure, and a list holding one. A
/// closure used up inside the `with` is fine.
#[test]
fn leaving_a_with() {
    insta::assert_snapshot!(render(&with_prelude(
        r#"
fn make_greeter() -> fn() -> () {
  with FileLogger {
    log = use Logger
    || log.info("hi")
  }
}

fn nested() {
  with PrimaryDb {
    p = use Db
    with ReplicaDb {
      r = use Db
      f = || r.query("...")
      [f]
    }
  }
}

fn used_up(names: List[String]) {
  with ConsoleLogger {
    log = use Logger
    names.map(|n| log.info(n))
  }
}

fn outer_is_fine() {
  with PrimaryDb {
    p = use Db
    with ReplicaDb { || p.query("...") }
  }
}

fn unbound() -> fn() -> () {
  with ConsoleLogger { || (use Logger).info("hi") }
}
"#
    )));
}

/// `[effect.escape]`: a data structure holds what its elements hold, a record what its fields
/// hold, and a local fn what it captures. A value whose type cannot contain a capability holds
/// nothing.
#[test]
fn what_values_hold() {
    insta::assert_snapshot!(render(&with_prelude(
        r#"
type Box(f: fn() -> Int uses { Db })

fn data() {
  with PrimaryDb {
    p = use Db
    t = (1, || p.query("t"))
    o = Some(|| p.query("o"))
    fn local() -> Int { p.query("l") }
    n = p.query("n")
    (t, o, local, n)
  }
}

fn plain() -> Int {
  with PrimaryDb {
    p = use Db
    f = || p.query("x")
    f()
  }
}
"#
    )));
}

/// `[effect.escape]`: the return value of a function is inferred from its body, in terms of its
/// parameters and of the effects its caller provides; a call replaces them with what the
/// arguments hold and with the root of the `with` around the call.
#[test]
fn through_calls() {
    insta::assert_snapshot!(render(&with_prelude(
        r#"
fn pass[A](x: A) -> A { x }

fn logger() -> fn() -> () {
  log = use Logger
  || log.info("hi")
}

fn keep(f: fn() -> Int) -> fn() -> Int { f }

fn by_param() -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    pass(|| p.query("x"))
  }
}

fn by_caller() -> fn() -> () {
  with ConsoleLogger { logger() }
}

fn by_pipe() -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    (|| p.query("x")) |> keep()
  }
}

fn countdown(n: Int, f: fn() -> Int) -> fn() -> Int {
  if n == 0 { f } else { countdown(n - 1, f) }
}

fn recursive() -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    countdown(3, || p.query("x"))
  }
}
"#
    )));
}

/// `[effect.escape]`: a declaration without a body returns what its arguments hold, and an
/// operation also its handler. A function type of a parameter returns what its arguments hold.
#[test]
fn declarations_without_a_body() {
    insta::assert_snapshot!(render(&with_prelude(
        r#"
effect Factory {
  fn make(n: Int) -> fn() -> Int
  fn wrap(f: fn() -> Int) -> fn() -> Int
}

handler Plain: Factory {
  fn make(n) { || n }
  fn wrap(f) { f }
}

fn op_result() -> fn() -> Int {
  with Plain {
    m = use Factory
    m.make(1)
  }
}

fn op_argument() -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    with Plain {
      m = use Factory
      m.wrap(|| {
        q = p
        1
      })
    }
  }
}

fn call_param(g: fn(fn() -> Int) -> fn() -> Int) -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    g(|| {
      q = p
      1
    })
  }
}

fn call_param_without_arguments(g: fn() -> fn() -> Int) -> fn() -> Int {
  with PrimaryDb {
    p = use Db
    g()
  }
}
"#
    )));
}

/// `[effect.use]`, `[type.fn]`, `[type.rules]`: calling an operation of a captured capability puts
/// the capability in R (`uses { p }`). It widens to its effect, so the closure can be passed where
/// the effect is expected; the call reaches the captured handler. A capability that a `uses`
/// clause lacks is reported as its effect, which can be written there.
#[test]
fn capabilities_in_r() {
    insta::assert_snapshot!(render(&with_prelude(
        r#"
fn run(f: fn() -> Int uses { Db }) -> Int { f() }

fn widened() -> Int {
  with PrimaryDb {
    p = use Db
    with ReplicaDb {
      f = || p.query("x")
      g = || (use Db).query("y")
      run(f) + run(g)
    }
  }
}

fn pure_rejects() -> Int {
  with PrimaryDb {
    p = use Db
    h: fn() -> Int uses {} = || p.query("x")
    h()
  }
}

fn declared_local() -> Int {
  with PrimaryDb {
    p = use Db
    fn g() -> Int uses {} { p.query("x") }
    g()
  }
}
"#
    )));
}

/// `[effect.use]`: whether a call may suspend (E0417) is decided by the effect of a capability in
/// R.
#[test]
fn suspension_by_the_effect() {
    insta::assert_snapshot!(render(
        r#"
import Std.Clock.{Clock}

effect Tick {
  fn tick() -> ()
}

handler Ticking: Tick {
  fn tick() {
    clock = use Clock
    f = || clock.sleep(1)
    f()
  }
}
"#
    ));
}

/// `[fiber.escape]`: the Scope of a `Fiber.scope`, and the handles it makes, are its roots. The
/// value of `Fiber.scope` holding one is W0402.
#[test]
fn leaving_a_scope() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

fn scope_closure() -> fn() -> Fiber[Int] {
  Fiber.scope(|| {
    s = use Scope
    || s.fork(|| 1)
  })
}

fn handle_in_a_closure() -> fn() -> Int {
  Fiber.scope(|| {
    h = Fiber.fork(|| 1)
    || Fiber.join(h)
  })
}

fn deferred_in_a_list() -> List[fn() -> Option[Int]] {
  Fiber.scope(|| {
    d = Fiber.deferred()
    [|| d.poll()]
  })
}

fn piped() -> fn() -> Fiber[Int] {
  (|| {
    s = use Scope
    || s.fork(|| 1)
  }) |> Fiber.scope()
}
"#
    ));
}

/// `[effect.escape]`, `[fiber.escape]` (`scope-handle-values`): a value taken out of a handle does
/// not hold the root of its scope, unless its type contains a handle. Other roots are passed on.
#[test]
fn values_taken_out_of_handles() {
    insta::assert_snapshot!(render(&format!(
        "import Std.Fiber.{{Fiber, Scope}}\n{}",
        with_prelude(
            r#"
fn run[A](f: fn() -> A) -> A {
  Fiber.scope(|| Fiber.join(Fiber.fork(f)))
}

fn awaited[A](f: fn() -> A) -> A {
  Fiber.scope(|| {
    d = Fiber.deferred()
    Fiber.fork(|| d.complete(f()))
    d.await()
  })
}

fn handle_of_a_handle() -> Fiber[Int] {
  Fiber.scope(|| Fiber.join(Fiber.fork(|| Fiber.fork(|| 1))))
}

fn other_roots() -> fn() -> () {
  with ConsoleLogger {
    log = use Logger
    Fiber.scope(|| Fiber.join(Fiber.fork(|| || log.info("x"))))
  }
}
"#
        )
    )));
}

/// `[fiber.escape]`: a Semaphore or Queue that a `Fiber.scope` makes is a root of the scope. The
/// value of `Fiber.scope` holding one, through a closure or a list, is W0402.
#[test]
fn semaphores_and_queues_leaving_a_scope() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber}
import Std.Queue.{Queue}

fn semaphore_in_a_closure() -> fn() -> Int {
  Fiber.scope(|| {
    s = Fiber.semaphore(1)
    || s.available()
  })
}

fn queue_in_a_list() -> List[fn() -> Option[Int]] {
  Fiber.scope(|| {
    q: Queue[Int] = Fiber.queue(1)
    [|| q.poll()]
  })
}

fn piped() -> fn() -> Bool {
  (|| {
    q: Queue[Int] = Fiber.queue(1)
    || q.try_offer(1)
  }) |> Fiber.scope()
}
"#
    ));
}

/// `[effect.escape]`, `[fiber.escape]`: a value taken out of a Semaphore or Queue (`take`, `poll`,
/// `with_permit`, `try_with_permit`, `available`) does not hold the root of its scope, unless its
/// type contains a handle. Other roots are passed on.
#[test]
fn values_taken_out_of_semaphores_and_queues() {
    insta::assert_snapshot!(render(&format!(
        "import Std.Fiber.{{Fiber}}\nimport Std.Queue.{{Queue, QueueClosed}}\n{}",
        with_prelude(
            r#"
fn taken() -> Int fails QueueClosed {
  Fiber.scope(|| {
    q: Queue[Int] = Fiber.queue(1)
    q.offer(1)
    q.take()
  })
}

fn polled() -> Option[Int] {
  Fiber.scope(|| {
    q: Queue[Int] = Fiber.queue(1)
    q.try_offer(1)
    q.poll()
  })
}

fn permitted[A](f: fn() -> A) -> A {
  Fiber.scope(|| Fiber.semaphore(1).with_permit(f))
}

fn tried[A](f: fn() -> A) -> Option[A] {
  Fiber.scope(|| Fiber.semaphore(1).try_with_permit(f))
}

fn counted() -> Int {
  Fiber.scope(|| Fiber.semaphore(2).available())
}

fn queue_of_a_queue() -> Queue[Int] fails QueueClosed {
  Fiber.scope(|| {
    qq: Queue[Queue[Int]] = Fiber.queue(1)
    qq.offer(Fiber.queue(1))
    qq.take()
  })
}

fn other_roots() -> fn() -> () {
  with ConsoleLogger {
    log = use Logger
    Fiber.scope(|| Fiber.semaphore(1).with_permit(|| || log.info("x")))
  }
}
"#
        )
    )));
}

/// `[lsp.hover]`: a root passed to a parameter is shown by the parameter's name, for a local fn
/// and a closure too.
#[test]
fn parameters_are_named() {
    insta::assert_snapshot!(render(
        r#"
fn outer() -> Int {
  fn wrap(f: fn() -> Int) -> fn() -> Int {
    g = f
    g
  }
  h = |k: fn() -> Int| {
    m = k
    m
  }
  wrap(h(|| 1))()
}
"#
    ));
}

/// `[effect.use]`: a capability value is passed as an argument, put into a data structure and
/// returned from a function as any other value. Its type is written `use E` and shown as written.
#[test]
fn capability_values_are_values() {
    insta::assert_snapshot!(render(&with_prelude(
        r##"
type Pair(first: use Db, second: use Db)

fn copy(from: use Db, to: use Db) -> Int {
  to.query("#{from.query("...")}")
}

fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db {
  if write { primary } else { replica }
}

fn first(dbs: List[use Db]) -> Option[use Db] {
  dbs match {
    [db, ..] -> Some(db)
    _ -> None
  }
}

fn pass[A](x: A) -> A { x }

fn caller() -> Int {
  with PrimaryDb {
    primary = use Db
    with ReplicaDb {
      replica = use Db
      pair = Pair(first: primary, second: replica)
      both = [primary, replica]
      chosen = pick(primary, replica, True)
      same = pass(replica)
      copy(replica, primary) + chosen.query("x") + same.query("y") + pair.first.query("z")
    }
  }
}
"##
    )));
}

/// `[effect.use]`: calling an operation of a capability received as a parameter adds that
/// capability to R, and a caller does not provide it by name: the call replaces it with what the
/// argument is. An operation of a capability bound in the body adds its effect, as before. The
/// capability elements of R are not written, so a `uses` clause that lacks them names the effect.
#[test]
fn capability_parameters_in_r() {
    insta::assert_snapshot!(render(&with_prelude(
        r##"
fn sync_users(from: use Db, to: use Db) -> Int {
  from.query("a") + to.query("b")
}

fn through_closure(db: use Db) -> Int {
  f = || db.query("c")
  f()
}

fn declared(db: use Db) -> Int uses { Db } {
  db.query("d")
}

fn pure_rejects(db: use Db) -> Int uses {} {
  db.query("e")
}

fn in_a_list(dbs: List[use Db]) -> Int {
  dbs.fold(from: 0, f: |n, db| n + db.query("f"))
}

fn caller() -> Int {
  with PrimaryDb {
    primary = use Db
    with ReplicaDb {
      replica = use Db
      sync_users(replica, primary) + through_closure(primary)
    }
  }
}

fn forwarded(db: use Db) -> Int {
  sync_users(db, db)
}

fn no_provider_needed() -> Int {
  with PrimaryDb {
    p = use Db
    run_without(p)
  }
}

fn run_without(db: use Db) -> Int {
  forwarded(db)
}
"##
    )));
}

/// `[effect.escape]`: a capability value, and any value that holds one, cannot leave the `with`
/// that provides it: returned as the value of the `with`, inside a data structure, or through a
/// function that returns its argument.
#[test]
fn capability_values_leaving_a_with() {
    insta::assert_snapshot!(render(&with_prelude(
        r##"
fn pass[A](x: A) -> A { x }

fn pick(primary: use Db, replica: use Db) -> use Db { primary }

fn bare() -> use Db {
  with PrimaryDb { use Db }
}

fn bound() -> use Db {
  with PrimaryDb {
    p = use Db
    p
  }
}

fn in_a_list() -> List[use Db] {
  with PrimaryDb {
    p = use Db
    [p]
  }
}

fn through_a_call() -> use Db {
  with PrimaryDb {
    p = use Db
    with ReplicaDb {
      r = use Db
      pick(p, r)
    }
  }
}

fn through_pass() -> use Db {
  with PrimaryDb { pass(use Db) }
}

fn outer_is_fine() -> Int {
  with PrimaryDb {
    p = use Db
    q = with ReplicaDb { p }
    q.query("x")
  }
}
"##
    )));
}

/// `[effect.handler]`: a field of a handler can have the type `use E`. The capability belongs to
/// a `with` that encloses the `with` of the handler, since the arguments of init are evaluated
/// outside it.
#[test]
fn capability_fields_of_a_handler() {
    insta::assert_snapshot!(render(&with_prelude(
        r##"
effect Users {
  fn find(id: Int) -> Int
}

handler PostgresUsers(db: use Db): Users {
  init(db: use Db) { PostgresUsers(db:) }
  fn find(id) { self.db.query("#{id}") }
}

fn run() -> Int {
  with PrimaryDb {
    with PostgresUsers(db: use Db) {
      (use Users).find(1)
    }
  }
}

fn from_param(db: use Db) -> Int {
  with PostgresUsers(db:) {
    (use Users).find(2)
  }
}
"##
    )));
}

/// `[effect.use]`: only the operations of E can be called on a value of type `use E` (E0401).
/// `use E` and `use F` are the same type only for the same effect with the same type arguments;
/// a label is a different effect. No trait is implemented for `use E`, and `==` is not defined.
#[test]
fn capability_type_rules() {
    insta::assert_snapshot!(render(&with_prelude(
        r##"
effect Replica of Db

effect State[S] {
  fn get() -> S
}

fn wrong_op(db: use Db) -> () {
  db.info("x")
}

fn other_effect(log: use Logger) -> use Db { log }

fn label(r: use Replica) -> use Db { r }

fn type_args(s: use State[Int]) -> use State[String] { s }

fn equal(a: use Db, b: use Db) -> Bool { a == b }

fn shown(a: use Db) -> String { "#{a}" }

fn op_value(db: use Db) -> fn(String) -> Int uses { Db } { db.query }
"##
    )));
}
