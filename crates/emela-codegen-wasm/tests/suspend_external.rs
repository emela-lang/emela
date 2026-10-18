//! A `suspend fn` external function bound to an internal host (spec 12.6): the task wrapper, an
//! in-place completion, and a completion from the driver's timers (the `after`/`now_ms`
//! primitives T-Clock's `sleep` will use).
//!
//! The hosts are the test ones in `emit::external::INTERNAL_HOSTS` (`emela:test`):
//! `sleep_ms(ms)` completes in place with `ms * 2`, `sleep_after(ms)` registers a timer. A
//! `suspend fn` external may only be called from a handler operation body (spec 12.2), so each
//! test wraps it in an effect.

mod common;

use common::*;

#[test]
fn a_suspend_external_host_completes_in_place() {
    let Some(run) = run(
        "suspend_external_in_place",
        r##"
@external(wasm, "test", "print")
fn print(s: String) -> () use {}

@external(wasm, "emela:test", "sleep_ms")
suspend fn sleep_ms(ms: Int) -> Int

effect Nap {
  suspend fn nap(ms: Int) -> Int
}
handler Naps implements Nap {
  suspend fn nap(ms, resume) { resume(sleep_ms(ms)) }
}

fn main() use { Async } {
  with Naps {
    nap = use Nap
    print("#{nap.nap(5)}")
    print("#{nap.nap(1) + nap.nap(2)}")
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "10\n6\n");
}

#[test]
fn a_suspend_external_host_completes_from_a_timer() {
    let Some(run) = run(
        "suspend_external_timer",
        r##"
import Std.Fiber.{Fiber}

@external(wasm, "test", "print")
fn print(s: String) -> () use {}

@external(wasm, "emela:test", "sleep_after")
suspend fn sleep_after(ms: Int) -> ()

effect Nap {
  suspend fn nap(ms: Int) -> ()
}
handler Naps implements Nap {
  suspend fn nap(ms, resume) {
    sleep_after(ms)
    resume(())
  }
}

fn main() use { Async } {
  with Naps {
    nap = use Nap
    print("before")
    # A child sleeps; the body joins it, so the driver's timer resumes the child.
    Fiber.scope(fn() {
      h = Fiber.fork(fn() { nap.nap(2) })
      Fiber.join(h)
    })
    nap.nap(1)
    print("after")
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "before\nafter\n");
}
