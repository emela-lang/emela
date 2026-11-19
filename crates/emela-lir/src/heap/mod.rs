//! The passes from the Flow IR to the Heap IR (memory made explicit), shared by the C and WASM
//! backends. They run in this order, each moving the module to the next [`crate::Stage`]:
//!
//! 1. [`convert_closures`]: closures become lifted functions and closure records.
//! 2. [`explicit_defects`]: defects and the cleanups of `protect` become explicit control.
//! 3. [`state_machines`]: suspending functions become resumable state machines, with their
//!    suspension points and the frames they save explicit.
//! 4. [`insert_borrow_signature`]: which parameters are borrowed, from the borrow facts
//!    ([`opt::LirRules::borrow`], on by default since M3; C2b).
//! 5. [`insert_rc`]: `dup` and `drop` (Perceus), last, on the final control flow (the suspension
//!    edges included).
//!
//! Each pass rebuilds the functions it changes (`copy::compact`), numbering the reachable
//! variables, statements, sequences and labels in the order met, so the dump's numbers differ from
//! those before the pass.

mod borrow;
mod closures;
mod copy;
mod defects;
pub(crate) mod live;
pub(crate) mod rc;
pub(crate) mod rc_check;
pub(crate) mod suspend;

pub use borrow::{BorrowReport, insert_borrow_signature};
pub use closures::convert_closures;
pub(crate) use copy::{Copier, compact};
pub(crate) use defects::may_defect;
pub use defects::{builtin_may_defect, explicit_defects, fiber_may_defect};
pub use rc::insert_rc;
pub use suspend::{may_suspend, state_machines};

use crate::ir::Module;
use crate::opt;

/// Runs every LIR optimisation and Heap IR pass on a Flow IR module.
pub fn to_heap(m: &mut Module) {
    let _ = to_heap_with(m, true);
}

/// [`to_heap`] with the LIR optimisation passes switched on or off.
///
/// The Flow IR passes run first, on the nested closures and the Flow IR control flow; then closure
/// conversion lifts the closures; then the closure-level passes run, before the state machines make
/// suspension points and frames explicit; then the Heap IR passes make memory and control explicit;
/// then the Heap IR optimisation passes run, with `dup` and `drop` explicit. The returned
/// [`opt::Stats`] is what all of them changed, so a build that turns optimisation off gets an
/// empty one.
///
/// This is [`to_heap_with_options`] with [`opt::Options::default`]: the M3 memory passes that pay
/// for themselves (borrow inference, the `rc` cleanup with the borrow elision and DCE) run; drop
/// specialisation, record SRA, closure devirtualisation and closure SRA stay off.
pub fn to_heap_with(m: &mut Module, optimize: bool) -> opt::Stats {
    to_heap_with_options(m, optimize, opt::Options::default())
}

/// [`to_heap_with`] with the LIR pass flags chosen by `options`.
///
/// The sequence is the one of [`to_heap_with`]. When `optimize` is on and
/// [`opt::LirRules::borrow`] is set, borrow inference runs after [`state_machines`] and before
/// [`insert_rc`], so reference counting follows each function's borrowed-parameter signature. With
/// `optimize` off no optimisation runs, borrow inference included; with [`opt::Options::default`]
/// this is exactly [`to_heap_with`].
pub fn to_heap_with_options(m: &mut Module, optimize: bool, options: opt::Options) -> opt::Stats {
    let mut stats = opt::run_flow(m, optimize);
    convert_closures(m);
    // SRA and closure devirtualisation need lifted closures, and must run before `state_machines`
    // fixes `Suspend::saved` (parent plan §4.3).
    stats.merge(if optimize {
        opt::run_mid_with(m, options)
    } else {
        opt::run_mid(m, false)
    });
    explicit_defects(m);
    state_machines(m);
    if optimize && options.rules.borrow {
        stats.merge(insert_borrow_signature(m).into_stats());
    }
    insert_rc(m);
    stats.merge(if optimize {
        opt::run_heap_with(m, options)
    } else {
        opt::run_heap(m, false)
    });
    stats
}
