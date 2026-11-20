//! E2 step 3: read an all-scalar array element directly into a by-value destination.
//!
//! `values` is on by default since E1 step 6 and `aggregate_arrays` is off by default; these
//! tests choose both explicitly. With `aggregate_arrays` on, an element of an all-scalar
//! record or tuple is stored inline (E2 step 2). With `values` also on, `Array.get`'s two
//! internal fusions, `ArrayGetOr` and `ArrayLoadUnchecked`, read each scalar leaf of the element
//! directly into the destination's wasm locals: no box is allocated and no reference is duped
//! (an inline element contains no references).
//!
//! `Array.get` and `Array.to_list` still box one element per read. `Array.get` returns
//! `Option[A]`, whose `Some` cell stores its payload as one reference (a multi-variant enum has
//! no by-value form), and a list element is a reference. The tests below pin both limitations.
//!
//! The source program must print the same thing, write the same standard error and exit the same
//! way with the options off and on, with and without the LIR passes.

mod common;

#[allow(clippy::duplicate_mod)] // `common` includes the same Core test support.
#[path = "../../emela-core/tests/support/mod.rs"]
mod support;

use common::*;
use emela_codegen_wasm::{Entry, WasmOptions, compile_with_wasm_options};
use emela_core::Roots;
use wasmparser::{Operator, Parser, Payload, ValType};

/// One read through each fusion: `Option.or(Array.get(..), d)` becomes `ArrayGetOr`, and the
/// `match` on `Array.get` becomes `ArrayLoadUnchecked` inside its bounds test.
const READS: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn get_or(a: Array[Point], i: Int) -> Int {
  p = Array.get(a, i) |> Option.or(Point(x: -1, y: -1))
  p.x * 100 + p.y
}

fn load(a: Array[Point], i: Int) -> Int {
  p = Array.get(a, i) match {
    Some(q) -> q
    None -> Point(x: -1, y: -1)
  }
  p.x * 100 + p.y
}

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  dbg(get_or(a, 1))
  dbg(get_or(a, 9))
  dbg(load(a, 0))
  dbg(load(a, 1))
}
"#;

const READS_OUTPUT: &str = "304\n-101\n102\n304\n";

/// `Array.get` whose `Option` reaches `dbg`, so the Core fusion leaves it as an `ArrayGet`.
const OPTION: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  dbg(Array.get(a, 0))
  dbg(Array.get(a, 9))
}
"#;

const OPTION_OUTPUT: &str = "Some(Point(x: 1, y: 2))\nNone\n";

/// `Array.to_list` builds list cells whose heads are references.
const TO_LIST: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn main() {
  a = Array.from_list([Point(x: 1, y: 2), Point(x: 3, y: 4)])
  dbg(Array.to_list(a))
}
"#;

const TO_LIST_OUTPUT: &str = "[Point(x: 1, y: 2), Point(x: 3, y: 4)]\n";

fn build(name: &str, src: &str, optimize: bool, values: bool, aggregate_arrays: bool) -> Vec<u8> {
    let m = support::try_lower(&[("main.emel", src)], Roots::Exported)
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
    wasmparser::validate(&output.module)
        .unwrap_or_else(|e| panic!("{name}: the module is not valid: {e}"));
    output.module
}

/// Runs the boxed-read and inline-read builds and checks that they agree.
fn agree(name: &str, src: &str, expected: &str, optimize: bool) {
    let off = build(&format!("{name}-off"), src, optimize, false, true);
    let on = build(&format!("{name}-on"), src, optimize, true, true);
    assert_ne!(off, on, "{name}: the options must change the emission");

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir(&format!("{name}-off")), &off);
    let on_run = run_bytes(&out_dir(&format!("{name}-on")), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "{name}: the inline read changed what the program prints or how it exits"
    );
    assert_eq!(on_run.code, 0, "{name}: {}", on_run.stderr);
    assert_eq!(on_run.stderr, expected, "{name}");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sig {
    params: Vec<ValType>,
    results: Vec<ValType>,
}

/// What a defined function's body does, as far as the structural check needs to know.
#[derive(Default)]
struct Body {
    /// The body is the single `unreachable` instruction: the runtime emitted a stub for a
    /// function that no reachable code calls.
    stub: bool,
    /// The body copies memory, as an inline box does.
    has_copy: bool,
    /// The functions the body calls.
    callees: Vec<u32>,
}

/// Per function index: how often the core module calls it, its signature and its body.
struct Calls {
    counts: Vec<usize>,
    sigs: Vec<Sig>,
    /// `None` for an import; `Some` for a defined function.
    bodies: Vec<Option<Body>>,
}

/// Reads the core module nested in the component and counts `call` instructions per function
/// index. The module carries no name section, so the callee is identified by its signature and
/// body.
fn calls(bytes: &[u8]) -> Calls {
    let mut types: Vec<Sig> = Vec::new();
    let mut import_tys: Vec<u32> = Vec::new();
    let mut defined_tys: Vec<u32> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut bodies: Vec<Body> = Vec::new();
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
                let mut parsed = Body::default();
                let mut operators = 0usize;
                let mut first_unreachable = false;
                for op in ops {
                    let op = op.expect("the operator parses");
                    if operators == 0 {
                        first_unreachable = matches!(op, Operator::Unreachable);
                    }
                    operators += 1;
                    match op {
                        Operator::Call { function_index } => {
                            let i = function_index as usize;
                            if counts.len() <= i {
                                counts.resize(i + 1, 0);
                            }
                            counts[i] += 1;
                            parsed.callees.push(function_index);
                        }
                        Operator::MemoryCopy { .. } => parsed.has_copy = true,
                        _ => {}
                    }
                }
                parsed.stub = first_unreachable
                    && operators <= 2
                    && parsed.callees.is_empty()
                    && !parsed.has_copy;
                bodies.push(parsed);
            }
            _ => {}
        }
    }
    let mut sigs = Vec::with_capacity(import_tys.len() + defined_tys.len());
    let imports = import_tys.len();
    for ty in import_tys.into_iter().chain(defined_tys) {
        sigs.push(types[ty as usize].clone());
    }
    counts.resize(sigs.len(), 0);
    let mut all_bodies: Vec<Option<Body>> =
        std::iter::repeat_with(|| None).take(sigs.len()).collect();
    for (i, body) in bodies.into_iter().enumerate() {
        all_bodies[imports + i] = Some(body);
    }
    Calls {
        counts,
        sigs,
        bodies: all_bodies,
    }
}

/// The allocator's wasm signature `(i32, i32) -> i32` (the slot count and the descriptor).
fn is_alloc_sig(sig: &Sig) -> bool {
    sig.params.len() == 2
        && sig.params.iter().all(|&ty| ty == ValType::I32)
        && sig.results == [ValType::I32]
}

/// The number of calls to a function taking `arity` `i32` parameters and returning an `i32`.
///
/// The array read helpers are the only functions of these shapes in a program that reads arrays
/// and prints integers: `array_get_or_agg` is four parameters (`a`, `index`, `d`, `elem_desc`)
/// and `array_load_unchecked_agg` is three (`a`, `index`, `elem_desc`). `array_get_agg` is five.
fn i32_calls(calls: &Calls, arity: usize) -> usize {
    calls
        .counts
        .iter()
        .zip(&calls.sigs)
        .filter(|(_, sig)| {
            sig.params.len() == arity
                && sig.params.iter().all(|&ty| ty == ValType::I32)
                && sig.results == [ValType::I32]
        })
        .map(|(count, _)| count)
        .sum()
}

/// The number of called read helpers of `arity` `i32` parameters that allocate a box and copy the
/// element into it: the `_agg` read functions an inline read would use if it were boxed.
fn called_boxing_reads(calls: &Calls, arity: usize) -> usize {
    (0..calls.sigs.len())
        .filter(|&i| {
            let sig = &calls.sigs[i];
            let read_shape = sig.params.len() == arity
                && sig.params.iter().all(|&ty| ty == ValType::I32)
                && sig.results == [ValType::I32];
            read_shape
                && calls.bodies[i].as_ref().is_some_and(|body| {
                    body.has_copy
                        && body
                            .callees
                            .iter()
                            .any(|&c| is_alloc_sig(&calls.sigs[c as usize]))
                })
                && calls.counts[i] > 0
        })
        .count()
}

#[test]
fn inline_reads_agree_with_the_boxed_reads() {
    agree("reads", READS, READS_OUTPUT, true);
    // The same program without the LIR passes: the Core fusions are already there, so the
    // inline lowering is exercised either way.
    agree("reads-noopt", READS, READS_OUTPUT, false);
}

#[test]
fn inline_reads_do_not_call_the_boxing_helpers() {
    let off = calls(&build("reads-calls-off", READS, true, false, true));
    let on = calls(&build("reads-calls-on", READS, true, true, true));
    assert_eq!(
        off.sigs, on.sigs,
        "both forms must keep the same function index space"
    );

    // The boxing read helpers are the functions of the element-read shapes that allocate a box,
    // copy the element into it and are no longer called with both options on. `array_get_or_agg`
    // is `(a, index, d, elem_desc) -> box` and `array_load_unchecked_agg` is
    // `(a, index, elem_desc) -> box`; each calls `alloc` and `memory.copy`.
    let mut helpers = 0;
    for i in 0..off.sigs.len() {
        let sig = &off.sigs[i];
        let read_shape = matches!(sig.params.len(), 3 | 4)
            && sig.params.iter().all(|&ty| ty == ValType::I32)
            && sig.results == [ValType::I32];
        if !read_shape {
            continue;
        }
        let (Some(before), Some(after)) = (&off.bodies[i], &on.bodies[i]) else {
            continue;
        };
        if !before.has_copy
            || !before
                .callees
                .iter()
                .any(|&c| is_alloc_sig(&off.sigs[c as usize]))
            || off.counts[i] == 0
            || !after.stub
        {
            continue;
        }
        assert_eq!(
            on.counts[i], 0,
            "an inline read must not call the boxing helper at function {i}"
        );
        helpers += 1;
    }
    assert!(
        helpers >= 2,
        "the boxed `ArrayGetOr` and `ArrayLoadUnchecked` helpers must disappear"
    );

    // `ArrayGetOr` is the only four-`i32` read helper in this program, so its call count alone is
    // decisive; no box is allocated on the inline path.
    assert!(
        i32_calls(&off, 4) > 0,
        "the boxed `ArrayGetOr` must call `array_get_or_agg`"
    );
    assert_eq!(
        i32_calls(&on, 4),
        0,
        "an inline `ArrayGetOr` must not call `array_get_or_agg` (an allocation)"
    );
}

#[test]
fn array_get_still_boxes_the_option_payload() {
    // `Option[Point]` is a two-variant enum and has no by-value form; its `Some` cell stores the
    // payload as one reference. `Array.get` therefore still copies the inline element into a new
    // box and puts that box in `Some`, whether or not `values` is on.
    agree("option", OPTION, OPTION_OUTPUT, true);
    let off = calls(&build("option-calls-off", OPTION, true, false, true));
    let on = calls(&build("option-calls-on", OPTION, true, true, true));
    assert_eq!(
        called_boxing_reads(&off, 5),
        1,
        "the boxed form calls `array_get_agg`"
    );
    assert_eq!(
        called_boxing_reads(&on, 5),
        1,
        "`Array.get` still calls `array_get_agg` when both options are on"
    );
}

#[test]
fn array_to_list_still_boxes_each_element() {
    // A list cell's head is a reference, so `to_list` needs one box per element. The runtime
    // helper boxes the element and conses the box whether or not `values` is on.
    agree("to-list", TO_LIST, TO_LIST_OUTPUT, true);
    let off = calls(&build("to-list-calls-off", TO_LIST, true, false, true));
    let on = calls(&build("to-list-calls-on", TO_LIST, true, true, true));
    assert_eq!(
        called_boxing_reads(&off, 3),
        1,
        "the boxed form calls `array_to_list_agg`"
    );
    assert_eq!(
        called_boxing_reads(&on, 3),
        1,
        "`Array.to_list` still calls `array_to_list_agg` when both options are on"
    );
}

/// The linear memory the program may grow to: nine 64 KiB pages, as in `tests/memory.rs`.
const LIMIT: u32 = 9 * 65536;

/// Builds a 16 384-element `Array[Point]` by doubling a one-element array (no per-element box
/// during construction), then reads the first 10 000 elements in a recursion that keeps each read
/// value live across the recursive call. The boxed read holds one 32-byte box per live frame
/// (320 KiB) in addition to the 256 KiB array; the inline read keeps two `i32` locals and fits.
const READS_MEMORY: &str = r#"
import Std.Array.{Array}

type Point(x: Int, y: Int)

fn double(a: Array[Point], n: Int) -> Array[Point] {
  if n == 0 { a } else { double(Array.concat([a, a]), n - 1) }
}

fn walk(a: Array[Point], i: Int, n: Int) -> Int {
  if i == n {
    0
  } else {
    p = Array.get(a, i) |> Option.or(Point(x: 0, y: 0))
    r = walk(a, i + 1, n)
    p.x + r
  }
}

fn main() {
  a = double(Array.from_list([Point(x: 1, y: 2)]), 14)
  dbg(Array.size(a))
  dbg(walk(a, 0, 10000))
}
"#;

fn run_limited(name: &str, bytes: &[u8]) -> common::Run {
    run_bytes_with_command(&out_dir(name), bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
        command.arg("-W").arg("trap-on-grow-failure=n");
        command.arg("-W").arg("max-wasm-stack=16777216");
    })
}

#[test]
fn inline_reads_fit_where_the_boxed_reads_do_not() {
    let Some(_) = wasmtime() else { return };
    let inline = build("reads-memory-inline", READS_MEMORY, true, true, true);
    let inline_run = run_limited("reads-memory-inline", &inline);
    assert_eq!(inline_run.code, 0, "inline: {}", inline_run.stderr);
    assert_eq!(
        inline_run.stderr, "16384\n10000\n",
        "the inline read changed the values"
    );

    let boxed = build("reads-memory-boxed", READS_MEMORY, true, false, true);
    let boxed_run = run_limited("reads-memory-boxed", &boxed);
    assert_ne!(
        boxed_run.code, 0,
        "the boxed read was expected to exceed the limit"
    );
}
