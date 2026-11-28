//! The state of a command's analysis: the sources, the module table and import graph, and the
//! diagnostics of every stage so far.

use std::collections::HashMap;

use emela_resolve::{ImportGraph, ModuleData, ModuleId, ModuleMap};
use la_arena::ArenaMap;

use crate::diagnostic::{Diagnostic, error_count};
use crate::output::host_files::HostRoot;
use crate::project::experimental::Features;
use crate::source::{FileId, SourceDb};

/// Analysis in progress. Diagnostics from every stage accumulate in stage order.
#[derive(Debug, Default)]
pub struct Analysis {
    pub sources: SourceDb,
    pub modules: ModuleMap,
    /// Files of read modules. Files that could not be read and, for a single file, modules
    /// unreachable from the entry are absent.
    pub files: ArenaMap<ModuleId, FileId>,
    pub graph: ImportGraph,
    /// `None` if there is no entry or it is not a module.
    pub entry: Option<ModuleId>,
    /// Whether the directory input selected a library-only project. Normal build lowering is
    /// skipped; test lowering still uses `entry` to collect and run the library's tests.
    pub library_only: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// Where the files `@external` points to are written in the JS output (spec 12.1).
    pub host_roots: Vec<HostRoot>,
    /// The experimental features of the project: its `[experimental]` and the flags.
    experimental: Features,
    /// The experimental features of each dependency, by its source path.
    dependency_experimental: HashMap<String, Features>,
}

impl Analysis {
    pub fn error_count(&self) -> usize {
        error_count(&self.diagnostics)
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    pub fn module(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id]
    }

    /// Sets the experimental features of the project: its `[experimental]` and the flags.
    pub(crate) fn set_experimental(&mut self, features: Features) {
        self.experimental = features;
    }

    /// Sets the experimental features of each dependency, by its source path.
    pub(crate) fn set_dependency_experimental(&mut self, features: HashMap<String, Features>) {
        self.dependency_experimental = features;
    }

    /// The experimental features enabled for a module: those of the Pome it belongs to
    /// (`[module.pome]`). The flags of the command count only for the project's modules. The
    /// bundled core modules belong to no Pome, and have none.
    pub fn experimental(&self, id: ModuleId) -> &Features {
        static NONE: std::sync::LazyLock<Features> = std::sync::LazyLock::new(Features::default);
        if self.modules.is_core(id) {
            return &NONE;
        }
        match self.modules.package_of(id) {
            Some(package) => self
                .dependency_experimental
                .get(package.name.as_str())
                .unwrap_or(&NONE),
            None => &self.experimental,
        }
    }
}
