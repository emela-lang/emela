//! Inference of error sets E (spec chapter 7, 9.4).
//!
//! After expression types are inferred, the HIR is walked again to compute each expression's E.
//! A call's E is the callee function type's E, so this is easier once types are settled.
//!
//! E is determined by the inclusion constraints collected during unification (when a function
//! value is placed where a function type is expected) and the ones added here (a definition
//! body's E ⊆ the declared E, a lambda body's E ⊆ the lambda type's E). Lower bounds are
//! collected per set variable, the least solution is computed, and then every constraint is
//! checked.
//!
//! - If `sup` of a constraint `sub ⊆ sup` has a set variable, the elements of `sub` not in `sup`
//!   become lower bounds of the first set variable in `sup`, minus the tags of `sup` (passing a
//!   function to a parameter of type `fails Timeout | E` puts everything but Timeout into `E`,
//!   spec 7.4).
//! - `escape` removes from the inner expression's E the tags whose values the arms fully catch.
//!   Set variables are replaced with fresh set variables whose solutions exclude those tags.
//!   Nothing is removed from set type parameters (sets with unknown contents) (spec 7.3).
//! - In the type of a definition being generalized, a set variable in a negative position (the E
//!   of a function-typed parameter) with neither lower nor upper bounds is generalized into a set
//!   type parameter instead of becoming empty. Other set variables take the least solution.
//!   A negative-position set variable that does have a lower bound (`fn(g) { if c { g } else
//!   { f } }`) is not generalized, so the type is not the most general one (an approximation).
//!
//! Constraints on effect sets R (`body/effects.rs`) go into the same table and are solved here
//! together. E and R get separate set variables, so they never mix.

use crate::sparse::SparseMap;
use emela_resolve::hir::{
    Arm, Block, DefId, DefKind, ExprId, ExprKind, Item, PatId, PatKind, Res, Span, Stmt, StrPart,
};
use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;

use super::{BodyInfer, FnParts, fn_parts};
use crate::coverage::Shapes;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::diagnostic::{EffectContext, LayerArg};
use crate::infer::{Origin, SetKind, Subset};
use crate::row::{Row, RowElem, RowVar, Tag};
use crate::ty::{Ty, TyParam};

/// One lower bound of a set variable: the elements of `elem` minus the tags in `minus`.
struct Lower {
    elem: RowElem,
    minus: Vec<Tag>,
}

impl BodyInfer<'_> {
    /// Computes the E of inferred definition bodies and adds constraints that they are contained
    /// in the declared E.
    pub(super) fn infer_errors(&mut self) {
        let program = self.program;
        for def in self.defs.clone() {
            self.current = Some(def);
            let (body, declared) = match program.item(def) {
                Some(Item::Fn(_) | Item::TraitFn(_)) | None => {
                    let Some(body) = fn_parts(program, def).and_then(|p| p.body) else {
                        continue;
                    };
                    let declared = match self.def_ty(def).map(|ty| self.ctx.shallow_resolve(&ty)) {
                        Some(Ty::Fn(f)) => f.fails,
                        _ => continue,
                    };
                    (body, declared)
                }
                // A const's value cannot fail (spec 5.6).
                Some(Item::Const(item)) => (item.value, Row::empty()),
                // A handler operation body's E must not exceed the operation's declared E
                // (spec 8.1).
                Some(Item::Handler(_)) => {
                    let bodies: Vec<(usize, ExprId, Row)> = self
                        .handler_bodies
                        .iter()
                        .filter(|b| b.handler == def)
                        .map(|b| (b.index, b.body, b.fails.clone()))
                        .collect();
                    for (index, body, fails) in bodies {
                        let row = self.expr_row(body);
                        self.ctx.add_subset(
                            row,
                            fails,
                            Origin::HandlerOp {
                                handler: def,
                                index,
                            },
                        );
                    }
                    // init's E goes into the E of `with` (spec 9.4); release's E is empty
                    // (spec 8.4).
                    let parts: Vec<(ExprId, bool)> = self
                        .handler_parts
                        .iter()
                        .filter(|p| p.handler == def)
                        .map(|p| (p.body, p.release))
                        .collect();
                    for (body, release) in parts {
                        let row = self.expr_row(body);
                        if release {
                            self.ctx.add_subset(row, Row::empty(), Origin::Release(def));
                        } else {
                            let fails = self.handler_init_fails(def);
                            self.ctx.add_subset(row, fails, Origin::HandlerPart);
                        }
                    }
                    continue;
                }
                // The arguments of a layer's items cannot fail (`[effect.layer]`).
                Some(Item::Layer(item)) => {
                    for (member, m) in item.members.iter().enumerate() {
                        for arg in m.args.iter().flat_map(|a| &a.args) {
                            let row = self.expr_row(arg.value);
                            let origin = Origin::LayerArg {
                                layer: def,
                                member,
                                arg: arg.value,
                            };
                            self.ctx.add_subset(row, Row::empty(), origin);
                        }
                    }
                    continue;
                }
                _ => continue,
            };
            let row = self.expr_row(body);
            self.ctx.add_subset(row, declared, Origin::Body(def));
        }
    }

    /// The declared type, or the monomorphic type within the group.
    pub(super) fn def_ty(&self, def: DefId) -> Option<Ty> {
        match self.sigs.get(def) {
            Some(sig) => Some(sig.scheme.ty.clone()),
            None => self.pending_ty(def).map(|(ty, _)| ty),
        }
    }

    /// The definition's type parameters (the names of `Bound(i)`).
    pub(super) fn def_params(&self, def: DefId) -> Vec<TyParam> {
        match self.sigs.get(def) {
            Some(sig) => sig.scheme.params.clone(),
            None => self
                .pending_ty(def)
                .map(|(_, params)| params)
                .unwrap_or_default(),
        }
    }

    /// Sub-expressions' E flows into the parent (spec 9.4). Records the computed E per
    /// expression.
    fn expr_row(&mut self, expr: ExprId) -> Row {
        let program = self.program;
        let e = &program.exprs[expr];
        let row = match &e.kind {
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. } => Row::empty(),
            ExprKind::Interpolated(parts) => {
                let mut row = Row::empty();
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        row.extend(&self.expr_row(*e));
                    }
                }
                row
            }
            ExprKind::Field { base, .. } => self.expr_row(*base),
            ExprKind::Call { callee, args } => {
                let mut row = self.expr_row(*callee);
                for arg in args {
                    row.extend(&self.expr_row(arg.value));
                }
                row.extend(&self.callee_fails(*callee, e.span));
                row
            }
            ExprKind::Unary { operand, .. } => self.expr_row(*operand),
            ExprKind::Binary {
                op: emela_resolve::hir::BinaryOp::Pipe,
                lhs,
                rhs,
            } => {
                // `x |> f(a)` is `f(x, a)`. The right side's call expression records E without
                // the left side's.
                let mut row = self.expr_row(*lhs);
                let rhs_row = match &program.exprs[*rhs].kind {
                    ExprKind::Call { callee, args } => {
                        let mut r = self.expr_row(*callee);
                        for arg in args {
                            r.extend(&self.expr_row(arg.value));
                        }
                        r.extend(&self.callee_fails(*callee, e.span));
                        self.expr_rows.insert(*rhs, r.clone());
                        r
                    }
                    _ => {
                        let mut r = self.expr_row(*rhs);
                        r.extend(&self.callee_fails(*rhs, e.span));
                        r
                    }
                };
                row.extend(&rhs_row);
                row
            }
            ExprKind::Binary { lhs, rhs, .. } => self.expr_row(*lhs).union(&self.expr_row(*rhs)),
            ExprKind::Tuple(elems) => self.rows_of(elems),
            ExprKind::List { elems, rest } => {
                let mut row = self.rows_of(elems);
                if let Some(rest) = rest {
                    row.extend(&self.expr_row(*rest));
                }
                row
            }
            ExprKind::Block(block) => self.block_row(block),
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let mut row = self.expr_row(*cond);
                row.extend(&self.expr_row(*then_branch));
                if let Some(e) = else_branch {
                    row.extend(&self.expr_row(*e));
                }
                row
            }
            ExprKind::Match { scrutinee, arms } => {
                let mut row = self.expr_row(*scrutinee);
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        row.extend(&self.expr_row(guard));
                    }
                    row.extend(&self.expr_row(arm.body));
                }
                row
            }
            ExprKind::Escape { expr: inner, arms } => {
                let inner_row = self.expr_row(*inner);
                let caught: Vec<(PatId, DefId)> = arms
                    .iter()
                    .filter_map(|arm| match &program.pats[arm.pat].kind {
                        PatKind::Ctor {
                            res: Res::Def(def), ..
                        } if program.defs[*def].kind == DefKind::Error => Some((arm.pat, *def)),
                        _ => None,
                    })
                    .collect();
                // Only errors whose values the arms fully catch are removed from E (spec 7.3).
                // Errors caught only partially (`NotFound(id: 1)`, guarded arms) stay in E, and
                // unmatched values propagate.
                let tags = self.escaped_tags(arms, &caught);
                // If some arm is not an error constructor (E0320), treat everything as caught so
                // errors do not cascade.
                let broken = arms.iter().any(|arm| {
                    !matches!(
                        &program.pats[arm.pat].kind,
                        PatKind::Ctor { .. } | PatKind::Missing
                    )
                });
                let inner_row = if broken { Row::empty() } else { inner_row };
                let mut row = Row::empty();
                for e in self.ctx.resolve_row(&inner_row).elems() {
                    match e {
                        RowElem::Tag(t) if tags.contains(&t.def) => {}
                        // Replace the set variable with one whose solution excludes the caught
                        // tags.
                        RowElem::Var(v) if !tags.is_empty() => {
                            let d = self.ctx.new_row_var();
                            let minus = tags.iter().map(|&t| Tag::new(t)).collect();
                            self.row_diffs.push((d, *v, minus));
                            row.insert(RowElem::Var(d));
                        }
                        // Nothing is removed from set type parameters or error type parameters
                        // (spec 7.3).
                        e => row.insert(e.clone()),
                    }
                }
                for arm in arms {
                    if let Some(guard) = arm.guard {
                        row.extend(&self.expr_row(guard));
                    }
                    row.extend(&self.expr_row(arm.body));
                }
                self.escapes.push((*inner, caught));
                row
            }
            ExprKind::Fail(inner) => {
                let mut row = self.expr_row(*inner);
                let ty = self.exprs.get(*inner).cloned().unwrap_or(Ty::Error);
                let ty = self.ctx.resolve(&ty);
                if self.is_error_ty(&ty) {
                    row.insert(RowElem::Of(ty));
                } else {
                    self.diags.push(Diagnostic {
                        span: Some(program.exprs[*inner].span),
                        kind: DiagnosticKind::NotAnError { found: ty },
                    });
                }
                row
            }
            ExprKind::Assert(e) => self.expr_row(*e),
            ExprKind::Update { ctor, fields, base } => {
                let mut row = self.expr_row(*ctor);
                for field in fields {
                    row.extend(&self.expr_row(field.value));
                }
                row.extend(&self.expr_row(*base));
                row
            }
            // The E of `with` is the body's E plus the init E of the listed handlers and the E of
            // the arguments of their inits (spec 9.4). The arguments of a handler a later item
            // overrides are not evaluated (`[effect.with]`).
            ExprKind::With { handlers, body } => {
                let entries =
                    crate::layer::with_entries(program, self.sigs, self.ty_table, handlers);
                let mut row = Row::empty();
                for (i, item) in handlers.iter().enumerate() {
                    let initialized = crate::layer::item_initialized(&entries, i);
                    for arg in crate::layer::item_args(item) {
                        let arg_row = self.expr_row(arg);
                        if initialized {
                            row.extend(&arg_row);
                        }
                    }
                }
                row.extend(&self.expr_row(*body));
                for entry in entries {
                    if let Some(h) = entry.handler() {
                        row.extend(&self.handler_init_fails(h));
                    }
                }
                row
            }
            // The body's E is enclosed in the lambda type's E; it surfaces when the lambda is
            // called (spec 9.4).
            ExprKind::Lambda { body, .. } => {
                let body_row = self.expr_row(*body);
                if let Some(&(v, _)) = self.lambda_rows.get(expr) {
                    self.ctx.add_subset(body_row, Row::var(v), Origin::Lambda);
                }
                Row::empty()
            }
        };
        self.expr_rows.insert(expr, row.clone());
        row
    }

    fn rows_of(&mut self, exprs: &[ExprId]) -> Row {
        let mut row = Row::empty();
        for &e in exprs {
            row.extend(&self.expr_row(e));
        }
        row
    }

    fn block_row(&mut self, block: &Block) -> Row {
        let mut row = Row::empty();
        for stmt in &block.stmts {
            match stmt {
                Stmt::Let { value, .. } => row.extend(&self.expr_row(*value)),
                Stmt::Expr(e) => row.extend(&self.expr_row(*e)),
            }
        }
        if let Some(tail) = block.tail {
            row.extend(&self.expr_row(tail));
        }
        row
    }

    /// The callee function type's E. If a single-error type parameter is instantiated with a
    /// non-error type, pushes E0319 at `span` (the call expression).
    fn callee_fails(&mut self, callee: ExprId, span: Span) -> Row {
        let ty = self.exprs.get(callee).cloned().unwrap_or(Ty::Error);
        let Ty::Fn(f) = self.ctx.shallow_resolve(&ty) else {
            return Row::empty();
        };
        let mut row = Row::empty();
        for e in f.fails.elems() {
            if let RowElem::Of(ty) = e {
                let ty = self.ctx.resolve(ty);
                if !self.is_error_ty(&ty) {
                    self.diags.push(Diagnostic {
                        span: Some(span),
                        kind: DiagnosticKind::NotAnError { found: ty },
                    });
                    continue;
                }
            }
            row.insert(e.clone());
        }
        row
    }

    /// Whether the type can be used as an error type: a named error type, a type parameter, or a
    /// type variable not yet determined. `Never` and `Ty::Error` are accepted (adding nothing to
    /// E) so errors do not cascade.
    fn is_error_ty(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Named(con, _) => self.ctx.error_tag(*con).is_some(),
            Ty::Bound(_) | Ty::Var(_) | Ty::Never | Ty::Error => true,
            _ => false,
        }
    }

    /// Tags are matched including type arguments; if those are not determined yet, unification is
    /// tried (spec 8.1, `InferCtx::same_tag`).
    fn contains_elem(&mut self, row: &Row, e: &RowElem) -> bool {
        match e {
            RowElem::Tag(t) => self.ctx.row_has_tag(row, t),
            e => row.contains(e),
        }
    }

    /// Solves the inclusion constraints and pushes diagnostics for unsatisfied ones. Later
    /// `resolve` calls use the solution.
    ///
    /// Constraints added while solving are kept, not dropped: for tags with set type arguments
    /// they are the whole point of the comparison. While solving, a constraint already recorded
    /// is not added again, so the rounds end.
    pub(super) fn solve_errors(&mut self) {
        // Comparing tags with set type arguments (`Scope[E, R]`, spec 11.2) adds constraints
        // while solving, so solve again until no new constraint appears.
        self.ctx.set_solving(true);
        let row_diffs = std::mem::take(&mut self.row_diffs);
        loop {
            let count = self.ctx.subsets().len();
            self.solve_round(&row_diffs);
            if self.ctx.subsets().len() == count {
                break;
            }
        }
        self.ctx.set_solving(false);
        let subsets = self.ctx.subsets().to_vec();
        self.check_subsets(&subsets);
    }

    /// Computes the least solution of the constraints collected so far.
    fn solve_round(&mut self, row_diffs: &[(RowVar, RowVar, Vec<Tag>)]) {
        // Start from the set variables, not from the previous round's solution.
        self.ctx.clear_row_solution();
        let subsets = self.ctx.subsets().to_vec();

        // Lower bounds of set variables.
        let mut lowers: FxHashMap<RowVar, Vec<Lower>> = FxHashMap::default();
        for (d, v, minus) in row_diffs.iter().cloned() {
            lowers.entry(d).or_default().push(Lower {
                elem: RowElem::Var(v),
                minus,
            });
        }
        // Set variables with an upper bound (contained in a set without set variables). Not
        // generalized.
        let mut upper: FxHashSet<RowVar> = FxHashSet::default();
        for c in &subsets {
            let sub = self.ctx.resolve_row(&c.sub);
            let sup = self.ctx.resolve_row(&c.sup);
            match sup.vars().next() {
                Some(v) => {
                    let minus: Vec<Tag> = sup.tags().cloned().collect();
                    for e in sub.elems() {
                        if !self.contains_elem(&sup, e) {
                            lowers.entry(v).or_default().push(Lower {
                                elem: e.clone(),
                                minus: minus.clone(),
                            });
                        }
                    }
                }
                None => upper.extend(sub.vars()),
            }
        }

        // Set variables to generalize.
        let mut negative = Vec::new();
        for (_, ty, _) in self.pending.clone() {
            self.negative_row_vars(&ty, true, &mut negative);
        }
        let mut solution: FxHashMap<RowVar, Row> = FxHashMap::default();
        for v in negative {
            if !lowers.contains_key(&v) && !upper.contains(&v) {
                solution.insert(v, Row::var(v));
            }
        }

        // The least solution: widen the union of lower bounds until nothing changes.
        loop {
            let mut changed = false;
            for (&v, entries) in &lowers {
                for lower in entries {
                    let atoms = match &lower.elem {
                        RowElem::Var(w) => solution.get(w).cloned().unwrap_or_default(),
                        e => self.ctx.resolve_row(&Row::from_elems([e.clone()])),
                    };
                    let target = solution.entry(v).or_default();
                    for a in atoms.elems() {
                        if let RowElem::Tag(t) = a
                            && lower.minus.iter().any(|m| self.ctx.same_tag(m, t))
                        {
                            continue;
                        }
                        if !target.contains(a) {
                            target.insert(a.clone());
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.ctx.set_row_solution(solution);
    }

    /// Pushes diagnostics for constraints the solution does not satisfy.
    fn check_subsets(&mut self, subsets: &[Subset]) {
        for c in subsets {
            let sub = self.ctx.resolve_row(&c.sub);
            let sup = self.ctx.resolve_row(&c.sup);
            let missing: Vec<RowElem> = sub
                .elems()
                .iter()
                .filter(|e| !self.contains_elem(&sup, e))
                .cloned()
                .collect();
            if missing.is_empty() {
                continue;
            }
            match (&c.origin, c.kind) {
                (Origin::Body(def), SetKind::Errors) => self.report_unhandled(*def, &missing),
                (Origin::HandlerOp { handler, index }, SetKind::Errors) => {
                    self.report_unhandled_op(*handler, *index, &missing)
                }
                (Origin::Release(handler), _) => self.report_release_fails(*handler, &missing),
                (Origin::Body(def), SetKind::Effects) => {
                    let context = match self.program.item(*def) {
                        Some(Item::Const(_)) => EffectContext::Const,
                        _ => EffectContext::Declared,
                    };
                    self.report_unhandled_effects(*def, &missing, context);
                }
                (&Origin::LayerArg { layer, member, arg }, kind) => {
                    let context = self.layer_arg(layer, member);
                    match kind {
                        SetKind::Errors => self.push_unhandled(arg, false, Some(context), &missing),
                        SetKind::Effects => self.push_unhandled_effects(
                            arg,
                            &[],
                            &missing,
                            EffectContext::LayerArg(context),
                        ),
                    }
                }
                (Origin::Closed(def), _) => {
                    if let Some(context) = self.closed.get(def).cloned() {
                        self.report_unhandled_effects(*def, &missing, context);
                    }
                }
                (Origin::Coerce { span, def }, kind) => {
                    let params = def.map(|d| self.def_params(d)).unwrap_or_default();
                    let cons = &self.ty_table.cons;
                    let kind = match kind {
                        SetKind::Errors => DiagnosticKind::ErrorSetMismatch {
                            expected: SmolStr::new(sup.display_with(cons, &params).to_string()),
                            found: SmolStr::new(sub.display_with(cons, &params).to_string()),
                        },
                        SetKind::Effects => DiagnosticKind::EffectSetMismatch {
                            expected: SmolStr::new(sup.display_effects(cons, &params).to_string()),
                            found: SmolStr::new(sub.display_effects(cons, &params).to_string()),
                        },
                    };
                    self.diags.push(Diagnostic { span: *span, kind });
                }
                // Lambda types' E and R and handler dependencies are set variables, so these
                // always hold.
                (Origin::Lambda | Origin::HandlerPart, _)
                | (Origin::HandlerOp { .. }, SetKind::Effects) => {}
            }
        }

        self.check_unraised();
    }

    /// Collects set variables in negative positions (the E and R of function-typed parameters).
    /// Inside type arguments of named types, both positions count.
    fn negative_row_vars(&mut self, ty: &Ty, positive: bool, out: &mut Vec<RowVar>) {
        match self.ctx.shallow_resolve(ty) {
            Ty::Fn(f) => {
                if !positive {
                    for v in f.fails.vars().chain(f.uses.vars()) {
                        if !out.contains(&v) {
                            out.push(v);
                        }
                    }
                }
                for p in &f.params {
                    self.negative_row_vars(p, !positive, out);
                }
                self.negative_row_vars(&f.ret, positive, out);
            }
            Ty::Named(_, args) => {
                for a in &args {
                    self.negative_row_vars(a, false, out);
                    self.negative_row_vars(a, true, out);
                }
            }
            // Set type arguments of named types count as both positions (called from both
            // above).
            Ty::Set(_, row) if !positive => {
                for v in row.vars() {
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
            Ty::List(elem) => self.negative_row_vars(&elem, positive, out),
            Ty::Tuple(elems) => {
                for e in &elems {
                    self.negative_row_vars(e, positive, out);
                }
            }
            _ => {}
        }
    }

    /// Body errors not contained in the declared E (E0318). For each error, finds the expression
    /// that raised it (`fail` or a call) and groups errors from the same expression into one
    /// diagnostic.
    fn report_unhandled(&mut self, def: DefId, missing: &[RowElem]) {
        let program = self.program;
        let (body, declared) = match program.item(def) {
            Some(Item::Const(item)) => (item.value, false),
            _ => match fn_parts(program, def) {
                Some(FnParts {
                    body: Some(body),
                    sig,
                    ..
                }) => (body, sig.is_some_and(|s| s.fails.is_some())),
                _ => return,
            },
        };
        self.push_unhandled(body, declared, None, missing);
    }

    /// The layer item whose arguments are being reported.
    fn layer_arg(&self, layer: DefId, member: usize) -> LayerArg {
        let program = self.program;
        let handler = match program.item(layer) {
            Some(Item::Layer(item)) => match item.members[member].res {
                Res::Def(h) => program.defs[h].name.clone(),
                _ => SmolStr::new("?"),
            },
            _ => SmolStr::new("?"),
        };
        LayerArg {
            layer: program.defs[layer].name.clone(),
            handler,
        }
    }

    /// Errors of a handler operation body not contained in the effect operation's declared E
    /// (E0318, spec 8.1).
    fn report_unhandled_op(&mut self, handler: DefId, index: usize, missing: &[RowElem]) {
        let program = self.program;
        let Some(Item::Handler(item)) = program.item(handler) else {
            return;
        };
        let Some(body) = item.ops[index].body else {
            return;
        };
        let declared = match item.ops[index].op {
            Res::Def(op) => matches!(program.item(op), Some(Item::Op(o)) if o.sig.fails.is_some()),
            _ => false,
        };
        self.push_unhandled(body, declared, None, missing);
    }

    /// E0420, spec 8.4. Grouped by the expression that raised each error.
    fn report_release_fails(&mut self, handler: DefId, missing: &[RowElem]) {
        let program = self.program;
        let Some(Item::Handler(item)) = program.item(handler) else {
            return;
        };
        let Some((_, body)) = item.release else {
            return;
        };
        let rows = self.resolved_rows(SetKind::Errors);
        let cons = &self.ty_table.cons;
        for (origin, elems) in self.group_by_origin(&rows, body, missing) {
            let errors = elems
                .iter()
                .map(|e| {
                    let row = Row::from_elems([e.clone()]);
                    SmolStr::new(row.display_with(cons, &[]).to_string())
                })
                .collect();
            self.diags.push(Diagnostic {
                span: Some(self.program.exprs[origin].span),
                kind: DiagnosticKind::ReleaseFails {
                    handler: program.defs[handler].name.clone(),
                    errors,
                },
            });
        }
    }

    /// Pushes E0318, grouped by the expression that raised each error.
    fn push_unhandled(
        &mut self,
        body: ExprId,
        declared: bool,
        layer_arg: Option<LayerArg>,
        missing: &[RowElem],
    ) {
        // Handler operations have no type parameters.
        let params = self.current_params(body).unwrap_or_default();
        let rows = self.resolved_rows(SetKind::Errors);
        let cons = &self.ty_table.cons;
        for (origin, elems) in self.group_by_origin(&rows, body, missing) {
            let errors = elems
                .iter()
                .map(|e| {
                    let row = Row::from_elems([e.clone()]);
                    SmolStr::new(row.display_with(cons, &params).to_string())
                })
                .collect();
            self.diags.push(Diagnostic {
                span: Some(self.program.exprs[origin].span),
                kind: DiagnosticKind::UnhandledErrors {
                    errors,
                    declared,
                    layer_arg: layer_arg.clone(),
                },
            });
        }
    }

    /// The body's R not contained in the R the declaration or context allows (E0402). For each
    /// effect, finds the expression that required it (`use` or a call) and groups effects from the
    /// same expression into one diagnostic.
    pub(super) fn report_unhandled_effects(
        &mut self,
        def: DefId,
        missing: &[RowElem],
        context: EffectContext,
    ) {
        let body = match self.program.item(def) {
            Some(Item::Const(item)) => item.value,
            _ => match fn_parts(self.program, def).and_then(|p| p.body) {
                Some(body) => body,
                None => return,
            },
        };
        let params = self.def_params(def);
        self.push_unhandled_effects(body, &params, missing, context);
    }

    /// E0402 for the effects `missing` of `body`, grouped by the expression that required them.
    fn push_unhandled_effects(
        &mut self,
        body: ExprId,
        params: &[TyParam],
        missing: &[RowElem],
        context: EffectContext,
    ) {
        let rows = self.resolved_rows(SetKind::Effects);
        let cons = &self.ty_table.cons;
        for (origin, elems) in self.group_by_origin(&rows, body, missing) {
            let effects = elems
                .iter()
                .map(|e| {
                    let row = Row::from_elems([e.clone()]);
                    SmolStr::new(row.display_effects(cons, params).to_string())
                })
                .collect();
            let scope = self.forked_from(origin, &elems);
            self.diags.push(Diagnostic {
                span: Some(self.program.exprs[origin].span),
                kind: DiagnosticKind::UnhandledEffects {
                    effects,
                    context: context.clone(),
                    scope,
                },
            });
        }
    }

    /// If `origin` calls a scope function (`Fiber.scope`) and some of `elems` are not required by
    /// its body itself, they come from fibers forked in it (through the `R` of `Scope[E, R]`,
    /// spec 11.2): returns the scope function's name for the note that they must be provided
    /// outside it.
    fn forked_from(&mut self, origin: ExprId, elems: &[RowElem]) -> Option<SmolStr> {
        let (scope, body) = self.scope_call(origin)?;
        let ty = self.exprs.get(body).cloned()?;
        let Ty::Fn(f) = self.ctx.resolve(&ty) else {
            return None;
        };
        elems.iter().any(|e| !f.uses.contains(e)).then_some(scope)
    }

    /// The scope function's name and the body argument, if `expr` calls a scope function.
    pub(super) fn scope_call(&self, expr: ExprId) -> Option<(SmolStr, ExprId)> {
        let program = self.program;
        let (callee, body) = match &program.exprs[expr].kind {
            ExprKind::Call { callee, args } => (*callee, args.first()?.value),
            ExprKind::Binary {
                op: emela_resolve::hir::BinaryOp::Pipe,
                lhs,
                rhs,
            } => match &program.exprs[*rhs].kind {
                ExprKind::Call { callee, .. } => (*callee, *lhs),
                _ => (*rhs, *lhs),
            },
            _ => return None,
        };
        let ExprKind::Path(Res::Def(def)) = program.exprs[callee].kind else {
            return None;
        };
        let scope = self.fibers.scope_fns.iter().find(|s| s.def == def)?;
        Some((scope.display.clone(), body))
    }

    /// Each expression's E (R if `kind` says so), with set variables solved.
    fn resolved_rows(&mut self, kind: SetKind) -> SparseMap<ExprId, Row> {
        let table = match kind {
            SetKind::Errors => self.expr_rows.clone(),
            SetKind::Effects => self.expr_effects.clone(),
        };
        let mut rows: SparseMap<ExprId, Row> = SparseMap::default();
        for (expr, row) in table.iter() {
            rows.insert(expr, self.ctx.resolve_row(row));
        }
        rows
    }

    /// Groups missing elements by the expression that raised them ([`Self::origin_of`]).
    fn group_by_origin(
        &self,
        rows: &SparseMap<ExprId, Row>,
        body: ExprId,
        missing: &[RowElem],
    ) -> Vec<(ExprId, Vec<RowElem>)> {
        let mut groups: Vec<(ExprId, Vec<RowElem>)> = Vec::new();
        for e in missing {
            let origin = self.origin_of(rows, body, e);
            match groups.iter_mut().find(|(o, _)| *o == origin) {
                Some((_, elems)) => elems.push(e.clone()),
                None => groups.push((origin, vec![e.clone()])),
            }
        }
        groups
    }

    /// `None` unless `body` is the body of a fn or const (that is, for handler operations).
    fn current_params(&self, body: ExprId) -> Option<Vec<TyParam>> {
        let program = self.program;
        let def = self.defs.iter().copied().find(|&d| match program.item(d) {
            Some(Item::Const(item)) => item.value == body,
            _ => fn_parts(program, d).is_some_and(|p| p.body == Some(body)),
        })?;
        Some(self.def_params(def))
    }

    /// The expression that raised error `elem`: follows the first sub-expression in source order
    /// whose E contains `elem`, and returns the one that cannot be followed further (`fail` or a
    /// call).
    fn origin_of(&self, rows: &SparseMap<ExprId, Row>, expr: ExprId, elem: &RowElem) -> ExprId {
        let mut current = expr;
        loop {
            let next = self
                .sub_exprs(current)
                .into_iter()
                .find(|&e| rows.get(e).is_some_and(|row| row.contains(elem)));
            match next {
                Some(next) => current = next,
                None => return current,
            }
        }
    }

    /// Sub-expressions whose E (R) flows into this one, in source order. Lambda bodies are
    /// excluded.
    pub(super) fn sub_exprs(&self, expr: ExprId) -> Vec<ExprId> {
        let program = self.program;
        let mut out = Vec::new();
        match &program.exprs[expr].kind {
            ExprKind::Missing
            | ExprKind::Lit(_)
            | ExprKind::Path(_)
            | ExprKind::Unit
            | ExprKind::Use { .. }
            | ExprKind::Lambda { .. } => {}
            ExprKind::Interpolated(parts) => {
                for part in parts {
                    if let StrPart::Interp(e) = part {
                        out.push(*e);
                    }
                }
            }
            ExprKind::Field { base, .. } => out.push(*base),
            ExprKind::Call { callee, args } => {
                out.push(*callee);
                out.extend(args.iter().map(|a| a.value));
            }
            ExprKind::Unary { operand, .. } => out.push(*operand),
            ExprKind::Binary { lhs, rhs, .. } => out.extend([*lhs, *rhs]),
            ExprKind::Tuple(elems) => out.extend(elems),
            ExprKind::List { elems, rest } => {
                out.extend(elems);
                out.extend(rest);
            }
            ExprKind::Block(block) => {
                for stmt in &block.stmts {
                    match stmt {
                        Stmt::Let { value, .. } => out.push(*value),
                        Stmt::Expr(e) => out.push(*e),
                    }
                }
                out.extend(block.tail);
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                out.extend([*cond, *then_branch]);
                out.extend(else_branch);
            }
            ExprKind::Match {
                scrutinee: inner,
                arms,
            }
            | ExprKind::Escape { expr: inner, arms } => {
                out.push(*inner);
                for arm in arms {
                    out.extend(arm.guard);
                    out.push(arm.body);
                }
            }
            ExprKind::Fail(e) | ExprKind::Assert(e) => out.push(*e),
            ExprKind::Update { ctor, fields, base } => {
                out.push(*ctor);
                out.extend(fields.iter().map(|f| f.value));
                out.push(*base);
            }
            // The arguments of a handler a later item overrides are not evaluated
            // (`[effect.with]`), so no error or call in them is reported.
            ExprKind::With { handlers, body } => {
                out.extend(crate::layer::evaluated_args(
                    self.program,
                    self.sigs,
                    self.ty_table,
                    handlers,
                ));
                out.push(*body);
            }
        }
        out
    }

    /// Errors whose values the `escape` arms fully catch (spec 7.3). Collects the arms per error
    /// and keeps those with no missing values by the exhaustiveness check. Errors have no type
    /// parameters, so this need not wait for types to settle.
    fn escaped_tags(&self, arms: &[Arm], caught: &[(PatId, DefId)]) -> Vec<DefId> {
        let shapes = Shapes {
            program: self.program,
            ty_table: self.ty_table,
            sigs: self.sigs,
        };
        let mut tags: Vec<DefId> = Vec::new();
        for &(_, def) in caught {
            if tags.contains(&def) {
                continue;
            }
            let Some(ty) = self.ty_table.self_ty(def) else {
                continue;
            };
            let group: Vec<Arm> = arms
                .iter()
                .filter(|arm| caught.iter().any(|&(p, d)| p == arm.pat && d == def))
                .cloned()
                .collect();
            if shapes.check_arms(&ty, &group).is_exhaustive() {
                tags.push(def);
            }
        }
        tags
    }

    /// Warns when an `escape` arm catches an error the inner expression cannot raise (W0302). No
    /// warning if the inner E has a set with unknown contents (a set type parameter, an error
    /// type parameter, a set variable to be generalized), since it may be caught at run time.
    fn check_unraised(&mut self) {
        for (inner, caught) in std::mem::take(&mut self.escapes) {
            let Some(row) = self.expr_rows.get(inner).cloned() else {
                continue;
            };
            let row = self.ctx.resolve_row(&row);
            if row.elems().iter().any(|e| !matches!(e, RowElem::Tag(_))) {
                continue;
            }
            for (pat, tag) in caught {
                if !row.contains(&RowElem::Tag(Tag::new(tag))) {
                    let error = self.program.defs[tag].name.clone();
                    self.diags.push(Diagnostic {
                        span: Some(self.program.pats[pat].span),
                        kind: DiagnosticKind::UnraisedError { error },
                    });
                }
            }
        }
    }
}
