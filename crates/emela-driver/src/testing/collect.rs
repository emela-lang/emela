//! ソースの `@test` の関数を集める（仕様 13.1）．

use emela_resolve::ModuleId;
use emela_syntax::ast::{self, AstNode, HasAttrs};
use emela_syntax::{Parse, SyntaxKind};
use line_index::TextRange;

use crate::code;
use crate::diagnostic::{Diagnostic, Span};
use crate::pipeline::Analysis;
use crate::source::FileId;

/// テスト関数の注釈の名前．
pub const TEST_ANNOTATION: &str = "test";

/// ソースの `@test` の関数．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFn {
    pub module: ModuleId,
    /// 関数の名前．
    pub name: String,
    /// 結果の表示とフィルタに使う名前．エントリのモジュールなら関数の名前だけ，
    /// ほかのモジュールなら `Http.Client.name` のようにモジュール名を前に付ける．
    pub display_name: String,
    pub file: FileId,
    /// 関数の名前のトークンの範囲．名前解決の `DefData::span` と同じ範囲なので，HIR の定義を引ける．
    pub name_range: TextRange,
}

/// 読んだモジュールの構文木から `@test` の関数を，モジュールの順，宣言の順に集める．
///
/// `@test` を付けられるのはトップレベルの引数のない fn で，本体を持つもの（`@external` でない）．
/// それ以外に付いた `@test` と，`@test(...)` のような引数は E0222 にする．誤りのある関数は集めない．
pub fn collect_tests<'p>(
    analysis: &Analysis,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> (Vec<TestFn>, Vec<Diagnostic>) {
    let mut tests = Vec::new();
    let mut diagnostics = Vec::new();
    for (module, data) in analysis.modules.iter() {
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        let error = |message: String, range: TextRange| {
            Diagnostic::error(message)
                .with_code(code::INVALID_TEST)
                .with_span(Span::new(file, range))
        };
        let tree = parse.tree();
        for annotation in tree
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            if annotation
                .name()
                .is_none_or(|name| name.text() != TEST_ANNOTATION)
            {
                continue;
            }
            let range = annotation.syntax().text_range();
            // `@test()` のように空の括弧でも引数の並びがある．
            if annotation
                .syntax()
                .children()
                .any(|n| n.kind() == SyntaxKind::ANNOT_ARG_LIST)
            {
                diagnostics.push(error("`@test` takes no arguments".into(), range));
                continue;
            }
            let top_level_fn = annotation
                .syntax()
                .parent()
                .filter(|p| p.parent().is_some_and(|pp| pp.kind() == SyntaxKind::ROOT))
                .and_then(ast::FnDecl::cast);
            let Some(decl) = top_level_fn else {
                diagnostics.push(
                    error(
                        "`@test` can only be used on top-level functions".into(),
                        range,
                    )
                    .with_note("a test is a function without parameters (13.1)"),
                );
                continue;
            };
            // 同じ関数に `@test` が2つあっても1回だけ集める．
            if decl
                .annotations()
                .find(|a| a.name().is_some_and(|n| n.text() == TEST_ANNOTATION))
                .is_some_and(|first| first.syntax() != annotation.syntax())
            {
                continue;
            }
            let Some(name) = decl.name() else {
                // 名前のない宣言は構文の診断が出ている．
                continue;
            };
            if let Some(params) = decl.param_list()
                && params.params().next().is_some()
            {
                diagnostics.push(error(
                    format!("test function `{}` cannot take parameters", name.text()),
                    params.syntax().text_range(),
                ));
                continue;
            }
            if decl.body().is_none() {
                // 本体がないこと自体は構文の段（E0130）か `@external` の扱いになる．
                diagnostics.push(error(
                    format!("test function `{}` needs a body", name.text()),
                    name.text_range(),
                ));
                continue;
            }
            let display_name = if analysis.entry == Some(module) {
                name.text().to_owned()
            } else {
                format!("{}.{}", data.name, name.text())
            };
            tests.push(TestFn {
                module,
                name: name.text().to_owned(),
                display_name,
                file,
                name_range: name.text_range(),
            });
        }
    }
    (tests, diagnostics)
}

/// fn の宣言に `@test` が付いているか．lowering が通常のビルドでテスト関数を除くのに使う（13.1）．
pub fn is_test_fn(decl: &ast::FnDecl) -> bool {
    decl.has_annotation(TEST_ANNOTATION)
}
