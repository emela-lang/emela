//! Inlay hints (LSP `textDocument/inlayHint`).
//!
//! Shows types, E (error sets) and R (effect sets) that are not written in source, inline
//! (spec 9.3).

use emela_resolve::ModuleId;
use emela_resolve::hir::{BinaryOp, DefKind, ExprId, ExprKind, Item, LocalId, PatKind, Stmt};
use emela_syntax::SyntaxKind;
use emela_syntax::ast::{self, AstNode};
use emela_types::{Row, Ty, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// Shows `label` after `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: TextSize,
    pub label: String,
    pub kind: HintKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    /// Types of bindings and parameters (`: Int`).
    Type,
    /// E and R of calls (`fails NotFound use Logger`).
    Effect,
    /// Omitted annotations of fns (`-> Int fails NotFound`).
    Signature,
}

/// Hints within `range` of `file`, in position order.
pub(crate) fn inlay_hints(
    program: &TypedProgram,
    analysis: &Analysis,
    file: FileId,
    range: TextRange,
) -> Vec<InlayHint> {
    let Some(module) = analysis
        .files
        .iter()
        .find(|(_, f)| **f == file)
        .map(|(m, _)| m)
    else {
        return Vec::new();
    };
    let mut cx = Hints {
        program,
        module,
        hints: Vec::new(),
    };
    cx.signatures(analysis.sources[file].text());
    cx.bodies();
    let mut hints = cx.hints;
    hints.retain(|h| range.contains_inclusive(h.offset));
    hints.sort_by_key(|h| h.offset);
    hints
}

struct Hints<'a> {
    program: &'a TypedProgram,
    module: ModuleId,
    hints: Vec<InlayHint>,
}

impl Hints<'_> {
    /// Omitted annotations of top-level fns: `-> T fails E use R` if the return type is omitted
    /// (E and R only when not empty), or `use R` if only the use clause is omitted. Hidden sets
    /// are shown as is (as in hover).
    fn signatures(&mut self, text: &str) {
        let hir = &self.program.hir;
        let types = &self.program.types;
        let cons = &types.ty_table.cons;
        for item in emela_syntax::parse(text).tree().items() {
            let ast::Item::Fn(f) = item else { continue };
            if f.body().is_none() {
                continue;
            }
            let Some(name) = f.name() else { continue };
            let Some(def) = hir.defs.iter().find_map(|(def, data)| {
                (data.kind == DefKind::Fn
                    && data
                        .span
                        .is_some_and(|s| s.module == self.module && s.range == name.text_range()))
                .then_some(def)
            }) else {
                continue;
            };
            let (Some(Item::Fn(item)), Some(sig)) = (hir.item(def), types.sigs.get(def)) else {
                continue;
            };
            let Ty::Fn(fn_ty) = &sig.scheme.ty else {
                continue;
            };
            let params = &sig.scheme.params;
            let node = f.syntax();
            let end_of = |kind: SyntaxKind| {
                node.children()
                    .find(|n| n.kind() == kind)
                    .map(|n| n.text_range().end())
            };
            let Some(after_params) = end_of(SyntaxKind::PARAM_LIST) else {
                continue;
            };
            let uses = (item.sig.uses.is_none() && !fn_ty.uses.is_empty())
                .then(|| format!("use {}", fn_ty.uses.display_effects(cons, params)));
            if item.sig.ret.is_none() {
                let mut label = format!("-> {}", fn_ty.ret.display_with(cons, params));
                if !fn_ty.fails.is_empty() {
                    label.push_str(&format!(
                        " fails {}",
                        fn_ty.fails.display_with(cons, params)
                    ));
                }
                if let Some(uses) = uses {
                    label.push_str(&format!(" {uses}"));
                }
                self.push(after_params, label, HintKind::Signature);
            } else if let Some(uses) = uses {
                let at = end_of(SyntaxKind::FAILS_CLAUSE)
                    .or_else(|| end_of(SyntaxKind::RET_TYPE))
                    .unwrap_or(after_params);
                self.push(at, uses, HintKind::Signature);
            }
        }
    }

    /// Inside bodies: types of unannotated bindings, lambda parameters, and parameters of handler
    /// operations and impl functions; and E and R of calls in statement position (statements,
    /// right-hand sides of bindings, block tails).
    fn bodies(&mut self) {
        let hir = &self.program.hir;
        let mut locals: Vec<LocalId> = Vec::new();
        let mut calls: Vec<ExprId> = Vec::new();
        for (_, expr) in hir.exprs.iter() {
            // The fns `@entry` makes have empty spans and no source to show hints in.
            if expr.span.module != self.module || expr.span.range.is_empty() {
                continue;
            }
            match &expr.kind {
                ExprKind::Block(block) => {
                    for stmt in &block.stmts {
                        match stmt {
                            Stmt::Let { pat, ty, value } => {
                                if ty.is_none()
                                    && let PatKind::Bind(local) = hir.pats[*pat].kind
                                {
                                    locals.push(local);
                                }
                                calls.push(*value);
                            }
                            Stmt::Expr(e) => calls.push(*e),
                        }
                    }
                    calls.extend(block.tail);
                }
                ExprKind::Lambda { params, .. } => {
                    locals.extend(params.iter().filter(|p| p.ty.is_none()).map(|p| p.local));
                }
                _ => {}
            }
        }
        // Parameters of handler operations and impl functions may omit types (the declaration
        // determines them).
        for (_, item) in hir.items.iter() {
            let params = match item {
                Item::Handler(h) => h.ops.iter().flat_map(|op| &op.params).collect::<Vec<_>>(),
                Item::Impl(i) => i.fns.iter().flat_map(|f| &f.params).collect(),
                _ => continue,
            };
            locals.extend(
                params
                    .into_iter()
                    .filter(|p| p.ty.is_none() && hir.locals[p.local].span.module == self.module)
                    .map(|p| p.local),
            );
        }
        for local in locals {
            self.local_type(local);
        }
        for expr in calls {
            self.call_effects(expr);
        }
    }

    /// Capability values (values of `use X`) and unknown types are not shown.
    fn local_type(&mut self, local: LocalId) {
        let data = &self.program.hir.locals[local];
        let Some(ty) = self.program.types.locals.get(local) else {
            return;
        };
        if !shows(ty) {
            return;
        }
        let params = self.params(data.span.range.start());
        let label = format!(
            ": {}",
            ty.display_with(&self.program.types.ty_table.cons, &params)
        );
        self.push(data.span.range.end(), label, HintKind::Type);
    }

    /// Calls including pipes. Nothing is shown if both are empty.
    fn call_effects(&mut self, expr: ExprId) {
        let hir = &self.program.hir;
        let e = &hir.exprs[expr];
        let is_call = matches!(
            e.kind,
            ExprKind::Call { .. }
                | ExprKind::Binary {
                    op: BinaryOp::Pipe,
                    ..
                }
        );
        if !is_call {
            return;
        }
        let types = &self.program.types;
        let empty = Row::empty();
        let errors = types.errors.get(expr).unwrap_or(&empty);
        let effects = types.effects.get(expr).unwrap_or(&empty);
        if errors.is_empty() && effects.is_empty() {
            return;
        }
        let params = self.params(e.span.range.start());
        let cons = &types.ty_table.cons;
        let mut parts = Vec::new();
        if !errors.is_empty() {
            parts.push(format!("fails {}", errors.display_with(cons, &params)));
        }
        if !effects.is_empty() {
            parts.push(format!("use {}", effects.display_effects(cons, &params)));
        }
        self.push(e.span.range.end(), parts.join(" "), HintKind::Effect);
    }

    /// Type parameters of the definition whose body contains the position (names of
    /// `Bound(i)`).
    fn params(&self, offset: TextSize) -> Vec<TyParam> {
        crate::hover::enclosing_def(self.program, self.module, offset)
            .and_then(|def| self.program.types.sigs.get(def))
            .map(|sig| sig.scheme.params.clone())
            .unwrap_or_default()
    }

    fn push(&mut self, offset: TextSize, label: String, kind: HintKind) {
        self.hints.push(InlayHint {
            offset,
            label,
            kind,
        });
    }
}

/// Capability values, inference variables and broken types are not shown.
fn shows(ty: &Ty) -> bool {
    match ty {
        Ty::Cap(_) | Ty::Var(_) | Ty::Error => false,
        Ty::Fn(f) => f.params.iter().all(shows) && shows(&f.ret),
        Ty::Tuple(tys) | Ty::Named(_, tys) => tys.iter().all(shows),
        Ty::List(t) => shows(t),
        // Sets passed as type arguments to named types (`Handler[E, R]`).
        Ty::Set(..) => true,
        Ty::Prim(_) | Ty::Never | Ty::Bound(_) => true,
    }
}
