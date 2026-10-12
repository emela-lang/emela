//! 同梱の core の中からだけ参照できる fn（`@internal`）の検査．
//!
//! `@internal` は同梱の core のソースのトップレベルの fn にだけ付けられ（E0229），付いた fn は
//! core のモジュールの外から参照できない（E0228）．opaque な型の中身を別の core のモジュールへ
//! 渡すときのように，core の中でだけ使わせたい fn に付ける．今の同梱の core に使う fn はない．

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, DefKind, DefModule, Program};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{Parse, SyntaxKind};
use line_index::TextRange;

use crate::code;
use crate::diagnostic::{Diagnostic, Span};
use crate::pipeline::Analysis;

/// core の中からだけ参照できる fn の注釈の名前．
pub const INTERNAL_ANNOTATION: &str = "internal";

/// `@internal` の置き場所を確かめ（E0229），core の外から `@internal` の fn を参照している箇所を
/// E0228 にする．参照には呼び出しだけでなく値としての参照も含む．
pub(crate) fn check_internal<'p>(
    analysis: &Analysis,
    hir: &Program,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    // `@internal` の付いた fn の，モジュールと名前の範囲（名前解決の `DefData::span` と同じ）．
    let mut names: Vec<(ModuleId, TextRange)> = Vec::new();
    for (module, _) in analysis.modules.iter() {
        let (Some(&file), Some(parse)) = (analysis.files.get(module), parse_of(module)) else {
            continue;
        };
        let tree = parse.tree();
        for annotation in tree
            .syntax()
            .descendants()
            .filter_map(ast::Annotation::cast)
        {
            if annotation
                .name()
                .is_none_or(|name| name.text() != INTERNAL_ANNOTATION)
            {
                continue;
            }
            let range = annotation.syntax().text_range();
            let error = |message: &str| {
                Diagnostic::error(message)
                    .with_code(code::MISPLACED_INTERNAL)
                    .with_span(Span::new(file, range))
            };
            if !analysis.modules.is_core(module) {
                diagnostics.push(error(
                    "`@internal` can only be used in the bundled core library",
                ));
                continue;
            }
            let top_level_fn = annotation
                .syntax()
                .parent()
                .filter(|p| p.parent().is_some_and(|pp| pp.kind() == SyntaxKind::ROOT))
                .and_then(ast::FnDecl::cast);
            let Some(name) = top_level_fn.and_then(|decl| decl.name()) else {
                diagnostics.push(error("`@internal` can only be used on top-level functions"));
                continue;
            };
            names.push((module, name.text_range()));
        }
    }

    let internal: Vec<DefId> = hir
        .defs
        .iter()
        .filter(|(_, data)| {
            data.kind == DefKind::Fn
                && data
                    .span
                    .is_some_and(|span| names.contains(&(span.module, span.range)))
        })
        .map(|(def, _)| def)
        .collect();
    if internal.is_empty() {
        return diagnostics;
    }
    for (def, data) in hir.defs.iter() {
        let DefModule::Source(module) = data.module else {
            continue;
        };
        if analysis.modules.is_core(module) {
            continue;
        }
        for (target, span) in emela_types::body_refs(hir, def) {
            if !internal.contains(&target) {
                continue;
            }
            let Some(&file) = analysis.files.get(span.module) else {
                continue;
            };
            let target = &hir.defs[target];
            let name = match target.module {
                DefModule::Source(m) => format!("{}.{}", analysis.modules[m].name, target.name),
                _ => target.name.to_string(),
            };
            diagnostics.push(
                Diagnostic::error(format!(
                    "function `{name}` can only be used from the bundled core library"
                ))
                .with_code(code::INTERNAL_FN_OUTSIDE_CORE)
                .with_span(Span::new(file, span.range))
                .with_note("it is marked `@internal`"),
            );
        }
    }
    diagnostics
}
