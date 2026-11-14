//! Array (`[api.std.array]`). An array is an object of kind 2: slot 0 holds the length n (an
//! `i32`), and slots 1 to n hold the elements, one 8-byte word each, whatever their
//! representation (an `i32` or a reference in the low 4 bytes, an `i64` or `f64` in all 8). The
//! descriptor's `refs[0]` says whether the elements are references, so `drop` and the functions
//! here know whether to count them.
//!
//! The program never sees an array change: `array_set` writes in place only into an array whose
//! count is 1, which its caller has given up (the Heap IR moves the last reference into the call,
//! `heap::rc` in `emela-lir`); any other array is copied first. Every function borrows its
//! arguments except `array_set`, which consumes the array.

use super::*;
use emela_core::Builtin;

/// The descriptor kind of an array.
pub(crate) const KIND: u32 = 2;

pub(super) const FUNCS: &[Sig] = &[
    (
        "array_from_list",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_to_list",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_get",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_set",
        &[ValType::I32, ValType::I32, ValType::I64],
        &[ValType::I32],
    ),
    (
        "array_copy",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_slice",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_concat",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "array_from_list" => i32s(5), // n, p, a, at, refs
        "array_to_list" => i32s(3),   // head, i, cell
        "array_get" => i32s(1),       // cell
        "array_set" => i32s(5),       // n, out, shown, msg, size
        "array_copy" => i32s(3),      // out, i, refs
        "array_slice" => i32s(2),     // n, from
        "array_concat" => i32s(7),    // total, p, out, at, part, i, refs
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "array_from_list" => array_from_list(b, ins),
        "array_to_list" => array_to_list(b, ins),
        "array_get" => array_get(b, ins),
        "array_set" => array_set(b, ins),
        "array_copy" => array_copy(b, ins),
        "array_slice" => array_slice(b, ins),
        "array_concat" => array_concat(b, ins),
        _ => return false,
    }
    true
}

/// Whether the program uses an Array builtin. A program without arrays keeps the same bytes: the
/// functions here are left out, and so is the branch of `drop` for arrays.
pub(crate) fn used(b: &Builder) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let { value: Value::Builtin { op, .. }, .. } if is_array(*op)
            )
        })
    })
}

fn is_array(op: Builtin) -> bool {
    matches!(
        op,
        Builtin::ArrayFromList
            | Builtin::ArrayToList
            | Builtin::ArraySize
            | Builtin::ArrayGet
            | Builtin::ArraySet
            | Builtin::ArraySlice
            | Builtin::ArrayConcat
    )
}

/// Pushes the address of element `i` (a local, 0-based) of the array in local `a`.
pub(crate) fn elem_at(ins: &mut InstructionSink, a: u32, i: u32) {
    ins.local_get(a);
    ins.i32_const((HDR + 8) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
}

/// Pushes whether the elements of the array in local `a` are references (`refs[0]` of its
/// descriptor).
pub(crate) fn elems_are_refs(ins: &mut InstructionSink, a: u32) {
    ins.local_get(a);
    ins.i32_load(mem(8, 4));
    refs_of_desc(ins);
}

/// Replaces the descriptor on the stack with its `refs[0]`.
fn refs_of_desc(ins: &mut InstructionSink) {
    ins.i32_load(mem(DESC_REFS, 4));
    ins.i32_load8_u(mem(0, 1));
}

/// Pushes the length of the array in local `a`.
fn len(ins: &mut InstructionSink, a: u32) {
    ins.local_get(a);
    ins.i32_load(mem(HDR, 4));
}

/// `alloc(n + 1, desc)` with the length stored, into local `out`.
fn new_array(
    b: &Builder,
    ins: &mut InstructionSink,
    n: u32,
    desc: impl Fn(&mut InstructionSink),
    out: u32,
) {
    ins.local_get(n);
    ins.i32_const(1);
    ins.i32_add();
    desc(ins);
    ins.call(rt(b, "alloc"));
    ins.local_set(out);
    ins.local_get(out);
    ins.local_get(n);
    ins.i32_store(mem(HDR, 4));
}

/// Dups the reference in the low 4 bytes of the word at the address on the stack.
fn dup_at(b: &Builder, ins: &mut InstructionSink) {
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dup"));
}

/// `array_from_list(xs, desc)`: the elements of the list in order. One pass counts, the second
/// copies the heads' words.
fn array_from_list(b: &Builder, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const P: u32 = 3;
    const A: u32 = 4;
    const AT: u32 = 5;
    const REFS: u32 = 6;
    ins.local_get(1);
    refs_of_desc(ins);
    ins.local_set(REFS);
    ins.i32_const(0);
    ins.local_set(N);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    new_array(
        b,
        ins,
        N,
        |ins| {
            ins.local_get(1);
        },
        A,
    );
    ins.i32_const(0);
    ins.local_set(AT);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    elem_at(ins, A, AT);
    ins.local_get(P);
    ins.i64_load(mem(HDR, 8));
    ins.i64_store(mem(0, 8));
    ins.local_get(REFS);
    ins.if_(BlockType::Empty);
    elem_at(ins, A, AT);
    dup_at(b, ins);
    ins.end();
    ins.local_get(AT);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(AT);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(A);
}

/// `array_to_list(a, cons_desc)`: the elements as a list, built from the last element to the
/// first.
fn array_to_list(b: &Builder, ins: &mut InstructionSink) {
    const HEAD: u32 = 2;
    const I: u32 = 3;
    const CELL: u32 = 4;
    ins.i32_const(0);
    ins.local_set(HEAD);
    len(ins, 0);
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
    ins.i32_const(2);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(CELL);
    ins.local_get(CELL);
    elem_at(ins, 0, I);
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(HDR, 8));
    ins.local_get(CELL);
    ins.local_get(HEAD);
    ins.i32_store(mem(HDR + 8, 4));
    elems_are_refs(ins, 0);
    ins.if_(BlockType::Empty);
    elem_at(ins, 0, I);
    dup_at(b, ins);
    ins.end();
    ins.local_get(CELL);
    ins.local_set(HEAD);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HEAD);
}

/// `array_get(a, index, some, none)`: `None` if `index` is negative or not below the length.
fn array_get(b: &Builder, ins: &mut InstructionSink) {
    const CELL: u32 = 4;
    // Unsigned, so a negative index is out of range too.
    ins.local_get(1);
    len(ins, 0);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.return_();
    ins.end();
    ins.i32_const(1);
    ins.local_get(2);
    ins.call(rt(b, "alloc"));
    ins.local_set(CELL);
    ins.local_get(CELL);
    elem_at(ins, 0, 1);
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(HDR, 8));
    elems_are_refs(ins, 0);
    ins.if_(BlockType::Empty);
    elem_at(ins, 0, 1);
    dup_at(b, ins);
    ins.end();
    ins.local_get(CELL);
}

/// `array_set(a, index, word)`: consumes `a` and returns the array with the element at `index`
/// replaced by the value in `word` (an `i32` or a reference zero-extended, the bits of an `i64` or
/// `f64`; the value is borrowed, so a reference is dupped). In place when `a`'s count is 1;
/// otherwise a copy, and `a` is given up. A position out of range is the defect of
/// `[api.std.array]`, and `a` is given up too.
fn array_set(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 3;
    const OUT: u32 = 4;
    const SHOWN: u32 = 5;
    const MSG: u32 = 6;
    const SIZE: u32 = 7;
    len(ins, 0);
    ins.local_set(N);
    // Unsigned, so a negative index is out of range too.
    ins.local_get(1);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    let prefix = b.string_literal(b"Array.set was given position ");
    let middle = b.string_literal(b" in an array of size ");
    ins.local_get(1);
    ins.call(rt(b, "int_to_string"));
    ins.local_set(SHOWN);
    ins.i32_const(prefix as i32);
    ins.local_get(SHOWN);
    ins.call(rt(b, "str_append"));
    ins.local_set(MSG);
    ins.local_get(SHOWN);
    ins.call(rt(b, "drop"));
    ins.local_get(MSG);
    ins.i32_const(middle as i32);
    ins.call(rt(b, "str_append"));
    ins.local_set(SHOWN);
    ins.local_get(MSG);
    ins.call(rt(b, "drop"));
    ins.local_get(N);
    ins.call(rt(b, "int_to_string"));
    ins.local_set(SIZE);
    ins.local_get(SHOWN);
    ins.local_get(SIZE);
    ins.call(rt(b, "str_append"));
    ins.local_set(MSG);
    // `raise` and `str_append` borrow their arguments.
    ins.local_get(MSG);
    ins.call(rt(b, "raise"));
    for l in [SHOWN, SIZE, MSG] {
        ins.local_get(l);
        ins.call(rt(b, "drop"));
    }
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The last reference: write in place. Static data (a count of 0) is never written.
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_set(OUT);
    ins.else_();
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(N);
    ins.call(rt(b, "array_copy"));
    ins.local_set(OUT);
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.end();
    elems_are_refs(ins, OUT);
    ins.if_(BlockType::Empty);
    // The new value's reference first, then the old element's, which may be the same object.
    ins.local_get(2);
    ins.i32_wrap_i64();
    ins.call(rt(b, "dup"));
    elem_at(ins, OUT, 1);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "drop"));
    ins.end();
    elem_at(ins, OUT, 1);
    ins.local_get(2);
    ins.i64_store(mem(0, 8));
    ins.local_get(OUT);
}

/// `array_copy(a, from, n)`: a new array of the `n` elements of `a` from `from`, with the same
/// descriptor, each reference dupped.
fn array_copy(b: &Builder, ins: &mut InstructionSink) {
    const OUT: u32 = 3;
    const I: u32 = 4;
    const REFS: u32 = 5;
    elems_are_refs(ins, 0);
    ins.local_set(REFS);
    new_array(
        b,
        ins,
        2,
        |ins| {
            ins.local_get(0);
            ins.i32_load(mem(8, 4));
        },
        OUT,
    );
    // Advance `a` by `from` elements, so element i of the copy is element i of the view.
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_set(0);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(2);
    ins.i32_ge_u();
    ins.br_if(1);
    elem_at(ins, OUT, I);
    elem_at(ins, 0, I);
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(0, 8));
    ins.local_get(REFS);
    ins.if_(BlockType::Empty);
    elem_at(ins, OUT, I);
    dup_at(b, ins);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(OUT);
}

/// Pushes local `v` clamped to 0 to local `n`.
fn clamp(ins: &mut InstructionSink, v: u32, n: u32) {
    ins.local_get(v);
    ins.i32_const(0);
    ins.local_get(v);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.select();
    ins.local_tee(v);
    ins.local_get(n);
    ins.local_get(v);
    ins.local_get(n);
    ins.i32_lt_s();
    ins.select();
}

/// `array_slice(a, from, to)`: both ends clamped to 0 to the length; empty if `from` is not less
/// than `to`. Always a new array.
fn array_slice(b: &Builder, ins: &mut InstructionSink) {
    const N: u32 = 3;
    const FROM: u32 = 4;
    len(ins, 0);
    ins.local_set(N);
    clamp(ins, 1, N);
    ins.local_set(FROM);
    clamp(ins, 2, N);
    ins.local_set(2);
    ins.local_get(0);
    ins.local_get(FROM);
    // The length: `to - from`, or 0 if `to` is not above `from`.
    ins.local_get(2);
    ins.local_get(FROM);
    ins.i32_sub();
    ins.i32_const(0);
    ins.local_get(2);
    ins.local_get(FROM);
    ins.i32_gt_s();
    ins.select();
    ins.call(rt(b, "array_copy"));
}

/// `array_concat(parts, desc)`: one pass for the total length, then one that copies.
fn array_concat(b: &Builder, ins: &mut InstructionSink) {
    const TOTAL: u32 = 2;
    const P: u32 = 3;
    const OUT: u32 = 4;
    const AT: u32 = 5;
    const PART: u32 = 6;
    const I: u32 = 7;
    const REFS: u32 = 8;
    ins.local_get(1);
    refs_of_desc(ins);
    ins.local_set(REFS);
    ins.i32_const(0);
    ins.local_set(TOTAL);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(TOTAL);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(HDR, 4));
    ins.i32_add();
    ins.local_set(TOTAL);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    new_array(
        b,
        ins,
        TOTAL,
        |ins| {
            ins.local_get(1);
        },
        OUT,
    );
    ins.i32_const(0);
    ins.local_set(AT);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(PART);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    len(ins, PART);
    ins.i32_ge_u();
    ins.br_if(1);
    elem_at(ins, OUT, AT);
    elem_at(ins, PART, I);
    ins.i64_load(mem(0, 8));
    ins.i64_store(mem(0, 8));
    ins.local_get(REFS);
    ins.if_(BlockType::Empty);
    elem_at(ins, OUT, AT);
    dup_at(b, ins);
    ins.end();
    ins.local_get(AT);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(AT);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(OUT);
}
