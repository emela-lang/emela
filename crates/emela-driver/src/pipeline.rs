//! The pipeline: collects modules, has the [`Frontend`] parse them, and checks the import graph.
//!
//! Parsing, type checking and emission (JS or C) are plugged in through [`Frontend`] and
//! [`Backend`].

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{DiagnosticKind, DirEntry, Import, ModuleId, SourceFs};
use la_arena::ArenaMap;

use crate::check::analysis::Analysis;
use crate::diagnostic::code;
use crate::diagnostic::{Diagnostic, Location};
use crate::ide::actions::CodeAction;
use crate::ide::complete::Completion;
use crate::ide::hints::InlayHint;
use crate::ide::hover::HoverInfo;
use crate::ide::nav::{NavTarget, RenameError};
use crate::ide::signature::SignatureInfo;
use crate::ide::tokens::SemanticToken;
use crate::output::{BuildOutput, write_output};
use crate::project::{ENTRY_FILE, Input, PROJECT_FILE, TEST_DIR};
use crate::run::{
    Output, ProgramCommand, RunOutput, native_command, node_command, wasmtime_command,
    wasmtime_serve_command,
};
use crate::source::{FileId, FileSystem, SourceFile};

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

/// The parts of the parse result the pipeline uses. The syntax tree stays in the [`Frontend`].
#[derive(Debug, Default)]
pub struct Parsed {
    pub imports: Vec<Import>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The listing for a single file: shows only files directly under the root, never
/// subdirectories. Filtering the listing keeps `emela_resolve::collect_modules` unchanged for
/// single files.
struct Shallow<'a>(&'a dyn FileSystem);

impl SourceFs for Shallow<'_> {
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = self.0.read_dir(dir)?;
        entries.retain(|entry| !entry.is_dir);
        Ok(entries)
    }
}

/// The plug-in point for the parser and type checker.
pub trait Frontend {
    /// A type-checked program, passed to [`Backend`].
    type Program;

    /// Lexes and parses one module, returning both lexer and parser diagnostics (the pipeline
    /// does no lexing itself). Keeps the tree in `self` for [`Frontend::check`].
    ///
    /// Lexing is left to the frontend because `emela_syntax::parse` lexes again and returns the
    /// lexer diagnostics too; lexing in the pipeline as well would report them twice.
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
    /// `emela run` uses it for E2009. `None` for frontends that cannot tell and when there is no
    /// entry (`run` then proceeds without checking).
    fn has_entry_main(&self) -> Option<bool> {
        None
    }

    /// Whether the program is an HTTP service in the last [`Frontend::check`]
    /// (`[lib.http-service]`). `emela run` does not ask it for `main` (E2009).
    fn is_http_service(&self) -> bool {
        false
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
///
/// The trait and its output ([`BuildOutput`]) are shared by every target, so their names do not
/// say JS (they were `JsBackend` and `JsOutput`). What only JS uses keeps the JS name
/// ([`crate::CoreJs`], [`crate::JS_ENTRY_FILE`], [`NoJsBackend`]).
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
    /// Generated output that requires a separate toolchain and runner.
    BuildOnly,
    /// A `.mjs` entry run on the JS host.
    Node,
    /// A native executable.
    Native,
    /// A `.wasm` component run with `wasmtime` (WASI 0.3).
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
        Err(vec![Diagnostic::error("JS output is not implemented yet")])
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
    let mut analysis = Analysis::default();
    analysis.library_only = input.library_only;
    analysis.sources.set_base(&input.base);
    analysis.host_roots = crate::output::host_files::roots(input);
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
    // A single file has no `Pome.toml`; only the flags enable features for it (`[cmd.build]`).
    let manifest = input.base.join(PROJECT_FILE);
    let manifest_text = if input.single_file {
        Err(())
    } else {
        fs.read_to_string(&manifest).map_err(|_| ())
    };
    let (features, diagnostics) = crate::project::experimental::project_features(
        manifest_text
            .as_deref()
            .ok()
            .map(|text| (text, manifest.as_path())),
        &input.experimental,
        &mut analysis.sources,
    );
    analysis.set_experimental(features);
    analysis.diagnostics.extend(diagnostics);
    if !input.single_file {
        let (dependency_diagnostics, more) =
            crate::project::dependencies::add(fs, &mut analysis.modules, &input.dependencies);
        resolve_diagnostics.extend(more);
        analysis.diagnostics.extend(dependency_diagnostics);
        analysis.set_dependency_experimental(
            input
                .dependencies
                .pomes
                .iter()
                .map(|pome| (pome.name.clone(), pome.experimental.clone()))
                .collect(),
        );
        if let Ok(text) = &manifest_text {
            analysis.diagnostics.extend(crate::project::license::check(
                text,
                &manifest,
                &mut analysis.sources,
            ));
        }
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
                analysis.sources.display_path(entry)
            ))
            .with_code(code::OUTSIDE_SOURCE_ROOT)
            .at(Location::Path(entry.clone())),
        );
    } else if let (Some(entry), None) = (&input.entry, analysis.entry) {
        // Not a module (a bad name, ...). The resolver has already reported why.
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "the entry `{}` is not a module",
                analysis.sources.display_path(entry)
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

/// With `require_main`, a missing main in the entry is E2009 and nothing is emitted
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
                analysis.sources.display_path(&input.root.join(ENTRY_FILE))
            ))
            .with_code(code::ENTRY_NOT_FOUND),
        );
    }
    if analysis.has_errors() {
        return (analysis, None);
    }
    // Not reported if there are other errors: a syntax error may just have hidden main.
    if require_main
        && (input.library_only
            || (frontend.has_entry_main() == Some(false) && !frontend.is_http_service()))
        && let Some(entry) = &input.entry
    {
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "entry `{}` has no `main` function",
                analysis.sources.display_path(entry)
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
    /// The command of the JS host ([`crate::js_host`]).
    pub js_host: PathBuf,
    /// Arguments to the program.
    pub args: Vec<OsString>,
    pub output: Output,
}

/// The `emela run` stage: builds and runs the entry (on the JS host for JS, the executable for C). Without main in the entry,
/// reports E2009 and does not start the program (spec 8.3).
///
/// `before_start` is called once after the build and before the program starts (even if the
/// build fails), with the command that will start it (`None` if there is none). The CLI prints
/// diagnostics and the command there, so warnings show first even for programs that never end
/// (servers). The run result is `None` if diagnostics had errors or the program could not start.
/// A diagnostic about failing to start the program is appended to `Analysis` after
/// `before_start`.
pub fn run<F: Frontend, B: Backend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &RunOptions,
    before_start: impl FnOnce(&Analysis, Option<&ProgramCommand>),
) -> (Analysis, Option<RunOutput>) {
    if backend.runner() == Runner::BuildOnly {
        let (mut analysis, _) = check(fs, input, frontend);
        analysis.diagnostics.push(build_only_diagnostic("run"));
        before_start(&analysis, None);
        return (analysis, None);
    }
    let (mut analysis, entry) = build_with(fs, input, frontend, backend, &options.out_dir, true);
    let server = frontend.is_http_service();
    let command = entry.map(|entry| match backend.runner() {
        Runner::Node => crate::js_host::check_version(&options.js_host).map(|()| {
            let script = if server {
                entry.with_file_name(emela_codegen_js::SERVE_FILE)
            } else {
                entry
            };
            node_command(&options.js_host, &script, &options.args)
        }),
        Runner::Native => Ok(native_command(&entry, &options.args)),
        Runner::Wasmtime if server => wasmtime_serve_command(&entry, &options.args),
        Runner::Wasmtime => wasmtime_command(&entry, &options.args),
        Runner::BuildOnly => unreachable!("build-only backends return before generating a command"),
    });
    before_start(&analysis, command.as_ref().and_then(|c| c.as_ref().ok()));
    let Some(command) = command else {
        return (analysis, None);
    };
    match command.and_then(|command| command.run(options.output)) {
        Ok(output) => (analysis, Some(output)),
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

pub(crate) fn build_only_diagnostic(command: &str) -> Diagnostic {
    Diagnostic::error(format!(
        "`{command}` is not available for a backend that only builds output"
    ))
    .with_note("help: build the output and run it with the backend's toolchain and runner")
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

    #[test]
    fn same_path_ignores_cur_dir() {
        assert!(same_path(
            Path::new("./src/main.emel"),
            Path::new("src/main.emel")
        ));
        assert!(!same_path(Path::new("src/a.emel"), Path::new("src/b.emel")));
    }
}
