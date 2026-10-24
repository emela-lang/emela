//! `Fiber.attempt` and the operations of `Std.Cause` (spec 11.5).
//!
//! `attempt` runs its function on the current fiber, so it needs no fiber of its own: the call is
//! an indirect call, and what it left (a value, a pending failure or a pending defect) becomes an
//! `Exit`. When the function suspends, `attempt` pushes a frame of its own after the callee's
//! frames, and `attempt_resume` turns the callee's result into the `Exit` when the continuation
//! is resumed.
//!
//! An interruption is not caught: the pending `interrupted` defect stays pending. While the run
//! unwinds as interrupted (it stalled, spec 14.5) a defect is not caught either, so that the
//! defect of a release stays the result of the run (`[fiber.cause]`). `unwind_ops` sets the
//! `unwinding` cell when it unwinds the whole run, and the entry clears it before each test.
//!
//! A `Cause` is an object of one slot, the failure (an error value) or the defect (a String), and
//! its kind in the header's `aux` word: 0 `Fail`, 1 `Die`, 2 `Interrupt` (the order of
//! `CauseKind`). The type's field (`raise`) only makes `E` an error set and is never called; the
//! debug display shows it as `<fn>`. Code that loads the field gets the reference in the slot, so
//! counting it stays sound. An `Exit` is the same object `Value::Make` builds for `Success` and
//! `Failure`, with the same descriptors.
//!
//! The module is only part of the output when the program uses one of these operations, so the
//! bytes of the other programs do not change.

use super::*;

use super::sched::{F_STATE, cell};

pub(super) const FUNCS: &[Sig] = &[
    ("cause_new", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("attempt_caught", &[], &[ValType::I32]),
    (
        "exit_success",
        &[ValType::I32, ValType::I64],
        &[ValType::I32],
    ),
    ("attempt_resume", &[ValType::I32], &[]),
    ("cause_to_string", &[ValType::I32], &[ValType::I32]),
    ("cause_raise", &[ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "cause_new" => i32s(1),       // cause
        "attempt_caught" => i32s(2),  // cause, exit
        "exit_success" => i32s(1),    // exit
        "attempt_resume" => i32s(3),  // desc, mode, exit
        "cause_to_string" => i32s(2), // shown, result
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "cause_new" => cause_new(b, ins),
        "attempt_caught" => attempt_caught(b, ins),
        "exit_success" => exit_success(b, ins),
        "attempt_resume" => attempt_resume(b, ins),
        "cause_to_string" => cause_to_string(b, ins),
        "cause_raise" => cause_raise(b, ins),
        _ => return false,
    }
    true
}

/// Whether the program uses `Fiber.attempt` or an operation of `Std.Cause`, the only concurrency
/// operations the target supports (`check::value`).
pub(crate) fn used(b: &Builder) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Fiber { .. },
                    ..
                }
            )
        })
    })
}

/// Whether some `Fiber.attempt` runs a function that may suspend, so the scheduler needs the
/// resume function of its frame.
pub(crate) fn suspends(b: &Builder) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let {
                    value: Value::Fiber {
                        op: FiberOp::Attempt { suspends: true, .. },
                        ..
                    },
                    ..
                }
            )
        })
    })
}

/// The kinds of a cause, in the order of `CauseKind`.
pub(crate) const CAUSE_FAIL: i32 = 0;
pub(crate) const CAUSE_DIE: i32 = 1;

/// The `aux` word of an object header, which holds the kind of a cause.
const AUX: u32 = 4;

fn cause_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Cause".to_owned(),
        show: None,
        refs: vec![true],
    })
}

/// The descriptor of `Failure(cause)`, the one `Value::Make` uses (`Exit` is not an error enum).
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

/// The descriptor of `Success(value)` for a value of `repr`, the one `Value::Make` uses.
pub(crate) fn success_desc(b: &mut Builder, repr: Repr) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "Success".to_owned(),
        show: None,
        refs: vec![repr == Repr::Ref],
    })
}

/// The descriptor of an attempt's frame: the resume function, the `Success` descriptor (in the
/// state slot) and the next frame.
pub(crate) fn frame_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "attempt".to_owned(),
        show: None,
        refs: vec![false, false, true],
    })
}

/// `cause_new(kind, value) -> Cause`: takes `value`.
fn cause_new(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 2;
    let desc = cause_desc(b);
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_tee(C);
    ins.local_get(0);
    ins.i32_store(mem(AUX, 4));
    ins.local_get(C);
    ins.local_get(1);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(C);
}

/// Leaves `Failure(cause)` of the cause made of `kind` and the pending value in `global`, which
/// it clears. `c` and `exit` are temporaries.
fn failure_of(
    b: &mut Builder,
    ins: &mut InstructionSink,
    kind: i32,
    global: u32,
    c: u32,
    exit: u32,
) {
    let desc = failure_desc(b);
    ins.i32_const(kind);
    ins.global_get(global);
    ins.call(rt(b, "cause_new"));
    ins.local_set(c);
    ins.i32_const(0);
    ins.global_set(global);
    ins.i32_const(1);
    ins.i32_const(desc as i32);
    ins.call(rt(b, "alloc"));
    ins.local_tee(exit);
    ins.local_get(c);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(exit);
}

/// `attempt_caught() -> Exit or 0`: after the function of an attempt returned, `Failure` of the
/// pending failure or defect, which it takes; 0 when there is none, or when the pending defect is
/// not caught (an interruption, or any defect while the run unwinds as interrupted).
fn attempt_caught(b: &mut Builder, ins: &mut InstructionSink) {
    const C: u32 = 0;
    const EXIT: u32 = 1;
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    if let Some(plan) = &b.sus {
        let (interrupted, unwinding) = (plan.interrupted, plan.unwinding);
        ins.global_get(G_DEFECT);
        ins.i32_const(interrupted as i32);
        ins.i32_eq();
        ins.i32_const(unwinding as i32);
        ins.i32_load(mem(0, 4));
        ins.i32_or();
        ins.if_(BlockType::Empty);
        ins.i32_const(0);
        ins.return_();
        ins.end();
    }
    failure_of(b, ins, CAUSE_DIE, G_DEFECT, C, EXIT);
    ins.return_();
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    failure_of(b, ins, CAUSE_FAIL, G_FAILURE, C, EXIT);
    ins.return_();
    ins.end();
    ins.i32_const(0);
}

/// `exit_success(desc, word) -> Exit`: `Success` of the value in `word` (as its representation
/// stores it in a slot), which it takes.
fn exit_success(b: &mut Builder, ins: &mut InstructionSink) {
    const E: u32 = 2;
    ins.i32_const(1);
    ins.local_get(0);
    ins.call(rt(b, "alloc"));
    ins.local_tee(E);
    ins.local_get(1);
    ins.i64_store(mem(HDR, 8));
    ins.local_get(E);
}

/// `attempt_resume(frame)`: the resume function of an attempt's frame. The input is the result
/// of the attempt's function; the output is its `Exit`, or the defect that is not caught.
fn attempt_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const DESC: u32 = 1;
    const MODE: u32 = 2;
    const EXIT: u32 = 3;
    let Some(plan) = &b.sus else {
        // Only a frame pushed by a suspending attempt reaches this, and then the suspend runtime
        // is there.
        ins.unreachable();
        return;
    };
    let (in_mode, in_word, out_mode, out_word) =
        (plan.in_mode, plan.in_word, plan.out_mode, plan.out_word);
    ins.local_get(0);
    ins.i32_load(mem(F_STATE, 4));
    ins.local_set(DESC);
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.i32_const(in_mode as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(MODE);
    ins.local_get(MODE);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(in_word as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_DEFECT);
    ins.end();
    ins.local_get(MODE);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(in_word as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_FAILURE);
    ins.end();
    ins.call(rt(b, "attempt_caught"));
    ins.local_tee(EXIT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    // Not caught: the defect goes on to the next frame.
    ins.i32_const(out_mode as i32);
    ins.i32_const(2);
    ins.i32_store(mem(0, 4));
    ins.i32_const(out_word as i32);
    ins.global_get(G_DEFECT);
    ins.i32_store(mem(0, 4));
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.return_();
    ins.end();
    ins.local_get(DESC);
    ins.i32_const(in_word as i32);
    ins.i64_load(mem(0, 8));
    ins.call(rt(b, "exit_success"));
    ins.local_set(EXIT);
    ins.end();
    ins.i32_const(out_mode as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.i32_const(out_word as i32);
    ins.local_get(EXIT);
    ins.i32_store(mem(0, 4));
}

/// `cause_to_string(cause) -> String`: `error: ...`, `defect: ...` or `interrupted`. Borrows the
/// cause.
fn cause_to_string(b: &mut Builder, ins: &mut InstructionSink) {
    const SHOWN: u32 = 1;
    const RESULT: u32 = 2;
    let error = b.string_literal(b"error: ");
    let defect = b.string_literal(b"defect: ");
    let interrupted = b.string_literal(b"interrupted");
    ins.local_get(0);
    ins.i32_load(mem(AUX, 4));
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // `show_failure` takes the error value; the cause keeps its own.
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "dup"));
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "show_failure"));
    ins.local_tee(SHOWN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // An error value without a display shows as nothing (spec 14.3, the same as `main`'s).
    ins.i32_const(error as i32);
    ins.return_();
    ins.end();
    ins.i32_const(error as i32);
    ins.local_get(SHOWN);
    ins.call(rt(b, "str_append"));
    ins.local_set(RESULT);
    ins.local_get(SHOWN);
    ins.call(rt(b, "drop"));
    ins.local_get(RESULT);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(AUX, 4));
    ins.i32_const(CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(defect as i32);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "str_append"));
    ins.return_();
    ins.end();
    ins.i32_const(interrupted as i32);
}

/// `cause_raise(cause)`: `Fail` leaves its error as the pending failure, `Die` raises its defect
/// again (the same String, so its markers stay), and `Interrupt` unwinds as interrupted. Borrows
/// the cause.
fn cause_raise(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(AUX, 4));
    ins.i32_const(CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "dup"));
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.global_set(G_FAILURE);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(AUX, 4));
    ins.i32_const(CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "raise"));
    ins.return_();
    ins.end();
    // `Interrupt`: only `Fiber.await` makes one, so it needs the suspend runtime.
    if b.sus.is_some() {
        ins.call(rt(b, "interrupted"));
        ins.call(rt(b, "raise"));
    }
}

/// The `unwinding` cell (see the module documentation), if the program uses this module and
/// suspends.
pub(super) fn unwinding_cell(b: &Builder) -> Option<u32> {
    if !used(b) {
        return None;
    }
    b.sus.as_ref().map(|_| cell(b, |p| p.unwinding))
}
