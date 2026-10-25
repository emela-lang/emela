//! Fibers, `Scope`, `Deferred`, interruption and release (spec chapter 11, 14.5). The programs are
//! ports of `crates/emela-codegen-c/tests/programs/fiber_*.emel`; `print` is `dbg` here, so the
//! order of the lines is what is compared. Skipped without `wasmtime`.

mod common;

use common::*;

/// Runs `src` and returns the run, failing if wasmtime is not available.
fn run_ok(name: &str, src: &str) -> Option<Run> {
    let run = run(name, src)?;
    assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    Some(run)
}

/// The scope of the language: `Fiber.scope` returns the body's value after the children finish.
#[test]
fn scope_joins_children_and_returns_the_body_value() {
    let Some(run) = run_ok(
        "fiber-scope-basic",
        r##"
import Std.Fiber.{Fiber, Scope}

fn fetch(n: Int) -> Int { n * 10 }

fn main() -> () use { Async } {
  total = Fiber.scope(fn() {
    s = use Scope
    a = s.fork(fn() { fetch(1) })
    b = Fiber.fork(fn() { fetch(2) })
    Fiber.join(a) + Fiber.join(b)
  })
  dbg(total)
  dbg(Fiber.scope(fn() {
    x = Fiber.fork(fn() { 1.5 })
    y = Fiber.fork(fn() { Int64.from_int(7) })
    u = Fiber.fork(fn() { dbg("unit child") })
    Fiber.join(u)
    "#{Fiber.join(x)} #{Fiber.join(y)}"
  }))
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "30\n\"unit child\"\n\"1.5 7\"\n");
}

/// Children that are still running when the body returns are interrupted, and the scope waits for
/// their unwinding (spec 11.2).
#[test]
fn scope_interrupts_children_left_at_the_end() {
    let Some(run) = run_ok(
        "fiber-scope-close",
        r##"
import Std.Fiber.{Fiber, Scope}

fn main() -> () use { Async } {
  v = Fiber.scope(fn() {
    h = Fiber.fork(fn() { dbg("child starts") })
    dbg("body returns")
    2
  })
  dbg(v)
  ()
}
"##,
    ) else {
        return;
    };
    // The child had not started when the body returned, so it ends with Interrupt without being
    // called ([fiber.scope]); only the body's output and value remain.
    assert_eq!(run.stderr, "\"body returns\"\n2\n");
}

/// Nested scopes: a child of an inner scope forks its own children, and the inner scope closes
/// before the outer body goes on (spec 11.2).
#[test]
fn nested_scopes_close_inside_out() {
    let Some(run) = run_ok(
        "fiber-scope-nested",
        r##"
import Std.Fiber.{Fiber, Scope}

fn main() -> () use { Async } {
  r = Fiber.scope(fn() {
    outer = use Scope
    late = outer.fork(fn() {
      dbg("late")
      9
    })
    inner = Fiber.scope(fn() {
      a = Fiber.fork(fn() {
        dbg("inner a")
        1
      })
      b = Fiber.fork(fn() {
        dbg("inner b")
        2
      })
      Fiber.join(a) + Fiber.join(b)
    })
    dbg(inner)
    Fiber.join(late)
  })
  dbg(r)
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "\"late\"\n\"inner a\"\n\"inner b\"\n3\n9\n");
}

/// Many fibers and many joins: the scheduler handles a long runnable queue.
#[test]
fn many_fibers_join_in_order() {
    let Some(run) = run_ok(
        "fiber-many",
        r##"
import Std.Fiber.{Fiber, Scope}

fn main() -> () use { Async } {
  total = Fiber.scope(fn() {
    hs = List.map(List.range(0, 200), fn(i) { Fiber.fork(fn() { i + 1 }) })
    List.fold(hs, 0, fn(acc, h) { acc + Fiber.join(h) })
  })
  dbg(total)
  ()
}
"##,
    ) else {
        return;
    };
    // sum of 1..=200
    assert_eq!(run.stderr, "20100\n");
}

/// A Deferred passes a value from a child to any number of waiting fibers; `complete` stores only
/// the first value and `poll` does not wait (spec 11.7).
#[test]
fn deferred_multiple_waiters_and_complete_once() {
    let Some(run) = run_ok(
        "fiber-deferred",
        r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

fn main() -> () use { Async } {
  Fiber.scope(fn() {
    d = Fiber.deferred()
    a = Fiber.fork(fn() { Deferred.await(d) + 1 })
    b = Fiber.fork(fn() { Deferred.await(d) + 2 })
    dbg(Deferred.poll(d))
    Fiber.fork(fn() { dbg(Deferred.complete(d, 40)) })
    dbg("#{Fiber.join(a)} #{Fiber.join(b)}")
    dbg(Deferred.complete(d, 1))
    dbg(Deferred.poll(d))
    names = Fiber.deferred()
    Fiber.fork(fn() { Deferred.complete(names, ["x", "y"]) })
    dbg(Deferred.await(names))
    ()
  })
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "None\nTrue\n\"41 42\"\nFalse\nSome(40)\n[\"x\", \"y\"]\n"
    );
}

/// `Fiber.interrupt` interrupts and waits for the unwinding; `await` reports `Interrupt`, and
/// `join` of an interrupted child is a defect (spec 11.5).
#[test]
fn interrupt_and_await() {
    let Some(run) = run_ok(
        "fiber-interrupt",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}
import Std.Deferred.{Deferred}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    Fiber.scope(fn() {
      h = Fiber.fork(fn() { spin(100000) })
      spin(1000)
      Fiber.interrupt(h)
      dbg(show(Fiber.await(h)))
      dbg(show(Fiber.attempt(fn() { Fiber.join(h) })))
      ok = Fiber.fork(fn() { 7 })
      dbg(show(Fiber.await(ok)))
      ()
    })
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"interrupted\"\n\"defect: fiber was interrupted\"\n\"Success\"\n"
    );
}

/// Interruption waits for a running release (spec 11.4): the release finishes, then the fiber
/// unwinds.
#[test]
fn interrupt_waits_for_a_release() {
    let Some(run) = run_ok(
        "fiber-release",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

effect Name {
  fn name() -> String
}

handler SlowRes: Name {
  init {
    dbg("init slow")
    SlowRes
  }
  fn name() { "slow" }
  release {
    dbg("release slow starts")
    spin(5000)
    dbg("release slow ends")
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    Fiber.scope(fn() {
      h = Fiber.fork(fn() { with SlowRes { spin(100000) } })
      spin(1500)
      dbg("interrupting")
      Fiber.interrupt(h)
      dbg(show(Fiber.await(h)))
      ()
    })
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"init slow\"\n\"interrupting\"\n\"release slow starts\"\n\"release slow ends\"\n\"interrupted\"\n"
    );
}

/// The first failure of a child becomes the scope's result; later failures are notes (spec 11.2).
#[test]
fn child_failure_notes_keep_the_first() {
    let Some(run) = run_ok(
        "fiber-notes",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

error Boom

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init {
    dbg("init res")
    Res
  }
  fn name() { "res" }
  release { panic("second") }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    caught = Fiber.attempt(fn() {
      Fiber.scope(fn() {
        Fiber.fork(fn() {
          spin(1000)
          fail Boom
        })
        Fiber.fork(fn() { with Res { spin(100000) } })
        spin(100000)
        ()
      })
    })
    dbg(show(caught))
    ()
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"init res\"\n\"error: Boom\\nnote: another fiber also failed: defect: second\"\n"
    );
}

/// A scope capability, a fiber handle or a Deferred used after its scope ended is a defect, not a
/// trap (spec 11.8).
#[test]
fn use_after_scope_close_is_a_defect() {
    let Some(run) = run(
        "fiber-after-close",
        r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

fn main() -> () use { Async } {
  join_later = Fiber.scope(fn() {
    h = Fiber.fork(fn() { 1 })
    fn() { Fiber.join(h) }
  })
  dbg("before join")
  join_later()
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"before join\"\ndefect: fiber was joined after its scope ended\n"
    );
}

#[test]
fn deferred_use_after_scope_close_is_a_defect() {
    let Some(run) = run(
        "fiber-deferred-after-close",
        r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

fn main() -> () use { Async } {
  complete_later = Fiber.scope(fn() {
    d = Fiber.deferred()
    fn() { Deferred.complete(d, 1) }
  })
  dbg("before complete")
  complete_later()
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"before complete\"\ndefect: deferred was completed after its scope ended\n"
    );
}

#[test]
fn poll_after_scope_close_is_a_defect() {
    let Some(run) = run(
        "fiber-poll-after-close",
        r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

fn main() -> () use { Async } {
  poll_later = Fiber.scope(fn() {
    d = Fiber.deferred()
    fn() { Deferred.poll(d) }
  })
  dbg("before poll")
  poll_later()
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"before poll\"\ndefect: deferred was polled after its scope ended\n"
    );
}

/// `Fiber.attempt` does not catch the interruption of its own fiber (spec 11.5).
#[test]
fn attempt_does_not_catch_its_own_interruption() {
    let Some(run) = run_ok(
        "fiber-attempt-interrupt",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    Fiber.scope(fn() {
      h = Fiber.fork(fn() {
        Fiber.attempt(fn() { spin(100000) }) match {
          Success(_) -> dbg("attempt success")
          Failure(c) -> dbg("attempt caught #{Cause.to_string(c)}")
        }
      })
      spin(1000)
      Fiber.interrupt(h)
      dbg(show(Fiber.await(h)))
      ()
    })
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "\"interrupted\"\n");
}

/// A failure of the body is the scope's result; a later failure of a child is a note. The
/// releases of the children run.
#[test]
fn body_failure_and_child_note() {
    let Some(run) = run_ok(
        "fiber-body-failure",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

error Boom

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    r = Fiber.scope(fn() {
      Fiber.fork(fn() {
        spin(1000)
        fail Boom
      })
      spin(100000)
      0
    }) escape { Boom -> -1 }
    dbg(r)
    ()
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "-1\n");
}

/// Repeated runs give the same lines: the scheduler's order is deterministic (spec 11.1).
#[test]
fn scheduler_order_is_deterministic() {
    let src = r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn step(n: Int, tag: String) -> Int use { Tick } {
  if n == 0 { 0 } else {
    (use Tick).tick()
    if n % 500 == 0 {
      dbg("#{tag} #{n}")
      ()
    } else { () }
    step(n - 1, tag) + 1
  }
}

fn main() -> () use { Async } {
  with Ticks {
    total = Fiber.scope(fn() {
      a = Fiber.fork(fn() { step(3000, "a") })
      b = Fiber.fork(fn() { step(2000, "b") })
      d = Fiber.deferred()
      Fiber.fork(fn() {
        step(1000, "c")
        Deferred.complete(d, "done")
      })
      dbg(Deferred.await(d))
      Fiber.join(a) + Fiber.join(b)
    })
    dbg(total)
    ()
  }
  ()
}
"##;
    let Some(first) = run("fiber-determinism", src) else {
        return;
    };
    assert_eq!(first.code, 0, "{}", first.stderr);
    for _ in 0..2 {
        let Some(again) = run("fiber-determinism", src) else {
            return;
        };
        assert_eq!(again, first, "the scheduler is not deterministic");
    }
}

/// Every failing child path ends with `defect: ...` and exit code 101, never a wasm trap.
#[test]
fn defects_do_not_trap() {
    for (name, src) in [
        (
            "fiber-trap-a",
            r##"
import Std.Fiber.{Fiber, Scope}

fn main() -> () use { Async } {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { panic("child defect") })
    Fiber.join(h)
    ()
  })
  ()
}
"##,
        ),
        (
            "fiber-trap-b",
            r##"
import Std.Fiber.{Fiber, Scope}
import Std.Deferred.{Deferred}

fn main() -> () use { Async } {
  Fiber.scope(fn() {
    d = Fiber.deferred()
    h = Fiber.fork(fn() { Deferred.await(d) })
    Fiber.interrupt(h)
    Fiber.join(h)
    ()
  })
  ()
}
"##,
        ),
    ] {
        let Some(run) = run(name, src) else { return };
        assert!(
            run.stderr.lines().any(|l| l.starts_with("defect: ")),
            "{name}: {}",
            run.stderr
        );
        assert!(!run.stderr.contains("wasm trap"), "{name}: {}", run.stderr);
        assert!(
            !run.stderr.contains("unreachable"),
            "{name}: {}",
            run.stderr
        );
    }
}
