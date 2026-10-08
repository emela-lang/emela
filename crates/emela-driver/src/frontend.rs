//! emela-syntax のパーサを差し込むフロントエンド．型検査はまだしない．

use emela_resolve::{Import, ModuleId, ModuleName};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{Parse, SyntaxKind};
use la_arena::ArenaMap;

use crate::diagnostic::Diagnostic;
use crate::pipeline::{Analysis, Checked, Frontend, Parsed};
use crate::source::{FileId, SourceFile};

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
