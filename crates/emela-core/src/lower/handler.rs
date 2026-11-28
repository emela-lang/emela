//! Handlers (`[effect.handler]`): the functions a handler definition becomes and the value of
//! `resume`.

use emela_resolve::hir::{DefId, Item, Res};
use emela_types::{Row, Tag, Ty};

use crate::ir::{Callee, EffectId, Expr, FnType, Lit, Local, Type, TypeId};

use super::{Lowerer, is_stateful};

impl<'a> Lowerer<'a> {
    /// Lowers a handler to IR functions (called from `define_fn`). Returns the parameters and body
    /// of the function that returns the handler value.
    ///
    /// A stateless handler is a function returning the handler value (operations as lambdas in
    /// declaration order). If it has dependencies (the R of operation bodies, spec 8.4), it takes
    /// the evidence table at the `with` as its last argument.
    ///
    /// A stateful handler (with fields, init or release, spec 8.4) becomes three functions. The init
    /// function (`name$init`) returns the state (self); the function returning the handler value
    /// takes the state as its first argument for the operation lambdas to capture; and the release
    /// function (`name$release`) takes the state and evaluates the release body. All three take the
    /// same evidence table as their last argument (dependencies(H) is the union of the R of init,
    /// release and operation bodies).
    pub(super) fn handler(&mut self, def: DefId) -> (Vec<Local>, TypeId, Expr) {
        let hir = self.hir;
        let Some(Item::Handler(h)) = hir.item(def) else {
            unreachable!("a handler definition");
        };
        let span = hir.defs[def].span;
        // The effects in the order of `Ty::Cap`, which is the order of the operation tables of a
        // handler of several effects (`[effect.handler]`).
        let tags = match Ty::cap(
            self.types.sigs.handler_effects(def).iter().cloned(),
            Row::empty(),
        ) {
            Ty::Cap(tags, _) if !tags.is_empty() => tags,
            _ => {
                return (
                    Vec::new(),
                    TypeId::UNIT,
                    self.unsupported_at("handler of an unknown effect", span),
                );
            }
        };
        let effect_ids: Vec<EffectId> = tags
            .iter()
            .map(|t| self.effect_id(&self.key_here(t)))
            .collect();
        let ret = match effect_ids.as_slice() {
            [one] => self.module.handler(*one),
            many => {
                let elems = many.iter().map(|&e| self.module.handler(e)).collect();
                self.module.tuple(elems)
            }
        };
        let takes_ev = self
            .types
            .handler_uses
            .get(def)
            .is_some_and(|r| self.row_needs_ev(r));
        let stateful = is_stateful(h);
        if stateful {
            self.handler_parts(def, h, takes_ev);
        }

        let mut params = Vec::new();
        if stateful {
            // Every operation's implicit self refers to this parameter.
            let state_ty = self.handler_state_ty(def, h);
            let state = self.module.local("self", state_ty);
            for op in &h.ops {
                self.locals.insert(op.self_local, state);
            }
            params.push(state);
        }
        self.ev = None;
        if takes_ev {
            let ev = self.ev_local();
            params.push(ev);
            self.ev = Some(ev);
        }
        self.self_cap = Some((
            def,
            stateful.then(|| params[0]),
            self.ev.filter(|_| takes_ev),
        ));
        // One table per effect; an operation goes into the table of the effect that declares it.
        let mut tables: Vec<Vec<Option<Expr>>> = effect_ids
            .iter()
            .map(|&e| vec![None; self.module.effects[e].ops.len()])
            .collect();
        let table_of = |o: DefId| {
            let parent = hir.defs[o].parent?;
            tags.iter().position(|t| t.def == parent)
        };
        for op in &h.ops {
            let place = match op.op {
                Res::Def(o) => table_of(o).and_then(|k| Some((k, self.op_index(tags[k].def, o)?))),
                _ => None,
            };
            let (Some((k, index)), Some(body)) = (place, op.body) else {
                continue;
            };
            // A suspend fn implementation takes the operation's parameters plus resume last
            // (spec 8.4). resume is an ordinary function value; calling it neither suspends nor
            // fails (its type is `fn(T) -> ()`).
            let mut ps: Vec<Local> = op.params.iter().map(|p| self.local(p.local)).collect();
            let mut lowered = self.expr(body);
            // The runtime passes resume without the evidence table, but function values take it
            // (`ty_takes_ev`), so the body sees resume through a lambda that takes the table and
            // drops it.
            if op.is_suspend
                && let Some(user) = ps.pop()
            {
                lowered = self.resume_value(&mut ps, user, lowered);
            }
            // If the body may suspend (awaiting an `@external suspend fn`, say), the implementation
            // is a generator too. Async that only calls resume later (spec 8.7) is not in the
            // body's R, so it does not suspend.
            let row_suspends = self
                .types
                .effects
                .get(body)
                .is_some_and(|r| self.row_suspends(r));
            // As in the type checker (E4303), an fn-operation implementation is not a generator
            // unless its body's R is not Immediate and it contains a suspending call. With Clock in
            // the body's R, calling only `clock.now()` does not suspend. When R is Immediate but a
            // true call occurs (inside a `with` of an immediately resuming handler), the backend
            // runs it on the spot, as in non-suspending function bodies.
            let suspends = if op.is_suspend {
                row_suspends
            } else {
                row_suspends && super::suspend::has_suspending_call(&lowered)
            };
            // A failure in an operation body returns to the operation's caller (as with lambdas).
            let ret = self.ty(self.expr_ty(body));
            let ty = self.module.func(FnType {
                params: ps.iter().map(|&l| self.module.locals[l].ty).collect(),
                ret,
                fails: self.types.errors.get(body).is_some_and(|r| !r.is_empty()),
                suspends,
                ev: false,
                uses: false,
                sets: None,
            });
            tables[k][index] = Some(Expr::Lambda {
                params: ps,
                ty,
                body: Box::new(lowered),
                suspends,
            });
        }
        // Handler members are implemented by the inner handlers (`lower/member.rs`).
        for &inner in &h.inner {
            let Some(Res::Def(member)) = hir.item(inner).and_then(|i| match i {
                Item::Handler(i) => i.member,
                _ => None,
            }) else {
                continue;
            };
            let Some(k) = table_of(member) else {
                continue;
            };
            let Some(index) = self.member_index(tags[k].def, member) else {
                continue;
            };
            let op = self.module.effects[effect_ids[k]].ops[index].clone();
            tables[k][index] = Some(self.member_impl(inner, &op));
        }
        if tables.iter().flatten().any(Option::is_none) {
            // Missing operations have already been reported by the type checker (E4001).
            return (
                params,
                ret,
                self.unsupported_at("handler with missing operations", span),
            );
        }
        self.self_cap = None;
        let name = hir.defs[def].name.to_string();
        let mut values: Vec<Expr> = tables
            .into_iter()
            .zip(&effect_ids)
            .map(|(ops, &effect)| Expr::Handler {
                name: name.clone(),
                effect,
                ops: ops.into_iter().map(Option::unwrap).collect(),
            })
            .collect();
        // A handler of several effects returns the tuple of its tables, which share `self`.
        let value = if values.len() == 1 {
            values.pop().expect("one table")
        } else {
            Expr::Tuple(values)
        };
        (params, ret, value)
    }

    /// Binds `user`, the resume parameter as the body of a suspend fn implementation sees it (a
    /// function value taking the evidence table), around `body` to a lambda that calls the
    /// resume the runtime passes, which is pushed onto `params` (`fn(T) -> ()` without the
    /// table).
    pub(super) fn resume_value(
        &mut self,
        params: &mut Vec<Local>,
        user: Local,
        body: Expr,
    ) -> Expr {
        let user_ty = self.module.locals[user].ty;
        let Type::Fn(f) = self.module.ty(user_ty).clone() else {
            unreachable!("resume has a function type");
        };
        let raw_ty = self.module.func(FnType::pure(f.params.clone(), f.ret));
        let raw = self.module.local("resume", raw_ty);
        params.push(raw);
        let xs: Vec<Local> = f
            .params
            .iter()
            .map(|&t| self.module.local("x", t))
            .collect();
        let mut lambda_params = xs.clone();
        lambda_params.push(self.module.local("ev", TypeId::EVIDENCE));
        Expr::Let {
            var: user,
            value: Box::new(Expr::Lambda {
                params: lambda_params,
                ty: user_ty,
                body: Box::new(Expr::Call {
                    callee: Callee::Indirect(Box::new(Expr::Var(raw))),
                    args: xs.into_iter().map(Expr::Var).collect(),
                    fails: false,
                    suspends: false,
                }),
                suspends: false,
            }),
            body: Box::new(body),
        }
    }

    /// Lowers the bodies of a stateful handler's init and release functions.
    pub(super) fn handler_parts(
        &mut self,
        def: DefId,
        h: &emela_resolve::hir::HandlerItem,
        takes_ev: bool,
    ) {
        let (init, release) = self.handler_fns(def);
        let ev_param = |cx: &mut Self| {
            cx.ev = None;
            takes_ev.then(|| {
                let ev = cx.ev_local();
                cx.ev = Some(ev);
                ev
            })
        };
        let state_ty = self.handler_state_ty(def, h);

        // init's value is the handler's state. For a handler without init (no fields), the state
        // is its constructor's value if it has one (`ConsoleLogger`), otherwise `()`. init takes
        // its own parameters (`[effect.handler]`) before the table.
        let mut params: Vec<Local> = h.init_params.iter().map(|p| self.local(p.local)).collect();
        // An init that opens handler members takes the cell that keeps their release
        // (`lower/member.rs`).
        let opens = self.init_opens(def);
        let cell = opens.map(|s| {
            let ty = self.opened_cell_ty(s);
            (self.module.local("opened", ty), s)
        });
        params.extend(cell.map(|(c, _)| c));
        params.extend(ev_param(self));
        self.opened_cell = cell;
        let body = match (h.init, h.ctor.and_then(|c| self.ctors.get(c).copied())) {
            (Some(body), _) => {
                let body = self.expr(body);
                match cell {
                    Some((c, s)) => {
                        let name = self.hir.defs[def].name.to_string();
                        self.guard_init(body, state_ty, c, s, &name)
                    }
                    None => body,
                }
            }
            (None, Some(ctor)) => Expr::Ctor {
                ctor,
                ty_args: Vec::new(),
                args: Vec::new(),
            },
            (None, None) => Expr::Lit(Lit::Unit),
        };
        self.opened_cell = None;
        self.module.define(init, params, state_ty, body);
        self.module.functions[init].type_params = self.handler_type_params(def);

        if let (Some(f), Some((self_local, body))) = (release, h.release) {
            // The state's type, which the type checker gives as the handler's type even when the
            // state is `()` (no fields and no init).
            let state = self.local(self_local);
            self.module.locals[state].ty = state_ty;
            let mut params = vec![state];
            params.extend(ev_param(self));
            self.self_cap = Some((def, Some(state), self.ev));
            // The release block's value is discarded (spec 8.4), but the function returns it as is.
            let ret = self.ty(self.expr_ty(body));
            let body = self.expr(body);
            self.self_cap = None;
            self.module.define(f, params, ret, body);
            self.module.functions[f].type_params = self.handler_type_params(def);
        }
    }

    /// The type arguments of a provider of a `with` (`[effect.with]`), as IR types.
    pub(super) fn bind_ty_args(&mut self, bind: &emela_types::WithBind) -> Vec<TypeId> {
        bind.ty_args.iter().map(|t| self.ty(t)).collect()
    }

    /// The effects of `handler` instantiated with the type arguments of an item, in the order
    /// written (provides(H), `[type.rules]`).
    pub(super) fn handler_tags(&self, handler: DefId, ty_args: &[Ty]) -> Vec<Tag> {
        let own = self.types.sigs.handler_effects(handler);
        if ty_args.is_empty() {
            return own.to_vec();
        }
        own.iter()
            .map(|t| t.map_args(|a| emela_types::substitute_bound(a, ty_args)))
            .collect()
    }
}
