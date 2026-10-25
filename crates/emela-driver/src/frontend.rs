//! Frontends plugging in emela-syntax's parser, name resolution and type checking.

use emela_resolve::hir::{DefId, DefKind, Program};
use emela_resolve::{BuiltinModules, Import, ModuleId, ModuleName, NoBuiltins};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{Parse, SyntaxKind};
use emela_types::{CheckOptions, ProgramTypes, check_program_with};
use la_arena::ArenaMap;

use crate::actions::CodeAction;
use crate::complete::Completion;
use crate::diagnostic::{Diagnostic, Location, error_count};
use crate::hints::InlayHint;
use crate::hover::HoverInfo;
use crate::internal::{check_external_in_dependencies, check_internal};
use crate::nav::{NavTarget, RenameError};
use crate::pipeline::{Analysis, Checked, CoreSource, Frontend, Parsed};
use crate::signature::SignatureInfo;
use crate::source::{FileId, SourceFile};
use crate::target::Target;
use crate::testing::{TestDef, collect_tests, resolve_tests};
use crate::tokens::SemanticToken;

/// A frontend up to parsing. Keeps a syntax tree per module and returns the import list.
/// [`Frontend::check`] returns `()` without diagnostics.
#[derive(Debug, Default)]
pub struct ParseOnly {
    parses: ArenaMap<ModuleId, Parse>,
}

impl ParseOnly {
    pub fn new() -> Self {
        Self::default()
    }

    /// `None` for modules that were not read.
    pub fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        self.parses.get(module)
    }
}

impl Frontend for ParseOnly {
    type Program = ();

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        let parse = emela_syntax::parse(source.text());
        let diagnostics = parse
            .diagnostics()
            .iter()
            .map(|d| Diagnostic::from_syntax(file, d))
            .collect();
        let imports = imports(&parse);
        self.parses.insert(module, parse);
        Parsed {
            imports,
            diagnostics,
        }
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<()> {
        Checked {
            program: Some(()),
            diagnostics: Vec::new(),
        }
    }
}

/// A frontend that resolves names after parsing ([`ParseOnly`]) and returns the resolver's
/// output (HIR) as the program.
///
/// Builtin modules (`String`, `List`, ...) come from `B`.
pub struct Resolve<B = NoBuiltins> {
    parse: ParseOnly,
    builtins: B,
    core: Vec<CoreSource>,
}

/// Reads the bundled core sources (`emela_core::core_sources()`). The compiler's default.
impl Default for Resolve<NoBuiltins> {
    fn default() -> Self {
        Resolve::new(NoBuiltins).with_core(CoreSource::bundled())
    }
}

impl<B: BuiltinModules> Resolve<B> {
    pub fn new(builtins: B) -> Self {
        Resolve {
            parse: ParseOnly::new(),
            builtins,
            core: Vec::new(),
        }
    }

    /// `None` for modules that were not read.
    pub fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        self.parse.parse_of(module)
    }

    /// Passes the bundled core sources ([`Frontend::core_sources`]).
    pub fn with_core(mut self, core: Vec<CoreSource>) -> Self {
        self.core = core;
        self
    }
}

impl<B: BuiltinModules> Frontend for Resolve<B> {
    type Program = Program;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.parse.parse(module, file, source)
    }

    fn core_sources(&self) -> Vec<CoreSource> {
        self.core.clone()
    }

    fn check(&mut self, analysis: &Analysis, _: &[ModuleId]) -> Checked<Program> {
        // Modules not read (unreachable from the entry of a single file) count as empty.
        let mut trees = ArenaMap::default();
        for (module, _) in analysis.modules.iter() {
            if let Some(parse) = self.parse.parse_of(module) {
                trees.insert(module, parse.tree());
            }
        }
        let (program, diagnostics) =
            emela_resolve::resolve(&analysis.modules, &trees, &self.builtins);
        Checked {
            program: Some(program),
            diagnostics: diagnostics
                .iter()
                .map(|d| Diagnostic::from_resolve(d, &analysis.sources))
                .collect(),
        }
    }
}

/// Turns the tree's imports into pairs of module name and path range.
///
/// Both `import A.B` and `import A.B.{x, Y}` name the module `A.B`. Unreadable imports are
/// skipped (a syntax diagnostic has been reported).
fn imports(parse: &Parse) -> Vec<Import> {
    parse
        .tree()
        .imports()
        .filter(|import| !is_broken(import))
        .filter_map(|import| {
            let path = import.path()?;
            Some(Import {
                path: ModuleName::new(path.segments().map(|t| t.text().to_owned())),
                range: path.syntax().text_range(),
            })
        })
        .collect()
}

/// An import unreadable due to syntax errors: it contains an ERROR, its `.{` is unclosed, or
/// something is left right after the statement (the `.` of `import A.` becomes an ERROR outside
/// the statement).
fn is_broken(import: &ast::Import) -> bool {
    let node = import.syntax();
    let has_error = node.descendants().any(|n| n.kind() == SyntaxKind::ERROR);
    let unclosed = node
        .children()
        .find(|n| n.kind() == SyntaxKind::IMPORT_LIST)
        .is_some_and(|list| {
            !list
                .children_with_tokens()
                .any(|e| e.kind() == SyntaxKind::R_BRACE)
        });
    let trailing =
        std::iter::successors(node.next_sibling_or_token(), |e| e.next_sibling_or_token())
            .find(|e| !e.kind().is_trivia())
            .is_some_and(|e| e.kind() == SyntaxKind::ERROR);
    has_error || unclosed || trailing
}

/// A frontend that type-checks after name resolution ([`Resolve`]). The default for
/// `emela check` and others.
///
/// Type checking runs even with resolution errors (inference treats unresolved names as
/// `Ty::Error` and does not pile up errors). The program pairs the HIR with the type checking
/// results.
pub struct Typecheck<B = NoBuiltins> {
    resolve: Resolve<B>,
    /// Whether the entry module had a `main` fn in the last `check`. `None` without an entry or
    /// HIR.
    entry_main: Option<bool>,
}

/// Name resolution and type checking results, which lowering reads (see [`ProgramTypes`]).
pub struct TypedProgram {
    pub hir: Program,
    pub types: ProgramTypes,
    /// `@test` functions in source, in module order and then declaration order.
    pub tests: Vec<TestDef>,
}

impl Default for Typecheck<NoBuiltins> {
    fn default() -> Self {
        Typecheck {
            resolve: Resolve::default(),
            entry_main: None,
        }
    }
}

impl<B: BuiltinModules> Typecheck<B> {
    pub fn new(resolve: Resolve<B>) -> Self {
        Typecheck {
            resolve,
            entry_main: None,
        }
    }

    /// `None` for modules that were not read.
    pub fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        self.resolve.parse_of(module)
    }
}

impl<B: BuiltinModules> Frontend for Typecheck<B> {
    type Program = TypedProgram;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.resolve.parse(module, file, source)
    }

    fn core_sources(&self) -> Vec<CoreSource> {
        self.resolve.core_sources()
    }

    fn hover(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<HoverInfo> {
        crate::hover::hover(program, analysis, file, offset)
    }

    fn has_entry_main(&self) -> Option<bool> {
        self.entry_main
    }

    fn document_highlights(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Vec<line_index::TextRange> {
        crate::nav::document_highlights(program, analysis, file, offset)
    }

    fn type_definition(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<NavTarget> {
        crate::nav::type_definition(program, analysis, file, offset)
    }

    fn implementations(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Vec<NavTarget> {
        crate::nav::implementations(program, analysis, file, offset)
    }

    fn definition(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<NavTarget> {
        crate::nav::definition(program, analysis, file, offset)
    }

    fn inlay_hints(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        range: line_index::TextRange,
    ) -> Vec<InlayHint> {
        crate::hints::inlay_hints(program, analysis, file, range)
    }

    fn semantic_tokens(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        range: line_index::TextRange,
    ) -> Vec<SemanticToken> {
        crate::tokens::semantic_tokens(program, analysis, file, range)
    }

    fn code_actions(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        range: line_index::TextRange,
    ) -> Vec<CodeAction> {
        crate::actions::code_actions(program, analysis, file, range)
    }

    fn signature_help(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<SignatureInfo> {
        crate::signature::signature_help(program, analysis, file, offset)
    }

    fn prepare_rename(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<line_index::TextRange> {
        crate::nav::prepare_rename(program, analysis, file, offset)
    }

    fn rename(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
        new_name: &str,
    ) -> Result<Vec<(NavTarget, String)>, RenameError> {
        crate::nav::rename(program, analysis, file, offset, new_name)
    }

    fn complete(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Vec<Completion> {
        crate::complete::complete(program, analysis, file, offset)
    }

    fn references(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
        declaration: bool,
    ) -> Vec<NavTarget> {
        crate::nav::references(program, analysis, file, offset, declaration)
    }

    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<TypedProgram> {
        self.entry_main = None;
        let resolved = self.resolve.check(analysis, order);
        let Some(mut hir) = resolved.program else {
            return Checked {
                program: None,
                diagnostics: resolved.diagnostics,
            };
        };
        let mut diagnostics = resolved.diagnostics;
        // `@test` errors (E0226) and test function reference errors (E0227) are checked here too,
        // so `check` and the LSP report them.
        let (tests, mut test_diagnostics) = collect_tests(analysis, |m| self.parse_of(m));
        let (tests, ref_diagnostics) = resolve_tests(analysis, &hir, tests);
        test_diagnostics.extend(ref_diagnostics);
        // Placement of `@internal` (E0229) and references from outside core (E0228).
        test_diagnostics.extend(check_internal(analysis, &hir, |m| self.parse_of(m)));
        test_diagnostics.extend(check_external_in_dependencies(analysis, |m| {
            self.parse_of(m)
        }));
        // The R of main and test functions must be empty (spec 8.3, 13.3).
        // A library's fn named `main` is an ordinary function, not an executable boundary.
        let main = analysis
            .entry
            .filter(|_| !analysis.library_only)
            .and_then(|entry| entry_main(&hir, entry));
        self.entry_main = analysis.entry.map(|_| main.is_some());
        let options = CheckOptions {
            main,
            tests: tests.iter().map(|t| t.def).collect(),
        };
        // The exports `@entry` makes are fns that type checking infers (`[ext.entry]`).
        let export_diagnostics = emela_types::add_entry_exports(&mut hir, analysis.entry);
        let mut types = check_program_with(&hir, &options);
        types.diags.extend(export_diagnostics);
        // Inference proceeds in call order, so sort by file and position like name resolution.
        let mut type_diagnostics: Vec<Diagnostic> = types
            .diags
            .iter()
            .map(|d| Diagnostic::from_type(d, &types.ty_table.cons, &analysis.files))
            .chain(test_diagnostics)
            .chain(crate::actions::incomplete_impl_diagnostics(
                &hir,
                analysis,
                |m| self.parse_of(m),
            ))
            .collect();
        type_diagnostics.sort_by_key(|d| match &d.location {
            Some(Location::Span(span)) => (Some(span.file), span.range.start()),
            _ => (None, 0.into()),
        });
        diagnostics.extend(type_diagnostics);
        Checked {
            program: Some(TypedProgram { hir, types, tests }),
            diagnostics,
        }
    }
}

/// A frontend that lowers to Core IR after type checking ([`Typecheck`]). The default for
/// `emela build` and `run`.
///
/// Lowering is skipped if name resolution or type checking reported any error (lowering assumes
/// an error-free program), or for library-only directory inputs (build is only a check). Only
/// fns reachable from the fns the entry module exports are lowered.
pub struct Lower<B = NoBuiltins> {
    typecheck: Typecheck<B>,
    target: Target,
}

impl Default for Lower<NoBuiltins> {
    fn default() -> Self {
        Lower {
            typecheck: Typecheck::default(),
            target: Target::Js,
        }
    }
}

impl<B: BuiltinModules> Lower<B> {
    pub fn new(typecheck: Typecheck<B>) -> Self {
        Lower {
            typecheck,
            target: Target::Js,
        }
    }

    /// Checks external functions against `target` (spec 12.1) instead of JS.
    pub fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
}

impl<B: BuiltinModules> Frontend for Lower<B> {
    type Program = emela_core::Module;

    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        self.typecheck.parse(module, file, source)
    }

    fn core_sources(&self) -> Vec<CoreSource> {
        self.typecheck.core_sources()
    }

    fn has_entry_main(&self) -> Option<bool> {
        self.typecheck.has_entry_main()
    }

    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<emela_core::Module> {
        let checked = self.typecheck.check(analysis, order);
        let mut diagnostics = checked.diagnostics;
        let program = checked.program.and_then(|typed| {
            // Library-only builds are checks, so do not lower exports or validate backend-
            // specific external functions. `LowerTests` still lowers the selected test roots.
            if analysis.library_only {
                return None;
            }
            lower_typed(
                analysis,
                &typed,
                emela_core::Roots::Exported,
                self.target,
                &mut diagnostics,
            )
            .map(|lowered| lowered.module)
        });
        Checked {
            program,
            diagnostics,
        }
    }
}

/// Lowers type checking results to Core IR. Returns `None` without calling lowering if
/// `diagnostics` has any error, or if there is no entry outside the tests (a project without
/// `main.emel` and `lib.emel` still runs its tests). Lowering errors are added to `diagnostics`,
/// returning `None`.
///
/// External functions are checked (spec 12.1) for `target`. Only build, run and test call
/// lowering (check and the LSP do not).
///
/// Lowering also gets each module's source path (`LowerInput::paths`), so that the file an
/// `@external` names is resolved next to its source.
pub(crate) fn lower_typed(
    analysis: &Analysis,
    typed: &TypedProgram,
    roots: emela_core::Roots,
    target: Target,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<emela_core::Lowered> {
    let entry = analysis.entry;
    if entry.is_none() && !matches!(roots, emela_core::Roots::Tests) {
        return None;
    }
    if error_count(diagnostics) > 0 {
        return None;
    }
    let mut sources = ArenaMap::default();
    let mut paths = ArenaMap::default();
    let mut shown_paths = ArenaMap::default();
    for (module, &file) in analysis.files.iter() {
        let source = &analysis.sources[file];
        sources.insert(module, source.text());
        paths.insert(module, source.path());
        shown_paths.insert(
            module,
            analysis
                .sources
                .display_path(source.path())
                .display()
                .to_string(),
        );
    }
    let input = emela_core::LowerInput {
        hir: &typed.hir,
        types: &typed.types,
        modules: &analysis.modules,
        sources,
        entry,
        tests: typed.tests.iter().map(|t| t.def).collect(),
        paths,
        shown_paths,
        bundled_files: emela_core::bundled_files(),
        target: Some(target.name()),
    };
    match emela_core::lower(&input, roots) {
        Ok(lowered) => {
            // Ill-typed IR is a lowering bug, not an error in the program. Lowering monomorphizes
            // (it is given a target) and checks the polymorphic IR first.
            #[cfg(debug_assertions)]
            if let Err(errors) = emela_core::verify_monomorphic(&lowered.module) {
                let lines: Vec<String> = errors.iter().map(ToString::to_string).collect();
                panic!(
                    "lowering produced ill-typed IR (a compiler bug):\n{}",
                    lines.join("\n")
                );
            }
            Some(lowered)
        }
        Err(errors) => {
            diagnostics.extend(
                errors
                    .iter()
                    .map(|e| Diagnostic::from_lower(e, &analysis.files)),
            );
            None
        }
    }
}

/// The entry module's `main` (a top-level fn).
fn entry_main(hir: &Program, entry: ModuleId) -> Option<DefId> {
    hir.modules.get(entry)?.items.iter().copied().find(|&def| {
        let data = &hir.defs[def];
        data.name == "main" && data.kind == DefKind::Fn
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (module name, range text) for each import.
    fn imports_of(src: &str) -> Vec<(String, String)> {
        imports(&emela_syntax::parse(src))
            .into_iter()
            .map(|i| (i.path.to_string(), src[i.range].to_owned()))
            .collect()
    }

    #[test]
    fn path_with_and_without_names() {
        assert_eq!(
            imports_of("import Http.Client\nimport Data.Json.{Json, decode}\nimport Util\n"),
            [
                ("Http.Client".into(), "Http.Client".into()),
                ("Data.Json".into(), "Data.Json".into()),
                ("Util".into(), "Util".into()),
            ]
        );
    }

    #[test]
    fn broken_imports_are_skipped() {
        let src = "import http\nimport Http.\nimport\nimport Json.{x\nimport Ok\n";
        assert!(!emela_syntax::parse(src).diagnostics().is_empty());
        assert_eq!(imports_of(src), [("Ok".into(), "Ok".into())]);
    }

    #[test]
    fn import_after_declaration_still_counts() {
        // E0127 is a placement error; the import itself is readable.
        let src = "let x = 1\nimport Json\n";
        assert_eq!(imports_of(src), [("Json".into(), "Json".into())]);
    }
}
