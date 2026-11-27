//! Floats: the shortest round-trip display, `parse`, and the conversions to Int.
//!
//! `Float.to_string` gives the text of Float's Show: the shortest digits that read back as the
//! same binary64, with `.0` on a finite integral value, `Infinity`, `-Infinity`, `NaN`, and
//! `-0.0` shown as `0.0` (`[trait.builtin]`). wasm has no `printf`, so the runtime does the work
//! itself: it writes the *exact* decimal expansion of the value (every binary64 is a finite
//! decimal) with a base-10^9 big integer, then takes the shortest prefix of those digits that
//! still rounds back to the value.
//!
//! The rounding interval of a value is the range between the midpoints to its neighbours. Its
//! bounds are dyadic, so they get the same exact decimal treatment. A prefix round-trips exactly
//! when it lies inside the interval (or on a midpoint whose even neighbour is the value).
//!
//! `Float.parse` is the other direction: it first reads the digits into the naive value
//! (`(the first 19 digits as u64) * 10^exponent`), then moves that value by single neighbours
//! until the input decimal lies in its rounding interval. That check is exact, so the result is
//! the binary64 nearest to the input, and a tie goes to the value with the even mantissa.
//!
//! The buffers live in one static block (`prepare`), allocated only when the program uses a
//! Float builtin, so modules without Float keep their bytes.

use super::*;
use display::to_string;
use limbs::{bounds, dec_build, dec_cmp, limbs_mul, pow10, some};
use parse::parse;

use emela_core::Builtin;

mod display;
mod limbs;
mod parse;

pub(in crate::emit::runtime) const FUNCS: &[Sig] = &[
    // Exact decimal helpers (base 10^9 limbs, then ASCII digits).
    ("limbs_mul", &[ValType::I64, ValType::I32], &[ValType::I32]),
    (
        "dec_build",
        &[ValType::I64, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    (
        "dec_cmp",
        &[
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
            ValType::I32,
        ],
        &[ValType::I32],
    ),
    ("float_pow10", &[ValType::I32], &[ValType::F64]),
    ("float_bounds", &[ValType::F64], &[ValType::I32]),
    (
        "float_some",
        &[ValType::F64, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    // The builtins.
    (
        "float_to_int",
        &[ValType::F64, ValType::F64],
        &[ValType::I32],
    ),
    ("float_floor", &[ValType::F64], &[ValType::I32]),
    ("float_ceil", &[ValType::F64], &[ValType::I32]),
    ("float_round", &[ValType::F64], &[ValType::I32]),
    ("float_truncate", &[ValType::F64], &[ValType::I32]),
    ("float_to_string", &[ValType::F64], &[ValType::I32]),
    (
        "float_parse",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
];

pub(in crate::emit::runtime) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "limbs_mul" => vec![(2, ValType::I64), (1, ValType::I32)],
        "dec_build" => vec![(2, ValType::I64), (8, ValType::I32)],
        "dec_cmp" => i32s(6),
        "float_pow10" => vec![(1, ValType::I32), (2, ValType::F64)],
        "float_bounds" => vec![(3, ValType::I64), (7, ValType::I32)],
        "float_some" => i32s(1),
        "float_to_int" => i32s(3),
        "float_round" => vec![(2, ValType::F64)],
        "float_to_string" => vec![(3, ValType::I64), (25, ValType::I32)],
        "float_parse" => vec![(6, ValType::I64), (19, ValType::I32), (1, ValType::F64)],
        _ => Vec::new(),
    }
}

pub(in crate::emit::runtime) fn emit(
    b: &mut Builder,
    name: &str,
    ins: &mut InstructionSink,
) -> bool {
    match name {
        "limbs_mul" => limbs_mul(b, ins),
        "dec_build" => dec_build(b, ins),
        "dec_cmp" => dec_cmp(ins),
        "float_pow10" => pow10(ins),
        "float_bounds" => bounds(b, ins),
        "float_some" => some(b, ins),
        "float_to_int" => to_int(b, ins),
        "float_floor" => round_to_int(b, ins, "floor"),
        "float_ceil" => round_to_int(b, ins, "ceil"),
        "float_round" => round_half_away(b, ins),
        "float_truncate" => round_to_int(b, ins, "trunc"),
        "float_to_string" => to_string(b, ins),
        "float_parse" => parse(b, ins),
        _ => return false,
    }
    true
}

// ---- when the module is needed ----

/// Whether reachable code reaches a Float builtin that this module implements.
pub(in crate::emit::runtime) fn used(b: &Builder) -> bool {
    b.reaches_builtin(implemented)
}

fn implemented(op: Builtin) -> bool {
    matches!(
        op,
        Builtin::FloatToString
            | Builtin::FloatParse
            | Builtin::FloatFloor
            | Builtin::FloatCeil
            | Builtin::FloatRound
            | Builtin::FloatTruncate
    )
}

// ---- the buffers ----

/// Limbs of a base 10^9 big integer. The exact decimal of the smallest subnormal times `5^1075`
/// is 767 digits (86 limbs); the extra room is for carries.
const LIMBS: u32 = 96;
/// Bytes for one decimal digit string: more than 767, the longest exact expansion.
const DIGITS: u32 = 872;

const AREA_LIMBS: u32 = 0;
const AREA_X: u32 = LIMBS * 4;
const AREA_LO: u32 = AREA_X + DIGITS;
const AREA_HI: u32 = AREA_LO + DIGITS;
const AREA_IN: u32 = AREA_HI + DIGITS;
const AREA_Q: u32 = AREA_IN + DIGITS;
const AREA_OUT: u32 = AREA_Q + 32;
/// The digit count and decimal exponent of the lower and upper midpoints.
const AREA_META: u32 = AREA_OUT + 48;
const AREA_SIZE: u32 = AREA_META + 16;

/// Reserves the block and remembers its address on the builder.
pub(in crate::emit::runtime) fn prepare(b: &mut Builder) {
    b.float_area = b.alloc_static(AREA_SIZE);
}

// ---- small emitters ----

const U32_SIZE: u32 = 4;
const I64_MASK: i64 = 0x000f_ffff_ffff_ffff;

fn area(b: &Builder, offset: u32) -> i32 {
    (b.float_area + offset) as i32
}

/// Pushes `base + local[index] * width`.
fn elem(ins: &mut InstructionSink, base: i32, index: u32, width: u32) {
    ins.i32_const(base);
    ins.local_get(index);
    if width != 1 {
        ins.i32_const(width as i32);
        ins.i32_mul();
    }
    ins.i32_add();
}

fn load_u8(ins: &mut InstructionSink, base: i32, index: u32) {
    elem(ins, base, index, 1);
    ins.i32_load8_u(mem(0, 1));
}

fn store_u8(ins: &mut InstructionSink, base: i32, index: u32, value: u8) {
    elem(ins, base, index, 1);
    ins.i32_const(value as i32);
    ins.i32_store8(mem(0, 1));
}

/// Pushes the byte at source index `i` of the string in param local `str`.
fn src_byte(ins: &mut InstructionSink, str: u32, i: u32) {
    ins.local_get(str);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

/// Pushes the byte at `str[i + k]`.
fn src_byte_at(ins: &mut InstructionSink, str: u32, i: u32, k: u32) {
    ins.local_get(str);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(i);
    if k != 0 {
        ins.i32_const(k as i32);
        ins.i32_add();
    }
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

fn bump(ins: &mut InstructionSink, index: u32) {
    ins.local_get(index);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(index);
}

// ---- Float -> Int ----

/// The range check and the conversion (`[target.number]`): NaN and anything outside Int is a
/// defect.
fn to_int(b: &mut Builder, ins: &mut InstructionSink) {
    const SHOWN: u32 = 2;
    const HEAD: u32 = 3;
    const MSG: u32 = 4;
    ins.local_get(1);
    ins.f64_const(f64::from(i32::MIN).into());
    ins.f64_ge();
    ins.local_get(1);
    ins.f64_const(f64::from(i32::MAX).into());
    ins.f64_le();
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let prefix = b.string_literal(b"cannot convert ");
    let suffix = b.string_literal(b" to Int");
    ins.local_get(0);
    ins.call(rt(b, "float_to_string"));
    ins.local_set(SHOWN);
    ins.i32_const(prefix as i32);
    ins.local_get(SHOWN);
    ins.call(rt(b, "str_append"));
    ins.local_set(HEAD);
    ins.local_get(HEAD);
    ins.i32_const(suffix as i32);
    ins.call(rt(b, "str_append"));
    ins.local_set(MSG);
    // `raise` and `str_append` borrow their arguments.
    ins.local_get(MSG);
    ins.call(rt(b, "raise"));
    for l in [SHOWN, HEAD, MSG] {
        ins.local_get(l);
        ins.call(rt(b, "drop"));
    }
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(1);
    ins.i32_trunc_f64_s();
}

fn round_to_int(b: &mut Builder, ins: &mut InstructionSink, how: &str) {
    ins.local_get(0);
    ins.local_get(0);
    match how {
        "floor" => ins.f64_floor(),
        "ceil" => ins.f64_ceil(),
        _ => ins.f64_trunc(),
    };
    ins.call(rt(b, "float_to_int"));
}

/// Ties away from zero (`[target.number]`), unlike `f64.nearest` (ties to even).
fn round_half_away(b: &mut Builder, ins: &mut InstructionSink) {
    const A: u32 = 1;
    const F: u32 = 2;
    // `a` is the magnitude.
    ins.local_get(0);
    ins.f64_abs();
    ins.local_set(A);
    ins.local_get(A);
    ins.f64_floor();
    ins.local_set(F);
    ins.local_get(A);
    ins.local_get(F);
    ins.f64_sub();
    ins.f64_const(0.5.into());
    ins.f64_ge();
    ins.if_(BlockType::Result(ValType::F64));
    ins.local_get(F);
    ins.f64_const(1.0.into());
    ins.f64_add();
    ins.else_();
    ins.local_get(F);
    ins.end();
    ins.local_set(F);
    // Back to the input's sign (this also keeps `-0.0`).
    ins.local_get(0);
    ins.f64_const(0.0.into());
    ins.f64_lt();
    ins.if_(BlockType::Empty);
    ins.local_get(F);
    ins.f64_neg();
    ins.local_set(F);
    ins.end();
    ins.local_get(0);
    ins.local_get(F);
    ins.call(rt(b, "float_to_int"));
}

/// Turns an equality with the kept prefix into "greater" when a nonzero digit was dropped.
fn apply_sticky(ins: &mut InstructionSink, cmp: u32, trunc: u32) {
    ins.local_get(cmp);
    ins.i32_eqz();
    ins.local_get(trunc);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(cmp);
    ins.end();
}
