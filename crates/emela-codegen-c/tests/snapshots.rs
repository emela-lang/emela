//! The C emitted for small programs, checked by snapshot.

mod common;

use common::{PRELUDE, to_c};

fn c_of(src: &str) -> String {
    to_c(&format!("{PRELUDE}{src}"))
        .unwrap_or_else(|e| panic!("{e:#?}"))
        .main
}

#[test]
fn loops_and_arithmetic() {
    insta::assert_snapshot!(c_of(
        r##"
fn sum(n: Int, acc: Int) -> Int {
  if n == 0 { acc } else { sum(n - 1, acc + n / 2) }
}

fn main() { print(Int.to_string(sum(10, 0))) }
"##
    ));
}

#[test]
fn enums_lists_and_matching() {
    insta::assert_snapshot!(c_of(
        r##"
enum Shape {
  Circle(Float)
  Empty
}

fn count(xs: List[Shape]) -> Int {
  xs match {
    [] -> 0
    [Circle(_), ..rest] -> 1 + count(rest)
    [Empty, ..rest] -> count(rest)
  }
}

fn main() { print("#{count([Circle(1.0), Empty])}") }
"##
    ));
}

#[test]
fn closures_and_handlers() {
    insta::assert_snapshot!(c_of(
        r##"
effect Ask {
  fn ask() -> Int
}

handler Fixed(n: Int) implements Ask {
  init { Fixed(n: 2) }
  release { print("bye") }
  fn ask() { self.n }
}

fn twice(f: fn(Int) -> Int, x: Int) -> Int { f(f(x)) }

fn main() {
  with Fixed {
    k = (use Ask).ask()
    print("#{twice(fn(x) { x * k }, 3)}")
  }
}
"##
    ));
}

#[test]
fn failures_and_constants() {
    insta::assert_snapshot!(c_of(
        r##"
error Missing(key: String)

const DEFAULT = "none"

fn find(key: String) -> String fails Missing {
  if key == "" { fail Missing(key:) }
  key
}

fn main() {
  print(find("a") escape { Missing(_) -> DEFAULT })
  print(find(""))
}
"##
    ));
}

#[test]
fn suspending_functions_and_fibers() {
    let src = r##"
effect Clock {
  suspend fn sleep(ms: Int) -> Int
}

handler Instant implements Clock {
  suspend fn sleep(ms, resume) { resume(ms + 1) }
}

fn nap(ms: Int) -> Int use { Clock } {
  n = (use Clock).sleep(ms)
  n + ms
}

fn main() -> () use { Async } {
  with Instant {
    total = Fiber.scope(fn() {
      a = Fiber.fork(fn() { nap(1) })
      nap(2) + Fiber.join(a)
    })
    print("#{total}")
  }
}
"##;
    let c = to_c(&format!("import Std.Fiber.{{Fiber}}\n{PRELUDE}{src}"))
        .unwrap_or_else(|e| panic!("{e:#?}"))
        .main;
    insta::assert_snapshot!(c);
}
