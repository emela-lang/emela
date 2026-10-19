//! Fibers (spec chapter 11): the built-in effect `Scope[E, R]`, `Fiber.scope`, the combinators of
//! `Std.Fiber`, `Std.Cause` and `Std.Deferred`.
//!
//! Checks that the bundled sources type-check with the types of spec 11.2 to 11.7, that the
//! examples of chapter 11 pass, and that the programs the chapter rejects stop: E0402 with the
//! note for effects of forked fibers, E0408 to E0410 on the Scope capability, E0413, E0424,
//! E0425 and E0426.
//!
//! The output shows declared types, local variable types, and diagnostics of `main.emel`.

mod common;

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
    let types = check_program_with(&program, &CheckOptions { main, tests });
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

/// The declared types of `Std.Fiber`, `Std.Cause` and `Std.Deferred` and the operations of
/// `Scope`, as spec 11.2 to 11.7 write them. The bundled sources have no diagnostics (the
/// combinators `all`, `race`, `both` and `map` are written in Emela and type-check).
#[test]
fn fiber_sources_typecheck() {
    let (program, _, resolve_diagnostics) = resolve_main("fn unused() { () }\n");
    assert_eq!(resolve_diagnostics, []);
    let types = check_program_with(&program, &CheckOptions::default());
    assert_eq!(types.diags, []);
    let cons = &types.ty_table.cons;
    let module_of = |name: &str| {
        program
            .defs
            .iter()
            .find(|(_, d)| d.kind == DefKind::Fn && d.name == name)
            .map(|(_, d)| d.module.clone())
            .unwrap()
    };
    let modules = [module_of("scope"), module_of("raise"), module_of("poll")];
    let scope = program
        .defs
        .iter()
        .find(|(_, d)| d.kind == DefKind::Effect && d.name == "Scope")
        .map(|(def, _)| def)
        .expect("the Scope effect exists");
    let mut out = String::new();
    for (def, data) in program.defs.iter() {
        let listed = (modules.contains(&data.module) && data.kind == DefKind::Fn)
            || data.parent == Some(scope);
        if !listed {
            continue;
        }
        if let Some(sig) = types.sigs.get(def) {
            writeln!(out, "{}: {}", data.name, sig.scheme.display(cons)).unwrap();
        }
    }
    insta::assert_snapshot!(out);
}

/// The examples of spec 11.2, 11.3 and 11.7: forking through the capability and through
/// `Fiber.fork`, joining, the combinators, and Deferred. Errors of the children reach the scope's
/// E; the effects they use reach its R.
#[test]
fn spec_examples() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

error NotFound(id: Int)

type User(name: String)
type Order(id: Int)
type Profile(user: User, orders: List[Order])

effect Users {
  suspend fn find(id: Int) -> User fails NotFound
}

fn fetch_user(id: Int) -> User fails NotFound use Users {
  (use Users).find(id)
}

fn fetch_orders(id: Int) -> List[Order] {
  [Order(id:)]
}

fn profile() {
  Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { fetch_user(1) })
    b = s.fork(fn() { fetch_orders(1) })
    Profile(user: Fiber.join(a), orders: Fiber.join(b))
  })
}

fn wrapped() {
  Fiber.scope(fn() {
    a = Fiber.fork(fn() { fetch_user(1) })
    Fiber.join(a)
  })
}

fn pairs() {
  (user, orders) = Fiber.both(fn() { fetch_user(1) }, fn() { fetch_orders(1) })
  users = Fiber.map([1, 2], fn(id) { fetch_user(id) })
  firsts = Fiber.all([fn() { fetch_user(1) }, fn() { fetch_user(2) }])
  fastest = Fiber.race([fn() { fetch_user(1) }, fn() { fetch_user(2) }])
  (user, users)
}

fn signal() -> Int use { Async } {
  Fiber.scope(fn() {
    ready = Fiber.deferred()
    Fiber.fork(fn() { Deferred.complete(ready, 1) })
    n = Deferred.await(ready)
    peek = Deferred.poll(ready)
    n
  })
}
"#
    ));
}

/// Helpers fork through `use Scope` (spec 11.2). A helper's R names the scope; its children's E
/// and R only have to be contained in those of the scope it runs in, so two helpers with
/// different children share one scope.
#[test]
fn helpers_use_scope() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

error Boom
error Bust

effect Log {
  fn info(msg: String) -> ()
}

fn boom() -> Int fails Boom { fail Boom }

fn bust() -> Int fails Bust { fail Bust }

fn spawn_boom() {
  Fiber.fork(fn() { boom() })
}

fn spawn_log() {
  (use Scope).fork(fn() { (use Log).info("hi") })
}

fn declared() -> Fiber[Int] use Scope[Bust, {}] {
  Fiber.fork(fn() { bust() })
}

fn both_helpers() {
  Fiber.scope(fn() {
    a = spawn_boom()
    b = spawn_log()
    c = declared()
    Fiber.join(a)
  })
}

fn outer_scope() {
  Fiber.scope(fn() {
    outer = use Scope
    Fiber.scope(fn() {
      outer.fork(fn() { boom() })
      Fiber.fork(fn() { (use Log).info("inner") })
      ()
    })
  })
}
"#
    ));
}

/// The results of fibers (spec 11.5): `await` gives an `Exit` whose failures are only
/// interruptions, `attempt` turns errors and defects into values, and `Cause.raise` raises a
/// cause again so `escape` can catch the error.
#[test]
fn exits_and_causes() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Exit, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die, Interrupt}

error Boom

effect Log {
  fn info(msg: String) -> ()
}

fn boom() -> Int fails Boom { fail Boom }

fn guarded() -> () use { Log } {
  Fiber.attempt(fn() { boom() }) match {
    Success(_) -> ()
    Failure(cause) -> (use Log).info(Cause.to_string(cause))
  }
}

fn reraise() {
  Fiber.attempt(fn() { boom() }) match {
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
    Interrupt -> False
  }
}

fn watched() {
  Fiber.scope(fn() {
    child = Fiber.fork(fn() { 1 })
    Fiber.interrupt(child)
    Fiber.await(child) match {
      Success(n) -> n
      Failure(_) -> 0
    }
  })
}
"#
    ));
}

/// The example of spec 11.2: an effect a forked fiber uses must be provided outside
/// `Fiber.scope`. A `with` inside the scope's body does not reach the children, so `main` reports
/// E0402 with the note. Providing it outside the scope passes, and an effect the body itself
/// uses gets no note.
#[test]
fn forked_effects_are_provided_outside() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

type User(name: String)

effect Users {
  fn find(id: Int) -> User
}

handler PostgresUsers: Users {
  fn find(id) { User(name: "alice") }
}

fn main() {
  Fiber.scope(fn() {
    s = use Scope
    with PostgresUsers {
      s.fork(fn() { (use Users).find(1) })
    }
    ()
  })
}

fn fixed() -> () use { Async } {
  with PostgresUsers {
    Fiber.scope(fn() {
      Fiber.fork(fn() { (use Users).find(1) })
      ()
    })
  }
}

fn body_itself() -> () use { Async } {
  Fiber.scope(fn() {
    (use Users).find(1)
    ()
  })
}

fn test_forked() {
  Fiber.scope(fn() {
    Fiber.fork(fn() { (use Users).find(1) })
    ()
  })
}
"#
    ));
}

/// The capability rules of spec 8.2 apply to Scope as they are (spec 11.8): passing it as an
/// argument (E0408), storing it in a data structure (E0409) and returning it (E0410) are errors.
#[test]
fn scope_capability_rules() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

fn take[A](x: A) -> () { () }

fn leaks() {
  Fiber.scope(fn() {
    s = use Scope
    take(s)
    xs = [s]
    ()
  })
}

fn returned() {
  use Scope
}

fn closure_is_fine() {
  Fiber.scope(fn() {
    s = use Scope
    run = fn() { s.fork(fn() { 1 }) }
    Fiber.join(run())
  })
}
"#
    ));
}

/// A fiber handle or a Deferred in the value type of `Fiber.scope` is E0425 (spec 11.8),
/// wherever it is in the type.
#[test]
fn handles_cannot_leave_the_scope() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

fn handle() {
  Fiber.scope(fn() { Fiber.fork(fn() { 1 }) })
}

fn optional() {
  Fiber.scope(fn() { Some(Fiber.fork(fn() { 1 })) })
}

fn deferred() {
  Fiber.scope(fn() { (Fiber.deferred(), 1) })
}

fn joined() {
  Fiber.scope(fn() { Fiber.join(Fiber.fork(fn() { 1 })) })
}

fn piped() {
  fn() { Fiber.fork(fn() { 1 }) } |> Fiber.scope()
}
"#
    ));
}

/// Only `Fiber.scope` provides Scope: a handler for it is E0413 (spec 11.2), and `with` it
/// provides nothing.
#[test]
fn scope_handler() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

handler MyScope: Scope[{}, {}] {
  fn fork(f) { todo() }
  fn deferred() { todo() }
}

fn provided() {
  with MyScope {
    Fiber.fork(fn() { 1 })
  }
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

fn wrapped() -> List[Int] use { Wrap } {
  (use Wrap).wrap(1)
}
"#
    ));
}

/// A child may fork its siblings (spec 11.2): the `Scope` in the child's R is provided by the
/// scope it is forked into, not from outside, through `Fiber.fork` and through the capability.
/// The errors and effects of the grandchildren still reach the scope.
#[test]
fn children_fork_siblings() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}
import Std.Console.{Console, SystemConsole}

error Boom

fn boom() -> Int fails Boom { fail Boom }

fn main() {
  with SystemConsole {
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        Fiber.fork(fn() { (use Console).print_line("x") })
        ()
      })
      ()
    })
  }
}

fn through_capability() {
  Fiber.scope(fn() {
    s = use Scope
    s.fork(fn() {
      s.fork(fn() { boom() })
      ()
    })
    ()
  })
}

fn spawn_tree(depth: Int) -> () use { Scope[Boom, {}] } {
  if depth > 0 {
    Fiber.fork(fn() {
      spawn_tree(depth - 1)
      boom()
    })
    ()
  } else {
    ()
  }
}

fn recursive() {
  Fiber.scope(fn() { spawn_tree(3) })
}
"#
    ));
}

/// In nested scopes, a child's `Scope` is provided by the scope it belongs to (spec 11.2): a
/// child of the inner scope forking a sibling stays in the inner scope, and the outer scope's R
/// does not get `Scope`.
#[test]
fn nested_scopes() {
    insta::assert_snapshot!(render(
        r#"
import Std.Fiber.{Fiber, Scope}

error Boom
error Bust

fn boom() -> Int fails Boom { fail Boom }

fn bust() -> Int fails Bust { fail Bust }

fn nested() {
  Fiber.scope(fn() {
    Fiber.fork(fn() { boom() })
    inner = Fiber.scope(fn() {
      Fiber.fork(fn() {
        Fiber.fork(fn() { bust() })
        ()
      })
      1
    })
    inner
  })
}

fn test_nested() {
  Fiber.scope(fn() {
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        Fiber.fork(fn() { () })
        ()
      })
      ()
    })
  })
}
"#
    ));
}
