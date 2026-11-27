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
//!   call that may suspend (E4303, spec 8.1). Whether a call may suspend is decided by the
//!   operation's declaration (`suspend fn` or not) for operation calls, always for
//!   `@external suspend fn`, and by the callee type's R (not Immediate) for functions and
//!   function values.

use emela_resolve::hir::{
    BinaryOp, DefId, ExprId, ExprKind, Item, LocalId, Program, Res, Span, Stmt, StrPart, WithItem,
};
use smol_str::SmolStr;

use super::{BodyInfer, ImmediateObligation};
use crate::diagnostic::{Diagnostic, DiagnosticKind, ImmediateSource, SuspendingCall};
use crate::lower::{IMMEDIATE, TyTable, is_immediate_effect};
use crate::row::{CapElem, Row, RowElem};
use crate::signature::Signatures;
use crate::ty::{Ty, TyParam};

impl BodyInfer<'_> {
    /// A reference to `def` instantiated type parameters `params` with the sets `rows`. Records
    /// the instantiations of set type parameters with an `Immediate` bound, to check later.
    /// For an external function, the External of its module may be in the R of a callback
    /// (`[ext.boundary]`, `[effect.external]`): the host calls it as a host side effect that a
    /// handler of that module gave the host.
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
            let borrowed = external && self.borrowed_param(def, &p.name);
            let source = if external && p.name.contains('(') {
                ImmediateSource::ExternalCallback { callee }
            } else {
                ImmediateSource::Bound {
                    param: p.name.clone(),
                    callee,
                }
            };
            let allowed_external = match program.defs[def].module {
                emela_resolve::hir::DefModule::Source(m) if external => program.external_effect(m),
                _ => None,
            };
            self.immediates.push(ImmediateObligation {
                row: row.clone(),
                span,
                owner: self.current,
                source,
                borrowed,
                allowed_external,
            });
        }
    }

    /// Whether the parameter of the external function `def` that the invisible set `set` (`R(f)`)
    /// belongs to is `@borrowed` (`[ext.boundary]`).
    fn borrowed_param(&self, def: DefId, set: &str) -> bool {
        let program = self.program;
        let Some(Item::Fn(f)) = program.item(def) else {
            return false;
        };
        let name = set
            .split_once('(')
            .map(|(_, rest)| rest.trim_end_matches(')'))
            .and_then(|rest| rest.split(',').next())
            .unwrap_or_default();
        f.sig
            .params
            .iter()
            .any(|p| p.borrowed.is_some() && program.locals[p.local].name == name)
    }

    /// Checks that the recorded sets are Immediate (E4302). Call after sets are solved.
    pub(super) fn check_immediates(&mut self) {
        let program = self.program;
        for ob in std::mem::take(&mut self.immediates) {
            let row = self.ctx.resolve_row(&ob.row);
            let params = ob.owner.map(|d| self.def_params(d)).unwrap_or_default();
            let mut effects: Vec<SmolStr> = Vec::new();
            let mut sets: Vec<SmolStr> = Vec::new();
            // The R of a callback cannot contain an effect, since the host passes no handlers
            // (`[ext.boundary]`). A capability is allowed: the callback reaches its handler
            // through it. Host effects (Async) are left to the check of Immediate. The External
            // of the module of the external function is allowed; that of another module is
            // shown with its module (`Log.External`, `[effect.external]`).
            let mut callback: Vec<SmolStr> = Vec::new();
            let mut callback_defs: Vec<DefId> = Vec::new();
            let mut externals: Vec<SmolStr> = Vec::new();
            if let ImmediateSource::ExternalCallback { .. } = &ob.source {
                for e in row.elems() {
                    if let RowElem::Tag(effect) = e
                        && !program.is_host_effect(effect.def)
                        && Some(effect.def) != ob.allowed_external
                    {
                        let name = match program.external_module(effect.def) {
                            Some(m) => {
                                let module = program.modules[m].name.to_string();
                                if !externals.iter().any(|x| x == &module) {
                                    externals.push(module.clone().into());
                                }
                                SmolStr::from(format!("{module}.External"))
                            }
                            None => program.defs[effect.def].name.clone(),
                        };
                        if !callback.contains(&name) {
                            callback.push(name);
                            callback_defs.push(effect.def);
                        }
                    }
                }
            }
            // Nor a capability, since the host may keep the callback after the with of the
            // capability has ended, unless the parameter is `@borrowed`.
            if let ImmediateSource::ExternalCallback { callee } = &ob.source
                && !ob.borrowed
            {
                let mut caps: Vec<SmolStr> = Vec::new();
                for c in row.caps() {
                    if matches!(c.id, crate::row::CapId::Named { .. }) {
                        continue;
                    }
                    if !caps.contains(&c.name) {
                        caps.push(c.name.clone());
                    }
                }
                if !caps.is_empty() {
                    self.diags.push(Diagnostic {
                        span: Some(ob.span),
                        kind: DiagnosticKind::CallbackCapabilities {
                            caps,
                            callee: callee.clone(),
                        },
                    });
                }
            }
            if !callback.is_empty() {
                let ImmediateSource::ExternalCallback { callee } = &ob.source else {
                    unreachable!("only callbacks are collected");
                };
                let bound_item = !self.callback_redirects(ob.owner, ob.span, &callback_defs);
                self.diags.push(Diagnostic {
                    span: Some(ob.span),
                    kind: DiagnosticKind::CallbackEffects {
                        externals,
                        effects: callback.clone(),
                        callee: Some(callee.clone()),
                        bound_item,
                    },
                });
            }
            for e in row.elems() {
                match e {
                    RowElem::Tag(effect) if callback_defs.contains(&effect.def) => {}
                    // A capability counts as its effect (`[effect.use]`).
                    RowElem::Tag(effect) | RowElem::Cap(CapElem { tag: effect, .. })
                        if !is_immediate_effect(program, effect.def) =>
                    {
                        if !effects.contains(&program.defs[effect.def].name) {
                            effects.push(program.defs[effect.def].name.clone());
                        }
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
    /// Checks that fn operation implementation bodies do not suspend (E4303, spec 8.1). For an fn
    /// operation, "the caller continues as soon as the handler returns a value", so callers are
    /// emitted assuming no suspension. Call after sets are solved.
    ///
    /// Passes if the body's R is Immediate or the body contains no call that may suspend; either
    /// alone guarantees the body does not suspend. The latter looks at each call in the body
    /// ([`Self::suspending_call`]). Calls inside a lambda's body count where the lambda is called,
    /// as a call of a function value. If neither holds, each call that may suspend is reported.
    ///
    /// The init and the release of an inner handler that implements a handler member without
    /// `suspend` are checked the same way (`[effect.decl]`).
    pub(super) fn check_fn_op_bodies(&mut self) {
        let program = self.program;
        let mut bodies: Vec<(DefId, Option<usize>, ExprId)> = self
            .handler_bodies
            .iter()
            .filter(|b| !b.suspend)
            .map(|b| (b.handler, Some(b.index), b.body))
            .collect();
        bodies.extend(
            self.handler_parts
                .iter()
                .filter(|p| {
                    matches!(
                        program.item(p.handler),
                        Some(Item::Handler(h)) if matches!(
                            h.member,
                            Some(Res::Def(m)) if matches!(
                                program.item(m),
                                Some(Item::Member(m)) if !m.sig.is_suspend
                            )
                        )
                    )
                })
                .map(|p| (p.handler, None, p.body)),
        );
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
                    RowElem::Tag(effect) | RowElem::Cap(CapElem { tag: effect, .. }) => {
                        is_immediate_effect(program, effect.def)
                    }
                    _ => true,
                })
            });
            if immediate {
                continue;
            }
            let report = |call: SuspendingCall| match index {
                Some(index) => DiagnosticKind::SuspendingFnOp {
                    op: h.ops[index].name.name.clone(),
                    call,
                },
                None => {
                    let member = match h.member {
                        Some(Res::Def(m)) => m,
                        _ => unreachable!("an inner handler of a handler member"),
                    };
                    DiagnosticKind::SuspendingMemberPart {
                        member: program.defs[member].name.clone(),
                        effect: program.defs[member]
                            .parent
                            .map(|e| program.defs[e].name.clone())
                            .unwrap_or_default(),
                        call,
                    }
                }
            };
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
                        kind: report(call),
                    });
                }
                // Opening a suspend handler member as an item of `with` may suspend, there and
                // where the `with` ends (`[effect.decl]`).
                if let ExprKind::With { handlers, .. } = &program.exprs[expr].kind {
                    for item in handlers {
                        if let Some(call) = self.suspending_member_item(item) {
                            self.diags.push(Diagnostic {
                                span: Some(item.span),
                                kind: report(call),
                            });
                        }
                    }
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
        if let Some(&member) = self.member_opens.get(callee) {
            return self.suspending_member(member);
        }
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
        let named = self.callee_def(callee);
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
        let mut effects: Vec<SmolStr> = Vec::new();
        for effect in row.effect_tags() {
            let name = &program.defs[effect.def].name;
            if !is_immediate_effect(program, effect.def) && !effects.contains(name) {
                effects.push(name.clone());
            }
        }
        if effects.is_empty() {
            return None;
        }
        let callee = match program.exprs[callee].kind {
            ExprKind::Path(Res::Local(l)) => Some(program.locals[l].name.clone()),
            _ => self
                .callee_def(callee)
                .map(|def| program.defs[def].name.clone()),
        };
        Some(SuspendingCall::Fn { callee, effects })
    }
}

impl BodyInfer<'_> {
    /// Why opening `member` may suspend: it is a `suspend handler` (`[effect.decl]`).
    fn suspending_member(&self, member: DefId) -> Option<SuspendingCall> {
        let program = self.program;
        let Some(Item::Member(m)) = program.item(member) else {
            return None;
        };
        m.sig.is_suspend.then(|| SuspendingCall::Member {
            effect: program.defs[member]
                .parent
                .map(|e| program.defs[e].name.clone())
                .unwrap_or_default(),
            member: program.defs[member].name.clone(),
        })
    }

    /// Why the item of `with` `item` may suspend: it opens a `suspend handler` member.
    fn suspending_member_item(&self, item: &WithItem) -> Option<SuspendingCall> {
        let member = self.member_item(item)?;
        self.suspending_member(member)
    }

    /// Whether the body `body` of a suspend fn implementation uses `resume` (the local `resume`)
    /// in any way other than calling it directly (spec 8.7). Calling directly means writing
    /// `resume(v)` or `v |> resume` outside any lambda. The arguments of a handler a later item of
    /// a `with` overrides are not evaluated (`[effect.with]`), so `resume` in them does not count.
    pub(super) fn resume_escapes(&self, body: ExprId, resume: LocalId) -> bool {
        Walker {
            program: self.program,
            sigs: self.sigs,
            ty_table: self.ty_table,
            insts: self.items_insts(),
            resume,
            escapes: false,
        }
        .run(body)
    }

    /// The first mention of `resume` in `expr`, including inside lambdas and local fns: a use of it
    /// that is not a direct call where the implementation runs (`[effect.async]`).
    fn mention_of(&self, expr: ExprId, resume: LocalId) -> Resumed {
        let program = self.program;
        if is_local(program, expr, resume) {
            return Some((program.exprs[expr].span, true));
        }
        let mut subs = self.sub_exprs(expr);
        match &program.exprs[expr].kind {
            ExprKind::Lambda { body, .. } => subs.push(*body),
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    if let Stmt::LocalFn(f) = stmt {
                        subs.push(f.body);
                    }
                }
            }
            _ => {}
        }
        subs.into_iter()
            .filter_map(|sub| self.mention_of(sub, resume))
            .min_by_key(|(span, _)| span.range.start())
    }
}

struct Walker<'a> {
    program: &'a Program,
    sigs: &'a Signatures,
    ty_table: &'a TyTable,
    insts: super::items::ItemsInsts<'a>,
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
                        // The body of a local fn runs where it is called, as a lambda's does.
                        Stmt::LocalFn(f) => self.expr(f.body, true),
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
                let insts = self.insts.of(expr, handlers);
                let args = crate::layer::evaluated_args(
                    program,
                    self.sigs,
                    self.ty_table,
                    handlers,
                    &insts,
                );
                for arg in args {
                    self.expr(arg, in_lambda);
                }
                self.expr(*body, in_lambda);
            }
            ExprKind::Lambda { body, .. } => self.expr(*body, true),
        }
    }
}

/// Where resume may have been called on some path so far: the span of the direct call or of the
/// other use, and whether it was another use (`passed`).
type Resumed = Option<(Span, bool)>;

impl BodyInfer<'_> {
    /// Checks that no call that may suspend can be evaluated after resume may have been called
    /// in a suspend fn implementation (E4305, `[effect.handler]`). Call after sets are solved.
    ///
    /// The check follows the order of evaluation (`[dyn.order]`) and joins the paths of the
    /// branches of an if, a match and an escape: what follows a branch that may resume is after
    /// resume. resume may be called at a direct call of it (`resume(v)`, `v |> resume`), and at
    /// any other mention of it, including inside a lambda or a local fn, which lets a callback or
    /// another fiber call it while the implementation is still running (`[effect.async]`). A
    /// call inside a lambda runs where the lambda is called, so it is checked as a call of a
    /// function value there.
    pub(super) fn check_suspend_after_resume(&mut self) {
        let bodies: Vec<(ExprId, LocalId)> = self
            .handler_bodies
            .iter()
            .filter_map(|b| Some((b.body, b.resume?)))
            .collect();
        for (body, resume) in bodies {
            self.after_resume(body, resume, None);
        }
    }

    /// Walks `expr` given where resume may have been called before it, reports the calls that may
    /// suspend after it, and returns where resume may have been called after `expr`.
    fn after_resume(&mut self, expr: ExprId, resume: LocalId, mut state: Resumed) -> Resumed {
        let program = self.program;
        let span = program.exprs[expr].span;
        match &program.exprs[expr].kind {
            ExprKind::Path(Res::Local(l)) if *l == resume => state.or(Some((span, true))),
            ExprKind::Lambda { body, .. } => state.or(self.mention_of(*body, resume)),
            ExprKind::Call { callee, args } => {
                let direct = is_local(program, *callee, resume);
                if !direct {
                    state = self.after_resume(*callee, resume, state);
                }
                for arg in args {
                    state = self.after_resume(arg.value, resume, state);
                }
                if direct {
                    return state.or(Some((span, false)));
                }
                self.report_after_resume(*callee, span, state);
                state
            }
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                state = self.after_resume(*lhs, resume, state);
                if matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    return self.after_resume(*rhs, resume, state);
                }
                if is_local(program, *rhs, resume) {
                    return state.or(Some((span, false)));
                }
                state = self.after_resume(*rhs, resume, state);
                self.report_after_resume(*rhs, span, state);
                state
            }
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    state = match stmt {
                        Stmt::Let { value, .. } | Stmt::Expr(value) => {
                            self.after_resume(*value, resume, state)
                        }
                        Stmt::LocalFn(f) => state.or(self.mention_of(f.body, resume)),
                    };
                }
                match block.tail {
                    Some(tail) => self.after_resume(tail, resume, state),
                    None => state,
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                state = self.after_resume(*cond, resume, state);
                let then = self.after_resume(*then_branch, resume, state);
                let other = match else_branch {
                    Some(e) => self.after_resume(*e, resume, state),
                    None => state,
                };
                then.or(other)
            }
            ExprKind::Match {
                scrutinee: inner,
                arms,
            }
            | ExprKind::Escape { expr: inner, arms } => {
                // A guard is evaluated after the guards of the arms before it.
                let mut guards = self.after_resume(*inner, resume, state);
                let mut out = guards;
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        guards = self.after_resume(guard, resume, guards);
                    }
                    out = out.or(self.after_resume(arm.body, resume, guards));
                }
                out.or(guards)
            }
            ExprKind::With { handlers, body } => {
                for arg in crate::layer::evaluated_args(
                    program,
                    self.sigs,
                    self.ty_table,
                    handlers,
                    &self.item_insts(expr, handlers),
                ) {
                    state = self.after_resume(arg, resume, state);
                }
                // Opening a suspend handler member may suspend where the item opens it and where
                // the with releases it (`[effect.decl]`).
                let opened = state;
                let after = self.after_resume(*body, resume, state);
                for item in handlers {
                    if let Some(call) = self.suspending_member_item(item)
                        && let Some((at, passed)) = opened.or(after)
                    {
                        self.diags.push(Diagnostic {
                            span: Some(item.span),
                            kind: DiagnosticKind::SuspendAfterResume {
                                call,
                                resume: at,
                                passed,
                            },
                        });
                    }
                }
                after
            }
            _ => {
                for sub in self.sub_exprs(expr) {
                    state = self.after_resume(sub, resume, state);
                }
                state
            }
        }
    }

    /// Reports the call `call` of `callee` if it may suspend and resume may have been called.
    fn report_after_resume(&mut self, callee: ExprId, call: Span, state: Resumed) {
        let Some((resume, passed)) = state else {
            return;
        };
        if let Some(why) = self.suspending_call(callee) {
            self.diags.push(Diagnostic {
                span: Some(call),
                kind: DiagnosticKind::SuspendAfterResume {
                    call: why,
                    resume,
                    passed,
                },
            });
        }
    }
}

fn is_local(program: &Program, expr: ExprId, local: LocalId) -> bool {
    matches!(program.exprs[expr].kind, ExprKind::Path(Res::Local(l)) if l == local)
}
