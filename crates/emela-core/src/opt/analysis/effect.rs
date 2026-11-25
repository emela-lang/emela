//! Effect and defect summaries of a function: what evaluating it may do.
//!
//! # Contract
//!
//! ```text
//! Analysis ID: AnalysisId::Effect
//! Domain / lattice: a product of five Bool facts, one per summary field. The top element is
//!                   `Fact::Unknown`; a `Known` summary is a conjunction of "may" facts, so the
//!                   safe direction is to set a field to true when unsure.
//! Transfer: a function's summary is the union (logical or) of the summaries of the expressions it
//!           evaluates, including calls, lambdas it may run and handler operations it may invoke.
//! Soundness: `Known(s)` says s.may_fail is false only when no path can fail, and likewise for the
//!            other fields. A defect is never folded into "purity": a type with no effects can
//!            still divide by zero or index out of bounds.
//! Deopt (unknown): a consumer that gets `Unknown` leaves the expression alone. It must not read
//!                  `Unknown` as `may_fail = false` or as "pure".
//! Consumers: the β and δ reductions (whether an argument may be moved or duplicated), the
//!            algebraic laws (whether a removed side is trivial), CSE/gVN and evidence
//!            specialization.
//! ```
//!
//! # Status
//!
//! Skeleton of B0/PR-002: [`analyze`] returns `Unknown`. B1/PR-020 implements the summary.

use crate::ir::Module;

use super::{Analysis, AnalysisId, Fact};

/// What evaluating a function may do. Every field is a "may": `true` is the conservative value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectSummary {
    /// The function may perform an effect operation through the evidence table.
    pub may_use_effect: bool,
    /// The function may fail (the `E` of its type is not empty on some path).
    pub may_fail: bool,
    /// The function may suspend.
    pub may_suspend: bool,
    /// The function may reach a defect (division by zero, array out of bounds, an assertion).
    pub may_defect: bool,
    /// The function may diverge.
    pub may_diverge: bool,
}

/// The effect analysis.
pub struct Effect;

impl Analysis for Effect {
    type Output = Fact<EffectSummary>;

    const ID: AnalysisId = AnalysisId::Effect;

    fn analyze(&self, _m: &Module) -> Self::Output {
        Fact::Unknown
    }
}

/// The effect summary of `m`, or `Unknown` (see the module contract).
pub fn analyze(m: &Module) -> Fact<EffectSummary> {
    Effect.analyze(m)
}
