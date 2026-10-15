//! Lowering concurrency (spec chapter 11).
//!
//! `@intrinsic` functions of `Std.Fiber`, `Std.Cause` and `Std.Deferred` are lowered to
//! `Expr::Fiber` in the Core IR ([`FiberFn`]). The capability value of the builtin effect Scope is
//! a handler value that lowering builds where `Fiber.scope` is called ([`ScopeCall`]). The
//! operations fork and deferred become ordinary operation calls; only fork wraps the child
//! function at the call site ([`ForkCall`]).
//!
//! A child uses the handlers in place when `Fiber.scope` was called, except that Scope refers to
//! the scope the child belongs to (spec 11.2). Scope's handler value captures the evidence table
//! at the `Fiber.scope` call, and fork's implementation calls the child with that table. Scope is
//! the capability value that forked the child (added when the call site wraps the child function).
//! A `with` inside the scope body does not reach children.

use emela_resolve::hir::{DefId, DefModule, ExprId, Item, Span};
use emela_types::{FnTy, Tag, Ty};

use super::{Call, Lowerer, Target};
use crate::ir::{Callee, EffectId, EnumId, Expr, FiberOp, FnType, Lit, Local, TypeId};

/// `@intrinsic` functions that lowering maps to `Expr::Fiber`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FiberFn {
    Scope,
    Join,
    Await,
    Interrupt,
    Attempt,
    Complete,
    DeferredAwait,
    Poll,
    CauseKind,
    CauseToString,
    CauseRaise,
}

impl FiberFn {
    /// Looks up from a bundled declaration (module name, function name).
    pub(super) fn lookup(module: &str, name: &str) -> Option<FiberFn> {
        Some(match (module, name) {
            ("Std.Fiber", "scope") => FiberFn::Scope,
            ("Std.Fiber", "join") => FiberFn::Join,
            ("Std.Fiber", "await") => FiberFn::Await,
            ("Std.Fiber", "interrupt") => FiberFn::Interrupt,
            ("Std.Fiber", "attempt") => FiberFn::Attempt,
            ("Std.Deferred", "complete") => FiberFn::Complete,
            ("Std.Deferred", "await") => FiberFn::DeferredAwait,
            ("Std.Deferred", "poll") => FiberFn::Poll,
            ("Std.Cause", "kind") => FiberFn::CauseKind,
            ("Std.Cause", "to_string") => FiberFn::CauseToString,
            ("Std.Cause", "raise") => FiberFn::CauseRaise,
            _ => return None,
        })
    }
}

/// What is needed to lower `Fiber.scope(body)`. Locals are allocated up front.
///
/// ```text
/// let b = body
/// fiber#scope(fn(s) {
///   let h = Scope { fork: fn(g) { fiber#fork(s, fn() { g(ev) }) }, deferred: fn() { fiber#deferred(s) } }
///   b(ev + { Scope: h })
/// })
/// ```
///
/// `ev` is the evidence table at the `Fiber.scope` call. fork's implementation calls the child
/// function `g` with that table (`g` is wrapped by the call site with [`ForkCall`], and adds Scope
/// to the table it receives before calling the child function).
pub(super) struct ScopeCall {
    /// Scope's IR effect.
    effect: EffectId,
    /// Indices of the operations fork and deferred in `EffectDef::ops`.
    ops: [usize; 2],
    /// The evidence table at the `Fiber.scope` call.
    ev: Option<Local>,
    /// Whether the scope (and its children) may fail.
    fails: bool,
    /// The mark of the body call.
    body_call: Call,
    body: Local,
    scope: Local,
    handler: Local,
    g: Local,
}

impl ScopeCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [body]: [Expr; 1] = args.try_into().expect("Fiber.scope takes one argument");
        let ev = || match self.ev {
            Some(ev) => Expr::Var(ev),
            None => Expr::Lit(Lit::Unit),
        };
        // Children run as suspending functions (the runtime steps g along even if it does not
        // suspend).
        let child = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.g))),
                args: vec![ev()],
                fails: self.fails,
                suspends: true,
            }),
            suspends: true,
        };
        let fork = Expr::Lambda {
            params: vec![self.g],
            body: Box::new(Expr::Fiber {
                op: FiberOp::Fork,
                args: vec![Expr::Var(self.scope), child],
            }),
            suspends: false,
        };
        let deferred = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Fiber {
                op: FiberOp::Deferred,
                args: vec![Expr::Var(self.scope)],
            }),
            suspends: false,
        };
        let mut ops = vec![Expr::Lit(Lit::Unit), Expr::Lit(Lit::Unit)];
        ops[self.ops[0]] = fork;
        ops[self.ops[1]] = deferred;
        let run = Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(self.body))),
            args: vec![Expr::Provide {
                base: self.ev.map(|ev| Box::new(Expr::Var(ev))),
                bindings: vec![(self.effect, Expr::Var(self.handler))],
            }],
            fails: self.body_call.fails,
            suspends: self.body_call.suspends,
        };
        let inner = Expr::Lambda {
            params: vec![self.scope],
            body: Box::new(Expr::Let {
                var: self.handler,
                value: Box::new(Expr::Handler {
                    name: "Scope".to_owned(),
                    effect: self.effect,
                    ops,
                }),
                body: Box::new(run),
            }),
            suspends: true,
        };
        Expr::Let {
            var: self.body,
            value: Box::new(body),
            body: Box::new(Expr::Fiber {
                op: FiberOp::Scope { fails: self.fails },
                args: vec![inner],
            }),
        }
    }
}

/// What is needed to lower `s.fork(f)`.
///
/// ```text
/// let c = s
/// let x = f
/// c.fork(fn(ev) { x(ev + { Scope: c }) })
/// ```
///
/// The wrapped function is called by fork's implementation in Scope's handler ([`ScopeCall`]) with
/// the table at the `Fiber.scope` call. The child's Scope is the capability value `c` that forked
/// it, so the child can fork siblings (spec 11.2). If `f`'s R needs no table, none is passed.
pub(super) struct ForkCall {
    /// The capability value expression.
    pub(super) handler: Expr,
    effect: EffectId,
    op: usize,
    /// The mark of calls to the child function `f`.
    f_call: Call,
    /// Whether `f` takes an evidence table.
    f_takes_ev: bool,
    cap: Local,
    f: Local,
    ev: Local,
}

impl ForkCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("fork takes one argument");
        let child_args = if self.f_takes_ev {
            vec![Expr::Provide {
                base: Some(Box::new(Expr::Var(self.ev))),
                bindings: vec![(self.effect, Expr::Var(self.cap))],
            }]
        } else {
            Vec::new()
        };
        let g = Expr::Lambda {
            params: vec![self.ev],
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
                args: child_args,
                fails: self.f_call.fails,
                suspends: self.f_call.suspends,
            }),
            suspends: self.f_call.suspends,
        };
        let fork = Expr::Call {
            callee: Callee::Op {
                handler: Box::new(Expr::Var(self.cap)),
                effect: self.effect,
                op: self.op,
            },
            args: vec![g],
            fails: false,
            suspends: false,
        };
        let lets = [(self.cap, self.handler), (self.f, f)];
        lets.into_iter()
            .rev()
            .fold(fork, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

/// What is needed to lower `Fiber.attempt(f)`. If `f`'s R needs a table, `f` is wrapped in a
/// parameterless function that calls it with the current table (the runtime calls `f` with no
/// arguments).
pub(super) struct AttemptCall {
    exit: EnumId,
    f_call: Call,
    /// The current evidence table to pass to `f`; `None` if it needs none.
    ev: Option<Expr>,
    f: Local,
}

impl AttemptCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("Fiber.attempt takes one argument");
        let op = FiberOp::Attempt {
            exit: self.exit,
            suspends: self.f_call.suspends,
        };
        let Some(ev) = self.ev else {
            return Expr::Fiber { op, args: vec![f] };
        };
        let wrapped = Expr::Lambda {
            params: Vec::new(),
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
                args: vec![ev],
                fails: self.f_call.fails,
                suspends: self.f_call.suspends,
            }),
            suspends: self.f_call.suspends,
        };
        Expr::Let {
            var: self.f,
            value: Box::new(f),
            body: Box::new(Expr::Fiber {
                op,
                args: vec![wrapped],
            }),
        }
    }
}

impl Lowerer<'_> {
    /// Whether it is a builtin effect (`@intrinsic effect`; only Scope today).
    pub(super) fn is_intrinsic_effect(&self, effect: DefId) -> bool {
        matches!(self.hir.item(effect), Some(Item::Effect(e)) if e.is_intrinsic)
    }

    /// If the `@intrinsic` fn `def` is a chapter 11 function, its callee. `callee` is the
    /// referencing expression.
    pub(super) fn fiber_target(
        &mut self,
        def: DefId,
        callee: ExprId,
        call: Call,
        span: Span,
    ) -> Option<Result<Target, Expr>> {
        let data = &self.hir.defs[def];
        let module = match &data.module {
            DefModule::Source(m) => self.modules[*m].name.to_string(),
            _ => return None,
        };
        let which = FiberFn::lookup(&module, &data.name)?;
        let Ty::Fn(fn_ty) = self.expr_ty(callee).clone() else {
            return Some(Err(
                self.unsupported(&format!("{module}.{}", data.name), span)
            ));
        };
        let simple = |op| Some(Ok(Target::Fiber(op)));
        match which {
            FiberFn::Scope => Some(
                self.scope_call(&fn_ty, call)
                    .map(|s| Target::Scope(Box::new(s)))
                    .ok_or_else(|| self.unsupported("Fiber.scope", span)),
            ),
            FiberFn::Join => simple(FiberOp::Join),
            FiberFn::Interrupt => simple(FiberOp::Interrupt),
            FiberFn::Complete => simple(FiberOp::Complete),
            FiberFn::DeferredAwait => simple(FiberOp::DeferredAwait),
            FiberFn::Poll => simple(FiberOp::Poll),
            FiberFn::CauseToString => simple(FiberOp::CauseToString),
            FiberFn::CauseRaise => simple(FiberOp::CauseRaise),
            FiberFn::Await => match self.ret_enum(&fn_ty) {
                Some(exit) => simple(FiberOp::Await { exit }),
                None => Some(Err(self.unsupported("Fiber.await", span))),
            },
            FiberFn::CauseKind => match self.ret_enum(&fn_ty) {
                Some(kind) => simple(FiberOp::CauseKind { kind }),
                None => Some(Err(self.unsupported("Cause.kind", span))),
            },
            FiberFn::Attempt => {
                let (Some(exit), Some(f_ty)) = (self.ret_enum(&fn_ty), fn_ty.params.first()) else {
                    return Some(Err(self.unsupported("Fiber.attempt", span)));
                };
                let f_call = Call {
                    fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
                    suspends: self.ty_suspends(f_ty),
                };
                let ev = self.ty_takes_ev(f_ty).then(|| self.ev_arg());
                let f_ty = self.ty(f_ty);
                Some(Ok(Target::Attempt(Box::new(AttemptCall {
                    exit,
                    f_call,
                    ev,
                    f: self.module.local("f", f_ty),
                }))))
            }
        }
    }

    /// The enum of the return type (Exit, CauseKind).
    fn ret_enum(&self, fn_ty: &FnTy) -> Option<EnumId> {
        match &fn_ty.ret {
            Ty::Named(con, _) => self.enums.get(self.types.ty_table.def(*con)).copied(),
            _ => None,
        }
    }

    /// `fn_ty` is the instantiated type of `Fiber.scope`.
    fn scope_call(&mut self, fn_ty: &FnTy, call: Call) -> Option<ScopeCall> {
        let Some(Ty::Fn(body)) = fn_ty.params.first() else {
            return None;
        };
        let tag = body
            .uses
            .tags()
            .find(|t| self.is_intrinsic_effect(t.def))?
            .clone();
        let (effect, ops) = self.scope_effect(&tag)?;
        let body_ty = self.ty(&fn_ty.params[0]);
        // The child function as wrapped by `ForkCall`: it takes the table at the `Fiber.scope`
        // call and returns the child's value, whose type each fork chooses.
        let g_ty = self.module.func(FnType {
            params: vec![TypeId::EVIDENCE],
            ret: TypeId::ANY,
            fails: call.fails,
            suspends: true,
            ev: false,
        });
        let handler_ty = self.module.handler(effect);
        Some(ScopeCall {
            effect,
            ops,
            ev: self.ev,
            fails: call.fails,
            body_call: Call {
                fails: !body.fails.is_empty(),
                suspends: self.row_suspends(&body.uses),
            },
            body: self.module.local("body", body_ty),
            scope: self.module.local("s", TypeId::FIBER_SCOPE),
            handler: self.module.local("scope", handler_ty),
            g: self.module.local("g", g_ty),
        })
    }

    /// Scope's IR effect for the tag, and the indices of the operations fork and deferred.
    fn scope_effect(&mut self, tag: &Tag) -> Option<(EffectId, [usize; 2])> {
        let effect = self.effect_id(&self.key_here(tag));
        let index = |name: &str| {
            self.module.effects[effect]
                .ops
                .iter()
                .position(|o| o.name == name)
        };
        Some((effect, [index("fork")?, index("deferred")?]))
    }

    /// Prepares lowering a fork call on the capability value `handler` (whose type's Scope tag is
    /// `tag`). `op_ty` is the instantiated type of fork.
    pub(super) fn fork_call(&mut self, handler: Expr, tag: &Tag, op_ty: &Ty) -> Option<ForkCall> {
        let (effect, [fork, _]) = self.scope_effect(tag)?;
        let Ty::Fn(op_ty) = op_ty else {
            return None;
        };
        let f_ty = op_ty.params.first()?;
        let f_ir_ty = self.ty(f_ty);
        let cap_ty = self.module.handler(effect);
        Some(ForkCall {
            handler,
            effect,
            op: fork,
            f_call: Call {
                fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
                suspends: self.ty_suspends(f_ty),
            },
            f_takes_ev: self.ty_takes_ev(f_ty),
            cap: self.module.local("scope", cap_ty),
            f: self.module.local("f", f_ir_ty),
            ev: self.ev_local(),
        })
    }

    /// Whether operation `op` on a capability value is Scope's fork.
    pub(super) fn is_fork_op(&self, effect: DefId, op: DefId) -> bool {
        self.is_intrinsic_effect(effect) && self.hir.defs[op].name == "fork"
    }
}
