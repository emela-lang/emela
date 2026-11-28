//! Waits: what a fiber is parked on, the targets it watches and its wakeup.

use super::*;

// ---- waits ----

/// Emits the head/tail slot offsets of the wait's target into locals `h` and `t`.
pub(super) fn target_slots(b: &mut Builder<'_>, ins: &mut InstructionSink, w: u32, h: u32, t: u32) {
    // A Deferred keeps its waiter list at D_WHEAD/D_WTAIL; a fiber at F_WHEAD/F_WTAIL.
    ldl(ins, w, W_KIND);
    c(ins, WK_DEFERRED);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, D_WHEAD as i32);
    ins.local_set(h);
    c(ins, D_WTAIL as i32);
    ins.local_set(t);
    ins.else_();
    c(ins, F_WHEAD as i32);
    ins.local_set(h);
    c(ins, F_WTAIL as i32);
    ins.local_set(t);
    ins.end();
    let _ = b;
}

/// `fb_wait_new(kind, repr) -> Ref`: a new wait, not yet registered or suspended.
pub(super) fn wait_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    let desc = plan(b).waiter_desc;
    c(ins, W_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(W);
    for off in [
        W_CHAIN, W_FIBER, W_TARGET, W_PREV, W_NEXT, W_LIVE, W_ONDROP, W_END, W_FLAGS, W_SEQ,
        W_HPREV, W_HNEXT, W_TPREV, W_TNEXT, W_IMPL, W_OWNER,
    ] {
        st_const(ins, W, off, 0);
    }
    stl(ins, W, W_KIND, 0);
    stl(ins, W, W_VERB, 0);
    stl(ins, W, W_REPR, 1);
    ins.local_get(W);
    ins.i64_const(0);
    ins.i64_store(mem(W_AT, 8));
    g(ins, W);
}

/// `fb_watch_add(target, w)`: add a wait to a fiber's or a Deferred's list of waiters.
pub(super) fn watch_add(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const T: u32 = 2;
    const HEAD: u32 = 3;
    const TAIL: u32 = 4;
    const TAILV: u32 = 5;
    g(ins, 0);
    ins.local_set(T);
    target_slots(b, ins, 1, HEAD, TAIL);
    stl(ins, 1, W_TARGET, 0);
    // `head`/`tail` are offsets from the target's record, held as the local values.
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(TAILV);
    stl(ins, 1, W_TPREV, TAILV);
    st_const(ins, 1, W_TNEXT, 0);
    g(ins, TAILV);
    ins.if_(BlockType::Empty);
    stl(ins, TAILV, W_TNEXT, 1);
    ins.else_();
    g(ins, T);
    g(ins, HEAD);
    ins.i32_add();
    g(ins, 1);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    g(ins, 1);
    ins.i32_store(mem(0, 4));
    ldl(ins, 1, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_or();
    ins.local_set(2);
    stl(ins, 1, W_FLAGS, 2);
}

/// `fb_watch_remove(w)`: take a wait out of its target's list.
pub(super) fn watch_remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const T: u32 = 2;
    const HEAD: u32 = 3;
    const TAIL: u32 = 4;
    const PREV: u32 = 5;
    const NEXT: u32 = 6;
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, W_TARGET);
    ins.local_set(T);
    target_slots(b, ins, 0, HEAD, TAIL);
    ldl(ins, 0, W_TPREV);
    ins.local_set(PREV);
    ldl(ins, 0, W_TNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_TNEXT, NEXT);
    ins.else_();
    g(ins, T);
    g(ins, HEAD);
    ins.i32_add();
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_TPREV, PREV);
    ins.else_();
    g(ins, T);
    g(ins, TAIL);
    ins.i32_add();
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(2);
    stl(ins, 0, W_FLAGS, 2);
    st_const(ins, 0, W_TARGET, 0);
    st_const(ins, 0, W_TPREV, 0);
    st_const(ins, 0, W_TNEXT, 0);
}

/// `fb_unlink_wait(w)`: take a wait out of every list it is in and forget the links to it.
pub(super) fn unlink_wait(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 2;
    const P: u32 = 3;
    const PREV: u32 = 4;
    const NEXT: u32 = 5;
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_TARGET);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    call(b, ins, "fb_watch_remove");
    ins.end();
    // The run's list, then the sleepers' list: both use their own pair of links.
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_ROOT);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, W_PREV);
    ins.local_set(PREV);
    ldl(ins, 0, W_NEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_NEXT, NEXT);
    ins.else_();
    c(ins, cell(b, |p| p.whead) as i32);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_PREV, PREV);
    ins.else_();
    c(ins, cell(b, |p| p.wtail) as i32);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, W_FLAGS);
    c(ins, -1 - WF_ROOT);
    ins.i32_and();
    ins.local_set(P);
    stl(ins, 0, W_FLAGS, P);
    c(ins, cell(b, |p| p.live_waits) as i32);
    c(ins, cell(b, |p| p.live_waits) as i32);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.end();
    for (flag, head, tail) in [
        (WF_HOST, cell(b, |p| p.host_head), cell(b, |p| p.host_tail)),
        (WF_TEST, cell(b, |p| p.test_head), cell(b, |p| p.test_tail)),
    ] {
        ldl(ins, 0, W_FLAGS);
        c(ins, flag);
        ins.i32_and();
        ins.if_(BlockType::Empty);
        ldl(ins, 0, W_HPREV);
        ins.local_set(PREV);
        ldl(ins, 0, W_HNEXT);
        ins.local_set(NEXT);
        g(ins, PREV);
        ins.if_(BlockType::Empty);
        stl(ins, PREV, W_HNEXT, NEXT);
        ins.else_();
        c(ins, head as i32);
        g(ins, NEXT);
        ins.i32_store(mem(0, 4));
        ins.end();
        g(ins, NEXT);
        ins.if_(BlockType::Empty);
        stl(ins, NEXT, W_HPREV, PREV);
        ins.else_();
        c(ins, tail as i32);
        g(ins, PREV);
        ins.i32_store(mem(0, 4));
        ins.end();
        ldl(ins, 0, W_FLAGS);
        c(ins, -1 - flag);
        ins.i32_and();
        ins.local_set(P);
        stl(ins, 0, W_FLAGS, P);
        ins.end();
    }
    st_const(ins, 0, W_LIVE, 0);
    // The fiber parked on it, and the operation call it belongs to.
    ldl(ins, 0, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    ins.if_(BlockType::Empty);
    ldl(ins, F, F_WAITER);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    st_const(ins, F, F_WAITER, 0);
    ins.end();
    ins.end();
    // The operation calls that pointed at this wait release it.
    ldl(ins, 0, W_ONDROP);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_CALLER);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_CALLER);
    ins.local_set(NEXT);
    st_const(ins, P, P_CALLER, 0);
    g(ins, NEXT);
    call(b, ins, "drop");
    ins.end();
    ins.end();
    ldl(ins, 0, W_IMPL);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    ins.local_set(NEXT);
    st_const(ins, P, P_IMPL, 0);
    g(ins, NEXT);
    call(b, ins, "drop");
    ins.end();
    ins.end();
    // The Semaphore or Queue of a line: a woken wait was taken out of the line by whoever served
    // it (`fb_drop_wait` takes the owner before it gets here, to leave the line).
    ldl(ins, 0, W_OWNER);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    st_const(ins, 0, W_OWNER, 0);
    g(ins, P);
    call(b, ins, "drop");
    ins.end();
    let _ = F;
}

/// `fb_drop_wait(w)`: a waiting continuation is abandoned (the run stalled, or its fiber is
/// interrupted). The implementation it waits for is unwound too, innermost first, as an orphan
/// continuation (`em_drop_wait` of the C runtime).
pub(super) fn drop_wait(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    const IW: u32 = 2;
    const CHAIN: u32 = 3;
    const F: u32 = 4;
    const OWNER: u32 = 5;
    ldl(ins, 0, W_LIVE);
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.return_();
    ins.end();
    st_const(ins, 0, W_LIVE, 0);
    ldl(ins, 0, W_OWNER);
    ins.local_set(OWNER);
    st_const(ins, 0, W_OWNER, 0);
    g(ins, 0);
    call(b, ins, "fb_unlink_wait");
    // A fiber interrupted while it waits in a line of a Semaphore or Queue leaves the line now,
    // and a semaphore serves its line again (`wait.onDrop`; the C runtime's `w->leave`).
    if plan(b).line.used {
        g(ins, OWNER);
        ins.if_(BlockType::Empty);
        g(ins, 0);
        g(ins, OWNER);
        c(ins, 1);
        call(b, ins, "fb_line_leave");
        dropv(b, ins, OWNER);
        ins.end();
    }
    ldl(ins, 0, W_ONDROP);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_USED);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ldl(ins, P, P_IMPL);
    ins.local_set(IW);
    g(ins, IW);
    ins.if_(BlockType::Empty);
    ldl(ins, IW, W_LIVE);
    ins.if_(BlockType::Empty);
    g(ins, IW);
    call(b, ins, "fb_drop_wait");
    ldl(ins, IW, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, IW, W_CHAIN, 0);
    ldl(ins, IW, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    call(b, ins, "dup");
    c(ins, JK_INTERRUPT);
    g(ins, F);
    g(ins, CHAIN);
    c(ins, WE_ORPHAN);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    // The recursion left the wait's own reference for this caller.
    dropv(b, ins, IW);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // The wait's references to operation calls go with it.
    for off in [W_ONDROP, W_IMPL] {
        ldl(ins, 0, off);
        ins.local_set(IW);
        st_const(ins, 0, off, 0);
        g(ins, IW);
        ins.if_(BlockType::Empty);
        g(ins, IW);
        call(b, ins, "drop");
        ins.end();
    }
}

/// `fb_wait_chain(w) -> Ref`: take the suspended continuation out of the wait.
pub(super) fn wait_chain(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 1;
    ldl(ins, 0, W_CHAIN);
    ins.local_set(C);
    st_const(ins, 0, W_CHAIN, 0);
    g(ins, C);
}

/// `fb_wait_wake(w, mode, word)`: continue the wait with the input. The wait is consumed.
///
/// A continuation of the fiber that is running resumes now, inside the wake; one of another
/// fiber is queued (the C runtime's `em_cont`). The event loop runs with nothing stepping, so its
/// wakes are queued too.
///
/// `fb_wait_served(w, mode, word)` is the same for a fiber given what it waited for in a line
/// (`served`): its queued job is a `JK_SERVED`, which does not deliver a pending interruption
/// (the C runtime's `em_cont_served`).
pub(super) fn wait_wake(b: &mut Builder<'_>, ins: &mut InstructionSink, served: bool) {
    const F: u32 = 3;
    const CHAIN: u32 = 4;
    const END: u32 = 5;
    const PERF: u32 = 6;
    const LIVE: u32 = 7;
    // A value that is unused (a second wakeup, an interruption of a finished fiber) is released.
    ldl(ins, 0, W_LIVE);
    ins.local_set(LIVE);
    g(ins, LIVE);
    ins.if_(BlockType::Empty);
    ins.else_();
    // Drop the input when the wait was already woken or unwound.
    g(ins, 1);
    ins.i32_eqz();
    ldl(ins, 0, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    // The caller still owns this wait: the wake consumes it even when it was already dead.
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    ins.if_(BlockType::Empty);
    ins.else_();
    // A live wait always names its fiber, so this is defensive; consume the wake like the dead
    // case so the caller's reference is not leaked.
    g(ins, 1);
    ins.i32_eqz();
    ldl(ins, 0, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    g(ins, F);
    call(b, ins, "dup");
    ldl(ins, 0, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, 0, W_CHAIN, 0);
    ldl(ins, 0, W_END);
    ins.local_set(END);
    // The operation call whose strand this continuation belongs to (0 for a fiber's own).  The
    // wait keeps the reference while it is unlinked: `fb_unlink_wait` clears the perform's
    // back-pointer and releases the wait reference the perform held.
    ldl(ins, 0, W_IMPL);
    ins.local_set(PERF);
    g(ins, 0);
    call(b, ins, "fb_unlink_wait");
    st_const(ins, 0, W_IMPL, 0);
    // The perform the wait held for a dropped caller is released with the wait.
    ldl(ins, 0, W_ONDROP);
    ins.local_set(LIVE);
    st_const(ins, 0, W_ONDROP, 0);
    g(ins, LIVE);
    ins.if_(BlockType::Empty);
    g(ins, LIVE);
    call(b, ins, "drop");
    ins.end();
    dropv(b, ins, 0);
    g(ins, CHAIN);
    ins.if_(BlockType::Empty);
    ins.else_();
    dropv(b, ins, F);
    dropv(b, ins, PERF);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // Resume inside the wake when the continuation is the fiber that is stepping now.
    i32_const_cell(b, ins, |p| p.stepping);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    g(ins, F);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, 1);
    ins.local_get(2);
    g(ins, PERF);
    let advance = b.rt("fb_advance");
    ins.call(advance);
    ins.return_();
    ins.end();
    ins.end();
    c(ins, if served { JK_SERVED } else { JK_RESUME });
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, 1);
    ins.local_get(2);
    g(ins, PERF);
    ldl(ins, 0, W_REPR);
    call(b, ins, "fb_queue_push");
}
