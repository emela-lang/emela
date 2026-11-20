//! E1 step 4: the flattened ABI of suspending code.
//!
//! A suspending function has one signature everywhere it appears: its ordinary wrapper, the state
//! machine body (its parameters then the frame pointer), its trampoline (the frame pointer alone),
//! a direct call, a closure built from it and a `call_indirect` of its type. An eligible record or
//! tuple parameter is its scalar leaves in all of them. A suspending effect operation's parameters
//! flatten the same way, because its implementation is a lifted closure of the operation's function
//! type and is called with the handler, the parameters and the resume function.
//!
//! A variable held by value that is live across a suspension is saved as one frame slot per scalar
//! leaf, in field order. The frame's descriptor marks those slots inert (they own no reference), and
//! the restore path reads them back in the same order.
//!
//! Two results stay boxed, and the tests assert the boxed form byte for byte:
//!
//! - a suspending function's result of more than one leaf, and every suspending operation's result.
//!   The runtime's resume channel carries one `i64` word selected by representation
//!   (`out_mode`/`out_word` for a function, `perform_new`/`perform_end` and the `resume_<repr>`
//!   thunk for an operation); a one-leaf function result is that word.
//! - a record passed to a fiber operation. Its thunk (`runtime::fiber::thunk`) is chosen by
//!   representation and carries one result word, so the type of the function value and its result
//!   stay boxed.
//!
//! The option is off by default; with it off the boxed form of every program here is unchanged.

mod common;

#[allow(clippy::duplicate_mod)] // `common` includes the same Core test support.
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use common::*;
use emela_codegen_wasm::{Entry, WasmOptions, compile_with_wasm_options};
use emela_core::Roots;
use emela_lir::opt::{LirRules, Options};
use wasmparser::{Operator, Parser, Payload, ValType};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sig {
    params: Vec<ValType>,
    results: Vec<ValType>,
}

/// The signatures of the core module nested in the component: every function, imports first, and
/// the type of every `call_indirect`.
struct Module {
    types: Vec<Sig>,
    functions: Vec<Sig>,
    indirect: Vec<Sig>,
}

fn module(bytes: &[u8]) -> Module {
    let mut types: Vec<Sig> = Vec::new();
    let mut import_tys: Vec<u32> = Vec::new();
    let mut defined_tys: Vec<u32> = Vec::new();
    let mut indirect_indices = Vec::new();
    let mut in_main = false;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("wasm should parse") {
            Payload::ModuleSection { .. } => in_main = true,
            Payload::End(_) if in_main => break,
            _ if !in_main => {}
            Payload::TypeSection(section) => {
                for group in section {
                    for ty in group.expect("the type group parses").into_types() {
                        match ty.composite_type.inner {
                            wasmparser::CompositeInnerType::Func(f) => types.push(Sig {
                                params: f.params().to_vec(),
                                results: f.results().to_vec(),
                            }),
                            _ => types.push(Sig {
                                params: Vec::new(),
                                results: Vec::new(),
                            }),
                        }
                    }
                }
            }
            Payload::ImportSection(section) => {
                for group in section {
                    match group.expect("the import group parses") {
                        wasmparser::Imports::Single(_, import) => {
                            if let wasmparser::TypeRef::Func(ty) = import.ty {
                                import_tys.push(ty);
                            }
                        }
                        wasmparser::Imports::Compact1 { items, .. } => {
                            for item in items {
                                let item = item.expect("the import parses");
                                if let wasmparser::TypeRef::Func(ty) = item.ty {
                                    import_tys.push(ty);
                                }
                            }
                        }
                        wasmparser::Imports::Compact2 { ty, names, .. } => {
                            if let wasmparser::TypeRef::Func(ty) = ty {
                                for _ in names {
                                    import_tys.push(ty);
                                }
                            }
                        }
                    }
                }
            }
            Payload::FunctionSection(section) => {
                for ty in section {
                    defined_tys.push(ty.expect("the function type parses"));
                }
            }
            Payload::CodeSectionEntry(body) => {
                let ops = body
                    .get_operators_reader()
                    .expect("the body parses")
                    .into_iter();
                for op in ops {
                    if let Operator::CallIndirect { type_index, .. } =
                        op.expect("the operator parses")
                    {
                        indirect_indices.push(type_index);
                    }
                }
            }
            _ => {}
        }
    }
    let functions = import_tys
        .into_iter()
        .chain(defined_tys)
        .map(|i| types[i as usize].clone())
        .collect();
    let indirect = indirect_indices
        .into_iter()
        .map(|i| types[i as usize].clone())
        .collect();
    Module {
        types,
        functions,
        indirect,
    }
}

/// Lowers `src` with the Core IR optimisations off (so function boundaries stay visible) and
/// compiles it with `values` and the LIR passes.
fn compile(name: &str, src: &str, values: bool) -> Vec<u8> {
    compile_with(name, src, values, true)
}

fn compile_with(name: &str, src: &str, values: bool, optimize: bool) -> Vec<u8> {
    let m = support::try_lower_off(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    let out = compile_with_wasm_options(
        &m,
        &Entry::Main,
        optimize,
        Options {
            rules: LirRules::default(),
        },
        WasmOptions {
            values,
            ..WasmOptions::default()
        },
    )
    .unwrap_or_else(|e| panic!("{name}: WASM compilation failed: {e:#?}"));
    wasmparser::validate(&out.module).unwrap_or_else(|e| {
        panic!("{name}: the module is not valid: {e}");
    });
    out.module
}

fn compile_pair(name: &str, src: &str) -> (Vec<u8>, Vec<u8>) {
    (
        compile(&format!("{name}-off"), src, false),
        compile(&format!("{name}-on"), src, true),
    )
}

/// Runs both forms and expects the same observable output.
fn same_output(name: &str, off: &[u8], on: &[u8], stdout: &str, stderr: &str) {
    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir(&format!("{name}-off")), off);
    let on_run = run_bytes(&out_dir(&format!("{name}-on")), on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "{name}: the by-value form changed what the program prints"
    );
    assert_eq!(on_run.code, 0, "{name}: {}", on_run.stderr);
    assert_eq!(on_run.stdout, stdout, "{name}: stdout");
    assert_eq!(on_run.stderr, stderr, "{name}: stderr");
}

/// A suspending function taking a two-leaf record and returning a one-leaf record, called directly
/// and through a function value, and a nested suspending call. `Meter` (one leaf) may flatten as a
/// result because the resume channel carries one word; `Point` is a parameter only.
const SUSPENDING_FUNCTION: &str = r#"
type Point(x: Int, f: Float)
type Meter(v: Int)

effect Wait {
  suspend fn wait() -> Int
}

handler Waiter: Wait {
  suspend fn wait(resume) {
    resume(2)
  }
}

fn shift(p: Point) -> Meter uses { Wait } {
  n = (use Wait).wait()
  Meter(v: p.x + 3 + n)
}

fn same(p: Point) -> Point { p }

fn rebuild(p: Point) -> Meter uses { Wait } {
  (use Wait).wait()
  q = same(Point(x: p.x + 1, f: p.f + 0.5))
  Meter(v: q.x + 1)
}

fn twice(p: Point) -> Meter uses { Wait } {
  m = shift(p)
  Meter(v: m.v + shift(Point(x: 1, f: 1.0)).v)
}

fn main() -> () uses { Async } {
  with Waiter {
    p = Point(x: 1, f: 3.0)
    m = shift(p)
    dbg(m.v)
    f = shift
    dbg(f(Point(x: 2, f: 4.0)).v)
    dbg(twice(p).v)
    dbg(rebuild(p).v)
    base = 10
    g = |q: Point| {
      n = (use Wait).wait()
      q.x + 3 + n + base
    }
    dbg(g(Point(x: 1, f: 2.0)))
    ()
  }
}
"#;

const SUSPENDING_FUNCTION_OUT: &str = "6\n7\n12\n3\n16\n";

/// The wrapper and body of `shift`: `(Point, evidence) -> Meter`. The wrapper's parameters are the
/// record's two leaves (`i32`, `f64`) then the evidence table; the state-machine body takes the
/// same then the frame pointer.
const SHIFT_WRAPPER: ([ValType; 3], [ValType; 1]) =
    ([ValType::I32, ValType::F64, ValType::I32], [ValType::I32]);
const SHIFT_BODY: ([ValType; 4], [ValType; 1]) = (
    [ValType::I32, ValType::F64, ValType::I32, ValType::I32],
    [ValType::I32],
);

#[test]
fn a_suspending_function_flattens_its_wrapper_body_and_calls() {
    let (off, on) = compile_pair("suspend-function", SUSPENDING_FUNCTION);
    assert_ne!(off, on, "the option must change a suspending signature");

    let off_module = module(&off);
    let on_module = module(&on);
    let has = |m: &Module, params: &[ValType], results: &[ValType]| {
        m.functions
            .iter()
            .any(|s| s.params == params && s.results == results)
    };
    assert!(
        !has(&off_module, &SHIFT_WRAPPER.0, &SHIFT_WRAPPER.1),
        "the boxed wrapper is one reference and the evidence table"
    );
    assert!(
        !has(&off_module, &SHIFT_BODY.0, &SHIFT_BODY.1),
        "the boxed body has the record as one reference"
    );
    assert!(
        has(&on_module, &SHIFT_WRAPPER.0, &SHIFT_WRAPPER.1),
        "the flattened wrapper is the record's leaves, the evidence table and the one-leaf result"
    );
    assert!(
        has(&on_module, &SHIFT_BODY.0, &SHIFT_BODY.1),
        "the state-machine body takes the leaves, the evidence table and the frame pointer"
    );
    // A closure built from the suspending function has the same parameters after its environment.
    assert!(
        on_module.indirect.iter().any(|s| s.params
            == [ValType::I32, ValType::I32, ValType::F64, ValType::I32]
            && s.results == [ValType::I32]),
        "the lifted closure and its indirect call agree: {:?}",
        on_module.indirect
    );
    same_output("suspend-function", &off, &on, "", SUSPENDING_FUNCTION_OUT);
}

/// A suspending function whose result has two leaves stays boxed: the trampoline reports the result
/// through the one-word resume channel.
const SUSPENDING_RESULT: &str = r#"
type Pair(a: Int, b: Int)

effect Wait {
  suspend fn wait() -> Int
}

handler Waiter: Wait {
  suspend fn wait(resume) {
    resume(1)
  }
}

fn pair() -> Pair uses { Wait } {
  n = (use Wait).wait()
  Pair(a: n, b: n + 1)
}

fn main() -> () uses { Async } {
  with Waiter {
    p = pair()
    dbg(p.a)
    dbg(p.b)
    ()
  }
}
"#;

#[test]
fn a_multi_leaf_suspending_result_keeps_the_box() {
    let (off, on) = compile_pair("suspend-result", SUSPENDING_RESULT);
    assert_eq!(
        off, on,
        "a suspending result of two leaves has no one-word channel: it must stay boxed"
    );
    same_output("suspend-result", &off, &on, "", "1\n2\n");
}

/// Records in a suspending function whose handler resumes in place: a two-leaf parameter, a local
/// built before the call, and a record of mixed leaf representations. The function's signature and
/// the locals follow the layouts; the real saved-frame path is in `SAVED_FRAME`.
const SUSPENDED_VALUES: &str = r#"
type Point(x: Int, y: Int)
type Mix(n: Int, f: Float, big: Int64)

effect Wait {
  suspend fn wait() -> Int
}

handler Waiter: Wait {
  suspend fn wait(resume) {
    resume(3)
  }
}

fn work(p: Point) -> Int uses { Wait } {
  n = (use Wait).wait()
  p.x + p.y + n
}

fn local() -> Int uses { Wait } {
  p = Point(x: 4, y: 5)
  n = (use Wait).wait()
  p.x + p.y + n
}

fn mixed(m: Mix) -> Int uses { Wait } {
  n = (use Wait).wait()
  dbg(m.f)
  m.n + Int64.to_int(m.big) + n
}

fn main() -> () uses { Async } {
  with Waiter {
    dbg(work(Point(x: 3, y: 4)))
    dbg(local())
    dbg(mixed(Mix(n: 1, f: 0.5, big: Int64.from_int(2))))
    ()
  }
}
"#;

#[test]
fn a_record_in_a_suspending_function_flattens() {
    let (off, on) = compile_pair("suspend-saved", SUSPENDED_VALUES);
    assert_ne!(off, on, "the option must flatten the saved record");

    // `mixed` has a parameter of mixed leaf types (`i32`, `f64`, `i64`) and the evidence table: the
    // state-machine body takes the three leaves, the evidence table and the frame pointer. The
    // saved record's leaves are frame slots of those types, in the same order.
    let on_module = module(&on);
    let body = [
        ValType::I32,
        ValType::F64,
        ValType::I64,
        ValType::I32,
        ValType::I32,
    ];
    assert!(
        on_module
            .functions
            .iter()
            .any(|s| s.params == body && s.results == [ValType::I32]),
        "the flattened state machine takes the record's leaves: {:?}",
        on_module.types
    );
    same_output("suspend-saved", &off, &on, "", "10\n12\n0.5\n6\n");
}

/// A record saved across a suspension that actually happens: `TestClock.sleep` parks the caller and
/// resumes it later, so the state machine captures a frame with the record. The parameter, a local
/// built before the sleep, and a record of mixed leaves are all saved by leaf.
const SAVED_FRAME: &str = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

type Point(x: Int, y: Int)
type Mix(n: Int, f: Float, big: Int64)
type Nested(p: Point, n: Int)

fn work(p: Point) -> Int uses { Clock } {
  (use Clock).sleep(10)
  p.x + p.y
}

fn local() -> Int uses { Clock } {
  m = Mix(n: 4, f: 0.5, big: Int64.from_int(2))
  (use Clock).sleep(10)
  m.n + Int64.to_int(m.big)
}

fn nested() -> Int uses { Clock } {
  q = Nested(p: Point(x: 1, y: 2), n: 3)
  (use Clock).sleep(10)
  q.p.x + q.p.y + q.n
}

fn rounds(n: Int, total: Int) -> Int uses { Clock } {
  if n == 0 {
    total
  } else {
    rounds(n - 1, total + work(Point(x: 1, y: 2)) + local() + nested())
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, || {
    with TestClock {
      dbg(rounds(1000, 0))
    }
  })
  dbg(Instant.to_unix_milliseconds(end))
}
"#;

#[test]
fn a_frame_saved_at_a_real_suspension_restores_its_leaves() {
    let (off, on) = compile_pair("suspend-frame", SAVED_FRAME);
    assert_ne!(off, on, "the option must flatten the saved records");
    same_output("suspend-frame", &off, &on, "", "15000\n30000\n");
    // The frames are allocated and released every round: 1000 rounds in one 64 KiB page means no
    // saved frame and no record box is leaked.
    let Some(_) = wasmtime() else { return };
    let limited = run_bytes_with_command(&out_dir("suspend-frame-limited"), &on, |command| {
        command.arg("-W").arg("max-memory-size=65536");
        command.arg("-W").arg("trap-on-grow-failure=n");
    });
    assert_eq!(limited.code, 0, "{}", limited.stderr);
    assert_eq!(limited.stderr, "15000\n30000\n");
}

/// A continuation dropped while suspended: the saved record's leaves own nothing, so freeing the
/// frame runs no release and leaks nothing; the `with` releases still run in order.
const DROPPED_CONTINUATION: &str = r#"
type Point(x: Int, y: Int)

effect Wait {
  suspend fn wait() -> Int
}

handler Ignores: Wait {
  suspend fn wait(resume) {
    dbg("not resuming")
    ()
  }
}

effect Name {
  fn name() -> String
}

handler Res: Name {
  init { Res }
  fn name() { "res" }
  release { dbg("release res") }
}

fn waits(p: Point) -> Int uses { Wait } {
  n = (use Wait).wait()
  p.x + p.y + n
}

fn main() -> () uses { Async } {
  with Res {
    with Ignores {
      dbg("never #{waits(Point(x: 3, y: 4))}")
    }
    dbg("after with")
  }
  dbg("done")
  ()
}
"#;

#[test]
fn a_dropped_continuation_releases_a_saved_record() {
    let (off, on) = compile_pair("suspend-dropped", DROPPED_CONTINUATION);
    let Some(_) = wasmtime() else { return };
    let a = run_bytes(&out_dir("suspend-dropped-off"), &off);
    let b = run_bytes(&out_dir("suspend-dropped-on"), &on);
    assert_eq!(a.code, 130, "{}", a.stderr);
    assert_eq!(b.code, 130, "{}", b.stderr);
    assert_eq!(
        (&a.stdout, &a.stderr),
        (&b.stdout, &b.stderr),
        "the by-value form changed the unwinding"
    );
    assert_eq!(b.stderr, "\"not resuming\"\n\"release res\"\ninterrupted\n");
}

/// The ABI of a suspending effect operation. The implementation is a closure of the operation's
/// function type, so a record parameter flattens there too; the result stays boxed because it comes
/// back through `perform_new`/`perform_end` as one word.
const SUSPENDING_OPERATION: &str = r#"
type Point(x: Int, y: Int)

effect Ask {
  suspend fn ask(p: Point) -> Int
}

handler Asker: Ask {
  suspend fn ask(p, resume) {
    resume(p.x + p.y)
  }
}

fn main() -> () uses { Async } {
  with Asker {
    dbg((use Ask).ask(Point(x: 1, y: 3)))
    ()
  }
}
"#;

#[test]
fn a_suspending_operation_flattens_its_parameters() {
    let (off, on) = compile_pair("suspend-operation", SUSPENDING_OPERATION);
    assert_ne!(off, on, "the option must flatten the operation's parameter");

    // The implementation closure is called with the handler, the two leaves and the resume
    // function, and returns nothing (the value comes from the resume).
    let expected = (
        [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32; 0],
    );
    let off_module = module(&off);
    let on_module = module(&on);
    assert!(
        !off_module
            .indirect
            .iter()
            .any(|s| s.params == expected.0 && s.results.is_empty()),
        "the boxed operation call is the handler, one reference and the resume function"
    );
    assert!(
        on_module
            .indirect
            .iter()
            .any(|s| s.params == expected.0 && s.results.is_empty()),
        "the operation is called with the handler, the leaves and the resume function: {:?}",
        on_module.indirect
    );
    same_output("suspend-operation", &off, &on, "", "4\n");
}

/// A suspending operation whose implementation itself suspends: the record parameter flattens into
/// the implementation closure and is saved in the implementation's frame across its sleep.
const SUSPENDING_OPERATION_IMPL: &str = r#"
import Std.Clock.{Clock, TestClock}
import Std.Instant.{Instant}
import Std.State.{State}

type Point(x: Int, y: Int)

effect Slow {
  suspend fn slow(p: Point) -> Int
}

handler Slowpoke: Slow {
  suspend fn slow(p, resume) {
    (use Clock).sleep(10)
    resume(p.x + p.y)
  }
}

fn main() {
  start = Instant.from_unix_milliseconds(Int64.from_int(0))
  (_, end) = State.run(start, || {
    with Slowpoke, TestClock {
      dbg((use Slow).slow(Point(x: 2, y: 3)))
    }
  })
  dbg(Instant.to_unix_milliseconds(end))
}
"#;

#[test]
fn a_suspending_operation_implementation_saves_its_record() {
    let (off, on) = compile_pair("suspend-operation-impl", SUSPENDING_OPERATION_IMPL);
    assert_ne!(
        off, on,
        "the option must flatten the implementation's record"
    );
    // The implementation closure takes its environment (the handler), the record's two leaves and
    // the resume function, and returns nothing.
    let on_module = module(&on);
    assert!(
        on_module.indirect.iter().any(|s| s.params
            == [ValType::I32, ValType::I32, ValType::I32, ValType::I32]
            && s.results.is_empty()),
        "the implementation is called with the record's leaves: {:?}",
        on_module.indirect
    );
    same_output("suspend-operation-impl", &off, &on, "", "5\n10\n");
}

/// A suspending operation taking and returning a record stays boxed: its result crosses the
/// one-word resume channel, so the whole type is excluded (the recorded limitation).
const SUSPENDING_OPERATION_RESULT: &str = r#"
type Point(x: Int, y: Int)

effect Ask {
  suspend fn ask(p: Point) -> Point
}

handler Asker: Ask {
  suspend fn ask(p, resume) {
    resume(Point(x: p.x + 1, y: p.y + 2))
  }
}

fn main() -> () uses { Async } {
  with Asker {
    q = (use Ask).ask(Point(x: 1, y: 2))
    dbg(q.x)
    dbg(q.y)
    ()
  }
}
"#;

#[test]
fn a_suspending_operation_record_result_stays_boxed() {
    let (off, on) = compile_pair("suspend-operation-result", SUSPENDING_OPERATION_RESULT);
    assert_eq!(
        off, on,
        "a suspending operation's result has no one-word channel: the type must stay boxed"
    );
    same_output("suspend-operation-result", &off, &on, "", "2\n4\n");
}

/// `Exit.attempt` calls the closure through a fiber thunk, which carries one result word selected
/// by representation: a record result stays boxed.
const ATTEMPT: &str = r#"
import Std.Exit.{Exit, Success, Failure}
import Std.Cause.{Cause}

type Point(x: Int, y: Int)

fn main() -> () uses { Async } {
  e = Exit.attempt(|| Point(x: 1, y: 2))
  e match {
    Success(p) -> {
      dbg(p.x + p.y)
      ()
    }
    Failure(c) -> {
      dbg(Cause.to_string(c))
      ()
    }
  }
  ()
}
"#;

#[test]
fn a_fiber_attempt_record_result_stays_boxed() {
    let (off, on) = compile_pair("suspend-attempt", ATTEMPT);
    assert_eq!(
        off, on,
        "a record crossing a fiber thunk must stay boxed: the thunk carries one word"
    );
    same_output("suspend-attempt", &off, &on, "", "3\n");
}

/// The memory each program may grow to: one 64 KiB page. The module's static data fits in it, so
/// the heap starts in the same page.
const LIMIT: u32 = 65536;

/// A recursion of suspending calls that keeps a ten-leaf record live across each call. The handler
/// resumes the operation in place, so no frame is captured: the boxed form allocates and keeps one
/// record box per live recursion level, the by-value form keeps its leaves in wasm locals.
const RECURSION: &str = r#"
type Wide(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int, j: Int)

effect Wait {
  suspend fn wait() -> Int
}

handler Waiter: Wait {
  suspend fn wait(resume) {
    resume(1)
  }
}

fn walk(n: Int, w: Wide) -> Int uses { Wait } {
  if n == 0 {
    (use Wait).wait()
    w.a + w.b + w.c + w.d + w.e + w.f + w.g + w.h + w.i + w.j
  } else {
    r = walk(n - 1, Wide(a: w.a, b: w.b, c: w.c, d: w.d, e: w.e, f: w.f, g: w.g, h: w.h, i: w.i, j: w.j))
    w.a + w.b + w.c + w.d + w.e + w.f + w.g + w.h + w.i + w.j + r
  }
}

fn main() -> () uses { Async } {
  with Waiter {
    dbg(walk(800, Wide(a: 1, b: 1, c: 1, d: 1, e: 1, f: 1, g: 1, h: 1, i: 1, j: 1)))
    ()
  }
}
"#;

fn run_limited(name: &str, bytes: &[u8]) -> Option<Run> {
    wasmtime()?;
    Some(run_bytes_with_command(&out_dir(name), bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
        command.arg("-W").arg("trap-on-grow-failure=n");
    }))
}

#[test]
fn a_suspended_record_uses_no_heap_where_the_box_does() {
    let boxed_bytes = compile("rec-off", RECURSION, false);
    let value_bytes = compile("rec-on", RECURSION, true);
    let Some(boxed) = run_limited("suspend-recursion-off", &boxed_bytes) else {
        return;
    };
    let by_value = run_limited("suspend-recursion-on", &value_bytes).expect("wasmtime is present");
    assert_ne!(
        boxed.code, 0,
        "the boxed form must exceed the memory limit: {}",
        boxed.stderr
    );
    assert_eq!(by_value.code, 0, "{}", by_value.stderr);
    assert_eq!(by_value.stderr, "8010\n");
}

/// The LIR passes do not change what a program with flattened suspending code prints.
#[test]
fn suspending_code_agrees_without_the_lir_passes() {
    let on = compile("suspend-noopt-on", SUSPENDING_FUNCTION, true);
    let noopt = compile_with("suspend-noopt", SUSPENDING_FUNCTION, true, false);
    let Some(_) = wasmtime() else { return };
    let a = run_bytes(&out_dir("suspend-noopt-a"), &on);
    let b = run_bytes(&out_dir("suspend-noopt-b"), &noopt);
    assert_eq!(
        (&a.stdout, &a.stderr, a.code),
        (&b.stdout, &b.stderr, b.code),
        "the LIR passes changed what the program prints"
    );
    assert_eq!(b.code, 0, "{}", b.stderr);
    assert_eq!(b.stderr, SUSPENDING_FUNCTION_OUT);
}
