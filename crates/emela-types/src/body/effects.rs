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
//! - A body's R is contained in the declared R. Without a `use` clause the declared R is a set
//!   variable, and the body's R becomes its solution as is (spec 9.3). The bodies of `main` and
//!   test functions must have an empty R apart from Async (spec 8.3, 13.3), since the runtime
//!   provides Async.
//! - A handler's dependencies are the union of the R of init, release and the operation bodies
//!   (a set variable). A suspend fn implementation that may call resume later also depends on
//!   Async (spec 8.7, `body/suspend.rs`).

use emela_resolve::hir::{BinaryOp, ExprId, ExprKind, Item, Res, Span};

use super::BodyInfer;
use crate::infer::{Origin, SetKind};
use crate::layer;
use crate::row::{Row, RowElem, Tag};
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Computes the R of inferred definition bodies and adds constraints that they are contained
    /// in the declared R.
    pub(super) fn infer_effects(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
            match program.item(def) {
                Some(Item::Fn(item)) => {
                    let Some(body) = item.body else {
                        continue;
                    };
                    let declared = match self.def_ty(def).map(|ty| self.ctx.shallow_resolve(&ty)) {
                        Some(Ty::Fn(f)) => f.uses,
                        _ => continue,
                    };
                    let row = self.expr_effects(body);
                    let closed = self.closed.contains_key(&def);
                    if closed {
                        // The runtime and the test runner provide Async (spec 8.3, 13.3).
                        let allowed = program
                            .prelude_def("Async")
                            .map_or_else(Row::empty, Row::tag);
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row.clone(),
                            allowed,
                            Origin::Closed(def),
                        );
                    }
                    // When `main` or a test function has a `use` clause, only report that it is
                    // not empty.
                    if !closed || item.sig.uses.is_none() {
                        self.ctx
                            .add_subset_of(SetKind::Effects, row, declared, Origin::Body(def));
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
                        let mut row = self.expr_effects(body);
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
                        self.ctx.add_subset_of(
                            SetKind::Effects,
                            row,
                            deps.clone(),
                            Origin::HandlerPart,
                        );
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
            // `use Async` is an error (E0414) and not added to R.
            ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } if program.prelude_def("Async") == Some(*effect) => Row::empty(),
            // The capability value type's tag (`State[Int]` for effects with type arguments,
            // spec 8.1).
            ExprKind::Use { .. } => match self.exprs.get(expr) {
                Some(Ty::Cap(tag)) => Row::of_tag(tag.clone()),
                _ => Row::empty(),
            },
            // The body's R is enclosed in the lambda type's R; it surfaces when the lambda is
            // called (spec 9.4).
            ExprKind::Lambda { body, .. } => {
                let body_row = self.expr_effects(*body);
                if let Some(&(_, v)) = self.lambda_rows.get(expr) {
                    self.ctx
                        .add_subset_of(SetKind::Effects, body_row, Row::var(v), Origin::Lambda);
                }
                Row::empty()
            }
            ExprKind::With { handlers, body } => self.with_effects(expr, handlers, *body),
            ExprKind::Call { callee, .. } => {
                let mut row = self.sub_effects(expr);
                row.extend(&self.callee_uses(*callee));
                row
            }
            // `x |> f(a)` is `f(x, a)`. If the right side is a call, that call expression adds
            // the callee's R.
            ExprKind::Binary {
                op: BinaryOp::Pipe,
                rhs,
                ..
            } => {
                let mut row = self.sub_effects(expr);
                if !matches!(program.exprs[*rhs].kind, ExprKind::Call { .. }) {
                    row.extend(&self.callee_uses(*rhs));
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

    fn callee_uses(&mut self, callee: ExprId) -> Row {
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        match self.ctx.shallow_resolve(&ty) {
            Ty::Fn(f) => f.uses,
            _ => Row::empty(),
        }
    }

    /// The R of `with H1, ..., Hn { body }` (spec 8.3, 8.5, 9.4). Layers are expanded, and for
    /// handlers of the same effect the later one is kept (`crate::layer::with_entries`).
    fn with_effects(&mut self, expr: ExprId, handlers: &[(Res, Span)], body: ExprId) -> Row {
        let program = self.program;
        let body_row = self.expr_effects(body);
        let entries = layer::with_entries(program, self.sigs, handlers);
        let deps: Vec<Row> = entries
            .iter()
            .map(|e| self.handler_deps(e.handler))
            .collect();
        let provided: Vec<Tag> = entries.iter().map(|e| e.effect.clone()).collect();
        let mut row = self.subtract(&body_row, &provided);
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
        self.withs.push((expr, entries, deps));
        row
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
                    self.row_diffs.push((d, *v, tags.to_vec()));
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
        for (expr, entries, deps) in std::mem::take(&mut self.withs) {
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
            self.with_handlers
                .insert(expr, order.iter().map(|&i| entries[i].handler).collect());
        }
    }
}
