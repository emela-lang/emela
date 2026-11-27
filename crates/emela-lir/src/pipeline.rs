//! The whole LIR pipeline after [`crate::from_core`]: the LIR optimisation passes ([`crate::opt`])
//! interleaved with the passes from the Flow IR to the Heap IR ([`crate::heap`]).
//!
//! It is apart from both, because it calls into both: the Heap IR passes do not depend on the
//! optimisation passes, and the optimisation passes use the Heap IR's helpers.

use crate::heap::{convert_closures, explicit_defects, insert_rc, state_machines};
use crate::ir::Module;
use crate::opt;
use crate::opt::borrow_signature::insert_borrow_signature;

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
/// for themselves (borrow inference, the `rc` cleanup with the borrow elision and DCE), the switch
/// normalisation, and closure devirtualisation with closure SRA run; drop specialisation and record
/// SRA stay off.
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
