//! Scheduler regressions (spec chapters 11 and 14): the runtime vectors grow instead of silently
//! dropping (W5-1), and the input/resume and note values the C runtime releases are released here
//! too (W3-6, W5-2, W5-4).

mod common;

use common::*;

/// A `suspend fn` that returns a reference (`Std.FileSystem.read_text` returns a `String`) must
/// deliver it: `perform_end` continues the caller with the value of a `MODE_VALUE_REF` input, not
/// `0` (the `examples/notes` regression). `dbg` prints the read text to stderr.
#[test]
fn a_suspend_fn_reference_result_survives() {
    let Some(run) = run(
        "scheduler_suspend_ref_result",
        r##"
import Std.FileSystem.{FileSystem, TestFileSystem, TestFiles}
import Std.State.{State}

fn main() {
  (_, _) = State.run(TestFiles(files: [("a.txt", "hi")], directories: ["notes"]), fn() {
    with TestFileSystem {
      fs = use FileSystem
      dbg(fs.read_text("a.txt"))
      fs.append_text("notes/monday.txt", "a\n")
      fs.append_text("notes/monday.txt", "b\n")
      dbg(fs.read_text("notes/monday.txt"))
      ()
    }
  })
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        run.stderr, "\"hi\"\n\"a\\nb\\n\"\n",
        "the reference returned by a suspend fn must not be dropped"
    );
}

/// A scope must keep every forked child, not just the first `S_CHILDREN` slots (256): the C
/// `em_vec_push` grows, and children left running when the body returns are interrupted (spec
/// 11.2). With the silent drop the 257th child onwards was untracked, escaped the interruption and
/// ran after its scope had ended (printing here).
#[test]
fn a_scope_tracks_more_children_than_its_initial_capacity() {
    let Some(run) = run(
        "scheduler_many_children",
        r##"
import Std.Fiber.{Fiber, Scope}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

fn child(i: Int) -> () use {} { print("c#{i}") }

fn spawn(i: Int, n: Int) -> () use { Scope[{}, {}] } {
  if i >= n { () } else {
    Fiber.fork(fn() { child(i) })
    spawn(i + 1, n)
  }
}

fn main() use { Async } {
  Fiber.scope(fn() {
    spawn(0, 300)
    print("body")
  })
  print("done")
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout, "body\ndone\n",
        "children left running must be interrupted, not escape their ended scope"
    );
}

/// Resuming twice is a defect; the second resume's reference value must be released, not leaked
/// (W5-2, the "used" path of `resume_with`).
#[test]
fn a_second_resume_is_a_defect_and_releases_its_value() {
    let Some(run) = run(
        "scheduler_resume_twice",
        r##"
import Std.Fiber.{Fiber, Scope}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

effect Ask {
  suspend fn ask() -> String
}

handler Asks implements Ask {
  suspend fn ask(resume) {
    resume("one")
    resume("two")
  }
}

fn main() use { Async } {
  with Asks {
    print((use Ask).ask())
  }
  print("not reached")
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "");
    assert_eq!(
        run.stderr,
        "defect: resume called twice in operation `ask` of handler `Asks`\n"
    );
}

/// A `suspend fn` implementation that fails after it resumed ends the run with the shown failure;
/// the failure value is consumed, not leaked (W3-6, `settled` -> `em_show_failure`).
#[test]
fn a_failure_after_resume_shows_the_failure() {
    let Some(run) = run(
        "scheduler_failure_after_resume",
        r##"
import Std.Fiber.{Fiber, Scope}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

error Invalid(reason: String)

effect Late {
  suspend fn late() -> Int fails Invalid
}

handler FailsLate implements Late {
  suspend fn late(resume) {
    resume(1)
    print("after resume")
    fail Invalid("boom")
  }
}

fn main() use { Async } {
  Fiber.scope(fn() {
    with FailsLate {
      r = (use Late).late() escape { Invalid(_) -> -1 }
      print("got #{r}")
    }
  })
  print("not reached")
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "after resume\n");
    assert_eq!(
        run.stderr,
        "defect: operation `late` of handler `FailsLate` failed after resume: Invalid(reason: \"boom\")\n"
    );
}

/// A second failure of a scope's children becomes a note with the shown cause, and the note's
/// intermediate strings are released (W5-4: `cause_note`/`notes_add`/`scope_failed`).
#[test]
fn a_later_child_failure_becomes_a_note() {
    let Some(run) = run(
        "scheduler_later_failure_note",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

error Invalid(reason: String)

effect Tick {
  suspend fn tick() -> ()
}
handler Ticks implements Tick {
  suspend fn tick(resume) { resume(()) }
}

effect Name {
  fn name() -> String
}
handler Res implements Name {
  init { Res }
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

fn main() use { Async } {
  with Ticks {
    caught = Fiber.attempt(fn() {
      Fiber.scope(fn() {
        Fiber.fork(fn() {
          spin(1000)
          fail Invalid("first")
        })
        Fiber.fork(fn() { with Res { spin(100000) } })
        spin(100000)
      })
    })
    print(show(caught))
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout,
        "error: Invalid(reason: \"first\")\nnote: another fiber also failed: defect: second\n"
    );
}

/// The same note path when the first failure is a defect: the defect's message gains the note and
/// the replaced string is released (W5-4, `cause_note`'s `CAUSE_DIE` branch).
#[test]
fn a_later_defect_is_notes_on_a_defect() {
    let Some(run) = run(
        "scheduler_later_defect_note",
        r##"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

effect Tick {
  suspend fn tick() -> ()
}
handler Ticks implements Tick {
  suspend fn tick(resume) { resume(()) }
}

effect Name {
  fn name() -> String
}
handler Res implements Name {
  init { Res }
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

fn main() use { Async } {
  with Ticks {
    caught = Fiber.attempt(fn() {
      Fiber.scope(fn() {
        Fiber.fork(fn() {
          spin(1000)
          panic("first")
        })
        Fiber.fork(fn() { with Res { spin(100000) } })
        spin(100000)
      })
    })
    print(show(caught))
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        run.stdout,
        "defect: first\nnote: another fiber also failed: defect: second\n"
    );
}
