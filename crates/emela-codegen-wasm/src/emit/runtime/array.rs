//! Array (`[api.std.array]`). An array is an object of kind 2: slot 0 holds the length n (an
//! `i32`), then n elements. An element is one 8-byte word by default (an `i32` or a reference in
//! the low 4 bytes, an `i64` or `f64` in all 8), or several slots when the element is an
//! all-scalar record or tuple stored inline (E2). The descriptor's `refs[0]` says whether the
//! elements are references, and its field after the usual descriptor size is the element stride
//! in 8-byte slots. Scalar elements keep a stride of 1, so their arrays behave as before.
//!
//! The program never sees an array change: `array_set` writes in place only into an array whose
//! count is 1, which its caller has given up (the Heap IR moves the last reference into the call,
//! `heap::rc` in `emela-lir`); any other array is copied first. Every function borrows its
//! arguments except `array_set` (and `array_set_agg`), which consumes the array.
//!
//! The `_agg` functions handle an inline all-scalar element. The IR still represents such an
//! element as a reference, so `array_get_agg`, `array_get_or_agg`, `array_load_unchecked_agg`
//! and `array_to_list_agg` allocate a box of the element's slots; construction, `set`, `slice`
//! and `concat` copy the box's slots into the array. The emitter does not call
//! `array_get_or_agg` or `array_load_unchecked_agg` when the read's destination is held by value
//! (E2 step 3): it loads each scalar leaf straight into the destination's locals, with no box.
//! `Array.get` keeps `array_get_agg` because its `Option` payload is one reference, and
//! `Array.to_list` keeps `array_to_list_agg` because a list head is a reference. Elements with a
//! reference have no inline form (there is no per-field drop map).

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    (
        "array_from_list",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("array_new", &[ValType::I64, ValType::I32], &[ValType::I32]),
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
    // The aggregate variants (E2): the element is `stride` slots stored inline, so a read through
    // these helpers makes a box and a construction copies the box's slots. The emitter bypasses
    // `array_get_or_agg` and `array_load_unchecked_agg` for a by-value destination (E2 step 3).
    (
        "array_from_list_agg",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_to_list_agg",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_get_agg",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        &[ValType::I32],
    ),
    (
        "array_set_agg",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_get_or_agg",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "array_load_unchecked_agg",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "array_from_list" => i32s(5),          // n, p, a, at, refs
        "array_new" => i32s(2),                // n, out
        "array_to_list" => i32s(3),            // head, i, cell
        "array_get" => i32s(1),                // cell
        "array_set" => i32s(5),                // n, out, shown, msg, size
        "array_copy" => i32s(4),               // out, i, refs, stride
        "array_slice" => i32s(2),              // n, from
        "array_concat" => i32s(7),             // total, p, out, at, part, i, refs
        "array_from_list_agg" => i32s(4),      // n, p, a, at
        "array_to_list_agg" => i32s(4),        // head, i, cell, box
        "array_get_agg" => i32s(2),            // cell, box
        "array_set_agg" => i32s(5),            // n, out, shown, msg, size
        "array_get_or_agg" => i32s(1),         // box
        "array_load_unchecked_agg" => i32s(1), // box
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "array_from_list" => array_from_list(b, ins),
        "array_new" => array_new(b, ins),
        "array_to_list" => array_to_list(b, ins),
        "array_get" => array_get(b, ins),
        "array_set" => array_set(b, ins),
        "array_copy" => array_copy(b, ins),
        "array_slice" => array_slice(b, ins),
        "array_concat" => array_concat(b, ins),
        "array_from_list_agg" => array_from_list_agg(b, ins),
        "array_to_list_agg" => array_to_list_agg(b, ins),
        "array_get_agg" => array_get_agg(b, ins),
        "array_set_agg" => array_set_agg(b, ins),
        "array_get_or_agg" => array_get_or_agg(b, ins),
        "array_load_unchecked_agg" => array_load_unchecked_agg(b, ins),
        _ => return false,
    }
    true
}

/// Pushes the address of element `i` (a local, 0-based) of the array in local `a`.
///
/// The element stride is the array descriptor's (E2): one slot for a scalar, the scalar leaves of
/// an all-scalar record or tuple stored inline.
pub(crate) fn elem_at(ins: &mut InstructionSink, a: u32, i: u32) {
    ins.local_get(a);
    ins.i32_const((HDR + 8) as i32);
    ins.i32_add();
    ins.local_get(i);
    stride_of(ins, a);
    ins.i32_mul();
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
}

/// Pushes the element stride of the array in local `a`, in 8-byte slots.
fn stride_of(ins: &mut InstructionSink, a: u32) {
    ins.local_get(a);
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_STRIDE, 4));
}

/// Pushes the byte size of one element of the array in local `a`.
fn elem_bytes(ins: &mut InstructionSink, a: u32) {
    stride_of(ins, a);
    ins.i32_const(3);
    ins.i32_shl();
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
    // The block has one length slot and `n * stride` element slots. The descriptor is kept in
    // `out` while its stride is read.
    ins.local_get(n);
    desc(ins);
    ins.local_tee(out);
    ins.i32_load(mem(DESC_STRIDE, 4));
    ins.i32_mul();
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(out);
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

/// The largest element count an array can hold in 8-byte slots: the length slot plus the
/// elements, so that `alloc` computes the block size in an i32 (issue #494).
const MAX_ELEMS: i64 = (i32::MAX as i64 - HDR as i64) / 8 - 1;

/// `array_new(n, desc)`: a new array of `n` elements, uninitialized. `$Array.map_range` writes
/// each element before the next call; a defect from the mapped function stops the loop there,
/// shortens the array to the elements already written and releases it, so no uninitialized slot is
/// ever released. An element type with no representation writes nothing and has no reference
/// elements to drop.
///
/// `n` is an i64 because `hi - lo` of an `Int` range is up to 2^32. A count that no wasm32 object
/// can hold is out of memory, like a failed `heap_reserve`: `alloc` would otherwise compute a
/// wrapped i32 block size, store a negative length and let the fill loop write past the block.
/// The bound counts the element slots with the descriptor's stride (E2).
fn array_new(b: &Builder, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const OUT: u32 = 3;
    // `n < 0`, or `n * stride + 1` slots (the length and the elements) past the largest i32
    // block: neither can be an array.
    ins.local_get(0);
    ins.i64_const(0);
    ins.i64_lt_s();
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_load(mem(DESC_STRIDE, 4));
    ins.i64_extend_i32_u();
    ins.i64_mul();
    ins.i64_const(MAX_ELEMS);
    ins.i64_gt_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    super::alloc::out_of_memory_loud(b, ins);
    ins.end();
    ins.local_get(0);
    ins.i32_wrap_i64();
    ins.local_set(N);
    new_array(
        b,
        ins,
        N,
        |ins| {
            ins.local_get(1);
        },
        OUT,
    );
    ins.local_get(OUT);
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
    // The element bytes are one contiguous run (an inline aggregate occupies several slots).
    // The source keeps the array pointer: `elem_at` reads the descriptor through it, so the
    // view's start is an offset in the address, not an advanced base.
    ins.local_get(OUT);
    ins.i32_const((HDR + 8) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_const((HDR + 8) as i32);
    ins.i32_add();
    ins.local_get(1);
    stride_of(ins, 0);
    ins.i32_mul();
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_get(2);
    stride_of(ins, 0);
    ins.i32_mul();
    ins.i32_const(3);
    ins.i32_shl();
    ins.memory_copy(0, 0);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(2);
    ins.i32_ge_u();
    ins.br_if(1);
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
    elem_bytes(ins, OUT);
    ins.memory_copy(0, 0);
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

/// Allocates a box of the inline element's slots from the descriptor in `elem`, into local `out`.
fn alloc_box(b: &Builder, ins: &mut InstructionSink, elem: u32, out: u32) {
    ins.local_get(elem);
    ins.i32_load(mem(DESC_NSLOTS, 4));
    ins.local_get(elem);
    ins.call(rt(b, "alloc"));
    ins.local_set(out);
}

/// `array_from_list_agg(xs, desc)`: like `array_from_list`, but the element is an all-scalar
/// record or tuple stored inline, so the list head (a box) is copied into the element slots.
fn array_from_list_agg(b: &Builder, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const P: u32 = 3;
    const A: u32 = 4;
    const AT: u32 = 5;
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
    // The list head is the box of the record, not the element bytes: follow the reference in
    // slot 0 to the box's first slot.
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_bytes(ins, A);
    ins.memory_copy(0, 0);
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

/// `array_to_list_agg(a, cons_desc, elem_desc)`: the elements as a list, each read into a new box.
fn array_to_list_agg(b: &Builder, ins: &mut InstructionSink) {
    const HEAD: u32 = 3;
    const I: u32 = 4;
    const CELL: u32 = 5;
    const BOX: u32 = 6;
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
    alloc_box(b, ins, 2, BOX);
    ins.local_get(BOX);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_at(ins, 0, I);
    elem_bytes(ins, 0);
    ins.memory_copy(0, 0);
    ins.i32_const(2);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(CELL);
    ins.local_get(CELL);
    ins.local_get(BOX);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(CELL);
    ins.local_get(HEAD);
    ins.i32_store(mem(HDR + 8, 4));
    ins.local_get(CELL);
    ins.local_set(HEAD);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HEAD);
}

/// `array_get_agg(a, index, elem_desc, some, none)`: `None` if `index` is negative or not below
/// the length, otherwise a new box of the inline element.
fn array_get_agg(b: &Builder, ins: &mut InstructionSink) {
    const CELL: u32 = 5;
    const BOX: u32 = 6;
    // Unsigned, so a negative index is out of range too.
    ins.local_get(1);
    len(ins, 0);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(4);
    ins.return_();
    ins.end();
    alloc_box(b, ins, 2, BOX);
    ins.local_get(BOX);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_at(ins, 0, 1);
    elem_bytes(ins, 0);
    ins.memory_copy(0, 0);
    // The `Some` cell holds the box as a reference, like `em_some(box, true)` in C.
    ins.i32_const(1);
    ins.local_get(3);
    ins.call(rt(b, "alloc"));
    ins.local_set(CELL);
    ins.local_get(CELL);
    ins.local_get(BOX);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(CELL);
}

/// `array_set_agg(a, index, value)`: consumes `a` and replaces element `index` with the bytes of
/// the box in `value`. In place when `a`'s count is 1; otherwise a copy, and `a` is given up. A
/// position out of range is the defect of `[api.std.array]`, and `a` is given up too.
fn array_set_agg(b: &mut Builder, ins: &mut InstructionSink) {
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
    elem_at(ins, OUT, 1);
    ins.local_get(2);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_bytes(ins, OUT);
    ins.memory_copy(0, 0);
    ins.local_get(OUT);
}

/// `array_get_or_agg(a, index, d, elem_desc)`: the boxed element when `index` is in range, and
/// otherwise a reference of its own to `d` (which is borrowed).
fn array_get_or_agg(b: &Builder, ins: &mut InstructionSink) {
    const BOX: u32 = 4;
    // Unsigned, so a negative index is out of range too.
    ins.local_get(1);
    len(ins, 0);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.call(rt(b, "dup"));
    ins.local_get(2);
    ins.return_();
    ins.end();
    alloc_box(b, ins, 3, BOX);
    ins.local_get(BOX);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_at(ins, 0, 1);
    elem_bytes(ins, 0);
    ins.memory_copy(0, 0);
    ins.local_get(BOX);
}

/// `array_load_unchecked_agg(a, index, elem_desc)`: a new box of the element. The caller has
/// already checked the bounds.
fn array_load_unchecked_agg(b: &Builder, ins: &mut InstructionSink) {
    const BOX: u32 = 3;
    alloc_box(b, ins, 2, BOX);
    ins.local_get(BOX);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    elem_at(ins, 0, 1);
    elem_bytes(ins, 0);
    ins.memory_copy(0, 0);
    ins.local_get(BOX);
}
