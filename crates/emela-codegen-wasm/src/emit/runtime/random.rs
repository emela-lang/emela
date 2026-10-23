//! `Std.Random` on WASI 0.3: `wasi:random/random.get-random-u64`.
//!
//! `Int` takes the low 32 bits as a signed 32-bit integer, so it is uniform over the whole
//! signed 32-bit range. `Float` takes the top 53 bits and multiplies by 2^-53, so it is uniform
//! in `[0, 1)` and never 1.0.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("random_int", &[], &[ValType::I32]),
    ("random_float", &[], &[ValType::F64]),
];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn used(b: &Builder) -> bool {
    b.uses_random()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "random_int" => {
            ins.call(b.random_u64());
            ins.i32_wrap_i64();
        }
        "random_float" => {
            ins.call(b.random_u64());
            ins.i64_const(11);
            ins.i64_shr_u();
            ins.f64_convert_i64_u();
            // 2^-53
            ins.f64_const((1.0 / 9007199254740992.0).into());
            ins.f64_mul();
        }
        _ => return false,
    }
    true
}
