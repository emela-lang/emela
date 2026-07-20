//! Reclamation checks for ARC (spec 0048): build a program to `wasm-wasi`,
//! run `_start` under wasmi, and assert the module's `live_bytes` export is
//! exactly zero afterwards — every heap block was released and freed (A1),
//! including `main`'s own result (released by `_start`).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use wasmi::errors::HostError;
use wasmi::{Engine, Linker, Module, Store};

static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("emela-rc-{label}-{}-{id}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Compiles `source` with the real `emela` binary and returns the wasm bytes.
fn build_wasm(label: &str, source: &str) -> Vec<u8> {
    let dir = temp_dir(label);
    let input = dir.join("main.emel");
    let output = dir.join("main.wasm");
    fs::write(&input, source).unwrap();
    let status = Command::new(env!("CARGO_BIN_EXE_emela"))
        .arg("build")
        .arg(&input)
        .arg("--backend")
        .arg("wasm-wasi")
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let bytes = fs::read(&output).unwrap();
    let _ = fs::remove_dir_all(&dir);
    bytes
}

#[derive(Debug)]
struct Exit(i32);

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "process exited with code {}", self.0)
    }
}

impl HostError for Exit {}

/// Runs `_start` and returns `(exit_code, live_bytes)`.
fn run_and_measure(wasm: &[u8]) -> (i32, i32) {
    let engine = Engine::default();
    let module = Module::new(&engine, wasm).unwrap();
    let mut store = Store::new(&engine, ());
    let mut linker: Linker<()> = Linker::new(&engine);
    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "proc_exit",
            |code: i32| -> Result<(), wasmi::Error> { Err(wasmi::Error::host(Exit(code))) },
        )
        .unwrap();
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    let start = instance.get_typed_func::<(), ()>(&store, "_start").unwrap();
    let code = match start.call(&mut store, ()) {
        Ok(()) => 0,
        Err(err) => match err.downcast_ref::<Exit>() {
            Some(Exit(code)) => *code,
            None => panic!("unexpected trap: {err}"),
        },
    };
    let live = instance
        .get_global(&store, "live_bytes")
        .expect("live_bytes export")
        .get(&store)
        .i32()
        .expect("live_bytes is i32");
    (code, live)
}

fn assert_reclaims(label: &str, source: &str) {
    let wasm = build_wasm(label, source);
    let (code, live) = run_and_measure(&wasm);
    assert_eq!(code, 0, "program exits cleanly");
    assert_eq!(live, 0, "all heap blocks are reclaimed (live_bytes)");
}

/// Per-iteration string temporaries die within their iteration (A5).
#[test]
fn string_churn_reclaims_to_zero() {
    assert_reclaims(
        "strings",
        "fn churn(n: Int) -> Int {\n  if n == 0 {\n    0\n  } else {\n    let _s = \"tick \" ++ \"tock\"\n    churn(n - 1)\n  }\n}\nfn main() -> Int { churn(1000) }\n",
    );
}

/// An array holding heap strings releases its elements when it dies.
#[test]
fn array_of_strings_reclaims_to_zero() {
    assert_reclaims(
        "array",
        "fn main() -> Int {\n  let a = [\"a\" ++ \"b\", \"c\" ++ \"d\", \"e\" ++ \"f\"]\n  0\n}\n",
    );
}

/// Enum payloads release recursively; a recursive list of strings dies whole.
#[test]
fn recursive_enum_reclaims_to_zero() {
    assert_reclaims(
        "list",
        "enum List<T> {\n  Cons(T, List<T>),\n  Nil\n}\nfn build(n: Int) -> List<String> {\n  if n == 0 {\n    List::Nil\n  } else {\n    List::Cons(\"a\" ++ \"b\", build(n - 1))\n  }\n}\nfn main() -> Int {\n  let _l = build(100)\n  0\n}\n",
    );
}

/// A match consumes its scrutinee; the arm's payload borrow does not leak.
#[test]
fn match_scrutinee_reclaims_to_zero() {
    assert_reclaims(
        "match",
        "enum Box {\n  Full(String),\n  Empty\n}\nfn wrap() -> Box { Box::Full(\"x\" ++ \"y\") }\nfn main() -> Int {\n  match wrap() {\n    Full(s) -> 0\n    Empty -> 1\n  }\n}\n",
    );
}

/// An escaping closure owns its captures; both die when the closure does.
#[test]
fn escaping_closure_reclaims_to_zero() {
    assert_reclaims(
        "closure",
        "fn make(s: String) -> () -> String {\n  fn () -> String { s ++ \"!\" }\n}\nfn main() -> Int {\n  let f = make(\"hey\" ++ \"ho\")\n  let _r = f()\n  0\n}\n",
    );
}

/// `main` returning a heap value still ends at zero: `_start` releases it.
#[test]
fn heap_main_result_is_released_by_start() {
    let wasm = build_wasm("main-ret", "fn main() -> String { \"a\" ++ \"b\" }\n");
    let (_code, live) = run_and_measure(&wasm);
    assert_eq!(live, 0, "the result released in _start");
}

/// Unwinding through a throwing call releases what the frame still owned, and
/// the caught error (with its heap payload) dies at the catch arm's tail
/// (spec 0048 A7).
#[test]
fn error_unwind_reclaims_to_zero() {
    assert_reclaims(
        "unwind",
        "enum E {\n  Boom(String)\n}\nfn risky() -> Int throws E { throw E::Boom(\"bad \" ++ \"input\") }\nfn attempt() -> Int throws E {\n  let _s = \"keep \" ++ \"me\"\n  risky()?\n}\nfn main() -> Int {\n  let r = try {\n    attempt()\n  } catch {\n    Boom(_m) -> 7\n  }\n  r - 7\n}\n",
    );
}

/// A catch arm that keeps looping (the 0046 serve-loop shape) frees the
/// caught error every iteration: constant memory across repeated errors.
#[test]
fn repeated_caught_errors_reclaim_to_zero() {
    assert_reclaims(
        "retry",
        "enum E {\n  Boom(String)\n}\nfn risky() -> Int throws E { throw E::Boom(\"kab\" ++ \"oom\") }\nfn spin(n: Int) -> Int {\n  if n == 0 {\n    0\n  } else {\n    let _r = try {\n      risky()\n    } catch {\n      Boom(_m) -> 0\n    }\n    spin(n - 1)\n  }\n}\nfn main() -> Int { spin(500) }\n",
    );
}

/// `?` on `None` early-returns; the frame's live bindings are still released.
#[test]
fn question_none_exit_reclaims_to_zero() {
    assert_reclaims(
        "question-none",
        "fn pick(flag: Bool) -> Option<String> {\n  if flag {\n    Some(\"yes\" ++ \"!\")\n  } else {\n    None\n  }\n}\nfn extract(flag: Bool) -> Option<String> {\n  let keep = \"held \" ++ \"value\"\n  let v = pick(flag)?\n  Some(v ++ keep)\n}\nfn main() -> Int {\n  let _a = extract(true)\n  let _b = extract(false)\n  0\n}\n",
    );
}

/// An uncaught error still cleans the whole call chain before `main` fails.
#[test]
fn partial_construction_across_throws_reclaims() {
    // The record's first field is owned and in flight when the second
    // field's evaluation throws: A-normalization keeps it releasable.
    assert_reclaims(
        "partial",
        "enum E {\n  Nope\n}\nrecord Pair {\n  a: String,\n  b: String\n}\nfn boom() -> String throws E { throw E::Nope }\nfn build() -> Pair throws E {\n  Pair { a: \"left\" ++ \"!\", b: boom()? }\n}\nfn main() -> Int {\n  let _p = try {\n    build()\n  } catch {\n    Nope -> Pair { a: \"x\" ++ \"y\", b: \"z\" ++ \"w\" }\n  }\n  0\n}\n",
    );
}
