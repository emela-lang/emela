//! Tasks, groups, `Deferred`, interruption and release (spec chapter 11, 14.5). The programs are
//! ports of `crates/emela-codegen-c/tests/programs/task_*.emel`; `print` is `dbg` here, so the
//! order of the lines is what is compared. Skipped without `wasmtime`.

use crate::common;

use common::*;

/// Runs `src` and returns the run, failing if wasmtime is not available.
fn run_ok(name: &str, src: &str) -> Option<Run> {
    let run = run(name, src)?;
    assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    Some(run)
}

/// A group returns the body's value after its tasks finish (`[task.group]`).
#[test]
fn scope_joins_children_and_returns_the_body_value() {
    let Some(run) = run_ok(
        "task-scope-basic",
        r##"
import Std.Task.{Task}

fn fetch(n: Int) -> Int { n * 10 }

fn main() -> () uses { Async } {
  total = with s = Task.Group {
    a = s.start(|| fetch(1))
    b = s.start(|| fetch(2))
    a.join() + b.join()
  }
  dbg(total)
  dbg(
    with tasks = Task.Group {
      x = tasks.start(|| 1.5)
      y = tasks.start(|| Int64.from_int(7))
      u = tasks.start(|| dbg("unit child"))
      u.join()
      "#{x.join()} #{y.join()}"
    },
  )
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
        "task-scope-close",
        r##"
import Std.Task.{Task}

fn main() -> () uses { Async } {
  v = with tasks = Task.Group {
    h = tasks.start(|| dbg("child starts"))
    dbg("body returns")
    2
  }
  dbg(v)
  ()
}
"##,
    ) else {
        return;
    };
    // The task had not started when the body returned, so it is interrupted without being called
    // (`[task.group]`); only the body's output and value remain.
    assert_eq!(run.stderr, "\"body returns\"\n2\n");
}

/// Nested scopes: a child of an inner scope forks its own children, and the inner scope closes
/// before the outer body goes on (spec 11.2).
#[test]
fn nested_scopes_close_inside_out() {
    let Some(run) = run_ok(
        "task-scope-nested",
        r##"
import Std.Task.{Task}

fn main() -> () uses { Async } {
  r = with outer = Task.Group {
    late = outer.start(|| {
      dbg("late")
      9
    })
    inner = with tasks = Task.Group {
      a = tasks.start(|| {
        dbg("inner a")
        1
      })
      b = tasks.start(|| {
        dbg("inner b")
        2
      })
      a.join() + b.join()
    }
    dbg(inner)
    late.join()
  }
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
        "task-many",
        r##"
import Std.Task.{Task}

fn main() -> () uses { Async } {
  total = with tasks = Task.Group {
    hs = List.map(List.range(0, 200), |i| tasks.start(|| i + 1))
    List.fold(hs, 0, |acc, h| acc + h.join())
  }
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
/// the first value and `poll` does not wait (`[task.deferred]`).
#[test]
fn deferred_multiple_waiters_and_complete_once() {
    let Some(run) = run_ok(
        "task-deferred",
        r##"
import Std.Task.{Task}
import Std.Deferred.{Deferred}

fn main() -> () uses { Async } {
  with d = Deferred {
    with names = Deferred {
      with tasks = Task.Group {
        a = tasks.start(|| d.await() + 1)
        b = tasks.start(|| d.await() + 2)
        dbg(d.poll())
        tasks.start(|| dbg(d.complete(40)))
        dbg("#{a.join()} #{b.join()}")
        dbg(d.complete(1))
        dbg(d.poll())
        tasks.start(|| names.complete(["x", "y"]))
        dbg(names.await())
        ()
      }
    }
  }
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

/// To stop one task, start it in a group of its own and end the with of that group
/// (`[task.interrupt]`); the outer group goes on.
#[test]
fn ending_a_nested_group_stops_its_task() {
    let Some(run) = run_ok(
        "task-stop",
        r##"
import Std.Task.{Task}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn main() -> () uses { Async } {
  with Ticks {
    with tasks = Task.Group {
      with stop = Task.Group {
        stop.start(|| {
          spin(100000)
          dbg("not reached")
        })
        spin(1000)
      }
      dbg("stopped")
      ok = tasks.start(|| 7)
      dbg(ok.join())
      ()
    }
  }
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "\"stopped\"\n7\n");
}

/// Interruption waits for a running release (`[task.interrupt]`): the release finishes, then the
/// task unwinds, and the group waits for it.
#[test]
fn interrupt_waits_for_a_release() {
    let Some(run) = run_ok(
        "task-release",
        r##"
import Std.Task.{Task}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
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
    Self
  }
  fn name() { "slow" }
  release {
    dbg("release slow starts")
    spin(5000)
    dbg("release slow ends")
  }
}

fn main() -> () uses { Async } {
  with Ticks {
    with tasks = Task.Group {
      tasks.start(|| with SlowRes { spin(100000) })
      spin(1500)
      dbg("interrupting")
    }
    dbg("interrupted")
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
        "task-notes",
        r##"
import Std.Task.{Task}
import Std.Exit.{Exit, Success, Failure}
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
    Self
  }
  fn name() { "res" }
  release { panic("second") }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
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

fn main() -> () uses { Async } {
  with Ticks {
    caught = Exit.attempt(|| {
      with tasks = Task.Group {
        tasks.start(|| {
          spin(1000)
          fail Boom
        })
        tasks.start(|| with Res { spin(100000) })
        spin(100000)
        ()
      }
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
        "\"init res\"\n\"error: Boom\\nnote: another task also failed: defect: second\"\n"
    );
}

/// A note is attached to the failure it happened in: two scopes that fail the same static error
/// must not share the notes of the first one (spec 11.2; the C runtime copies a static error).
#[test]
fn a_note_belongs_to_its_own_failure() {
    let Some(run) = run_ok(
        "task-notes-static",
        r##"
import Std.Task.{Task}
import Std.Exit.{Exit, Success, Failure}
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
    Self
  }
  fn name() { "res" }
  release { panic("second") }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn once() -> String uses { Async, Tick } {
  caught = Exit.attempt(|| {
    with tasks = Task.Group {
      tasks.start(|| {
        spin(1000)
        fail Boom
      })
      tasks.start(|| with Res { spin(100000) })
      spin(100000)
      ()
    }
  })
  caught match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() -> () uses { Async } {
  with Ticks {
    dbg(once())
    dbg(once())
  }
  ()
}
"##,
    ) else {
        return;
    };
    let note = "note: another task also failed: defect: second";
    let causes: Vec<&str> = run
        .stderr
        .lines()
        .filter(|line| line.starts_with("\"error: Boom"))
        .collect();
    assert_eq!(causes.len(), 2, "{}", run.stderr);
    for cause in causes {
        assert_eq!(cause.matches(note).count(), 1, "{cause}");
    }
}

/// `Exit.attempt` does not catch the interruption of its own task (`[task.cause]`).
#[test]
fn attempt_does_not_catch_its_own_interruption() {
    let Some(run) = run_ok(
        "task-attempt-interrupt",
        r##"
import Std.Task.{Task}
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
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

fn main() -> () uses { Async } {
  with Ticks {
    with tasks = Task.Group {
      tasks.start(|| {
        Exit.attempt(|| spin(100000)) match {
          Success(_) -> dbg("attempt success")
          Failure(c) -> dbg("attempt caught #{Cause.to_string(c)}")
        }
      })
      spin(1000)
    }
    dbg("interrupted")
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
        "task-body-failure",
        r##"
import Std.Task.{Task}
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause}

error Boom

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn spin(n: Int) -> () uses { Tick } {
  if n == 0 {
    ()
  } else {
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

fn main() -> () uses { Async } {
  with Ticks {
    r = with tasks = Task.Group {
      tasks.start(|| {
        spin(1000)
        fail Boom
      })
      spin(100000)
      0
    } escape { Boom -> -1 }
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
import Std.Task.{Task}
import Std.Deferred.{Deferred}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

fn step(n: Int, tag: String) -> Int uses { Tick } {
  if n == 0 {
    0
  } else {
    (use Tick).tick()
    if n % 500 == 0 {
      dbg("#{tag} #{n}")
      ()
    } else {
      ()
    }
    step(n - 1, tag) + 1
  }
}

fn main() -> () uses { Async } {
  with Ticks {
    total = with d = Deferred {
      with tasks = Task.Group {
        a = tasks.start(|| step(3000, "a"))
        b = tasks.start(|| step(2000, "b"))
        tasks.start(|| {
          step(1000, "c")
          d.complete("done")
        })
        dbg(d.await())
        a.join() + b.join()
      }
    }
    dbg(total)
    ()
  }
  ()
}
"##;
    let Some(first) = run("task-determinism", src) else {
        return;
    };
    assert_eq!(first.code, 0, "{}", first.stderr);
    for _ in 0..2 {
        let Some(again) = run("task-determinism", src) else {
            return;
        };
        assert_eq!(again, first, "the scheduler is not deterministic");
    }
}

/// Every failing task path ends with `defect: ...` and exit code 101, never a wasm trap.
#[test]
fn defects_do_not_trap() {
    for (name, src) in [
        (
            "task-trap-a",
            r##"
import Std.Task.{Task}

fn main() -> () uses { Async } {
  with tasks = Task.Group {
    h = tasks.start(|| panic("child defect"))
    h.join()
    ()
  }
  ()
}
"##,
        ),
        (
            "task-trap-b",
            r##"
import Std.Task.{Task}
import Std.Deferred.{Deferred}

fn main() -> () uses { Async } {
  with d = Deferred[Int] {
    with tasks = Task.Group {
      tasks.start(|| d.await())
      panic("body defect")
    }
  }
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
