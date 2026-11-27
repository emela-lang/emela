//! `Std.FileSystem` on WASI 0.3 (`wasi:filesystem`).
//!
//! The p3 interface is descriptor based: the guest gets the preopened work directory from
//! `wasi:filesystem/preopens.get-directories`, opens paths relative to it with
//! `descriptor.open-at`, and moves bytes through `stream`s (`read-via-stream`,
//! `write-via-stream`, `append-via-stream`, `read-directory`). The `open-at`, `stat-at` and the
//! other path operations are `async func`s, so the runtime calls their `[async-lower]` form,
//! joins the subtask to a waitable set and waits with `waitable-set.wait` until the host reports
//! it completed. The stream and future operations are waited on the same way. The waits block the
//! current task; nothing else runs during a file operation, but the host event loop keeps
//! driving it. A parked guest scheduler would have to carry the result of a multi-call operation
//! across the suspension, which the Emela host functions cannot express: each is one call.
//!
//! The host functions return the same outcome object as the JS and C host files: the error name,
//! a message, the text and the listed names. `file_system.emel`'s handler turns the name into the
//! matching Emela error, so the classification (not found, permission, a directory, ...) is what
//! this module decides. WASI `error-code`s are mapped per operation, like the JS `classify` and
//! the C `emela_fs_classify` tables.
//!
//! The outcome block and the strings it names are not reference counted and are not freed: a
//! command is short lived and the WASM target has no leak check, so this matches the JS handler
//! (whose `release` is a no-op too). Only paths relative to the preopened directory are
//! supported; an absolute path fails with the WASI `not-permitted` code, which is classified as
//! `FileAccessDenied`.

mod dir;
mod ops;
mod stream;

use super::*;
use dir::{mkdir, names_push, names_sort};
use ops::{
    create_directory, exists, list_directory, outcome, outcome_name_at, outcome_string, read_text,
    remove, write_text,
};
use stream::{read_all, write_all};

pub(in crate::emit::runtime) const FUNCS: &[Sig] = &[
    ("fs_alloc_raw", &[ValType::I32], &[ValType::I32]),
    ("fs_utf8", &[ValType::I32; 2], &[ValType::I32]),
    ("fs_root", &[], &[ValType::I32]),
    ("fs_await", &[ValType::I32], &[ValType::I32]),
    ("fs_wait_code", &[ValType::I32], &[ValType::I32]),
    ("fs_reset", &[], &[]),
    ("fs_classify", &[ValType::I32; 2], &[ValType::I32]),
    ("fs_open", &[ValType::I32; 5], &[ValType::I32]),
    ("fs_stat", &[ValType::I32; 3], &[ValType::I32]),
    ("fs_read_all", &[ValType::I32], &[ValType::I32]),
    ("fs_write_all", &[ValType::I32; 3], &[]),
    ("fs_mkdir", &[ValType::I32; 3], &[ValType::I32]),
    ("fs_names_push", &[ValType::I32], &[]),
    ("fs_names_sort", &[], &[]),
    ("fs_outcome", &[], &[ValType::I32]),
    ("fs_read_text", &[ValType::I32], &[ValType::I32]),
    ("fs_write_text", &[ValType::I32; 2], &[ValType::I32]),
    ("fs_append_text", &[ValType::I32; 2], &[ValType::I32]),
    ("fs_exists", &[ValType::I32], &[ValType::I32]),
    ("fs_is_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_list_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_create_directory", &[ValType::I32], &[ValType::I32]),
    ("fs_remove", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_error", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_message", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_text", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_name_count", &[ValType::I32], &[ValType::I32]),
    ("fs_outcome_name_at", &[ValType::I32; 2], &[ValType::I32]),
    ("fs_release_outcome", &[ValType::I32], &[]),
];

pub(in crate::emit::runtime) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "fs_alloc_raw" => i32s(1),
        // i, continuation bytes, first byte, low bound, high bound, j
        "fs_utf8" => i32s(6),
        // list pointer, list length, descriptor
        "fs_root" => i32s(3),
        // status, handle, set, event status, event code
        "fs_await" => i32s(5),
        // set, code, event code
        "fs_wait_code" => i32s(3),
        // discriminant
        "fs_classify" => i32s(1),
        // status
        "fs_open" | "fs_stat" | "fs_mkdir" => i32s(1),
        // buffer, capacity, length, chunk, code, amount, status, stream, future, string
        "fs_read_all" => i32s(10),
        // packed stream ends, reader, writer, future, pointer, remaining, chunk, code, amount,
        // status
        "fs_write_all" => vec![(1, ValType::I64), (9, ValType::I32)],
        // new array, new capacity
        "fs_names_push" => i32s(2),
        // start, end, root, child, a, b
        "fs_names_sort" => i32s(6),
        // outcome, error name
        "fs_outcome" => i32s(2),
        // root, descriptor, text
        "fs_read_text" => i32s(3),
        // root, descriptor
        "fs_write_text" | "fs_append_text" => i32s(2),
        "fs_exists" | "fs_is_directory" => i32s(1),
        // root, descriptor, stream, future, code, amount, status, i, pointer, length
        "fs_list_directory" => i32s(10),
        // root, pointer, length, i
        "fs_create_directory" => i32s(4),
        // root, packed, status
        "fs_remove" => i32s(3),
        "fs_outcome_error" | "fs_outcome_message" | "fs_outcome_text" => i32s(1),
        // names, name
        "fs_outcome_name_at" => i32s(2),
        _ => Vec::new(),
    }
}

/// The scratch cells and static data of the file system runtime.
#[derive(Clone, Copy)]
pub(crate) struct Plan {
    /// The preopened work directory's descriptor (0 until fetched).
    pub(crate) root: u32,
    /// The error index of the last operation (0 is success).
    pub(crate) err: u32,
    /// The message string of the last outcome.
    pub(crate) msg: u32,
    /// The text string of the last outcome.
    pub(crate) text: u32,
    /// The names array of the last listing, and its length and capacity.
    pub(crate) names: u32,
    pub(crate) count: u32,
    pub(crate) cap: u32,
    /// The `descriptor-type` discriminant of the last `stat-at`.
    pub(crate) ty: u32,
    /// A result area for the async-lowered calls (up to a `descriptor-stat`).
    pub(crate) buf: u32,
    /// The indirect parameters of `open-at`.
    pub(crate) params: u32,
    /// One chunk of `read-directory` entries (16 records of 24 bytes).
    pub(crate) dirbuf: u32,
    /// A descriptor for raw (non-reference-counted) byte and pointer blocks.
    pub(crate) raw_desc: u32,
    /// The descriptor of an outcome block (five reference-free slots).
    pub(crate) outcome_desc: u32,
    /// The static empty string.
    pub(crate) empty: u32,
    /// The eight error names, by error index.
    pub(crate) err_table: u32,
}

/// The slot of the error name in an outcome block.
const OUT_ERR: u32 = HDR;
const OUT_MSG: u32 = HDR + 8;
const OUT_TEXT: u32 = HDR + 16;
const OUT_NAMES: u32 = HDR + 24;
const OUT_COUNT: u32 = HDR + 32;

// The error indices. They index `Plan::err_table`, whose order `fs_outcome` relies on.
const E_NOT_FOUND: i32 = 1;
const E_DENIED: i32 = 2;
const E_IS_DIR: i32 = 3;
const E_NOT_DIR: i32 = 4;
const E_NOT_EMPTY: i32 = 5;
const E_INVALID_TEXT: i32 = 6;
const E_FAILURE: i32 = 7;

/// How an operation classifies `error-code::not-directory` (`stat` is the read kind).
const OP_READ: i32 = 0;
const OP_WRITE: i32 = 1;
const OP_LIST: i32 = 2;
const OP_CREATE: i32 = 3;
const OP_REMOVE: i32 = 4;

/// The largest item count one stream operation may carry (the canonical ABI packs the count
/// above the four status bits).
const MAX_COUNT: i32 = (1 << 28) - 1;

/// The `error-code` discriminants this module maps (`wasi:filesystem/types`).
const EC_ACCESS: i32 = 0;
const EC_EXIST: i32 = 6;
const EC_IS_DIRECTORY: i32 = 13;
const EC_NO_ENTRY: i32 = 19;
const EC_NOT_DIRECTORY: i32 = 23;
const EC_NOT_EMPTY: i32 = 24;
const EC_NOT_PERMITTED: i32 = 30;

/// The canonical `descriptor-type::directory` discriminant.
const DT_DIRECTORY: i32 = 2;

pub(in crate::emit::runtime) fn used(b: &Builder) -> bool {
    b.uses_std_host(StdHost::is_fs)
}

fn plan(b: &Builder<'_>) -> Plan {
    *b.fs_plan.as_ref().expect("the file system runtime")
}

/// Adds every `wasi:filesystem` import the runtime may use. The imports are fixed before the
/// function index space is assigned, so a runtime body never adds one.
pub(crate) fn imports(b: &mut Builder<'_>) {
    use ValType::I32;
    const T: &str = "wasi:filesystem/types@0.3.0";
    b.host_import(
        "wasi:filesystem/preopens@0.3.0",
        "get-directories",
        vec![I32],
        Vec::new(),
    );
    b.host_import(T, "[resource-drop]descriptor", vec![I32], Vec::new());
    for (name, params, results) in [
        (
            "[async-lower][method]descriptor.open-at",
            vec![I32, I32],
            vec![I32],
        ),
        (
            "[async-lower][method]descriptor.stat-at",
            vec![I32; 5],
            vec![I32],
        ),
        (
            "[async-lower][method]descriptor.create-directory-at",
            vec![I32; 4],
            vec![I32],
        ),
        (
            "[async-lower][method]descriptor.unlink-file-at",
            vec![I32; 4],
            vec![I32],
        ),
        (
            "[async-lower][method]descriptor.remove-directory-at",
            vec![I32; 4],
            vec![I32],
        ),
        (
            "[method]descriptor.read-via-stream",
            vec![I32, ValType::I64, I32],
            Vec::new(),
        ),
        (
            "[method]descriptor.write-via-stream",
            vec![I32, I32, ValType::I64],
            vec![I32],
        ),
        (
            "[method]descriptor.append-via-stream",
            vec![I32; 2],
            vec![I32],
        ),
        (
            "[method]descriptor.read-directory",
            vec![I32; 2],
            Vec::new(),
        ),
        (
            "[async-lower][stream-read-0][method]descriptor.read-via-stream",
            vec![I32; 3],
            vec![I32],
        ),
        (
            "[async-lower][future-read-1][method]descriptor.read-via-stream",
            vec![I32; 2],
            vec![I32],
        ),
        (
            "[stream-drop-readable-0][method]descriptor.read-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[future-drop-readable-1][method]descriptor.read-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[stream-new-0][method]descriptor.write-via-stream",
            Vec::new(),
            vec![ValType::I64],
        ),
        (
            "[async-lower][stream-write-0][method]descriptor.write-via-stream",
            vec![I32; 3],
            vec![I32],
        ),
        (
            "[stream-drop-writable-0][method]descriptor.write-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[async-lower][future-read-1][method]descriptor.write-via-stream",
            vec![I32; 2],
            vec![I32],
        ),
        (
            "[future-drop-readable-1][method]descriptor.write-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[stream-new-0][method]descriptor.append-via-stream",
            Vec::new(),
            vec![ValType::I64],
        ),
        (
            "[async-lower][stream-write-0][method]descriptor.append-via-stream",
            vec![I32; 3],
            vec![I32],
        ),
        (
            "[stream-drop-writable-0][method]descriptor.append-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[async-lower][future-read-1][method]descriptor.append-via-stream",
            vec![I32; 2],
            vec![I32],
        ),
        (
            "[future-drop-readable-1][method]descriptor.append-via-stream",
            vec![I32],
            Vec::new(),
        ),
        (
            "[async-lower][stream-read-0][method]descriptor.read-directory",
            vec![I32; 3],
            vec![I32],
        ),
        (
            "[async-lower][future-read-1][method]descriptor.read-directory",
            vec![I32; 2],
            vec![I32],
        ),
        (
            "[stream-drop-readable-0][method]descriptor.read-directory",
            vec![I32],
            Vec::new(),
        ),
        (
            "[future-drop-readable-1][method]descriptor.read-directory",
            vec![I32],
            Vec::new(),
        ),
    ] {
        b.host_import(T, name, params, results);
    }
}

fn import(b: &mut Builder<'_>, name: &str, params: Vec<ValType>, results: Vec<ValType>) -> u32 {
    b.host_import("wasi:filesystem/types@0.3.0", name, params, results)
}

fn descriptor_drop(b: &mut Builder<'_>) -> u32 {
    import(
        b,
        "[resource-drop]descriptor",
        vec![ValType::I32],
        Vec::new(),
    )
}

/// Allocates the runtime's cells, descriptors and static strings. Runs in `runtime::register`,
/// before the runtime function indices are assigned (it has no index side effect).
pub(in crate::emit::runtime) fn prepare(b: &mut Builder<'_>) {
    if !used(b) {
        return;
    }
    let root = b.alloc_static(4);
    let err = b.alloc_static(4);
    let msg = b.alloc_static(4);
    let text = b.alloc_static(4);
    let names = b.alloc_static(4);
    let count = b.alloc_static(4);
    let cap = b.alloc_static(4);
    let ty = b.alloc_static(4);
    // A `descriptor-stat` is 104 bytes and sits at offset 8 of the `result` (alignment 8).
    let buf = b.alloc_static(128);
    // path-flags, path pointer, path length, open-flags, descriptor-flags.
    let params = b.alloc_static(32);
    // 16 `directory-entry` records of 24 bytes.
    let dirbuf = b.alloc_static(24 * 16);
    let raw_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "fs_raw".to_owned(),
        show: None,
        refs: Vec::new(),
        stride: 0,
    });
    let outcome_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "fs_outcome".to_owned(),
        show: None,
        refs: vec![false; 5],
        stride: 0,
    });
    let empty = b.string_literal(b"");
    let err_table = b.alloc_static(8 * 4);
    for (i, name) in [
        "",
        "FileNotFound",
        "FileAccessDenied",
        "IsADirectory",
        "NotADirectory",
        "DirectoryNotEmpty",
        "InvalidText",
        "FileSystemFailure",
    ]
    .into_iter()
    .enumerate()
    {
        let p = b.string_literal(name.as_bytes());
        b.write_u32(err_table + i as u32 * 4, p);
    }
    b.write_u32(msg, empty);
    b.write_u32(text, empty);
    b.fs_plan = Some(Plan {
        root,
        err,
        msg,
        text,
        names,
        count,
        cap,
        ty,
        buf,
        params,
        dirbuf,
        raw_desc,
        outcome_desc,
        empty,
        err_table,
    });
}

pub(in crate::emit::runtime) fn emit(
    b: &mut Builder<'_>,
    name: &str,
    ins: &mut InstructionSink,
) -> bool {
    match name {
        "fs_alloc_raw" => alloc_raw(b, ins),
        "fs_utf8" => utf8(ins),
        "fs_root" => root(b, ins),
        "fs_await" => await_(b, ins),
        "fs_wait_code" => wait_code(b, ins),
        "fs_reset" => reset(b, ins),
        "fs_classify" => classify(ins),
        "fs_open" => open(b, ins),
        "fs_stat" => stat(b, ins),
        "fs_read_all" => read_all(b, ins),
        "fs_write_all" => write_all(b, ins),
        "fs_mkdir" => mkdir(b, ins),
        "fs_names_push" => names_push(b, ins),
        "fs_names_sort" => names_sort(b, ins),
        "fs_outcome" => outcome(b, ins),
        "fs_read_text" => read_text(b, ins),
        "fs_write_text" => write_text(b, ins, false),
        "fs_append_text" => write_text(b, ins, true),
        "fs_exists" => exists(b, ins, false),
        "fs_is_directory" => exists(b, ins, true),
        "fs_list_directory" => list_directory(b, ins),
        "fs_create_directory" => create_directory(b, ins),
        "fs_remove" => remove(b, ins),
        "fs_outcome_error" => outcome_string(b, ins, OUT_ERR),
        "fs_outcome_message" => outcome_string(b, ins, OUT_MSG),
        "fs_outcome_text" => outcome_string(b, ins, OUT_TEXT),
        "fs_outcome_name_count" => {
            ins.local_get(0);
            ins.i32_load(mem(OUT_COUNT, 4));
        }
        "fs_outcome_name_at" => outcome_name_at(b, ins),
        "fs_release_outcome" => {}
        _ => return false,
    }
    true
}

// ---- small helpers ----

/// `fs_alloc_raw(size)`: a raw block of at least `size` bytes, from its header (`HDR`).
fn alloc_raw(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SLOTS: u32 = 1;
    let p = plan(b);
    let alloc = b.rt("alloc");
    ins.local_get(0);
    ins.i32_const(7);
    ins.i32_add();
    ins.i32_const(3);
    ins.i32_shr_u();
    ins.local_set(SLOTS);
    ins.local_get(SLOTS);
    ins.i32_const(p.raw_desc as i32);
    ins.call(alloc);
}

/// `fs_utf8(ptr, len) -> i32`: whether the bytes are valid UTF-8 (the fatal WHATWG decoder's
/// rules: no overlong forms, no surrogates, nothing above U+10FFFF).
pub(in crate::emit::runtime) fn utf8(ins: &mut InstructionSink) {
    const I: u32 = 2;
    const N: u32 = 3;
    const C: u32 = 4;
    const LO: u32 = 5;
    const HI: u32 = 6;
    const J: u32 = 7;
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
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(C);
    // ASCII
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
    // Two-byte forms.
    ins.local_get(C);
    ins.i32_const(0xC2);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xDF);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(N);
    ins.else_();
    // Three-byte forms.
    ins.local_get(C);
    ins.i32_const(0xE0);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xEF);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(3);
    ins.local_set(N);
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
    // Four-byte forms.
    ins.local_get(C);
    ins.i32_const(0xF0);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(0xF4);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(4);
    ins.local_set(N);
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
    ins.local_get(N);
    ins.i32_add();
    ins.local_get(1);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(1, 1));
    ins.local_set(C);
    ins.local_get(C);
    ins.local_get(LO);
    ins.i32_lt_u();
    ins.local_get(C);
    ins.local_get(HI);
    ins.i32_gt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    // The remaining continuation bytes.
    ins.i32_const(2);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(J);
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
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(1);
}

/// `fs_reset()`: the operation's outcome cells back to their initial values.
fn reset(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let p = plan(b);
    for (cell, value) in [
        (p.err, 0),
        (p.msg, p.empty),
        (p.text, p.empty),
        (p.names, 0),
        (p.count, 0),
        (p.cap, 0),
        (p.ty, 0),
    ] {
        ins.i32_const(cell as i32);
        ins.i32_const(value as i32);
        ins.i32_store(mem(0, 4));
    }
}

/// `fs_root()`: the first preopened descriptor, fetched once.
fn root(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PTR: u32 = 0;
    const LEN: u32 = 1;
    const FD: u32 = 2;
    let p = plan(b);
    let get = b.host_import(
        "wasi:filesystem/preopens@0.3.0",
        "get-directories",
        vec![ValType::I32],
        Vec::new(),
    );
    ins.i32_const(p.root as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.root as i32);
    ins.i32_load(mem(0, 4));
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.call(get);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // No preopen: there is no directory to work in. Report it as a host failure.
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(PTR);
    // The first entry is `tuple<descriptor, string>`: the handle is its first word.
    ins.local_get(PTR);
    ins.i32_load(mem(0, 4));
    ins.local_set(FD);
    ins.i32_const(p.root as i32);
    ins.local_get(FD);
    ins.i32_store(mem(0, 4));
    ins.local_get(FD);
}

/// `fs_await(packed)`: block until the subtask wrapped in `packed` completes, and return its
/// final status. A call that completed synchronously (`RETURNED`) has no subtask to wait for.
fn await_(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const STATUS: u32 = 1;
    const HANDLE: u32 = 2;
    const SET: u32 = 3;
    const ST: u32 = 4;
    const EVENT: u32 = 5;
    let set_new = b.waitable_set_new();
    let join = b.waitable_join();
    let wait = b.waitable_set_wait();
    let set_drop = b.waitable_set_drop();
    let subtask_drop = b.subtask_drop();
    ins.local_get(0);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(STATUS);
    ins.local_get(0);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(HANDLE);
    // RETURNED: the result is already in the buffer.
    ins.local_get(STATUS);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(STATUS);
    ins.return_();
    ins.end();
    // START_CANCELLED / RETURN_CANCELLED: drop the handle and report the status.
    ins.local_get(STATUS);
    ins.i32_const(3);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(HANDLE);
    ins.call(subtask_drop);
    ins.local_get(STATUS);
    ins.return_();
    ins.end();
    ins.call(set_new);
    ins.local_set(SET);
    ins.local_get(HANDLE);
    ins.local_get(SET);
    ins.call(join);
    // Wait until the subtask itself reports a terminal state; an event for any other waitable
    // or of another kind is skipped, so the subtask always leaves the set before it is dropped.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(SET);
    ins.i32_const(WAITBUF as i32);
    ins.call(wait);
    ins.local_set(EVENT);
    ins.local_get(EVENT);
    ins.i32_const(1); // EVENT_SUBTASK
    ins.i32_ne();
    ins.br_if(0);
    ins.i32_const(WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(HANDLE);
    ins.i32_ne();
    ins.br_if(0);
    ins.i32_const((WAITBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_eq();
    ins.local_get(ST);
    ins.i32_const(4);
    ins.i32_eq();
    ins.i32_or();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HANDLE);
    ins.i32_const(0);
    ins.call(join);
    ins.local_get(SET);
    ins.call(set_drop);
    ins.local_get(HANDLE);
    ins.call(subtask_drop);
    ins.local_get(ST);
}

/// `fs_wait_code(waitable)`: wait for a stream or future operation and return its packed code.
pub(in crate::emit::runtime) fn wait_code(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SET: u32 = 1;
    const CODE: u32 = 2;
    const EVENT: u32 = 3;
    let set_new = b.waitable_set_new();
    let join = b.waitable_join();
    let wait = b.waitable_set_wait();
    let set_drop = b.waitable_set_drop();
    ins.call(set_new);
    ins.local_set(SET);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(SET);
    ins.call(join);
    ins.local_get(SET);
    ins.i32_const(WAITBUF as i32);
    ins.call(wait);
    ins.local_set(EVENT);
    // Only a stream or future event (2 to 5) for this waitable carries the operation's code.
    ins.local_get(EVENT);
    ins.i32_const(2);
    ins.i32_sub();
    ins.i32_const(3);
    ins.i32_gt_u();
    ins.br_if(0);
    ins.i32_const(WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(0);
    ins.i32_ne();
    ins.br_if(0);
    // `waitable-set.wait` writes `[the waitable that fired, its code]`; the code is the
    // operation's return code.
    ins.i32_const((WAITBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_ne();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.i32_const(0);
    ins.call(join);
    ins.local_get(SET);
    ins.call(set_drop);
    ins.local_get(CODE);
}

/// `fs_classify(code_ptr, op)`: the error index of the `error-code` at `code_ptr`.
fn classify(ins: &mut InstructionSink) {
    const DISC: u32 = 2;
    ins.local_get(0);
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(DISC);
    // access and not-permitted are a permission denial.
    ins.local_get(DISC);
    ins.i32_const(EC_ACCESS);
    ins.i32_eq();
    ins.local_get(DISC);
    ins.i32_const(EC_NOT_PERMITTED);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_DENIED);
    ins.return_();
    ins.end();
    // exist: a create is NotADirectory, a remove DirectoryNotEmpty, anything else a failure.
    ins.local_get(DISC);
    ins.i32_const(EC_EXIST);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_const(OP_CREATE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_NOT_DIR);
    ins.return_();
    ins.end();
    ins.local_get(1);
    ins.i32_const(OP_REMOVE);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_NOT_EMPTY);
    ins.return_();
    ins.end();
    ins.i32_const(E_FAILURE);
    ins.return_();
    ins.end();
    ins.local_get(DISC);
    ins.i32_const(EC_IS_DIRECTORY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_IS_DIR);
    ins.return_();
    ins.end();
    ins.local_get(DISC);
    ins.i32_const(EC_NO_ENTRY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_NOT_FOUND);
    ins.return_();
    ins.end();
    // not-directory: listing and creating answer NotADirectory, reading and writing answer
    // FileNotFound, like the JS/C classification of ENOTDIR.
    ins.local_get(DISC);
    ins.i32_const(EC_NOT_DIRECTORY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.i32_const(OP_LIST);
    ins.i32_eq();
    ins.local_get(1);
    ins.i32_const(OP_CREATE);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_NOT_DIR);
    ins.return_();
    ins.end();
    ins.i32_const(E_NOT_FOUND);
    ins.return_();
    ins.end();
    ins.local_get(DISC);
    ins.i32_const(EC_NOT_EMPTY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(E_NOT_EMPTY);
    ins.return_();
    ins.end();
    ins.i32_const(E_FAILURE);
}

/// `fs_open(root, path, open_flags, descriptor_flags, op)`: the descriptor, or -1 with `err` set.
fn open(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ST: u32 = 5;
    let p = plan(b);
    let open_at = import(
        b,
        "[async-lower][method]descriptor.open-at",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let await_ = b.rt("fs_await");
    // `open-at` is a method: the receiver comes first, then the five flat parameters. They do
    // not fit the async limit, so the call takes a pointer to the whole area:
    // `[self, path-flags, path.ptr, path.len, open-flags, flags]`.
    ins.i32_const(p.params as i32);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.i32_const((p.params + 4) as i32);
    ins.i32_const(1); // path-flags::symlink-follow
    ins.i32_store(mem(0, 4));
    ins.i32_const((p.params + 8) as i32);
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.i32_const((p.params + 12) as i32);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.i32_store(mem(0, 4));
    ins.i32_const((p.params + 16) as i32);
    ins.local_get(2);
    ins.i32_store(mem(0, 4));
    ins.i32_const((p.params + 20) as i32);
    ins.local_get(3);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.params as i32);
    ins.i32_const(p.buf as i32);
    ins.call(open_at);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(-1);
    ins.return_();
    ins.end();
    // result<descriptor, error-code>: discriminant at 0, payload at 4.
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.local_get(4);
    ins.call(b.rt("fs_classify"));
    ins.i32_store(mem(0, 4));
    ins.i32_const(-1);
    ins.return_();
    ins.end();
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load(mem(0, 4));
}

/// `fs_stat(root, path, path_flags) -> i32`: 0 on success (the type is in `ty`), -1 with `err`
/// set. `path_flags` is 1 (`symlink-follow`) to describe a final symbolic link's target, 0 to
/// describe the link itself.
fn stat(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ST: u32 = 3;
    let p = plan(b);
    let stat_at = import(
        b,
        "[async-lower][method]descriptor.stat-at",
        vec![ValType::I32; 5],
        vec![ValType::I32],
    );
    let await_ = b.rt("fs_await");
    ins.local_get(0);
    ins.local_get(2);
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(p.buf as i32);
    ins.call(stat_at);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(-1);
    ins.return_();
    ins.end();
    // result<descriptor-stat, error-code>: the payload (either arm) is at offset 8.
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 8) as i32);
    ins.i32_const(OP_READ);
    ins.call(b.rt("fs_classify"));
    ins.i32_store(mem(0, 4));
    ins.i32_const(-1);
    ins.return_();
    ins.end();
    ins.i32_const(p.ty as i32);
    ins.i32_const((p.buf + 8) as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_store(mem(0, 4));
    ins.i32_const(0);
}

/// Sets `err` from the `result<_, error-code>` a future read left in `buf`. A read whose code
/// is not `COMPLETED` delivered no value, so `buf` still holds earlier bytes and the operation
/// is a `FileSystemFailure`.
fn future_result(ins: &mut InstructionSink, p: Plan, code: u32, op: i32, classify: u32) {
    ins.local_get(code);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.else_();
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_const(op);
    ins.call(classify);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.end();
}
