//! モジュールの読み込み，import，名前解決，pub / opaque，命名規則の検査．
//!
//! 仕様の 2.3（名前の字句クラス），4章（モジュールと可視性）を扱う．

mod diagnostic;
mod graph;
mod module_name;
mod naming;
mod source;

pub use diagnostic::{Diagnostic, DiagnosticKind};
pub use graph::{Import, ImportEdge, ImportGraph, build_import_graph};
pub use module_name::ModuleName;
pub use naming::{NameError, is_type_name, to_type_name};
pub use source::{
    DirEntry, EXTENSION, MemoryFs, ModuleData, ModuleId, ModuleMap, OsFs, SourceFs, collect_modules,
};

/// 暗黙に見えるモジュールの名前．今は予約するだけで，中身はまだない．
pub const PRELUDE: &str = "Prelude";
