//! Behavioural tests for records and tuples held by value in the C backend (`layout`): the
//! by-value form and the boxed form print the same, with the LIR optimisation on and off, and an
//! array of a record allocates no object per element.
//!
//! Every run has the leak check on, so a box that the emitter makes at a boundary (a list
//! element, a closure capture, an enum payload, `Any`) must be owned exactly once.

mod common;

#[allow(clippy::duplicate_mod)] // `common` includes the same Core test support.
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use common::{PRELUDE, Run, build_and_run, cc, out_dir};
use emela_codegen_c::{COutput, Entry, compile_with_values};
use emela_core::Roots;
use support::try_lower;

/// A program with a record or a tuple in every place the emitter converts: an array of records
/// (read, written, sliced, concatenated, turned into a list), a list of records, a record in
/// `Option`, nested records, 32-bit fields with padding after them, a record passed to and
/// returned from a function by value, a record built by a closure, and a tuple built and
/// unpacked in `List.fold`. A handler whose state is a record is in the snapshot of
/// `snapshots.rs`, and in `tests/programs/attempt_cause.emel`.
const PROGRAM: &str = r##"
type Point(x: Int, y: Int)

type Mixed(n: Int, f: Float)

type Pair(left: Point, right: Point)

type Body(x: Float, y: Float, z: Float, vx: Float, mass: Float)

fn bump(p: Point) -> Point { Point(x: p.x + 1, y: p.y + 1) }

fn sum_pair(p: Pair) -> Int { p.left.x + p.left.y + p.right.x + p.right.y }

fn step(b: Body) -> Body {
  Body(x: b.x + b.vx, y: b.y + b.vx, z: b.z, vx: b.vx * 0.5, mass: b.mass)
}

fn point_at(points: Array[Point], i: Int) -> Point {
  Array.get(points, i) |> Option.or(Point(x: 0, y: 0))
}

fn body_at(bodies: Array[Body], i: Int) -> Body {
  Array.get(bodies, i) |> Option.or(Body(x: 0.0, y: 0.0, z: 0.0, vx: 0.0, mass: 0.0))
}

fn main() {
  points = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4), Point(x: 5, y: 6)])
  p = point_at(points, 1)
  print("#{p.x} #{p.y}")
  points = Array.set(points, 0, bump(p))
  q = point_at(points, 0)
  print("#{q.x} #{q.y}")
  got = Array.get(points, 2)
  near = Option.or(got, Point(x: 0, y: 0))
  print("#{near.x} #{near.y}")
  print("#{Array.size(Array.slice(points, 1, 3))}")
  print("#{Array.size(Array.concat([points, Array.from_list([Point(x: 7, y: 8)])]))}")
  print("#{List.length(Array.to_list(points))}")

  xs = [Point(x: 7, y: 8), Point(x: 9, y: 10)]
  first = List.fold(xs, Point(x: 0, y: 0), |acc, x| Point(x: acc.x + x.x, y: acc.y + x.y))
  print("#{first.x} #{first.y}")

  m = Mixed(n: 41, f: 1.5)
  print("#{m.n} #{m.f}")

  pair = Pair(left: Point(x: 1, y: 2), right: Point(x: 3, y: 4))
  print("#{sum_pair(pair)} #{pair.right.y}")

  f = |x| Body(x: x, y: 0.0, z: 0.0, vx: 2.0, mass: 1.0)
  b = f(1.0)
  print("#{step(b).x} #{step(b).vx}")

  (n, w) = List.fold(List.range(0, 3), from: (0, 0.0), f: |acc, i| {
    (a, v) = acc
    (a + i, v + Float.from_int(i) * 0.5)
  })
  print("#{n} #{w}")

  made = Array.from_list(List.map(List.range(0, 4), |i| Point(x: i, y: i + 1)))
  print("#{Array.size(made)} #{point_at(made, 3).y}")

  bodies = Array.from_list([
    Body(x: 0.0, y: 0.0, z: 0.0, vx: 1.0, mass: 2.0),
    Body(x: 1.0, y: 1.0, z: 1.0, vx: 2.0, mass: 3.0),
  ])
  bodies = Array.set(bodies, 0, step(body_at(bodies, 1)))
  moved = body_at(bodies, 0)
  print("#{moved.x} #{moved.vx}")
}
"##;

/// Lowers and compiles `src`, with the LIR passes and the by-value layout on or off.
fn to_c(name: &str, src: &str, optimize: bool, values: bool) -> COutput {
    let core = try_lower(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    compile_with_values(&core, &Entry::Main, optimize, values)
        .unwrap_or_else(|e| panic!("{name}: C compilation failed: {e:#?}"))
}

/// Compiles `src` with the LIR passes and the by-value layout on or off, and runs it.
fn run(name: &str, src: &str, optimize: bool, values: bool) -> Run {
    let out = to_c(name, src, optimize, values);
    build_and_run(&out_dir(name), &out, &["-O1"])
}

/// The by-value layout changes only what the specification leaves implementation-defined: the
/// same output and exit status with it on and off, and with the LIR passes on and off.
#[test]
fn values_change_nothing_observable() {
    if cc().is_none() {
        return;
    }
    let src = format!("import Std.Array.{{Array}}\n{PRELUDE}{PROGRAM}");
    let boxed_off = run("values-off-noopt", &src, false, false);
    let boxed_on = run("values-on-noopt", &src, false, true);
    let values_off = run("values-off", &src, true, false);
    let values_on = run("values-on", &src, true, true);
    for (what, got) in [
        ("the boxed form with the LIR passes off", &boxed_off),
        ("the by-value form with the LIR passes off", &boxed_on),
        ("the boxed form", &values_off),
    ] {
        assert_eq!(
            (got.stdout.as_str(), got.stderr.as_str(), got.code),
            (
                values_on.stdout.as_str(),
                values_on.stderr.as_str(),
                values_on.code
            ),
            "{what} differs from the by-value form"
        );
    }
    assert_eq!(values_on.code, 0, "{}", values_on.stderr);
    assert!(values_on.stderr.is_empty(), "{}", values_on.stderr);
    // The program prints something, so that the comparison is not of two empty runs.
    assert!(
        values_on.stdout.lines().count() >= 10,
        "{}",
        values_on.stdout
    );
}

/// An array of a record holds the records inline: one allocation for the array, none for its
/// elements. A list of the same records boxes each one, because a list element is a reference.
#[test]
fn an_array_of_records_allocates_no_object_per_element() {
    if cc().is_none() {
        return;
    }
    let src = r##"
type Body(x: Float, y: Float, mass: Float)

fn step(b: Body) -> Body { Body(x: b.x + 1.0, y: b.y, mass: b.mass) }

fn body_at(bodies: Array[Body], i: Int) -> Body {
  Array.get(bodies, i) |> Option.or(Body(x: 0.0, y: 0.0, mass: 0.0))
}

fn main() {
  made = Array.from_list(List.map(List.range(0, 100), |i| Body(x: Float.from_int(i), y: 0.0, mass: 1.0)))
  print("#{Array.size(made)}")
  bodies = Array.from_list([Body(x: 0.0, y: 0.0, mass: 1.0), Body(x: 1.0, y: 1.0, mass: 2.0)])
  moved = List.fold(List.range(0, 1000), from: bodies, f: |bs, i| {
    Array.set(bs, i % 2, step(body_at(bs, i % 2)))
  })
  print("#{Array.size(moved)}")
}
"##;
    let src = format!("import Std.Array.{{Array}}\n{PRELUDE}{src}");
    let out = to_c("values-alloc", &src, true, true);
    let got = build_and_run(
        &out_dir("values-alloc"),
        &out,
        &["-O1", "-DEMELA_ALLOC_STATS"],
    );
    assert_eq!(got.code, 0, "{}", got.stderr);
    let report: Vec<&str> = got
        .stderr
        .lines()
        .skip_while(|line| !line.starts_with("emela-alloc "))
        .collect();
    assert!(!report.is_empty(), "a report");
    let text = report.join("\n");
    let count = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(&format!("emela-alloc {name} ")))
            .map(|line| line.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap())
    };
    // The two bodies of the list literal are boxed, then copied into the array; the loop reads
    // and writes records in place.
    assert_eq!(count("Body"), Some(2), "only the list boxes the records");
    assert_eq!(count("tuple"), None, "no tuple is allocated");
    assert!(
        count("Array").unwrap_or(0) <= 5,
        "the two arrays, and at most the copies of a `set`: {}",
        got.stderr
    );
    // The boxed form allocates the records of the array, so the counts above are not empty.
    let boxed = to_c("values-alloc-boxed", &src, true, false);
    let got = build_and_run(
        &out_dir("values-alloc-boxed"),
        &boxed,
        &["-O1", "-DEMELA_ALLOC_STATS"],
    );
    assert_eq!(got.code, 0, "{}", got.stderr);
    let text = got.stderr;
    let count = |name: &str| {
        text.lines()
            .find(|line| line.starts_with(&format!("emela-alloc {name} ")))
            .map(|line| line.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap())
    };
    assert_eq!(
        count("Body"),
        Some(2102),
        "the boxed form boxes the 100 `$Array.map_range` elements and every read"
    );
}
