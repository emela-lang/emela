//! WASI 0.3 host calls: `write-via-stream` on stdout/stderr and `exit-with-code`.
//!
//! p3 has no `fd_write`: a guest creates a `stream<u8>`, hands the readable end to
//! `wasi:cli/stdout|stderr.write-via-stream`, writes the bytes into the writable end, drops
//! it, and waits for the returned `future<result<_, error-code>>`. The waits are synchronous
//! (`waitable-set.wait`); this stage does not yield to the component-model scheduler.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("debug_print", &[ValType::I32], &[]),
    ("write", &[ValType::I32, ValType::I32, ValType::I32], &[]),
    ("write_stdout", &[ValType::I32, ValType::I32], &[]),
    ("write_stderr", &[ValType::I32, ValType::I32], &[]),
    ("exit", &[ValType::I32], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "write_stdout" | "write_stderr" => vec![
            (1, ValType::I64), // local 2: the packed stream ends
            (8, ValType::I32), // locals 3..10: reader, writer, future, set, code, ptr, remaining, count
        ],
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "debug_print" => debug_print(b, ins),
        "write" => write(b, ins),
        "write_stdout" => {
            let imports = b.output_imports("wasi:cli/stdout@0.3.0");
            write_stream(b, ins, imports)
        }
        "write_stderr" => {
            let imports = b.output_imports("wasi:cli/stderr@0.3.0");
            write_stream(b, ins, imports)
        }
        "exit" => exit(b, ins),
        _ => return false,
    }
    true
}

// ---- host calls ----

/// `write(fd, ptr, len)`: `fd` 1 is stdout and 2 is stderr.
fn write(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "write_stderr"));
    ins.else_();
    ins.local_get(1);
    ins.local_get(2);
    ins.call(rt(b, "write_stdout"));
    ins.end();
}

/// `write_stdout(ptr, len)` / `write_stderr(ptr, len)`: write the bytes through a p3 stream and
/// wait until the host has consumed them and the result future is ready.
fn write_stream(b: &mut Builder, ins: &mut InstructionSink, imports: OutputImports) {
    const H: u32 = 2;
    const R: u32 = 3;
    const W: u32 = 4;
    const F: u32 = 5;
    const SET: u32 = 6;
    const CODE: u32 = 7;
    const PTR: u32 = 8;
    const REM: u32 = 9;
    const N: u32 = 10;
    /// The largest item count one `stream.write` may carry. The canonical ABI packs the status
    /// into the low four bits and the count into the upper 28, so a copy of `>= 1 << 28` items
    /// traps.
    const MAX_COUNT: i32 = (1 << 28) - 1;

    let set_new = b.waitable_set_new();
    let set_drop = b.waitable_set_drop();
    let waits = Waits {
        join: b.waitable_join(),
        wait: b.waitable_set_wait(),
    };

    // Split the new stream into its reader (to the host) and writer (our bytes).
    ins.call(imports.stream_new);
    ins.local_set(H);
    ins.local_get(H);
    ins.i32_wrap_i64();
    ins.local_set(R);
    ins.local_get(H);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.i32_wrap_i64();
    ins.local_set(W);
    ins.local_get(R);
    ins.call(imports.write);
    ins.local_set(F);
    ins.call(set_new);
    ins.local_set(SET);
    ins.local_get(0);
    ins.local_set(PTR);
    ins.local_get(1);
    ins.local_set(REM);

    // Write every byte, waiting whenever the host has not accepted the whole buffer yet.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // One copy is limited to `(1 << 28) - 1` items. Larger buffers are written in chunks.
    ins.local_get(REM);
    ins.i32_const(MAX_COUNT);
    ins.local_get(REM);
    ins.i32_const(MAX_COUNT);
    ins.i32_lt_u();
    ins.select();
    ins.local_set(N);
    ins.local_get(W);
    ins.local_get(PTR);
    ins.local_get(N);
    ins.call(imports.stream_write);
    ins.local_set(CODE);
    // `-1` (BLOCKED): wait for the stream's event and use its payload as the status.
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    wait_event(ins, &waits, W, SET);
    ins.local_set(CODE);
    ins.end();
    // The status is `amount << 4 | code`; advance by the amount.
    ins.local_get(CODE);
    ins.i32_const(4);
    ins.i32_shr_u();
    ins.local_set(R);
    ins.local_get(REM);
    ins.local_get(R);
    ins.i32_sub();
    ins.local_set(REM);
    ins.local_get(PTR);
    ins.local_get(R);
    ins.i32_add();
    ins.local_set(PTR);
    // `DROPPED` (1) and `CANCELLED` (2) are terminal: the host will reject a further write.
    // Only `COMPLETED` (0) may continue with the remaining bytes.
    ins.local_get(CODE);
    ins.i32_const(0xF);
    ins.i32_and();
    ins.br_if(1);
    // Done when everything was written, or when the other end hung up (no progress).
    ins.local_get(REM);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(R);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();

    // Close the writable end so the host sees the end of the stream.
    ins.local_get(W);
    ins.call(imports.stream_drop_writable);

    // Wait for `future<result<_, error-code>>`; the value itself is not used (writing to
    // stdout/stderr is best effort, like `fd_write` before).
    ins.local_get(F);
    ins.i32_const(FUTUREBUF as i32);
    ins.call(imports.future_read);
    ins.i32_const(-1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    wait_event(ins, &waits, F, SET);
    ins.drop();
    ins.end();

    ins.local_get(F);
    ins.call(imports.future_drop_readable);
    ins.local_get(SET);
    ins.call(set_drop);
}

/// The canonical built-ins `wait_event` calls.
struct Waits {
    join: u32,
    wait: u32,
}

/// Blocks until the waitable `handle` has an event and leaves the event's payload on the stack.
/// `handle` is joined to the waitable set `set` only for the wait.
fn wait_event(ins: &mut InstructionSink, waits: &Waits, handle: u32, set: u32) {
    ins.local_get(handle);
    ins.local_get(set);
    ins.call(waits.join);
    ins.local_get(set);
    ins.i32_const(WAITBUF as i32);
    ins.call(waits.wait);
    ins.drop();
    ins.local_get(handle);
    ins.i32_const(0);
    ins.call(waits.join);
    ins.i32_const(WAITBUF as i32);
    ins.i32_load(mem(0, 4));
}

fn debug_print(b: &mut Builder, ins: &mut InstructionSink) {
    let write = rt(b, "write");
    ins.i32_const(2);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(10);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(2);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(1);
    ins.call(write);
}

fn exit(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.call(b.exit_with_code());
    ins.unreachable();
}
