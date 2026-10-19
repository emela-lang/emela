//! Header-only refactoring for missing impl methods (spec 10.1–10.2).

use emela_resolve::ModuleId;
use emela_resolve::hir::{DefId, ImplItem, Item, Program, Res};
use emela_syntax::SyntaxKind;
use emela_syntax::ast::{self, AstNode};
use line_index::TextRange;

use super::quickfix::append;
use super::{CodeAction, CodeActionKind};

pub(super) fn insert_stubs(
    hir: &Program,
    module: ModuleId,
    i: &ast::ImplDecl,
    range: TextRange,
    text: &str,
) -> Option<CodeAction> {
    let node = i.syntax();
    let list = node
        .children()
        .find(|n| n.kind() == SyntaxKind::ITEM_LIST)?;
    let open = list
        .children_with_tokens()
        .find(|e| e.kind() == SyntaxKind::L_BRACE)?;
    let header = TextRange::new(node.text_range().start(), open.text_range().end());
    let in_header = if range.is_empty() {
        header.contains(range.start())
    } else {
        header.intersect(range).is_some_and(|r| !r.is_empty())
    };
    if !in_header {
        return None;
    }
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
    Some(CodeAction {
        title: "Implement missing trait methods".to_owned(),
        kind: CodeActionKind::RefactorRewrite,
        diagnostics: Vec::new(),
        edits: vec![append(&list, SyntaxKind::FN_DECL, &stubs, text)?],
    })
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
