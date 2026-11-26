//! The native target (`[target.native]`) through the `emela build`, `run` and `test` paths: the
//! output is compiled with the C compiler (`EMELA_CC` or `cc`) and run. Tests that need cc are skipped, with a
//! message, when it cannot start.

use std::os::unix::fs::PermissionsExt;
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

/// Builds and runs `main` on the native target.
fn run_c(name: &str, main: &str, backend: &mut CoreC) -> (Analysis, Option<RunOutput>) {
    let (fs, input) = project(main);
    let options = RunOptions {
        out_dir: out_dir(name),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let result = run(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Native),
        backend,
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    result
}

/// The diagnostics but W0403: the programs still write `use E` as an expression (deprecated).
fn codes(analysis: &Analysis) -> Vec<(Option<&str>, &str)> {
    analysis
        .diagnostics
        .iter()
        .filter(|d| d.code != Some(emela_driver::code::DEPRECATED_USE))
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

/// The optimisation switch reaches the LIR passes (`CoreC::with_optimize`): with them off, the
/// program prints the same (T01).
#[test]
fn without_optimisation_runs_the_same_program() {
    if !have_cc() {
        return;
    }
    let main = "fn main() {\n  dbg(1 + 1)\n  ()\n}\n";
    let (analysis, output) = run_c("no-opt", main, &mut CoreC::default().with_optimize(false));
    assert_eq!(codes(&analysis), []);
    assert_eq!(text(&output.unwrap().stderr), "2\n");
}

#[test]
fn build_writes_the_executable() {
    if !have_cc() {
        return;
    }
    let (fs, input) = project("fn main() {\n  dbg(1 + 1)\n  ()\n}\n");
    let dir = out_dir("build");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Native),
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
  ()
}
"##;
    let (analysis, output) = run_c("missing", main, &mut CoreC::default());
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0412"),
            "external function `now` has no `@external(c, ...)` for target `native`"
        )]
    );
}

/// `Process` bundles `SystemTcp`, whose init and release are reached by every `with Process`
/// (`[ext.decl]`): they have a C binding even though the operations do not yet.
#[test]
fn process_with_system_tcp_runs_natively() {
    if !have_cc() {
        return;
    }
    let main = r##"import Std.Console.{Console}
import Std.Process.{Process}

fn main() {
  with Process {
    console <- Console
    console.print_line("ok")
  }
}
"##;
    let (analysis, output) = run_c("process-tcp", main, &mut CoreC::default());
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(output.code, 0);
    assert_eq!(text(&output.stdout), "ok\n");
}

/// The operations of `SystemTcp` have no C binding yet (`[api.std.tcp]`): calling one is E0412.
#[test]
fn tcp_operations_are_e0412_natively() {
    let main = r##"import Std.Tcp.{Tcp, SystemTcp}

fn main() {
  with SystemTcp {
    tcp <- Tcp
    tcp.close(tcp.connect("localhost", 80))
  } escape {
    _ -> ()
  }
}
"##;
    let (analysis, output) = run_c("tcp-e0412", main, &mut CoreC::default());
    assert!(output.is_none());
    let found = codes(&analysis);
    assert!(
        found.contains(&(
            Some("E0412"),
            "external function `host_connect` has no `@external(c, ...)` for target `native`"
        )),
        "{found:?}"
    );
}

#[test]
fn callbacks_are_rejected() {
    let main = r##"@external(c, "take")
fn take(f: fn(Int) -> Int) -> () uses {}

fn main() { take(|x| x) }
"##;
    let (analysis, _) = run_c("callback", main, &mut CoreC::default());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0427"),
            "external function `take` takes a callback, which target `native` does not support yet"
        )]
    );
}

/// An exported fn with a callback is not supported on C yet; it is not E0427, which is about
/// external functions (`[target.native.export]`).
#[test]
fn exported_callbacks_are_rejected() {
    let main = r##"@export(c, "apply")
fn apply(x: Int, f: fn(Int) -> Int fails {} uses {}) -> Int { f(x) }

fn main() {}
"##;
    let (analysis, _) = run_c("export-callback", main, &mut CoreC::default());
    assert_eq!(
        codes(&analysis),
        [(
            None,
            "exported function `apply` takes a callback, which target `native` does not support yet"
        )]
    );
}

/// `[ext.boundary]`: the native target does not support Array at the boundary yet either. An external
/// function reachable from the entry, or an exported fn, that takes or returns a type that is an
/// Array (of any element type, Bytes and nested arrays included) is E0443.
#[test]
fn arrays_at_the_boundary_are_e0443() {
    let main = r##"import Std.Array.{Array}
import Std.Bytes.{Bytes}

@external(c, "rows")
fn rows() -> Array[String] uses {}

@external(c, "sum")
fn sum(xs: Array[Int]) -> Int uses {}

@external(c, "chunks")
fn chunks(xs: Array[Array[Bytes]]) -> () uses {}

fn main() {
  dbg(sum(Array.from_list([1])))
  dbg(rows())
  chunks(Array.empty())
}
"##;
    let (analysis, output) = run_c("array-boundary", main, &mut CoreC::default());
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [
            (
                Some("E0443"),
                "external function `sum` takes `Array[Int]`, which target `native` does not support yet"
            ),
            (
                Some("E0443"),
                "external function `rows` returns `Array[String]`, which target `native` does not support yet"
            ),
            (
                Some("E0443"),
                "external function `chunks` takes `Array[Array[Bytes]]`, which target `native` does not support yet"
            ),
        ]
    );
    let main = r##"import Std.Array.{Array}

@export(c, "size")
fn size(a: Array[Int]) -> Int { Array.size(a) }

@export(c, "names")
fn names() -> Array[String] { Array.empty() }

fn main() {}
"##;
    let (analysis, output) = run_c("array-export", main, &mut CoreC::default());
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [
            (
                Some("E0443"),
                "exported function `size` takes `Array[Int]`, which target `native` does not support yet"
            ),
            (
                Some("E0443"),
                "exported function `names` returns `Array[String]`, which target `native` does not support yet"
            ),
        ]
    );
}

#[test]
fn cc_problems_are_e0907_and_e0908() {
    let main = "fn main() {\n  dbg(1)\n  ()\n}\n";
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
    // `[target.native.cc]`: the help when it is not found.
    assert_eq!(
        analysis.diagnostics[0].notes,
        [
            "help: install a C11 compiler and add it to PATH as `cc`, or set EMELA_CC to the compiler"
        ]
    );

    // A compiler that fails. The message names no emitted file (`[target.native.cc]`).
    let (analysis, output) = run_c("bad-cc", main, &mut CoreC::with_cc("false", &[]));
    assert!(output.is_none());
    let found = codes(&analysis);
    assert_eq!(
        found,
        [(
            Some("E0908"),
            "`false` failed to compile the output of target `native`"
        )]
    );
}

/// `[target.native.cc]`: the whole value of `EMELA_CC` names one program, so a value with flags
/// cannot be started, and the help points to `EMELA_CC_ARGS`.
#[test]
fn a_compiler_with_flags_in_its_name_points_to_emela_cc_args() {
    let main = "fn main() {\n  dbg(1)\n  ()\n}\n";
    let (analysis, output) = run_c(
        "cc-with-flags",
        main,
        &mut CoreC::with_cc("cc -fsanitize=address", &[]),
    );
    assert!(output.is_none());
    assert_eq!(
        codes(&analysis),
        [(
            Some("E0907"),
            "cannot start `cc -fsanitize=address`: No such file or directory (os error 2)"
        )]
    );
    assert_eq!(
        analysis.diagnostics[0].notes,
        ["help: EMELA_CC names only the compiler; pass flags in EMELA_CC_ARGS"]
    );
}

/// `[target.native.cc]`: one invocation compiles every source, and the flags of `EMELA_CC_ARGS`
/// come at the end of the command line, after every source file. `[target.native.output]`: the
/// output directory has the executable `main` and the header `emela.h`.
#[test]
fn emela_cc_args_come_after_the_sources() {
    let dir = out_dir("cc-args");
    std::fs::create_dir_all(&dir).unwrap();
    // Records its arguments, one per line, and makes `main`.
    let cc = dir.join("record-cc");
    let log = dir.join("args.txt");
    std::fs::write(
        &cc,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ntouch main\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&cc, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (fs, input) = project("fn main() {\n  dbg(1)\n  ()\n}\n");
    let out = dir.join("out");
    let (analysis, entry) = build(
        &fs,
        &input,
        &mut Lower::default().with_target(Target::Native),
        &mut CoreC::with_cc(&cc, &["-std=c23", "-lsqlite3"]),
        &out,
    );
    assert_eq!(codes(&analysis), []);
    assert_eq!(entry.unwrap(), out.join(C_EXECUTABLE));
    assert!(out.join("emela.h").exists());
    let args = std::fs::read_to_string(&log).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let last_source = args.iter().rposition(|a| a.ends_with(".c")).unwrap();
    assert_eq!(&args[args.len() - 2..], ["-std=c23", "-lsqlite3"]);
    assert!(last_source < args.len() - 2, "{args:?}");
    // The project's C sources are compiled as C11 or later, with threads and the math library.
    assert!(args.contains(&"-std=c11"), "{args:?}");
    assert!(args.contains(&"-pthread"), "{args:?}");
    assert!(args.contains(&"-lm"), "{args:?}");
    let _ = std::fs::remove_dir_all(&dir);
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
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default().with_target(Target::Native),
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
    for target in [Target::Js, Target::Native] {
        let options = TestOptions {
            out_dir: out_dir(&format!("assert-inspect-{target}")),
            js_host: emela_driver::js_host_program(),
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
    assert_eq!(
        read("[build]\ntarget = \"native\"\n"),
        Ok(Some(Target::Native))
    );
    // `[target.native]`: `c` is an unknown target since 0.29.
    assert_eq!(
        read("[build]\ntarget = \"c\"\n"),
        Err((
            Some("E0232"),
            "unknown target `c` in `[build].target`".to_owned()
        ))
    );
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
import Std.Task.{Task}

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
    console.print_line("#{Task.timeout(20, || host.double(5, 60000))}")
    console.print_line("#{Task.all([|| host.double(1, 30), || host.double(2, 10)])}")
    console.print_line("#{Task.race([|| host.double(3, 60000), || host.double(4, 10)])}")
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
    disk_project(name, HOST_EMEL, HOST_C)
}

/// A project on disk with `main` as `src/main.emel` and `host` as `src/host.c`.
fn disk_project(name: &str, main: &str, host: &str) -> (PathBuf, Input) {
    let dir = out_dir(&format!("{name}-project"));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Pome.toml"), "").unwrap();
    std::fs::write(dir.join("src/main.emel"), main).unwrap();
    std::fs::write(dir.join("src/host.c"), host).unwrap();
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
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let started = std::time::Instant::now();
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut Lower::default().with_target(Target::Native),
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
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &OsFs,
        &input,
        &mut LowerTests::default().with_target(Target::Native),
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

/// The host side of [`EXPORT_EMEL`]: a constructor calls the exports before main starts, so the
/// first call starts the runtime, and `host_calls_back` calls one while main runs.
const EXPORT_C: &str = r#"#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int32_t add(int32_t a, int32_t b);
int32_t add_again(int32_t a, int32_t b);
char *greet(const char *name);
bool is_even(int32_t n);
int64_t widen(int32_t n);
void *keep(void *h);
void tick(void);

__attribute__((constructor)) static void before_main(void) {
  printf("%d %d\n", add(2, 3), add_again(4, 5));
  char *s = greet("c\xff");
  printf("%s\n", s);
  free(s);
  int x = 7;
  printf("%d %d %lld %d\n", is_even(3), is_even(4), (long long)widen(3), keep(&x) == &x);
  fflush(stdout);
  tick();
}

int32_t host_calls_back(int32_t n) { return add(n, 1); }
"#;

const EXPORT_EMEL: &str = r##"@external
type Handle

@export(c, "add")
@export(c, "add_again")
fn add(a: Int, b: Int) -> Int { a + b }

@export(c, "greet")
fn greet(name: String) -> String { "hello, #{name}" }

@export(c, "is_even")
fn is_even(n: Int) -> Bool { n % 2 == 0 }

@export(c, "widen")
fn widen(n: Int) -> Int64 { Int64.from_int(n) }

@export(c, "keep")
fn keep(h: Handle) -> Handle { h }

@export(c, "tick")
fn tick() -> () {
  dbg("tick")
  ()
}

@external(c, "./host.c", "host_calls_back")
fn host_calls_back(n: Int) -> Int uses {}

fn main() {
  dbg("main")
  dbg(host_calls_back(1))
  ()
}
"##;

/// `[target.native.export]`: each `@export(c, "name")` is a function with external linkage and the
/// prototype of `[ext.boundary]`. Called before main, the first call starts the runtime. Called
/// while a launch is running on the same thread (here, from a host function main called), it is a
/// defect.
#[test]
fn exported_functions_are_called_from_c() {
    if !have_cc() {
        return;
    }
    let dir = out_dir("export-project");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Pome.toml"), "").unwrap();
    std::fs::write(dir.join("src/main.emel"), EXPORT_EMEL).unwrap();
    std::fs::write(dir.join("src/host.c"), EXPORT_C).unwrap();
    let input = Input::resolve(&OsFs, &dir).unwrap();
    let options = RunOptions {
        out_dir: dir.join("out"),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut Lower::default().with_target(Target::Native),
        &mut leak_checked(),
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(text(&output.stdout), "5 9\nhello, c\u{fffd}\n0 1 3 1\n");
    assert_eq!(
        text(&output.stderr),
        "\"tick\"\n\"main\"\ndefect: exported function `add` was called while Emela code is running\n"
    );
    assert_eq!(output.code, 101);
}

/// The host side of [`BYTES_EMEL`]: external functions that take and return Bytes
/// (`[ext.boundary]`), and `suspend fn` ones that complete with `em_complete_bytes`
/// (`[ext.c-host]`).
const BYTES_C: &str = r#"#define _POSIX_C_SOURCE 200809L
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "emela.h"

/* The empty Bytes may come as NULL with a length of 0. */
int32_t host_sum(const uint8_t *data, size_t len) {
  int32_t sum = 0;
  for (size_t i = 0; i < len; i++) sum += data[i];
  return sum;
}

/* A Bytes takes two C parameters in its position. */
int32_t host_sizes(const uint8_t *a, size_t a_len, const char *s, const uint8_t *b, size_t b_len) {
  (void)a;
  (void)b;
  return (int32_t)(a_len * 100 + strlen(s) * 10 + b_len);
}

static em_bytes reversed(const uint8_t *data, size_t len) {
  em_bytes r = {NULL, len};
  if (len == 0) return r;
  r.data = malloc(len);
  for (size_t i = 0; i < len; i++) r.data[i] = data[len - 1 - i];
  return r;
}

em_bytes host_reverse(const uint8_t *data, size_t len) { return reversed(data, len); }

em_bytes host_empty(void) {
  em_bytes r = {NULL, 0};
  return r;
}

em_bytes host_null(void) {
  em_bytes r = {NULL, 3};
  return r;
}

/* The runtime frees `data` although it rejects the length. */
em_bytes host_huge(void) {
  em_bytes r = {malloc(1), (size_t)INT32_MAX + 1};
  return r;
}

/* Completes in place. */
void host_reverse_now(const uint8_t *data, size_t len, em_task *task) {
  em_bytes r = reversed(data, len);
  em_complete_bytes(task, r.data, r.len);
}

static void never(em_task *task, void *ctx) {
  (void)task;
  (void)ctx;
}

static void pause_ms(long ms) {
  struct timespec ts = {0, ms * 1000000};
  nanosleep(&ts, NULL);
}

typedef struct job {
  em_task *task;
  const uint8_t *data;
  size_t len;
  const char *s;
} job;

static void start(void *(*work)(void *), em_task *task, const uint8_t *data, size_t len, const char *s) {
  em_after(60000, task, never, NULL);
  job *j = malloc(sizeof *j);
  j->task = task;
  j->data = data;
  j->len = len;
  j->s = s;
  pthread_t thread;
  pthread_create(&thread, NULL, work, j);
  pthread_detach(thread);
}

/* Reads the bytes on another thread, after the C function has returned. */
static void *reverse_work(void *arg) {
  job *j = arg;
  pause_ms(20);
  em_bytes r = reversed(j->data, j->len);
  em_complete_bytes(j->task, r.data, r.len);
  free(j);
  return NULL;
}

void host_reverse_threaded(const uint8_t *data, size_t len, em_task *task) {
  start(reverse_work, task, data, len, NULL);
}

static pthread_mutex_t held_lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t held_cond = PTHREAD_COND_INITIALIZER;
static bool released = false;
static bool read_done = false;
static char held[64] = "";
static em_task *held_waiter = NULL;

/* Waits for `host_release`, then reads what `host_hold` was given and completes both tasks, that
 * of `host_hold` first: the program ends once `host_held` completes, and a task still running
 * then holds its parameters. */
static void *hold_work(void *arg) {
  job *j = arg;
  pthread_mutex_lock(&held_lock);
  while (!released) pthread_cond_wait(&held_cond, &held_lock);
  snprintf(held, sizeof held, "%.*s %s", (int)j->len, (const char *)j->data, j->s);
  pthread_mutex_unlock(&held_lock);
  em_complete_unit(j->task);
  pthread_mutex_lock(&held_lock);
  read_done = true;
  em_task *waiter = held_waiter;
  pthread_mutex_unlock(&held_lock);
  if (waiter != NULL) em_complete_string(waiter, strdup(held));
  free(j);
  return NULL;
}

/* Ignores the interruption of the waiting fiber, and reads the bytes and the string on another
 * thread after it, once `host_release` lets it. */
void host_hold(const uint8_t *data, size_t len, const char *s, em_task *task) {
  start(hold_work, task, data, len, s);
}

void host_release(void) {
  pthread_mutex_lock(&held_lock);
  released = true;
  pthread_cond_broadcast(&held_cond);
  pthread_mutex_unlock(&held_lock);
}

static void *held_work(void *arg) {
  em_complete_string(arg, strdup(held));
  return NULL;
}

/* Completes with what the thread of `host_hold` read, once it has read it. It completes on
 * another thread, after the task of `host_hold` (completions from other threads are delivered in
 * the order they came): a completion in place could end the program while that task still holds
 * its parameters. */
void host_held(em_task *task) {
  em_after(60000, task, never, NULL);
  pthread_mutex_lock(&held_lock);
  if (read_done) {
    pthread_mutex_unlock(&held_lock);
    pthread_t thread;
    pthread_create(&thread, NULL, held_work, task);
    pthread_detach(thread);
    return;
  }
  held_waiter = task;
  pthread_mutex_unlock(&held_lock);
}

void host_null_now(em_task *task) { em_complete_bytes(task, NULL, 2); }

static void huge(em_task *task, void *ctx) {
  (void)ctx;
  em_complete_bytes(task, malloc(1), (size_t)INT32_MAX + 1);
}

void host_huge_later(em_task *task) { em_after(1, task, huge, NULL); }

/* The runtime frees `data` although the completion does not match. */
void host_bytes_for_string(em_task *task) { em_complete_bytes(task, malloc(2), 2); }
"#;

const BYTES_EMEL: &str = r##"import Std.Bytes.{Bytes}
import Std.Clock.{SystemClock}
import Std.Console.{Console, SystemConsole}
import Std.Task.{Task}

@external(c, "./host.c", "host_sum")
fn host_sum(b: Bytes) -> Int uses {}

@external(c, "./host.c", "host_sizes")
fn host_sizes(a: Bytes, s: String, b: Bytes) -> Int uses {}

@external(c, "./host.c", "host_reverse")
fn host_reverse(b: Bytes) -> Bytes uses {}

@external(c, "./host.c", "host_empty")
fn host_empty() -> Bytes uses {}

@external(c, "./host.c", "host_null")
fn host_null() -> Bytes uses {}

@external(c, "./host.c", "host_huge")
fn host_huge() -> Bytes uses {}

@external(c, "./host.c", "host_release")
fn host_release() -> () uses {}

@external(c, "./host.c", "host_held")
suspend fn host_held() -> String

@external(c, "./host.c", "host_reverse_now")
suspend fn host_reverse_now(b: Bytes) -> Bytes

@external(c, "./host.c", "host_reverse_threaded")
suspend fn host_reverse_threaded(b: Bytes) -> Bytes

@external(c, "./host.c", "host_hold")
suspend fn host_hold(b: Bytes, s: String) -> ()

@external(c, "./host.c", "host_null_now")
suspend fn host_null_now() -> Bytes

@external(c, "./host.c", "host_huge_later")
suspend fn host_huge_later() -> Bytes

@external(c, "./host.c", "host_bytes_for_string")
suspend fn host_bytes_for_string() -> String

effect Host {
  suspend fn reverse_now(b: Bytes) -> Bytes
  suspend fn reverse_threaded(b: Bytes) -> Bytes
  suspend fn hold(b: Bytes, s: String) -> ()
  suspend fn held() -> String
  suspend fn null_now() -> Bytes
  suspend fn huge_later() -> Bytes
  suspend fn bytes_for_string() -> String
}

handler CHost: Host {
  suspend fn reverse_now(b, resume) { resume(host_reverse_now(b)) }
  suspend fn reverse_threaded(b, resume) { resume(host_reverse_threaded(b)) }
  suspend fn hold(b, s, resume) { resume(host_hold(b, s)) }
  suspend fn held(resume) { resume(host_held()) }
  suspend fn null_now(resume) { resume(host_null_now()) }
  suspend fn huge_later(resume) { resume(host_huge_later()) }
  suspend fn bytes_for_string(resume) { resume(host_bytes_for_string()) }
}

fn main() {
  with SystemConsole, SystemClock, CHost {
    console = use Console
    host = use Host
    console.print_line("#{host_sum(Bytes.hex("0102ff"))} #{host_sum(Bytes.empty())}")
    console.print_line("#{host_sizes(Bytes.hex("0102"), "abc", Bytes.empty())}")
    console.print_line(Bytes.to_hex(host_reverse(Bytes.hex("010203"))))
    console.print_line("#{Bytes.size(host_empty())} #{Bytes.size(host_reverse(Bytes.empty()))}")
    console.print_line(Bytes.to_hex(host.reverse_now(Bytes.hex("0a0b"))))
    console.print_line(Bytes.to_hex(host.reverse_threaded(Bytes.hex("0a0b0c"))))
    n = host_sum(Bytes.hex("07"))
    console.print_line(
      "#{Task.timeout(10, || host.hold(Bytes.encode_utf8("bytes #{n}"), "string #{n}"))}"
    )
    # Values of the same sizes as the ones the interrupted call was given, made after the
    # interruption and before the host reads those.
    other_string = "string #{n + 1}"
    other_bytes = Bytes.encode_utf8("bytes #{n + 1}")
    host_release()
    console.print_line(host.held())
    console.print_line("#{String.byte_size(other_string)} #{Bytes.size(other_bytes)}")
  }
}

@test
fn returned_null() {
  assert Bytes.size(host_null()) == 3
}

@test
fn returned_too_many() {
  assert Bytes.size(host_huge()) == 1
}

@test
fn completed_with_null() {
  with CHost { assert Bytes.size((use Host).null_now()) == 2 }
}

@test
fn completed_with_too_many() {
  with CHost { assert Bytes.size((use Host).huge_later()) == 1 }
}

@test
fn completed_with_bytes_for_a_string() {
  with CHost { assert (use Host).bytes_for_string() == "" }
}
"##;

/// `[ext.boundary]`, `[ext.c-host]`: a Bytes parameter is a pointer and a length, a returned Bytes
/// an `em_bytes` that the runtime copies and frees, and an external `suspend fn` completes with
/// `em_complete_bytes`. The bytes and the string of a `suspend fn`'s parameters stay valid until
/// its task completes, after the waiting fiber has been interrupted too.
#[test]
fn bytes_cross_to_c() {
    if !have_cc() {
        return;
    }
    let (dir, input) = disk_project("bytes-run", BYTES_EMEL, BYTES_C);
    let options = RunOptions {
        out_dir: dir.join("out"),
        js_host: emela_driver::js_host_program(),
        args: Vec::new(),
        output: Output::Capture,
    };
    let (analysis, output) = run(
        &OsFs,
        &input,
        &mut Lower::default().with_target(Target::Native),
        &mut sanitized(),
        &options,
        |_, _| {},
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(codes(&analysis), []);
    let output = output.unwrap();
    assert_eq!(text(&output.stderr), "");
    assert_eq!(
        text(&output.stdout),
        "258 0\n230\n030201\n0 0\n0b0a\n0c0b0a\nNone\nbytes 7 string 7\n8 7\n"
    );
    assert_eq!(output.code, 0);
}

/// `[ext.boundary]`, `[ext.c-host]`: a NULL `data` with a length, and a length above 2^31 - 1,
/// are defects, returned or passed to `em_complete_bytes`; so is `em_complete_bytes` for a
/// function that does not return Bytes. The runtime frees `data` in each case (the leak check).
#[test]
fn bytes_from_c_misbehaving() {
    if !have_cc() {
        return;
    }
    let (dir, input) = disk_project("bytes-test", BYTES_EMEL, BYTES_C);
    let options = TestOptions {
        out_dir: dir.join("out"),
        js_host: emela_driver::js_host_program(),
        filter: None,
        output: Output::Capture,
        reporter: emela_driver::Reporter::Plain,
        color: false,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &OsFs,
        &input,
        &mut LowerTests::default().with_target(Target::Native),
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
    let defect = |name: &str, message: String| (name.to_owned(), TestOutcome::Defect(message));
    let null = |f: &str| format!("external function `{f}` returned NULL");
    let many =
        |f: &str| format!("external function `{f}` returned more bytes than a Bytes can hold");
    assert_eq!(
        results,
        [
            defect("returned_null", null("host_null")),
            defect("returned_too_many", many("host_huge")),
            defect("completed_with_null", null("host_null_now")),
            defect("completed_with_too_many", many("host_huge_later")),
            defect(
                "completed_with_bytes_for_a_string",
                "external function `host_bytes_for_string` was completed with `em_complete_bytes`, which does not match its return type".to_owned()
            ),
        ]
    );
}

/// The host side of [`BYTES_EXPORT_EMEL`]: a constructor calls the exports before main starts,
/// then `LAST`, which is replaced by a call that is a defect of the launch, or by nothing.
const BYTES_EXPORT_C: &str = r#"#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

#include "emela.h"

int32_t size(const uint8_t *data, size_t len);
em_bytes twice(const uint8_t *data, size_t len);
em_bytes nothing(void);

__attribute__((constructor)) static void before_main(void) {
  uint8_t b[] = {1, 2, 0};
  em_bytes r = twice(b, 3);
  for (size_t i = 0; i < r.len; i++) printf("%02x", r.data[i]);
  printf("\n");
  free(r.data);
  printf("%d %d\n", size(b, 3), size(NULL, 0));
  em_bytes e = nothing();
  printf("%d %d\n", (int)e.len, e.data == NULL);
  fflush(stdout);
  LAST;
}

/* Links this file into the program. */
void host_main(void) {}
"#;

const BYTES_EXPORT_EMEL: &str = r##"import Std.Bytes.{Bytes}

@export(c, "size")
fn size(b: Bytes) -> Int { Bytes.size(b) }

@export(c, "twice")
fn twice(b: Bytes) -> Bytes { Bytes.append(b, b) }

@export(c, "nothing")
fn nothing() -> Bytes { Bytes.empty() }

@external(c, "./host.c", "host_main")
fn host_main() -> () uses {}

fn main() {
  dbg("main")
  host_main()
}
"##;

/// `[target.native.export]`: an exported fn takes a Bytes as `const uint8_t *data, size_t len` and
/// returns an `em_bytes` the host frees (NULL `data` for no bytes). A NULL `data` with a length,
/// and a length above 2^31 - 1, are defects of the launch.
#[test]
fn exported_functions_take_and_return_bytes() {
    if !have_cc() {
        return;
    }
    let cases = [
        ("", 0, "\"main\"\n"),
        (
            "size(NULL, 2)",
            101,
            "defect: exported function `size` was given NULL\n",
        ),
        (
            "size(b, (size_t)INT32_MAX + 1)",
            101,
            "defect: exported function `size` was given more bytes than a Bytes can hold\n",
        ),
    ];
    for (i, (last, code, stderr)) in cases.into_iter().enumerate() {
        let host = BYTES_EXPORT_C.replace("LAST", if last.is_empty() { "(void)0" } else { last });
        let (dir, input) = disk_project(&format!("bytes-export-{i}"), BYTES_EXPORT_EMEL, &host);
        let options = RunOptions {
            out_dir: dir.join("out"),
            js_host: emela_driver::js_host_program(),
            args: Vec::new(),
            output: Output::Capture,
        };
        let (analysis, output) = run(
            &OsFs,
            &input,
            &mut Lower::default().with_target(Target::Native),
            &mut sanitized(),
            &options,
            |_, _| {},
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(codes(&analysis), []);
        let output = output.unwrap();
        assert_eq!(text(&output.stdout), "010200010200\n3 0\n0 1\n", "{last}");
        assert_eq!(text(&output.stderr), stderr, "{last}");
        assert_eq!(output.code, code, "{last}");
    }
}
