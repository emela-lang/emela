//! The `Std.Clock` WASM hosts (`emela:clock`): the REALTIME `now_milliseconds`, the suspending
//! `sleep_milliseconds` (a driver timer), the corpus' value-returning `sleep`, and TestClock's
//! `when_stalled` virtual-time hook.

mod common;

use common::*;

#[test]
fn system_clock_now_reads_the_unix_epoch() {
    let Some(run) = run(
        "clock_now",
        r#"
import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Instant.{Instant}

fn main() {
  with SystemConsole, SystemClock {
    ms = Instant.to_unix_milliseconds((use Clock).now())
    # A wall clock after 2020-09-13, so the REALTIME clock (not the monotonic one) is read.
    (use Console).print_line(if ms > 1600000000000 { "epoch" } else { "zero" })
  }
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "epoch\n");
}

#[test]
fn system_clock_sleep_wakes_the_first_fiber() {
    // `sleep` suspends through a real timer, so the shorter sleeper ends first (spec 8.7).
    let Some(run) = run(
        "clock_sleep_race",
        r##"
import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Fiber.{Fiber}

fn nap(id: Int, ms: Int) -> Int use { Clock } {
  (use Clock).sleep(ms)
  id
}

fn main() {
  with SystemConsole, SystemClock {
    winner = Fiber.race([fn() { nap(1, 1000) }, fn() { nap(2, 20) }])
    (use Console).print_line("#{winner}")
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "2\n");
}

#[test]
fn a_suspend_external_sleep_returns_its_argument() {
    // The corpus' host (`@external(wasm, "emela:clock", "sleep")`) really suspends and resumes
    // with the milliseconds slept, like JS' `sleep.mjs` and C's `emela_test_sleep`.
    let Some(run) = run(
        "clock_sleep_value",
        r##"
import Std.Console.{Console, SystemConsole}

@external(wasm, "emela:clock", "sleep")
suspend fn sleep(ms: Int) -> Int

effect Nap {
  suspend fn nap(ms: Int) -> Int
}
handler Naps implements Nap {
  suspend fn nap(ms, resume) { resume(sleep(ms)) }
}

fn main() {
  with SystemConsole, Naps {
    nap = use Nap
    (use Console).print_line("#{nap.nap(7)}")
    (use Console).print_line("#{nap.nap(3) + nap.nap(4)}")
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "7\n7\n");
}

#[test]
fn test_clock_advances_virtual_time() {
    // TestClock sleeps through `when_stalled`, which moves the virtual time forward without a real
    // wait (spec 8.8).
    let Some(run) = run(
        "clock_test_clock",
        r##"
import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

fn main() {
  with SystemConsole {
    start = Instant.from_unix_milliseconds(Int64.from_int(0))
    (_, end) = State.run(start, fn() {
      with TestClock {
        (use Clock).sleep(1000)
        (use Clock).sleep(500)
      }
    })
    (use Console).print_line("#{Instant.to_unix_milliseconds(end)}")
  }
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "stderr: {}", run.stderr);
    assert_eq!(run.stdout, "1500\n");
}
