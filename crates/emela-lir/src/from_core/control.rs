//! Control: the sequences being built, `if` / loops / `protect`, failures and releases.
//!
//! `open` and `close` make a sequence; statements go into the innermost one (`push`) and it ends
//! with a terminator (`term`). A failure edge goes to the innermost enclosing `escape` or out of
//! the function (`on_fail`, `fail_term`). `with_` turns a release into `protect` / `cleanup`;
//! `assert` becomes an `if` whose failing branch ends in `defect`.

use emela_core::{Assert, AssertCond, BinOp, Expr, TypeId, WithBind};

use super::concat::{self, Piece};
use super::{Div, Lower, Open, R, TMP, Val};
use crate::ir::{Const, OnFail, Operand, RuntimeOp, Seq, SeqId, Stmt, Term, Value};

impl<'s> Lower<'s> {
    pub(super) fn open(&mut self) {
        self.open.push(Open {
            stmts: Vec::new(),
            term: None,
            cache_mark: self.cache.len(),
        });
    }

    /// Ends the innermost open sequence (with `yield` without value unless it has a terminator).
    pub(super) fn close(&mut self) -> SeqId {
        let open = self.open.pop().expect("no open sequence");
        self.cache.truncate(open.cache_mark);
        self.f.seqs.alloc(Seq {
            stmts: open.stmts,
            term: open.term.unwrap_or(Term::Yield(None)),
        })
    }

    /// An empty sequence (`{}`).
    pub(super) fn empty(&mut self) -> SeqId {
        self.open();
        self.close()
    }

    pub(super) fn push(&mut self, stmt: Stmt) {
        let id = self.f.stmts.alloc(stmt);
        let open = self.open.last_mut().expect("no open sequence");
        debug_assert!(open.term.is_none(), "a statement after a terminator");
        open.stmts.push(id);
    }

    pub(super) fn term(&mut self, term: Term) {
        let open = self.open.last_mut().expect("no open sequence");
        debug_assert!(open.term.is_none(), "a second terminator");
        open.term = Some(term);
    }

    /// Where a failure goes from here.
    pub(super) fn on_fail(&self) -> OnFail {
        match self.fail.last() {
            Some(&l) => OnFail::Break(l),
            None => OnFail::Propagate,
        }
    }

    /// The terminator passing the error value to where failures go from here.
    pub(super) fn fail_term(&self, error: Operand) -> Term {
        match self.fail.last() {
            Some(&label) => Term::Break {
                label,
                value: Some(error),
            },
            None => Term::Fail(error),
        }
    }

    /// `with`: the bindings in order; each binding with release puts the rest inside `protect`,
    /// whose cleanup evaluates the release and then marks the handler value released (also when
    /// the release ends in a defect).
    pub(super) fn with_(&mut self, binds: &[WithBind], body: &Expr) -> R {
        for (i, b) in binds.iter().enumerate() {
            self.bind(b.var, &b.value)?;
            let Some(release) = &b.release else {
                continue;
            };
            let handler = self.locals[&b.var].clone();
            self.open();
            let ty = match self.with_(&binds[i + 1..], body) {
                Ok(v) => {
                    self.term(Term::Yield(v.op));
                    Some(v.ty)
                }
                Err(Div) => None,
            };
            let protected = self.close();
            // The release cannot fail (spec 8.4), so it has no failure target.
            let fail = std::mem::take(&mut self.fail);
            // A release that may suspend is not interrupted until it ends (spec 11.4).
            let hold = may_suspend(&release.body);
            self.open();
            if hold {
                self.runtime(RuntimeOp::Hold);
            }
            self.open();
            let _ = self.expr(&release.body);
            let released = self.close();
            self.open();
            if release.mark
                && let Some(h) = handler
            {
                self.let_(
                    TMP,
                    TypeId::UNIT,
                    Value::Runtime {
                        op: RuntimeOp::Retire,
                        args: vec![h, Operand::Const(Const::String(release.handler.clone()))],
                    },
                );
            }
            if hold {
                self.runtime(RuntimeOp::Unhold);
            }
            let retire = self.close();
            self.push(Stmt::Protect {
                dst: None,
                body: released,
                cleanup: retire,
            });
            let cleanup = self.close();
            self.fail = fail;
            let dst = ty.and_then(|t| self.new_var(TMP, t));
            self.push(Stmt::Protect {
                dst,
                body: protected,
                cleanup,
            });
            return self.joined(dst, ty);
        }
        self.expr(body)
    }

    pub(super) fn assert(&mut self, a: &Assert) -> R {
        match &a.cond {
            AssertCond::Compare {
                op,
                op_ty,
                ty,
                lhs,
                rhs,
            } => {
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                let (test, _) = self.binop(*op, *op_ty, &l, &r);
                self.assert_compare(a, *op, *ty, test, l, r);
            }
            // The sides are variables bound before, so reading them after `cond` changes nothing.
            AssertCond::Shown {
                cond,
                op,
                ty,
                lhs,
                rhs,
            } => {
                let v = self.expr(cond)?;
                let test = v.op.unwrap_or(Operand::Const(Const::Bool(false)));
                let l = self.expr(lhs)?;
                let r = self.expr(rhs)?;
                self.assert_compare(a, *op, *ty, test, l, r);
            }
            AssertCond::Bool(c) => {
                let v = self.expr(c)?;
                let cond = v.op.unwrap_or(Operand::Const(Const::Bool(false)));
                let then = self.empty();
                self.open();
                let msg = if a.text.is_empty() {
                    "assertion failed".to_owned()
                } else {
                    format!("assertion failed: {}", a.text)
                };
                self.term(Term::Defect(
                    Operand::Const(Const::String(msg)),
                    a.location.clone(),
                ));
                let else_ = self.close();
                self.push(Stmt::If {
                    dst: None,
                    cond,
                    then,
                    else_,
                });
            }
        }
        Ok(Val {
            op: None,
            ty: TypeId::UNIT,
        })
    }

    /// Unless `test`, a defect showing both sides `l` and `r` (spec 13.2).
    fn assert_compare(&mut self, a: &Assert, op: BinOp, ty: TypeId, test: Operand, l: Val, r: Val) {
        let then = self.empty();
        self.open();
        let left = self.show(ty, l.op);
        let right = self.show(ty, r.op);
        let head = if a.text.is_empty() {
            format!("left {} right", op_text(op))
        } else {
            a.text.clone()
        };
        let msg = concat::build(
            self,
            vec![
                Piece::Text(format!("assertion failed: {head}\n  left: ")),
                Piece::Op(left),
                Piece::Text("\n right: ".into()),
                Piece::Op(right),
            ],
        );
        self.term(Term::Defect(msg, a.location.clone()));
        let else_ = self.close();
        self.push(Stmt::If {
            dst: None,
            cond: test,
            then,
            else_,
        });
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        _ => "?",
    }
}

/// Whether evaluating `e` may suspend: a suspending call or concurrency operation outside
/// lambdas (the same test as the JS backend's for a release, spec 11.4).
fn may_suspend(e: &Expr) -> bool {
    match e {
        Expr::Lambda { .. } => false,
        Expr::Call { suspends: true, .. } => true,
        Expr::Fiber { op, .. } if op.suspends() => true,
        _ => {
            let mut found = false;
            e.for_each_child(&mut |c| found = found || may_suspend(c));
            found
        }
    }
}
