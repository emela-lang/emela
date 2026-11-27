//! Host timers, TestClock sleepers and the idle loop that waits for them.

use super::*;

// ---- host timers and TestClock sleepers ----

/// `fb_sleep_start(handle)`: suspend on a host timer (the subtask joined to the waitable set).
pub(super) fn sleep_start(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 1;
    const TAIL: u32 = 2;
    c(ins, WK_HOST);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    stl(ins, W, W_SEQ, 0);
    stl(ins, W, W_HPREV, 0);
    st_const(ins, W, W_HNEXT, 0);
    i32_const_cell(b, ins, |p| p.host_tail);
    ins.i32_load(mem(0, 4));
    ins.local_set(TAIL);
    stl(ins, W, W_HPREV, TAIL);
    g(ins, TAIL);
    ins.if_(BlockType::Empty);
    stl(ins, TAIL, W_HNEXT, W);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_head);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.host_tail);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ldl(ins, W, W_FLAGS);
    c(ins, WF_HOST);
    ins.i32_or();
    ins.local_set(TAIL);
    stl(ins, W, W_FLAGS, TAIL);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_test_sleep(at)`: suspend until the run stalls and this is the earliest sleeper.
pub(super) fn test_sleep(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const TAIL: u32 = 3;
    const SEQ: u32 = 4;
    c(ins, WK_TEST);
    c(ins, 0);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    ins.local_get(W);
    ins.local_get(0);
    ins.i64_store(mem(W_AT, 8));
    i32_const_cell(b, ins, |p| p.test_seq);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.local_set(SEQ);
    i32_const_cell(b, ins, |p| p.test_seq);
    g(ins, SEQ);
    ins.i32_store(mem(0, 4));
    stl(ins, W, W_SEQ, SEQ);
    st_const(ins, W, W_HPREV, 0);
    st_const(ins, W, W_HNEXT, 0);
    i32_const_cell(b, ins, |p| p.test_tail);
    ins.i32_load(mem(0, 4));
    ins.local_set(TAIL);
    stl(ins, W, W_HPREV, TAIL);
    g(ins, TAIL);
    ins.if_(BlockType::Empty);
    stl(ins, TAIL, W_HNEXT, W);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_head);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.test_tail);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    ldl(ins, W, W_FLAGS);
    c(ins, WF_TEST);
    ins.i32_or();
    ins.local_set(TAIL);
    stl(ins, W, W_FLAGS, TAIL);
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_idle() -> i32`: 1 when a wake was queued, 0 when nothing can wake the run.
pub(super) fn idle(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const HANDLE: u32 = 1;
    const STATUS: u32 = 2;
    const EVENT: u32 = 3;
    const PREV: u32 = 4;
    const NEXT: u32 = 5;
    const TAIL: u32 = 6;
    const FOUND: u32 = 7;
    let set = cell(b, |p| p.wait_set);
    if !b.uses_std_host(StdHost::is_monotonic) {
        // Only the TestClock can wake this run; a host subtask is never started.
        let wake_test = b.rt("fb_wake_test");
        ins.call(wake_test);
        return;
    }
    i32_const_cell(b, ins, |p| p.host_head);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let wake_test = b.rt("fb_wake_test");
    ins.call(wake_test);
    ins.return_();
    ins.end();
    let set_wait = b.waitable_set_wait();
    c(ins, set as i32);
    ins.i32_load(mem(0, 4));
    c(ins, WAITBUF as i32);
    ins.call(set_wait);
    ins.local_set(EVENT);
    g(ins, EVENT);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(HANDLE);
    c(ins, (WAITBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STATUS);
    // Find the sleeper with this handle.
    c(ins, 0);
    ins.local_set(FOUND);
    i32_const_cell(b, ins, |p| p.host_head);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_SEQ);
    g(ins, HANDLE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(FOUND);
    // Out of the loop and its enclosing block (0: the `if`, 1: the loop, 2: the block).
    ins.br(2);
    ins.end();
    ldl(ins, W, W_HNEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    g(ins, FOUND);
    ins.if_(BlockType::Empty);
    // Unlink the sleeper.
    ldl(ins, FOUND, W_HPREV);
    ins.local_set(PREV);
    ldl(ins, FOUND, W_HNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_HNEXT, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_head);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_HPREV, PREV);
    ins.else_();
    i32_const_cell(b, ins, |p| p.host_tail);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, FOUND, W_HPREV, 0);
    st_const(ins, FOUND, W_HNEXT, 0);
    ldl(ins, FOUND, W_FLAGS);
    c(ins, -1 - WF_HOST);
    ins.i32_and();
    ins.local_set(TAIL);
    stl(ins, FOUND, W_FLAGS, TAIL);
    g(ins, STATUS);
    c(ins, 2);
    ins.i32_eq();
    g(ins, STATUS);
    c(ins, 4);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    let drop_subtask = b.subtask_drop();
    g(ins, HANDLE);
    ins.call(drop_subtask);
    g(ins, FOUND);
    c(ins, 0);
    ins.i64_const(0);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    c(ins, 1);
    ins.return_();
    ins.end();
    // Still pending: join it again and leave the run to stall. wasmtime 49 only returns terminal
    // statuses from `waitable-set.wait`, so leaving 0 here does not keep a run that could go on.
    let join = b.waitable_join();
    g(ins, HANDLE);
    c(ins, set as i32);
    ins.i32_load(mem(0, 4));
    ins.call(join);
    c(ins, 0);
    ins.return_();
    ins.end();
    // An event for no sleeper: consume it.
    let drop_subtask = b.subtask_drop();
    g(ins, HANDLE);
    ins.call(drop_subtask);
    c(ins, 1);
    ins.return_();
    ins.end();
    c(ins, 0);
}

/// `fb_wake_test() -> i32`: wake the earliest TestClock sleeper (by time, then by the order the
/// sleeps were called, spec 8.8).
pub(super) fn wake_test(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const BEST: u32 = 1;
    const PREV: u32 = 2;
    const NEXT: u32 = 3;
    const TAIL: u32 = 4;
    i32_const_cell(b, ins, |p| p.test_head);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    c(ins, 0);
    ins.local_set(BEST);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    g(ins, BEST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(BEST);
    ins.else_();
    g(ins, W);
    ins.i64_load(mem(W_AT, 8));
    g(ins, BEST);
    ins.i64_load(mem(W_AT, 8));
    ins.i64_lt_s();
    g(ins, W);
    ins.i64_load(mem(W_AT, 8));
    g(ins, BEST);
    ins.i64_load(mem(W_AT, 8));
    ins.i64_eq();
    ldl(ins, W, W_SEQ);
    ldl(ins, BEST, W_SEQ);
    ins.i32_lt_u();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    g(ins, W);
    ins.local_set(BEST);
    ins.end();
    ins.end();
    ldl(ins, W, W_HNEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    g(ins, BEST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    // Unlink the chosen sleeper.
    ldl(ins, BEST, W_HPREV);
    ins.local_set(PREV);
    ldl(ins, BEST, W_HNEXT);
    ins.local_set(NEXT);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, W_HNEXT, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_head);
    g(ins, NEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, W_HPREV, PREV);
    ins.else_();
    i32_const_cell(b, ins, |p| p.test_tail);
    g(ins, PREV);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, BEST, W_HPREV, 0);
    st_const(ins, BEST, W_HNEXT, 0);
    ldl(ins, BEST, W_FLAGS);
    c(ins, -1 - WF_TEST);
    ins.i32_and();
    ins.local_set(TAIL);
    stl(ins, BEST, W_FLAGS, TAIL);
    g(ins, BEST);
    c(ins, 0);
    ins.i64_const(0);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    c(ins, 1);
}
