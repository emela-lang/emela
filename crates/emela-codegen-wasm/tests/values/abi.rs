//! E1 step 3: the direct-call ABI of records and tuples held by value.
//!
//! The option is on by default since E1 step 6; these tests choose it and the boxed form
//! explicitly. With it on, a function whose value the module never takes has a flattened wasm
//! signature: an `Agg` parameter is its scalar leaves and an `Agg` result is one wasm result per
//! leaf (multi-value). The same program must print the same thing and exit the same way in both
//! forms.
//!
//! The structural checks read the core module inside the component: the flattened form has
//! functions with several wasm results, and a record passed to or returned from a function no
//! longer calls the allocator. The memory test keeps a record live across a recursive call: the
//! boxed form allocates one object per live frame and exceeds a tight limit; the by-value form
//! keeps the leaves in wasm locals and passes.

use crate::common;

use crate::common::support;

use common::*;
use emela_codegen_wasm::{Entry, WasmOptions, compile_with_wasm_options};
use emela_core::Roots;
use wasmparser::{Operator, Parser, Payload, ValType};

/// A direct function taking and returning an all-scalar record, a nested record, a tuple, several
/// parameters and results of different leaf shapes, nested calls and recursion that keeps a
/// record live across the recursive call.
const PROGRAM: &str = r#"
import Std.Console.{Console, SystemConsole}

error Nope

type Point(x: Int, y: Int)
type Mixed(n: Int, f: Float)
type Pair(left: Point, right: Point)

# A record returned along the failure path: every path of a failed call returns dummy leaves,
# and the escape supplies the value instead.
fn risky(n: Int, p: Point) -> Point fails Nope {
  if n == 0 {
    p
  } else {
    if p.x > 10 {
      fail Nope
    } else {
      risky(n - 1, Point(x: p.x + 1, y: p.y))
    }
  }
}

fn bump(p: Point) -> Point {
  if p.x == 0 {
    p
  } else {
    q = bump(Point(x: p.x - 1, y: p.y))
    Point(x: q.x + 1, y: q.y + 1)
  }
}

fn sum_pair(p: Pair) -> Int {
  if p.left.x == 0 {
    p.left.y + p.right.x + p.right.y
  } else {
    sum_pair(Pair(left: Point(x: p.left.x - 1, y: p.left.y), right: bump(p.right))) + 1
  }
}

fn mix(a: Point, b: Mixed, k: Int) -> Mixed {
  if k == 0 {
    b
  } else {
    mix(Point(x: a.x + 1, y: a.y), Mixed(n: b.n + 1, f: b.f + 0.5), k - 1)
  }
}

fn swap(t: (Int, Float)) -> (Int, Float) {
  (n, f) = t
  if n == 0 {
    (n, f)
  } else {
    swap((n - 1, f + 0.5))
  }
}

fn many(a: Point, b: Mixed, c: Int, d: Int, k: Int) -> Int {
  a.x + a.y + b.n + c + d + k
}

# A direct function that uses an effect takes the evidence table as its last parameter; the
# record crosses the flattened signature as its leaves too.
fn announce(n: Int, p: Point) -> Point uses { Console } {
  if n == 0 {
    p
  } else {
    (use Console).print_line("announce")
    announce(n - 1, Point(x: p.x + 1, y: p.y))
  }
}

fn tail(n: Int, p: Point) -> Point {
  if n == 0 {
    p
  } else {
    tail(n - 1, bump(p))
  }
}

fn outer(p: Point) -> Int { sum_pair(Pair(left: p, right: bump(p))) }

fn walk(n: Int, p: Point) -> Int {
  if n == 0 {
    p.x + p.y
  } else {
    q = Point(x: p.x + 1, y: p.y + 2)
    r = walk(n - 1, q)
    p.x + p.y + r
  }
}

fn main() {
  p = Point(x: 1, y: 2)
  dbg(p.x + p.y)
  q = bump(p)
  dbg(q.x)
  dbg(q.y)
  dbg(sum_pair(Pair(left: p, right: q)))
  m = Mixed(n: 41, f: 1.5)
  m2 = mix(p, m, 5)
  dbg(m2.n)
  dbg(m2.f)
  t = (3, 2.5)
  (tn, tf) = swap(t)
  dbg(tn)
  dbg(tf)
  dbg(many(p, m, 1, 2, 3))
  dbg(outer(p))
  dbg(walk(10, p))
  t2 = tail(3, p)
  dbg(t2.x)
  dbg(t2.y)
  r = risky(3, p) escape { Nope -> Point(x: -1, y: -1) }
  dbg(r.x)
  dbg(r.y)
  failed = risky(100, p) escape { Nope -> Point(x: -1, y: -1) }
  dbg(failed.x)
  dbg(failed.y)
  with SystemConsole {
    a2 = announce(2, p)
    dbg(a2.x)
    dbg(a2.y)
  }
}
"#;

/// The expected `dbg` output (stderr) of [`PROGRAM`].
const OUTPUT: &str = "3\n1\n3\n8\n46\n4.0\n0\n4.0\n50\n8\n198\n1\n5\n4\n2\n-1\n-1\n3\n2\n";

/// The `print_line` output (stdout) of [`PROGRAM`].
const STDOUT: &str = "announce\nannounce\n";

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

fn compile(name: &str, values: bool) -> Vec<u8> {
    let out = to_output_with(
        PROGRAM,
        WasmOptions {
            values,
            ..WasmOptions::default()
        },
    );
    wasmparser::validate(&out.module).unwrap_or_else(|e| {
        panic!("{name}: the module is not valid: {e}");
    });
    out.module
}

#[test]
fn direct_calls_flatten_the_signature_and_agree_with_the_boxed_form() {
    let off = compile("abi-off", false);
    let on = compile("abi-on", true);
    assert_ne!(off, on, "the option must change the direct-call ABI");

    let off_calls = calls(&off);
    let on_calls = calls(&on);
    assert_eq!(
        off_calls.sigs.len(),
        on_calls.sigs.len(),
        "the function index space"
    );

    // The boxed form has no function with several results; the flattened form has the record and
    // tuple results of `bump`, `mix`, `swap` and the nested construction in them.
    let multi = |c: &Calls| {
        c.sigs
            .iter()
            .filter(|s| s.results.len() > 1)
            .map(|s| s.results.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(multi(&off_calls), Vec::<Vec<ValType>>::new());
    let on_multi = multi(&on_calls);
    assert!(
        on_multi.len() >= 3,
        "the flattened form has several results: {on_multi:?}"
    );
    assert!(
        on_multi.contains(&vec![ValType::I32, ValType::I32]),
        "a `Point` result is two `i32` results: {on_multi:?}"
    );
    assert!(
        on_multi.contains(&vec![ValType::I32, ValType::F64]),
        "a `Mixed` or tuple result is an `i32` and an `f64`: {on_multi:?}"
    );
    let has = |params: &[ValType], results: &[ValType]| {
        on_calls
            .sigs
            .iter()
            .any(|s| s.params == params && s.results == results)
    };
    assert!(
        has(&[ValType::I32, ValType::I32], &[ValType::I32, ValType::I32]),
        "a `Point -> Point` function is two parameters and two results"
    );
    assert!(
        has(
            &[ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32]
        ),
        "an `(Int, Point) -> Point` function is three parameters and two results"
    );
    assert!(
        has(&[ValType::I32, ValType::F64], &[ValType::I32, ValType::F64]),
        "a tuple function is its two leaves"
    );
    assert!(
        has(
            &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            &[ValType::I32, ValType::I32]
        ),
        "`(Int, Point) -> Point` with its evidence table is four parameters"
    );
    assert!(
        has(
            &[
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::F64,
                ValType::I32
            ],
            &[ValType::I32, ValType::F64]
        ),
        "`(Point, Mixed, Int) -> Mixed` is its five parameters and two results"
    );
    assert!(
        has(
            &[
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::F64,
                ValType::I32,
                ValType::I32,
                ValType::I32
            ],
            &[ValType::I32]
        ),
        "a five-parameter function spreads its two records"
    );

    // A record passed to or returned from a direct function no longer goes through the
    // allocator: at least one callee with the allocator's `(i32, i32) -> i32` signature lost
    // calls, and none gained.
    let mut lost = 0;
    for (i, (before, after)) in off_calls.counts.iter().zip(&on_calls.counts).enumerate() {
        if before == after {
            continue;
        }
        let sig = &off_calls.sigs[i];
        if sig.params == [ValType::I32, ValType::I32] && sig.results == [ValType::I32] {
            assert!(
                before > after,
                "a by-value record must stop calling `alloc`"
            );
            lost += 1;
        }
    }
    assert!(lost >= 1, "the allocator lost calls");

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("abi-off-run"), &off);
    let on_run = run_bytes(&out_dir("abi-on-run"), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "the by-value ABI changed what the program prints"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
    assert_eq!(on_run.stdout, STDOUT);
    assert_eq!(on_run.stderr, OUTPUT);
}

/// Compiles [`PROGRAM`] without the LIR optimisation passes, with the by-value ABI on or off.
fn compile_unoptimized(name: &str, values: bool) -> Vec<u8> {
    let m = support::try_lower(&[("main.emel", PROGRAM)], Roots::Exported)
        .unwrap_or_else(|e| panic!("{name}: lowering failed: {e:#?}"));
    let out = compile_with_wasm_options(
        &m,
        &Entry::Main,
        false,
        emela_lir::opt::Options::default(),
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

#[test]
fn direct_calls_agree_without_the_lir_passes() {
    let off = compile_unoptimized("abi-off-noopt", false);
    let on = compile_unoptimized("abi-on-noopt", true);
    assert_ne!(off, on, "the option must change the direct-call ABI");

    let Some(_) = wasmtime() else { return };
    let off_run = run_bytes(&out_dir("abi-off-noopt-run"), &off);
    let on_run = run_bytes(&out_dir("abi-on-noopt-run"), &on);
    assert_eq!(
        (&off_run.stdout, &off_run.stderr, off_run.code),
        (&on_run.stdout, &on_run.stderr, on_run.code),
        "the by-value ABI changed what the unoptimised program prints"
    );
    assert_eq!(on_run.code, 0, "{}", on_run.stderr);
    assert_eq!(on_run.stdout, STDOUT);
    assert_eq!(on_run.stderr, OUTPUT);

    // And the LIR passes do not change what it prints either.
    let optimized = run_bytes(&out_dir("abi-on-run-again"), &compile("abi-on-again", true));
    assert_eq!(
        (&on_run.stdout, &on_run.stderr, on_run.code),
        (&optimized.stdout, &optimized.stderr, optimized.code)
    );
}

/// The memory each program may grow to: six 64 KiB pages, as in `tests/programs/memory.rs`.
const LIMIT: u32 = 6 * 65536;

/// A recursion that keeps one record live across each recursive call: `p` is read after the
/// call, so the boxed form holds one heap object per live frame and the by-value form holds its
/// leaves in wasm locals.
const RECURSION: &str = r#"
type Cell(a: Int, b: Int, c: Int, d: Int, e: Int, f: Int, g: Int, h: Int, i: Int, j: Int)

fn walk(n: Int, p: Cell) -> Int {
  if n == 0 {
    p.a
  } else {
    q = Cell(a: p.a + 1, b: p.b, c: p.c, d: p.d, e: p.e, f: p.f, g: p.g, h: p.h, i: p.i, j: p.j)
    r = walk(n - 1, q)
    p.j + r
  }
}

fn main() {
  p = Cell(a: 0, b: 0, c: 0, d: 0, e: 0, f: 0, g: 0, h: 0, i: 0, j: 1)
  dbg(walk(4000, p))
}
"#;

/// Runs `src` under [`LIMIT`] with the options; `None` without `wasmtime`.
fn run_limited(name: &str, src: &str, values: bool) -> Option<common::Run> {
    wasmtime()?;
    let out = to_output_with(
        src,
        WasmOptions {
            values,
            ..WasmOptions::default()
        },
    );
    wasmparser::validate(&out.module).expect("the module is valid");
    Some(run_bytes_with_command(
        &out_dir(name),
        &out.module,
        |command| {
            command.arg("-W").arg(format!("max-memory-size={LIMIT}"));
            // The guest must see `memory.grow` return -1 and run its own failure path.
            command.arg("-W").arg("trap-on-grow-failure=n");
        },
    ))
}

#[test]
fn a_value_live_across_recursion_uses_no_heap() {
    // Each live frame of the boxed form holds one `Cell`: ten 8-byte slots and the header is
    // 96 bytes, so 4000 frames is more than the six pages the by-value form runs in.
    let Some(boxed) = run_limited("abi-recursion-off", RECURSION, false) else {
        return;
    };
    assert_ne!(
        boxed.code, 0,
        "the boxed form must exceed the memory limit: {}",
        boxed.stderr
    );
    let by_value = run_limited("abi-recursion-on", RECURSION, true).expect("wasmtime is present");
    assert_eq!(by_value.code, 0, "the by-value form: {}", by_value.stderr);
    assert_eq!(by_value.stderr, "8000\n");
}
