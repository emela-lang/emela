//! The reduction kernel of the Core IR.
//!
//! A reduction is an equation between a Core IR expression and the expression that replaces it.
//! Every module here holds one reduction (`zeta`, `beta`, `eta`, `iota`, `delta`, `fold`,
//! `algebra`, `cse`), or the shared substitution engine they build on (`subst`). The file order
//! below is the order of plan §4; the order [`crate::opt::run_with`] applies them in is in the
//! rustdoc of [`crate::opt`].
//!
//! Most rules have the shape `fn(&mut Module) -> RuleStats`; δ also takes the bundled core
//! functions that it must leave as calls (`fn(&mut Module, &CoreFns) -> RuleStats`). Every rule
//! fills only its counter. A rule that cannot keep its contract leaves the module untouched and
//! records the reason in `RuleStats::deopt`; the pipeline merges the result into
//! [`crate::opt::Stats`].
//!
//! # Status
//!
//! Every rule of the kernel is implemented (A2/PR-011 to A8/PR-017, A11/PR-040 and A12/PR-041),
//! and the rules of A2–A8 are on by default (A10/PR-019). `bounds` and `cse` stay off in
//! `Rules::default` until their integrations. A rule whose [`crate::opt::Rules`] flag is off is
//! skipped by the pipeline and contributes nothing, not even a deopt. [`subst`] is the shared
//! substitution engine the rules build on.
//!
//! [`bounds`] is the first rule that depends on an analysis (B5's ranges) and the first that stays
//! off in [`crate::opt::Rules::default`] until its milestone measures it (A11/PR-040).

pub mod subst;

mod algebra;
mod beta;
mod bounds;
mod cse;
mod delta;
mod eta;
mod fold;
mod iota;
mod zeta;

// The rules are visible to the pipeline in [`crate::opt`], which owns the order. They are not part
// of the crate's public API: only the pipeline calls them.
pub(super) use algebra::algebra;
pub(super) use beta::beta;
pub(super) use bounds::bounds;
pub(super) use cse::cse;
pub(super) use delta::delta;
pub(super) use eta::eta;
pub(super) use fold::fold;
pub(super) use iota::iota;
pub(super) use zeta::zeta;
