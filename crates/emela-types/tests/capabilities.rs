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
