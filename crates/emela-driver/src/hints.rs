//! インレイヒント（LSP の `textDocument/inlayHint`）．
//!
//! ソースに書かない型と E（エラーの集合），R（効果の集合）を行の中に見せる（9.3）．

use emela_resolve::ModuleId;
use emela_resolve::hir::{BinaryOp, DefKind, ExprId, ExprKind, Item, LocalId, PatKind, Stmt};
use emela_syntax::SyntaxKind;
use emela_syntax::ast::{self, AstNode};
use emela_types::{Row, Ty, TyParam};
use line_index::{TextRange, TextSize};

use crate::frontend::TypedProgram;
use crate::pipeline::Analysis;
use crate::source::FileId;

/// インレイヒント．`offset` の位置の後ろに `label` を出す．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHint {
    pub offset: TextSize,
    pub label: String,
    pub kind: HintKind,
}

/// ヒントの種類．
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HintKind {
    /// 束縛と引数の型（`: Int`）．
    Type,
    /// 呼び出しの E と R（`fails NotFound use Logger`）．
    Effect,
    /// fn の省いた注釈（`-> Int fails NotFound`）．
    Signature,
}

/// `file` の `range` の中のヒント．位置の順．
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
    /// トップレベルの fn の省いた注釈．戻り値の型を省いていれば `-> T fails E use R`（E と R は
    /// 空でなければ），use 節だけを省いていれば `use R`．見えない集合もそのまま出す（ホバーと同じ）．
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

    /// 本体の中．型注釈のない束縛，ラムダの引数，handler の操作と impl の関数の引数の型と，
    /// 文の位置（文，束縛の右辺，ブロックの末尾）の呼び出しの E と R．
    fn bodies(&mut self) {
        let hir = &self.program.hir;
        let mut locals: Vec<LocalId> = Vec::new();
        let mut calls: Vec<ExprId> = Vec::new();
        for (_, expr) in hir.exprs.iter() {
            if expr.span.module != self.module {
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
        // handler の操作と impl の関数の引数は型を省ける（宣言から決まる）．
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

    /// 局所変数の型．能力の値（`use X` の値）と，型の分からないものは出さない．
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

    /// 呼び出し（パイプを含む）の E と R．どちらも空なら出さない．
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

    /// 位置を本体に含む定義の型引数（`Bound(i)` の名前）．
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

/// 型のヒントに出す型か．能力の値，推論の変数，壊れた型を含むものは出さない．
fn shows(ty: &Ty) -> bool {
    match ty {
        Ty::Cap(_) | Ty::Var(_) | Ty::Error => false,
        Ty::Fn(f) => f.params.iter().all(shows) && shows(&f.ret),
        Ty::Tuple(tys) | Ty::Named(_, tys) => tys.iter().all(shows),
        Ty::List(t) => shows(t),
        // 名前付き型の型引数に渡した集合（`Handler[E, R]`）．
        Ty::Set(..) => true,
        Ty::Prim(_) | Ty::Never | Ty::Bound(_) => true,
    }
}
