//! suspend, Async and Immediate (spec 8.4, 8.7, 9.5).
//!
//! Function types carry no suspend flag. That a call may suspend is expressed by the effects in
//! its R (an effect with a suspend fn, or Async); whether an operation suspends is read from its
//! declaration (`OpItem::sig.is_suspend` in the HIR).
//!
//! - If the body of a suspend fn implementation uses `resume` in any way other than calling it
//!   directly (`resume(v)`), resume is assumed to be callable later (spec 8.7). This is decided
//!   syntactically. References inside a lambda do not count as direct calls even if they look
//!   like one, since the closure captured `resume`. Async is added to that operation body's R
//!   (making it a handler dependency).
//! - When a definition with a type parameter bounded by `R: Immediate` is referenced, the
//!   instantiating set is recorded and checked for Immediate after sets are solved: each effect
//!   must have no suspend fn and not be Async, and each set type parameter must have an
//!   `Immediate` bound. Set variables not yet generalized get the `Immediate` bound when they are
//!   generalized.
//! - Invisible sets from an omitted `use` on a function-typed parameter of an external function
//!   carry an `Immediate` bound (callbacks must be Immediate, spec 9.5), so the same mechanism
//!   checks them.
//! - The body of an fn operation implementation must either have an Immediate R or contain no
//!   call that may suspend (E0417, spec 8.1). Whether a call may suspend is decided by the
//!   operation's declaration (`suspend fn` or not) for operation calls, always for
//!   `@external suspend fn`, and by the callee type's R (not Immediate) for functions and
//!   function values.

use emela_resolve::hir::{
    BinaryOp, DefId, ExprId, ExprKind, Item, LocalId, Program, Res, Span, Stmt, StrPart,
};
use smol_str::SmolStr;

use super::{BodyInfer, ImmediateObligation};
use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource, SuspendingCall};
use crate::lower::{IMMEDIATE, TyTable, is_immediate_effect};
use crate::row::{Row, RowElem};
use crate::signature::Signatures;
use crate::ty::{Ty, TyParam};

impl BodyInfer<'_> {
    /// A reference to `def` instantiated type parameters `params` with the sets `rows`. Records
    /// the instantiations of set type parameters with an `Immediate` bound, to check later.
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
            // An external function's invisible sets (`R(f)`) are callback Rs. Written type
            // parameters are reported as bounds.
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

    /// Checks that the recorded sets are Immediate (E0416). Call after sets are solved.
    pub(super) fn check_immediates(&mut self) {
        let program = self.program;
        for ob in std::mem::take(&mut self.immediates) {
            let row = self.ctx.resolve_row(&ob.row);
            let params = ob.owner.map(|d| self.def_params(d)).unwrap_or_default();
            let mut effects: Vec<SmolStr> = Vec::new();
            let mut sets: Vec<SmolStr> = Vec::new();
            for e in row.elems() {
                match e {
                    RowElem::Tag(effect) if !is_immediate_effect(program, effect.def) => {
                        effects.push(program.defs[effect.def].name.clone());
                    }
                    RowElem::Bound(i) => match params.get(*i as usize) {
                        Some(p) if p.bounds.iter().any(|b| b == IMMEDIATE) => {}
                        Some(p) => sets.push(p.name.clone()),
                        None => {}
                    },
                    // A set variable to be generalized; it gets the bound at generalization.
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
    /// Checks that fn operation implementation bodies do not suspend (E0417, spec 8.1). For an fn
    /// operation, "the caller continues as soon as the handler returns a value", so callers are
    /// emitted assuming no suspension. Call after sets are solved.
    ///
    /// Passes if the body's R is Immediate or the body contains no call that may suspend; either
    /// alone guarantees the body does not suspend. The latter looks at each call in the body
    /// ([`Self::suspending_call`]). Calls inside a lambda's body count where the lambda is called,
    /// as a call of a function value. If neither holds, each call that may suspend is reported.
    pub(super) fn check_fn_op_bodies(&mut self) {
        let program = self.program;
        let bodies: Vec<(DefId, usize, ExprId)> = self
            .handler_bodies
            .iter()
            .filter(|b| !b.suspend)
            .map(|b| (b.handler, b.index, b.body))
            .collect();
        for (handler, index, body) in bodies {
            let Some(Item::Handler(h)) = program.item(handler) else {
                continue;
            };
            // If the body's R is Immediate, the body does not suspend even if it contains calls
            // that may (inside a `with` of a handler that resumes immediately), so pass without
            // looking at the calls. Handlers have no type parameters, so only tags in R matter.
            let row = self
                .expr_effects
                .get(body)
                .cloned()
                .map(|r| self.ctx.resolve_row(&r));
            let immediate = row.is_some_and(|r| {
                r.elems().iter().all(|e| match e {
                    RowElem::Tag(effect) => is_immediate_effect(program, effect.def),
                    _ => true,
                })
            });
            if immediate {
                continue;
            }
            let op = h.ops[index].name.name.clone();
            let mut stack = vec![body];
            while let Some(expr) = stack.pop() {
                let callee = match &program.exprs[expr].kind {
                    ExprKind::Call { callee, .. } => Some(*callee),
                    // `x |> f` is `f(x)`. If the right side is a call, count that call expression.
                    ExprKind::Binary {
                        op: BinaryOp::Pipe,
                        rhs,
                        ..
                    } if !matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) => Some(*rhs),
                    _ => None,
                };
                if let Some(call) = callee.and_then(|c| self.suspending_call(c)) {
                    self.diags.push(Diagnostic {
                        span: Some(program.exprs[expr].span),
                        kind: DiagnosticKind::SuspendingFnOp {
                            op: op.clone(),
                            call,
                        },
                    });
                }
                // Push in reverse so reports come in source order.
                stack.extend(self.sub_exprs(expr).into_iter().rev());
            }
        }
    }

    /// Why a call of `callee` may suspend, if it may (spec 8.1).
    ///
    /// - A capability operation call may suspend if the operation is declared `suspend fn`.
    /// - A call of an `@external suspend fn` may suspend.
    /// - A call of a function or function value may suspend if the callee type's R is not
    ///   Immediate. Not suspending is never inferred from the function's body, since signatures
    ///   carry no suspension marker.
    fn suspending_call(&mut self, callee: ExprId) -> Option<SuspendingCall> {
        let program = self.program;
        if let Some(&op) = self.ops.get(callee) {
            let Some(Item::Op(o)) = program.item(op) else {
                return None;
            };
            if !o.sig.is_suspend {
                return None;
            }
            let data = &program.defs[op];
            let effect = data
                .parent
                .map(|e| program.defs[e].name.clone())
                .unwrap_or_default();
            return Some(SuspendingCall::Op {
                effect,
                op: data.name.clone(),
            });
        }
        let named = match program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => Some(def),
            _ => None,
        };
        if let Some(def) = named
            && let Some(Item::Fn(f)) = program.item(def)
            && f.is_external
            && f.sig.is_suspend
        {
            return Some(SuspendingCall::External {
                callee: program.defs[def].name.clone(),
            });
        }
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        let Ty::Fn(f) = self.ctx.shallow_resolve(&ty) else {
            return None;
        };
        let row = self.ctx.resolve_row(&f.uses);
        let effects: Vec<SmolStr> = row
            .elems()
            .iter()
            .filter_map(|e| match e {
                RowElem::Tag(effect) if !is_immediate_effect(program, effect.def) => {
                    Some(program.defs[effect.def].name.clone())
                }
                _ => None,
            })
            .collect();
        if effects.is_empty() {
            return None;
        }
        let callee = match program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => Some(program.defs[def].name.clone()),
            ExprKind::Path(Res::Local(l)) => Some(program.locals[l].name.clone()),
            _ => None,
        };
        Some(SuspendingCall::Fn { callee, effects })
    }
}

/// Whether the body `body` of a suspend fn implementation uses `resume` (the local `resume`) in
/// any way other than calling it directly (spec 8.7). Calling directly means writing `resume(v)`
/// or `v |> resume` outside any lambda. The arguments of a handler a later item of a `with`
/// overrides are not evaluated (`[effect.with]`), so `resume` in them does not count.
pub(super) fn resume_escapes(infer: &BodyInfer<'_>, body: ExprId, resume: LocalId) -> bool {
    Walker {
        program: infer.program,
        sigs: infer.sigs,
        ty_table: infer.ty_table,
        resume,
        escapes: false,
    }
    .run(body)
}

struct Walker<'a> {
    program: &'a Program,
    sigs: &'a Signatures,
    ty_table: &'a TyTable,
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

    /// An expression in callee position. For resume, a direct call outside any lambda.
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
            ExprKind::With { handlers, body } => {
                let args =
                    crate::layer::evaluated_args(program, self.sigs, self.ty_table, handlers);
                for arg in args {
                    self.expr(arg, in_lambda);
                }
                self.expr(*body, in_lambda);
            }
            ExprKind::Lambda { body, .. } => self.expr(*body, true),
        }
    }
}
