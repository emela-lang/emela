//! Incomplete impl diagnostics and stub generation (spec 10.1–10.2).

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, ImplItem, Item, Program, Res};
use emela_syntax::ast::{self, AstNode};
use emela_syntax::{Parse, SyntaxKind};
use line_index::TextRange;

use super::quickfix::{Fix, append};
use crate::code;
use crate::diagnostic::{Diagnostic, Span};
use crate::pipeline::Analysis;

pub(super) fn insert_stubs(
    hir: &Program,
    module: ModuleId,
    range: TextRange,
    tree: &ast::SourceFile,
    text: &str,
) -> Option<Fix> {
    let i = tree.items().find_map(|i| match i {
        ast::Item::Impl(i) if header_range(&i) == Some(range) => Some(i),
        _ => None,
    })?;
    let node = i.syntax();
    let list = node
        .children()
        .find(|n| n.kind() == SyntaxKind::ITEM_LIST)?;
    let item = hir.items.iter().find_map(|(def, item)| {
        let span = hir.defs[def].span?;
        match item {
            Item::Impl(item) if span.module == module && span.range == node.text_range() => {
                Some(item)
            }
            _ => None,
        }
    })?;
    let stubs = missing_methods(hir, item)
        .into_iter()
        .filter_map(|def| {
            let Item::TraitFn(f) = hir.item(def)? else {
                return None;
            };
            let params: Vec<_> = f
                .sig
                .params
                .iter()
                .map(|p| hir.locals[p.local].name.as_str())
                .collect();
            Some(format!(
                "fn {}({}) {{ todo() }}",
                hir.defs[def].name,
                params.join(", ")
            ))
        })
        .collect::<Vec<_>>();
    if stubs.is_empty() {
        return None;
    }
    Some((
        "Implement missing trait methods".to_owned(),
        vec![append(&list, SyntaxKind::FN_DECL, &stubs, text)?],
    ))
}

fn header_range(i: &ast::ImplDecl) -> Option<TextRange> {
    let keyword = i
        .syntax()
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::IMPL_KW)?;
    // `impl User: Show`, up to the trait.
    let end = match i.trait_ref() {
        Some(t) => t.syntax().text_range().end(),
        None => i.self_ty()?.text_range().end(),
    };
    Some(TextRange::new(keyword.text_range().start(), end))
}

pub(crate) fn incomplete_impl_diagnostics<'p>(
    hir: &Program,
    analysis: &Analysis,
    parse_of: impl Fn(ModuleId) -> Option<&'p Parse>,
) -> Vec<Diagnostic> {
    hir.items
        .iter()
        .filter_map(|(def, item)| {
            let Item::Impl(item) = item else { return None };
            let missing = missing_methods(hir, item);
            if missing.is_empty() {
                return None;
            }
            let span = hir.defs[def].span?;
            let file = *analysis.files.get(span.module)?;
            let range = parse_of(span.module)?
                .tree()
                .items()
                .find_map(|i| match i {
                    ast::Item::Impl(i) if i.syntax().text_range() == span.range => header_range(&i),
                    _ => None,
                })?;
            let names: Vec<_> = missing
                .iter()
                .map(|&d| format!("`{}`", hir.defs[d].name))
                .collect();
            Some(
                Diagnostic::error(format!(
                    "incomplete impl: missing methods {}",
                    names.join(", ")
                ))
                .with_code(code::INCOMPLETE_IMPL)
                .with_span(Span::new(file, range)),
            )
        })
        .collect()
}

fn missing_methods(hir: &Program, item: &ImplItem) -> Vec<DefId> {
    let Res::Def(trait_def) = item.trait_ref else {
        return Vec::new();
    };
    let Some(Item::Trait(t)) = hir.item(trait_def) else {
        return Vec::new();
    };
    t.fns
        .iter()
        .copied()
        .filter(|&def| {
            matches!(hir.item(def), Some(Item::TraitFn(f)) if f.body.is_none())
                && !item.fns.iter().any(|f| f.trait_fn == Res::Def(def))
        })
        .collect()
}
