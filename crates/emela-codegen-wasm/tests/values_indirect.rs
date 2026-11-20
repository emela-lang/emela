//! E1 step 4: the ABI of an indirect, operation or closure call of a record held by value.
//!
//! A function value's type has one signature: a parameter or result that has an `Agg` layout is
//! flattened in a direct call, a `call_indirect`, a closure and a non-suspending effect operation
//! alike. The option is on by default since E1 step 6; each test compares it with the boxed
//! form explicitly, so the boxed path is still exercised.
//!
//! The suspending paths are in `values_suspend.rs`: a suspending function and the record
//! parameters of a suspending operation flatten, a suspending result of more than one leaf and
//! every value that crosses a fiber thunk stay boxed.
//!
//! Each test compiles the same program with `values` off and on, validates both modules and (when
//! `wasmtime` is present) compares stdout, stderr and the exit code. The structural checks read the
//! core module nested in the component.

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

/// The signatures of the core module and the type of every `call_indirect`.
struct Module {
    types: Vec<Sig>,
    indirect: Vec<Sig>,
}

/// Reads the core module nested in the component.
fn module(bytes: &[u8]) -> Module {
    let mut types: Vec<Sig> = Vec::new();
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
    let indirect = indirect_indices
        .into_iter()
        .map(|i| types[i as usize].clone())
        .collect();
    Module { types, indirect }
}

/// Lowers `src` and compiles it with `values` and the LIR rules.
///
/// The Core IR optimisations are off so that the closures and their indirect calls reach the
/// backend (they would otherwise be inlined into `main`); the LIR passes, including closure
/// devirtualisation, still run.
fn compile(name: &str, src: &str, values: bool, rules: LirRules) -> Vec<u8> {
    let m = support::try_lower_off(&[("main.emel", src)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    let out = compile_with_wasm_options(
        &m,
        &Entry::Main,
        true,
        Options { rules },
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

/// Compiles `src` with the default rules, both forms.
fn compile_pair(name: &str, src: &str) -> (Vec<u8>, Vec<u8>) {
    (
        compile(&format!("{name}-off"), src, false, LirRules::default()),
        compile(&format!("{name}-on"), src, true, LirRules::default()),
    )
}

/// Runs both forms and expects the same observable output, `stdout` and `stderr`.
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

/// A closure taking and returning an all-scalar record and a tuple, in a direct call, an indirect
/// call and a closure with a capture.
const CLOSURES: &str = r#"
type Point(x: Int, y: Int)
type Pair(a: Int, b: Int)

fn offset(p: Point) -> Point {
  Point(x: p.x + 10, y: p.y + 20)
}

fn chooser(n: Int) -> fn(Point) -> Pair {
  if n == 0 {
    |p| Pair(a: p.x + 1, b: p.y + 2)
  } else {
    |p| Pair(a: p.x * 2, b: p.y * 3)
  }
}

fn main() {
  p = Point(x: 1, y: 2)
  q = offset(p)
  dbg(q.x)
  dbg(q.y)
  r = chooser(0)(p)
  dbg(r.a)
  dbg(r.b)
  s = chooser(1)(p)
  dbg(s.a)
  dbg(s.b)
  k = p.x + 3
  h = |x: Int| Point(x: x + k, y: k)
  t = h(3)
  dbg(t.x)
  dbg(t.y)
  local = |p: Point| Pair(a: p.x - k, b: p.y + k)
  u = local(Point(x: 8, y: 9))
  dbg(u.a)
  dbg(u.b)
  sw = |t: (Int, Int)| {
    (a, b) = t
    (b, a)
  }
  v = sw((7, 8))
  (va, vb) = v
  dbg(va)
  dbg(vb)
}
"#;

const CLOSURES_OUT: &str = "11\n22\n2\n4\n2\n6\n7\n4\n4\n13\n8\n7\n";

/// A flattened `Point -> Pair` closure is `(i32 env, i32, i32) -> (i32, i32)`.
const POINT_TO_PAIR: ([ValType; 3], [ValType; 2]) = (
    [ValType::I32, ValType::I32, ValType::I32],
    [ValType::I32, ValType::I32],
);

#[test]
fn closure_calls_flatten_the_record_and_agree_with_the_boxed_form() {
    let (off, on) = compile_pair("indirect-closures", CLOSURES);
    assert_ne!(off, on, "the option must change an indirect call's ABI");

    let off_module = module(&off);
    let on_module = module(&on);
    assert!(
        !off_module
            .indirect
            .iter()
            .any(|s| s.params == POINT_TO_PAIR.0 && s.results == POINT_TO_PAIR.1),
        "the boxed form must not have a flattened `Point -> Pair` closure call"
    );
    assert!(
        on_module
            .indirect
            .iter()
            .any(|s| s.params == POINT_TO_PAIR.0 && s.results == POINT_TO_PAIR.1),
        "the closure is called with its environment and the record's leaves: {:?}",
        on_module.indirect
    );
    assert!(
        on_module
            .types
            .iter()
            .any(|s| s.params == POINT_TO_PAIR.0 && s.results == POINT_TO_PAIR.1),
        "the lifted closure has the flattened signature"
    );
    same_output("indirect-closures", &off, &on, "", CLOSURES_OUT);
}

#[test]
fn a_devirtualised_closure_call_uses_the_same_flattened_signature() {
    let rules = LirRules {
        devirt: true,
        ..LirRules::default()
    };
    let off = compile("devirt-off", CLOSURES, false, rules);
    let on = compile("devirt-on", CLOSURES, true, rules);
    assert_ne!(off, on, "the option must change a devirtualised call's ABI");

    // `local` (and `h`) are created and only called in `main`, so the pass makes their calls
    // direct; the closures returned by `chooser` still escape and stay indirect. The baseline has
    // devirtualisation off, because it is on by default and would remove the same calls there.
    let base = module(&compile(
        "devirt-base",
        CLOSURES,
        true,
        LirRules {
            devirt: false,
            sra_closures: false,
            ..LirRules::default()
        },
    ));
    let on_module = module(&on);
    assert!(
        on_module.indirect.len() < base.indirect.len(),
        "devirtualisation must remove indirect calls: {} -> {}",
        base.indirect.len(),
        on_module.indirect.len()
    );
    assert!(
        on_module
            .types
            .iter()
            .any(|s| s.params == POINT_TO_PAIR.0 && s.results == POINT_TO_PAIR.1),
        "the devirtualised closure function has the flattened signature"
    );
    same_output("devirt", &off, &on, "", CLOSURES_OUT);
}

/// A non-suspending operation taking and returning an all-scalar record: its implementation is a
/// closure of the operation's function type, so it flattens with the call.
const OPERATION: &str = r#"
type Point(x: Int, y: Int)

effect Shift {
  fn shift(p: Point) -> Point
}

handler Shifter: Shift {
  fn shift(p) { Point(x: p.x + 1, y: p.y + 2) }
}

fn main() {
  with Shifter {
    q = (use Shift).shift(Point(x: 1, y: 2))
    dbg(q.x)
    dbg(q.y)
  }
}
"#;

const OPERATION_OUT: &str = "2\n4\n";

#[test]
fn a_non_suspending_operation_flattens_its_parameters_and_result() {
    let (off, on) = compile_pair("indirect-operation", OPERATION);
    assert_ne!(off, on, "the option must change the operation's ABI");

    // The handler is one reference parameter; the record is its two leaves.
    let expected = (
        [ValType::I32, ValType::I32, ValType::I32],
        [ValType::I32, ValType::I32],
    );
    let off_module = module(&off);
    let on_module = module(&on);
    assert!(
        !off_module
            .indirect
            .iter()
            .any(|s| s.params == expected.0 && s.results == expected.1),
        "the boxed operation call is one reference parameter and one result"
    );
    assert!(
        on_module
            .indirect
            .iter()
            .any(|s| s.params == expected.0 && s.results == expected.1),
        "the operation is called with the handler and the record's leaves: {:?}",
        on_module.indirect
    );
    same_output("indirect-operation", &off, &on, "", OPERATION_OUT);
}

/// The memory each program may grow to: six 64 KiB pages, as in `tests/memory.rs`.
const LIMIT: u32 = 2 * 65536;

/// An indirect call that keeps the returned record live across each recursive frame: the boxed
/// form allocates one object per frame, the by-value form keeps the leaves in wasm locals.
const RECURSION: &str = r#"
type Cell(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int, j: Int)

fn run(f: fn(Cell) -> Cell, n: Int, p: Cell) -> Cell {
  if n == 0 {
    p
  } else {
    q = f(p)
    r = run(f, n - 1, q)
    Cell(a: r.a + p.a, b: r.b, c: r.c, d: r.d, e: r.e, f: r.f, g: r.g, h: r.h, i: r.i, j: r.j)
  }
}

fn main() {
  f = |p: Cell| Cell(a: p.a + 1, b: p.b, c: p.c, d: p.d, e: p.e, f: p.f, g: p.g, h: p.h, i: p.i, j: p.j)
  p = Cell(a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, h: 0, i: 0, j: 1)
  r = run(f, 2000, p)
  dbg(r.j)
}
"#;

fn run_limited(name: &str, src: &str, values: bool) -> Option<Run> {
    wasmtime()?;
    let bytes = compile(name, src, values, LirRules::default());
    Some(run_bytes_with_command(&out_dir(name), &bytes, |command| {
        command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
        command.arg("-W").arg("trap-on-grow-failure=n");
    }))
}

#[test]
fn an_indirect_call_live_across_recursion_uses_no_heap() {
    let Some(boxed) = run_limited("indirect-recursion-off", RECURSION, false) else {
        return;
    };
    assert_ne!(
        boxed.code, 0,
        "the boxed form must exceed the memory limit: {}",
        boxed.stderr
    );
    let by_value =
        run_limited("indirect-recursion-on", RECURSION, true).expect("wasmtime is present");
    assert_eq!(by_value.code, 0, "the by-value form: {}", by_value.stderr);
    assert_eq!(by_value.stderr, "1\n");
}
