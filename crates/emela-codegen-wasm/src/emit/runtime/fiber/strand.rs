//! Causes and the end of a strand: how a fiber or the run settles its result.

use super::*;

/// `fb_cause_new(kind, value) -> Cause`: the value is borrowed (the cause duplicates it).
pub(super) fn cause_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 2;
    let desc = plan(b).cause_desc;
    c(ins, 1);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(C);
    g(ins, C);
    g(ins, 0);
    ins.i32_store(mem(4, 4));
    g(ins, 1);
    ins.if_(BlockType::Empty);
    st_dup(b, ins, C, HDR, 1);
    ins.else_();
    st_const(ins, C, HDR, 0);
    ins.end();
    g(ins, C);
}

/// `fb_cause_string(cause) -> String`: `error: ...` or `defect: ...`. Borrows the cause.
pub(super) fn cause_string(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 1;
    const TMP: u32 = 2;
    let error = b.string_literal(b"error: ");
    let defect = b.string_literal(b"defect: ");
    let append = b.rt("str_append");
    let show = b.rt("show_failure");
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // `show_failure` borrows the cause's error; the cause keeps its own reference.
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(show);
    ins.local_tee(SHOWN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, error as i32);
    ins.return_();
    ins.end();
    c(ins, error as i32);
    g(ins, SHOWN);
    ins.call(append);
    ins.local_set(TMP);
    dropv(b, ins, SHOWN);
    g(ins, TMP);
    ins.return_();
    ins.end();
    c(ins, defect as i32);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(append);
}

/// Emits the `Any` value stored at `base`+`off`, unboxed to the representation code in local
/// `repr`, as an `i64` word in local `word` (the C runtime's `em_unbox`). A reference is the
/// value itself and is duplicated for the caller; `Void` is zero; any other value is the first
/// slot of its box.
pub(super) fn unbox_word(
    b: &mut Builder<'_>,
    ins: &mut InstructionSink,
    repr: u32,
    base: u32,
    off: u32,
    tmp: u32,
    word: u32,
) {
    ldl(ins, base, off);
    ins.local_set(tmp);
    g(ins, repr);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, tmp);
    call(b, ins, "dup");
    g(ins, tmp);
    ins.i64_extend_i32_u();
    ins.local_set(word);
    ins.else_();
    g(ins, repr);
    c(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.local_set(word);
    ins.else_();
    g(ins, repr);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, tmp);
    ins.i32_load(mem(HDR, 4));
    ins.i64_extend_i32_u();
    ins.local_set(word);
    ins.else_();
    g(ins, tmp);
    ins.i64_load(mem(HDR, 8));
    ins.local_set(word);
    ins.end();
    ins.end();
    ins.end();
}

/// `fb_watch_settle(w, f) -> (mode, word)`: what a fiber's end delivers to one of its watchers.
/// The value is borrowed from the fiber where it can be (the wait duplicates it).
pub(super) fn watch_settle(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const VERB: u32 = 2;
    const REPR: u32 = 3;
    const CAUSE: u32 = 4;
    const E: u32 = 5;
    const TMP: u32 = 6;
    const WORD: u32 = 7; // i64
    ldl(ins, 0, W_VERB);
    ins.local_set(VERB);
    ldl(ins, 0, W_REPR);
    ins.local_set(REPR);
    ldl(ins, 1, F_CAUSE);
    ins.local_set(CAUSE);
    g(ins, CAUSE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The child ended with a value. Its value is an `Any`: a reference is the value itself,
    // anything else is a box (the erased child returns `Any`), so unbox it to the expected
    // representation (the C runtime's `em_unbox`).
    g(ins, VERB);
    c(ins, WV_JOIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    unbox_word(b, ins, REPR, 1, F_RESVAL, TMP, WORD);
    c(ins, 0);
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    ins.end();
    // A task that failed or was interrupted has unwound its whole group, so a joiner that sees it
    // (only while it runs a release) unwinds as interrupted (`[task.cause]`).
    g(ins, VERB);
    c(ins, WV_JOIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    let intr = b.rt("fb_interrupted_new");
    ins.call(intr);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    c(ins, 2);
    ins.local_get(WORD);
    ins.return_();
    ins.end();
    c(ins, 0);
    ins.i64_const(0);
    ins.return_();
    let _ = (CAUSE, E);
}

/// `fb_strand_end(f, end, mode, word, perform)`: a continuation ended. The input is owned.
pub(super) fn strand_end(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const MODE: u32 = 5;
    const PERF: u32 = 6;
    const TMP: u32 = 7;
    const WORD: u32 = 8; // i64
    g(ins, 2);
    ins.local_set(MODE);
    ins.local_get(3);
    ins.local_set(WORD);
    g(ins, 4);
    ins.local_set(PERF);
    g(ins, 1);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, MODE);
    ins.local_get(WORD);
    let finish = b.rt("fb_finish");
    ins.call(finish);
    ins.return_();
    ins.end();
    g(ins, 1);
    c(ins, WE_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, PERF);
    g(ins, MODE);
    ins.local_get(WORD);
    let settled = b.rt("fb_settled_impl");
    ins.call(settled);
    dropv(b, ins, PERF);
    ins.return_();
    ins.end();
    g(ins, 1);
    c(ins, WE_SYNC);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    g(ins, MODE);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    // An orphan continuation: its defect is the run's result when nothing else has one (spec
    // 14.5), and an interruption is not a defect of the run.
    g(ins, MODE);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    let intr = b.rt("fb_is_interrupted");
    ins.call(intr);
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    i32_const_cell(b, ins, |p| p.orphan);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    i32_const_cell(b, ins, |p| p.orphan);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.return_();
    ins.end();
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.return_();
    ins.end();
    g(ins, MODE);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    let _ = TMP;
}

/// `fb_finish(f, mode, word)`: the fiber ended with the input (owned).
pub(super) fn finish(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const FLAGS: u32 = 3;
    const CAUSE: u32 = 4;
    const REPR: u32 = 5;
    const SCOPE: u32 = 6;
    const W: u32 = 7;
    const NEXT: u32 = 8;
    const TMP: u32 = 9;
    const WORD2: u32 = 10; // i64
    ldl(ins, 0, F_FLAGS);
    ins.local_set(FLAGS);
    g(ins, FLAGS);
    c(ins, FF_ENDED);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.else_();
    ldl(ins, 0, F_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.end();
    ins.return_();
    ins.end();
    g(ins, FLAGS);
    c(ins, FF_ENDED);
    ins.i32_or();
    ins.local_set(FLAGS);
    stl(ins, 0, F_FLAGS, FLAGS);
    st_const(ins, 0, F_WAITER, 0);
    c(ins, 0);
    ins.local_set(CAUSE);
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_FAIL);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.end();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let intr = b.rt("fb_is_interrupted");
    ins.call(intr);
    ins.if_(BlockType::Empty);
    c(ins, CAUSE_INTERRUPT);
    c(ins, 0);
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.else_();
    c(ins, CAUSE_DIE);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let make = b.rt("fb_cause_new");
    ins.call(make);
    ins.local_set(CAUSE);
    ins.end();
    ins.end();
    stl(ins, 0, F_CAUSE, CAUSE);
    ldl(ins, 0, F_REPR);
    ins.local_set(REPR);
    ldl(ins, 0, F_SCOPE);
    ins.local_set(SCOPE);
    g(ins, SCOPE);
    ins.if_(BlockType::Empty);
    // A child fiber: remember how it ended, then tell the scope and the watchers.
    stl(ins, 0, F_RESMODE, 1);
    ins.local_get(0);
    ins.local_get(2);
    ins.i64_store(mem(F_RESWORD, 8));
    c(ins, 0);
    ins.local_set(TMP);
    g(ins, 1);
    ins.i32_eqz();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.i32_store(mem(F_RESVAL, 4));
    ins.end();
    ins.else_();
    // The root fiber: its end is the run's result.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 1);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.else_();
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    c(ins, 2);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.else_();
    g(ins, REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    c(ins, 0);
    c(ins, 0);
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.end();
    ins.end();
    ins.end();
    // The fiber's own copy of the input is released once the cause or the result has it.
    g(ins, SCOPE);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    // The scope, then the fibers waiting for it.
    g(ins, SCOPE);
    g(ins, 0);
    let child_ended = b.rt("fb_scope_child_ended");
    ins.call(child_ended);
    ldl(ins, 0, F_WHEAD);
    ins.local_set(W);
    st_const(ins, 0, F_WHEAD, 0);
    st_const(ins, 0, F_WTAIL, 0);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, W);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, W, W_TNEXT);
    ins.local_set(NEXT);
    st_const(ins, W, W_TNEXT, 0);
    st_const(ins, W, W_TPREV, 0);
    ldl(ins, W, W_FLAGS);
    c(ins, -1 - WF_TARGET);
    ins.i32_and();
    ins.local_set(TMP);
    stl(ins, W, W_FLAGS, TMP);
    g(ins, W);
    g(ins, 0);
    let settle = b.rt("fb_watch_settle");
    ins.call(settle);
    // (mode, word): the word is on top.
    ins.local_set(WORD2);
    ins.local_set(FLAGS);
    g(ins, W);
    g(ins, FLAGS);
    ins.local_get(WORD2);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    g(ins, NEXT);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    // Out of the fiber's list; the list's reference is released at the end.
    ldl(ins, 0, F_PREVF);
    ins.local_set(TMP);
    g(ins, TMP);
    ins.if_(BlockType::Empty);
    stl(ins, TMP, F_NEXTF, NEXT);
    ins.else_();
    i32_const_cell(b, ins, |p| p.fhead);
    ldl(ins, 0, F_NEXTF);
    ins.i32_store(mem(0, 4));
    ins.end();
    ldl(ins, 0, F_NEXTF);
    ins.local_set(NEXT);
    g(ins, NEXT);
    ins.if_(BlockType::Empty);
    stl(ins, NEXT, F_PREVF, TMP);
    ins.else_();
    i32_const_cell(b, ins, |p| p.ftail);
    ldl(ins, 0, F_PREVF);
    ins.i32_store(mem(0, 4));
    ins.end();
    // A fiber that ends may still carry interruptions (one interrupted before it started ends
    // this way); their nodes are not reachable from the record once it is dropped.
    let clear = b.rt("fb_pending_clear");
    g(ins, 0);
    ins.call(clear);
    dropv(b, ins, 0);
}

/// `fb_run_result(mode, word)`: record the run's result (0 value, 1 failure, 2 defect). The word
/// is owned. The result outlives the strands still finishing, so `run_word` keeps its own
/// reference and `finish_run` puts it back before the exit report.
pub(super) fn run_result(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let run_over = cell(b, |p| p.run_over);
    let run_mode = cell(b, |p| p.run_mode);
    let run_word = cell(b, |p| p.run_word);
    let dup = b.rt("dup");
    let drop = b.rt("drop");
    // The first result stays (the C runtime's `em_root_finish`).
    c(ins, run_over as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.call(drop);
    ins.end();
    ins.return_();
    ins.end();
    c(ins, run_over as i32);
    c(ins, 1);
    ins.i32_store(mem(0, 4));
    c(ins, run_mode as i32);
    g(ins, 0);
    ins.i32_store(mem(0, 4));
    c(ins, run_word as i32);
    g(ins, 0);
    ins.if_(BlockType::Result(ValType::I32));
    g(ins, 1);
    ins.call(dup);
    g(ins, 1);
    ins.else_();
    c(ins, 0);
    ins.end();
    ins.i32_store(mem(0, 4));
    g(ins, 0);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    g(ins, 0);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    ins.global_set(G_DEFECT);
    ins.end();
}

/// `fb_settled_impl(perform, mode, word)`: the continuation of an operation implementation
/// ended. A defect or failure before the resume is the caller's input; after the resume it ends
/// the run (spec 14.5, the C runtime's `em_settled`). A value is not observable.
pub(super) fn settled_impl(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 3;
    const TMP: u32 = 4;
    const W: u32 = 5;
    let word: u32 = 2; // the i64 parameter
    let wake = b.rt("fb_wait_wake");
    // A defect.
    g(ins, 1);
    c(ins, 2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    c(ins, 2);
    ins.local_get(word);
    ins.i32_wrap_i64();
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.return_();
    ins.end();
    ins.local_get(0);
    c(ins, 1);
    ins.i32_store(mem(P_USED, 4));
    ldl(ins, 0, P_CALLER);
    ins.local_set(W);
    st_const(ins, 0, P_CALLER, 0);
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    call(b, ins, "drop");
    g(ins, W);
    c(ins, 2);
    ins.local_get(word);
    ins.call(wake);
    ins.else_();
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // A failure.
    g(ins, 1);
    c(ins, 1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(word);
    ins.i32_wrap_i64();
    let show = b.rt("show_failure");
    ins.call(show);
    ins.local_set(SHOWN);
    // `show_failure` borrows the failure; release it after building the message.
    ins.local_get(word);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ldl(ins, 0, P_WHERE);
    let after = plan(b).failed_after;
    c(ins, after as i32);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(TMP);
    g(ins, SHOWN);
    ins.if_(BlockType::Empty);
    g(ins, TMP);
    g(ins, SHOWN);
    ins.call(append);
    ins.local_set(TMP);
    ins.end();
    g(ins, TMP);
    let raise = b.rt("raise");
    ins.call(raise);
    dropv(b, ins, TMP);
    dropv(b, ins, SHOWN);
    c(ins, 2);
    ins.global_get(G_DEFECT);
    let result = b.rt("fb_run_result");
    ins.call(result);
    ins.return_();
    ins.end();
    ins.local_get(0);
    c(ins, 1);
    ins.i32_store(mem(P_USED, 4));
    ldl(ins, 0, P_CALLER);
    ins.local_set(W);
    st_const(ins, 0, P_CALLER, 0);
    g(ins, W);
    ins.if_(BlockType::Empty);
    g(ins, W);
    call(b, ins, "drop");
    g(ins, W);
    c(ins, 1);
    ins.local_get(word);
    ins.call(wake);
    ins.else_();
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
    ins.return_();
    ins.end();
    // A value after a resume is not observable; release it when it is a reference.
    ldl(ins, 0, P_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(word);
    ins.i32_wrap_i64();
    ins.drop();
    ins.end();
}
