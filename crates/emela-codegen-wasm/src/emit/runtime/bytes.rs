//! Bytes (`[api.std.bytes]`). A Bytes is a string object (the byte length, then the bytes) whose
//! bytes need not be UTF-8, so `size`, `encode_utf8` and `decode_utf8` of checked bytes are those
//! of a String (`builtins.rs`) and the functions here only read and write bytes. Nothing changes a
//! Bytes after it is made.

use super::string::append_cell;
use super::*;
use emela_core::Builtin;

pub(super) const FUNCS: &[Sig] = &[
    (
        "bytes_from_list",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("bytes_to_list", &[ValType::I32], &[ValType::I32]),
    (
        "bytes_get",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "bytes_slice",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("bytes_concat", &[ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "bytes_from_list" => i32s(5), // n, p, x, out, i
        "bytes_to_list" => i32s(5),   // head, last, i, n, cell
        "bytes_get" => i32s(1),       // cell
        "bytes_slice" => i32s(2),     // n, from
        "bytes_concat" => i32s(3),    // total, p, out
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "bytes_from_list" => bytes_from_list(b, ins),
        "bytes_to_list" => bytes_to_list(b, ins),
        "bytes_get" => bytes_get(b, ins),
        "bytes_slice" => bytes_slice(b, ins),
        "bytes_concat" => bytes_concat(b, ins),
        _ => return false,
    }
    true
}

/// Whether the program uses a Bytes builtin that is implemented here. A program without Bytes
/// keeps the same bytes.
pub(super) fn used(b: &Builder) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let { value: Value::Builtin { op, .. }, .. } if implemented(*op)
            )
        })
    })
}

fn implemented(op: Builtin) -> bool {
    matches!(
        op,
        Builtin::BytesFromList
            | Builtin::BytesToList
            | Builtin::BytesGet
            | Builtin::BytesSlice
            | Builtin::BytesConcat
    )
}

/// The address of byte `i` (a local) of the string object in local `s`.
fn byte_at(ins: &mut InstructionSink, s: u32, i: u32) {
    ins.local_get(s);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_add();
}

/// `Bytes.from_list(xs, some, none)`: `None` if an element is outside 0 to 255. One pass checks
/// and counts, the second writes the bytes.
fn bytes_from_list(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 3;
    const P: u32 = 4;
    const X: u32 = 5;
    const OUT: u32 = 6;
    const I: u32 = 7;
    ins.i32_const(0);
    ins.local_set(N);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(X);
    // Unsigned, so a negative element is above 255 too.
    ins.local_get(X);
    ins.i32_const(255);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
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
    ins.local_get(N);
    ins.call(rt(b, "str_new"));
    ins.local_set(OUT);
    ins.i32_const(0);
    ins.local_set(I);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    byte_at(ins, OUT, I);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    // `Some(the bytes)`.
    ins.i32_const(1);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(OUT);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
}

/// `Bytes.to_list(b)`: the bytes as a list of Int, front to back.
fn bytes_to_list(b: &mut Builder, ins: &mut InstructionSink) {
    const HEAD: u32 = 1;
    const LAST: u32 = 2;
    const I: u32 = 3;
    const N: u32 = 4;
    const CELL: u32 = 5;
    let cons_val = b.cons_val_desc;
    ins.i32_const(0);
    ins.local_set(HEAD);
    ins.i32_const(0);
    ins.local_set(LAST);
    ins.i32_const(0);
    ins.local_set(I);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    byte_at(ins, 0, I);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(0);
    ins.i32_const(cons_val as i32);
    ins.call(rt(b, "cons"));
    ins.local_set(CELL);
    append_cell(ins, HEAD, LAST, CELL);
    ins.local_get(CELL);
    ins.local_set(LAST);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HEAD);
}

/// `Bytes.get(b, index, some, none)`: `None` if `index` is negative or not below the size.
fn bytes_get(b: &mut Builder, ins: &mut InstructionSink) {
    const CELL: u32 = 4;
    // Unsigned, so a negative index is out of range too.
    ins.local_get(1);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
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
    byte_at(ins, 0, 1);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store(mem(HDR, 4));
    ins.local_get(CELL);
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

/// `Bytes.slice(b, from, to)`: both ends clamped to 0 to the size; empty if `from` is not less
/// than `to`. Always a new object, so the result never holds more than its own bytes.
fn bytes_slice(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 3;
    const FROM: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    clamp(ins, 1, N);
    ins.local_set(FROM);
    clamp(ins, 2, N);
    ins.local_set(2);
    // The length: `to - from`, or 0 if `to` is not above `from`.
    ins.local_get(0);
    ins.local_get(FROM);
    ins.local_get(2);
    ins.local_get(FROM);
    ins.i32_sub();
    ins.i32_const(0);
    ins.local_get(2);
    ins.local_get(FROM);
    ins.i32_gt_s();
    ins.select();
    ins.call(rt(b, "str_slice"));
}

/// `Bytes.concat(parts)`: one pass for the total size, then one that copies.
fn bytes_concat(b: &mut Builder, ins: &mut InstructionSink) {
    const TOTAL: u32 = 1;
    const P: u32 = 2;
    const OUT: u32 = 3;
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
    ins.local_get(TOTAL);
    ins.call(rt(b, "str_new"));
    ins.local_set(OUT);
    // TOTAL counts the bytes copied so far from here on.
    ins.i32_const(0);
    ins.local_set(TOTAL);
    ins.local_get(0);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    // copy_range(dst, dst_off, src, src_off, len)
    ins.local_get(OUT);
    ins.local_get(TOTAL);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(0);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "copy_range"));
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
    ins.local_get(OUT);
}
