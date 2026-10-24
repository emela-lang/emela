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
        "defect_take" => i32s(1),
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "raise" => raise(b, ins),
        "defect_take" => defect_take(ins),
        "defect_resume" => defect_resume(b, ins),
        "retire" => retire(ins),
        _ => return false,
    }
    true
}

// ---- defects ----

fn raise(b: &mut Builder, ins: &mut InstructionSink) {
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    // A defect raised while one is already pending (spec 14.4): the first stays the result and
    // the later one becomes a note. Notes are not recorded yet (`[target.wasm.unsupported]`), so
    // release it.
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.end();
}

fn defect_take(ins: &mut InstructionSink) {
    ins.global_get(G_DEFECT);
    ins.local_set(0);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.local_get(0);
}

fn defect_resume(b: &mut Builder, ins: &mut InstructionSink) {
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // Nothing was raised by the cleanup: put the taken defect back.
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    // A defect raised while the first unwound (spec 14.4): the first (`local 0`) stays the
    // result. The later one becomes a note; notes are not recorded yet
    // (`[target.wasm.unsupported]`), so release it and restore the first.
    ins.global_get(G_DEFECT);
    ins.call(rt(b, "drop"));
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.end();
}

fn retire(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
}
