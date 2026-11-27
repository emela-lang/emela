//! `Std.Env` on WASI 0.3: `wasi:cli/environment.get-environment`.
//!
//! The host returns a `list<tuple<string, string>>`, so the guest passes a return pointer
//! (`HOSTBUF`) and the host allocates the list and its strings in linear memory through the
//! exported `cabi_realloc` (in `canon`). `env_find` returns the matching entry, or 0.
//!
//! The find returns the first match in the host's list; the WIT contract of `get-environment`
//! holds one value per name. The environment is a value import, so the list is
//! fetched once and cached in the `G_ENV_*` globals; `has` and `get` then see the same list. A
//! name or value that is not UTF-8 cannot appear in the WIT `list<tuple<string, string>>`; the
//! host decides what it exposes (wasmtime rejects an inherited environment with such a variable
//! before the program starts), so only UTF-8 names can match.
//!
//! The list and its strings are allocated by the host through `cabi_realloc` and are kept for the
//! rest of the command: the runtime has no raw-byte free list, and the bump allocator keeps them
//! out of the object heap. Fetching the environment once bounds that memory to one copy.
//!
//! A lookup does not scan the list. The first lookup builds a name -> entry index over the list
//! in one block from the runtime allocator ([`env_build`]): an open-addressing table of entry
//! pointers, keyed by FNV-1a over the name bytes, with the length compared before the bytes. The
//! environment is immutable for the whole process (`get-environment` is a value import and the
//! core has no writer), so the table is never reclaimed and the scan runs once, not per lookup.
//! Duplicate names keep the first entry: the table is filled in the host's order and an insert
//! that finds an equal name does not replace it.
//!
//! An empty list builds no table: every lookup is a miss, and [`guard_empty`] answers it without
//! a call.

use super::*;

pub(in crate::emit::runtime) const FUNCS: &[Sig] = &[
    ("env_find", &[ValType::I32], &[ValType::I32]),
    ("env_build", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("env_hash", &[ValType::I32, ValType::I32], &[ValType::I32]),
    (
        "env_key_eq",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("env_has", &[ValType::I32], &[ValType::I32]),
    ("env_get", &[ValType::I32], &[ValType::I32]),
];

pub(in crate::emit::runtime) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        // LP, P, MASK, SLOTS, H, J, S, LN, NL, NP
        "env_find" => i32s(10),
        // CAP, MASK, SLOTS, I, E, J, S, K, P
        "env_build" => i32s(9),
        // H, J
        "env_hash" => i32s(2),
        // J, K, P
        "env_key_eq" => i32s(3),
        // entry, value pointer, value length
        "env_get" => i32s(3),
        _ => Vec::new(),
    }
}

pub(in crate::emit::runtime) fn used(b: &Builder) -> bool {
    b.uses_env()
}

pub(in crate::emit::runtime) fn emit(
    b: &mut Builder,
    name: &str,
    ins: &mut InstructionSink,
) -> bool {
    match name {
        "env_find" => env_find(b, ins),
        "env_build" => env_build(b, ins),
        "env_hash" => env_hash(ins),
        "env_key_eq" => env_key_eq(ins),
        "env_has" => env_has(b, ins),
        "env_get" => env_get(b, ins),
        _ => return false,
    }
    true
}

/// The key of the cache's cell in `Builder::rt_data`.
const CACHE_CELL_KEY: &str = "env.cache";
/// The mask of the cache's slot count, the block's first word, and the offset of its slots: the
/// mask is one less than a power of two, and 0 marks an empty slot.
const CACHE_MASK: u32 = HDR;
const CACHE_SLOTS: u32 = HDR + 4;
/// The smallest number of slots; the table doubles until it holds twice the entries.
const CACHE_MIN_SLOTS: u32 = 8;

/// FNV-1a, 32 bits: the offset basis and the prime. The finalizer's multiplier spreads the hash
/// over the low bits, which the table's slot index reads.
const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;
const FINAL_MIX: u32 = 0x7feb_352d;

/// The static cell that holds the pointer of the cache block (0 until the first build).
fn cache_cell(b: &mut Builder) -> u32 {
    if let Some(&off) = b.rt_data.get(CACHE_CELL_KEY) {
        return off;
    }
    let off = b.alloc_static(4);
    b.rt_data.insert(CACHE_CELL_KEY, off);
    off
}

/// The descriptor of the cache block: words the runtime only ever reads and never drops, so it
/// has no reference slots. Its shape is never used; the block carries it because `alloc` builds
/// every object with one.
fn cache_desc(b: &mut Builder) -> u32 {
    b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "env_cache".to_owned(),
        show: None,
        refs: Vec::new(),
        stride: 0,
    })
}

/// `env_find(name)`: the host's entry for `name` (a pointer to `{key.ptr, key.len, value.ptr,
/// value.len}`), or 0.
fn env_find(b: &mut Builder, ins: &mut InstructionSink) {
    const LP: u32 = 1;
    const P: u32 = 2;
    const MASK: u32 = 3;
    const SLOTS: u32 = 4;
    const H: u32 = 5;
    const J: u32 = 6;
    const S: u32 = 7;
    const LN: u32 = 8;
    const NL: u32 = 9;
    const NP: u32 = 10;

    let get = b.environment_get();
    let build = rt(b, "env_build");
    let hash = rt(b, "env_hash");
    let key_eq = rt(b, "env_key_eq");
    let cell = cache_cell(b);

    // Fetch the environment list once. The WIT contract says `get-environment` returns the same
    // values each time it is called (it is morally a value import).
    ins.global_get(G_ENV_LOADED);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(HOSTBUF as i32);
    ins.call(get);
    ins.i32_const(HOSTBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_ENV_PTR);
    ins.i32_const((HOSTBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_ENV_LEN);
    ins.i32_const(1);
    ins.global_set(G_ENV_LOADED);
    ins.end();
    ins.global_get(G_ENV_PTR);
    ins.local_set(LP);
    ins.global_get(G_ENV_LEN);
    ins.local_set(LN);
    // Nothing can match an empty list, so there is no table to build and nothing to probe.
    ins.local_get(LN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The name is a string object: its length is the first word after the header and its bytes
    // follow it.
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(NL);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(NP);
    // Build the index on the first lookup and keep it in the static cell for the rest of the
    // command.
    ins.i32_const(cell as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(P);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(LP);
    ins.local_get(LN);
    ins.call(build);
    ins.local_set(P);
    ins.i32_const(cell as i32);
    ins.local_get(P);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.local_get(P);
    ins.i32_load(mem(CACHE_MASK, 4));
    ins.local_set(MASK);
    ins.local_get(P);
    ins.i32_const(CACHE_SLOTS as i32);
    ins.i32_add();
    ins.local_set(SLOTS);
    // Probe from the hash of the name. An empty slot ends the search, and the first entry whose
    // name is equal is the answer (the table keeps the first of duplicate names).
    ins.local_get(NP);
    ins.local_get(NL);
    ins.call(hash);
    ins.local_set(H);
    ins.local_get(H);
    ins.local_get(MASK);
    ins.i32_and();
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(SLOTS);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(S);
    ins.local_get(NP);
    ins.local_get(NL);
    ins.call(key_eq);
    ins.if_(BlockType::Empty);
    ins.local_get(S);
    ins.return_();
    ins.end();
    // A different name with the same slot: probe the next one. The table is never full, because
    // it has at least twice as many slots as entries.
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(MASK);
    ins.i32_and();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// `env_build(list, count)`: the cache block for the host's list, an open-addressing table of
/// entry pointers. The first word is the mask of the slot count (one less than its power of
/// two), then the slots; 0 marks an empty slot, and an entry pointer is never 0.
fn env_build(b: &mut Builder, ins: &mut InstructionSink) {
    const CAP: u32 = 2;
    const MASK: u32 = 3;
    const SLOTS: u32 = 4;
    const I: u32 = 5;
    const E: u32 = 6;
    const J: u32 = 7;
    const S: u32 = 8;
    const K: u32 = 9;
    const P: u32 = 10;

    let alloc = rt(b, "alloc");
    let hash = rt(b, "env_hash");
    let key_eq = rt(b, "env_key_eq");
    let desc = cache_desc(b);

    // Twice the entries keeps the probe short and leaves an empty slot for every search.
    ins.i32_const(CACHE_MIN_SLOTS as i32);
    ins.local_set(CAP);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CAP);
    ins.local_get(1);
    ins.i32_const(1);
    ins.i32_shl();
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(CAP);
    ins.i32_const(1);
    ins.i32_shl();
    ins.local_set(CAP);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(CAP);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(MASK);
    // The mask word and the slots are `CAP + 1` words, rounded up to the allocator's 8-byte
    // slots.
    ins.local_get(CAP);
    ins.i32_const(2);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_shr_u();
    ins.i32_const(desc as i32);
    ins.call(alloc);
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(MASK);
    ins.i32_store(mem(CACHE_MASK, 4));
    ins.local_get(P);
    ins.i32_const(CACHE_SLOTS as i32);
    ins.i32_add();
    ins.local_set(SLOTS);
    // The allocator's blocks hold whatever the previous object left, so the slots start empty.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(CAP);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(SLOTS);
    ins.local_get(I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // Insert the entries in the host's order. An equal name is left where it is, so the first
    // entry of a duplicate name is the one every lookup finds.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(1);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(I);
    ins.i32_const(16);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(E);
    ins.local_get(E);
    ins.i32_load(mem(4, 4));
    ins.local_set(K);
    ins.local_get(E);
    ins.i32_load(mem(0, 4));
    ins.local_get(K);
    ins.call(hash);
    ins.local_get(MASK);
    ins.i32_and();
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(SLOTS);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(SLOTS);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(E);
    ins.i32_store(mem(0, 4));
    // Leave the probe block: this entry is in the table.
    ins.br(2);
    ins.end();
    ins.local_get(S);
    ins.local_get(E);
    ins.i32_load(mem(0, 4));
    ins.local_get(K);
    ins.call(key_eq);
    ins.if_(BlockType::Empty);
    // The same name was inserted earlier: the first entry stays.
    ins.br(2);
    ins.end();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(MASK);
    ins.i32_and();
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
    ins.local_get(P);
}

/// `h = (h ^ word at ptr + offset) * prime`.
fn mix_word(ins: &mut InstructionSink, h: u32, ptr: u32, offset: u32) {
    ins.local_get(h);
    ins.local_get(ptr);
    ins.local_get(offset);
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.i32_xor();
    ins.i32_const(FNV_PRIME as i32);
    ins.i32_mul();
    ins.local_set(h);
}

/// `h = (h ^ the byte at ptr + offset) * prime`.
fn mix_byte(ins: &mut InstructionSink, h: u32, ptr: u32, offset: u32) {
    ins.local_get(h);
    ins.local_get(ptr);
    ins.local_get(offset);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_xor();
    ins.i32_const(FNV_PRIME as i32);
    ins.i32_mul();
    ins.local_set(h);
}

/// `env_hash(ptr, len)`: FNV-1a, 32 bits, mixing one 32-bit word per step: a byte at a time
/// makes the multiply a loop-carried dependency of four cycles per byte, and the names are
/// short. The length goes in first, and the last word of a name that is not a multiple of four
/// bytes overlaps the word before it, so every byte is still mixed and no tail loop is needed.
/// Names shorter than four bytes are mixed a byte at a time.
///
/// The slot comes from the low bits, which FNV-1a mixes poorly (names that differ in their last
/// bytes only, like `X1` to `X1999`, collide), so the hash ends with the finalizer of
/// MurmurHash3.
fn env_hash(ins: &mut InstructionSink) {
    const H: u32 = 2;
    const J: u32 = 3;
    const LEN: u32 = 1;
    const PTR: u32 = 0;
    ins.i32_const(FNV_OFFSET as i32);
    ins.local_get(LEN);
    ins.i32_xor();
    ins.local_set(H);
    ins.local_get(LEN);
    ins.i32_const(4);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_get(LEN);
    ins.i32_gt_u();
    ins.br_if(1);
    mix_word(ins, H, PTR, J);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    // The bytes after the last whole word, in the word that ends at the last byte.
    ins.local_get(J);
    ins.local_get(LEN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(LEN);
    ins.i32_const(4);
    ins.i32_sub();
    ins.local_set(J);
    mix_word(ins, H, PTR, J);
    ins.end();
    ins.else_();
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    mix_byte(ins, H, PTR, J);
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    // `h ^= h >> 16; h *= 0x7feb_352d; h ^= h >> 15`.
    ins.local_get(H);
    ins.local_get(H);
    ins.i32_const(16);
    ins.i32_shr_u();
    ins.i32_xor();
    ins.local_set(H);
    ins.local_get(H);
    ins.i32_const(FINAL_MIX as i32);
    ins.i32_mul();
    ins.local_set(H);
    ins.local_get(H);
    ins.local_get(H);
    ins.i32_const(15);
    ins.i32_shr_u();
    ins.i32_xor();
    ins.local_set(H);
    ins.local_get(H);
}

/// `env_key_eq(entry, ptr, len)`: 1 when the entry's key has the length and the bytes of the
/// name at `ptr`. The length is compared first, so a name of another length costs one load; the
/// bytes are then compared a word at a time, with the last word overlapping.
fn env_key_eq(ins: &mut InstructionSink) {
    const J: u32 = 3;
    const K: u32 = 4;
    const A: u32 = 5;
    const B: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    ins.local_tee(K);
    ins.local_get(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(A);
    ins.i32_const(0);
    ins.local_set(J);
    ins.local_get(K);
    ins.i32_const(4);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_get(K);
    ins.i32_gt_u();
    ins.br_if(1);
    word_ne(ins, J);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(J);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    // The bytes after the last whole word, in the word that ends at the last byte.
    ins.local_get(J);
    ins.local_get(K);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(K);
    ins.i32_const(4);
    ins.i32_sub();
    ins.local_set(J);
    word_ne(ins, J);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.end();
    ins.else_();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(K);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(A);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_get(B);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ins.i32_const(1);
}

/// `(the word of the key at J) != (the word of the name at J)`, with the key in `A` and the name
/// in `B` (locals of [`env_key_eq`]).
fn word_ne(ins: &mut InstructionSink, j: u32) {
    const A: u32 = 5;
    const B: u32 = 1;
    ins.local_get(A);
    ins.local_get(j);
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_get(B);
    ins.local_get(j);
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.i32_ne();
}

/// `if (the list is loaded and empty) { return absent }`: the answer of every lookup when the
/// host exposes no variables.
///
/// An empty environment is the common answer of a lookup (`emela run` and `emela test` pass no
/// `--env`), and the probe below costs more than the guard: this skips the call into `env_find`
/// and the table. The length means nothing before the first lookup has fetched the list, so the
/// guard waits for `G_ENV_LOADED`.
fn guard_empty(ins: &mut InstructionSink, absent: u32) {
    ins.global_get(G_ENV_LOADED);
    ins.if_(BlockType::Empty);
    ins.global_get(G_ENV_LEN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(absent as i32);
    ins.return_();
    ins.end();
    ins.end();
}

fn env_has(b: &mut Builder, ins: &mut InstructionSink) {
    guard_empty(ins, 0);
    ins.local_get(0);
    ins.call(rt(b, "env_find"));
    ins.i32_const(0);
    ins.i32_ne();
}

fn env_get(b: &mut Builder, ins: &mut InstructionSink) {
    const ENTRY: u32 = 1;
    const VALUE: u32 = 2;
    const LEN: u32 = 3;
    let empty = b.string_literal(b"");
    guard_empty(ins, empty);
    ins.local_get(0);
    ins.call(rt(b, "env_find"));
    ins.local_set(ENTRY);
    ins.local_get(ENTRY);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // `Std.Env` only asks for the value after `has`, but an empty string is a safe answer if the
    // environment changed in between.
    ins.i32_const(empty as i32);
    ins.return_();
    ins.end();
    ins.local_get(ENTRY);
    ins.i32_load(mem(8, 4));
    ins.local_set(VALUE);
    ins.local_get(ENTRY);
    ins.i32_load(mem(12, 4));
    ins.local_set(LEN);
    ins.local_get(VALUE);
    ins.local_get(LEN);
    ins.call(rt(b, "str_from_raw"));
}
