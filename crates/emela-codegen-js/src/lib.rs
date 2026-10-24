//! Emits JS from the Core IR.
//!
//! Functions that reach a suspend operation become generators (spec 16.5). The JS runtime is
//! embedded with `include_str!` and written out alongside the output.
//!
//! The output is an ES module. `main` and the test functions are exported under their Emela names
//! for the launcher and the test runner; the fns with `@export(js, ...)` are exported under their
//! host names, as named exports or properties of the default export (`[target.js.module]`).
//!
//! # Value representation
//!
//! ```text
//! Emela                 JS
//! Int                   number, wrapped to 32 bits after every operation
//! Int64                 BigInt, wrapped with BigInt.asIntN(64, …)
//! Float                 number
//! Bool                  boolean
//! String                string
//! ()                    undefined
//! tuple                 array [a, b]
//! List                  cons cells { h, t }, the empty list is null
//! enum / type value     { $tag: variant index, $0, $1, … or named fields }
//! error value           { $tag: $err$Name, … }  ($tag is a per-error descriptor)
//! failure               the runtime's $Fail (`error` holds the error value)
//! handler value         an object of functions keyed by operation name
//! evidence table        an object of handler values keyed by effect, chained by __proto__
//! ```
//!
//! - List is cons cells rather than an array, so both the pattern `[x, ..rest]` and the
//!   construction `[x, ..rest]` are O(1).
//! - Bool is a JS boolean rather than an enum value, which would slow down `if` and `&&`.
//! - A variant without fields is one frozen value shared by the module (`Enum$Variant`). Values
//!   with fields are not frozen, for speed.
//! - A field or operation named `__proto__` is written as the computed key `["__proto__"]`, since
//!   in an object literal the plain key would set the prototype instead.
//! - A value has the same representation whatever its type. This is what lets lowering export a
//!   generic `pub fn` as a single copy with `Any` type arguments (`[target.js.module]`).

mod emit;
mod test_runner;

use emela_core::{FnId, Module};

pub use test_runner::{TEST_EVENT_PREFIX, TEST_RUNNER, TestEntry, emit_test_main};

/// The JS runtime source (an ES module).
pub const RUNTIME: &str = include_str!("runtime.mjs");

/// Default file name when the runtime is written next to the output.
pub const RUNTIME_FILE: &str = "emela_runtime.mjs";

/// The directory, next to the output, where the caller writes the bundled host files
/// (`Module::bundled_files`) under their paths. The output imports a `HostModule::Bundled` file
/// as `./emela_core/<path>`.
pub const BUNDLED_DIR: &str = "emela_core";

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

/// Self tail call loopification (`emela_core::tail::loopify`) must already be done; `emela_core::lower`
/// does it. Panics on invalid IR (operator and type mismatches, wrong argument counts, ...).
pub fn emit_module(module: &Module, options: &Options) -> String {
    emit::emit_module(module, options, &[])
}

/// Like [`emit_module`], but also exports `extra` as if they were `exported` (the selected test
/// functions of `emela test`), without copying the module to flip the flag.
pub fn emit_module_exporting(module: &Module, options: &Options, extra: &[FnId]) -> String {
    emit::emit_module(module, options, extra)
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
