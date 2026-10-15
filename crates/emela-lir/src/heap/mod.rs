//! The passes from the Flow IR to the Heap IR (memory made explicit), shared by the C and WASM
//! backends. They run in this order, each moving the module to the next [`crate::Stage`]:
//!
//! 1. [`convert_closures`]: closures become lifted functions and closure records.
//!
//! Functions that may suspend are only closure-converted; the suspend state machine that the other
//! passes need comes later.

mod closures;
mod copy;

pub use closures::convert_closures;

use crate::ir::Module;

/// Runs every Heap IR pass on a Flow IR module.
pub fn to_heap(m: &mut Module) {
    convert_closures(m);
}
