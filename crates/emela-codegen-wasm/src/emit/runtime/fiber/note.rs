//! The notes of the other failures of a group, added to its first cause (`[task.group]`).

use super::*;

/// `fb_note(first, other)`: add `note: another task also failed: ...` to the first cause
/// (`[task.group]`). Both causes are borrowed.
pub(super) fn note(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SHOWN: u32 = 2;
    const NOTE: u32 = 3;
    const D: u32 = 4;
    const TMP: u32 = 5;
    let prefix = plan(b).note_other;
    let newline = b.string_literal(b"\n");
    g(ins, 1);
    let string = b.rt("fb_cause_string");
    ins.call(string);
    ins.local_set(SHOWN);
    c(ins, prefix as i32);
    g(ins, SHOWN);
    let append = b.rt("str_append");
    ins.call(append);
    ins.local_set(NOTE);
    dropv(b, ins, SHOWN);
    ldl(ins, 0, 4);
    c(ins, CAUSE_FAIL);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The error slot of the first cause (the note is keyed by the error it becomes).
    g(ins, 0);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, NOTE);
    let add = b.rt("fb_notes_add");
    ins.call(add);
    ins.else_();
    ldl(ins, 0, 4);
    c(ins, CAUSE_DIE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, 0, HDR);
    ins.local_set(D);
    g(ins, D);
    c(ins, newline as i32);
    ins.call(append);
    ins.local_set(TMP);
    g(ins, TMP);
    g(ins, NOTE);
    ins.call(append);
    ins.local_set(D);
    // The defect keeps its markers (`aux`).
    g(ins, D);
    ldl(ins, 0, HDR);
    ins.i32_load(mem(4, 4));
    ins.i32_store(mem(4, 4));
    // Drop the old defect and the intermediate string.
    g(ins, TMP);
    call(b, ins, "drop");
    ldl(ins, 0, HDR);
    call(b, ins, "drop");
    stl(ins, 0, HDR, D);
    ins.end();
    ins.end();
    dropv(b, ins, NOTE);
}

/// `fb_notes_of(error) -> String`: the notes of a failure (a new reference), or 0.
pub(super) fn notes_of(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 1;
    let head = plan(b).notes;
    // A program without the fiber runtime has no notes cell; 0 is the scratch area, not a list.
    // The check is compile-time so the emitted function never reads address 0.
    if head == 0 {
        c(ins, 0);
        ins.return_();
        return;
    }
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, E);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, E, HDR);
    g(ins, 0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, E, HDR + 8);
    call(b, ins, "dup");
    ldl(ins, E, HDR + 8);
    ins.return_();
    ins.end();
    ldl(ins, E, HDR + 16);
    ins.local_set(E);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// `fb_notes_add(slot, note)`: attach a note to the error held in `slot` (a cause's error slot).
/// `slot` is updated to a copy when the error is static (a constructor with `rc == 0`), so the
/// notes stay with this failure; `note` is borrowed.
pub(super) fn notes_add(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 2;
    const COPY: u32 = 3;
    const N: u32 = 4;
    const LINE: u32 = 5;
    const NEXT: u32 = 6;
    const I: u32 = 7;
    const D: u32 = 8;
    let head = plan(b).notes;
    let desc = plan(b).note_desc;
    let append = b.rt("str_append");
    // A static error is copied first, and the cause's slot is updated, so later notes of this
    // failure find the copy (the C runtime's `em_notes_add`).
    ldl(ins, 0, 0);
    ins.local_set(E);
    ldl(ins, E, 0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ldl(ins, E, 8);
    ins.local_set(D);
    ldl(ins, D, DESC_NSLOTS);
    ins.local_set(N);
    g(ins, N);
    g(ins, D);
    call(b, ins, "alloc");
    ins.local_set(COPY);
    c(ins, 0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, I);
    g(ins, N);
    ins.i32_ge_u();
    ins.br_if(1);
    g(ins, COPY);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    g(ins, E);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(0, 8));
    ldl(ins, D, DESC_REFS);
    g(ins, I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    g(ins, E);
    c(ins, HDR as i32);
    ins.i32_add();
    g(ins, I);
    c(ins, 8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    call(b, ins, "dup");
    ins.end();
    g(ins, I);
    c(ins, 1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    g(ins, COPY);
    ins.i32_store(mem(0, 4));
    g(ins, COPY);
    ins.local_set(E);
    ins.end();
    // An existing entry: append a line, releasing the note string it replaces.
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(NEXT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, NEXT);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, NEXT, HDR);
    g(ins, E);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ldl(ins, NEXT, HDR + 8);
    let nl = b.string_literal(b"\n");
    c(ins, nl as i32);
    ins.call(append);
    ins.local_set(LINE);
    g(ins, LINE);
    g(ins, 1);
    ins.call(append);
    ins.local_set(I);
    g(ins, LINE);
    call(b, ins, "drop");
    ldl(ins, NEXT, HDR + 8);
    call(b, ins, "drop");
    stl(ins, NEXT, HDR + 8, I);
    ins.return_();
    ins.end();
    ldl(ins, NEXT, HDR + 16);
    ins.local_set(NEXT);
    ins.br(0);
    ins.end();
    ins.end();
    // Push the entry at the head of the list.
    c(ins, 3);
    c(ins, desc as i32);
    call(b, ins, "alloc");
    ins.local_set(COPY);
    st_dup(b, ins, COPY, HDR, E);
    st_dup(b, ins, COPY, HDR + 8, 1);
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(NEXT);
    stl(ins, COPY, HDR + 16, NEXT);
    ins.i32_const(head as i32);
    g(ins, COPY);
    ins.i32_store(mem(0, 4));
}

/// `fb_notes_clear()`: release every note entry at the end of a run.
pub(super) fn notes_clear(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const E: u32 = 0;
    const NEXT: u32 = 1;
    let head = plan(b).notes;
    // No fiber runtime: nothing was ever recorded, and 0 is the scratch area. Emit no read.
    if head == 0 {
        ins.return_();
        return;
    }
    ins.i32_const(head as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    g(ins, E);
    ins.i32_eqz();
    ins.br_if(1);
    ldl(ins, E, HDR + 16);
    ins.local_set(NEXT);
    dropv(b, ins, E);
    g(ins, NEXT);
    ins.local_set(E);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(head as i32);
    c(ins, 0);
    ins.i32_store(mem(0, 4));
    let _ = (E, NEXT);
}
