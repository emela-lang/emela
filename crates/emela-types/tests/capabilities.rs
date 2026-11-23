//! Capabilities told apart in types and in R (`[effect.use]`, `[effect.with]`,
//! `[effect.escape]`, `[type.rules]`).
//!
//! The output shows each fn's type and its local variables' types, then the diagnostics.

use crate::common;

use common::render_bodies;

const DECLS: &str = r#"
import Std.State.{State}

effect Logger {
  fn info(msg: String) -> ()
}

handler Quiet: Logger {
  fn info(msg) { () }
}

handler Loud: Logger {
  fn info(msg) { () }
}

effect Counter {
  fn next() -> Int
}

effect Counters {
  handler Open(start: Int): Counter
}

handler Plain: Counters {
  handler Open(start: Int): Counter {
    init(start: Int) { Self(start:) }
    fn next() { self.start + 1 }
  }
}
"#;

fn run(src: &str) -> String {
    render_bodies(&format!("{DECLS}{src}"))
}

/// Calling an operation of a capability adds that capability to R, and the `with` that made it
/// removes it. Capabilities of different `with`s fit in one list: the element type records both,
/// and calling an operation of an element adds both (`[type.rules]`, #701). A handler member's
/// capability is the same.
#[test]
fn capabilities_in_r() {
    insta::assert_snapshot!(run(r#"
fn direct() {
  with log = Quiet {
    log.info("a")
  }
}

fn listed() -> List[Int] {
  with Plain, x = Counters.Open(1), y = Counters.Open(10) {
    [x, y].map(|c| c.next())
  }
}

fn two() {
  with a = Quiet {
    with b = Loud {
      [a, b].map(|l| l.info("x"))
      ()
    }
  }
}
"#));
}

/// A capability parameter gives the function a set type parameter, decided at each call; the
/// call puts the argument's set in R. The result type takes the sets of the parameters
/// (`[type.rules]`).
#[test]
fn capability_parameters() {
    insta::assert_snapshot!(run(r#"
fn tell(l: use Logger) { l.info("x") }

fn pick(a: use Logger, b: use Logger, first: Bool) -> use Logger {
  if first { a } else { b }
}

fn declared(l: use Logger) -> () uses {} { l.info("y") }

fn main() {
  with a = Quiet {
    with b = Loud {
      tell(b)
      pick(a, b, True).info("z")
      declared(a)
    }
  }
}
"#));
}

/// A capability cannot leave the `with` that made it (E0455): in the value of the body, in the R
/// of a closure that is the value, and in a type fixed outside (the type argument of an outer
/// `State`). A closure used up inside is fine (`[effect.escape]`).
#[test]
fn capability_leaks() {
    insta::assert_snapshot!(run(r#"
fn value() {
  with log = Quiet { log }
}

fn closure() {
  with log = Quiet {
    || log.info("hi")
  }
}

fn into_state() {
  State.run(None, || {
    st <- State[Option[use Logger]]
    with log = Quiet {
      st.set(Some(log))
    }
  })
}

fn used_up(names: List[String]) {
  with log = Quiet {
    names.map(|n| log.info(n))
  }
}
"#));
}

/// A capability is not widened to its effect: a closure whose R has `g` does not fit
/// `uses { Logger }` (E0403, with a help).
#[test]
fn capabilities_are_not_widened() {
    insta::assert_snapshot!(run(r#"
fn keep(f: fn() -> () uses { Logger }) { () }

fn main() {
  with g = Quiet {
    keep(|| g.info("x"))
  }
}
"#));
}

/// `use E` written as a parameter of a function type is quantified at it (`[type.rules]`): a
/// closure checked against it receives an unknown set, which can be in its R but not in its
/// result (E0455). A call puts the argument's set in place of the quantified one. A function of
/// a capability parameter fits too. A closure without an expected type is not quantified.
#[test]
fn quantified_function_types() {
    insta::assert_snapshot!(run(r#"
fn with_all[T](n: Int, acc: List[use Counter], k: fn(List[use Counter]) -> T) -> T uses { Counters } {
  if n == 0 {
    k(acc)
  } else {
    with c = Counters.Open(n) { with_all(n - 1, [c, ..acc], k) }
  }
}

fn sum(cs: List[use Counter]) -> Int {
  cs.fold(from: 0, f: |n, c| n + c.next())
}

fn uses_them() -> Int {
  with Plain {
    with_all(3, [], |cs| cs.map(|c| c.next()).fold(from: 0, f: |a, b| a + b))
  }
}

fn named() -> Int {
  with Plain { with_all(2, [], sum) }
}

fn returns_them() {
  with Plain {
    with_all(1, [], |cs| cs)
  }
}

fn not_quantified() -> Int {
  with Plain, c = Counters.Open(1) {
    k = |cs| sum(cs)
    k([c])
  }
}
"#));
}

/// A capability type written in a field gives the type a hidden set, decided when a value is
/// made: a record of capabilities of two `with`s records both, and cannot leave either
/// (`[type.rules]`, E0455). The hidden sets are not shown.
#[test]
fn fields_record_capabilities() {
    insta::assert_snapshot!(run(r#"
type Pair(first: use Logger, second: use Logger)

fn both(p: Pair) {
  p.first.info("a")
  p.second.info("b")
}

fn main() {
  with a = Quiet {
    with b = Loud {
      p = Pair(first: a, second: b)
      both(p)
    }
  }
}

fn leaks() {
  with a = Quiet {
    Pair(first: a, second: a)
  }
}
"#));
}

/// A type or enum with `uses E` is a capability-bound type of E: only a handler of E makes its
/// values (E0458), and a value made there records the handler's `self`. An operation of E that
/// returns one records the capability it is called on, which cannot leave its `with` (E0455).
/// An operation's result cannot contain a capability type, or a bound type of another effect
/// (E0456).
#[test]
fn capability_bound_types() {
    insta::assert_snapshot!(run(r#"
type Ticket(n: Int) uses Tickets

effect Tickets {
  fn issue() -> Ticket
}

effect Wrong {
  fn leak() -> use Logger
  fn other() -> Ticket
}

handler Issuer: Tickets {
  fn issue() { Ticket(1) }
}

fn outside() -> Ticket uses { Tickets } { Ticket(2) }

fn kept() -> Int {
  with t = Issuer {
    t.issue().n
  }
}

fn leaves() {
  with t = Issuer {
    t.issue()
  }
}
"#));
}

/// Handlers and capabilities (`[effect.handler]`): a field of type `use E` takes the set of the
/// capability given to init at each item; a capability parameter of an operation is an unknown
/// set in the implementation; `self` is the capability of the handler and can be passed to a
/// parameter of type `use E`. The fields given to `Self(...)` cannot record a capability of a
/// with inside the init, nor a handler member opened through its handlers, by name or through a
/// capability (E0455, #658); one used inside that with is fine.
#[test]
fn handlers_and_capabilities() {
    insta::assert_snapshot!(run(r#"
effect Store {
  fn keep(l: use Logger) -> ()
}

handler Keeper: Store {
  fn keep(l) { l.info("kept") }
}

handler Holder(c: use Counter): Logger {
  init(c: use Counter) { Self(c:) }
  fn info(msg) {
    self.c.next()
    ()
  }
}

handler Inner(c: use Counter): Logger {
  init() {
    with x = Counters.Open(1) { Self(c: x) }
  }
  fn info(msg) { () }
}

handler ByName(c: use Counter): Logger {
  init() {
    with Plain { Self(c: Counters.Open(1)) }
  }
  fn info(msg) { () }
}

handler ByCapability(c: use Counter): Logger {
  init() {
    with p = Plain { Self(c: p.Open(1)) }
  }
  fn info(msg) { () }
}

handler UsedInside(n: Int): Logger {
  init() {
    n = with Plain { Counters.Open(1).next() }
    Self(n:)
  }
  fn info(msg) { () }
}

fn again(l: use Logger) { l.info("") }

handler Echo: Logger {
  fn info(msg) {
    if msg == "" { () } else { again(self) }
  }
}

fn main() {
  with Plain, x = Counters.Open(1) {
    with h = Holder(c: x), s = Keeper {
      s.keep(h)
    }
  }
}
"#));
}

/// A callback passed to an external function cannot have a capability in its R: the host may
/// keep it after the capability's with has ended (E0454). A parameter marked `@borrowed` may
/// take one. `@borrowed` is written only on a parameter of function type of an external
/// function (E0457, `[ext.boundary]`).
#[test]
fn callbacks_of_external_functions() {
    insta::assert_snapshot!(run(r#"
@external(js, "setTimeout")
fn set_timeout(ms: Int, f: fn() -> ()) -> ()

@external(js, "./ffi.mjs", "each")
fn each_now(n: Int, @borrowed f: fn(Int) -> ()) -> ()

@external(js, "./ffi.mjs", "twice")
fn twice(@borrowed n: Int) -> Int

fn plain(@borrowed f: fn() -> ()) { f() }

handler Ticking: Counter {
  fn next() {
    with log = Quiet {
      set_timeout(1, || log.info("late"))
      each_now(3, |i| log.info("now"))
    }
    1
  }
}
"#));
}

/// An error whose fields record capabilities records them in its tag in E (`[type.rules]`): it
/// cannot leave the with that made them (E0455, #719), and the fields of an error caught by
/// `escape` take the capabilities of the tag, so a value taken out of it cannot leave either.
#[test]
fn errors_record_capabilities() {
    insta::assert_snapshot!(render_bodies(
        r#"
import Std.Fiber.{Fiber, Scope}

error Leak(f: Fiber[Int])

fn raise(s: use Scope[{}, {}]) {
  f = s.fork(|| 1)
  fail Leak(f:)
}

fn caught() uses { Async } {
  Fiber.scope(|s| {
    raise(s) escape {
      Leak(f:) -> ()
    }
  })
}

fn leaves() -> Int fails Leak uses { Async } {
  Fiber.scope(|s| {
    f = s.fork(|| 1)
    if True { fail Leak(f:) } else { Fiber.join(f) }
  })
}

fn taken_out() uses { Async } {
  Fiber.scope(|s| {
    f = s.fork(|| 1)
    (if True { fail Leak(f:) } else { f }) escape {
      Leak(f:) -> f
    }
  })
}
"#
    ));
}
