//! suspend と Async と Immediate（8.4，8.7，9.5）．
//!
//! - suspend fn の実装の本体で，`resume` を直接呼ぶ（`resume(v)`）以外の使い方をしていれば，
//!   resume を後から呼びうるとみなす（8.7）．判定は構文だけで行う．無名関数の中の参照は，
//!   直接の呼び出しでもクロージャで捕まえたものなので，直接の呼び出しに数えない．
//!   その操作の本体の R には Async を足す（ハンドラの依存になる）．
//! - `R: Immediate` の制約のある型引数を持つ定義を参照したら，具体化した集合を覚えておき，
//!   集合を解いた後に Immediate か確かめる．要素ごとに，エフェクトは suspend fn を持たず
//!   Async でないこと，集合の型引数は `Immediate` の制約を持つこと．まだ汎化していない集合変数は，
//!   汎化するときに `Immediate` の制約を付ける．
//! - 外部関数の引数の関数型で `use` を省略したときの見えない集合は `Immediate` の制約を持つ
//!   （コールバックは Immediate でなければならない，9.5）ので，同じ仕組みで確かめる．

use emela_resolve::hir::{
    BinaryOp, DefId, ExprId, ExprKind, Item, LocalId, Program, Res, Span, Stmt, StrPart,
};
use smol_str::SmolStr;

use super::{BodyInfer, ImmediateObligation};
use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource};
use crate::lower::{IMMEDIATE, is_immediate_effect};
use crate::row::{Row, RowElem};
use crate::ty::TyParam;

impl BodyInfer<'_> {
    /// 定義 `def` の参照で，型引数 `params` を集合 `rows` に具体化した．`Immediate` の制約のある
    /// 集合の型引数の具体化を，後で確かめるために覚える．
    pub(super) fn require_immediate(
        &mut self,
        def: DefId,
        params: &[TyParam],
        rows: &[Row],
        span: Span,
    ) {
        let program = self.program;
        let external = matches!(program.item(def), Some(Item::Fn(f)) if f.is_external);
        for (p, row) in params.iter().zip(rows) {
            if !p.bounds.iter().any(|b| b == IMMEDIATE) {
                continue;
            }
            let callee = program.defs[def].name.clone();
            // 外部関数の見えない集合（`R(f)`）はコールバックの R．書いた型引数は制約として出す．
            let source = if external && p.name.contains('(') {
                ImmediateSource::ExternalCallback { callee }
            } else {
                ImmediateSource::Bound {
                    param: p.name.clone(),
                    callee,
                }
            };
            self.immediates.push(ImmediateObligation {
                row: row.clone(),
                span,
                owner: self.current,
                source,
            });
        }
    }

    /// 覚えておいた集合が Immediate か確かめる（E0416）．集合を解いた後に呼ぶ．
    pub(super) fn check_immediates(&mut self) {
        let program = self.program;
        for ob in std::mem::take(&mut self.immediates) {
            let row = self.ctx.resolve_row(&ob.row);
            let params = ob.owner.map(|d| self.def_params(d)).unwrap_or_default();
            let mut effects: Vec<SmolStr> = Vec::new();
            let mut sets: Vec<SmolStr> = Vec::new();
            for e in row.elems() {
                match e {
                    RowElem::Tag(effect) if !is_immediate_effect(program, *effect) => {
                        effects.push(program.defs[*effect].name.clone());
                    }
                    RowElem::Bound(i) => match params.get(*i as usize) {
                        Some(p) if p.bounds.iter().any(|b| b == IMMEDIATE) => {}
                        Some(p) => sets.push(p.name.clone()),
                        None => {}
                    },
                    // 汎化する集合変数．汎化するときに制約を付ける．
                    RowElem::Var(v) => {
                        self.immediate_vars.insert(*v);
                    }
                    _ => {}
                }
            }
            if effects.is_empty() && sets.is_empty() {
                continue;
            }
            self.diags.push(Diagnostic {
                span: Some(ob.span),
                kind: DiagnosticKind::NotImmediate {
                    effects,
                    sets,
                    required: ob.source,
                },
            });
        }
    }
}

impl BodyInfer<'_> {
    /// fn の操作の実装の本体が中断しないか確かめる（E0417）．fn の操作は「ハンドラが値を返すと，
    /// 呼び出し元はすぐ続きを実行する」（8.1）ので，呼び出し元は中断しない前提で出力する．
    /// 集合を解いた後に呼ぶ．handler は型引数を持たないので，本体の R の要素はタグだけを見る．
    pub(super) fn check_fn_op_bodies(&mut self) {
        let program = self.program;
        let bodies: Vec<(DefId, usize, ExprId)> = self
            .handler_bodies
            .iter()
            .filter(|b| !b.suspend)
            .map(|b| (b.handler, b.index, b.body))
            .collect();
        for (handler, index, body) in bodies {
            let Some(row) = self.expr_effects.get(body).cloned() else {
                continue;
            };
            let row = self.ctx.resolve_row(&row);
            let effects: Vec<SmolStr> = row
                .elems()
                .iter()
                .filter_map(|e| match e {
                    RowElem::Tag(effect) if !is_immediate_effect(program, *effect) => {
                        Some(program.defs[*effect].name.clone())
                    }
                    _ => None,
                })
                .collect();
            if effects.is_empty() {
                continue;
            }
            let Some(Item::Handler(h)) = program.item(handler) else {
                continue;
            };
            let name = &h.ops[index].name;
            self.diags.push(Diagnostic {
                span: Some(name.span),
                kind: DiagnosticKind::SuspendingFnOp {
                    op: name.name.clone(),
                    effects,
                },
            });
        }
    }
}

/// suspend fn の実装の本体 `body` で，`resume`（局所変数 `resume`）を直接呼ぶ以外の使い方を
/// しているか（8.7）．直接呼ぶとは，無名関数の外で `resume(v)` か `v |> resume` と書くこと．
pub(super) fn resume_escapes(program: &Program, body: ExprId, resume: LocalId) -> bool {
    Walker {
        program,
        resume,
        escapes: false,
    }
    .run(body)
}

struct Walker<'a> {
    program: &'a Program,
    resume: LocalId,
    escapes: bool,
}

impl Walker<'_> {
    fn run(mut self, body: ExprId) -> bool {
        self.expr(body, false);
        self.escapes
    }

    fn is_resume(&self, expr: ExprId) -> bool {
        matches!(self.program.exprs[expr].kind, ExprKind::Path(Res::Local(l)) if l == self.resume)
    }

    /// 呼ばれる側の位置の式．resume なら，無名関数の外での直接の呼び出し．
    fn callee(&mut self, callee: ExprId, in_lambda: bool) {
        if self.is_resume(callee) {
            if in_lambda {
                self.escapes = true;
            }
        } else {
            self.expr(callee, in_lambda);
        }
    }

    fn expr(&mut self, expr: ExprId, in_lambda: bool) {
        let program = self.program;
        match &program.exprs[expr].kind {
            ExprKind::Path(Res::Local(l)) => {
                if *l == self.resume {
                    self.escapes = true;
                }
            }
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. } => {}
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        self.expr(*e, in_lambda);
                    }
                }
            }
            ExprKind::Field { base, .. } => self.expr(*base, in_lambda),
            ExprKind::Call { callee, args } => {
                self.callee(*callee, in_lambda);
                for arg in args {
                    self.expr(arg.value, in_lambda);
                }
            }
            ExprKind::Unary { operand, .. } => self.expr(*operand, in_lambda),
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                self.expr(*lhs, in_lambda);
                match &program.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        self.callee(*callee, in_lambda);
                        for arg in args {
                            self.expr(arg.value, in_lambda);
                        }
                    }
                    _ => self.callee(*rhs, in_lambda),
                }
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                self.expr(*lhs, in_lambda);
                self.expr(*rhs, in_lambda);
            }
            ExprKind::Tuple(elems) => {
                for &e in elems {
                    self.expr(e, in_lambda);
                }
            }
            ExprKind::List { elems, rest } => {
                for &e in elems.iter().chain(rest) {
                    self.expr(e, in_lambda);
                }
            }
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { value, .. } => self.expr(*value, in_lambda),
                        Stmt::Expr(e) => self.expr(*e, in_lambda),
                    }
                }
                if let Some(tail) = block.tail {
                    self.expr(tail, in_lambda);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.expr(*cond, in_lambda);
                self.expr(*then_branch, in_lambda);
                if let Some(e) = else_branch {
                    self.expr(*e, in_lambda);
                }
            }
            ExprKind::Match {
                scrutinee: inner,
                arms,
            }
            | ExprKind::Escape { expr: inner, arms } => {
                self.expr(*inner, in_lambda);
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        self.expr(guard, in_lambda);
                    }
                    self.expr(arm.body, in_lambda);
                }
            }
            ExprKind::Fail(e) | ExprKind::Assert(e) => self.expr(*e, in_lambda),
            ExprKind::Update { ctor, fields, base } => {
                self.expr(*ctor, in_lambda);
                for field in fields {
                    self.expr(field.value, in_lambda);
                }
                self.expr(*base, in_lambda);
            }
            ExprKind::With { body, .. } => self.expr(*body, in_lambda),
            ExprKind::Lambda { body, .. } => self.expr(*body, true),
        }
    }
}
