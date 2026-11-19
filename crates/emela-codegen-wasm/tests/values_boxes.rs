//! E1 step 5: a value held in WASM locals is boxed at an escape boundary and unboxed when it is
//! read back.
//!
//! The option is off by default. With it on, a record is a span of locals; a list head, an enum
//! payload field, a closure capture, and an array element store a box of its scalar leaves, and a
//! read into a value unboxes it. The same program must print the same thing, write the same
//! standard error and exit the same way in both forms, with and without the LIR passes and with
//! E2's inline array elements on or off.
//!
//! The memory tests keep the option on: a read that unboxes a box owned by the list or the array
//! must not take a reference of its own, and a temporary box made for `Array.set` must be
//! released, so a loop that only reads and writes stays inside a fixed amount of linear memory.
//!
//! The source language has no value of type `Any`, so the `Any` direction is exercised by a
//! hand-built IR module in `emit::values`'s unit test rather than from a program here.

mod common;

#[allow(clippy::duplicate_mod)] // `common` includes the same Core test support.
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use common::*;
use emela_codegen_wasm::{Entry, WasmOptions, compile_with_wasm_options};
use emela_core::Roots;

/// A record read from a list head, an enum payload, a closure capture and a handler, an array
/// element read and written (E1 step 5), and a nested record in a list.
const PROGRAM: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)
type Pair(left: Point, right: Point)

enum Maybe {
  Has(value: Point)
  Nothing
}

effect Peek {
  fn peek() -> Point
}

handler Holder(p: Point): Peek {
  init { Holder(p: Point(x: 11, y: 12)) }
  fn peek() { self.p }
}

fn first(xs: List[Point]) -> Int {
  List.head(xs) match {
    Some(p) -> p.x * 10 + p.y
    None -> -1
  }
}

fn uint(p: Point) -> Int { p.x * 100 + p.y }

fn maybe(p: Point) -> Int {
  m = Has(value: p)
  m match {
    Has(value: q) -> q.x * 10 + q.y
    Nothing -> -1
  }
}

fn apply(f: fn() -> Int) -> Int { f() }

fn capture() -> Int {
  p = Point(x: 5, y: 6)
  f = || p.x * 10 + p.y
  apply(f)
}

fn peek() -> Int {
  with Holder {
    p = (use Peek).peek()
    p.x * 10 + p.y
  }
}

fn nested() -> Int {
  ps = [Pair(left: Point(x: 1, y: 2), right: Point(x: 3, y: 4))]
  List.head(ps) match {
    Some(p) -> p.left.x * 10 + p.right.y
    None -> -1
  }
}

fn nested_array() -> Int {
  a = Array.from_list([Pair(left: Point(x: 1, y: 2), right: Point(x: 3, y: 4))])
  p = Array.get(a, 0) |> Option.or(Pair(left: Point(x: 0, y: 0), right: Point(x: 0, y: 0)))
  p.left.x * 10 + p.right.y
}

fn array_ops(a: Array[Point]) -> () {
  p = Array.get(a, 0) |> Option.or(Point(x: 0, y: 0))
  q = Array.get(a, 9) |> Option.or(Point(x: 9, y: 9))
  b = Array.set(a, 1, Point(x: 7, y: 8))
  r = Array.get(b, 1) |> Option.or(Point(x: 0, y: 0))
  dbg(uint(p))
  dbg(uint(q))
  dbg(uint(r))
  dbg(List.head(Array.to_list(b)) match {
    Some(z) -> uint(z)
    None -> -1
  })
  dbg(Array.get(b, 0) match {
    Some(z) -> uint(z)
    None -> -1
  })
  ()
}

fn main() {
  dbg(first([Point(x: 1, y: 2), Point(x: 3, y: 4)]))
  dbg(maybe(Point(x: 3, y: 4)))
  dbg(capture())
  dbg(peek())
  dbg(nested())
  dbg(nested_array())
  dbg([Point(x: 1, y: 2)])
  dbg(Array.from_list([Point(x: 1, y: 2)]))
  array_ops(Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)]))
}
"#;

const OUTPUT: &str = "12\n34\n56\n122\n14\n14\n[Point(x: 1, y: 2)]\nArray.from_list([Point(x: 1, y: 2)])\n102\n909\n708\n102\n102\n";

fn build(name: &str, optimize: bool, values: bool, aggregate_arrays: bool) -> Vec<u8> {
    let m = support::try_lower(&[("main.emel", PROGRAM)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    let output = compile_with_wasm_options(
        &m,
        &Entry::Main,
        optimize,
        emela_lir::opt::Options::default(),
        WasmOptions {
            values,
            aggregate_arrays,
        },
    )
    .unwrap_or_else(|e| panic!("{name}: WASM compilation failed: {e:#?}"));
    wasmparser::validate(&output.module).unwrap_or_else(|e| {
        panic!("{name}: the module is not valid: {e}");
    });
    output.module
}

/// Runs the boxed and by-value builds and checks that they agree.
fn agree(name: &str, optimize: bool, aggregate_arrays: bool) {
    let off = build(&format!("{name}-off"), optimize, false, aggregate_arrays);
    let on = build(&format!("{name}-on"), optimize, true, aggregate_arrays);
    assert_ne!(
        off, on,
        "{name}: the option must change the boundary representation"
    );

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir(&format!("{name}-off")), &off);
    let on_run = run_bytes(&out_dir(&format!("{name}-on")), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "{name}: the by-value form changed what the program prints"
    );
    assert_eq!(on_run.code, 0, "{name}: {}", on_run.stderr);
    assert_eq!(on_run.stderr, OUTPUT, "{name}");
}

#[test]
fn escape_boundaries_agree_on_and_off() {
    agree("boxes", true, false);
}

#[test]
fn inline_array_elements_agree_with_escape_boundaries() {
    agree("boxes-agg", true, true);
}

#[test]
fn escape_boundaries_agree_without_the_lir_passes() {
    agree("boxes-noopt", false, false);
    agree("boxes-noopt-agg", false, true);
}

/// The linear memory the loop may grow to: six 64 KiB pages, as in `tests/memory.rs`.
const LIMIT: u32 = 6 * 65536;

/// Reads and writes element 0 of a two-leaf record array 20 000 times with the option on. A read
/// that dups the box the array owns, or an `Array.set` box that is not released, would leak about
/// 32 bytes a round and exceed the limit.
const ROUNDS: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn rounds(n: Int, total: Int, a: Array[Point]) -> Int {
  if n == 0 {
    total
  } else {
    p = Array.get(a, 0) |> Option.or(Point(x: 0, y: 0))
    b = Array.set(a, 1, Point(x: p.x + 1, y: p.y + 2))
    q = Array.get(b, 1) |> Option.or(Point(x: 0, y: 0))
    rounds(n - 1, total + p.x + p.y + q.x, b)
  }
}

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  dbg(rounds(20000, 0, a))
}
"#;

#[test]
fn reading_and_setting_a_value_element_does_not_leak() {
    let Some(_) = wasmtime() else { return };
    for (name, aggregate_arrays) in [("boxes-leak-boxed", false), ("boxes-leak-inline", true)] {
        let bytes = build_source(
            name,
            ROUNDS,
            WasmOptions {
                values: true,
                aggregate_arrays,
            },
        );
        let run = run_bytes_with_command(&out_dir(name), &bytes, |command| {
            command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
            command.arg("-W").arg("trap-on-grow-failure=n");
        });
        assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    }
}

fn build_source(name: &str, src: &str, options: WasmOptions) -> Vec<u8> {
    let m = support::try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    let output = compile_with_wasm_options(
        &m,
        &Entry::Main,
        true,
        emela_lir::opt::Options::default(),
        options,
    )
    .unwrap_or_else(|e| panic!("{name}: WASM compilation failed: {e:#?}"));
    wasmparser::validate(&output.module)
        .unwrap_or_else(|e| panic!("{name}: the module is not valid: {e}"));
    output.module
}
