//! Suspending programs: in-place resumes, the values `resume` carries, a dropped continuation
//! and a second resume (spec 8.4, 8.7, 14.5). Skipped without `wasmtime`.
//!
//! Fibers and a host callback for `suspend fn` external functions are not implemented yet, so
//! nothing can resume a continuation *later*; a continuation that suspended is unwound as
//! interrupted, which runs its `with` releases.

mod common;

use common::*;
use emela_codegen_wasm::WasmErrorKind;

/// Runs `src`, expecting exit code `code`.
fn run_code(name: &str, src: &str, code: i32) -> Option<Run> {
    let run = run(name, src)?;
    assert_eq!(run.code, code, "{name}: {}", run.stderr);
    Some(run)
}

#[test]
fn in_place_resume_continues_the_caller() {
    // A handler that resumes in place: the caller does not suspend, and the value continues
    // through a recursive suspending function (if, calls, suspension edges).
    let Some(run) = run_code(
        "suspend-inline",
        r#"
effect Tick {
  suspend fn tick() -> Int
}

handler Ticks: Tick {
  suspend fn tick(resume) {
    resume(3)
  }
}

fn total(n: Int) -> Int use { Tick } {
  if n == 0 { 0 } else {
    x = (use Tick).tick()
    x + total(n - 1)
  }
}

fn main() -> () use { Async } {
  with Ticks {
    dbg(total(4))
    ()
  }
}
"#,
        0,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "12\n");
}

#[test]
fn resumes_carry_every_representation() {
    // The resume closure has one entry per representation of the operation's result.
    let Some(run) = run_code(
        "suspend-values",
        r#"
effect Values {
  suspend fn int() -> Int
  suspend fn long() -> Int64
  suspend fn float() -> Float
  suspend fn text() -> String
  suspend fn unit() -> ()
}

handler Resumes: Values {
  suspend fn int(resume) { resume(41) }
  suspend fn long(resume) { resume(Int64.from_int(42)) }
  suspend fn float(resume) { resume(1.5) }
  suspend fn text(resume) { resume("ok") }
  suspend fn unit(resume) { resume(()) }
}

fn main() -> () use { Async } {
  with Resumes {
    dbg((use Values).int())
    dbg((use Values).long())
    dbg((use Values).float())
    dbg((use Values).text())
    (use Values).unit()
    dbg("done")
    ()
  }
}
"#,
        0,
    ) else {
        return;
    };
    assert_eq!(run.stderr, "41\n42\n1.5\n\"ok\"\n\"done\"\n");
}

#[test]
fn a_dropped_continuation_in_a_non_suspending_function_is_interrupted() {
    // Nothing can resume it (spec 8.7): the continuation unwinds at once as interrupted and the
    // handler releases run (spec 14.5).
    let Some(run) = run_code(
        "suspend-dropped",
        r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) {
    dbg("not resuming")
    ()
  }
}

handler Now: Wait {
  suspend fn wait(resume) { resume(7) }
}

effect Name {
  fn name() -> String
}

handler Res(label: String): Name {
  init { Res(label: "res") }
  fn name() { self.label }
  release { dbg("release #{self.label}") }
}

fn waits() -> Int use { Wait, Name } {
  n = (use Wait).wait()
  dbg("after wait #{n}")
  n
}

fn main() {
  with Now, Res {
    dbg("now #{waits()}")
  }
  with Res {
    with Ignores {
      dbg("never #{waits()}")
    }
  }
}
"#,
        130,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"after wait 7\"\n\"now 7\"\n\"release res\"\n\"not resuming\"\n\"release res\"\n\
         interrupted\n"
    );
}

#[test]
fn a_dropped_continuation_in_a_suspending_main_stalls() {
    // The main continuation suspends and the run stalls: every waiting continuation is unwound
    // as interrupted, from the inside (its releases run).
    let Some(run) = run_code(
        "suspend-stall",
        r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) {
    dbg("not resuming")
    ()
  }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { dbg("release res") }
}

fn main() -> () use { Async } {
  with Res {
    with Ignores {
      dbg("never #{(use Wait).wait()}")
    }
    dbg("after with")
  }
  dbg("done")
  ()
}
"#,
        130,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"not resuming\"\n\"release res\"\ninterrupted\n"
    );
}

#[test]
fn a_failure_after_resume_is_a_defect() {
    // The implementation resumed, so its later failure cannot go to the caller: it ends the
    // program with a defect that shows the error (spec 14.5).
    let Some(run) = run_code(
        "suspend-after-resume",
        r#"
error Nope

effect Late {
  suspend fn late() -> Int fails Nope
}

handler FailsLate: Late {
  suspend fn late(resume) {
    resume(1)
    dbg("after resume")
    fail Nope
  }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { dbg("release res") }
}

fn main() {
  with Res, FailsLate {
    r = (use Late).late() escape { Nope -> -1 }
    dbg("got #{r}")
  }
  dbg("not reached")
  ()
}
"#,
        101,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"after resume\"\n\"release res\"\n\
         defect: operation `late` of handler `FailsLate` failed after resume: Nope\n"
    );
}

#[test]
fn a_second_resume_is_a_defect() {
    let Some(run) = run_code(
        "suspend-twice",
        r#"
effect Twice {
  suspend fn twice() -> Int
}

handler TwiceResumer: Twice {
  suspend fn twice(resume) {
    resume(1)
    resume(2)
  }
}

fn main() {
  with TwiceResumer {
    dbg("got #{(use Twice).twice()}")
  }
}
"#,
        101,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "defect: resume called twice in operation `twice` of handler `TwiceResumer`\n"
    );
}

#[test]
fn a_waiting_operation_inside_a_waiting_implementation_is_unwound() {
    // Each implementation waits on the next operation and the innermost never resumes. Every
    // implementation's continuation is unwound as interrupted, from the inside (spec 14.2, 14.5).
    let Some(run) = run_code(
        "suspend-nested-waits",
        r#"
effect Inner {
  suspend fn inner() -> Int
}

handler Stuck: Inner {
  suspend fn inner(resume) {
    dbg("inner never resumes")
    ()
  }
}

effect Middle {
  suspend fn middle() -> Int
}

handler Middles: Middle {
  suspend fn middle(resume) {
    with ResMiddle {
      resume((use Inner).inner())
    }
  }
}

effect Outer {
  suspend fn outer() -> Int
}

handler Outers: Outer {
  suspend fn outer(resume) {
    with ResOuter {
      resume((use Middle).middle())
    }
  }
}

effect Name {
  fn name() -> String
}

handler ResMiddle: Name {
  init { ResMiddle }
  fn name() { "middle" }
  release { dbg("release middle") }
}

handler ResOuter: Name {
  init { ResOuter }
  fn name() { "outer" }
  release { dbg("release outer") }
}

fn main() -> () use { Async } {
  with Stuck, Middles, Outers {
    dbg((use Outer).outer())
    ()
  }
}
"#,
        130,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"inner never resumes\"\n\"release middle\"\n\"release outer\"\ninterrupted\n"
    );
}

#[test]
fn a_defect_in_an_unwound_implementation_replaces_the_interruption() {
    // The implementation's continuation is unwound before the caller's, and its release's defect
    // stays the result when the caller is unwound as interrupted afterwards (spec 14.4).
    let Some(run) = run_code(
        "suspend-impl-release-defect",
        r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Stuck: Wait {
  suspend fn wait(resume) {
    dbg("never resumes")
    ()
  }
}

effect Outer {
  suspend fn outer() -> Int
}

handler Outers: Outer {
  suspend fn outer(resume) {
    with Boom {
      resume((use Wait).wait())
    }
  }
}

effect Name {
  fn name() -> String
}

handler Boom: Name {
  init { Boom }
  fn name() { "boom" }
  release {
    dbg("release boom")
    panic("release boom")
  }
}

fn main() -> () use { Async } {
  with Stuck, Outers {
    dbg((use Outer).outer())
    ()
  }
}
"#,
        101,
    ) else {
        return;
    };
    assert_eq!(
        run.stderr,
        "\"never resumes\"\n\"release boom\"\ndefect: release boom\n"
    );
}

#[test]
fn an_interrupted_test_is_reported() {
    // A test whose continuation is unwound as interrupted: the runner's protocol reports the
    // `interrupted` outcome (spec 13.4, 14.5).
    let Some(_) = wasmtime() else { return };
    let src = r#"
effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) { () }
}

@test
fn interrupted_one() -> () use { Async } {
  with Ignores {
    dbg((use Wait).wait())
    ()
  }
}

@test
fn passes_after() {
  assert 1 + 1 == 2
}
"#;
    let bytes = to_wasm_tests(src, &["interrupted_one", "passes_after"])
        .unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    let run = run_bytes(&out_dir("suspend-test-protocol"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains(
            "\x1eemela-test {\"event\":\"result\",\"name\":\"interrupted_one\",\
             \"outcome\":\"interrupted\"}"
        ),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains(
            "\x1eemela-test {\"event\":\"result\",\"name\":\"passes_after\",\
             \"outcome\":\"ok\"}"
        ),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("\x1eemela-test {\"event\":\"done\"}"),
        "{}",
        run.stdout
    );
}

/// A suspending function with `points` suspension points, optionally inside a `with` whose release
/// prints (the shape that copies a cleanup into every defect landing). Returns the source and the
/// value it prints.
fn many_points(points: usize, with_release: bool) -> (String, i32) {
    assert!(points > 0);
    let body: String = (0..points)
        .map(|i| format!("  v{i} = (use Tick).tick() + {i}\n"))
        .collect();
    let last = points - 1;
    let name = if with_release {
        r#"
effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { dbg("release res") }
}
"#
    } else {
        ""
    };
    let work = if with_release {
        format!("  with Res {{\n{body}    v{last}\n  }}")
    } else {
        format!("{body}  v{last}")
    };
    // The last binding is `tick() + (points - 1)`, and every `tick` resumes with 1.
    let total = points as i32;
    let src = format!(
        r#"effect Tick {{
  suspend fn tick() -> Int
}}

handler Ticks: Tick {{
  suspend fn tick(resume) {{ resume(1) }}
}}
{name}
fn work() -> Int use {{ Tick }} {{
{work}
}}

fn main() {{
  with Ticks {{
    dbg(work())
  }}
}}
"#
    );
    (src, total)
}

#[test]
fn many_suspension_points_run() {
    // A defect landing runs in place and a fairness yield needs no state, so 300 points do not
    // make a deeply nested dispatch that an engine takes seconds to compile.
    let (src, total) = many_points(300, true);
    let Some(run) = run("suspend-many", &src) else {
        return;
    };
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains(&format!("{total}\n")), "{}", run.stderr);
    assert!(run.stderr.contains("\"release res\""), "{}", run.stderr);
}

#[test]
fn a_thousand_suspension_points_compile() {
    // Compilation only: running it would spend the test budget in the engine's compiler.
    let (src, _) = many_points(1000, false);
    let bytes = to_wasm(&src).unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
}

#[test]
fn a_huge_state_machine_is_diagnosed() {
    // One block per state makes the engine's compile time and memory grow sharply, so a machine
    // past the limit is refused with a diagnostic instead of exhausting the compiler.
    let (src, _) = many_points(1100, false);
    let errors = to_wasm(&src).expect_err("a machine past the state limit must be refused");
    assert!(
        errors
            .iter()
            .any(|error| error.kind == WasmErrorKind::Unsupported
                && error.message.contains("states")
                && error.message.contains("limit")),
        "{errors:#?}"
    );
}
