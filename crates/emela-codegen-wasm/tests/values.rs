//! E1 step 2: records and tuples held by value in WASM locals.
//!
//! The option is on by default since E1 step 6; these tests choose it and the boxed form
//! explicitly. With it on, an eligible record is a span of locals instead of a heap object; the
//! same program must print the same thing and exit the same way, and the local construction must
//! stop calling the allocator.
//!
//! The structural check reads the core module inside the component. `alloc` is the only callee
//! with the signature `(i32, i32) -> i32` whose call count is one lower with the option on: the
//! construction that used to call it is gone. Runtime bodies still call `alloc`, so the test
//! compares per-function-index call counts instead of looking for an absent name.

mod common;

use common::*;
use emela_codegen_wasm::WasmOptions;
use wasmparser::{Operator, Parser, Payload, ValType};

/// A record, a nested record, an `if` whose arms both build one, and a `match` that reads it.
/// Every value stays local: no function takes or returns a record, and none is put in a list,
/// array, enum payload, closure capture or `Any`.
const PROGRAM: &str = r#"
type Point(x: Int, y: Int)
type Mixed(n: Int, f: Float)
type Pair(left: Point, right: Point)

fn main() {
  p = Point(x: 1, y: 2)
  dbg(p.x + p.y)
  m = Mixed(n: 41, f: 1.5)
  dbg(m.n)
  dbg(m.f)
  pair = Pair(left: Point(x: 3, y: 4), right: Point(x: 5, y: 6))
  dbg(pair.left.x + pair.right.y)
  q = if p.x == 1 { Point(x: 10, y: 20) } else { Point(x: 30, y: 40) }
  dbg(q.x + q.y)
  v = p match { Point(x:, y:) -> x * 100 + y }
  dbg(v)
}
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Sig {
    params: Vec<ValType>,
    results: Vec<ValType>,
}

/// Per function index: how often the core module calls it and its signature.
struct Calls {
    counts: Vec<usize>,
    sigs: Vec<Sig>,
}

/// Reads the core module nested in the component and counts `call` instructions per function
/// index.
fn calls(bytes: &[u8]) -> Calls {
    let mut types: Vec<Sig> = Vec::new();
    let mut import_tys: Vec<u32> = Vec::new();
    let mut defined_tys: Vec<u32> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut in_main = false;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("wasm should parse") {
            // The component nests its main module first.
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
                    if let Operator::Call { function_index } = op.expect("the operator parses") {
                        let i = function_index as usize;
                        if counts.len() <= i {
                            counts.resize(i + 1, 0);
                        }
                        counts[i] += 1;
                    }
                }
            }
            _ => {}
        }
    }
    let mut sigs = Vec::with_capacity(import_tys.len() + defined_tys.len());
    for ty in import_tys.into_iter().chain(defined_tys) {
        sigs.push(types[ty as usize].clone());
    }
    counts.resize(sigs.len(), 0);
    Calls { counts, sigs }
}

#[test]
fn values_on_and_off_agree_and_a_local_record_stops_allocating() {
    let off = to_output_with(
        PROGRAM,
        WasmOptions {
            values: false,
            ..WasmOptions::default()
        },
    );
    let on = to_output_with(
        PROGRAM,
        WasmOptions {
            values: true,
            ..WasmOptions::default()
        },
    );
    wasmparser::validate(&off.module).expect("the boxed module is valid");
    wasmparser::validate(&on.module).expect("the by-value module is valid");
    assert_ne!(
        off.module, on.module,
        "the option must change the local representation"
    );

    let off_calls = calls(&off.module);
    let on_calls = calls(&on.module);
    assert_eq!(off_calls.sigs, on_calls.sigs, "the function index space");

    // `alloc` is `(i32, i32) -> i32`. Its calls from the runtime are the same in both builds; the
    // calls from the record constructions are fewer with the option on. Only a callee whose count
    // changed is considered: other callees with the same signature are untouched.
    let mut missing_alloc = 0;
    for (i, (before, after)) in off_calls.counts.iter().zip(&on_calls.counts).enumerate() {
        if before == after {
            continue;
        }
        let sig = &off_calls.sigs[i];
        if sig.params == [ValType::I32, ValType::I32] && sig.results == [ValType::I32] {
            assert!(
                before > after,
                "a record construction must stop calling `alloc` with the option on"
            );
            missing_alloc += 1;
        }
    }
    assert_eq!(missing_alloc, 1, "exactly one `alloc` callee lost calls");

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("values-off"), &off.module);
    let on_run = run_bytes(&out_dir("values-on"), &on.module);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "the by-value form changed what the program prints"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
    assert_eq!(on_run.stderr, "3\n41\n1.5\n9\n30\n102\n");
}
