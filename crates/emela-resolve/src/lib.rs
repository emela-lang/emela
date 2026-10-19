//! Module loading, imports, name resolution, pub / opaque, and naming-rule checks.
//!
//! Covers spec 2.3 (lexical classes of names) and chapter 4 (modules and visibility). The result
//! is a tree in which every name points to its definition ([`hir`]); type inference sees only
//! this.

mod builtins;
mod diagnostic;
mod dump;
mod graph;
pub mod hir;
mod lower;
mod module_name;
mod naming;
mod resolve;
mod source;
mod suggest;

pub use builtins::{BuiltinKind, BuiltinModules, BuiltinTable, NoBuiltins};
pub use diagnostic::{Diagnostic, DiagnosticKind, Expected};
pub use dump::dump_module;
pub use graph::{Import, ImportEdge, ImportGraph, build_import_graph};
pub use module_name::ModuleName;
pub use naming::{NameError, is_type_name, to_type_name};
pub use resolve::{imports, resolve};
pub use source::{
    DirEntry, EXTENSION, MemoryFs, ModuleData, ModuleId, ModuleMap, OsFs, Package, PackageId,
    SourceFs, collect_modules,
};
pub use suggest::best_match;

/// The implicitly visible module. It cannot be placed directly under the source root (spec 4.1).
/// Its contents are [`hir::Prelude`].
pub const PRELUDE: &str = "Prelude";

/// The namespace of the bundled std modules (`Std.Console`, spec 15.2). `std.emel` and `std/`
/// cannot be placed directly under the source root (spec 4.1).
pub const STD: &str = "Std";
