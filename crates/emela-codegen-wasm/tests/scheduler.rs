//! Scheduler regressions (spec chapters 11 and 14): the runtime vectors grow instead of silently
//! dropping (W5-1), and the input/resume values the C runtime releases are released here too.

mod common;

use common::*;

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
