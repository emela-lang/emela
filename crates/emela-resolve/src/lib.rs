//! モジュールの読み込み，import，名前解決，pub / opaque，命名規則の検査．
//!
//! 仕様の 2.3（名前の字句クラス），4章（モジュールと可視性）を扱う．名前解決の結果は
//! 名前がすべて定義を指している木（[`hir`]）で，型推論はこれだけを見る．

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
    DirEntry, EXTENSION, MemoryFs, ModuleData, ModuleId, ModuleMap, OsFs, SourceFs, collect_modules,
};

/// 暗黙に見えるモジュールの名前．ソースのルート直下には置けない（4.1）．中身は [`hir::Prelude`]．
pub const PRELUDE: &str = "Prelude";
