//! Freed objects go back to the allocator: a program that keeps making and dropping the same
//! objects runs in a fixed amount of linear memory. Each program here runs under a memory limit
//! far below what it allocates in total. Skipped without `wasmtime`.

mod common;

use common::*;

/// The linear memory each program may grow to: six 64 KiB pages.
const LIMIT: u32 = 6 * 65536;

/// Runs `src` with linear memory limited to [`LIMIT`], expecting it to finish with `stderr`.
fn run_limited(name: &str, src: &str, stderr: &str) {
    let Some(_) = wasmtime() else { return };
    let bytes = compile_checked(src);
    let run = run_bytes_with_command(&out_dir(name), &bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
    });
    assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    assert_eq!(run.stderr, stderr, "{name}");
}

#[test]
fn dropping_a_closure_frees_what_it_captured() {
    // Each round drops a closure whose environment holds a list of 100 cells: the cells are freed
    // after the environment, not only the environment itself.
    run_limited(
        "memory-closure",
        r#"
fn make(n: Int) -> fn() -> Int {
  xs = List.map(List.range(0, n), fn(i) { i * 2 })
  fn() { List.length(xs) }
}

fn rounds(n: Int, total: Int) -> Int {
  if n == 0 {
    total
  } else {
    f = make(100)
    rounds(n - 1, total + f())
  }
}

fn main() {
  dbg(rounds(2000, 0))
}
"#,
        "200000\n",
    );
}

#[test]
fn a_finished_operation_call_frees_its_record() {
    // A `suspend fn` operation call makes a perform record, its resume function and the name of
    // the operation for diagnostics; all of them are freed once the call returns.
    run_limited(
        "memory-operation",
        r#"
effect Retry {
  suspend fn go() -> Int
}

handler Retrying: Retry {
  suspend fn go(resume) {
    resume(1)
  }
}

fn ten() -> Int use { Retry } {
  (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go()
}

fn rounds(n: Int, total: Int) -> Int use { Retry } {
  if n == 0 {
    total
  } else {
    rounds(n - 1, total + ten())
  }
}

fn main() {
  with Retrying {
    dbg(rounds(1000, 0))
  }
}
"#,
        "10000\n",
    );
}

#[test]
fn a_sleeping_operation_call_reuses_its_memory() {
    // A call that sleeps also makes frames and a queued continuation, of other sizes than the
    // perform record; a block reused for a smaller object is given back whole.
    run_limited(
        "memory-sleeping-operation",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Retry {
  suspend fn go() -> Int
}

handler Retrying: Retry {
  suspend fn go(resume) {
    (use Clock).sleep(1)
    resume(1)
  }
}

fn rounds(n: Int, total: Int) -> Int use { Retry } {
  if n == 0 {
    total
  } else {
    rounds(n - 1, total + (use Retry).go())
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with Retrying {
        dbg(rounds(4000, 0))
      }
    }
  })
}
"#,
        "4000\n",
    );
}
