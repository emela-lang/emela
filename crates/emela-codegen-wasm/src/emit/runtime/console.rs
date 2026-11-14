//! `Std.Console` and the writes of SystemStdout and SystemStderr (`Std.Process`) on WASI 0.3: text
//! to stdout/stderr, on the one stream the runtime holds for each (`runtime/host.rs`).
//!
//! `print_line` and `print_error_line` write the text, then a newline; `print_stdout` and
//! `print_stderr` write the text as is (SystemStdout and SystemStderr add the newline of their
//! `print_line` to the text). The p3 writes are synchronous (the runtime waits for the stream's
//! future), so stdout written before a write to stderr has already been consumed and the order is
//! kept without an explicit flush. A write error is ignored.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("print_line", &[ValType::I32], &[]),
    ("print_error_line", &[ValType::I32], &[]),
    ("print_stdout", &[ValType::I32], &[]),
    ("print_stderr", &[ValType::I32], &[]),
];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_console()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    let (write, newline) = match name {
        "print_line" => (rt(b, "write_stdout"), true),
        "print_error_line" => (rt(b, "write_stderr"), true),
        "print_stdout" => (rt(b, "write_stdout"), false),
        "print_stderr" => (rt(b, "write_stderr"), false),
        _ => return false,
    };
    text(ins, write);
    if newline {
        ins.i32_const(NEWLINE as i32);
        ins.i32_const(10);
        ins.i32_store8(mem(0, 1));
        ins.i32_const(NEWLINE as i32);
        ins.i32_const(1);
        ins.call(write);
    }
    true
}

/// `text(s)`: write the string object's bytes to `write`.
fn text(ins: &mut InstructionSink, write: u32) {
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
}
