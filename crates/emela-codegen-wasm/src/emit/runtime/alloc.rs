//! The allocator, reference counting, descriptors and evidence lookup.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("alloc", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("heap_reserve", &[ValType::I32], &[]),
    ("dup", &[ValType::I32], &[]),
    ("drop", &[ValType::I32], &[]),
    ("tag", &[ValType::I32], &[ValType::I32]),
    ("is_error", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ev_lookup", &[ValType::I32, ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "alloc" => i32s(5),        // need, cur, prev, next, p
        "heap_reserve" => i32s(1), // memory bytes
        "dup" => i32s(1),          // rc
        "drop" => i32s(13),        // rc..size
        "ev_lookup" => i32s(3),    // count, i, e
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "alloc" => alloc(b, ins),
        "heap_reserve" => heap_reserve(ins),
        "dup" => dup(ins),
        "drop" => drop_(ins),
        "tag" => tag(ins),
        "is_error" => is_error(ins),
        "ev_lookup" => ev_lookup(ins),
        _ => return false,
    }
    true
}

// ---- allocation and reference counting ----

const ALLOC_NEED: u32 = 2;
const ALLOC_CUR: u32 = 3;
const ALLOC_PREV: u32 = 4;
const ALLOC_NEXT: u32 = 5;
const ALLOC_P: u32 = 6;

fn alloc(b: &mut Builder, ins: &mut InstructionSink) {
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
    // Bump from the next 8-byte boundary (`cabi_realloc` may leave the heap end unaligned).
    ins.global_get(G_HEAP);
    ins.i32_const(7);
    ins.i32_add();
    ins.i32_const(-8);
    ins.i32_and();
    ins.local_set(ALLOC_P);
    ins.local_get(ALLOC_P);
    ins.local_get(ALLOC_NEED);
    ins.i32_add();
    ins.call(rt(b, "heap_reserve"));
    ins.local_get(ALLOC_P);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_P);
    ins.local_get(1);
    ins.i32_store(mem(8, 4));
    ins.local_get(ALLOC_P);
}

/// `heap_reserve(end)`: moves the bump pointer to `end`, growing the memory by whole pages when
/// `end` is past it. Shared by `alloc` and `cabi_realloc`.
fn heap_reserve(ins: &mut InstructionSink) {
    const BYTES: u32 = 1;
    ins.memory_size(0);
    ins.i32_const(PAGE as i32);
    ins.i32_mul();
    ins.local_set(BYTES);
    ins.local_get(0);
    ins.local_get(BYTES);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(BYTES);
    ins.i32_sub();
    ins.i32_const((PAGE - 1) as i32);
    ins.i32_add();
    ins.i32_const(PAGE as i32);
    ins.i32_div_u();
    ins.memory_grow(0);
    ins.drop();
    ins.end();
    ins.local_get(0);
    ins.global_set(G_HEAP);
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
