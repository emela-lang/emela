//! No call that may suspend after resume (E0461, `[effect.handler]`, `[dyn.suspend]`).
//!
//! In the implementation of a suspend fn, a call that may suspend is reported when some path
//! through the body reaches it after a direct call of resume, or after another use of resume
//! (stored, captured, passed on; `[effect.async]`). The output is `render_bodies`: declared types
//! and the diagnostics of `main.emel`, each with the resume it follows.

use crate::common;

use common::render_bodies;

const DECLS: &str = r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

effect Logger {
  fn info(msg: String) -> ()
}

effect Op {
  suspend fn op(flag: Bool) -> Int
}

@external(js, "setTimeout")
fn set_timeout(f: fn() -> () uses {}, ms: Int) -> () uses {}

@external(js, "./wait.mjs", "wait")
suspend fn wait() -> ()
"#;

fn render(src: &str) -> String {
    render_bodies(&format!("{DECLS}{src}"))
}

/// A suspending call after a direct call of resume is E0461: an operation, an `@external suspend
/// fn`, a function whose R may suspend, and a function value. Calls that do not suspend (an fn
/// operation, a function whose R is Immediate) are allowed, and so is a second resume.
#[test]
fn after_a_direct_call() {
    insta::assert_snapshot!(render(
        r#"
fn stamp() -> Int uses { Clock } { (use Clock).now() }
fn log(msg: String) -> () uses { Logger } { (use Logger).info(msg) }

handler Direct: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    logger <- Logger
    resume(1)
    clock.sleep(1)
    wait()
    stamp()
    f = || clock.sleep(2)
    f()
    clock.now()
    logger.info("fine")
    log("fine")
  }
}
"#
    ));
}

/// A suspending call before resume, or in another branch than the one that resumes, is allowed.
#[test]
fn before_resume_is_allowed() {
    insta::assert_snapshot!(render(
        r#"
handler Before: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    clock.sleep(1)
    wait()
    if flag { resume(1) } else {
      clock.sleep(2)
      resume(2)
    }
  }
}
"#
    ));
}

/// If one branch of an if or a match resumes, what follows the branch is after resume. The
/// arguments of a call are evaluated before it, so a resume in an argument comes first. `x |>
/// resume` is a direct call too.
#[test]
fn paths_through_branches() {
    insta::assert_snapshot!(render(
        r#"
fn delay(n: Int) -> () uses { Clock } { (use Clock).sleep(n) }

handler IfBranch: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    if flag { resume(1) } else { () }
    clock.sleep(1)
  }
}

handler MatchArm: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    Some(1) match {
      Some(n) -> resume(n)
      None -> ()
    }
    clock.sleep(1)
    resume(2)
  }
}

handler InArgument: Op {
  suspend fn op(flag, resume) {
    delay(resume(1) |> (|_| 1))
  }
}

handler Piped: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    1 |> resume
    clock.sleep(1)
  }
}
"#
    ));
}

/// A use of resume other than a direct call (captured in a closure, stored, passed as an
/// argument) may resume the caller from a callback or another fiber while the implementation is
/// still running, so what follows it is after resume. A call inside the closure runs where the
/// closure is called, so it is not reported where the closure is written.
#[test]
fn after_resume_is_passed_on() {
    insta::assert_snapshot!(render(
        r#"
handler Callback: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    set_timeout(|| resume(1), 0)
    clock.sleep(1000)
  }
}

handler Stored: Op {
  suspend fn op(flag, resume) {
    r = resume
    wait()
    r(1)
  }
}

handler CallbackLast: Op {
  suspend fn op(flag, resume) {
    clock <- Clock
    clock.sleep(1)
    set_timeout(|| resume(1), 0)
  }
}
"#
    ));
}
