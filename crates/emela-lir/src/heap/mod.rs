//! The passes from the Flow IR to the Heap IR (memory made explicit), shared by the C and WASM
//! backends. They run in this order, each moving the module to the next [`crate::Stage`]:
//!
//! 1. [`convert_closures`]: closures become lifted functions and closure records.
//! 2. [`explicit_defects`]: defects and the cleanups of `protect` become explicit control.
//! 3. [`insert_rc`]: `dup` and `drop` (Perceus), last, on the final control flow.
//!
//! Functions that may suspend are only closure-converted; the suspend state machine that the other
//! passes need comes later.

mod closures;
mod copy;
mod defects;
mod rc;
pub(crate) mod rc_check;

pub use closures::convert_closures;
pub(crate) use defects::may_defect;
pub use defects::{builtin_may_defect, explicit_defects, fiber_may_defect};
pub use rc::insert_rc;

use crate::ir::Module;

/// Runs every Heap IR pass on a Flow IR module.
pub fn to_heap(m: &mut Module) {
    convert_closures(m);
    explicit_defects(m);
    insert_rc(m);
}
