//! Expressions, blocks and patterns, other than calls ([`super::call`]).

use emela_resolve::ModuleId;
use emela_resolve::hir::{
    Arg, Arm as HirArm, BinaryOp, Block, DefId, DefKind, ExprId, ExprKind, Item, Literal,
    LocalKind, PatId, PatKind, Res, Span, Stmt, StrPart as HirStrPart, UnaryOp, int_literal_value,
};
use emela_types::{Row, Tag, Ty};
use la_arena::ArenaMap;

use crate::ir::{
    Arm, Assert, BinOp, Callee, CtorRef, Expr, Lit, Local, OpTy, Pat, StrPart, TypeId, UnOp,
};

use super::{Bind, Lowerer, comparison, is_atomic, prim_op_ty};

impl<'a> Lowerer<'a> {
    /// `use X` (spec 8.2, 8.3). The handler of the innermost `with` providing X in the same
    /// function, or else from the evidence table passed by the caller. For effects with type
    /// arguments, the type arguments must match too (spec 8.1).
    pub(super) fn use_(&mut self, id: ExprId, span: Span) -> Expr {
        let Some(Ty::Cap(tags, _)) = self.types.exprs.get(id) else {
            return self.unsupported("unresolved effect", span);
        };
        let [tag] = tags.as_slice() else {
            return self.unsupported("`use` of several effects", span);
        };
        let key = self.key_here(tag);
        match self.lookup_effect(&key) {
            Some(e) => e,
            // Unreachable once type checking has checked R.
            None => self.unsupported("`use` without a provided effect", span),
        }
    }

    /// A lambda. If its type's R is not empty, it takes the evidence table as its last argument.
    ///
    /// The handlers that the calls in the body need are passed by the side that calls the
    /// lambda; the enclosing `with`s do not provide them (`[effect.with]`, `[type.rules]`). The
    /// type checker keeps the R of a lambda's body in the lambda's type, so the table its caller
    /// passes holds them. A capability value the lambda captures is a local, and keeps referring
    /// to its handler.
    pub(super) fn lambda(
        &mut self,
        id: ExprId,
        params: &[emela_resolve::hir::Param],
        body: ExprId,
    ) -> Expr {
        let mut ps: Vec<Local> = params.iter().map(|p| self.local(p.local)).collect();
        let suspends = self.ty_suspends(self.expr_ty(id));
        let ty = self.ty(self.expr_ty(id));
        let bound = std::mem::take(&mut self.bound);
        if !self.ty_takes_ev(self.expr_ty(id)) {
            let body = self.expr(body);
            self.bound = bound;
            return Expr::Lambda {
                params: ps,
                ty,
                body: Box::new(body),
                suspends,
            };
        }
        let p = self.ev_local();
        ps.push(p);
        let body = self.body_with_ev(p, body);
        self.bound = bound;
        Expr::Lambda {
            params: ps,
            ty,
            body: Box::new(body),
            suspends,
        }
    }

    /// An operation on a capability value used as a value without calling it
    /// (`List.each(xs, log.info)`). Becomes a lambda that just calls it. The capability value is
    /// evaluated at the reference.
    ///
    /// Through an effect alias or `(use E)` (`x.op`, `(use E).op`), the handler is looked up when
    /// the function value is called, in the table its caller passes (`[effect.use]`): the lambda
    /// is `|a| x.op(a)`.
    pub(super) fn op_value(&mut self, id: ExprId, base: ExprId, op: DefId, span: Span) -> Expr {
        let Some(Ty::Cap(tags, _)) = self.types.exprs.get(base).cloned() else {
            return self.unsupported("effect operation", span);
        };
        let Some(tag) = self.op_tag(&tags, op) else {
            return self.unsupported("effect operation", span);
        };
        let Some(index) = self.op_index(tag.def, op) else {
            return self.unsupported("effect operation", span);
        };
        let effect_id = self.effect_id(&self.key_here(&tag));
        let effect_id = self.module.handler_effect(effect_id);
        let fails = self.callee_fails(id);
        let suspends = self.module.effects[effect_id].ops[index].suspend;
        let (param_tys, ev) = match self.expr_ty(id) {
            Ty::Fn(f) => (f.params.clone(), self.row_needs_ev(&f.uses)),
            _ => unreachable!("an operation reference has a function type"),
        };
        let param_tys: Vec<TypeId> = param_tys.iter().map(|t| self.ty(t)).collect();
        let ty = self.ty(self.expr_ty(id));
        let ev = ev.then(|| self.ev_local());
        let at_call = ev.is_some() && self.hir.alias_uses.get(base).is_some();
        let value = if at_call {
            let (outer, bound) = (self.ev, std::mem::take(&mut self.bound));
            self.ev = ev;
            let value = self.expr(base);
            self.ev = outer;
            self.bound = bound;
            value
        } else {
            self.expr(base)
        };
        let (handler, _) = self
            .cap_table(value, &tags, op)
            .expect("the operation's effect is one of the capability's");
        let h_ty = self.module.handler(effect_id);
        let h = self.module.local("h", h_ty);
        // The handler is bound inside the lambda when it is looked up at the call, and outside
        // it otherwise.
        let bind = |lambda: Expr, handler: Expr| -> Expr {
            match lambda {
                Expr::Lambda {
                    params,
                    ty,
                    body,
                    suspends,
                } if at_call => Expr::Lambda {
                    params,
                    ty,
                    body: Box::new(Expr::Let {
                        var: h,
                        value: Box::new(handler),
                        body,
                    }),
                    suspends,
                },
                lambda => Expr::Let {
                    var: h,
                    value: Box::new(handler),
                    body: Box::new(lambda),
                },
            }
        };
        // start of TaskGroup and Spawn wraps the task function, as at a call (`fiber.rs`).
        if self.is_start_op(tag.def, op) {
            let op_ty = self.expr_ty(id).clone();
            let Some(fork) = self.start_call(Expr::Var(h), &tag, &op_ty, id) else {
                return self.unsupported("start", span);
            };
            let x_ty = param_tys.first().copied().unwrap_or(TypeId::UNIT);
            let x = self.module.local("x", x_ty);
            // The evidence table, if the type takes one, is used only to look up the handler:
            // operation calls take no table (`Callee::Op`).
            let mut params = vec![x];
            params.extend(ev);
            let lambda = Expr::Lambda {
                params,
                ty,
                body: Box::new(fork.apply(vec![Expr::Var(x)])),
                suspends: false,
            };
            return bind(lambda, handler);
        }
        let mut params: Vec<Local> = param_tys
            .into_iter()
            .map(|t| self.module.local("x", t))
            .collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        params.extend(ev);
        let call = Expr::Call {
            callee: Callee::Op {
                handler: Box::new(Expr::Var(h)),
                effect: effect_id,
                op: index,
                ty_args: self.op_ty_args(effect_id, index, id),
            },
            args,
            fails,
            suspends,
        };
        let lambda = Expr::Lambda {
            params,
            ty,
            body: Box::new(self.builtin_direct(call)),
            suspends,
        };
        bind(lambda, handler)
    }

    /// The capability of the handler whose operation body or release is being lowered: the value
    /// that the function of the handler makes from the state.
    pub(super) fn self_capability(&mut self) -> Option<Expr> {
        let (def, state, ev) = self.self_cap?;
        let args = state.into_iter().chain(ev).map(Expr::Var).collect();
        Some(Expr::Call {
            callee: Callee::Direct {
                f: self.fn_id(def, 0),
                ty_args: Vec::new(),
            },
            args,
            fails: false,
            suspends: false,
        })
    }

    /// A capability passed as fewer effects (`[effect.use]`): the handlers of `wanted`, taken
    /// from the tuple `value` holds. The other handlers are dropped, so the operations of the
    /// other effects cannot be reached through the result.
    pub(super) fn narrow(&mut self, id: ExprId, value: Expr, wanted: &[Tag]) -> Expr {
        let Some(Ty::Cap(have, _)) = self.types.exprs.get(id).cloned() else {
            return value;
        };
        let n = have.len();
        let mut picks: Vec<(usize, TypeId)> = Vec::with_capacity(wanted.len());
        for w in wanted {
            let Some(k) = have.iter().position(|h| h.def == w.def) else {
                return self.unsupported("narrowing a capability", self.hir.exprs[id].span);
            };
            let effect = self.effect_id(&self.key_here(&have[k]));
            let effect = self.module.handler_effect(effect);
            picks.push((k, self.module.handler(effect)));
        }
        if let [(k, ty)] = picks.as_slice() {
            return super::member::tuple_project(value, *k, n, *ty, &mut self.module);
        }
        let all_ty = self.ty(&Ty::Cap(have, Row::empty()));
        let all = self.module.local("cap", all_ty);
        let elems = picks
            .into_iter()
            .map(|(k, ty)| super::member::tuple_get(all, k, n, ty, &mut self.module))
            .collect();
        Expr::Let {
            var: all,
            value: Box::new(value),
            body: Box::new(Expr::Tuple(elems)),
        }
    }

    pub(super) fn expr_kind(&mut self, id: ExprId) -> Expr {
        // `self.hir` is `&'a Program`, so a copy lets us read the HIR without borrowing `self`.
        let hir = self.hir;
        let e = &hir.exprs[id];
        match &e.kind {
            ExprKind::Lit(lit) => self.lit(id, lit),
            ExprKind::Path(res) => self.path(id, *res, e.span),
            ExprKind::Call { callee, args } => {
                let mut written: Vec<ExprId> = args.iter().map(|a| a.value).collect();
                // A method call `x.f(a)` is `M.f(x, a)` (`[type.rules]`).
                if let ExprKind::Field { base, .. } = &hir.exprs[*callee].kind
                    && self.types.methods.get(*callee).is_some()
                {
                    written.insert(0, *base);
                }
                self.call(id, *callee, written)
            }
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                // `x |> f(a)` is `f(x, a)`, and `x |> f` is `f(x)` (spec 6.7).
                let (callee, mut written) = match &hir.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        (*callee, args.iter().map(|a| a.value).collect())
                    }
                    _ => (*rhs, Vec::new()),
                };
                written.insert(0, *lhs);
                self.call(id, callee, written)
            }
            ExprKind::Binary { op, lhs, rhs } => self.binary(*op, *lhs, *rhs, e.span),
            ExprKind::Unary { op, operand } => {
                let (op, ty) = match op {
                    UnaryOp::Neg => (UnOp::Neg, self.op_ty(*operand)),
                    UnaryOp::Not => (UnOp::Not, Some(OpTy::Bool)),
                };
                let Some(ty) = ty else {
                    return self.unsupported("negation of this type", e.span);
                };
                Expr::Unary {
                    op,
                    ty,
                    operand: Box::new(self.expr(*operand)),
                }
            }
            ExprKind::Unit => Expr::Lit(Lit::Unit),
            ExprKind::Tuple(es) => Expr::Tuple(es.iter().map(|&e| self.expr(e)).collect()),
            ExprKind::List { elems, rest } => Expr::List {
                elem: match self.expr_ty(id) {
                    Ty::List(t) => self.ty(t),
                    _ => unreachable!("a list expression has a list type"),
                },
                elems: elems.iter().map(|&e| self.expr(e)).collect(),
                tail: rest.map(|r| Box::new(self.expr(r))),
            },
            ExprKind::Block(block) => self.block(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => Expr::If {
                cond: Box::new(self.expr(*cond)),
                then: Box::new(self.expr(*then_branch)),
                else_: Box::new(match else_branch {
                    Some(b) => self.expr(*b),
                    None => Expr::Lit(Lit::Unit),
                }),
            },
            ExprKind::Missing => self.unsupported("missing expression", e.span),
            ExprKind::Interpolated(parts) => self.interpolated(parts),
            ExprKind::Field { base, .. } => self.field(id, *base, e.span),
            ExprKind::Match { scrutinee, arms } => Expr::Match {
                scrutinee: Box::new(self.expr(*scrutinee)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Lambda { params, body } => self.lambda(id, params, *body),
            ExprKind::Assert(cond) => {
                let location = self.location(e.span);
                Assert::at(self.assert(*cond), location)
            }
            ExprKind::Update { ctor, fields, base } => {
                self.update(id, *ctor, fields, *base, e.span)
            }
            ExprKind::Escape { expr, arms } => Expr::Escape {
                body: Box::new(self.expr(*expr)),
                arms: arms.iter().map(|arm| self.arm(arm)).collect(),
            },
            ExprKind::Fail(inner) => {
                // An error reaching the entry is shown by its Show, through the impls if any
                // are written (monomorphization's `error_display`, spec 14.3).
                let shown = match self.expr_ty(*inner) {
                    Ty::Named(con, _) => {
                        let def = self.types.ty_table.def(*con);
                        let traits = &self.types.traits;
                        hir.prelude_def("Show").is_some_and(|show| {
                            traits.impl_of(show, def).is_some()
                                || traits.derived(show, def).is_some()
                        })
                    }
                    _ => true,
                };
                if shown {
                    self.prelude_trait_fn_id("Show", "show");
                }
                Expr::Fail(Box::new(self.expr(*inner)))
            }
            ExprKind::Use { .. } => self.use_(id, e.span),
            ExprKind::With { handlers, body } => self.with(id, handlers, *body, e.span),
        }
    }

    pub(super) fn lit(&mut self, id: ExprId, lit: &Literal) -> Expr {
        Expr::Lit(self.lit_value(self.expr_ty(id), lit))
    }

    /// The type (Int or Int64) decides an integer's width. The range was checked by the type
    /// checker (E3004).
    pub(super) fn lit_value(&self, ty: &Ty, lit: &Literal) -> Lit {
        match lit {
            Literal::Int(text) => {
                let n = int_literal_value(text)
                    .expect("an integer whose range the type checker checked");
                if *ty == Ty::INT64 {
                    Lit::Int64(n as i64)
                } else {
                    Lit::Int(n as i32)
                }
            }
            Literal::Float(text) => Lit::Float(text.parse().expect("a float checked by the lexer")),
            Literal::String(s) => Lit::String(s.clone()),
        }
    }

    /// A name reference outside a call (a reference as a value).
    pub(super) fn path(&mut self, id: ExprId, res: Res, span: Span) -> Expr {
        if self.types.self_caps.contains(&id)
            && let Some(value) = self.self_capability()
        {
            return value;
        }
        let def = match res {
            // `self` of a stateless handler has no value (the function building the handler value
            // does not take one).
            Res::Local(l)
                if self.hir.locals[l].kind == LocalKind::SelfParam
                    && self.locals.get(l).is_none() =>
            {
                return Expr::Lit(Lit::Unit);
            }
            Res::Local(l) => {
                return match self.lifted(res) {
                    Some(lifted) => self.lifted_value(id, &lifted),
                    None => Expr::Var(self.local(l)),
                };
            }
            Res::Def(def) => def,
            Res::Err => return self.unsupported("unresolved name", span),
        };
        if Some(def) == self.true_def {
            return Expr::Lit(Lit::Bool(true));
        }
        if Some(def) == self.false_def {
            return Expr::Lit(Lit::Bool(false));
        }
        match self.hir.item(def) {
            // A generic fn with effects with type arguments in its R must add keys at every call,
            // so it is wrapped.
            Some(Item::Fn(f)) if f.body.is_some() && self.has_generic_tags(def) => {
                self.wrap_in_lambda(id, span)
            }
            Some(Item::Fn(f)) if f.body.is_some() => self.fn_value(def, id),
            // Builtins and constructors with fields are not IR values, so they are wrapped in a
            // lambda.
            Some(Item::Fn(f)) if f.is_intrinsic => self.wrap_in_lambda(id, span),
            Some(Item::Fn(_)) => self.fn_value(def, id),
            _ if self.hir.defs[def].kind == DefKind::TraitFn => {
                if self.has_generic_tags(def) {
                    self.wrap_in_lambda(id, span)
                } else {
                    self.fn_value(def, id)
                }
            }
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) if self.module.variant(ctor).fields.is_empty() => Expr::Ctor {
                    ctor,
                    ty_args: self.ty_args(def, id),
                    args: Vec::new(),
                },
                _ => self.wrap_in_lambda(id, span),
            },
            // A const's value cannot fail (spec 5.6).
            Some(Item::Const(_)) => Expr::Call {
                callee: Callee::Direct {
                    f: self.fn_id(def, 0),
                    ty_args: self.ty_args(def, id),
                },
                args: Vec::new(),
                fails: false,
                suspends: false,
            },
            _ => self.unsupported("reference to this kind of definition", span),
        }
    }

    /// The fn `def` referenced by `expr` as a value. Function values always take the evidence
    /// table (`ty_takes_ev`); monomorphization references a copy that does not through an entry
    /// that takes it (`Mono::ev_entry`).
    pub(super) fn fn_value(&mut self, def: DefId, expr: ExprId) -> Expr {
        Expr::Fn {
            f: self.fn_ref(def, expr),
            ty_args: self.ty_args(def, expr),
        }
    }

    pub(super) fn binary(&mut self, op: BinaryOp, lhs: ExprId, rhs: ExprId, span: Span) -> Expr {
        let operand = self.op_ty(lhs);
        let (op, ty) = match op {
            BinaryOp::Add => (BinOp::Add, operand),
            BinaryOp::Sub => (BinOp::Sub, operand),
            BinaryOp::Mul => (BinOp::Mul, operand),
            BinaryOp::Div => (BinOp::Div, operand),
            BinaryOp::Rem => (BinOp::Rem, operand),
            // Equality of other values goes through Eq: monomorphization compares by structure
            // where Eq gives the same, and calls `Eq.eq` otherwise (spec 10.3, 10.6).
            BinaryOp::Eq => (BinOp::Eq, Some(self.eq_op_ty(operand))),
            BinaryOp::Ne => (BinOp::Ne, Some(self.eq_op_ty(operand))),
            // Ordering of other values is `Ord.compare` (spec 10.3).
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge if operand.is_none() => {
                let ty = self.ty(self.expr_ty(lhs));
                let (a, b) = (self.expr(lhs), self.expr(rhs));
                return self.compare(op, ty, a, b, span);
            }
            BinaryOp::Lt => (BinOp::Lt, operand),
            BinaryOp::Le => (BinOp::Le, operand),
            BinaryOp::Gt => (BinOp::Gt, operand),
            BinaryOp::Ge => (BinOp::Ge, operand),
            BinaryOp::And => (BinOp::And, Some(OpTy::Bool)),
            BinaryOp::Or => (BinOp::Or, Some(OpTy::Bool)),
            BinaryOp::Pipe => unreachable!("a pipe is lowered as a call"),
        };
        let Some(ty) = ty else {
            return self.unsupported("this operator on this type", span);
        };
        Expr::Binary {
            op,
            ty,
            lhs: Box::new(self.expr(lhs)),
            rhs: Box::new(self.expr(rhs)),
        }
    }

    /// The operand type of `==` on values whose primitive operand type is `operand`:
    /// `OpTy::Structural` if not primitive, and then `Eq.eq` is lowered for monomorphization.
    pub(super) fn eq_op_ty(&mut self, operand: Option<OpTy>) -> OpTy {
        operand.unwrap_or_else(|| {
            self.prelude_trait_fn_id("Eq", "eq");
            OpTy::Structural
        })
    }

    /// `lhs < rhs` (and `<=`, `>`, `>=`) on values that are not primitive:
    /// `Ord.compare(lhs, rhs) match { Less -> True, _ -> False }` and so on (spec 10.3).
    pub(super) fn compare(
        &mut self,
        op: BinaryOp,
        ty: TypeId,
        lhs: Expr,
        rhs: Expr,
        span: Span,
    ) -> Expr {
        let hir = self.hir;
        let (Some(compare), Some(less), Some(greater)) = (
            self.prelude_trait_fn_id("Ord", "compare"),
            hir.prelude_def("Less")
                .and_then(|d| self.ctors.get(d).copied()),
            hir.prelude_def("Greater")
                .and_then(|d| self.ctors.get(d).copied()),
        ) else {
            return self.unsupported("this operator on this type", span);
        };
        // The variant that decides, and the answer when it is the result.
        let (ctor, when) = match op {
            BinaryOp::Lt => (less, true),
            BinaryOp::Le => (greater, false),
            BinaryOp::Gt => (greater, true),
            _ => (less, false),
        };
        let call = Expr::Call {
            callee: Callee::Direct {
                f: compare,
                ty_args: vec![ty],
            },
            args: vec![lhs, rhs],
            fails: false,
            suspends: false,
        };
        Expr::Match {
            scrutinee: Box::new(call),
            arms: vec![
                Arm {
                    pat: Pat::Ctor {
                        ctor,
                        fields: Vec::new(),
                    },
                    guard: None,
                    body: Expr::Lit(Lit::Bool(when)),
                },
                Arm {
                    pat: Pat::Wild,
                    guard: None,
                    body: Expr::Lit(Lit::Bool(!when)),
                },
            ],
        }
    }

    /// `None` unless it is a primitive type.
    pub(super) fn op_ty(&self, operand: ExprId) -> Option<OpTy> {
        prim_op_ty(self.expr_ty(operand))
    }

    /// `assert expr` (spec 13.2). A comparison becomes the form that shows both sides with the
    /// debug display (`Assert::compare`, `inspect` in spec 10.6), which shows functions as `<fn>`
    /// and needs no Show. Only a side holding a capability value falls back to the form that
    /// shows no values (`Assert::bool`).
    pub(super) fn assert(&mut self, cond: ExprId) -> Expr {
        let hir = self.hir;
        let c = &hir.exprs[cond];
        let text = self.source_text(c.span);
        if let ExprKind::Binary { op, lhs, rhs } = &c.kind
            && let Some(bin) = comparison(*op)
        {
            let ty = self.ty(self.expr_ty(*lhs));
            let op_ty = match (self.op_ty(*lhs), bin) {
                (Some(t), _) => Some(t),
                (None, BinOp::Eq | BinOp::Ne) => Some(self.eq_op_ty(None)),
                (None, _) => None,
            };
            // Monomorphization shows the sides through a written Show impl (spec 10.6).
            if prim_op_ty(self.expr_ty(*lhs)).is_none() {
                self.prelude_trait_fn_id("Show", "show");
            }
            let (l, r) = (self.expr(*lhs), self.expr(*rhs));
            if let Some(op_ty) = op_ty {
                return Assert::compare(bin, op_ty, ty, l, r, text);
            }
            // An ordering through `Ord.compare`: the sides are bound first and shown if it
            // fails (spec 13.2).
            let (a, b) = (
                self.module.local("left", ty),
                self.module.local("right", ty),
            );
            let decide = self.compare(*op, ty, Expr::Var(a), Expr::Var(b), c.span);
            return Assert::shown(decide, bin, ty, (a, l), (b, r), text);
        }
        let cond = self.expr(cond);
        Assert::bool(cond, text)
    }

    /// `<path>:<line>:<column>` of the start of `span` (`[cmd.test.outcome]`). Lines and columns
    /// count from 1, and a column counts characters (Unicode scalar values) from the start of the
    /// line, as the code frames of diagnostics do. `None` without a source or a shown path.
    pub(super) fn location(&self, span: Span) -> Option<String> {
        let path = self.shown_paths.get(span.module)?;
        let source = self.sources.get(span.module)?;
        let before = source.get(..usize::from(span.range.start()))?;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let line = before.matches('\n').count() + 1;
        let column = before[line_start..].chars().count() + 1;
        Some(format!("{path}:{line}:{column}"))
    }

    /// The source text; empty for positions without a source.
    ///
    /// The return value borrows the source (`'a`), not `self`, so `self.expr(..)` (`&mut self`) can
    /// be called while holding the text.
    pub(super) fn source_text(&self, span: Span) -> &'a str {
        let range = usize::from(span.range.start())..usize::from(span.range.end());
        let sources: &'a ArenaMap<ModuleId, &'a str> = self.sources;
        sources
            .get(span.module)
            .and_then(|src| src.get(range))
            .unwrap_or_default()
    }

    /// A field access. Only single-constructor types (type, error) have fields, so the constructor
    /// is that type's only one. The index was looked up by the type checker
    /// (`ProgramTypes::fields`).
    pub(super) fn field(&mut self, id: ExprId, base: ExprId, span: Span) -> Expr {
        // Missing from the table means a capability operation (`log.info`) used as a value.
        let Some(&index) = self.types.fields.get(id) else {
            return match self.types.ops.get(id) {
                Some(&op) => self.op_value(id, base, op, span),
                None => self.unsupported("effect operation", span),
            };
        };
        let Ty::Named(con, _) = self.expr_ty(base) else {
            unreachable!("a value with a field has a named type");
        };
        let ctor = CtorRef {
            enum_id: self.enums[self.types.ty_table.def(*con)],
            variant: 0,
        };
        Expr::Field {
            base: Box::new(self.expr(base)),
            ctor,
            index,
        }
    }

    /// A record update `C(f: e, ..base)` (spec 5.3). Written fields are evaluated in written order,
    /// and `..base` last (spec 14.1). Unwritten fields are taken from `base`.
    pub(super) fn update(
        &mut self,
        id: ExprId,
        ctor: ExprId,
        fields: &[Arg],
        base: ExprId,
        span: Span,
    ) -> Expr {
        let ExprKind::Path(Res::Def(def)) = self.hir.exprs[ctor].kind else {
            unreachable!("the constructor of a record update is a constructor reference");
        };
        // The constructor table has every constructor (including stateful handlers').
        let Some(&ctor) = self.ctors.get(def) else {
            return self.unsupported("record update of this constructor", span);
        };
        let slots = &self.types.calls[id];
        let mut args: Vec<Option<Expr>> = vec![None; slots.len()];
        let mut lets = Vec::new();
        for field in fields {
            let pos = slots
                .iter()
                .position(|s| *s == Some(field.value))
                .expect("the type checker placed each field at a position");
            let value = self.expr(field.value);
            args[pos] = Some(if is_atomic(&value) {
                value
            } else {
                let ty = self.expr_ty(field.value);
                let ty = self.ty(ty);
                let l = self.module.local("arg", ty);
                lets.push((l, value));
                Expr::Var(l)
            });
        }
        let base_ty = self.expr_ty(base);
        let base_ty = self.ty(base_ty);
        let b = self.module.local("base", base_ty);
        let base = self.expr(base);
        lets.push((b, base));
        let args = args
            .into_iter()
            .enumerate()
            .map(|(index, arg)| {
                arg.unwrap_or_else(|| Expr::Field {
                    base: Box::new(Expr::Var(b)),
                    ctor,
                    index,
                })
            })
            .collect();
        // The update has the constructed type, whose arguments are the enum's.
        let ty_args = match self.expr_ty(id) {
            Ty::Named(_, tys) => tys.iter().map(|t| self.ty(t)).collect(),
            _ => Vec::new(),
        };
        let value = Expr::Ctor {
            ctor,
            ty_args,
            args,
        };
        lets.into_iter()
            .rev()
            .fold(value, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }

    /// String interpolation. Embedded values carry their types (backends build show functions from
    /// them).
    pub(super) fn interpolated(&mut self, parts: &[HirStrPart]) -> Expr {
        let parts = parts
            .iter()
            .map(|part| match part {
                HirStrPart::Text(s) => StrPart::Lit(s.clone()),
                HirStrPart::Interp(e) => {
                    // The type checker has checked Show (spec 10.6); no trait is implemented
                    // for a capability value (`[effect.use]`).
                    let ty = self.ty(self.expr_ty(*e));
                    // Monomorphization shows by structure where Show gives the same, and calls
                    // `Show.show` otherwise (spec 10.3, 10.6).
                    if prim_op_ty(self.expr_ty(*e)).is_none() {
                        self.prelude_trait_fn_id("Show", "show");
                    }
                    StrPart::Value(self.expr(*e), ty)
                }
            })
            .collect();
        Expr::Concat(parts)
    }

    /// A block: statements become nested `let`s, and statements whose value is unused are bound to
    /// a discard variable. Bindings with patterns other than a variable or `_` (`(a, b) = pair`)
    /// become a one-arm match with the remaining statements in its body. The type checker has
    /// checked that binding patterns always succeed (E3108).
    pub(super) fn block(&mut self, block: &Block) -> Expr {
        let hir = self.hir;
        let mut binds = Vec::new();
        for stmt in &block.stmts {
            let (pat, value) = match stmt {
                Stmt::Let { pat, value, .. } => (Some(*pat), *value),
                Stmt::LocalFn(f) => {
                    self.local_fn(f);
                    continue;
                }
                Stmt::Expr(e) => (None, *e),
            };
            let value_ty = self.expr_ty(value);
            let value_ty = self.ty(value_ty);
            let value = self.expr(value);
            let bind = match pat.map(|p| (p, &hir.pats[p].kind)) {
                None | Some((_, PatKind::Wild)) => {
                    Bind::Var(self.module.local("_", value_ty), value)
                }
                Some((_, PatKind::Bind(l))) => Bind::Var(self.local(*l), value),
                Some((p, _)) => {
                    let pat = self.pat(p);
                    // Bindings that always succeed (E3108) never have const patterns.
                    if !std::mem::take(&mut self.pat_guards).is_empty() {
                        self.report("const pattern in binding", hir.pats[p].span);
                    }
                    Bind::Pat(pat, value)
                }
            };
            binds.push(bind);
        }
        let tail = match block.tail {
            Some(e) => self.expr(e),
            None => Expr::Lit(Lit::Unit),
        };
        binds.into_iter().rev().fold(tail, |body, bind| match bind {
            Bind::Var(var, value) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            },
            Bind::Pat(pat, value) => Expr::Match {
                scrutinee: Box::new(value),
                arms: vec![Arm {
                    pat,
                    guard: None,
                    body,
                }],
            },
        })
    }

    /// A match arm. Comparisons for const patterns become a guard evaluated before the written
    /// guard.
    pub(super) fn arm(&mut self, arm: &HirArm) -> Arm {
        let pat = self.pat(arm.pat);
        let mut guards = std::mem::take(&mut self.pat_guards);
        if let Some(g) = arm.guard {
            guards.push(self.expr(g));
        }
        Arm {
            pat,
            guard: guards.into_iter().reduce(|a, b| Expr::Binary {
                op: BinOp::And,
                ty: OpTy::Bool,
                lhs: Box::new(a),
                rhs: Box::new(b),
            }),
            body: self.expr(arm.body),
        }
    }

    /// Const patterns push their comparisons onto `pat_guards` (the caller turns them into a
    /// guard).
    pub(super) fn pat(&mut self, id: PatId) -> Pat {
        let hir = self.hir;
        let p = &hir.pats[id];
        match &p.kind {
            PatKind::Wild => Pat::Wild,
            PatKind::Bind(l) => Pat::Bind(self.local(*l)),
            PatKind::Lit(lit) => Pat::Lit(self.lit_value(&self.types.pats[id], lit)),
            PatKind::Unit => Pat::Lit(Lit::Unit),
            PatKind::Tuple(ps) => Pat::Tuple(ps.iter().map(|&p| self.pat(p)).collect()),
            PatKind::List { elems, rest } => Pat::List {
                elems: elems.iter().map(|&p| self.pat(p)).collect(),
                rest: rest.map(|r| {
                    Box::new(match r.binding {
                        Some(l) => Pat::Bind(self.local(l)),
                        None => Pat::Wild,
                    })
                }),
            },
            PatKind::Ctor {
                res: Res::Def(def), ..
            } => {
                if Some(*def) == self.true_def {
                    return Pat::Lit(Lit::Bool(true));
                }
                if Some(*def) == self.false_def {
                    return Pat::Lit(Lit::Bool(false));
                }
                let Some(&ctor) = self.ctors.get(*def) else {
                    self.report("this constructor pattern", p.span);
                    return Pat::Wild;
                };
                // The type checker reordered fields by position; omitted fields are `None`.
                // Constructor patterns without parentheses (`Empty`) are not in the table.
                let n = self.module.variant(ctor).fields.len();
                let fields = match self.types.pat_fields.get(id) {
                    Some(fs) => fs
                        .iter()
                        .map(|f| match f {
                            Some(f) => self.pat(*f),
                            None => Pat::Wild,
                        })
                        .collect(),
                    None => vec![Pat::Wild; n],
                };
                Pat::Ctor { ctor, fields }
            }
            // A const pattern binds a fresh variable and compares it with the const's value in a
            // guard.
            PatKind::Const(Res::Def(def)) => {
                let types = self.types;
                let x_ty = self.ty(&types.pats[id]);
                let x = self.module.local("c", x_ty);
                let ty = self.eq_op_ty(prim_op_ty(&types.pats[id]));
                let value = Expr::Call {
                    callee: Callee::Direct {
                        f: self.fn_id(*def, 0),
                        ty_args: self.const_ty_args(*def, &types.pats[id]),
                    },
                    args: Vec::new(),
                    fails: false,
                    suspends: false,
                };
                self.pat_guards.push(Expr::Binary {
                    op: BinOp::Eq,
                    ty,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(value),
                });
                Pat::Bind(x)
            }
            PatKind::Ctor { .. } | PatKind::Const(_) | PatKind::Missing => {
                self.report("this pattern", p.span);
                Pat::Wild
            }
        }
    }
}
