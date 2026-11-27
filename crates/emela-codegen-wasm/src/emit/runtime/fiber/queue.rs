//! The run's queue of jobs and the driver that advances the continuations.

use super::*;

// ---- the run's queue ----

/// `fb_queue_push(kind, fiber, chain, end, mode, word, perform, repr)`: append a job. The job
/// takes the references; when the run is over they are released here. `repr` is the
/// representation of a value-mode `word` (0 when it cannot be a reference), so the drain can
/// release the value an interruption replaces.
pub(super) fn queue_push(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    list: impl Fn(&Plan) -> (u32, u32),
) {
    const J: u32 = 8;
    let desc = plan(b).job_desc;
    let (head, tail) = list(plan(b));
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    for l in [1u32, 2, 6] {
        dropv(b, ins, l);
    }
    ins.return_();
    ins.end();
    // A slot from the ring; when it is full, a heap job, so that no job is dropped or overwritten.
    // `fb_job_free` tells the two apart and returns the storage the job used.
    i32_const_cell(b, ins, |p| p.job_free);
    ins.i32_load(mem(0, 4));
    ins.local_tee(J);
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.job_free);
    ldl(ins, J, J_NEXT);
    ins.i32_store(mem(0, 4));
    ins.else_();
    c(ins, J_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(J);
    ins.end();
    stl(ins, J, J_KIND, 0);
    stl(ins, J, J_END, 3);
    stl(ins, J, J_FIBER, 1);
    stl(ins, J, J_CHAIN, 2);
    stl(ins, J, J_PERF, 6);
    stl(ins, J, J_MODE, 4);
    stl(ins, J, J_REPR, 7);
    ins.local_get(J);
    ins.local_get(5);
    ins.i64_store(mem(J_WORD, 8));
    st_const(ins, J, J_NEXT, 0);
    c(ins, tail as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    c(ins, tail as i32);
    ins.i32_load(mem(0, 4));
    g(ins, J);
    ins.i32_store(mem(J_NEXT, 4));
    ins.else_();
    c(ins, head as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
    ins.end();
    c(ins, tail as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
}

/// `fb_later_move()`: move the head of the later list to the run's queue, so the next drain runs
/// it after a turn of the event loop (the C runtime's `em_run`).
pub(super) fn later_move(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const J: u32 = 0;
    let lhead = cell(b, |p| p.lhead);
    let ltail = cell(b, |p| p.ltail);
    let qhead = cell(b, |p| p.qhead);
    let qtail = cell(b, |p| p.qtail);
    c(ins, lhead as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(J);
    g(ins, J);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    c(ins, lhead as i32);
    ldl(ins, J, J_NEXT);
    ins.i32_store(mem(0, 4));
    c(ins, lhead as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, ltail as i32);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    st_const(ins, J, J_NEXT, 0);
    c(ins, qtail as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    c(ins, qtail as i32);
    ins.i32_load(mem(0, 4));
    g(ins, J);
    ins.i32_store(mem(J_NEXT, 4));
    ins.else_();
    c(ins, qhead as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
    ins.end();
    c(ins, qtail as i32);
    g(ins, J);
    ins.i32_store(mem(0, 4));
}

/// `fb_job_free(job)`: give back the storage of a job whose references the caller has released
/// already (`drain` moved them into its locals and cleared their slots; `finish_run` releases
/// them before it calls this). A ring slot goes back onto the free list; a job the full ring
/// could not take is freed on the heap. The ring is static data below the heap, so the address
/// tells which storage the job used.
pub(super) fn job_free(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const J: u32 = 0;
    let base = plan(b).job_ring;
    g(ins, J);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, J);
    c(ins, base as i32);
    ins.i32_ge_u();
    g(ins, J);
    c(ins, (base + JOB_ENTRIES * JOB_BYTES) as i32);
    ins.i32_lt_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Back onto the free list; its queue link is not needed any more.
    g(ins, J);
    i32_const_cell(b, ins, |p| p.job_free);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(J_NEXT, 4));
    i32_const_cell(b, ins, |p| p.job_free);
    g(ins, J);
    ins.i32_store(mem(0, 4));
    ins.else_();
    // A heap job: its slots hold no references any more, so `drop` only frees the shell.
    dropv(b, ins, J);
    ins.end();
}

/// `fb_drain()`: run the queued jobs in order until the queue is empty or the run ends.
pub(super) fn drain(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const J: u32 = 0;
    const KIND: u32 = 1;
    const F: u32 = 2;
    const CHAIN: u32 = 3;
    const PERF: u32 = 4;
    const END: u32 = 5;
    const MODE: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    const REPR: u32 = 9;
    const WAIT: u32 = 10;
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.qhead);
    ins.i32_load(mem(0, 4));
    ins.local_tee(J);
    ins.i32_eqz();
    ins.br_if(1);
    // Pop it and cut the links.
    ldl(ins, J, J_NEXT);
    ins.local_set(TMP);
    i32_const_cell(b, ins, |p| p.qhead);
    g(ins, TMP);
    ins.i32_store(mem(0, 4));
    g(ins, TMP);
    ins.if_(BlockType::Empty);
    ins.else_();
    i32_const_cell(b, ins, |p| p.qtail);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, J, J_KIND);
    ins.local_set(KIND);
    ldl(ins, J, J_FIBER);
    ins.local_set(F);
    ldl(ins, J, J_CHAIN);
    ins.local_set(CHAIN);
    ldl(ins, J, J_PERF);
    ins.local_set(PERF);
    ldl(ins, J, J_END);
    ins.local_set(END);
    ldl(ins, J, J_MODE);
    ins.local_set(MODE);
    ldl(ins, J, J_REPR);
    ins.local_set(REPR);
    ins.local_get(J);
    ins.i64_load(mem(J_WORD, 8));
    ins.local_set(WORD);
    // The job's shell goes; the values are now in locals.
    st_const(ins, J, J_FIBER, 0);
    st_const(ins, J, J_CHAIN, 0);
    st_const(ins, J, J_PERF, 0);
    g(ins, J);
    call(b, ins, "fb_job_free");
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    dropv(b, ins, F);
    dropv(b, ins, CHAIN);
    dropv(b, ins, PERF);
    // The loop is one label out from inside this `if`.
    ins.br(1);
    ins.end();
    // A fairness yield: the job holds the pause wait, which holds the continuation (the C
    // runtime's `EM_JOB_GO`). Taking it here keeps the wait's links to the fiber and to the
    // operation call, so an interruption can still find and unwind them; a wait an interruption
    // already took is skipped.
    g(ins, KIND);
    c(ins, JK_GO);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, CHAIN);
    ins.local_set(WAIT);
    ldl(ins, WAIT, W_LIVE);
    ins.if_(BlockType::Empty);
    ldl(ins, WAIT, W_CHAIN);
    ins.local_set(CHAIN);
    st_const(ins, WAIT, W_CHAIN, 0);
    ldl(ins, WAIT, W_END);
    ins.local_set(END);
    ldl(ins, WAIT, W_IMPL);
    ins.local_set(PERF);
    g(ins, WAIT);
    call(b, ins, "fb_unlink_wait");
    // The perform reference moved to `PERF`; the wait must not drop it too.
    st_const(ins, WAIT, W_IMPL, 0);
    // The run's list and the job each owned a reference to the wait.
    dropv(b, ins, WAIT);
    dropv(b, ins, WAIT);
    c(ins, JK_RESUME);
    ins.local_set(KIND);
    ins.else_();
    // The wait was taken (an interruption unwound its continuation): drop the job's references
    // and go on with the next job (the loop is two labels out: the target `if` and the kind `if`).
    dropv(b, ins, WAIT);
    dropv(b, ins, F);
    ins.br(2);
    ins.end();
    ins.end();
    g(ins, KIND);
    c(ins, JK_BEGIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    g(ins, CHAIN);
    g(ins, MODE);
    g(ins, WORD);
    ins.i32_wrap_i64();
    let begin = b.rt("fb_begin");
    ins.call(begin);
    ins.br(1);
    ins.end();
    g(ins, KIND);
    c(ins, JK_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    // A queued wake resumes as the main continuation only when the fiber is not interrupted: the
    // value is not delivered, the interruption is (the C runtime's `em_fiber_resume`). A fairness
    // yield of the main continuation is queued the same way. A `JK_SERVED` wake skips this and
    // resumes with its value.
    g(ins, KIND);
    c(ins, JK_RESUME);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, END);
    c(ins, WE_MAIN);
    ins.i32_eq();
    g(ins, MODE);
    c(ins, 2);
    ins.i32_ne();
    ins.i32_and();
    g(ins, F);
    let interrupting = b.rt("fb_interrupting");
    ins.call(interrupting);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Release the value the interruption replaces: a failure word is always a reference, a value
    // word only in the reference representation.
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    g(ins, MODE);
    ins.i32_eqz();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    g(ins, F);
    let deliver = b.rt("fb_deliver");
    ins.call(deliver);
    ins.local_set(TMP);
    g(ins, TMP);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, F);
    g(ins, CHAIN);
    g(ins, END);
    g(ins, MODE);
    g(ins, WORD);
    g(ins, PERF);
    let advance = b.rt("fb_advance");
    ins.call(advance);
    ins.br(0);
    ins.end();
    ins.end();
}

/// `fb_advance(f, chain, end, mode, word, perform)`: run the continuation until it ends or
/// suspends. It takes the references to the fiber, the chain and the operation call.
pub(super) fn advance(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREVF: u32 = 6;
    const PREVM: u32 = 7;
    const FR: u32 = 8;
    const MODE: u32 = 9;
    const W: u32 = 10;
    const CHAIN: u32 = 11;
    const PREVS: u32 = 13;
    const WORD: u32 = 14; // i64
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVF);
    i32_const_cell(b, ins, |p| p.stepping);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVS);
    i32_const_cell(b, ins, |p| p.stepping);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVM);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, 2);
    ins.i32_eqz();
    ins.i32_store(mem(0, 4));
    g(ins, 3);
    ins.local_set(MODE);
    ins.local_get(4);
    ins.local_set(WORD);
    g(ins, 1);
    ins.local_set(CHAIN);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.run_over);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    dropv(b, ins, CHAIN);
    // The perform reference was handed to this continuation and is not used any more.
    dropv(b, ins, 5);
    g(ins, MODE);
    ins.if_(BlockType::Empty);
    g(ins, WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    // Inside the `if`, depth 2 is the end of the outer block (0: the `if`, 1: the loop).
    ins.br(2);
    ins.end();
    g(ins, CHAIN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, 2);
    g(ins, MODE);
    ins.local_get(WORD);
    g(ins, 5);
    let strand = b.rt("fb_strand_end");
    ins.call(strand);
    ins.br(2);
    ins.end();
    g(ins, CHAIN);
    ins.local_set(FR);
    ldl(ins, FR, F_NEXT);
    ins.local_set(CHAIN);
    st_const(ins, FR, F_NEXT, 0);
    sic(b, ins, |p| p.in_mode);
    g(ins, MODE);
    ins.i32_store(mem(0, 4));
    sic(b, ins, |p| p.in_word);
    ins.local_get(WORD);
    ins.i64_store(mem(0, 8));
    // The frame's resume trampoline takes the frame.
    let ty = b.call_ty(&[Repr::Ref], Repr::Void);
    g(ins, FR);
    g(ins, FR);
    ins.i32_load(mem(HDR, 4));
    ins.call_indirect(0, ty);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    let take = b.rt("frame_take");
    g(ins, CHAIN);
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
    g(ins, 2);
    g(ins, 5);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    dropv(b, ins, CHAIN);
    ins.end();
    // `fb_suspended` took its own reference to the perform; release the one this continuation
    // was called with.
    dropv(b, ins, 5);
    // Inside the suspension `if`, depth 2 is the end of the outer block: stop running the
    // continuation here; the wait resumes it when it is woken.
    ins.br(2);
    ins.end();
    // The frame's resume function reports the next frame's input as `out_mode`/`out_word`
    // (0 value, 1 failure, 2 defect; 3 is the suspension handled above), like the C runtime's
    // `$advance` reads it.
    sic(b, ins, |p| p.out_mode);
    ins.i32_load(mem(0, 4));
    ins.local_set(MODE);
    sic(b, ins, |p| p.out_word);
    ins.i64_load(mem(0, 8));
    ins.local_set(WORD);
    ins.br(0);
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, PREVF);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, PREVM);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.stepping);
    g(ins, PREVS);
    ins.i32_store(mem(0, 4));
    dropv(b, ins, 0);
}

/// `fb_begin(f, closure, thunk, repr)`: start a forked fiber by calling its function.
pub(super) fn begin(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREVF: u32 = 4;
    const PREVM: u32 = 5;
    const MODE: u32 = 6;
    const CHAIN: u32 = 7;
    const W: u32 = 8;
    const TMP: u32 = 9;
    const WORD: u32 = 11; // i64
    // A child interrupted before it started ends without calling its function (spec 11.2).
    g(ins, 0);
    c(ins, 0);
    let has = b.rt("fb_pending_has");
    ins.call(has);
    ins.if_(BlockType::Empty);
    dropv(b, ins, 1);
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.local_set(TMP);
    g(ins, 0);
    c(ins, 2);
    g(ins, TMP);
    ins.i64_extend_i32_u();
    let finish = b.rt("fb_finish");
    ins.call(finish);
    // The job's reference to the fiber (the run's list reference is the one `fb_finish` drops).
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVF);
    i32_const_cell(b, ins, |p| p.cur_main);
    ins.i32_load(mem(0, 4));
    ins.local_set(PREVM);
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    let ty = b.call_ty(&[Repr::Ref], Repr::I64);
    g(ins, 1);
    g(ins, 2);
    ins.call_indirect(0, ty);
    ins.local_set(WORD);
    dropv(b, ins, 1);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The child suspended: attach its continuation while it is still the current fiber (a
    // fairness yield queues it under its own record).
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
    // The wait took its own reference to the fiber; release the begin job's.
    dropv(b, ins, 0);
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    g(ins, PREVF);
    ins.i32_store(mem(0, 4));
    i32_const_cell(b, ins, |p| p.cur_main);
    g(ins, PREVM);
    ins.i32_store(mem(0, 4));
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_DEFECT);
    c(ins, 2);
    ins.local_set(MODE);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 0);
    ins.global_set(G_FAILURE);
    c(ins, 1);
    ins.local_set(MODE);
    ins.else_();
    c(ins, 0);
    ins.local_set(MODE);
    ins.end();
    ins.end();
    g(ins, 0);
    c(ins, WE_MAIN);
    g(ins, MODE);
    ins.local_get(WORD);
    c(ins, 0);
    let strand = b.rt("fb_strand_end");
    ins.call(strand);
    // The job's reference to the fiber: `fb_strand_end` (through `fb_finish`) drops the run's
    // list reference and may free the record.
    dropv(b, ins, 0);
}

/// `fb_suspended(w, chain, end, impl)`: attach the captured continuation to the request it
/// suspended at. `end` is whose continuation it is; an implementation continuation also names the
/// operation call it settles (`impl`). A fairness yield queues the fiber again.
pub(super) fn suspended(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const F: u32 = 4;
    const P: u32 = 5;
    const KIND: u32 = 6;
    const FLAGS: u32 = 7;
    const TMP: u32 = 8;
    ldl(ins, 0, W_KIND);
    ins.local_set(KIND);
    // A fairness yield waits like any other suspension (the C runtime's `em_suspended`): the
    // attach below links it to the fiber or to the operation call it belongs to, and the job
    // queued at the end resumes it. Interrupting the fiber then finds the wait and unwinds the
    // implementation.
    // Attach the captured frames to the request. Enclosing operation calls attach their own
    // frames as their implementation unwinds too, so append to the chain instead of replacing it
    // (the C runtime's `em_take_suspension`).
    g(ins, 1);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.else_();
    ldl(ins, 0, W_CHAIN);
    ins.local_set(KIND);
    g(ins, KIND);
    ins.if_(BlockType::Empty);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ldl(ins, KIND, F_NEXT);
    ins.local_tee(P);
    ins.i32_eqz();
    ins.br_if(1);
    g(ins, P);
    ins.local_set(KIND);
    ins.br(0);
    ins.end();
    ins.end();
    stl(ins, KIND, F_NEXT, 1);
    ins.else_();
    ins.local_get(0);
    g(ins, 1);
    ins.i32_store(mem(W_CHAIN, 4));
    ins.end();
    ins.end();
    i32_const_cell(b, ins, |p| p.cur_fiber);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    st_dup(b, ins, 0, W_FIBER, F);
    stl(ins, 0, W_END, 2);
    st_const(ins, 0, W_LIVE, 1);
    // The run's list of waits (newest last).
    i32_const_cell(b, ins, |p| p.wtail);
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    stl(ins, 0, W_PREV, P);
    st_const(ins, 0, W_NEXT, 0);
    g(ins, P);
    ins.if_(BlockType::Empty);
    stl(ins, P, W_NEXT, 0);
    ins.else_();
    i32_const_cell(b, ins, |p| p.whead);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    ins.end();
    i32_const_cell(b, ins, |p| p.wtail);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    ldl(ins, 0, W_FLAGS);
    c(ins, WF_ROOT);
    ins.i32_or();
    ins.local_set(FLAGS);
    stl(ins, 0, W_FLAGS, FLAGS);
    i32_const_cell(b, ins, |p| p.live_waits);
    i32_const_cell(b, ins, |p| p.live_waits);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    g(ins, 2);
    c(ins, WE_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The fiber's main continuation is parked here (the C runtime's `f->wait`).
    stl(ins, F, F_WAITER, 0);
    ins.else_();
    g(ins, 2);
    c(ins, WE_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // An implementation's continuation: the operation call settles when it ends. Both ways point
    // at each other, like `impl->impl_wait` and `w->impl` in the C runtime.
    g(ins, 3);
    ins.local_set(P);
    g(ins, P);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, 0, W_IMPL, P);
    st_dup(b, ins, P, P_IMPL, 0);
    ins.end();
    ins.end();
    ins.end();
    // Every 16th fairness yield waits for a turn of the event loop (the C runtime's `later`), so
    // a fiber that never waits still lets the others run in the same order.
    ldl(ins, 0, W_KIND);
    c(ins, WK_PAUSE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, cell(b, |p| p.pauses) as i32);
    ins.i32_load(mem(0, 4));
    c(ins, 1);
    ins.i32_add();
    ins.local_set(TMP);
    c(ins, cell(b, |p| p.pauses) as i32);
    g(ins, TMP);
    ins.i32_store(mem(0, 4));
    g(ins, TMP);
    c(ins, 16);
    ins.i32_rem_u();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, JK_GO);
    g(ins, F);
    call(b, ins, "dup");
    g(ins, F);
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    c(ins, 0);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let late = b.rt("fb_later_push");
    ins.call(late);
    ins.else_();
    c(ins, JK_GO);
    g(ins, F);
    call(b, ins, "dup");
    g(ins, F);
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    c(ins, 0);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    let push = b.rt("fb_queue_push");
    ins.call(push);
    ins.end();
    ins.end();
    let _ = F;
}
