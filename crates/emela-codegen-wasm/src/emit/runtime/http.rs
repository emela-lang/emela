//! `Std.Http.serve` on WASI 0.3 (`wasi:http/types`, `[lib.http-service]`).
//!
//! The component exports `handle` of `wasi:http/handler@0.3.0` (`emit_entry`). Each call serves
//! one request to its end before it returns: the launch runs, `task.return` hands the response
//! to the host, and the body and the trailers are written after it. Every wait (the request
//! body, the response body, the trailers) blocks the task with `waitable-set.wait`, like the file
//! system runtime. A task lifted with a callback runs alone in its instance, and one that blocks
//! in `waitable-set.wait` keeps the instance, so the host starts the next `handle` only after this
//! one returned: requests are served one after another, which `[lib.http-service]` allows (the
//! host MAY call again before an earlier call returned; it is not required to). One request at a
//! time is also why the request and the response live in this module's cells: the `Received` and
//! `Draft` values of `http.emel` only stand for them, and `Incoming` and `Outgoing` are boxes of
//! the resource handles (`http_box`).
//!
//! The request is read whole by `http_read`: its method, the URL `<scheme>://<authority><path>`
//! (`http` when the host gives no scheme, `[target.wasm.output]`), the headers as received (names
//! and values; `http.emel` puts them in the client's form), and the body. Header values are
//! bytes; they are read as Latin-1, as the Headers of the JS Fetch API read them. The strings the
//! host returns through `cabi_realloc` go to an arena that is reused for every request, so serving
//! does not grow the heap.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("http_alloc_raw", &[ValType::I32], &[ValType::I32]),
    ("http_box", &[ValType::I32], &[ValType::I32]),
    ("http_utf8", &[ValType::I32; 2], &[ValType::I32]),
    ("http_wait_code", &[ValType::I32], &[ValType::I32]),
    ("http_latin1", &[ValType::I32; 2], &[ValType::I32]),
    (
        "http_option_str",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("http_reset", &[], &[]),
    ("http_read_body", &[ValType::I32], &[]),
    ("http_read", &[ValType::I32], &[ValType::I32]),
    ("http_received_text", &[ValType::I32], &[ValType::I32]),
    ("http_received_method", &[ValType::I32], &[ValType::I32]),
    ("http_received_url", &[ValType::I32], &[ValType::I32]),
    ("http_received_body", &[ValType::I32], &[ValType::I32]),
    (
        "http_received_header_count",
        &[ValType::I32],
        &[ValType::I32],
    ),
    (
        "http_received_header_name",
        &[ValType::I32; 2],
        &[ValType::I32],
    ),
    (
        "http_received_header_value",
        &[ValType::I32; 2],
        &[ValType::I32],
    ),
    ("http_new_draft", &[ValType::I32; 2], &[ValType::I32]),
    ("http_draft_header", &[ValType::I32; 3], &[ValType::I32]),
    ("http_finish_draft", &[ValType::I32], &[ValType::I32]),
    ("http_respond", &[], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "http_alloc_raw" | "http_box" => i32s(1),
        // i, continuation bytes, first byte, low bound, high bound, j
        "http_utf8" => i32s(6),
        // set, code, event code
        "http_wait_code" => i32s(3),
        // i, extra, string, destination, byte
        "http_latin1" => i32s(5),
        // i, pointer
        "http_reset" => i32s(2),
        // packed future ends, res reader, res writer, stream, trailers, buffer, capacity, length,
        // chunk, code, amount, status, new buffer, start
        "http_read_body" => vec![(1, ValType::I64), (13, ValType::I32)],
        // scheme, authority, path, joined, joined again, fields, list pointer, list length, i,
        // entry, headers
        "http_read" => i32s(11),
        "http_received_method" | "http_received_url" | "http_received_body" => i32s(1),
        "http_received_header_name" | "http_received_header_value" => i32s(1),
        // packed stream ends, packed future ends, response
        "http_finish_draft" => vec![(2, ValType::I64), (1, ValType::I32)],
        // pointer, remaining, code, amount, status, response
        "http_respond" => i32s(6),
        _ => Vec::new(),
    }
}

/// The cells and static data of the HTTP runtime.
#[derive(Clone, Copy)]
pub(crate) struct Plan {
    /// A result area for the host calls (up to `option<scheme>`, a `result` or a pair).
    pub(crate) buf: u32,
    /// The request being served: method, URL and body strings, whether the body is UTF-8, and
    /// the headers (a raw block of `[name, value]` string pairs) and their count.
    method: u32,
    url: u32,
    body: u32,
    text: u32,
    headers: u32,
    count: u32,
    /// The response being built: its `fields`, status and body string, then the `response`, the
    /// writable ends of its body stream and trailers future, and the readable end of the future
    /// `response.new` returns.
    fields: u32,
    status: u32,
    out_body: u32,
    response: u32,
    body_writer: u32,
    trailers_writer: u32,
    sent: u32,
    /// The arena `cabi_realloc` bumps while a request is read (`canon.rs`): its next free byte
    /// (0 when the arena is not in use), its start and its end.
    pub(crate) arena_cur: u32,
    arena_start: u32,
    pub(crate) arena_end: u32,
    raw_desc: u32,
    empty: u32,
    /// The method names by `method` discriminant (`get` to `patch`), then the schemes.
    methods: u32,
    http: u32,
    https: u32,
    separator: u32,
    slash: u32,
}

/// The size of the arena for the strings of one request. A request whose method, URL and
/// headers do not fit falls back to the heap (`cabi_realloc`).
const ARENA: i32 = 64 * 1024;

/// The import module of the HTTP types.
const T: &str = "wasi:http/types@0.3.0";

pub(super) fn used(b: &Builder) -> bool {
    b.uses_std_host(StdHost::is_http)
}

fn plan(b: &Builder<'_>) -> Plan {
    *b.http_plan.as_ref().expect("the HTTP runtime")
}

/// Every `wasi:http/types` import of the runtime, and the result of `handle`. The imports are
/// fixed before the function index space is assigned, so a runtime body never adds one.
pub(crate) fn imports(b: &mut Builder<'_>) {
    use ValType::{I32, I64};
    for (name, params, results) in IMPORTS {
        let params = params.iter().map(|&t| if t { I64 } else { I32 }).collect();
        let results = results.iter().map(|&t| if t { I64 } else { I32 }).collect();
        b.host_import(T, name, params, results);
    }
    task_return(b);
}

/// The `wasi:http/types` imports, with their parameters and results (`true` for i64).
const IMPORTS: &[(&str, &[bool], &[bool])] = &[
    ("[method]request.get-method", &[false; 2], &[]),
    ("[method]request.get-scheme", &[false; 2], &[]),
    ("[method]request.get-authority", &[false; 2], &[]),
    ("[method]request.get-path-with-query", &[false; 2], &[]),
    ("[method]request.get-headers", &[false], &[false]),
    ("[method]fields.copy-all", &[false; 2], &[]),
    ("[resource-drop]fields", &[false], &[]),
    ("[constructor]fields", &[], &[false]),
    ("[method]fields.append", &[false; 6], &[]),
    ("[static]request.consume-body", &[false; 3], &[]),
    ("[future-new-0][static]request.consume-body", &[], &[true]),
    (
        "[async-lower][future-write-0][static]request.consume-body",
        &[false; 2],
        &[false],
    ),
    (
        "[future-drop-writable-0][static]request.consume-body",
        &[false],
        &[],
    ),
    (
        "[async-lower][stream-read-1][static]request.consume-body",
        &[false; 3],
        &[false],
    ),
    (
        "[stream-drop-readable-1][static]request.consume-body",
        &[false],
        &[],
    ),
    (
        "[future-drop-readable-2][static]request.consume-body",
        &[false],
        &[],
    ),
    ("[static]response.new", &[false; 5], &[]),
    ("[stream-new-0][static]response.new", &[], &[true]),
    ("[future-new-1][static]response.new", &[], &[true]),
    (
        "[async-lower][stream-write-0][static]response.new",
        &[false; 3],
        &[false],
    ),
    (
        "[stream-drop-writable-0][static]response.new",
        &[false],
        &[],
    ),
    (
        "[async-lower][future-write-1][static]response.new",
        &[false; 2],
        &[false],
    ),
    (
        "[future-drop-writable-1][static]response.new",
        &[false],
        &[],
    ),
    (
        "[future-drop-readable-2][static]response.new",
        &[false],
        &[],
    ),
    ("[method]response.set-status-code", &[false; 2], &[false]),
];

fn import(b: &mut Builder<'_>, name: &str) -> u32 {
    let (_, params, results) = IMPORTS
        .iter()
        .find(|(n, ..)| *n == name)
        .unwrap_or_else(|| panic!("no HTTP import `{name}`"));
    let params = params
        .iter()
        .map(|&t| if t { ValType::I64 } else { ValType::I32 })
        .collect();
    let results = results
        .iter()
        .map(|&t| if t { ValType::I64 } else { ValType::I32 })
        .collect();
    b.host_import(T, name, params, results)
}

/// The `task.return` of `handle`: the flattened `result<response, error-code>`.
pub(crate) fn task_return(b: &mut Builder<'_>) -> u32 {
    use ValType::{I32, I64};
    b.host_import(
        "[export]wasi:http/handler@0.3.0",
        "[task-return]handle",
        vec![I32, I32, I32, I64, I32, I32, I32, I32],
        Vec::new(),
    )
}

/// Allocates the cells and static strings. Runs in `runtime::register`, before the runtime
/// function indices are assigned.
pub(super) fn prepare(b: &mut Builder<'_>) {
    if !used(b) {
        return;
    }
    let buf = b.alloc_static(64);
    let cells: Vec<u32> = (0..19).map(|_| b.alloc_static(4)).collect();
    let raw_desc = b.desc(DescKey {
        kind: 0,
        tag: 0,
        enum_id: -1,
        name: "http_raw".to_owned(),
        show: None,
        refs: Vec::new(),
    });
    let empty = b.string_literal(b"");
    // The string cells start with the empty string, so `http_reset` can always drop them.
    for &cell in &cells[0..3] {
        b.write_u32(cell, empty);
    }
    b.write_u32(cells[8], empty);
    let methods = b.alloc_static(9 * 4);
    for (i, name) in [
        "GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH",
    ]
    .into_iter()
    .enumerate()
    {
        let s = b.string_literal(name.as_bytes());
        b.write_u32(methods + i as u32 * 4, s);
    }
    let http = b.string_literal(b"http");
    let https = b.string_literal(b"https");
    let separator = b.string_literal(b"://");
    let slash = b.string_literal(b"/");
    b.http_plan = Some(Plan {
        buf,
        method: cells[0],
        url: cells[1],
        body: cells[2],
        text: cells[3],
        headers: cells[4],
        count: cells[5],
        fields: cells[6],
        status: cells[7],
        out_body: cells[8],
        response: cells[9],
        body_writer: cells[10],
        trailers_writer: cells[11],
        sent: cells[12],
        arena_cur: cells[13],
        arena_start: cells[14],
        arena_end: cells[15],
        raw_desc,
        empty,
        methods,
        http,
        https,
        separator,
        slash,
    });
}

pub(super) fn emit(b: &mut Builder<'_>, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "http_alloc_raw" => alloc_raw(b, ins),
        "http_box" => box_(b, ins),
        "http_utf8" => super::fs::utf8(ins),
        "http_wait_code" => super::fs::wait_code(b, ins),
        "http_latin1" => latin1(b, ins),
        "http_option_str" => option_str(b, ins),
        "http_reset" => reset(b, ins),
        "http_read_body" => read_body(b, ins),
        "http_read" => read(b, ins),
        "http_received_text" => {
            ins.i32_const(plan(b).text as i32);
            ins.i32_load(mem(0, 4));
        }
        "http_received_method" => owned_cell(b, ins, plan(b).method),
        "http_received_url" => owned_cell(b, ins, plan(b).url),
        "http_received_body" => owned_cell(b, ins, plan(b).body),
        "http_received_header_count" => {
            ins.i32_const(plan(b).count as i32);
            ins.i32_load(mem(0, 4));
        }
        "http_received_header_name" => header_at(b, ins, 0),
        "http_received_header_value" => header_at(b, ins, 4),
        "http_new_draft" => new_draft(b, ins),
        "http_draft_header" => draft_header(b, ins),
        "http_finish_draft" => finish_draft(b, ins),
        "http_respond" => respond(b, ins),
        _ => return false,
    }
    true
}

/// `http_alloc_raw(size)`: a raw block of at least `size` bytes, from its header (`HDR`).
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

/// `http_box(handle)`: a heap object holding a resource handle, for a value of an `@external
/// type` (`Incoming`, `Outgoing`). Such values are counted references on this target, so a bare
/// handle cannot be one. The handle is the first word after the header.
fn box_(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const P: u32 = 1;
    let alloc_raw = b.rt("http_alloc_raw");
    ins.i32_const(4);
    ins.call(alloc_raw);
    ins.local_tee(P);
    ins.local_get(0);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(P);
}

/// `http_latin1(ptr, len)`: a new string of the bytes read as Latin-1: a byte from 0x80 is the
/// code point of the same value, two bytes of UTF-8.
fn latin1(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const I: u32 = 2;
    const EXTRA: u32 = 3;
    const S: u32 = 4;
    const DST: u32 = 5;
    const BYTE: u32 = 6;
    let str_new = b.rt("str_new");
    let count_loop = |ins: &mut InstructionSink, body: &dyn Fn(&mut InstructionSink)| {
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
        ins.local_set(BYTE);
        body(ins);
        ins.local_get(I);
        ins.i32_const(1);
        ins.i32_add();
        ins.local_set(I);
        ins.br(0);
        ins.end();
        ins.end();
    };
    ins.i32_const(0);
    ins.local_set(EXTRA);
    count_loop(ins, &|ins| {
        ins.local_get(EXTRA);
        ins.local_get(BYTE);
        ins.i32_const(7);
        ins.i32_shr_u();
        ins.i32_add();
        ins.local_set(EXTRA);
    });
    ins.local_get(1);
    ins.local_get(EXTRA);
    ins.i32_add();
    ins.call(str_new);
    ins.local_set(S);
    ins.local_get(S);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(DST);
    count_loop(ins, &|ins| {
        ins.local_get(BYTE);
        ins.i32_const(0x80);
        ins.i32_lt_u();
        ins.if_(BlockType::Empty);
        ins.local_get(DST);
        ins.local_get(BYTE);
        ins.i32_store8(mem(0, 1));
        ins.local_get(DST);
        ins.i32_const(1);
        ins.i32_add();
        ins.local_set(DST);
        ins.else_();
        ins.local_get(DST);
        ins.local_get(BYTE);
        ins.i32_const(6);
        ins.i32_shr_u();
        ins.i32_const(0xC0);
        ins.i32_or();
        ins.i32_store8(mem(0, 1));
        ins.local_get(DST);
        ins.local_get(BYTE);
        ins.i32_const(0x3F);
        ins.i32_and();
        ins.i32_const(0x80);
        ins.i32_or();
        ins.i32_store8(mem(1, 1));
        ins.local_get(DST);
        ins.i32_const(2);
        ins.i32_add();
        ins.local_set(DST);
        ins.end();
    });
    ins.local_get(S);
}

/// `http_option_str(ptr, default)`: a new string of the `option<string>` at `ptr`, or `default`
/// (a static string) for `none`.
fn option_str(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let from_raw = b.rt("str_from_raw");
    ins.local_get(0);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(0);
    ins.i32_load(mem(4, 4));
    ins.local_get(0);
    ins.i32_load(mem(8, 4));
    ins.call(from_raw);
    ins.else_();
    ins.local_get(1);
    ins.end();
}

/// Stores `value` in `cell` and gives up the string the cell held.
fn replace(ins: &mut InstructionSink, drop: u32, cell: u32, value: impl Fn(&mut InstructionSink)) {
    ins.i32_const(cell as i32);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.i32_const(cell as i32);
    value(ins);
    ins.i32_store(mem(0, 4));
}

/// `http_reset()`: gives up the request served last (its strings and headers) and the response
/// built last.
fn reset(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const I: u32 = 0;
    const PTR: u32 = 1;
    let p = plan(b);
    let drop = b.rt("drop");
    for cell in [p.method, p.url, p.body, p.out_body] {
        replace(ins, drop, cell, |ins| {
            ins.i32_const(p.empty as i32);
        });
    }
    ins.i32_const(p.headers as i32);
    ins.i32_load(mem(0, 4));
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(p.count as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_ge_u();
    ins.br_if(1);
    ins.i32_const(p.headers as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(PTR);
    ins.local_get(PTR);
    ins.i32_load(mem(HDR, 4));
    ins.call(drop);
    ins.local_get(PTR);
    ins.i32_load(mem(HDR + 4, 4));
    ins.call(drop);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(p.headers as i32);
    ins.i32_load(mem(0, 4));
    ins.call(drop);
    ins.end();
    for cell in [
        p.headers,
        p.count,
        p.text,
        p.fields,
        p.status,
        p.response,
        p.body_writer,
        p.trailers_writer,
        p.sent,
    ] {
        ins.i32_const(cell as i32);
        ins.i32_const(0);
        ins.i32_store(mem(0, 4));
    }
}

/// `http_read_body(request)`: reads the whole body of `request` (which `consume-body` takes) into
/// `body`, and sets `text` when it is UTF-8. A leading BOM is dropped. A body that is not UTF-8
/// leaves `body` empty.
fn read_body(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PACKED: u32 = 1;
    const RES_READER: u32 = 2;
    const RES_WRITER: u32 = 3;
    const STREAM: u32 = 4;
    const TRAILERS: u32 = 5;
    const BUF: u32 = 6;
    const CAP: u32 = 7;
    const LEN: u32 = 8;
    const CHUNK: u32 = 9;
    const CODE: u32 = 10;
    const AMOUNT: u32 = 11;
    const STATUS: u32 = 12;
    const NEW: u32 = 13;
    const START: u32 = 14;
    let p = plan(b);
    let future_new = import(b, "[future-new-0][static]request.consume-body");
    let consume = import(b, "[static]request.consume-body");
    let stream_read = import(
        b,
        "[async-lower][stream-read-1][static]request.consume-body",
    );
    let drop_stream = import(b, "[stream-drop-readable-1][static]request.consume-body");
    let drop_trailers = import(b, "[future-drop-readable-2][static]request.consume-body");
    let future_write = import(
        b,
        "[async-lower][future-write-0][static]request.consume-body",
    );
    let drop_res = import(b, "[future-drop-writable-0][static]request.consume-body");
    let wait_code = b.rt("http_wait_code");
    let alloc_raw = b.rt("http_alloc_raw");
    let utf8 = b.rt("http_utf8");
    let from_raw = b.rt("str_from_raw");
    let drop = b.rt("drop");
    // The future that tells the host how reading the body ended.
    ins.call(future_new);
    ins.local_set(PACKED);
    ins.local_get(PACKED);
    ins.i32_wrap_i64();
    ins.local_set(RES_READER);
    ins.local_get(PACKED);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.i32_wrap_i64();
    ins.local_set(RES_WRITER);
    ins.local_get(0);
    ins.local_get(RES_READER);
    ins.i32_const(p.buf as i32);
    ins.call(consume);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(STREAM);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(4, 4));
    ins.local_set(TRAILERS);
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
    ins.local_set(NEW);
    ins.local_get(NEW);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(BUF);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(LEN);
    ins.memory_copy(0, 0);
    ins.local_get(BUF);
    ins.call(drop);
    ins.local_get(NEW);
    ins.local_set(BUF);
    ins.local_get(CAP);
    ins.i32_const(1);
    ins.i32_shl();
    ins.local_set(CAP);
    ins.end();
    ins.local_get(CAP);
    ins.local_get(LEN);
    ins.i32_sub();
    ins.local_set(CHUNK);
    ins.local_get(STREAM);
    ins.local_get(BUF);
    ins.i32_const(HDR as i32);
    ins.i32_add();
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
    // COMPLETED (0) reads on; DROPPED or CANCELLED ends the body.
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(STREAM);
    ins.call(drop_stream);
    ins.local_get(TRAILERS);
    ins.call(drop_trailers);
    // `ok`: the body was read.
    ins.i32_const(p.buf as i32);
    ins.i64_const(0);
    ins.i64_store(mem(0, 8));
    ins.local_get(RES_WRITER);
    ins.i32_const(p.buf as i32);
    ins.call(future_write);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(RES_WRITER);
    ins.call(wait_code);
    ins.drop();
    ins.end();
    ins.local_get(RES_WRITER);
    ins.call(drop_res);
    // The body as text, without a leading BOM.
    ins.local_get(BUF);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.local_get(LEN);
    ins.call(utf8);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(START);
    ins.local_get(LEN);
    ins.i32_const(3);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(BUF);
    ins.i32_load16_u(mem(HDR, 1));
    ins.i32_const(0xBBEF);
    ins.i32_eq();
    ins.local_get(BUF);
    ins.i32_load8_u(mem(HDR + 2, 1));
    ins.i32_const(0xBF);
    ins.i32_eq();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(3);
    ins.local_set(START);
    ins.end();
    ins.end();
    replace(ins, drop, p.body, |ins| {
        ins.local_get(BUF);
        ins.i32_const(HDR as i32);
        ins.i32_add();
        ins.local_get(START);
        ins.i32_add();
        ins.local_get(LEN);
        ins.local_get(START);
        ins.i32_sub();
        ins.call(from_raw);
    });
    ins.i32_const(p.text as i32);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.local_get(BUF);
    ins.call(drop);
}

/// `http_read(incoming) -> 0`: reads the request being served into the cells. `incoming` is the
/// box of the `request` handle; `consume-body` takes the handle, and the box is left to its
/// owner.
fn read(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const SCHEME: u32 = 1;
    const AUTHORITY: u32 = 2;
    const PATH: u32 = 3;
    const JOINED: u32 = 4;
    const JOINED2: u32 = 5;
    const FIELDS: u32 = 6;
    const LIST: u32 = 7;
    const LEN: u32 = 8;
    const I: u32 = 9;
    const ENTRY: u32 = 10;
    const HEADERS: u32 = 11;
    let p = plan(b);
    let get_method = import(b, "[method]request.get-method");
    let get_scheme = import(b, "[method]request.get-scheme");
    let get_authority = import(b, "[method]request.get-authority");
    let get_path = import(b, "[method]request.get-path-with-query");
    let get_headers = import(b, "[method]request.get-headers");
    let copy_all = import(b, "[method]fields.copy-all");
    let drop_fields = import(b, "[resource-drop]fields");
    let reset = b.rt("http_reset");
    let alloc_raw = b.rt("http_alloc_raw");
    let option_str = b.rt("http_option_str");
    let from_raw = b.rt("str_from_raw");
    let latin1 = b.rt("http_latin1");
    let append = b.rt("str_append");
    let drop = b.rt("drop");
    let read_body = b.rt("http_read_body");
    ins.call(reset);
    // `Incoming` is a box (`http_box`); the calls below take the handle.
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(0);
    // The arena for the host's strings, made once and reused.
    ins.i32_const(p.arena_start as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.arena_start as i32);
    ins.i32_const(ARENA);
    ins.call(alloc_raw);
    ins.i32_const(HDR as i32);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.arena_end as i32);
    ins.i32_const(p.arena_start as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(ARENA);
    ins.i32_add();
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.i32_const(p.arena_cur as i32);
    ins.i32_const(p.arena_start as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(0, 4));
    // The method: a name by its case, or the name of `other`.
    ins.local_get(0);
    ins.i32_const(p.buf as i32);
    ins.call(get_method);
    replace(ins, drop, p.method, |ins| {
        ins.i32_const(p.buf as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_const(9);
        ins.i32_eq();
        ins.if_(BlockType::Result(ValType::I32));
        ins.i32_const(p.buf as i32);
        ins.i32_load(mem(4, 4));
        ins.i32_const(p.buf as i32);
        ins.i32_load(mem(8, 4));
        ins.call(from_raw);
        ins.else_();
        ins.i32_const(p.buf as i32);
        ins.i32_load8_u(mem(0, 1));
        ins.i32_const(4);
        ins.i32_mul();
        ins.i32_load(mem(p.methods, 4));
        ins.end();
    });
    // The scheme: `http` when there is none (`[target.wasm.output]`).
    ins.local_get(0);
    ins.i32_const(p.buf as i32);
    ins.call(get_scheme);
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(0, 1));
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(4, 1));
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(8, 4));
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(12, 4));
    ins.call(from_raw);
    ins.else_();
    // `HTTP` is case 0 and `HTTPS` case 1.
    ins.i32_const(p.https as i32);
    ins.i32_const(p.http as i32);
    ins.i32_const(p.buf as i32);
    ins.i32_load8_u(mem(4, 1));
    ins.select();
    ins.end();
    ins.else_();
    ins.i32_const(p.http as i32);
    ins.end();
    ins.local_set(SCHEME);
    ins.local_get(0);
    ins.i32_const(p.buf as i32);
    ins.call(get_authority);
    ins.i32_const(p.buf as i32);
    ins.i32_const(p.empty as i32);
    ins.call(option_str);
    ins.local_set(AUTHORITY);
    ins.local_get(0);
    ins.i32_const(p.buf as i32);
    ins.call(get_path);
    ins.i32_const(p.buf as i32);
    ins.i32_const(p.slash as i32);
    ins.call(option_str);
    ins.local_set(PATH);
    // `<scheme>://<authority><path-with-query>`
    ins.local_get(SCHEME);
    ins.i32_const(p.separator as i32);
    ins.call(append);
    ins.local_set(JOINED);
    ins.local_get(JOINED);
    ins.local_get(AUTHORITY);
    ins.call(append);
    ins.local_set(JOINED2);
    replace(ins, drop, p.url, |ins| {
        ins.local_get(JOINED2);
        ins.local_get(PATH);
        ins.call(append);
    });
    for local in [SCHEME, AUTHORITY, PATH, JOINED, JOINED2] {
        ins.local_get(local);
        ins.call(drop);
    }
    // The headers as received: names as strings, values read as Latin-1.
    ins.local_get(0);
    ins.call(get_headers);
    ins.local_set(FIELDS);
    ins.local_get(FIELDS);
    ins.i32_const(p.buf as i32);
    ins.call(copy_all);
    ins.local_get(FIELDS);
    ins.call(drop_fields);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(LIST);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(4, 4));
    ins.local_set(LEN);
    ins.local_get(LEN);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_const(8);
    ins.i32_add();
    ins.call(alloc_raw);
    ins.local_set(HEADERS);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    // `tuple<field-name, field-value>`: two (pointer, length) pairs.
    ins.local_get(LIST);
    ins.local_get(I);
    ins.i32_const(16);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(ENTRY);
    ins.local_get(HEADERS);
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(ENTRY);
    ins.i32_load(mem(0, 4));
    ins.local_get(ENTRY);
    ins.i32_load(mem(4, 4));
    ins.call(from_raw);
    ins.i32_store(mem(HDR, 4));
    ins.local_get(HEADERS);
    ins.local_get(I);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_get(ENTRY);
    ins.i32_load(mem(8, 4));
    ins.local_get(ENTRY);
    ins.i32_load(mem(12, 4));
    ins.call(latin1);
    ins.i32_store(mem(HDR + 4, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(p.headers as i32);
    ins.local_get(HEADERS);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.count as i32);
    ins.local_get(LEN);
    ins.i32_store(mem(0, 4));
    // The host's strings are copied: the arena is free again.
    ins.i32_const(p.arena_cur as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.call(read_body);
    ins.i32_const(0);
}

/// Returns the string in `cell`, counted for the caller.
fn owned_cell(b: &mut Builder<'_>, ins: &mut InstructionSink, cell: u32) {
    const S: u32 = 1;
    let dup = b.rt("dup");
    ins.i32_const(cell as i32);
    ins.i32_load(mem(0, 4));
    ins.local_tee(S);
    ins.call(dup);
    ins.local_get(S);
}

/// `http_received_header_*(received, i)`: the name (`offset` 0) or value (4) of header `i`.
fn header_at(b: &mut Builder<'_>, ins: &mut InstructionSink, offset: u32) {
    const S: u32 = 2;
    let p = plan(b);
    let dup = b.rt("dup");
    ins.i32_const(p.headers as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(1);
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.i32_load(mem(HDR + offset, 4));
    ins.local_tee(S);
    ins.call(dup);
    ins.local_get(S);
}

/// `http_new_draft(status, body) -> 0`: starts the response: new `fields`, the status and the
/// body.
fn new_draft(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let p = plan(b);
    let fields_new = import(b, "[constructor]fields");
    let dup = b.rt("dup");
    let drop = b.rt("drop");
    ins.i32_const(p.fields as i32);
    ins.call(fields_new);
    ins.i32_store(mem(0, 4));
    ins.i32_const(p.status as i32);
    ins.local_get(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(1);
    ins.call(dup);
    replace(ins, drop, p.out_body, |ins| {
        ins.local_get(1);
    });
    ins.i32_const(0);
}

/// `http_draft_header(draft, name, value) -> 0`: appends a header. `http.emel` checked the name
/// and the value, so `append` does not fail.
fn draft_header(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    let p = plan(b);
    let append = import(b, "[method]fields.append");
    ins.i32_const(p.fields as i32);
    ins.i32_load(mem(0, 4));
    for s in [1, 2] {
        ins.local_get(s);
        ins.i32_const((HDR + 4) as i32);
        ins.i32_add();
        ins.local_get(s);
        ins.i32_load(mem(HDR, 4));
    }
    ins.i32_const(p.buf as i32);
    ins.call(append);
    ins.i32_const(0);
}

/// `http_finish_draft(draft) -> outgoing`: the `response` (in a box, `http_box`) with the
/// headers and the status, a stream for the body and a future for the trailers. The body is written by `http_respond`,
/// after `task.return`.
fn finish_draft(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const STREAM: u32 = 1;
    const FUTURE: u32 = 2;
    const RESPONSE: u32 = 3;
    let p = plan(b);
    let stream_new = import(b, "[stream-new-0][static]response.new");
    let future_new = import(b, "[future-new-1][static]response.new");
    let response_new = import(b, "[static]response.new");
    let set_status = import(b, "[method]response.set-status-code");
    ins.call(stream_new);
    ins.local_set(STREAM);
    ins.call(future_new);
    ins.local_set(FUTURE);
    ins.i32_const(p.fields as i32);
    ins.i32_load(mem(0, 4));
    // `some(stream)`
    ins.i32_const(1);
    ins.local_get(STREAM);
    ins.i32_wrap_i64();
    ins.local_get(FUTURE);
    ins.i32_wrap_i64();
    ins.i32_const(p.buf as i32);
    ins.call(response_new);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(RESPONSE);
    ins.i32_const(p.sent as i32);
    ins.i32_const(p.buf as i32);
    ins.i32_load(mem(4, 4));
    ins.i32_store(mem(0, 4));
    ins.local_get(RESPONSE);
    ins.i32_const(p.status as i32);
    ins.i32_load(mem(0, 4));
    ins.call(set_status);
    ins.drop();
    ins.i32_const(p.response as i32);
    ins.local_get(RESPONSE);
    ins.i32_store(mem(0, 4));
    for (cell, packed) in [(p.body_writer, STREAM), (p.trailers_writer, FUTURE)] {
        ins.i32_const(cell as i32);
        ins.local_get(packed);
        ins.i64_const(32);
        ins.i64_shr_u();
        ins.i32_wrap_i64();
        ins.i32_store(mem(0, 4));
    }
    ins.local_get(RESPONSE);
    ins.call(b.rt("http_box"));
}

/// `http_respond()`: ends `handle`. Returns the response built by the launch with `task.return`,
/// then writes its body and completes its trailers with `ok(none)`. A launch that made no
/// response (it ended interrupted, `[lib.http-service]`) returns `error-code.internal-error`.
fn respond(b: &mut Builder<'_>, ins: &mut InstructionSink) {
    const PTR: u32 = 0;
    const REM: u32 = 1;
    const CODE: u32 = 2;
    const AMOUNT: u32 = 3;
    const STATUS: u32 = 4;
    const RESPONSE: u32 = 5;
    let p = plan(b);
    let task_return = task_return(b);
    let stream_write = import(b, "[async-lower][stream-write-0][static]response.new");
    let drop_writer = import(b, "[stream-drop-writable-0][static]response.new");
    let future_write = import(b, "[async-lower][future-write-1][static]response.new");
    let drop_trailers = import(b, "[future-drop-writable-1][static]response.new");
    let drop_sent = import(b, "[future-drop-readable-2][static]response.new");
    let wait_code = b.rt("http_wait_code");
    ins.i32_const(p.response as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(RESPONSE);
    ins.local_get(RESPONSE);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // `err(internal-error(none))`: case 38 of `error-code`, its `none` in the next word.
    ins.i32_const(1);
    ins.i32_const(38);
    ins.i32_const(0);
    ins.i64_const(0);
    for _ in 0..4 {
        ins.i32_const(0);
    }
    ins.call(task_return);
    ins.return_();
    ins.end();
    // `ok(response)`
    ins.i32_const(0);
    ins.local_get(RESPONSE);
    ins.i32_const(0);
    ins.i64_const(0);
    for _ in 0..4 {
        ins.i32_const(0);
    }
    ins.call(task_return);
    // The body, in chunks the host takes.
    ins.i32_const(p.out_body as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_set(PTR);
    ins.i32_const(p.out_body as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_load(mem(HDR, 4));
    ins.local_set(REM);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(REM);
    ins.i32_eqz();
    ins.br_if(1);
    ins.i32_const(p.body_writer as i32);
    ins.i32_load(mem(0, 4));
    ins.local_get(PTR);
    ins.local_get(REM);
    ins.call(stream_write);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.body_writer as i32);
    ins.i32_load(mem(0, 4));
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
    // The host dropped the stream (the client went away): stop.
    ins.local_get(STATUS);
    ins.i32_const(0);
    ins.i32_ne();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(p.body_writer as i32);
    ins.i32_load(mem(0, 4));
    ins.call(drop_writer);
    // `ok(none)`: no trailers.
    ins.i32_const(p.buf as i32);
    ins.i64_const(0);
    ins.i64_store(mem(0, 8));
    ins.i32_const(p.buf as i32);
    ins.i64_const(0);
    ins.i64_store(mem(8, 8));
    ins.i32_const(p.trailers_writer as i32);
    ins.i32_load(mem(0, 4));
    ins.i32_const(p.buf as i32);
    ins.call(future_write);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(p.trailers_writer as i32);
    ins.i32_load(mem(0, 4));
    ins.call(wait_code);
    ins.drop();
    ins.end();
    ins.i32_const(p.trailers_writer as i32);
    ins.i32_load(mem(0, 4));
    ins.call(drop_trailers);
    ins.i32_const(p.sent as i32);
    ins.i32_load(mem(0, 4));
    ins.call(drop_sent);
    ins.i32_const(p.response as i32);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
}
