//! Inference of effect sets R (spec chapter 8, 9.4).
//!
//! As with E, after expression types are inferred the HIR is walked again to compute each
//! expression's R. Constraints go into the same table as E and are solved together in
//! `body/errors.rs`.
//!
//! - The R of `use X` is `{X}`. Even inside a branch it lands in the whole function's R, since
//!   sub-expressions' R flows into the parent (spec 8.2).
//! - A call adds the callee function type's R. A capability operation's type has that effect as
//!   its R.
//! - The R of `with H1, ..., Hn { e }` is `e`'s R minus the provided effects, plus each handler's
//!   dependencies (spec 9.4). Layers expand into their handlers (spec 8.5, `crate::layer`). A
//!   handler dependency met by a handler in the same `with` providing another effect is not
//!   added (spec 14.2). Effects are removed from set variables by replacing them with fresh set
//!   variables whose solutions exclude those effects; nothing is removed from set type
//!   parameters (sets with unknown contents).
//! - A lambda encloses its body's R in its function type.
//! - Calling an operation of a capability adds the capabilities its type records (`RowElem::Cap`,
//!   set variables and the set type parameters of capability parameters, `[effect.use]`). The
//!   `with` that made a capability removes it. A capability parameter's set stays in the R of
//!   its function, and a call puts the argument's set in its place by instantiation.
//! - A closure or a local fn also has in its R the capabilities in the types of the values it
//!   captures (`[effect.use]`).
//! - The `self` of a handler ([`crate::row::CapId::SelfOf`]) is removed at the end of the body
//!   of its operation, init or release.
//! - A body's R is contained in the declared R. Without a `uses` clause the declared R is a set
//!   variable, and the body's R becomes its solution as is (spec 9.3). The bodies of `main` and
//!   test functions must have an empty R apart from Async (spec 8.3, 13.3), since the runtime
//!   provides Async.
//! - A handler's dependencies are the union of the R of init, release and the operation bodies
//!   (a set variable). A suspend fn implementation that may call resume later also depends on
//!   Async (spec 8.7, `body/suspend.rs`).

use emela_resolve::hir::{BinaryOp, DefId, ExprId, ExprKind, Item, Res, Stmt, WithItem};

use super::{BodyInfer, fn_parts};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::{Origin, SetKind};
use crate::layer::{self, PendingWith, Supplier, WithBind, WithInfo};
use crate::row::{CapElem, CapId, Row, RowElem, Tag};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Computes the R of inferred definition bodies and adds constraints that they are contained
    /// in the declared R.
    pub(super) fn infer_effects(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
            self.ctx.scopes.reset();
            match program.item(def) {
                Some(Item::Fn(_) | Item::TraitFn(_)) | None => {
                    let Some(parts) = fn_parts(program, def) else {
                        continue;
                    };
                    let Some(body) = parts.body else {
                        continue;
                    };
                    let declared = match self.def_ty(def).map(|ty| self.ctx.shallow_resolve(&ty)) {
                        Some(Ty::Fn(f)) => f.uses,
                        _ => continue,
                    };
                    let row = self.expr_effects(body);
                    let closed = self.closed.get(&def);
                    if let Some(context) = closed {
                        // The entry provides its host effects (`[effect.host]`): Async and
                        // ProcessHost for main and tests, Async for an `@export` fn, and what an
                        // entry adapter declares.
                        let mut allowed = Row::empty();
                        for name in context.provided_hosts() {
                            if let Some(host) = program.prelude_def(&name) {
                                allowed.insert(RowElem::Tag(Tag::new(host)));
                            }
                        }
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row.clone(),
                            allowed,
                            Origin::Closed(def),
                        );
                    }
                    // When `main` or a test function has a `uses` clause, only report that it is
                    // not empty.
                    if closed.is_none() || parts.sig.is_none_or(|sig| sig.uses.is_none()) {
                        self.ctx
                            .add_subset_of(SetKind::Effects, row, declared, Origin::Body(def));
                    }
                }
                // The arguments of a layer's items cannot have effects (`[effect.layer]`).
                Some(Item::Layer(item)) => {
                    for (member, m) in item.members.iter().enumerate() {
                        for arg in m.args.iter().flat_map(|a| &a.args) {
                            let row = self.expr_effects(arg.value);
                            let origin = Origin::LayerArg {
                                layer: def,
                                member,
                                arg: arg.value,
                            };
                            self.ctx
                                .add_subset_of(SetKind::Effects, row, Row::empty(), origin);
                        }
                    }
                }
                // A const's value cannot have effects (spec 5.6).
                Some(Item::Const(item)) => {
                    let row = self.expr_effects(item.value);
                    self.ctx
                        .add_subset_of(SetKind::Effects, row, Row::empty(), Origin::Body(def));
                }
                Some(Item::Handler(_)) => {
                    let deps = self.handler_deps(def);
                    let bodies: Vec<(usize, ExprId, bool)> = self
                        .handler_bodies
                        .iter()
                        .filter(|b| b.handler == def)
                        .map(|b| (b.index, b.body, b.resume_later))
                        .collect();
                    for (index, body, resume_later) in bodies {
                        self.ctx.scopes.enter(body);
                        let row = self.expr_effects(body);
                        self.ctx.scopes.exit();
                        let mut row = self.end_self(&row, def, true);
                        // The capability parameters end with the body (`[effect.handler]`).
                        if let Some(rigid) = self.quantified_lambdas.get(&body).cloned() {
                            let minus: Vec<RowElem> = rigid.into_iter().map(RowElem::Cap).collect();
                            row = self.subtract_elems(&row, &minus);
                        }
                        // A suspend fn implementation that may call resume later requires Async
                        // (spec 8.7).
                        if resume_later && let Some(async_) = program.prelude_def("Async") {
                            row.insert(RowElem::Tag(Tag::new(async_)));
                        }
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row,
                            deps.clone(),
                            Origin::HandlerOp {
                                handler: def,
                                index,
                            },
                        );
                    }
                    // The R of init and release bodies are dependencies too (spec 9.4). release
                    // may call suspend operations (spec 8.4).
                    let parts: Vec<(ExprId, bool)> = self
                        .handler_parts
                        .iter()
                        .filter(|p| p.handler == def)
                        .map(|p| (p.body, p.release))
                        .collect();
                    for (body, release) in parts {
                        let row = self.expr_effects(body);
                        let row = self.end_self(&row, def, release);
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row,
                            deps.clone(),
                            Origin::HandlerPart,
                        );
                    }
                    // The requirements of the inner handlers are the outer handler's; they are
                    // provided where the outer handler is (`[effect.handler]`).
                    if let Some(Item::Handler(item)) = program.item(def) {
                        for &inner in &item.inner {
                            let row = self.handler_deps(inner);
                            self.ctx.add_subset_of(
                                SetKind::Effects,
                                row,
                                deps.clone(),
                                Origin::HandlerPart,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Sub-expressions' R flows into the parent (spec 9.4). Records the computed R per
    /// expression.
    fn expr_effects(&mut self, expr: ExprId) -> Row {
        let program = self.program;
        let row = match &program.exprs[expr].kind {
            // `use` of a host effect is an error (E0414) and not added to R.
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } if program.is_host_effect(*effect) => Row::empty(),
            // The capability value type's tag (`State[Int]` for effects with type arguments,
            // spec 8.1).
            ExprKind::Use { .. } => match self.exprs.get(expr) {
                Some(Ty::Cap(tags, _)) => Row::from_elems(tags.iter().cloned().map(RowElem::Tag)),
                _ => Row::empty(),
            },
            // The body's R is enclosed in the lambda type's R; it surfaces when the lambda is
            // called (spec 9.4). The `with`s around the lambda do not provide it
            // (`[effect.with]`), so `use E` inside has no root outside.
            ExprKind::Lambda { body, .. } => {
                let outer = self.binder.replace(expr);
                let withs = std::mem::take(&mut self.with_stack);
                self.ctx.scopes.enter(expr);
                let mut body_row = self.expr_effects(*body);
                self.ctx.scopes.exit();
                self.with_stack = withs;
                self.binder = outer;
                body_row.extend(&self.captured_caps(expr));
                // The unknown sets of a closure checked against a quantified function type are
                // in the R of that type; the side that calls it provides them (`[type.rules]`).
                if let Some(rigid) = self.quantified_lambdas.get(&expr).cloned() {
                    let rigid: Vec<RowElem> = rigid.into_iter().map(RowElem::Cap).collect();
                    body_row = self.subtract_elems(&body_row, &rigid);
                }
                if let Some(&(_, v)) = self.lambda_rows.get(expr) {
                    self.ctx
                        .add_subset_of(SetKind::Effects, body_row, Row::var(v), Origin::Lambda);
                }
                Row::empty()
            }
            // The body's R of a local fn is enclosed in its type's R, as for a lambda
            // (`[fn.local]`). The block's R does not include it (`sub_exprs`).
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    if let Stmt::LocalFn(f) = stmt
                        && let Some(info) = self.local_fns.get(f.local)
                    {
                        let (body, uses) = (info.body, info.uses.clone());
                        let outer = self.binder.replace(body);
                        let withs = std::mem::take(&mut self.with_stack);
                        self.ctx.scopes.enter(body);
                        let mut body_row = self.expr_effects(body);
                        self.ctx.scopes.exit();
                        self.with_stack = withs;
                        self.binder = outer;
                        body_row.extend(&self.captured_caps(body));
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            body_row,
                            uses,
                            Origin::LocalFn(f.local),
                        );
                    }
                }
                self.sub_effects(expr)
            }
            ExprKind::With { handlers, body } => self.with_effects(expr, handlers, *body),
            // `x.op` without a call, through an effect alias or `(use E)`: a function value that
            // resolves E when it is called (`[effect.use]`). E is in the R of its type, not in
            // the R of the function that makes it.
            ExprKind::Field { base, .. }
                if self.ops.get(expr).is_some()
                    && program.alias_uses.get(*base).is_some()
                    && !self.called.contains(&expr) =>
            {
                Row::empty()
            }
            ExprKind::Call { callee, .. } => {
                self.called.insert(*callee);
                let mut row = self.sub_effects(expr);
                let uses = self.callee_uses(*callee);
                row.extend(&uses);
                row
            }
            // `x |> f(a)` is `f(x, a)`. If the right side is a call, that call expression adds
            // the callee's R.
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                rhs,
                ..
            } => {
                if matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    self.piped_calls.insert(*rhs, expr);
                } else {
                    self.called.insert(*rhs);
                }
                let mut row = self.sub_effects(expr);
                if !matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    let uses = self.callee_uses(*rhs);
                    row.extend(&uses);
                }
                row
            }
            _ => self.sub_effects(expr),
        };
        self.expr_effects.insert(expr, row.clone());
        row
    }

    fn sub_effects(&mut self, expr: ExprId) -> Row {
        let mut row = Row::empty();
        for e in self.sub_exprs(expr) {
            row.extend(&self.expr_effects(e));
        }
        row
    }

    pub(super) fn callee_uses(&mut self, callee: ExprId) -> Row {
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        match self.ctx.shallow_resolve(&ty) {
            Ty::Fn(f) => f.uses,
            _ => Row::empty(),
        }
    }

    /// The R of `with H1, ..., Hn { body }` (spec 8.3, 8.5, 9.4). Layers are expanded, and for
    /// providers of the same effect the later one is kept (`crate::layer::with_entries`).
    ///
    /// A label `L` listed alone provides its base and depends on `L`; `L by P` provides `L` and
    /// depends on what `P` depends on (a handler's dependencies, or the effect `P`). A provider of
    /// an effect also provides the effect's labels declared `default`, unless the same `with`
    /// gives them a provider; those are removed from the body's R only, not from the other
    /// providers' dependencies (issue #255).
    ///
    /// The arguments of init are evaluated outside the `with`, so their R is added as is: the
    /// items of the same `with` do not provide it. The R of the arguments of a handler a later
    /// item overrides is not added, since they are not evaluated (`[effect.with]`).
    fn with_effects(&mut self, expr: ExprId, items: &[WithItem], body: ExprId) -> Row {
        let program = self.program;
        let arg_rows: Vec<Vec<Row>> = items
            .iter()
            .map(|item| {
                layer::item_args(item)
                    .map(|arg| self.expr_effects(arg))
                    .collect()
            })
            .collect();
        let insts = self.item_insts(expr, items);
        let entries = layer::with_entries(program, self.sigs, self.ty_table, items, &insts);
        let provides: Vec<Tag> = entries
            .iter()
            .filter(|e| e.named)
            .map(|e| e.effect.clone())
            .collect();
        self.with_stack.push((expr, provides.clone()));
        self.ctx.scopes.enter(expr);
        let body_row = self.expr_effects(body);
        self.ctx.scopes.exit();
        self.with_stack.pop();
        layer::check_with_items(
            program,
            self.sigs,
            self.ty_table,
            items,
            &insts,
            &mut self.diags,
        );
        // requires(H) instantiated with the type arguments of the item (`[type.rules]`).
        let deps: Vec<Row> = entries
            .iter()
            .map(|e| match &e.supplier {
                Supplier::Handler(h) => {
                    if let Some(row) = self.builtin_handler_sets(*h, &e.ty_args, SetKind::Effects) {
                        return row;
                    }
                    let deps = self.handler_deps(*h);
                    self.instantiate_facts(deps, &e.ty_args, SetKind::Effects)
                }
                Supplier::Effect(t) => Row::of_tag(t.clone()),
                Supplier::Member { .. } => Row::empty(),
            })
            .collect();
        self.check_several_fits(expr, &entries, &body_row, &deps);
        let provided = provides;
        let mut defaults: Vec<(Tag, usize)> = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if !entry.named {
                continue;
            }
            for label in self.sigs.default_labels(&entry.effect) {
                let tag = Tag::new(label);
                if !provided.contains(&tag) && !defaults.iter().any(|(t, _)| *t == tag) {
                    defaults.push((tag, i));
                }
            }
        }
        let mut for_body = provided.clone();
        for_body.extend(defaults.iter().map(|(t, _)| t.clone()));
        // The effects it provides, and the capabilities it made (`[effect.with]`), are removed
        // at once: a set variable made outside the `with` cannot have its capabilities, even on
        // the way (`[effect.escape]`).
        let mut minus: Vec<RowElem> = for_body.iter().cloned().map(RowElem::Tag).collect();
        // The handler members that an init opened through this `with`'s handlers end with it too
        // (`[effect.handler]`, [`crate::row::OPENED_IN_INIT`]).
        minus.extend(
            (0..items.len() as u32)
                .chain([crate::row::OPENED_IN_INIT])
                .map(|i| {
                    RowElem::Cap(CapElem {
                        id: CapId::Item {
                            with: expr,
                            item: i,
                        },
                        name: Default::default(),
                        tag: Tag::new(self.current.expect("a `with` is inside a definition")),
                    })
                }),
        );
        let mut row = self.subtract_elems(&body_row, &minus);
        // Handler dependencies may be met by handlers in the same `with` providing other effects
        // (spec 14.2). The handler's own effects, all those of a handler of several effects, are
        // met from outside (`[effect.handler]`). Dependencies a layer leaves unmet also remain
        // (spec 8.5).
        for (entry, deps) in entries.iter().zip(&deps) {
            let others: Vec<Tag> = provided
                .iter()
                .filter(|e| !entry.own.contains(e))
                .cloned()
                .collect();
            // The tasks of a built-in handler may use the capabilities of the items before it, and
            // its own to start siblings (`[task.group]`); they are met in the same `with`. Those
            // of the items after it are removed too, so that they do not leave through the R of
            // the `with`: the set type argument of the item reports them (`check_task_items`).
            if let Supplier::Handler(h) = &entry.supplier
                && self
                    .builtin_handler_sets(*h, &entry.ty_args, SetKind::Effects)
                    .is_some()
            {
                let mut minus: Vec<RowElem> = others.into_iter().map(RowElem::Tag).collect();
                minus.extend((0..items.len() as u32).map(|i| {
                    RowElem::Cap(CapElem {
                        id: CapId::Item {
                            with: expr,
                            item: i,
                        },
                        name: Default::default(),
                        tag: Tag::new(self.current.expect("a `with` is inside a definition")),
                    })
                }));
                // A capability the tasks cannot have is reported at the item.
                let hint = self.ctx.hint();
                self.ctx.set_hint(Some(entry.span));
                let deps = self.subtract_elems(deps, &minus);
                self.ctx.set_hint(hint);
                row.extend(&deps);
                continue;
            }
            row.extend(&self.subtract(deps, &others));
        }
        // An item that opens a handler member requires the effect that declares it, or the
        // capability it is opened through; a provider of that effect in the same `with` meets it
        // (`[effect.with]`, `[dyn.with]`).
        for (_, f) in self.with_members.get(expr).cloned().unwrap_or_default() {
            row.extend(&self.subtract(&f.uses, &provided));
        }
        let initialized: Vec<bool> = (0..items.len())
            .map(|i| layer::item_evaluated(program, &entries, items, i))
            .collect();
        for (rows, &init) in arg_rows.iter().zip(&initialized) {
            if init {
                for r in rows {
                    row.extend(r);
                }
            }
        }
        self.withs.push(PendingWith {
            expr,
            body,
            entries,
            deps,
            defaults,
            initialized,
        });
        row
    }

    /// `row` without the `self` of the handler `def` ([`CapId::SelfOf`]): it ends with the body
    /// of the operation, the init or the release (`[effect.handler]`). With `fields`, in an
    /// operation body or the release, also without the sets of its fields (`Bound(i)`,
    /// [`BodyInfer::field_sets`]): the capabilities they hold belong to a with around the
    /// handler's, so they are not requirements of the handler, also when they are passed to an
    /// operation or recorded in a value (#866). In init, `Bound(i)` are the sets of init's
    /// parameters.
    fn end_self(&mut self, row: &Row, def: DefId, fields: bool) -> Row {
        let mut minus = vec![RowElem::Cap(CapElem {
            id: CapId::SelfOf(def),
            name: Default::default(),
            tag: Tag::new(def),
        })];
        if fields {
            minus.extend(self.field_sets(def));
        }
        self.subtract_elems(row, &minus)
    }

    /// The capabilities in the types of the values that the closure or local fn `binder`
    /// captures (`[effect.use]`): they are in its R. A captured `self` is the capability of its
    /// handler.
    fn captured_caps(&mut self, binder: ExprId) -> Row {
        let program = self.program;
        let mut out = Row::empty();
        for local in self.captured_locals(binder) {
            if program.locals[local].kind == emela_resolve::hir::LocalKind::SelfParam
                && let Some(def) = self.current
                && matches!(program.item(def), Some(Item::Handler(_)))
            {
                out.insert(RowElem::Cap(CapElem {
                    id: CapId::SelfOf(def),
                    name: "self".into(),
                    tag: Tag::new(def),
                }));
                continue;
            }
            let Some(ty) = self.locals.get(local).cloned() else {
                continue;
            };
            let ty = self.ctx.resolve(&ty);
            caps_in(&ty, &mut out);
        }
        out
    }

    /// The local variables referred to inside `binder` (the expression of a closure, or the body
    /// of a local fn) that are bound outside it.
    fn captured_locals(&self, binder: ExprId) -> Vec<emela_resolve::hir::LocalId> {
        let program = self.program;
        let span = program.exprs[binder].span;
        // Bound in `binder` or in a closure or local fn inside it.
        let bound_inside = |local: emela_resolve::hir::LocalId| match self.local_binder.get(local) {
            Some(Some(b)) => {
                *b == binder || {
                    let s = program.exprs[*b].span;
                    s.module == span.module && span.range.contains_range(s.range)
                }
            }
            _ => false,
        };
        let mut out = Vec::new();
        let mut stack = vec![binder];
        while let Some(e) = stack.pop() {
            if let ExprKind::Path(Res::Local(local)) = program.exprs[e].kind
                && !bound_inside(local)
                && !out.contains(&local)
            {
                out.push(local);
            }
            stack.extend(self.sub_exprs(e));
            match &program.exprs[e].kind {
                ExprKind::Lambda { body, .. } => stack.push(*body),
                ExprKind::Block(block) => {
                    for stmt in &block.stmts {
                        if let Stmt::LocalFn(f) = stmt {
                            stack.push(f.body);
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Removes tags and capabilities from `row`, as [`Self::subtract`] does tags. A tag removes
    /// only the same tag, and a capability only the same variable.
    fn subtract_elems(&mut self, row: &Row, minus: &[RowElem]) -> Row {
        let mut out = Row::empty();
        for e in self.ctx.resolve_row(row).elems() {
            let removed = match e {
                RowElem::Tag(t) => minus
                    .iter()
                    .any(|m| matches!(m, RowElem::Tag(m) if self.ctx.same_tag(m, t))),
                RowElem::Cap(c) => minus.iter().any(|m| match m {
                    RowElem::Cap(m) => m.same(c),
                    // In R, the name of an effect is the effect (`[type.rules]`).
                    RowElem::Tag(m) => {
                        matches!(c.id, CapId::Named { .. }) && self.ctx.same_tag(m, &c.tag)
                    }
                    _ => false,
                }),
                RowElem::Bound(_) => minus.contains(e),
                RowElem::Var(v) => {
                    let d = self.ctx.new_row_var();
                    self.ctx.effect_vars.insert(d);
                    self.row_diffs.push((d, *v, minus.to_vec()));
                    out.insert(RowElem::Var(d));
                    continue;
                }
                _ => false,
            };
            if !removed {
                out.insert(e.clone());
            }
        }
        out
    }

    /// Removes the tags `tags` from `row`. Inference set variables are replaced with fresh ones
    /// whose solutions exclude those tags; nothing is removed from set type parameters. Effects
    /// with type arguments are removed only with matching type arguments (spec 8.1); undetermined
    /// type arguments are unified first (`InferCtx::same_tag`).
    fn subtract(&mut self, row: &Row, tags: &[Tag]) -> Row {
        let mut out = Row::empty();
        for e in self.ctx.resolve_row(row).elems() {
            match e {
                RowElem::Tag(t) if tags.iter().any(|p| self.ctx.same_tag(p, t)) => {}
                // In R, the name of an effect is the effect (`[type.rules]`).
                RowElem::Cap(c)
                    if matches!(c.id, CapId::Named { .. })
                        && tags.iter().any(|p| self.ctx.same_tag(p, &c.tag)) => {}
                RowElem::Var(v) if !tags.is_empty() => {
                    let d = self.ctx.new_row_var();
                    self.ctx.effect_vars.insert(d);
                    let minus = tags.iter().map(|t| RowElem::Tag(t.clone())).collect();
                    self.row_diffs.push((d, *v, minus));
                    out.insert(RowElem::Var(d));
                }
                e => out.insert(e.clone()),
            }
        }
        out
    }

    /// Decides the initialization order of handlers listed in the same `with` (spec 14.2); a
    /// dependency cycle is E0411. Checked with the solved dependencies after sets are solved.
    /// Cycles only inside a layer are reported at the layer declaration.
    pub(super) fn check_handler_cycles(&mut self) {
        let program = self.program;
        for w in std::mem::take(&mut self.withs) {
            let PendingWith {
                expr,
                body,
                entries,
                deps,
                defaults,
                initialized,
            } = w;
            let deps: Vec<Row> = deps.iter().map(|d| self.ctx.resolve_row(d)).collect();
            let order = match layer::init_order(&entries, &deps) {
                Ok(order) => order,
                Err(cycle) => {
                    if !layer::same_inner_layer(&entries, &cycle) {
                        self.diags
                            .push(layer::cycle_diagnostic(program, &entries, &cycle));
                    }
                    (0..entries.len()).collect()
                }
            };
            // Only labels in the body's solved R are bound. A label left in a set the body
            // cannot see (`f()` with `f`'s R a set parameter) is provided from outside, as the
            // type says (spec 8.3).
            let body_row = self
                .expr_effects
                .get(body)
                .map(|r| self.ctx.resolve_row(r))
                .unwrap_or_default();
            // An item that opens a handler member of a label uses the label (`[effect.label]`).
            let mut used = body_row.clone();
            for (_, f) in self.with_members.get(expr).cloned().unwrap_or_default() {
                used.extend(&self.ctx.resolve_row(&f.uses));
            }
            self.unused_label_providers(&entries, &deps, &used);
            let defaults = defaults
                .into_iter()
                .filter(|(t, _)| body_row.contains(&RowElem::Tag(t.clone())))
                .filter_map(|(t, i)| Some((t, order.iter().position(|&j| j == i)?)))
                .collect();
            let mut binds: Vec<WithBind> = order
                .iter()
                .map(|&i| WithBind {
                    effect: entries[i].effect.map_args(|a| self.ctx.resolve(a)),
                    supplier: entries[i].supplier.clone(),
                    ty_args: entries[i]
                        .ty_args
                        .iter()
                        .map(|t| self.ctx.resolve(t))
                        .collect(),
                    args: entries[i].args,
                    named: entries[i].named,
                })
                .collect();
            self.place_member_items(expr, &entries, &order, &mut binds);
            self.with_handlers.insert(
                expr,
                WithInfo {
                    binds,
                    defaults,
                    initialized,
                },
            );
        }
    }

    /// Puts the items of the `with` `expr` that open handler members into `binds`, the providers
    /// in initialization order (`[dyn.with]`). An item that opens a member of an effect another
    /// item of the same `with` provides comes after that item; otherwise items keep the order
    /// they are written in.
    fn place_member_items(
        &mut self,
        expr: ExprId,
        entries: &[layer::Entry],
        order: &[usize],
        binds: &mut Vec<WithBind>,
    ) {
        let program = self.program;
        let Some(opened) = self.with_members.get(expr).cloned() else {
            return;
        };
        let ExprKind::With {
            handlers: items, ..
        } = &program.exprs[expr].kind
        else {
            return;
        };
        let mut members: Vec<(usize, WithBind, Option<usize>)> = Vec::new();
        for (item, f) in opened {
            let Some(member) = self.member_item(&items[item]) else {
                continue;
            };
            // For an item that opens a handler member, the effect only names the capability it
            // binds; with several effects, the first (its local has the whole type).
            let effect = match self.ctx.resolve(&f.ret) {
                Ty::Cap(tags, _) if !tags.is_empty() => tags[0].clone(),
                _ => continue,
            };
            let (provider, after) = match &items[item].cap {
                Some((cap, _)) => (layer::MemberProvider::Cap(*cap), None),
                None => {
                    let uses = self.ctx.resolve_row(&f.uses);
                    let Some(tag) = uses.effect_tags().next().cloned() else {
                        continue;
                    };
                    // The provider of the effect in the same `with`, by its place in `order`.
                    let after = order.iter().position(|&i| {
                        entries[i].named && self.ctx.same_tag(&entries[i].effect, &tag)
                    });
                    (layer::MemberProvider::Effect(tag), after)
                }
            };
            members.push((
                item,
                WithBind {
                    effect,
                    supplier: Supplier::Member {
                        member,
                        provider,
                        item,
                    },
                    ty_args: Vec::new(),
                    args: layer::ArgsSource::Item(item),
                    named: false,
                },
                after,
            ));
        }
        // Merge: take the earlier written of the next provider and the first member item whose
        // provider is placed.
        let providers = std::mem::take(binds);
        let origins: Vec<usize> = order.iter().map(|&i| entries[i].origin).collect();
        let mut next = 0;
        while next < providers.len() || !members.is_empty() {
            let ready = members
                .iter()
                .position(|(_, _, after)| after.is_none_or(|a| a < next));
            let take_member = match (ready, origins.get(next)) {
                (Some(m), Some(&origin)) => members[m].0 < origin,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if take_member {
                let (_, bind, _) = members.remove(ready.expect("a member item is ready"));
                binds.push(bind);
            } else {
                binds.push(providers[next].clone());
                next += 1;
            }
        }
    }

    /// W0401 for label items of a `with` (`with L`, `L by P`, issue #255) whose effect neither
    /// the body nor another provider of the same `with` uses. Only when the body's R is fully
    /// known: a set with unknown contents (`f()` with `f`'s R a set parameter) may use it.
    fn unused_label_providers(&mut self, entries: &[layer::Entry], deps: &[Row], body: &Row) {
        if body.elems().iter().any(|e| !matches!(e, RowElem::Tag(_))) {
            return;
        }
        let program = self.program;
        for (i, entry) in entries.iter().enumerate() {
            let is_label_item = matches!(entry.supplier, Supplier::Effect(_))
                || self.sigs.label_base(entry.effect.def).is_some();
            // A bound label item provides nothing by name, so the label never appears in the R
            // of the body; it is used through its capability (`[effect.label]`).
            if !is_label_item || entry.from_layer || !entry.named {
                continue;
            }
            let tag = RowElem::Tag(entry.effect.clone());
            let used = body.contains(&tag)
                || deps
                    .iter()
                    .enumerate()
                    .any(|(j, d)| j != i && entries[j].effect != entry.effect && d.contains(&tag));
            if !used {
                // `with L` provides the base from `L`; `L by P` provides `L`.
                let label = match &entry.supplier {
                    Supplier::Effect(t) if self.sigs.label_base(entry.effect.def).is_none() => {
                        t.def
                    }
                    _ => entry.effect.def,
                };
                self.diags.push(Diagnostic {
                    span: Some(entry.span),
                    kind: DiagnosticKind::UnusedLabelProvider {
                        label: program.defs[label].name.clone(),
                    },
                });
            }
        }
    }
}

/// Adds the capabilities that `ty` records to `out` (`[effect.use]`): the sets of its capability
/// types and capability-bound types, and the capabilities in the R of its function types. In R,
/// the name of an effect is the effect.
fn caps_in(ty: &Ty, out: &mut Row) {
    match ty {
        Ty::Cap(_, caps) | Ty::Set(SetKind::Caps, caps) => out.extend(&caps.names_as_effects()),
        Ty::Fn(f) => {
            for c in f.uses.caps() {
                out.insert(RowElem::Cap(c.clone()));
            }
        }
        _ => {}
    }
    ty.for_each_child(|child| caps_in(child, out));
}
