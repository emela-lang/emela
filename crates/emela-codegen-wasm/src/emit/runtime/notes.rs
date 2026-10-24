//! The notes of a second defect (spec 14.4).
//!
//! If a defect is already pending, the later one does not replace it: the first stays the result
//! and the later one is appended to its message as
//!
//! ```text
//! note: the release of handler <name> also failed: <message>
//! ```
//!
//! Notes come in the order they happened, one per line. A release that raises a defect names the
//! handler retired last (`retire` stores it in `G_LAST_RETIRED`).

use super::*;

pub(super) const FUNCS: &[Sig] = &[(
    "note_release",
    &[ValType::I32, ValType::I32],
    &[ValType::I32],
)];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    i32s(2)
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "note_release" => note_release(b, ins),
        _ => return false,
    }
    true
}

/// `(first, later) -> String`: `first` with the release note for `later` appended. Neither
/// argument is consumed; the result is a new reference.
fn note_release(b: &mut Builder, ins: &mut InstructionSink) {
    const TMP: u32 = 2;
    const NEXT: u32 = 3;
    let before = b.string_literal(b"\nnote: the release of handler ");
    let after = b.string_literal(b" also failed: ");
    let append = rt(b, "str_append");
    let drop = rt(b, "drop");
    // tmp = append(first, "\nnote: the release of handler ")
    ins.local_get(0);
    ins.i32_const(before as i32);
    ins.call(append);
    ins.local_set(TMP);
    // The handler name, if a handler was retired (it always is before a release's defect).
    ins.global_get(G_LAST_RETIRED);
    ins.if_(BlockType::Empty);
    ins.local_get(TMP);
    ins.global_get(G_LAST_RETIRED);
    ins.call(append);
    ins.local_set(NEXT);
    ins.local_get(TMP);
    ins.call(drop);
    ins.local_get(NEXT);
    ins.local_set(TMP);
    ins.end();
    // tmp = append(tmp, " also failed: ")
    ins.local_get(TMP);
    ins.i32_const(after as i32);
    ins.call(append);
    ins.local_set(NEXT);
    ins.local_get(TMP);
    ins.call(drop);
    ins.local_get(NEXT);
    ins.local_set(TMP);
    // result = append(tmp, later)
    ins.local_get(TMP);
    ins.local_get(1);
    ins.call(append);
    ins.local_set(NEXT);
    ins.local_get(TMP);
    ins.call(drop);
    // Keep the first defect's self-reference marker (spec 5.6): a note does not change what the
    // first defect is. `str_new` gave `next` a zero `aux`.
    ins.local_get(NEXT);
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    ins.i32_store(mem(4, 4));
    ins.local_get(NEXT);
}
