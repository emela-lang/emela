//! Suspending operations, `Async` and the `Immediate` constraint.

use super::render_effects;

/// suspend fn operations and implementations (spec 8.1, 8.4, 8.7). An implementation takes resume
/// as its last parameter (a function taking the operation's return type and returning `()`).
/// Using resume other than by calling it directly (storing it, capturing it in a closure, passing
/// it) puts Async in the handler's dependencies. Only Async may remain in the R of `main` and test
/// functions.
#[test]
fn suspend_operations() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

effect Ask {
  suspend fn ask() -> Int
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

fn later(f: fn(()) -> ()) { () }

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { 0 }
}

handler SystemClock: Clock {
  suspend fn sleep(ms, resume) {
    set_timeout(ms, || resume(()))
  }
  fn now() { 0 }
}

handler Stored: Clock {
  suspend fn sleep(ms, resume) {
    r = resume
    r(())
  }
  fn now() { 0 }
}

handler Passed: Clock {
  suspend fn sleep(ms, resume) { later(resume) }
  fn now() { 0 }
}

handler Piped: Ask {
  suspend fn ask(resume) { 42 |> resume }
}

handler WrongValue: Ask {
  suspend fn ask(resume) { resume("x") }
}

handler ReturnsValue: Ask {
  suspend fn ask(resume) { 42 }
}

handler NoResume: Ask {
  suspend fn ask() { () }
}

handler Mismatched: Clock {
  fn sleep(ms) { () }
  suspend fn now(resume) { resume(0) }
}

fn nap() {
  (use Clock).sleep(10)
  (use Clock).now()
}

fn instant() { with InstantClock { nap() } }

fn waits() { with SystemClock { nap() } }

fn pure_only() -> Int uses {} { with SystemClock { nap() } }

fn main() {
  with SystemClock { nap() }
}

fn test_waits() {
  with SystemClock { nap() }
}
"#
    ));
}

/// Async is a built-in effect (spec 8.7). Handlers for it cannot be written (E4003), nor can
/// `use Async` (E4402). It may appear in use clauses.
#[test]
fn async_effect() {
    insta::assert_snapshot!(render_effects(
        r#"
handler MyAsync: Async {
}

fn grab() { use Async }

fn declared() -> Int uses { Async } { 1 }

fn main() {
  with MyAsync { declared() }
}
"#
    ));
}

/// The body of an fn operation implementation must either have an Immediate R or contain no call
/// that may suspend (E4303, spec 8.1), the latter judged per call. Operation calls are decided by
/// the operation's declaration, so the fn operation `now` of Clock (which has a suspend fn) may be
/// called (Clock lands in the body's R and becomes a handler dependency). Calls of functions and
/// function values are decided by their type's R, so going through a function that only calls fn
/// operations, a closure capturing the capability, or an operation taken out as a value stays
/// E4303. Calls of suspend fn operations are E4303 too. But if the body's R is Immediate, the
/// calls are not looked at (sleep inside a `with` of PureClock, which resumes immediately). Mixing
/// the outer Clock's now with sleep inside the inner `with` satisfies neither and is E4303.
#[test]
fn fn_op_bodies_are_checked_per_call() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Clock {
  suspend fn sleep(ms: Int) -> ()
  fn now() -> Int
}

effect Logger {
  fn info(msg: String) -> ()
}

handler InstantClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { (use Clock).now() }
}

handler CapturedClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    (use Logger).info("now")
    clock.now() + (clock.now() |> (|x| x))
  }
}

handler LazyClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    later = || clock.sleep(1)
    1
  }
}

fn stamp() -> Int uses { Clock } { (use Clock).now() }

handler ViaFn: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { stamp() }
}

handler ViaClosure: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    f = || clock.now()
    f()
  }
}

handler ViaValue: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    clock = use Clock
    g = clock.now
    g()
  }
}

handler Sleeping: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    (use Clock).sleep(1)
    0
  }
}

handler PureClock: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() { 0 }
}

handler LocalSleep: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    with PureClock { (use Clock).sleep(1) }
    0
  }
}

handler MixedSleep: Clock {
  suspend fn sleep(ms, resume) { resume(()) }
  fn now() {
    t = (use Clock).now()
    with PureClock { (use Clock).sleep(1) }
    t
  }
}
"#
    ));
}

/// `R: Immediate` (spec 9.5). An effect is Immediate when it has no suspend fn and is not Async; a
/// set type parameter only when it has the bound. Callbacks passed to external functions must be
/// Immediate too, and their R cannot contain an effect other than a host effect at all (E5003,
/// `[ext.boundary]`).
#[test]
fn immediate_constraint() {
    insta::assert_snapshot!(render_effects(
        r#"
effect Logger {
  fn info(msg: String) -> ()
}

effect Clock {
  suspend fn sleep(ms: Int) -> ()
}

@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "./ffi.mjs", "fetchText")
suspend fn fetch_text(url: String) -> String

@external(js, "onTick")
fn on_tick(f: fn() -> () uses { Clock }) -> ()

fn update[A, R: Immediate](x: A, f: fn(A) -> A uses { R }) -> A uses { R } { f(x) }

fn logs() -> Int {
  update(1, |x| {
    (use Logger).info("x")
    x
  })
}

fn sleeps() -> Int {
  update(1, |x| {
    (use Clock).sleep(1)
    x
  })
}

fn hidden(f: fn(Int) -> Int) -> Int { update(1, f) }

fn named[R: Immediate](f: fn(Int) -> Int uses { R }) -> Int uses { R } { update(1, f) }

fn unnamed[R](f: fn(Int) -> Int uses { R }) -> Int uses { R } { update(1, f) }

fn inferred() { |g| update(1, g) }

fn not_effect[A: Immediate](x: A) -> A { x }

fn trait_bound[R: Show](f: fn() -> () uses { R }) -> () uses { R } { f() }

handler Ticking: Logger {
  fn info(msg) {
    set_timeout(1, || (use Logger).info(msg))
    set_timeout(1, || (use Clock).sleep(1))
    set_timeout(1, || {
      fetch_text(msg)
      ()
    })
  }
}
"#
    ));
}
