//! Semantic tokens (LSP `textDocument/semanticTokens`).
//!
//! Only capability values (locals bound to `use X`, spec 8.2) are marked, where they are bound
//! and where they are used. Other names are left to the editor's syntax highlighting.

use std::collections::HashSet;

use emela_resolve::hir::{ExprKind, LocalId, PatKind, Res, Stmt};
use emela_types::Ty;
use line_index::TextRange;

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub range: TextRange,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// A local holding a capability value (`log = use Logger`).
    Capability,
}

/// Tokens of `file` that overlap `range`, in position order.
pub(crate) fn semantic_tokens(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    range: TextRange,
) -> Vec<SemanticToken> {
    let Some(module) = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
    else {
        return Vec::new();
    };
    let hir = &program.hir;
    // A local is a capability value if its type is one. If type checking gave up on it, a
    // binding whose right-hand side is `use X` still counts.
    let mut capabilities: HashSet<LocalId> = program
        .types
        .locals
        .iter()
        .filter(|(_, ty)| matches!(ty, Ty::Cap(_)))
        .map(|(local, _)| local)
        .collect();
    for (_, expr) in hir.exprs.iter() {
        let ExprKind::Block(block) = &expr.kind else {
            continue;
        };
        for stmt in &block.stmts {
            if let Stmt::Let { pat, value, .. } = stmt
                && let PatKind::Bind(local) = hir.pats[*pat].kind
                && matches!(hir.exprs[*value].kind, ExprKind::Use { .. })
            {
                capabilities.insert(local);
            }
        }
    }
    let bindings = capabilities.iter().filter_map(|&local| {
        let span = hir.locals[local].span;
        (span.module == module).then_some(span.range)
    });
    let uses = hir.exprs.iter().filter_map(|(_, expr)| match expr.kind {
        ExprKind::Path(Res::Local(local))
            if expr.span.module == module && capabilities.contains(&local) =>
        {
            Some(expr.span.range)
        }
        _ => None,
    });
    let mut ranges: Vec<TextRange> = bindings
        .chain(uses)
        .filter(|r| !r.is_empty() && r.intersect(range).is_some())
        .collect();
    ranges.sort_by_key(|r| (r.start(), r.end()));
    ranges.dedup();
    ranges
        .into_iter()
        .map(|range| SemanticToken {
            range,
            kind: TokenKind::Capability,
        })
        .collect()
}
