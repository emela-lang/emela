//! Freed objects go back to the allocator: a program that keeps making and dropping the same
//! objects runs in a fixed amount of linear memory. Each program here runs under a memory limit
//! far below what it allocates in total. Skipped without `wasmtime`.
//!
//! A program that does exhaust its memory ends abruptly. The runtime checks the result of
//! `memory.grow` and traps before moving the bump pointer, so the end of the program is not an
//! out-of-bounds store past the memory.

mod common;

use common::*;
use wasmparser::{Operator, Parser, Payload};

/// The linear memory each program may grow to: six 64 KiB pages.
const LIMIT: u32 = 6 * 65536;

/// Runs `src` with linear memory limited to [`LIMIT`], expecting it to finish with `stderr`.
fn run_limited(name: &str, src: &str, stderr: &str) {
    let Some(_) = wasmtime() else { return };
    let bytes = compile_checked(src);
    let run = run_bytes_with_command(&out_dir(name), &bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
    });
    assert_eq!(run.code, 0, "{name}: {}", run.stderr);
    assert_eq!(run.stderr, stderr, "{name}");
}

#[test]
fn dropping_a_closure_frees_what_it_captured() {
    // Each round drops a closure whose environment holds a list of 100 cells: the cells are freed
    // after the environment, not only the environment itself.
    run_limited(
        "memory-closure",
        r#"
fn make(n: Int) -> fn() -> Int {
  xs = List.map(List.range(0, n), fn(i) { i * 2 })
  fn() { List.length(xs) }
}

fn rounds(n: Int, total: Int) -> Int {
  if n == 0 {
    total
  } else {
    f = make(100)
    rounds(n - 1, total + f())
  }
}

fn main() {
  dbg(rounds(2000, 0))
}
"#,
        "200000\n",
    );
}

#[test]
fn a_finished_operation_call_frees_its_record() {
    // A `suspend fn` operation call makes a perform record, its resume function and the name of
    // the operation for diagnostics; all of them are freed once the call returns.
    run_limited(
        "memory-operation",
        r#"
effect Retry {
  suspend fn go() -> Int
}

handler Retrying: Retry {
  suspend fn go(resume) {
    resume(1)
  }
}

fn ten() -> Int use { Retry } {
  (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go() + (use Retry).go()
}

fn rounds(n: Int, total: Int) -> Int use { Retry } {
  if n == 0 {
    total
  } else {
    rounds(n - 1, total + ten())
  }
}

fn main() {
  with Retrying {
    dbg(rounds(1000, 0))
  }
}
"#,
        "10000\n",
    );
}

#[test]
fn a_sleeping_operation_call_reuses_its_memory() {
    // A call that sleeps also makes frames and a queued continuation, of other sizes than the
    // perform record; a block reused for a smaller object is given back whole.
    run_limited(
        "memory-sleeping-operation",
        r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

effect Retry {
  suspend fn go() -> Int
}

handler Retrying: Retry {
  suspend fn go(resume) {
    (use Clock).sleep(1)
    resume(1)
  }
}

fn rounds(n: Int, total: Int) -> Int use { Retry } {
  if n == 0 {
    total
  } else {
    rounds(n - 1, total + (use Retry).go())
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      with Retrying {
        dbg(rounds(4000, 0))
      }
    }
  })
}
"#,
        "4000\n",
    );
}

#[test]
fn attempts_free_their_exits_and_causes() {
    // Each round makes an `Exit` of every kind, a `Cause` with its error or defect and the text of
    // the cause, in place and across a suspension (the attempt's frame); all of them are freed.
    run_limited(
        "memory-attempt",
        r#"
import Std.Fiber.{Fiber, Exit, Success, Failure}
import Std.Cause.{Cause}
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

error Boom(text: String) derive Show

fn length[A, E](e: Exit[A, E]) -> Int {
  e match {
    Success(_) -> 1
    Failure(c) -> String.length(Cause.to_string(c))
  }
}

fn round(i: Int) -> Int use { Clock } {
  a = length(Fiber.attempt(fn() { "value #{i}" }))
  b = length(Fiber.attempt(fn() { fail Boom(text: "boom #{i}") }))
  c = length(Fiber.attempt(fn() { panic("bad #{i}") }))
  d = length(Fiber.attempt(fn() {
    (use Clock).sleep(1)
    fail Boom(text: "late #{i}")
  }))
  e = Fiber.attempt(fn() { fail Boom(text: "again #{i}") }) match {
    Success(_) -> 0
    Failure(cause) -> Cause.raise(cause)
  } escape { Boom(text) -> String.length(text) }
  a + b + c + d + e
}

fn rounds(n: Int, total: Int) -> Int use { Clock } {
  if n == 0 { total } else { rounds(n - 1, total + round(n % 10)) }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  State.run(start, fn() {
    with TestClock {
      dbg(rounds(3000, 0))
    }
  })
}
"#,
        "225000\n",
    );
}

/// The core module inside the component: its function imports in index order, the operators of
/// each defined function body, the initial `i32` value of each global, and the static data.
struct CoreModule<'a> {
    imports: Vec<(&'a str, &'a str)>,
    bodies: Vec<Vec<Operator<'a>>>,
    globals: Vec<Option<i32>>,
    data: Vec<u8>,
}

/// The value of a `ConstExpr` that is a single `i32.const`.
fn const_i32(init: &wasmparser::ConstExpr<'_>) -> Option<i32> {
    match init.get_operators_reader().into_iter().next()?.ok()? {
        Operator::I32Const { value } => Some(value),
        _ => None,
    }
}

/// Appends the function imports of one import group, in index order.
fn push_imports<'a>(imports: &mut Vec<(&'a str, &'a str)>, group: wasmparser::Imports<'a>) {
    match group {
        wasmparser::Imports::Single(_, import) => {
            if matches!(import.ty, wasmparser::TypeRef::Func(_)) {
                imports.push((import.module, import.name));
            }
        }
        wasmparser::Imports::Compact1 { module, items } => {
            for item in items {
                let item = item.expect("the import parses");
                if matches!(item.ty, wasmparser::TypeRef::Func(_)) {
                    imports.push((module, item.name));
                }
            }
        }
        wasmparser::Imports::Compact2 { module, ty, names } => {
            if matches!(ty, wasmparser::TypeRef::Func(_)) {
                for name in names {
                    imports.push((module, name.expect("the import name parses")));
                }
            }
        }
    }
}

fn core_module<'a>(bytes: &'a [u8]) -> CoreModule<'a> {
    let mut imports = Vec::new();
    let mut bodies = Vec::new();
    let mut globals = Vec::new();
    let mut data = None;
    let mut in_main = false;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("wasm should parse") {
            // The component nests its main module first; the shim and fixup modules after it have
            // their own index spaces.
            Payload::ModuleSection { .. } => in_main = true,
            Payload::End(_) if in_main => break,
            _ if !in_main => {}
            Payload::ImportSection(section) => {
                for group in section {
                    push_imports(&mut imports, group.expect("the imports parse"));
                }
            }
            Payload::GlobalSection(section) => {
                for global in section {
                    globals.push(const_i32(&global.expect("the global parses").init_expr));
                }
            }
            Payload::DataSection(section) => {
                let segment = section
                    .into_iter()
                    .next()
                    .expect("the data section has a segment")
                    .expect("the data segment parses");
                data = Some(segment.data.to_vec());
            }
            Payload::CodeSectionEntry(body) => bodies.push(
                body.get_operators_reader()
                    .expect("the body parses")
                    .into_iter()
                    .map(|op| op.expect("the operator parses"))
                    .collect(),
            ),
            _ => {}
        }
    }
    CoreModule {
        imports,
        bodies,
        globals,
        data: data.expect("the core module has a data section"),
    }
}

#[test]
fn heap_reservation_checks_the_grow_and_traps_before_moving_the_heap() {
    // An allocation failure is an abrupt end of the program, not a defect. The bump pointer must
    // not move past the end of the memory, or the next store traps with an unrelated
    // out-of-bounds error instead of the runtime's own trap.
    let output = to_output("fn main() { dbg(1) }\n");
    wasmparser::validate(&output.module).expect("the module is valid");
    let core = core_module(&output.module);
    let heap_body = core
        .bodies
        .iter()
        .position(|ops| {
            ops.iter()
                .any(|op| matches!(op, Operator::MemoryGrow { .. }))
        })
        .expect("the runtime reserves the heap with `memory.grow`");
    let ops = &core.bodies[heap_body];

    // `write_stderr` is the function that calls the stderr stream's `write-via-stream`.
    let stderr_write = core
        .imports
        .iter()
        .position(|(module, name)| module.contains("stderr") && *name == "write-via-stream")
        .expect("the runtime imports the stderr stream") as u32;
    let write_stderr = u32::try_from(
        core.bodies
            .iter()
            .position(|body| {
                body.iter()
                    .any(|op| matches!(op, Operator::Call { function_index } if *function_index == stderr_write))
            })
            .expect("the runtime has a wrapper for the stderr stream"),
    )
    .unwrap()
        + core.imports.len() as u32;

    let grow = ops
        .iter()
        .position(|op| matches!(op, Operator::MemoryGrow { .. }))
        .expect("the reservation grows the memory");
    assert_eq!(
        ops.get(grow + 1),
        Some(&Operator::I32Const { value: -1 }),
        "the result of `memory.grow` is compared with -1"
    );
    assert_eq!(ops.get(grow + 2), Some(&Operator::I32Eq));
    assert!(matches!(ops.get(grow + 3), Some(Operator::If { .. })));

    // The failure branch writes the message through `write_stderr(ptr, len)`.
    let (ptr, len, callee) = match (ops.get(grow + 4), ops.get(grow + 5), ops.get(grow + 6)) {
        (
            Some(Operator::I32Const { value: ptr }),
            Some(Operator::I32Const { value: len }),
            Some(Operator::Call { function_index }),
        ) => (*ptr, *len, *function_index),
        other => panic!("the failed grow does not write the message: {other:?}"),
    };
    assert_eq!(callee, write_stderr, "the failure is not written to stderr");
    assert_eq!(
        usize::try_from(len).expect("the length is not negative"),
        b"emela: out of memory\n".len()
    );
    let start = usize::try_from(ptr).expect("the pointer is not negative");
    assert_eq!(
        &core.data[start..start + len as usize],
        b"emela: out of memory\n",
        "the message is not at the pointer the failure passes"
    );

    let trap = grow
        + 4
        + ops[grow + 4..]
            .iter()
            .position(|op| matches!(op, Operator::Unreachable))
            .expect("a failed grow traps");
    // `G_HEAP` is the global initialised to `heap_base`.
    let heap_global = u32::try_from(
        core.globals
            .iter()
            .position(|init| *init == Some(output.layout.heap_base as i32))
            .expect("a global holds `heap_base`"),
    )
    .unwrap();
    assert!(
        ops[grow..trap]
            .iter()
            .all(|op| !matches!(op, Operator::GlobalSet { .. })),
        "a global moves in the failed grow path: {ops:?}"
    );
    let sets: Vec<u32> = ops[trap..]
        .iter()
        .filter_map(|op| match op {
            Operator::GlobalSet { global_index } => Some(*global_index),
            _ => None,
        })
        .collect();
    assert_eq!(
        sets,
        vec![heap_global],
        "the reservation moves a different global after the trap: {ops:?}"
    );
}

#[test]
fn exhausting_the_memory_ends_the_program_abruptly() {
    let Some(_) = wasmtime() else { return };
    // A million cells do not fit in 1 MiB, and the list is never dropped.
    let bytes = compile_checked(
        r#"
fn main() {
  xs = List.range(0, 1000000)
  dbg(List.length(xs))
}
"#,
    );
    let run = run_bytes_with_command(&out_dir("memory-exhaustion"), &bytes, |command| {
        command.arg("-W").arg("max-memory-size=1048576");
    });
    // The exit code and the trap text are implementation-defined. What the backend guarantees is
    // that the program does not finish, and that the failure comes from its own check.
    assert_ne!(run.code, 0, "the program finished: {}", run.stderr);
    assert_eq!(run.stdout, "");
    assert!(
        run.stderr.contains("emela: out of memory"),
        "the failure is not reported by the runtime: {}",
        run.stderr
    );
}

#[test]
fn fiber_attempt_does_not_catch_an_allocation_failure() {
    let Some(_) = wasmtime() else { return };
    // The attempt cannot make the program continue: the failure is not a defect or an error value
    // and the runtime traps. Nothing after the attempt runs.
    let bytes = compile_checked(
        r#"
import Std.Fiber.{Fiber}

fn main() {
  e = Fiber.attempt(fn() {
    xs = List.range(0, 1000000)
    dbg(List.length(xs))
  })
  dbg("after")
}
"#,
    );
    let run = run_bytes_with_command(&out_dir("memory-exhaustion-attempt"), &bytes, |command| {
        command.arg("-W").arg("max-memory-size=1048576");
    });
    assert_ne!(run.code, 0, "the program finished: {}", run.stderr);
    assert!(
        run.stderr.contains("emela: out of memory"),
        "the failure is not reported by the runtime: {}",
        run.stderr
    );
    assert!(
        !run.stderr.contains("after"),
        "the attempt let the program continue: {}",
        run.stderr
    );
}
