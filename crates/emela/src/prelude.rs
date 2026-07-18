//! The embedded Core Prelude (spec 0021) and embedded std modules (spec 0038).
//!
//! The compiler carries the Core Prelude source in the binary and merges it into
//! every compilation unit, so the operator traits (spec 0020) and their built-in
//! `Int`/`Float`/`String` instances are always in scope. This is what lets
//! `1 + 2` compile with no explicit import even though operators are no longer
//! built into the compiler.
//!
//! Spec 0038 extends the same treatment to every std module that declares
//! `intrinsic fn` (spec 0021) or standard-registry `extern fn` (spec 0013):
//! those declarations are version-locked to the backends that supply them, so
//! the modules ship inside the compiler and resolve as `std.<name>` with no
//! `--package`. Unlike the prelude they are not merged implicitly; programs
//! import them explicitly (`import std.io`).

/// The Core Prelude module name. Built-in types are considered "owned" by this
/// module for the orphan rule (spec 0020).
pub(crate) const CORE_MODULE: &str = "core";

/// The Core Prelude source, embedded and parsed on every compile.
pub(crate) const CORE_SRC: &str = include_str!("std/core.emel");

/// The embedded std modules (spec 0038), keyed by their `std.<name>` module
/// name. Sorted by name.
pub(crate) const EMBEDDED_STD: &[(&str, &str)] = &[
    ("clock", include_str!("std/clock.emel")),
    ("float", include_str!("std/float.emel")),
    ("io", include_str!("std/io.emel")),
    ("string", include_str!("std/string.emel")),
];

/// The source of the embedded std module `name`, or `None` when `name` is not
/// embedded (and so resolves through packages as before).
pub(crate) fn embedded_std_source(name: &str) -> Option<&'static str> {
    EMBEDDED_STD
        .iter()
        .find(|(module, _)| *module == name)
        .map(|(_, source)| *source)
}
