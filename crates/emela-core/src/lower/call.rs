//! Calls: the target a callee lowers to ([`super::Target`]) and the call built from it.

use emela_resolve::hir::{DefId, DefKind, DefModule, ExprId, ExprKind, Item, Res, Span};
use emela_types::{Tag, Ty};

use crate::intrinsics::Builtin;
use crate::ir::{Expr, FnId, FnType, Lit, Local, Type, TypeId};
use crate::named;

use super::{Call, Lowerer, StateRun, Target, has_bound};

impl<'a> Lowerer<'a> {
    /// `written` are the arguments in written order (for a pipe, the left side first). Each
    /// argument's position comes from the parameter order arranged by the type checker
    /// (`ProgramTypes::calls`).
    pub(super) fn call(&mut self, id: ExprId, callee: ExprId, written: Vec<ExprId>) -> Expr {
        let span = self.hir.exprs[id].span;
        let slots = &self.types.calls[id];
        let positions: Vec<usize> = written
            .iter()
            .map(|&arg| {
                slots
                    .iter()
                    .position(|s| *s == Some(arg))
                    .expect("the type checker placed each argument at a parameter position")
            })
            .collect();

        // Resolve the callee before the arguments (evaluation is left to right, spec 14.1).
        let target = match self.callee(callee, span) {
            Ok(t) => t,
            Err(e) => return e,
        };
        let args: Vec<(usize, Expr, TypeId)> = positions
            .into_iter()
            .zip(written)
            .map(|(pos, arg)| {
                let ty = self.expr_ty(arg);
                let ty = self.ty(ty);
                (pos, self.expr(arg), ty)
            })
            .collect();
        let (target, binding) = match target {
            Target::Value(f, call, ev) if !matches!(f, Expr::Var(_) | Expr::Fn { .. }) => {
                // Reordering arguments and binding them with `let` would evaluate them before the
                // callee, so bind the callee first.
                let ty = self.expr_ty(callee);
                let ty = self.ty(ty);
                let l = self.module.local("f", ty);
                (Target::Value(Expr::Var(l), call, ev), Some((l, f)))
            }
            Target::Op {
                handler,
                effect,
                op,
                ty_args,
                call,
            } if !matches!(handler, Expr::Var(_) | Expr::Use { .. }) => {
                // An expression that only looks up the evidence table (`(use X).op()`) has no
                // side effects, so it is not bound.
                let h_ty = self.module.handler(effect);
                let l = self.module.local("h", h_ty);
                let t = Target::Op {
                    handler: Expr::Var(l),
                    effect,
                    op,
                    ty_args,
                    call,
                };
                (t, Some((l, handler)))
            }
            t => (t, None),
        };
        let opened = match (&target, self.member_callee(callee)) {
            (Target::Op { effect, op, .. }, Some(_)) => Some((*effect, *op)),
            _ => None,
        };
        let call = named::in_written_order(&mut self.module, args, |args| target.apply(args));
        let mut call = self.builtin_direct(call);
        if let Some((effect, index)) = opened {
            call = self.open_in_init(call, callee, effect, index);
        }
        match binding {
            Some((var, value)) => Expr::Let {
                var,
                value: Box::new(value),
                body: Box::new(call),
            },
            None => call,
        }
    }

    /// Turns a reference (`id`) to a builtin or constructor into a lambda that just calls it:
    /// `List.map(xs, Some)` becomes `List.map(xs, |x| Some(x))`. The arity comes from the
    /// reference's type.
    ///
    /// The lambda takes the evidence table as its last argument, as every function value does
    /// (`ty_takes_ev`), and uses it as the caller's table (`State.run`, and generic fns with
    /// effects with type arguments in their R).
    pub(super) fn wrap_in_lambda(&mut self, id: ExprId, span: Span) -> Expr {
        let ev = self.ty_takes_ev(self.expr_ty(id)).then(|| self.ev_local());
        let (outer, bound) = (self.ev, std::mem::take(&mut self.bound));
        if ev.is_some() {
            self.ev = ev;
        }
        let target = self.callee(id, span);
        self.ev = outer;
        self.bound = bound;
        let target = match target {
            Ok(t) => t,
            Err(e) => return e,
        };
        let param_tys = match self.expr_ty(id) {
            Ty::Fn(f) => &f.params,
            _ => unreachable!("a reference to a callable definition has a function type"),
        };
        let mut params: Vec<Local> = param_tys
            .iter()
            .map(|t| {
                let ty = self.ty(t);
                self.module.local("x", ty)
            })
            .collect();
        let args = params.iter().map(|&l| Expr::Var(l)).collect();
        params.extend(ev);
        Expr::Lambda {
            params,
            ty: self.ty(self.expr_ty(id)),
            body: Box::new({
                let call = target.apply(args);
                self.builtin_direct(call)
            }),
            suspends: self.ty_suspends(self.expr_ty(id)),
        }
    }

    /// If it cannot be lowered, records a diagnostic and puts a substitute expression in `Err`.
    pub(super) fn callee(&mut self, callee: ExprId, span: Span) -> Result<Target, Expr> {
        let hir = self.hir;
        // A call to a function value is decided by the callee expression's type.
        let call = Call {
            fails: self.callee_fails(callee),
            suspends: self.ty_suspends(self.expr_ty(callee)),
        };
        // A local fn called by its name (`[fn.local]`).
        if let ExprKind::Path(res) = hir.exprs[callee].kind
            && let Some(lifted) = self.lifted(res)
        {
            return Ok(self.lifted_target(&lifted));
        }
        // Opening a handler member inside an init (`Files.Open(p)`, `fs.Open(p)`): the
        // operation that opens it (`lower/member.rs`).
        if let Some(member) = self.member_callee(callee) {
            let provider = match &hir.exprs[callee].kind {
                ExprKind::Field { base, .. } => emela_types::MemberProvider::Cap(*base),
                _ => match self.expr_ty(callee) {
                    Ty::Fn(f) => match f.uses.tags().next() {
                        Some(tag) => emela_types::MemberProvider::Effect(tag.clone()),
                        None => return Err(self.unsupported("handler member", span)),
                    },
                    _ => return Err(self.unsupported("handler member", span)),
                },
            };
            let Some((handler, effect, index)) = self.member_provider(member, &provider, &[])
            else {
                return Err(self.unsupported("handler member without a provider", span));
            };
            let op = &self.module.effects[effect].ops[index];
            let call = Call {
                fails: op.fails,
                suspends: op.suspend,
            };
            return Ok(Target::Op {
                handler,
                effect,
                op: index,
                ty_args: Vec::new(),
                call,
            });
        }
        let ev = self
            .ty_takes_ev(self.expr_ty(callee))
            .then(|| self.ev_arg());
        // An operation on a capability value (`log.info(...)`).
        if let ExprKind::Field { base, .. } = &hir.exprs[callee].kind
            && let Some(&op) = self.types.ops.get(callee)
            && let Some(Ty::Cap(tags, _)) = self.types.exprs.get(*base).cloned()
            && let Some(tag) = self.op_tag(&tags, op)
            && let Some(index) = self.op_index(tag.def, op)
        {
            let effect = self.effect_id(&self.key_here(&tag));
            // Through a label, the operation is the base's (`EffectDef::label_of`).
            let effect = self.module.handler_effect(effect);
            let value = self.expr(*base);
            let (handler, _) = self
                .cap_table(value, &tags, op)
                .expect("the operation's effect is one of the capability's");
            // start of TaskGroup and Spawn wraps the task function (`fiber.rs`).
            if self.is_start_op(tag.def, op) {
                let op_ty = self.expr_ty(callee).clone();
                return match self.start_call(handler, &tag, &op_ty, callee) {
                    Some(start) => Ok(Target::Start(Box::new(start))),
                    None => Err(self.unsupported("start", span)),
                };
            }
            // Calls to suspend fn operations may suspend (spec 8.1); fn operation bodies do not
            // (E4303).
            let call = Call {
                suspends: self.module.effects[effect].ops[index].suspend,
                ..call
            };
            return Ok(Target::Op {
                handler,
                effect,
                op: index,
                ty_args: self.op_ty_args(effect, index, callee),
                call,
            });
        }
        // A method call's callee is typed as a reference to the function it calls.
        let def = match hir.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => def,
            _ => match self.types.methods.get(callee) {
                Some(&def) => def,
                None => return Ok(Target::Value(self.expr(callee), call, ev)),
            },
        };
        let is_trait_fn = hir.defs[def].kind == DefKind::TraitFn;
        match hir.item(def) {
            Some(Item::Fn(f)) if f.body.is_some() => {
                let ev = self
                    .takes_ev(def)
                    .then(|| self.generic_ev(def, callee, self.ev_arg()));
                let f = self.fn_ref(def, callee);
                let ty_args = self.ty_args(def, callee);
                Ok(Target::Direct(f, ty_args, self.direct_call(f, call), ev))
            }
            Some(Item::Fn(f)) if f.is_intrinsic && self.is_state_run(def) => self
                .state_run(callee, span)
                .map(|r| Target::StateRun(Box::new(r))),
            Some(Item::Fn(f)) if f.is_intrinsic => {
                if let Some(target) = self.fiber_target(def, callee, call, span) {
                    return target;
                }
                let data = &hir.defs[def];
                let module = match &data.module {
                    DefModule::Source(m) => self.modules[*m].name.to_string(),
                    _ => String::new(),
                };
                let Some(op) = Builtin::lookup_intrinsic(&module, &data.name) else {
                    return Err(self.unsupported(&format!("{module}.{}", data.name), span));
                };
                let ty_args = match self.types.type_args.get(callee) {
                    Some(tys) => tys.iter().map(|t| self.ty(t)).collect(),
                    None => Vec::new(),
                };
                // Monomorphization turns it into a call of `Show.show` (`Module::traits`), and
                // `dbg` and `inspect` call it for a type with a written Show impl (spec 10.6).
                let basic = ty_args.first().is_some_and(|&t| {
                    matches!(
                        self.module.ty(t),
                        Type::Int
                            | Type::Int64
                            | Type::Float
                            | Type::Bool
                            | Type::String
                            | Type::Unit
                    )
                });
                if op == Builtin::ShowNested
                    || matches!(op, Builtin::Dbg | Builtin::Inspect) && !basic
                {
                    self.prelude_trait_fn_id("Show", "show");
                }
                Ok(Target::Builtin { op, ty_args })
            }
            // External functions do not fail (spec 12.5), and their R is empty or only Async
            // (spec 12.2), so no table is passed.
            Some(Item::Fn(_)) => {
                let f = self.fn_ref(def, callee);
                let ty_args = self.ty_args(def, callee);
                let call = Call {
                    fails: false,
                    ..call
                };
                Ok(Target::Direct(f, ty_args, self.direct_call(f, call), None))
            }
            Some(Item::Ctor(_) | Item::Error(_)) => match self.ctors.get(def) {
                Some(&ctor) => {
                    let mut ty_args = self.ty_args(def, callee);
                    ty_args.truncate(self.visible_params(def));
                    Ok(Target::Ctor(ctor, ty_args))
                }
                None => Err(self.unsupported("this constructor", span)),
            },
            // A trait function, called directly; monomorphization picks the impl (spec 10.3).
            _ if is_trait_fn => {
                let ev = self
                    .takes_ev(def)
                    .then(|| self.generic_ev(def, callee, self.ev_arg()));
                let f = self.fn_ref(def, callee);
                let ty_args = self.ty_args(def, callee);
                Ok(Target::Direct(f, ty_args, self.direct_call(f, call), ev))
            }
            _ => Ok(Target::Value(self.expr(callee), call, ev)),
        }
    }

    /// Whether the definition's declared R has tags with type parameters (`S` of `State[S]`).
    pub(super) fn has_generic_tags(&self, def: DefId) -> bool {
        !self.generic_tags(def).is_empty()
    }

    /// Tags in the declared R that contain type parameters.
    pub(super) fn generic_tags(&self, def: DefId) -> Vec<Tag> {
        match self.types.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => f
                .uses
                .tags()
                .filter(|t| t.args.iter().any(has_bound))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The evidence table for calling the generic definition `def` (spec 8.1). The callee's body
    /// looks up keys written with its own type parameters (`State[$modify.0]`), so the handler for
    /// the caller's instantiated key (`State[Int]`) is added to the current table `ev` under that
    /// key too. The instantiated type arguments are the `type_args` of the referencing expression
    /// `callee`.
    pub(super) fn generic_ev(&mut self, def: DefId, callee: ExprId, ev: Expr) -> Expr {
        let tags = self.generic_tags(def);
        let Some(args) = self.types.type_args.get(callee).cloned() else {
            // Monomorphic references within a group (recursion) do not instantiate type
            // parameters, so the keys work as is.
            return ev;
        };
        let mut bindings = Vec::new();
        for tag in tags {
            let inner = self.tag_key(&tag, Some(def));
            let instance = tag.map_args(|t| emela_types::substitute_bound(t, &args));
            let outer = self.key_here(&instance);
            if inner == outer {
                continue;
            }
            if let Some(h) = self.lookup_effect(&outer) {
                bindings.push((self.effect_id(&inner), h));
            }
        }
        if bindings.is_empty() {
            return ev;
        }
        Expr::Provide {
            base: match ev {
                Expr::Lit(Lit::Unit) => None,
                ev => Some(Box::new(ev)),
            },
            bindings,
        }
    }

    /// Whether it is the bundled core's `State.run` (spec 8.8).
    pub(super) fn is_state_run(&self, def: DefId) -> bool {
        let data = &self.hir.defs[def];
        let module = match &data.module {
            DefModule::Source(m) => self.modules[*m].name.to_string(),
            _ => return false,
        };
        crate::intrinsics::is_runtime_handler(&module, &data.name)
    }

    /// Prepares lowering a `State.run` call. `callee` is the expression referencing `State.run`.
    pub(super) fn state_run(&mut self, callee: ExprId, span: Span) -> Result<StateRun, Expr> {
        let Ty::Fn(f) = self.expr_ty(callee).clone() else {
            return Err(self.unsupported("State.run", span));
        };
        let Some(Ty::Fn(body)) = f.params.get(1) else {
            return Err(self.unsupported("State.run", span));
        };
        // The State[S] in body's R: the State effect of the same module as State.run.
        let module = &self.hir.defs[self.hir_def_of(callee)].module;
        let Some(tag) = body
            .uses
            .effect_tags()
            .find(|t| {
                let d = &self.hir.defs[t.def];
                d.kind == DefKind::Effect && d.name == "State" && d.module == *module
            })
            .cloned()
        else {
            return Err(self.unsupported("State.run", span));
        };
        let effect = self.effect_id(&self.key_here(&tag));
        let index = |name: &str| {
            self.module.effects[effect]
                .ops
                .iter()
                .position(|o| o.name == name)
        };
        let (Some(get), Some(set), Some(update)) = (index("get"), index("set"), index("update"))
        else {
            return Err(self.unsupported("State.run", span));
        };
        let call = Call {
            fails: !body.fails.is_empty(),
            suspends: self.row_suspends(&body.uses),
        };
        let base = self.ev.map(Expr::Var);
        // `State.run(initial: S, body: fn() -> A uses { State[S] }) -> (A, S)` (spec 8.8).
        let state = match f.params.first() {
            Some(s) => self.ty(s),
            None => TypeId::UNIT,
        };
        let result = self.ty(&body.ret);
        let update_ty = self.module.func(FnType {
            params: vec![state],
            ret: state,
            fails: false,
            suspends: false,
            // A function value (`ty_takes_ev`), which is pure (spec 8.8).
            ev: true,
            uses: false,
            sets: None,
        });
        let cell_ty = self.module.cell(state);
        let handler_ty = self.module.handler(effect);
        let op_tys = [
            self.module.fn_type(Vec::new(), state),
            self.module.fn_type(vec![state], TypeId::UNIT),
            self.module.fn_type(vec![update_ty], TypeId::UNIT),
        ];
        Ok(StateRun {
            effect,
            ops: [get, set, update],
            base,
            call,
            state,
            op_tys,
            cell: self.module.local("cell", cell_ty),
            handler: self.module.local("state", handler_ty),
            value: self.module.local("value", state),
            f: self.module.local("f", update_ty),
            result: self.module.local("result", result),
        })
    }

    /// The definition of a referencing expression.
    pub(super) fn hir_def_of(&self, expr: ExprId) -> DefId {
        match self.hir.exprs[expr].kind {
            ExprKind::Path(Res::Def(def)) => def,
            // The callee of a method call (`[type.rules]`).
            _ => *self
                .types
                .methods
                .get(expr)
                .expect("a definition reference"),
        }
    }

    /// The mark for a direct call of `f`. Whether it suspends matches the function's mark
    /// (`Expr::Call`).
    pub(super) fn direct_call(&self, f: FnId, call: Call) -> Call {
        Call {
            suspends: self.module.functions[f].suspends,
            ..call
        }
    }

    /// Whether the callee may fail: true if the E of the callee expression's function type (as
    /// solved by the type checker) is not empty. Opaque sets, such as set type parameters and
    /// generalized set variables, are not empty, so they count as true. Builtins and constructors
    /// have an empty E, and builtins take no function arguments whose failures they could pass on.
    pub(super) fn callee_fails(&self, callee: ExprId) -> bool {
        matches!(self.expr_ty(callee), Ty::Fn(f) if !f.fails.is_empty())
    }
}
