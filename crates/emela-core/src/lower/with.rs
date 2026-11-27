//! `with` (`[effect.with]`, `[dyn.with]`): providing handlers, their init and release, and the
//! evidence table of the body.

use emela_resolve::hir::{DefId, ExprId, Item, Res, Span, WithItem};
use emela_types::{ArgsSource, Row, Supplier, Tag, Ty};
use la_arena::ArenaMap;
use rustc_hash::FxHashMap;

use crate::ir::{Callee, EffectId, Expr, FnId, Lit, Local, Type, WithBind};

use super::{
    EffectKey, Lowerer, OnFailure, TaskItem, direct, direct_marked, generic_call, is_stateful,
    mentions,
};

impl<'a> Lowerer<'a> {
    /// `with H1, ..., Hn { body }`（8.3，14.2）．
    ///
    /// Builds the handlers in the initialization order chosen by the type checker
    /// (`ProgramTypes::with_handlers`; layers expanded, handlers overridden by later ones removed)
    /// and evaluates the body with the current evidence table extended by the provided effects. A
    /// handler's dependencies are satisfied by handlers in the same `with` providing other effects;
    /// the rest, and dependencies on its own effect, by the outer table. Dependency cycles have been
    /// reported by the type checker (E4101).
    ///
    /// For a stateful handler, init's value (the state) becomes a binding without release, and the
    /// handler value built from the state a binding with release (`WithBind::managed`). Without a
    /// release function, release is `()`.
    ///
    /// The arguments of init (`H(url: ...)`) come first, as bindings without release, in the
    /// order the items are written and with the outer table (`[dyn.with]`). A failure there ends
    /// the `with` before any init runs, so nothing is released. The arguments of a handler a
    /// later item overrides are not evaluated.
    pub(super) fn with(
        &mut self,
        id: ExprId,
        items: &[WithItem],
        body: ExprId,
        span: Span,
    ) -> Expr {
        let hir = self.hir;
        let Some(info) = self.types.with_handlers.get(id) else {
            return self.unsupported("with without handlers", span);
        };
        // (supplier, key of the provided effect).
        let mut hs: Vec<(Supplier, EffectKey)> = Vec::new();
        for bind in &info.binds {
            let own = self.key_here(&bind.effect);
            match &bind.supplier {
                Supplier::Handler(def) => {
                    // Errors go at the handler as written; for handlers expanded from a layer, at
                    // the `with`.
                    let span = items
                        .iter()
                        .find_map(|item| match item.by {
                            Some((res, s)) if res == Res::Def(*def) => Some(s),
                            None if item.res == Res::Def(*def) => Some(item.span),
                            _ => None,
                        })
                        .unwrap_or(span);
                    if !matches!(hir.item(*def), Some(Item::Handler(_))) {
                        return self.unsupported("this kind of handler", span);
                    }
                }
                Supplier::Effect(_) | Supplier::Member { .. } => {}
            }
            hs.push((bind.supplier.clone(), own));
        }
        // Whether each provider provides its effect by name; an item that opens a handler member,
        // or a bound item a later one overrides, is reached only through its name
        // (`[effect.with]`).
        let named: Vec<bool> = info.binds.iter().map(|b| b.named).collect();
        // Labels declared `default`, provided by the provider of their base (issue #255).
        let defaults: Vec<(EffectKey, usize)> = info
            .defaults
            .iter()
            .map(|(tag, i)| (self.key_here(tag), *i))
            .collect();
        let outer = self.ev;
        let mut locals: Vec<Local> = Vec::with_capacity(hs.len());
        let mut binds = Vec::new();
        let mut written_args: FxHashMap<ExprId, Local> = FxHashMap::default();
        for (i, item) in items.iter().enumerate() {
            if !info.initialized.get(i).copied().unwrap_or(false) {
                continue;
            }
            let (_, args) = item.supplier();
            for arg in args.iter().flat_map(|a| &a.args) {
                let ty = self.ty(self.expr_ty(arg.value));
                let l = self.module.local("arg", ty);
                let value = self.expr(arg.value);
                binds.push(WithBind::new(l, value));
                written_args.insert(arg.value, l);
            }
        }
        // A handler of several effects has a bind for each effect it provides. It is initialized
        // once, at its first bind (`[effect.with]`): the tuple of its handlers is bound with its
        // release as a cleanup, then each handler is taken from the tuple and bound so that it
        // is marked released when the `with` ends. Each bind of the handler refers to its own.
        // (handler, where its arguments are) → the tuple and its handlers.
        let mut tuples: Vec<((DefId, ArgsSource), Local, Vec<Local>)> = Vec::new();
        // The items of groups and backgrounds (`lower/fiber.rs`).
        let mut tasks: Vec<TaskItem> = Vec::new();
        for (i, (supplier, own)) in hs.iter().enumerate() {
            let own_id = self.effect_id(own);
            let handler_ty = self.module.handler_effect(own_id);
            let handler_ty = self.module.handler(handler_ty);
            if let Supplier::Handler(h) = supplier
                && let Some((_, _, parts)) = tuples
                    .iter()
                    .find(|(k, _, _)| *k == (*h, info.binds[i].args))
            {
                let bind = &info.binds[i];
                let part = parts[self.tuple_index(*h, &bind.ty_args, &bind.effect)];
                let l = self.module.local("h", handler_ty);
                locals.push(l);
                binds.push(WithBind::new(l, Expr::Var(part)));
                continue;
            }
            let h = match supplier {
                Supplier::Handler(h) => *h,
                // Opening a handler member (`lower/member.rs`): the pair of the handler value and
                // its release, then the handler value bound with that release.
                Supplier::Member {
                    member, provider, ..
                } => {
                    let earlier: Vec<(EffectKey, Local)> = hs[..i]
                        .iter()
                        .zip(&locals)
                        .zip(&named)
                        .filter(|(_, n)| **n)
                        .map(|(((_, k), &l), _)| (k.clone(), l))
                        .collect();
                    let Some((value, effect, index)) =
                        self.member_provider(*member, provider, &earlier)
                    else {
                        return self.unsupported("handler member without a provider", span);
                    };
                    let op = self.module.effects[effect].ops[index].clone();
                    let args = self.init_args(id, info.binds[i].args, &written_args);
                    let pair = self.module.local("opened", op.ret);
                    let open = Expr::Call {
                        callee: Callee::Op {
                            handler: Box::new(value),
                            effect,
                            op: index,
                            ty_args: Vec::new(),
                        },
                        args,
                        fails: op.fails,
                        suspends: op.suspend,
                    };
                    let open = self.builtin_direct(open);
                    binds.push(WithBind::new(pair, open));
                    let (h_ty, release_ty) = match self.module.ty(op.ret) {
                        Type::Tuple(elems) => (elems[0], elems[1]),
                        _ => unreachable!("a member's operation returns a pair"),
                    };
                    let l = self.module.local("h", h_ty);
                    locals.push(l);
                    let release = Expr::Call {
                        callee: Callee::Indirect(Box::new(super::member::tuple_get(
                            pair,
                            1,
                            2,
                            release_ty,
                            &mut self.module,
                        ))),
                        args: Vec::new(),
                        fails: false,
                        suspends: op.suspend,
                    };
                    let value = super::member::tuple_get(pair, 0, 2, h_ty, &mut self.module);
                    // A handler member of several effects opens the tuple of their handlers
                    // (`[effect.handler]`): the tuple gets the release, and each handler in it is
                    // marked released, as at an item of a handler of several effects.
                    if let Type::Tuple(parts) = self.module.ty(h_ty).clone() {
                        binds.push(WithBind::cleanup(l, value, &op.name, release));
                        let n = parts.len();
                        for (k, ty) in parts.into_iter().enumerate() {
                            let part = self.module.local("h", ty);
                            let value = super::member::tuple_get(l, k, n, ty, &mut self.module);
                            binds.push(WithBind::managed(
                                part,
                                value,
                                &op.name,
                                Expr::Lit(Lit::Unit),
                            ));
                        }
                        continue;
                    }
                    binds.push(WithBind::managed(l, value, &op.name, release));
                    continue;
                }
                // The provider of an effect already in scope: a handler in the same `with`
                // initialized earlier, or the one the outer table holds (issue #255).
                Supplier::Effect(tag) => {
                    let key = self.key_here(tag);
                    let found = (0..i).find(|&j| named[j] && hs[j].1 == key);
                    let value = match found {
                        Some(j) => Expr::Var(locals[j]),
                        None => match self.lookup_effect(&key) {
                            Some(e) => e,
                            None => return self.unsupported("label without a provider", span),
                        },
                    };
                    let l = self.module.local("h", handler_ty);
                    locals.push(l);
                    binds.push(WithBind::new(l, value));
                    continue;
                }
            };
            // A built-in handler: its value binds the runtime object it makes, and its release
            // does nothing (`lower/builtin_handler.rs`).
            if self.is_builtin_handler(h) {
                let args = self.init_args(id, info.binds[i].args, &written_args);
                // A group or a background: the rest of the with runs in its run-time scope
                // (`lower/fiber.rs`).
                if let Some(background) = self.task_handler(own_id) {
                    let l = self.module.local("h", handler_ty);
                    let earlier: Vec<(EffectId, Expr)> = hs[..i]
                        .iter()
                        .zip(&locals)
                        .zip(&named)
                        .filter(|(_, n)| **n)
                        .map(|(((_, k), &l), _)| (self.effect_id(k), Expr::Var(l)))
                        .collect();
                    let ev = if earlier.is_empty() {
                        self.ev_arg()
                    } else {
                        Expr::Provide {
                            base: outer.map(|l| Box::new(Expr::Var(l))),
                            bindings: earlier,
                        }
                    };
                    locals.push(l);
                    let fails = self.tasks_fail(id, &info.binds[i].ty_args, background);
                    let background = match (background, args.into_iter().next()) {
                        (true, Some(value)) => {
                            let Some(f_ty) = self.on_failure_ty(id, info.binds[i].args) else {
                                return self.unsupported("Task.Background", span);
                            };
                            let ty = self.ty(&f_ty);
                            Some(OnFailure {
                                value,
                                ty,
                                takes_ev: self.ty_takes_ev(&f_ty),
                                suspends: self.ty_suspends(&f_ty),
                            })
                        }
                        (false, _) => None,
                        (true, None) => return self.unsupported("Task.Background", span),
                    };
                    tasks.push(TaskItem {
                        at: binds.len(),
                        local: l,
                        effect: own_id,
                        background,
                        ev,
                        fails,
                    });
                    binds.push(WithBind::new(l, Expr::Lit(Lit::Unit)));
                    continue;
                }
                let Some(value) = self.builtin_handler(h, own_id, args) else {
                    return self.unsupported("this built-in handler", span);
                };
                let l = self.module.local("h", handler_ty);
                locals.push(l);
                binds.push(WithBind::new(l, value));
                continue;
            }
            let f = self.fn_id(h, 0);
            // The type arguments of a handler with type parameters (`[effect.with]`): its
            // functions are generic, and its effects and requirements are instantiated.
            let bind = &info.binds[i];
            let ty_args = self.bind_ty_args(bind);
            let ctx = if self
                .types
                .handler_uses
                .get(h)
                .is_some_and(|r| self.row_needs_ev(r))
            {
                // Dependencies satisfied by handlers of other effects initialized earlier in the
                // same `with`. Those on any of the handler's own effects are met from outside
                // (`[effect.handler]`). A handler plugged into a label (`L by H`) provides only
                // L, so its dependency on the underlying effect is met in the same `with` too
                // (`[effect.label]`, issue #679), as `layer::with_entries` orders it.
                let row = self.types.handler_uses.get(h).cloned().unwrap_or_default();
                let row = row.map_tys(|t| emela_types::substitute_bound(t, &bind.ty_args));
                let effects = self.handler_tags(h, &bind.ty_args);
                let owns: Vec<EffectKey> = if effects.contains(&info.binds[i].effect) {
                    effects.iter().map(|t| self.key_here(t)).collect()
                } else {
                    Vec::new()
                };
                let keys: Vec<EffectKey> = row
                    .tags()
                    .map(|t| self.key_here(t))
                    .filter(|y| y != own && !owns.contains(y))
                    .collect();
                let bindings: Vec<(EffectId, Expr)> = keys
                    .into_iter()
                    .filter_map(|y| {
                        let j = (0..i).find(|&j| named[j] && hs[j].1 == y)?;
                        Some((self.effect_id(&y), Expr::Var(locals[j])))
                    })
                    .collect();
                Some(if bindings.is_empty() {
                    self.ev_arg()
                } else {
                    Expr::Provide {
                        base: outer.map(|l| Box::new(Expr::Var(l))),
                        bindings,
                    }
                })
            } else {
                None
            };
            let l = self.module.local("h", handler_ty);
            locals.push(l);
            let Some(Item::Handler(item)) = hir.item(h) else {
                unreachable!("a handler definition");
            };
            // For a handler of several effects, the value is bound to the tuple, and `l` takes its
            // own handler from it after.
            let several = self.types.sigs.handler_effects(h).len() > 1;
            let target = if several {
                let ty = Ty::cap(self.handler_tags(h, &bind.ty_args), Row::empty());
                let ty = self.ty(&ty);
                self.module.local("hs", ty)
            } else {
                l
            };
            if !is_stateful(item) {
                binds.push(WithBind::new(
                    target,
                    generic_call(direct(f, ctx.into_iter().collect(), false), &ty_args),
                ));
                if several {
                    let parts = self.bind_tuple_parts(h, &bind.ty_args, target, None, &mut binds);
                    let part = parts[self.tuple_index(h, &bind.ty_args, &bind.effect)];
                    binds.push(WithBind::new(l, Expr::Var(part)));
                    tuples.push(((h, info.binds[i].args), target, parts));
                }
                continue;
            }
            // A stateful handler uses the table three times, so bind it to a variable.
            let ctx = ctx.map(|c| match c {
                Expr::Var(_) | Expr::Lit(_) => c,
                c => {
                    let v = self.ev_local();
                    binds.push(WithBind::new(v, c));
                    Expr::Var(v)
                }
            });
            let (init, release) = self.handler_fns(h);
            let state_ty = self.handler_state_ty(h, item);
            let state_ty = self.module.types.subst(state_ty, &ty_args);
            let state = self.module.local("self", state_ty);
            let init_fails = self
                .types
                .handler_fails
                .get(h)
                .is_some_and(|r| !r.is_empty());
            let mut init_args = self.init_args(id, info.binds[i].args, &written_args);
            // An init that opens handler members keeps their release in a cell; they are released
            // after the handler's release (`lower/member.rs`).
            let cell = self.init_opens(h).map(|s| {
                let ty = self.opened_cell_ty(s);
                (self.module.local("opened", ty), s)
            });
            if let Some((c, s)) = cell {
                let value = self.new_opened_cell(s);
                binds.push(WithBind::new(c, value));
                init_args.push(Expr::Var(c));
            }
            init_args.extend(ctx.clone());
            binds.push(WithBind::new(
                state,
                generic_call(
                    direct_marked(&self.module, init, init_args, init_fails),
                    &ty_args,
                ),
            ));
            let with_state = |ctx: &Option<Expr>| {
                std::iter::once(Expr::Var(state))
                    .chain(ctx.clone())
                    .collect::<Vec<_>>()
            };
            let mut release = match release {
                Some(r) => generic_call(
                    direct_marked(&self.module, r, with_state(&ctx), false),
                    &ty_args,
                ),
                None => Expr::Lit(Lit::Unit),
            };
            if let Some((c, s)) = cell {
                let name = hir.defs[h].name.to_string();
                release = self.then_release_opened(release, c, s, &name);
            }
            if several {
                let name = hir.defs[h].name.as_str();
                binds.push(WithBind::cleanup(
                    target,
                    generic_call(direct(f, with_state(&ctx), false), &ty_args),
                    name,
                    release,
                ));
                let parts = self.bind_tuple_parts(h, &bind.ty_args, target, Some(name), &mut binds);
                let part = parts[self.tuple_index(h, &bind.ty_args, &bind.effect)];
                binds.push(WithBind::new(l, Expr::Var(part)));
                tuples.push(((h, info.binds[i].args), target, parts));
                continue;
            }
            binds.push(WithBind::managed(
                target,
                generic_call(direct(f, with_state(&ctx), false), &ty_args),
                &hir.defs[h].name,
                release,
            ));
        }
        // A name bound to an item is the handler value of that item (`[effect.with]`): the tuple
        // of a handler of several effects.
        for (b, &l) in info.binds.iter().zip(&locals) {
            if let ArgsSource::Item(origin) = b.args
                && let Some(local) = items.get(origin).and_then(|item| item.bind)
            {
                let value = match &b.supplier {
                    Supplier::Handler(h) => tuples
                        .iter()
                        .find(|(k, _, _)| *k == (*h, b.args))
                        .map_or(l, |(_, all, _)| *all),
                    _ => l,
                };
                self.locals.insert(local, value);
            }
        }
        // Handler members that the init around opens through this `with`'s handlers are released
        // at its end, before its handlers (`[effect.handler]`, `lower/member.rs`).
        let opens: Vec<DefId> = self
            .types
            .member_open_withs
            .iter()
            .filter(|(_, w)| **w == id)
            .filter_map(|(callee, _)| self.types.members.get(callee).copied())
            .collect();
        let with_cell = (!opens.is_empty()).then(|| {
            let s = opens
                .iter()
                .any(|&m| super::member::member_suspends(hir, m));
            let ty = self.opened_cell_ty(s);
            let c = self.module.local("opened", ty);
            let value = self.new_opened_cell(s);
            let release = self.release_opened(c, s);
            binds.push(WithBind::cleanup(c, value, "opened", release));
            (c, s)
        });
        if let Some(cell) = with_cell {
            self.with_cells.push((id, cell));
        }
        let ctx = self.ev_local();
        let depth = self.bound.len();
        for (((_, effect), &l), _) in hs.iter().zip(&locals).zip(&named).filter(|(_, n)| **n) {
            self.bound.push((effect.clone(), l));
        }
        for (label, i) in &defaults {
            self.bound.push((label.clone(), locals[*i]));
        }
        self.ev = Some(ctx);
        let body = self.expr(body);
        self.ev = outer;
        self.bound.truncate(depth);
        if with_cell.is_some() {
            self.with_cells.pop();
        }
        // Not built if the body does not use the table (when `use` only refers to this `with`'s
        // handlers directly).
        if mentions(&body, ctx) {
            let bindings = hs
                .iter()
                .zip(&locals)
                .zip(&named)
                .filter(|(_, n)| **n)
                .map(|(((_, effect), &l), _)| (effect, l))
                .chain(defaults.iter().map(|(label, i)| (label, locals[*i])))
                .map(|(effect, l)| (self.effect_id(effect), Expr::Var(l)))
                .collect();
            binds.push(WithBind::new(
                ctx,
                Expr::Provide {
                    base: outer.map(|l| Box::new(Expr::Var(l))),
                    bindings,
                },
            ));
        }
        let ret = self.ty(self.expr_ty(id));
        self.wrap_task_items(binds, body, ret, tasks)
    }

    /// Whether the tasks of a group or a background at an item of the with `id` may fail, and with
    /// them the run-time scope. For a group, the E of the with, which includes that of its tasks
    /// (`[task.group]`); for a background, the E of its tasks, its first type argument
    /// (`[task.background]`).
    pub(super) fn tasks_fail(&self, id: ExprId, ty_args: &[Ty], background: bool) -> bool {
        let with = self.types.errors.get(id).is_some_and(|r| !r.is_empty());
        match ty_args.first() {
            Some(Ty::Set(_, row)) if background => with || !row.is_empty(),
            _ => with,
        }
    }

    /// The type of the argument `on_failure` of `Task.Background` at an item of the with `id`.
    pub(super) fn on_failure_ty(&self, id: ExprId, source: ArgsSource) -> Option<Ty> {
        let ArgsSource::Item(i) = source else {
            return None;
        };
        let arg = self
            .types
            .with_args
            .get(id)?
            .get(i)?
            .first()?
            .as_ref()
            .copied()?;
        Some(self.expr_ty(arg).clone())
    }

    /// The effects of `handler`, a handler of several effects, in the order of its tuple of
    /// handlers (`Ty::Cap`), instantiated with `ty_args` (`[effect.with]`).
    pub(super) fn tuple_tags(&self, handler: DefId, ty_args: &[Ty]) -> Vec<Tag> {
        match Ty::cap(self.handler_tags(handler, ty_args), Row::empty()) {
            Ty::Cap(tags, _) => tags,
            _ => unreachable!("a capability type"),
        }
    }

    /// The position of the handler of `effect` in the tuple of the handlers of `handler`. For a
    /// label (`L by H`), that of its underlying effect (`[effect.label]`).
    pub(super) fn tuple_index(&self, handler: DefId, ty_args: &[Ty], effect: &Tag) -> usize {
        let base = self.types.sigs.base_of(effect);
        self.tuple_tags(handler, ty_args)
            .iter()
            .position(|t| t == effect || t == base)
            .expect("the bind's effect is one of the handler's")
    }

    /// Binds each handler in the tuple `all` of the handlers of `handler`, in order. With
    /// `release` (the handler's name), each is marked released when the `with` ends, as the value
    /// of a handler with release is (`WithBind::managed`); the tuple itself is not a handler.
    pub(super) fn bind_tuple_parts(
        &mut self,
        handler: DefId,
        ty_args: &[Ty],
        all: Local,
        release: Option<&str>,
        binds: &mut Vec<WithBind>,
    ) -> Vec<Local> {
        let tags = self.tuple_tags(handler, ty_args);
        let n = tags.len();
        tags.iter()
            .enumerate()
            .map(|(k, tag)| {
                let id = self.effect_id(&self.key_here(tag));
                let id = self.module.handler_effect(id);
                let ty = self.module.handler(id);
                let part = self.module.local("h", ty);
                let value = super::member::tuple_get(all, k, n, ty, &mut self.module);
                binds.push(match release {
                    Some(name) => WithBind::managed(part, value, name, Expr::Lit(Lit::Unit)),
                    None => WithBind::new(part, value),
                });
                part
            })
            .collect()
    }

    /// The arguments of a handler's init at the `with` `id`, in parameter order. Those written in
    /// the `with` are the locals they were bound to; those written in a layer are computed by
    /// their own functions ([`Self::layer_arg`]).
    pub(super) fn init_args(
        &mut self,
        id: ExprId,
        source: ArgsSource,
        written: &FxHashMap<ExprId, Local>,
    ) -> Vec<Expr> {
        let types = self.types;
        // Lowering runs only without type errors, so every parameter has its argument.
        match source {
            ArgsSource::Item(i) => types
                .with_args
                .get(id)
                .and_then(|items| items.get(i))
                .into_iter()
                .flatten()
                .flatten()
                .map(|arg| Expr::Var(written[arg]))
                .collect(),
            ArgsSource::Layer { layer, member } => types
                .layer_args
                .get(layer)
                .and_then(|members| members.get(member))
                .into_iter()
                .flatten()
                .flatten()
                .map(|&arg| direct(self.layer_arg(layer, arg), Vec::new(), false))
                .collect(),
        }
    }

    /// The IR function computing the argument `arg` of an item of `layer`. The arguments in a
    /// layer are pure (`[effect.layer]`), so each one becomes a const (spec 5.6): evaluated once,
    /// at the first `with` that needs it. Lowering the expression at each `with` instead would
    /// bind the locals in it (those of a lambda, say) more than once.
    pub(super) fn layer_arg(&mut self, layer: DefId, arg: ExprId) -> FnId {
        if let Some(&f) = self.layer_args.get(&arg) {
            return f;
        }
        let name = format!("{}$arg{}", self.ir_name(layer), self.layer_args.len());
        let f = self.module.declare(&name, false);
        self.module.functions[f].constant = true;
        self.layer_args.insert(arg, f);
        self.layer_arg_work.push((layer, arg));
        f
    }

    /// Lowers the body of a function made by [`Self::layer_arg`], with fresh state as in
    /// `define_fn`.
    pub(super) fn define_layer_arg(&mut self, layer: DefId, arg: ExprId) {
        let f = self.layer_args[&arg];
        self.ev = None;
        self.bound.clear();
        self.locals = ArenaMap::default();
        self.local_fns = ArenaMap::default();
        self.fn_name = self.module.functions[f].name.clone();
        self.current_def = Some(layer);
        self.spec = 0;
        let ret = self.ty(self.expr_ty(arg));
        let body = self.expr(arg);
        self.module.define(f, Vec::new(), ret, body);
    }
}
