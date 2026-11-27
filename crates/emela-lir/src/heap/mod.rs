//! The passes from the Flow IR to the Heap IR (memory made explicit), shared by the C and WASM
//! backends. They run in this order, each moving the module to the next [`crate::Stage`]:
//!
//! 1. [`convert_closures`]: closures become lifted functions and closure records.
//! 2. [`explicit_defects`]: defects and the cleanups of `protect` become explicit control.
//! 3. [`state_machines`]: suspending functions become resumable state machines, with their
//!    suspension points and the frames they save explicit.
//! 4. [`insert_rc`]: `dup` and `drop` (Perceus), last, on the final control flow (the suspension
//!    edges included).
//!
//! [`crate::to_heap_with_options`] runs them, with the LIR optimisation passes between them. Borrow
//! inference ([`crate::insert_borrow_signature`], behind [`crate::opt::LirRules::borrow`], on by
//! default since M3; C2b) is one of those and runs between the state machines and `insert_rc`.
//!
//! Each pass rebuilds the functions it changes (`copy::compact`), numbering the reachable
//! variables, statements, sequences and labels in the order met, so the dump's numbers differ from
//! those before the pass.

mod closures;
mod copy;
mod defects;
pub(crate) mod live;
pub(crate) mod rc;
pub(crate) mod rc_check;
pub(crate) mod suspend;

pub use closures::convert_closures;
pub(crate) use copy::{Copier, compact};
pub(crate) use defects::may_defect;
pub use defects::{builtin_may_defect, explicit_defects, fiber_may_defect};
pub use rc::insert_rc;
pub use suspend::{may_suspend, state_machines};
