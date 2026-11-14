//! The arguments of `Std.Process` on WASI 0.3: `wasi:cli/environment.get-arguments`.
//!
//! The host returns a `list<string>` through a return pointer (`HOSTBUF`), allocated in linear
//! memory with `cabi_realloc` (in `canon`), like `get-environment` (`runtime/env.rs`). The list is
//! fetched once and cached in the `G_ARGS_*` globals, so every call sees the same arguments
//! (`[api.std.process]`). Its first element is the name of the program, which `Args` leaves out.
//!
//! The tests and an HTTP service have no arguments (`[api.std.process]`): their output does not
//! import `get-arguments`, and `args_count` is 0 there.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("args_load", &[], &[]),
    ("args_count", &[], &[ValType::I32]),
    ("args_at", &[ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        // the element
        "args_at" => i32s(1),
        _ => Vec::new(),
    }
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_std_host(StdHost::is_args)
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "args_load" => args_load(b, ins),
        "args_count" => args_count(b, ins),
        "args_at" => args_at(b, ins),
        _ => return false,
    }
    true
}

/// Fetches the list once (nothing without the import).
fn args_load(b: &mut Builder, ins: &mut InstructionSink) {
    if !b.args_from_host() {
        return;
    }
    let get = b.arguments_get();
    ins.global_get(G_ARGS_LOADED);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(HOSTBUF as i32);
    ins.call(get);
    ins.i32_const(HOSTBUF as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_ARGS_PTR);
    ins.i32_const((HOSTBUF + 4) as i32);
    ins.i32_load(mem(0, 4));
    ins.global_set(G_ARGS_LEN);
    ins.i32_const(1);
    ins.global_set(G_ARGS_LOADED);
    ins.end();
}

/// The number of arguments without the name of the program.
fn args_count(b: &mut Builder, ins: &mut InstructionSink) {
    ins.call(rt(b, "args_load"));
    // An empty list (no name either) has no arguments.
    ins.global_get(G_ARGS_LEN);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.global_get(G_ARGS_LEN);
    ins.i32_const(1);
    ins.i32_sub();
}

/// `args_at(i)`: the argument at `i` (from 0), for `0 <= i < args_count()`; `""` otherwise.
fn args_at(b: &mut Builder, ins: &mut InstructionSink) {
    const ELEMENT: u32 = 1;
    let empty = b.string_literal(b"");
    ins.call(rt(b, "args_count"));
    ins.local_get(0);
    ins.i32_le_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(empty as i32);
    ins.return_();
    ins.end();
    // Each element is `{ptr, len}`; the element after the name of the program.
    ins.global_get(G_ARGS_PTR);
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_const(8);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(ELEMENT);
    ins.local_get(ELEMENT);
    ins.i32_load(mem(0, 4));
    ins.local_get(ELEMENT);
    ins.i32_load(mem(4, 4));
    ins.call(rt(b, "str_from_raw"));
}
