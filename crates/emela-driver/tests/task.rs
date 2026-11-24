//! Concurrency (spec chapter 11) through the `emela run` and `emela test` paths. Runs source
//! through lowering, codegen-js and node, checking the behavior of the chapter 11 examples
//! (spec 11.2, 11.3, 11.5, 11.7, 11.8). When cc starts, every program also runs on the native target
//! (the runtime with its leak check) and must give the same result (spec chapter 16).
//!
//! Time advances with SystemClock's real timers. Wait times differ widely, and only the order of
//! events is checked.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    CoreC, CoreJs, Input, Lower, LowerTests, MemoryFiles, Output, RunOptions, RunOutput, Target,
    TestOptions, TestOutcome, cc_command, run, test,
};

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-task-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The C backend with the runtime's leak check, or `None` (with a message) without cc.
fn c_backend() -> Option<CoreC> {
    let (cc, mut flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping C: cannot start the C compiler `{}`", cc.display());
        return None;
    }
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    Some(CoreC::with_cc(cc, &flags))
}

/// Runs main with `emela run` on JS, and on C when cc starts (the results must agree). Returns
/// (exit code, stdout, stderr).
fn run_main(name: &str, main: &str) -> (i32, String, String) {
    let js = run_main_on(name, main, Target::Js);
    if c_backend().is_some() {
        let c = run_main_on(name, main, Target::Native);
        if js != c {
            // Written to stderr too, in case the panic message is lost.
            eprintln!("{name}: JS and C differ\nJS: {js:?}\nC: {c:?}");
        }
        assert_eq!(js, c, "JS and C differ");
    }
    js
}

fn run_main_on(name: &str, main: &str, target: Target) -> (i32, String, String) {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_, _| {}),
        Target::Native => run(
            &fs,
            &input,
            &mut frontend,
            &mut c_backend().expect("checked by the caller"),
            &options,
            |_, _| {},
        ),
        Target::Wasm => unreachable!("the task tests run js and c"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    // The programs may still write `use E` as an expression (W0403, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| matches!(d.code, Some(emela_driver::code::DEPRECATED_USE))),
        "{target}: {:?}",
        analysis.diagnostics
    );
    let RunOutput {
        code,
        stdout,
        stderr,
    } = output.unwrap();
    (
        code,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

/// Runs with `emela test` on JS, and on C when cc starts (the results must agree). Returns
/// (name, outcome) per test.
fn run_tests(name: &str, main: &str) -> Vec<(String, TestOutcome)> {
    let js = run_tests_on(name, main, Target::Js);
    if c_backend().is_some() {
        let c = run_tests_on(name, main, Target::Native);
        if js != c {
            // Written to stderr too, in case the panic message is lost.
            eprintln!("{name}: JS and C differ\nJS: {js:?}\nC: {c:?}");
        }
        assert_eq!(js, c, "JS and C differ");
    }
    js
}

fn run_tests_on(name: &str, main: &str, target: Target) -> Vec<(String, TestOutcome)> {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", main)];
    let fs = MemoryFiles::new(sources.iter().copied());
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let mut frontend = LowerTests::default().with_target(target);
    let (analysis, report) = match target {
        Target::Js => test(
            &fs,
            &input,
            &mut frontend,
            &mut CoreJs,
            &options,
            |_| {},
            &mut out,
        ),
        Target::Native => test(
            &fs,
            &input,
            &mut frontend,
            &mut c_backend().expect("checked by the caller"),
            &options,
            |_| {},
            &mut out,
        ),
        Target::Wasm => unreachable!("the task tests run js and c"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    // The programs still write `use E` as an expression (W0403, deprecated).
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| d.code == Some(emela_driver::code::DEPRECATED_USE)),
        "{target}: {:?}",
        analysis.diagnostics
    );
    report
        .unwrap()
        .results
        .iter()
        .map(|r| (r.name.clone(), r.outcome.clone()))
        .collect()
}

/// Declarations shared by every example. `fetch(n, ms)` waits ms, prints `fetched n` and returns
/// `n * 10`. `Res` is a handler that prints on init and release.
const PRELUDE: &str = r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{Clock, SystemClock}
import Std.Task.{Task}
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die}
import Std.Deferred.{Deferred}

error Boom

effect Name {
  fn name() -> String
}

handler Outer: Name {
  fn name() { "outer" }
}

handler Inner: Name {
  fn name() { "inner" }
}

handler Res: Name {
  init {
    (use Console).print_line("init res")
    Res
  }
  fn name() { "res" }
  release { (use Console).print_line("release res") }
}

fn say(text: String) -> () uses { Console } {
  (use Console).print_line(text)
}

fn fetch(n: Int, ms: Int) -> Int uses { Clock, Console } {
  (use Clock).sleep(ms)
  say("fetched #{n}")
  n * 10
}

fn boom() -> Int fails Boom { fail Boom }

fn kind_name(k: CauseKind) -> String {
  k match {
    Fail -> "Fail"
    Die -> "Die"
  }
}

fn show[A, E](e: Exit[A, E]) -> String {
  e match {
    Success(_) -> "Success"
    Failure(c) -> "#{kind_name(Cause.kind(c))} #{Cause.to_string(c)}"
  }
}
"##;

fn source(main: &str) -> String {
    format!("{PRELUDE}\n{main}")
}

#[test]
fn children_run_concurrently_and_are_joined() {
    // `[task.group]`: tasks wait concurrently and their values are received with join.
    let (code, out, err) = run_main(
        "scope",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    total = with s = Task.Group {
      a = s.start(|| fetch(1, 60))
      b = s.start(|| fetch(2, 10))
      a.join() + b.join()
    }
    say("total #{total}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "fetched 2\nfetched 1\ntotal 30\n", "")
    );
}

#[test]
fn combinators() {
    // spec 11.3: all, race, both, map, timeout. Results keep the original order; race takes the
    // first to finish.
    let (code, out, err) = run_main(
        "combinators",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    xs = Task.all([|| fetch(1, 60), || fetch(2, 10)])
    say("all #{xs}")
    w = Task.race([|| fetch(3, 200), || fetch(4, 10)])
    say("race #{w}")
    (p, q) = Task.both(|| fetch(5, 10), || "x")
    say("both #{p} #{q}")
    ys = Task.map([1, 2, 3], |i| fetch(i, 100 - i * 40))
    say("map #{ys}")
    t = Task.timeout(200, || fetch(6, 10))
    u = Task.timeout(10, || fetch(7, 1000))
    say("timeout #{t} #{u}")
    r = Task.race([|| boom(), || fetch(8, 1000)]) escape { Boom -> -1 }
    say("race failed #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "fetched 2\nfetched 1\nall [10, 20]\nfetched 4\nrace 40\nfetched 5\nboth 50 x\n\
             fetched 3\nfetched 2\nfetched 1\nmap [10, 20, 30]\nfetched 6\n\
             timeout Some(60) None\nrace failed -1\n",
            ""
        )
    );
}

#[test]
fn tasks_get_the_handlers_outside_their_group() {
    // `[task.group]`: a task uses the handlers in effect outside the with of its group. A `with`
    // inside the body of the group does not reach tasks, even through a closure created inside that `with`
    // (`[effect.with]`: the side that calls a closure provides its R).
    let (code, out, err) = run_main(
        "handlers",
        &source(
            r##"
fn main() {
  with SystemConsole, Outer {
    with tasks = Task.Group {
      outside = || say((use Name).name())
      with Inner {
        a = tasks.start(outside)
        b = tasks.start(|| say((use Name).name()))
        say("body #{(use Name).name()}")
        a.join()
        b.join()
      }
    }
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "body inner\nouter\nouter\n", "")
    );
}

#[test]
fn tasks_start_siblings_into_their_own_group() {
    // `[task.group]`: a task can start siblings into its group, and the group waits for them too.
    // With nested groups, a task of the inner group starts into the inner group, and the
    // capability of the outer group starts into the outer group. An error from a task started by a
    // task of the inner group reaches the inner group.
    let (code, out, err) = run_main(
        "siblings",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    with tasks = Task.Group {
      a = tasks.start(|| {
        b = tasks.start(|| fetch(2, 10))
        say("child")
        b.join()
      })
      say("joined #{a.join()}")
    }
    r = with outer = Task.Group {
      late = outer.start(|| fetch(9, 50))
      inner = with tasks = Task.Group {
        tasks.start(|| {
          h = tasks.start(|| boom())
          h.join()
        })
        (use Clock).sleep(1000)
        0
      } escape { Boom -> -1 }
      say("inner #{inner}")
      late.join()
    }
    say("outer #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "child\nfetched 2\njoined 20\ninner -1\nfetched 9\nouter 90\n",
            ""
        )
    );
}

#[test]
fn a_failing_task_interrupts_its_siblings() {
    // `[task.group]`, `[task.interrupt]`: a task's failure interrupts its siblings and the body,
    // runs releases, then becomes the failure of the with. Remaining tasks are interrupted even
    // when the body returns a value.
    let (code, out, err) = run_main(
        "child-fails",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    r = with tasks = Task.Group {
      tasks.start(|| with Res { (use Clock).sleep(1000) })
      tasks.start(|| {
        (use Clock).sleep(10)
        boom()
      })
      (use Clock).sleep(1000)
      say("not reached")
      1
    } escape { Boom -> -1 }
    say("group #{r}")
    v = with tasks = Task.Group {
      tasks.start(|| with Res { (use Clock).sleep(1000) })
      (use Clock).sleep(10)
      2
    }
    say("value #{v}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "init res\nrelease res\ngroup -1\ninit res\nrelease res\nvalue 2\n",
            ""
        )
    );
}

#[test]
fn later_failures_are_notes() {
    // `[task.group]`: if two or more tasks fail, the first failure is the result and later ones
    // become notes.
    let (code, out, err) = run_main(
        "notes",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    with tasks = Task.Group {
      tasks.start(|| {
        (use Clock).sleep(10)
        panic("first")
      })
      tasks.start(|| {
        with Res {
          (use Clock).sleep(1000)
        }
      })
      (use Clock).sleep(1000)
    }
  }
}
"##,
        )
        .replace(
            "release { (use Console).print_line(\"release res\") }",
            "release { panic(\"second\") }",
        ),
    );
    assert_eq!((code, out.as_str()), (101, "init res\n"), "{err}");
    assert!(err.contains("first"), "{err}");
    assert!(
        err.contains("note: another task also failed: defect: second"),
        "{err}"
    );
}

#[test]
fn attempt_and_raise() {
    // `[task.cause]`: attempt catches Fail and Die. Ending a nested group stops its task
    // (`[task.interrupt]`). An error re-raised with Cause.raise can be caught by escape.
    let (code, out, err) = run_main(
        "attempt-raise",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    say(show(Exit.attempt(|| boom())))
    say(show(Exit.attempt(|| panic("bad"))))
    say(show(Exit.attempt(|| fetch(1, 1))))
    with tasks = Task.Group {
      with stop = Task.Group {
        stop.start(|| with Res { (use Clock).sleep(1000) })
        (use Clock).sleep(10)
      }
      say("stopped")
      ok = tasks.start(|| 7)
      say("join #{show(Exit.attempt(|| ok.join()))}")
    }
    r = with tasks = Task.Group {
      Exit.attempt(|| boom()) match {
        Success(v) -> v
        Failure(c) -> Cause.raise(c)
      }
    } escape { Boom -> -2 }
    say("raise #{r}")
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (
            0,
            "Fail error: Boom\nDie defect: bad\nfetched 1\nSuccess\ninit res\nrelease res\n\
             stopped\njoin Success\nraise -2\n",
            ""
        )
    );
}

#[test]
fn deferred() {
    // `[task.deferred]`: a Deferred can be completed once, resuming every waiter. poll does not
    // wait.
    let (code, out, err) = run_main(
        "deferred",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    with d = Deferred {
      with tasks = Task.Group {
        a = tasks.start(|| d.await() + 1)
        b = tasks.start(|| d.await() + 2)
        tasks.start(|| {
          (use Clock).sleep(10)
          d.complete(40)
        })
        say("poll #{d.poll()}")
        say("joined #{a.join()} #{b.join()}")
        say("again #{d.complete(1)} #{d.poll()}")
      }
    }
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "poll None\njoined 41 42\nagain False Some(40)\n", "")
    );
}

#[test]
fn tasks_in_tests() {
    // spec chapter 13: tasks also run inside test functions. A task's failure becomes that test's
    // error or defect. `child_defect` infers a type parameter (the task returns Never);
    // the native target instantiates it (with `Never`) instead of skipping the test.
    let outcomes = run_tests(
        "tests",
        &source(
            r##"
@test
fn joins() {
  with SystemConsole, SystemClock {
    xs = Task.all([|| fetch(1, 20), || fetch(2, 10)])
    assert xs == [10, 20]
  }
}

@test
fn races() {
  with SystemConsole, SystemClock {
    assert Task.race([|| fetch(1, 200), || fetch(2, 10)]) == 20
  }
}

@test
fn child_error() {
  with tasks = Task.Group {
    h = tasks.start(|| boom())
    h.join()
  }
}

@test
fn child_defect() {
  with tasks = Task.Group {
    h = tasks.start(|| panic("child"))
    h.join()
  }
}
"##,
        ),
    );
    assert_eq!(
        outcomes,
        [
            ("joins".to_owned(), TestOutcome::Passed),
            ("races".to_owned(), TestOutcome::Passed),
            ("child_error".to_owned(), TestOutcome::Error("Boom".into())),
            (
                "child_defect".to_owned(),
                TestOutcome::Defect("child".into())
            ),
        ]
    );
}

#[test]
fn test_clock_runs_on_virtual_time() {
    // spec 8.8: TestClock runs on virtual time. When no task can make progress, it advances to
    // the earliest scheduled time and wakes sleepers at the same time in the order sleep was
    // called. Task.timeout and race with a one-hour sleep finish without really waiting
    // (spec 11.3).
    let started = std::time::Instant::now();
    let outcomes = run_tests(
        "virtual-time",
        &format!(
            "{}{PRELUDE}{}",
            "import Std.Clock.{TestClock}\nimport Std.Console.{TestConsole}\n\
             import Std.State.{State}\nimport Std.Instant.{Instant}\n",
            r##"
fn at(text: String) -> () uses { Clock, Console } {
  ms = Instant.to_unix_milliseconds((use Clock).now())
  say("#{text} at #{ms}")
}

fn zero() -> Instant {
  Instant.from_unix_milliseconds(Int64.from_int(0))
}

@test
fn wakes_in_time_order() {
  ((_, lines), end) = State.run(zero(), || {
    State.run([], || {
      with TestConsole, TestClock {
        with tasks = Task.Group {
          tasks.start(|| {
            (use Clock).sleep(30)
            at("a")
          })
          tasks.start(|| {
            (use Clock).sleep(10)
            at("b")
          })
          tasks.start(|| {
            (use Clock).sleep(10)
            at("c")
          })
          (use Clock).sleep(40)
          at("body")
        }
      }
    })
  })
  assert lines == ["b at 10", "c at 10", "a at 30", "body at 40"]
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(40)
}

@test
fn timeout_gives_up_at_once() {
  (r, end) = State.run(zero(), || {
    with TestClock {
      Task.timeout(60000, || {
        (use Clock).sleep(3600000)
        1
      })
    }
  })
  assert r == None
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(60000)
}

@test
fn timeout_returns_in_time() {
  (r, end) = State.run(zero(), || {
    with TestClock {
      Task.timeout(60000, || {
        (use Clock).sleep(10)
        5
      })
    }
  })
  assert r == Some(5)
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(10)
}

@test
fn race_takes_the_earliest() {
  (r, end) = State.run(zero(), || {
    with TestClock {
      Task.race([
        || {
          (use Clock).sleep(3600000)
          1
        },
        || {
          (use Clock).sleep(10)
          2
        },
      ])
    }
  })
  assert r == 2
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(10)
}
"##,
        ),
    );
    assert_eq!(
        outcomes,
        [
            ("wakes_in_time_order".to_owned(), TestOutcome::Passed),
            ("timeout_gives_up_at_once".to_owned(), TestOutcome::Passed),
            ("timeout_returns_in_time".to_owned(), TestOutcome::Passed),
            ("race_takes_the_earliest".to_owned(), TestOutcome::Passed),
        ]
    );
    assert!(
        started.elapsed().as_secs() < 30,
        "took {:?}",
        started.elapsed()
    );
}
