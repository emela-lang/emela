//! The canonical ABI helpers of the host calls that return a `list` or a `string`
//! (`get-environment`, `get-directories`, `read-directory`): the exported `cabi_realloc` the host
//! allocates them with, and `str_from_raw`, which copies the host's bytes into a string object.
//! `Std.Env` and `Std.FileSystem` share them.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    (
        "str_from_raw",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "cabi_realloc",
        &[ValType::I32, ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        // the new string
        "str_from_raw" => i32s(1),
        // pointer, copy length
        "cabi_realloc" => i32s(2),
        _ => Vec::new(),
    }
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_env() || b.uses_std_host(StdHost::is_fs)
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "str_from_raw" => str_from_raw(b, ins),
        "cabi_realloc" => cabi_realloc(b, ins),
        _ => return false,
    }
    true
}

/// `str_from_raw(ptr, len)`: a new string object holding the host's bytes.
fn str_from_raw(b: &mut Builder, ins: &mut InstructionSink) {
    const DST: u32 = 2;
    ins.local_get(1);
    ins.call(rt(b, "str_new"));
    ins.local_set(DST);
    ins.local_get(DST);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.local_get(1);
    ins.memory_copy(0, 0);
    ins.local_get(DST);
}

/// The canonical ABI's `cabi_realloc(old_ptr, old_size, align, new_size)`.
///
/// The run is bumped from the heap end rounded up to `align` only, so short host strings stay
/// packed; `alloc` rounds the heap end back up to 8 before the next object. Reallocation with a
/// live `old_ptr` copies the common bytes.
fn cabi_realloc(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 4;
    const COPY: u32 = 5;

    // `realloc(_, _, _, 0)` frees.
    ins.local_get(3);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // Shrinking in place.
    ins.local_get(0);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.local_get(1);
    ins.local_get(3);
    ins.i32_ge_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.return_();
    ins.end();
    // P = align_up(G_HEAP, align); `align` is a power of two.
    ins.global_get(G_HEAP);
    ins.local_get(2);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(0);
    ins.local_get(2);
    ins.i32_sub();
    ins.i32_and();
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(3);
    ins.i32_add();
    ins.call(rt(b, "heap_reserve"));
    // Copy the old contents when growing in place was not possible.
    ins.local_get(0);
    ins.i32_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.local_get(3);
    ins.local_get(1);
    ins.local_get(3);
    ins.i32_lt_u();
    ins.select();
    ins.local_set(COPY);
    ins.local_get(P);
    ins.local_get(0);
    ins.local_get(COPY);
    ins.memory_copy(0, 0);
    ins.end();
    ins.local_get(P);
}
