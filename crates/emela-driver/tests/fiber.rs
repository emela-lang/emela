//! Concurrency (spec chapter 11) through the `emela run` and `emela test` paths. Runs source
//! through lowering, codegen-js and node, checking the behavior of the chapter 11 examples
//! (spec 11.2, 11.3, 11.5, 11.7, 11.8). When cc starts, every program also runs on the C target
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
    let dir =
        std::env::temp_dir().join(format!("emela-driver-fiber-{}-{name}", std::process::id()));
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
        let c = run_main_on(name, main, Target::C);
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
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_, _| {}),
        Target::C => run(
            &fs,
            &input,
            &mut frontend,
            &mut c_backend().expect("checked by the caller"),
            &options,
            |_, _| {},
        ),
        Target::Wasm | Target::CEmbedded => unreachable!("the fiber tests run js and c"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert!(
        analysis.diagnostics.is_empty(),
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
        let c = run_tests_on(name, main, Target::C);
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
        node: PathBuf::from("node"),
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
        Target::C => test(
            &fs,
            &input,
            &mut frontend,
            &mut c_backend().expect("checked by the caller"),
            &options,
            |_| {},
            &mut out,
        ),
        Target::Wasm | Target::CEmbedded => unreachable!("the fiber tests run js and c"),
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(analysis.diagnostics, [], "{target}");
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
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause, CauseKind, Fail, Die, Interrupt}
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

fn say(text: String) -> () use { Console } {
  (use Console).print_line(text)
}

fn fetch(n: Int, ms: Int) -> Int use { Clock, Console } {
  (use Clock).sleep(ms)
  say("fetched #{n}")
  n * 10
}

fn boom() -> Int fails Boom { fail Boom }

fn kind_name(k: CauseKind) -> String {
  k match {
    Fail -> "Fail"
    Die -> "Die"
    Interrupt -> "Interrupt"
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
    // spec 11.2: children wait concurrently and their values are received with join. Both the
    // capability value's fork and Fiber.fork work.
    let (code, out, err) = run_main(
        "scope",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    total = Fiber.scope(fn() {
      s = use Scope
      a = s.fork(fn() { fetch(1, 60) })
      b = Fiber.fork(fn() { fetch(2, 10) })
      Fiber.join(a) + Fiber.join(b)
    })
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
    xs = Fiber.all([fn() { fetch(1, 60) }, fn() { fetch(2, 10) }])
    say("all #{xs}")
    w = Fiber.race([fn() { fetch(3, 200) }, fn() { fetch(4, 10) }])
    say("race #{w}")
    (p, q) = Fiber.both(fn() { fetch(5, 10) }, fn() { "x" })
    say("both #{p} #{q}")
    ys = Fiber.map([1, 2, 3], fn(i) { fetch(i, 100 - i * 40) })
    say("map #{ys}")
    t = Fiber.timeout(200, fn() { fetch(6, 10) })
    u = Fiber.timeout(10, fn() { fetch(7, 1000) })
    say("timeout #{t} #{u}")
    r = Fiber.race([fn() { boom() }, fn() { fetch(8, 1000) }]) escape { Boom -> -1 }
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
fn children_get_the_handlers_where_scope_is_called() {
    // spec 11.2: a child uses the handlers in effect when Fiber.scope was called. A `with` inside
    // the scope body does not reach children, but a closure created inside a `with` uses that
    // `with`'s handler.
    let (code, out, err) = run_main(
        "handlers",
        &source(
            r##"
fn main() {
  with SystemConsole, Outer {
    Fiber.scope(fn() {
      outside = fn() { say((use Name).name()) }
      with Inner {
        a = Fiber.fork(outside)
        b = Fiber.fork(fn() { say((use Name).name()) })
        say("body #{(use Name).name()}")
        Fiber.join(a)
        Fiber.join(b)
      }
    })
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str(), err.as_str()),
        (0, "body inner\nouter\ninner\n", "")
    );
}

#[test]
fn children_fork_siblings_into_their_own_scope() {
    // spec 11.2: the Scope inside a child is the scope the child belongs to. A child can fork
    // siblings, and the scope waits for them too. With nested scopes, an inner child's fork goes
    // into the inner scope, and a capability value taken outside forks into the outer scope. An
    // error from an inner child's grandchild reaches the inner scope.
    let (code, out, err) = run_main(
        "siblings",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      a = Fiber.fork(fn() {
        b = Fiber.fork(fn() { fetch(2, 10) })
        say("child")
        Fiber.join(b)
      })
      say("joined #{Fiber.join(a)}")
    })
    r = Fiber.scope(fn() {
      outer = use Scope
      late = outer.fork(fn() { fetch(9, 50) })
      inner = Fiber.scope(fn() {
        Fiber.fork(fn() {
          h = Fiber.fork(fn() { boom() })
          Fiber.join(h)
        })
        (use Clock).sleep(1000)
        0
      }) escape { Boom -> -1 }
      say("inner #{inner}")
      Fiber.join(late)
    })
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
fn a_failing_child_interrupts_its_siblings() {
    // spec 11.2, 11.4: a child's failure interrupts its siblings and the body, runs releases, then
    // becomes the scope's failure. Remaining children are interrupted even when the body returns
    // a value.
    let (code, out, err) = run_main(
        "child-fails",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    r = Fiber.scope(fn() {
      Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        boom()
      })
      (use Clock).sleep(1000)
      say("not reached")
      1
    }) escape { Boom -> -1 }
    say("scope #{r}")
    v = Fiber.scope(fn() {
      Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      (use Clock).sleep(10)
      2
    })
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
            "init res\nrelease res\nscope -1\ninit res\nrelease res\nvalue 2\n",
            ""
        )
    );
}

#[test]
fn later_failures_are_notes() {
    // spec 11.2: if two or more children fail, the first failure is the result and later ones
    // become notes.
    let (code, out, err) = run_main(
        "notes",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        panic("first")
      })
      Fiber.fork(fn() {
        with Res {
          (use Clock).sleep(1000)
        }
      })
      (use Clock).sleep(1000)
    })
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
        err.contains("note: another fiber also failed: defect: second"),
        "{err}"
    );
}

#[test]
fn attempt_and_await() {
    // The table in spec 11.5: attempt catches Fail and Die, and await catches interruption.
    // Joining an interrupted child is a defect. An error re-raised with Cause.raise can be caught
    // by escape.
    let (code, out, err) = run_main(
        "attempt-await",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    say(show(Fiber.attempt(fn() { boom() })))
    say(show(Fiber.attempt(fn() { panic("bad") })))
    say(show(Fiber.attempt(fn() { fetch(1, 1) })))
    Fiber.scope(fn() {
      h = Fiber.fork(fn() { with Res { (use Clock).sleep(1000) } })
      (use Clock).sleep(10)
      Fiber.interrupt(h)
      say("await #{show(Fiber.await(h))}")
      say("join #{show(Fiber.attempt(fn() { Fiber.join(h) }))}")
      ok = Fiber.fork(fn() { 7 })
      say("await #{show(Fiber.await(ok))}")
    })
    r = Fiber.scope(fn() {
      Fiber.attempt(fn() { boom() }) match {
        Success(v) -> v
        Failure(c) -> Cause.raise(c)
      }
    }) escape { Boom -> -2 }
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
             await Interrupt interrupted\njoin Die defect: fiber was interrupted\n\
             await Success\nraise -2\n",
            ""
        )
    );
}

#[test]
fn deferred() {
    // spec 11.7: a Deferred can be completed once, resuming every waiter. poll does not wait.
    let (code, out, err) = run_main(
        "deferred",
        &source(
            r##"
fn main() {
  with SystemConsole, SystemClock {
    Fiber.scope(fn() {
      d = Fiber.deferred()
      a = Fiber.fork(fn() { Deferred.await(d) + 1 })
      b = Fiber.fork(fn() { Deferred.await(d) + 2 })
      Fiber.fork(fn() {
        (use Clock).sleep(10)
        Deferred.complete(d, 40)
      })
      say("poll #{Deferred.poll(d)}")
      say("joined #{Fiber.join(a)} #{Fiber.join(b)}")
      say("again #{Deferred.complete(d, 1)} #{Deferred.poll(d)}")
    })
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
fn using_an_ended_scope_is_a_defect() {
    // spec 11.8: an escape the types do not stop (captured in a closure and taken outside) is a
    // defect at run time.
    let (code, out, err) = run_main(
        "ended-scope",
        &source(
            r##"
fn main() {
  with SystemConsole {
    join_later = Fiber.scope(fn() {
      h = Fiber.fork(fn() { 1 })
      fn() { Fiber.join(h) }
    })
    say(show(Fiber.attempt(fn() { join_later() })))
    complete_later = Fiber.scope(fn() {
      d = Fiber.deferred()
      fn() { Deferred.complete(d, 1) }
    })
    say(show(Fiber.attempt(fn() { complete_later() })))
    poll_later = Fiber.scope(fn() {
      d = Fiber.deferred()
      fn() { Deferred.poll(d) }
    })
    poll_later()
    ()
  }
}
"##,
        ),
    );
    assert_eq!(
        (code, out.as_str()),
        (
            101,
            "Die defect: fiber was joined after its scope ended\n\
             Die defect: deferred was completed after its scope ended\n"
        )
    );
    assert!(
        err.contains("deferred was polled after its scope ended"),
        "{err}"
    );
}

#[test]
fn fibers_in_tests() {
    // spec chapter 13: fibers also run inside test functions. A child's failure becomes that
    // test's error or defect. `child_defect` infers a type parameter (the child returns Never);
    // the C target instantiates it (with `Never`) instead of skipping the test.
    let outcomes = run_tests(
        "tests",
        &source(
            r##"
@test
fn joins() {
  with SystemConsole, SystemClock {
    xs = Fiber.all([fn() { fetch(1, 20) }, fn() { fetch(2, 10) }])
    assert xs == [10, 20]
  }
}

@test
fn races() {
  with SystemConsole, SystemClock {
    assert Fiber.race([fn() { fetch(1, 200) }, fn() { fetch(2, 10) }]) == 20
  }
}

@test
fn child_error() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { boom() })
    Fiber.join(h)
  })
}

@test
fn child_defect() {
  Fiber.scope(fn() {
    h = Fiber.fork(fn() { panic("child") })
    Fiber.join(h)
  })
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
    // spec 8.8: TestClock runs on virtual time. When no fiber can make progress, it advances to
    // the earliest scheduled time and wakes sleepers at the same time in the order sleep was
    // called. Fiber.timeout and race with a one-hour sleep finish without really waiting
    // (spec 11.3).
    let started = std::time::Instant::now();
    let outcomes = run_tests(
        "virtual-time",
        &format!(
            "{}{PRELUDE}{}",
            "import Std.Clock.{TestClock}\nimport Std.Console.{TestConsole}\n\
             import Std.State.{State}\nimport Std.Instant.{Instant}\n",
            r##"
fn at(text: String) -> () use { Clock, Console } {
  ms = Instant.to_unix_milliseconds((use Clock).now())
  say("#{text} at #{ms}")
}

fn zero() -> Instant {
  Instant.from_unix_milliseconds(Int64.from_int(0))
}

@test
fn wakes_in_time_order() {
  ((_, lines), end) = State.run(zero(), fn() {
    State.run([], fn() {
      with TestConsole, TestClock {
        Fiber.scope(fn() {
          Fiber.fork(fn() {
            (use Clock).sleep(30)
            at("a")
          })
          Fiber.fork(fn() {
            (use Clock).sleep(10)
            at("b")
          })
          Fiber.fork(fn() {
            (use Clock).sleep(10)
            at("c")
          })
          (use Clock).sleep(40)
          at("body")
        })
      }
    })
  })
  assert lines == ["b at 10", "c at 10", "a at 30", "body at 40"]
  assert Instant.to_unix_milliseconds(end) == Int64.from_int(40)
}

@test
fn timeout_gives_up_at_once() {
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.timeout(60000, fn() {
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
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.timeout(60000, fn() {
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
  (r, end) = State.run(zero(), fn() {
    with TestClock {
      Fiber.race([
        fn() {
          (use Clock).sleep(3600000)
          1
        },
        fn() {
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
