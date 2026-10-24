//! The C target (spec chapter 16) through the `emela build`, `run` and `test` paths: the output
//! is compiled with cc (`EMELA_CC` or `cc`) and run. Tests that need cc are skipped, with a
//! message, when it cannot start.

use std::path::{Path, PathBuf};
use std::process::Command;

use emela_driver::{
    Analysis, C_EXECUTABLE, CoreC, CoreJs, Input, Lower, LowerTests, MemoryFiles, OsFs, Output,
    RunOptions, RunOutput, Target, TestOptions, TestOutcome, build, cc_command, project_target,
    run, test,
};

/// Whether the C compiler starts; prints why a test is skipped otherwise.
fn have_cc() -> bool {
    let (cc, flags) = cc_command();
    let ok = Command::new(&cc)
        .args(&flags)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipping: cannot start the C compiler `{}`", cc.display());
    }
    ok
}

/// A per-test output directory. Leftovers from earlier runs are removed.
fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-c-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn project(main: &str) -> (MemoryFiles, Input) {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    (fs, input)
}

/// Builds and runs `main` on the C target.
fn run_c(name: &str, main: &str, backend: &mut CoreC) -> (Analysis, Option<RunOutput>) {
    let (fs, input) = project(main);
    let options = RunOptions {
        out_dir: out_dir(name),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let result = run(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::C),
        backend,
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

fn codes(analysis: &Analysis) -> Vec<(Option<&str>, &str)> {
    analysis
        .diagnostics
        .iter()
        .map(|d| (d.code, d.message.as_str()))
        .collect()
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

#[test]
fn platform_handlers_run_natively() {
    if !have_cc() {
        return;
    }
    let main = r##"import Std.Console.{Console, SystemConsole}
import Std.Env.{Env, SystemEnv}
import Std.Random.{Random, SystemRandom}

error Bad(n: Int) derive Show

fn check(n: Int) -> Int fails Bad {
  if n > 1 { fail Bad(n:) }
  n
}

fn main() {
  with SystemConsole, SystemEnv, SystemRandom {
    console = use Console
    console.print_line("hi from c")
    x = (use Random).float()
    console.print_line("#{x >= 0.0 && x < 1.0}")
    console.print_line("#{(use Env).get("EMELA_SURELY_UNSET_VARIABLE")}")
    console.print_error_line("to stderr")
    console.print_line("#{check(1)}")
    console.print_line("#{check(2)}")
  }
}
"##;
    let (analysis, output) = run_c("platform", main, &mut CoreC::default());
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "hi from c\nTrue\nNone\n1\n");
    assert_eq!(text(&output.stderr), "to stderr\nerror: Bad(n: 2)\n");
    assert_eq!(output.code, 1);
}

#[test]
fn build_writes_the_executable() {
    if !have_cc() {
        return;
    }
    let (fs, input) = project("fn main() { dbg(1 + 1) }\n");
    let dir = out_dir("build");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::C),
        &mut CoreC::default(),
        &dir,
    );
    assert_eq!(codes(&analysis), []);
    let entry = entry.unwrap();
    assert_eq!(entry, dir.join(C_EXECUTABLE));
    for file in ["main.c", "emela_runtime.c", "emela_runtime.h"] {
        assert!(dir.join(file).exists(), "{file}");
    }
    let out = Command::new(&entry).output().unwrap();
    assert_eq!(text(&out.stderr), "2\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_c_external_is_e0412() {
    let main = r##"@external(js, "Date.now")
fn now() -> Float

handler Clock: Tick {
  fn tick() { now() }
}

effect Tick {
  fn tick() -> Float
}

fn main() {
  with Clock { (use Tick).tick() }
}
"##;
    let (analysis, output) = run_c("missing", main, &mut CoreC::default());
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0412"),
            "external function `now` has no `@external` for target `c`"
        )]
    );
}

#[test]
fn callbacks_are_rejected() {
    let main = r##"@external(c, "take")
fn take(f: fn(Int) -> Int) -> () use {}

fn main() { take(fn(x) { x }) }
"##;
    let (analysis, _) = run_c("callback", main, &mut CoreC::default());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0427"),
            "external function `take` takes a callback, which target `c` does not support yet"
        )]
    );
}

#[test]
fn cc_problems_are_e0907_and_e0908() {
    let main = "fn main() { dbg(1) }\n";
    let (analysis, output) = run_c(
        "no-cc",
        main,
        &mut CoreC::with_cc("/nonexistent/emela-test-cc", &[]),
    );
    assert!(output.is_none());
    let found = codes(&analysis);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some("E0907"));
    assert!(
        found[0]
            .1
            .starts_with("cannot start `/nonexistent/emela-test-cc`")
    );

    // A compiler that fails.
    let (analysis, output) = run_c("bad-cc", main, &mut CoreC::with_cc("false", &[]));
    assert!(output.is_none());
    let found = codes(&analysis);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, Some("E0908"));
    assert!(found[0].1.starts_with("`false` failed to compile `"));
}

#[test]
fn tests_run_natively() {
    if !have_cc() {
        return;
    }
    // An error without Show is reported by its name (spec 13.3, 14.3).
    let main = r##"error Nope(why: String)

fn half(n: Int) -> Int fails Nope {
  if n % 2 != 0 { fail Nope(why: "odd") }
  n / 2
}

@test
fn passes() {
  assert half(4) == 2
}

@test
fn fails_with_an_error() {
  assert half(3) == 1
}

@test
fn raises_a_defect() {
  assert 1 + 1 == 3
}
"##;
    let (fs, input) = project(main);
    let options = TestOptions {
        out_dir: out_dir("tests"),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default().with_target(Target::C),
        &mut CoreC::default(),
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(codes(&analysis), []);
    let results: Vec<(String, TestOutcome)> = report
        .unwrap()
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect();
    assert_eq!(
        results,
        [
            ("passes".to_owned(), TestOutcome::Passed),
            (
                "fails_with_an_error".to_owned(),
                TestOutcome::Error("Nope".to_owned())
            ),
            (
                "raises_a_defect".to_owned(),
                TestOutcome::Defect(
                    "assertion failed: 1 + 1 == 3\n  left: 2\n right: 3".to_owned()
                )
            ),
        ]
    );
}

/// A failed comparison shows both sides with `inspect` (spec 13.2, 10.6), values of a type
/// parameter too, the same on JS and C. Sides whose type does not satisfy Show are not shown.
#[test]
fn assert_shows_both_sides_with_inspect() {
    if !have_cc() {
        return;
    }
    let main = r##"type Button(label: String, on_click: fn(Int) -> Int)

type Hidden(n: Int) derive Eq

fn twice(x: Int) -> Int { x * 2 }

fn same[A: Eq](a: A, b: A) -> () { assert a == b }

@test
fn generic() {
  same([Some("a")], [None])
}

@test
fn field_of_a_record() {
  assert Button(label: "a", on_click: twice).label == "b"
}

@test
fn without_show() {
  same(Hidden(n: 1), Hidden(n: 2))
}
"##;
    let (fs, input) = project(main);
    let expected = [
        (
            "generic".to_owned(),
            TestOutcome::Defect(
                "assertion failed: a == b\n  left: [Some(\"a\")]\n right: [None]".to_owned(),
            ),
        ),
        (
            "field_of_a_record".to_owned(),
            TestOutcome::Defect(
                "assertion failed: Button(label: \"a\", on_click: twice).label == \"b\"\n  \
                 left: \"a\"\n right: \"b\""
                    .to_owned(),
            ),
        ),
        (
            "without_show".to_owned(),
            TestOutcome::Defect("assertion failed: a == b".to_owned()),
        ),
    ];
    for target in [Target::Js, Target::C] {
        let options = TestOptions {
            out_dir: out_dir(&format!("assert-inspect-{target}")),
            node: emela_driver::node_program(),
            filter: None,
            output: Output::Capture,
            reporter: emela_driver::Reporter::Plain,
            color: false,
        };
        let mut frontend = LowerTests::default().with_target(target);
        let mut out = Vec::new();
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
            _ => test(
                &fs,
                &input,
                &mut frontend,
                &mut CoreC::default(),
                &options,
                |_| {},
                &mut out,
            ),
        };
        let _ = std::fs::remove_dir_all(&options.out_dir);
        assert_eq!(codes(&analysis), [], "{target}");
        let results: Vec<(String, TestOutcome)> = report
            .unwrap()
            .results
            .into_iter()
            .map(|r| (r.name, r.outcome))
            .collect();
        assert_eq!(results, expected, "{target}");
    }
}

#[test]
fn build_target_from_pome_toml() {
    let read = |toml: &str| {
        let fs = MemoryFiles::new([("app/Pome.toml", toml), ("app/src/main.emel", "")]);
        let input = Input::resolve(&fs, Path::new("app")).unwrap();
        project_target(&fs, &input).map_err(|d| (d.code, d.message))
    };
    assert_eq!(read(""), Ok(None));
    assert_eq!(read("[build]\ntarget = \"c\"\n"), Ok(Some(Target::C)));
    assert_eq!(
        read("[build]\ntarget = \"node\"\n"),
        Err((
            Some("E0232"),
            "unknown target `node` in `[build].target`".to_owned()
        ))
    );
}

/// The host side of [`HOST_EMEL`]: `suspend fn` external functions written against `emela.h`
/// (spec 12.6).
const HOST_C: &str = r#"#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "emela.h"

static void doubled(em_task *task, void *ctx) { em_complete_int(task, (int32_t)(intptr_t)ctx * 2); }

static void cancelled(em_task *task, void *ctx) {
  printf("cancelled %d\n", (int)(intptr_t)ctx);
  em_complete_int(task, 0);
}

/* Completes with n * 2 after ms milliseconds; says so when the fiber is interrupted. */
void host_double(int32_t n, int32_t ms, em_task *task) {
  em_after(ms, task, doubled, (void *)(intptr_t)n);
  em_on_cancel(task, cancelled, (void *)(intptr_t)n);
}

/* Completes in place. */
void host_greet(const char *name, em_task *task) {
  size_t n = strlen(name) + 8;
  char *s = malloc(n);
  snprintf(s, n, "hello, %s", name);
  em_complete_string(task, s);
}

typedef struct job {
  em_task *task;
  int32_t n;
} job;

static void *work(void *arg) {
  job *j = arg;
  struct timespec ts = {0, 20 * 1000000};
  nanosleep(&ts, NULL);
  em_complete_int(j->task, j->n + 1);
  free(j);
  return NULL;
}

static void never(em_task *task, void *ctx) {
  (void)task;
  (void)ctx;
}

/* Completed from another thread. The long timer keeps the event loop waiting until then;
 * completing the task removes it. */
void host_threaded(int32_t n, em_task *task) {
  em_after(60000, task, never, NULL);
  job *j = malloc(sizeof *j);
  j->task = task;
  j->n = n;
  pthread_t thread;
  pthread_create(&thread, NULL, work, j);
  pthread_detach(thread);
}

void host_twice(em_task *task) {
  em_complete_int(task, 1);
  em_complete_int(task, 2);
}

static void twice_later(em_task *task, void *ctx) {
  (void)ctx;
  em_complete_int(task, 1);
  em_complete_int(task, 2);
}

void host_twice_later(em_task *task) { em_after(1, task, twice_later, NULL); }

void host_broken(em_task *task) { em_complete_defect(task, "disk on fire"); }

static void forgotten(em_task *task, void *ctx) {
  (void)ctx;
  printf("cancelled forgotten\n");
  em_complete_unit(task);
}

/* Registers nothing with the event loop and does not complete: the run stalls. */
void host_forgotten(em_task *task) { em_on_cancel(task, forgotten, NULL); }

/* Late second completions. `host_keep` completes in a callback and keeps the task, which the
 * runtime frees once it is back in its event loop. */
static em_task *kept = NULL;

static void keep_and_complete(em_task *task, void *ctx) {
  (void)ctx;
  kept = task;
  em_complete_int(task, 1);
}

void host_keep(em_task *task) { em_after(1, task, keep_and_complete, NULL); }

/* Completes the kept task again from a callback, turns of the event loop later. */
static void complete_kept(em_task *task, void *ctx) {
  (void)ctx;
  em_complete_int(kept, 2);
  em_complete_int(task, 0);
}

void host_again_later(em_task *task) { em_after(1, task, complete_kept, NULL); }

static void *complete_kept_on_thread(void *task) {
  struct timespec ts = {0, 20 * 1000000};
  nanosleep(&ts, NULL);
  em_complete_int(kept, 2);
  em_complete_int(task, 0);
  return NULL;
}

/* The same from another thread. */
void host_again_threaded(em_task *task) {
  em_after(60000, task, never, NULL);
  pthread_t thread;
  pthread_create(&thread, NULL, complete_kept_on_thread, task);
  pthread_detach(thread);
}
"#;

const HOST_EMEL: &str = r##"import Std.Console.{Console, SystemConsole}
import Std.Clock.{SystemClock}
import Std.Fiber.{Fiber}

@external(c, "./host.c", "host_double")
suspend fn host_double(n: Int, ms: Int) -> Int

@external(c, "./host.c", "host_greet")
suspend fn host_greet(name: String) -> String

@external(c, "./host.c", "host_threaded")
suspend fn host_threaded(n: Int) -> Int

@external(c, "./host.c", "host_twice")
suspend fn host_twice() -> Int

@external(c, "./host.c", "host_twice_later")
suspend fn host_twice_later() -> Int

@external(c, "./host.c", "host_broken")
suspend fn host_broken() -> Int

@external(c, "./host.c", "host_forgotten")
suspend fn host_forgotten() -> ()

@external(c, "./host.c", "host_keep")
suspend fn host_keep() -> Int

@external(c, "./host.c", "host_again_later")
suspend fn host_again_later() -> Int

@external(c, "./host.c", "host_again_threaded")
suspend fn host_again_threaded() -> Int

effect Host {
  suspend fn double(n: Int, ms: Int) -> Int
  suspend fn greet(name: String) -> String
  suspend fn threaded(n: Int) -> Int
  suspend fn twice() -> Int
  suspend fn twice_later() -> Int
  suspend fn broken() -> Int
  suspend fn forgotten() -> ()
  suspend fn keep() -> Int
  suspend fn again_later() -> Int
  suspend fn again_threaded() -> Int
}

handler CHost: Host {
  suspend fn double(n, ms, resume) { resume(host_double(n, ms)) }
  suspend fn greet(name, resume) { resume(host_greet(name)) }
  suspend fn threaded(n, resume) { resume(host_threaded(n)) }
  suspend fn twice(resume) { resume(host_twice()) }
  suspend fn twice_later(resume) { resume(host_twice_later()) }
  suspend fn broken(resume) { resume(host_broken()) }
  suspend fn forgotten(resume) { resume(host_forgotten()) }
  suspend fn keep(resume) { resume(host_keep()) }
  suspend fn again_later(resume) { resume(host_again_later()) }
  suspend fn again_threaded(resume) { resume(host_again_threaded()) }
}

fn main() {
  with SystemConsole, SystemClock, CHost {
    console = use Console
    host = use Host
    console.print_line(host.greet("c"))
    console.print_line("#{host.double(21, 10)}")
    console.print_line("#{host.threaded(41)}")
    console.print_line("#{Fiber.timeout(20, fn() { host.double(5, 60000) })}")
    console.print_line("#{Fiber.all([fn() { host.double(1, 30) }, fn() { host.double(2, 10) }])}")
    console.print_line("#{Fiber.race([fn() { host.double(3, 60000) }, fn() { host.double(4, 10) }])}")
  }
}

@test
fn completed_twice() {
  with CHost { (use Host).twice() }
}

@test
fn completed_twice_later() {
  with CHost {
    host = use Host
    host.double(host.twice_later(), 60000)
  }
}

@test
fn host_defect() {
  with CHost { (use Host).broken() }
}

@test
fn abandoned() {
  with CHost { (use Host).forgotten() }
}

@test
fn completed_again_from_a_callback() {
  with CHost {
    host = use Host
    assert host.keep() == 1
    host.again_later()
  }
}

@test
fn completed_again_from_a_thread() {
  with CHost {
    host = use Host
    assert host.keep() == 1
    host.again_threaded()
  }
}

@test
fn after_the_others() {
  with CHost { assert (use Host).double(2, 1) == 4 }
}
"##;

/// A project on disk (`@external(c, "./host.c", ...)` points to a real file) with [`HOST_EMEL`]
/// and [`HOST_C`].
fn host_project(name: &str) -> (PathBuf, Input) {
    let dir = out_dir(&format!("{name}-project"));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Pome.toml"), "").unwrap();
    std::fs::write(dir.join("src/main.emel"), HOST_EMEL).unwrap();
    std::fs::write(dir.join("src/host.c"), HOST_C).unwrap();
    let input = Input::resolve(&OsFs, &dir).unwrap();
    (dir, input)
}

/// The C backend with the runtime's leak check compiled in.
fn leak_checked() -> CoreC {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    CoreC::with_cc(cc, &flags)
}

/// [`leak_checked`] with UBSan too, when cc supports it (ASan hangs on some machines).
fn sanitized() -> CoreC {
    let (cc, mut flags) = cc_command();
    flags.push("-DEMELA_LEAK_CHECK".to_owned());
    let dir = out_dir("ubsan-probe");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("probe.c"), "int main(void) { return 0; }\n").unwrap();
    let ubsan = ["-fsanitize=undefined", "-fno-sanitize-recover=all"];
    let ok = Command::new(&cc)
        .args(&flags)
        .args(ubsan)
        .arg(dir.join("probe.c"))
        .arg("-o")
        .arg(dir.join("probe"))
        .output()
        .is_ok_and(|o| o.status.success());
    let _ = std::fs::remove_dir_all(&dir);
    if ok {
        flags.extend(ubsan.map(str::to_owned));
    }
    let flags: Vec<&str> = flags.iter().map(String::as_str).collect();
    CoreC::with_cc(cc, &flags)
}

#[test]
fn suspend_externals_complete_later() {
    if !have_cc() {
        return;
    }
    let (dir, input) = host_project("host-run");
    let options = RunOptions {
        out_dir: dir.join("out"),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let started = std::time::Instant::now();
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut Lower::default().with_target(Target::C),
        &mut leak_checked(),
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(text(&output.stderr), "");
    assert_eq!(
        text(&output.stdout),
        "hello, c\n42\n42\ncancelled 5\nNone\n[2, 4]\ncancelled 3\n8\n"
    );
    assert_eq!(output.code, 0);
    // The one-minute timers were removed: by the completion from the thread, by interruption.
    assert!(started.elapsed().as_secs() < 30, "{:?}", started.elapsed());
}

#[test]
fn suspend_externals_misbehaving() {
    if !have_cc() {
        return;
    }
    let (dir, input) = host_project("host-test");
    let options = TestOptions {
        out_dir: dir.join("out"),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &OsFs,
        &input,
        &mut LowerTests::default().with_target(Target::C),
        &mut sanitized(),
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(codes(&analysis), []);
    let results: Vec<(String, TestOutcome)> = report
        .unwrap()
        .results
        .into_iter()
        .map(|r| (r.name, r.outcome))
        .collect();
    let twice = |f: &str| format!("the task of external function `{f}` was completed twice");
    assert_eq!(
        results,
        [
            (
                "completed_twice".to_owned(),
                TestOutcome::Defect(twice("host_twice"))
            ),
            (
                "completed_twice_later".to_owned(),
                TestOutcome::Defect(twice("host_twice_later"))
            ),
            (
                "host_defect".to_owned(),
                TestOutcome::Defect("disk on fire".to_owned())
            ),
            ("abandoned".to_owned(), TestOutcome::Interrupted),
            // The kept task was freed before the second completion (spec 12.6: exactly once).
            (
                "completed_again_from_a_callback".to_owned(),
                TestOutcome::Defect(twice("host_keep"))
            ),
            (
                "completed_again_from_a_thread".to_owned(),
                TestOutcome::Defect(twice("host_keep"))
            ),
            ("after_the_others".to_owned(), TestOutcome::Passed),
        ]
    );
}
