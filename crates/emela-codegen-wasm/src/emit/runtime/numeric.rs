//! Integer division, shifts and the float remainder.

use super::*;

pub(super) const FUNCS: &[Sig] = &[
    ("idiv", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("irem", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ineg", &[ValType::I32], &[ValType::I32]),
    ("ishl", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ishr", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("iushr", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("ldiv", &[ValType::I64, ValType::I64], &[ValType::I64]),
    ("lrem", &[ValType::I64, ValType::I64], &[ValType::I64]),
    ("lneg", &[ValType::I64], &[ValType::I64]),
    ("lshl", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("lshr", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("lushr", &[ValType::I64, ValType::I32], &[ValType::I64]),
    ("fmod", &[ValType::F64, ValType::F64], &[ValType::F64]),
];

pub(super) fn locals(_name: &str) -> Vec<(u32, ValType)> {
    Vec::new()
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "idiv" | "irem" | "ineg" => int_binop(ins, name),
        "ishl" | "ishr" | "iushr" => ishift(b, ins, name),
        "ldiv" | "lrem" | "lneg" => l_int_binop(ins, name),
        "lshl" | "lshr" | "lushr" => lshift(b, ins, name),
        "fmod" => fmod(ins),
        _ => return false,
    }
    true
}

// ---- numeric helpers ----

fn int_binop(ins: &mut InstructionSink, name: &str) {
    let neg = |ins: &mut InstructionSink| {
        ins.i32_const(0);
        ins.local_get(0);
        ins.i32_sub();
    };
    match name {
        "ineg" => neg(ins),
        "idiv" => {
            ins.local_get(1);
            ins.i32_const(-1);
            ins.i32_eq();
            ins.if_(BlockType::Result(ValType::I32));
            neg(ins);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i32_div_s();
            ins.end();
        }
        "irem" => {
            ins.local_get(1);
            ins.i32_const(-1);
            ins.i32_eq();
            ins.if_(BlockType::Result(ValType::I32));
            ins.i32_const(0);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i32_rem_s();
            ins.end();
        }
        _ => {}
    }
}

fn l_int_binop(ins: &mut InstructionSink, name: &str) {
    let neg = |ins: &mut InstructionSink| {
        ins.i64_const(0);
        ins.local_get(0);
        ins.i64_sub();
    };
    match name {
        "lneg" => neg(ins),
        "ldiv" => {
            ins.local_get(1);
            ins.i64_const(-1);
            ins.i64_eq();
            ins.if_(BlockType::Result(ValType::I64));
            neg(ins);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i64_div_s();
            ins.end();
        }
        "lrem" => {
            ins.local_get(1);
            ins.i64_const(-1);
            ins.i64_eq();
            ins.if_(BlockType::Result(ValType::I64));
            ins.i64_const(0);
            ins.else_();
            ins.local_get(0);
            ins.local_get(1);
            ins.i64_rem_s();
            ins.end();
        }
        _ => {}
    }
}

fn ishift(b: &mut Builder, ins: &mut InstructionSink, name: &str) {
    negative_shift(b, ins, Repr::I32);
    ins.local_get(1);
    ins.i32_const(32);
    ins.i32_ge_u();
    ins.if_(BlockType::Result(ValType::I32));
    match name {
        "ishr" => {
            ins.local_get(0);
            ins.i32_const(31);
            ins.i32_shr_s();
        }
        _ => {
            ins.i32_const(0);
        }
    }
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    match name {
        "ishl" => {
            ins.i32_shl();
        }
        "ishr" => {
            ins.i32_shr_s();
        }
        _ => {
            ins.i32_shr_u();
        }
    }
    ins.end();
}

fn lshift(b: &mut Builder, ins: &mut InstructionSink, name: &str) {
    negative_shift(b, ins, Repr::I64);
    ins.local_get(1);
    ins.i32_const(64);
    ins.i32_ge_u();
    ins.if_(BlockType::Result(ValType::I64));
    match name {
        "lshr" => {
            ins.local_get(0);
            ins.i64_const(63);
            ins.i64_shr_s();
        }
        _ => {
            ins.i64_const(0);
        }
    }
    ins.else_();
    ins.local_get(0);
    ins.local_get(1);
    ins.i64_extend_i32_u();
    match name {
        "lshl" => {
            ins.i64_shl();
        }
        "lshr" => {
            ins.i64_shr_s();
        }
        _ => {
            ins.i64_shr_u();
        }
    }
    ins.end();
}

/// Raises `negative shift amount: <by>` when the shift amount is negative, then returns a dummy.
fn negative_shift(b: &mut Builder, ins: &mut InstructionSink, ret: Repr) {
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    let prefix = b.string_literal(b"negative shift amount: ");
    ins.i32_const(prefix as i32);
    ins.local_get(1);
    ins.call(rt(b, "int_to_string"));
    ins.call(rt(b, "str_append"));
    ins.call(rt(b, "raise"));
    dummy_return(ins, ret);
    ins.end();
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
