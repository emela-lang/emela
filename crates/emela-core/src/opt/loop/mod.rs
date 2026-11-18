//! Loop transformations: LICM, unswitching and induction variables.
//!
//! These transformations are not reductions. They move work between the inside and the outside of
//! a loop, or change the counter that a loop walks, so their contracts name the loop they apply to
//! and the evaluations they move instead of an equation between expressions.
//!
//! The module is the skeleton of A0/PR-002 and holds no code yet. Its passes arrive in A9/PR-018:
//!
//! - **Loop-invariant code motion (LICM)**: a computation in a loop whose operands are bound
//!   outside the loop is evaluated once before it. This is the part of the current
//!   `super::hoist` that moves `let x = y` and `let x = <literal>`.
//! - **Unswitching**: an `if` inside a loop whose condition is invariant is lifted around the loop,
//!   so the test runs once. This is the second part of `super::hoist`.
//! - **Induction variables**: a counter whose value is an affine function of the loop counter is
//!   replaced by an affine update, or a derived counter is removed when the base counter and the
//!   step are known. B5/PR-024 supplies the ranges this needs.
//!
//! Until A9/PR-018 lands, `super::hoist` keeps running unchanged in the pipeline. It is not moved
//! or rewritten by this skeleton, so the pipeline's behaviour stays as it is.
