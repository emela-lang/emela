//! The memoisation of a const (spec 5.6).
//!
//! A const's wrapper (`emit::Builder::const_wrapper`) returns the remembered value itself and
//! otherwise asks `const_enter` whether to evaluate the body. The first reference evaluates it and the wrapper stores the value in the cell, then calls
//! `const_leave`; a failure is wrapped as `while evaluating const NAME: …` and remembered, so that
//! later references raise the same defect without evaluating again. A reference to the const
//! while it is being evaluated is a self-reference: it raises a defect that no const remembers.
//!
//! A cell is static data (see `CONST_*` in `emit`): state, value, defect, and the self-reference
//! and wrapped-defect messages. Its state is 0 (not evaluated), 1 (being evaluated), 2 (the value)
//! or 3 (the defect). References keep their own count: the cell owns one and each use gets a
//! duplicate.
//!
//! The self-reference message has 1 in its `aux` word (bit 0; the bits above it are the location
//! of a failed assert, see `defect`). The marker travels with the defect
//! (`defect_take`, a release note and `defect_resume` all keep the string), unlike a global flag:
//! nested cleanups can take and restore several defects.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("const_enter", &[ValType::I32], &[ValType::I32]),
    ("const_leave", &[ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "const_enter" => i32s(1),
        "const_leave" => i32s(1),
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "const_enter" => const_enter(b, ins),
        "const_leave" => const_leave(b, ins),
        _ => return false,
    }
    true
}

/// `(cell) -> i32` for a cell without a value: 0 to evaluate the body, 1 for the remembered (or
/// self-reference) defect, which is raised here.
fn const_enter(b: &mut Builder, ins: &mut InstructionSink) {
    const STATE: u32 = 1;
    // state = cell.state
    ins.local_get(0);
    ins.i32_load(mem(CONST_STATE, 4));
    ins.local_set(STATE);
    ins.local_get(STATE);
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    // Remembered defect: raise it; the cell keeps its own reference.
    ins.local_get(0);
    ins.i32_load(mem(CONST_DEFECT, 4));
    let raise = rt(b, "raise");
    ins.call(raise);
    ins.i32_const(1);
    ins.else_();
    ins.local_get(STATE);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    // Self-reference: the message's `aux` marker keeps every const from wrapping or remembering
    // it, even when a release adds a note during the unwind.
    ins.local_get(0);
    ins.i32_load(mem(CONST_SELF, 4));
    ins.call(raise);
    ins.i32_const(1);
    ins.else_();
    // Not evaluated yet: mark it and evaluate.
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(CONST_STATE, 4));
    ins.i32_const(0);
    ins.end();
    ins.end();
}

/// `(cell) -> ()`: after the body, remember its value or wrap and remember the pending defect.
fn const_leave(b: &mut Builder, ins: &mut InstructionSink) {
    const NEW: u32 = 1;
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The wrapper stored the value (and gave the caller its own reference).
    ins.local_get(0);
    ins.i32_const(2);
    ins.i32_store(mem(CONST_STATE, 4));
    ins.return_();
    ins.end();
    // A self-reference is not remembered: it would be raised again on every reference anyway.
    // The marker is the defect string's `aux` word (see the module comment).
    ins.global_get(G_DEFECT);
    ins.i32_load(mem(4, 4));
    ins.i32_const(1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_store(mem(CONST_STATE, 4));
    ins.return_();
    ins.end();
    // `while evaluating const NAME: <defect>`, which becomes the remembered defect.
    ins.local_get(0);
    ins.i32_load(mem(CONST_PREFIX, 4));
    ins.global_get(G_DEFECT);
    let append = rt(b, "str_append");
    ins.call(append);
    ins.local_set(NEW);
    // The remembered defect keeps the location of a failed assert (`aux`).
    ins.local_get(NEW);
    ins.global_get(G_DEFECT);
    ins.i32_load(mem(4, 4));
    ins.i32_store(mem(4, 4));
    ins.global_get(G_DEFECT);
    let drop = rt(b, "drop");
    ins.call(drop);
    ins.local_get(NEW);
    ins.global_set(G_DEFECT);
    // The cell keeps its own reference.
    ins.local_get(NEW);
    let dup = rt(b, "dup");
    ins.call(dup);
    ins.local_get(0);
    ins.local_get(NEW);
    ins.i32_store(mem(CONST_DEFECT, 4));
    ins.local_get(0);
    ins.i32_const(3);
    ins.i32_store(mem(CONST_STATE, 4));
}
