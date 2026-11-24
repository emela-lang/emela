//! Semaphores and Queues (`[task.semaphore]`, `[task.queue]`), on the scheduler of
//! `runtime::fiber`.
//!
//! This is the WASM counterpart of the section "Semaphore and Queue" of the C runtime
//! (`crates/emela-codegen-c/src/emela_runtime.c`), so that all targets wake fibers in the same
//! order:
//!
//! - Fibers wait in first-in first-out lines. A line is a chain of entry records (a head that
//!   counts the first entry, each entry counting the next, and a raw tail). An entry counts the
//!   wait of its fiber and, for a fiber waiting to offer, the value it offers. A Queue keeps its
//!   values in entries without a wait.
//! - A fiber given what it waits for (permits, a value, the news that the queue is closed) is
//!   taken out of the line and woken with `fb_wait_served`: an interruption that arrives after
//!   that takes effect at its next suspension point.
//! - A fiber interrupted while it waits leaves its line when its wait is dropped (`fb_drop_wait`
//!   calls `fb_line_leave` with the wait's `W_OWNER`), and a semaphore then serves its line again.
//! - A Semaphore or a Queue belongs to no scope: the with of its built-in handler bounds its
//!   lifetime, and a fiber that can use it is in a scope inside that with, so none waits on it
//!   when the with ends (`[task.semaphore]`, `[task.queue]`).
//! - A wait in a line counts its owner, so the record lives while a fiber waits on it. The wait
//!   forgets it when woken or dropped, which breaks the cycle through the line.

use super::fiber::{
    R_REF, W_LIVE, W_OWNER, W_REPR, WK_LINE, c, call, dropv, g, i32_const_cell, ldl, plan,
    st_const, stl,
};
use super::*;

// ---- the Semaphore and Queue records ----

/// 0 for a Semaphore, 1 for a Queue.
const C_KIND: u32 = HDR;
/// The number of permits of a Semaphore; the capacity of a Queue.
const C_A: u32 = HDR + 8;
/// The free permits of a Semaphore; 1 when a Queue is closed.
const C_B: u32 = HDR + 16;
/// A Semaphore's line; a Queue's values, front first. The head (Ref) and the tail (raw, at +8).
const C_LINE: u32 = HDR + 24;
const SEM_SLOTS: i32 = 5;
/// The number of values of a Queue.
const Q_SIZE: u32 = HDR + 40;
/// The fibers waiting to take (only while the queue is empty) and to offer (only while it is full;
/// their entries hold the values). Each is a head and a tail, like `C_LINE`.
const Q_TAKERS: u32 = HDR + 48;
const Q_OFFERERS: u32 = HDR + 64;
const QUEUE_SLOTS: i32 = 10;

// ---- the entries of a line ----

const E_NEXT: u32 = HDR; // Ref
const E_WAIT: u32 = HDR + 8; // Ref: the fiber's wait, 0 for a value of a queue
const E_N: u32 = HDR + 16; // the permits a fiber waits for
const E_ISREF: u32 = HDR + 24;
const E_WORD: u32 = HDR + 32; // i64: the value
const E_VAL: u32 = HDR + 40; // Ref: the value when it is a reference
const E_SLOTS: i32 = 6;

/// The descriptors and messages of Semaphores and Queues.
#[derive(Default)]
pub(crate) struct Plan {
    /// Whether the program has Semaphore or Queue operations; without them none of this is
    /// emitted.
    pub(crate) used: bool,
    semaphore_desc: u32,
    queue_desc: u32,
    entry_desc: u32,
    some_ref: u32,
    some_val: u32,
    none: u32,
    msg_semaphore_permits: u32,
    msg_needs: [u32; 2],
    msg_asked: [u32; 2],
    msg_permits_of: u32,
    msg_capacity: u32,
}

/// Whether the program has Semaphore or Queue operations.
pub(crate) fn used(b: &Builder<'_>) -> bool {
    super::super::suspend::used(b)
        && b.m.functions.iter().any(|(_, f)| {
            f.stmts.iter().any(|(_, s)| {
                matches!(
                    s,
                    Stmt::Let {
                        value: Value::Fiber { op, .. },
                        ..
                    } if matches!(
                        op,
                        FiberOp::Semaphore
                            | FiberOp::Queue
                            | FiberOp::Acquire
                            | FiberOp::TryAcquire
                            | FiberOp::Release
                            | FiberOp::Available
                            | FiberOp::Offer
                            | FiberOp::Take
                            | FiberOp::TryOffer
                            | FiberOp::QueuePoll
                            | FiberOp::Size
                            | FiberOp::Close
                    )
                )
            })
        })
}

/// Called by `fiber::prepare`.
pub(super) fn prepare(b: &mut Builder<'_>) -> Plan {
    if !used(b) {
        return Plan::default();
    }
    let record = |b: &mut Builder<'_>, name: &str, refs: Vec<bool>| {
        b.desc(DescKey {
            kind: 0,
            tag: 0,
            enum_id: -1,
            name: name.to_owned(),
            show: None,
            refs,
        })
    };
    let semaphore_desc = record(b, "Semaphore", vec![false, false, false, true, false]);
    let queue_desc = record(
        b,
        "Queue",
        vec![
            false, false, false, true, false, false, true, false, true, false,
        ],
    );
    let entry_desc = record(b, "line", vec![true, true, false, false, false, true]);
    let (some_ref, none) = b.option_consts(true);
    let (some_val, _) = b.option_consts(false);
    let msg = |b: &mut Builder<'_>, s: &[u8]| b.string_literal(s);
    Plan {
        used: true,
        semaphore_desc,
        queue_desc,
        entry_desc,
        some_ref,
        some_val,
        none,
        msg_semaphore_permits: msg(b, b"semaphore needs at least 1 permit, but got "),
        msg_needs: [
            msg(b, b"`Permits` needs at least 1 permit, but got "),
            msg(b, b"`TryPermits` needs at least 1 permit, but got "),
        ],
        msg_asked: [
            msg(b, b"`Permits` asked for "),
            msg(b, b"`TryPermits` asked for "),
        ],
        msg_permits_of: msg(b, b" permits of a semaphore with "),
        msg_capacity: msg(b, b"queue needs a capacity of at least 1, but got "),
    }
}

fn lp<'a>(b: &'a Builder<'_>) -> &'a Plan {
    &plan(b).line
}

pub(super) const FUNCS: &[Sig] = &[
    ("fb_semaphore", &[ValType::I32], &[ValType::I32]),
    ("fb_queue", &[ValType::I32], &[ValType::I32]),
    ("fb_acquire", &[ValType::I32, ValType::I32], &[]),
    (
        "fb_try_acquire",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_release", &[ValType::I32, ValType::I32], &[]),
    ("fb_available", &[ValType::I32], &[ValType::I32]),
    (
        "fb_offer",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_take", &[ValType::I32], &[ValType::I32]),
    (
        "fb_try_offer",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_queue_poll", &[ValType::I32], &[ValType::I32]),
    ("fb_queue_size", &[ValType::I32], &[ValType::I32]),
    ("fb_queue_close", &[ValType::I32], &[]),
    (
        "fb_line_leave",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[],
    ),
    (
        "fb_cell_new",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_line_push", &[ValType::I32, ValType::I32], &[]),
    ("fb_line_shift", &[ValType::I32], &[ValType::I32]),
    (
        "fb_line_remove",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_line_wait",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I64,
            ValType::I32,
            ValType::I32,
        ],
        &[],
    ),
    (
        "fb_line_wake",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
    (
        "fb_entry_new",
        &[ValType::I32, ValType::I32, ValType::I64, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fb_sem_check",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fb_sem_serve", &[ValType::I32], &[]),
    (
        "fb_qcell_put",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
    ("fb_qcell_shift", &[ValType::I32], &[ValType::I32]),
    (
        "fb_some_new",
        &[ValType::I32, ValType::I64],
        &[ValType::I32],
    ),
    (
        "fb_raise_count",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[],
    ),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "fb_semaphore" | "fb_queue" => i32s(1),
        "fb_acquire" | "fb_try_acquire" | "fb_release" => i32s(1),
        "fb_offer" | "fb_try_offer" => i32s(1),
        "fb_take" | "fb_queue_poll" => i32s(1),
        "fb_queue_close" => i32s(2),
        "fb_cell_new" => i32s(2),
        "fb_line_push" => i32s(1),
        "fb_line_shift" => i32s(2),
        "fb_line_remove" => i32s(3),
        "fb_line_wait" => i32s(2),
        "fb_entry_new" => i32s(1),
        "fb_sem_serve" => i32s(3),
        "fb_qcell_put" => i32s(3),
        "fb_qcell_shift" => i32s(4),
        "fb_some_new" => i32s(1),
        "fb_raise_count" => i32s(3),
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fb_semaphore" => new_cell(b, ins, false),
        "fb_queue" => new_cell(b, ins, true),
        "fb_acquire" => acquire(b, ins),
        "fb_try_acquire" => try_acquire(b, ins),
        "fb_release" => release(b, ins),
        "fb_available" => available(b, ins),
        "fb_offer" => offer(b, ins),
        "fb_take" => take(b, ins),
        "fb_try_offer" => try_offer(b, ins),
        "fb_queue_poll" => queue_poll(b, ins),
        "fb_queue_size" => queue_size(b, ins),
        "fb_queue_close" => queue_close(b, ins),
        "fb_line_leave" => line_leave(b, ins),
        "fb_cell_new" => cell_new(b, ins),
        "fb_line_push" => line_push(b, ins),
        "fb_line_shift" => line_shift(b, ins),
        "fb_line_remove" => line_remove(b, ins),
        "fb_line_wait" => line_wait(b, ins),
        "fb_line_wake" => line_wake(b, ins),
        "fb_entry_new" => entry_new(b, ins),
        "fb_sem_check" => sem_check(b, ins),
        "fb_sem_serve" => sem_serve(b, ins),
        "fb_qcell_put" => qcell_put(b, ins),
        "fb_qcell_shift" => qcell_shift(b, ins),
        "fb_some_new" => some_new(b, ins),
        "fb_raise_count" => raise_count(b, ins),
        _ => return false,
    }
    true
}

// ---- small helpers ----

/// Pushes the address of the line at `off` of the record in local `l`.
fn line_at(ins: &mut InstructionSink, l: u32, off: u32) {
    g(ins, l);
    c(ins, off as i32);
    ins.i32_add();
}

/// Adds `delta` to the i32 slot `off` of the record in local `l`.
fn add_slot(ins: &mut InstructionSink, l: u32, off: u32, delta: i32) {
    g(ins, l);
    ldl(ins, l, off);
    c(ins, delta);
    ins.i32_add();
    ins.i32_store(mem(off, 4));
}

/// Takes the wait out of the entry in local `e` into local `w`, and releases the entry (with the
/// value it holds).
fn take_wait(b: &mut Builder<'_>, ins: &mut InstructionSink, e: u32, w: u32) {
    ldl(ins, e, E_WAIT);
    ins.local_set(w);
    st_const(ins, e, E_WAIT, 0);
    dropv(b, ins, e);
}

// ---- lines ----

/// `fb_line_push(line, entry)`: append the entry (its reference) at the end of the line.
fn line_push(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const T: u32 = 2;
    st_const(ins, 1, E_NEXT, 0);
    g(ins, 0);
    ins.i32_load(mem(8, 4));
    ins.local_tee(T);
    ins.if_(BlockType::Empty);
    stl(ins, T, E_NEXT, 1);
    ins.else_();
    g(ins, 0);
    g(ins, 1);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, 0);
    g(ins, 1);
    ins.i32_store(mem(8, 4));
}

/// `fb_line_shift(line) -> entry`: take the first entry out of the line (its reference), or 0.
fn line_shift(_b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    const N: u32 = 2;
    g(ins, 0);
    ins.i32_load(mem(0, 4));
    ins.local_tee(E);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, E, E_NEXT);
    ins.local_set(N);
    g(ins, 0);
    g(ins, N);
    ins.i32_store(mem(0, 4));
    g(ins, N);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    c(ins, 0);
    ins.i32_store(mem(8, 4));
    ins.end();
    st_const(ins, E, E_NEXT, 0);
    g(ins, E);
}

/// `fb_line_remove(line, w) -> i32`: take the entry of the wait out of the line and release it
/// (with the value it holds); 0 if the wait is not in the line.
fn line_remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PREV: u32 = 2;
    const E: u32 = 3;
    const N: u32 = 4;
    c(ins, 0);
    ins.local_set(PREV);
    g(ins, 0);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, E);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, E, E_WAIT);
    g(ins, 1);
    ins.i32_eq();
    ins.br_if(1);
    g(ins, E);
    ins.local_set(PREV);
    ldl(ins, E, E_NEXT);
    ins.local_set(E);
    ins.br(0);
    ins.end();
    ins.end();
    ldl(ins, E, E_NEXT);
    ins.local_set(N);
    g(ins, PREV);
    ins.if_(BlockType::Empty);
    stl(ins, PREV, E_NEXT, N);
    ins.else_();
    g(ins, 0);
    g(ins, N);
    ins.i32_store(mem(0, 4));
    ins.end();
    g(ins, N);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, PREV);
    ins.i32_store(mem(8, 4));
    ins.end();
    st_const(ins, E, E_NEXT, 0);
    dropv(b, ins, E);
    c(ins, 1);
}

/// `fb_entry_new(w, n, word, isref) -> entry`: an entry for the wait `w` (counted, 0 for none)
/// waiting for `n` permits, or holding the value `word` (its reference, when `isref`).
fn entry_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 4;
    let desc = lp(b).entry_desc;
    c(ins, E_SLOTS);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(E);
    st_const(ins, E, E_NEXT, 0);
    stl(ins, E, E_WAIT, 0);
    g(ins, 0);
    ins.if_(BlockType::Empty);
    g(ins, 0);
    call(b, ins, "dup");
    ins.end();
    stl(ins, E, E_N, 1);
    stl(ins, E, E_ISREF, 3);
    g(ins, E);
    ins.local_get(2);
    ins.i64_store(mem(E_WORD, 8));
    g(ins, E);
    g(ins, 3);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.else_();
    c(ins, 0);
    ins.end();
    ins.i32_store(mem(E_VAL, 4));
    g(ins, E);
}

/// `fb_line_wait(owner, line, n, word, isref, repr)`: suspend the current fiber at the end of
/// the line of `owner` (borrowed). `repr` is the representation of the value it is woken with.
fn line_wait(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const W: u32 = 6;
    const E: u32 = 7;
    c(ins, WK_LINE);
    g(ins, 5);
    call(b, ins, "fb_wait_new");
    ins.local_set(W);
    g(ins, 0);
    call(b, ins, "dup");
    stl(ins, W, W_OWNER, 0);
    g(ins, W);
    g(ins, 2);
    ins.local_get(3);
    g(ins, 4);
    call(b, ins, "fb_entry_new");
    ins.local_set(E);
    g(ins, 1);
    g(ins, E);
    call(b, ins, "fb_line_push");
    i32_const_cell(b, ins, |p| p.susp_wait);
    g(ins, W);
    ins.i32_store(mem(0, 4));
    c(ins, 1);
    ins.global_set(G_SUSPENDING);
}

/// `fb_line_wake(w, mode, word, served)`: wake the wait of an entry taken out of a line with the
/// input (owned), and release the entry's reference to the wait. A wait that is no longer live
/// is not woken; the input is released.
fn line_wake(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, W_LIVE);
    ins.if_(BlockType::Empty);
    // The wake takes the run's reference to the wait.
    g(ins, 3);
    ins.if_(BlockType::Empty);
    g(ins, 0);
    g(ins, 1);
    ins.local_get(2);
    call(b, ins, "fb_wait_served");
    ins.else_();
    g(ins, 0);
    g(ins, 1);
    ins.local_get(2);
    call(b, ins, "fb_wait_wake");
    ins.end();
    ins.else_();
    // A failure or a defect is a reference; a value only in the reference representation.
    g(ins, 1);
    ldl(ins, 0, W_REPR);
    c(ins, R_REF);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.i32_wrap_i64();
    call(b, ins, "drop");
    ins.end();
    ins.end();
    dropv(b, ins, 0);
}

/// `fb_line_leave(w, owner, serve)`: the wait was dropped while it waited in a line of `owner`
/// (borrowed): take it out, and when `serve`, let a semaphore serve its line again.
fn line_leave(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 1, C_KIND);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    line_at(ins, 1, C_LINE);
    g(ins, 0);
    call(b, ins, "fb_line_remove");
    g(ins, 2);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    g(ins, 1);
    call(b, ins, "fb_sem_serve");
    ins.end();
    ins.else_();
    line_at(ins, 1, Q_TAKERS);
    g(ins, 0);
    call(b, ins, "fb_line_remove");
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    line_at(ins, 1, Q_OFFERERS);
    g(ins, 0);
    call(b, ins, "fb_line_remove");
    ins.drop();
    ins.end();
    ins.end();
}

// ---- records ----

/// `fb_cell_new(kind, slots, desc) -> record`: a new Semaphore or Queue record with every slot 0.
fn cell_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const C: u32 = 3;
    const I: u32 = 4;
    g(ins, 1);
    g(ins, 2);
    call(b, ins, "alloc");
    ins.local_set(C);
    c(ins, 0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, I);
    g(ins, 1);
    ins.i32_ge_s();
    ins.br_if(1);
    g(ins, C);
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    ins.i64_const(0);
    ins.i64_store(mem(HDR, 8));
    g(ins, I);
    c(ins, 1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    stl(ins, C, C_KIND, 0);
    g(ins, C);
}

/// `fb_some_new(isref, word) -> Option`: `Some(word)`, taking the value's reference.
fn some_new(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const O: u32 = 2;
    let (some_ref, some_val) = (lp(b).some_ref, lp(b).some_val);
    c(ins, 1);
    g(ins, 0);
    ins.if_(BlockType::Result(ValType::I32));
    c(ins, some_ref as i32);
    ins.else_();
    c(ins, some_val as i32);
    ins.end();
    call(b, ins, "alloc");
    ins.local_set(O);
    g(ins, O);
    ins.local_get(1);
    ins.i64_store(mem(HDR, 8));
    g(ins, O);
}

/// `fb_raise_count(prefix, n, middle, m)`: raise the defect `prefix` `n`, followed by `middle`
/// `m` when `middle` is not 0.
fn raise_count(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const S: u32 = 4;
    const T: u32 = 5;
    const U: u32 = 6;
    g(ins, 1);
    call(b, ins, "int_to_string");
    ins.local_set(T);
    g(ins, 0);
    g(ins, T);
    call(b, ins, "str_append");
    ins.local_set(S);
    dropv(b, ins, T);
    g(ins, 2);
    ins.if_(BlockType::Empty);
    g(ins, S);
    g(ins, 2);
    call(b, ins, "str_append");
    ins.local_set(U);
    dropv(b, ins, S);
    g(ins, 3);
    call(b, ins, "int_to_string");
    ins.local_set(T);
    g(ins, U);
    g(ins, T);
    call(b, ins, "str_append");
    ins.local_set(S);
    dropv(b, ins, U);
    dropv(b, ins, T);
    ins.end();
    g(ins, S);
    call(b, ins, "raise");
    dropv(b, ins, S);
}

/// `fb_semaphore(permits)` / `fb_queue(capacity)`: the inits of the built-in handlers
/// `Semaphore(permits:)` (`[task.semaphore]`) and `Queue(capacity:)` (`[task.queue]`).
fn new_cell(b: &mut Builder<'_>, ins: &mut InstructionSink, queue: bool) {
    const C: u32 = 1;
    let p = lp(b);
    let (small, desc, slots) = if queue {
        (p.msg_capacity, p.queue_desc, QUEUE_SLOTS)
    } else {
        (p.msg_semaphore_permits, p.semaphore_desc, SEM_SLOTS)
    };
    g(ins, 0);
    c(ins, 1);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    c(ins, small as i32);
    g(ins, 0);
    c(ins, 0);
    c(ins, 0);
    call(b, ins, "fb_raise_count");
    c(ins, 0);
    ins.return_();
    ins.end();
    c(ins, i32::from(queue));
    c(ins, slots);
    c(ins, desc as i32);
    call(b, ins, "fb_cell_new");
    ins.local_set(C);
    stl(ins, C, C_A, 0);
    if !queue {
        stl(ins, C, C_B, 0);
    }
    g(ins, C);
}

// ---- Semaphore ----

/// `fb_sem_check(s, n, try) -> i32`: raise the defect of a Semaphore asked for a bad number of
/// permits by `Permits` (`try` 0) or `TryPermits` (1); 1 if it did.
fn sem_check(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let p = lp(b);
    let (needs, asked, of) = (p.msg_needs, p.msg_asked, p.msg_permits_of);
    g(ins, 1);
    c(ins, 1);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    c(ins, needs[1] as i32);
    c(ins, needs[0] as i32);
    g(ins, 2);
    ins.select();
    g(ins, 1);
    c(ins, 0);
    c(ins, 0);
    call(b, ins, "fb_raise_count");
    c(ins, 1);
    ins.return_();
    ins.end();
    g(ins, 1);
    ldl(ins, 0, C_A);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    c(ins, asked[1] as i32);
    c(ins, asked[0] as i32);
    g(ins, 2);
    ins.select();
    g(ins, 1);
    c(ins, of as i32);
    ldl(ins, 0, C_A);
    call(b, ins, "fb_raise_count");
    c(ins, 1);
    ins.return_();
    ins.end();
    c(ins, 0);
}

/// `fb_sem_serve(s)`: give permits to the fibers at the front of the line while enough are free
/// for the first. The semaphore is held meanwhile, since a woken wait drops its reference to it.
fn sem_serve(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    const W: u32 = 2;
    g(ins, 0);
    call(b, ins, "dup");
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ldl(ins, 0, C_LINE);
    ins.local_tee(E);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, E, E_N);
    ldl(ins, 0, C_B);
    ins.i32_gt_s();
    ins.br_if(1);
    line_at(ins, 0, C_LINE);
    call(b, ins, "fb_line_shift");
    ins.local_set(E);
    g(ins, 0);
    ldl(ins, 0, C_B);
    ldl(ins, E, E_N);
    ins.i32_sub();
    ins.i32_store(mem(C_B, 4));
    take_wait(b, ins, E, W);
    g(ins, W);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 1);
    call(b, ins, "fb_line_wake");
    ins.br(0);
    ins.end();
    ins.end();
    dropv(b, ins, 0);
}

/// `fb_acquire(s, n)`: the init of the handler member `Permits` (`[task.semaphore]`). Waits until `n` permits
/// are free and no fiber is ahead in the line.
fn acquire(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    g(ins, 0);
    g(ins, 1);
    c(ins, 0);
    call(b, ins, "fb_sem_check");
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ldl(ins, 0, C_LINE);
    ins.i32_eqz();
    ldl(ins, 0, C_B);
    g(ins, 1);
    ins.i32_ge_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    sub_slot_local(ins, 0, C_B, 1);
    ins.return_();
    ins.end();
    g(ins, 0);
    line_at(ins, 0, C_LINE);
    g(ins, 1);
    ins.i64_const(0);
    c(ins, 0);
    c(ins, 0);
    call(b, ins, "fb_line_wait");
}

/// Subtracts local `n` from the i32 slot `off` of the record in local `l`.
fn sub_slot_local(ins: &mut InstructionSink, l: u32, off: u32, n: u32) {
    g(ins, l);
    ldl(ins, l, off);
    g(ins, n);
    ins.i32_sub();
    ins.i32_store(mem(off, 4));
}

/// `fb_try_acquire(s, n) -> Bool`: the init of the handler member `TryPermits` takes the permits
/// only if no fiber waits and enough are free (`[task.semaphore]`).
fn try_acquire(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    g(ins, 0);
    g(ins, 1);
    c(ins, 1);
    call(b, ins, "fb_sem_check");
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, C_LINE);
    ldl(ins, 0, C_B);
    g(ins, 1);
    ins.i32_lt_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    sub_slot_local(ins, 0, C_B, 1);
    c(ins, 1);
}

/// `fb_release(s, n)`: the release of the handler members `Permits` and `TryPermits`.
fn release(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    g(ins, 0);
    ldl(ins, 0, C_B);
    g(ins, 1);
    ins.i32_add();
    ins.i32_store(mem(C_B, 4));
    g(ins, 0);
    call(b, ins, "fb_sem_serve");
}

/// `fb_available(s) -> Int`: `available()` of a Semaphore.
fn available(_: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, C_B);
}

// ---- Queue ----

/// `fb_qcell_put(q, word, isref)`: add the value (its reference): to the first fiber waiting to
/// take, or at the end. The caller has checked that it fits and holds the queue.
fn qcell_put(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 3;
    const W: u32 = 4;
    line_at(ins, 0, Q_TAKERS);
    call(b, ins, "fb_line_shift");
    ins.local_tee(E);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    c(ins, 0);
    ins.local_get(1);
    g(ins, 2);
    call(b, ins, "fb_entry_new");
    ins.local_set(E);
    line_at(ins, 0, C_LINE);
    g(ins, E);
    call(b, ins, "fb_line_push");
    add_slot(ins, 0, Q_SIZE, 1);
    ins.return_();
    ins.end();
    take_wait(b, ins, E, W);
    g(ins, W);
    c(ins, 0);
    g(ins, 2);
    ins.local_get(1);
    call(b, ins, "fb_some_new");
    ins.i64_extend_i32_u();
    c(ins, 1);
    call(b, ins, "fb_line_wake");
}

/// `fb_qcell_shift(q) -> Option`: remove the value at the front as `Some`; the value of the first
/// fiber waiting to offer takes its place at the end. The caller holds the queue.
fn qcell_shift(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    const O: u32 = 2;
    const W: u32 = 3;
    line_at(ins, 0, C_LINE);
    call(b, ins, "fb_line_shift");
    ins.local_set(E);
    add_slot(ins, 0, Q_SIZE, -1);
    ldl(ins, E, E_ISREF);
    g(ins, E);
    ins.i64_load(mem(E_WORD, 8));
    call(b, ins, "fb_some_new");
    ins.local_set(O);
    // The `Some` took the value's reference.
    st_const(ins, E, E_VAL, 0);
    dropv(b, ins, E);
    line_at(ins, 0, Q_OFFERERS);
    call(b, ins, "fb_line_shift");
    ins.local_tee(E);
    ins.if_(BlockType::Empty);
    ldl(ins, E, E_WAIT);
    ins.local_set(W);
    st_const(ins, E, E_WAIT, 0);
    // The entry keeps the offered value at the end of the queue.
    line_at(ins, 0, C_LINE);
    g(ins, E);
    call(b, ins, "fb_line_push");
    add_slot(ins, 0, Q_SIZE, 1);
    g(ins, W);
    c(ins, 0);
    ins.i64_const(1);
    c(ins, 1);
    call(b, ins, "fb_line_wake");
    ins.end();
    g(ins, O);
}

/// Emits `local 3 = repr == Ref` and duplicates the borrowed value `local 1` when it is a
/// reference: the queue keeps its own.
fn own_value(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const R: u32 = 3;
    g(ins, 2);
    c(ins, R_REF);
    ins.i32_eq();
    ins.local_tee(R);
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_wrap_i64();
    call(b, ins, "dup");
    ins.end();
}

/// Emits a `fb_qcell_put` of `local 1` (its representation flag in `local 3`) holding the queue.
fn put_held(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    ins.local_get(1);
    g(ins, 3);
    call(b, ins, "fb_qcell_put");
    dropv(b, ins, 0);
}

/// `fb_offer(q, word, repr) -> Bool`: `offer` of a Queue (`[task.queue]`). Waits while the queue is
/// full; False when it is closed (lowering makes it the failure `QueueClosed`).
fn offer(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, C_B);
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    own_value(b, ins);
    ldl(ins, 0, Q_SIZE);
    ldl(ins, 0, C_A);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    put_held(b, ins);
    c(ins, 1);
    ins.return_();
    ins.end();
    g(ins, 0);
    line_at(ins, 0, Q_OFFERERS);
    c(ins, 0);
    ins.local_get(1);
    g(ins, 3);
    // The fiber is woken with a Bool.
    c(ins, 1);
    call(b, ins, "fb_line_wait");
    c(ins, 0);
}

/// `fb_take(q) -> Option`: `take` of a Queue (`[task.queue]`). Waits while the queue is empty; `None`
/// when it is closed and empty.
fn take(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const O: u32 = 1;
    let none = lp(b).none;
    call(b, ins, "fb_interrupt_here");
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    ldl(ins, 0, Q_SIZE);
    ins.if_(BlockType::Empty);
    shift_held(b, ins, O);
    g(ins, O);
    ins.return_();
    ins.end();
    ldl(ins, 0, C_B);
    ins.if_(BlockType::Empty);
    c(ins, none as i32);
    ins.return_();
    ins.end();
    g(ins, 0);
    line_at(ins, 0, Q_TAKERS);
    c(ins, 0);
    ins.i64_const(0);
    c(ins, 0);
    // The fiber is woken with an Option.
    c(ins, R_REF);
    call(b, ins, "fb_line_wait");
    c(ins, 0);
}

/// Emits a `fb_qcell_shift` of the queue in local 0, holding it, into local `o`.
fn shift_held(b: &mut Builder<'_>, ins: &mut InstructionSink, o: u32) {
    g(ins, 0);
    call(b, ins, "dup");
    g(ins, 0);
    call(b, ins, "fb_qcell_shift");
    ins.local_set(o);
    dropv(b, ins, 0);
}

/// `fb_try_offer(q, word, repr) -> Bool`: `try_offer` of a Queue (`[task.queue]`).
fn try_offer(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, C_B);
    ldl(ins, 0, Q_SIZE);
    ldl(ins, 0, C_A);
    ins.i32_ge_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    c(ins, 0);
    ins.return_();
    ins.end();
    own_value(b, ins);
    put_held(b, ins);
    c(ins, 1);
}

/// `fb_queue_poll(q) -> Option`: `poll` of a Queue (`[task.queue]`).
fn queue_poll(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const O: u32 = 1;
    let none = lp(b).none;
    ldl(ins, 0, Q_SIZE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    c(ins, none as i32);
    ins.return_();
    ins.end();
    shift_held(b, ins, O);
    g(ins, O);
}

/// `fb_queue_size(q) -> Int`: `size()` of a Queue (`[task.queue]`).
fn queue_size(_: &mut Builder<'_>, ins: &mut InstructionSink) {
    ldl(ins, 0, Q_SIZE);
}

/// `fb_queue_close(q)`: `close` of a Queue (`[task.queue]`). The fibers waiting to take get `None`
/// and those waiting to offer get False (lowering makes both the failure `QueueClosed`).
fn queue_close(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    const W: u32 = 2;
    let none = lp(b).none;
    ldl(ins, 0, C_B);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    st_const(ins, 0, C_B, 1);
    g(ins, 0);
    call(b, ins, "dup");
    for (line, word) in [(Q_TAKERS, none as i64), (Q_OFFERERS, 0)] {
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        line_at(ins, 0, line);
        call(b, ins, "fb_line_shift");
        ins.local_tee(E);
        ins.i32_eqz();
        ins.br_if(1);
        take_wait(b, ins, E, W);
        g(ins, W);
        c(ins, 0);
        ins.i64_const(word);
        c(ins, 1);
        call(b, ins, "fb_line_wake");
        ins.br(0);
        ins.end();
        ins.end();
    }
    dropv(b, ins, 0);
}
