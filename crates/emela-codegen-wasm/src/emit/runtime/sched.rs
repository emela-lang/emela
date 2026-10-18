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
    // ---- the scheduler (spec chapters 11 and 14) ----
    ("rt_alloc", &[ValType::I32], &[ValType::I32]),
    ("vec_new", &[ValType::I32], &[ValType::I32]),
    ("vec_push", &[ValType::I32, ValType::I32], &[]),
    ("vec_has", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("vec_remove", &[ValType::I32, ValType::I32], &[]),
    ("vec_len", &[ValType::I32], &[ValType::I32]),
    ("queue_new", &[ValType::I32], &[ValType::I32]),
    ("queue_push", &[ValType::I32, ValType::I32], &[]),
    ("queue_pop", &[ValType::I32], &[ValType::I32]),
    ("job_new", &[ValType::I32], &[ValType::I32]),
    ("wait_new", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("wait_take", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("wait_drop", &[ValType::I32, ValType::I32], &[]),
    ("wait_forget", &[ValType::I32], &[]),
    ("susp_on", &[ValType::I32], &[]),
    ("fiber_new", &[ValType::I32, ValType::I32], &[ValType::I32]),
    (
        "fiber_finish",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
    ("scope_child_ended", &[ValType::I32, ValType::I32], &[]),
    ("scope_failed", &[ValType::I32, ValType::I32], &[]),
    ("watch_add", &[ValType::I32; 6], &[]),
    ("watch_deliver", &[ValType::I32, ValType::I32], &[]),
    ("unbox", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("cause_new", &[ValType::I32, ValType::I64], &[ValType::I32]),
    ("cause_of", &[ValType::I32, ValType::I64], &[ValType::I32]),
    ("cause_kind", &[ValType::I32], &[ValType::I32]),
    ("cause_to_string", &[ValType::I32], &[ValType::I32]),
    ("cause_raise", &[ValType::I32], &[]),
    ("cause_note", &[ValType::I32, ValType::I32], &[]),
    ("notes_add", &[ValType::I32, ValType::I32], &[]),
    ("notes_of", &[ValType::I32], &[ValType::I32]),
    ("notes_clear", &[], &[]),
    ("is_unwinding", &[ValType::I32], &[ValType::I32]),
    ("interrupting", &[ValType::I32], &[ValType::I32]),
    ("deliver", &[ValType::I32], &[ValType::I32]),
    ("interrupt_fiber", &[ValType::I32, ValType::I32], &[]),
    ("interrupt_here", &[], &[ValType::I32]),
    ("root_unlist", &[ValType::I32, ValType::I32], &[]),
    (
        "fiber_resume",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
        ],
        &[],
    ),
    (
        "step",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
        ],
        &[],
    ),
    (
        "advance",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
        ],
        &[],
    ),
    (
        "strand_end",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
        ],
        &[],
    ),
    ("suspended", &[ValType::I32; 6], &[]),
    ("cont", &[ValType::I32, ValType::I32, ValType::I64], &[]),
    ("run_job", &[ValType::I32, ValType::I32], &[]),
    ("drain", &[ValType::I32], &[]),
    ("schedule", &[ValType::I32, ValType::I32], &[]),
    (
        "entry_begin",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[],
    ),
    (
        "root_finish",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    (
        "root_ended",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[],
    ),
    ("root_interrupt", &[ValType::I32], &[]),
    ("run", &[ValType::I32, ValType::I32], &[]),
    ("settled", &[ValType::I32, ValType::I32, ValType::I64], &[]),
    ("pass", &[ValType::I32, ValType::I32, ValType::I64], &[]),
    ("run_fails", &[ValType::I32, ValType::I32], &[]),
    // ---- the `suspend fn` external task primitives (spec 12.6) ----
    ("task_new", &[ValType::I32], &[ValType::I32]),
    ("task_end", &[ValType::I32], &[ValType::I64]),
    ("task_complete_int", &[ValType::I32, ValType::I32], &[]),
    ("task_complete_int64", &[ValType::I32, ValType::I64], &[]),
    ("task_complete_float", &[ValType::I32, ValType::F64], &[]),
    ("task_complete_unit", &[ValType::I32], &[]),
    ("task_complete_string", &[ValType::I32, ValType::I32], &[]),
    ("task_complete_defect", &[ValType::I32, ValType::I32], &[]),
    // ---- the driver's timers (only when a timer host is requested) ----
    ("now_ms", &[], &[ValType::I32]),
    ("after", &[ValType::I32, ValType::I32], &[]),
    ("timers_due", &[ValType::I32], &[ValType::I32]),
    // ---- the test hosts (`emela:test`) ----
    ("test_sleep_ms", &[ValType::I32, ValType::I32], &[]),
    ("test_sleep_after", &[ValType::I32, ValType::I32], &[]),
];

/// The slots of a `perform` (a `suspend fn` operation call, spec 8.4): counted by hand.
/// The slots of a resume value: the table index of its closure and its `perform`.
const RES_TABLE: u32 = HDR;
const RES_PERF: u32 = HDR + 8;

/// Whether `name` is a function of this fragment (registered only when the module suspends).
pub(super) fn handles(name: &str) -> bool {
    FUNCS.iter().any(|(n, ..)| *n == name)
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

// ---- layout of the runtime structures (offsets from the `rt_alloc` pointer) ----

pub(super) const V_ITEMS: u32 = 0;
pub(super) const V_LEN: u32 = 8;
const V_CAP: u32 = 16;

const Q_ITEMS: u32 = 0;
const Q_HEAD: u32 = 8;
const Q_LEN: u32 = 16;
const Q_CAP: u32 = 24;

const J_KIND: u32 = 0;
const J_W: u32 = 8;
const J_FIBER: u32 = 16;
const J_FN: u32 = 24;
const J_MODE: u32 = 32;
const J_BITS: u32 = 40;
const J_END: u32 = 48;

pub(super) const W_KIND: u32 = 0;
const W_LIVE: u32 = 8;
const W_CLOSING: u32 = 16;
pub(super) const W_CHAIN: u32 = 24;
const W_FIBER: u32 = 32;
pub(super) const W_END: u32 = 40;
const W_IMPL: u32 = 48;
pub(super) const W_ON_DROP: u32 = 56;
const W_PREV: u32 = 72;
const W_NEXT: u32 = 80;

pub(super) const F_SELF: u32 = 0;
pub(super) const F_ROOT: u32 = 8;
pub(super) const F_SCOPE: u32 = 16;
pub(super) const F_WAIT: u32 = 24;
pub(super) const F_PENDING: u32 = 32;
pub(super) const F_UNWINDING: u32 = 40;
pub(super) const F_UNWINDS: u32 = 48;
pub(super) const F_HOLDS: u32 = 56;
pub(super) const F_SCOPES: u32 = 64;
pub(super) const F_BUDGET: u32 = 72;
pub(super) const F_ENDED: u32 = 80;
pub(super) const F_LISTED: u32 = 88;
// `F_RES_MODE`/`F_RES_REF` record how a fiber ended (C's `em_input` of the result), but only
// `F_RES_VAL` is read back (by `fiber_join`/`fiber_await`/`watch_deliver`): the result's reference
// ownership is not tracked yet (W5-7). Kept for the C `em_fiber` layout.
pub(super) const F_RES_MODE: u32 = 96;
pub(super) const F_RES_REF: u32 = 104;
pub(super) const F_RES_VAL: u32 = 112;
pub(super) const F_CAUSE: u32 = 120;
pub(super) const F_WATCHERS: u32 = 128;

pub(super) const S_SELF: u32 = 0;
pub(super) const S_FIBER: u32 = 8;
pub(super) const S_DEPTH: u32 = 16;
pub(super) const S_CHILDREN: u32 = 24;
pub(super) const S_CLOSED: u32 = 32;
pub(super) const S_BODY_DONE: u32 = 40;
pub(super) const S_ENDED: u32 = 48;
pub(super) const S_FIRST: u32 = 56;
pub(super) const S_CLOSER: u32 = 64;

pub(super) const D_SELF: u32 = 0;
pub(super) const D_SCOPE: u32 = 8;
pub(super) const D_FULL: u32 = 16;
pub(super) const D_REF: u32 = 24;
pub(super) const D_VALUE: u32 = 32;
pub(super) const D_WAITERS: u32 = 40;

const P_RUN: u32 = 0;
pub(super) const P_OP: u32 = 16;
pub(super) const P_HANDLER: u32 = 24;
const P_SYNC: u32 = 32;
const P_SYNC_PHASE: u32 = 40;
const P_USED: u32 = 48;
const P_HAS_NOW: u32 = 56;
const P_NOW_MODE: u32 = 64;
const P_NOW_BITS: u32 = 72;
const P_CALLER: u32 = 80;
pub(super) const P_IMPL_WAIT: u32 = 88;
const P_PREV_MAIN: u32 = 96;

const R_OVER: u32 = 0;
const R_RUNNING: u32 = 8;
// `R_HAS_RESULT`/`R_RES_REF` mirror the C `em_root` (a result is present, and owns a reference),
// but only `R_RES_MODE`/`R_RES_VAL` are read back in `run` (W5-7). Kept for the layout.
const R_HAS_RESULT: u32 = 16;
const R_RES_MODE: u32 = 24;
const R_RES_REF: u32 = 32;
const R_RES_VAL: u32 = 40;
const R_ENTRY: u32 = 48;
const R_REPR: u32 = 56;
const R_QUEUE: u32 = 64;
const R_LATER: u32 = 96;
const R_FIRST_WAIT: u32 = 128;
const R_LAST_WAIT: u32 = 136;
const R_FIBERS: u32 = 144;
const R_ORPHAN: u32 = 152;
const R_PAUSES: u32 = 160;

pub(super) const CR_KIND: u32 = HDR;
pub(super) const CR_VALUE: u32 = HDR + 8;

const WAT_W: u32 = 0;
const WAT_VERB: u32 = 8;
pub(super) const WAT_REPR: u32 = 16;
pub(super) const WAT_D0: u32 = 24;
pub(super) const WAT_D1: u32 = 32;

pub(super) const WAIT_PAUSE: i32 = 0;
pub(super) const WAIT_PERFORM: i32 = 1;
pub(super) const WAIT_FIBER: i32 = 2;
pub(super) const WAIT_HOST: i32 = 3;

pub(super) const END_MAIN: i32 = 0;
pub(super) const END_IMPL: i32 = 1;
const END_ORPHAN: i32 = 2;

pub(super) const VERB_JOIN: i32 = 0;
pub(super) const VERB_AWAIT: i32 = 1;
pub(super) const VERB_INTERRUPT: i32 = 2;
pub(super) const VERB_DEFERRED: i32 = 3;

const KIND_ENTRY: i32 = 0;
const KIND_BEGIN: i32 = 1;
const KIND_RESUME: i32 = 2;
const KIND_GO: i32 = 3;
const KIND_INTERRUPT: i32 = 4;
const KIND_ROOT_THROW: i32 = 5;
const KIND_DROP_IMPL: i32 = 6;

pub(super) const CAUSE_FAIL: i32 = 0;
pub(super) const CAUSE_DIE: i32 = 1;
pub(super) const CAUSE_INTERRUPT: i32 = 2;

const BUDGET: i32 = 1000;
const HOST_EVERY: i32 = 16;

pub(super) const MODE_VALUE: i32 = 0;
pub(super) const MODE_THROW: i32 = 1;
pub(super) const MODE_FAIL: i32 = 2;
/// A value input that owns a reference (`mode` is otherwise `MODE_VALUE`). A thrown defect or a
/// failure always owns one; this bit marks the value case so an undelivered input can be released
/// like the C `em_input_drop`. The producers here set it; `fiber::deferred_*` (a different
/// fragment) still passes `MODE_VALUE` for a possibly-reference value, so a deferred resolution
/// left pending when the run ends is not released yet (W5-3, needs that file).
const MODE_VALUE_REF: i32 = 4;

pub(super) const REF_CODE: i32 = 3;
pub(super) const VOID_CODE: i32 = 4;

// ---- descriptor helpers ----

fn rtstruct(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "rt".to_owned(),
        show: None,
        refs: Vec::new(),
    })
}

pub(super) fn desc_fiber(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Fiber".to_owned(),
        show: None,
        refs: vec![false],
    })
}

pub(super) fn desc_scope(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Scope".to_owned(),
        show: None,
        refs: vec![false],
    })
}

pub(super) fn desc_deferred(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Deferred".to_owned(),
        show: None,
        refs: vec![false],
    })
}

pub(super) fn desc_cause(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Cause".to_owned(),
        show: None,
        refs: vec![false, true],
    })
}

// ---- emit helpers ----

fn store32(ins: &mut InstructionSink, base: u32, off: u32, val: u32) {
    ins.local_get(base);
    ins.local_get(val);
    ins.i32_store(mem(off, 4));
}

fn setc(ins: &mut InstructionSink, base: u32, off: u32, v: i32) {
    ins.local_get(base);
    ins.i32_const(v);
    ins.i32_store(mem(off, 4));
}

/// Pushes `x` (an i32 local) and retains a reference: `dup(x)` then `x` again, so the stack gets
/// one owned value while the reference count is incremented.
fn dup_push(ins: &mut InstructionSink, b: &Builder, x: u32) {
    ins.local_get(x);
    ins.call(rt(b, "dup"));
    ins.local_get(x);
}

fn vec_item(ins: &mut InstructionSink, v: u32, i: u32) {
    ins.local_get(v);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(i);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "hold" => hold(ins),
        "unhold" => unhold(ins),
        "yield" => yield_(b, ins),
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
        "rt_alloc" => rt_alloc(b, ins),
        "vec_new" => vec_new(b, ins),
        "vec_push" => vec_push(b, ins),
        "vec_has" => vec_has(ins),
        "vec_remove" => vec_remove(ins),
        "vec_len" => vec_len(ins),
        "queue_new" => queue_new(b, ins),
        "queue_push" => queue_push(ins),
        "queue_pop" => queue_pop(ins),
        "job_new" => job_new(b, ins),
        "wait_new" => wait_new(b, ins),
        "wait_take" => wait_take(b, ins),
        "wait_drop" => wait_drop(b, ins),
        "wait_forget" => wait_forget(ins),
        "susp_on" => susp_on(ins),
        "fiber_new" => fiber_new(b, ins),
        "fiber_finish" => fiber_finish(b, ins),
        "scope_child_ended" => scope_child_ended(b, ins),
        "scope_failed" => scope_failed(b, ins),
        "watch_add" => watch_add(b, ins),
        "watch_deliver" => watch_deliver(b, ins),
        "unbox" => unbox(b, ins),
        "cause_new" => cause_new(b, ins),
        "cause_of" => cause_of(b, ins),
        "cause_kind" => cause_kind(ins),
        "cause_to_string" => cause_to_string(b, ins),
        "cause_raise" => cause_raise(b, ins),
        "cause_note" => cause_note(b, ins),
        "notes_add" => notes_add(b, ins),
        "notes_of" => notes_of(ins),
        "notes_clear" => notes_clear(ins),
        "is_unwinding" => is_unwinding(ins),
        "interrupting" => interrupting(ins),
        "deliver" => deliver(b, ins),
        "interrupt_fiber" => interrupt_fiber(b, ins),
        "interrupt_here" => interrupt_here(b, ins),
        "root_unlist" => root_unlist(ins),
        "fiber_resume" => fiber_resume(b, ins),
        "step" => step(b, ins),
        "advance" => advance(b, ins),
        "strand_end" => strand_end(b, ins),
        "suspended" => suspended(b, ins),
        "cont" => cont(b, ins),
        "run_job" => run_job(b, ins),
        "drain" => drain(b, ins),
        "schedule" => schedule(b, ins),
        "entry_begin" => entry_begin(b, ins),
        "root_finish" => root_finish(b, ins),
        "root_ended" => root_ended(b, ins),
        "root_interrupt" => root_interrupt(b, ins),
        "run" => run(b, ins),
        "task_new" => task_new(b, ins),
        "task_end" => task_end(b, ins),
        "task_complete_int" => task_complete_int(b, ins),
        "task_complete_int64" => task_complete_int64(b, ins),
        "task_complete_float" => task_complete_float(b, ins),
        "task_complete_unit" => task_complete_unit(b, ins),
        "task_complete_string" => task_complete_string(b, ins),
        "task_complete_defect" => task_complete_defect(b, ins),
        "now_ms" => now_ms(b, ins),
        "after" => after(b, ins),
        "timers_due" => timers_due(b, ins),
        "test_sleep_ms" => test_sleep_ms(b, ins),
        "test_sleep_after" => test_sleep_after(b, ins),
        "settled" => settled(b, ins),
        "pass" => pass(b, ins),
        "run_fails" => run_fails(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    let l = match name {
        "frame_new" => vec![(1, ValType::I32)],
        "frame_free" => vec![(2, ValType::I32)],
        "yield" => vec![(2, ValType::I32)],
        "sync_leave" => vec![(6, ValType::I32), (2, ValType::I64)],
        "resume_new" => vec![(2, ValType::I32)],
        "perform_end" => vec![(5, ValType::I32), (2, ValType::I64)],
        "interrupted_new" => vec![(1, ValType::I32)],
        "str_from_raw" => vec![(2, ValType::I32)],
        "resume_i32" | "resume_i64" | "resume_f64" | "resume_ref" => {
            vec![(3, ValType::I32), (1, ValType::I64)]
        }
        "resume_void" => vec![(4, ValType::I32), (1, ValType::I64)],
        "rt_alloc" => vec![(1, ValType::I32)],
        "vec_new" => vec![(2, ValType::I32)],
        "vec_push" => vec![(5, ValType::I32)],
        "vec_has" => vec![(2, ValType::I32)],
        "vec_remove" => vec![(4, ValType::I32)],
        "queue_new" => vec![(2, ValType::I32)],
        "queue_push" => vec![(4, ValType::I32)],
        "queue_pop" => vec![(2, ValType::I32)],
        "job_new" => vec![(1, ValType::I32)],
        "wait_new" => vec![(1, ValType::I32)],
        "wait_take" => vec![(1, ValType::I32)],
        "wait_drop" => vec![(4, ValType::I32)],
        "wait_forget" => vec![(2, ValType::I32)],
        "fiber_new" => vec![(2, ValType::I32)],
        "fiber_finish" => vec![(6, ValType::I32)],
        "scope_child_ended" => vec![(2, ValType::I32)],
        "scope_failed" => vec![(2, ValType::I32)],
        "watch_add" => vec![(4, ValType::I32)],
        "watch_deliver" => vec![(6, ValType::I32), (1, ValType::I64)],
        "unbox" => vec![(2, ValType::I32)],
        "cause_new" => vec![(1, ValType::I32)],
        "cause_of" => vec![(1, ValType::I32)],
        "cause_to_string" => vec![(7, ValType::I32)],
        "cause_raise" => vec![(1, ValType::I32)],
        "cause_note" => vec![(4, ValType::I32)],
        "notes_add" => vec![(9, ValType::I32)],
        "notes_of" => vec![(2, ValType::I32)],
        "notes_clear" => vec![(1, ValType::I32)],
        "is_unwinding" => vec![(1, ValType::I32)],
        "interrupting" => vec![(1, ValType::I32)],
        "deliver" => vec![(5, ValType::I32)],
        "interrupt_fiber" => vec![(2, ValType::I32)],
        "fiber_resume" => vec![(2, ValType::I32), (1, ValType::I64)],
        "step" => vec![(3, ValType::I32)],
        "advance" => vec![(4, ValType::I32), (1, ValType::I64)],
        "strand_end" => vec![(2, ValType::I32), (1, ValType::I64)],
        "suspended" => vec![(2, ValType::I32)],
        "cont" => vec![(5, ValType::I32), (1, ValType::I64)],
        "run_job" => vec![(7, ValType::I32), (1, ValType::I64)],
        "drain" => vec![(1, ValType::I32)],
        "schedule" => vec![(1, ValType::I32)],
        "entry_begin" => vec![(4, ValType::I32), (2, ValType::I64)],
        "root_finish" => vec![(3, ValType::I32)],
        "root_ended" => vec![(1, ValType::I32)],
        "root_interrupt" => vec![(8, ValType::I32)],
        "run" => vec![(8, ValType::I32), (1, ValType::I64)],
        "task_new" => vec![(1, ValType::I32)],
        "task_end" => vec![(2, ValType::I32)],
        "task_complete_int"
        | "task_complete_int64"
        | "task_complete_float"
        | "task_complete_unit"
        | "task_complete_string"
        | "task_complete_defect" => vec![],
        "now_ms" => vec![(1, ValType::I32)],
        "after" => vec![(2, ValType::I32)],
        "timers_due" => vec![(8, ValType::I32), (2, ValType::I64)],
        "test_sleep_ms" => vec![],
        "test_sleep_after" => vec![],
        "settled" => vec![(2, ValType::I32)],
        "pass" => vec![(1, ValType::I32)],
        "run_fails" => Vec::new(),
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
    ins.call(rt(b, "wait_forget"));
    interrupted(b, ins, BITS);
    ins.else_();
    // No request: a defect that ended a `sync` call (a failure after resume, spec 8.7) is the
    // result; otherwise the call is unwound as interrupted.
    ins.global_get(G_SYNC_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_SYNC_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(BITS);
    ins.i32_const(0);
    ins.global_set(G_SYNC_DEFECT);
    ins.else_();
    interrupted(b, ins, BITS);
    ins.end();
    ins.end();
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
    ins.call(rt(b, "wait_forget"));
    ins.end();
    ins.local_get(FR);
    ins.call(drop);
    ins.global_get(G_SYNC_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_SYNC_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(BITS);
    ins.i32_const(0);
    ins.global_set(G_SYNC_DEFECT);
    ins.else_();
    interrupted(b, ins, BITS);
    ins.end();
    ins.i32_const(1);
    ins.local_set(MODE);
    ins.br(1);
    ins.end();
    input_taken(ins, OUT, MODE, BITS);
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
pub(super) fn set_input(ins: &mut InstructionSink, mode: u32, bits: u32) {
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
fn input_taken(ins: &mut InstructionSink, out: u32, mode: u32, bits: u32) {
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

// ---- the perform of a `suspend fn` operation ----

/// Pushes `operation `op` of handler `name`` for the perform in local `p`.
fn build_where(b: &mut Builder, ins: &mut InstructionSink, p: u32) {
    let append = rt(b, "str_append");
    let prefix = b.string_literal(b"operation `");
    let mid = b.string_literal(b"` of handler `");
    let suffix = b.string_literal(b"`");
    ins.i32_const(prefix as i32);
    ins.local_get(p);
    ins.i32_load(mem(P_OP, 4));
    ins.call(append);
    ins.i32_const(mid as i32);
    ins.call(append);
    ins.local_get(p);
    ins.i32_load(mem(P_HANDLER, 4));
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_NAME, 4));
    ins.local_get(p);
    ins.i32_load(mem(P_HANDLER, 4));
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
    let rdesc = resume_desc(b);
    ins.i32_const(104);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(P);
    setc(ins, P, P_USED, 0);
    setc(ins, P, P_HAS_NOW, 0);
    setc(ins, P, P_NOW_MODE, 0);
    setc(ins, P, P_CALLER, 0);
    setc(ins, P, P_IMPL_WAIT, 0);
    ins.local_get(P);
    ins.i64_const(0);
    ins.i64_store(mem(P_NOW_BITS, 8));
    ins.local_get(P);
    ins.global_get(G_SYNC_DEPTH);
    ins.i32_const(0);
    ins.i32_ne();
    ins.i32_store(mem(P_SYNC, 4));
    setc(ins, P, P_SYNC_PHASE, 1);
    ins.local_get(P);
    ins.local_get(1);
    ins.i32_store(mem(P_OP, 4));
    ins.local_get(P);
    ins.local_get(0);
    ins.i32_store(mem(P_HANDLER, 4));
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    ins.local_get(P);
    ins.global_get(G_RUN_ID);
    ins.i32_store(mem(P_RUN, 4));
    ins.local_get(P);
    ins.global_get(G_CUR_MAIN);
    ins.i32_store(mem(P_PREV_MAIN, 4));
    ins.i32_const(0);
    ins.global_set(G_CUR_MAIN);
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
    const DEAD: u32 = 4;
    const MODE: u32 = 5;
    const OUT: u32 = 6;
    const BITS: u32 = 7;
    let drop = rt(b, "drop");
    ins.local_get(0);
    ins.i32_load(mem(RES_PERF, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_load(mem(P_PREV_MAIN, 4));
    ins.global_set(G_CUR_MAIN);
    ins.i64_const(0);
    ins.local_set(OUT);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(CHAIN);
    clear_suspension(ins);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.global_get(G_ROOT);
    ins.global_get(G_CUR);
    ins.local_get(W);
    ins.local_get(CHAIN);
    ins.i32_const(END_IMPL);
    ins.local_get(P);
    ins.call(rt(b, "suspended"));
    ins.else_();
    ins.local_get(CHAIN);
    ins.call(drop);
    ins.end();
    ins.else_();
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.i64_extend_i32_u();
    ins.local_set(BITS);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.i32_const(MODE_THROW);
    ins.local_set(MODE);
    ins.local_get(P);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "settled"));
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.i64_extend_i32_u();
    ins.local_set(BITS);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(MODE_FAIL);
    ins.local_set(MODE);
    ins.local_get(P);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "settled"));
    ins.end();
    ins.end();
    ins.end();
    setc(ins, P, P_SYNC_PHASE, 0);
    ins.local_get(0);
    ins.call(drop);
    // dead: a defect already ended the sync call (a `sync` call) or the run is over.
    ins.local_get(P);
    ins.i32_load(mem(P_SYNC, 4));
    ins.if_(BlockType::Result(ValType::I32));
    ins.global_get(G_SYNC_DEFECT);
    ins.i32_const(0);
    ins.i32_ne();
    ins.else_();
    ins.global_get(G_ROOT);
    ins.i32_eqz();
    ins.global_get(G_ROOT);
    ins.i32_load(mem(R_OVER, 4));
    ins.i32_or();
    ins.end();
    ins.local_set(DEAD);
    ins.local_get(DEAD);
    ins.if_(BlockType::Empty);
    setc(ins, P, P_HAS_NOW, 0);
    ins.i32_const(0);
    ins.call(rt(b, "susp_on"));
    ins.else_();
    ins.local_get(P);
    ins.i32_load(mem(P_HAS_NOW, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_NOW_MODE, 4));
    ins.local_set(MODE);
    ins.local_get(P);
    ins.i64_load(mem(P_NOW_BITS, 8));
    ins.local_set(BITS);
    set_input(ins, MODE, BITS);
    // A value result is `MODE_VALUE` or `MODE_VALUE_REF` (the low bits are 0); anything else is a
    // defect or a failure, which carries no value to continue with.
    ins.local_get(MODE);
    ins.i32_const(3);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I64));
    ins.local_get(BITS);
    ins.else_();
    ins.i64_const(0);
    ins.end();
    ins.local_set(OUT);
    setc(ins, P, P_HAS_NOW, 0);
    ins.else_();
    ins.i32_const(WAIT_PERFORM);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    store32(ins, W, W_ON_DROP, P);
    store32(ins, P, P_CALLER, W);
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.end();
    ins.end();
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

/// `settled(p, mode, bits)`: how the implementation ended with a pending defect (mode 1) or
/// failure (mode 2). Before it resumes this goes to the caller; after it ends the run (spec 14.5).
fn settled(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 0;
    const MODE: u32 = 1;
    const BITS: u32 = 2;
    const MSG: u32 = 3;
    const TMP: u32 = 4;
    let append = rt(b, "str_append");
    ins.local_get(MODE);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.call(rt(b, "run_fails"));
    ins.else_();
    ins.local_get(P);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "pass"));
    ins.end();
    ins.else_();
    ins.local_get(MODE);
    ins.i32_const(MODE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    build_where(b, ins, P);
    let tail = b.string_literal(b" failed after resume: ");
    ins.i32_const(tail as i32);
    ins.call(append);
    ins.local_get(BITS);
    ins.i32_wrap_i64();
    ins.local_set(TMP);
    // `show_failure` consumes the failure (C's `em_show_failure(in.value.p)`); do not keep a second
    // reference to it (W3-6).
    ins.local_get(TMP);
    ins.call(rt(b, "show_failure"));
    ins.call(append);
    ins.local_set(MSG);
    ins.local_get(P);
    ins.local_get(MSG);
    ins.call(rt(b, "run_fails"));
    ins.else_();
    ins.local_get(P);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "pass"));
    ins.end();
    ins.else_();
    // A value the caller never consumes (C's `em_input_drop`).
    input_drop(ins, b, MODE, BITS);
    ins.end();
    ins.end();
}

/// `run_fails(p, defect)`: the implementation's run ended with `defect`.
fn run_fails(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 0;
    const D: u32 = 1;
    ins.local_get(P);
    ins.i32_load(mem(P_SYNC, 4));
    ins.if_(BlockType::Empty);
    ins.global_get(G_SYNC_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(D);
    ins.global_set(G_SYNC_DEFECT);
    ins.else_();
    ins.local_get(D);
    ins.call(rt(b, "drop"));
    ins.end();
    ins.else_();
    ins.global_get(G_ROOT);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_RUN, 4));
    ins.global_get(G_RUN_ID);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_ROOT);
    ins.i32_const(MODE_THROW);
    ins.local_get(D);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "root_finish"));
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(D);
    ins.call(rt(b, "drop"));
    ins.end();
}

/// `pass(p, mode, bits)`: passes the implementation's result to the caller (remembered during the
/// call, otherwise by waking the caller's wait).
fn pass(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 0;
    const MODE: u32 = 1;
    const BITS: u32 = 2;
    const W: u32 = 3;
    setc(ins, P, P_USED, 1);
    ins.local_get(P);
    ins.i32_load(mem(P_SYNC_PHASE, 4));
    ins.if_(BlockType::Empty);
    setc(ins, P, P_HAS_NOW, 1);
    store32(ins, P, P_NOW_MODE, MODE);
    ins.local_get(P);
    ins.local_get(BITS);
    ins.i64_store(mem(P_NOW_BITS, 8));
    ins.else_();
    ins.local_get(P);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_set(W);
    setc(ins, P, P_CALLER, 0);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.local_get(MODE);
    ins.local_get(BITS);
    ins.call(rt(b, "cont"));
    ins.else_();
    // No caller will consume it (C's `em_input_drop`).
    input_drop(ins, b, MODE, BITS);
    ins.end();
    ins.end();
}

/// Drops an input `(mode, bits)` nothing will consume (C's `em_input_drop`): a thrown defect, a
/// failure, or a value carrying [`MODE_VALUE_REF`]. A plain value is not a reference to release.
fn input_drop(ins: &mut InstructionSink, b: &Builder, mode: u32, bits: u32) {
    ins.local_get(mode);
    ins.if_(BlockType::Empty);
    ins.local_get(bits);
    ins.i32_wrap_i64();
    ins.call(rt(b, "drop"));
    ins.end();
}

/// Releases a resume value the call will not deliver: a reference the input owned (C's
/// `em_input_drop` in `em_resume_with`).
fn drop_resume_value(b: &mut Builder, ins: &mut InstructionSink, value: Repr, param: u32) {
    if value == Repr::Ref {
        ins.local_get(param);
        ins.call(rt(b, "drop"));
    }
}

/// `resume_with(env, value)`: the implementation resumed the operation.
fn resume_with(b: &mut Builder, ins: &mut InstructionSink, value: Repr, param: u32) {
    const P: u32 = 2;
    const MSG: u32 = 3;
    const BITS: u32 = 5;
    let raise = rt(b, "raise");
    ins.local_get(0);
    ins.i32_load(mem(RES_PERF, 4));
    ins.local_set(P);
    // A resume after the run is over is ignored (spec 14.5), and the value it carried is released
    // (C's `em_input_drop` in `em_resume_with`).
    ins.global_get(G_ROOT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    drop_resume_value(b, ins, value, param);
    ins.return_();
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(P_RUN, 4));
    ins.global_get(G_RUN_ID);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    drop_resume_value(b, ins, value, param);
    ins.return_();
    ins.end();
    ins.global_get(G_ROOT);
    ins.i32_load(mem(R_OVER, 4));
    ins.if_(BlockType::Empty);
    drop_resume_value(b, ins, value, param);
    ins.return_();
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(P_USED, 4));
    ins.if_(BlockType::Empty);
    let twice = b.string_literal(b"resume called twice in ");
    ins.i32_const(twice as i32);
    build_where(b, ins, P);
    ins.call(rt(b, "str_append"));
    ins.local_set(MSG);
    ins.local_get(MSG);
    ins.call(raise);
    drop_resume_value(b, ins, value, param);
    ins.return_();
    ins.end();
    match value {
        Repr::I64 => {
            ins.local_get(param);
            ins.local_set(BITS);
        }
        Repr::F64 => {
            ins.local_get(param);
            ins.i64_reinterpret_f64();
            ins.local_set(BITS);
        }
        Repr::I32 | Repr::Ref => {
            ins.local_get(param);
            ins.i64_extend_i32_u();
            ins.local_set(BITS);
        }
        Repr::Void => {
            ins.i64_const(0);
            ins.local_set(BITS);
        }
    }
    ins.local_get(P);
    // A reference value the input owns must be released if it is never delivered.
    ins.i32_const(if value == Repr::Ref {
        MODE_VALUE_REF
    } else {
        MODE_VALUE
    });
    ins.local_get(BITS);
    ins.call(rt(b, "pass"));
}

/// The body of a resume closure: `(env, arg)` -> `resume_with(env's perform, arg)`.
fn resume_call(b: &mut Builder, ins: &mut InstructionSink, value: Repr, param: u32) {
    resume_with(b, ins, value, param);
}

// ---- fairness, holds and the containers ----

fn hold(ins: &mut InstructionSink) {
    ins.global_get(G_CUR);
    ins.if_(BlockType::Empty);
    ins.global_get(G_CUR);
    ins.global_get(G_CUR);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

fn unhold(ins: &mut InstructionSink) {
    ins.global_get(G_CUR);
    ins.if_(BlockType::Empty);
    ins.global_get(G_CUR);
    ins.global_get(G_CUR);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(F_HOLDS, 4));
    ins.end();
}

/// `yield(perform)`: the fairness budget (spec 11.1); a `perform` yield also delivers a pending
/// interruption (spec 11.4).
fn yield_(b: &mut Builder, ins: &mut InstructionSink) {
    const F: u32 = 1;
    ins.global_get(G_CUR);
    ins.local_set(F);
    ins.local_get(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.global_get(G_SYNC_DEPTH);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(F);
    ins.local_get(F);
    ins.i32_load(mem(F_BUDGET, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(F_BUDGET, 4));
    ins.local_get(F);
    ins.i32_load(mem(F_BUDGET, 4));
    ins.i32_const(0);
    ins.i32_le_s();
    ins.if_(BlockType::Empty);
    ins.global_get(G_CUR_MAIN);
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.call(rt(b, "interrupting"));
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.call(rt(b, "deliver"));
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(F);
    ins.i32_const(BUDGET);
    ins.i32_store(mem(F_BUDGET, 4));
    ins.i32_const(WAIT_PAUSE);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.call(rt(b, "susp_on"));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.if_(BlockType::Empty);
    ins.call(rt(b, "interrupt_here"));
    ins.drop();
    ins.end();
}

fn rt_alloc(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    let desc = rtstruct(b);
    ins.local_get(0);
    ins.i32_const(7);
    ins.i32_add();
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.local_set(N);
    ins.local_get(N);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.i32_const(HDR as i32);
    ins.i32_add();
}

fn vec_new(b: &mut Builder, ins: &mut InstructionSink) {
    const V: u32 = 1;
    ins.i32_const(24);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(V);
    ins.local_get(V);
    ins.local_get(0);
    ins.i32_const(4);
    ins.i32_mul();
    ins.call(rt(b, "rt_alloc"));
    ins.i32_store(mem(V_ITEMS, 4));
    setc(ins, V, V_LEN, 0);
    store32(ins, V, V_CAP, 0);
    ins.local_get(V);
}

fn vec_push(b: &mut Builder, ins: &mut InstructionSink) {
    const T: u32 = 2; // the grown items block
    const CAP: u32 = 3; // the new capacity
    const I: u32 = 4; // a scratch index / the old block's size
    const OLD: u32 = 5; // the old items block
    const SIZE: u32 = 6; // its allocation size
    // Grow when full, doubling (from 4), like the C `em_vec_push`. Without this a vector at its
    // initial capacity silently drops the element (spec 11.2: a scope must keep every child).
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_get(0);
    ins.i32_load(mem(V_CAP, 4));
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(V_CAP, 4));
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(0);
    ins.i32_load(mem(V_CAP, 4));
    ins.i32_const(2);
    ins.i32_mul();
    ins.else_();
    ins.i32_const(4);
    ins.end();
    ins.local_set(CAP);
    ins.local_get(CAP);
    ins.i32_const(4);
    ins.i32_mul();
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(T);
    // Copy the live items over.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(T);
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    vec_item(ins, 0, I);
    ins.i32_store(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Release the old block: `rt_alloc` sized it as ceil(bytes / 8) words plus the header, and its
    // address `V_ITEMS` points just past that header.
    ins.local_get(0);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.i32_const(HDR as i32);
    ins.i32_sub();
    ins.local_set(OLD);
    ins.local_get(0);
    ins.i32_load(mem(V_CAP, 4));
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_const(7);
    ins.i32_add();
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(SIZE);
    free_raw(ins, OLD, SIZE);
    store32(ins, 0, V_ITEMS, T);
    store32(ins, 0, V_CAP, CAP);
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(V_LEN, 4));
}

fn vec_has(ins: &mut InstructionSink) {
    const I: u32 = 2;
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    vec_item(ins, 0, I);
    ins.local_get(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

fn vec_remove(ins: &mut InstructionSink) {
    const I: u32 = 2;
    const K: u32 = 3;
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(K);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    vec_item(ins, 0, I);
    ins.local_get(1);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(K);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    vec_item(ins, 0, I);
    ins.i32_store(mem(0, 4));
    ins.local_get(K);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(K);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    store32(ins, 0, V_LEN, K);
}

fn vec_len(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(V_LEN, 4));
}

fn queue_new(b: &mut Builder, ins: &mut InstructionSink) {
    const Q: u32 = 1;
    ins.i32_const(32);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(Q);
    ins.local_get(Q);
    ins.local_get(0);
    ins.i32_const(4);
    ins.i32_mul();
    ins.call(rt(b, "rt_alloc"));
    ins.i32_store(mem(Q_ITEMS, 4));
    setc(ins, Q, Q_HEAD, 0);
    setc(ins, Q, Q_LEN, 0);
    store32(ins, Q, Q_CAP, 0);
    ins.local_get(Q);
}

fn queue_push(ins: &mut InstructionSink) {
    const IDX: u32 = 2;
    ins.local_get(0);
    ins.i32_load(mem(Q_LEN, 4));
    ins.local_get(0);
    ins.i32_load(mem(Q_CAP, 4));
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(Q_HEAD, 4));
    ins.local_get(0);
    ins.i32_load(mem(Q_LEN, 4));
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(Q_CAP, 4));
    ins.i32_rem_u();
    ins.local_set(IDX);
    ins.local_get(0);
    ins.i32_load(mem(Q_ITEMS, 4));
    ins.local_get(IDX);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(Q_LEN, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(Q_LEN, 4));
    ins.end();
}

fn queue_pop(ins: &mut InstructionSink) {
    const ITEM: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(Q_ITEMS, 4));
    ins.local_get(0);
    ins.i32_load(mem(Q_HEAD, 4));
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(ITEM);
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(Q_HEAD, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(Q_CAP, 4));
    ins.i32_rem_u();
    ins.i32_store(mem(Q_HEAD, 4));
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(Q_LEN, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(Q_LEN, 4));
    ins.local_get(ITEM);
}

fn job_new(b: &mut Builder, ins: &mut InstructionSink) {
    const J: u32 = 1;
    ins.i32_const(56);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(J);
    setc(ins, J, J_W, 0);
    setc(ins, J, J_FIBER, 0);
    setc(ins, J, J_FN, 0);
    setc(ins, J, J_MODE, 0);
    setc(ins, J, J_END, 0);
    ins.local_get(J);
    ins.local_get(0);
    ins.i32_store(mem(J_KIND, 4));
    ins.local_get(J);
    ins.i64_const(0);
    ins.i64_store(mem(J_BITS, 8));
    ins.local_get(J);
}

fn wait_new(b: &mut Builder, ins: &mut InstructionSink) {
    const W: u32 = 2;
    ins.i32_const(88);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(W);
    store32(ins, W, W_KIND, 0);
    store32(ins, W, W_CLOSING, 1);
    setc(ins, W, W_LIVE, 1);
    setc(ins, W, W_CHAIN, 0);
    setc(ins, W, W_FIBER, 0);
    setc(ins, W, W_END, 0);
    setc(ins, W, W_IMPL, 0);
    setc(ins, W, W_ON_DROP, 0);
    setc(ins, W, W_PREV, 0);
    setc(ins, W, W_NEXT, 0);
    ins.local_get(W);
}

fn root_unlist(ins: &mut InstructionSink) {
    ins.local_get(1);
    ins.i32_load(mem(W_PREV, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_PREV, 4));
    ins.local_get(1);
    ins.i32_load(mem(W_NEXT, 4));
    ins.i32_store(mem(W_NEXT, 4));
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_load(mem(W_NEXT, 4));
    ins.i32_store(mem(R_FIRST_WAIT, 4));
    ins.end();
    ins.local_get(1);
    ins.i32_load(mem(W_NEXT, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_NEXT, 4));
    ins.local_get(1);
    ins.i32_load(mem(W_PREV, 4));
    ins.i32_store(mem(W_PREV, 4));
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_load(mem(W_PREV, 4));
    ins.i32_store(mem(R_LAST_WAIT, 4));
    ins.end();
    setc(ins, 1, W_PREV, 0);
    setc(ins, 1, W_NEXT, 0);
}

fn wait_take(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(1);
    ins.i32_load(mem(W_LIVE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    setc(ins, 1, W_LIVE, 0);
    ins.local_get(0);
    ins.local_get(1);
    ins.call(rt(b, "root_unlist"));
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.i32_load(mem(F_WAIT, 4));
    ins.local_get(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.i32_const(0);
    ins.i32_store(mem(F_WAIT, 4));
    ins.end();
    ins.end();
    setc(ins, 1, W_ON_DROP, 0);
    ins.i32_const(1);
}

fn wait_drop(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 2;
    const IW: u32 = 3;
    const J: u32 = 4;
    setc(ins, 1, W_LIVE, 0);
    ins.local_get(0);
    ins.local_get(1);
    ins.call(rt(b, "root_unlist"));
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.i32_load(mem(F_WAIT, 4));
    ins.local_get(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(W_FIBER, 4));
    ins.i32_const(0);
    ins.i32_store(mem(F_WAIT, 4));
    ins.end();
    ins.end();
    ins.local_get(1);
    ins.i32_load(mem(W_ON_DROP, 4));
    ins.local_set(P);
    setc(ins, 1, W_ON_DROP, 0);
    ins.local_get(P);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_IMPL_WAIT, 4));
    ins.local_set(IW);
    ins.local_get(P);
    ins.i32_load(mem(P_USED, 4));
    ins.i32_eqz();
    ins.local_get(IW);
    ins.i32_const(0);
    ins.i32_ne();
    ins.i32_and();
    ins.local_get(IW);
    ins.i32_load(mem(W_LIVE, 4));
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(IW);
    ins.call(rt(b, "wait_drop"));
    ins.i32_const(KIND_DROP_IMPL);
    ins.call(rt(b, "job_new"));
    ins.local_set(J);
    store32(ins, J, J_W, IW);
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.local_get(J);
    ins.call(rt(b, "queue_push"));
    ins.end();
    ins.end();
}

fn wait_forget(ins: &mut InstructionSink) {
    const P: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(W_ON_DROP, 4));
    ins.local_set(P);
    setc(ins, 0, W_ON_DROP, 0);
    ins.local_get(P);
    ins.if_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_load(mem(P_CALLER, 4));
    ins.local_get(0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    setc(ins, P, P_CALLER, 0);
    ins.end();
    ins.end();
}

fn susp_on(ins: &mut InstructionSink) {
    ins.i32_const(1);
    ins.global_set(G_SUSPENDING);
    ins.local_get(0);
    ins.global_set(G_SUSP_WAIT);
    ins.i32_const(0);
    ins.global_set(G_SUSP_HEAD);
    ins.i32_const(0);
    ins.global_set(G_SUSP_TAIL);
}

// ---- fibers and scopes ----

fn fiber_new(b: &mut Builder, ins: &mut InstructionSink) {
    const F: u32 = 2;
    const T: u32 = 3;
    let desc = desc_fiber(b);
    ins.i32_const(136);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(F);
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(T);
    ins.local_get(T);
    ins.local_get(F);
    ins.i32_store(mem(HDR, 4));
    store32(ins, F, F_SELF, T);
    store32(ins, F, F_ROOT, 0);
    store32(ins, F, F_SCOPE, 1);
    setc(ins, F, F_WAIT, 0);
    setc(ins, F, F_UNWINDING, 0);
    setc(ins, F, F_HOLDS, 0);
    setc(ins, F, F_SCOPES, 0);
    setc(ins, F, F_ENDED, 0);
    setc(ins, F, F_LISTED, 1);
    setc(ins, F, F_RES_MODE, 0);
    setc(ins, F, F_RES_REF, 0);
    setc(ins, F, F_CAUSE, 0);
    setc(ins, F, F_BUDGET, BUDGET);
    ins.local_get(F);
    ins.i64_const(0);
    ins.i64_store(mem(F_RES_VAL, 8));
    ins.i32_const(64);
    ins.call(rt(b, "vec_new"));
    ins.local_set(T);
    store32(ins, F, F_PENDING, T);
    ins.i32_const(64);
    ins.call(rt(b, "vec_new"));
    ins.local_set(T);
    store32(ins, F, F_UNWINDS, T);
    ins.i32_const(256);
    ins.call(rt(b, "vec_new"));
    ins.local_set(T);
    store32(ins, F, F_WATCHERS, T);
    ins.local_get(0);
    ins.i32_load(mem(R_FIBERS, 4));
    ins.local_get(F);
    ins.call(rt(b, "vec_push"));
    ins.local_get(F);
}

fn fiber_finish(b: &mut Builder, ins: &mut InstructionSink) {
    const WS: u32 = 4;
    const I: u32 = 5;
    const CAUSE: u32 = 6;
    ins.local_get(0);
    ins.i32_load(mem(F_ENDED, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    setc(ins, 0, F_ENDED, 1);
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "cause_of"));
    ins.local_set(CAUSE);
    store32(ins, 0, F_CAUSE, CAUSE);
    setc(ins, 0, F_WAIT, 0);
    ins.local_get(0);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    setc(ins, 0, F_RES_MODE, 0);
    setc(ins, 0, F_RES_REF, 0);
    ins.local_get(0);
    ins.i64_const(0);
    ins.i64_store(mem(F_RES_VAL, 8));
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "root_ended"));
    ins.else_();
    store32(ins, 0, F_RES_MODE, 1);
    store32(ins, 0, F_RES_REF, 3);
    ins.local_get(0);
    ins.local_get(2);
    ins.i64_store(mem(F_RES_VAL, 8));
    ins.local_get(0);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.local_get(0);
    ins.call(rt(b, "scope_child_ended"));
    ins.local_get(0);
    ins.i32_load(mem(F_WATCHERS, 4));
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
    ins.local_get(0);
    vec_item(ins, WS, I);
    ins.call(rt(b, "watch_deliver"));
    ins.br(0);
    ins.end();
    ins.end();
    setc(ins, WS, V_LEN, 0);
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_LISTED, 4));
    ins.if_(BlockType::Empty);
    setc(ins, 0, F_LISTED, 0);
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.i32_load(mem(R_FIBERS, 4));
    ins.local_get(0);
    ins.call(rt(b, "vec_remove"));
    ins.end();
}

fn scope_child_ended(b: &mut Builder, ins: &mut InstructionSink) {
    const CAUSE: u32 = 2;
    const W: u32 = 3;
    ins.local_get(0);
    ins.i32_load(mem(S_CHILDREN, 4));
    ins.local_get(1);
    ins.call(rt(b, "vec_remove"));
    ins.local_get(1);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.local_set(CAUSE);
    ins.local_get(CAUSE);
    ins.if_(BlockType::Empty);
    ins.local_get(CAUSE);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_INTERRUPT);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    dup_push(ins, b, CAUSE);
    ins.call(rt(b, "scope_failed"));
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(S_CHILDREN, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(S_CLOSER, 4));
    ins.local_set(W);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    setc(ins, 0, S_CLOSER, 0);
    ins.local_get(W);
    ins.i32_const(MODE_VALUE);
    ins.i64_const(0);
    ins.call(rt(b, "cont"));
    ins.end();
    ins.end();
}

fn scope_failed(b: &mut Builder, ins: &mut InstructionSink) {
    const CH: u32 = 2;
    const I: u32 = 3;
    ins.local_get(0);
    ins.i32_load(mem(S_FIRST, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(S_FIRST, 4));
    ins.local_get(1);
    ins.call(rt(b, "cause_note"));
    // The cause is taken by the call and released after the note (C's `em_scope_failed`).
    ins.local_get(1);
    ins.call(rt(b, "drop"));
    ins.return_();
    ins.end();
    store32(ins, 0, S_FIRST, 1);
    setc(ins, 0, S_CLOSED, 1);
    ins.local_get(0);
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
    vec_item(ins, CH, I);
    ins.i32_const(0);
    ins.call(rt(b, "interrupt_fiber"));
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(S_BODY_DONE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(S_FIBER, 4));
    ins.local_get(0);
    ins.call(rt(b, "interrupt_fiber"));
    ins.end();
}

fn watch_add(b: &mut Builder, ins: &mut InstructionSink) {
    const WT: u32 = 6;
    ins.i32_const(40);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(WT);
    store32(ins, WT, WAT_W, 1);
    store32(ins, WT, WAT_VERB, 2);
    store32(ins, WT, WAT_REPR, 3);
    store32(ins, WT, WAT_D0, 4);
    store32(ins, WT, WAT_D1, 5);
    ins.local_get(0);
    ins.local_get(WT);
    ins.call(rt(b, "vec_push"));
}

fn watch_deliver(b: &mut Builder, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const VERB: u32 = 3;
    const REPR: u32 = 4;
    const TMP: u32 = 5;
    const V: u32 = 8;
    ins.local_get(1);
    ins.i32_load(mem(WAT_W, 4));
    ins.local_set(W);
    ins.local_get(1);
    ins.i32_load(mem(WAT_VERB, 4));
    ins.local_set(VERB);
    ins.local_get(1);
    ins.i32_load(mem(WAT_REPR, 4));
    ins.local_set(REPR);
    ins.local_get(VERB);
    ins.i32_const(VERB_JOIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i64_load(mem(F_RES_VAL, 8));
    ins.i32_wrap_i64();
    ins.local_set(TMP);
    dup_push(ins, b, TMP);
    ins.i64_extend_i32_u();
    ins.local_get(REPR);
    ins.call(rt(b, "unbox"));
    ins.local_set(V);
    ins.local_get(W);
    // The unboxed value owns a reference exactly when its repr is a reference.
    ins.local_get(REPR);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(MODE_VALUE_REF);
    ins.else_();
    ins.i32_const(MODE_VALUE);
    ins.end();
    ins.local_get(V);
    ins.call(rt(b, "cont"));
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_const(MODE_THROW);
    let s = b.string_literal(b"fiber was interrupted");
    ins.i32_const(s as i32);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cont"));
    ins.else_();
    ins.local_get(W);
    ins.i32_const(MODE_THROW);
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cont"));
    ins.end();
    ins.end();
    ins.else_();
    ins.local_get(VERB);
    ins.i32_const(VERB_AWAIT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // Success(value) or Failure(cause)
    ins.local_get(0);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i64_load(mem(F_RES_VAL, 8));
    ins.i32_wrap_i64();
    ins.local_set(TMP);
    dup_push(ins, b, TMP);
    ins.i64_extend_i32_u();
    ins.local_get(REPR);
    ins.call(rt(b, "unbox"));
    ins.local_set(V);
    ins.i32_const(1);
    super::fiber::success_desc(b, ins, REPR);
    ins.call(rt(b, "alloc"));
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.local_get(V);
    ins.i64_store(mem(HDR, 8));
    ins.local_get(W);
    // The Exit object is a reference the input owns.
    ins.i32_const(MODE_VALUE_REF);
    ins.local_get(TMP);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cont"));
    ins.else_();
    ins.i32_const(1);
    ins.i32_const(super::fiber::failure_desc(b) as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.local_get(0);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.i32_store(mem(HDR, 4));
    ins.local_get(0);
    ins.i32_load(mem(F_CAUSE, 4));
    ins.call(rt(b, "dup"));
    ins.local_get(W);
    // The Failure(Exit) object is a reference the input owns.
    ins.i32_const(MODE_VALUE_REF);
    ins.local_get(TMP);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cont"));
    ins.end();
    ins.else_();
    ins.local_get(W);
    ins.i32_const(MODE_VALUE);
    ins.i64_const(0);
    ins.call(rt(b, "cont"));
    ins.end();
    ins.end();
}

fn unbox(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 2;
    ins.local_get(1);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.return_();
    ins.end();
    ins.local_get(1);
    ins.i32_const(VOID_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_wrap_i64();
    ins.call(rt(b, "drop"));
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_wrap_i64();
    ins.local_set(P);
    ins.local_get(P);
    ins.i64_load(mem(HDR, 8));
    ins.local_get(P);
    ins.call(rt(b, "drop"));
}

// ---- causes and notes ----

fn cause_new(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 2;
    let desc = desc_cause(b);
    ins.i32_const(2);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_set(C);
    store32(ins, C, CR_KIND, 0);
    ins.local_get(C);
    ins.local_get(1);
    ins.i64_store(mem(CR_VALUE, 8));
    ins.local_get(C);
}

fn cause_of(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(MODE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.local_set(2);
    ins.i32_const(CAUSE_FAIL);
    dup_push(ins, b, 2);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cause_new"));
    ins.else_();
    ins.local_get(0);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.call(rt(b, "is_interrupted"));
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(CAUSE_INTERRUPT);
    ins.i64_const(0);
    ins.call(rt(b, "cause_new"));
    ins.else_();
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.local_set(2);
    ins.i32_const(CAUSE_DIE);
    dup_push(ins, b, 2);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "cause_new"));
    ins.end();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.end();
}

fn cause_kind(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
}

fn cause_to_string(b: &mut Builder, ins: &mut InstructionSink) {
    const KIND: u32 = 1;
    const S: u32 = 2;
    const SHOWN: u32 = 3;
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
    ins.local_set(KIND);
    ins.local_get(KIND);
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    let prefix = b.string_literal(b"error: ");
    ins.i32_const(prefix as i32);
    ins.local_set(S);
    ins.local_get(0);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.local_set(SHOWN);
    dup_push(ins, b, SHOWN);
    ins.call(rt(b, "show_failure"));
    ins.local_set(SHOWN);
    ins.local_get(SHOWN);
    ins.if_(BlockType::Empty);
    ins.local_get(S);
    ins.local_get(SHOWN);
    ins.call(rt(b, "str_append"));
    ins.local_set(S);
    ins.end();
    ins.local_get(S);
    ins.else_();
    ins.local_get(KIND);
    ins.i32_const(CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    let dp = b.string_literal(b"defect: ");
    ins.i32_const(dp as i32);
    ins.local_get(0);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.call(rt(b, "str_append"));
    ins.else_();
    let ip = b.string_literal(b"interrupted");
    ins.i32_const(ip as i32);
    ins.end();
    ins.end();
}

fn cause_raise(b: &mut Builder, ins: &mut InstructionSink) {
    const V: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.local_set(V);
    dup_push(ins, b, V);
    ins.global_set(G_FAILURE);
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.local_set(V);
    dup_push(ins, b, V);
    ins.call(rt(b, "raise"));
    ins.else_();
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.call(rt(b, "raise"));
    ins.end();
    ins.end();
}

fn cause_note(b: &mut Builder, ins: &mut InstructionSink) {
    const SHOWN: u32 = 2;
    const NOTE: u32 = 3;
    const OLD: u32 = 4;
    const TMP: u32 = 5;
    ins.local_get(1);
    ins.call(rt(b, "cause_to_string"));
    ins.local_set(SHOWN);
    let pre = b.string_literal(b"note: another fiber also failed: ");
    ins.i32_const(pre as i32);
    ins.local_get(SHOWN);
    ins.call(rt(b, "str_append"));
    ins.local_set(NOTE);
    // The shown cause has been copied into the note (C's `em_drop(shown)`).
    ins.local_get(SHOWN);
    ins.call(rt(b, "drop"));
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const(CR_VALUE as i32);
    ins.i32_add();
    ins.local_get(NOTE);
    ins.call(rt(b, "notes_add"));
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(CR_KIND, 4));
    ins.i32_const(CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // `d + "\n" + note`, releasing the old defect and the intermediate (C's `em_buf` copies).
    ins.local_get(0);
    ins.i32_load(mem(CR_VALUE, 4));
    ins.local_set(OLD);
    let nl = b.string_literal(b"\n");
    ins.local_get(OLD);
    ins.i32_const(nl as i32);
    ins.call(rt(b, "str_append"));
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.local_get(NOTE);
    ins.call(rt(b, "str_append"));
    ins.local_set(SHOWN);
    store32(ins, 0, CR_VALUE, SHOWN);
    ins.local_get(OLD);
    ins.call(rt(b, "drop"));
    ins.local_get(TMP);
    ins.call(rt(b, "drop"));
    ins.end();
    ins.end();
    // The note has been copied (or referenced) by `notes_add`/the defect (C's `em_drop(note)`).
    ins.local_get(NOTE);
    ins.call(rt(b, "drop"));
}

fn notes_add(b: &mut Builder, ins: &mut InstructionSink) {
    const E: u32 = 2;
    const D: u32 = 3;
    const N: u32 = 4;
    const I: u32 = 5;
    const V: u32 = 6;
    const COPY: u32 = 7;
    const OLD: u32 = 8;
    const NEW: u32 = 9;
    const X: u32 = 10;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.local_get(E);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    // Copy a static error first.
    ins.local_get(E);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(E);
    ins.i32_load(mem(8, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(DESC_NSLOTS, 4));
    ins.local_set(N);
    ins.local_get(N);
    ins.local_get(D);
    ins.call(rt(b, "alloc"));
    ins.local_set(COPY);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(E);
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR, 4));
    ins.local_set(V);
    ins.local_get(COPY);
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(V);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(D);
    ins.i32_load(mem(DESC_REFS, 4));
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.local_get(V);
    ins.call(rt(b, "dup"));
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(COPY);
    ins.i32_store(mem(0, 4));
    ins.local_get(COPY);
    ins.local_set(E);
    ins.end();
    // Search the table.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.global_get(G_NOTE_ERRORS);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.global_get(G_NOTE_ERRORS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_get(E);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    let nl = b.string_literal(b"\n");
    ins.global_get(G_NOTE_LINES);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(OLD);
    ins.local_get(OLD);
    ins.i32_const(nl as i32);
    ins.call(rt(b, "str_append"));
    ins.local_set(X);
    ins.local_get(X);
    ins.local_get(1);
    ins.call(rt(b, "str_append"));
    ins.local_set(NEW);
    ins.global_get(G_NOTE_LINES);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(NEW);
    ins.i32_store(mem(0, 4));
    // The replaced line and the intermediate are released (C's `em_drop(old)`; C builds the line
    // in a buffer).
    ins.local_get(OLD);
    ins.call(rt(b, "drop"));
    ins.local_get(X);
    ins.call(rt(b, "drop"));
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(E);
    ins.call(rt(b, "dup"));
    ins.global_get(G_NOTE_ERRORS);
    ins.local_get(E);
    ins.call(rt(b, "vec_push"));
    ins.local_get(1);
    ins.call(rt(b, "dup"));
    ins.global_get(G_NOTE_LINES);
    ins.local_get(1);
    ins.call(rt(b, "vec_push"));
}

fn notes_of(ins: &mut InstructionSink) {
    const I: u32 = 1;
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.global_get(G_NOTE_ERRORS);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.global_get(G_NOTE_ERRORS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_get(0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.global_get(G_NOTE_LINES);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// Reserved: the C `em_notes_clear` resets the notes table, but the WASM `run` builds fresh
/// `G_NOTE_ERRORS`/`G_NOTE_LINES` vectors each time it starts, so nothing calls this yet. The
/// function is registered (its index is part of the fragment), so it is kept (W5-7).
fn notes_clear(ins: &mut InstructionSink) {
    ins.global_get(G_NOTE_ERRORS);
    ins.if_(BlockType::Empty);
    ins.global_get(G_NOTE_ERRORS);
    ins.i32_const(0);
    ins.i32_store(mem(V_LEN, 4));
    ins.global_get(G_NOTE_LINES);
    ins.i32_const(0);
    ins.i32_store(mem(V_LEN, 4));
    ins.end();
}

// ---- interruptions ----

fn is_unwinding(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDING, 4));
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(1);
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.end();
}

fn interrupting(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_eqz();
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDING, 4));
    ins.i32_or();
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.i32_or();
    ins.i32_and();
}

fn deliver(b: &mut Builder, ins: &mut InstructionSink) {
    const TARGET: u32 = 1;
    const I: u32 = 2;
    const N: u32 = 3;
    const S: u32 = 4;
    const PLEN: u32 = 5;
    ins.i32_const(0);
    ins.local_set(TARGET);
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDING, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_const(0);
    ins.call(rt(b, "vec_has"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(PLEN);
    ins.local_get(PLEN);
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_add();
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_get(PLEN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(S);
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.local_get(PLEN);
    ins.i32_sub();
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(S);
    ins.end();
    ins.local_get(TARGET);
    ins.i32_eqz();
    ins.local_get(S);
    ins.i32_load(mem(S_DEPTH, 4));
    ins.local_get(TARGET);
    ins.i32_load(mem(S_DEPTH, 4));
    ins.i32_lt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(S);
    ins.local_set(TARGET);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(TARGET);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    setc(ins, 0, F_UNWINDING, 1);
    setc(ins, 0, F_PENDING, 0);
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_const(0);
    ins.i32_store(mem(V_LEN, 4));
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.i32_const(0);
    ins.i32_store(mem(V_LEN, 4));
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.local_get(TARGET);
    ins.call(rt(b, "vec_remove"));
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.local_get(TARGET);
    ins.call(rt(b, "vec_has"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.local_get(TARGET);
    ins.call(rt(b, "vec_push"));
    ins.end();
    ins.local_get(TARGET);
    ins.i32_load(mem(S_SELF, 4));
    ins.call(rt(b, "interrupted_new"));
}

fn interrupt_fiber(b: &mut Builder, ins: &mut InstructionSink) {
    const W: u32 = 2;
    const J: u32 = 3;
    ins.local_get(0);
    ins.i32_load(mem(F_ENDED, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDING, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(1);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_const(0);
    ins.call(rt(b, "vec_has"));
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.local_get(1);
    ins.call(rt(b, "vec_has"));
    ins.local_get(0);
    ins.i32_load(mem(F_UNWINDS, 4));
    ins.local_get(1);
    ins.call(rt(b, "vec_has"));
    ins.i32_or();
    ins.end();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_PENDING, 4));
    ins.local_get(1);
    ins.call(rt(b, "vec_push"));
    ins.local_get(0);
    ins.i32_load(mem(F_HOLDS, 4));
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_WAIT, 4));
    ins.local_set(W);
    ins.local_get(W);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(W);
    ins.i32_load(mem(W_LIVE, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_get(W);
    ins.call(rt(b, "wait_drop"));
    ins.i32_const(KIND_INTERRUPT);
    ins.call(rt(b, "job_new"));
    ins.local_set(J);
    store32(ins, J, J_W, W);
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_get(J);
    ins.call(rt(b, "schedule"));
}

fn interrupt_here(b: &mut Builder, ins: &mut InstructionSink) {
    ins.global_get(G_CUR);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_CUR_MAIN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_SYNC_DEPTH);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_CUR);
    ins.call(rt(b, "interrupting"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_CUR);
    ins.call(rt(b, "deliver"));
    ins.global_set(G_DEFECT);
    ins.i32_const(1);
}

// ---- the driver loop (spec 14.5) ----

fn fiber_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 6;
    ins.local_get(2);
    ins.i32_const(END_MAIN);
    ins.i32_eq();
    ins.local_get(4);
    ins.i32_const(MODE_THROW);
    ins.i32_ne();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "interrupting"));
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "deliver"));
    ins.local_set(D);
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.local_get(3);
    ins.i32_const(MODE_THROW);
    ins.local_get(D);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "step"));
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.local_get(3);
    ins.local_get(4);
    ins.local_get(5);
    ins.call(rt(b, "step"));
}

fn step(b: &mut Builder, ins: &mut InstructionSink) {
    const PREV: u32 = 7;
    const PM: u32 = 8;
    ins.global_get(G_CUR);
    ins.local_set(PREV);
    ins.global_get(G_CUR_MAIN);
    ins.local_set(PM);
    ins.local_get(1);
    ins.global_set(G_CUR);
    ins.local_get(3);
    ins.i32_const(END_MAIN);
    ins.i32_eq();
    ins.global_set(G_CUR_MAIN);
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.local_get(3);
    ins.local_get(4);
    ins.local_get(5);
    ins.local_get(6);
    ins.call(rt(b, "advance"));
    ins.local_get(PREV);
    ins.global_set(G_CUR);
    ins.local_get(PM);
    ins.global_set(G_CUR_MAIN);
}

fn advance(b: &mut Builder, ins: &mut InstructionSink) {
    const FR: u32 = 7;
    const RT: u32 = 8;
    const TAIL: u32 = 9;
    const W: u32 = 10;
    const OUT: u32 = 11;
    let drop = rt(b, "drop");
    let resume_ty = b.ty(vec![ValType::I32], vec![ValType::I64]);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_OVER, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.call(drop);
    // Nothing will consume the input (C's `em_input_drop`).
    input_drop(ins, b, 5, 6);
    ins.return_();
    ins.end();
    ins.local_get(2);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(3);
    ins.local_get(4);
    ins.local_get(5);
    ins.local_get(6);
    ins.call(rt(b, "strand_end"));
    ins.return_();
    ins.end();
    ins.local_get(2);
    ins.local_set(FR);
    ins.local_get(FR);
    ins.i32_load(mem(FRAME_NEXT, 4));
    ins.local_set(2);
    ins.local_get(FR);
    ins.i32_const(0);
    ins.i32_store(mem(FRAME_NEXT, 4));
    set_input(ins, 5, 6);
    ins.local_get(FR);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(RT);
    ins.local_get(FR);
    ins.local_get(RT);
    ins.call_indirect(0, resume_ty);
    ins.local_set(OUT);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.global_get(G_SUSP_TAIL);
    ins.local_set(TAIL);
    ins.local_get(TAIL);
    ins.if_(BlockType::Empty);
    ins.local_get(TAIL);
    ins.local_get(2);
    ins.i32_store(mem(FRAME_NEXT, 4));
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(2);
    ins.end();
    clear_suspension(ins);
    ins.local_get(W);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.call(drop);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(W);
    ins.local_get(2);
    ins.local_get(3);
    ins.local_get(4);
    ins.call(rt(b, "suspended"));
    ins.return_();
    ins.end();
    input_taken(ins, OUT, 5, 6);
    ins.br(0);
    ins.end();
    ins.end();
}

fn strand_end(b: &mut Builder, ins: &mut InstructionSink) {
    const REF: u32 = 6;
    ins.local_get(2);
    ins.i32_const(END_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // Whether the main strand's value owns a reference (C's `in.ref`). `fiber_finish` stores it
    // in `F_RES_REF`, which is not read back yet (W5-7).
    ins.i32_const(0);
    ins.local_set(REF);
    // `mode` is `MODE_VALUE` or `MODE_VALUE_REF` for a value (the low bits are 0).
    ins.local_get(4);
    ins.i32_const(3);
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(F_SCOPE, 4));
    ins.i32_const(0);
    ins.i32_ne();
    ins.local_get(0);
    ins.i32_load(mem(R_REPR, 4));
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.i32_or();
    ins.local_set(REF);
    ins.end();
    ins.local_get(1);
    ins.local_get(4);
    ins.local_get(5);
    ins.local_get(REF);
    ins.call(rt(b, "fiber_finish"));
    ins.else_();
    ins.local_get(2);
    ins.i32_const(END_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.local_get(4);
    ins.local_get(5);
    ins.call(rt(b, "settled"));
    ins.else_();
    ins.local_get(4);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.local_get(5);
    ins.i32_wrap_i64();
    ins.call(rt(b, "is_interrupted"));
    ins.i32_eqz();
    ins.i32_and();
    ins.local_get(0);
    ins.i32_load(mem(R_ORPHAN, 4));
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(5);
    ins.i32_wrap_i64();
    ins.i32_store(mem(R_ORPHAN, 4));
    ins.else_();
    // The run keeps no orphan for it: release the input (C's `em_strand_end` default).
    input_drop(ins, b, 4, 5);
    ins.end();
    ins.end();
    ins.end();
}

fn suspended(b: &mut Builder, ins: &mut InstructionSink) {
    const J: u32 = 6;
    store32(ins, 2, W_CHAIN, 3);
    store32(ins, 2, W_FIBER, 1);
    store32(ins, 2, W_END, 4);
    store32(ins, 2, W_IMPL, 5);
    setc(ins, 2, W_LIVE, 1);
    ins.local_get(0);
    ins.i32_load(mem(R_LAST_WAIT, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_LAST_WAIT, 4));
    ins.local_get(2);
    ins.i32_store(mem(W_NEXT, 4));
    ins.else_();
    ins.local_get(0);
    ins.local_get(2);
    ins.i32_store(mem(R_FIRST_WAIT, 4));
    ins.end();
    ins.local_get(2);
    ins.local_get(0);
    ins.i32_load(mem(R_LAST_WAIT, 4));
    ins.i32_store(mem(W_PREV, 4));
    setc(ins, 2, W_NEXT, 0);
    ins.local_get(0);
    ins.local_get(2);
    ins.i32_store(mem(R_LAST_WAIT, 4));
    ins.local_get(4);
    ins.i32_const(END_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    store32(ins, 1, F_WAIT, 2);
    ins.else_();
    ins.local_get(4);
    ins.i32_const(END_IMPL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    store32(ins, 5, P_IMPL_WAIT, 2);
    ins.end();
    ins.end();
    ins.local_get(2);
    ins.i32_load(mem(W_KIND, 4));
    ins.i32_const(WAIT_PAUSE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(R_PAUSES, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(R_PAUSES, 4));
    ins.i32_const(KIND_GO);
    ins.call(rt(b, "job_new"));
    ins.local_set(J);
    store32(ins, J, J_W, 2);
    ins.local_get(0);
    ins.i32_load(mem(R_PAUSES, 4));
    ins.i32_const(HOST_EVERY);
    ins.i32_rem_u();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_LATER, 4));
    ins.local_get(J);
    ins.call(rt(b, "queue_push"));
    ins.else_();
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.local_get(J);
    ins.call(rt(b, "queue_push"));
    ins.end();
    ins.end();
}

fn cont(b: &mut Builder, ins: &mut InstructionSink) {
    const F: u32 = 3;
    const R: u32 = 4;
    const CHAIN: u32 = 5;
    const IMPL: u32 = 6;
    const J: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(W_FIBER, 4));
    ins.local_set(F);
    ins.local_get(F);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(F);
    ins.i32_load(mem(F_ROOT, 4));
    ins.local_set(R);
    ins.local_get(R);
    ins.local_get(0);
    ins.call(rt(b, "wait_take"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.global_get(G_CUR);
    ins.local_get(F);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(W_CHAIN, 4));
    ins.local_set(CHAIN);
    ins.local_get(0);
    ins.i32_load(mem(W_IMPL, 4));
    ins.local_set(IMPL);
    setc(ins, 0, W_CHAIN, 0);
    setc(ins, 0, W_IMPL, 0);
    ins.local_get(R);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(0);
    ins.i32_load(mem(W_END, 4));
    ins.local_get(IMPL);
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "step"));
    ins.else_();
    ins.i32_const(KIND_RESUME);
    ins.call(rt(b, "job_new"));
    ins.local_set(J);
    store32(ins, J, J_W, 0);
    store32(ins, J, J_MODE, 1);
    ins.local_get(J);
    ins.local_get(2);
    ins.i64_store(mem(J_BITS, 8));
    ins.local_get(R);
    ins.local_get(J);
    ins.call(rt(b, "schedule"));
    ins.end();
}

fn run_job(b: &mut Builder, ins: &mut InstructionSink) {
    const KIND: u32 = 2;
    const W: u32 = 3;
    const F: u32 = 4;
    const CHAIN: u32 = 5;
    const IMPL: u32 = 6;
    const END: u32 = 7;
    ins.local_get(1);
    ins.i32_load(mem(J_KIND, 4));
    ins.local_set(KIND);
    ins.local_get(1);
    ins.i32_load(mem(J_W, 4));
    ins.local_set(W);
    ins.local_get(KIND);
    ins.i32_const(KIND_ENTRY);
    ins.i32_eq();
    ins.local_get(KIND);
    ins.i32_const(KIND_BEGIN);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_load(mem(J_FIBER, 4));
    ins.local_get(1);
    ins.i32_load(mem(J_FN, 4));
    ins.call(rt(b, "entry_begin"));
    ins.return_();
    ins.end();
    ins.local_get(KIND);
    ins.i32_const(KIND_GO);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(W);
    ins.call(rt(b, "wait_take"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(W);
    ins.i32_load(mem(W_FIBER, 4));
    ins.local_set(F);
    ins.local_get(W);
    ins.i32_load(mem(W_CHAIN, 4));
    ins.local_set(CHAIN);
    ins.local_get(W);
    ins.i32_load(mem(W_IMPL, 4));
    ins.local_set(IMPL);
    ins.local_get(W);
    ins.i32_load(mem(W_END, 4));
    ins.local_set(END);
    setc(ins, W, W_CHAIN, 0);
    setc(ins, W, W_IMPL, 0);
    ins.local_get(KIND);
    ins.i32_const(KIND_RESUME);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(END);
    ins.local_get(IMPL);
    ins.local_get(1);
    ins.i32_load(mem(J_MODE, 4));
    ins.local_get(1);
    ins.i64_load(mem(J_BITS, 8));
    ins.call(rt(b, "fiber_resume"));
    ins.return_();
    ins.end();
    ins.local_get(KIND);
    ins.i32_const(KIND_GO);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(END);
    ins.local_get(IMPL);
    ins.i32_const(MODE_VALUE);
    ins.i64_const(0);
    ins.call(rt(b, "fiber_resume"));
    ins.return_();
    ins.end();
    ins.local_get(KIND);
    ins.i32_const(KIND_INTERRUPT);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(END);
    ins.local_get(IMPL);
    ins.i32_const(MODE_THROW);
    ins.local_get(F);
    ins.call(rt(b, "deliver"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "step"));
    ins.return_();
    ins.end();
    ins.local_get(KIND);
    ins.i32_const(KIND_ROOT_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_load(mem(J_END, 4));
    ins.i32_const(END_MAIN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(END);
    ins.local_get(IMPL);
    ins.i32_const(MODE_THROW);
    ins.local_get(F);
    ins.call(rt(b, "deliver"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "step"));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.i32_const(END_ORPHAN);
    ins.i32_const(0);
    ins.i32_const(MODE_THROW);
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "step"));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(F);
    ins.local_get(CHAIN);
    ins.local_get(END);
    ins.local_get(IMPL);
    ins.i32_const(MODE_THROW);
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.i64_extend_i32_u();
    ins.call(rt(b, "step"));
}

fn drain(b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, R_RUNNING, 1);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_OVER, 4));
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.i32_load(mem(Q_LEN, 4));
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.call(rt(b, "queue_pop"));
    ins.call(rt(b, "run_job"));
    ins.br(0);
    ins.end();
    ins.end();
    setc(ins, 0, R_RUNNING, 0);
}

fn schedule(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.local_get(1);
    ins.call(rt(b, "queue_push"));
    ins.local_get(0);
    ins.i32_load(mem(R_RUNNING, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "drain"));
    ins.end();
}

fn entry_begin(b: &mut Builder, ins: &mut InstructionSink) {
    const PREV: u32 = 3;
    const PM: u32 = 4;
    const W: u32 = 5;
    const CHAIN: u32 = 6;
    const OUT: u32 = 7;
    const BITS: u32 = 8;
    ins.local_get(1);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_load(mem(V_LEN, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_const(MODE_THROW);
    ins.i32_const(0);
    ins.call(rt(b, "interrupted_new"));
    ins.i64_extend_i32_u();
    ins.i32_const(0);
    ins.call(rt(b, "fiber_finish"));
    ins.return_();
    ins.end();
    ins.global_get(G_CUR);
    ins.local_set(PREV);
    ins.global_get(G_CUR_MAIN);
    ins.local_set(PM);
    ins.local_get(1);
    ins.global_set(G_CUR);
    ins.i32_const(1);
    ins.global_set(G_CUR_MAIN);
    ins.local_get(2);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I64));
    ins.local_get(0);
    ins.i32_load(mem(R_ENTRY, 4));
    let entry_ty = b.ty(vec![], vec![ValType::I64]);
    ins.call_indirect(0, entry_ty);
    ins.else_();
    ins.local_get(2);
    ins.local_get(2);
    ins.i32_load(mem(HDR, 4));
    let child_ty = b.ty(vec![ValType::I32], vec![ValType::I32]);
    ins.call_indirect(0, child_ty);
    ins.i64_extend_i32_u();
    ins.end();
    ins.local_set(OUT);
    ins.local_get(PREV);
    ins.global_set(G_CUR);
    ins.local_get(PM);
    ins.global_set(G_CUR_MAIN);
    ins.global_get(G_SUSPENDING);
    ins.if_(BlockType::Empty);
    ins.global_get(G_SUSP_WAIT);
    ins.local_set(W);
    ins.global_get(G_SUSP_HEAD);
    ins.local_set(CHAIN);
    clear_suspension(ins);
    ins.local_get(W);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(CHAIN);
    ins.call(rt(b, "drop"));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(W);
    ins.local_get(CHAIN);
    ins.i32_const(END_MAIN);
    ins.i32_const(0);
    ins.call(rt(b, "suspended"));
    ins.return_();
    ins.end();
    input_taken(ins, OUT, W, BITS);
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_const(END_MAIN);
    ins.i32_const(0);
    ins.local_get(W);
    ins.local_get(BITS);
    ins.call(rt(b, "strand_end"));
}

fn root_finish(b: &mut Builder, ins: &mut InstructionSink) {
    const W: u32 = 3;
    const N: u32 = 4;
    const F: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(R_OVER, 4));
    ins.if_(BlockType::Empty);
    // The run is already over: release the input (C's `em_input_drop`). A value result is a
    // reference exactly when the run's `repr` is one.
    ins.local_get(1);
    ins.i32_eqz();
    ins.local_get(0);
    ins.i32_load(mem(R_REPR, 4));
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.call(rt(b, "drop"));
    ins.end();
    input_drop(ins, b, 1, 2);
    ins.return_();
    ins.end();
    setc(ins, 0, R_OVER, 1);
    ins.local_get(0);
    ins.i32_load(mem(R_FIRST_WAIT, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(W);
    ins.i32_load(mem(W_NEXT, 4));
    ins.local_set(N);
    setc(ins, W, W_LIVE, 0);
    ins.local_get(0);
    ins.local_get(W);
    ins.call(rt(b, "root_unlist"));
    ins.local_get(W);
    ins.i32_load(mem(W_FIBER, 4));
    ins.local_set(F);
    ins.local_get(F);
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.i32_load(mem(F_WAIT, 4));
    ins.local_get(W);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    setc(ins, F, F_WAIT, 0);
    ins.end();
    ins.end();
    ins.local_get(N);
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    store32(ins, 0, R_RES_MODE, 1);
    setc(ins, 0, R_RES_REF, 0);
    ins.local_get(0);
    ins.local_get(2);
    ins.i64_store(mem(R_RES_VAL, 8));
    setc(ins, 0, R_HAS_RESULT, 1);
}

fn root_ended(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 3;
    ins.local_get(1);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.call(rt(b, "is_interrupted"));
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_ORPHAN, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(R_ORPHAN, 4));
    ins.local_set(D);
    setc(ins, 0, R_ORPHAN, 0);
    // The interruption the root ended with is replaced by the orphan and released (C's
    // `em_root_ended`).
    input_drop(ins, b, 1, 2);
    ins.local_get(0);
    ins.i32_const(MODE_THROW);
    ins.local_get(D);
    ins.i64_extend_i32_u();
    ins.call(rt(b, "root_finish"));
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "root_finish"));
}

fn root_interrupt(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const I: u32 = 3;
    const F: u32 = 4;
    const WAITS: u32 = 5;
    const W: u32 = 6;
    const J: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(R_OVER, 4));
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(R_FIBERS, 4));
    ins.local_set(1);
    ins.local_get(1);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    vec_item(ins, 1, I);
    ins.local_set(F);
    ins.local_get(F);
    ins.i32_load(mem(F_UNWINDING, 4));
    ins.i32_eqz();
    ins.local_get(F);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_const(0);
    ins.call(rt(b, "vec_has"));
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.i32_load(mem(F_PENDING, 4));
    ins.i32_const(0);
    ins.call(rt(b, "vec_push"));
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1024);
    ins.call(rt(b, "vec_new"));
    ins.local_set(WAITS);
    ins.local_get(0);
    ins.i32_load(mem(R_LAST_WAIT, 4));
    ins.local_set(W);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(W);
    ins.i32_load(mem(W_CLOSING, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(WAITS);
    ins.local_get(W);
    ins.call(rt(b, "vec_push"));
    ins.end();
    ins.local_get(W);
    ins.i32_load(mem(W_PREV, 4));
    ins.local_set(W);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(WAITS);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    vec_item(ins, WAITS, I);
    ins.local_set(W);
    ins.local_get(W);
    ins.i32_load(mem(W_LIVE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(W);
    ins.call(rt(b, "wait_drop"));
    ins.i32_const(KIND_ROOT_THROW);
    ins.call(rt(b, "job_new"));
    ins.local_set(J);
    store32(ins, J, J_W, W);
    ins.local_get(J);
    ins.local_get(W);
    ins.i32_load(mem(W_END, 4));
    ins.i32_store(mem(J_END, 4));
    ins.local_get(0);
    ins.i32_load(mem(R_QUEUE, 4));
    ins.local_get(J);
    ins.call(rt(b, "queue_push"));
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(R_RUNNING, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "drain"));
    ins.end();
}

fn run(b: &mut Builder, ins: &mut InstructionSink) {
    const R: u32 = 2;
    const T: u32 = 3;
    const JOB: u32 = 4;
    const J: u32 = 5;
    const MODE: u32 = 6;
    const PREV: u32 = 7;
    const RES: u32 = 10;
    ins.i32_const(168);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(R);
    setc(ins, R, R_OVER, 0);
    setc(ins, R, R_RUNNING, 0);
    setc(ins, R, R_HAS_RESULT, 0);
    setc(ins, R, R_RES_MODE, 0);
    setc(ins, R, R_RES_REF, 0);
    ins.local_get(R);
    ins.i64_const(0);
    ins.i64_store(mem(R_RES_VAL, 8));
    store32(ins, R, R_ENTRY, 0);
    store32(ins, R, R_REPR, 1);
    setc(ins, R, R_FIRST_WAIT, 0);
    setc(ins, R, R_LAST_WAIT, 0);
    setc(ins, R, R_ORPHAN, 0);
    setc(ins, R, R_PAUSES, 0);
    ins.i32_const(65536);
    ins.call(rt(b, "queue_new"));
    ins.local_set(T);
    store32(ins, R, R_QUEUE, T);
    ins.i32_const(4096);
    ins.call(rt(b, "queue_new"));
    ins.local_set(T);
    store32(ins, R, R_LATER, T);
    ins.i32_const(1024);
    ins.call(rt(b, "vec_new"));
    ins.local_set(T);
    store32(ins, R, R_FIBERS, T);
    if b.uses_timers {
        ins.i32_const(256);
        ins.call(rt(b, "vec_new"));
        ins.global_set(G_TIMERS);
        ins.i32_const(0);
        ins.global_set(G_TIMER_SEQ);
    }
    ins.i32_const(256);
    ins.call(rt(b, "vec_new"));
    ins.global_set(G_NOTE_ERRORS);
    ins.i32_const(256);
    ins.call(rt(b, "vec_new"));
    ins.global_set(G_NOTE_LINES);
    ins.global_get(G_ROOT);
    ins.local_set(PREV);
    ins.local_get(R);
    ins.global_set(G_ROOT);
    ins.global_get(G_RUN_ID);
    ins.i32_const(1);
    ins.i32_add();
    ins.global_set(G_RUN_ID);
    ins.local_get(R);
    ins.i32_const(0);
    ins.call(rt(b, "fiber_new"));
    ins.local_set(T);
    ins.i32_const(KIND_ENTRY);
    ins.call(rt(b, "job_new"));
    ins.local_set(JOB);
    store32(ins, JOB, J_FIBER, T);
    setc(ins, JOB, J_FN, 0);
    ins.local_get(R);
    ins.local_get(JOB);
    ins.call(rt(b, "schedule"));
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(R);
    ins.i32_load(mem(R_OVER, 4));
    ins.br_if(1);
    ins.local_get(R);
    ins.i32_load(mem(R_LATER, 4));
    ins.i32_load(mem(Q_LEN, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(R);
    ins.i32_load(mem(R_LATER, 4));
    ins.call(rt(b, "queue_pop"));
    ins.local_set(J);
    ins.local_get(R);
    ins.local_get(J);
    ins.call(rt(b, "schedule"));
    // Continue the loop: depth 0 is the `if`, depth 1 is the loop.
    ins.br(1);
    ins.end();
    if b.uses_timers {
        // The event loop is empty: run the earliest timer (T-Clock's `sleep`, spec 12.6).
        ins.local_get(R);
        ins.call(rt(b, "timers_due"));
        ins.if_(BlockType::Empty);
        ins.br(1);
        ins.end();
    }
    ins.local_get(R);
    ins.call(rt(b, "root_interrupt"));
    ins.local_get(R);
    ins.i32_load(mem(R_LATER, 4));
    ins.i32_load(mem(Q_LEN, 4));
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(R);
    ins.i32_load(mem(R_OVER, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(R);
    ins.i32_const(MODE_VALUE);
    ins.i64_const(0);
    ins.call(rt(b, "root_finish"));
    ins.end();
    ins.local_get(R);
    ins.i32_load(mem(R_RES_MODE, 4));
    ins.local_set(MODE);
    ins.local_get(R);
    ins.i64_load(mem(R_RES_VAL, 8));
    ins.local_set(RES);
    ins.local_get(MODE);
    ins.i32_const(MODE_THROW);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(RES);
    ins.i32_wrap_i64();
    ins.global_set(G_DEFECT);
    ins.else_();
    ins.local_get(MODE);
    ins.i32_const(MODE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(RES);
    ins.i32_wrap_i64();
    ins.global_set(G_FAILURE);
    ins.else_();
    ins.local_get(1);
    ins.i32_const(REF_CODE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(RES);
    ins.i32_wrap_i64();
    ins.call(rt(b, "drop"));
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(PREV);
    ins.global_set(G_ROOT);
}

// ---- the `suspend fn` external task primitives and the driver's timers (spec 12.6) ----

/// Whether `name` is a driver timer function: it uses WASI's clock, so it is emitted only when a
/// host that uses timers is requested (`emit::external::TIMER_HOSTS`).
pub(super) fn is_timer(name: &str) -> bool {
    matches!(name, "now_ms" | "after" | "timers_due")
}

const T_GOT: u32 = 0;
pub(super) const T_VALUE: u32 = 8;
const T_STR: u32 = 16;
const T_FUNC: u32 = 24;
const T_WAIT: u32 = 32;
const T_SIZE: u32 = 40;

const TM_AT: u32 = 0;
const TM_SEQ: u32 = 8;
const TM_H: u32 = 16;
const TM_SIZE: u32 = 24;

/// `task_new(name) -> task`: the record of a `suspend fn` external call (spec 12.6). The wrapper
/// makes one, calls the host with it, and `task_end` takes the completed value or suspends.
fn task_new(b: &mut Builder, ins: &mut InstructionSink) {
    const T: u32 = 1;
    ins.i32_const(T_SIZE as i32);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(T);
    setc(ins, T, T_GOT, 0);
    setc(ins, T, T_STR, 0);
    setc(ins, T, T_WAIT, 0);
    ins.local_get(T);
    ins.i64_const(0);
    ins.i64_store(mem(T_VALUE, 8));
    ins.local_get(T);
    ins.local_get(0);
    ins.i32_store(mem(T_FUNC, 4));
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    ins.local_get(T);
}

/// `task_end(task) -> word`: the host completed the task in place gives its value (or a pending
/// defect); otherwise the caller suspends on a host wait the driver wakes.
fn task_end(b: &mut Builder, ins: &mut InstructionSink) {
    const GOT: u32 = 1;
    const W: u32 = 2;
    ins.local_get(0);
    ins.i32_load(mem(T_GOT, 4));
    ins.local_set(GOT);
    ins.local_get(GOT);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(T_STR, 4));
    ins.global_set(G_DEFECT);
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.local_get(GOT);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i64_load(mem(T_VALUE, 8));
    ins.return_();
    ins.end();
    // Not completed in place: the run is over, or the fiber waits for the host.
    ins.global_get(G_ROOT);
    ins.i32_eqz();
    ins.global_get(G_ROOT);
    ins.i32_load(mem(R_OVER, 4));
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.call(rt(b, "susp_on"));
    ins.i64_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(WAIT_HOST);
    ins.i32_const(0);
    ins.call(rt(b, "wait_new"));
    ins.local_set(W);
    ins.local_get(0);
    ins.local_get(W);
    ins.i32_store(mem(T_WAIT, 4));
    ins.local_get(W);
    ins.call(rt(b, "susp_on"));
    ins.i64_const(0);
}

fn task_complete_int(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 1);
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    ins.i64_store(mem(T_VALUE, 8));
}

fn task_complete_int64(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 1);
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_store(mem(T_VALUE, 8));
}

fn task_complete_float(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 1);
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_reinterpret_f64();
    ins.i64_store(mem(T_VALUE, 8));
}

fn task_complete_unit(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 1);
    ins.local_get(0);
    ins.i64_const(0);
    ins.i64_store(mem(T_VALUE, 8));
}

fn task_complete_string(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 1);
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    ins.i64_store(mem(T_VALUE, 8));
}

fn task_complete_defect(_b: &mut Builder, ins: &mut InstructionSink) {
    setc(ins, 0, T_GOT, 2);
    store32(ins, 0, T_STR, 1);
}

/// `now_ms() -> i32`: the monotonic clock in milliseconds (WASI `clock_time_get`).
fn now_ms(b: &mut Builder, ins: &mut InstructionSink) {
    let buf = b.alloc_static(8);
    let clock = b.clock_time_get_fn;
    ins.i32_const(0); // CLOCK_MONOTONIC
    ins.i64_const(1_000_000);
    ins.i32_const(buf as i32);
    ins.call(clock);
    ins.drop();
    ins.i32_const(buf as i32);
    ins.i64_load(mem(0, 8));
    ins.i64_const(1_000_000);
    ins.i64_div_s();
    ins.i32_wrap_i64();
}

/// `after(ms, task)`: completes `task` with `()` once the run has been idle for `ms`. T-Clock's
/// `sleep` host calls this and returns; the driver fires the timer.
fn after(b: &mut Builder, ins: &mut InstructionSink) {
    const T: u32 = 2;
    ins.i32_const(TM_SIZE as i32);
    ins.call(rt(b, "rt_alloc"));
    ins.local_set(T);
    ins.local_get(T);
    ins.call(rt(b, "now_ms"));
    ins.i64_extend_i32_u();
    ins.local_get(0);
    ins.i64_extend_i32_u();
    ins.i64_add();
    ins.i64_store(mem(TM_AT, 8));
    ins.local_get(T);
    ins.global_get(G_TIMER_SEQ);
    ins.i32_store(mem(TM_SEQ, 4));
    ins.global_get(G_TIMER_SEQ);
    ins.i32_const(1);
    ins.i32_add();
    ins.global_set(G_TIMER_SEQ);
    ins.local_get(T);
    ins.local_get(1);
    ins.i32_store(mem(TM_H, 4));
    ins.global_get(G_TIMERS);
    ins.local_get(T);
    ins.call(rt(b, "vec_push"));
}

/// `timers_due(root) -> bool`: waits until the earliest timer is due and fires every due timer
/// (waking its waiting fiber). False when there is no timer.
fn timers_due(b: &mut Builder, ins: &mut InstructionSink) {
    const I: u32 = 1;
    const LEN: u32 = 2;
    const T: u32 = 3;
    const BEST: u32 = 4;
    const H: u32 = 5;
    const W: u32 = 6;
    const NOW: u32 = 9;
    const VAL: u32 = 10;
    ins.global_get(G_TIMERS);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_TIMERS);
    ins.i32_load(mem(V_LEN, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The earliest timer (by time, then sequence).
    ins.global_get(G_TIMERS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.i32_load(mem(0, 4));
    ins.local_set(BEST);
    ins.i32_const(1);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.global_get(G_TIMERS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(T);
    ins.local_get(T);
    ins.i64_load(mem(TM_AT, 8));
    ins.local_get(BEST);
    ins.i64_load(mem(TM_AT, 8));
    ins.i64_lt_s();
    ins.local_get(T);
    ins.i64_load(mem(TM_AT, 8));
    ins.local_get(BEST);
    ins.i64_load(mem(TM_AT, 8));
    ins.i64_eq();
    ins.local_get(T);
    ins.i32_load(mem(TM_SEQ, 4));
    ins.local_get(BEST);
    ins.i32_load(mem(TM_SEQ, 4));
    ins.i32_lt_u();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(T);
    ins.local_set(BEST);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Wait until it is due. WASM has no `poll(2)` to sleep on, so this spins on the WASI
    // monotonic clock until the wall-clock deadline (`now_ms`). This is deliberate: T-Clock's
    // timers are real-time (the driver has no virtual clock), so a long `sleep` really does
    // occupy the host for its duration, and a host whose clock does not advance would spin here
    // (W5-6).
    ins.call(rt(b, "now_ms"));
    ins.i64_extend_i32_u();
    ins.local_set(NOW);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(NOW);
    ins.local_get(BEST);
    ins.i64_load(mem(TM_AT, 8));
    ins.i64_ge_s();
    ins.br_if(1);
    ins.call(rt(b, "now_ms"));
    ins.i64_extend_i32_u();
    ins.local_set(NOW);
    ins.br(0);
    ins.end();
    ins.end();
    // Fire every due timer.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.global_get(G_TIMERS);
    ins.i32_load(mem(V_LEN, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.global_get(G_TIMERS);
    ins.i32_load(mem(V_ITEMS, 4));
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(T);
    ins.local_get(T);
    ins.i64_load(mem(TM_AT, 8));
    ins.local_get(NOW);
    ins.i64_le_s();
    ins.if_(BlockType::Empty);
    ins.global_get(G_TIMERS);
    ins.local_get(T);
    ins.call(rt(b, "vec_remove"));
    ins.local_get(T);
    ins.i32_load(mem(TM_H, 4));
    ins.local_set(H);
    // The value the waiting fiber resumes with. Every timer is fired as a completed task
    // (`task_complete_int64`), so its recorded `T_VALUE` must hold the value the resume expects:
    // `0` for T-Clock's unit `sleep`/a plain `after` (a unit timer), and `ms` for
    // `clock_sleep`/`after(ms, task)`, which the task recorded (W6-5). `TIMERS` entries that
    // never went through a `task_complete_*` therefore resume with `0`.
    ins.local_get(H);
    ins.i64_load(mem(T_VALUE, 8));
    ins.local_set(VAL);
    ins.local_get(H);
    ins.local_get(VAL);
    ins.call(rt(b, "task_complete_int64"));
    ins.local_get(H);
    ins.i32_load(mem(T_WAIT, 4));
    ins.local_set(W);
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_load(mem(W_LIVE, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(W);
    ins.i32_const(MODE_VALUE);
    ins.local_get(VAL);
    ins.call(rt(b, "cont"));
    ins.end();
    ins.end();
    ins.else_();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

/// The `emela:test` hosts, for the WASM tests of the `suspend fn` external ABI.
fn test_sleep_ms(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(1);
    ins.local_get(0);
    ins.i32_const(2);
    ins.i32_mul();
    ins.call(rt(b, "task_complete_int"));
}

fn test_sleep_after(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.local_get(1);
    ins.call(rt(b, "after"));
}
