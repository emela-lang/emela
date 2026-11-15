//! Optimisation of the Core IR, between lowering and monomorphisation.
//!
//! The passes work on the target-neutral Core IR, so every target gets them. [`run`] is the fixed
//! sequence; there is no generic pass framework. A build turns all of them off together with
//! `--no-opt`, so the same source can be compiled with and without them and the observable
//! behaviour compared.
//!
//! Every pass preserves the observable behaviour of the program: evaluation order, effects,
//! failures, the order and count of the releases of `with`, and drop points. It may change memory
//! use and when a program runs out of memory (spec `[effect.handler]`, `[target.native]`). A pass
//! that cannot keep a contract gives up on that expression instead of changing it.

use crate::ir::Module;

/// What each Core IR pass changed in one [`run`], for tests and the differential harness.
///
/// The fields are added with the passes. An empty `Stats` says that no pass changed anything,
/// which is what a test of a pass must not see on the corpus it runs on.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {}

/// Runs the Core IR optimisation passes on `m` if `optimize`, and returns what they changed.
///
/// The sequence is the order the passes are listed in; a later pass may rely on an earlier one
/// having run. With `optimize` false the module is left untouched and `Stats` is empty.
pub fn run(_m: &mut Module, _optimize: bool) -> Stats {
    Stats {}
}
