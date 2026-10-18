//! `Cause.to_string` on a defect `Fiber.attempt` caught (spec 11.5).
//!
//! The message of a `runtime defect` is a borrowed operand: the Heap IR drops the caller's
//! reference right after raising it, so the runtime must keep its own (`em_raise` dups). A fiber
//! program captures the pending defect afterwards (`Fiber.attempt`), and without the counted
//! reference the message's memory is reused and `Cause.to_string` reads a corrupted string.

mod common;

use common::*;

#[test]
fn a_caught_defect_keeps_a_heap_message() {
    let Some(run) = run(
        "cause_heap_message",
        r##"
import Std.Fiber.{Fiber, Exit, Success, Failure}
import Std.Cause.{Cause}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> Cause.to_string(c)
  }
}

fn main() {
  dbg(show(Fiber.attempt(fn() { panic(String.append("hello ", "world")) })))
  dbg(show(Fiber.attempt(fn() { panic("literal") })))
  n = 40
  dbg(show(Fiber.attempt(fn() { panic("n is #{n + 2}") })))
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(
        run.stderr,
        "\"defect: hello world\"\n\"defect: literal\"\n\"defect: n is 42\"\n"
    );
}
