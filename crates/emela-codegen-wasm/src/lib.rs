//! Emits WebAssembly from the Heap IR (spec chapter 16, Tier 1).
//!
//! [`compile`] runs the whole pipeline from lowered Core IR: monomorphisation
//! (`emela_core::monomorphize_from`, from the entry), the Flow IR (`emela_lir::from_core`), the
//! Heap IR passes (`emela_lir::to_heap`: closure conversion, explicit defects, the suspend state
//! machines and reference counting) and then this crate's emitter, which follows the Heap IR
//! mechanically, like `emela-codegen-c` does for C.
//!
//! The WASM runtime is emitted into the same module (Perceus reference counting on linear memory,
//! strings, the numeric conversions and the runtime operations) and the host is WASI 0.3, so the
//! output is a component exporting `wasi:cli/run@0.3.0`, run with an external engine
//! (`wasmtime`).
//!
//! The target supports the WASI hosts the bundled `Std` handlers use (Console, Env, Random,
//! Clock.now and Clock.sleep), and suspending functions are compiled as state machines with heap
//! frames (`emit::suspend`) and a FIFO continuation scheduler (`emit::runtime::sched`). A sleep
//! starts `wasi:clocks/monotonic-clock.wait-for` as an asynchronous subtask and the scheduler waits
//! on its waitable set only when it has no continuation left (`emit::runtime::async_bridge`);
//! TestClock uses the same suspension without a host timer. Fibers, other `Async` wakeups and
//! custom host WIT are not implemented yet; reaching them is a [`WasmError`] or a stall unwound as
//! interrupted (spec 14.5). The Unicode tables for grapheme clusters (UAX #29, Unicode 17.0.0) are
//! part of the runtime.

mod check;
mod component;
mod emit;
mod std_host;

pub use self::emit::layout;

use emela_core::FnId;

/// What the generated `wasi:cli/run@0.3.0` export runs.
#[derive(Debug, Clone)]
pub enum Entry {
    /// The entry module's `main` (an exported fn named `main`). Without one, the program does
    /// nothing.
    Main,
    /// Test functions (spec 13.4), called one by one in order. Each result is written to stdout
    /// as a line of [`TEST_EVENT_PREFIX`] and JSON, the protocol of the JS test runner: `{"event":
    /// "result", "name": …, "outcome": "ok" | "defect" | "error", "message": …}`, then `{"event":
    /// "done"}`. `message` is left out when an error value has no display, and the defect of a
    /// failed assert also has `"location"` (`[test.assert]`). The protocol is
    /// internal between the module and `emela test`, so the spec only fixes what `emela test`
    /// shows (`[cmd.test.output]`, `[cmd.test.outcome]`).
    Tests(Vec<TestEntry>),
}

/// A test function of [`Entry::Tests`].
#[derive(Debug, Clone)]
pub struct TestEntry {
    /// The name shown in the results.
    pub name: String,
    /// The test function in the Core module. It takes no arguments.
    pub function: FnId,
}

/// The start of a test result line (the same as the JS test runner's).
pub const TEST_EVENT_PREFIX: &str = "\u{1e}emela-test ";

/// The output of [`compile`].
#[derive(Debug, Clone)]
pub struct WasmOutput {
    /// The encoded component (`main.wasm`), exporting `wasi:cli/run@0.3.0`.
    pub module: Vec<u8>,
    /// 線形メモリの確定した配置（`emit/layout.rs`）．出力と同じモジュールから
    /// `wasmparser` で読み取った値と突き合わせられる。
    pub layout: layout::Layout,
}

/// Why a program cannot be compiled to WASM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasmError {
    pub kind: WasmErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmErrorKind {
    /// A feature the WASM target does not support yet (fibers, `suspend fn` external functions).
    Unsupported,
    /// An external function reachable from the entry takes or returns a callback (E0427).
    Callback,
    /// A `suspend fn` external function with `@external(wasm, ...)`. (E0428 was retired once the
    /// C target learned to run them; WASM reports it without a code.)
    SuspendExternal,
    /// A bug in the compiler (the IR failed a verifier, ...).
    Internal,
}

impl WasmError {
    fn new(kind: WasmErrorKind, message: impl Into<String>) -> Self {
        WasmError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Compiles lowered Core IR (self tail calls loopified, as `emela_core::lower` leaves it) to WASM.
pub fn compile(core: &emela_core::Module, entry: &Entry) -> Result<WasmOutput, Vec<WasmError>> {
    let roots: Vec<FnId> = match entry {
        Entry::Main => core
            .functions
            .iter()
            .filter(|(_, f)| f.exported && f.name == "main")
            .map(|(id, _)| id)
            .collect(),
        Entry::Tests(tests) => tests.iter().map(|t| t.function).collect(),
    };
    let (mono, copies) = emela_core::monomorphize_from(core, &roots);
    // The Flow IR functions are the Core functions in the same order, so a test keeps the index
    // of its copy.
    let entry = match entry {
        // Call the monomorphized copy itself: its name and `exported` flag can differ from the
        // input's (`main[Never]` is not exported), so looking the entry up by name is fragile.
        Entry::Main => emit::Run::Main(copies.iter().flatten().next().map(|copy| {
            emela_lir::FnId::from_raw(la_arena::RawIdx::from(u32::from(copy.into_raw())))
        })),
        Entry::Tests(tests) => emit::Run::Tests(
            tests
                .iter()
                .zip(copies)
                .filter_map(|(t, copy)| {
                    let index = u32::from(copy?.into_raw());
                    Some((
                        t.name.clone(),
                        emela_lir::FnId::from_raw(la_arena::RawIdx::from(index)),
                    ))
                })
                .collect(),
        ),
    };
    if let Err(errors) = emela_core::verify_monomorphic(&mono) {
        return Err(internal("monomorphisation", errors));
    }
    let mut lir = emela_lir::from_core(mono).map_err(|errors| internal("from_core", errors))?;
    let errors = check::unsupported(&lir, &entry.roots());
    if !errors.is_empty() {
        return Err(errors);
    }
    emela_lir::to_heap(&mut lir);
    if cfg!(debug_assertions)
        && let Err(errors) = emela_lir::verify(&lir)
    {
        return Err(internal("the Heap IR verifier", errors));
    }
    emit::emit(&lir, &entry)
}

fn internal<E: std::fmt::Display>(stage: &str, errors: Vec<E>) -> Vec<WasmError> {
    let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
    vec![WasmError::new(
        WasmErrorKind::Internal,
        format!("internal error in {stage}:\n{}", lines.join("\n")),
    )]
}
