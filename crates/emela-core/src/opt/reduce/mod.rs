//! The reduction kernel of the Core IR.
//!
//! A reduction is an equation between a Core IR expression and the expression that replaces it.
//! Every module here holds one reduction (`zeta`, `beta`, `eta`, `iota`, `delta`, `fold`,
//! `algebra`), or the shared substitution engine they build on (`subst`). The file order below is
//! the order of plan §4; the order [`crate::opt::run_with`] applies them in is in the rustdoc of
//! [`crate::opt`].
//!
//! Most rules have the shape `fn(&mut Module) -> RuleStats`; δ also takes the bundled core
//! functions that it must leave as calls (`fn(&mut Module, &CoreFns) -> RuleStats`). Every rule
//! fills only its counter. A rule that cannot keep its contract leaves the module untouched and
//! records the reason in `RuleStats::deopt`; the pipeline merges the result into
//! [`crate::opt::Stats`].
//!
//! # Status
//!
//! This module is the fixed skeleton of A0/PR-002. The rule functions exist so that later work
//! fills them one at a time, and every one of them currently returns `RuleStats::default` and
//! rewrites nothing. [`subst`] exposes the substitution API with a conservative body that only
//! handles trivial values; A1/PR-010 completes it. The rules are no-ops until their own pull
//! requests land (A2/PR-011 to A8/PR-017).

pub mod subst;

mod algebra;
mod beta;
mod delta;
mod eta;
mod fold;
mod iota;
mod zeta;

// The rules are visible to the pipeline in [`crate::opt`], which owns the order. They are not part
// of the crate's public API: only the pipeline calls them.
pub(super) use algebra::algebra;
pub(super) use beta::beta;
pub(super) use delta::delta;
pub(super) use eta::eta;
pub(super) use fold::fold;
pub(super) use iota::iota;
pub(super) use zeta::zeta;
