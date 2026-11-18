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
//! - Calling an operation of a captured capability, or of a parameter of the function, adds the
//!   capability itself (`RowElem::Cap`, `[effect.use]`). It is removed where its root `with`
//!   ends. At the end of the body that binds the variable it is removed if the variable was
//!   bound to `use E` (which has already added E, so removing it loses nothing), and replaced
//!   with its effect otherwise (the variable may hold a capability that no `use E` of the body
//!   made). The parameters of a function stay in its R.
//! - A call replaces each capability element of the callee's R that is one of its parameters
//!   with what the argument is (`[type.rules]`): the capability of the argument variable when it
//!   is itself an element (captured, or a parameter of the caller), and the effect otherwise.
//!   A capability element of another function's parameter that a call cannot replace (the callee
//!   is a function value) is replaced with its effect.
//! - A body's R is contained in the declared R. Without a `uses` clause the declared R is a set
//!   variable, and the body's R becomes its solution as is (spec 9.3). The bodies of `main` and
//!   test functions must have an empty R apart from Async (spec 8.3, 13.3), since the runtime
//!   provides Async.
//! - A handler's dependencies are the union of the R of init, release and the operation bodies
//!   (a set variable). A suspend fn implementation that may call resume later also depends on
//!   Async (spec 8.7, `body/suspend.rs`).

use emela_resolve::hir::{BinaryOp, DefId, ExprId, ExprKind, Item, PatKind, Res, Stmt, WithItem};

use super::{BodyInfer, fn_parts};
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::infer::{Origin, SetKind};
use crate::layer::{self, PendingWith, Supplier, WithBind, WithInfo};
use crate::row::{CapElem, Row, RowElem, Tag};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Computes the R of inferred definition bodies and adds constraints that they are contained
    /// in the declared R.
    pub(super) fn infer_effects(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
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
                    let row = self.end_binder(&row, def, None);
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
                        let row = self.expr_effects(body);
                        let mut row = self.end_binder(&row, def, None);
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
                    let parts: Vec<ExprId> = self
                        .handler_parts
                        .iter()
                        .filter(|p| p.handler == def)
                        .map(|p| p.body)
                        .collect();
                    for body in parts {
                        let row = self.expr_effects(body);
                        let row = self.end_binder(&row, def, None);
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
                Some(Ty::Cap(tag)) => Row::of_tag(tag.clone()),
                _ => Row::empty(),
            },
            // The body's R is enclosed in the lambda type's R; it surfaces when the lambda is
            // called (spec 9.4).
            ExprKind::Lambda { body, .. } => {
                let outer = self.binder.replace(expr);
                let body_row = self.expr_effects(*body);
                self.binder = outer;
                let body_row = self.end_binder_here(&body_row, Some(expr));
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
                        let body_row = self.expr_effects(body);
                        self.binder = outer;
                        let body_row = self.end_binder_here(&body_row, Some(body));
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            body_row,
                            uses,
                            Origin::LocalFn(f.local),
                        );
                    }
                    if let Stmt::Let { pat, value, .. } = stmt
                        && let PatKind::Bind(local) = program.pats[*pat].kind
                        && let ExprKind::Use { .. } = program.exprs[*value].kind
                        && let Some(Ty::Cap(tag)) = self.exprs.get(*value).cloned()
                    {
                        self.use_bound.insert(local);
                        if let Some(with) = self.innermost_with(&tag) {
                            self.cap_roots.insert(local, with);
                        }
                    }
                }
                self.sub_effects(expr)
            }
            ExprKind::With { handlers, body } => self.with_effects(expr, handlers, *body),
            ExprKind::Call { callee, .. } => {
                let mut row = self.sub_effects(expr);
                let uses = self.callee_uses(*callee);
                // The arguments of `x |> f(a)` are recorded on the pipe expression.
                let call = self.piped_calls.get(expr).copied().unwrap_or(expr);
                row.extend(&self.replace_params(call, *callee, &uses));
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
                }
                let mut row = self.sub_effects(expr);
                if !matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    let uses = self.callee_uses(*rhs);
                    row.extend(&self.replace_params(expr, *rhs, &uses));
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

    /// `uses`, the R of the callee of the call `call`, with each capability element that is a
    /// parameter of the callee replaced by what the argument is (`[type.rules]`). For a pipe
    /// `x |> f(a)`, `call` is the pipe expression, where the arguments are recorded, and `callee`
    /// is `f`.
    ///
    /// A capability element of a variable of another definition that is not a parameter of the
    /// callee (a function value whose R names the parameters of the function it refers to) is
    /// replaced with its effect.
    fn replace_params(&mut self, call: ExprId, callee: ExprId, uses: &Row) -> Row {
        let program = self.program;
        let uses = self.ctx.resolve_row(uses);
        if !uses.elems().iter().any(|e| matches!(e, RowElem::Cap(_))) {
            return uses;
        }
        let callee_def = match &program.exprs[callee].kind {
            ExprKind::Path(Res::Def(def)) => Some(*def),
            ExprKind::Field { .. } => self.methods.get(callee).copied(),
            _ => None,
        };
        let params: Vec<emela_resolve::hir::LocalId> = callee_def
            .and_then(|def| fn_parts(program, def))
            .map(|p| p.params.iter().map(|p| p.local).collect())
            .unwrap_or_default();
        let args = self.calls.get(call).cloned().unwrap_or_default();
        let mut out = Row::empty();
        for e in uses.elems() {
            let RowElem::Cap(c) = e else {
                out.insert(e.clone());
                continue;
            };
            if let Some(i) = params.iter().position(|&p| p == c.local) {
                let arg = args.get(i).copied().flatten();
                match arg.and_then(|a| self.named_capability(a)) {
                    Some(local) => out.insert(RowElem::Cap(CapElem {
                        local,
                        name: program.locals[local].name.clone(),
                        tag: c.tag.clone(),
                    })),
                    None => out.insert(RowElem::Tag(c.tag.clone())),
                }
            } else if self.local_owner.get(c.local) == self.current.as_ref() {
                out.insert(e.clone());
            } else {
                out.insert(RowElem::Tag(c.tag.clone()));
            }
        }
        out
    }

    fn callee_uses(&mut self, callee: ExprId) -> Row {
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
        let entries = layer::with_entries(program, self.sigs, self.ty_table, items);
        let provides: Vec<Tag> = entries
            .iter()
            .filter(|e| e.named)
            .map(|e| e.effect.clone())
            .collect();
        // A variable bound to an item is the capability of that item; it cannot leave the
        // `with` (`[effect.use]`). The capability of a handler that provides its effect by name
        // stands for that effect, as one bound to `use E` does.
        for item in items {
            if let Some(local) = item.bind {
                self.cap_roots.insert(local, expr);
                if !layer::is_member_item(program, item) {
                    self.use_bound.insert(local);
                }
            }
        }
        self.with_stack.push((expr, provides.clone()));
        let body_row = self.expr_effects(body);
        self.with_stack.pop();
        layer::check_with_items(program, self.sigs, self.ty_table, items, &mut self.diags);
        let deps: Vec<Row> = entries
            .iter()
            .map(|e| match &e.supplier {
                Supplier::Handler(h) => self.handler_deps(*h),
                Supplier::Effect(t) => Row::of_tag(t.clone()),
                Supplier::Member { .. } => Row::empty(),
            })
            .collect();
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
        let mut row = self.subtract(&body_row, &for_body);
        // The capabilities whose root is this `with` (`[effect.use]`).
        let locals: Vec<_> = self
            .cap_roots
            .iter()
            .filter(|&(_, &with)| with == expr)
            .map(|(local, _)| local)
            .collect();
        let roots: Vec<RowElem> = locals
            .into_iter()
            .filter_map(|l| self.cap_elem(l))
            .collect();
        if !roots.is_empty() {
            row = self.subtract_elems(&row, &roots);
        }
        // Handler dependencies may be met by handlers in the same `with` providing other effects
        // (spec 14.2). The handler's own effect is met from outside. Dependencies a layer leaves
        // unmet also remain (spec 8.5).
        for (entry, deps) in entries.iter().zip(&deps) {
            let others: Vec<Tag> = provided
                .iter()
                .filter(|e| **e != entry.effect)
                .cloned()
                .collect();
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

    /// The innermost enclosing `with` that provides `tag` (`[effect.with]`): `use` inside a
    /// closure looks at the `with`s around it in the same top-level definition.
    fn innermost_with(&mut self, tag: &Tag) -> Option<ExprId> {
        let stack = self.with_stack.clone();
        stack
            .iter()
            .rev()
            .find(|(_, provides)| provides.iter().any(|p| self.ctx.same_tag(p, tag)))
            .map(|(with, _)| *with)
    }

    /// The capability element of a local variable holding a capability value.
    fn cap_elem(&mut self, local: emela_resolve::hir::LocalId) -> Option<RowElem> {
        let ty = self.locals.get(local).cloned()?;
        match self.ctx.resolve(&ty) {
            Ty::Cap(tag) => Some(RowElem::Cap(CapElem {
                local,
                name: self.program.locals[local].name.clone(),
                tag,
            })),
            _ => None,
        }
    }

    /// `row` without the capabilities of the variables bound in `binder` of `def`: their binding
    /// ends with that body (`[effect.use]`).
    fn end_binder(&mut self, row: &Row, def: DefId, binder: Option<ExprId>) -> Row {
        let locals: Vec<_> = self
            .local_binder
            .iter()
            .filter(|&(local, b)| *b == binder && self.local_owner.get(local) == Some(&def))
            .map(|(local, _)| local)
            .filter(|&local| !self.is_fn_param(local))
            .collect();
        // The `use E` that bound a variable has already added E, so its capability is removed.
        // Another variable may hold a capability no `use E` of the body made, so its capability
        // is replaced with its effect.
        let (used, others): (Vec<_>, Vec<_>) =
            locals.into_iter().partition(|l| self.use_bound.contains(l));
        // The `self` of a handler stands for the capabilities in its fields (`self.inner.read`):
        // removed by the variable alone, whatever their effects.
        let used: Vec<RowElem> = used
            .into_iter()
            .filter_map(|l| self.cap_elem(l).or_else(|| self.self_elem(l, def)))
            .collect();
        let others: Vec<RowElem> = others
            .into_iter()
            .filter_map(|l| self.cap_elem(l).or_else(|| self.self_elem(l, def)))
            .collect();
        let mut row = row.clone();
        if !used.is_empty() {
            row = self.subtract_elems(&row, &used);
        }
        if !others.is_empty() {
            row = self.widen_elems(&row, &others);
        }
        row
    }

    /// The element of the `self` of a handler `def`, which stands for the capabilities in its
    /// fields. Only its variable matters when it is removed or widened; each element in a row
    /// keeps its own effect.
    fn self_elem(&self, local: emela_resolve::hir::LocalId, def: DefId) -> Option<RowElem> {
        (self.program.locals[local].kind == emela_resolve::hir::LocalKind::SelfParam).then(|| {
            RowElem::Cap(CapElem {
                local,
                name: self.program.locals[local].name.clone(),
                tag: Tag::new(def),
            })
        })
    }

    /// Replaces the capabilities `caps` in `row` with their effects. Set variables are replaced
    /// with fresh ones whose solutions do the same.
    fn widen_elems(&mut self, row: &Row, caps: &[RowElem]) -> Row {
        let mut out = Row::empty();
        for e in self.ctx.resolve_row(row).elems() {
            match e {
                RowElem::Cap(c)
                    if caps
                        .iter()
                        .any(|m| matches!(m, RowElem::Cap(m) if m.local == c.local)) =>
                {
                    out.insert(RowElem::Tag(c.tag.clone()));
                }
                RowElem::Var(v) => {
                    let d = self.ctx.new_row_var();
                    self.row_widens.push((d, *v, caps.to_vec()));
                    out.insert(RowElem::Var(d));
                }
                e => out.insert(e.clone()),
            }
        }
        out
    }

    /// [`Self::end_binder`] for a closure or local fn of the definition being computed.
    fn end_binder_here(&mut self, row: &Row, binder: Option<ExprId>) -> Row {
        match self.current {
            Some(def) => self.end_binder(row, def, binder),
            None => row.clone(),
        }
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
                RowElem::Cap(c) => minus
                    .iter()
                    .any(|m| matches!(m, RowElem::Cap(m) if m.local == c.local)),
                RowElem::Var(v) => {
                    let d = self.ctx.new_row_var();
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
                RowElem::Var(v) if !tags.is_empty() => {
                    let d = self.ctx.new_row_var();
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
                    effect: entries[i].effect.clone(),
                    supplier: entries[i].supplier.clone(),
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
            let effect = match self.ctx.resolve(&f.ret) {
                Ty::Cap(tag) => tag,
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
            if !is_label_item || entry.from_layer {
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
