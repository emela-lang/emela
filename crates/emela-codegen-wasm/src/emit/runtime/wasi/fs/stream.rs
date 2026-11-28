//! Reading and writing a whole file through the descriptor's streams.

use super::*;

/// `fs_read_all(fd)`: the file's text, or 0 with `err` set.
pub(super) fn read_all(b: &mut Builder<'_>, ins: &mut InstructionSink) {
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
pub(super) fn write_all(b: &mut Builder<'_>, ins: &mut InstructionSink) {
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
