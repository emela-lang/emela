//! Frames, `suspend fn` operations and the continuation scheduler.
//!
//! This is the WASM counterpart of the C runtime's suspension machinery (spec 8.4, 8.7, 14.5),
//! without fibers or a host callback yet:
//!
//! - A frame is a heap object: the resume trampoline's table index, the resume point, the next
//!   frame and the saved variables. The frames of a captured continuation are a singly linked
//!   list, innermost first.
//! - A `perform` record is the state of one `suspend fn` operation call: whether the
//!   implementation resumed it, the pending failure or defect, and the caller's continuation.
//!   The `resume` value handed to the implementation is a closure whose environment is the
//!   record.
//! - A resume of an implementation that is still running continues the caller after the
//!   implementation finishes (in place, spec 14.5 item 3). A resume after it returned queues the
//!   caller's continuation; the queue is FIFO.
//! - A suspension cascade pushes one `perform` record per operation whose implementation suspends,
//!   innermost first, and `P_NEXT` links them outward. `pending` is the cascade's outermost (the
//!   one whose caller is the parked continuation) and `sleep_op` is the innermost (the one the
//!   host event resumes); parking a cascade resets `pending`, so an implementation that suspends
//!   again after a resume starts a new cascade instead of overwriting an outer caller.
//! - When nothing can resume a continuation (there is no fiber and no host yet), the run stalls:
//!   the continuation is resumed with `interrupted`, which runs the `with` releases, and the
//!   entry ends with exit code 130 (spec 14.5). A suspension in a non-suspending function is
//!   unwound the same way at once (`sync_enter` / `sync_leave`, spec 8.7).
//!
//! All of the scheduler's state is static memory allocated by `emit::suspend::prepare`; the
//! module runs on one thread and resumes one continuation at a time.

use super::*;

use super::super::suspend::{QUEUE_ENTRIES, QUEUE_STRIDE};

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
        &[ValType::I32, ValType::I32, ValType::I32],
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
    ("root_run", &[], &[]),
    ("sched_drain", &[], &[]),
    ("advance", &[ValType::I32, ValType::I32, ValType::I64], &[]),
    (
        "resume_with",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
    ("where_new", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("handler_name", &[ValType::I32], &[ValType::I32]),
    (
        "queue_push",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    ("unwind_ops", &[ValType::I32, ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "frame_new" => i32s(1),
        "frame_push" => i32s(1),
        "frame_take" => i32s(2),
        "perform_new" => i32s(3),
        // 1: the perform record, 2: the input word (i64), 3: a chain, 4-6: temporaries.
        "perform_end" => vec![(1, ValType::I32), (1, ValType::I64), (4, ValType::I32)],
        "resume_with" => i32s(3),
        "sync_leave" => i32s(4),
        "root_run" => i32s(1),
        "queue_push" => i32s(3),
        "unwind_ops" => i32s(4),
        "sched_drain" => {
            let mut l = i32s(4);
            l.push((1, ValType::I64));
            l.extend(i32s(2));
            l
        }
        "advance" => {
            let mut l = i32s(3);
            l.push((1, ValType::I64));
            l
        }
        "handler_name" => i32s(5),
        "where_new" => i32s(2),
        _ => Vec::new(),
    }
}

/// The slots of a `perform` record (see `emit::suspend`'s descriptor).
pub(super) const P_USED: u32 = HDR;
pub(super) const P_PHASE: u32 = HDR + 8;
pub(super) const P_WHERE: u32 = HDR + 16;
pub(super) const P_NOW_MODE: u32 = HDR + 24;
pub(super) const P_NOW_WORD: u32 = HDR + 32;
pub(super) const P_CALLER: u32 = HDR + 40;
pub(super) const P_HAS_CALLER: u32 = HDR + 48;
/// The `sync_enter` depth the call was made at: 0 outside every non-suspending function (spec
/// 8.7). `sync_leave` unwinds only the operations made inside its own region (`unwind_ops`), not a
/// cascade parked before the region began.
pub(super) const P_SYNC: u32 = HDR + 56;
pub(super) const P_IMPL: u32 = HDR + 64;
/// The next outer active operation. The chain starts at the innermost operation (`pending`) and
/// links outward (spec 14.5).
pub(super) const P_NEXT: u32 = HDR + 72;
const P_SLOTS: i32 = 10;

/// A frame's slots.
const F_RESUME: u32 = HDR;
pub(super) const F_STATE: u32 = HDR + 8;
const F_NEXT: u32 = HDR + 16;

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
        "root_run" => root_run(b, ins),
        "sched_drain" => sched_drain(b, ins),
        "advance" => advance(b, ins),
        "resume_with" => resume_with(b, ins),
        "where_new" => where_new(b, ins),
        "handler_name" => handler_name(b, ins),
        "queue_push" => queue_push(b, ins),
        "unwind_ops" => unwind_ops(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn cell(b: &Builder<'_>, f: impl Fn(&super::super::suspend::Plan) -> u32) -> u32 {
    f(b.sus.as_ref().expect("the suspend runtime"))
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

/// `handler_name(handler)`: a new string of the handler's descriptor name and a closing
/// backtick.
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

/// `where_new(prefix, handler)`: a new string of `prefix` and the handler's name. `str_append`
/// borrows its arguments, so the name it was built from is released here.
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

// ---- `suspend fn` operations ----

fn perform_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const WHERE: u32 = 3;
    const P: u32 = 4;
    const RS: u32 = 5;
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
    // used = 0, phase = 1, where, now mode = -1, now word = 0, caller = 0, has caller = 0,
    // sync, impl = 0.
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

/// `resume_with(resume, mode, word, owns_ref)`: delivers a resume value to the operation call.
fn resume_with(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 4;
    const CHAIN: u32 = 5;
    const MSG: u32 = 6;
    let sleep_op = cell(b, |p| p.sleep_op);
    // p = resume.slot[1]
    ins.local_get(0);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
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
    ins.local_set(MSG);
    let raise = b.rt("raise");
    ins.local_get(MSG);
    ins.call(raise);
    // `raise` borrows the message.
    let drop = b.rt("drop");
    ins.local_get(MSG);
    ins.call(drop);
    ins.return_();
    ins.end();
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(P_USED, 4));
    // Still calling the implementation: the caller goes on after it returns (item 3).
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
    // The implementation already returned: wake the caller's continuation through the FIFO.
    ins.local_get(P);
    ins.i32_load(mem(P_HAS_CALLER, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_tee(CHAIN);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_HAS_CALLER, 4));
    ins.else_();
    ins.i32_const(0);
    ins.local_set(CHAIN);
    ins.end();
    ins.local_get(CHAIN);
    ins.local_get(1);
    ins.local_get(2);
    let push = b.rt("queue_push");
    ins.call(push);
    // The innermost active operation is done once its caller has been queued; the next outer
    // operation (if any) becomes the innermost (spec 14.5 item 3).
    ins.local_get(P);
    ins.i32_const(sleep_op as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(sleep_op as i32);
    ins.local_get(P);
    ins.i32_load(mem(P_NEXT, 4));
    ins.i32_store(mem(0, 4));
    let drop = b.rt("drop");
    ins.local_get(P);
    ins.call(drop);
    ins.end();
    ins.else_();
    ins.local_get(3);
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    let drop = b.rt("drop");
    ins.call(drop);
    ins.end();
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

/// `settled(p, mode, word)`: the implementation ended with a failure or defect. Before a resume
/// it goes to the caller; after one it is the run's defect (spec 14.5).
fn settled(b: &mut Builder<'_>, ins: &mut InstructionSink, p: u32, mode: u32, word: u32) {
    const SHOWN: u32 = 4;
    const MSG: u32 = 5;
    // Free in this branch: `pass` (which uses it) is only called in the other one.
    const TMP: u32 = 6;
    ins.local_get(p);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(mode as i32);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // A failure after resume is the run's defect, showing the error.
    ins.local_get(word);
    ins.i32_wrap_i64();
    let show = b.rt("show_failure");
    ins.call(show);
    ins.local_set(SHOWN);
    ins.local_get(p);
    ins.i32_load(mem(P_WHERE, 4));
    let after = cell(b, |p| p.after_prefix);
    ins.i32_const(after as i32);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.local_get(SHOWN);
    ins.call(append);
    ins.local_set(MSG);
    // `str_append` borrows its arguments.
    let drop = b.rt("drop");
    ins.local_get(TMP);
    ins.call(drop);
    ins.local_get(SHOWN);
    ins.call(drop);
    ins.local_get(p);
    ins.i32_const(2);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(p);
    ins.local_get(MSG);
    ins.i64_extend_i32_u();
    ins.i64_store(mem(P_NOW_WORD, 8));
    ins.else_();
    ins.local_get(p);
    ins.i32_const(2);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(p);
    ins.local_get(word);
    ins.i64_store(mem(P_NOW_WORD, 8));
    ins.end();
    ins.else_();
    // Not resumed: the failure or defect is the operation call's result.
    pass(b, ins, p, mode, word);
    ins.end();
}

/// `pass(p, mode, word)`: delivers the operation call's result to the caller.
fn pass(b: &mut Builder<'_>, ins: &mut InstructionSink, p: u32, mode: u32, word: u32) {
    const CHAIN: u32 = 6;
    ins.local_get(p);
    ins.i32_const(1);
    ins.i32_store(mem(P_USED, 4));
    ins.local_get(p);
    ins.i32_load(mem(P_PHASE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(p);
    ins.i32_const(mode as i32);
    ins.i32_store(mem(P_NOW_MODE, 4));
    ins.local_get(p);
    ins.local_get(word);
    ins.i64_store(mem(P_NOW_WORD, 8));
    ins.else_();
    ins.local_get(p);
    ins.i32_load(mem(P_HAS_CALLER, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(p);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_tee(CHAIN);
    ins.if_(BlockType::Empty);
    ins.local_get(p);
    ins.i32_const(0);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(p);
    ins.i32_const(0);
    ins.i32_store(mem(P_HAS_CALLER, 4));
    ins.local_get(CHAIN);
    ins.i32_const(mode as i32);
    ins.local_get(word);
    let push = b.rt("queue_push");
    ins.call(push);
    ins.end();
    ins.end();
    ins.end();
}

fn perform_end(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    const WORD: u32 = 2;
    const CHAIN: u32 = 3;
    const OLD: u32 = 4;
    let out_mode = cell(b, |p| p.out_mode);
    let out_word = cell(b, |p| p.out_word);
    let pending = cell(b, |p| p.pending);
    let sleep_op = cell(b, |p| p.sleep_op);
    ins.local_get(0);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    // The implementation itself suspended: its continuation is part of this operation call.
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    ins.local_get(P);
    ins.local_get(CHAIN);
    ins.i32_store(mem(P_IMPL, 4));
    ins.end();
    // A failure or defect from the implementation.
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    settled(b, ins, P, 2, WORD);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(WORD);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    settled(b, ins, P, 1, WORD);
    ins.end();
    ins.end();
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_PHASE, 4));
    // The implementation resumed: continue with that input. Else the caller suspends.
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
    // A failure or defect is the operation call's pending result; its landings run in the
    // caller.
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
    // The implementation suspended after it resumed: it is left over, so its releases do not
    // run (spec 14.5).
    ins.local_get(P);
    ins.i32_load(mem(P_IMPL, 4));
    ins.local_tee(CHAIN);
    ins.if_(BlockType::Empty);
    let drop = b.rt("drop");
    ins.local_get(CHAIN);
    ins.call(drop);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_IMPL, 4));
    ins.end();
    ins.else_();
    // Nobody resumed: the caller waits. `pending` is the outermost operation of the current
    // suspension cascade (0 when none is under way) and `sleep_op` is the innermost active
    // operation. A record created inside the cascade is the current outermost's outer; the
    // first record of a cascade is the new innermost, whose outer is the operation whose
    // implementation is running again after an earlier resume (spec 14.5 item 3).
    let dup = b.rt("dup");
    let drop = b.rt("drop");
    ins.i32_const(pending as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(OLD);
    ins.if_(BlockType::Empty);
    // The current cascade's outermost: this record is its outer.
    ins.local_get(OLD);
    ins.local_get(CHAIN);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(OLD);
    ins.i32_const(1);
    ins.i32_store(mem(P_HAS_CALLER, 4));
    // This record goes between the outermost and the outermost's outer, which is the operation
    // that started this cascade from its resumed implementation, if any; dropping that link would
    // leave it out of the chain, so it would never be unwound or released.
    ins.local_get(P);
    ins.local_get(OLD);
    ins.i32_load(mem(P_NEXT, 4));
    ins.i32_store(mem(P_NEXT, 4));
    ins.local_get(OLD);
    ins.local_get(P);
    ins.i32_store(mem(P_NEXT, 4));
    // Its implementation continuation is now the outermost's caller link, which keeps this
    // record alive (through the resume closure that link holds); the outer link is not counted.
    // The previous outermost's reference is not released here: it is still in the chain and is
    // released when it completes (`resume_with`) or when the run unwinds (`unwind_ops`).
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_IMPL, 4));
    ins.else_();
    // The first record of a new cascade: the new innermost operation.
    ins.local_get(P);
    ins.i32_const(sleep_op as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(P_NEXT, 4));
    ins.i32_const(sleep_op as i32);
    ins.local_get(P);
    ins.i32_store(mem(0, 4));
    ins.end();
    // This record is the cascade's new outermost; keep it alive and remember it.
    ins.local_get(P);
    ins.call(dup);
    ins.i32_const(pending as i32);
    ins.local_get(P);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(out_mode as i32);
    ins.i32_const(3);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.local_get(0);
    ins.call(drop);
}

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
    let depth = cell(b, |p| p.sync_depth);
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
    ins.i32_const(0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    // The continuation is never resumed: unwind it as interrupted at once (spec 14.5). A
    // suspended operation implementation is inside the caller's continuation, so it is unwound
    // first and its releases run first (spec 14.2: from the inside). Only the operations made in
    // this region are unwound: a cascade parked before it (an implementation that resumed from a
    // sleep and then called this function) is outside, and the interruption reaches it as this
    // call's defect. Nothing in the region waits on the host: a handler that can resume later
    // requires Async, which a non-suspending function cannot have (spec 8.7).
    let unwind = b.rt("unwind_ops");
    ins.local_get(CHAIN);
    ins.i32_const(depth as i32);
    ins.i32_load(mem(0, 4));
    ins.call(unwind);
    let drain = b.rt("sched_drain");
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

// ---- the continuation queue ----

fn queue_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const LEN: u32 = 3;
    const IDX: u32 = 4;
    const BASE: u32 = 5;
    let queue = cell(b, |p| p.queue);
    let qhead = cell(b, |p| p.qhead);
    let qlen = cell(b, |p| p.qlen);
    let full = cell(b, |p| p.queue_full);
    ins.i32_const(qlen as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(LEN);
    ins.i32_const(QUEUE_ENTRIES as i32);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    // The job cannot be kept: release the continuation it was given, then end the run with the
    // internal error as its defect.
    let drop = b.rt("drop");
    ins.local_get(0);
    ins.call(drop);
    ins.i32_const(full as i32);
    let raise = b.rt("raise");
    ins.call(raise);
    ins.return_();
    ins.end();
    ins.i32_const(qhead as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(LEN);
    ins.i32_add();
    ins.i32_const((QUEUE_ENTRIES - 1) as i32);
    ins.i32_and();
    ins.local_set(IDX);
    ins.local_get(IDX);
    ins.i32_const(QUEUE_STRIDE as i32);
    ins.i32_mul();
    ins.i32_const(queue as i32);
    ins.i32_add();
    ins.local_set(BASE);
    ins.local_get(BASE);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(BASE);
    ins.local_get(1);
    ins.i32_store(mem(4, 4));
    ins.local_get(BASE);
    ins.local_get(2);
    ins.i64_store(mem(8, 8));
    ins.i32_const(qlen as i32);
    ins.local_get(LEN);
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
}

fn sched_drain(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const IDX: u32 = 0;
    const BASE: u32 = 1;
    const CHAIN: u32 = 2;
    const MODE: u32 = 3;
    const WORD: u32 = 4;
    const EARLIER: u32 = 5;
    const LATER: u32 = 6;
    let queue = cell(b, |p| p.queue);
    let qhead = cell(b, |p| p.qhead);
    let qlen = cell(b, |p| p.qlen);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.i32_const(qlen as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.br_if(1);
    ins.i32_const(qhead as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(IDX);
    ins.local_get(IDX);
    ins.i32_const(QUEUE_STRIDE as i32);
    ins.i32_mul();
    ins.i32_const(queue as i32);
    ins.i32_add();
    ins.local_set(BASE);
    ins.local_get(BASE);
    ins.i32_load(mem(0, 4));
    ins.local_set(CHAIN);
    ins.local_get(BASE);
    ins.i32_load(mem(4, 4));
    ins.local_set(MODE);
    ins.local_get(BASE);
    ins.i64_load(mem(8, 8));
    ins.local_set(WORD);
    ins.i32_const(qhead as i32);
    ins.local_get(IDX);
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_const((QUEUE_ENTRIES - 1) as i32);
    ins.i32_and();
    ins.i32_store(mem(0, 4));
    ins.i32_const(qlen as i32);
    ins.i32_const(qlen as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    // The job runs with no defect pending: its frames take theirs from the input. A defect an
    // earlier job ended with is set aside and combined with this job's afterwards.
    ins.global_get(G_DEFECT);
    ins.local_set(EARLIER);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.local_get(CHAIN);
    ins.local_get(MODE);
    ins.local_get(WORD);
    let advance = b.rt("advance");
    ins.call(advance);
    ins.global_get(G_DEFECT);
    ins.local_set(LATER);
    ins.local_get(EARLIER);
    ins.global_set(G_DEFECT);
    combine_defect(b, ins, LATER);
    ins.br(0);
    ins.end();
    ins.end();
}

/// Combines the defect in local `later` (owned, may be 0) with the pending one (spec 14.4): an
/// interruption gives way to a defect, and of two defects the first stays the result and the
/// later one becomes its note.
fn combine_defect(b: &mut Builder<'_>, ins: &mut InstructionSink, later: u32) {
    let interrupted = b.rt("interrupted");
    ins.local_get(later);
    ins.if_(BlockType::Empty);
    ins.local_get(later);
    ins.call(interrupted);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(later);
    ins.global_set(G_DEFECT);
    ins.end();
    ins.else_();
    // `raise` borrows its argument.
    let raise = b.rt("raise");
    ins.local_get(later);
    ins.call(raise);
    let drop = b.rt("drop");
    ins.local_get(later);
    ins.call(drop);
    ins.end();
    ins.end();
}

/// Continues the frames with the input until the continuation ends or suspends again.
fn advance(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const FR: u32 = 3;
    const CHAIN: u32 = 4;
    const MODE: u32 = 5;
    const WORD: u32 = 6;
    let in_mode = cell(b, |p| p.in_mode);
    let in_word = cell(b, |p| p.in_word);
    let out_mode = cell(b, |p| p.out_mode);
    let out_word = cell(b, |p| p.out_word);
    ins.local_get(0);
    ins.local_set(CHAIN);
    ins.local_get(1);
    ins.local_set(MODE);
    ins.local_get(2);
    ins.local_set(WORD);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CHAIN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The continuation ended: its result is the run's.
    ins.local_get(MODE);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.else_();
    ins.local_get(MODE);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WORD);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.end();
    ins.end();
    ins.return_();
    ins.end();
    ins.local_get(CHAIN);
    ins.local_set(FR);
    ins.local_get(FR);
    ins.i32_load(mem(F_NEXT, 4));
    ins.local_set(CHAIN);
    ins.local_get(FR);
    ins.i32_const(0);
    ins.i32_store(mem(F_NEXT, 4));
    ins.i32_const(in_mode as i32);
    ins.local_get(MODE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(in_word as i32);
    ins.local_get(WORD);
    ins.i64_store(mem(0, 8));
    ins.local_get(FR);
    ins.local_get(FR);
    ins.i32_load(mem(F_RESUME, 4));
    let ty = b.call_ty(&[Repr::Ref], Repr::Void);
    ins.call_indirect(0, ty);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // Suspended again: park the remaining continuation for the bridge, or stall at once when
    // there is no asynchronous host.
    ins.local_get(CHAIN);
    let take = b.rt("frame_take");
    ins.call(take);
    if b.async_plan.is_some() {
        let park = b.rt("wait_park");
        ins.call(park);
    } else {
        let unwind = b.rt("unwind_ops");
        ins.i32_const(-1);
        ins.call(unwind);
    }
    ins.return_();
    ins.end();
    ins.i32_const(out_mode as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(MODE);
    ins.i32_const(out_word as i32);
    ins.i64_load(mem(0, 8));
    ins.local_set(WORD);
    ins.br(0);
    ins.end();
    ins.end();
}

/// `unwind_ops(extra, depth)`: queues every waiting continuation for interruption, from the
/// innermost implementation outwards (spec 14.2, 14.5). `extra` is the caller chain a
/// non-suspending function was unwinding; it is unwound after the operations (the outermost
/// caller). Only the operations made deeper than `depth` (`P_SYNC`) are unwound: `sync_leave`
/// passes its own depth, and the stalled run passes -1 to unwind them all.
fn unwind_ops(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const EXTRA: u32 = 0;
    const DEPTH: u32 = 1;
    const P: u32 = 2;
    const NEXT: u32 = 3;
    const CHAIN: u32 = 4;
    const TOP: u32 = 5;
    let pending = cell(b, |p| p.pending);
    let sleep_op = cell(b, |p| p.sleep_op);
    let push = b.rt("queue_push");
    let interrupted = b.rt("interrupted");
    let drop = b.rt("drop");
    let push_interrupted = |ins: &mut InstructionSink, chain: u32| {
        ins.local_get(chain);
        ins.if_(BlockType::Empty);
        ins.local_get(chain);
        ins.i32_const(2);
        ins.call(interrupted);
        ins.i64_extend_i32_u();
        ins.call(push);
        ins.end();
    };
    // Unwinding the whole run (`depth` -1): `Fiber.attempt` no longer catches a defect.
    if let Some(unwinding) = super::attempt::unwinding_cell(b) {
        ins.local_get(DEPTH);
        ins.i32_const(-1);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(unwinding as i32);
        ins.i32_const(1);
        ins.i32_store(mem(0, 4));
        ins.end();
    }
    // `sleep_op` is the innermost active operation; `P_NEXT` links outward. The innermost
    // implementation's continuation is unwound first, then every caller link outward, down to the
    // first operation made at `depth` or shallower.
    ins.i32_const(sleep_op as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_load(mem(P_SYNC, 4));
    ins.local_get(DEPTH);
    ins.i32_le_s();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_load(mem(P_IMPL, 4));
    ins.local_set(CHAIN);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_IMPL, 4));
    push_interrupted(ins, CHAIN);
    ins.local_get(P);
    ins.i32_load(mem(P_NEXT, 4));
    ins.local_set(NEXT);
    ins.local_get(P);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_set(CHAIN);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_CALLER, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(P_HAS_CALLER, 4));
    push_interrupted(ins, CHAIN);
    ins.local_get(NEXT);
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    // The caller chain of a non-suspending function is the outermost continuation.
    push_interrupted(ins, EXTRA);
    // The operations left are outside the unwound region and stay active.
    ins.i32_const(sleep_op as i32);
    ins.local_get(P);
    ins.i32_store(mem(0, 4));
    // One reference is held by the pending slot when its operation was unwound; the resume
    // closure keeps the record until the interrupted implementation has run.
    ins.i32_const(pending as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(TOP);
    ins.if_(BlockType::Empty);
    ins.local_get(TOP);
    ins.i32_load(mem(P_SYNC, 4));
    ins.local_get(DEPTH);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(pending as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(TOP);
    ins.call(drop);
    ins.end();
    ins.end();
}

fn root_run(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const CHAIN: u32 = 0;
    ins.global_get(G_SUSPENDING);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    let take = b.rt("frame_take");
    ins.call(take);
    ins.local_set(CHAIN);
    let drain = b.rt("sched_drain");
    if b.async_plan.is_some() {
        // Park the caller continuation and let the bridge wait for an event or the TestClock
        // idle hook; without either it unwinds the run as interrupted.
        let park = b.rt("wait_park");
        ins.local_get(CHAIN);
        ins.call(park);
        let run = b.rt("wait_run");
        ins.call(run);
    } else {
        ins.local_get(CHAIN);
        let unwind = b.rt("unwind_ops");
        ins.i32_const(-1);
        ins.call(unwind);
        ins.call(drain);
    }
}
