//! 能力の値の制約（8.2）．能力の値は，関数の引数に渡せず，データ構造に入れられず，返せない．
//!
//! 型が決まってから（集合を解いた後に）確かめる．型引数や無名関数の引数を通して能力の値が
//! 渡ることがあるので，式の型を見る．見るのは値の最上位の型だけ．入れ子の中の能力の値
//! （`List[use Logger]` など）は，それを作った式で誤りになっている．
//!
//! with の外への持ち出し（capture set，8.6）は 0.22 の範囲なので，ここでは見ない．

use emela_resolve::hir::{BinaryOp, DefKind, ExprId, ExprKind, Item, Res};

use super::BodyInfer;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::ty::Ty;

impl BodyInfer<'_> {
    pub(super) fn check_capabilities(&mut self) {
        let program = self.program;
        let exprs: Vec<ExprId> = self.exprs.iter().map(|(e, _)| e).collect();
        for expr in exprs {
            match &program.exprs[expr].kind {
                ExprKind::Call { callee, args } => {
                    let kind = if self.is_ctor(*callee) {
                        DiagnosticKind::CapabilityInData
                    } else {
                        DiagnosticKind::CapabilityArgument
                    };
                    for arg in args {
                        self.reject_cap(arg.value, kind.clone());
                    }
                }
                ExprKind::Binary {
                    op: BinaryOp::Pipe,
                    lhs,
                    rhs,
                } => {
                    let callee = match &program.exprs[*rhs].kind {
                        ExprKind::Call { callee, .. } => *callee,
                        _ => *rhs,
                    };
                    let kind = if self.is_ctor(callee) {
                        DiagnosticKind::CapabilityInData
                    } else {
                        DiagnosticKind::CapabilityArgument
                    };
                    self.reject_cap(*lhs, kind);
                }
                ExprKind::Tuple(elems) => {
                    for &e in elems {
                        self.reject_cap(e, DiagnosticKind::CapabilityInData);
                    }
                }
                ExprKind::List { elems, rest } => {
                    for &e in elems.iter().chain(rest) {
                        self.reject_cap(e, DiagnosticKind::CapabilityInData);
                    }
                }
                ExprKind::Update { fields, .. } => {
                    for field in fields {
                        self.reject_cap(field.value, DiagnosticKind::CapabilityInData);
                    }
                }
                // 無名関数の引数をそのまま返すだけなら，能力の値は引数として渡ってきたもので，
                // 渡した側で誤りになっている．
                ExprKind::Lambda { params, body } => {
                    let value = tail(program, *body);
                    let returns_param = matches!(
                        program.exprs[value].kind,
                        ExprKind::Path(Res::Local(local)) if params.iter().any(|p| p.local == local)
                    );
                    if !returns_param {
                        self.reject_cap(value, DiagnosticKind::CapabilityReturned);
                    }
                }
                _ => {}
            }
        }

        // fn の本体と handler の操作の本体の値．
        for def in self.defs.clone() {
            let bodies: Vec<ExprId> = match program.item(def) {
                Some(Item::Fn(item)) => item.body.into_iter().collect(),
                Some(Item::Handler(item)) => item.ops.iter().filter_map(|op| op.body).collect(),
                _ => Vec::new(),
            };
            for body in bodies {
                self.reject_cap(tail(program, body), DiagnosticKind::CapabilityReturned);
            }
        }
    }

    /// 呼ばれる側が構成子か（引数はデータ構造に入る）．
    fn is_ctor(&self, callee: ExprId) -> bool {
        match self.program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => {
                matches!(self.program.defs[def].kind, DefKind::Ctor | DefKind::Error)
            }
            _ => false,
        }
    }

    /// 式の値が能力の値なら，`kind` の診断をその式に積む．
    fn reject_cap(&mut self, expr: ExprId, kind: DiagnosticKind) {
        let Some(ty) = self.exprs.get(expr).cloned() else {
            return;
        };
        if let Ty::Cap(_) = self.ctx.resolve(&ty) {
            self.diags.push(Diagnostic {
                span: Some(self.program.exprs[expr].span),
                kind,
            });
        }
    }
}

/// 本体の値を作る式．ブロックなら最後の式を辿る．
fn tail(program: &emela_resolve::hir::Program, mut expr: ExprId) -> ExprId {
    while let ExprKind::Block(block) = &program.exprs[expr].kind {
        match block.tail {
            Some(t) => expr = t,
            None => break,
        }
    }
    expr
}
