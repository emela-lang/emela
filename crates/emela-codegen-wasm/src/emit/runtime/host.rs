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
            (7, ValType::I32), // locals 3..9: reader, writer, future, set, code, ptr, remaining
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

    let set_new = b.waitable_set_new();
    let set_wait = b.waitable_set_wait();
    let set_drop = b.waitable_set_drop();
    let join = b.waitable_join();

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
    ins.local_get(W);
    ins.local_get(PTR);
    ins.local_get(REM);
    ins.call(imports.stream_write);
    ins.local_set(CODE);
    // `-1` (BLOCKED): wait for the stream's event and use its payload as the status.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_ne();
    ins.br_if(1);
    ins.local_get(W);
    ins.local_get(SET);
    ins.call(join);
    ins.local_get(SET);
    ins.i32_const(WAITBUF as i32);
    ins.call(set_wait);
    ins.drop();
    ins.i32_const(WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(CODE);
    ins.br(0);
    ins.end();
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
    ins.local_get(W);
    ins.i32_const(0);
    ins.call(join);
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
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(F);
    ins.i32_const(FUTUREBUF as i32);
    ins.call(imports.future_read);
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_ne();
    ins.br_if(1);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(F);
    ins.local_get(SET);
    ins.call(join);
    ins.local_get(SET);
    ins.i32_const(WAITBUF as i32);
    ins.call(set_wait);
    ins.drop();
    ins.i32_const(WAITBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.local_set(CODE);
    ins.local_get(CODE);
    ins.i32_const(-1);
    ins.i32_ne();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.br(1);
    ins.end();
    ins.end();

    ins.local_get(F);
    ins.i32_const(0);
    ins.call(join);
    ins.local_get(F);
    ins.call(imports.future_drop_readable);
    ins.local_get(SET);
    ins.call(set_drop);
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
