//! The WASI host calls, the runtime side of the internal host ABI, and the `Std.Console` /
//! `Std.Env` / `Std.Random` hosts.
//!
//! `write` wraps `fd_write`; `debug_print` writes a string to stderr and `print_string` to stdout
//! (both with a trailing newline); `console_log` / `console_error` do the same for `Std.Console`
//! (fd 1 and fd 2); `exit` calls `proc_exit`. `Std.Env` reads `environ_sizes_get` / `environ_get`
//! and `Std.Random` fills a buffer with `random_get`. The string layout is the one the emitter's
//! `string_literal` uses and `str_byte_size` reads: the length is the `i32` at `ptr + HDR`, the
//! UTF-8 bytes start at `ptr + HDR + 4`.

use super::super::layout::{IOVEC, NEWLINE, NWRITTEN};
use super::*;

/// The host-call fragment, between `CORE_HEAD` and `CORE_TAIL` (see `runtime::FUNCS`).
///
/// `print_string` is emitted only when an internal host references it (`INTERNAL_HOSTS`); see
/// `runtime::is_on_demand`. That keeps the modules of programs that use no internal host
/// unchanged.
pub(super) const FUNCS: &[Sig] = &[
    ("debug_print", &[ValType::I32], &[]),
    ("write", &[ValType::I32, ValType::I32, ValType::I32], &[]),
    ("exit", &[ValType::I32], &[]),
    ("print_string", &[ValType::I32], &[]),
    ("console_log", &[ValType::I32], &[]),
    ("console_error", &[ValType::I32], &[]),
    ("env_has", &[ValType::I32], &[ValType::I32]),
    ("env_get", &[ValType::I32], &[ValType::I32]),
    ("random_int", &[], &[ValType::I32]),
    ("random_float", &[], &[ValType::F64]),
];

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "debug_print" => write_line(b, ins, 2),
        "write" => write(b, ins),
        "exit" => exit(b, ins),
        "print_string" => write_line(b, ins, 1),
        "console_log" => write_line(b, ins, 1),
        "console_error" => write_line(b, ins, 2),
        "env_has" => env_has(b, ins),
        "env_get" => env_get(b, ins),
        "random_int" => random_int(b, ins),
        "random_float" => random_float(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    match name {
        "env_has" | "env_get" => Some(vec![(12, ValType::I32)]),
        _ => None,
    }
}

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

/// Writes the bytes of the string in local 0 (length at `[ptr + HDR]`, bytes at `ptr + HDR + 4`)
/// and a newline to the file descriptor `fd`. All of `debug_print`, `print_string` and the
/// `Console` internal hosts are this with a different descriptor.
fn write_line(b: &mut Builder, ins: &mut InstructionSink, fd: i32) {
    let write = rt(b, "write");
    ins.i32_const(fd);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(10);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(fd);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(1);
    ins.call(write);
}

fn exit(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.call(b.proc_exit());
    ins.unreachable();
}

// ---- Std.Env ----

/// The locals shared by [`environ_find`] and the `Std.Env` hosts that inline it.
const ENV_COUNT: u32 = 1;
const ENV_ENTRIES: u32 = 3;
const ENV_BUF: u32 = 4;
const ENV_I: u32 = 5;
const ENV_ENTRY: u32 = 6;
const ENV_J: u32 = 7;
const ENV_NAME_LEN: u32 = 8;
const ENV_LEN: u32 = 9;
const ENV_K: u32 = 10;
const ENV_OUT: u32 = 11;
const ENV_STRING: u32 = 12;

/// `env_has(name)`: whether the environment variable `name` is set.
fn env_has(b: &mut Builder, ins: &mut InstructionSink) {
    environ_find(b, ins);
    ins.i32_const(0);
    ins.i32_ne();
}

/// `env_get(name)`: a copy of the value of the environment variable `name`, or the empty string.
/// The handler only calls it after `env_has`, so the empty string is only the not-set fallback.
fn env_get(b: &mut Builder, ins: &mut InstructionSink) {
    let str_new = rt(b, "str_new");
    environ_find(b, ins);
    ins.local_set(ENV_OUT);
    ins.local_get(ENV_OUT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.call(str_new);
    ins.return_();
    ins.end();
    // The value length, up to the NUL terminator of the `NAME=VALUE` entry.
    ins.i32_const(0);
    ins.local_set(ENV_LEN);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ENV_OUT);
    ins.local_get(ENV_LEN);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(ENV_LEN);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ENV_LEN);
    ins.br(0);
    ins.end();
    ins.end();
    // Copy the value into a fresh string.
    ins.local_get(ENV_LEN);
    ins.call(str_new);
    ins.local_set(ENV_STRING);
    ins.i32_const(0);
    ins.local_set(ENV_K);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ENV_K);
    ins.local_get(ENV_LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(ENV_STRING);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(ENV_K);
    ins.i32_add();
    ins.local_get(ENV_OUT);
    ins.local_get(ENV_K);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store8(mem(0, 1));
    ins.local_get(ENV_K);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ENV_K);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(ENV_STRING);
}

/// Leaves on the stack the pointer to the value (the character after `=`) of the environment
/// variable named by the string in local 0, or 0 if it is not set.
///
/// WASI has no `getenv`, so it asks `environ_sizes_get` for the count and size, copies the entry
/// pointers and the `NAME=VALUE\0` bytes into two fresh heap objects with `environ_get` and scans
/// them. It reuses the scratch `IOVEC` slot for the `environ_sizes_get` results.
///
/// **The two objects are intentionally leaked** (as the C host's `getenv` result is not owned):
/// freeing them needs the byte size the WASI calls only report through the scratch slot, and they
/// are read through the returned pointer until the caller copies the value. `Std.Env.get` calls
/// `env_has` and then `env_get`, so one lookup allocates four blocks. That is bounded by the
/// number of lookups and the process is a short-lived command, so it is not worth a `free_raw`
/// runtime function and a size-carrying protocol; a long-running host (`Std.Http`, …) that looks
/// the environment up in a loop should revisit this.
fn environ_find(b: &mut Builder, ins: &mut InstructionSink) {
    let alloc = rt(b, "alloc");
    let sizes_get = b.environ_sizes_get();
    let get = b.environ_get();
    // The count and the total size, at IOVEC and IOVEC + 4.
    ins.i32_const(IOVEC as i32);
    ins.i32_const((IOVEC + 4) as i32);
    ins.call(sizes_get);
    // A failed call leaves the scratch slot untouched: report "not set" instead of reading stale
    // bytes as a count. (An empty environment looks the same; there is no defect a host can
    // usefully raise for a missing variable.)
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(ENV_COUNT);
    ins.i32_const(0);
    ins.local_set(ENV_OUT);
    ins.local_get(ENV_COUNT);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.else_();
    // The pointer array (`count` pointers) and the byte buffer (`size` bytes) after a header.
    ins.local_get(ENV_COUNT);
    ins.global_get(G_STRDESC);
    ins.call(alloc);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(ENV_ENTRIES);
    // The total byte size is at `IOVEC + 4`, after the count at `IOVEC`.
    ins.i32_const((IOVEC + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(7);
    ins.i32_add();
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.global_get(G_STRDESC);
    ins.call(alloc);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_set(ENV_BUF);
    ins.local_get(ENV_ENTRIES);
    ins.local_get(ENV_BUF);
    ins.call(get);
    // A failed call leaves the two buffers without trustworthy entry pointers: treat the variable
    // as not set rather than scanning garbage.
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The name's byte length (the bytes start at `name + HDR + 4`).
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(ENV_NAME_LEN);
    ins.i32_const(0);
    ins.local_set(ENV_I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ENV_I);
    ins.local_get(ENV_COUNT);
    ins.i32_ge_u();
    ins.br_if(1);
    // The entry pointer.
    ins.local_get(ENV_ENTRIES);
    ins.local_get(ENV_I);
    ins.i32_const(4);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(ENV_ENTRY);
    // Compare the name, byte by byte, up to the name's length.
    ins.i32_const(0);
    ins.local_set(ENV_J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ENV_J);
    ins.local_get(ENV_NAME_LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(ENV_ENTRY);
    ins.local_get(ENV_J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(ENV_J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(ENV_J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ENV_J);
    ins.br(0);
    ins.end();
    ins.end();
    // A prefix match is only a match when `=` follows.
    ins.local_get(ENV_J);
    ins.local_get(ENV_NAME_LEN);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(ENV_ENTRY);
    ins.local_get(ENV_NAME_LEN);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(b'=' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(ENV_ENTRY);
    ins.local_get(ENV_NAME_LEN);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ENV_OUT);
    ins.end();
    ins.end();
    ins.local_get(ENV_I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ENV_I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(ENV_OUT);
}

// ---- Std.Random ----

/// `random_int()`: four bytes from WASI `random_get`, read as a signed 32-bit integer (the whole
/// `Int` range). The scratch `IOVEC` slot is a byte buffer here.
fn random_int(b: &mut Builder, ins: &mut InstructionSink) {
    random_bytes(b, ins, 4);
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
}

/// `random_float()`: uniform in `[0, 1)`. Eight bytes, the top 53 bits scaled by `2^-53`, exactly
/// as the C (`random.c`) and JS (`random.mjs`) hosts do: `((hi >>> 5) * 2^26 + (lo >>> 6)) / 2^53`.
fn random_float(b: &mut Builder, ins: &mut InstructionSink) {
    random_bytes(b, ins, 8);
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(5);
    ins.i32_shr_u();
    ins.f64_convert_i32_u();
    ins.f64_const(67108864.0.into());
    ins.f64_mul();
    ins.i32_const((IOVEC + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(6);
    ins.i32_shr_u();
    ins.f64_convert_i32_u();
    ins.f64_add();
    ins.f64_const(9007199254740992.0.into());
    ins.f64_div();
}

/// Fills `len` bytes at `IOVEC` with WASI `random_get`. A failed call leaves the scratch bytes
/// stale, so they are cleared: never return old scratch (or another host's leftovers) as a random
/// value.
fn random_bytes(b: &mut Builder, ins: &mut InstructionSink, len: u32) {
    ins.i32_const(IOVEC as i32);
    ins.i32_const(len as i32);
    ins.call(b.random_get());
    ins.if_(BlockType::Empty);
    ins.i32_const(IOVEC as i32);
    if len == 8 {
        ins.i64_const(0);
        ins.i64_store(mem(0, 8));
    } else {
        ins.i32_const(0);
        ins.i32_store(mem(0, 4));
    }
    ins.end();
}
