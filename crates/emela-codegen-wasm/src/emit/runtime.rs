//! The WASM runtime, emitted into the same module as the program.
//!
//! It holds the Perceus allocator (`alloc`, `dup`, `drop` with the iterative free list), the
//! string and integer conversions, the evidence tables and the WASI host calls (`fd_write` for
//! `dbg` and the test output, `proc_exit`). It is written with `wasm-encoder` like the rest of
//! the module, so the backend has no external toolchain.
//!
//! Memory layout (see `emit.rs`): a 16-byte object header (rc, aux, descriptor, padding) then
//! 8-byte slots. Static data (descriptors, string literals, nullary constructors, capture-less
//! closures) has `rc == 0` and is never counted.

use wasm_encoder::{BlockType, InstructionSink, ValType};

use super::*;

/// The scratch area below the heap: the WASI iovec and `nwritten` at 0..12, the integer digit
/// buffers at 64.., the newline byte at 88. `HEAP_BASE` (in `emit.rs`) is above it.
const IOVEC: u32 = 0;
const NWRITTEN: u32 = 8;
const DIGITS: u32 = 64;
const NEWLINE: u32 = 88;
const DIGITS64: u32 = 96;
/// A short buffer for the bytes of a JSON escape (up to 6).
const JSONBUF: u32 = 128;
/// A buffer for encoding one code point as UTF-8 (up to 4 bytes).
const CPBUF: u32 = 160;

/// A runtime function: its name and wasm signature.
type Sig = (&'static str, &'static [ValType], &'static [ValType]);

/// Every runtime function, in definition order.
pub(crate) const FUNCS: &[Sig] = &[
    ("alloc", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("dup", &[ValType::I32], &[]),
    ("drop", &[ValType::I32], &[]),
    ("tag", &[ValType::I32], &[ValType::I32]),
    ("is_error", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ev_lookup", &[ValType::I32, ValType::I32], &[ValType::I32]),
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
    ("show_string", &[ValType::I32], &[ValType::I32]),
    ("int_to_string", &[ValType::I32], &[ValType::I32]),
    ("int64_to_string", &[ValType::I64], &[ValType::I32]),
    ("debug_print", &[ValType::I32], &[]),
    ("write", &[ValType::I32, ValType::I32, ValType::I32], &[]),
    ("exit", &[ValType::I32], &[]),
    ("raise", &[ValType::I32], &[]),
    ("defect_take", &[], &[ValType::I32]),
    ("defect_resume", &[ValType::I32], &[]),
    ("retire", &[ValType::I32, ValType::I32], &[]),
    ("show_failure", &[ValType::I32], &[ValType::I32]),
    ("json_string", &[ValType::I32], &[]),
    ("test_result", &[ValType::I32], &[]),
    ("test_done", &[], &[]),
    ("finish", &[], &[]),
    ("idiv", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("irem", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ineg", &[ValType::I32], &[ValType::I32]),
    ("ishl", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ishr", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("iushr", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ldiv", &[ValType::I64, ValType::I64], &[ValType::I64]),
    ("lrem", &[ValType::I64, ValType::I64], &[ValType::I64]),
    ("lneg", &[ValType::I64], &[ValType::I64]),
    ("lshl", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("lshr", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("lushr", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("fmod", &[ValType::F64, ValType::F64], &[ValType::F64]),
];

/// Reserves the runtime functions in the function index space.
pub(crate) fn register(b: &mut Builder) {
    for (name, params, results) in FUNCS {
        let ty = b.ty(params.to_vec(), results.to_vec());
        let index = b.imports.len() as u32 + b.defs.len() as u32;
        b.defs.push(ty);
        b.rt.insert(name, index);
    }
}

fn rt(b: &Builder, name: &'static str) -> u32 {
    b.rt[name]
}

fn i32s(first: u32, n: u32) -> Vec<(u32, ValType)> {
    let _ = first;
    vec![(n, ValType::I32)]
}

/// Emits every runtime function body, in `FUNCS` order.
pub(crate) fn emit(b: &mut Builder) {
    for (name, ..) in FUNCS {
        let mut f = WasmFunction::new(locals(name));
        {
            let mut ins = f.instructions();
            body(b, name, &mut ins);
            ins.end();
        }
        b.bodies.push(f);
    }
}

fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "alloc" => i32s(2, 6),      // need, cur, prev, next, p, pages
        "dup" => i32s(1, 1),        // rc
        "drop" => i32s(1, 13),      // rc..size
        "ev_lookup" => i32s(2, 3),  // count, i, e
        "str_new" => i32s(1, 1),    // p
        "str_eq" => i32s(2, 3),     // la, lb, i
        "str_cmp" => i32s(2, 6),    // la, lb, n, i, x, y
        "str_append" => i32s(2, 5), // la, lb, p, off, i
        "str_starts_with" | "str_ends_with" => i32s(2, 3), // n, m, i
        "str_find" => i32s(3, 4),   // n, m, i, j
        "copy_range" => i32s(5, 1), // i
        "str_slice" => i32s(3, 1),  // p
        "str_split" => i32s(2, 8),  // head..cell
        "str_join" => i32s(2, 7),   // total, p, first, out, pos, len
        "str_trim" => i32s(1, 6),   // n, start, end, at, cp, len
        "str_code_points" => i32s(1, 6), // head, last, i, n, cp, cell
        "cons" => i32s(3, 1),       // p
        "utf8_cp" => i32s(2, 4),    // b0, b1, b2, b3
        "is_white_space" => i32s(1, 1), // acc
        "int_parse" => vec![(5, ValType::I32), (1, ValType::I64)], // p, n, i, neg, b / v
        "int_checked_add" => vec![(1, ValType::I64), (1, ValType::I32)], // r / p
        "from_code_point" => i32s(3, 4), // p, len, i, cell
        "show_string" => i32s(1, 5), // n, i, j, p, b
        "int_to_string" => i32s(1, 6), // neg, u, i, p, len, j
        "int64_to_string" => vec![(5, ValType::I32), (1, ValType::I64)], // neg,i,p,len,j / u
        "defect_take" => i32s(0, 1),
        "show_failure" => i32s(1, 3), // desc, show, result
        "json_string" => i32s(1, 3),  // i, byte, hex
        "test_result" => i32s(1, 3),  // kind, message, error
        "finish" => i32s(0, 1),       // shown
        _ => Vec::new(),
    }
}

fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) {
    match name {
        "alloc" => alloc(b, ins),
        "dup" => dup(ins),
        "drop" => drop_(ins),
        "tag" => tag(ins),
        "is_error" => is_error(ins),
        "ev_lookup" => ev_lookup(ins),
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
        "int_parse" => int_parse(b, ins),
        "int_checked_add" => int_checked_add(b, ins),
        "from_code_point" => from_code_point(b, ins),
        "show_string" => show_string(b, ins),
        "int_to_string" => int_to_string(b, ins),
        "int64_to_string" => int64_to_string(b, ins),
        "debug_print" => debug_print(b, ins),
        "write" => write(b, ins),
        "exit" => exit(b, ins),
        "raise" => raise(ins),
        "defect_take" => defect_take(ins),
        "defect_resume" => defect_resume(b, ins),
        "retire" => retire(ins),
        "show_failure" => show_failure(b, ins),
        "json_string" => json_string(b, ins),
        "test_result" => test_result(b, ins),
        "test_done" => test_done(b, ins),
        "finish" => finish(b, ins),
        "idiv" | "irem" | "ineg" => int_binop(ins, name),
        "ishl" | "ishr" | "iushr" => ishift(b, ins, name),
        "ldiv" | "lrem" | "lneg" => l_int_binop(ins, name),
        "lshl" | "lshr" | "lushr" => lshift(b, ins, name),
        "fmod" => fmod(ins),
        _ => {}
    }
}

// ---- allocation and reference counting ----

const ALLOC_NEED: u32 = 2;
const ALLOC_CUR: u32 = 3;
const ALLOC_PREV: u32 = 4;
const ALLOC_NEXT: u32 = 5;
const ALLOC_P: u32 = 6;
const ALLOC_PAGES: u32 = 7;

fn alloc(_b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(ALLOC_NEED);
    ins.i32_const(0);
    ins.local_set(ALLOC_PREV);
    ins.global_get(G_FREE);
    ins.local_set(ALLOC_CUR);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ALLOC_CUR);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(ALLOC_CUR);
    ins.i32_load(mem(0, 4));
    ins.local_get(ALLOC_NEED);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(ALLOC_CUR);
    ins.i32_load(mem(4, 4));
    ins.local_set(ALLOC_NEXT);
    ins.local_get(ALLOC_PREV);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(ALLOC_NEXT);
    ins.global_set(G_FREE);
    ins.else_();
    ins.local_get(ALLOC_PREV);
    ins.local_get(ALLOC_NEXT);
    ins.i32_store(mem(4, 4));
    ins.end();
    ins.local_get(ALLOC_CUR);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_CUR);
    ins.local_get(1);
    ins.i32_store(mem(8, 4));
    ins.local_get(ALLOC_CUR);
    ins.return_();
    ins.end();
    ins.local_get(ALLOC_CUR);
    ins.local_set(ALLOC_PREV);
    ins.local_get(ALLOC_CUR);
    ins.i32_load(mem(4, 4));
    ins.local_set(ALLOC_CUR);
    ins.br(0);
    ins.end();
    ins.end();
    // Bump.
    ins.global_get(G_HEAP);
    ins.local_set(ALLOC_P);
    ins.local_get(ALLOC_P);
    ins.local_get(ALLOC_NEED);
    ins.i32_add();
    ins.memory_size(0);
    ins.i32_const(PAGE as i32);
    ins.i32_mul();
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(ALLOC_P);
    ins.local_get(ALLOC_NEED);
    ins.i32_add();
    ins.memory_size(0);
    ins.i32_const(PAGE as i32);
    ins.i32_mul();
    ins.i32_sub();
    ins.i32_const((PAGE - 1) as i32);
    ins.i32_add();
    ins.i32_const(PAGE as i32);
    ins.i32_div_u();
    ins.local_set(ALLOC_PAGES);
    ins.local_get(ALLOC_PAGES);
    ins.memory_grow(0);
    ins.drop();
    ins.end();
    ins.local_get(ALLOC_P);
    ins.local_get(ALLOC_NEED);
    ins.i32_add();
    ins.global_set(G_HEAP);
    ins.local_get(ALLOC_P);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_P);
    ins.local_get(1);
    ins.i32_store(mem(8, 4));
    ins.local_get(ALLOC_P);
}

fn dup(ins: &mut InstructionSink) {
    const RC: u32 = 1;
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(RC);
    ins.local_get(RC);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(RC);
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
}

fn drop_(ins: &mut InstructionSink) {
    const RC: u32 = 2;
    const PUSH: u32 = 3;
    const Q: u32 = 4;
    const D: u32 = 5;
    const KIND: u32 = 6;
    const N: u32 = 7;
    const REFS: u32 = 8;
    const I: u32 = 9;
    const C: u32 = 10;
    const CRC: u32 = 11;
    const LEN: u32 = 12;
    const SIZE: u32 = 13;
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(RC);
    ins.local_get(RC);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(RC);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(RC);
    ins.local_get(0);
    ins.local_get(RC);
    ins.i32_store(mem(0, 4));
    ins.local_get(RC);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_set(PUSH);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(PUSH);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(PUSH);
    ins.local_set(Q);
    ins.local_get(Q);
    ins.i32_load(mem(4, 4));
    ins.local_set(PUSH);
    ins.local_get(Q);
    ins.i32_load(mem(8, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(DESC_KIND, 4));
    ins.local_set(KIND);
    ins.local_get(KIND);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(Q);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.i32_const((HDR + 4 + 7) as i32);
    ins.i32_add();
    ins.i32_const(-8);
    ins.i32_and();
    ins.local_set(SIZE);
    free_raw(ins, Q, SIZE);
    ins.else_();
    ins.local_get(D);
    ins.i32_load(mem(DESC_NSLOTS, 4));
    ins.local_set(N);
    ins.local_get(D);
    ins.i32_load(mem(DESC_REFS, 4));
    ins.local_set(REFS);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(REFS);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.local_get(Q);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(C);
    ins.local_get(C);
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i32_load(mem(0, 4));
    ins.local_set(CRC);
    ins.local_get(CRC);
    ins.if_(BlockType::Empty);
    ins.local_get(CRC);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(CRC);
    ins.local_get(C);
    ins.local_get(CRC);
    ins.i32_store(mem(0, 4));
    ins.local_get(CRC);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.local_get(PUSH);
    ins.i32_store(mem(4, 4));
    ins.local_get(C);
    ins.local_set(PUSH);
    ins.end();
    ins.end();
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
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(SIZE);
    free_raw(ins, Q, SIZE);
    ins.end();
    ins.br(1);
    ins.end();
    ins.end();
}

/// Pushes `q` onto the free list with `size` in the `rc` field and the next pointer in `aux`.
fn free_raw(ins: &mut InstructionSink, q: u32, size: u32) {
    ins.local_get(q);
    ins.local_get(size);
    ins.i32_store(mem(0, 4));
    ins.local_get(q);
    ins.global_get(G_FREE);
    ins.i32_store(mem(4, 4));
    ins.local_get(q);
    ins.global_set(G_FREE);
}

fn tag(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_TAG, 4));
}

fn is_error(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.i32_load(mem(DESC_ENUM, 4));
    ins.local_get(1);
    ins.i32_eq();
    ins.return_();
}

fn ev_lookup(ins: &mut InstructionSink) {
    const COUNT: u32 = 2;
    const I: u32 = 3;
    const E: u32 = 4;
    ins.loop_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR + 8, 4));
    ins.local_set(COUNT);
    ins.local_get(COUNT);
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
    ins.local_get(0);
    ins.i32_const((HDR + 16) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(E);
    ins.local_get(E);
    ins.local_get(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const((HDR + 16) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(8, 4));
    ins.return_();
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(0);
    ins.br(0);
    ins.end();
    ins.unreachable();
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

/// Shows a string inside a value: `"` at both ends, with `"` and `\` escaped.
fn show_string(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    const I: u32 = 2;
    const J: u32 = 3;
    const P: u32 = 4;
    const BYTE: u32 = 5;
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
    ins.local_tee(BYTE);
    ins.i32_const(b'"' as i32);
    ins.i32_eq();
    ins.local_get(BYTE);
    ins.i32_const(b'\\' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(N);
    ins.i32_add();
    ins.local_set(N);
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
    dst_byte(ins, P, J);
    ins.i32_const(b'"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.local_set(J);
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
    ins.local_get(BYTE);
    ins.i32_const(b'"' as i32);
    ins.i32_eq();
    ins.local_get(BYTE);
    ins.i32_const(b'\\' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    dst_byte(ins, P, J);
    ins.i32_const(b'\\' as i32);
    ins.i32_store8(mem(0, 1));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.end();
    dst_byte(ins, P, J);
    ins.local_get(BYTE);
    ins.i32_store8(mem(0, 1));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Closing quote.
    dst_byte(ins, P, J);
    ins.i32_const(b'"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.local_get(P);
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

// ---- host calls ----

fn write(b: &mut Builder, ins: &mut InstructionSink) {
    ins.i32_const(IOVEC as i32);
    ins.local_get(1);
    ins.i32_store(mem(0, 4));
    ins.i32_const((IOVEC + 4) as i32);
    ins.local_get(2);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.i32_const(IOVEC as i32);
    ins.i32_const(1);
    ins.i32_const(NWRITTEN as i32);
    ins.call(b.fd_write());
    ins.drop();
}

fn debug_print(b: &mut Builder, ins: &mut InstructionSink) {
    let write = rt(b, "write");
    ins.i32_const(2);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(10);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(2);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(1);
    ins.call(write);
}

fn exit(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.call(b.proc_exit());
    ins.unreachable();
}

// ---- defects ----

fn raise(ins: &mut InstructionSink) {
    ins.global_get(G_DEFECT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.global_set(G_DEFECT);
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
    ins.local_get(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.end();
}

fn retire(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
}

// ---- numeric helpers ----

fn int_binop(ins: &mut InstructionSink, name: &str) {
    let neg = |ins: &mut InstructionSink| {
        ins.i32_const(0);
        ins.local_get(0);
        ins.i32_sub();
    };
    match name {
        "ineg" => neg(ins),
        "idiv" => {
            ins.local_get(1);
            ins.i32_const(-1);
            ins.i32_eq();
            ins.if_(BlockType::Result(ValType::I32));
            neg(ins);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i32_div_s();
            ins.end();
        }
        "irem" => {
            ins.local_get(1);
            ins.i32_const(-1);
            ins.i32_eq();
            ins.if_(BlockType::Result(ValType::I32));
            ins.i32_const(0);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i32_rem_s();
            ins.end();
        }
        _ => {}
    }
}

fn l_int_binop(ins: &mut InstructionSink, name: &str) {
    let neg = |ins: &mut InstructionSink| {
        ins.i64_const(0);
        ins.local_get(0);
        ins.i64_sub();
    };
    match name {
        "lneg" => neg(ins),
        "ldiv" => {
            ins.local_get(1);
            ins.i64_const(-1);
            ins.i64_eq();
            ins.if_(BlockType::Result(ValType::I64));
            neg(ins);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i64_div_s();
            ins.end();
        }
        "lrem" => {
            ins.local_get(1);
            ins.i64_const(-1);
            ins.i64_eq();
            ins.if_(BlockType::Result(ValType::I64));
            ins.i64_const(0);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i64_rem_s();
            ins.end();
        }
        _ => {}
    }
}

fn ishift(b: &mut Builder, ins: &mut InstructionSink, name: &str) {
    negative_shift(b, ins, Repr::I32);
    ins.local_get(1);
    ins.i32_const(32);
    ins.i32_ge_u();
    ins.if_(BlockType::Result(ValType::I32));
    match name {
        "ishr" => {
            ins.local_get(0);
            ins.i32_const(31);
            ins.i32_shr_s();
        }
        _ => {
            ins.i32_const(0);
        }
    }
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    match name {
        "ishl" => {
            ins.i32_shl();
        }
        "ishr" => {
            ins.i32_shr_s();
        }
        _ => {
            ins.i32_shr_u();
        }
    }
    ins.end();
}

fn lshift(b: &mut Builder, ins: &mut InstructionSink, name: &str) {
    negative_shift(b, ins, Repr::I64);
    ins.local_get(1);
    ins.i32_const(64);
    ins.i32_ge_u();
    ins.if_(BlockType::Result(ValType::I64));
    match name {
        "lshr" => {
            ins.local_get(0);
            ins.i64_const(63);
            ins.i64_shr_s();
        }
        _ => {
            ins.i64_const(0);
        }
    }
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    match name {
        "lshl" => {
            ins.i64_shl();
        }
        "lshr" => {
            ins.i64_shr_s();
        }
        _ => {
            ins.i64_shr_u();
        }
    }
    ins.end();
}

/// Raises `negative shift amount: <by>` when the shift amount is negative, then returns a dummy.
fn negative_shift(b: &mut Builder, ins: &mut InstructionSink, ret: Repr) {
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    let prefix = b.string_literal(b"negative shift amount: ");
    ins.i32_const(prefix as i32);
    ins.local_get(1);
    ins.call(rt(b, "int_to_string"));
    ins.call(rt(b, "str_append"));
    ins.call(rt(b, "raise"));
    dummy_return(ins, ret);
    ins.end();
}

fn fmod(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.local_get(0);
    ins.local_get(1);
    ins.f64_div();
    ins.f64_trunc();
    ins.local_get(1);
    ins.f64_mul();
    ins.f64_sub();
}

// ---- failures and the test protocol (spec 13.4) ----

/// The litteral pieces of the test result line (the JS test runner's protocol).
const TEST_PREFIX: &[u8] = b"\x1eemela-test {\"event\":\"result\",\"name\":";
const TEST_OK: &[u8] = b",\"outcome\":\"ok\"}\n";
const TEST_DEFECT: &[u8] = b",\"outcome\":\"defect\"";
const TEST_ERROR: &[u8] = b",\"outcome\":\"error\"";
const TEST_MESSAGE: &[u8] = b",\"message\":";
const TEST_TAIL: &[u8] = b"}\n";
const TEST_DONE_LINE: &[u8] = b"\x1eemela-test {\"event\":\"done\"}\n";

/// `show_failure(e)`: the message of an error value, calling the `show` function of its descriptor
/// (spec 10.6). Returns a null reference when there is none (or a defect happened while showing).
fn show_failure(b: &mut Builder, ins: &mut InstructionSink) {
    const D: u32 = 1;
    const SHOW: u32 = 2;
    const RESULT: u32 = 3;
    ins.local_get(0);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.local_set(D);
    ins.local_get(D);
    ins.i32_load(mem(DESC_SHOW, 4));
    ins.local_tee(SHOW);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.call(rt(b, "drop"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    let ty = b.call_ty(&[Repr::Ref], Repr::Ref);
    ins.local_get(0);
    ins.local_get(SHOW);
    ins.call_indirect(0, ty);
    ins.local_set(RESULT);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.local_get(RESULT);
    ins.call(rt(b, "drop"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(RESULT);
}

/// Writes `bytes` (a static string object) to `fd`.
fn write_static(b: &mut Builder, ins: &mut InstructionSink, fd: i32, bytes: &[u8]) {
    let s = b.string_literal(bytes);
    ins.i32_const(fd);
    ins.i32_const((s + HDR + 4) as i32);
    ins.i32_const(bytes.len() as i32);
    ins.call(rt(b, "write"));
}

/// Writes `s` to stdout as a JSON string (spec 13.4): `"` at both ends and the escapes the JS test
/// runner uses (`\"`, `\\`, `\n`, and `\u00XX` for the other control characters).
fn json_string(b: &mut Builder, ins: &mut InstructionSink) {
    const I: u32 = 1;
    const B: u32 = 2;
    const HEX: u32 = 3;
    let write = rt(b, "write");
    // Opening quote.
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(B);
    // `"` or `\`.
    ins.local_get(B);
    ins.i32_const('"' as i32);
    ins.i32_eq();
    ins.local_get(B);
    ins.i32_const('\\' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    write_escape_byte(ins, write, B);
    ins.else_();
    // `\n`.
    ins.local_get(B);
    ins.i32_const('\n' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    write_two_bytes(ins, write, b'\\', b'n');
    ins.else_();
    // Other control characters: `\u00XX`.
    ins.local_get(B);
    ins.i32_const(0x20);
    ins.i32_lt_u();
    ins.local_get(B);
    ins.i32_const(0x7F);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('\\' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.i32_const('u' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 2) as i32);
    ins.i32_const('0' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 3) as i32);
    ins.i32_const('0' as i32);
    ins.i32_store8(mem(0, 1));
    hex_digit(ins, JSONBUF + 4, B, 4, HEX);
    hex_digit(ins, JSONBUF + 5, B, 15, HEX);
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(6);
    ins.call(write);
    ins.else_();
    ins.i32_const(JSONBUF as i32);
    ins.local_get(B);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
    ins.end();
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
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('"' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(1);
    ins.call(write);
}

/// Writes `\` and the byte in local `of` to stdout.
fn write_escape_byte(ins: &mut InstructionSink, write: u32, of: u32) {
    ins.i32_const(JSONBUF as i32);
    ins.i32_const('\\' as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.local_get(of);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(2);
    ins.call(write);
}

/// Writes the two bytes `a` and `b` to stdout.
fn write_two_bytes(ins: &mut InstructionSink, write: u32, a: u8, b: u8) {
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(a as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const((JSONBUF + 1) as i32);
    ins.i32_const(b as i32);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(JSONBUF as i32);
    ins.i32_const(2);
    ins.call(write);
}

/// Writes one lowercase hex digit of the nibble `n` (local) at `addr`.
fn hex_digit(ins: &mut InstructionSink, addr: u32, n: u32, mask: u32, hex: u32) {
    ins.i32_const(addr as i32);
    ins.local_get(n);
    if mask == 4 {
        ins.i32_const(4);
        ins.i32_shr_u();
    } else {
        ins.i32_const(15);
        ins.i32_and();
    }
    ins.local_tee(hex);
    ins.i32_const(10);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(hex);
    ins.i32_const('0' as i32);
    ins.i32_add();
    ins.else_();
    ins.local_get(hex);
    ins.i32_const('a' as i32 - 10);
    ins.i32_add();
    ins.end();
    ins.i32_store8(mem(0, 1));
}

/// Reports one test's result (spec 13.4): the name, the outcome, and the failure's message.
fn test_result(b: &mut Builder, ins: &mut InstructionSink) {
    const KIND: u32 = 1;
    const MESSAGE: u32 = 2;
    const E: u32 = 3;
    write_static(b, ins, 1, TEST_PREFIX);
    ins.local_get(0);
    ins.call(rt(b, "json_string"));
    ins.i32_const(0);
    ins.local_set(KIND);
    ins.i32_const(0);
    ins.local_set(MESSAGE);
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(KIND);
    ins.global_get(G_DEFECT);
    ins.local_set(MESSAGE);
    ins.i32_const(0);
    ins.global_set(G_DEFECT);
    ins.else_();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(E);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    ins.i32_const(2);
    ins.local_set(KIND);
    ins.local_get(E);
    ins.call(rt(b, "show_failure"));
    ins.local_set(MESSAGE);
    ins.end();
    ins.end();
    ins.local_get(KIND);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_OK);
    ins.else_();
    ins.local_get(KIND);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_DEFECT);
    ins.else_();
    write_static(b, ins, 1, TEST_ERROR);
    ins.end();
    ins.local_get(MESSAGE);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    write_static(b, ins, 1, TEST_MESSAGE);
    ins.local_get(MESSAGE);
    ins.call(rt(b, "json_string"));
    ins.local_get(MESSAGE);
    ins.call(rt(b, "drop"));
    ins.end();
    write_static(b, ins, 1, TEST_TAIL);
    ins.end();
}

/// Writes the closing `done` line.
fn test_done(b: &mut Builder, ins: &mut InstructionSink) {
    write_static(b, ins, 1, TEST_DONE_LINE);
}

/// Reports a pending defect (exit 101) or failure (exit 1) at the end of the program, like the C
/// runtime's `em_exit` (spec 14.3).
fn finish(b: &mut Builder, ins: &mut InstructionSink) {
    const SHOWN: u32 = 0;
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    write_static(b, ins, 2, b"defect: ");
    ins.i32_const(2);
    ins.global_get(G_DEFECT);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.global_get(G_DEFECT);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "write"));
    write_static(b, ins, 2, b"\n");
    ins.i32_const(101);
    ins.call(rt(b, "exit"));
    ins.end();
    ins.global_get(G_FAILURE);
    ins.if_(BlockType::Empty);
    ins.global_get(G_FAILURE);
    ins.local_set(SHOWN);
    ins.i32_const(0);
    ins.global_set(G_FAILURE);
    write_static(b, ins, 2, b"error: ");
    ins.local_get(SHOWN);
    ins.call(rt(b, "show_failure"));
    ins.local_set(SHOWN);
    ins.local_get(SHOWN);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_get(SHOWN);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(SHOWN);
    ins.i32_load(mem(HDR, 4));
    ins.call(rt(b, "write"));
    ins.end();
    write_static(b, ins, 2, b"\n");
    ins.i32_const(1);
    ins.call(rt(b, "exit"));
    ins.end();
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
