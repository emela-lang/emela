//! The suspension edges of the Heap IR (spec chapters 11 and 14).
//!
//! **Declared only.** The emitter does not read the `to_heap` suspend state machines yet;
//! `check::unsupported` rejects suspending code before this runs, so `on_suspend` is a bug if it
//! is ever `Some`.
//!
//! The ABI a later wave fills in (the same one `runtime/sched.rs` documents):
//!
//! - A `Stmt::Let` with an `on_suspend` landing resumes at the top of the landing sequence: the
//!   call that suspended left a frame in `G_SUSPENDING` and returned a dummy; `G_RESUME` holds
//!   the `i64` resume input. The edge tests `G_SUSPENDING`, clears it, and branches into the
//!   landing with the resume input bound to the statement's `dst`.
//! - `G_FAILURE` and `G_DEFECT` carry a `fail` value and a defect out of a suspension exactly as
//!   they carry them out of a normal return, so the existing failure/defect landings keep working
//!   across a suspension.
//! - The dispatch itself is `loop $dispatch` + `br_table` on the frame's state tag (state 0 runs
//!   the body, state `k` jumps into the `k`-th resume point); see `runtime::sched`.

use super::*;

impl<'m> Builder<'m> {
    /// `Stmt::Let`'s `on_suspend` edge: unreachable while suspending code is rejected by the
    /// check pass.
    pub(super) fn on_suspend(&mut self, cx: &FnCtx, on_suspend: Option<emela_lir::Suspend>) {
        if on_suspend.is_some() {
            self.internal(format!("suspension edge left in {}", cx.f.name));
        }
    }
}
