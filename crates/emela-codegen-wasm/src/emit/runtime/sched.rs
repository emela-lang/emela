//! Frames, `suspend fn` operation calls and in-place unwinding.
//!
//! The continuation scheduler, the fibers and the waits live in [`super::fiber`]; this module
//! holds what an operation call and a captured frame need (spec 8.4, 8.7, 14.5):
//!
//! - A frame is a heap object: the resume trampoline's table index, the resume point, the next
//!   frame and the saved variables. The frames of a captured continuation are a singly linked
//!   list, innermost first.
//! - A `perform` record is the state of one `suspend fn` operation call: the caller's wait, the
//!   implementation's wait, whether the implementation resumed it and the value it resumed with.
//!   The `resume` value handed to the implementation is a closure whose environment is the
//!   record.
//! - A resume while the implementation is still running continues the caller with the value after
//!   the implementation returns (in place, spec 14.5 item 3). A resume after it returned wakes the
//!   caller's wait; the wait's queue keeps the order of the wakeups.
//! - The whole continuation and the fibers are run by `runtime::fiber`.
//!
//! All of the statics are allocated by `emit::suspend::prepare`; the module runs on one thread.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    (
        "frame_new",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("frame_push", &[ValType::I32], &[]),
    ("frame_take", &[ValType::I32], &[ValType::I32]),
    ("interrupted", &[], &[ValType::I32]),
    (
        "perform_new",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("perform_end", &[ValType::I32], &[]),
    ("resume_void", &[ValType::I32], &[]),
    ("resume_i32", &[ValType::I32, ValType::I32], &[]),
    ("resume_i64", &[ValType::I32, ValType::I64], &[]),
    ("resume_f64", &[ValType::I32, ValType::F64], &[]),
    ("resume_ref", &[ValType::I32, ValType::I32], &[]),
    ("sync_enter", &[], &[]),
    ("sync_leave", &[], &[]),
    (
        "resume_with",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
    ("where_new", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("handler_name", &[ValType::I32], &[ValType::I32]),
    ("unwind_sync", &[ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "frame_new" => i32s(1),
        "frame_push" => i32s(1),
        "frame_take" => i32s(2),
        "perform_new" => i32s(6),
        // 1: the perform, 2: the input word (i64), 3-6: temporaries.
        "perform_end" => vec![(1, ValType::I32), (1, ValType::I64), (5, ValType::I32)],
        "handler_name" => i32s(5),
        "where_new" => i32s(2),
        "resume_with" => i32s(4),
        "sync_leave" => i32s(3),
        "unwind_sync" => i32s(6),
        _ => Vec::new(),
    }
}

/// The slots of a `perform` record (see `emit::suspend`'s descriptor).
pub(super) const P_USED: u32 = HDR;
pub(super) const P_PHASE: u32 = HDR + 8;
pub(super) const P_WHERE: u32 = HDR + 16;
pub(super) const P_NOW_MODE: u32 = HDR + 24;
pub(super) const P_NOW_WORD: u32 = HDR + 32;
/// The caller's wait while it is parked (a reference), or 0.
pub(super) const P_CALLER: u32 = HDR + 40;
pub(super) const P_HAS_CALLER: u32 = HDR + 48;
/// The `sync_enter` depth the call was made at (spec 8.7).
pub(super) const P_SYNC: u32 = HDR + 56;
/// The implementation's wait while it is suspended (a reference), or 0.
pub(super) const P_IMPL: u32 = HDR + 64;
pub(super) const P_NEXT: u32 = HDR + 72;
/// The `cur_main` of the caller (interruption is not delivered inside an implementation).
pub(super) const P_PREVMAIN: u32 = HDR + 80;
/// The operation's result representation (for an implementation that ends after a resume).
pub(super) const P_REPR: u32 = HDR + 88;
const P_SLOTS: i32 = 12;

/// A frame's slots.
const F_RESUME: u32 = HDR;
pub(super) const F_STATE: u32 = HDR + 8;
pub(super) const F_NEXT: u32 = HDR + 16;

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "frame_new" => frame_new(b, ins),
        "frame_push" => frame_push(b, ins),
        "frame_take" => frame_take(b, ins),
        "interrupted" => interrupted(b, ins),
        "perform_new" => perform_new(b, ins),
        "perform_end" => perform_end(b, ins),
        "resume_void" => resume_void(b, ins),
        "resume_i32" => resume_i32(b, ins),
        "resume_i64" => resume_i64(b, ins),
        "resume_f64" => resume_f64(b, ins),
        "resume_ref" => resume_ref(b, ins),
        "sync_enter" => sync_enter(b, ins),
        "sync_leave" => sync_leave(b, ins),
        "resume_with" => resume_with(b, ins),
        "where_new" => where_new(b, ins),
        "handler_name" => handler_name(b, ins),
        "unwind_sync" => unwind_sync(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn cell(b: &Builder<'_>, f: impl Fn(&super::super::suspend::Plan) -> u32) -> u32 {
    f(b.sus.as_ref().expect("the suspend runtime"))
}

fn cur_main_cell(b: &Builder<'_>) -> u32 {
    super::fiber::plan(b).cur_main
}

fn susp_wait_cell(b: &Builder<'_>) -> u32 {
    super::fiber::plan(b).susp_wait
}

// ---- frames ----

fn frame_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 4;
    let alloc = b.rt("alloc");
    ins.local_get(3);
    ins.i32_const(3);
    ins.i32_add();
    ins.local_get(0);
    ins.call(alloc);
    ins.local_tee(P);
    ins.local_get(1);
    ins.i32_store(mem(F_RESUME, 4));
    ins.local_get(P);
    ins.local_get(2);
    ins.i32_store(mem(F_STATE, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(F_NEXT, 4));
    ins.local_get(P);
}

fn frame_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let head = cell(b, |p| p.chain_head);
    let tail = cell(b, |p| p.chain_tail);
    ins.i32_const(tail as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(1);
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.local_get(0);
    ins.i32_store(mem(F_NEXT, 4));
    ins.else_();
    ins.i32_const(head as i32);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.i32_const(tail as i32);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
}

fn frame_take(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let head = cell(b, |p| p.chain_head);
    let tail = cell(b, |p| p.chain_tail);
    ins.i32_const(tail as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(2);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.local_get(0);
    ins.i32_store(mem(F_NEXT, 4));
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(1);
    ins.else_();
    ins.local_get(0);
    ins.local_set(1);
    ins.end();
    ins.i32_const(0);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(head as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.i32_const(tail as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(1);
}

fn interrupted(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let value = cell(b, |p| p.interrupted);
    ins.i32_const(value as i32);
}

// ---- strings for the diagnostics ----

fn handler_name(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const D: u32 = 1;
    const PTR: u32 = 2;
    const LEN: u32 = 3;
    const I: u32 = 4;
    const S: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(DESC_NAME, 4));
    ins.local_set(PTR);
    ins.local_get(D);
    ins.i32_load(mem(DESC_NAMELEN, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.i32_const(1);
    ins.i32_add();
    let str_new = b.rt("str_new");
    ins.call(str_new);
    ins.local_set(S);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(S);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(PTR);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(S);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(LEN);
    ins.i32_add();
    ins.i32_const('`' as i32);
    ins.i32_store8(mem(0, 1));
    ins.local_get(S);
}

fn where_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const NAME: u32 = 2;
    const WHERE: u32 = 3;
    let name = b.rt("handler_name");
    ins.local_get(1);
    ins.call(name);
    ins.local_set(NAME);
    ins.local_get(0);
    ins.local_get(NAME);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(WHERE);
    let drop = b.rt("drop");
    ins.local_get(NAME);
    ins.call(drop);
    ins.local_get(WHERE);
}

// ---- `suspend fn` operation calls ----

fn perform_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const WHERE: u32 = 4;
    const P: u32 = 5;
    const RS: u32 = 6;
    let perform_desc = cell(b, |p| p.perform_desc);
    let resume_desc = cell(b, |p| p.resume_desc);
    let sync_depth = cell(b, |p| p.sync_depth);
    ins.local_get(0);
    ins.local_get(1);
    let where_new = b.rt("where_new");
    ins.call(where_new);
    ins.local_set(WHERE);
    ins.i32_const(P_SLOTS);
    ins.i32_const(perform_desc as i32);
    let alloc = b.rt("alloc");
    ins.call(alloc);
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_USED, 4));
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(P_PHASE, 4));
    ins.local_get(P);
    ins.local_get(WHERE);
    ins.i32_store(mem(P_WHERE, 4));
    ins.local_get(P);
    ins.i32_const(-1);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(P);
    ins.i64_const(0);
    ins.i64_store(mem(P_NOW_WORD, 8));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_HAS_CALLER, 4));
    ins.local_get(P);
    ins.i32_const(sync_depth as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(P_SYNC, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_IMPL, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_NEXT, 4));
    // Remember the caller's `cur_main` and mark the implementation as not the main continuation.
    ins.i32_const(super::fiber::plan(b).cur_main as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(0);
    ins.local_get(P);
    ins.local_get(0);
    ins.i32_store(mem(P_PREVMAIN, 4));
    ins.i32_const(super::fiber::plan(b).cur_main as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(P);
    ins.local_get(3);
    ins.i32_store(mem(P_REPR, 4));
    ins.i32_const(2);
    ins.i32_const(resume_desc as i32);
    let alloc = b.rt("alloc");
    ins.call(alloc);
    ins.local_set(RS);
    ins.local_get(RS);
    ins.local_get(2);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(RS);
    ins.local_get(P);
    ins.i32_store(mem(HDR + 8, 4));
    let dup = b.rt("dup");
    ins.local_get(RS);
    ins.call(dup);
    ins.local_get(RS);
}

/// `resume_with(resume, mode, word, owns_ref)`: deliver a resume value to the operation call.
fn resume_with(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 4;
    const W: u32 = 5;
    let run_over = super::fiber::plan(b).run_over;
    let sleep_op = cell(b, |p| p.sleep_op);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.i32_const(run_over as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    ins.return_();
    ins.end();
    // A second resume is a defect (spec 14.5).
    ins.local_get(P);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    let twice = cell(b, |p| p.twice_prefix);
    ins.i32_const(twice as i32);
    ins.local_get(P);
    ins.i32_load(mem(P_WHERE, 4));
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(W); // reused as the message temporary
    let raise = b.rt("raise");
    ins.local_get(W);
    ins.call(raise);
    let drop = b.rt("drop");
    ins.local_get(W);
    ins.call(drop);
    ins.return_();
    ins.end();
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(P_USED, 4));
    // Still calling the implementation: the value goes to the caller after it returns.
    ins.local_get(P);
    ins.i32_load(mem(P_PHASE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.local_get(1);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(P);
    ins.local_get(2);
    ins.i64_store(mem(P_NOW_WORD, 8));
    ins.return_();
    ins.end();
    // The implementation returned already: wake the caller's wait.
    ins.local_get(P);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(W);
    ins.local_get(1);
    ins.local_get(2);
    let wake = b.rt("fb_wait_wake");
    ins.call(wake);
    ins.else_();
    // Nobody waits (the run is over or the caller was unwound).
    ins.local_get(3);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    ins.end();
    // The innermost active operation is done once its caller has been queued.
    ins.local_get(P);
    ins.i32_const(sleep_op as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(sleep_op as i32);
    ins.local_get(P);
    ins.i32_load(mem(P_NEXT, 4));
    ins.i32_store(mem(0, 4));
    ins.end();
}

fn resume_void(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.i64_const(0);
    ins.i32_const(0);
    let f = b.rt("resume_with");
    ins.call(f);
}

fn resume_i32(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    ins.i32_const(0);
    let f = b.rt("resume_with");
    ins.call(f);
}

fn resume_i64(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i32_const(0);
    let f = b.rt("resume_with");
    ins.call(f);
}

fn resume_f64(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i64_reinterpret_f64();
    ins.i32_const(0);
    let f = b.rt("resume_with");
    ins.call(f);
}

fn resume_ref(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    ins.i32_const(1);
    let f = b.rt("resume_with");
    ins.call(f);
}

fn perform_end(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    const WORD: u32 = 2; // i64
    const CHAIN: u32 = 3;
    const W: u32 = 4;
    const TMP: u32 = 5;
    const C: u32 = 6;
    let susp_wait = susp_wait_cell(b);
    let cur_main = cur_main_cell(b);
    let out_mode = cell(b, |p| p.out_mode);
    let out_word = cell(b, |p| p.out_word);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    let dup = b.rt("dup");
    ins.local_get(P);
    ins.call(dup);
    // The implementation is done; the caller's `cur_main` comes back.
    ins.i32_const(cur_main as i32);
    ins.local_get(P);
    ins.i32_load(mem(P_PREVMAIN, 4));
    ins.i32_store(mem(0, 4));
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The implementation itself suspended: its frames become its wait's continuation.
    ins.i32_const(0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    ins.i32_const(susp_wait as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.i32_const(susp_wait as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    // The wait owns the operation call (for the end of the implementation) and the call owns the
    // wait.
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(W);
    ins.local_get(P);
    ins.i32_store(mem(super::fiber::W_ONDROP, 4));
    ins.local_get(W);
    ins.call(dup);
    ins.local_get(P);
    ins.local_get(W);
    ins.i32_store(mem(P_IMPL, 4));
    ins.local_get(W);
    ins.local_get(CHAIN);
    ins.i32_const(super::fiber::WE_IMPL);
    let suspended = b.rt("fb_suspended");
    ins.call(suspended);
    ins.else_();
    ins.local_get(CHAIN);
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
    ins.else_();
    // The implementation returned: a pending defect or failure is the caller's input.
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(2);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(P);
    ins.global_get(G_DEFECT);
    ins.i32_store(mem(P_NOW_WORD, 4));
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(P);
    ins.global_get(G_FAILURE);
    ins.i32_store(mem(P_NOW_WORD, 4));
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_PHASE, 4));
    let drop = b.rt("drop");
    ins.local_get(0);
    ins.call(drop);
    // The caller continues: with the value the implementation resumed it with, or by waiting for
    // it.
    ins.local_get(P);
    ins.i32_load(mem(P_NOW_MODE, 4));
    ins.i32_const(0);
    ins.i32_ge_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(out_mode as i32);
    ins.local_get(P);
    ins.i32_load(mem(P_NOW_MODE, 4));
    ins.i32_store(mem(0, 4));
    ins.i32_const(out_word as i32);
    ins.local_get(P);
    ins.i64_load(mem(P_NOW_WORD, 8));
    ins.i64_store(mem(0, 8));
    ins.local_get(P);
    ins.i32_load(mem(P_NOW_MODE, 4));
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(out_word as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_FAILURE);
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(P_NOW_MODE, 4));
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(out_word as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    ins.end();
    ins.local_get(P);
    ins.i32_const(-1);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.else_();
    // Nothing resumed: the caller parks on a wait it owns.
    ins.i32_const(super::fiber::WK_CALLER);
    ins.local_get(P);
    ins.i32_load(mem(P_REPR, 4));
    let new = b.rt("fb_wait_new");
    ins.call(new);
    ins.local_set(C);
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(C);
    ins.local_get(P);
    ins.i32_store(mem(super::fiber::W_ONDROP, 4));
    ins.local_get(C);
    ins.call(dup);
    ins.local_get(P);
    ins.local_get(C);
    ins.i32_store(mem(P_CALLER, 4));
    ins.i32_const(susp_wait as i32);
    ins.local_get(C);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(out_mode as i32);
    ins.i32_const(3);
    ins.i32_store(mem(0, 4));
    ins.end();
    let drop = b.rt("drop");
    ins.local_get(P);
    ins.call(drop);
    let _ = TMP;
}

// ---- calls of suspending code from non-suspending functions (spec 8.7) ----

fn sync_enter(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let depth = cell(b, |p| p.sync_depth);
    ins.i32_const(depth as i32);
    ins.i32_const(depth as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
}

fn sync_leave(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const CHAIN: u32 = 1;
    const W: u32 = 2;
    let depth = cell(b, |p| p.sync_depth);
    let susp_wait = susp_wait_cell(b);
    ins.i32_const(depth as i32);
    ins.i32_const(depth as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.global_get(G_SUSPENDING);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    // A request that was never attached (it was made in this region): drop it.
    ins.i32_const(susp_wait as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(W);
    ins.if_(BlockType::Empty);
    ins.i32_const(susp_wait as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(W);
    let unlink = b.rt("fb_unlink_wait");
    ins.call(unlink);
    let drop = b.rt("drop");
    ins.local_get(W);
    ins.call(drop);
    ins.end();
    ins.i32_const(0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    // The continuation is never resumed: unwind it as interrupted at once (spec 14.5). The
    // operations made in this region are unwound first, from the inside out.
    let unwind = b.rt("unwind_sync");
    ins.local_get(CHAIN);
    ins.call(unwind);
    let drain = b.rt("fb_drain");
    ins.call(drain);
    // A failure while unwinding is cut short by the interruption; a defect stays the result.
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    let drop = b.rt("drop");
    ins.call(drop);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.end();
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let interrupted = b.rt("interrupted");
    ins.call(interrupted);
    ins.global_set(G_DEFECT);
    ins.end();
}

// ---- unwinding a sync region ----

/// `unwind_sync(extra)`: queue the continuations of the current fiber that wait inside a
/// non-suspending function, and the caller chain `extra`, to be resumed with an interruption
/// (spec 8.7, 14.5). Their releases run; the input they end with is the call's defect.
fn unwind_sync(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 1;
    const PREV: u32 = 2;
    const CHAIN: u32 = 3;
    const F: u32 = 4;
    const NEXT: u32 = 5;
    let whead = super::fiber::plan(b).whead;
    let wtail = super::fiber::plan(b).wtail;
    let cur = super::fiber::plan(b).cur_fiber;
    let push = b.rt("fb_queue_push");
    let unlink = b.rt("fb_unlink_wait");
    let dup = b.rt("dup");
    let drop = b.rt("drop");
    ins.i32_const(cur as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(F);
    ins.i32_const(wtail as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(W);
    ins.i32_load(mem(super::fiber::W_PREV, 4));
    ins.local_set(PREV);
    ins.local_get(W);
    ins.i32_load(mem(super::fiber::W_FIBER, 4));
    ins.local_get(F);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_load(mem(super::fiber::W_LIVE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_load(mem(super::fiber::W_CHAIN, 4));
    ins.local_set(CHAIN);
    ins.local_get(W);
    ins.i32_const(0);
    ins.i32_store(mem(super::fiber::W_CHAIN, 4));
    ins.local_get(W);
    ins.call(unlink);
    ins.local_get(W);
    ins.call(drop);
    ins.local_get(CHAIN);
    ins.if_(BlockType::Empty);
    ins.i32_const(super::fiber::JK_INTERRUPT);
    ins.local_get(F);
    ins.call(dup);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.i32_const(super::fiber::WE_SYNC);
    ins.i32_const(0);
    ins.i64_const(0);
    ins.i32_const(0);
    ins.call(push);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(PREV);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    // The caller chain of the sync function is the outermost continuation.
    ins.local_get(0);
    ins.if_(BlockType::Empty);
    ins.i32_const(super::fiber::JK_INTERRUPT);
    ins.local_get(F);
    ins.call(dup);
    ins.local_get(F);
    ins.local_get(0);
    ins.i32_const(super::fiber::WE_SYNC);
    ins.i32_const(0);
    ins.i64_const(0);
    ins.i32_const(0);
    ins.call(push);
    ins.end();
    let _ = NEXT;
}
