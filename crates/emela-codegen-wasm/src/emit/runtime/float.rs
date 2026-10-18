//! Floating-point runtime helpers.
//!
//! `fmod` is the `Float % Float` operation (the wasm `f64` instructions have no remainder). The
//! rest of the float arithmetic is inline in the emitter (`emit::prim`); this module will grow
//! the `Float.to_string` / `Float.parse` / rounding implementations in a later wave.

use super::*;

/// The float fragment, after `CORE_TAIL` (see `runtime::FUNCS`).
pub(super) const FUNCS: &[Sig] = &[("fmod", &[ValType::F64, ValType::F64], &[ValType::F64])];

pub(super) fn body(_b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fmod" => fmod(ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(_name: &str) -> Option<Vec<(u32, ValType)>> {
    None
}

fn fmod(ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.local_get(0);
    ins.local_get(1);
    ins.f64_div();
    ins.f64_trunc();
    ins.local_get(1);
    ins.f64_mul();
    ins.f64_sub();
}
