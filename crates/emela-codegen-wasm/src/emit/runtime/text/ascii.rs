//! ASCII case conversion: `String.ascii_lower` and `String.ascii_upper`, the private builtins
//! under `String.to_ascii_lowercase` and `String.to_ascii_uppercase`.
//!
//! Each copies the bytes of a String once, replacing the ASCII letters of the other case. Every
//! other byte, including each byte of a multi-byte UTF-8 sequence, is copied as it is, so the
//! result has the byte length of the input. The module is trimmable: a program that calls neither
//! function does not pay for them.

use super::*;
use emela_core::Builtin;

pub(in crate::emit::runtime) const FUNCS: &[Sig] = &[
    ("str_ascii_lower", &[ValType::I32], &[ValType::I32]),
    ("str_ascii_upper", &[ValType::I32], &[ValType::I32]),
];

pub(in crate::emit::runtime) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "str_ascii_lower" | "str_ascii_upper" => i32s(4), // n, p, i, c
        _ => Vec::new(),
    }
}

pub(in crate::emit::runtime) fn used(b: &Builder) -> bool {
    b.reaches_builtin(implemented)
}

fn implemented(op: Builtin) -> bool {
    matches!(op, Builtin::StringAsciiLower | Builtin::StringAsciiUpper)
}

pub(in crate::emit::runtime) fn emit(
    b: &mut Builder,
    name: &str,
    ins: &mut InstructionSink,
) -> bool {
    match name {
        "str_ascii_lower" => ascii_case(b, ins, 'A', 'Z', 'a' as i32 - 'A' as i32),
        "str_ascii_upper" => ascii_case(b, ins, 'a', 'z', 'A' as i32 - 'a' as i32),
        _ => return false,
    }
    true
}

/// The bytes of `s` in a new String, with the bytes from `lo` to `hi` replaced by themselves plus
/// `delta`.
fn ascii_case(b: &mut Builder, ins: &mut InstructionSink, lo: char, hi: char, delta: i32) {
    const N: u32 = 1;
    const P: u32 = 2;
    const I: u32 = 3;
    const C: u32 = 4;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(N);
    ins.call(rt(b, "str_new"));
    ins.local_set(P);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    // `s[20 + i]`
    ins.local_get(0);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(C);
    ins.local_get(C);
    ins.i32_const(lo as i32);
    ins.i32_ge_u();
    ins.local_get(C);
    ins.i32_const(hi as i32);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(C);
    ins.i32_const(delta);
    ins.i32_add();
    ins.local_set(C);
    ins.end();
    // `p[20 + i] = c`
    ins.local_get(P);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(C);
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(P);
}
