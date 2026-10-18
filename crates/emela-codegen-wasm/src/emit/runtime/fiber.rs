//! The fiber runtime (spec chapter 11): `Fiber.scope`, `Fiber.fork`, `Fiber.join`,
//! `Deferred`, `Fiber.attempt` and the `Cause` operations.
//!
//! **Declared only.** `check::unsupported` rejects fibers before the Heap IR passes, so no body is
//! emitted here yet. The Fiber IR nodes are lowered in `emit::fiber`; this fragment will hold the
//! `fiber_new` / `fiber_resume` / `fiber_join` runtime functions and the run queue when the
//! scheduler ABI in `runtime::sched` is filled in.

use super::*;

/// The fiber fragment (currently empty).
pub(super) const FUNCS: &[Sig] = &[];

pub(super) fn body(_b: &mut Builder, _name: &str, _ins: &mut InstructionSink) -> bool {
    false
}

pub(super) fn locals(_name: &str) -> Option<Vec<(u32, ValType)>> {
    None
}
