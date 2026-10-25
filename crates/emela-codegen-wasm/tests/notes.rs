//! `show_failure` outside a fiber program (spec 10.6, 14.3). A program that suspends without
//! using fibers has no notes cell (`fiber::prepare` leaves it 0 when the fiber runtime is not
//! used); the display must not read address 0 (the scratch area) as a notes list.

mod common;

use common::*;
use wasmparser::{Operator, Parser, Payload};

/// The program waits on a host subtask, then fails: the wait writes its handle into the scratch
/// area at `WAITBUF`, which address 0 aliases.
const SUSPENDING_WITHOUT_FIBERS: &str = r#"
import Std.Clock.{Clock, SystemClock}

error Boom

fn main() {
  with SystemClock {
    (use Clock).sleep(1)
    fail Boom
  }
}
"#;

/// What the component's core module contains: the number of imported functions and the bodies of
/// its defined functions, in index order.
struct CoreModule<'a> {
    fn_imports: u32,
    bodies: Vec<Vec<Operator<'a>>>,
}

fn core_module<'a>(bytes: &'a [u8]) -> CoreModule<'a> {
    use wasmparser::{Imports, TypeRef};

    let mut fn_imports = 0u32;
    let mut bodies = Vec::new();
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
                    match group.expect("the imports parse") {
                        Imports::Single(_, import) => {
                            if matches!(import.ty, TypeRef::Func(_)) {
                                fn_imports += 1;
                            }
                        }
                        Imports::Compact1 { items, .. } => {
                            for item in items {
                                if matches!(item.expect("item").ty, TypeRef::Func(_)) {
                                    fn_imports += 1;
                                }
                            }
                        }
                        Imports::Compact2 { ty, names, .. } => {
                            if matches!(ty, TypeRef::Func(_)) {
                                fn_imports += names.into_iter().count() as u32;
                            }
                        }
                    }
                }
            }
            Payload::CodeSectionEntry(body) => {
                let mut reader = body.get_operators_reader().expect("operators");
                let mut ops = Vec::new();
                while !reader.eof() {
                    ops.push(reader.read().expect("operator"));
                }
                bodies.push(ops);
            }
            _ => {}
        }
    }
    CoreModule { fn_imports, bodies }
}

/// Whether `ops` reads address 0 as a value (`i32.const 0; i32.load`), the bug of a missing notes
/// cell: address 0 is the scratch area.
fn reads_address_zero<'a>(ops: &[Operator<'a>]) -> bool {
    ops.windows(2).any(|pair| {
        matches!(
            pair,
            [
                Operator::I32Const { value: 0 },
                Operator::I32Load { memarg }
            ] if memarg.offset == 0
        )
    })
}

#[test]
fn a_suspending_program_without_fibers_has_no_notes_list_at_address_zero() {
    let output = to_output(SUSPENDING_WITHOUT_FIBERS);
    let module = core_module(&output.module);
    // The display function: it loads the descriptor's `show` slot (offset 20) and calls it.
    let shows: Vec<&Vec<Operator<'_>>> = module
        .bodies
        .iter()
        .filter(|ops| {
            ops.iter()
                .any(|op| matches!(op, Operator::I32Load { memarg } if memarg.offset == 20))
                && ops
                    .iter()
                    .any(|op| matches!(op, Operator::CallIndirect { .. }))
        })
        .collect();
    assert!(!shows.is_empty(), "show_failure was not emitted");
    for show in shows {
        for op in show {
            let Operator::Call { function_index } = op else {
                continue;
            };
            if *function_index < module.fn_imports {
                continue;
            }
            let body = &module.bodies[(*function_index - module.fn_imports) as usize];
            assert!(
                !reads_address_zero(body),
                "show_failure calls function {function_index}, which reads address 0 as the notes list"
            );
        }
    }
}

#[test]
fn a_suspending_program_without_fibers_displays_its_error() {
    let Some(_) = wasmtime() else { return };
    let bytes = compile_checked(SUSPENDING_WITHOUT_FIBERS);
    let run = run_bytes(&out_dir("notes-no-fibers"), &bytes);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(run.stderr, "error: Boom\n");
}

/// A test whose failure has a note, and whose error's `show` raises a defect: the test result must
/// not append the note to a message while the defect is pending (spec 14.3).
#[test]
fn a_failing_show_does_not_turn_notes_into_a_test_message() {
    let src = r#"
import Std.Fiber.{Fiber, Scope, Exit, Success, Failure}
import Std.Cause.{Cause}

error Boom(text: String)

impl Boom: Show {
  fn show(self) { panic("show failed") }
}

effect Tick {
  suspend fn tick() -> ()
}

handler Ticks: Tick {
  suspend fn tick(resume) { resume(()) }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { panic("second") }
}

fn spin(n: Int) -> () use { Tick } {
  if n == 0 { () } else {
    (use Tick).tick()
    spin(n - 1)
  }
}

fn once() -> Exit[(), Boom] use { Async } {
  with Ticks {
    Fiber.attempt(fn() {
      Fiber.scope(fn() {
        Fiber.fork(fn() {
          spin(1000)
          fail Boom(text: "boom")
        })
        Fiber.fork(fn() { with Res { spin(100000) } })
        spin(100000)
        ()
      })
    })
  }
}

@test
fn failing_show() -> () fails Boom use { Async } {
  once() match {
    Success(_) -> ()
    Failure(c) -> Cause.raise(c)
  }
}
"#;
    let bytes = to_wasm_tests(src, &["failing_show"])
        .unwrap_or_else(|e| panic!("WASM compilation failed: {e:#?}"));
    wasmparser::validate(&bytes).unwrap_or_else(|e| panic!("the module is not valid: {e}"));
    let run = run_bytes(&out_dir("notes-failing-show"), &bytes);
    // The defect from `show` is the run's result; the test line was already written.
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(
        !run.stdout.contains("note: another fiber also failed"),
        "{}",
        run.stdout
    );
}
