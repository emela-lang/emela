//! Builtins that build an `Option` value (`Int.parse`, `Int.checked_add`,
//! `String.from_code_point`).

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    (
        "int_parse",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "int_checked_add",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "from_code_point",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "int_parse" => vec![(5, ValType::I32), (1, ValType::I64)], // p, n, i, neg, b / v
        "int_checked_add" => vec![(1, ValType::I64), (1, ValType::I32)], // r / p
        "from_code_point" => i32s(4),                              // p, len, i, cell
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "int_parse" => int_parse(b, ins),
        "int_checked_add" => int_checked_add(b, ins),
        "from_code_point" => from_code_point(b, ins),
        _ => return false,
    }
    true
}

// ---- builtins returning `Option` ----

/// `Int.parse(s, some, none)`: decimal digits with an optional sign, within the Int range.
fn int_parse(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    const N: u32 = 4;
    const I: u32 = 5;
    const NEG: u32 = 6;
    const B: u32 = 7;
    const V: u32 = 8;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(NEG);
    // An optional sign.
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_load8_u(mem(HDR + 4, 1));
    ins.local_set(B);
    ins.local_get(B);
    ins.i32_const('+' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.else_();
    ins.local_get(B);
    ins.i32_const('-' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(NEG);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.i64_const(0);
    ins.local_set(V);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(B);
    ins.local_get(B);
    ins.i32_const('0' as i32);
    ins.i32_lt_u();
    ins.local_get(B);
    ins.i32_const('9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    // Accumulate only while it fits, so a long digit string cannot wrap the i64 and the range
    // check then rejects it (the same as the C runtime).
    ins.local_get(V);
    ins.i64_const(2147483648);
    ins.i64_le_s();
    ins.if_(BlockType::Empty);
    ins.local_get(V);
    ins.i64_const(10);
    ins.i64_mul();
    ins.local_get(B);
    ins.i32_const('0' as i32);
    ins.i32_sub();
    ins.i64_extend_i32_s();
    ins.i64_add();
    ins.local_set(V);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(NEG);
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.local_get(V);
    ins.i64_sub();
    ins.local_set(V);
    ins.end();
    ins.local_get(V);
    ins.i64_const(-2147483648);
    ins.i64_lt_s();
    ins.local_get(V);
    ins.i64_const(2147483647);
    ins.i64_gt_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.i32_const(1);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(V);
    ins.i32_wrap_i64();
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
}

/// `Int.checked_add(a, b, some, none)`.
fn int_checked_add(b: &mut Builder, ins: &mut InstructionSink) {
    const R: u32 = 4;
    const P: u32 = 5;
    ins.local_get(0);
    ins.i64_extend_i32_s();
    ins.local_get(1);
    ins.i64_extend_i32_s();
    ins.i64_add();
    ins.local_set(R);
    ins.local_get(R);
    ins.i64_const(-2147483648);
    ins.i64_lt_s();
    ins.local_get(R);
    ins.i64_const(2147483647);
    ins.i64_gt_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(3);
    ins.return_();
    ins.end();
    ins.i32_const(1);
    ins.local_get(2);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(R);
    ins.i32_wrap_i64();
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
}

/// `String.from_code_point(n, some, none)`: the UTF-8 string of a Unicode scalar value.
fn from_code_point(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    const LEN: u32 = 4;
    const I: u32 = 5;
    const CELL: u32 = 6;
    // Not a scalar value.
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.local_get(0);
    ins.i32_const(0x10FFFF);
    ins.i32_gt_s();
    ins.i32_or();
    ins.local_get(0);
    ins.i32_const(0xD800);
    ins.i32_ge_u();
    ins.local_get(0);
    ins.i32_const(0xDFFF);
    ins.i32_le_u();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    // Encode as UTF-8 into the scratch buffer.
    ins.local_get(0);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(CPBUF as i32);
    ins.local_get(0);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.local_set(LEN);
    ins.else_();
    ins.local_get(0);
    ins.i32_const(0x800);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    encode_byte(ins, CPBUF, &[6], 0xC0, 0);
    encode_byte(ins, CPBUF + 1, &[0], 0x80, 0x3F);
    ins.i32_const(2);
    ins.local_set(LEN);
    ins.else_();
    ins.local_get(0);
    ins.i32_const(0x10000);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    encode_byte(ins, CPBUF, &[12], 0xE0, 0);
    encode_byte(ins, CPBUF + 1, &[6], 0x80, 0x3F);
    encode_byte(ins, CPBUF + 2, &[0], 0x80, 0x3F);
    ins.i32_const(3);
    ins.local_set(LEN);
    ins.else_();
    encode_byte(ins, CPBUF, &[18], 0xF0, 0);
    encode_byte(ins, CPBUF + 1, &[12], 0x80, 0x3F);
    encode_byte(ins, CPBUF + 2, &[6], 0x80, 0x3F);
    encode_byte(ins, CPBUF + 3, &[0], 0x80, 0x3F);
    ins.i32_const(4);
    ins.local_set(LEN);
    ins.end();
    ins.end();
    ins.end();
    // A string of those bytes.
    ins.local_get(LEN);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_const(CPBUF as i32);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // `Some(the string)`.
    ins.i32_const(1);
    ins.local_get(1);
    ins.call(rt(b, "alloc"));
    ins.local_set(CELL);
    ins.local_get(CELL);
    ins.local_get(P);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(CELL);
}

/// Stores one UTF-8 byte at `addr`: the high bits are `n >> shift | high`, or `n & mask | high`
/// when `shift` is 0.
fn encode_byte(ins: &mut InstructionSink, addr: u32, shift: &[u32], high: u32, mask: u32) {
    ins.i32_const(addr as i32);
    ins.local_get(0);
    if let [s] = shift
        && *s > 0
    {
        ins.i32_const(*s as i32);
        ins.i32_shr_u();
    }
    if mask > 0 {
        ins.i32_const(mask as i32);
        ins.i32_and();
    }
    ins.i32_const(high as i32);
    ins.i32_or();
    ins.i32_store8(mem(0, 1));
}
