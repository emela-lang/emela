//! WebAssembly bindings that run the Emela compiler in the browser.
//!
//! The single [`compile`] entry point takes Emela source and a target name and
//! returns a JSON string describing the result, so the JavaScript side never
//! has to reach across the wasm-bindgen boundary for anything but a string.

use emela::EmitMode;
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Diagnostic label used for playground sources.
const LABEL: &str = "playground.emel";

/// The shape returned to JavaScript (as JSON).
#[derive(Serialize)]
struct CompileResult {
    /// Whether compilation succeeded.
    ok: bool,
    /// A tag describing `output`: `text`, `ir`, `js`, `wat`, or `error`.
    kind: &'static str,
    /// The rendered output (IR / JS / WAT / a success note). Empty on error.
    output: String,
    /// The rendered diagnostic, or `null` on success.
    error: Option<String>,
}

impl CompileResult {
    fn ok(kind: &'static str, output: String) -> Self {
        Self {
            ok: true,
            kind,
            output,
            error: None,
        }
    }

    fn err(message: String) -> Self {
        Self {
            ok: false,
            kind: "error",
            output: String::new(),
            error: Some(message),
        }
    }
}

/// Installs a panic hook that forwards Rust panics to the browser console.
/// Called automatically on first [`compile`]; safe to call repeatedly.
fn install_panic_hook() {
    use std::sync::Once;
    static HOOK: Once = Once::new();
    HOOK.call_once(console_error_panic_hook::set_once);
}

/// Compiles `source` for `target` and returns a JSON [`CompileResult`].
///
/// `target` is one of:
/// - `"check"` — type-check only
/// - `"ir"`    — lowered codegen IR, as text
/// - `"js"`    — JavaScript source (the `js-node` backend)
/// - `"wasm"`  — WebAssembly text format / WAT (the `wasm-wasi` backend)
#[wasm_bindgen]
pub fn compile(source: &str, target: &str) -> String {
    install_panic_hook();
    let result = dispatch(source, target);
    serde_json::to_string(&result).unwrap_or_else(|err| {
        format!(
            "{{\"ok\":false,\"kind\":\"error\",\"output\":\"\",\"error\":\"failed to serialize result: {err}\"}}"
        )
    })
}

fn dispatch(source: &str, target: &str) -> CompileResult {
    match target {
        "check" => match emela::check_source(LABEL, source) {
            Ok(()) => CompileResult::ok("text", "Type check passed.".to_string()),
            Err(err) => CompileResult::err(err.to_string()),
        },
        "ir" => match emela::ir_source(LABEL, source) {
            Ok(text) => CompileResult::ok("ir", text),
            Err(err) => CompileResult::err(err.to_string()),
        },
        "js" => emit("js-node", "js", source, EmitMode::Default),
        "wasm" => emit("wasm-wasi", "wat", source, EmitMode::Text),
        other => CompileResult::err(format!(
            "unknown target `{other}` (expected `check`, `ir`, `js`, or `wasm`)"
        )),
    }
}

fn emit(backend: &str, kind: &'static str, source: &str, mode: EmitMode) -> CompileResult {
    match emela::compile_source(LABEL, source, backend, mode) {
        Ok(artifact) => {
            CompileResult::ok(kind, String::from_utf8_lossy(&artifact.bytes).into_owned())
        }
        Err(err) => CompileResult::err(err.to_string()),
    }
}

/// The shape returned to JavaScript by [`run`] (as JSON).
#[derive(Serialize)]
struct RunResult {
    /// Whether compilation succeeded (execution was attempted). A compile or
    /// type error sets this to `false`; a program that runs but traps is still
    /// `true` (see `trap`).
    ok: bool,
    /// Everything the program wrote to stdout.
    stdout: String,
    /// Everything the program wrote to stderr.
    stderr: String,
    /// The process exit code, when the program exited cleanly.
    exit_code: Option<i32>,
    /// A runtime trap message (`panic` / `unreachable`), or `null`.
    trap: Option<String>,
    /// The rendered compile / type diagnostic, or `null` on success.
    error: Option<String>,
}

/// Compiles `source` with the `wasm-wasi` backend and runs it in-process via
/// the embedded `wasmi` interpreter — the playground's "Run" button — returning
/// a JSON [`RunResult`] with the captured stdout/stderr.
///
/// This mirrors `emela run`. Compile / type errors populate `error`; a program
/// that runs but traps populates `trap`. Networking is unavailable in the
/// browser, so programs that use sockets / HTTP trap at run time.
#[wasm_bindgen]
pub fn run(source: &str) -> String {
    install_panic_hook();
    let result = match emela::run_source(LABEL, source) {
        Ok(out) => RunResult {
            ok: true,
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.exit_code,
            trap: out.trap,
            error: None,
        },
        Err(err) => RunResult {
            ok: false,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            trap: None,
            error: Some(err.to_string()),
        },
    };
    serde_json::to_string(&result).unwrap_or_else(|err| {
        format!(
            "{{\"ok\":false,\"stdout\":\"\",\"stderr\":\"\",\"exit_code\":null,\"trap\":null,\"error\":\"failed to serialize result: {err}\"}}"
        )
    })
}
