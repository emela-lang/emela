//! E2: arrays of all-scalar records and tuples stored inline.
//!
//! The option is off by default. With it on, an element of an all-scalar record or tuple is one
//! 8-byte slot per scalar leaf inside the array instead of a reference to a separately allocated
//! box. The IR still says `Ref`, so every read boxes one element; construction, `set`, `slice`
//! and `concat` copy the slots. The same program must print the same thing, write the same
//! standard error and exit the same way with the option off and on.

use crate::common;

use common::*;
use emela_codegen_wasm::WasmOptions;

const PROGRAM: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)
type Pair(left: Point, right: Point)
type Named(name: String, n: Int)

fn make(lo: Int, hi: Int) -> Array[Point] {
  Array.from_list(List.map(List.range(lo, hi), |i| Point(x: i, y: i * 2)))
}

fn bump(a: Array[Point], i: Int, n: Int) -> Array[Point] {
  if i == n { a } else { bump(Array.set(a, i, Point(x: i, y: -i)), i + 1, n) }
}

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4), Point(x: 5, y: 6)])
  dbg(a)
  dbg(Array.size(a))
  dbg(Array.get(a, 0))
  dbg(Array.get(a, 9))
  dbg(Array.get(a, 1) |> Option.or(Point(x: 9, y: 9)))
  dbg(Array.get(a, 9) |> Option.or(Point(x: 9, y: 9)))
  dbg(Array.get(a, 2) match {
    Some(p) -> p.x * 100 + p.y
    None -> -1
  })

  # A second reference keeps the old array alive, so `set` copies before writing.
  read = || Array.get(a, 0) |> Option.or(Point(x: 0, y: 0))
  b = Array.set(a, 1, Point(x: 50, y: 60))
  dbg(read())
  dbg(b)

  dbg(Array.slice(b, 0, 2))
  dbg(Array.slice(b, -1, 99))
  dbg(Array.slice(b, 2, 1))
  dbg(Array.slice(b, 2, 3))
  dbg(Array.concat([Array.slice(b, 2, 3)]))
  dbg(Array.concat([Array.slice(b, 0, 1), Array.empty(), Array.slice(b, 2, 3)]))

  m = make(0, 4)
  dbg(m)
  dbg(bump(m, 0, Array.size(m)))
  dbg(Array.to_list(m))

  # Tuples and nested records are inline too.
  pairs = Array.from_list([(1, 2), (3, 4)])
  dbg(pairs)
  dbg(Array.set(pairs, 0, (7, 8)))
  nested = Array.from_list([Pair(left: Point(x: 1, y: 2), right: Point(x: 3, y: 4))])
  dbg(nested)
  dbg(Array.get(nested, 0) |> Option.or(Pair(left: Point(x: 0, y: 0), right: Point(x: 0, y: 0))))

  # An element with a reference field keeps the boxed representation.
  named = Array.from_list([Named(name: "a", n: 1), Named(name: "b", n: 2)])
  dbg(named)
  dbg(Array.set(named, 0, Named(name: "c", n: 3)))
}
"#;

fn build(src: &str, aggregate_arrays: bool) -> Vec<u8> {
    let output = to_output_with(
        src,
        WasmOptions {
            aggregate_arrays,
            ..WasmOptions::default()
        },
    );
    wasmparser::validate(&output.module).expect("the module is valid");
    output.module
}

#[test]
fn aggregate_arrays_on_and_off_agree() {
    let off = build(PROGRAM, false);
    let on = build(PROGRAM, true);
    assert_ne!(off, on, "the option must change the array representation");

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("arrays-off"), &off);
    let on_run = run_bytes(&out_dir("arrays-on"), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "the inline array changed what the program prints or how it exits"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
    assert!(
        !on_run.stderr.is_empty(),
        "the program must print its values"
    );
}

#[test]
fn aggregate_arrays_with_by_value_locals_agree() {
    // Both options on: the array element is still excluded from the local value form (E1
    // excludes every array element subtree), so the read paths must stay boxed and agree with
    // the default build.
    let off = build(PROGRAM, false);
    let output = to_output_with(
        PROGRAM,
        WasmOptions {
            values: true,
            aggregate_arrays: true,
        },
    );
    wasmparser::validate(&output.module).expect("the module is valid");
    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("arrays-both-off"), &off);
    let on_run = run_bytes(&out_dir("arrays-both-on"), &output.module);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "the two representation options together changed the output"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
}

#[test]
fn a_shared_array_is_not_mutated_by_set() {
    // The same program with one array read through a closure while `set` writes the other
    // reference. The inline form must copy the whole element slots on the copy-on-write path.
    const COW: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  first = || Array.get(a, 0) |> Option.or(Point(x: 0, y: 0))
  b = Array.set(a, 0, Point(x: 9, y: 9))
  dbg(first())
  dbg(b)
  dbg(Array.get(a, 0) |> Option.or(Point(x: 0, y: 0)))
  dbg(Array.get(a, 1) |> Option.or(Point(x: 0, y: 0)))
}
"#;
    let off = build(COW, false);
    let on = build(COW, true);
    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("arrays-cow-off"), &off);
    let on_run = run_bytes(&out_dir("arrays-cow-on"), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "copy-on-write differs between the boxed and the inline form"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
}
