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
    ("stdout_lost", &[], &[]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "stdout_lost" => vec![(2, ValType::I32)], // the fiber walked, the current one
        _ => Vec::new(),
    }
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_console()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    if name == "stdout_lost" {
        stdout_lost(b, ins);
        return true;
    }
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

/// The value of `G_STDOUT` once a write found standard output closed by its reader (`DROPPED`);
/// -1 is a stream the program closed itself, at its end.
pub(crate) const STDOUT_LOST: i32 = -2;

/// `stdout_lost()`: a write to standard output found its reader gone (`[api.std.process]`). The
/// write does not return normally: every other fiber is interrupted (11.4), and the fiber that
/// wrote unwinds as interrupted from here, through the pending interruption, unless a defect is
/// already pending. `G_STDOUT` is `STDOUT_LOST`, so later writes to standard output are dropped,
/// `Exit.attempt` does not catch what unwinds (`attempt_caught`), and the program ends with 141
/// and writes nothing (`finish`).
///
/// A program without the suspend runtime has no interruption value and runs no other fiber: the
/// pending defect is then a static String that nothing shows, and the same two checks of
/// `G_STDOUT` keep it from being caught or shown.
fn stdout_lost(b: &mut Builder, ins: &mut InstructionSink) {
    const F: u32 = 0;
    const CUR: u32 = 1;
    if let Some((fhead, cur)) = b.fiber_plan.as_ref().map(|p| (p.fhead, p.cur_fiber)) {
        let interrupt = b.rt("fb_interrupt_fiber");
        ins.i32_const(cur as i32);
        ins.i32_load(mem(0, 4));
        ins.local_set(CUR);
        ins.i32_const(fhead as i32);
        ins.i32_load(mem(0, 4));
        ins.local_set(F);
        ins.block(BlockType::Empty);
        ins.loop_(BlockType::Empty);
        ins.local_get(F);
        ins.i32_eqz();
        ins.br_if(1);
        ins.local_get(F);
        ins.local_get(CUR);
        ins.i32_ne();
        ins.if_(BlockType::Empty);
        ins.local_get(F);
        ins.i32_const(0);
        ins.call(interrupt);
        ins.end();
        ins.local_get(F);
        ins.i32_load(mem(super::fiber::F_NEXTF, 4));
        ins.local_set(F);
        ins.br(0);
        ins.end();
        ins.end();
    }
    ins.global_get(G_DEFECT);
    ins.if_(BlockType::Empty);
    ins.return_();
    ins.end();
    if b.fiber_plan.is_some() {
        ins.i32_const(0);
        let new = b.rt("fb_interrupted_new");
        ins.call(new);
    } else if b.sus.is_some() {
        let interrupted = b.rt("interrupted");
        ins.call(interrupted);
    } else {
        let value = b.string_literal(b"interrupted");
        ins.i32_const(value as i32);
    }
    ins.global_set(G_DEFECT);
}
