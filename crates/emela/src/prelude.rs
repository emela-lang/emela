//! The embedded Core Prelude (spec 0021).
//!
//! The compiler carries the Core Prelude source in the binary and merges it into
//! every compilation unit, so the operator traits (spec 0020) and their built-in
//! `Int`/`Float`/`String` instances are always in scope. This is what lets
//! `1 + 2` compile with no explicit import even though operators are no longer
//! built into the compiler.

/// The Core Prelude module name. Built-in types are considered "owned" by this
/// module for the orphan rule (spec 0020).
pub(crate) const CORE_MODULE: &str = "core";

/// The Core Prelude source, embedded and parsed on every compile.
pub(crate) const CORE_SRC: &str = include_str!("std/core.emel");
