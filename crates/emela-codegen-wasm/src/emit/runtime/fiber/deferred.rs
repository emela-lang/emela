//! Forks, joins, Deferred values, fairness yields, holds and interruptions of a fiber.

use super::*;

/// `fb_fork(scope, f, thunk, repr) -> Fiber`: start a task (`[task.group]`).
pub(super) fn fork(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 4;
    const PREV: u32 = 5;
    const F: u32 = 6;
    let desc = plan(b).fiber_desc;
    let msg = plan(b).msg_scope_fork;
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_CLOSED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    raise_msg(b, ins, msg);
    dropv(b, ins, 1);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The child's record (also the user's handle).
    c(ins, F_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(C);
    ins.local_get(C);
    ins.i64_const(0);
    ins.i64_store(mem(F_RESWORD, 8));
    for off in [
        F_SCOPE, F_FLAGS, F_HOLDS, F_SCOPES, F_BUDGET, F_CAUSE, F_RESMODE, F_RESREF, F_RESVAL,
        F_WAITER, F_WHEAD, F_WTAIL, F_PHEAD, F_PTAIL, F_UHEAD, F_UTAIL, F_NEXTF, F_PREVF, F_ITEM,
        F_ITEMPREV, F_REPR, F_WAITING,
    ] {
        st_const(ins, C, off, 0);
    }
    st_dup(b, ins, C, F_SCOPE, 0);
    ldl(ins, C, F_FLAGS);
    c(ins, FF_LISTED);
    ins.i32_or();
    ins.local_set(PREV);
    stl(ins, C, F_FLAGS, PREV);
    st_const(ins, C, F_BUDGET, BUDGET);
    stl(ins, C, F_REPR, 3);
    // The run's list of live fibers.
    i32_const_cell(b, ins, |p| p.ftail);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREV);
    stl(ins, C, F_PREVF, PREV);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, F_NEXTF, C);
    ins.else_();
    i32_const_cell(b, ins, |p| p.fhead);
    g(ins, C);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.ftail);
    g(ins, C);
    ins.i32_store(mem(0, 4));
    // The scope's list of children.
    ldl(ins, 0, S_CTAIL);
    ins.local_set(PREV);
    stl(ins, C, F_ITEMPREV, PREV);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, F_ITEM, C);
    ins.else_();
    stl(ins, 0, S_CHEAD, C);
    ins.end();
    stl(ins, 0, S_CTAIL, C);
    // A background does not close: a task started after its with was interrupted from outside is
    // interrupted before it starts running, so it ends without calling `f` and `on_failure` is not
    // called for it. The release waits for it like any other task (`[task.background]`).
    ldl(ins, 0, S_FLAGS);
    c(ins, SF_BACKGROUND);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    let interrupted = b.rt("fb_scope_interrupted");
    ins.call(interrupted);
    ins.if_(BlockType::Empty);
    g(ins, C);
    c(ins, 0);
    let push = b.rt("fb_pending_push");
    ins.call(push);
    ins.end();
    ins.end();
    // The child starts when the parent next suspends (spec 11.2: fork is not a suspension point).
    c(ins, JK_BEGIN);
    g(ins, C);
    call(b, ins, "dup");
    g(ins, C);
    g(ins, 1);
    call(b, ins, "dup");
    g(ins, 1);
    c(ins, WE_MAIN);
    g(ins, 2);
    g(ins, 3);
    ins.i64_extend_i32_u();
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    g(ins, C);
    call(b, ins, "dup");
    g(ins, C);
    let _ = F;
}

/// `fb_join(h, repr) -> word`: wait for a task and return its value (`[task.cause]`).
pub(super) fn join(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const MODE: u32 = 3;
    const WORD: u32 = 7; // i64
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Already finished: the result or the defect is immediate.
    c(ins, WK_FIBER);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_JOIN);
    g(ins, W);
    g(ins, 0);
    let settle = b.rt("fb_watch_settle");
    ins.call(settle);
    ins.local_set(WORD);
    ins.local_set(MODE);
    dropv(b, ins, W);
    g(ins, MODE);
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    c(ins, WK_FIBER);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    st_const(ins, W, W_VERB, WV_JOIN);
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i64_const(0);
}

/// `fb_deferred() -> Deferred`: the init of the built-in handler `Deferred`, an empty Deferred
/// (`[task.deferred]`). Its with bounds its lifetime, so it belongs to no scope.
pub(super) fn deferred(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const D: u32 = 0;
    let desc = plan(b).deferred_desc;
    c(ins, D_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(D);
    st_const(ins, D, D_FULL, 0);
    st_const(ins, D, D_ISREF, 0);
    st_const(ins, D, D_VAL, 0);
    st_const(ins, D, D_WHEAD, 0);
    st_const(ins, D, D_WTAIL, 0);
    ins.local_get(D);
    ins.i64_const(0);
    ins.i64_store(mem(D_WORD, 8));
    g(ins, D);
}

/// `fb_complete(d, word, repr) -> Bool`: store the value if empty (`[task.deferred]`).
pub(super) fn complete(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 4;
    const NEXT: u32 = 5;
    const V: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    ins.local_get(1);
    ins.local_set(WORD);
    ldl(ins, 0, D_FULL);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    st_const(ins, 0, D_FULL, 1);
    g(ins, 2);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    st_const(ins, 0, D_ISREF, 1);
    // The Deferred keeps its own reference: the caller drops the value it passed.
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    call(b, ins, "dup");
    ins.local_get(0);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.i32_store(mem(D_VAL, 4));
    ins.else_();
    st_const(ins, 0, D_ISREF, 0);
    ins.end();
    ins.local_get(0);
    ins.local_get(WORD);
    ins.i64_store(mem(D_WORD, 8));
    ldl(ins, 0, D_WHEAD);
    ins.local_set(W);
    st_const(ins, 0, D_WHEAD, 0);
    st_const(ins, 0, D_WTAIL, 0);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_TNEXT);
    ins.local_set(NEXT);
    st_const(ins, W, W_TNEXT, 0);
    st_const(ins, W, W_TPREV, 0);
    st_const(ins, W, W_TARGET, 0);
    ldl(ins, W, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(TMP);
    stl(ins, W, W_FLAGS, TMP);
    ldl(ins, W, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ldl(ins, 0, D_VAL);
    ins.local_set(V);
    ins.else_();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.local_set(V);
    ins.end();
    g(ins, W);
    c(ins, 0);
    g(ins, V);
    ins.i64_extend_i32_u();
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    c(ins, 1);
}

/// `fb_deferred_await(d, repr) -> word`: wait until a value is stored (`[task.deferred]`).
pub(super) fn deferred_await(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ldl(ins, 0, D_FULL);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ldl(ins, 0, D_VAL);
    ins.i64_extend_i32_u();
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i64_load(mem(D_WORD, 8));
    ins.return_();
    ins.end();
    c(ins, WK_DEFERRED);
    g(ins, 1);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    g(ins, 0);
    g(ins, W);
    call(b, ins, "fb_watch_add");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
    ins.i64_const(0);
}

/// `fb_poll(d, repr, some, none) -> Option`: the stored value without waiting (`[task.deferred]`).
pub(super) fn poll(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 4;
    ldl(ins, 0, D_FULL);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 3);
    ins.return_();
    ins.end();
    c(ins, 1);
    g(ins, 2);
    call(b, ins, "alloc");
    ins.local_set(E);
    g(ins, 1);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, D_VAL);
    call(b, ins, "dup");
    ins.local_get(E);
    ldl(ins, 0, D_VAL);
    ins.i32_store(mem(HDR, 4));
    ins.else_();
    ins.local_get(E);
    ins.local_get(0);
    ins.i64_load(mem(D_WORD, 8));
    ins.i64_store(mem(HDR, 8));
    ins.end();
    g(ins, E);
}

/// `fb_yield(perform)`: the fairness budget (spec 11.1). A `perform` yield also delivers a pending
/// interruption (spec 11.4).
pub(super) fn yield_(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 1;
    const W: u32 = 2;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    let sync = super::sched::cell(b, |p| p.sync_depth);
    ins.i32_const(sync as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, F, F_BUDGET);
    c(ins, 1);
    ins.i32_sub();
    ins.local_set(W);
    stl(ins, F, F_BUDGET, W);
    ldl(ins, F, F_BUDGET);
    ins.i32_eqz();
    ins.i32_eqz();
    // budget > 0: only a `perform` yield may deliver an interruption.
    ins.if_(BlockType::Empty);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    call(b, ins, "fb_interrupt_here");
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.if_(BlockType::Empty);
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.end();
    st_const(ins, F, F_BUDGET, BUDGET);
    c(ins, WK_PAUSE);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_hold()` / `fb_unhold()`: a release that may suspend is not interrupted (spec 11.4).
pub(super) fn hold(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.if_(BlockType::Empty);
    // `F` stays the fiber in this branch: the count is read, changed and stored back through it.
    g(ins, F);
    g(ins, F);
    ins.i32_load(mem(F_HOLDS, 4));
    c(ins, 1);
    ins.i32_add();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

pub(super) fn unhold(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, F);
    ins.i32_load(mem(F_HOLDS, 4));
    c(ins, 1);
    ins.i32_sub();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

/// `fb_interrupt_here() -> i32`: deliver a pending interruption at a suspension point of the
/// current fiber's main continuation (spec 11.4).
pub(super) fn interrupt_here(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_tee(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    let sync = super::sched::cell(b, |p| p.sync_depth);
    ins.i32_const(sync as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.global_set(G_DEFECT);
    ins.i32_const(1);
}

/// `fb_interrupt_fiber(f, target)`: ask a fiber to unwind (spec 11.4). `target` is 0 for the whole
/// fiber, or a scope whose body runs on it.
pub(super) fn interrupt_fiber(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const P: u32 = 3;
    const CHAIN: u32 = 5;
    const END: u32 = 6;
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, 1);
    ins.if_(BlockType::Result(ValType::I32));
    g(ins, 0);
    g(ins, 1);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    g(ins, 0);
    g(ins, 1);
    let uh = b.rt("fb_unwinds_has");
    ins.call(uh);
    ins.i32_or();
    ins.else_();
    g(ins, 0);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.end();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, 0);
    g(ins, 1);
    let push = b.rt("fb_pending_push");
    ins.call(push);
    // The release of a background that waits for its tasks passes the interruption on to them
    // (`[task.background]`).
    ldl(ins, 0, F_WAITING);
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    ldl(ins, W, S_FLAGS);
    c(ins, SF_INTERRUPTED);
    ins.i32_or();
    ins.local_set(CHAIN);
    stl(ins, W, S_FLAGS, CHAIN);
    ldl(ins, W, S_CHEAD);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, P);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, P, F_ITEM);
    ins.local_set(CHAIN);
    g(ins, P);
    c(ins, 0);
    let again = b.rt("fb_interrupt_fiber");
    ins.call(again);
    g(ins, CHAIN);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ldl(ins, 0, F_HOLDS);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, F_WAITER);
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.return_();
    ins.end();
    // Dropping a caller also unwinds the implementation it waits for, and any implementation that
    // one waits for in turn, innermost first (spec 14.5; the C runtime's `em_drop_wait`).
    g(ins, W);
    let drop_wait = b.rt("fb_drop_wait");
    ins.call(drop_wait);
    // Then the fiber's own continuation, with the interruption when the job runs.
    ldl(ins, W, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, W, W_CHAIN, 0);
    ldl(ins, W, W_END);
    ins.local_set(END);
    g(ins, 0);
    ins.local_set(P);
    g(ins, P);
    call(b, ins, "dup");
    dropv(b, ins, W);
    c(ins, JK_INTERRUPT);
    g(ins, P);
    g(ins, CHAIN);
    g(ins, END);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
}
