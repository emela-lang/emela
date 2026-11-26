//! Where suggesting a bound item would change meaning (`[effect.with]`).
//!
//! A bound item provides nothing by name. Some diagnostics suggest rewriting a `with` into one
//! with a bound item (`with x = H`): W4401 and E4404 for `use E` and an effect alias, E5003 for a
//! callback. That rewrite sends every other use of E by name in the body of the `with` to an outer
//! handler, so the suggestion is left out when the body of the `with` that provides E by name also
//! uses E by name.
//!
//! "Uses E by name" is read from the R of the expressions of the body: an expression whose R has
//! E and none of whose sub-expressions has it is where E is used, and so is a call whose callee's
//! R has E. Effects are compared by declaration, not by type arguments.

use emela_resolve::hir::{DefId, ExprId, ExprKind, Res, Span};

use super::{BodyInfer, fn_parts};
use crate::row::{Row, SetKind};
use crate::sparse::SparseMap;
use crate::ty::Ty;

impl BodyInfer<'_> {
    /// Records in [`Self::bound_item_redirects`] the `use E` expressions (W4401, `[effect.use]`)
    /// and the references to an effect alias written as a value (E4404), not as the receiver of
    /// an operation, for which a bound item would send other uses of E by name elsewhere. Call
    /// after sets are solved.
    pub(super) fn find_bound_item_redirects(&mut self) {
        let program = self.program;
        let rows = self.resolved_rows(SetKind::Effects);
        let bodies: Vec<ExprId> = self
            .defs
            .iter()
            .filter_map(|&d| fn_parts(program, d).and_then(|p| p.body))
            .collect();
        let mut sites: Vec<(ExprId, Vec<ExprId>)> = Vec::new();
        for body in bodies {
            self.collect_use_sites(body, false, &mut Vec::new(), &mut sites);
        }
        for (site, withs) in sites {
            let ExprKind::Use {
                effect: Res::Def(effect),
                ..
            } = program.exprs[site].kind
            else {
                continue;
            };
            // Rewriting `x = use E` or an effect alias `x <- E` into a bound item turns every use
            // of x into a use of its capability.
            let alias = program.alias_uses.get(site).copied().flatten();
            let excluded = |e: ExprId| match alias {
                Some(local) => program.alias_uses.get(e).copied().flatten() == Some(local),
                None => e == site,
            };
            if self.redirects(&rows, &withs, effect, &excluded) {
                self.bound_item_redirects.push(program.exprs[site].span);
            }
        }
    }

    /// Whether a bound item would send other uses by name elsewhere at the reference `at` to an
    /// external function whose callback uses one of `effects` (E5003, `[ext.boundary]`), in the
    /// body of `owner`. The callback is a lambda, whose body is not in the R of the `with`, or a
    /// function value, whose R is in its type.
    pub(super) fn callback_redirects(
        &mut self,
        owner: Option<DefId>,
        at: Span,
        effects: &[DefId],
    ) -> bool {
        let program = self.program;
        let Some(body) = owner
            .and_then(|d| fn_parts(program, d))
            .and_then(|p| p.body)
        else {
            return false;
        };
        let Some(withs) =
            self.withs_around(body, &mut Vec::new(), &|e| program.exprs[e].span == at)
        else {
            return false;
        };
        let rows = self.resolved_rows(SetKind::Effects);
        // The call of the external function has the R of the callback in its own R.
        let the_call = |e: ExprId| {
            matches!(&program.exprs[e].kind, ExprKind::Call { callee, .. }
                if program.exprs[*callee].span == at)
        };
        effects
            .iter()
            .any(|&effect| self.redirects(&rows, &withs, effect, &the_call))
    }

    /// The `with`s (outermost first) whose bodies enclose the first expression of `expr` for which
    /// `found` holds, following the sub-expressions whose R flows into `expr`. The arguments of
    /// the items of a `with` are outside it.
    fn withs_around(
        &self,
        expr: ExprId,
        withs: &mut Vec<ExprId>,
        found: &dyn Fn(ExprId) -> bool,
    ) -> Option<Vec<ExprId>> {
        if found(expr) {
            return Some(withs.clone());
        }
        let body = match &self.program.exprs[expr].kind {
            ExprKind::With { body, .. } => Some(*body),
            _ => None,
        };
        for sub in self.sub_exprs(expr) {
            let inside = Some(sub) == body;
            if inside {
                withs.push(expr);
            }
            let out = self.withs_around(sub, withs, found);
            if inside {
                withs.pop();
            }
            if out.is_some() {
                return out;
            }
        }
        None
    }

    /// The `use E` expressions under `expr` that are not the receiver of an operation, with the
    /// `with`s around them.
    fn collect_use_sites(
        &self,
        expr: ExprId,
        receiver: bool,
        withs: &mut Vec<ExprId>,
        out: &mut Vec<(ExprId, Vec<ExprId>)>,
    ) {
        let kind = &self.program.exprs[expr].kind;
        if matches!(kind, ExprKind::Use { .. }) && !receiver && !withs.is_empty() {
            out.push((expr, withs.clone()));
        }
        let (body, base) = match kind {
            ExprKind::With { body, .. } => (Some(*body), None),
            ExprKind::Field { base, .. } => (None, Some(*base)),
            _ => (None, None),
        };
        for sub in self.sub_exprs(expr) {
            let inside = Some(sub) == body;
            if inside {
                withs.push(expr);
            }
            self.collect_use_sites(sub, Some(sub) == base, withs, out);
            if inside {
                withs.pop();
            }
        }
    }

    /// Whether the innermost of `withs` that provides `effect` by name has a body that uses
    /// `effect` by name outside `excluded`.
    fn redirects(
        &mut self,
        rows: &SparseMap<ExprId, Row>,
        withs: &[ExprId],
        effect: DefId,
        excluded: &dyn Fn(ExprId) -> bool,
    ) -> bool {
        let program = self.program;
        for &with in withs.iter().rev() {
            let ExprKind::With { handlers, body } = &program.exprs[with].kind else {
                continue;
            };
            let insts = self.item_insts(with, handlers);
            let entries =
                crate::layer::with_entries(program, self.sigs, self.ty_table, handlers, &insts);
            if entries.iter().any(|e| e.named && e.effect.def == effect) {
                return self.uses_by_name(rows, *body, effect, excluded);
            }
        }
        false
    }

    /// Whether `effect` is used by name in `expr` outside `excluded`.
    fn uses_by_name(
        &mut self,
        rows: &SparseMap<ExprId, Row>,
        expr: ExprId,
        effect: DefId,
        excluded: &dyn Fn(ExprId) -> bool,
    ) -> bool {
        let has = |e: ExprId| {
            rows.get(e)
                .is_some_and(|r| r.tags().any(|t| t.def == effect))
        };
        if excluded(expr) || !has(expr) {
            return false;
        }
        let subs: Vec<ExprId> = self
            .sub_exprs(expr)
            .into_iter()
            .filter(|&e| has(e))
            .collect();
        if subs.is_empty() || self.call_uses(expr, effect) {
            return true;
        }
        subs.into_iter()
            .any(|e| self.uses_by_name(rows, e, effect, excluded))
    }

    /// Whether `expr` is a call of a function whose declared R has `effect`, which is a use by
    /// name besides those in its arguments. The R of the callee's instantiated type is not used:
    /// a capability parameter given an argument that uses `effect` by name has `effect` there,
    /// and that use is the argument's. A call of a function value, or of a function whose type
    /// is still being inferred, counts only through its arguments (an approximation that keeps
    /// the help).
    fn call_uses(&self, expr: ExprId, effect: DefId) -> bool {
        let program = self.program;
        let ExprKind::Call { callee, .. } = &program.exprs[expr].kind else {
            return false;
        };
        let ExprKind::Path(Res::Def(def)) = program.exprs[*callee].kind else {
            return false;
        };
        match self.sigs.get(def).map(|s| &s.scheme.ty) {
            Some(Ty::Fn(f)) => f.uses.tags().any(|t| t.def == effect),
            _ => false,
        }
    }
}
