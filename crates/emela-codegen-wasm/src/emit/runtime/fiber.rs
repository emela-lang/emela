//! The fiber runtime (spec chapter 11): `Fiber.scope`, `Fiber.fork`, `Fiber.join`,
//! `Deferred`, `Fiber.attempt` and the `Cause` operations.
//!
//! The scheduler (roots, fibers, waits, the driver loop, causes and notes) lives in
//! `runtime::sched`; this fragment holds the operations the `Fiber` values lower to (`emit::fiber`)
//! and the resume frames of `Fiber.scope` and `Fiber.attempt`.

use super::sched::{
    CAUSE_DIE, CAUSE_FAIL, CAUSE_INTERRUPT, CR_KIND, CR_VALUE, D_FULL, D_REF, D_SCOPE, D_SELF,
    D_VALUE, D_WAITERS, F_CAUSE, F_ENDED, F_HOLDS, F_PENDING, F_RES_VAL, F_SCOPE, F_SCOPES,
    F_UNWINDS, F_WATCHERS, MODE_FAIL, MODE_THROW, MODE_VALUE, REF_CODE, S_BODY_DONE, S_CHILDREN,
    S_CLOSED, S_CLOSER, S_DEPTH, S_ENDED, S_FIBER, S_FIRST, S_SELF, V_ITEMS, V_LEN, VERB_AWAIT,
    VERB_DEFERRED, VERB_INTERRUPT, VERB_JOIN, WAIT_FIBER, desc_deferred, desc_scope, set_input,
};
use super::*;

/// The saved slots of a fiber frame (after the header's resume/state/next).
const FS0: u32 = HDR + 24;
const FS1: u32 = HDR + 32;
const FS2: u32 = HDR + 40;
const FS3: u32 = HDR + 48;

const J_FIBER: u32 = 16;
const J_FN: u32 = 24;
const KIND_BEGIN: i32 = 1;

/// The fiber fragment (see `runtime::FUNCS`).
pub(super) const FUNCS: &[Sig] = &[
    (
        "fiber_scope",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    ("fiber_fork", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("fiber_deferred", &[ValType::I32], &[ValType::I32]),
    ("fiber_join", &[ValType::I32, ValType::I32], &[ValType::I64]),
    (
        "fiber_await",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fiber_interrupt", &[ValType::I32], &[]),
    (
        "fiber_attempt",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "deferred_complete",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I32],
    ),
    (
        "deferred_await",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    ),
    ("deferred_poll", &[ValType::I32; 4], &[ValType::I32]),
    ("scope_body_resume", &[ValType::I32], &[ValType::I64]),
    ("scope_close_resume", &[ValType::I32], &[ValType::I64]),
    ("attempt_resume", &[ValType::I32], &[ValType::I64]),
    (
        "scope_finish",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[ValType::I64],
    ),
    (
        "scope_body_done",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I64],
        &[ValType::I64],
    ),
    (
        "attempt_finish",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[ValType::I32],
    ),
    ("exit_new", &[ValType::I32, ValType::I64], &[ValType::I32]),
];

pub(super) fn handles(name: &str) -> bool {
    FUNCS.iter().any(|(n, ..)| *n == name)
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fiber_scope" => fiber_scope(b, ins),
        "fiber_fork" => fiber_fork(b, ins),
        "fiber_deferred" => fiber_deferred(b, ins),
        "fiber_join" => fiber_join(b, ins),
        "fiber_await" => fiber_await(b, ins),
        "fiber_interrupt" => fiber_interrupt(b, ins),
        "fiber_attempt" => fiber_attempt(b, ins),
        "deferred_complete" => deferred_complete(b, ins),
        "deferred_await" => deferred_await(b, ins),
        "deferred_poll" => deferred_poll(b, ins),
        "scope_body_resume" => scope_body_resume(b, ins),
        "scope_close_resume" => scope_close_resume(b, ins),
        "attempt_resume" => attempt_resume(b, ins),
        "scope_finish" => scope_finish(b, ins),
        "scope_body_done" => scope_body_done(b, ins),
        "attempt_finish" => attempt_finish(b, ins),
        "exit_new" => exit_new(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    let l = match name {
        "fiber_scope" => vec![(5, ValType::I32), (2, ValType::I64)],
        "fiber_fork" => vec![(3, ValType::I32)],
        "fiber_deferred" => vec![(4, ValType::I32)],
        "fiber_join" => vec![(2, ValType::I32)],
        "fiber_await" => vec![(3, ValType::I32), (1, ValType::I64)],
        "fiber_interrupt" => vec![(2, ValType::I32)],
        "fiber_attempt" => vec![(3, ValType::I32), (2, ValType::I64)],
        "deferred_complete" => vec![(3, ValType::I32)],
        "deferred_await" => vec![(2, ValType::I32)],
        "deferred_poll" => vec![(2, ValType::I32)],
        "scope_body_resume" => vec![(3, ValType::I32), (1, ValType::I64)],
        "scope_close_resume" => vec![(3, ValType::I32), (1, ValType::I64)],
        "attempt_resume" => vec![(2, ValType::I32), (1, ValType::I64)],
        "scope_finish" => vec![(3, ValType::I32)],
        "scope_body_done" => vec![(6, ValType::I32)],
        "attempt_finish" => vec![(2, ValType::I32)],
        "exit_new" => vec![(1, ValType::I32)],
        _ => return None,
    };
    Some(l)
}

fn rt(b: &Builder, name: &'static str) -> u32 {
    b.rt[name]
}

fn st32(ins: &mut InstructionSink, base: u32, off: u32, val: u32) {
    ins.local_get(base);
    ins.local_get(val);
    ins.i32_store(mem(off, 4));
}

fn setc(ins: &mut InstructionSink, base: u32, off: u32, v: i32) {
    ins.local_get(base);
    ins.i32_const(v);
    ins.i32_store(mem(off, 4));
}

/// Pushes `env`'s closure record, then its table index, for a one-argument indirect call.
fn push_env1(ins: &mut InstructionSink, env: u32) {
    ins.local_get(env);
    ins.local_get(env);
    ins.i32_load(mem(HDR, 4));
}

/// Pushes `env`, `arg`, then `env`'s table index, for a two-argument indirect call.
fn push_env2(ins: &mut InstructionSink, env: u32, arg: u32) {
    ins.local_get(env);
    ins.local_get(arg);
    ins.local_get(env);
    ins.i32_load(mem(HDR, 4));
}

/// Calls `env` with one argument and its `repr`, widening the result to a word (`i64`).
fn call1_word(b: &mut Builder, ins: &mut InstructionSink, env: u32, repr: u32) {
    let t_i32 = b.ty(vec![ValType::I32], vec![ValType::I32]);
    let t_i64 = b.ty(vec![ValType::I32], vec![ValType::I64]);
    let t_f64 = b.ty(vec![ValType::I32], vec![ValType::F64]);
    let t_void = b.ty(vec![ValType::I32], vec![]);
    ins.local_get(repr);
    ins.i32_const(0);
    ins.i32_eq();
    ins.local_get(repr);
    ins.i32_const(3);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Result(ValType::I64));
    push_env1(ins, env);
    ins.call_indirect(0, t_i32);
    ins.i64_extend_i32_u();
    ins.else_();
    ins.local_get(repr);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I64));
    push_env1(ins, env);
    ins.call_indirect(0, t_i64);
    ins.else_();
    ins.local_get(repr);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I64));
    push_env1(ins, env);
    ins.call_indirect(0, t_f64);
    ins.i64_reinterpret_f64();
    ins.else_();
    push_env1(ins, env);
    ins.call_indirect(0, t_void);
    ins.i64_const(0);
    ins.end();
    ins.end();
    ins.end();
}

/// Calls `env` with two arguments (`env`, `arg`) and its `repr`, widening to a word.
fn call2_word(b: &mut Builder, ins: &mut InstructionSink, env: u32, arg: u32, repr: u32) {
    let t_i32 = b.ty(vec![ValType::I32; 2], vec![ValType::I32]);
    let t_i64 = b.ty(vec![ValType::I32; 2], vec![ValType::I64]);
    let t_f64 = b.ty(vec![ValType::I32; 2], vec![ValType::F64]);
    let t_void = b.ty(vec![ValType::I32; 2], vec![]);
    ins.local_get(repr);
    ins.i32_const(0);
    ins.i32_eq();
    ins.local_get(repr);
    ins.i32_const(3);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Result(ValType::I64));
    push_env2(ins, env, arg);
    ins.call_indirect(0, t_i32);
    ins.i64_extend_i32_u();
    ins.else_();
    ins.local_get(repr);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I64));
    push_env2(ins, env, arg);
    ins.call_indirect(0, t_i64);
    ins.else_();
    ins.local_get(repr);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I64));
    push_env2(ins, env, arg);
    ins.call_indirect(0, t_f64);
    ins.i64_reinterpret_f64();
    ins.else_();
    push_env2(ins, env, arg);
    ins.call_indirect(0, t_void);
    ins.i64_const(0);
    ins.end();
    ins.end();
    ins.end();
}

/// Reads the resume input from the globals: the pending defect or failure, or `G_RESUME`.
fn read_input(ins: &mut InstructionSink, mode: u32, bits: u32) {
    ins.global_get(G_RESUME);
    ins.local_set(bits);
    ins.i32_const(MODE_VALUE);
    ins.local_set(mode);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(MODE_THROW);
    ins.local_set(mode);
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(MODE_FAIL);
    ins.local_set(mode);
    ins.end();
}

/// Puts the pending defect or failure into `(mode, bits)`; otherwise `(VALUE, out)`.
fn taken(ins: &mut InstructionSink, out: u32, mode: u32, bits: u32) {
    ins.i32_const(MODE_VALUE);
    ins.local_set(mode);
    ins.local_get(out);
    ins.local_set(bits);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(MODE_THROW);
    ins.local_set(mode);
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(bits);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(MODE_FAIL);
    ins.local_set(mode);
    ins.end();
}

fn success_desc(b: &mut Builder, ins: &mut InstructionSink, repr: u32) {
    let val = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Success".to_owned(),
        show: None,
        refs: vec![false],
    });
    let rf = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Success".to_owned(),
        show: None,
        refs: vec![true],
    });
    ins.local_get(repr);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(rf as i32);
    ins.else_();
    ins.i32_const(val as i32);
    ins.end();
}

fn failure_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 1,
        enum_id: -1,
        name: "Failure".to_owned(),
        show: None,
        refs: vec![true],
    })
}

fn exit_new(b: &mut Builder, ins: &mut InstructionSink) {
    const E: u32 = 2;
    ins.i32_const(1);
    ins.local_get(0);
    ins.call(rt(b, "alloc"));
    ins.local_set(E);
    ins.local_get(E);
    ins.local_get(1);
    ins.i64_store(mem(HDR, 8));
    ins.local_get(E);
}

fn fiber_scope(b: &mut Builder, ins: &mut InstructionSink) {
    const S: u32 = 2;
    const SO: u32 = 3;
    const FR: u32 = 4;
    const MODE: u32 = 5;
    const OUT: u32 = 7;
    const BITS: u32 = 8;
    let desc = desc_scope(b);
    let body_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.scope".to_owned(),
        show: None,
        refs: vec![false, false, true, true, false],
    });
    let table = b.fiber_resume[0];
    ins.i32_const(72);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(S);
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(SO);
    ins.local_get(SO);
    ins.local_get(S);
    ins.i32_store(mem(HDR, 4));
    st32(ins, S, S_SELF, SO);
    ins.local_get(S);
    ins.global_get(G_CUR);
    ins.i32_store(mem(S_FIBER, 4));
    ins.local_get(S);
    ins.global_get(G_CUR);
    ins.i32_load(mem(F_SCOPES, 4));
    ins.i32_store(mem(S_DEPTH, 4));
    ins.global_get(G_CUR);
    ins.global_get(G_CUR);
    ins.i32_load(mem(F_SCOPES, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(F_SCOPES, 4));
    setc(ins, S, S_CLOSED, 0);
    setc(ins, S, S_BODY_DONE, 0);
    setc(ins, S, S_ENDED, 0);
    setc(ins, S, S_FIRST, 0);
    setc(ins, S, S_CLOSER, 0);
    ins.i32_const(256);
    ins.call(rt(b, "vec_new"));
    ins.local_set(FR);
    st32(ins, S, S_CHILDREN, FR);
    // The body gets an owned reference to the scope value.
    ins.local_get(SO);
    ins.call(rt(b, "dup"));
    call2_word(b, ins, 0, SO, 1);
    ins.local_set(OUT);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.i32_const(body_desc as i32);
    ins.i32_const(table as i32);
    ins.i32_const(0);
    ins.i32_const(2);
    ins.call(rt(b, "frame_new"));
    ins.local_set(FR);
    st32(ins, FR, FS0, SO);
    st32(ins, FR, FS1, 1);
    ins.local_get(FR);
    ins.call(rt(b, "frame_push"));
    ins.i64_const(0);
    ins.return_();
    ins.end();
    taken(ins, OUT, MODE, BITS);
    ins.local_get(SO);
    ins.local_get(1);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "scope_body_done"));
}

fn fiber_fork(b: &mut Builder, ins: &mut InstructionSink) {
    const S: u32 = 2;
    const CHILD: u32 = 3;
    const JOB: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_load(mem(S_CLOSED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"fiber scope was used after it ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_ROOT);
    ins.local_get(S);
    ins.call(rt(b, "fiber_new"));
    ins.local_set(CHILD);
    ins.local_get(S);
    ins.i32_load(mem(S_CHILDREN, 4));
    ins.local_get(CHILD);
    ins.call(rt(b, "vec_push"));
    ins.i32_const(KIND_BEGIN);
    ins.call(rt(b, "job_new"));
    ins.local_set(JOB);
    st32(ins, JOB, J_FIBER, CHILD);
    ins.local_get(JOB);
    ins.local_get(1);
    ins.i32_store(mem(J_FN, 4));
    ins.local_get(1);
    ins.call(rt(b, "dup"));
    ins.global_get(G_ROOT);
    ins.local_get(JOB);
    ins.call(rt(b, "schedule"));
    ins.local_get(CHILD);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dup"));
    ins.local_get(CHILD);
    ins.i32_load(mem(0, 4));
}

fn fiber_deferred(b: &mut Builder, ins: &mut InstructionSink) {
    const S: u32 = 1;
    const D: u32 = 2;
    const O: u32 = 3;
    const T: u32 = 4;
    let desc = desc_deferred(b);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_load(mem(S_CLOSED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"fiber scope was used after it ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(48);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(D);
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(O);
    ins.local_get(O);
    ins.local_get(D);
    ins.i32_store(mem(HDR, 4));
    st32(ins, D, D_SELF, O);
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    ins.local_get(D);
    ins.local_get(0);
    ins.i32_store(mem(D_SCOPE, 4));
    setc(ins, D, D_FULL, 0);
    setc(ins, D, D_REF, 0);
    ins.i32_const(256);
    ins.call(rt(b, "vec_new"));
    ins.local_set(T);
    st32(ins, D, D_WAITERS, T);
    ins.local_get(O);
}

fn fiber_join(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 2;
    const W: u32 = 3;
    ins.call(rt(b, "interrupt_here"));
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(C);
    ins.local_get(C);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"fiber was joined after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(C);
    ins.i32_load(mem(F_ENDED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i64_load(mem(F_RES_VAL, 8));
    ins.i32_wrap_i64();
    ins.local_set(W);
    ins.local_get(W);
    ins.call(rt(b, "dup"));
    ins.local_get(W);
    ins.i64_extend_i32_u();
    ins.local_get(1);
    ins.call(rt(b, "unbox"));
    ins.return_();
    ins.else_();
    ins.local_get(C);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    let s = b.string_literal(b"fiber was interrupted");
    ins.i32_const(s as i32);
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.return_();
    ins.else_();
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.end();
    ins.end();
    ins.i32_const(WAIT_FIBER);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    ins.local_get(C);
    ins.i32_load(mem(F_WATCHERS, 4));
    ins.local_get(W);
    ins.i32_const(VERB_JOIN);
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_const(0);
    ins.call(rt(b, "watch_add"));
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.i64_const(0);
}

fn fiber_await(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 2;
    const W: u32 = 3;
    const TMP: u32 = 4;
    const SV: u32 = 5;
    let fail = failure_desc(b);
    ins.call(rt(b, "interrupt_here"));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(C);
    ins.local_get(C);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"fiber was awaited after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(C);
    ins.i32_load(mem(F_ENDED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i64_load(mem(F_RES_VAL, 8));
    ins.i32_wrap_i64();
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.call(rt(b, "dup"));
    ins.local_get(TMP);
    ins.i64_extend_i32_u();
    ins.local_get(1);
    ins.call(rt(b, "unbox"));
    ins.local_set(SV);
    success_desc(b, ins, 1);
    ins.local_get(SV);
    ins.call(rt(b, "exit_new"));
    ins.return_();
    ins.else_();
    ins.i32_const(fail as i32);
    ins.local_get(C);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.local_set(W);
    ins.local_get(W);
    ins.call(rt(b, "dup"));
    ins.local_get(W);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "exit_new"));
    ins.return_();
    ins.end();
    ins.end();
    ins.i32_const(WAIT_FIBER);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    ins.local_get(C);
    ins.i32_load(mem(F_WATCHERS, 4));
    ins.local_get(W);
    ins.i32_const(VERB_AWAIT);
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_const(0);
    ins.call(rt(b, "watch_add"));
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.i32_const(0);
}

fn fiber_interrupt(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 1;
    const W: u32 = 2;
    ins.call(rt(b, "interrupt_here"));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(C);
    ins.local_get(C);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"fiber was interrupted after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.return_();
    ins.end();
    ins.local_get(C);
    ins.i32_const(0);
    ins.call(rt(b, "interrupt_fiber"));
    ins.local_get(C);
    ins.global_get(G_CUR);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.call(rt(b, "deliver"));
    ins.global_set(G_DEFECT);
    ins.end();
    ins.return_();
    ins.end();
    ins.local_get(C);
    ins.i32_load(mem(F_ENDED, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.i32_const(WAIT_FIBER);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    ins.local_get(C);
    ins.i32_load(mem(F_WATCHERS, 4));
    ins.local_get(W);
    ins.i32_const(VERB_INTERRUPT);
    ins.i32_const(4);
    ins.i32_const(0);
    ins.i32_const(0);
    ins.call(rt(b, "watch_add"));
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
}

fn fiber_attempt(b: &mut Builder, ins: &mut InstructionSink) {
    const FR: u32 = 2;
    const MODE: u32 = 3;
    const OUT: u32 = 5;
    const BITS: u32 = 6;
    let attempt_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.attempt".to_owned(),
        show: None,
        refs: vec![false, false, true, false],
    });
    let table = b.fiber_resume[2];
    call1_word(b, ins, 0, 1);
    ins.local_set(OUT);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.i32_const(attempt_desc as i32);
    ins.i32_const(table as i32);
    ins.i32_const(0);
    ins.i32_const(1);
    ins.call(rt(b, "frame_new"));
    ins.local_set(FR);
    st32(ins, FR, FS0, 1);
    ins.local_get(FR);
    ins.call(rt(b, "frame_push"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    taken(ins, OUT, MODE, BITS);
    ins.local_get(1);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "attempt_finish"));
}

fn deferred_complete(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 3;
    const WS: u32 = 4;
    const I: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(D_SCOPE, 4));
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"deferred was completed after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(D);
    ins.i32_load(mem(D_FULL, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    setc(ins, D, D_FULL, 1);
    ins.local_get(D);
    ins.local_get(2);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.i32_store(mem(D_REF, 4));
    ins.local_get(D);
    ins.local_get(1);
    ins.i64_store(mem(D_VALUE, 8));
    ins.local_get(2);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.call(rt(b, "dup"));
    ins.end();
    ins.local_get(D);
    ins.i32_load(mem(D_WAITERS, 4));
    ins.local_set(WS);
    ins.local_get(WS);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(D);
    ins.i32_load(mem(D_REF, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.call(rt(b, "dup"));
    ins.end();
    ins.local_get(WS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.i32_load(mem(0, 4));
    ins.i32_const(MODE_VALUE);
    ins.local_get(1);
    ins.call(rt(b, "cont"));
    ins.br(0);
    ins.end();
    ins.end();
    setc(ins, WS, V_LEN, 0);
    ins.i32_const(1);
}

fn deferred_await(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 2;
    const W: u32 = 3;
    ins.call(rt(b, "interrupt_here"));
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(D_SCOPE, 4));
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"deferred was awaited after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(D);
    ins.i32_load(mem(D_FULL, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(D);
    ins.i32_load(mem(D_REF, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(D);
    ins.i64_load(mem(D_VALUE, 8));
    ins.i32_wrap_i64();
    ins.call(rt(b, "dup"));
    ins.end();
    ins.local_get(D);
    ins.i64_load(mem(D_VALUE, 8));
    ins.return_();
    ins.end();
    ins.i32_const(WAIT_FIBER);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    ins.local_get(D);
    ins.i32_load(mem(D_WAITERS, 4));
    ins.local_get(W);
    ins.i32_const(VERB_DEFERRED);
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_const(0);
    ins.call(rt(b, "watch_add"));
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.i64_const(0);
}

fn deferred_poll(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 4;
    const O: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(D_SCOPE, 4));
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(S_ENDED, 4));
    ins.if_(BlockType::Empty);
    let m = b.string_literal(b"deferred was polled after its scope ended");
    ins.i32_const(m as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(D);
    ins.i32_load(mem(D_FULL, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.local_get(D);
    ins.i32_load(mem(D_REF, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(D);
    ins.i64_load(mem(D_VALUE, 8));
    ins.i32_wrap_i64();
    ins.call(rt(b, "dup"));
    ins.end();
    ins.i32_const(1);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(O);
    ins.local_get(O);
    ins.local_get(D);
    ins.i64_load(mem(D_VALUE, 8));
    ins.i64_store(mem(HDR, 8));
    ins.local_get(O);
}

fn scope_body_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const SO: u32 = 1;
    const REPR: u32 = 2;
    const MODE: u32 = 3;
    const BITS: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(FS0, 4));
    ins.local_set(SO);
    ins.local_get(0);
    ins.i32_load(mem(FS1, 4));
    ins.local_set(REPR);
    ins.local_get(0);
    ins.call(rt(b, "frame_free"));
    read_input(ins, MODE, BITS);
    ins.local_get(SO);
    ins.local_get(REPR);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "scope_body_done"));
}

fn scope_close_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const SO: u32 = 1;
    const MODE: u32 = 2;
    const FIB: u32 = 3;
    const BITS: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(FS0, 4));
    ins.local_set(SO);
    ins.local_get(0);
    ins.i32_load(mem(FS2, 4));
    ins.local_set(MODE);
    ins.local_get(0);
    ins.i64_load(mem(FS3, 8));
    ins.local_set(BITS);
    ins.local_get(0);
    ins.call(rt(b, "frame_free"));
    ins.local_get(SO);
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(S_FIBER, 4));
    ins.local_set(FIB);
    ins.local_get(FIB);
    ins.local_get(FIB);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.local_get(SO);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "scope_finish"));
}

fn attempt_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const REPR: u32 = 1;
    const MODE: u32 = 2;
    const BITS: u32 = 3;
    ins.local_get(0);
    ins.i32_load(mem(FS0, 4));
    ins.local_set(REPR);
    ins.local_get(0);
    ins.call(rt(b, "frame_free"));
    read_input(ins, MODE, BITS);
    ins.local_get(REPR);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "attempt_finish"));
    ins.i64_extend_i32_u();
}

/// `scope_finish(so, mode, bits) -> word`: after every child ended, the first failure or the body's
/// result (spec 11.2).
fn scope_finish(b: &mut Builder, ins: &mut InstructionSink) {
    const S: u32 = 3;
    const FIRST: u32 = 4;
    const V: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(S);
    setc(ins, S, S_ENDED, 1);
    ins.local_get(S);
    ins.i32_load(mem(S_FIRST, 4));
    ins.local_set(FIRST);
    ins.local_get(FIRST);
    ins.if_(BlockType::Empty);
    ins.local_get(FIRST);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.local_set(V);
    ins.local_get(V);
    ins.call(rt(b, "dup"));
    ins.local_get(FIRST);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(V);
    ins.global_set(G_FAILURE);
    ins.else_();
    ins.local_get(V);
    ins.global_set(G_DEFECT);
    ins.end();
    ins.i64_const(0);
    ins.return_();
    ins.end();
    set_input(ins, 1, 2);
    ins.local_get(1);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I64));
    ins.local_get(2);
    ins.else_();
    ins.i64_const(0);
    ins.end();
}

fn scope_body_done(b: &mut Builder, ins: &mut InstructionSink) {
    const S: u32 = 4;
    const F: u32 = 5;
    const W: u32 = 6;
    const FR: u32 = 7;
    const CH: u32 = 8;
    const I: u32 = 9;
    let close_ref = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.scope".to_owned(),
        show: None,
        refs: vec![false, false, true, true, false, false, true],
    });
    let close_val = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber.scope".to_owned(),
        show: None,
        refs: vec![false, false, true, true, false, false, false],
    });
    let close_table = b.fiber_resume[1];
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_load(mem(S_FIBER, 4));
    ins.local_set(F);
    ins.local_get(F);
    ins.local_get(F);
    ins.i32_load(mem(F_SCOPES, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(F_SCOPES, 4));
    ins.local_get(F);
    ins.i32_load(mem(F_PENDING, 4));
    ins.local_get(S);
    ins.call(rt(b, "vec_remove"));
    ins.local_get(F);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.local_get(S);
    ins.call(rt(b, "vec_remove"));
    setc(ins, S, S_BODY_DONE, 1);
    setc(ins, S, S_CLOSED, 1);
    ins.local_get(2);
    ins.i32_const(MODE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(S);
    ins.local_get(3);
    ins.i32_wrap_i64();
    ins.local_set(I);
    ins.i32_const(CAUSE_FAIL);
    ins.local_get(I);
    ins.call(rt(b, "dup"));
    ins.local_get(I);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cause_new"));
    ins.call(rt(b, "scope_failed"));
    ins.else_();
    ins.local_get(2);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.i32_wrap_i64();
    ins.call(rt(b, "is_interrupted"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(S);
    ins.local_get(3);
    ins.i32_wrap_i64();
    ins.local_set(I);
    ins.i32_const(CAUSE_DIE);
    ins.local_get(I);
    ins.call(rt(b, "dup"));
    ins.local_get(I);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cause_new"));
    ins.call(rt(b, "scope_failed"));
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(S);
    ins.i32_load(mem(S_CHILDREN, 4));
    ins.local_set(CH);
    ins.local_get(CH);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(CH);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.i32_const(0);
    ins.call(rt(b, "interrupt_fiber"));
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(S);
    ins.i32_load(mem(S_CHILDREN, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(2);
    ins.local_get(3);
    ins.call(rt(b, "scope_finish"));
    ins.return_();
    ins.end();
    ins.local_get(F);
    ins.local_get(F);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.i32_const(WAIT_FIBER);
    ins.i32_const(1);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    st32(ins, S, S_CLOSER, W);
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.local_get(2);
    ins.i32_const(MODE_VALUE);
    ins.i32_eq();
    ins.local_get(1);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(close_ref as i32);
    ins.else_();
    ins.i32_const(close_val as i32);
    ins.end();
    ins.i32_const(close_table as i32);
    ins.i32_const(0);
    ins.i32_const(4);
    ins.call(rt(b, "frame_new"));
    ins.local_set(FR);
    st32(ins, FR, FS0, 0);
    st32(ins, FR, FS1, 1);
    st32(ins, FR, FS2, 2);
    ins.local_get(FR);
    ins.local_get(3);
    ins.i64_store(mem(FS3, 8));
    ins.local_get(FR);
    ins.call(rt(b, "frame_push"));
    ins.i64_const(0);
}

fn attempt_finish(b: &mut Builder, ins: &mut InstructionSink) {
    const BITS: u32 = 2;
    const C: u32 = 3;
    let fail = failure_desc(b);
    ins.local_get(1);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.call(rt(b, "is_interrupted"));
    ins.global_get(G_CUR);
    ins.i32_const(0);
    ins.i32_ne();
    ins.if_(BlockType::Result(ValType::I32));
    ins.global_get(G_CUR);
    ins.call(rt(b, "is_unwinding"));
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.i32_or();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.i32_const(0);
    ins.else_();
    ins.i32_const(fail as i32);
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.local_set(C);
    ins.i32_const(CAUSE_DIE);
    ins.local_get(C);
    ins.call(rt(b, "dup"));
    ins.local_get(C);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cause_new"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "exit_new"));
    ins.end();
    ins.else_();
    ins.local_get(1);
    ins.i32_const(MODE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(fail as i32);
    ins.i32_const(CAUSE_FAIL);
    ins.local_get(BITS);
    ins.call(rt(b, "cause_new"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "exit_new"));
    ins.else_();
    success_desc(b, ins, 0);
    ins.local_get(BITS);
    ins.call(rt(b, "exit_new"));
    ins.end();
    ins.end();
}
