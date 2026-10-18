//! The scheduler and the suspend machinery (spec chapters 11 and 14).
//!
//! The state machines (see `emit::suspend`) push a frame of their live variables onto the
//! captured continuation (`G_SUSP_HEAD`/`G_SUSP_TAIL`) and return when a value comes back
//! suspended (`G_SUSPENDING`). This fragment holds the runtime side: the frames, the resume input
//! (`G_RESUME`), the `sync` calls of spec 8.7 (`sync_enter`/`sync_leave`, which unwind a
//! continuation that nothing will resume as interrupted), the `perform` a `suspend fn` operation
//! waits on (`resume_new`/`perform_end`) and the interruption object (`interrupted_new`).
//!
//! A program that reaches no suspending code keeps the exact bytes it had before (`runtime::register`
//! skips this fragment unless `Builder::uses_suspend`).

use super::*;

/// The scheduler fragment (see `runtime::FUNCS`).
pub(super) const FUNCS: &[Sig] = &[
    ("hold", &[], &[]),
    ("unhold", &[], &[]),
    ("yield", &[ValType::I32], &[]),
    ("frame_new", &[ValType::I32; 4], &[ValType::I32]),
    ("frame_push", &[ValType::I32], &[]),
    ("frame_free", &[ValType::I32], &[]),
    ("sync_enter", &[], &[]),
    ("sync_leave", &[], &[]),
    ("resume_new", &[ValType::I32; 3], &[ValType::I32]),
    ("perform_end", &[ValType::I32], &[ValType::I64]),
    ("is_interrupted", &[ValType::I32], &[ValType::I32]),
    ("interrupted_new", &[ValType::I32], &[ValType::I32]),
    ("str_from_raw", &[ValType::I32; 2], &[ValType::I32]),
    ("resume_i32", &[ValType::I32, ValType::I32], &[]),
    ("resume_i64", &[ValType::I32, ValType::I64], &[]),
    ("resume_f64", &[ValType::I32, ValType::F64], &[]),
    ("resume_ref", &[ValType::I32, ValType::I32], &[]),
    ("resume_void", &[ValType::I32], &[]),
];

/// The slots of a `perform` (a `suspend fn` operation call, spec 8.4): counted by hand.
const PERF_USED: u32 = HDR;
const PERF_HAS_NOW: u32 = HDR + 8;
const PERF_SYNC: u32 = HDR + 16;
const PERF_SYNC_PHASE: u32 = HDR + 24;
const PERF_NOW: u32 = HDR + 32;
const PERF_OP: u32 = HDR + 40;
const PERF_HANDLER: u32 = HDR + 48;
const PERF_SLOTS: u32 = 7;
/// The slots of a resume value: the table index of its closure and its `perform`.
const RES_TABLE: u32 = HDR;
const RES_PERF: u32 = HDR + 8;

/// Whether `name` is a function of this fragment (registered only when the module suspends).
pub(super) fn handles(name: &str) -> bool {
    FUNCS.iter().any(|(n, ..)| *n == name)
}

fn perform_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "perform".to_owned(),
        show: None,
        refs: vec![false, false, false, false, false, false, true],
    })
}

fn resume_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "resume".to_owned(),
        show: None,
        refs: vec![false, false],
    })
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "hold" | "unhold" | "yield" => {}
        "frame_new" => frame_new(b, ins),
        "frame_push" => frame_push(ins),
        "frame_free" => frame_free(ins),
        "sync_enter" => sync_enter(ins),
        "sync_leave" => sync_leave(b, ins),
        "resume_new" => resume_new(b, ins),
        "perform_end" => perform_end(b, ins),
        "is_interrupted" => is_interrupted(ins),
        "interrupted_new" => interrupted_new(b, ins),
        "str_from_raw" => str_from_raw(b, ins),
        "resume_void" => resume_call(b, ins, Repr::Void, 0),
        "resume_i32" => resume_call(b, ins, Repr::I32, 1),
        "resume_i64" => resume_call(b, ins, Repr::I64, 1),
        "resume_f64" => resume_call(b, ins, Repr::F64, 1),
        "resume_ref" => resume_call(b, ins, Repr::Ref, 1),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    let l = match name {
        "frame_new" => vec![(1, ValType::I32)],
        "frame_free" => vec![(2, ValType::I32)],
        "sync_leave" => vec![(6, ValType::I32), (2, ValType::I64)],
        "resume_new" => vec![(2, ValType::I32)],
        "perform_end" => vec![(8, ValType::I32), (2, ValType::I64)],
        "interrupted_new" => vec![(1, ValType::I32)],
        "str_from_raw" => vec![(2, ValType::I32)],
        "resume_i32" | "resume_i64" | "resume_f64" | "resume_ref" => vec![(3, ValType::I32)],
        "resume_void" => vec![(3, ValType::I32)],
        _ => return None,
    };
    Some(l)
}

fn rt(b: &Builder, name: &'static str) -> u32 {
    b.rt[name]
}

// ---- frames ----

/// `frame_new(desc, resume_table, state, nsaved) -> fr`: an object of `3 + nsaved` slots
/// `{resume table index, state, next, saved…}`.
fn frame_new(b: &mut Builder, ins: &mut InstructionSink) {
    const FR: u32 = 4;
    ins.local_get(3);
    ins.i32_const(3);
    ins.i32_add();
    ins.local_get(0);
    ins.call(rt(b, "alloc"));
    ins.local_set(FR);
    ins.local_get(FR);
    ins.local_get(1);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(FR);
    ins.local_get(2);
    ins.i32_store(mem(HDR + 8, 4));
    ins.local_get(FR);
    ins.i32_const(0);
    ins.i32_store(mem(FRAME_NEXT, 4));
    ins.local_get(FR);
}

fn frame_push(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_store(mem(FRAME_NEXT, 4));
    ins.global_get(G_SUSP_TAIL);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.global_set(G_SUSP_HEAD);
    ins.else_();
    ins.global_get(G_SUSP_TAIL);
    ins.local_get(0);
    ins.i32_store(mem(FRAME_NEXT, 4));
    ins.end();
    ins.local_get(0);
    ins.global_set(G_SUSP_TAIL);
}

/// Frees a frame whose saved variables were moved out (its `desc->nslots` gives the size).
fn frame_free(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_NSLOTS, 4));
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(1);
    free_raw(ins, 0, 1);
}

// ---- the suspension ----

fn sync_enter(ins: &mut InstructionSink) {
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(1);
    ins.i32_add();
    ins.global_set(G_SYNC_DEPTH);
}

/// Takes the suspension that bubbled out of a suspending call in a non-suspending function and
/// unwinds it as interrupted at once (spec 8.7, 14.5): nothing will ever resume it. The releases
/// of its open `with`s run while it unwinds; if one suspends again it is cut short the same way.
fn sync_leave(b: &mut Builder, ins: &mut InstructionSink) {
    const CHAIN: u32 = 0;
    const MODE: u32 = 1;
    const FR: u32 = 2;
    const W: u32 = 3;
    const RT: u32 = 4;
    const BITS: u32 = 6;
    const OUT: u32 = 7;
    let drop = rt(b, "drop");
    let resume_ty = b.ty(vec![ValType::I32], vec![ValType::I64]);
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(1);
    ins.i32_sub();
    ins.global_set(G_SYNC_DEPTH);
    ins.global_get(G_SUSPENDING);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(CHAIN);
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.i32_const(0);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(0);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
    // `sync_forget`: the implementation is not resumed.
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.call(drop);
    ins.end();
    interrupted(b, ins, BITS);
    ins.i32_const(1);
    ins.local_set(MODE);
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(1);
    ins.i32_add();
    ins.global_set(G_SYNC_DEPTH);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CHAIN);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(CHAIN);
    ins.local_set(FR);
    ins.local_get(FR);
    ins.i32_load(mem(FRAME_NEXT, 4));
    ins.local_set(CHAIN);
    ins.local_get(FR);
    ins.i32_const(0);
    ins.i32_store(mem(FRAME_NEXT, 4));
    set_input(ins, MODE, BITS);
    ins.local_get(FR);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(RT);
    ins.local_get(FR);
    ins.local_get(RT);
    ins.call_indirect(0, resume_ty);
    ins.local_set(OUT);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The unwinding release suspended again: cut it short.
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(FR);
    ins.i32_const(0);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(0);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.call(drop);
    ins.end();
    ins.local_get(FR);
    ins.call(drop);
    interrupted(b, ins, BITS);
    ins.i32_const(1);
    ins.local_set(MODE);
    ins.br(1);
    ins.end();
    input_taken(b, ins, OUT, MODE, BITS);
    ins.br(0);
    ins.end();
    ins.end();
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(1);
    ins.i32_sub();
    ins.global_set(G_SYNC_DEPTH);
    // A throw becomes the pending defect; anything else is interrupted.
    ins.local_get(MODE);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.else_();
    interrupted(b, ins, BITS);
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.end();
}

/// Pushes a fresh interruption into `bits` (a local i64).
fn interrupted(b: &mut Builder, ins: &mut InstructionSink, bits: u32) {
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.i64_extend_i32_u();
    ins.local_set(bits);
}

/// Sets the resume input (`mode` 0 value, 1 defect, 2 failure) from `BITS`.
fn set_input(ins: &mut InstructionSink, mode: u32, bits: u32) {
    ins.local_get(mode);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(bits);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.global_set(G_RESUME);
    ins.else_();
    ins.local_get(mode);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(bits);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.i64_const(0);
    ins.global_set(G_RESUME);
    ins.else_();
    ins.local_get(bits);
    ins.global_set(G_RESUME);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.end();
    ins.end();
}

/// The input after a call returned `out` (a word): takes a pending defect or failure into `BITS`
/// and leaves the mode in `MODE` (0 value, 1 defect, 2 failure).
fn input_taken(b: &mut Builder, ins: &mut InstructionSink, out: u32, mode: u32, bits: u32) {
    let drop = rt(b, "drop");
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(1);
    ins.local_set(mode);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(2);
    ins.local_set(mode);
    ins.else_();
    ins.local_get(out);
    ins.local_set(bits);
    ins.i32_const(0);
    ins.local_set(mode);
    ins.end();
    ins.end();
    let _ = drop;
}

// ---- the interruption ----

fn interrupted_new(b: &mut Builder, ins: &mut InstructionSink) {
    let desc = b.interrupted_desc;
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(1);
    ins.local_get(1);
    ins.local_get(0);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    ins.local_get(1);
}

/// `is_interrupted(x)`: whether `x`'s descriptor is the `interrupted` one.
fn is_interrupted(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.global_get(G_INTERRUPTED);
    ins.i32_eq();
    ins.else_();
    ins.i32_const(0);
    ins.end();
}

// ---- the perform of a `suspend fn` operation ----

/// `str_from_raw(ptr, len) -> String`: a String from a raw byte range (a descriptor's name).
fn str_from_raw(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 2;
    const I: u32 = 3;
    ins.local_get(1);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(1);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(0);
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
    ins.local_get(P);
}

/// Pushes `operation `op` of handler `name`` for the perform in local `p`.
fn build_where(b: &mut Builder, ins: &mut InstructionSink, p: u32) {
    let append = rt(b, "str_append");
    let prefix = b.string_literal(b"operation `");
    let mid = b.string_literal(b"` of handler `");
    let suffix = b.string_literal(b"`");
    ins.i32_const(prefix as i32);
    ins.local_get(p);
    ins.i32_load(mem(PERF_OP, 4));
    ins.call(append);
    ins.i32_const(mid as i32);
    ins.call(append);
    ins.local_get(p);
    ins.i32_load(mem(PERF_HANDLER, 4));
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_NAME, 4));
    ins.local_get(p);
    ins.i32_load(mem(PERF_HANDLER, 4));
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_NAMELEN, 4));
    ins.call(rt(b, "str_from_raw"));
    ins.call(append);
    ins.i32_const(suffix as i32);
    ins.call(append);
}

/// `resume_new(handler, op, repr) -> rs`: the resume value passed to a `suspend fn` operation's
/// implementation.
fn resume_new(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    const RS: u32 = 4;
    let desc = perform_desc(b);
    let rdesc = resume_desc(b);
    ins.i32_const(PERF_SLOTS as i32);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(PERF_USED, 4));
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(PERF_HAS_NOW, 4));
    ins.local_get(P);
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(0);
    ins.i32_ne();
    ins.i32_store(mem(PERF_SYNC, 4));
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(PERF_SYNC_PHASE, 4));
    ins.local_get(P);
    ins.i64_const(0);
    ins.i64_store(mem(PERF_NOW, 8));
    ins.local_get(P);
    ins.local_get(1);
    ins.i32_store(mem(PERF_OP, 4));
    ins.local_get(P);
    ins.local_get(0);
    ins.i32_store(mem(PERF_HANDLER, 4));
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    // The resume closure's table index, indexed by the repr code.
    let base = {
        let closures = b.resume_closure;
        let base = b.alloc_static(20);
        for (i, &t) in closures.iter().enumerate() {
            b.write_u32(base + i as u32 * 4, t);
        }
        base
    };
    ins.i32_const(2);
    ins.i32_const(rdesc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(RS);
    ins.local_get(RS);
    ins.i32_const(base as i32);
    ins.local_get(2);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(RES_TABLE, 4));
    ins.local_get(RS);
    ins.local_get(P);
    ins.i32_store(mem(RES_PERF, 4));
    ins.local_get(RS);
    ins.call(rt(b, "dup"));
    ins.local_get(RS);
}

/// `perform_end(rs) -> i64`: the value to continue the operation call with, or a suspension.
fn perform_end(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 1;
    const W: u32 = 2;
    const CHAIN: u32 = 3;
    const REQUEST: u32 = 4;
    const DEAD: u32 = 5;
    const MODE: u32 = 6;
    const VAL: u32 = 7;
    const OUT: u32 = 9;
    const NOW: u32 = 10;
    let drop = rt(b, "drop");
    let raise = rt(b, "raise");
    ins.local_get(0);
    ins.i32_load(mem(RES_PERF, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(PERF_SYNC_PHASE, 4));
    ins.i64_const(0);
    ins.local_set(OUT);
    ins.i32_const(0);
    ins.local_set(REQUEST);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    // The implementation suspended: a `sync` call never resumes it.
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(CHAIN);
    clear_suspension(ins);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.call(drop);
    ins.end();
    ins.local_get(CHAIN);
    ins.call(drop);
    ins.else_();
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.local_set(VAL);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(1);
    ins.local_set(MODE);
    settled(b, ins, P, MODE, VAL);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(VAL);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(2);
    ins.local_set(MODE);
    settled(b, ins, P, MODE, VAL);
    ins.end();
    ins.end();
    ins.end();
    // dead: a defect already ended the sync call.
    ins.local_get(P);
    ins.i32_load(mem(PERF_SYNC, 4));
    ins.if_(BlockType::Result(ValType::I32));
    ins.global_get(G_SYNC_DEFECT);
    ins.i32_const(0);
    ins.i32_ne();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.local_set(DEAD);
    ins.local_get(DEAD);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(PERF_HAS_NOW, 4));
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(0);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
    ins.else_();
    ins.local_get(P);
    ins.i32_load(mem(PERF_HAS_NOW, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i64_load(mem(PERF_NOW, 8));
    ins.local_set(OUT);
    ins.local_get(P);
    ins.i32_const(0);
    ins.i32_store(mem(PERF_HAS_NOW, 4));
    ins.else_();
    ins.i32_const(1);
    ins.local_set(REQUEST);
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.local_get(P);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.call(drop);
    ins.local_get(REQUEST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.call(drop);
    ins.end();
    let _ = (NOW, raise);
    ins.local_get(OUT);
}

fn clear_suspension(ins: &mut InstructionSink) {
    ins.i32_const(0);
    ins.global_set(G_SUSPENDING);
    ins.i32_const(0);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
}

/// How the implementation ended with a pending defect (`mode` 1) or failure (`mode` 2). Before it
/// resumes this goes to the caller; after it ends the run (spec 14.5).
fn settled(b: &mut Builder, ins: &mut InstructionSink, p: u32, mode: u32, value: u32) {
    let raise = rt(b, "raise");
    ins.local_get(p);
    ins.i32_load(mem(PERF_USED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(mode);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // A failure after resume ends the run with the message (spec 14.5).
    build_where(b, ins, p);
    let tail = b.string_literal(b" failed after resume: ");
    ins.i32_const(tail as i32);
    ins.call(rt(b, "str_append"));
    ins.call(raise);
    ins.else_();
    ins.local_get(value);
    ins.call(raise);
    ins.end();
    ins.else_();
    pass(b, ins, p, value);
    ins.end();
}

/// Passes the input to the caller: during the call it is remembered; otherwise (a `sync` call that
/// already returned) it is dropped.
fn pass(b: &mut Builder, ins: &mut InstructionSink, p: u32, value: u32) {
    ins.local_get(p);
    ins.i32_load(mem(PERF_SYNC_PHASE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(p);
    ins.i32_const(1);
    ins.i32_store(mem(PERF_HAS_NOW, 4));
    ins.local_get(p);
    ins.local_get(value);
    ins.i64_extend_i32_u();
    ins.i64_store(mem(PERF_NOW, 8));
    ins.else_();
    ins.local_get(value);
    ins.call(rt(b, "drop"));
    ins.end();
}

/// `resume_with(p, value)`: the implementation resumed the operation.
fn resume_with(b: &mut Builder, ins: &mut InstructionSink, value: Repr, param: u32) {
    const P: u32 = 2;
    const MSG: u32 = 3;
    let raise = rt(b, "raise");
    ins.local_get(0);
    ins.i32_load(mem(RES_PERF, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_load(mem(PERF_USED, 4));
    ins.if_(BlockType::Empty);
    // Resume called twice (spec 14.5).
    let twice = b.string_literal(b"resume called twice in ");
    ins.i32_const(twice as i32);
    build_where(b, ins, P);
    ins.call(rt(b, "str_append"));
    ins.local_set(MSG);
    ins.local_get(MSG);
    ins.call(raise);
    if value == Repr::Ref {
        ins.local_get(param);
        ins.call(rt(b, "drop"));
    }
    ins.return_();
    ins.end();
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(PERF_USED, 4));
    // pass(p, the value): the sync phase remembers it for `perform_end`.
    ins.local_get(P);
    ins.i32_load(mem(PERF_SYNC_PHASE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_store(mem(PERF_HAS_NOW, 4));
    ins.local_get(P);
    match value {
        Repr::I64 => {
            ins.local_get(param);
        }
        Repr::F64 => {
            ins.local_get(param);
            ins.i64_reinterpret_f64();
        }
        Repr::I32 | Repr::Ref => {
            ins.local_get(param);
            ins.i64_extend_i32_u();
        }
        Repr::Void => {
            ins.i64_const(0);
        }
    }
    ins.i64_store(mem(PERF_NOW, 8));
    ins.else_();
    if value == Repr::Ref {
        ins.local_get(param);
        ins.call(rt(b, "drop"));
    }
    ins.end();
}

/// The body of a resume closure: `(env, arg)` -> `resume_with(env's perform, arg)`.
fn resume_call(b: &mut Builder, ins: &mut InstructionSink, value: Repr, param: u32) {
    resume_with(b, ins, value, param);
}
