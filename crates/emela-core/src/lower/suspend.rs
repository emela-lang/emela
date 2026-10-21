//! Marking what may suspend (spec 8.1, 9.5, 16.5).
//!
//! Functions, lambdas and calls are marked by whether the effect set R of their type is not
//! Immediate (may suspend). Per element of R, an effect may suspend if it has a suspend fn or is
//! Async (spec 9.5). An operation call is decided by the operation's declaration (`suspend fn` or
//! not). An fn-operation implementation may suspend only when its body's R is not Immediate and the
//! body contains a call marked as suspending (E0417, spec 8.1).
//!
//! Whether a set type parameter (such as `R(f)` of `List.map`) may suspend depends on its
//! instantiation. A suspending function becomes a generator and is called differently, so such a
//! function gets "suspending copies" as separate IR functions for calls that instantiate it with
//! suspending sets. Which type parameters may suspend is encoded as a [`Specialization`] (a bit per
//! effect-set type parameter instantiated with a suspending set). 0 means all are Immediate and is
//! the original function (unchanged output). Which one to call is decided per reference from the
//! instantiated sets (`ProgramTypes::type_rows`). Inside a suspending copy, whether each set type
//! parameter may suspend is fixed, so marks on lambdas and function value calls follow uniquely
//! from the types.
//!
//! Why copies rather than making every function that might suspend a generator: a call
//! instantiated with Immediate sets would then call a generator as an ordinary function (and the
//! other way round). Inside a copy, a lambda that escapes has an instantiated R that may suspend,
//! so its callers mark the call the same way.
//!
//! A set type parameter that appears only in parameter types also gets a bit, so copies are made
//! that do not suspend themselves (a function that only builds lambdas). The output is right; the
//! code is only larger.

use emela_resolve::hir::{DefId, ExprId, Item};
use emela_types::{Row, RowElem, Scheme, SetKind, Ty};

use super::Lowerer;
use crate::ir::Expr;

/// Which suspending copy: a bit per index of an effect-set type parameter instantiated with a
/// suspending set. 0 is the original function (all Immediate).
pub(super) type Specialization = u64;

/// The name of the builtin constraint Immediate (spec 9.5).
const IMMEDIATE: &str = "Immediate";

impl Lowerer<'_> {
    /// Whether the effect is Immediate (spec 9.5): it has no suspend fn and is not Async.
    pub(super) fn is_immediate_effect(&self, effect: DefId) -> bool {
        let hir = self.hir;
        if hir.prelude_def("Async") == Some(effect) {
            return false;
        }
        // A label is Immediate when its base is (issue #255).
        hir.effect_ops(effect)
            .iter()
            .all(|&op| !matches!(hir.item(op), Some(Item::Op(o)) if o.sig.is_suspend))
    }

    /// Whether the set `row` inside the function being lowered may suspend. Set type parameters
    /// (and set variables generalized by inference) are determined by the function being lowered
    /// (the original or a particular suspending copy).
    pub(super) fn row_suspends(&self, row: &Row) -> bool {
        row.elems().iter().any(|e| match e {
            RowElem::Tag(effect) => !self.is_immediate_effect(effect.def),
            RowElem::Bound(i) => spec_has(self.spec, *i),
            RowElem::Var(v) => self
                .current_def
                .and_then(|def| self.types.generalized_rows.get(def))
                .and_then(|rows| rows.iter().find(|(w, _)| w == v))
                .is_some_and(|&(_, i)| spec_has(self.spec, i)),
            RowElem::Of(_) => false,
        })
    }

    /// Whether calling a value of function type `ty` inside the function being lowered may suspend.
    pub(super) fn ty_suspends(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Fn(f) if self.row_suspends(&f.uses))
    }

    /// Whether copy `spec` of definition `def` may suspend (becomes a generator). Decided by the R
    /// of the declared type.
    pub(super) fn fn_suspends(&self, def: DefId, spec: Specialization) -> bool {
        let Some(Ty::Fn(f)) = self.types.sigs.get(def).map(|s| &s.scheme.ty) else {
            return false;
        };
        f.uses.elems().iter().any(|e| match e {
            RowElem::Tag(effect) => !self.is_immediate_effect(effect.def),
            RowElem::Bound(i) => spec_has(spec, *i),
            _ => false,
        })
    }

    /// The copy to call when definition `def` is referenced by expression `expr`. With `expr` of
    /// `None` (a root with no referencing expression), the original function (0).
    pub(super) fn specialization_of(&self, def: DefId, expr: Option<ExprId>) -> Specialization {
        let Some(sig) = self.types.sigs.get(def) else {
            return 0;
        };
        let candidates = effect_params(&sig.scheme);
        if candidates == 0 {
            return 0;
        }
        let Some(expr) = expr else {
            return 0;
        };
        let mut spec = 0;
        match self.types.type_rows.get(expr) {
            Some(rows) => {
                for (i, row) in rows.iter().enumerate() {
                    if spec_has(candidates, i as u32) && self.row_suspends(row) {
                        spec |= 1 << i;
                    }
                }
            }
            // A self reference is not instantiated (`instantiate_def` in the type checker), so it
            // is the same as the function being lowered.
            None if self.current_def == Some(def) => return self.spec,
            // References to definitions in the same group not yet generalized are not instantiated
            // either; their type parameters share the generalized set variables as is.
            None => {
                for &(v, i) in self.types.generalized_rows.get(def).into_iter().flatten() {
                    if spec_has(candidates, i) && self.row_suspends(&Row::var(v)) {
                        spec |= 1 << i;
                    }
                }
            }
        }
        spec
    }
}

/// Whether the lowered expression `e` contains a suspending call (`Expr::Call::suspends`). Lambda
/// bodies are not visited (calling the lambda is marked as a function value call). Calls are marked
/// by the same rule as E0417 in the type checker (operation calls by the operation's declaration,
/// function calls by the R of the type).
pub(super) fn has_suspending_call(e: &Expr) -> bool {
    match e {
        Expr::Lambda { .. } => false,
        Expr::Call { suspends: true, .. } => true,
        Expr::Fiber { op, .. } if op.suspends() => true,
        _ => {
            let mut found = false;
            e.for_each_child(&mut |c| found = found || has_suspending_call(c));
            found
        }
    }
}

/// Whether the `i`th type parameter may suspend in copy `spec`.
fn spec_has(spec: Specialization, i: u32) -> bool {
    i < Specialization::BITS && spec & (1 << i) != 0
}

/// The scheme's type parameters used as effect sets without an Immediate constraint (the candidate
/// bits for suspending copies).
fn effect_params(scheme: &Scheme) -> Specialization {
    let mut out = 0;
    collect_effect_params(&scheme.ty, &mut out);
    for (i, p) in scheme.params.iter().enumerate() {
        if p.bounds.iter().any(|b| b == IMMEDIATE) && (i as u32) < Specialization::BITS {
            out &= !(1 << i);
        }
    }
    out
}

fn collect_effect_params(ty: &Ty, out: &mut Specialization) {
    let mut row = |row: &Row| {
        for e in row.elems() {
            if let RowElem::Bound(i) = e
                && *i < Specialization::BITS
            {
                *out |= 1 << i;
            }
        }
    };
    match ty {
        Ty::Fn(f) => row(&f.uses),
        Ty::Set(SetKind::Effects, r) => row(r),
        _ => {}
    }
    ty.for_each_child(|child| collect_effect_params(child, out));
}
