//! Expressions: the traversal, and the statements for each kind of expression.
//!
//! `expr` walks `let` chains without recursion and leaves the rest to `node`, which converts one
//! expression in the current sequence. A compound expression is converted in a new sequence
//! (`branch`) and its value read from the destination of the statement just pushed (`joined`).

use emela_core::{Expr, StrPart, Type, TypeId, UnOp};

use super::concat::Piece;
use super::value::{lit_val, prim_ty};
use super::{Div, Lower, R, TMP, Val, ix};
use crate::ir::{Const, Operand, PrimOp, PrimTy, Stmt, Term, Value};
use crate::repr::Repr;

impl<'s> Lower<'s> {
    pub(super) fn node(&mut self, e: &Expr) -> R {
        match e {
            Expr::Lit(lit) => Ok(lit_val(lit)),
            Expr::Var(l) => {
                let ty = self.sh.core.locals[*l].ty;
                let op = match self.locals.get(l) {
                    Some(op) => op.clone(),
                    None => {
                        let hint = self.sh.core.locals[*l].hint.clone();
                        self.error(format!("local {hint}%{} is used unbound", ix(*l)));
                        None
                    }
                };
                self.val(op, ty)
            }
            Expr::Fn { f, .. } => {
                let ty = self.fn_value_type(*f);
                let target = self.sh.fn_map[ix(*f)];
                self.emit(ty, Value::FnRef(target))
            }
            Expr::Let { .. } => self.expr(e),
            Expr::With { binds, body } => self.with_(binds, body),
            Expr::Call {
                callee,
                args,
                suspends,
                ..
            } => self.call(callee, args, *suspends),
            Expr::Builtin { op, ty_args, args } => self.builtin(*op, ty_args, args),
            Expr::Ctor {
                ctor,
                ty_args,
                args,
            } => {
                let args = self.slots(args)?;
                let ty = self.intern(Type::Enum(ctor.enum_id, ty_args.clone()));
                self.emit(ty, Value::Make { ctor: *ctor, args })
            }
            Expr::Field { base, ctor, index } => {
                let b = self.expr(base)?;
                let Type::Enum(_, args) = self.ty(b.ty).clone() else {
                    self.error("a field of a value that is not an enum".into());
                    return Ok(Val {
                        op: None,
                        ty: TypeId::UNIT,
                    });
                };
                let field = self.sh.core.variant(*ctor).tys[*index];
                let ty = self.subst(field, &args);
                if self.repr(ty) == Repr::Void {
                    return self.val(None, ty);
                }
                let base = self.must_var(&b, "a field's base");
                self.emit(
                    ty,
                    Value::Field {
                        base,
                        ctor: *ctor,
                        index: *index,
                    },
                )
            }
            Expr::Tuple(es) => {
                let mut ops = Vec::with_capacity(es.len());
                let mut tys = Vec::with_capacity(es.len());
                for e in es {
                    let v = self.expr(e)?;
                    ops.push(v.op.unwrap_or(Operand::Const(Const::Unit)));
                    tys.push(v.ty);
                }
                let ty = self.intern(Type::Tuple(tys));
                self.emit(ty, Value::Tuple(ops))
            }
            Expr::List { elem, elems, tail } => {
                let elems = self.slots(elems)?;
                let tail = match tail {
                    Some(t) => self.expr(t)?.op,
                    None => None,
                };
                let ty = self.intern(Type::List(*elem));
                if elems.is_empty()
                    && let Some(t) = tail
                {
                    return self.val(Some(t), ty);
                }
                self.emit(ty, Value::List { elems, tail })
            }
            Expr::If { cond, then, else_ } => {
                let c = self.expr(cond)?;
                let cond = c.op.unwrap_or(Operand::Const(Const::Bool(false)));
                let (then, t) = self.branch(then);
                let (else_, f) = self.branch(else_);
                let ty = t.or(f);
                let dst = ty.and_then(|t| self.new_var(TMP, t));
                self.push(Stmt::If {
                    dst,
                    cond,
                    then,
                    else_,
                });
                self.joined(dst, ty)
            }
            Expr::Match { scrutinee, arms } => self.match_(scrutinee, arms),
            Expr::Binary { op, ty, lhs, rhs } => self.binary(*op, *ty, lhs, rhs),
            Expr::Unary { op, ty, operand } => {
                let v = self.expr(operand)?;
                let a = v.op.unwrap_or(Operand::Const(Const::Int(0)));
                let (op, pty, rty) = match op {
                    UnOp::Neg => (PrimOp::Neg, prim_ty(*ty), v.ty),
                    UnOp::Not => (PrimOp::Not, PrimTy::I32, TypeId::BOOL),
                };
                self.emit(
                    rty,
                    Value::Prim {
                        op,
                        ty: pty,
                        args: vec![a],
                    },
                )
            }
            Expr::Lambda {
                params, ty, body, ..
            } => self.lambda(params, *ty, body),
            Expr::Concat(parts) => self.interpolate(parts),
            Expr::Loop { vars, inits, body } => {
                let mut init_ops = Vec::new();
                for i in inits {
                    init_ops.extend(self.expr(i)?.op);
                }
                let mut loop_vars = Vec::new();
                for &l in vars {
                    loop_vars.extend(self.local_var(l));
                }
                let (body, ty) = self.branch(body);
                let dst = ty.and_then(|t| self.new_var(TMP, t));
                self.push(Stmt::Loop {
                    dst,
                    vars: loop_vars,
                    inits: init_ops,
                    body,
                });
                self.joined(dst, ty)
            }
            Expr::Recur(args) => {
                let mut ops = Vec::new();
                for a in args {
                    ops.extend(self.expr(a)?.op);
                }
                self.term(Term::Recur(ops));
                Err(Div)
            }
            Expr::Assert(a) => self.assert(a),
            Expr::Fail(e) => {
                let v = self.expr(e)?;
                let error = v.op.unwrap_or(Operand::Const(Const::Unit));
                let t = self.fail_term(error);
                self.term(t);
                Err(Div)
            }
            Expr::Escape { body, arms } => self.escape(body, arms),
            Expr::Use { ctx, effect } => {
                let c = self.expr(ctx)?;
                let ev = self.must_var(&c, "an evidence table");
                let ty = self.intern(Type::Handler(self.sh.core.handler_effect(*effect)));
                self.emit(
                    ty,
                    Value::EvLookup {
                        ev,
                        effect: *effect,
                    },
                )
            }
            Expr::Provide { base, bindings } => {
                let base = match base {
                    Some(b) => {
                        let v = self.expr(b)?;
                        Some(self.must_var(&v, "an evidence table"))
                    }
                    None => None,
                };
                let mut bs = Vec::with_capacity(bindings.len());
                for (effect, h) in bindings {
                    let v = self.expr(h)?;
                    bs.push((*effect, self.must_var(&v, "a handler")));
                }
                self.emit(TypeId::EVIDENCE, Value::EvExtend { base, bindings: bs })
            }
            Expr::Handler { name, effect, ops } => self.handler(name, *effect, ops),
            Expr::Fiber { op, args } => self.fiber(*op, args),
        }
    }

    /// String interpolation: the values left to right, then the string. A String is embedded as
    /// is; other values are shown.
    fn interpolate(&mut self, parts: &[StrPart]) -> R {
        let mut values = Vec::new();
        for p in parts {
            if let StrPart::Value(e, ty) = p {
                values.push((self.expr(e)?.op, *ty));
            }
        }
        let mut values = values.into_iter();
        let mut pieces = Vec::with_capacity(parts.len());
        for p in parts {
            match p {
                StrPart::Lit(text) => pieces.push(Piece::Text(text.clone())),
                StrPart::Value(..) => {
                    let (op, ty) = values.next().expect("one value per part");
                    let shown = match (ty, op) {
                        (TypeId::STRING, Some(op)) => op,
                        (ty, op) => self.show(ty, op),
                    };
                    pieces.push(Piece::Op(shown));
                }
            }
        }
        let s = self.concat(pieces);
        Ok(Val {
            op: Some(s),
            ty: TypeId::STRING,
        })
    }
}
