//! Built-in handlers: `Deferred`, `Queue` and `Semaphore` (`[task.deferred]`, `[task.queue]`,
//! `[task.semaphore]`).
//!
//! A built-in handler has no body: the runtime holds what it makes. An item of a with that names
//! one makes the runtime object with a concurrency operation of the Core IR, and binds a handler
//! value whose operations are lambdas over that object:
//!
//! ```text
//! with d = Deferred { … }
//! ⇒ let c = fiber#deferred()
//!   let d = Deferred {
//!     complete: fn(v) { fiber#complete(c, v) },
//!     await: fn(resume) { resume(fiber#deferred_await(c)) },
//!     poll: fn() { fiber#poll(c) },
//!   }
//! ```
//!
//! Their releases do nothing: a task that can use the capability is in a group whose with is
//! inside the with of the item, so none is left when it ends. The runtime objects are typed as
//! opaque external types (`EnumDef::external`), one per kind, so that the values that pass
//! through them keep their types.
//!
//! The effect gets a hidden operation `$cell` that returns the runtime object. A call of an
//! operation that waits (`await`, `offer`, `take`, opening `Permits`) takes the object with it and
//! waits in place in the calling task ([`Lowerer::builtin_op_call`]), so a task interrupted while
//! it waits loses nothing it was given. The handler value still implements those operations, by
//! waiting before it calls resume, for completeness. `offer` and `take` report a closed queue as a value,
//! which the lambda turns into the failure `QueueClosed`, so the runtimes never build an error
//! value. The handler members `Permits` and `TryPermits` of Semaphore open a `Permit` handler
//! value paired with the function that gives the permits back (`lower/member.rs`); a task
//! interrupted while it waits in `fiber#acquire` has taken no permit, and the member's init does
//! not complete, so no release runs (`[dyn.with]`).

use emela_resolve::hir::{DefId, DefModule, Item};

use super::Lowerer;
use crate::ir::{
    Arm, Callee, CtorRef, EffectId, EnumDef, Expr, FiberOp, FnType, Lit, Local, OpDef, Pat, Type,
    TypeId,
};

/// The name of the operation that returns the runtime object (`Lowerer::cell_op`). It cannot
/// be the name of an operation written in source.
const CELL_OP: &str = "$cell";

/// The built-in handlers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Builtin {
    Deferred,
    Queue,
    Semaphore,
}

impl Lowerer<'_> {
    /// How `dbg` shows a capability of `effect` (`EffectDef::display`): the built-in effects of
    /// the built-in handlers Deferred, Queue and Semaphore.
    pub(super) fn capability_display(&self, effect: DefId) -> Option<&'static str> {
        if !self.is_intrinsic_effect(effect) {
            return None;
        }
        let data = &self.hir.defs[effect];
        let DefModule::Source(m) = data.module else {
            return None;
        };
        match (
            self.modules[m].name.to_string().as_str(),
            data.name.as_str(),
        ) {
            ("Std.Deferred", "Deferred") => Some("<deferred>"),
            ("Std.Queue", "Queue") => Some("<queue>"),
            ("Std.Semaphore", "Semaphore") => Some("<semaphore>"),
            _ => None,
        }
    }

    /// Whether `handler` is a built-in handler (`@intrinsic`, `[kw.annotation]`).
    pub(super) fn is_builtin_handler(&self, handler: DefId) -> bool {
        matches!(self.hir.item(handler), Some(Item::Handler(h)) if h.is_intrinsic)
    }

    /// The built-in handler whose capabilities are of the effect `effect`.
    fn builtin_of(&self, effect: EffectId) -> Option<Builtin> {
        match self.module.effects[effect].display? {
            "<deferred>" => Some(Builtin::Deferred),
            "<queue>" => Some(Builtin::Queue),
            "<semaphore>" => Some(Builtin::Semaphore),
            _ => None,
        }
    }

    /// The operation `$cell` that the IR adds after the operations and handler members of the
    /// effect of a built-in handler: it returns the runtime object, so that a call of an
    /// operation that waits can wait in place (`Self::builtin_op_call`). `None` for other effects.
    pub(super) fn cell_op(
        &mut self,
        display: Option<&'static str>,
        args: &[TypeId],
    ) -> Option<OpDef> {
        let name = match display? {
            "<deferred>" => "Std.Deferred.Deferred",
            "<queue>" => "Std.Queue.Queue",
            "<semaphore>" => "Std.Semaphore.Semaphore",
            _ => return None,
        };
        let ty = self.runtime_cell(name, args.to_vec());
        Some(OpDef::new(CELL_OP, Vec::new(), ty))
    }

    /// The handler value of the built-in handler `handler`, which provides `effect`, made with the
    /// arguments of its init (`args`). The value binds the runtime object it makes. `None` for a
    /// built-in handler this lowering does not know.
    pub(super) fn builtin_handler(
        &mut self,
        handler: DefId,
        effect: EffectId,
        args: Vec<Expr>,
    ) -> Option<Expr> {
        let which = self.builtin_of(effect)?;
        let name = self.hir.defs[handler].name.to_string();
        let ops = self.module.effects[effect].ops.clone();
        let cell_ty = ops.iter().find(|o| o.name == CELL_OP)?.ret;
        let make = match which {
            Builtin::Deferred => FiberOp::Deferred,
            Builtin::Queue => FiberOp::Queue,
            Builtin::Semaphore => FiberOp::Semaphore,
        };
        let cell = self.module.local(&name.to_lowercase(), cell_ty);
        let mut lambdas = Vec::with_capacity(ops.len());
        for op in &ops {
            let params: Vec<Local> = op
                .params
                .iter()
                .map(|&t| self.module.local("x", t))
                .collect();
            let value = self.builtin_op_value(which, op, cell, &params)?;
            lambdas.push(self.op_lambda(op.suspend, op.fails, params, op.ret, value));
        }
        let made = Expr::Fiber { op: make, args };
        Some(Expr::Let {
            var: cell,
            value: Box::new(made),
            body: Box::new(Expr::Handler {
                name,
                effect,
                ops: lambdas,
            }),
        })
    }

    /// A call of the operation `op` of `effect` on the capability `handler`, when it is a suspend
    /// fn or a suspend handler member of a built-in handler: it takes the runtime object with
    /// `$cell` and waits in place, rather than through the handler's implementation and resume.
    /// The task that waits is the caller, so an interruption reaches it only while it waits, and
    /// what it waited for (a value, permits) is not lost after (`[task.interrupt]`). `None` for
    /// every other operation, which is called as usual.
    pub(super) fn builtin_op_call(
        &mut self,
        handler: &Expr,
        effect: EffectId,
        op: usize,
        args: &[Expr],
    ) -> Option<Expr> {
        let which = self.builtin_of(effect)?;
        let def = self.module.effects[effect].ops[op].clone();
        if !def.suspend {
            return None;
        }
        let index = self.module.effects[effect]
            .ops
            .iter()
            .position(|o| o.name == CELL_OP)?;
        let cell_ty = self.module.effects[effect].ops[index].ret;
        let h_ty = self.module.handler(effect);
        let h = self.module.local("h", h_ty);
        let cell = self.module.local("cell", cell_ty);
        let params: Vec<Local> = def
            .params
            .iter()
            .map(|&t| self.module.local("x", t))
            .collect();
        let value = self.builtin_op_value(which, &def, cell, &params)?;
        let take_cell = Expr::Call {
            callee: Callee::Op {
                handler: Box::new(Expr::Var(h)),
                effect,
                op: index,
                ty_args: Vec::new(),
            },
            args: Vec::new(),
            fails: false,
            suspends: false,
        };
        let lets = std::iter::once((h, handler.clone()))
            .chain(params.iter().copied().zip(args.iter().cloned()))
            .chain([(cell, take_cell)])
            .collect::<Vec<_>>();
        Some(
            lets.into_iter()
                .rev()
                .fold(value, |body, (var, value)| Expr::Let {
                    var,
                    value: Box::new(value),
                    body: Box::new(body),
                }),
        )
    }

    /// `e` with its call of an operation that waits of a built-in handler made to wait in place
    /// ([`Self::builtin_op_call`]). The call is `e` itself or the body of the `let`s that bind its
    /// arguments in the order they are written.
    pub(super) fn builtin_direct(&mut self, e: Expr) -> Expr {
        match e {
            Expr::Call {
                callee:
                    Callee::Op {
                        ref handler,
                        effect,
                        op,
                        ..
                    },
                ref args,
                ..
            } => match self.builtin_op_call(handler, effect, op, args) {
                Some(direct) => direct,
                None => e,
            },
            Expr::Let { var, value, body } => Expr::Let {
                var,
                value,
                body: Box::new(self.builtin_direct(*body)),
            },
            e => e,
        }
    }

    /// The value of the operation `op` of a built-in handler whose runtime object is `cell`, with
    /// its parameters bound to `params` (for a suspend fn, what it resumes with).
    fn builtin_op_value(
        &mut self,
        which: Builtin,
        op: &OpDef,
        cell: Local,
        params: &[Local],
    ) -> Option<Expr> {
        let c = || Expr::Var(cell);
        let p = |i: usize| Expr::Var(params[i]);
        let fiber = |op: FiberOp, args: Vec<Expr>| Expr::Fiber { op, args };
        Some(match (which, op.name.as_str()) {
            (_, CELL_OP) => c(),
            (Builtin::Deferred, "complete") => fiber(FiberOp::Complete, vec![c(), p(0)]),
            (Builtin::Deferred, "await") => fiber(FiberOp::DeferredAwait, vec![c()]),
            (Builtin::Deferred, "poll") => fiber(FiberOp::Poll, vec![c()]),
            (Builtin::Queue, "offer") => {
                let closed = self.std_error("Std.Queue", "QueueClosed")?;
                Expr::If {
                    cond: Box::new(fiber(FiberOp::Offer, vec![c(), p(0)])),
                    then: Box::new(Expr::Lit(Lit::Unit)),
                    else_: Box::new(fail(closed)),
                }
            }
            (Builtin::Queue, "take") => {
                let closed = self.std_error("Std.Queue", "QueueClosed")?;
                let some = self.module.some_ctor();
                let x = self.module.local("x", op.ret);
                Expr::Match {
                    scrutinee: Box::new(fiber(FiberOp::Take, vec![c()])),
                    arms: vec![
                        Arm {
                            pat: Pat::Ctor {
                                ctor: some,
                                fields: vec![Pat::Bind(x)],
                            },
                            guard: None,
                            body: Expr::Var(x),
                        },
                        Arm {
                            pat: Pat::Wild,
                            guard: None,
                            body: fail(closed),
                        },
                    ],
                }
            }
            (Builtin::Queue, "try_offer") => fiber(FiberOp::TryOffer, vec![c(), p(0)]),
            (Builtin::Queue, "poll") => fiber(FiberOp::QueuePoll, vec![c()]),
            (Builtin::Queue, "size") => fiber(FiberOp::Size, vec![c()]),
            (Builtin::Queue, "close") => fiber(FiberOp::Close, vec![c()]),
            (Builtin::Semaphore, "available") => fiber(FiberOp::Available, vec![c()]),
            (Builtin::Semaphore, "Permits") => {
                let opened = self.permits(op.ret, cell, params[0], op.suspend)?;
                Expr::Let {
                    var: self.module.local("_", TypeId::UNIT),
                    value: Box::new(fiber(FiberOp::Acquire, vec![c(), p(0)])),
                    body: Box::new(opened),
                }
            }
            (Builtin::Semaphore, "TryPermits") => {
                let none = self.std_error("Std.Semaphore", "NoPermits")?;
                let opened = self.permits(op.ret, cell, params[0], op.suspend)?;
                Expr::If {
                    cond: Box::new(fiber(FiberOp::TryAcquire, vec![c(), p(0)])),
                    then: Box::new(opened),
                    else_: Box::new(fail(none)),
                }
            }
            _ => return None,
        })
    }

    /// The lambda implementing an operation whose value is `value`. A suspend fn operation takes
    /// resume last and resumes with the value (`Expr::Handler`); it suspends exactly when the
    /// value does.
    fn op_lambda(
        &mut self,
        suspend: bool,
        fails: bool,
        mut params: Vec<Local>,
        ret: TypeId,
        value: Expr,
    ) -> Expr {
        let suspends = super::suspend::has_suspending_call(&value);
        let (body, ret) = if suspend {
            let resume_ty = self.module.fn_type(vec![ret], TypeId::UNIT);
            let resume = self.module.local("resume", resume_ty);
            params.push(resume);
            let body = Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(resume))),
                args: vec![value],
                fails: false,
                suspends: false,
            };
            (body, TypeId::UNIT)
        } else {
            (value, ret)
        };
        let ty = self.module.func(FnType {
            params: params.iter().map(|&l| self.module.locals[l].ty).collect(),
            ret,
            fails,
            suspends,
            ev: false,
            sets: None,
        });
        Expr::Lambda {
            params,
            ty,
            body: Box::new(body),
            suspends,
        }
    }

    /// The pair that opening `Permits` or `TryPermits` returns, once `n` permits of `cell` are
    /// taken: a `Permit` handler value whose `count` is `n`, and the function that gives them
    /// back. `pair` is the type of the pair (`OpDef::ret` of the member).
    fn permits(&mut self, pair: TypeId, cell: Local, n: Local, suspend: bool) -> Option<Expr> {
        let Type::Tuple(parts) = self.module.ty(pair).clone() else {
            return None;
        };
        let [permit_ty, release_ty] = parts[..] else {
            return None;
        };
        let Type::Handler(permit) = self.module.ty(permit_ty).clone() else {
            return None;
        };
        let count_ty = self.module.fn_type(Vec::new(), TypeId::INT);
        let count = Expr::Lambda {
            params: Vec::new(),
            ty: count_ty,
            body: Box::new(Expr::Var(n)),
            suspends: false,
        };
        let value = Expr::Handler {
            name: "Permit".to_owned(),
            effect: permit,
            ops: vec![count],
        };
        let release = Expr::Lambda {
            params: Vec::new(),
            ty: release_ty,
            body: Box::new(Expr::Fiber {
                op: FiberOp::Release,
                args: vec![Expr::Var(cell), Expr::Var(n)],
            }),
            // Marked as its type (`release_fn_ty`): giving permits back does not suspend, but
            // the release of a suspend handler member is called as one that may.
            suspends: suspend,
        };
        Some(Expr::Tuple(vec![value, release]))
    }

    /// The opaque type of the runtime object of a built-in handler, named as the type the
    /// runtimes show (`EnumDef::runtime_display`). Added once per module.
    fn runtime_cell(&mut self, name: &str, args: Vec<TypeId>) -> TypeId {
        let found = self
            .module
            .enums
            .iter()
            .find(|(_, e)| e.external && e.name == name)
            .map(|(id, _)| id);
        let id = match found {
            Some(id) => id,
            None => self.module.add_enum(EnumDef {
                name: name.to_owned(),
                params: args.len(),
                variants: Vec::new(),
                error: false,
                external: true,
            }),
        };
        self.module.enum_type(id, args)
    }

    /// The constructor of the error `name` of the bundled module `module`.
    fn std_error(&self, module: &str, name: &str) -> Option<CtorRef> {
        self.hir.defs.iter().find_map(|(def, data)| {
            let here = matches!(&data.module, DefModule::Source(m)
                if self.modules[*m].name.to_string() == module);
            if here && data.name == name {
                self.ctors.get(def).copied()
            } else {
                None
            }
        })
    }
}

/// `fail E` for an error without fields.
fn fail(ctor: CtorRef) -> Expr {
    Expr::Fail(Box::new(Expr::Ctor {
        ctor,
        ty_args: Vec::new(),
        args: Vec::new(),
    }))
}
