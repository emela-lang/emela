//! A `Show` that itself raises a defect ([test.command], [dyn.error]): the test result protocol
//! and the end of `main`.

mod common;

use common::*;

/// An error whose `Show` raises a defect instead of returning a display.
const SHOW_DEFECTS: &str = r#"
error Evil

impl Evil: Show {
  fn show(self) {
    panic("show failed")
  }
}
"#;

#[test]
fn a_show_defect_is_reported_for_its_test_and_cleared() {
    // Showing the error of a test raises a defect. The defect is the message of that test's
    // result and is cleared there, so the next test is not reported as a defect ([test.command]).
    let Some(_) = wasmtime() else { return };
    let src = format!(
        "{SHOW_DEFECTS}{}",
        r#"
@test
fn fails_with_evil() {
  fail Evil
}

@test
fn passes_after() {
  assert 1 + 1 == 2
}
"#
    );
    let bytes = to_wasm_tests(&src, &["fails_with_evil", "passes_after"])
        .unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    let run = run_bytes(&out_dir("show-defect-test-protocol"), &bytes);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stdout.contains(
            "\x1eemela-test {\"event\":\"result\",\"name\":\"fails_with_evil\",\
             \"outcome\":\"error\",\"message\":\"show failed\"}"
        ),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains(
            "\x1eemela-test {\"event\":\"result\",\"name\":\"passes_after\",\"outcome\":\"ok\"}"
        ),
        "{}",
        run.stdout
    );
}

#[test]
fn a_show_defect_at_the_end_of_main_is_dropped() {
    // At the end of `main` the failure has already been chosen, so a defect raised while showing
    // it is dropped: the line is `error: ` with no display, and the exit code stays 1. This is
    // the C runtime's `em_exit` behavior (the spec does not fix the case).
    let Some(run) = run(
        "show-defect-main",
        &format!(
            "{SHOW_DEFECTS}{}",
            r#"
fn main() {
  fail Evil
}
"#
        ),
    ) else {
        return;
    };
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert_eq!(run.stderr, "error: \n");
}

#[test]
fn a_show_defect_while_showing_a_cause_is_reported_at_the_end() {
    // `Cause.to_string` is called after the attempt has ended, so no attempt frame catches the
    // defect its `show` raises. The cause shows as `error: ` and the defect stays pending, so the
    // program reports it as a `defect:` line and exits with 101 ([api.std.cause], [dyn.error]).
    let Some(run) = run(
        "show-defect-cause",
        &format!(
            "{}{SHOW_DEFECTS}{}",
            r#"
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause}
"#,
            r#"
fn main() {
  Exit.attempt(|| fail Evil) match {
    Success(_) -> dbg("success")
    Failure(c) -> dbg(Cause.to_string(c))
  }
}
"#
        ),
    ) else {
        return;
    };
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert_eq!(run.stdout, "");
    assert_eq!(run.stderr, "\"error: \"\ndefect: show failed\n");
}
