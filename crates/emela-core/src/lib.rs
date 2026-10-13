//! The Core IR and lowering from the typed tree.
//!
//! Derive expansion, self tail call loopification (spec 6.8) and layer wiring (spec 8.5) are done
//! here, so backends only see desugared IR.

pub mod build;
mod dump;
pub mod intrinsics;
mod ir;
mod lower;
pub mod named;
mod sources;
pub mod tail;

#[cfg(test)]
mod tests;

pub use dump::dump;
pub use intrinsics::{Builtin, BuiltinInfo, Sig};
pub use ir::*;
pub use lower::{LowerError, LowerInput, Lowered, Roots, lower};
pub use sources::{CORE_JS_DIR, PRELUDE, STD, core_js_files, core_source, core_sources, lib_dir};
