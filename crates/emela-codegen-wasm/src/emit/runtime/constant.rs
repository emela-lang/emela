//! The values of `const`s (spec 5.6).
//!
//! A const is evaluated on its first reference and remembered, value or defect, in a static cell
//! (`emit::CELL_*`): state 0 (not evaluated yet), 1 (being evaluated), 2 (the value) or 3 (the
//! defect the evaluation raised). The wrapper the emitter generates calls [`const_enter`] to pick
//! the path, evaluates the body when it says so, and calls [`const_record`] with the value.
//!
//! A defect raised while a const evaluates is remembered with the const's name as a prefix
//! (`while evaluating const X: …`), unless it is a self-reference (`const X refers to itself while
//! being evaluated`, spec 5.6) or an interruption. A self-reference is signalled through the
//! `self_reference` global: the flag is per module because the const it happens in is not
//! necessarily the one whose `const_record` runs next (another const may be evaluating on the way
//! out), and clearing it (in `raise`) matches the C runtime's `em_self_reference`.
//!
//! This fragment is registered only when the module has a const (`Builder::uses_const`), so
//! programs without one keep the exact bytes they had.

use super::*;

/// The const fragment (see `runtime::FUNCS`), registered only when the module has a const.
pub(super) const FUNCS: &[Sig] = &[
    // `(cell, is_ref) -> i32`: 1 to evaluate the body, 0 to use the remembered value (raising the
    // remembered defect, or a self-reference defect, first).
    (
        "const_enter",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    // `(cell, value, is_ref) -> ()`: remember `value`, or prefix the pending defect with the
    // const's name and remember that (spec 5.6).
    (
        "const_record",
        &[ValType::I32, ValType::I64, ValType::I32],
        &[],
    ),
];

/// Whether `name` is a function of this fragment (registered only when the module has a const).
pub(super) fn handles(name: &str) -> bool {
    FUNCS.iter().any(|(n, ..)| *n == name)
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    match name {
        "const_enter" => Some(Vec::new()),
        // The new defect `prefix ++ defect`.
        "const_record" => Some(vec![(1, ValType::I32)]),
        _ => None,
    }
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "const_enter" => const_enter(b, ins),
        "const_record" => const_record(b, ins),
        _ => return false,
    }
    true
}

/// `const_enter(cell, is_ref) -> i32`.
fn const_enter(b: &mut Builder, ins: &mut InstructionSink) {
    let dup = rt(b, "dup");
    let raise = rt(b, "raise");
    let self_reference = b.self_reference;
    // state == 2: the value is remembered. A reference is counted out to the caller.
    ins.local_get(0);
    ins.i32_load(mem(CELL_STATE, 4));
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(CELL_VALUE, 4));
    ins.call(dup);
    ins.end();
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // state == 3: raise the remembered defect again.
    ins.local_get(0);
    ins.i32_load(mem(CELL_STATE, 4));
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(CELL_DEFECT, 4));
    ins.call(raise);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // state == 1: the const refers to itself while it is evaluated. The flag is set after the
    // raise, which cleared it, so that the consts on the way out do not remember the defect.
    ins.local_get(0);
    ins.i32_load(mem(CELL_STATE, 4));
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load(mem(CELL_SELF_MSG, 4));
    ins.call(raise);
    ins.i32_const(1);
    ins.global_set(self_reference);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // state == 0: evaluate the body (and mark the const as being evaluated).
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(CELL_STATE, 4));
    ins.i32_const(1);
}

/// `const_record(cell, value, is_ref)`.
fn const_record(b: &mut Builder, ins: &mut InstructionSink) {
    let dup = rt(b, "dup");
    let drop = rt(b, "drop");
    let append = rt(b, "str_append");
    let self_reference = b.self_reference;
    // No defect: remember the value (and keep a reference if it is one). `const_enter` state 2
    // hands it back to the next caller with a `dup`.
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_store(mem(CELL_VALUE, 8));
    ins.local_get(2);
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.call(dup);
    ins.end();
    ins.local_get(0);
    ins.i32_const(2);
    ins.i32_store(mem(CELL_STATE, 4));
    ins.return_();
    ins.end();
    // A defect. A self-reference, and an interruption of the evaluation (a suspend operation
    // dropped), is not remembered: the const raises it again from scratch.
    ins.global_get(self_reference);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I32));
    if b.uses_suspend {
        ins.global_get(G_DEFECT);
        ins.call(rt(b, "is_interrupted"));
    } else {
        ins.i32_const(0);
    }
    ins.else_();
    ins.i32_const(1);
    ins.end();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_store(mem(CELL_STATE, 4));
    ins.return_();
    ins.end();
    // Remember `prefix ++ defect`, and leave it pending.
    ins.local_get(0);
    ins.i32_load(mem(CELL_PREFIX, 4));
    ins.global_get(G_DEFECT);
    ins.call(append);
    ins.local_set(3);
    ins.global_get(G_DEFECT);
    ins.call(drop);
    ins.local_get(3);
    ins.global_set(G_DEFECT);
    ins.local_get(3);
    ins.call(dup);
    ins.local_get(0);
    ins.local_get(3);
    ins.i32_store(mem(CELL_DEFECT, 4));
    ins.local_get(0);
    ins.i32_const(3);
    ins.i32_store(mem(CELL_STATE, 4));
}
