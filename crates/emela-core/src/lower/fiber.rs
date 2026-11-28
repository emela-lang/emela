//! Lowering concurrency (spec chapter 11).
//!
//! `@intrinsic` functions of `Std.Task`, `Std.Exit` and `Std.Cause` are lowered to `Expr::Fiber`
//! in the Core IR ([`FiberFn`]). An item of a with that names the built-in handler `Task.Group` or
//! `Task.Background` makes a run-time scope, and its capability is a handler value that lowering
//! builds there ([`TaskItem`]). Its operation start becomes an ordinary operation call that wraps
//! the task function at the call site ([`StartCall`]). The built-in handlers Deferred, Queue and
//! Semaphore are lowered in `lower/builtin_handler.rs`.
//!
//! The rest of the with (the items after the group and the body) runs inside the run-time scope:
//!
//! ```text
//! with a = A, tasks = Task.Group, b = B { body }
//! ⇒ with a = A {
//!     fiber#scope(fn(s) {
//!       with tasks = TaskGroup { start: fn(g) { fiber#fork(s, fn() { g(ev) }) } }, b = B { body }
//!     })
//!   }
//! ```
//!
//! So the release of the group (waiting for its tasks) runs after those of the items after it and
//! before those of the items before it (`[effect.with]`). A task uses the handlers in effect
//! outside the with, and those of the items before the group (`[task.group]`): `ev` is that table,
//! captured by the handler value, and start's implementation calls the task with it. The group
//! itself is the capability value that started the task (added when the call site wraps the task
//! function), so a task can start siblings.
//!
//! Why a handler value, rather than passing the run-time scope to `FiberOp::Fork` directly: the
//! task's table must be tied to the capability value, since it is the table at the with, not where
//! `tasks.start` is. The lambdas of a handler value just capture it.
//!
//! A background (`[task.background]`) is the same run-time scope in its background mode
//! (`FiberOp::Scope::background`): its release waits without interrupting, and a failure of a task
//! does not stop the others. The failure goes to `on_failure`, which is called in the task: the
//! task function runs inside `Exit.attempt`, and a `Failure` is passed to `on_failure`. An
//! interruption is not caught by attempt, so `on_failure` is not called for it.
//!
//! The TaskGroup key is always `Std.Task.TaskGroup[_, _]`: its type arguments are sets, which keys
//! leave out. In nested groups, `use TaskGroup` finds the innermost handler, and a capability value
//! taken out in an outer group starts into that outer group. Since a task's table holds the
//! capability value that started it, the tasks of an inner group start their siblings into it.

use emela_resolve::hir::{DefId, DefModule, ExprId, Span};
use emela_types::{FnTy, Tag, Ty};

use super::{Call, Lowerer, Target};
use crate::ir::{
    Arm, Callee, CtorRef, EXIT_FAILURE, EffectId, EnumId, Expr, FiberOp, FnType, Lit, Local, Pat,
    Type, TypeId, WithBind,
};

/// `@intrinsic` functions that lowering maps to `Expr::Fiber`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FiberFn {
    Join,
    Attempt,
    CauseKind,
    CauseToString,
    CauseRaise,
}

impl FiberFn {
    /// Looks up from a bundled declaration (module name, function name).
    pub(super) fn lookup(module: &str, name: &str) -> Option<FiberFn> {
        Some(match (module, name) {
            ("Std.Task", "join") => FiberFn::Join,
            ("Std.Exit", "attempt") => FiberFn::Attempt,
            ("Std.Cause", "kind") => FiberFn::CauseKind,
            ("Std.Cause", "to_string") => FiberFn::CauseToString,
            ("Std.Cause", "raise") => FiberFn::CauseRaise,
            _ => return None,
        })
    }
}

/// An item of a with that names `Task.Group` or `Task.Background`, found while lowering the with.
/// Its bind is a placeholder until [`Lowerer::wrap_task_items`] puts the rest of the with inside
/// the run-time scope.
pub(super) struct TaskItem {
    /// The index of its placeholder in the binds of the with.
    pub(super) at: usize,
    /// The local its handler value is bound to.
    pub(super) local: Local,
    /// The IR effect it provides (TaskGroup or Spawn).
    pub(super) effect: EffectId,
    /// A background, and its `on_failure` with how to call it.
    pub(super) background: Option<OnFailure>,
    /// The table the tasks get: the one outside the with, with the items before this one.
    pub(super) ev: Expr,
    /// Whether the tasks (and the rest of the with) may fail.
    pub(super) fails: bool,
}

/// The `on_failure` of a background, bound to a local before the scope.
pub(super) struct OnFailure {
    pub(super) value: Expr,
    /// Its type, and whether it takes the table of the tasks.
    pub(super) ty: TypeId,
    pub(super) takes_ev: bool,
    pub(super) suspends: bool,
}

/// What is needed to lower `tasks.start(f)`.
///
/// ```text
/// let c = tasks
/// let x = f
/// c.start(fn(ev) { x(ev + { TaskGroup: c }) })
/// ```
///
/// The wrapped function is called by start's implementation in the handler value of the item
/// ([`TaskItem`]) with the table at the with. The task's TaskGroup is the capability value `c`
/// that started it, so the task can start siblings (`[task.group]`). If `f`'s R needs no table,
/// none is passed.
pub(super) struct StartCall {
    /// The capability value expression.
    pub(super) handler: Expr,
    effect: EffectId,
    op: usize,
    /// The type argument of start (`A`, the task's value type).
    ty_args: Vec<TypeId>,
    /// The mark of calls to the task function `f`.
    f_call: Call,
    /// Whether `f` takes an evidence table.
    f_takes_ev: bool,
    /// The type of the wrapped task function (`fn(ev) -> A`).
    g_ty: TypeId,
    cap: Local,
    f: Local,
    ev: Local,
}

impl StartCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("start takes one argument");
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
            ty: self.g_ty,
            body: Box::new(Expr::Call {
                callee: Callee::Indirect(Box::new(Expr::Var(self.f))),
                args: child_args,
                fails: self.f_call.fails,
                suspends: self.f_call.suspends,
            }),
            suspends: self.f_call.suspends,
        };
        let start = Expr::Call {
            callee: Callee::Op {
                handler: Box::new(Expr::Var(self.cap)),
                effect: self.effect,
                op: self.op,
                ty_args: self.ty_args,
            },
            args: vec![g],
            fails: false,
            suspends: false,
        };
        let lets = [(self.cap, self.handler), (self.f, f)];
        lets.into_iter()
            .rev()
            .fold(start, |body, (var, value)| Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(body),
            })
    }
}

/// What is needed to lower `Exit.attempt(f)`. If `f`'s R needs a table, `f` is wrapped in a
/// parameterless function that calls it with the current table (the runtime calls `f` with no
/// arguments).
pub(super) struct AttemptCall {
    exit: EnumId,
    f_call: Call,
    /// The current evidence table to pass to `f`; `None` if it needs none.
    ev: Option<Expr>,
    /// The type of `f` wrapped in a parameterless function.
    wrapped_ty: TypeId,
    f: Local,
}

impl AttemptCall {
    pub(super) fn apply(self, args: Vec<Expr>) -> Expr {
        let [f]: [Expr; 1] = args.try_into().expect("Exit.attempt takes one argument");
        let op = FiberOp::Attempt {
            exit: self.exit,
            suspends: self.f_call.suspends,
        };
        let Some(ev) = self.ev else {
            return Expr::Fiber { op, args: vec![f] };
        };
        let wrapped = Expr::Lambda {
            params: Vec::new(),
            ty: self.wrapped_ty,
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
            FiberFn::Join => simple(FiberOp::Join),
            FiberFn::CauseToString => simple(FiberOp::CauseToString),
            FiberFn::CauseRaise => simple(FiberOp::CauseRaise { fails: call.fails }),
            FiberFn::CauseKind => match self.ret_enum(&fn_ty) {
                Some(kind) => simple(FiberOp::CauseKind { kind }),
                None => Some(Err(self.unsupported("Cause.kind", span))),
            },
            FiberFn::Attempt => {
                let (Some(exit), Some(f_ty)) = (self.ret_enum(&fn_ty), fn_ty.params.first()) else {
                    return Some(Err(self.unsupported("Exit.attempt", span)));
                };
                let f_call = Call {
                    fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
                    suspends: self.ty_suspends(f_ty),
                };
                let ev = self.ty_takes_ev(f_ty).then(|| self.ev_arg());
                let wrapped_ty = self.wrapped_ty(f_ty, Vec::new(), f_call);
                let f_ty = self.ty(f_ty);
                Some(Ok(Target::Attempt(Box::new(AttemptCall {
                    exit,
                    f_call,
                    ev,
                    wrapped_ty,
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

    /// Whether the built-in handler that provides `effect` makes a run-time scope: the effect has
    /// the operation start (TaskGroup, Spawn). `Some(true)` for a background: start returns `()`
    /// (Spawn, `[task.background]`).
    pub(super) fn task_handler(&self, effect: EffectId) -> Option<bool> {
        let def = self.module.effects[effect]
            .ops
            .iter()
            .find(|o| o.name == "start")?;
        Some(def.ret == TypeId::UNIT)
    }

    /// The index of the operation start of the IR effect `effect`.
    fn start_op(&self, effect: EffectId) -> Option<usize> {
        self.module.effects[effect]
            .ops
            .iter()
            .position(|o| o.name == "start")
    }

    /// `with` binds and body, with the rest of the with after each of `items` put inside its
    /// run-time scope (see the module documentation).
    pub(super) fn wrap_task_items(
        &mut self,
        mut binds: Vec<WithBind>,
        body: Expr,
        ret: TypeId,
        mut items: Vec<TaskItem>,
    ) -> Expr {
        if items.is_empty() {
            return Expr::With {
                binds,
                body: Box::new(body),
            };
        }
        let item = items.remove(0);
        let after = binds.split_off(item.at + 1);
        binds.pop();
        for other in &mut items {
            other.at -= item.at;
        }
        let scope = self.module.local("s", TypeId::FIBER_SCOPE);
        let on_failure = item
            .background
            .as_ref()
            .map(|f| (self.module.local("on_failure", f.ty), f));
        let handler =
            self.task_handler_value(&item, scope, on_failure.as_ref().map(|(l, f)| (*l, *f)));
        let mut inner = vec![WithBind::new(item.local, handler)];
        inner.extend(after);
        let inner = self.wrap_task_items(inner, body, ret, items);
        let inner_ty = self.module.func(FnType {
            params: vec![TypeId::FIBER_SCOPE],
            ret,
            fails: item.fails,
            suspends: true,
            ev: false,
            uses: false,
            sets: None,
        });
        let run = Expr::Fiber {
            op: FiberOp::Scope {
                fails: item.fails,
                background: item.background.is_some(),
            },
            args: vec![Expr::Lambda {
                params: vec![scope],
                ty: inner_ty,
                body: Box::new(inner),
                suspends: true,
            }],
        };
        let run = match on_failure {
            Some((l, f)) => Expr::Let {
                var: l,
                value: Box::new(f.value.clone()),
                body: Box::new(run),
            },
            None => run,
        };
        if binds.is_empty() {
            return run;
        }
        Expr::With {
            binds,
            body: Box::new(run),
        }
    }

    /// The handler value of a group or a background on the run-time scope `scope`:
    ///
    /// ```text
    /// TaskGroup { start: fn(g) { fiber#fork(s, fn() { g(ev) }) } }
    /// Spawn { start: fn(g) { fiber#fork(s, fn() { attempt(fn() { g(ev) }) match {
    ///   Failure(c) -> on_failure(c), _ -> () } }); () } }
    /// ```
    fn task_handler_value(
        &mut self,
        item: &TaskItem,
        scope: Local,
        on_failure: Option<(Local, &OnFailure)>,
    ) -> Expr {
        let effect = item.effect;
        let ops_len = self.module.effects[effect].ops.len();
        let start = self
            .start_op(effect)
            .expect("a task handler's effect has start");
        let background = on_failure.is_some();
        let ret = if background {
            TypeId::UNIT
        } else {
            TypeId::ANY
        };
        // The task function as wrapped by `StartCall`: it takes the table at the with and returns
        // the task's value, whose type each start chooses.
        let g_ty = self.module.func(FnType {
            params: vec![TypeId::EVIDENCE],
            ret,
            fails: item.fails,
            suspends: true,
            ev: false,
            uses: false,
            sets: None,
        });
        let g = self.module.local("g", g_ty);
        let child_ty = self.module.func(FnType {
            params: Vec::new(),
            ret,
            fails: item.fails && !background,
            suspends: true,
            ev: false,
            uses: false,
            sets: None,
        });
        let call_g = Expr::Call {
            callee: Callee::Indirect(Box::new(Expr::Var(g))),
            args: vec![item.ev.clone()],
            fails: item.fails,
            suspends: true,
        };
        // Tasks run as suspending functions (the runtime steps them along even if they do not
        // suspend).
        let task = match on_failure {
            None => call_g,
            Some((f, info)) => {
                let attempted_ty = self.module.func(FnType {
                    params: Vec::new(),
                    ret: TypeId::UNIT,
                    fails: item.fails,
                    suspends: true,
                    ev: false,
                    uses: false,
                    sets: None,
                });
                let exit = self.exit_enum().expect("Std.Exit is bundled");
                let exit_ty = self.module.enum_type(exit, vec![TypeId::UNIT; 2]);
                let x = self.module.local("exit", exit_ty);
                let cause_ty = match self.module.ty(info.ty).clone() {
                    Type::Fn(f) => f.params.first().copied().unwrap_or(TypeId::ANY),
                    _ => TypeId::ANY,
                };
                let c = self.module.local("cause", cause_ty);
                let mut args = vec![Expr::Var(c)];
                if info.takes_ev {
                    args.push(item.ev.clone());
                }
                let report = Expr::Call {
                    callee: Callee::Indirect(Box::new(Expr::Var(f))),
                    args,
                    fails: false,
                    suspends: info.suspends,
                };
                Expr::Let {
                    var: x,
                    value: Box::new(Expr::Fiber {
                        op: FiberOp::Attempt {
                            exit,
                            suspends: true,
                        },
                        args: vec![Expr::Lambda {
                            params: Vec::new(),
                            ty: attempted_ty,
                            body: Box::new(call_g),
                            suspends: true,
                        }],
                    }),
                    body: Box::new(Expr::Match {
                        scrutinee: Box::new(Expr::Var(x)),
                        arms: vec![
                            Arm {
                                pat: Pat::Ctor {
                                    ctor: CtorRef {
                                        enum_id: exit,
                                        variant: EXIT_FAILURE,
                                    },
                                    fields: vec![Pat::Bind(c)],
                                },
                                guard: None,
                                body: report,
                            },
                            Arm {
                                pat: Pat::Wild,
                                guard: None,
                                body: Expr::Lit(Lit::Unit),
                            },
                        ],
                    }),
                }
            }
        };
        let child = Expr::Lambda {
            params: Vec::new(),
            ty: child_ty,
            body: Box::new(task),
            suspends: true,
        };
        let fork = Expr::Fiber {
            op: FiberOp::Fork,
            args: vec![Expr::Var(scope), child],
        };
        // start of Spawn returns `()`, not the task.
        let fork = if background {
            Expr::Let {
                var: self.module.local("_", TypeId::ANY),
                value: Box::new(fork),
                body: Box::new(Expr::Lit(Lit::Unit)),
            }
        } else {
            fork
        };
        let start_ty = self.module.fn_type(
            vec![g_ty],
            if background {
                TypeId::UNIT
            } else {
                TypeId::ANY
            },
        );
        let mut ops = vec![Expr::Lit(Lit::Unit); ops_len];
        ops[start] = Expr::Lambda {
            params: vec![g],
            ty: start_ty,
            body: Box::new(fork),
            suspends: false,
        };
        Expr::Handler {
            name: if background { "Background" } else { "Group" }.to_owned(),
            effect,
            ops,
        }
    }

    /// The enum `Exit` of `Std.Exit`.
    fn exit_enum(&self) -> Option<EnumId> {
        self.hir.defs.iter().find_map(|(def, data)| {
            let here = matches!(&data.module, DefModule::Source(m)
                if self.modules[*m].name.to_string() == "Std.Exit");
            if here && data.name == "Exit" {
                self.enums.get(def).copied()
            } else {
                None
            }
        })
    }

    /// Prepares lowering a start call on the capability value `handler` (whose type's tag is
    /// `tag`, TaskGroup or Spawn). `op_ty` is the instantiated type of start.
    pub(super) fn start_call(
        &mut self,
        handler: Expr,
        tag: &Tag,
        op_ty: &Ty,
        expr: ExprId,
    ) -> Option<StartCall> {
        let effect = self.effect_id(&self.key_here(tag));
        let start = self.start_op(effect)?;
        let Ty::Fn(op_ty) = op_ty else {
            return None;
        };
        let f_ty = op_ty.params.first()?;
        let f_ir_ty = self.ty(f_ty);
        let cap_ty = self.module.handler(effect);
        let f_call = Call {
            fails: matches!(f_ty, Ty::Fn(f) if !f.fails.is_empty()),
            suspends: self.ty_suspends(f_ty),
        };
        let g_ty = self.wrapped_ty(f_ty, vec![TypeId::EVIDENCE], f_call);
        let ty_args = self.op_ty_args(effect, start, expr);
        Some(StartCall {
            handler,
            effect,
            op: start,
            ty_args,
            f_call,
            f_takes_ev: self.ty_takes_ev(f_ty),
            g_ty,
            cap: self.module.local("tasks", cap_ty),
            f: self.module.local("f", f_ir_ty),
            ev: self.ev_local(),
        })
    }

    /// The type of a lambda with parameters `params` that calls a value of type `f_ty` with the
    /// mark `call` and returns its value.
    fn wrapped_ty(&mut self, f_ty: &Ty, params: Vec<TypeId>, call: Call) -> TypeId {
        let ret = match f_ty {
            Ty::Fn(f) => self.ty(&f.ret),
            _ => TypeId::ANY,
        };
        self.module.func(FnType {
            params,
            ret,
            fails: call.fails,
            suspends: call.suspends,
            ev: false,
            uses: false,
            sets: None,
        })
    }

    /// The type of start's parameter in the IR (`OpDef::params`): the task function `f` as wrapped
    /// by [`StartCall`], which takes the evidence table as an ordinary parameter and returns `f`'s
    /// value.
    pub(super) fn wrapped_child(&mut self, f: TypeId) -> TypeId {
        match self.module.ty(f).clone() {
            Type::Fn(f) => self.module.func(FnType {
                params: vec![TypeId::EVIDENCE],
                ev: false,
                sets: None,
                ..f
            }),
            _ => f,
        }
    }

    /// Whether operation `op` on a capability value is start of TaskGroup or Spawn.
    pub(super) fn is_start_op(&self, effect: DefId, op: DefId) -> bool {
        self.is_intrinsic_effect(effect) && self.hir.defs[op].name == "start"
    }
}
