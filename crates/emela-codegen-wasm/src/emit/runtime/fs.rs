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

use super::*;

pub(super) const FUNCS: &[Sig] = &[
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

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
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
        // i, j, a, b
        "fs_names_sort" => i32s(4),
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

pub(super) fn used(b: &Builder) -> bool {
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
pub(super) fn prepare(b: &mut Builder<'_>) {
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
    });
    let outcome_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "fs_outcome".to_owned(),
        show: None,
        refs: vec![false; 5],
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

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
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
fn utf8(ins: &mut InstructionSink) {
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
fn wait_code(b: &mut Builder<'_>, ins: &mut InstructionSink) {
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

/// `fs_read_all(fd)`: the file's text, or 0 with `err` set.
fn read_all(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const BUF: u32 = 1;
    const CAP: u32 = 2;
    const LEN: u32 = 3;
    const CHUNK: u32 = 4;
    const CODE: u32 = 5;
    const AMOUNT: u32 = 6;
    const STATUS: u32 = 7;
    const STREAM: u32 = 8;
    const FUTURE: u32 = 9;
    const S: u32 = 10;
    let p = plan(b);
    let read_via = import(
        b,
        "[method]descriptor.read-via-stream",
        vec![ValType::I32, ValType::I64, ValType::I32],
        Vec::new(),
    );
    let stream_read = import(
        b,
        "[async-lower][stream-read-0][method]descriptor.read-via-stream",
        vec![ValType::I32; 3],
        vec![ValType::I32],
    );
    let future_read = import(
        b,
        "[async-lower][future-read-1][method]descriptor.read-via-stream",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let drop_stream = import(
        b,
        "[stream-drop-readable-0][method]descriptor.read-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let drop_future = import(
        b,
        "[future-drop-readable-1][method]descriptor.read-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let wait_code = b.rt("fs_wait_code");
    let alloc_raw = b.rt("fs_alloc_raw");
    let str_from_raw = b.rt("str_from_raw");
    let classify = b.rt("fs_classify");
    let utf8 = b.rt("fs_utf8");
    // read-via-stream(fd, 0, buf) -> [stream, future]
    ins.local_get(0);
    ins.i64_const(0);
    ins.i32_const(p.buf as i32);
    ins.call(read_via);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STREAM);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FUTURE);
    ins.i32_const(4096);
    ins.call(alloc_raw);
    ins.local_set(BUF);
    ins.i32_const(4096);
    ins.local_set(CAP);
    ins.i32_const(0);
    ins.local_set(LEN);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // Grow the buffer when it is full.
    ins.local_get(LEN);
    ins.local_get(CAP);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(CAP);
    ins.i32_const(1);
    ins.i32_shl();
    ins.call(alloc_raw);
    ins.local_set(S);
    ins.local_get(S);
    ins.local_get(BUF);
    ins.local_get(LEN);
    ins.memory_copy(0, 0);
    ins.local_get(S);
    ins.local_set(BUF);
    ins.local_get(CAP);
    ins.i32_const(1);
    ins.i32_shl();
    ins.local_set(CAP);
    ins.end();
    // chunk = min(cap - len, MAX_COUNT)
    ins.local_get(CAP);
    ins.local_get(LEN);
    ins.i32_sub();
    ins.local_set(CHUNK);
    ins.local_get(CHUNK);
    ins.i32_const(MAX_COUNT);
    ins.local_get(CHUNK);
    ins.i32_const(MAX_COUNT);
    ins.i32_lt_u();
    ins.select();
    ins.local_set(CHUNK);
    ins.local_get(STREAM);
    ins.local_get(BUF);
    ins.local_get(LEN);
    ins.i32_add();
    ins.local_get(CHUNK);
    ins.call(stream_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(STREAM);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    ins.local_get(CODE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(AMOUNT);
    ins.local_get(CODE);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(STATUS);
    ins.local_get(LEN);
    ins.local_get(AMOUNT);
    ins.i32_add();
    ins.local_set(LEN);
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    // The future carries the read error, if any.
    ins.local_get(FUTURE);
    ins.i32_const(p.buf as i32);
    ins.call(future_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(FUTURE);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    future_result(ins, p, CODE, OP_READ, classify);
    ins.local_get(STREAM);
    ins.call(drop_stream);
    ins.local_get(FUTURE);
    ins.call(drop_future);
    // Invalid UTF-8 is InvalidText, like the JS fatal decoder and the C validator; the bytes are
    // never read with replacement characters, so writing the text back would not change the file.
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(BUF);
    ins.local_get(LEN);
    ins.call(utf8);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_INVALID_TEXT);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.end();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(BUF);
    ins.local_get(LEN);
    ins.call(str_from_raw);
}

/// `fs_write_all(fd, text, append)`: writes or appends `text`, setting `err` on failure.
fn write_all(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PACKED: u32 = 3;
    const READER: u32 = 4;
    const WRITER: u32 = 5;
    const FUTURE: u32 = 6;
    const PTR: u32 = 7;
    const REM: u32 = 8;
    const CHUNK: u32 = 9;
    const CODE: u32 = 10;
    const AMOUNT: u32 = 11;
    const STATUS: u32 = 12;
    let p = plan(b);
    let stream_new = import(
        b,
        "[stream-new-0][method]descriptor.write-via-stream",
        Vec::new(),
        vec![ValType::I64],
    );
    let write_via = import(
        b,
        "[method]descriptor.write-via-stream",
        vec![ValType::I32, ValType::I32, ValType::I64],
        vec![ValType::I32],
    );
    let append_via = import(
        b,
        "[method]descriptor.append-via-stream",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let append_stream_new = import(
        b,
        "[stream-new-0][method]descriptor.append-via-stream",
        Vec::new(),
        vec![ValType::I64],
    );
    let stream_write = import(
        b,
        "[async-lower][stream-write-0][method]descriptor.write-via-stream",
        vec![ValType::I32; 3],
        vec![ValType::I32],
    );
    let drop_writer = import(
        b,
        "[stream-drop-writable-0][method]descriptor.write-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let future_read = import(
        b,
        "[async-lower][future-read-1][method]descriptor.write-via-stream",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let drop_future = import(
        b,
        "[future-drop-readable-1][method]descriptor.write-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let append_stream_write = import(
        b,
        "[async-lower][stream-write-0][method]descriptor.append-via-stream",
        vec![ValType::I32; 3],
        vec![ValType::I32],
    );
    let append_drop_writer = import(
        b,
        "[stream-drop-writable-0][method]descriptor.append-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let append_future_read = import(
        b,
        "[async-lower][future-read-1][method]descriptor.append-via-stream",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let append_drop_future = import(
        b,
        "[future-drop-readable-1][method]descriptor.append-via-stream",
        vec![ValType::I32],
        Vec::new(),
    );
    let wait_code = b.rt("fs_wait_code");
    let classify = b.rt("fs_classify");
    // A fresh stream for the bytes; the readable end goes to the host.
    ins.local_get(2);
    ins.if_(BlockType::Result(ValType::I64));
    ins.call(append_stream_new);
    ins.else_();
    ins.call(stream_new);
    ins.end();
    ins.local_set(PACKED);
    ins.local_get(PACKED);
    ins.i32_wrap_i64();
    ins.local_set(READER);
    ins.local_get(PACKED);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.i32_wrap_i64();
    ins.local_set(WRITER);
    ins.local_get(2);
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.local_get(READER);
    ins.call(append_via);
    ins.local_set(FUTURE);
    ins.else_();
    ins.local_get(0);
    ins.local_get(READER);
    ins.i64_const(0);
    ins.call(write_via);
    ins.local_set(FUTURE);
    ins.end();
    ins.local_get(1);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(PTR);
    ins.local_get(1);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(REM);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(REM);
    ins.i32_const(MAX_COUNT);
    ins.local_get(REM);
    ins.i32_const(MAX_COUNT);
    ins.i32_lt_u();
    ins.select();
    ins.local_set(CHUNK);
    ins.local_get(2);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(WRITER);
    ins.local_get(PTR);
    ins.local_get(CHUNK);
    ins.call(append_stream_write);
    ins.else_();
    ins.local_get(WRITER);
    ins.local_get(PTR);
    ins.local_get(CHUNK);
    ins.call(stream_write);
    ins.end();
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(WRITER);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    ins.local_get(CODE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(AMOUNT);
    ins.local_get(CODE);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(STATUS);
    ins.local_get(REM);
    ins.local_get(AMOUNT);
    ins.i32_sub();
    ins.local_set(REM);
    ins.local_get(PTR);
    ins.local_get(AMOUNT);
    ins.i32_add();
    ins.local_set(PTR);
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(REM);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(AMOUNT);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(2);
    ins.if_(BlockType::Empty);
    ins.local_get(WRITER);
    ins.call(append_drop_writer);
    ins.else_();
    ins.local_get(WRITER);
    ins.call(drop_writer);
    ins.end();
    // The future carries the write error, if any.
    ins.local_get(2);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(FUTURE);
    ins.i32_const(p.buf as i32);
    ins.call(append_future_read);
    ins.else_();
    ins.local_get(FUTURE);
    ins.i32_const(p.buf as i32);
    ins.call(future_read);
    ins.end();
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(FUTURE);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    future_result(ins, p, CODE, OP_WRITE, classify);
    ins.local_get(2);
    ins.if_(BlockType::Empty);
    ins.local_get(FUTURE);
    ins.call(append_drop_future);
    ins.else_();
    ins.local_get(FUTURE);
    ins.call(drop_future);
    ins.end();
}

/// `fs_mkdir(root, ptr, len) -> i32`: creates one directory; `exist` is not an error.
fn mkdir(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ST: u32 = 3;
    let p = plan(b);
    let create = import(
        b,
        "[async-lower][method]descriptor.create-directory-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    let await_ = b.rt("fs_await");
    let classify = b.rt("fs_classify");
    ins.local_get(0);
    ins.local_get(1);
    ins.local_get(2);
    ins.i32_const(p.buf as i32);
    ins.call(create);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    // Already there is fine for `create_directory`.
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(EC_EXIST);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_const(OP_CREATE);
    ins.call(classify);
    ins.i32_store(mem(0, 4));
    ins.i32_const(1);
    ins.return_();
    ins.end();
    ins.i32_const(0);
}

/// `fs_names_push(name)`: appends one string object to the listing.
fn names_push(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const NEW: u32 = 1;
    const CAP: u32 = 2;
    let p = plan(b);
    let alloc = b.rt("alloc");
    // Grow when full. The first push starts with room for four names.
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(p.cap as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.cap as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_shl();
    ins.i32_const(4);
    ins.i32_add();
    ins.local_set(CAP);
    ins.local_get(CAP);
    ins.i32_const(p.raw_desc as i32);
    ins.call(alloc);
    ins.local_set(NEW);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.local_get(NEW);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(3);
    ins.i32_shl();
    ins.memory_copy(0, 0);
    ins.end();
    ins.i32_const(p.names as i32);
    ins.local_get(NEW);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.cap as i32);
    ins.local_get(CAP);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.count as i32);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
}

/// `fs_names_sort()`: insertion sort of the listed names by UTF-8 byte order.
fn names_sort(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const I: u32 = 0;
    const J: u32 = 1;
    const A: u32 = 2;
    const B: u32 = 3;
    let p = plan(b);
    let cmp = b.rt("str_cmp");
    ins.i32_const(1);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.i32_eqz();
    ins.br_if(1);
    // A = slot[j - 1], B = slot[j].
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(A);
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(B);
    ins.local_get(A);
    ins.local_get(B);
    ins.call(cmp);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.br_if(1);
    // slot[j - 1] = B; slot[j] = A.
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_get(B);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(J);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.local_get(A);
    ins.i32_store(mem(0, 4));
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
}

/// `fs_outcome()`: the outcome block for the operation's error, message, text and names.
fn outcome(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const O: u32 = 0;
    const NAME: u32 = 1;
    let p = plan(b);
    let alloc = b.rt("alloc");
    ins.i32_const(5);
    ins.i32_const(p.outcome_desc as i32);
    ins.call(alloc);
    ins.local_set(O);
    // The error name from the table.
    ins.i32_const(p.err_table as i32);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(2);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME);
    ins.local_get(O);
    ins.local_get(NAME);
    ins.i32_store(mem(OUT_ERR, 4));
    ins.local_get(O);
    ins.i32_const(p.msg as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_MSG, 4));
    ins.local_get(O);
    ins.i32_const(p.text as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_TEXT, 4));
    ins.local_get(O);
    ins.i32_const(p.names as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_NAMES, 4));
    ins.local_get(O);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(OUT_COUNT, 4));
    ins.local_get(O);
}

fn read_text(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const FD: u32 = 2;
    const TEXT: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let read_all = b.rt("fs_read_all");
    let outcome = b.rt("fs_outcome");
    let drop_ = descriptor_drop(b);
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(0); // open-flags
    ins.i32_const(1); // descriptor-flags::read
    ins.i32_const(OP_READ);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.call(read_all);
    ins.local_set(TEXT);
    ins.local_get(FD);
    ins.call(drop_);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.text as i32);
    ins.local_get(TEXT);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
}

fn write_text(b: &mut Builder<'_>, ins: &mut InstructionSink, append: bool) {
    const ROOT: u32 = 2;
    const FD: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let write_all = b.rt("fs_write_all");
    let outcome = b.rt("fs_outcome");
    let drop_ = descriptor_drop(b);
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    // write_text truncates; append_text opens for append.
    ins.i32_const(if append { 1 } else { 9 }); // create | truncate
    ins.i32_const(2); // descriptor-flags::write
    ins.i32_const(OP_WRITE);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.local_get(1);
    ins.i32_const(if append { 1 } else { 0 });
    ins.call(write_all);
    ins.local_get(FD);
    ins.call(drop_);
    ins.call(outcome);
}

fn exists(b: &mut Builder<'_>, ins: &mut InstructionSink, directory: bool) {
    const ROOT: u32 = 1;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(1); // path-flags::symlink-follow
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    if directory {
        ins.i32_const(p.ty as i32);
        ins.i32_load(mem(0, 4));
        ins.i32_const(DT_DIRECTORY);
        ins.i32_eq();
    } else {
        ins.i32_const(1);
    }
}

fn list_directory(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const FD: u32 = 2;
    const STREAM: u32 = 3;
    const FUTURE: u32 = 4;
    const CODE: u32 = 5;
    const AMOUNT: u32 = 6;
    const STATUS: u32 = 7;
    const I: u32 = 8;
    const NAME_PTR: u32 = 9;
    const NAME_LEN: u32 = 10;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let open = b.rt("fs_open");
    let outcome = b.rt("fs_outcome");
    let wait_code = b.rt("fs_wait_code");
    let push = b.rt("fs_names_push");
    let sort = b.rt("fs_names_sort");
    let str_from_raw = b.rt("str_from_raw");
    let classify = b.rt("fs_classify");
    let drop_ = descriptor_drop(b);
    let read_directory = import(
        b,
        "[method]descriptor.read-directory",
        vec![ValType::I32; 2],
        Vec::new(),
    );
    let stream_read = import(
        b,
        "[async-lower][stream-read-0][method]descriptor.read-directory",
        vec![ValType::I32; 3],
        vec![ValType::I32],
    );
    let future_read = import(
        b,
        "[async-lower][future-read-1][method]descriptor.read-directory",
        vec![ValType::I32; 2],
        vec![ValType::I32],
    );
    let drop_stream = import(
        b,
        "[stream-drop-readable-0][method]descriptor.read-directory",
        vec![ValType::I32],
        Vec::new(),
    );
    let drop_future = import(
        b,
        "[future-drop-readable-1][method]descriptor.read-directory",
        vec![ValType::I32],
        Vec::new(),
    );
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(2); // open-flags::directory
    ins.i32_const(1); // descriptor-flags::read
    ins.i32_const(OP_LIST);
    ins.call(open);
    ins.local_set(FD);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(FD);
    ins.i32_const(p.buf as i32);
    ins.call(read_directory);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STREAM);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(FUTURE);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(STREAM);
    ins.i32_const(p.dirbuf as i32);
    ins.i32_const(16);
    ins.call(stream_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(STREAM);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    ins.local_get(CODE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(AMOUNT);
    ins.local_get(CODE);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.local_set(STATUS);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(AMOUNT);
    ins.i32_ge_u();
    ins.br_if(1);
    // One `directory-entry` is 24 bytes: type (16) then the name string (pointer, length).
    ins.i32_const((p.dirbuf + 16) as i32);
    ins.local_get(I);
    ins.i32_const(24);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME_PTR);
    ins.i32_const((p.dirbuf + 20) as i32);
    ins.local_get(I);
    ins.i32_const(24);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(NAME_LEN);
    ins.local_get(NAME_PTR);
    ins.local_get(NAME_LEN);
    ins.call(str_from_raw);
    ins.call(push);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(AMOUNT);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    // The future carries the listing error, if any.
    ins.local_get(FUTURE);
    ins.i32_const(p.buf as i32);
    ins.call(future_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(FUTURE);
    ins.call(wait_code);
    ins.local_set(CODE);
    ins.end();
    future_result(ins, p, CODE, OP_LIST, classify);
    ins.local_get(STREAM);
    ins.call(drop_stream);
    ins.local_get(FUTURE);
    ins.call(drop_future);
    ins.local_get(FD);
    ins.call(drop_);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.call(sort);
    ins.call(outcome);
}

fn create_directory(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const PTR: u32 = 2;
    const LEN: u32 = 3;
    const I: u32 = 4;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    let outcome = b.rt("fs_outcome");
    let mkdir = b.rt("fs_mkdir");
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(PTR);
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(LEN);
    // A path that starts with `/` is absolute; the preopen cannot name it.
    ins.local_get(LEN);
    ins.if_(BlockType::Empty);
    ins.local_get(PTR);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('/' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_DENIED);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    // The whole path is the last prefix.
    ins.local_get(LEN);
    ins.i32_eqz();
    ins.br_if(2);
    ins.local_get(ROOT);
    ins.local_get(PTR);
    ins.local_get(LEN);
    ins.call(mkdir);
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.br(2);
    ins.end();
    ins.local_get(PTR);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('/' as i32);
    ins.i32_eq();
    ins.local_get(I);
    ins.i32_const(0);
    ins.i32_ne();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(ROOT);
    ins.local_get(PTR);
    ins.local_get(I);
    ins.call(mkdir);
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    // A file at the path (or in the middle of it) is NotADirectory.
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(1); // path-flags::symlink-follow
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.ty as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(DT_DIRECTORY);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_NOT_DIR);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.call(outcome);
}

fn remove(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const ROOT: u32 = 1;
    const PACKED: u32 = 2;
    const ST: u32 = 3;
    let p = plan(b);
    let reset = b.rt("fs_reset");
    let root = b.rt("fs_root");
    let stat = b.rt("fs_stat");
    let outcome = b.rt("fs_outcome");
    let await_ = b.rt("fs_await");
    let classify = b.rt("fs_classify");
    let unlink = import(
        b,
        "[async-lower][method]descriptor.unlink-file-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    let rmdir = import(
        b,
        "[async-lower][method]descriptor.remove-directory-at",
        vec![ValType::I32; 4],
        vec![ValType::I32],
    );
    ins.call(reset);
    ins.call(root);
    ins.local_set(ROOT);
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    // The final symbolic link is removed itself, not its target: stat it without following,
    // so a link (to a directory, or dangling) is unlinked like a file.
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const(0);
    ins.call(stat);
    ins.drop();
    ins.i32_const(p.err as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.ty as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(DT_DIRECTORY);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(p.buf as i32);
    ins.call(rmdir);
    ins.local_set(PACKED);
    ins.else_();
    ins.local_get(ROOT);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.i32_const(p.buf as i32);
    ins.call(unlink);
    ins.local_set(PACKED);
    ins.end();
    ins.local_get(PACKED);
    ins.call(await_);
    ins.local_set(ST);
    ins.local_get(ST);
    ins.i32_const(2);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const(E_FAILURE);
    ins.i32_store(mem(0, 4));
    ins.call(outcome);
    ins.return_();
    ins.end();
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Empty);
    ins.i32_const(p.err as i32);
    ins.i32_const((p.buf + 4) as i32);
    ins.i32_const(OP_REMOVE);
    ins.call(classify);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.call(outcome);
}

/// `outcome_error(o)` / `outcome_message(o)` / `outcome_text(o)`: a fresh reference to the string
/// in `slot`.
fn outcome_string(b: &mut Builder<'_>, ins: &mut InstructionSink, slot: u32) {
    const P: u32 = 1;
    let dup = b.rt("dup");
    ins.local_get(0);
    ins.i32_load(mem(slot, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(P);
}

fn outcome_name_at(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const NAMES: u32 = 2;
    const P: u32 = 3;
    let dup = b.rt("dup");
    ins.local_get(0);
    ins.i32_load(mem(OUT_NAMES, 4));
    ins.local_set(NAMES);
    ins.local_get(NAMES);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(1);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(P);
    ins.local_get(P);
    ins.call(dup);
    ins.local_get(P);
}
