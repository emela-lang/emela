//! `Std.Clock.sleep` on the WASI 0.3 target: the real monotonic clock and the TestClock's virtual
//! time (spec 8.7, 8.8). Skipped without `wasmtime`.

mod common;

use std::time::{Duration, Instant};

use common::*;

/// Runs a compiled program, returning its outcome and how long the run took.
fn run_timed(name: &str, src: &str) -> (Run, Duration) {
    let bytes = compile_checked(src);
    let start = Instant::now();
    let run = run_bytes(&out_dir(name), &bytes);
    (run, start.elapsed())
}

#[test]
fn system_clock_sleep_waits_on_the_host_clock() {
    let Some(_) = wasmtime() else { return };
    let (run, elapsed) = run_timed(
        "clock-system-sleep",
        r#"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    clock = use Clock
    dbg("before")
    clock.sleep(200)
    dbg("after")
  }
}
"#,
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"before\"\n\"after\"\n");
    assert!(
        elapsed >= Duration::from_millis(150),
        "the sleep returned too early after {elapsed:?}"
    );
}

#[test]
fn zero_and_negative_sleeps_do_not_wait() {
    let Some(_) = wasmtime() else { return };
    let (run, elapsed) = run_timed(
        "clock-clamp",
        r#"
import Std.Clock.{Clock, SystemClock}

fn main() {
  with SystemClock {
    clock = use Clock
    clock.sleep(0)
    dbg("zero")
    clock.sleep(-5)
    dbg("negative")
    clock.sleep(0)
    dbg("zero again")
  }
}
"#,
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"zero\"\n\"negative\"\n\"zero again\"\n");
    assert!(
        elapsed < Duration::from_secs(5),
        "zero and negative sleeps waited for {elapsed:?}"
    );
}

#[test]
fn test_clock_advances_virtual_time() {
    let Some(_) = wasmtime() else { return };
    let Some(run) = run(
        "clock-test-clock",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() {
    with TestClock {
      clock = use Clock
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(1000)
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(-5)
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(0)
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(2147483647)
      dbg(Instant.to_unix_milliseconds(clock.now()))
    }
  })
  dbg(Instant.to_unix_milliseconds(end))
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "0\n1000\n1000\n1000\n2147484647\n2147484647\n");
}

#[test]
fn several_fibers_sleep_in_the_order_of_their_wake_times() {
    // Two fibers sleep for different times in the same TestClock. When nothing can go on, the
    // earliest sleeper wakes first (spec 8.8); the later one wakes after it, and the virtual time
    // ends at the later wake time. The observable lines are the order and the times, not the
    // handler's update at call time (an open question in the handoff).
    let Some(_) = wasmtime() else { return };
    let Some(run) = run(
        "clock-many-sleepers",
        r##"
import Std.Fiber.{Fiber, Scope}
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

fn sleeper(label: String, ms: Int) -> () use { Clock } {
  (use Clock).sleep(ms)
  dbg("#{label} #{Instant.to_unix_milliseconds((use Clock).now())}")
  ()
}

fn main() -> () use { Async } {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() {
    with TestClock {
      Fiber.scope(fn() {
        a = Fiber.fork(fn() { sleeper("a", 100) })
        b = Fiber.fork(fn() { sleeper("b", 50) })
        Fiber.join(a)
        Fiber.join(b)
        ()
      })
    }
  })
  dbg("end #{Instant.to_unix_milliseconds(end)}")
  ()
}
"##,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"b 50\"\n\"a 100\"\n\"end 100\"\n");
}

#[test]
fn a_sleep_inside_an_operation_implementation_resumes_outwards() {
    let Some(_) = wasmtime() else { return };
    let Some(run) = run(
        "clock-nested",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Outer {
  suspend fn go() -> Int
}

handler OuterHandler: Outer {
  suspend fn go(resume) {
    clock = use Clock
    clock.sleep(100)
    resume(7)
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() {
    with TestClock {
      with OuterHandler {
        dbg("go #{(use Outer).go()}")
      }
      dbg("now #{Instant.to_unix_milliseconds((use Clock).now())}")
    }
  })
  dbg("end #{Instant.to_unix_milliseconds(end)}")
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"go 7\"\n\"now 100\"\n\"end 100\"\n");
}

#[test]
fn an_implementation_that_suspends_twice_resumes() {
    // An operation implementation may suspend again after a resume (spec 14.5 item 3): the same
    // handler sleeps twice, sleeps in a `List.map` repetition and sleeps before calling another
    // `suspend fn` operation. Each resume must find its caller, so the run ends normally rather
    // than unwinding as `interrupted`. The virtual time is 100 + 50 + 3 * 10 + 5 = 185.
    let Some(_) = wasmtime() else { return };
    let Some(run) = run(
        "clock-resuspend",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Retry {
  suspend fn go() -> Int
}
effect Ping {
  suspend fn ping() -> Int
}

handler Pinging: Ping {
  suspend fn ping(resume) {
    (use Clock).sleep(5)
    resume(1)
  }
}

handler Retrying: Retry {
  suspend fn go(resume) {
    clock = use Clock
    clock.sleep(100)
    clock.sleep(50)
    List.map(List.range(0, 3), fn(i) { clock.sleep(10) })
    n = (use Ping).ping()
    resume(n)
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() {
    with TestClock {
      with Pinging, Retrying {
        dbg("go #{(use Retry).go()}")
      }
    }
  })
  dbg("end #{Instant.to_unix_milliseconds(end)}")
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.stderr, "\"go 1\"\n\"end 185\"\n");
}

#[test]
fn an_interrupted_sleeper_reports_interrupted() {
    // The handler sleeps, then returns without resuming. Nothing can wake the caller, so the run
    // stalls and unwinds as interrupted (spec 14.5), which is exit code 130 on every target.
    let Some(_) = wasmtime() else { return };
    let Some(run) = run(
        "clock-interrupted",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Outer {
  suspend fn go() -> Int
}

handler Ignoring: Outer {
  suspend fn go(resume) {
    clock = use Clock
    clock.sleep(10)
    dbg("not resuming")
    ()
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with Ignoring {
        dbg("go #{(use Outer).go()}")
      }
    }
  })
  dbg("not reached")
}
"#,
    ) else {
        return;
    };
    assert_eq!(run.code, 130, "{}", run.stderr);
    assert_eq!(run.stderr, "\"not resuming\"\ninterrupted\n");
}

#[test]
fn an_abandoned_sleeper_does_not_poison_the_next_run() {
    // The first test resumes its caller, then goes to sleep; spec 14.5 does not wait for that
    // implementation continuation, so the run ends with a live TestClock wait. The wait's list
    // links must be reset before the next run, whose own sleep must not follow a stale tail.
    let Some(_) = wasmtime() else { return };
    let src = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Job {
  suspend fn run() -> Int
}

handler Later: Job {
  suspend fn run(resume) {
    resume(7)
    (use Clock).sleep(1)
    dbg("impl woke")
    ()
  }
}

@test
fn leaves_a_sleeping_implementation() -> () {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with Later {
        dbg((use Job).run())
        ()
      }
    }
  })
  ()
}

@test
fn sleeps_after_the_run() -> () {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      (use Clock).sleep(1)
      dbg("after")
    }
  })
  ()
}
"#;
    let bytes = to_wasm_tests(
        src,
        &["leaves_a_sleeping_implementation", "sleeps_after_the_run"],
    )
    .unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    let run = run_bytes(&out_dir("clock-abandoned-sleeper"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    // The abandoned continuation must not be woken by the next run's TestClock.
    assert!(!run.stderr.contains("impl woke"), "{}", run.stderr);
    assert!(run.stderr.contains("\"after\""), "{}", run.stderr);
    for name in ["leaves_a_sleeping_implementation", "sleeps_after_the_run"] {
        assert!(
            run.stdout.contains(&format!(
                "\x1eemela-test {{\"event\":\"result\",\"name\":\"{name}\",\"outcome\":\"ok\"}}"
            )),
            "{name}: {}",
            run.stdout
        );
    }
}
