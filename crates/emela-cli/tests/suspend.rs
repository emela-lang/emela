//! Integration tests for suspend and Async (spec 8.4, 8.7, 14.5) through `emela run` and
//! `emela test`.
//!
//! The JsClock of spec 8.4 (a `suspend fn sleep` resumed later by `setTimeout`) is written in an
//! `.mjs` next to the source. The same program also uses InstantClock, which resumes immediately.

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str, main: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("emela-cli-suspend-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = [
            ("Pome.toml", ""),
            ("src/clock.mjs", CLOCK_MJS),
            ("src/main.emel", &format!("{IMPORTS}\n{CLOCK}\n{main}")),
        ];
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project { dir }
    }

    /// Runs emela. For `run`, the line naming the command that started the program is left out of
    /// stderr, which then holds only the program's own output.
    fn emela(&self, args: &[&str]) -> (Option<i32>, String, String) {
        // `emela test` prints in the `plain` format outside a coding agent ([cmd.test.reporter]).
        let mut command = Command::new(env!("CARGO_BIN_EXE_emela"));
        for var in emela_driver::AGENT_ENV_VARS {
            command.env_remove(var);
        }
        let output = command
            .args(args)
            .current_dir(&self.dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        let stderr = if args.first() == Some(&"run") {
            let mut found = false;
            stderr
                .split_inclusive('\n')
                .filter(|line| {
                    let command = !found && line.starts_with("running `");
                    found |= command;
                    !command
                })
                .collect()
        } else {
            stderr
        };
        (
            output.status.code(),
            String::from_utf8(output.stdout).unwrap(),
            stderr,
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The bundled Console that the programs print with (spec 15.2).
const IMPORTS: &str = "import Std.Console.{Console, SystemConsole}\n";

const CLOCK_MJS: &str = "export function after(ms, f) { setTimeout(f, ms); }\n";

/// The JsClock of spec 8.4, and InstantClock, which resumes immediately.
const CLOCK: &str = r#"effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

@external(js, "./clock.mjs", "after")
fn after(ms: Int, f: fn() -> ()) -> ()

handler JsClock: Clock {
  suspend fn sleep(ms, resume) {
    after(ms, fn() { resume(ms) })
  }
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(0) }
}
"#;

#[test]
fn clocks_resume_later_and_in_place() {
    // JsClock's sleep resumes after setTimeout; InstantClock never suspends. List.map calls its
    // suspending copy when given a suspending lambda, and the original otherwise.
    let project = Project::new(
        "clocks",
        r#"fn total(xs: List[Int]) -> Int use Clock {
  clock = use Clock
  List.fold(List.map(xs, fn(ms) { clock.sleep(ms) }), 0, fn(a, b) { a + b })
}

fn main() {
  with SystemConsole {
    console = use Console
    with JsClock {
      console.print_line("start")
      slept = (use Clock).sleep(20)
      console.print_line("slept #{slept}")
      console.print_line("total #{total([3, 1, 2])}")
    }
    with InstantClock {
      console.print_line("instant #{total([5, 5])}")
    }
    console.print_line("doubled #{List.map([1, 2], fn(n) { n * 2 })}")
  }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (
            Some(0),
            "start\nslept 20\ntotal 6\ninstant 0\ndoubled [2, 4]\n",
            ""
        )
    );
    let js = std::fs::read_to_string(project.dir.join("target/emela/js/main.mjs")).unwrap();
    assert!(js.contains("function* main() {"), "{js}");
    assert!(js.contains("function* List_map$suspend"), "{js}");
    // Monomorphized: the copy for `List.map[Int, Int, {}, {}]`.
    assert!(js.contains("function List_map$Int_Int("), "{js}");
}

#[test]
fn resuming_twice_is_a_defect() {
    let project = Project::new(
        "twice",
        r#"handler TwiceClock: Clock {
  suspend fn sleep(ms, resume) {
    resume(1)
    resume(2)
  }
}

fn main() {
  with TwiceClock { (use Clock).sleep(1) }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (
            Some(101),
            "",
            "defect: resume called twice in operation `sleep` of handler `TwiceClock`\n"
        )
    );
}

#[test]
fn dropped_resume_interrupts_main() {
    let project = Project::new(
        "dropped",
        r#"handler NeverClock: Clock {
  suspend fn sleep(ms, resume) { () }
}

fn main() {
  with SystemConsole {
    console = use Console
    with NeverClock {
      console.print_line("before")
      (use Clock).sleep(1)
      console.print_line("never")
    }
  }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (Some(130), "before\n", "interrupted\n")
    );
}

#[test]
fn tests_wait_for_clocks() {
    // The test runner provides Async in a test's R (spec 13.3). A test that is never resumed fails.
    let project = Project::new(
        "test",
        r#"handler NeverClock: Clock {
  suspend fn sleep(ms, resume) { () }
}

@test
fn sleeps() {
  with JsClock { assert (use Clock).sleep(5) == 5 }
}

@test
fn instant() {
  with InstantClock { assert (use Clock).sleep(5) == 0 }
}

@test
fn never_resumed() {
  with NeverClock { (use Clock).sleep(1) }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["test"]);
    assert_eq!(code, Some(1), "{stdout}{stderr}");
    assert!(
        stdout.contains("test sleeps ... ok\ntest instant ... ok\ntest never_resumed ... FAILED\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("---- never_resumed ----\ninterrupted\n"),
        "{stdout}"
    );
}

/// An fn-operation implementation that only calls fn operations does not suspend (spec 8.1,
/// E0417). The inner InstantTimer's `now` returns the outer JsTimer's `now`, and `now` does not
/// become a generator.
#[test]
fn fn_op_calls_outer_fn_op() {
    let project = Project::new(
        "fn-op",
        r#"effect Timer {
  suspend fn wait(ms: Int) -> Int
  fn now() -> Int
}

handler JsTimer: Timer {
  suspend fn wait(ms, resume) {
    after(ms, fn() { resume(ms) })
  }
  fn now() { 42 }
}

handler InstantTimer: Timer {
  suspend fn wait(ms, resume) { resume(0) }
  fn now() { (use Timer).now() + 1 }
}

handler PureTimer: Timer {
  suspend fn wait(ms, resume) { resume(ms * 2) }
  fn now() { 0 }
}

handler LocalWait: Timer {
  suspend fn wait(ms, resume) { resume(0) }
  fn now() {
    with PureTimer { (use Timer).wait(7) }
  }
}

fn main() {
  with SystemConsole {
    console = use Console
    with JsTimer {
      waited = (use Timer).wait(5)
      with InstantTimer {
        console.print_line("now #{(use Timer).now()} after #{waited}")
      }
      with LocalWait {
        console.print_line("local #{(use Timer).now()}")
      }
    }
  }
}
"#,
    );
    let (code, stdout, stderr) = project.emela(&["run"]);
    assert_eq!(
        (code, stdout.as_str(), stderr.as_str()),
        (Some(0), "now 43 after 5\nlocal 14\n", "")
    );
    let js = std::fs::read_to_string(project.dir.join("target/emela/js/main.mjs")).unwrap();
    assert!(!js.contains("now: (function*"), "{js}");
    assert!(js.contains("now: (() => (((ev$"), "{js}");
}
