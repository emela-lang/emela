//! Showing values: strings and the integer conversions.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("show_string", &[ValType::I32], &[ValType::I32]),
    ("int_to_string", &[ValType::I32], &[ValType::I32]),
    ("int64_to_string", &[ValType::I64], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "show_string" => i32s(7),   // n, i, j, p, b, esc, dig
        "int_to_string" => i32s(6), // neg, u, i, p, len, j
        "int64_to_string" => vec![(5, ValType::I32), (1, ValType::I64)], // neg,i,p,len,j / u
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "show_string" => show_string(b, ins),
        "int_to_string" => int_to_string(b, ins),
        "int64_to_string" => int64_to_string(b, ins),
        _ => return false,
    }
    true
}

/// Shows a string inside a value: `"` at both ends, escaping `\"`, `\\`, `\n`, `\t`, `\r` and
/// `\#` (before `#{`), and any other control character as `\u{...}` (spec 10.6).
fn show_string(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    const I: u32 = 2;
    const J: u32 = 3;
    const P: u32 = 4;
    const BYTE: u32 = 5;
    const ESC: u32 = 6;
    const DIG: u32 = 7;
    // First pass: the output length.
    ins.i32_const(2);
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    src_byte(ins, I);
    ins.local_set(BYTE);
    classify_byte(ins, BYTE, I, ESC);
    // The byte itself.
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    ins.local_get(ESC);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The backslash in front of it.
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    ins.else_();
    ins.local_get(ESC);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // `\u{...}`: `\u{`, `}` and one or two hex digits.
    ins.local_get(N);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_get(BYTE);
    ins.i32_const(0x10);
    ins.i32_ge_u();
    ins.i32_add();
    ins.local_set(N);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(N);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    // Opening quote.
    ins.i32_const(0);
    ins.local_set(J);
    put_byte(ins, P, J, b'"' as i32);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    src_byte(ins, I);
    ins.local_set(BYTE);
    classify_byte(ins, BYTE, I, ESC);
    // Both escapes start with a backslash.
    ins.local_get(ESC);
    ins.i32_const(0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    put_byte(ins, P, J, b'\\' as i32);
    ins.end();
    ins.local_get(ESC);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // The second character: the control characters get a letter, the rest stand for themselves.
    ins.local_get(BYTE);
    ins.i32_const(b'\n' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_byte(ins, P, J, b'n' as i32);
    ins.else_();
    ins.local_get(BYTE);
    ins.i32_const(b'\t' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_byte(ins, P, J, b't' as i32);
    ins.else_();
    ins.local_get(BYTE);
    ins.i32_const(b'\r' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_byte(ins, P, J, b'r' as i32);
    ins.else_();
    put_local(ins, P, J, BYTE);
    ins.end();
    ins.end();
    ins.end();
    ins.else_();
    ins.local_get(ESC);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_byte(ins, P, J, b'u' as i32);
    put_byte(ins, P, J, b'{' as i32);
    // One digit for 0x0–0xF, two for 0x10–0x7F.
    ins.local_get(BYTE);
    ins.i32_const(0x10);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(BYTE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(DIG);
    put_hex_digit(ins, P, J, DIG);
    ins.end();
    ins.local_get(BYTE);
    ins.i32_const(0x0f);
    ins.i32_and();
    ins.local_set(DIG);
    put_hex_digit(ins, P, J, DIG);
    put_byte(ins, P, J, b'}' as i32);
    ins.else_();
    put_local(ins, P, J, BYTE);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Closing quote.
    put_byte(ins, P, J, b'"' as i32);
    ins.local_get(P);
}

/// Classifies the byte in local `byte` at source index `i` into local `esc`: 0 for a byte shown as
/// itself, 1 for a two-character escape, and 2 for `\u{...}` (the string is param 0).
fn classify_byte(ins: &mut InstructionSink, byte: u32, i: u32, esc: u32) {
    ins.i32_const(0);
    ins.local_set(esc);
    // `"`, `\`, `\n`, `\t` and `\r`: a backslash and one character.
    ins.local_get(byte);
    ins.i32_const(b'"' as i32);
    ins.i32_eq();
    ins.local_get(byte);
    ins.i32_const(b'\\' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.local_get(byte);
    ins.i32_const(b'\n' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.local_get(byte);
    ins.i32_const(b'\t' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.local_get(byte);
    ins.i32_const(b'\r' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(esc);
    ins.else_();
    // Any other control character (`\t`, `\n` and `\r` are above).
    ins.local_get(byte);
    ins.i32_const(0x1f);
    ins.i32_le_u();
    ins.local_get(byte);
    ins.i32_const(0x7f);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(esc);
    ins.else_();
    // `#` right before `{`, which would start an interpolation.
    ins.local_get(byte);
    ins.i32_const(b'#' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(i);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(i);
    ins.i32_const(1);
    ins.i32_add();
    src_byte_at(ins);
    ins.i32_const(b'{' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(esc);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    ins.end();
}

/// Stores the ASCII byte `value` at `P + J` and advances `J`.
fn put_byte(ins: &mut InstructionSink, p: u32, j: u32, value: i32) {
    dst_byte(ins, p, j);
    ins.i32_const(value);
    ins.i32_store8(mem(0, 1));
    ins.local_get(j);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(j);
}

/// Stores the byte in local `v` at `P + J` and advances `J`.
fn put_local(ins: &mut InstructionSink, p: u32, j: u32, v: u32) {
    dst_byte(ins, p, j);
    ins.local_get(v);
    ins.i32_store8(mem(0, 1));
    ins.local_get(j);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(j);
}

/// Stores the hex digit in local `d` (0–15) at `P + J` and advances `J`, lower case.
fn put_hex_digit(ins: &mut InstructionSink, p: u32, j: u32, d: u32) {
    dst_byte(ins, p, j);
    ins.local_get(d);
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.local_get(d);
    ins.i32_const(10);
    ins.i32_ge_u();
    ins.i32_const((b'a' - b'0' - 10) as i32);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.local_get(j);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(j);
}

/// Pushes the byte at the string's index already on the stack (the string is param 0).
fn src_byte_at(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

/// Pushes the byte at the source string's index `I` (the string is param 0).
fn src_byte(ins: &mut InstructionSink, i: u32) {
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

/// Pushes the address of the destination string's index `J` (the string is local `p`).
fn dst_byte(ins: &mut InstructionSink, p: u32, j: u32) {
    ins.local_get(p);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(j);
    ins.i32_add();
}

fn int_to_string(b: &mut Builder, ins: &mut InstructionSink) {
    const NEG: u32 = 1;
    const U: u32 = 2;
    const I: u32 = 3;
    const P: u32 = 4;
    const LEN: u32 = 5;
    const J: u32 = 6;
    ins.i32_const(0);
    ins.local_set(NEG);
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(NEG);
    ins.end();
    ins.local_get(NEG);
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.local_get(0);
    ins.i32_sub();
    ins.else_();
    ins.local_get(0);
    ins.end();
    ins.local_set(U);
    ins.i32_const((DIGITS + 16) as i32);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(I);
    ins.local_get(U);
    ins.i32_const(10);
    ins.i32_rem_u();
    ins.i32_const(48);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.local_get(U);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.local_set(U);
    ins.local_get(U);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(NEG);
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(I);
    ins.i32_const(b'-' as i32);
    ins.i32_store8(mem(0, 1));
    ins.end();
    ins.i32_const((DIGITS + 16) as i32);
    ins.local_get(I);
    ins.i32_sub();
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_add();
    ins.local_get(I);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store8(mem(0, 1));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(P);
}

fn int64_to_string(b: &mut Builder, ins: &mut InstructionSink) {
    const NEG: u32 = 1;
    const I: u32 = 2;
    const P: u32 = 3;
    const LEN: u32 = 4;
    const J: u32 = 5;
    const U: u32 = 6;
    ins.local_get(0);
    ins.i64_const(0);
    ins.i64_lt_s();
    ins.local_set(NEG);
    ins.local_get(NEG);
    ins.if_(BlockType::Result(ValType::I64));
    ins.i64_const(0);
    ins.local_get(0);
    ins.i64_sub();
    ins.else_();
    ins.local_get(0);
    ins.end();
    ins.local_set(U);
    ins.i32_const((DIGITS64 + 24) as i32);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(I);
    ins.local_get(U);
    ins.i64_const(10);
    ins.i64_rem_u();
    ins.i32_wrap_i64();
    ins.i32_const(48);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.local_get(U);
    ins.i64_const(10);
    ins.i64_div_u();
    ins.local_set(U);
    ins.local_get(U);
    ins.i64_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(NEG);
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(I);
    ins.i32_const(b'-' as i32);
    ins.i32_store8(mem(0, 1));
    ins.end();
    ins.i32_const((DIGITS64 + 24) as i32);
    ins.local_get(I);
    ins.i32_sub();
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_add();
    ins.local_get(I);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store8(mem(0, 1));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(P);
}
