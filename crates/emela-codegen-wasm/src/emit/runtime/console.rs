//! `Std.Console` on WASI 0.3: a line to stdout/stderr.
//!
//! `print_line` and `print_error_line` write the text, then a newline. The p3 writes are synchronous (the runtime waits for the stream's future), so
//! stdout written before a `print_error_line` has already been consumed and the order is kept
//! without an explicit flush. A write error is ignored.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("print_line", &[ValType::I32], &[]),
    ("print_error_line", &[ValType::I32], &[]),
];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_console()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    let write = match name {
        "print_line" => rt(b, "write_stdout"),
        "print_error_line" => rt(b, "write_stderr"),
        _ => return false,
    };
    line(ins, write);
    true
}

/// `line(text)`: write the string object's bytes, then a newline, to `write`.
fn line(ins: &mut InstructionSink, write: u32) {
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.call(write);
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(10);
    ins.i32_store8(mem(0, 1));
    ins.i32_const(NEWLINE as i32);
    ins.i32_const(1);
    ins.call(write);
}
