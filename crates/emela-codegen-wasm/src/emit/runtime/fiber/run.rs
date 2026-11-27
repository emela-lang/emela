//! The call thunks and the run: its root fiber, its end and its interruption.

use super::*;

// ---- the call thunks ----
//
// A forked function and a scope body have the value's own representation, but the queued job and
// the scope frame have a fixed shape. One thunk per representation calls the closure and turns its
// result into a word; the reference-counted reference is already the word.

pub(super) fn thunk(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr, args: usize) {
    let mut params = vec![Repr::Ref];
    if args == 1 {
        params.push(Repr::Ref);
    }
    let ty = b.call_ty(&params, ret);
    g(ins, 0);
    if args == 1 {
        g(ins, 1);
    }
    g(ins, 0);
    ins.i32_load(mem(HDR, 4));
    ins.call_indirect(0, ty);
    match ret {
        Repr::I32 | Repr::Ref => {
            ins.i64_extend_i32_u();
        }
        Repr::I64 => {}
        Repr::F64 => {
            ins.i64_reinterpret_f64();
        }
        Repr::Void => {
            ins.i64_const(0);
        }
    }
}

pub(super) fn call0(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr) {
    thunk(b, ins, ret, 0);
}

pub(super) fn call1(b: &mut Builder<'_>, ins: &mut InstructionSink, ret: Repr) {
    thunk(b, ins, ret, 1);
}

// ---- the run ----

/// `fb_root_begin()`: make the run's root fiber the current one.
pub(super) fn root_begin(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 1;
    let desc = plan(b).fiber_desc;
    // An orphan defect left by the previous run was not the run's result (it only replaces an
    // interruption): release it before the cells are reset.
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    call(b, ins, "drop");
    ins.end();
    // Reset the run's state (a test run follows the previous one).
    for f in [
        |p: &Plan| p.fhead,
        |p: &Plan| p.ftail,
        |p: &Plan| p.whead,
        |p: &Plan| p.wtail,
        |p: &Plan| p.qhead,
        |p: &Plan| p.qtail,
        |p: &Plan| p.lhead,
        |p: &Plan| p.ltail,
        |p: &Plan| p.host_head,
        |p: &Plan| p.host_tail,
        |p: &Plan| p.test_head,
        |p: &Plan| p.test_tail,
        |p: &Plan| p.test_seq,
        |p: &Plan| p.run_mode,
        |p: &Plan| p.run_word,
        |p: &Plan| p.live_waits,
        |p: &Plan| p.orphan,
        |p: &Plan| p.pauses,
    ] {
        i32_const_cell(b, ins, f);
        c(ins, 0);
        ins.i32_store(mem(0, 4));
    }
    i32_const_cell(b, ins, |p| p.run_over);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The notes belong to the run's result, which is reported after the run: drop them when the
    // next run starts.
    let clear = b.rt("fb_notes_clear");
    ins.call(clear);
    c(ins, F_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(F);
    ins.local_get(F);
    ins.i64_const(0);
    ins.i64_store(mem(F_RESWORD, 8));
    for off in [
        F_SCOPE, F_FLAGS, F_HOLDS, F_SCOPES, F_BUDGET, F_CAUSE, F_RESMODE, F_RESREF, F_RESVAL,
        F_WAITER, F_WHEAD, F_WTAIL, F_PHEAD, F_PTAIL, F_UHEAD, F_UTAIL, F_NEXTF, F_PREVF, F_ITEM,
        F_ITEMPREV, F_REPR, F_WAITING,
    ] {
        st_const(ins, F, off, 0);
    }
    st_const(ins, F, F_FLAGS, FF_LISTED);
    st_const(ins, F, F_BUDGET, BUDGET);
    stl(ins, F, F_REPR, 0); // the root's result is not observed (the entry released it)
    i32_const_cell(b, ins, |p| p.fhead);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.ftail);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, F);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
}

/// `fb_root_run()`: after main or a test returned, finish the run.
pub(super) fn root_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    const CHAIN: u32 = 1;
    const W: u32 = 2;
    const MODE: u32 = 3;
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    i32_const_cell(b, ins, |p| p.susp_wait);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    i32_const_cell(b, ins, |p| p.susp_wait);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    g(ins, CHAIN);
    c(ins, WE_MAIN);
    c(ins, 0);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    dropv(b, ins, CHAIN);
    ins.end();
    ins.else_();
    c(ins, 0);
    ins.local_set(W);
    c(ins, 0);
    ins.local_set(MODE);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.local_set(W);
    c(ins, 0);
    ins.global_set(G_DEFECT);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(W);
    c(ins, 0);
    ins.global_set(G_FAILURE);
    c(ins, 1);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, F);
    g(ins, MODE);
    ins.local_get(W);
    ins.i64_extend_i32_u();
    let finish = b.rt("fb_finish");
    ins.call(finish);
    ins.end();
    // Run the queue; wait on the host or the TestClock only when nothing is runnable.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    let drain = b.rt("fb_drain");
    ins.call(drain);
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.br_if(1);
    // A job that waits for the event loop runs after the queue is empty; the C runtime moves one
    // per turn, so the jobs it adds run before the next.
    i32_const_cell(b, ins, |p| p.lhead);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    let later = b.rt("fb_later_move");
    ins.call(later);
    ins.br(1);
    ins.end();
    let has = b.rt("fb_has_waits");
    ins.call(has);
    ins.i32_eqz();
    ins.br_if(1);
    let idle = b.rt("fb_idle");
    ins.call(idle);
    ins.if_(BlockType::Empty);
    // Inside the `if`, depth 1 is the loop: a queued wake goes back to the drain, and only an
    // empty event loop falls through to the interruption of the run.
    ins.br(1);
    ins.end();
    let root_interrupt = b.rt("fb_root_interrupt");
    ins.call(root_interrupt);
    let drain = b.rt("fb_drain");
    ins.call(drain);
    // The stall unwinds as interrupted (spec 14.5 cuts off running releases).  The next turn
    // re-checks the later queue and the live waits before the run ends; the C runtime checks
    // `later`/`em_loop_busy` at the same point and ends when both are empty.
    ins.br(1);
    ins.end();
    ins.end();
    let finish_run = b.rt("fb_finish_run");
    ins.call(finish_run);
}

/// `fb_finish_run()`: release the waits and fibers left when the run ended (spec 14.5: their withs
/// are not released) and start the next run from a clean slate.
pub(super) fn finish_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 0;
    const NEXT: u32 = 1;
    const F: u32 = 2;
    // Put the run's result back: the strands that finished after the run ended cleared it from
    // the pending globals so they could not take it with them.
    let drop = b.rt("drop");
    i32_const_cell(b, ins, |p| p.run_mode);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.call(drop);
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_FAILURE);
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.end();
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_mode);
    ins.i32_load(mem(0, 4));
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.call(drop);
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.run_word);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.end();
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.run_word);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // A defect in an operation implementation's release wins over the run's interruption.
    let intr = b.rt("fb_is_interrupted");
    ins.global_get(G_DEFECT);
    ins.call(intr);
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.call(rt_drop(b));
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    i32_const_cell(b, ins, |p| p.orphan);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.end();
    // The waits whose continuations will not run.
    i32_const_cell(b, ins, |p| p.whead);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_NEXT);
    ins.local_set(NEXT);
    // Take it out of every list it is in (the run's, its target's, the host or TestClock
    // sleepers', the line of a Semaphore or Queue): the heads and tails must not keep pointing
    // at a released wait, and a line must not keep its fiber.
    if plan(b).line.used {
        ldl(ins, W, W_OWNER);
        ins.if_(BlockType::Empty);
        g(ins, W);
        ldl(ins, W, W_OWNER);
        c(ins, 0);
        call(b, ins, "fb_line_leave");
        ins.end();
    }
    g(ins, W);
    call(b, ins, "fb_unlink_wait");
    dropv(b, ins, W);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.whead);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.wtail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.live_waits);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The fibers still in the run's list.
    i32_const_cell(b, ins, |p| p.fhead);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, F);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, F, F_NEXTF);
    ins.local_set(NEXT);
    st_const(ins, F, F_NEXTF, 0);
    st_const(ins, F, F_PREVF, 0);
    g(ins, F);
    call(b, ins, "fb_pending_clear");
    dropv(b, ins, F);
    g(ins, NEXT);
    ins.local_set(F);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.fhead);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.ftail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    // The jobs a "failed after resume" left, queued or waiting for the event loop.
    for list in [|p: &Plan| p.qhead, |p: &Plan| p.lhead] {
        i32_const_cell(b, ins, list);
        ins.i32_load(mem(0, 4));
        ins.local_set(W);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        g(ins, W);
        ins.i32_eqz();
        ins.br_if(1);
        ldl(ins, W, J_NEXT);
        ins.local_set(NEXT);
        // The run ended before the job ran: its references are still in the record. Clear them
        // before `fb_job_free`, which then only returns the storage.
        for off in [J_FIBER, J_CHAIN, J_PERF] {
            ldl(ins, W, off);
            call(b, ins, "drop");
            st_const(ins, W, off, 0);
        }
        g(ins, W);
        call(b, ins, "fb_job_free");
        g(ins, NEXT);
        ins.local_set(W);
        ins.br(0);
        ins.end();
        ins.end();
    }
    for list in [
        |p: &Plan| p.qhead,
        |p: &Plan| p.qtail,
        |p: &Plan| p.lhead,
        |p: &Plan| p.ltail,
    ] {
        i32_const_cell(b, ins, list);
        c(ins, 0);
        ins.i32_store(mem(0, 4));
    }
    i32_const_cell(b, ins, |p| p.cur_fiber);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    let _ = W;
}

pub(super) fn rt_drop(b: &mut Builder<'_>) -> u32 {
    b.rt("drop")
}

/// `fb_has_waits() -> i32`: whether a continuation is still registered.
pub(super) fn has_waits(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    i32_const_cell(b, ins, |p| p.live_waits);
    ins.i32_load(mem(0, 4));
    let _ = b;
}

/// `fb_root_interrupt()`: nothing can go on; interrupt every fiber and unwind every waiting
/// continuation (spec 14.5). Scopes waiting for their children keep waiting.
pub(super) fn root_interrupt(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 0;
    const W: u32 = 1;
    const PREV: u32 = 2;
    const CHAIN: u32 = 3;
    const END: u32 = 4;
    const P: u32 = 5;
    // Every fiber gets a whole-fiber interruption.
    i32_const_cell(b, ins, |p| p.fhead);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, F);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, F, F_FLAGS);
    c(ins, FF_UNWINDING);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, F);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, F);
    c(ins, 0);
    let push = b.rt("fb_pending_push");
    ins.call(push);
    ins.end();
    ins.end();
    ldl(ins, F, F_NEXTF);
    ins.local_set(F);
    ins.br(0);
    ins.end();
    ins.end();
    // The live waits of fibers' main continuations, newest first: their continuations unwind
    // with their releases. An implementation waits only before it resumes (E4305), and is
    // unwound by dropping its caller (`fb_drop_wait`).
    i32_const_cell(b, ins, |p| p.wtail);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_PREV);
    ins.local_set(PREV);
    ldl(ins, W, W_FLAGS);
    c(ins, WF_CLOSING);
    ins.i32_and();
    ins.i32_eqz();
    ldl(ins, W, W_LIVE);
    ins.i32_and();
    ldl(ins, W, W_END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ldl(ins, W, W_FIBER);
    ins.local_set(F);
    g(ins, F);
    call(b, ins, "dup");
    ldl(ins, W, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, W, W_CHAIN, 0);
    c(ins, WE_MAIN);
    ins.local_set(END);
    // Drop the wait first: a caller's implementation is unwound before the caller, and its
    // continuation is queued ahead of this one (the C runtime's `em_drop_wait`).
    g(ins, W);
    let drop_wait = b.rt("fb_drop_wait");
    ins.call(drop_wait);
    dropv(b, ins, W);
    c(ins, JK_INTERRUPT);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    ins.end();
    g(ins, PREV);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    let _ = P;
}
