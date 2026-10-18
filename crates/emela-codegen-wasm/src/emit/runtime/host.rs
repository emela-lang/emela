//! The WASI host calls, the runtime side of the internal host ABI, and `print_string`.
//!
//! `write` is the only syscall wrapper (`fd_write`); `debug_print` writes a string to stderr and
//! `print_string` to stdout (both with a trailing newline), and `exit` calls `proc_exit`. The
//! string layout is the one the emitter's `string_literal` uses and `str_byte_size` reads: the
//! length is the `i32` at `ptr + HDR`, the UTF-8 bytes start at `ptr + HDR + 4`.

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
];

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "debug_print" => debug_print(b, ins),
        "write" => write(b, ins),
        "exit" => exit(b, ins),
        "print_string" => print_string(b, ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(_name: &str) -> Option<Vec<(u32, ValType)>> {
    None
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

/// `print(s)`, the internal host `("test", "print")` of the acceptance corpus: the bytes of the
/// string argument at `ptr + HDR + 4`, its length at `[ptr + HDR]`, then a newline, on stdout
/// (fd 1). It is `debug_print` with a different file descriptor.
fn print_string(b: &mut Builder, ins: &mut InstructionSink) {
    let write = rt(b, "write");
    ins.i32_const(1);
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(10);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(1);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(1);
    ins.call(write);
}

fn exit(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.call(b.proc_exit());
    ins.unreachable();
}
