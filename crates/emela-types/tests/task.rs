//! Tasks (`[task]`): the built-in effects `TaskGroup[E, R]` and `Spawn[E, R]`, the built-in
//! handlers `Task.Group` and `Task.Background`, the combinators of `Std.Task`, and `Std.Exit`,
//! `Std.Cause`, `Std.Deferred`, `Std.Semaphore` and `Std.Queue`.
//!
//! Checks that the bundled sources type-check with the types of `[task]`, that the examples of
//! chapter 11 pass, and that the programs the chapter rejects stop: E0402 with the note for
//! effects of tasks, E0413, E0423, E0424, E0426 and E0455.
//!
//! The output shows declared types, local variable types, and diagnostics of `main.emel`.

use crate::common;

use std::fmt::Write;

use emela_resolve::hir::{DefKind, DefModule};
use emela_types::{CheckOptions, check_program_with};

use common::{describe, resolve_main};

fn render(src: &str) -> String {
    let (program, module, resolve_diagnostics) = resolve_main(src);
    let in_main =
        |def: emela_resolve::hir::DefId| program.defs[def].module == DefModule::Source(module);
    let main = program
        .defs
        .iter()
        .find(|(def, data)| in_main(*def) && data.kind == DefKind::Fn && data.name == "main")
        .map(|(def, _)| def);
    let tests = program
        .defs
        .iter()
        .filter(|(def, data)| in_main(*def) && data.name.starts_with("test_"))
        .map(|(def, _)| def)
        .collect();
    let types = check_program_with(
        &program,
        &CheckOptions {
            entry: None,
            main,
            tests,
        },
    );
    let cons = &types.ty_table.cons;

    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        if !in_main(def) || data.kind != DefKind::Fn {
            continue;
        }
        match types.sigs.get(def) {
            Some(sig) => writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap(),
            None => writeln!(out, "{}: <no type>", data.name).unwrap(),
        }
        for (local, ty) in types.locals.iter() {
            if types.local_owner.get(local) == Some(&def) {
                let name = &program.locals[local].name;
                writeln!(out, "  {name}: {}", ty.display(cons)).unwrap();
            }
        }
    }
    for d in &resolve_diagnostics {
        writeln!(out, "resolve: {d}").unwrap();
    }
    let mut diags: Vec<_> = types
        .diags
        .iter()
        .filter(|d| d.span.is_some_and(|s| s.module == module))
        .collect();
    diags.sort_by_key(|d| d.span.map(|s| s.range.start()));
    for d in diags {
        writeln!(out, "{}", describe(src, cons, d)).unwrap();
    }
    out
}

/// The declared types of `Std.Task`, `Std.Exit`, `Std.Cause` and `Std.Semaphore`, and the
/// operations of the built-in effects `TaskGroup`, `Spawn`, `Deferred`, `Queue`, `Semaphore` and
/// `Permit`, as `[task]` writes them. The bundled sources have no diagnostics (the combinators
/// `all`, `race`, `both`, `map` and `timeout` are written in Emela and type-check).
#[test]
fn task_sources_typecheck() {
    let (program, _, resolve_diagnostics) = resolve_main("fn unused() { () }\n");
    assert_eq!(resolve_diagnostics, []);
    let types = check_program_with(&program, &CheckOptions::default());
    assert_eq!(types.diags, []);
    let cons = &types.ty_table.cons;
    // The module that defines `name`, among the modules named `modules` (`Std.Dynamic` has an
    // `attempt` too).
    let module_of = |name: &str, module: &str| {
        program
            .defs
            .iter()
            .find(|(_, d)| {
                d.kind == DefKind::Fn
                    && d.name == name
                    && match d.module {
                        emela_resolve::hir::DefModule::Source(m) => {
                            program.modules[m].name.to_string() == module
                        }
                        _ => false,
                    }
            })
            .map(|(_, d)| d.module.clone())
            .unwrap()
    };
    let modules = [
        module_of("join", "Std.Task"),
        module_of("attempt", "Std.Exit"),
        module_of("raise", "Std.Cause"),
        module_of("with_permits", "Std.Semaphore"),
    ];
    let effects: Vec<_> = program
        .defs
        .iter()
        .filter(|(_, d)| {
            d.kind == DefKind::Effect
                && [
                    "TaskGroup",
                    "Spawn",
                    "Deferred",
                    "Queue",
                    "Semaphore",
                    "Permit",
                ]
                .contains(&d.name.as_str())
        })
        .map(|(def, _)| def)
        .collect();
    assert_eq!(effects.len(), 6, "the built-in effects exist");
    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        let listed = (modules.contains(&data.module) && data.kind == DefKind::Fn)
            || data.parent.is_some_and(|p| effects.contains(&p));
        if !listed {
            continue;
        }
        if let Some(sig) = types.sigs.get(def) {
            writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap();
        }
    }
    insta::assert_snapshot!(out);
}

/// The examples of `[task.group]`, `[task.background]`, `[task.combinators]` and
/// `[task.deferred]`: starting through the capability, joining, the combinators, and Deferred.
/// Errors of the tasks of a group reach the with's E; the effects they use reach its R, with
/// Async. The errors of the tasks of a background go to `on_failure`, not to the with.
#[test]
fn spec_examples() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task, TaskGroup}
import Std.Deferred.{Deferred}
import Std.Cause.{Cause}

error NotFound(id: Int)

type User(name: String)
type Order(id: Int)
type Profile(user: User, orders: List[Order])
type Request(path: String)

effect Users {
  suspend fn find(id: Int) -> User fails NotFound
}

effect Log {
  fn warn(msg: String) -> ()
}

fn fetch_user(id: Int) -> User fails NotFound uses { Users } {
  (use Users).find(id)
}

fn fetch_orders(id: Int) -> List[Order] {
  [Order(id:)]
}

fn audit(request: Request) -> () fails NotFound uses { Users } {
  fetch_user(1)
  ()
}

fn respond(request: Request) -> String {
  request.path
}

fn profile() {
  with tasks = Task.Group {
    user = tasks.start(|| fetch_user(1))
    orders = tasks.start(|| fetch_orders(1))
    Profile(user: user.join(), orders: orders.join())
  }
}

fn served(request: Request) {
  log <- Log
  with bg = Task.Background(on_failure: |cause| log.warn(Cause.to_string(cause))) {
    bg.start(|| audit(request))
    respond(request)
  }
}

fn pairs() {
  (user, orders) = Task.both(|| fetch_user(1), || fetch_orders(1))
  users = Task.map([1, 2], |id| fetch_user(id))
  firsts = Task.all([|| fetch_user(1), || fetch_user(2)])
  fastest = Task.race([|| fetch_user(1), || fetch_user(2)])
  (user, users)
}

fn signal() -> Int uses { Async } {
  with ready = Deferred, tasks = Task.Group {
    tasks.start(|| ready.complete(1))
    n = ready.await()
    peek = ready.poll()
    n
  }
}
"#
    ));
}

/// The examples of `[task.semaphore]` and `[task.queue]`, completed into functions. A Semaphore
/// and a Queue are items of a with before the group whose tasks use them. `with_permits` has the E
/// and R of `f` plus Async; `try_with_permits` has those of `f` only. `offer` and `take` fail with
/// `QueueClosed`, which an escape arm removes from E.
#[test]
fn semaphore_and_queue() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task}
import Std.Semaphore.{Semaphore}
import Std.Queue.{Queue, QueueClosed}

error NotFound(id: Int)

type User(id: Int)
type Job(id: Int)

effect Users {
  fn find(id: Int) -> User fails NotFound
}

fn load(id: Int) -> User fails NotFound uses { Users } {
  (use Users).find(id)
}

fn load_all(ids: List[Int]) {
  with db = Semaphore(permits: 10) {
    Task.map(ids, |id| Semaphore.with_permit(db, || load(id)))
  }
}

fn weighted(s: use Semaphore, n: Int) {
  Semaphore.with_permits(s, n, || load(n))
}

fn tried(s: use Semaphore) {
  Semaphore.try_with_permit(s, || load(1))
}

fn tried_n(s: use Semaphore) {
  Semaphore.try_with_permits(s, 2, || 1)
}

fn counted(s: use Semaphore) {
  s.available()
}

fn run(jobs: List[Job]) {
  with q = Queue(capacity: 100), tasks = Task.Group {
    tasks.start(|| {
      jobs.each(|job| q.offer(job))
      q.close()
    })
    work(q)
  }
}

fn work(q: use Queue[Job]) {
  next = Some(q.take()) escape {
    QueueClosed -> None
  }
  next match {
    Some(job) -> work(q)
    None -> ()
  }
}

fn taken(q: use Queue[Job]) {
  q.take()
}

fn others(q: use Queue[Job]) {
  (q.try_offer(Job(id: 1)), q.poll(), q.size())
}

fn query(sql: String) -> List[String] {
  [sql]
}

fn spec_semaphore(sql: String) {
  with db = Semaphore(permits: 10) {
    rows = Semaphore.with_permit(db, || query(sql))
    rows
  }
}

fn spec_queue(job: Job) {
  with jobs = Queue(capacity: 100), tasks = Task.Group {
    tasks.start(|| jobs.offer(job))
    next = jobs.take()
    next
  }
}
"#
    ));
}

/// Functions that start tasks in a group they do not make receive it, with the type arguments
/// written with type parameters (`[task.group]`). Their tasks' E and R only have to be contained
/// in those of the group, so two helpers with different tasks share one group. A `use` expression
/// and an effect alias may omit the type arguments; a uses clause and a parameter type may not
/// (E0423).
#[test]
fn helpers_receive_the_group() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task, TaskGroup}

error Boom
error Bust

effect Log {
  fn info(msg: String) -> ()
}

fn boom() -> Int fails Boom { fail Boom }

fn bust() -> Int fails Bust { fail Bust }

fn crawl[E, R](tasks: use TaskGroup[E, { Log, R }], url: String) -> () {
  tasks.start(|| (use Log).info(url))
  ()
}

fn spawn_boom[E, R](tasks: use TaskGroup[E | Boom, R]) -> Task[Int] {
  tasks.start(|| boom())
}

fn declared() -> Task[Int] uses { TaskGroup[Bust, {}] } {
  tasks <- TaskGroup
  tasks.start(|| bust())
}

fn both_helpers() -> Int uses { Log, Async } {
  with tasks = Task.Group {
    a = spawn_boom(tasks)
    crawl(tasks, "a")
    a.join()
  } escape {
    Boom -> 0
  }
}

fn omitted(tasks: use TaskGroup) -> () {
  ()
}

fn omitted_uses() -> () uses { TaskGroup } {
  ()
}
"#
    ));
}

/// `attempt` turns errors and defects into values, and `Cause.raise` raises a cause again so
/// `escape` can catch the error (`[task.cause]`). `CauseKind` has Fail and Die only: importing
/// `Interrupt` is an error.
#[test]
fn exits_and_causes() {
    insta::assert_snapshot!(render(
        r#"
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die, Interrupt}

error Boom

effect Log {
  fn info(msg: String) -> ()
}

fn boom() -> Int fails Boom { fail Boom }

fn guarded() -> () uses { Log } {
  Exit.attempt(|| boom()) match {
    Success(_) -> ()
    Failure(cause) -> (use Log).info(Cause.to_string(cause))
  }
}

fn reraise() {
  Exit.attempt(|| boom()) match {
    Success(n) -> n
    Failure(cause) -> Cause.raise(cause)
  }
}

fn recovered() {
  reraise() escape {
    Boom -> 0
  }
}

fn kinds(c: Cause[Boom]) -> Bool {
  Cause.kind(c) match {
    Fail -> True
    Die -> False
  }
}

"#
    ));
}

/// The example of `[task.group]`: an effect a task uses is provided outside the with of its group.
/// A `with` inside the body does not reach the tasks, so E0402 points at the start, with the
/// note. Providing it outside the group, or by an item before it in the same with, passes, and
/// an effect the body itself uses gets no note.
#[test]
fn task_effects_are_provided_outside() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task}

type User(name: String)

effect Users {
  fn find(id: Int) -> User
}

handler PostgresUsers: Users {
  fn find(id) { User(name: "alice") }
}

fn main() {
  with tasks = Task.Group {
    with PostgresUsers {
      tasks.start(|| (use Users).find(1))
    }
    ()
  }
}

fn fixed() -> () uses { Async } {
  with PostgresUsers {
    with tasks = Task.Group {
      tasks.start(|| (use Users).find(1))
      ()
    }
  }
}

fn same_with() -> () uses { Async } {
  with PostgresUsers, tasks = Task.Group {
    tasks.start(|| (use Users).find(1))
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

fn body_itself() -> () uses { Async } {
  with tasks = Task.Group {
    (use Users).find(1)
    ()
  }
}

fn test_started() {
  with tasks = Task.Group {
    tasks.start(|| (use Users).find(1))
    ()
  }
}
"#
    ));
}

/// The R of a task is provided outside the with of its group: a capability made by an item before
/// the group in the same with can be used by its tasks; one made by an item after it, or by a
/// with inside its body, cannot (E0455, `[task.group]`). A task, being a capability-bound type of
/// the group, cannot leave the with (E0455). Neither can a Deferred, a Semaphore or a Queue.
#[test]
fn capabilities_and_tasks_stay_inside() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task}
import Std.Deferred.{Deferred}
import Std.Queue.{Queue}
import Std.Semaphore.{Semaphore}

fn before() -> () uses { Async } {
  with d = Deferred[Int], tasks = Task.Group {
    tasks.start(|| d.complete(1))
    ()
  }
}

fn after() -> () uses { Async } {
  with tasks = Task.Group, d = Deferred[Int] {
    tasks.start(|| d.complete(1))
    ()
  }
}

fn inside() -> () uses { Async } {
  with tasks = Task.Group {
    with d = Deferred[Int] {
      tasks.start(|| d.complete(1))
      ()
    }
  }
}

fn handle() -> () uses { Async } {
  t = with tasks = Task.Group {
    tasks.start(|| 1)
  }
  ()
}

fn optional() -> () uses { Async } {
  t = with tasks = Task.Group {
    Some(tasks.start(|| 1))
  }
  ()
}

fn joined() -> Int uses { Async } {
  with tasks = Task.Group {
    tasks.start(|| 1).join()
  }
}

fn deferred() {
  with d = Deferred[Int] { (d, 1) }
}

fn semaphore() {
  with s = Semaphore(permits: 2) { s }
}

fn queue() {
  with q = Queue[Int](capacity: 1) { [q] }
}
"#
    ));
}

/// Only `Task.Group` provides TaskGroup and only `Task.Background` provides Spawn: a handler for
/// either is E0413 (`[task.group]`, `[task.background]`).
#[test]
fn task_handlers() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task, TaskGroup, Spawn}

handler MyGroup: TaskGroup[{}, {}] {
  fn start(f) { todo() }
}

handler MySpawn: Spawn[{}, {}] {
  fn start(f) { () }
}
"#
    ));
}

/// User effects take neither operation type parameters (E0426) nor set type parameters (E0424,
/// spec 8.1). `@intrinsic` on an effect is only for the bundled sources (E0224).
#[test]
fn user_effects_stay_plain() {
    insta::assert_snapshot!(render(
        r#"
effect Wrap {
  fn wrap[A](x: A) -> List[A]
}

effect Raise[E] {
  fn run(f: fn() -> () fails E) -> ()
}

@intrinsic
effect Mine[E] {
  fn go(f: fn() -> () fails E) -> ()
}

fn wrapped() -> List[Int] uses { Wrap } {
  (use Wrap).wrap(1)
}
"#
    ));
}

/// A task may start its siblings (`[task.group]`): the R of a task may have the capability of its
/// group, or TaskGroup by name, provided by the group it was started in. The errors and effects of
/// the siblings it starts still reach the group. A function that uses TaskGroup by name starts
/// tasks recursively.
#[test]
fn tasks_start_siblings() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task, TaskGroup}
import Std.Console.{Console, SystemConsole}

error Boom

fn boom() -> Int fails Boom { fail Boom }

fn main() {
  with SystemConsole, tasks = Task.Group {
    tasks.start(|| {
      tasks.start(|| (use Console).print_line("x"))
      ()
    })
    ()
  }
}

fn through_capability() -> () fails Boom uses { Async } {
  with tasks = Task.Group {
    tasks.start(|| {
      tasks.start(|| boom())
      ()
    })
    ()
  }
}

fn spawn_tree(depth: Int) -> () uses { TaskGroup[Boom, {}] } {
  if depth > 0 {
    tasks <- TaskGroup
    tasks.start(|| {
      spawn_tree(depth - 1)
      boom()
    })
    ()
  } else {
    ()
  }
}

fn recursive() {
  with tasks = Task.Group { spawn_tree(3) }
}
"#
    ));
}

/// In nested groups, each task starts into the group it was given (`[task.group]`): the errors of
/// the inner group's tasks reach the inner with, and through it the outer.
#[test]
fn nested_groups() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task}

error Boom
error Bust

fn boom() -> Int fails Boom { fail Boom }

fn bust() -> Int fails Bust { fail Bust }

fn nested() {
  with outer = Task.Group {
    outer.start(|| boom())
    inner = with tasks = Task.Group {
      tasks.start(|| {
        tasks.start(|| bust())
        ()
      })
      outer.start(|| boom())
      1
    }
    inner
  }
}
"#
    ));
}

/// A background (`[task.background]`): start takes a function that returns `()` and returns `()`;
/// a task whose value is wanted does not fit (E0301). The errors of its tasks go to `on_failure`,
/// which receives a `Cause` of them and has no errors of its own; they do not reach the with. Its
/// R and that of the tasks reach the with's R, with Async.
#[test]
fn background() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{Task, Spawn}
import Std.Cause.{Cause}

error Boom

effect Log {
  fn warn(msg: String) -> ()
}

fn boom() -> () fails Boom { fail Boom }

fn logged() {
  with bg = Task.Background(on_failure: |cause| (use Log).warn(Cause.to_string(cause))) {
    bg.start(|| boom())
    1
  }
}

fn value_wanted() -> () uses { Async } {
  with bg = Task.Background(on_failure: |_| ()) {
    bg.start(|| 1)
  }
}

fn failing_handler() -> () uses { Async } {
  with bg = Task.Background(on_failure: |_| boom()) {
    ()
  }
}

fn helper[E, R](spawn: use Spawn[E | Boom, R]) -> () {
  spawn.start(|| boom())
}

fn through_helper() {
  with bg = Task.Background(on_failure: |_| ()) {
    helper(bg)
  }
}
"#
    ));
}

/// The sets of TaskGroup and Spawn in the R of a function value are compared too: a function that
/// starts tasks using more effects, or failing with more errors, does not fit where its caller
/// provides a group whose tasks have fewer (`[task.group]`, `[type.rules]`). Fewer fit, and a set
/// variable takes what is started.
#[test]
fn task_sets_of_function_values() {
    insta::assert_snapshot!(render(
        r#"
import Std.Task.{TaskGroup, Spawn}

effect Log {
  fn info(msg: String) -> ()
}

error Boom

fn takes_group(f: fn() -> () uses { TaskGroup[{}, { Async }] }) { () }

fn takes_spawn[F](f: fn() -> () uses { Spawn[F, { Async }] }) { () }

fn more_effects() uses { TaskGroup[{}, { Async, Log }] } { () }

fn more_errors() uses { TaskGroup[Boom, { Async }] } { () }

fn fewer() uses { TaskGroup[{}, {}] } { () }

fn spawns_boom() uses { Spawn[Boom, { Async }] } { () }

fn spawns_log() uses { Spawn[{}, { Log }] } { () }

fn main() {
  takes_group(more_effects)
  takes_group(more_errors)
  takes_group(fewer)
  takes_spawn(spawns_boom)
  takes_spawn(spawns_log)
}
"#
    ));
}
