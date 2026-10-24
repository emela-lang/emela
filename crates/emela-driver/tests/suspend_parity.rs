//! Suspension behaviour that the JS, C and WASM targets must agree on (spec chapter 16, 14.4,
//! 14.5): a release that fails while the run unwinds as interrupted, a release that suspends, and
//! the order releases run in when a continuation is unwound.
//!
//! Needs node, wasmtime and a C compiler (`EMELA_CC` or `cc`); the tests are skipped with a
//! message when one of them cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, CoreC, CoreJs, CoreWasm, Input, Lower, MemoryFiles, Output, RunOptions, Target,
    cc_command, node_program, run, wasmtime_program,
};

type Outcome = (i32, String, String);

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "emela-driver-suspend-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn have(program: &Path) -> bool {
    Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn messages(analysis: &Analysis) -> Vec<String> {
    analysis
        .diagnostics
        .iter()
        .map(|d| format!("{:?} {}", d.code, d.message))
        .collect()
}

/// Runs `src` on one target.
fn run_on(name: &str, src: &str, target: Target) -> Result<Outcome, Vec<String>> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", src)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: out_dir(&format!("{name}-{target}")),
        node: match target {
            Target::Wasm => wasmtime_program(),
            _ => node_program(),
        },
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, output) = match target {
        Target::Js => run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_, _| {}),
        Target::Wasm => run(
            &fs,
            &input,
            &mut frontend,
            &mut CoreWasm,
            &options,
            |_, _| {},
        ),
        Target::C => {
            let (cc, flags) = cc_command();
            let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
            let mut backend = CoreC::with_cc(cc, &flags);
            run(
                &fs,
                &input,
                &mut frontend,
                &mut backend,
                &options,
                |_, _| {},
            )
        }
    };
    let _ = std::fs::remove_dir_all(&options.out_dir);
    match output {
        Some(out) if !analysis.has_errors() => Ok((
            out.code,
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )),
        _ => Err(messages(&analysis)),
    }
}

/// Runs `src` on all three targets and returns the common outcome, or `None` when a tool is
/// missing. A target that does not build fails the test.
fn compare(name: &str, src: &str) -> Option<Outcome> {
    let (cc, _) = cc_command();
    let tools = [
        (Target::Js, node_program()),
        (Target::C, cc),
        (Target::Wasm, wasmtime_program()),
    ];
    for (target, program) in &tools {
        if !have(program) {
            eprintln!(
                "skipping {name}: cannot start the {target} tool `{}`",
                program.display()
            );
            return None;
        }
    }
    let mut outcomes = Vec::new();
    for (target, _) in &tools {
        let outcome = run_on(name, src, *target).unwrap_or_else(|messages| {
            panic!("{name}: {target} did not build:\n{}", messages.join("\n"))
        });
        outcomes.push((*target, outcome));
    }
    let (first_target, first) = &outcomes[0];
    for (target, outcome) in &outcomes[1..] {
        assert_eq!(
            outcome, first,
            "{name}: {first_target} and {target} disagree:\n{first_target}: {first:?}\n\
             {target}: {outcome:?}",
        );
    }
    Some(first.clone())
}

/// A release that fails while the run unwinds as interrupted: the defect replaces the
/// interruption (spec 14.4), so the run ends as a defect, not as `interrupted`.
const RELEASE_DEFECT: &str = r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release {
    dbg("release res")
    x = 1 / 0
    dbg("never #{x}")
  }
}

fn main() -> () use { Async } {
  with Res {
    with Ignores {
      dbg("body #{(use Wait).wait()}")
    }
  }
  ()
}
"#;

/// A release that suspends and resumes in place: it was rejected when the release could not
/// suspend (spec 11.4).
const RELEASE_SUSPENDS: &str = r#"
effect Tick {
  suspend fn tick() -> Int
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(1) }
}

effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release {
    dbg("release res")
    dbg("tick #{(use Tick).tick()}")
  }
}

fn main() -> () use { Async } {
  with Ticks {
    with Res {
      with Ignores {
        dbg("body #{(use Wait).wait()}")
      }
    }
  }
  dbg("not reached")
  ()
}
"#;

/// A stalled run with a suspended operation implementation: the implementation's releases are
/// inside the caller's, so they run first (spec 14.2, 14.5).
const NESTED_RELEASES: &str = r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

effect Go {
  suspend fn go() -> Int
}

handler Slow: Go {
  suspend fn go(resume) {
    with InnerRes {
      dbg("impl body")
      (use Wait).wait()
      dbg("impl after wait")
      resume(1)
    }
  }
}

effect Name {
  fn name() -> String
}

handler OuterRes: Name {
  init { OuterRes }
  fn name() { "outer" }
  release { dbg("release outer") }
}

handler InnerRes: Name {
  init { InnerRes }
  fn name() { "inner" }
  release { dbg("release inner") }
}

fn caller() -> Int use { Go } {
  with OuterRes {
    v = (use Go).go()
    dbg("caller got #{v}")
    v
  }
}

fn main() -> () use { Async } {
  with Ignores {
    with Slow {
      dbg("main got #{caller()}")
    }
  }
  ()
}
"#;

#[test]
fn a_release_defect_replaces_an_interruption() {
    let Some((code, _stdout, stderr)) = compare("release-defect", RELEASE_DEFECT) else {
        return;
    };
    assert_eq!(code, 101, "{stderr}");
    assert!(
        stderr.contains("defect: division by zero"),
        "the release's defect must be the result: {stderr}"
    );
    assert!(
        !stderr.contains("interrupted"),
        "the interruption must not replace the release's defect: {stderr}"
    );
}

#[test]
fn a_release_that_suspends_runs() {
    let Some((code, _stdout, stderr)) = compare("release-suspends", RELEASE_SUSPENDS) else {
        return;
    };
    assert_eq!(code, 130, "{stderr}");
    assert!(
        stderr.contains("\"release res\"\n\"tick 1\"\n"),
        "the release and its in-place resume must run: {stderr}"
    );
    assert!(stderr.ends_with("interrupted\n"), "{stderr}");
    assert!(!stderr.contains("not reached"), "{stderr}");
}

#[test]
fn releases_run_from_the_inside() {
    let Some((code, _stdout, stderr)) = compare("nested-releases", NESTED_RELEASES) else {
        return;
    };
    assert_eq!(code, 130, "{stderr}");
    let inner = stderr.find("\"release inner\"").expect(&stderr);
    let outer = stderr.find("\"release outer\"").expect(&stderr);
    assert!(
        inner < outer,
        "the implementation's release is inside the caller's: {stderr}"
    );
    assert!(!stderr.contains("impl after wait"), "{stderr}");
}

/// The TestClock is virtual on every target (spec 8.8): it advances only when the run stalls, a
/// negative sleep is zero, and `now` observes the advanced time. A zero-length SystemClock sleep
/// crosses the host boundary without waiting.
const CLOCKS: &str = r#"
import Std.Clock.{Clock, TestClock, SystemClock}
import Std.Instant.{Instant}
import Std.State.{State}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, fn() {
    with TestClock {
      clock = use Clock
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(250)
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(-1)
      dbg(Instant.to_unix_milliseconds(clock.now()))
      clock.sleep(5)
      dbg(Instant.to_unix_milliseconds(clock.now()))
    }
  })
  dbg(Instant.to_unix_milliseconds(end))
  with SystemClock {
    (use Clock).sleep(0)
    dbg("real zero")
  }
}
"#;

#[test]
fn clocks_agree_on_virtual_and_real_time() {
    let Some(outcome) = compare("clocks", CLOCKS) else {
        return;
    };
    assert_eq!(
        outcome,
        (
            0,
            String::new(),
            "0\n250\n250\n255\n255\n\"real zero\"\n".to_owned()
        )
    );
}

/// An operation implementation that suspends more than once (spec 14.5 item 3): the same handler
/// implementation sleeps twice, sleeps in a `List.map` repetition, and sleeps before calling
/// another `suspend fn` operation. Each resume runs the implementation again, so the caller links
/// must not be overwritten. The virtual time is 100 + 50 + 3 * 10 + 5 = 185.
const REPEATED_SUSPENSION: &str = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Retry { suspend fn go() -> Int }
effect Ping { suspend fn ping() -> Int }

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
  dbg("end #{end |> Instant.to_unix_milliseconds}")
}
"#;

#[test]
fn an_implementation_resumes_more_than_once() {
    let Some(outcome) = compare("repeated-suspension", REPEATED_SUSPENSION) else {
        return;
    };
    assert_eq!(
        outcome,
        (0, String::new(), "\"go 1\"\n\"end 185\"\n".to_owned())
    );
}

/// The same repeated suspension on the real clock (spec 8.7). The virtual time is not asserted;
/// only that every resume runs and the implementation's result reaches the caller.
const REPEATED_SUSPENSION_REAL: &str = r#"
import Std.Clock.{Clock, SystemClock}

effect Retry { suspend fn go() -> Int }
effect Ping { suspend fn ping() -> Int }

handler Pinging: Ping {
  suspend fn ping(resume) {
    (use Clock).sleep(10)
    resume(1)
  }
}

handler Retrying: Retry {
  suspend fn go(resume) {
    clock = use Clock
    clock.sleep(10)
    clock.sleep(10)
    List.map(List.range(0, 3), fn(i) { clock.sleep(10) })
    n = (use Ping).ping()
    resume(n)
  }
}

fn main() {
  with SystemClock {
    with Pinging, Retrying {
      dbg("go #{(use Retry).go()}")
    }
  }
}
"#;

#[test]
fn an_implementation_resumes_more_than_once_on_the_real_clock() {
    let Some(outcome) = compare("repeated-suspension-real", REPEATED_SUSPENSION_REAL) else {
        return;
    };
    assert_eq!(outcome, (0, String::new(), "\"go 1\"\n".to_owned()));
}

/// A non-suspending function that suspends inside an operation implementation that already
/// resumed from a sleep: the function's continuation is unwound as interrupted at once (spec
/// 14.5), and the implementation is inside the caller's continuation, so `inner`'s release runs
/// before `main`'s (spec 14.2). The parked cascade of the sleep must not be unwound ahead of it.
const SYNC_AFTER_PARK: &str = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Outer {
  suspend fn go() -> Int
}

effect Wait {
  suspend fn wait() -> Int
}

effect Name {
  fn name() -> String
}

effect Tag {
  fn tag() -> String
}

handler Ignores: Wait {
  suspend fn wait(resume) {
    dbg("not resuming")
    ()
  }
}

handler MainRes: Name {
  init { MainRes }
  fn name() { "main" }
  release { dbg("release main") }
}

handler InnerRes: Tag {
  init { InnerRes }
  fn tag() { "inner" }
  release { dbg("release inner") }
}

fn inner() -> Int {
  with InnerRes {
    with Ignores {
      (use Wait).wait()
    }
  }
}

handler Going: Outer {
  suspend fn go(resume) {
    (use Clock).sleep(100)
    dbg("woke")
    n = inner()
    resume(n)
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with MainRes {
        with Going {
          dbg("go #{(use Outer).go()}")
        }
      }
    }
  })
  dbg("not reached")
}
"#;

#[test]
fn a_sync_suspension_after_a_park_unwinds_from_the_inside() {
    let Some(outcome) = compare("sync-after-park", SYNC_AFTER_PARK) else {
        return;
    };
    assert_eq!(
        outcome,
        (
            130,
            String::new(),
            "\"woke\"\n\"not resuming\"\n\"release inner\"\n\"release main\"\ninterrupted\n"
                .to_owned()
        )
    );
}

/// A cascade of two operations started from an implementation that resumed from a sleep: the
/// implementation calls `ping`, whose implementation sleeps and never resumes. The run stalls and
/// every waiting continuation is unwound, including `go`'s caller, so `main`'s release runs
/// (spec 14.5). `go` must stay in the chain behind the new cascade.
const CASCADE_AFTER_RESUME: &str = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Retry {
  suspend fn go() -> Int
}

effect Ping {
  suspend fn ping() -> Int
}

effect Name {
  fn name() -> String
}

handler MainRes: Name {
  init { MainRes }
  fn name() { "main" }
  release { dbg("release main") }
}

handler Dropping: Ping {
  suspend fn ping(resume) {
    (use Clock).sleep(5)
    dbg("ping not resuming")
    ()
  }
}

handler Retrying: Retry {
  suspend fn go(resume) {
    (use Clock).sleep(100)
    dbg("go woke")
    n = (use Ping).ping()
    resume(n)
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with MainRes {
        with Dropping, Retrying {
          dbg("go #{(use Retry).go()}")
        }
      }
    }
  })
  dbg("not reached")
}
"#;

#[test]
fn a_cascade_after_a_resume_keeps_the_outer_operation() {
    let Some(outcome) = compare("cascade-after-resume", CASCADE_AFTER_RESUME) else {
        return;
    };
    assert_eq!(
        outcome,
        (
            130,
            String::new(),
            "\"go woke\"\n\"ping not resuming\"\n\"release main\"\ninterrupted\n".to_owned()
        )
    );
}
