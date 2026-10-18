//! The file-system runtime: the WASI (preview1) syscalls, the `Std.FileSystem` internal hosts
//! and the opaque `Outcome` handle they return.
//!
//! The handlers of `std/file_system.emel` call the `host_*` functions, which are `suspend fn`s
//! (spec 12.6). The task-attachment of a suspending external lives in the fiber/scheduler track;
//! this module is the synchronous work underneath: each `host_*` binds (through
//! `emit::external::INTERNAL_HOSTS`) to one runtime function here, and the runtime function does
//! the WASI call at once and returns an `Outcome` (an `i32` pointer, see `emit::handle`).
//!
//! Relative paths are resolved against the first preopened directory (fd 3 for `wasmtime run
//! --dir=...`), the WASI stand-in for the process's current directory. Text is UTF-8 only: a
//! file that is not valid UTF-8 is `InvalidText` (never read with replacement characters). The
//! names of `list_directory` are sorted by name, which for UTF-8 is code point order.
//!
//! The whole fragment is emitted only when a program reaches a `Std.FileSystem` host, so the
//! modules of programs that do not keep the exact bytes they had (`runtime::register`).

use super::super::handle::{
    self, OUTCOME_ACCESS_DENIED, OUTCOME_COUNT, OUTCOME_ERROR, OUTCOME_FAILURE,
    OUTCOME_INVALID_TEXT, OUTCOME_IS_A_DIRECTORY, OUTCOME_MESSAGE, OUTCOME_NAMES,
    OUTCOME_NOT_A_DIRECTORY, OUTCOME_NOT_EMPTY, OUTCOME_NOT_FOUND, OUTCOME_OK, OUTCOME_SLOTS,
    OUTCOME_TEXT,
};
use super::super::layout::{IOVEC, NWRITTEN};
use super::*;

// ---- WASI preview1 constants ----

/// `oflags`: create the file if it does not exist.
const O_CREAT: i32 = 1 << 0;
/// `oflags`: the path must be a directory.
const O_DIRECTORY: i32 = 1 << 1;
/// `oflags`: truncate the file to zero length.
const O_TRUNC: i32 = 1 << 3;
/// `fdflags`: writes append to the end.
const FD_APPEND: i32 = 1 << 0;
/// `lookupflags`: follow a symbolic link at the end of the path.
const LOOKUP_FOLLOW: i32 = 1;

/// `rights`: read, seek and stat an opened file.
const RIGHTS_READ: i64 = (1 << 1) | (1 << 2) | (1 << 21);
/// `rights`: write, seek, stat and (for `O_TRUNC`) resize an opened file.
const RIGHTS_WRITE: i64 = (1 << 6) | (1 << 2) | (1 << 21) | (1 << 22);
/// `rights`: read the entries of an opened directory.
const RIGHTS_DIR: i64 = 1 << 14;

/// The `errno` values the classification looks at (the others become `FileSystemFailure`).
const EACCES: i32 = 2;
const EEXIST: i32 = 20;
const EISDIR: i32 = 31;
const ENOENT: i32 = 44;
const ENOTDIR: i32 = 54;
const ENOTEMPTY: i32 = 55;
const EPERM: i32 = 63;
const ENOTCAPABLE: i32 = 76;

/// The readdir buffer size (a multiple of 8 so every entry starts aligned).
const READDIR_BUF: i32 = 4096;
/// The initial capacity (in names) of the list_directory array.
const NAMES_CAP: i32 = 16;

/// The runtime functions of the fragment (all `Std.FileSystem`, emitted only on demand).
pub(super) const FUNCS: &[Sig] = &[
    ("fs_base_fd", &[], &[ValType::I32]),
    ("fs_str_copy", &[ValType::I32], &[ValType::I32]),
    (
        "fs_str_from_raw",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fs_is_utf8", &[ValType::I32], &[ValType::I32]),
    ("fs_classify", &[ValType::I32], &[ValType::I32]),
    ("fs_code_name", &[ValType::I32], &[ValType::I32]),
    (
        "fs_failure_message",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fs_outcome_error", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_message", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_text", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_name_count", &[ValType::I32], &[ValType::I32]),
    (
        "fs_outcome_name_at",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fs_release_outcome", &[ValType::I32], &[]),
    ("fs_read_text", &[ValType::I32], &[ValType::I32]),
    (
        "fs_write_text",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "fs_append_text",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("fs_exists", &[ValType::I32], &[ValType::I32]),
    ("fs_is_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_list_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_create_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_remove", &[ValType::I32], &[ValType::I32]),
];

/// Whether `name` is one of this fragment's runtime functions (registered only when a program
/// reaches a `Std.FileSystem` host).
pub(super) fn handles(name: &str) -> bool {
    name.starts_with("fs_")
}

/// Whether the module reaches any `Std.FileSystem` host, so the fragment (and its WASI imports)
/// are needed.
pub(super) fn requested(b: &Builder) -> bool {
    b.requested_hosts.iter().any(|h| h.starts_with("fs_"))
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fs_base_fd" => fs_base_fd(b, ins),
        "fs_str_copy" => fs_str_copy(b, ins),
        "fs_str_from_raw" => fs_str_from_raw(b, ins),
        "fs_is_utf8" => fs_is_utf8(ins),
        "fs_classify" => fs_classify(ins),
        "fs_code_name" => fs_code_name(b, ins),
        "fs_failure_message" => fs_failure_message(b, ins),
        "fs_outcome_error" => fs_outcome_error(b, ins),
        "fs_outcome_message" => fs_outcome_message(b, ins),
        "fs_outcome_text" => fs_outcome_text(b, ins),
        "fs_outcome_name_count" => fs_outcome_name_count(ins),
        "fs_outcome_name_at" => fs_outcome_name_at(b, ins),
        "fs_release_outcome" => {}
        "fs_read_text" => fs_read_text(b, ins),
        "fs_write_text" => fs_write(b, ins, O_CREAT | O_TRUNC, 0),
        "fs_append_text" => fs_write(b, ins, O_CREAT, FD_APPEND),
        "fs_exists" => fs_exists(b, ins, false),
        "fs_is_directory" => fs_exists(b, ins, true),
        "fs_list_directory" => fs_list_directory(b, ins),
        "fs_create_directory" => fs_create_directory(b, ins),
        "fs_remove" => fs_remove(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    let i32s = |n: u32| Some(vec![(n, ValType::I32)]);
    match name {
        "fs_base_fd" => i32s(1),
        "fs_str_copy" => i32s(2),
        "fs_str_from_raw" => i32s(2),
        "fs_is_utf8" => i32s(7),
        "fs_failure_message" => i32s(4),
        "fs_outcome_error" | "fs_outcome_message" | "fs_outcome_text" => i32s(1),
        "fs_outcome_name_at" => i32s(3),
        "fs_read_text" => i32s(11),
        "fs_write_text" | "fs_append_text" => i32s(6),
        "fs_exists" | "fs_is_directory" => i32s(3),
        "fs_remove" => i32s(6),
        "fs_create_directory" => i32s(9),
        "fs_list_directory" => Some(vec![(18, ValType::I32), (2, ValType::I64)]),
        _ => None,
    }
}

// ---- small emit helpers ----

/// A static string literal, as an `i32` pointer (the value's `rc` is 0, so `drop` ignores it).
fn lit(b: &mut Builder, s: &str) -> i32 {
    b.string_literal(s.as_bytes()) as i32
}

fn call(ins: &mut InstructionSink, index: u32) {
    ins.call(index);
}

/// Allocates a zeroed outcome into local `out` (the fields are all set explicitly: the allocator
/// may hand back a recycled block).
fn outcome_zero(b: &mut Builder, ins: &mut InstructionSink, out: u32) {
    let desc = handle::outcome_desc(b);
    ins.i32_const(OUTCOME_SLOTS as i32);
    ins.i32_const(desc as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(out);
    for off in [
        OUTCOME_ERROR,
        OUTCOME_MESSAGE,
        OUTCOME_TEXT,
        OUTCOME_COUNT,
        OUTCOME_NAMES,
    ] {
        ins.local_get(out);
        ins.i32_const(0);
        ins.i32_store(mem(off, 4));
    }
}

/// Pushes an outcome whose error is `code` (and whose message is empty).
fn outcome_code(b: &mut Builder, ins: &mut InstructionSink, out: u32, code: i32) {
    outcome_zero(b, ins, out);
    ins.local_get(out);
    ins.i32_const(code);
    ins.i32_store(mem(OUTCOME_ERROR, 4));
    ins.local_get(out);
}

/// Pushes an outcome for a failed WASI call: the error code is `fs_classify(errno)`, and a
/// `FileSystemFailure` also carries a message built from the call and the path.
#[allow(clippy::too_many_arguments)]
fn outcome_err(
    b: &mut Builder,
    ins: &mut InstructionSink,
    out: u32,
    code: u32,
    msg: u32,
    errno: u32,
    syscall: i32,
    path: u32,
) {
    ins.local_get(errno);
    call(ins, rt(b, "fs_classify"));
    ins.local_set(code);
    outcome_zero(b, ins, out);
    ins.local_get(out);
    ins.local_get(code);
    ins.i32_store(mem(OUTCOME_ERROR, 4));
    ins.local_get(code);
    ins.i32_const(OUTCOME_FAILURE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(errno);
    ins.i32_const(syscall);
    ins.local_get(path);
    call(ins, rt(b, "fs_failure_message"));
    ins.local_set(msg);
    ins.local_get(out);
    ins.local_get(msg);
    ins.i32_store(mem(OUTCOME_MESSAGE, 4));
    ins.end();
    ins.local_get(out);
}

/// Pushes an outcome with the error `code` and returns from the function.
fn ret_code(b: &mut Builder, ins: &mut InstructionSink, out: u32, code: i32) {
    outcome_code(b, ins, out, code);
    ins.return_();
}

/// Pushes the outcome of `outcome_err` and returns from the function.
#[allow(clippy::too_many_arguments)]
fn ret_err(
    b: &mut Builder,
    ins: &mut InstructionSink,
    out: u32,
    code: u32,
    msg: u32,
    errno: u32,
    syscall: i32,
    path: u32,
) {
    outcome_err(b, ins, out, code, msg, errno, syscall, path);
    ins.return_();
}

/// `path_filestat_get(base, flags, path, path_len, st)`, leaving the errno on the stack.
fn filestat(b: &mut Builder, ins: &mut InstructionSink, base: u32, flags: i32, st: u32, path: u32) {
    ins.local_get(base);
    ins.i32_const(flags);
    ins.local_get(path);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(path);
    ins.i32_load(mem(HDR, 4));
    ins.local_get(st);
    call(ins, b.wasi_path_filestat_get());
}

/// `path_open(base, LOOKUP_FOLLOW, path, path_len, oflags, rights, 0, fdflags, &IOVEC)`, leaving
/// the errno on the stack (the opened fd lands in the `IOVEC` scratch slot).
fn path_open(
    b: &mut Builder,
    ins: &mut InstructionSink,
    base: u32,
    oflags: i32,
    fdflags: i32,
    rights: i64,
    path: u32,
) {
    ins.local_get(base);
    ins.i32_const(LOOKUP_FOLLOW);
    ins.local_get(path);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(path);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(oflags);
    ins.i64_const(rights);
    ins.i64_const(0);
    ins.i32_const(fdflags);
    ins.i32_const(IOVEC as i32);
    call(ins, b.wasi_path_open());
}

/// Pushes the byte at `str + HDR + 4 + idx + extra` (the string in local `str`, the index in local
/// `idx`).
fn byte_of(ins: &mut InstructionSink, str_local: u32, idx_local: u32, extra: u32) {
    ins.local_get(str_local);
    ins.local_get(idx_local);
    ins.i32_const((HDR + 4 + extra) as i32);
    ins.i32_add();
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

// ---- the preopen base ----

/// The file descriptor all relative paths resolve against: the first preopened directory (fd 3 in
/// the usual WASI layout), or 3 when there is none (the calls then fail and are classified).
fn fs_base_fd(b: &mut Builder, ins: &mut InstructionSink) {
    const I: u32 = 0;
    ins.i32_const(3);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(32);
    ins.i32_gt_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(IOVEC as i32);
    call(ins, b.wasi_fd_prestat_get());
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.return_();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(3);
}

// ---- strings and errors ----

/// `fs_str_copy(s)`: a fresh Emela string with the same bytes (the host's `strdup`).
fn fs_str_copy(b: &mut Builder, ins: &mut InstructionSink) {
    const LEN: u32 = 1;
    const DST: u32 = 2;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    call(ins, rt(b, "str_new"));
    ins.local_set(DST);
    ins.local_get(DST);
    ins.i32_const(0);
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(LEN);
    call(ins, rt(b, "copy_range"));
    ins.local_get(DST);
}

/// `fs_str_from_raw(ptr, len)`: a fresh Emela string of `len` raw bytes.
fn fs_str_from_raw(b: &mut Builder, ins: &mut InstructionSink) {
    const DST: u32 = 2;
    const I: u32 = 3;
    ins.local_get(1);
    call(ins, rt(b, "str_new"));
    ins.local_set(DST);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(1);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(DST);
    ins.local_get(I);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_add();
    ins.local_get(0);
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
    ins.local_get(DST);
}

/// `fs_is_utf8(s)`: whether the bytes are UTF-8 as a fatal WHATWG decoder accepts it (no overlong
/// forms, no surrogates, nothing above U+10FFFF) — the C host's `emela_fs_utf8`.
fn fs_is_utf8(ins: &mut InstructionSink) {
    const N: u32 = 1;
    const I: u32 = 2;
    const C: u32 = 3;
    const LEN: u32 = 4;
    const LO: u32 = 5;
    const HI: u32 = 6;
    const K: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    byte_of(ins, 0, I, 0);
    ins.local_set(C);
    // ASCII.
    ins.local_get(C);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(1);
    ins.end();
    ins.i32_const(0x80);
    ins.local_set(LO);
    ins.i32_const(0xBF);
    ins.local_set(HI);
    // 2 bytes: C2..DF.
    ins.local_get(C);
    ins.i32_const(0xC2);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xDF);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(LEN);
    ins.else_();
    // 3 bytes: E0..EF.
    ins.local_get(C);
    ins.i32_const(0xE0);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xEF);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(3);
    ins.local_set(LEN);
    ins.local_get(C);
    ins.i32_const(0xE0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0xA0);
    ins.local_set(LO);
    ins.end();
    ins.local_get(C);
    ins.i32_const(0xED);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0x9F);
    ins.local_set(HI);
    ins.end();
    ins.else_();
    // 4 bytes: F0..F4.
    ins.local_get(C);
    ins.i32_const(0xF0);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xF4);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(4);
    ins.local_set(LEN);
    ins.local_get(C);
    ins.i32_const(0xF0);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0x90);
    ins.local_set(LO);
    ins.end();
    ins.local_get(C);
    ins.i32_const(0xF4);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0x8F);
    ins.local_set(HI);
    ins.end();
    ins.else_();
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.end();
    ins.end();
    // The first continuation byte.
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_add();
    ins.local_get(N);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    byte_of(ins, 0, I, 1);
    ins.local_get(LO);
    ins.i32_lt_u();
    byte_of(ins, 0, I, 1);
    ins.local_get(HI);
    ins.i32_gt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The rest: 80..BF.
    ins.i32_const(2);
    ins.local_set(K);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(K);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(K);
    ins.i32_add();
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(C);
    ins.local_get(C);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.local_get(C);
    ins.i32_const(0xBF);
    ins.i32_gt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(K);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(K);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

/// `fs_classify(errno)`: the error code of an outcome. The codes the operation's `fails` does not
/// list become `FileSystemFailure` (`Std.FileSystem`'s `check_*` maps the rest).
fn fs_classify(ins: &mut InstructionSink) {
    let is = |ins: &mut InstructionSink, e: i32| {
        ins.local_get(0);
        ins.i32_const(e);
        ins.i32_eq();
    };
    // FileNotFound: ENOENT / ENOTDIR.
    is(ins, ENOENT);
    is(ins, ENOTDIR);
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(OUTCOME_NOT_FOUND);
    ins.return_();
    ins.end();
    // FileAccessDenied: EACCES / EPERM / ENOTCAPABLE.
    is(ins, EACCES);
    is(ins, EPERM);
    ins.i32_or();
    is(ins, ENOTCAPABLE);
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(OUTCOME_ACCESS_DENIED);
    ins.return_();
    ins.end();
    // IsADirectory.
    is(ins, EISDIR);
    ins.if_(BlockType::Empty);
    ins.i32_const(OUTCOME_IS_A_DIRECTORY);
    ins.return_();
    ins.end();
    // DirectoryNotEmpty.
    is(ins, ENOTEMPTY);
    ins.if_(BlockType::Empty);
    ins.i32_const(OUTCOME_NOT_EMPTY);
    ins.return_();
    ins.end();
    ins.i32_const(OUTCOME_FAILURE);
}

/// `fs_code_name(errno)`: the C host's short error code (`"EIO"`, ...) for the message.
fn fs_code_name(b: &mut Builder, ins: &mut InstructionSink) {
    for (errno, name) in [
        (ENOENT, "ENOENT"),
        (ENOTDIR, "ENOTDIR"),
        (EACCES, "EACCES"),
        (EPERM, "EPERM"),
        (EISDIR, "EISDIR"),
        (EEXIST, "EEXIST"),
        (ENOTEMPTY, "ENOTEMPTY"),
        (16, "EBUSY"),
        (28, "EINVAL"),
        (29, "EIO"),
        (33, "EMFILE"),
        (37, "ENAMETOOLONG"),
        (51, "ENOSPC"),
        (62, "ELOOP"),
        (69, "EROFS"),
    ] {
        ins.local_get(0);
        ins.i32_const(errno);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(lit(b, name));
        ins.return_();
        ins.end();
    }
    ins.i32_const(lit(b, "UNKNOWN"));
}

/// `fs_failure_message(errno, syscall, path)`: `"<CODE>: <syscall> '<path>'"`, the WASM stand-in
/// for the C host's `strerror` message (which the WASI surface does not expose).
fn fs_failure_message(b: &mut Builder, ins: &mut InstructionSink) {
    const M1: u32 = 3;
    const M2: u32 = 4;
    const M3: u32 = 5;
    const M4: u32 = 6;
    ins.local_get(0);
    call(ins, rt(b, "fs_code_name"));
    ins.i32_const(lit(b, ": "));
    call(ins, rt(b, "str_append"));
    ins.local_set(M1);
    ins.local_get(M1);
    ins.local_get(1);
    call(ins, rt(b, "str_append"));
    ins.local_set(M2);
    ins.local_get(M2);
    ins.i32_const(lit(b, " '"));
    call(ins, rt(b, "str_append"));
    ins.local_set(M3);
    ins.local_get(M3);
    ins.local_get(2);
    call(ins, rt(b, "str_append"));
    ins.local_set(M4);
    ins.local_get(M4);
    ins.i32_const(lit(b, "'"));
    call(ins, rt(b, "str_append"));
}

// ---- the outcome accessors ----

/// `outcome_error(o)`: the error name, or `""` on success.
fn fs_outcome_error(b: &mut Builder, ins: &mut InstructionSink) {
    const CODE: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_ERROR, 4));
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(lit(b, ""));
    ins.return_();
    ins.end();
    for code in 1..=6 {
        ins.local_get(CODE);
        ins.i32_const(code);
        ins.i32_eq();
        ins.if_(BlockType::Empty);
        ins.i32_const(lit(b, handle::error_name(code)));
        ins.return_();
        ins.end();
    }
    ins.i32_const(lit(b, "FileSystemFailure"));
}

/// `outcome_message(o)`: a fresh copy of the host's message for `FileSystemFailure`.
fn fs_outcome_message(b: &mut Builder, ins: &mut InstructionSink) {
    const M: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_MESSAGE, 4));
    ins.local_set(M);
    ins.local_get(M);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(lit(b, ""));
    ins.return_();
    ins.end();
    ins.local_get(M);
    call(ins, rt(b, "fs_str_copy"));
}

/// `outcome_text(o)`: a fresh copy of the text read by `read_text`.
fn fs_outcome_text(b: &mut Builder, ins: &mut InstructionSink) {
    const T: u32 = 1;
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_TEXT, 4));
    ins.local_set(T);
    ins.local_get(T);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(lit(b, ""));
    ins.return_();
    ins.end();
    ins.local_get(T);
    call(ins, rt(b, "fs_str_copy"));
}

/// `outcome_name_count(o)`.
fn fs_outcome_name_count(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_COUNT, 4));
}

/// `outcome_name_at(o, i)`: the `i`-th name, or `""` when out of range.
fn fs_outcome_name_at(b: &mut Builder, ins: &mut InstructionSink) {
    const COUNT: u32 = 2;
    const ARR: u32 = 3;
    const P: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_COUNT, 4));
    ins.local_set(COUNT);
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.local_get(1);
    ins.local_get(COUNT);
    ins.i32_ge_s();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(lit(b, ""));
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_load(mem(OUTCOME_NAMES, 4));
    ins.local_set(ARR);
    ins.local_get(ARR);
    ins.local_get(1);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(lit(b, ""));
    ins.return_();
    ins.end();
    ins.local_get(P);
    call(ins, rt(b, "fs_str_copy"));
}

// ---- read_text ----

fn fs_read_text(b: &mut Builder, ins: &mut InstructionSink) {
    const BASE: u32 = 1;
    const ST: u32 = 2;
    const ERR: u32 = 3;
    const SIZE: u32 = 4;
    const FD: u32 = 5;
    const TEXT: u32 = 6;
    const TOTAL: u32 = 7;
    const GOT: u32 = 8;
    const OUT: u32 = 9;
    const CODE: u32 = 10;
    const MSG: u32 = 11;
    let bd = handle::buffer_desc(b);
    let open = lit(b, "open");
    let read = lit(b, "read");
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    // The stat: the size up front, and IsADirectory before opening.
    ins.i32_const(8);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(ST);
    filestat(b, ins, BASE, LOOKUP_FOLLOW, ST, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ret_err(b, ins, OUT, CODE, MSG, ERR, open, 0);
    ins.end();
    ins.local_get(ST);
    ins.i32_load8_u(mem(16, 1));
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_IS_A_DIRECTORY);
    ins.end();
    ins.local_get(ST);
    ins.i32_load(mem(32, 4));
    ins.local_set(SIZE);
    // Open read-only.
    path_open(b, ins, BASE, 0, 0, RIGHTS_READ, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ret_err(b, ins, OUT, CODE, MSG, ERR, open, 0);
    ins.end();
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FD);
    // A string of the file size, then read into it.
    ins.local_get(SIZE);
    call(ins, rt(b, "str_new"));
    ins.local_set(TEXT);
    ins.i32_const(0);
    ins.local_set(TOTAL);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(TOTAL);
    ins.local_get(SIZE);
    ins.i32_ge_u();
    ins.br_if(1);
    // The iovec: the remaining bytes of the string.
    ins.i32_const(IOVEC as i32);
    ins.local_get(TEXT);
    ins.local_get(TOTAL);
    ins.i32_add();
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.i32_const((IOVEC + 4) as i32);
    ins.local_get(SIZE);
    ins.local_get(TOTAL);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.local_get(FD);
    ins.i32_const(IOVEC as i32);
    ins.i32_const(1);
    ins.i32_const(NWRITTEN as i32);
    call(ins, b.wasi_fd_read());
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    ret_err(b, ins, OUT, CODE, MSG, ERR, read, 0);
    ins.end();
    ins.i32_const(NWRITTEN as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(GOT);
    ins.local_get(GOT);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(TOTAL);
    ins.local_get(GOT);
    ins.i32_add();
    ins.local_set(TOTAL);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    // The file may have shrunk: the text is exactly `total` bytes.
    ins.local_get(TEXT);
    ins.local_get(TOTAL);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(TEXT);
    call(ins, rt(b, "fs_is_utf8"));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_INVALID_TEXT);
    ins.end();
    outcome_zero(b, ins, OUT);
    ins.local_get(OUT);
    ins.local_get(TEXT);
    ins.i32_store(mem(OUTCOME_TEXT, 4));
    ins.local_get(OUT);
}

// ---- write_text / append_text ----

/// The shared body of `write_text` (`oflags`/`fdflags` zero) and `append_text` (create, append).
fn fs_write(b: &mut Builder, ins: &mut InstructionSink, oflags: i32, fdflags: i32) {
    const BASE: u32 = 2;
    const ERR: u32 = 3;
    const FD: u32 = 4;
    const OUT: u32 = 5;
    const CODE: u32 = 6;
    const MSG: u32 = 7;
    let open = lit(b, "open");
    let write = lit(b, "write");
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    path_open(b, ins, BASE, oflags, fdflags, RIGHTS_WRITE, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ret_err(b, ins, OUT, CODE, MSG, ERR, open, 0);
    ins.end();
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FD);
    // The iovec: the text's bytes.
    ins.i32_const(IOVEC as i32);
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.i32_const((IOVEC + 4) as i32);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.i32_store(mem(0, 4));
    ins.local_get(FD);
    ins.i32_const(IOVEC as i32);
    ins.i32_const(1);
    ins.i32_const(NWRITTEN as i32);
    call(ins, b.fd_write());
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    ret_err(b, ins, OUT, CODE, MSG, ERR, write, 0);
    ins.end();
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    outcome_code(b, ins, OUT, OUTCOME_OK);
}

// ---- exists / is_directory ----

/// `fs_exists` (`directory` false) and `fs_is_directory` (`directory` true): a single stat, which
/// follows symbolic links like the C host's `stat`.
fn fs_exists(b: &mut Builder, ins: &mut InstructionSink, directory: bool) {
    const BASE: u32 = 1;
    const ST: u32 = 2;
    const ERR: u32 = 3;
    let bd = handle::buffer_desc(b);
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    ins.i32_const(8);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(ST);
    filestat(b, ins, BASE, LOOKUP_FOLLOW, ST, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.i32_eqz();
    if directory {
        ins.local_get(ST);
        ins.i32_load8_u(mem(16, 1));
        ins.i32_const(3);
        ins.i32_eq();
        ins.i32_and();
    }
}

// ---- list_directory ----

fn fs_list_directory(b: &mut Builder, ins: &mut InstructionSink) {
    const BASE: u32 = 1;
    const FD: u32 = 2;
    const ERR: u32 = 3;
    const BUF: u32 = 4;
    const USED: u32 = 5;
    const OFF: u32 = 6;
    const NAMELEN: u32 = 7;
    const NPTR: u32 = 8;
    const COUNT: u32 = 9;
    const CAP: u32 = 10;
    const ARR: u32 = 11;
    const OUT: u32 = 12;
    const I: u32 = 13;
    const J: u32 = 14;
    const A: u32 = 15;
    const BB: u32 = 16;
    const TMP: u32 = 17;
    const CODE: u32 = 18;
    const COOKIE: u32 = 19;
    const NEXT: u32 = 20;
    let bd = handle::buffer_desc(b);
    let opendir = lit(b, "opendir");
    let readdir = lit(b, "readdir");
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    path_open(b, ins, BASE, O_DIRECTORY, 0, RIGHTS_DIR, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ins.local_get(ERR);
    ins.i32_const(ENOTDIR);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_NOT_A_DIRECTORY);
    ins.end();
    ret_err(b, ins, OUT, CODE, A, ERR, opendir, 0);
    ins.end();
    ins.i32_const(IOVEC as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FD);
    // The entries are copied into a fresh array that grows as needed.
    ins.i32_const(READDIR_BUF / 8);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(BUF);
    ins.i32_const(0);
    ins.local_set(COUNT);
    ins.i32_const(NAMES_CAP);
    ins.local_set(CAP);
    ins.local_get(CAP);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(ARR);
    ins.i64_const(0);
    ins.local_set(COOKIE);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(FD);
    ins.local_get(BUF);
    ins.i32_const(READDIR_BUF);
    ins.local_get(COOKIE);
    ins.i32_const(NWRITTEN as i32);
    call(ins, b.wasi_fd_readdir());
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    ret_err(b, ins, OUT, CODE, A, ERR, readdir, 0);
    ins.end();
    ins.i32_const(NWRITTEN as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(USED);
    ins.i32_const(0);
    ins.local_set(OFF);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // A whole dirent: 24 bytes of header, then the name.
    ins.local_get(OFF);
    ins.i32_const(24);
    ins.i32_add();
    ins.local_get(USED);
    ins.i32_gt_u();
    ins.br_if(1);
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i32_load(mem(16, 4));
    ins.local_set(NAMELEN);
    ins.local_get(OFF);
    ins.i32_const(24);
    ins.i32_add();
    ins.local_get(NAMELEN);
    ins.i32_add();
    ins.local_get(USED);
    ins.i32_gt_u();
    ins.br_if(1);
    // Skip "." and "..".
    ins.local_get(NAMELEN);
    ins.i32_const(1);
    ins.i32_eq();
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i32_load8_u(mem(24, 1));
    ins.i32_const(b'.' as i32);
    ins.i32_eq();
    ins.i32_and();
    ins.local_get(NAMELEN);
    ins.i32_const(2);
    ins.i32_eq();
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i32_load8_u(mem(24, 1));
    ins.i32_const(b'.' as i32);
    ins.i32_eq();
    ins.i32_and();
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i32_load8_u(mem(25, 1));
    ins.i32_const(b'.' as i32);
    ins.i32_eq();
    ins.i32_and();
    ins.i32_or();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i32_const(24);
    ins.i32_add();
    ins.local_get(NAMELEN);
    call(ins, rt(b, "fs_str_from_raw"));
    ins.local_set(NPTR);
    // Grow the array when it is full.
    ins.local_get(COUNT);
    ins.local_get(CAP);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(CAP);
    ins.i32_const(2);
    ins.i32_mul();
    ins.local_set(TMP);
    ins.local_get(TMP);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(COUNT);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_get(J);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(ARR);
    ins.local_get(J);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR, 4));
    ins.i32_store(mem(HDR, 4));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.local_set(ARR);
    ins.local_get(TMP);
    ins.local_set(CAP);
    ins.end();
    // arr[count] = name.
    ins.local_get(ARR);
    ins.local_get(COUNT);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(NPTR);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(COUNT);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(COUNT);
    ins.end();
    // The next cookie and the next entry.
    ins.local_get(BUF);
    ins.local_get(OFF);
    ins.i32_add();
    ins.i64_load(mem(0, 8));
    ins.local_set(NEXT);
    ins.local_get(NEXT);
    ins.local_set(COOKIE);
    ins.local_get(OFF);
    ins.i32_const(24);
    ins.i32_add();
    ins.local_get(NAMELEN);
    ins.i32_add();
    ins.local_set(OFF);
    ins.br(0);
    ins.end();
    ins.end();
    // A short read means the directory is exhausted.
    ins.local_get(USED);
    ins.i32_const(READDIR_BUF);
    ins.i32_lt_u();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(FD);
    call(ins, b.wasi_fd_close());
    ins.drop();
    // Insertion sort by name (UTF-8 byte order is code point order).
    ins.i32_const(1);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(COUNT);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(ARR);
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR, 4));
    ins.local_set(A);
    ins.local_get(ARR);
    ins.local_get(J);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR, 4));
    ins.local_set(BB);
    ins.local_get(A);
    ins.local_get(BB);
    call(ins, rt(b, "str_cmp"));
    ins.i32_const(0);
    ins.i32_le_s();
    ins.br_if(1);
    ins.local_get(ARR);
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(BB);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(ARR);
    ins.local_get(J);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(A);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
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
    outcome_zero(b, ins, OUT);
    ins.local_get(OUT);
    ins.local_get(COUNT);
    ins.i32_store(mem(OUTCOME_COUNT, 4));
    ins.local_get(OUT);
    ins.local_get(ARR);
    ins.i32_store(mem(OUTCOME_NAMES, 4));
    ins.local_get(OUT);
}

// ---- create_directory ----

fn fs_create_directory(b: &mut Builder, ins: &mut InstructionSink) {
    const BASE: u32 = 1;
    const N: u32 = 2;
    const I: u32 = 3;
    const ST: u32 = 4;
    const ERR: u32 = 5;
    const PREFIX: u32 = 6;
    const OUT: u32 = 7;
    const CODE: u32 = 8;
    const MSG: u32 = 9;
    let bd = handle::buffer_desc(b);
    let mkdir = lit(b, "mkdir");
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    ins.i32_const(8);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(ST);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(1);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // Create the prefix `path[0..i]` when `i` ends a component (a `/`, or the end of the path).
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_eq();
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    byte_of(ins, 0, I, 0);
    ins.i32_const(b'/' as i32);
    ins.i32_eq();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const(0);
    ins.local_get(I);
    call(ins, rt(b, "str_slice"));
    ins.local_set(PREFIX);
    ins.local_get(BASE);
    ins.local_get(PREFIX);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    call(ins, b.wasi_path_create_directory());
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.else_();
    // A file in the middle: NotADirectory.
    ins.local_get(ERR);
    ins.i32_const(ENOTDIR);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_NOT_A_DIRECTORY);
    ins.end();
    // Already there: fine when it is a directory.
    ins.local_get(ERR);
    ins.i32_const(EEXIST);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    filestat(b, ins, BASE, LOOKUP_FOLLOW, ST, PREFIX);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(ST);
    ins.i32_load8_u(mem(16, 1));
    ins.i32_const(3);
    ins.i32_eq();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_NOT_A_DIRECTORY);
    ins.end();
    ins.else_();
    ret_code(b, ins, OUT, OUTCOME_NOT_A_DIRECTORY);
    ins.end();
    ins.else_();
    ret_err(b, ins, OUT, CODE, MSG, ERR, mkdir, 0);
    ins.end();
    ins.end();
    ins.end();
    // Stop after the whole path.
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    outcome_code(b, ins, OUT, OUTCOME_OK);
}

// ---- remove ----

fn fs_remove(b: &mut Builder, ins: &mut InstructionSink) {
    const BASE: u32 = 1;
    const ST: u32 = 2;
    const ERR: u32 = 3;
    const OUT: u32 = 4;
    const CODE: u32 = 5;
    const MSG: u32 = 6;
    let bd = handle::buffer_desc(b);
    let stat = lit(b, "stat");
    let remove = lit(b, "remove");
    ins.call(rt(b, "fs_base_fd"));
    ins.local_set(BASE);
    ins.i32_const(8);
    ins.i32_const(bd as i32);
    call(ins, rt(b, "alloc"));
    ins.local_set(ST);
    // `lstat`: a symbolic link is removed itself.
    filestat(b, ins, BASE, 0, ST, 0);
    ins.local_set(ERR);
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    ret_err(b, ins, OUT, CODE, MSG, ERR, stat, 0);
    ins.end();
    ins.local_get(ST);
    ins.i32_load8_u(mem(16, 1));
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(BASE);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    call(ins, b.wasi_path_remove_directory());
    ins.local_set(ERR);
    ins.else_();
    ins.local_get(BASE);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    call(ins, b.wasi_path_unlink_file());
    ins.local_set(ERR);
    ins.end();
    ins.local_get(ERR);
    ins.if_(BlockType::Empty);
    // A non-empty directory (ENOTEMPTY or, on some hosts, EEXIST).
    ins.local_get(ERR);
    ins.i32_const(ENOTEMPTY);
    ins.i32_eq();
    ins.local_get(ERR);
    ins.i32_const(EEXIST);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ret_code(b, ins, OUT, OUTCOME_NOT_EMPTY);
    ins.end();
    ret_err(b, ins, OUT, CODE, MSG, ERR, remove, 0);
    ins.end();
    outcome_code(b, ins, OUT, OUTCOME_OK);
}

// ---- the WASI imports ----

impl<'m> Builder<'m> {
    /// `path_open(fd, dirflags, path, path_len, oflags, rights_base, rights_inheriting, fdflags,
    /// opened_fd)`.
    fn wasi_path_open(&mut self) -> u32 {
        self.wasi_import(
            "path_open",
            vec![
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I64,
                ValType::I64,
                ValType::I32,
                ValType::I32,
            ],
            vec![ValType::I32],
        )
    }

    /// `fd_read(fd, iovs, iovs_len, nread)`.
    fn wasi_fd_read(&mut self) -> u32 {
        self.wasi_import(
            "fd_read",
            vec![ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `fd_readdir(fd, buf, buf_len, cookie, bufused)`.
    fn wasi_fd_readdir(&mut self) -> u32 {
        self.wasi_import(
            "fd_readdir",
            vec![
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I64,
                ValType::I32,
            ],
            vec![ValType::I32],
        )
    }

    /// `fd_close(fd)`.
    fn wasi_fd_close(&mut self) -> u32 {
        self.wasi_import("fd_close", vec![ValType::I32], vec![ValType::I32])
    }

    /// `path_filestat_get(fd, flags, path, path_len, buf)`.
    fn wasi_path_filestat_get(&mut self) -> u32 {
        self.wasi_import(
            "path_filestat_get",
            vec![
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            vec![ValType::I32],
        )
    }

    /// `path_create_directory(fd, path, path_len)`.
    fn wasi_path_create_directory(&mut self) -> u32 {
        self.wasi_import(
            "path_create_directory",
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `path_unlink_file(fd, path, path_len)`.
    fn wasi_path_unlink_file(&mut self) -> u32 {
        self.wasi_import(
            "path_unlink_file",
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `path_remove_directory(fd, path, path_len)`.
    fn wasi_path_remove_directory(&mut self) -> u32 {
        self.wasi_import(
            "path_remove_directory",
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// `fd_prestat_get(fd, buf)`.
    fn wasi_fd_prestat_get(&mut self) -> u32 {
        self.wasi_import(
            "fd_prestat_get",
            vec![ValType::I32, ValType::I32],
            vec![ValType::I32],
        )
    }

    /// Registers the WASI imports the `Std.FileSystem` hosts need. Called from `Builder::build`
    /// before the function index space is fixed, and only when a host was requested, so programs
    /// without `Std.FileSystem` keep their exact bytes.
    pub(crate) fn fs_imports(&mut self) {
        if !requested(self) {
            return;
        }
        self.wasi_path_open();
        self.wasi_fd_read();
        self.wasi_fd_readdir();
        self.wasi_fd_close();
        self.wasi_path_filestat_get();
        self.wasi_path_create_directory();
        self.wasi_path_unlink_file();
        self.wasi_path_remove_directory();
        self.wasi_fd_prestat_get();
    }
}
