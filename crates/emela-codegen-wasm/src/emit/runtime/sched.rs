//! The scheduler and the suspend machinery (spec chapters 11 and 14).
//!
//! **Declared only.** The emitter does not read the Heap IR's suspend state machines yet, so this
//! fragment emits no runtime function and the scheduler ABI below is a contract for a later wave.
//!
//! The convention a later wave fills in:
//!
//! - A suspending call returns a *frame*: `{ resumable, resume_input, payload, … }`. It suspends
//!   by leaving a non-null frame in `G_SUSPENDING` and returning a dummy value, with the resume
//!   input taken from the `i64` global `G_RESUME` (the `resume` `runtime op` prints it; a `yield`
//!   reads it back).
//! - Resuming enters `loop $dispatch` and `br_table`s on the frame's state tag: state 0 runs the
//!   body, state `k` jumps into the `k`-th resume point, and the default state unwinds. `G_FAILURE`
//!   carries a `fail` value out of a suspension edge and `G_DEFECT` a defect, exactly as a normal
//!   return does (the same two globals the non-suspending path uses).
//! - `Fiber` (see `runtime::fiber`) and the `FileSystem`/io work (see `runtime::fs`) are separate
//!   fragments so each can land as its own change.

use super::*;

/// The scheduler fragment (currently empty).
pub(super) const FUNCS: &[Sig] = &[];

// ---- the suspend ABI (declared only, no behavior yet) ----

/// The state tag of a suspended frame: the `br_table` selector. State 0 runs the body from the
/// top, state `k` jumps into the `k`-th resume point, and the default state unwinds.
#[allow(dead_code)]
pub(crate) const FRAME_STATE: u32 = 0;
/// The `i64` resume input of a suspended frame (the value a `resume`/`yield` delivered).
#[allow(dead_code)]
pub(crate) const FRAME_RESUME: u32 = 4;
/// The payload a suspended call left behind (its destination value, or a failure/defect marker).
#[allow(dead_code)]
pub(crate) const FRAME_PAYLOAD: u32 = 16;

/// The resume-input `i64` global. It is appended after `G_STRDESC` (index 6) when the state
/// machines are read, at which point `emit::GLOBALS` grows from 6 to 7 and the global section
/// gets one more mutable `i64` (initialised to 0). Declared here, not emitted yet.
#[allow(dead_code)]
pub(crate) const G_RESUME: u32 = 6;

pub(super) fn body(_b: &mut Builder, _name: &str, _ins: &mut InstructionSink) -> bool {
    false
}

pub(super) fn locals(_name: &str) -> Option<Vec<(u32, ValType)>> {
    None
}
