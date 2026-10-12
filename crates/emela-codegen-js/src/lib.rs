//! Emits JS from the Core IR.
//!
//! Functions that reach a suspend operation become generators (spec 16.5). The JS runtime is
//! embedded with `include_str!` and written out alongside the output.
//!
//! The output is an ES module. Public functions are exported under their Emela names.

mod emit;
mod test_runner;

use emela_core::Module;

pub use test_runner::{TEST_EVENT_PREFIX, TEST_RUNNER, TestEntry, emit_test_main};

/// The JS runtime source (an ES module).
pub const RUNTIME: &str = include_str!("runtime.mjs");

/// Default file name when the runtime is written next to the output.
pub const RUNTIME_FILE: &str = "emela_runtime.mjs";

#[derive(Debug, Clone)]
pub enum RuntimeMode {
    /// Embeds the runtime at the top of the output, so it is self-contained in one file.
    Inline,
    /// Imports the runtime from the same directory; `None` means `RUNTIME_FILE`. The caller
    /// writes it out from `RUNTIME`.
    Import(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub runtime: RuntimeMode,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            runtime: RuntimeMode::Import(None),
        }
    }
}

/// Self tail call loopification (`emela_core::tail::loopify`) must already be done by the caller.
/// Panics on invalid IR (operator and type mismatches, wrong argument counts, ...).
pub fn emit_module(module: &Module, options: &Options) -> String {
    emit::emit_module(module, options)
}

/// The runtime with `export`s removed, so it can be embedded in other code.
pub fn inline_runtime() -> String {
    let mut out = String::with_capacity(RUNTIME.len());
    for line in RUNTIME.lines() {
        out.push_str(line.strip_prefix("export ").unwrap_or(line));
        out.push('\n');
    }
    out
}
