//! emela-syntax のパーサ，名前解決，型検査を差し込むフロントエンド．

use emela_resolve::hir::{DefId, DefKind, Program};
use emela_resolve::{BuiltinModules, Import, ModuleId, ModuleName, NoBuiltins};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{Parse, SyntaxKind};
use emela_types::{CheckOptions, ProgramTypes, check_program_with};
use la_arena::ArenaMap;

use crate::complete::Completion;
use crate::diagnostic::{Diagnostic, Location, error_count};
use crate::hover::HoverInfo;
use crate::internal::check_internal;
use crate::nav::{NavTarget, RenameError};
use crate::pipeline::{Analysis, Checked, CoreSource, Frontend, Parsed};
use crate::signature::SignatureInfo;
use crate::source::{FileId, SourceFile};
use crate::testing::{TestDef, collect_tests, resolve_tests};

/// 構文解析までのフロントエンド．構文木をモジュールごとに持ち，import の一覧を返す．
/// 型検査はまだないので，[`Frontend::check`] は診断なしで `()` を返す．
#[derive(Debug, Default)]
pub struct ParseOnly {
    parses: ArenaMap<ModuleId, Parse>,
}

impl ParseOnly {
    pub fn new() -> Self {
        Self::default()
    }

    /// 構文解析したモジュールの結果．読まなかったモジュールは `None`．
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

/// 構文解析（[`ParseOnly`]）の後に名前を解決するフロントエンド．型検査はまだないので，
/// 名前解決の結果（HIR）をそのままプログラムとして返す．
///
/// 組み込みのモジュール（`String`，`List` など）は `B` から引く．
pub struct Resolve<B = NoBuiltins> {
    parse: ParseOnly,
    builtins: B,
    core: Vec<CoreSource>,
}

/// 同梱の core のソース（`emela_core::core_sources()`）を読むフロントエンド．コンパイラの既定．
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

    /// 構文解析したモジュールの結果．読まなかったモジュールは `None`．
    pub fn parse_of(&self, module: ModuleId) -> Option<&Parse> {
        self.parse.parse_of(module)
    }

    /// 同梱の core のソースを渡す（[`Frontend::core_sources`]）．
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
        // 読まなかったモジュール（単独ファイルでエントリからたどれないもの）は空とみなされる．
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

/// 構文木の import を，モジュール名とパスの範囲の組にする．
///
/// `import A.B` も `import A.B.{x, Y}` もモジュール名は `A.B`．読めない import は飛ばす
/// （構文の診断が出ている）．
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

/// 構文の誤りで読めない import．中に ERROR がある，`.{` が閉じていない，
/// または文の直後に読み残しがある（`import A.` の `.` は文の外の ERROR になる）．
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

/// 名前解決（[`Resolve`]）の後に型検査をするフロントエンド．`emela check` などの既定．
///
/// 型検査は名前解決に誤りがあっても行う（解決できなかった名前は型推論が `Ty::Error` として
/// 扱い，誤りを重ねない）．プログラムは HIR と型検査の結果の組．
pub struct Typecheck<B = NoBuiltins> {
    resolve: Resolve<B>,
    /// 直前の `check` で，エントリのモジュールに `main` の fn があったか．エントリか HIR がなければ `None`．
    entry_main: Option<bool>,
}

/// 名前解決と型検査の結果．lowering はこの2つを読む（emela-types の NOTES.md「結果の形」）．
pub struct TypedProgram {
    pub hir: Program,
    pub types: ProgramTypes,
    /// ソースの `@test` の関数．モジュールの順，宣言の順．
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

    /// 構文解析したモジュールの結果．読まなかったモジュールは `None`．
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

    fn definition(
        program: &TypedProgram,
        analysis: &Analysis,
        file: FileId,
        offset: line_index::TextSize,
    ) -> Option<NavTarget> {
        crate::nav::definition(program, analysis, file, offset)
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
        let Some(hir) = resolved.program else {
            return Checked {
                program: None,
                diagnostics: resolved.diagnostics,
            };
        };
        let mut diagnostics = resolved.diagnostics;
        // `@test` の誤り（E0226）とテスト関数の参照の誤り（E0227）も，check と LSP で出すためにここで見る．
        let (tests, mut test_diagnostics) = collect_tests(analysis, |m| self.parse_of(m));
        let (tests, ref_diagnostics) = resolve_tests(analysis, &hir, tests);
        test_diagnostics.extend(ref_diagnostics);
        // `@internal` の置き場所（E0229）と core の外からの参照（E0228）．
        test_diagnostics.extend(check_internal(analysis, &hir, |m| self.parse_of(m)));
        // main とテスト関数の R は空でなければならない（8.3，13.3）．
        let main = analysis.entry.and_then(|entry| entry_main(&hir, entry));
        self.entry_main = analysis.entry.map(|_| main.is_some());
        let options = CheckOptions {
            main,
            tests: tests.iter().map(|t| t.def).collect(),
        };
        let types = check_program_with(&hir, &options);
        // 型推論は呼び出しの順に進むので，名前解決と同じくファイルと位置の順に並べ直す．
        let mut type_diagnostics: Vec<Diagnostic> = types
            .diags
            .iter()
            .map(|d| Diagnostic::from_type(d, &types.ty_table.cons, &analysis.files))
            .chain(test_diagnostics)
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

/// 型検査（[`Typecheck`]）の後に Core IR へ写すフロントエンド．`emela build` と `run` の既定．
///
/// 名前解決か型検査に誤りが1件でもあれば lowering は呼ばない（lowering は誤りのない
/// プログラムを前提にする）．エントリのモジュールの export する fn からたどれる fn だけを写す．
pub struct Lower<B = NoBuiltins> {
    typecheck: Typecheck<B>,
}

impl Default for Lower<NoBuiltins> {
    fn default() -> Self {
        Lower {
            typecheck: Typecheck::default(),
        }
    }
}

impl<B: BuiltinModules> Lower<B> {
    pub fn new(typecheck: Typecheck<B>) -> Self {
        Lower { typecheck }
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
            lower_typed(
                analysis,
                &typed,
                emela_core::Roots::Exported,
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

/// build，run，test のターゲット．外部関数の `@external` の検査（12.1）に使う．
pub const JS_TARGET: &str = "js";

/// 型検査の結果を Core IR にする．`diagnostics` に誤りが1件でもあるか，エントリがなければ
/// lowering を呼ばずに `None`．lowering の誤りは `diagnostics` に足して `None`．
///
/// 外部関数の検査（12.1）は JS のターゲット（[`JS_TARGET`]）で行う．lowering を呼ぶのは
/// build，run，test だけ（check と LSP は呼ばない）．
pub(crate) fn lower_typed(
    analysis: &Analysis,
    typed: &TypedProgram,
    roots: emela_core::Roots,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<emela_core::Lowered> {
    let entry = analysis.entry?;
    if error_count(diagnostics) > 0 {
        return None;
    }
    let mut sources = ArenaMap::default();
    let mut paths = ArenaMap::default();
    for (module, &file) in analysis.files.iter() {
        sources.insert(module, analysis.sources[file].text());
        paths.insert(module, analysis.sources[file].path());
    }
    let input = emela_core::LowerInput {
        hir: &typed.hir,
        types: &typed.types,
        modules: &analysis.modules,
        sources,
        entry,
        tests: typed.tests.iter().map(|t| t.def).collect(),
        paths,
        core_js_files: emela_core::core_js_files(),
        target: Some(JS_TARGET),
    };
    match emela_core::lower(&input, roots) {
        Ok(lowered) => Some(lowered),
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

/// エントリのモジュールの `main`（トップレベルの fn）．
fn entry_main(hir: &Program, entry: ModuleId) -> Option<DefId> {
    hir.modules.get(entry)?.items.iter().copied().find(|&def| {
        let data = &hir.defs[def];
        data.name == "main" && data.kind == DefKind::Fn
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// import ごとに（モジュール名，範囲の文字列）．
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
        // E0127 は位置の誤りで，import 自体は読める．
        let src = "let x = 1\nimport Json\n";
        assert_eq!(imports_of(src), [("Json".into(), "Json".into())]);
    }
}
