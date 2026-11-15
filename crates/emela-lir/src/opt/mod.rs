//! Optimisation of the low IR (LIR), after the Core IR and before code generation.
//!
//! Two stages take part, because they see different things:
//!
//! - [`run_flow`] works on the Flow IR, after `from_core` and before closure conversion, where
//!   closures are still nested and values are still variables.
//! - [`run_heap`] works on the Heap IR, after reference counting has inserted `dup` and `drop`,
//!   where every allocation and every reference is explicit.
//!
//! Both are called by [`crate::to_heap_with`], so a build switches all of them off together with
//! `--no-opt`. There is no generic pass framework: each stage runs a fixed sequence.
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory. A pass that cannot keep its contract gives up on
//! that function instead of changing it.

use crate::ir::Module;

/// What each LIR pass changed in one run, for tests and the differential harness.
///
/// The fields are added with the passes. An empty `Stats` says that no pass changed anything,
/// which is what a test of a pass must not see on the corpus it runs on.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {}

impl Stats {
    /// Adds what `other` changed to these.
    pub fn merge(&mut self, other: Stats) {
        let Stats {} = other;
    }
}

/// Runs the Flow IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// The function must be at [`crate::Stage::Flow`]: the passes run before closure conversion, which
/// keeps the order of the Heap IR passes fixed (`heap`).
pub fn run_flow(_m: &mut Module, _optimize: bool) -> Stats {
    Stats {}
}

/// Runs the Heap IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// The function must be at the stage of the Heap IR (`insert_rc` has run), where `dup` and `drop`
/// are explicit. Drop points are not moved: the passes may remove a `dup` and its matching `drop`
/// together, and make the last use of a reference a move, but never change when a value is
/// released.
pub fn run_heap(_m: &mut Module, _optimize: bool) -> Stats {
    Stats {}
}
