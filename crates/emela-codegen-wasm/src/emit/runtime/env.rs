//! `Std.Env` on WASI 0.3: `wasi:cli/environment.get-environment`.
//!
//! The host returns a `list<tuple<string, string>>`, so the guest passes a return pointer
//! (`HOSTBUF`) and the host allocates the list and its strings in linear memory through the
//! exported `cabi_realloc` (in `canon`). `env_find` scans that list and returns the matching entry, or 0.
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

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("env_find", &[ValType::I32], &[ValType::I32]),
    ("env_has", &[ValType::I32], &[ValType::I32]),
    ("env_get", &[ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        // LP, LN, I, E, KL, KP, NL, J, OK
        "env_find" => i32s(9),
        // entry, value pointer, value length
        "env_get" => i32s(3),
        _ => Vec::new(),
    }
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_env()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "env_find" => env_find(b, ins),
        "env_has" => env_has(b, ins),
        "env_get" => env_get(b, ins),
        _ => return false,
    }
    true
}

/// `env_find(name)`: the host's entry for `name` (a pointer to `{key.ptr, key.len, value.ptr,
/// value.len}`), or 0.
fn env_find(b: &mut Builder, ins: &mut InstructionSink) {
    const LP: u32 = 1;
    const LN: u32 = 2;
    const I: u32 = 3;
    const E: u32 = 4;
    const KL: u32 = 5;
    const KP: u32 = 6;
    const NL: u32 = 7;
    const J: u32 = 8;
    const OK: u32 = 9;

    let get = b.environment_get();

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
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(NL);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // Past the last entry: not found.
    ins.local_get(I);
    ins.local_get(LN);
    ins.i32_ge_u();
    ins.br_if(1);
    // E = LP + I * 16
    ins.local_get(LP);
    ins.local_get(I);
    ins.i32_const(16);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(E);
    ins.local_get(E);
    ins.i32_load(mem(4, 4));
    ins.local_set(KL);
    ins.local_get(KL);
    ins.local_get(NL);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.local_get(E);
    ins.i32_load(mem(0, 4));
    ins.local_set(KP);
    ins.i32_const(1);
    ins.local_set(OK);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(NL);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(KP);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(OK);
    ins.br(2);
    ins.end();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(OK);
    ins.if_(BlockType::Empty);
    ins.local_get(E);
    ins.return_();
    ins.end();
    ins.end();
    // I += 1; continue.
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

fn env_has(b: &mut Builder, ins: &mut InstructionSink) {
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
