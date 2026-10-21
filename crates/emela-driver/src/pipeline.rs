//! The pipeline: collects modules, lexes them, and checks the import graph.
//!
//! Parsing, type checking and emission (JS or C) are plugged in through [`Frontend`] and
//! [`Backend`].

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{
    DiagnosticKind, DirEntry, Import, ImportGraph, ModuleData, ModuleId, ModuleMap, SourceFs,
};
use la_arena::ArenaMap;

use crate::actions::CodeAction;
use crate::c::run_native;
use crate::code;
use crate::complete::Completion;
use crate::diagnostic::{Diagnostic, Location, error_count};
use crate::hints::InlayHint;
use crate::hover::HoverInfo;
use crate::nav::{NavTarget, RenameError};
use crate::output::{BuildOutput, write_output};
use crate::run::{Output, RunOutput, run_node, run_wasmtime};
use crate::signature::SignatureInfo;
use crate::source::{FileId, FileSystem, SourceDb, SourceFile};
use crate::target::Target;
use crate::tokens::SemanticToken;

/// The file marking a project (spec 4.1). In 0.20 only its presence matters.
pub const PROJECT_FILE: &str = "Pome.toml";
/// The source root inside a project.
pub const SOURCE_DIR: &str = "src";
/// The test root inside a project: modules only the tests use (spec 4.1, 13.1).
pub const TEST_DIR: &str = "test";
/// The project entry, relative to the source root.
pub const ENTRY_FILE: &str = "main.emel";
/// The default JS output directory, relative to the project (or to the file's directory for a
/// single file).
pub const JS_OUT_DIR: &str = "target/emela/js";
/// The default C output directory (`--target c`), relative like [`JS_OUT_DIR`].
pub const C_OUT_DIR: &str = "target/emela/c";
/// The embedded C output directory (`--target c-embedded`), relative like [`JS_OUT_DIR`].
pub const C_EMBEDDED_OUT_DIR: &str = "target/emela/c-embedded";
/// The default WASM output directory (`--target wasm`), relative like [`JS_OUT_DIR`].
pub const WASM_OUT_DIR: &str = "target/emela/wasm";
/// The default output directory of `emela doc`, relative like [`JS_OUT_DIR`].
pub const DOC_OUT_DIR: &str = "target/emela/doc";
/// A virtual directory holding the bundled core sources. Diagnostics show paths like
/// `<core>/list.emel`.
pub const CORE_DIR: &str = "<core>";

/// Core sources bundled with the compiler (module name and contents), passed through
/// [`Frontend::core_sources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSource {
    /// The module name (`List`, `Prelude`), as given by `emela_core::core_sources()`.
    pub module: String,
    pub text: String,
}

impl CoreSource {
    /// All of `emela_core::core_sources()` (the Prelude and core modules).
    pub fn bundled() -> Vec<CoreSource> {
        emela_core::core_sources()
            .iter()
            .map(|(module, text)| CoreSource {
                module: (*module).to_owned(),
                text: (*text).to_owned(),
            })
            .collect()
    }

    /// The virtual path in the source table: `<core>/list.emel` for `List`, `<core>/int64.emel`
    /// for `Int64`, and `<core>/std/console.emel` for `Std.Console` (module name segments become
    /// directories).
    fn path(&self) -> PathBuf {
        let mut path = PathBuf::from(CORE_DIR);
        let segments: Vec<&str> = self.module.split('.').collect();
        for (i, segment) in segments.iter().enumerate() {
            let mut file = String::new();
            for (j, c) in segment.chars().enumerate() {
                if c.is_ascii_uppercase() && j > 0 {
                    file.push('_');
                }
                file.push(c.to_ascii_lowercase());
            }
            if i + 1 == segments.len() {
                file.push_str(".emel");
            }
            path.push(file);
        }
        path
    }
}

/// The source root and entry decided from the command line path (spec 4.1). All paths are
/// absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// Module names are paths relative to this.
    pub root: PathBuf,
    /// `None` when a directory is passed and it has neither `src/main.emel` nor `src/lib.emel`.
    pub entry: Option<PathBuf>,
    /// A directory-selected project with `src/lib.emel` but no `src/main.emel`.
    /// Build only checks it; run rejects it even if the library defines a fn named `main`.
    /// Explicit file inputs are never library-only.
    pub library_only: bool,
    /// The project directory, or the file's directory for a single file. `target/` goes here,
    /// and diagnostic paths are relative to it.
    pub base: PathBuf,
    /// A single file without `Pome.toml`. Only the `.emel` files directly under the root are
    /// modules.
    pub single_file: bool,
    /// The Pomes the project depends on, resolved from `Pome.lock` by the caller (spec 4.7).
    pub dependencies: Dependencies,
    /// The test root to read (`<project>/test`), set by [`Input::with_tests`] for check, test and
    /// the language server. build and run leave it out (spec 13.1).
    pub tests: Option<PathBuf>,
}

/// The Pomes a project depends on, directly or not (spec 4.7). The caller (emela pome) resolves
/// them from `Pome.lock` and gets their files; the pipeline adds their modules under root names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dependencies {
    pub pomes: Vec<DependencyPome>,
    /// The project's `[dependencies]`.
    pub project: Vec<DependencyRef>,
    /// The project's `[dev-dependencies]`, for the test root only. Empty unless the tests are
    /// read.
    pub dev: Vec<DependencyRef>,
    /// The project's `Pome.toml`, where root name errors of the project are reported.
    pub manifest: PathBuf,
    /// `[external] allow` of the project: the source paths of the Pomes whose `@external` it
    /// allows.
    pub allow_external: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyPome {
    /// The source path (`github.com/emela-lang/quince`).
    pub name: String,
    /// The directory with its `Pome.toml`; its modules are under `src`.
    pub dir: PathBuf,
    /// The last element of the source path without `/vN` (`quince`). The default root name.
    pub last_element: String,
    /// `[pome].module`: the root name its author chose.
    pub module: Option<String>,
    /// `[pome].emela`: the oldest compiler it needs.
    pub emela: Option<String>,
    /// `[pome].targets`: the targets it runs on; `None` for all of them.
    pub targets: Option<Vec<String>>,
    /// The locked version (`v0.2.0`), or `replace` for a `[replace]` directory. Names its
    /// directory in the output (spec 12.1).
    pub version: String,
    /// Its `[dependencies]`.
    pub dependencies: Vec<DependencyRef>,
}

/// An entry of `[dependencies]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyRef {
    /// Index into [`Dependencies::pomes`].
    pub pome: usize,
    /// The root name the depending side gives it (`module` of the entry).
    pub module: Option<String>,
}

impl Input {
    /// Makes the given path absolute and walks up its ancestors; the first `Pome.toml` decides
    /// the project.
    ///
    /// - A directory without `Pome.toml` is an error. The entry is `<project>/src/main.emel`,
    ///   falling back to `src/lib.emel` for a library-only project.
    /// - A file without `Pome.toml` is a single file, rooted at the file's directory.
    /// - With `Pome.toml`, a file outside `<project>/src` and `<project>/test`, or a directory
    ///   that is neither the project directory nor under one of them, is an error (E0208).
    pub fn resolve(fs: &dyn FileSystem, path: &Path) -> Result<Input, Diagnostic> {
        let not_found = || {
            Diagnostic::error(format!("`{}` not found", path.display()))
                .with_code(code::PATH_NOT_FOUND)
        };
        let absolute = fs.absolute(path).map_err(|_| not_found())?;
        let is_file = fs.is_file(&absolute);
        if !is_file && !fs.is_dir(&absolute) {
            return Err(not_found());
        }
        let start = if is_file {
            absolute.parent().expect("a file has a parent")
        } else {
            &absolute
        };
        let project = start
            .ancestors()
            .find(|dir| fs.is_file(&dir.join(PROJECT_FILE)))
            .map(Path::to_owned);

        match (project, is_file) {
            (Some(project), _) => {
                let root = project.join(SOURCE_DIR);
                // The project directory itself means the whole project. Any other directory
                // outside the source and test roots would be ignored, so it is rejected rather
                // than silently checking `src` instead (spec appendix B).
                let inside = absolute.starts_with(&root)
                    || absolute.starts_with(project.join(TEST_DIR))
                    || (!is_file && absolute == project);
                if !inside {
                    let kind = if is_file { "file" } else { "directory" };
                    return Err(Diagnostic::error(format!(
                        "{kind} `{}` is outside the source root `{SOURCE_DIR}`",
                        path.display()
                    ))
                    .with_code(code::OUTSIDE_SOURCE_ROOT)
                    .with_note(format!(
                        "source files of a project (the directory with {PROJECT_FILE}) go under {SOURCE_DIR}/, and those only the tests use under {TEST_DIR}/; pass the project directory, or a file or directory under one of them"
                    )));
                }
                let entry = if is_file {
                    Some(absolute)
                } else {
                    let entry = root.join(ENTRY_FILE);
                    if fs.is_file(&entry) {
                        Some(entry)
                    } else {
                        // A library-only project still has an entry module for checking and
                        // test discovery. It is not an executable entry, even if it defines
                        // a fn named `main`; `run` will report E0230.
                        let library = root.join("lib.emel");
                        fs.is_file(&library).then_some(library)
                    }
                };
                let library_only = !is_file && entry.as_deref() == Some(&root.join("lib.emel"));
                Ok(Input {
                    root,
                    entry,
                    library_only,
                    base: project,
                    single_file: false,
                    dependencies: Dependencies::default(),
                    tests: None,
                })
            }
            (None, true) => {
                let dir = start.to_owned();
                Ok(Input {
                    root: dir.clone(),
                    entry: Some(absolute),
                    library_only: false,
                    base: dir,
                    single_file: true,
                    dependencies: Dependencies::default(),
                    tests: None,
                })
            }
            (None, false) => Err(Diagnostic::error(format!(
                "no {PROJECT_FILE} found in `{}` or its parent directories",
                path.display()
            ))
            .with_code(code::NO_PROJECT_FILE)
            .with_note(format!(
                "put a {PROJECT_FILE} (it may be empty) in the project directory, or pass a single file"
            ))),
        }
    }

    /// Reads the test root too, if the project has one (check, test and the language server).
    pub fn with_tests(mut self, fs: &dyn FileSystem) -> Input {
        let dir = self.base.join(TEST_DIR);
        if !self.single_file && fs.is_dir(&dir) {
            self.tests = Some(dir);
        }
        self
    }

    pub fn default_out_dir(&self) -> PathBuf {
        self.base.join(JS_OUT_DIR)
    }

    /// The default output directory of build and run for the target.
    pub fn default_out_dir_for(&self, target: Target) -> PathBuf {
        match target {
            Target::Js => self.base.join(JS_OUT_DIR),
            Target::C => self.base.join(C_OUT_DIR),
            Target::CEmbedded => self.base.join(C_EMBEDDED_OUT_DIR),
            Target::Wasm => self.base.join(WASM_OUT_DIR),
        }
    }

    pub fn default_doc_out_dir(&self) -> PathBuf {
        self.base.join(DOC_OUT_DIR)
    }
}

/// The listing for a single file: shows only files directly under the root, never
/// subdirectories.
struct Shallow<'a>(&'a dyn FileSystem);

impl SourceFs for Shallow<'_> {
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = self.0.read_dir(dir)?;
        entries.retain(|entry| !entry.is_dir);
        Ok(entries)
    }
}

/// The parts of the parse result the pipeline uses. The syntax tree stays in the [`Frontend`].
#[derive(Debug, Default)]
pub struct Parsed {
    pub imports: Vec<Import>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The plug-in point for the parser and type checker.
pub trait Frontend {
    /// A type-checked program, passed to [`Backend`].
    type Program;

    /// Lexes and parses one module, returning both lexer and parser diagnostics (the pipeline
    /// does no lexing itself). Keeps the tree in `self` for [`Frontend::check`].
    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed;

    /// Called once after all read modules are parsed and imports checked. `order` is the
    /// dependency order of the read modules only. Not called if imports are cyclic, since there
    /// is no dependency order.
    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program>;

    /// Core sources bundled with the compiler. The pipeline adds them to the module table and,
    /// like source modules, always reads them (even for a single file) and passes them to
    /// [`Frontend::parse`].
    fn core_sources(&self) -> Vec<CoreSource> {
        Vec::new()
    }

    /// Information at `offset` in `file` (LSP hover). Only frontends with types return it.
    fn hover(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Option<HoverInfo> {
        None
    }

    /// Whether the entry module had a fn named `main` in the last [`Frontend::check`] (spec 8.3).
    /// `emela run` uses it for E0230. `None` for frontends that cannot tell and when there is no
    /// entry (`run` then proceeds without checking).
    fn has_entry_main(&self) -> Option<bool> {
        None
    }

    /// Same-file occurrences of the name, including its declaration (LSP documentHighlight).
    fn document_highlights(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Vec<line_index::TextRange> {
        Vec::new()
    }

    /// The outer nominal type's definition (LSP typeDefinition).
    fn type_definition(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Option<NavTarget> {
        None
    }

    /// Explicit implementations of a trait or trait method (LSP implementation).
    fn implementations(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Vec<NavTarget> {
        Vec::new()
    }

    /// The definition of the name at `offset` in `file` (LSP go to definition). Only frontends
    /// that resolve names return it.
    fn definition(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Option<NavTarget> {
        None
    }

    /// Completion candidates at `offset` in `file` (LSP completion). Only frontends with types
    /// return them.
    fn complete(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Vec<Completion> {
        Vec::new()
    }

    /// Inlay hints within `range` of `file` (LSP inlay hints).
    fn inlay_hints(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _range: line_index::TextRange,
    ) -> Vec<InlayHint> {
        Vec::new()
    }

    /// Semantic tokens of `file` overlapping `range` (LSP semantic tokens).
    fn semantic_tokens(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _range: line_index::TextRange,
    ) -> Vec<SemanticToken> {
        Vec::new()
    }

    /// Code actions touching `range` of `file` (LSP code actions).
    fn code_actions(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _range: line_index::TextRange,
    ) -> Vec<CodeAction> {
        Vec::new()
    }

    /// The signature of the call whose parentheses contain `offset` in `file` (LSP signature
    /// help).
    fn signature_help(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Option<SignatureInfo> {
        None
    }

    /// The range of the name at `offset` in `file`, if it can be renamed (LSP prepareRename).
    fn prepare_rename(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
    ) -> Option<line_index::TextRange> {
        None
    }

    /// Edits renaming the name at `offset` in `file` to `new_name` (LSP rename).
    fn rename(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
        _new_name: &str,
    ) -> Result<Vec<(NavTarget, String)>, RenameError> {
        Err(RenameError::NotRenamable)
    }

    /// References to the name at `offset` in `file` (LSP find references). With `declaration`,
    /// the definition's name is included.
    fn references(
        _program: &Self::Program,
        _analysis: &Analysis,
        _file: FileId,
        _offset: line_index::TextSize,
        _declaration: bool,
    ) -> Vec<NavTarget> {
        Vec::new()
    }
}

/// The type checking result. The program may be returned despite errors, as long as it can
/// continue.
#[derive(Debug)]
pub struct Checked<P> {
    pub program: Option<P>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The plug-in point for the output stage (JS, or C with [`crate::CoreC`]). Called only when
/// there are no errors.
pub trait Backend<P> {
    fn emit(&mut self, program: &P, analysis: &Analysis) -> Result<BuildOutput, Vec<Diagnostic>>;

    /// Called after the output is written under `out_dir`, with the path of its entry. Returns the
    /// path of what runs: the entry itself for JS, the executable cc made for C.
    fn finish(&mut self, _out_dir: &Path, entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        Ok(entry)
    }

    /// How what [`Backend::finish`] returned runs.
    fn runner(&self) -> Runner {
        Runner::Node
    }
}

/// How the built program runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    /// Generated sources that require a separate board toolchain and runner.
    BuildOnly,
    /// A `.mjs` entry run on node.
    Node,
    /// A native executable.
    Native,
    /// A `.wasm` module run with `wasmtime` (WASI).
    Wasmtime,
}

/// A lexer-only frontend: returns only lexer diagnostics, no imports, and no type checking.
#[derive(Debug, Default, Clone, Copy)]
pub struct LexOnly;

impl Frontend for LexOnly {
    type Program = ();

    fn parse(&mut self, _: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        let lexed = emela_syntax::lex(source.text());
        Parsed {
            imports: Vec::new(),
            diagnostics: lexed
                .diagnostics
                .iter()
                .map(|d| Diagnostic::from_syntax(file, d))
                .collect(),
        }
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<()> {
        Checked {
            program: Some(()),
            diagnostics: Vec::new(),
        }
    }
}

/// Reports, as a diagnostic, that JS emission is unavailable.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoJsBackend;

impl<P> Backend<P> for NoJsBackend {
    fn emit(&mut self, _: &P, _: &Analysis) -> Result<BuildOutput, Vec<Diagnostic>> {
        Err(vec![
            Diagnostic::error("JS output is not implemented yet").with_code(code::NO_JS_BACKEND),
        ])
    }
}

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
    pub host_roots: Vec<crate::host_files::HostRoot>,
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
}

/// The `emela check` stage: collects modules, lexes and parses them, checks imports, and type
/// checks.
///
/// The module table is always complete. A project reads and analyzes every module; a single file
/// only those reachable from the entry through imports.
pub fn check<F: Frontend>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
) -> (Analysis, Option<F::Program>) {
    let mut analysis = Analysis {
        library_only: input.library_only,
        ..Analysis::default()
    };
    analysis.sources.set_base(&input.base);
    analysis.host_roots = crate::host_files::roots(input);
    let (modules, mut resolve_diagnostics) = if input.single_file {
        emela_resolve::collect_modules(&Shallow(fs), &input.root)
    } else {
        emela_resolve::collect_modules(fs, &input.root)
    };
    analysis.modules = modules;
    if let Some(tests) = &input.tests {
        resolve_diagnostics.extend(analysis.modules.add_test_root(fs, tests));
    }
    // Bundled core sources. A source module with the same name is hidden, and a reserved name
    // diagnostic is reported.
    let mut core_texts: ArenaMap<ModuleId, String> = ArenaMap::default();
    for source in frontend.core_sources() {
        let name = emela_resolve::ModuleName::parse(&source.module);
        let (id, shadowed) = analysis.modules.add_core(name, source.path());
        resolve_diagnostics.extend(shadowed);
        core_texts.insert(id, source.text);
    }
    if !input.single_file {
        let (dependency_diagnostics, more) =
            crate::dependencies::add(fs, &mut analysis.modules, &input.dependencies);
        resolve_diagnostics.extend(more);
        analysis.diagnostics.extend(dependency_diagnostics);
    }
    if let Some(entry) = &input.entry {
        analysis.entry = analysis
            .modules
            .iter()
            .find(|(_, data)| same_path(&data.file, entry))
            .map(|(id, _)| id);
    }

    // The work list. For a single file it starts at the entry and grows as imports are found.
    let mut work: Vec<ModuleId> = if input.single_file {
        let core = analysis
            .modules
            .iter()
            .map(|(id, _)| id)
            .filter(|&id| analysis.modules.is_core(id));
        analysis.entry.into_iter().chain(core).collect()
    } else {
        analysis.modules.iter().map(|(id, _)| id).collect()
    };
    let mut queued: ArenaMap<ModuleId, ()> = ArenaMap::default();
    for &id in &work {
        queued.insert(id, ());
    }
    let mut imports: ArenaMap<ModuleId, Vec<Import>> = ArenaMap::default();
    // Diagnostics are ordered by module, not by reading order.
    let mut file_diagnostics: ArenaMap<ModuleId, Vec<Diagnostic>> = ArenaMap::default();
    while let Some(module) = work.pop() {
        let path = &analysis.modules[module].file;
        let read = match core_texts.remove(module) {
            Some(text) => Ok(text),
            None => fs.read_to_string(path),
        };
        let text = match read {
            Ok(text) => text,
            Err(err) => {
                file_diagnostics.insert(
                    module,
                    vec![
                        Diagnostic::error(format!("cannot read file: {err}"))
                            .with_code(code::CANNOT_READ)
                            .at(Location::Path(path.clone())),
                    ],
                );
                continue;
            }
        };
        let file = analysis.sources.add(path, text);
        analysis.files.insert(module, file);
        let parsed = frontend.parse(module, file, &analysis.sources[file]);
        file_diagnostics.insert(module, parsed.diagnostics);
        if input.single_file {
            for import in &parsed.imports {
                if let Some(target) = analysis.modules.lookup(&import.path)
                    && !queued.contains_idx(target)
                {
                    queued.insert(target, ());
                    work.push(target);
                }
            }
        }
        imports.insert(module, parsed.imports);
    }

    // Name resolution diagnostics are converted after files are in the table, so they can point
    // by ID. For a single file, only those about reached modules are reported.
    let resolve_diagnostics: Vec<Diagnostic> = resolve_diagnostics
        .iter()
        .filter(|d| !input.single_file || concerns_reached(d, input, &analysis, &queued))
        .map(|d| Diagnostic::from_resolve(d, &analysis.sources))
        .collect();
    analysis.diagnostics.extend(resolve_diagnostics);
    for (module, _) in analysis.modules.iter() {
        if let Some(diagnostics) = file_diagnostics.remove(module) {
            analysis.diagnostics.extend(diagnostics);
        }
    }

    if let (Some(entry), None) = (&input.entry, analysis.entry)
        && input.tests.is_none()
        && entry.starts_with(input.base.join(TEST_DIR))
    {
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "`{}` is under the test root, which only check and test read",
                analysis.sources.display_path(entry).display()
            ))
            .with_code(code::OUTSIDE_SOURCE_ROOT)
            .at(Location::Path(entry.clone())),
        );
    } else if let (Some(entry), None) = (&input.entry, analysis.entry) {
        // Not a module (a bad name, ...). The resolver has already reported why.
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "the entry `{}` is not a module",
                analysis.sources.display_path(entry).display()
            ))
            .with_code(code::ENTRY_NOT_MODULE)
            .at(Location::Path(entry.clone())),
        );
    }

    let (graph, graph_diagnostics) = emela_resolve::build_import_graph(&analysis.modules, &imports);
    analysis.graph = graph;
    analysis.diagnostics.extend(
        graph_diagnostics
            .iter()
            .map(|d| Diagnostic::from_resolve(d, &analysis.sources)),
    );

    let Some(order) = analysis.graph.order() else {
        return (analysis, None);
    };
    let order: Vec<ModuleId> = order
        .iter()
        .copied()
        .filter(|&id| analysis.files.contains_idx(id))
        .collect();
    let checked = frontend.check(&analysis, &order);
    analysis.diagnostics.extend(checked.diagnostics);
    (analysis, checked.program)
}

/// For a single file, whether a name resolution diagnostic concerns the entry or a reached
/// module. Name errors only count for the entry file, and duplicates only when that module name
/// was reached.
fn concerns_reached(
    diagnostic: &emela_resolve::Diagnostic,
    input: &Input,
    analysis: &Analysis,
    reached: &ArenaMap<ModuleId, ()>,
) -> bool {
    if let DiagnosticKind::DuplicateModule { name, .. } = &diagnostic.kind
        && let Some(id) = analysis.modules.lookup(name)
        && reached.contains_idx(id)
    {
        return true;
    }
    input
        .entry
        .as_deref()
        .is_some_and(|entry| same_path(&diagnostic.file, entry))
        || analysis.sources.file_id(&diagnostic.file).is_some()
}

/// The `emela build` stage: emits JS if checks pass and writes it to `out_dir`. Returns the path
/// of the written entry. Output is produced even without main in the entry (spec 8.3), except
/// for library-only directory inputs: those are only checked, with no backend calls or output.
pub fn build<F: Frontend, B: Backend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    out_dir: &Path,
) -> (Analysis, Option<PathBuf>) {
    build_with(fs, input, frontend, backend, out_dir, false)
}

/// With `require_main`, a missing main in the entry is E0230 and nothing is emitted
/// (`emela run`, spec 8.3).
fn build_with<F: Frontend, B: Backend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    out_dir: &Path,
    require_main: bool,
) -> (Analysis, Option<PathBuf>) {
    let (mut analysis, program) = check(fs, input, frontend);
    if input.entry.is_none() {
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "the entry `{}` does not exist",
                analysis
                    .sources
                    .display_path(&input.root.join(ENTRY_FILE))
                    .display()
            ))
            .with_code(code::ENTRY_NOT_FOUND),
        );
    }
    if analysis.has_errors() {
        return (analysis, None);
    }
    // Not reported if there are other errors: a syntax error may just have hidden main.
    if require_main
        && (input.library_only || frontend.has_entry_main() == Some(false))
        && let Some(entry) = &input.entry
    {
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "entry `{}` has no `main` function",
                analysis.sources.display_path(entry).display()
            ))
            .with_code(code::ENTRY_WITHOUT_MAIN)
            .at(Location::Path(entry.clone())),
        );
        return (analysis, None);
    }
    if input.library_only {
        // A library-only build is a successful check, not an executable or a library artifact.
        // In particular, do not emit output, invoke cc, or require a lowered program.
        return (analysis, None);
    }
    let Some(program) = program else {
        // No errors but no program is a frontend bug.
        analysis.diagnostics.push(Diagnostic::error(
            "internal error: the type checker returned no program",
        ));
        return (analysis, None);
    };
    let output = match backend.emit(&program, &analysis) {
        Ok(output) => output,
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            return (analysis, None);
        }
    };
    let entry = match write_output(out_dir, &output) {
        Ok(entry) => entry,
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            return (analysis, None);
        }
    };
    match backend.finish(out_dir, entry) {
        Ok(entry) => (analysis, Some(entry)),
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            (analysis, None)
        }
    }
}

/// How `emela run` runs the program.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub out_dir: PathBuf,
    /// The node executable ([`crate::node_program`]).
    pub node: PathBuf,
    /// Arguments to the program.
    pub args: Vec<OsString>,
    pub output: Output,
}

/// The `emela run` stage: builds and runs the entry (on node for JS, the executable for C). Without main in the entry, reports
/// E0230 and does not start node (spec 8.3).
///
/// `before_node` is called once after the build and before node starts (even if the build
/// fails). The CLI prints diagnostics there, so warnings show first even for programs that never
/// end (servers). The run result is `None` if diagnostics had errors or node could not start.
/// A diagnostic about failing to start node is appended to `Analysis` after `before_node`.
pub fn run<F: Frontend, B: Backend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &RunOptions,
    before_node: impl FnOnce(&Analysis),
) -> (Analysis, Option<RunOutput>) {
    if backend.runner() == Runner::BuildOnly {
        let (mut analysis, _) = check(fs, input, frontend);
        analysis.diagnostics.push(build_only_diagnostic("run"));
        before_node(&analysis);
        return (analysis, None);
    }
    let (mut analysis, entry) = build_with(fs, input, frontend, backend, &options.out_dir, true);
    before_node(&analysis);
    let Some(entry) = entry else {
        return (analysis, None);
    };
    let result = match backend.runner() {
        Runner::BuildOnly => Err(build_only_diagnostic("run")),
        Runner::Node => run_node(&options.node, &entry, &options.args, options.output),
        Runner::Native => run_native(&entry, &options.args, options.output),
        Runner::Wasmtime => run_wasmtime(&entry, &options.args, options.output),
    };
    match result {
        Ok(output) => (analysis, Some(output)),
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

pub(crate) fn build_only_diagnostic(command: &str) -> Diagnostic {
    Diagnostic::error(format!(
        "`{command}` is not available for a backend that only generates embedded C sources"
    ))
    .with_note("help: use `emela build --target c-embedded`, then build and run the generated C with the board's toolchain and runner")
}

/// Treats `./src/main.emel` and `src/main.emel` as the same.
fn same_path(a: &Path, b: &Path) -> bool {
    fn normal(p: &Path) -> impl Iterator<Item = Component<'_>> {
        p.components().filter(|c| !matches!(c, Component::CurDir))
    }
    normal(a).eq(normal(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryFiles;

    fn input(files: &[&str], path: &str) -> Result<Input, Diagnostic> {
        let fs = MemoryFiles::new(files.iter().map(|f| (*f, "")));
        Input::resolve(&fs, Path::new(path))
    }

    fn project(root: &str, entry: Option<&str>, base: &str) -> Result<Input, Diagnostic> {
        Ok(Input {
            root: root.into(),
            entry: entry.map(Into::into),
            library_only: false,
            base: base.into(),
            single_file: false,
            dependencies: Dependencies::default(),
            tests: None,
        })
    }

    fn message(result: Result<Input, Diagnostic>) -> String {
        result.unwrap_err().message
    }

    #[test]
    fn project_from_directory() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
        ];
        let expected = project("/app/src", Some("/app/src/main.emel"), "/app");
        assert_eq!(input(&files, "/app"), expected);
        // Same for a relative path and one with `./` (in-memory files are rooted at `/`).
        assert_eq!(input(&files, "./app/"), expected);
        // A directory inside the project walks up to the same project.
        assert_eq!(input(&files, "/app/src/http"), expected);
        // Without an entry it is still a project (check does not require an entry).
        assert_eq!(
            input(&["/lib/Pome.toml", "/lib/src/util.emel"], "/lib"),
            project("/lib/src", None, "/lib")
        );
    }

    #[test]
    fn directory_without_pome_toml() {
        assert_eq!(
            message(input(&["/app/src/main.emel"], "/app")),
            "no Pome.toml found in `/app` or its parent directories"
        );
    }

    #[test]
    fn project_from_file() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
            "/app/scripts/x.emel",
        ];
        assert_eq!(
            input(&files, "/app/src/http/client.emel"),
            project("/app/src", Some("/app/src/http/client.emel"), "/app")
        );
        assert_eq!(
            message(input(&files, "/app/scripts/x.emel")),
            "file `/app/scripts/x.emel` is outside the source root `src`"
        );
    }

    #[test]
    fn directory_outside_source_root() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/scripts/x.emel",
        ];
        assert_eq!(
            message(input(&files, "/app/scripts")),
            "directory `/app/scripts` is outside the source root `src`"
        );
        assert_eq!(
            input(&files, "/app/scripts").unwrap_err().code,
            Some(code::OUTSIDE_SOURCE_ROOT)
        );
        // The project directory and directories under the source root still mean the whole
        // project.
        let expected = project("/app/src", Some("/app/src/main.emel"), "/app");
        assert_eq!(input(&files, "/app"), expected);
        assert_eq!(input(&files, "/app/src"), expected);
    }

    #[test]
    fn nearest_pome_toml_wins() {
        // For nested projects, the inner one wins.
        let files = [
            "/outer/Pome.toml",
            "/outer/src/inner/Pome.toml",
            "/outer/src/inner/src/main.emel",
        ];
        assert_eq!(
            input(&files, "/outer/src/inner/src/main.emel"),
            project(
                "/outer/src/inner/src",
                Some("/outer/src/inner/src/main.emel"),
                "/outer/src/inner"
            )
        );
    }

    #[test]
    fn single_file() {
        let files = ["/home/src/proj/main.emel", "/home/src/proj/util.emel"];
        // An ancestor named `src` without Pome.toml still means a single file.
        assert_eq!(
            input(&files, "/home/src/proj/main.emel"),
            Ok(Input {
                root: "/home/src/proj".into(),
                entry: Some("/home/src/proj/main.emel".into()),
                library_only: false,
                base: "/home/src/proj".into(),
                single_file: true,
                dependencies: Dependencies::default(),
                tests: None,
            })
        );
        assert_eq!(
            message(input(&files, "/home/missing.emel")),
            "`/home/missing.emel` not found"
        );
    }

    #[test]
    fn same_path_ignores_cur_dir() {
        assert!(same_path(
            Path::new("./src/main.emel"),
            Path::new("src/main.emel")
        ));
        assert!(!same_path(Path::new("src/a.emel"), Path::new("src/b.emel")));
    }
}
