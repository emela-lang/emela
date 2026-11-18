//! The allocator, reference counting, descriptors and evidence lookup.

use emela_lir::opt::drop_spec::DropKind;

use super::*;

/// The line written to standard error before a failed heap reservation traps. It is best effort:
/// the specification does not require an allocation failure to write anything, and a reservation
/// made from `cabi_realloc` does not write it at all ([`heap_reserve`]).
const OOM_MESSAGE: &[u8] = b"emela: out of memory\n";
/// Where [`setup`] records the offset of [`OOM_MESSAGE`] in `Builder::rt_data`.
const OOM_MESSAGE_KEY: &str = "alloc.oom_message";

/// The size of the largest block served by a size-class bin, in bytes. A block is the object
/// header plus 8-byte slots, so its size is a multiple of 8: the bins are 16, 24, ..., 256.
const MAX_CLASS: u32 = 256;
/// The number of size classes, one per multiple of 8 from [`HDR`] to [`MAX_CLASS`].
const BINS: u32 = (MAX_CLASS - HDR) / 8 + 1;
/// Where [`setup`] records the offset of the bin heads in `Builder::rt_data`. There is one i32
/// head per size class: the first free block of that size, linked to the next through `aux`.
const BINS_KEY: &str = "alloc.bins";
/// Where [`setup`] records the offset of the count of blocks in the bins. It is zero exactly
/// when every bin is empty, so an allocation in a phase that only bumps skips the search.
const COUNT_KEY: &str = "alloc.count";

pub(super) const FUNCS: &[Sig] = &[
    ("alloc", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("heap_reserve", &[ValType::I64, ValType::I32], &[]),
    ("dup", &[ValType::I32], &[]),
    ("drop", &[ValType::I32], &[]),
    ("tag", &[ValType::I32], &[ValType::I32]),
    ("is_error", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ev_lookup", &[ValType::I32, ValType::I32], &[ValType::I32]),
];

/// The block drop of drop specialisation (C3/PR-027) is its own runtime module, appended after
/// every other one: a program without a specialised drop keeps the function indices and the bytes
/// it had, and the lowering ([`drop_block_used`]) only adds it when a function has one.
pub(super) const DROP_BLOCK_FUNCS: &[Sig] = &[("drop_block", &[ValType::I32], &[])];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "alloc" => i32s(6),                        // need, cur, prev, next, p, idx
        "heap_reserve" => vec![(1, ValType::I64)], // memory bytes, after the parameters
        "dup" => i32s(1),                          // rc
        "drop" => i32s(13),                        // rc..size, slot
        "ev_lookup" => i32s(3),                    // count, i, e
        _ => Vec::new(),                           // other modules own their locals
    }
}

pub(super) fn drop_block_locals(_name: &str) -> Vec<(u32, ValType)> {
    // rc, size, and `free_raw`'s bin-slot scratch (local 13).
    i32s(13)
}

/// Writes the out-of-memory line and the bin heads into static data before any runtime body is
/// emitted. The bin heads start out zero: every bin is empty.
pub(super) fn setup(b: &mut Builder) {
    let off = b.alloc_static(OOM_MESSAGE.len() as u32);
    let at = off as usize;
    b.data[at..at + OOM_MESSAGE.len()].copy_from_slice(OOM_MESSAGE);
    b.rt_data.insert(OOM_MESSAGE_KEY, off);
    let bins = b.alloc_static(BINS * 4);
    b.rt_data.insert(BINS_KEY, bins);
    let count = b.alloc_static(4);
    b.rt_data.insert(COUNT_KEY, count);
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "alloc" => alloc(b, ins),
        "heap_reserve" => heap_reserve(b, ins),
        "dup" => dup(ins),
        "drop" => drop_(b, ins),
        "drop_block" => drop_block(b, ins),
        "tag" => tag(ins),
        "is_error" => is_error(ins),
        "ev_lookup" => ev_lookup(ins),
        _ => return false,
    }
    true
}

/// Whether a function of the module has a `drop` drop specialisation classified as a block release
/// (C3/PR-027). The runtime module of [`DROP_BLOCK_FUNCS`] is only registered when one has, so a
/// program the flag did not run on keeps its bytes.
pub(super) fn drop_block_used(b: &Builder) -> bool {
    b.m.functions
        .iter()
        .any(|(_, f)| f.drop_kinds.values().any(|k| *k == DropKind::Block))
}

// ---- allocation and reference counting ----

const ALLOC_NEED: u32 = 2;
const ALLOC_CUR: u32 = 3;
const ALLOC_PREV: u32 = 4;
const ALLOC_NEXT: u32 = 5;
const ALLOC_P: u32 = 6;
/// The bin slot `alloc` searches from, and the bin slot a bump records: a static address.
const ALLOC_IDX: u32 = 7;

/// Where an object's header keeps its block: its size in bytes, or, for a block small enough
/// for a bin, the address of the bin slot with bit 0 set (see [`free_raw`]).
const SIZE_AT: u32 = 12;

/// Writes the slot address of a block of `size` bytes into `out`: `bins - 8 + (size >> 3) * 4`.
/// The two classes below [`HDR`] leave one empty slot each, and the caller has checked that
/// `size` is one of the [`BINS`] class sizes. The shift is what a bump costs; a reuse keeps the
/// address stored in the block.
fn bin_slot(b: &Builder, ins: &mut InstructionSink, size: u32, out: u32) {
    ins.i32_const(bins_offset(b) as i32 - 8);
    ins.local_get(size);
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.i32_const(2);
    ins.i32_shl();
    ins.i32_add();
    ins.local_set(out);
}

fn bins_offset(b: &Builder) -> u32 {
    *b.rt_data
        .get(BINS_KEY)
        .expect("alloc::setup writes the bin heads")
}

fn count_offset(b: &Builder) -> u32 {
    *b.rt_data
        .get(COUNT_KEY)
        .expect("alloc::setup writes the bin count")
}

/// Allocates a block for an object of `size` slots and stores `descriptor` in its header.
///
/// A free block is reused before the bump pointer moves. Blocks up to [`MAX_CLASS`] come from
/// `BINS` size-class bins, one per block size, searched upward from the requested class so the
/// smallest fitting block is taken. A block keeps the size class of its block when a smaller
/// object reuses it, and `drop` gives the whole block back to that class.
///
/// A request no bin can serve falls back to the first-fit list in `G_FREE`, then to the bump
/// pointer (`G_HEAP`). In this backend an allocation failure is not a defect: [`heap_reserve`]
/// traps before the bump pointer moves.
fn alloc(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(ALLOC_NEED);
    // The size-class path, for a block no larger than the last bin. The count of bin blocks
    // keeps a phase that only bumps from walking the bins; otherwise they are searched from the
    // requested class upward, so the block taken is the smallest one that fits.
    ins.local_get(ALLOC_NEED);
    ins.i32_const(MAX_CLASS as i32);
    ins.i32_le_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(count_offset(b) as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    bin_slot(b, ins, ALLOC_NEED, ALLOC_IDX);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ALLOC_IDX);
    ins.i32_const((bins_offset(b) + BINS * 4) as i32);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(ALLOC_IDX);
    ins.i32_load(mem(0, 4));
    ins.local_set(ALLOC_CUR);
    ins.local_get(ALLOC_CUR);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(ALLOC_IDX);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_set(ALLOC_IDX);
    ins.br(1);
    ins.end();
    // Pop the head of the bin. The block's size word still names its bin, so reuse does not
    // recompute it.
    ins.local_get(ALLOC_CUR);
    ins.i32_load(mem(4, 4));
    ins.local_set(ALLOC_NEXT);
    ins.local_get(ALLOC_IDX);
    ins.local_get(ALLOC_NEXT);
    ins.i32_store(mem(0, 4));
    ins.i32_const(count_offset(b) as i32);
    ins.i32_const(count_offset(b) as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_CUR);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_CUR);
    ins.local_get(1);
    ins.i32_store(mem(8, 4));
    ins.local_get(ALLOC_CUR);
    ins.return_();
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // First fit over the blocks larger than the last class.
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
    // The block keeps its own size, which can be more than this object needs: `drop` gives
    // back the whole block, so reusing it does not shrink it.
    ins.local_get(ALLOC_CUR);
    ins.local_get(ALLOC_CUR);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(SIZE_AT, 4));
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
    ins.i64_extend_i32_u();
    ins.local_get(ALLOC_NEED);
    ins.i64_extend_i32_u();
    ins.i64_add();
    // `alloc` does not run inside a host callback, so its failure writes the line.
    ins.i32_const(0);
    ins.call(rt(b, "heap_reserve"));
    ins.local_get(ALLOC_P);
    // A small block records its bin slot (with bit 0 set) instead of its size, so `free_raw`
    // gives it back without recomputing the class; a larger block records its size, whose low
    // bits are zero because it is a multiple of 8.
    ins.local_get(ALLOC_NEED);
    ins.i32_const(MAX_CLASS as i32);
    ins.i32_le_u();
    ins.if_(BlockType::Result(ValType::I32));
    bin_slot(b, ins, ALLOC_NEED, ALLOC_IDX);
    ins.local_get(ALLOC_IDX);
    ins.i32_const(1);
    ins.i32_or();
    ins.else_();
    ins.local_get(ALLOC_NEED);
    ins.end();
    ins.i32_store(mem(SIZE_AT, 4));
    ins.local_get(ALLOC_P);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.local_get(ALLOC_P);
    ins.local_get(1);
    ins.i32_store(mem(8, 4));
    ins.local_get(ALLOC_P);
}

/// `heap_reserve(end, quiet)`: moves the bump pointer to `end`, growing the memory by whole pages
/// when `end` is past it. Shared by `alloc` and `cabi_realloc`.
///
/// `end` is the sum of two u32 offsets (`alloc`: the aligned heap pointer and the block size;
/// `cabi_realloc`: the aligned heap pointer and the new size), so it is taken as an i64: the sum
/// can pass 4 GiB, and an i32 would wrap it to a small value. The checks below then end the
/// program instead of (a) taking free space below 4 GiB for exhausted memory, because
/// `memory.size * PAGE` also needs 2^32 at 65536 pages, or (b) moving `G_HEAP` back to 0 and
/// storing over the beginning of the heap.
///
/// A failed `memory.grow` returns `-1`. The program then ends abruptly: it traps
/// (`unreachable`), and `G_HEAP` keeps its old value, so no store is made past the end of the
/// memory. In this backend an allocation failure is not a defect: it does not set `G_DEFECT`,
/// does not run releases, and cannot be observed at `Exit.attempt`. The classification, the
/// message and the trap are implementation-defined.
///
/// `quiet` is non-zero for `cabi_realloc`, which runs inside a host call: a blocking write from
/// there can be forbidden by the host, so the reservation traps without writing the line. With
/// `quiet` zero, the trap writes [`OOM_MESSAGE`] first, best effort.
fn heap_reserve(b: &mut Builder, ins: &mut InstructionSink) {
    // Parameters 0 (`end`) and 1 (`quiet`); locals start at 2.
    const BYTES: u32 = 2;
    // Past the last address a wasm32 pointer can hold.
    ins.local_get(0);
    ins.i64_const(u32::MAX as i64);
    ins.i64_gt_u();
    ins.if_(BlockType::Empty);
    out_of_memory(b, ins);
    ins.end();
    // The heap only grows, so an `end` below `G_HEAP` means the caller's rounding wrapped.
    ins.local_get(0);
    ins.global_get(G_HEAP);
    ins.i64_extend_i32_u();
    ins.i64_lt_u();
    ins.if_(BlockType::Empty);
    out_of_memory(b, ins);
    ins.end();
    // At 65536 pages the size in bytes is 2^32, which an i32 would wrap to 0.
    ins.memory_size(0);
    ins.i64_extend_i32_u();
    ins.i64_const(PAGE as i64);
    ins.i64_mul();
    ins.local_set(BYTES);
    ins.local_get(0);
    ins.local_get(BYTES);
    ins.i64_gt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(BYTES);
    ins.i64_sub();
    ins.i64_const((PAGE - 1) as i64);
    ins.i64_add();
    ins.i64_const(PAGE as i64);
    ins.i64_div_u();
    ins.i32_wrap_i64();
    ins.memory_grow(0);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    out_of_memory(b, ins);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_wrap_i64();
    ins.global_set(G_HEAP);
}

/// Ends the program after a reservation cannot be made: traps, after writing the line unless the
/// reservation is quiet. The caller must keep `G_HEAP` at its old value.
///
/// The quiet flag is parameter 1 of [`heap_reserve`].
fn out_of_memory(b: &Builder, ins: &mut InstructionSink) {
    ins.local_get(1);
    ins.if_(BlockType::Empty);
    ins.unreachable();
    ins.end();
    out_of_memory_loud(b, ins);
}

/// [`out_of_memory`] for a caller other than [`heap_reserve`], which has no quiet flag: writes the
/// line, best effort, and traps. An `array_new` count that cannot fit in the i32 `alloc` sizes its
/// block with ends here rather than in a wrapped block (issue #494).
pub(super) fn out_of_memory_loud(b: &Builder, ins: &mut InstructionSink) {
    let message = *b
        .rt_data
        .get(OOM_MESSAGE_KEY)
        .expect("alloc::setup writes the out-of-memory line");
    ins.i32_const(message as i32);
    ins.i32_const(OOM_MESSAGE.len() as i32);
    ins.call(rt(b, "write_stderr"));
    ins.unreachable();
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

fn drop_(b: &Builder, ins: &mut InstructionSink) {
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
    const SIZE: u32 = 12;
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
    // The worklist of objects to free is linked through `aux`. The first object's `aux` may hold
    // anything (a stale free-list link from a reused block, a string's self-reference marker), so
    // it ends the list here; the others get theirs when they are pushed.
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_store(mem(4, 4));
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
    ins.local_get(Q);
    ins.i32_load(mem(SIZE_AT, 4));
    ins.local_set(SIZE);
    // An array (`array.rs`) holds references in slots 1 to its length when its descriptor's
    // `refs[0]` says so. Only a program with arrays has this branch, so the others keep their
    // bytes.
    let arrays = super::array::used(b);
    if arrays {
        ins.local_get(KIND);
        ins.i32_const(super::array::KIND as i32);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.local_get(D);
        ins.i32_load(mem(DESC_REFS, 4));
        ins.i32_load8_u(mem(0, 1));
        ins.if_(BlockType::Empty);
        ins.local_get(Q);
        ins.i32_load(mem(HDR, 4));
        ins.local_set(N);
        ins.i32_const(1);
        ins.local_set(I);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        ins.local_get(I);
        ins.local_get(N);
        ins.i32_gt_u();
        ins.br_if(1);
        release_slot(ins, Q, I, C, CRC, PUSH);
        ins.local_get(I);
        ins.i32_const(1);
        ins.i32_add();
        ins.local_set(I);
        ins.br(0);
        ins.end();
        ins.end();
        ins.end();
        ins.else_();
    }
    // A string holds no references.
    ins.local_get(KIND);
    ins.i32_const(1);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
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
    release_slot(ins, Q, I, C, CRC, PUSH);
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    if arrays {
        ins.end();
    }
    free_raw(b, ins, Q, SIZE);
    // Next object on the worklist (its children were pushed above).
    ins.br(0);
    ins.end();
    ins.end();
}

/// The block drop of drop specialisation (C3/PR-027): the generic `drop` without the descriptor
/// switch and the worklist. The type of the value proves that its payload holds no reference and
/// has no finalizer, so the block is given back directly. Static data (`rc == 0`) is untouched,
/// like `drop`.
fn drop_block(b: &Builder, ins: &mut InstructionSink) {
    const RC: u32 = 2;
    const SIZE: u32 = 3;
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
    // The size word says how the block goes back (a bin slot, or its size in bytes).
    ins.local_get(0);
    ins.i32_load(mem(SIZE_AT, 4));
    ins.local_set(SIZE);
    free_raw(b, ins, 0, SIZE);
}

/// Gives up the reference in slot `i` of the object `q` (locals): when it was the last one, the
/// object is pushed onto the worklist `push` of objects to free. `c` and `crc` are scratch.
fn release_slot(ins: &mut InstructionSink, q: u32, i: u32, c: u32, crc: u32, push: u32) {
    ins.local_get(q);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(c);
    ins.local_get(c);
    ins.if_(BlockType::Empty);
    ins.local_get(c);
    ins.i32_load(mem(0, 4));
    ins.local_set(crc);
    ins.local_get(crc);
    ins.if_(BlockType::Empty);
    ins.local_get(crc);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(crc);
    ins.local_get(c);
    ins.local_get(crc);
    ins.i32_store(mem(0, 4));
    ins.local_get(crc);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(c);
    ins.local_get(push);
    ins.i32_store(mem(4, 4));
    ins.local_get(c);
    ins.local_set(push);
    ins.end();
    ins.end();
    ins.end();
}

/// Gives `q` back as a free block. `size` is the block's size word: for a block with a bin, the
/// address of the bin slot with bit 0 set; otherwise the size in bytes. A free block links the
/// next block of its list through `aux`.
fn free_raw(b: &Builder, ins: &mut InstructionSink, q: u32, size: u32) {
    /// The scratch local of `drop` for the bin slot.
    const SLOT: u32 = 13;
    ins.local_get(size);
    ins.i32_const(1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(size);
    ins.i32_const(-2);
    ins.i32_and();
    ins.local_set(SLOT);
    // A free block is not static: keep a non-zero `rc`, as the first-fit list does, so a stale
    // reference is never mistaken for an `rc == 0` value.
    ins.local_get(q);
    ins.local_get(size);
    ins.i32_store(mem(0, 4));
    // Push onto the bin, linking the old head through `aux`.
    ins.local_get(q);
    ins.local_get(SLOT);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(4, 4));
    ins.local_get(SLOT);
    ins.local_get(q);
    ins.i32_store(mem(0, 4));
    ins.i32_const(count_offset(b) as i32);
    ins.i32_const(count_offset(b) as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.else_();
    ins.local_get(q);
    ins.local_get(size);
    ins.i32_store(mem(0, 4));
    ins.local_get(q);
    ins.global_get(G_FREE);
    ins.i32_store(mem(4, 4));
    ins.local_get(q);
    ins.global_set(G_FREE);
    ins.end();
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
