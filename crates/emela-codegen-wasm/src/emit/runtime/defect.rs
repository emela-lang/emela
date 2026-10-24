//! Defects and retirement.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("raise", &[ValType::I32], &[]),
    ("defect_take", &[], &[ValType::I32]),
    ("defect_resume", &[ValType::I32], &[]),
    ("retire", &[ValType::I32, ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "raise" => i32s(2),
        "defect_take" => i32s(1),
        "defect_resume" => i32s(2),
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "raise" => raise(b, ins),
        "defect_take" => defect_take(b, ins),
        "defect_resume" => defect_resume(b, ins),
        "retire" => retire(b, ins),
        _ => return false,
    }
    true
}

// ---- defects ----

fn raise(b: &mut Builder, ins: &mut InstructionSink) {
    const FIRST: u32 = 1;
    const NOTE: u32 = 2;
    let dup = rt(b, "dup");
    let drop = rt(b, "drop");
    let note = rt(b, "note_release");
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(dup);
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    // A defect raised while one is already pending (spec 14.4): the first stays the result and
    // the later one becomes a note of it. `local 0` stays the caller's. The note keeps the
    // first's self-reference marker (`aux`), so replacing the slot cannot lose it.
    ins.global_get(G_DEFECT);
    ins.local_set(FIRST);
    ins.local_get(FIRST);
    ins.local_get(0);
    ins.call(note);
    ins.local_set(NOTE);
    ins.local_get(NOTE);
    ins.global_set(G_DEFECT);
    ins.local_get(FIRST);
    ins.call(drop);
    ins.end();
}

fn defect_take(b: &mut Builder, ins: &mut InstructionSink) {
    ins.global_get(G_DEFECT);
    ins.local_set(0);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    // With nothing pending the result is an empty string, so `defect_resume` always gets a
    // defect to put back or to add a note to.
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.call(rt(b, "str_new"));
    ins.local_set(0);
    ins.end();
    ins.local_get(0);
}

fn defect_resume(b: &mut Builder, ins: &mut InstructionSink) {
    const LATER: u32 = 1;
    const NOTE: u32 = 2;
    let drop = rt(b, "drop");
    let note = rt(b, "note_release");
    ins.global_get(G_DEFECT);
    ins.local_set(LATER);
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.local_get(LATER);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // Nothing was raised by the cleanup.
    ins.return_();
    ins.end();
    // A defect raised while the first unwound (spec 14.4): the first (`local 0`) stays the
    // result, and the later one is appended to its message as a note. The note keeps the first's
    // self-reference marker (`aux`).
    ins.local_get(0);
    ins.local_get(LATER);
    ins.call(note);
    ins.local_set(NOTE);
    ins.local_get(NOTE);
    ins.global_set(G_DEFECT);
    ins.local_get(0);
    ins.call(drop);
    ins.local_get(LATER);
    ins.call(drop);
}

fn retire(b: &mut Builder, ins: &mut InstructionSink) {
    let dup = rt(b, "dup");
    let drop = rt(b, "drop");
    // Mark the handler value released (spec 8.6): slot 0 is the flag.
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(HDR, 4));
    // Remember the name for the note of a release that fails while another defect unwinds.
    ins.local_get(1);
    ins.call(dup);
    ins.global_get(G_LAST_RETIRED);
    ins.call(drop);
    ins.local_get(1);
    ins.global_set(G_LAST_RETIRED);
}
