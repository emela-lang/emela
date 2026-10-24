//! Strings, lists and UTF-8 helpers.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("str_new", &[ValType::I32], &[ValType::I32]),
    ("str_eq", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("str_cmp", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("str_append", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("str_byte_size", &[ValType::I32], &[ValType::I32]),
    (
        "str_contains",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "str_starts_with",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "str_ends_with",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "str_find",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "str_slice",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("str_split", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("str_join", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("str_trim", &[ValType::I32], &[ValType::I32]),
    ("str_code_points", &[ValType::I32], &[ValType::I32]),
    (
        "copy_range",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        &[],
    ),
    (
        "cons",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("utf8_len", &[ValType::I32], &[ValType::I32]),
    ("utf8_cp", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("is_white_space", &[ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "str_new" => i32s(1),                           // p
        "str_eq" => i32s(3),                            // la, lb, i
        "str_cmp" => i32s(6),                           // la, lb, n, i, x, y
        "str_append" => i32s(5),                        // la, lb, p, off, i
        "str_starts_with" | "str_ends_with" => i32s(3), // n, m, i
        "str_find" => i32s(4),                          // n, m, i, j
        "copy_range" => i32s(1),                        // i
        "str_slice" => i32s(1),                         // p
        "str_split" => i32s(8),                         // head..cell
        "str_join" => i32s(7),                          // total, p, first, out, pos, len
        "str_trim" => i32s(6),                          // n, start, end, at, cp, len
        "str_code_points" => i32s(6),                   // head, last, i, n, cp, cell
        "cons" => i32s(1),                              // p
        "utf8_cp" => i32s(4),                           // b0, b1, b2, b3
        "is_white_space" => i32s(1),                    // acc
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "str_new" => str_new(b, ins),
        "str_eq" => str_eq(ins),
        "str_cmp" => str_cmp(ins),
        "str_append" => str_append(b, ins),
        "str_byte_size" => str_byte_size(ins),
        "str_contains" => str_contains(b, ins),
        "str_starts_with" => str_starts_with(ins),
        "str_ends_with" => str_ends_with(ins),
        "str_find" => str_find(ins),
        "str_slice" => str_slice(b, ins),
        "str_split" => str_split(b, ins),
        "str_join" => str_join(b, ins),
        "str_trim" => str_trim(b, ins),
        "str_code_points" => str_code_points(b, ins),
        "copy_range" => copy_range(ins),
        "cons" => cons(b, ins),
        "utf8_len" => utf8_len(ins),
        "utf8_cp" => utf8_cp(ins),
        "is_white_space" => is_white_space(ins),
        _ => return false,
    }
    true
}

// ---- strings ----

fn str_new(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 1;
    ins.local_get(0);
    ins.i32_const((HDR + 4 + 7) as i32);
    ins.i32_add();
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.global_get(G_STRDESC);
    ins.call(rt(b, "alloc"));
    ins.local_tee(P);
    ins.local_get(0);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
}

fn str_eq(ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const I: u32 = 4;
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LA);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

fn str_cmp(ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const N: u32 = 4;
    const I: u32 = 5;
    const X: u32 = 6;
    const Y: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_lt_u();
    ins.select();
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
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
    ins.local_set(X);
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(Y);
    ins.local_get(X);
    ins.local_get(Y);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(X);
    ins.local_get(Y);
    ins.i32_sub();
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_sub();
}

fn str_append(b: &mut Builder, ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const P: u32 = 4;
    const OFF: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_add();
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(OFF);
    copy_bytes(ins, P, OFF, 0, LA);
    ins.local_get(LA);
    ins.local_set(OFF);
    copy_bytes(ins, P, OFF, 1, LB);
    ins.local_get(P);
}

/// Copies `len` bytes of the source string `src` (a local) to the string `dst` (a local) starting
/// at the byte offset in `off` (a local). Reuses local `I` (index 6).
fn copy_bytes(ins: &mut InstructionSink, dst: u32, off: u32, src: u32, len: u32) {
    const I: u32 = 6;
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(len);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(dst);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(off);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(src);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
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
}

fn str_byte_size(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
}

/// `String.contains(s, sub)`: a substring (the same as matching code points, 16.2).
fn str_contains(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.local_get(1);
    ins.i32_const(0);
    ins.call(rt(b, "str_find"));
    ins.i32_const(0);
    ins.i32_ge_s();
}

fn str_starts_with(ins: &mut InstructionSink) {
    const N: u32 = 2;
    const M: u32 = 3;
    const I: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(M);
    ins.local_get(M);
    ins.local_get(N);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(M);
    ins.i32_ge_u();
    ins.br_if(1);
    byte_at(ins, 0, I, 0);
    byte_at(ins, 1, I, 0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

fn str_ends_with(ins: &mut InstructionSink) {
    const N: u32 = 2;
    const M: u32 = 3;
    const I: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(M);
    ins.local_get(M);
    ins.local_get(N);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(M);
    ins.i32_ge_u();
    ins.br_if(1);
    // `s[20 + n - m + i]`
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(N);
    ins.local_get(M);
    ins.i32_sub();
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    byte_at(ins, 1, I, 0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

/// Pushes the byte at byte offset `extra + local[i]` of the string in local `str` (bytes start at
/// offset 20).
fn byte_at(ins: &mut InstructionSink, str: u32, i: u32, extra: u32) {
    ins.local_get(str);
    ins.i32_const((HDR + 4 + extra) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

/// `str_find(s, sub, from)`: the byte offset of the first `sub` at or after `from`, or -1.
fn str_find(ins: &mut InstructionSink) {
    const N: u32 = 3;
    const M: u32 = 4;
    const I: u32 = 5;
    const J: u32 = 6;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(M);
    ins.local_get(M);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.local_get(M);
    ins.local_get(N);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1);
    ins.return_();
    ins.end();
    ins.local_get(2);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(M);
    ins.i32_add();
    ins.local_get(N);
    ins.i32_gt_u();
    ins.br_if(1);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(M);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.return_();
    ins.end();
    // `s[20 + i + j]` vs `sub[20 + j]`
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    byte_at(ins, 1, J, 0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.br(2);
    ins.end();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(-1);
}

/// Copies `len` bytes from `src + 20 + src_off` to `dst + 20 + dst_off`.
fn copy_range(ins: &mut InstructionSink) {
    const I: u32 = 5;
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(4);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(1);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(2);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(3);
    ins.i32_add();
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
}

/// `str_slice(s, off, len)`: a new string of `len` bytes of `s` from byte `off`.
fn str_slice(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    ins.local_get(2);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_const(0);
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "copy_range"));
    ins.local_get(P);
}

/// A list cell `{head, tail}` (the head as a word).
fn cons(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    ins.i32_const(2);
    ins.local_get(2);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(0);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
    ins.local_get(1);
    ins.i32_store(mem(HDR + 8, 4));
    ins.local_get(P);
}

/// `String.split(s, sep)`, with a non-empty separator. An empty one needs the grapheme tables,
/// so it raises a defect instead (16.2).
fn str_split(b: &mut Builder, ins: &mut InstructionSink) {
    const HEAD: u32 = 2;
    const LAST: u32 = 3;
    const START: u32 = 4;
    const N: u32 = 5;
    const M: u32 = 6;
    const AT: u32 = 7;
    const SEG: u32 = 8;
    const CELL: u32 = 9;
    let cons_ref = b.cons_ref_desc;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(M);
    ins.local_get(M);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let msg = b.string_literal(
        "`String.split` with an empty separator needs the Unicode grapheme cluster tables, which \
         target `wasm` does not support yet"
            .as_bytes(),
    );
    ins.i32_const(msg as i32);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(HEAD);
    ins.i32_const(0);
    ins.local_set(LAST);
    ins.i32_const(0);
    ins.local_set(START);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(START);
    ins.call(rt(b, "str_find"));
    ins.local_set(AT);
    ins.local_get(AT);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(START);
    ins.local_get(AT);
    ins.local_get(START);
    ins.i32_sub();
    ins.call(rt(b, "str_slice"));
    ins.local_set(SEG);
    ins.local_get(SEG);
    ins.i32_const(0);
    ins.i32_const(cons_ref as i32);
    ins.call(rt(b, "cons"));
    ins.local_set(CELL);
    append_cell(ins, HEAD, LAST, CELL);
    ins.local_get(CELL);
    ins.local_set(LAST);
    ins.local_get(AT);
    ins.local_get(M);
    ins.i32_add();
    ins.local_set(START);
    ins.br(0);
    ins.end();
    ins.end();
    // The last segment.
    ins.local_get(0);
    ins.local_get(START);
    ins.local_get(N);
    ins.local_get(START);
    ins.i32_sub();
    ins.call(rt(b, "str_slice"));
    ins.local_set(SEG);
    ins.local_get(SEG);
    ins.i32_const(0);
    ins.i32_const(cons_ref as i32);
    ins.call(rt(b, "cons"));
    ins.local_set(CELL);
    append_cell(ins, HEAD, LAST, CELL);
    ins.local_get(HEAD);
}

/// Appends the cell in `cell` to the list whose head and last cells are `head` and `last`.
fn append_cell(ins: &mut InstructionSink, head: u32, last: u32, cell: u32) {
    ins.local_get(last);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(cell);
    ins.local_set(head);
    ins.else_();
    ins.local_get(last);
    ins.local_get(cell);
    ins.i32_store(mem(HDR + 8, 4));
    ins.end();
}

/// `String.join(parts, sep)`.
fn str_join(b: &mut Builder, ins: &mut InstructionSink) {
    const TOTAL: u32 = 2;
    const P: u32 = 3;
    const FIRST: u32 = 4;
    const OUT: u32 = 5;
    const POS: u32 = 6;
    const LEN: u32 = 7;
    const HEAD: u32 = 8;
    // Pass 1: the total byte length.
    ins.i32_const(0);
    ins.local_set(TOTAL);
    ins.local_get(0);
    ins.local_set(P);
    ins.i32_const(1);
    ins.local_set(FIRST);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LEN);
    ins.local_get(TOTAL);
    ins.local_get(LEN);
    ins.i32_add();
    ins.local_set(TOTAL);
    ins.local_get(FIRST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(TOTAL);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.i32_add();
    ins.local_set(TOTAL);
    ins.end();
    ins.i32_const(0);
    ins.local_set(FIRST);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    // Pass 2: copy.
    ins.local_get(TOTAL);
    ins.call(rt(b, "str_new"));
    ins.local_set(OUT);
    ins.i32_const(0);
    ins.local_set(POS);
    ins.local_get(0);
    ins.local_set(P);
    ins.i32_const(1);
    ins.local_set(FIRST);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(FIRST);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(OUT);
    ins.local_get(POS);
    ins.local_get(1);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "copy_range"));
    ins.local_get(POS);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.i32_add();
    ins.local_set(POS);
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(HEAD);
    ins.local_get(OUT);
    ins.local_get(POS);
    ins.local_get(HEAD);
    ins.i32_const(0);
    ins.local_get(HEAD);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "copy_range"));
    ins.local_get(POS);
    ins.local_get(HEAD);
    ins.i32_load(mem(HDR, 4));
    ins.i32_add();
    ins.local_set(POS);
    ins.i32_const(0);
    ins.local_set(FIRST);
    ins.local_get(P);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(P);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(OUT);
}

/// `String.trim`: removes leading and trailing Unicode White_Space (16.2).
fn str_trim(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    const START: u32 = 2;
    const END: u32 = 3;
    const AT: u32 = 4;
    const CP: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(START);
    // Leading white space.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(START);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(START);
    ins.call(rt(b, "utf8_cp"));
    ins.local_set(CP);
    ins.local_get(CP);
    ins.call(rt(b, "is_white_space"));
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(START);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(START);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.call(rt(b, "utf8_len"));
    ins.i32_add();
    ins.local_set(START);
    ins.br(0);
    ins.end();
    ins.end();
    // Trailing white space.
    ins.local_get(N);
    ins.local_set(END);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(END);
    ins.local_get(START);
    ins.i32_le_u();
    ins.br_if(1);
    ins.local_get(END);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(AT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(AT);
    ins.local_get(START);
    ins.i32_le_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(AT);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(-64);
    ins.i32_and();
    ins.i32_const(128);
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(AT);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(AT);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(AT);
    ins.call(rt(b, "utf8_cp"));
    ins.local_set(CP);
    ins.local_get(CP);
    ins.call(rt(b, "is_white_space"));
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(AT);
    ins.local_set(END);
    ins.br(0);
    ins.end();
    ins.end();
    // Unchanged: give back the same string.
    ins.local_get(START);
    ins.i32_eqz();
    ins.local_get(END);
    ins.local_get(N);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "dup"));
    ins.local_get(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(START);
    ins.local_get(END);
    ins.local_get(START);
    ins.i32_sub();
    ins.call(rt(b, "str_slice"));
}

/// `String.code_points`: a `List[Int]` of the code points.
fn str_code_points(b: &mut Builder, ins: &mut InstructionSink) {
    const HEAD: u32 = 1;
    const LAST: u32 = 2;
    const I: u32 = 3;
    const N: u32 = 4;
    const CP: u32 = 5;
    const CELL: u32 = 6;
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
    ins.local_get(0);
    ins.local_get(I);
    ins.call(rt(b, "utf8_cp"));
    ins.local_set(CP);
    ins.local_get(CP);
    ins.i32_const(0);
    ins.i32_const(cons_val as i32);
    ins.call(rt(b, "cons"));
    ins.local_set(CELL);
    append_cell(ins, HEAD, LAST, CELL);
    ins.local_get(CELL);
    ins.local_set(LAST);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.call(rt(b, "utf8_len"));
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HEAD);
}

/// The length in bytes of the UTF-8 sequence whose first byte is the parameter.
fn utf8_len(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(1);
    ins.else_();
    ins.local_get(0);
    ins.i32_const(0xE0);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(2);
    ins.else_();
    ins.local_get(0);
    ins.i32_const(0xF0);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(3);
    ins.else_();
    ins.i32_const(4);
    ins.end();
    ins.end();
    ins.end();
}

/// The code point of the UTF-8 sequence at byte offset `i` of the string.
fn utf8_cp(ins: &mut InstructionSink) {
    const B0: u32 = 2;
    const B1: u32 = 3;
    const B2: u32 = 4;
    const B3: u32 = 5;
    byte_at(ins, 0, 1, 0);
    ins.local_set(B0);
    ins.local_get(B0);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(B0);
    ins.return_();
    ins.end();
    byte_at(ins, 0, 1, 1);
    ins.local_set(B1);
    ins.local_get(B0);
    ins.i32_const(0xE0);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(B0);
    ins.i32_const(0x1F);
    ins.i32_and();
    ins.i32_const(6);
    ins.i32_shl();
    ins.local_get(B1);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_or();
    ins.return_();
    ins.end();
    byte_at(ins, 0, 1, 2);
    ins.local_set(B2);
    ins.local_get(B0);
    ins.i32_const(0xF0);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(B0);
    ins.i32_const(0x0F);
    ins.i32_and();
    ins.i32_const(12);
    ins.i32_shl();
    ins.local_get(B1);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_const(6);
    ins.i32_shl();
    ins.i32_or();
    ins.local_get(B2);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_or();
    ins.return_();
    ins.end();
    byte_at(ins, 0, 1, 3);
    ins.local_set(B3);
    ins.local_get(B0);
    ins.i32_const(0x07);
    ins.i32_and();
    ins.i32_const(18);
    ins.i32_shl();
    ins.local_get(B1);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_const(12);
    ins.i32_shl();
    ins.i32_or();
    ins.local_get(B2);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_const(6);
    ins.i32_shl();
    ins.i32_or();
    ins.local_get(B3);
    ins.i32_const(0x3F);
    ins.i32_and();
    ins.i32_or();
}

/// The Unicode White_Space property (the 25 characters of 16.2).
fn is_white_space(ins: &mut InstructionSink) {
    const ACC: u32 = 1;
    // 0x09..=0x0D
    ins.local_get(0);
    ins.i32_const(0x09);
    ins.i32_ge_u();
    ins.local_get(0);
    ins.i32_const(0x0D);
    ins.i32_le_u();
    ins.i32_and();
    ins.local_set(ACC);
    for k in [
        0x20, 0x85, 0xA0, 0x1680, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000,
    ] {
        ins.local_get(ACC);
        ins.local_get(0);
        ins.i32_const(k);
        ins.i32_eq();
        ins.i32_or();
        ins.local_set(ACC);
    }
    // 0x2000..=0x200A
    ins.local_get(ACC);
    ins.local_get(0);
    ins.i32_const(0x2000);
    ins.i32_ge_u();
    ins.local_get(0);
    ins.i32_const(0x200A);
    ins.i32_le_u();
    ins.i32_and();
    ins.i32_or();
    ins.local_set(ACC);
    ins.local_get(ACC);
}
