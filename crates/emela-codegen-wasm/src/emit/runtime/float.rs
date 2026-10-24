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

use emela_core::Builtin;

pub(super) const FUNCS: &[Sig] = &[
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

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        "limbs_mul" => vec![(2, ValType::I64), (1, ValType::I32)],
        "dec_build" => vec![(2, ValType::I64), (8, ValType::I32)],
        "dec_cmp" => i32s(6),
        "float_pow10" => vec![(1, ValType::I32), (2, ValType::F64)],
        "float_bounds" => vec![(3, ValType::I64), (7, ValType::I32)],
        "float_some" => i32s(1),
        "float_round" => vec![(2, ValType::F64)],
        "float_to_string" => vec![(3, ValType::I64), (25, ValType::I32)],
        "float_parse" => vec![(6, ValType::I64), (19, ValType::I32), (1, ValType::F64)],
        _ => Vec::new(),
    }
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
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

/// Whether the program reaches a Float builtin that this module implements.
pub(super) fn used(b: &Builder) -> bool {
    b.m.functions.iter().any(|(_, f)| {
        f.stmts.iter().any(|(_, s)| {
            matches!(
                s,
                Stmt::Let { value: Value::Builtin { op, .. }, .. } if implemented(*op)
            )
        })
    })
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
pub(super) fn prepare(b: &mut Builder) {
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

// ---- base 10^9 limbs ----

/// Multiplies the big integer by `mult` and returns its new limb count.
fn limbs_mul(b: &mut Builder, ins: &mut InstructionSink) {
    const V: u32 = 2;
    const CARRY: u32 = 3;
    const I: u32 = 4;
    let base = area(b, AREA_LIMBS);
    ins.i64_const(0);
    ins.local_set(CARRY);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(1);
    ins.i32_ge_u();
    ins.br_if(1);
    elem(ins, base, I, U32_SIZE);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.local_get(0);
    ins.i64_mul();
    ins.local_get(CARRY);
    ins.i64_add();
    ins.local_set(V);
    elem(ins, base, I, U32_SIZE);
    ins.local_get(V);
    ins.i64_const(1_000_000_000);
    ins.i64_rem_u();
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(V);
    ins.i64_const(1_000_000_000);
    ins.i64_div_u();
    ins.local_set(CARRY);
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CARRY);
    ins.i64_eqz();
    ins.br_if(1);
    elem(ins, base, 1, U32_SIZE);
    ins.local_get(CARRY);
    ins.i64_const(1_000_000_000);
    ins.i64_rem_u();
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(CARRY);
    ins.i64_const(1_000_000_000);
    ins.i64_div_u();
    ins.local_set(CARRY);
    bump(ins, 1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(1);
}

/// `dec_build(f, e, dest)`: the exact decimal digits of `f * 2^e`, most significant first, into
/// `dest`, and their count. `f` is the binary mantissa of a finite, non-zero binary64.
fn dec_build(b: &mut Builder, ins: &mut InstructionSink) {
    const V: u32 = 3;
    const I: u32 = 5;
    const N: u32 = 6;
    const T: u32 = 7;
    const DTOP: u32 = 8;
    const TOTAL: u32 = 9;
    const POS: u32 = 10;
    const LIMB: u32 = 11;
    const ND: u32 = 12;
    let base = area(b, AREA_LIMBS);
    // Clear the limbs.
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(LIMBS as i32);
    ins.i32_ge_u();
    ins.br_if(1);
    elem(ins, base, I, U32_SIZE);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    // The mantissa, least significant limb first.
    ins.local_get(0);
    ins.local_set(V);
    ins.i32_const(0);
    ins.local_set(N);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    elem(ins, base, N, U32_SIZE);
    ins.local_get(V);
    ins.i64_const(1_000_000_000);
    ins.i64_rem_u();
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(V);
    ins.i64_const(1_000_000_000);
    ins.i64_div_u();
    ins.local_set(V);
    bump(ins, N);
    ins.local_get(V);
    ins.i64_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    // `e > 0`: multiply by 2^15 chunks, then by 2. `e < 0`: by 5^13 chunks, then by 5.
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.local_set(T);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.i32_const(15);
    ins.i32_lt_s();
    ins.br_if(1);
    ins.i64_const(32768);
    ins.local_get(N);
    ins.call(rt(b, "limbs_mul"));
    ins.local_set(N);
    ins.local_get(T);
    ins.i32_const(15);
    ins.i32_sub();
    ins.local_set(T);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.i32_eqz();
    ins.br_if(1);
    ins.i64_const(2);
    ins.local_get(N);
    ins.call(rt(b, "limbs_mul"));
    ins.local_set(N);
    ins.local_get(T);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(T);
    ins.br(0);
    ins.end();
    ins.end();
    ins.else_();
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i32_sub();
    ins.local_set(T);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.i32_const(13);
    ins.i32_lt_s();
    ins.br_if(1);
    ins.i64_const(1_220_703_125);
    ins.local_get(N);
    ins.call(rt(b, "limbs_mul"));
    ins.local_set(N);
    ins.local_get(T);
    ins.i32_const(13);
    ins.i32_sub();
    ins.local_set(T);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.i32_eqz();
    ins.br_if(1);
    ins.i64_const(5);
    ins.local_get(N);
    ins.call(rt(b, "limbs_mul"));
    ins.local_set(N);
    ins.local_get(T);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(T);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // The decimal length of the top limb.
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    elem(ins, base, I, U32_SIZE);
    ins.i32_load(mem(0, 4));
    ins.local_set(T);
    ins.i32_const(1);
    ins.local_set(DTOP);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.i32_const(10);
    ins.i32_lt_u();
    ins.br_if(1);
    ins.local_get(T);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.local_set(T);
    bump(ins, DTOP);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(9);
    ins.i32_mul();
    ins.local_get(DTOP);
    ins.i32_add();
    ins.local_set(TOTAL);
    // Write the digits backwards from `dest + total`.
    ins.local_get(2);
    ins.local_get(TOTAL);
    ins.i32_add();
    ins.local_set(POS);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    elem(ins, base, I, U32_SIZE);
    ins.i32_load(mem(0, 4));
    ins.local_set(LIMB);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_eq();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(DTOP);
    ins.else_();
    ins.i32_const(9);
    ins.end();
    ins.local_set(ND);
    ins.i32_const(0);
    ins.local_set(T);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(T);
    ins.local_get(ND);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(POS);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(POS);
    ins.local_get(POS);
    ins.local_get(LIMB);
    ins.i32_const(10);
    ins.i32_rem_u();
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.local_get(LIMB);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.local_set(LIMB);
    bump(ins, T);
    ins.br(0);
    ins.end();
    ins.end();
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(TOTAL);
}

/// `dec_cmp(adigits, alen, aexp, bdigits, blen, bexp)`: compares `a * 10^aexp` with
/// `b * 10^bexp` as exact decimals, returning -1, 0 or 1.
fn dec_cmp(ins: &mut InstructionSink) {
    const I: u32 = 6;
    const N: u32 = 7;
    const NA: u32 = 8;
    const NB: u32 = 9;
    const DA: u32 = 10;
    const DB: u32 = 11;
    // The number of digits before the decimal point fixes the magnitude.
    ins.local_get(2);
    ins.local_get(1);
    ins.i32_add();
    ins.local_set(NA);
    ins.local_get(5);
    ins.local_get(4);
    ins.i32_add();
    ins.local_set(NB);
    ins.local_get(NA);
    ins.local_get(NB);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(NA);
    ins.local_get(NB);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(-1);
    ins.else_();
    ins.i32_const(1);
    ins.end();
    ins.return_();
    ins.end();
    // Same magnitude: compare the digits, padding the shorter with zeros.
    ins.local_get(1);
    ins.local_get(4);
    ins.local_get(1);
    ins.local_get(4);
    ins.i32_gt_s();
    ins.select();
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.i32_const(0);
    ins.local_set(DA);
    ins.local_get(I);
    ins.local_get(1);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    // `a[i]` (the base is param 0).
    ins.local_get(0);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.local_set(DA);
    ins.end();
    ins.i32_const(0);
    ins.local_set(DB);
    ins.local_get(I);
    ins.local_get(4);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    // `b[i]` (the base is param 3).
    ins.local_get(3);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.local_set(DB);
    ins.end();
    ins.local_get(DA);
    ins.local_get(DB);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(DA);
    ins.local_get(DB);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(-1);
    ins.else_();
    ins.i32_const(1);
    ins.end();
    ins.return_();
    ins.end();
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// `float_pow10(e)`: 10^e by squaring. Overflows to `Infinity` for a large `e`, which is what the
/// scaling paths want.
fn pow10(ins: &mut InstructionSink) {
    const E: u32 = 1;
    const RESULT: u32 = 2;
    const BASE: u32 = 3;
    ins.local_get(0);
    ins.local_set(E);
    ins.f64_const(1.0.into());
    ins.local_set(RESULT);
    ins.f64_const(10.0.into());
    ins.local_set(BASE);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(E);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(E);
    ins.i32_const(1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(RESULT);
    ins.local_get(BASE);
    ins.f64_mul();
    ins.local_set(RESULT);
    ins.end();
    ins.local_get(E);
    ins.i32_const(1);
    ins.i32_shr_u();
    ins.local_set(E);
    ins.local_get(E);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.br(1);
    ins.end();
    ins.local_get(BASE);
    ins.local_get(BASE);
    ins.f64_mul();
    ins.local_set(BASE);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(RESULT);
}

/// `float_bounds(y)`: writes the exact decimal digits and exponents of the midpoints to the
/// neighbours of the positive finite `y` into the meta area, and returns the low bit of its
/// mantissa (the round-to-even tie rule).
fn bounds(b: &mut Builder, ins: &mut InstructionSink) {
    const BITS: u32 = 1;
    const F: u32 = 2;
    const M: u32 = 3;
    const EBITS: u32 = 4;
    const E: u32 = 5;
    const DLO: u32 = 6;
    const SCALE: u32 = 7;
    const DHI: u32 = 8;
    const POW: u32 = 9;
    const SHIFT: u32 = 10;
    let meta = area(b, AREA_META);
    ins.local_get(0);
    ins.i64_reinterpret_f64();
    ins.local_set(BITS);
    ins.local_get(BITS);
    ins.i64_const(52);
    ins.i64_shr_u();
    ins.i64_const(0x7ff);
    ins.i64_and();
    ins.i32_wrap_i64();
    ins.local_set(EBITS);
    ins.local_get(EBITS);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I64));
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.else_();
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.i64_const(1i64 << 52);
    ins.i64_or();
    ins.end();
    ins.local_set(F);
    ins.local_get(EBITS);
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(-1074);
    ins.else_();
    ins.local_get(EBITS);
    ins.i32_const(1075);
    ins.i32_sub();
    ins.end();
    ins.local_set(E);
    // A normal power of two has half the spacing below it, so its lower midpoint is
    // `(4f - 1) * 2^(e - 2)`; every other value uses `(2f - 1) * 2^(e - 1)`.
    ins.local_get(EBITS);
    ins.i32_const(2);
    ins.i32_ge_s();
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.i64_eqz();
    ins.i32_and();
    ins.local_set(POW);
    // `shift` is 2 for a power of two, 1 otherwise.
    ins.i32_const(2);
    ins.i32_const(1);
    ins.local_get(POW);
    ins.select();
    ins.local_set(SHIFT);
    // Lower midpoint: `((f << shift) - 1) * 2^(e - shift)`.
    ins.local_get(F);
    ins.local_get(SHIFT);
    ins.i64_extend_i32_u();
    ins.i64_shl();
    ins.i64_const(1);
    ins.i64_sub();
    ins.local_set(M);
    ins.local_get(M);
    ins.local_get(E);
    ins.local_get(SHIFT);
    ins.i32_sub();
    ins.i32_const(area(b, AREA_LO));
    ins.call(rt(b, "dec_build"));
    ins.local_set(DLO);
    // The decimal scale of the lower bound: `max(0, shift - e)`.
    ins.local_get(SHIFT);
    ins.local_get(E);
    ins.i32_sub();
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(SHIFT);
    ins.local_get(E);
    ins.i32_sub();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.local_set(SCALE);
    ins.i32_const(meta);
    ins.local_get(DLO);
    ins.i32_store(mem(0, 4));
    ins.i32_const(meta + 4);
    ins.i32_const(0);
    ins.local_get(SCALE);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    // Upper midpoint: `(2f + 1) * 2^(e - 1)`. The spacing above the next binade does not move
    // this midpoint: it is still halfway to the next representable value.
    ins.local_get(F);
    ins.i64_const(2);
    ins.i64_mul();
    ins.i64_const(1);
    ins.i64_add();
    ins.local_set(M);
    ins.local_get(M);
    ins.local_get(E);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(area(b, AREA_HI));
    ins.call(rt(b, "dec_build"));
    ins.local_set(DHI);
    // The decimal scale of the upper bound: `max(0, 1 - e)`.
    ins.local_get(E);
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(1);
    ins.local_get(E);
    ins.i32_sub();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.local_set(SCALE);
    ins.i32_const(meta + 8);
    ins.local_get(DHI);
    ins.i32_store(mem(0, 4));
    ins.i32_const(meta + 12);
    ins.i32_const(0);
    ins.local_get(SCALE);
    ins.i32_sub();
    ins.i32_store(mem(0, 4));
    ins.local_get(F);
    ins.i64_const(1);
    ins.i64_and();
    ins.i32_wrap_i64();
}

/// `float_some(y, negative, some)`: applies the sign and builds `Some(y)`.
fn some(b: &mut Builder, ins: &mut InstructionSink) {
    const P: u32 = 3;
    ins.local_get(1);
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.f64_neg();
    ins.local_set(0);
    ins.end();
    ins.i32_const(1);
    ins.local_get(2);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    ins.local_get(P);
    ins.local_get(0);
    ins.f64_store(mem(HDR, 8));
    ins.local_get(P);
}

// ---- Float -> Int ----

/// The range check and the conversion (`[target.number]`): NaN and anything outside Int is a
/// defect.
fn to_int(b: &mut Builder, ins: &mut InstructionSink) {
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
    ins.i32_const(prefix as i32);
    ins.local_get(0);
    ins.call(rt(b, "float_to_string"));
    ins.call(rt(b, "str_append"));
    ins.i32_const(suffix as i32);
    ins.call(rt(b, "str_append"));
    ins.call(rt(b, "raise"));
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

// ---- to_string ----

/// Copies `count` digits from `src` (starting at local `i`) to `dst` (starting at local `j`).
fn copy_digits(ins: &mut InstructionSink, src: i32, dst: i32, i: u32, j: u32, count: u32) {
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(i);
    ins.local_get(count);
    ins.i32_ge_u();
    ins.br_if(1);
    elem(ins, dst, j, 1);
    load_u8(ins, src, i);
    ins.i32_store8(mem(0, 1));
    bump(ins, i);
    bump(ins, j);
    ins.br(0);
    ins.end();
    ins.end();
}

/// Writes `count` zeros at `dst` from local `j`.
fn zeros(ins: &mut InstructionSink, dst: i32, j: u32, count: u32) {
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(count);
    ins.i32_eqz();
    ins.br_if(1);
    elem(ins, dst, j, 1);
    ins.i32_const(b'0' as i32);
    ins.i32_store8(mem(0, 1));
    bump(ins, j);
    ins.local_get(count);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(count);
    ins.br(0);
    ins.end();
    ins.end();
}

fn put_out(ins: &mut InstructionSink, dst: i32, j: u32, byte: u8) {
    elem(ins, dst, j, 1);
    ins.i32_const(byte as i32);
    ins.i32_store8(mem(0, 1));
    bump(ins, j);
}

/// Stores the byte in local `value` at `dst[j]` and advances `j`.
fn put_digit(ins: &mut InstructionSink, dst: i32, j: u32, value: u32) {
    elem(ins, dst, j, 1);
    ins.local_get(value);
    ins.i32_store8(mem(0, 1));
    bump(ins, j);
}

/// Adds one to the `len`-digit decimal at `base`, adjusting `len` and `exp` on a carry out.
fn carry_one(ins: &mut InstructionSink, base: i32, len: u32, exp: u32, dexp: u32, i: u32) {
    ins.local_get(len);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(i);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    load_u8(ins, base, i);
    ins.i32_const(b'9' as i32);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    elem(ins, base, i, 1);
    load_u8(ins, base, i);
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.br(2);
    ins.end();
    store_u8(ins, base, i, b'0');
    ins.local_get(i);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(len);
    ins.i32_const(0);
    ins.local_set(i);
    store_u8(ins, base, i, b'1');
    ins.local_get(dexp);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(exp);
    ins.br(2);
    ins.end();
    ins.local_get(i);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(i);
    ins.br(0);
    ins.end();
    ins.end();
}

/// A p-digit candidate: `base[0..len] * 10^exp`, with the local that receives the interval test.
#[derive(Clone, Copy)]
struct Candidate {
    base: i32,
    len: u32,
    exp: u32,
    out: u32,
}

/// Whether `q` is in the rounding interval in the meta area. `cmp` and `tmp` are scratch locals.
fn inside(b: &Builder, ins: &mut InstructionSink, q: Candidate, parity: u32, cmp: u32, tmp: u32) {
    let meta = area(b, AREA_META);
    ins.i32_const(q.base);
    ins.local_get(q.len);
    ins.local_get(q.exp);
    ins.i32_const(area(b, AREA_LO));
    ins.i32_const(meta);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 4);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(cmp);
    ins.i32_const(0);
    ins.local_set(q.out);
    ins.local_get(cmp);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(q.base);
    ins.local_get(q.len);
    ins.local_get(q.exp);
    ins.i32_const(area(b, AREA_HI));
    ins.i32_const(meta + 8);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 12);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(tmp);
    ins.local_get(tmp);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.local_get(tmp);
    ins.i32_eqz();
    ins.local_get(parity);
    ins.i32_eqz();
    ins.i32_and();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(q.out);
    ins.end();
    ins.else_();
    ins.local_get(cmp);
    ins.i32_eqz();
    ins.local_get(parity);
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(q.out);
    ins.end();
    ins.end();
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

fn to_string(b: &mut Builder, ins: &mut InstructionSink) {
    const BITS: u32 = 1;
    const F: u32 = 2;
    const EBITS: u32 = 4;
    const E: u32 = 5;
    const SCALE: u32 = 6;
    const D: u32 = 7;
    const DEXP: u32 = 8;
    const P: u32 = 12;
    const I: u32 = 13;
    const QLEN: u32 = 14;
    const QEXP: u32 = 15;
    const CMP: u32 = 16;
    const N: u32 = 17;
    const J: u32 = 18;
    const K: u32 = 19;
    const DIG: u32 = 20;
    const SIGN: u32 = 21;
    const UP: u32 = 23;
    const PARITY: u32 = 24;
    const CLEN: u32 = 25;
    const CEXP: u32 = 26;
    const INSF: u32 = 27;
    const INSC: u32 = 28;
    let out = area(b, AREA_OUT);
    let dq = area(b, AREA_Q);
    // The second candidate (`dq + 1`) uses the output buffer until the format step starts.
    let dqc = out;
    let dx = area(b, AREA_X);
    let nan = b.string_literal(b"NaN");
    let inf = b.string_literal(b"Infinity");
    let ninf = b.string_literal(b"-Infinity");
    let zero = b.string_literal(b"0.0");
    // Decode.
    ins.local_get(0);
    ins.i64_reinterpret_f64();
    ins.local_set(BITS);
    ins.local_get(BITS);
    ins.i64_const(0);
    ins.i64_lt_s();
    ins.local_set(SIGN);
    ins.local_get(BITS);
    ins.i64_const(52);
    ins.i64_shr_u();
    ins.i64_const(0x7ff);
    ins.i64_and();
    ins.i32_wrap_i64();
    ins.local_set(EBITS);
    ins.local_get(EBITS);
    ins.i32_const(0x7ff);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.i64_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(SIGN);
    ins.if_(BlockType::Empty);
    ins.i32_const(ninf as i32);
    ins.return_();
    ins.else_();
    ins.i32_const(inf as i32);
    ins.return_();
    ins.end();
    ins.else_();
    ins.i32_const(nan as i32);
    ins.return_();
    ins.end();
    ins.end();
    ins.local_get(EBITS);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.i64_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(zero as i32);
    ins.return_();
    ins.end();
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.local_set(F);
    ins.i32_const(-1074);
    ins.local_set(E);
    ins.else_();
    ins.local_get(BITS);
    ins.i64_const(I64_MASK);
    ins.i64_and();
    ins.i64_const(1i64 << 52);
    ins.i64_or();
    ins.local_set(F);
    ins.local_get(EBITS);
    ins.i32_const(1075);
    ins.i32_sub();
    ins.local_set(E);
    ins.end();
    // The exact digits of the value.
    ins.local_get(F);
    ins.local_get(E);
    ins.i32_const(dx);
    ins.call(rt(b, "dec_build"));
    ins.local_set(D);
    // `scale = max(0, -e)`; `dexp = digits - 1 - scale`.
    ins.local_get(E);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.local_get(E);
    ins.i32_sub();
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.local_set(SCALE);
    ins.local_get(D);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_get(SCALE);
    ins.i32_sub();
    ins.local_set(DEXP);
    // The rounding interval.
    ins.local_get(0);
    ins.call(rt(b, "float_bounds"));
    ins.local_set(PARITY);
    // The shortest prefix of the digits that stays in the interval. At each length try the
    // nearest p-digit decimal first, then the other side when the interval is asymmetric.
    ins.i32_const(1);
    ins.local_set(P);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(P);
    ins.i32_const(17);
    ins.i32_gt_s();
    ins.br_if(1);
    // The floor candidate: the first `p` digits.
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(J);
    copy_digits(ins, dx, dq, I, J, P);
    ins.local_get(P);
    ins.local_set(QLEN);
    ins.local_get(DEXP);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(P);
    ins.i32_sub();
    ins.local_set(QEXP);
    // The ceiling candidate: the same digits plus one.
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(J);
    copy_digits(ins, dx, dqc, I, J, P);
    ins.local_get(P);
    ins.local_set(CLEN);
    ins.local_get(QEXP);
    ins.local_set(CEXP);
    carry_one(ins, dqc, CLEN, CEXP, DEXP, I);
    // `UP` when the nearest p-digit decimal is the ceiling; ties go to the even last digit.
    ins.i32_const(0);
    ins.local_set(UP);
    ins.local_get(P);
    ins.local_get(D);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    load_u8(ins, dx, P);
    ins.local_set(DIG);
    ins.local_get(DIG);
    ins.i32_const(b'5' as i32);
    ins.i32_gt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(UP);
    ins.else_();
    ins.local_get(DIG);
    ins.i32_const(b'5' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    // Any non-zero digit after `p` rounds up; an exact tie rounds to the even last digit.
    ins.i32_const(0);
    ins.local_set(K);
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(D);
    ins.i32_ge_u();
    ins.br_if(1);
    load_u8(ins, dx, I);
    ins.i32_const(b'0' as i32);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(K);
    ins.end();
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(K);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(UP);
    ins.else_();
    ins.local_get(P);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    load_u8(ins, dq, I);
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.i32_const(1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(UP);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // Which candidate is inside? The nearest one wins when both are.
    inside(
        b,
        ins,
        Candidate {
            base: dq,
            len: QLEN,
            exp: QEXP,
            out: INSF,
        },
        PARITY,
        CMP,
        DIG,
    );
    inside(
        b,
        ins,
        Candidate {
            base: dqc,
            len: CLEN,
            exp: CEXP,
            out: INSC,
        },
        PARITY,
        CMP,
        DIG,
    );
    ins.i32_const(0);
    ins.local_set(K);
    ins.local_get(UP);
    ins.if_(BlockType::Empty);
    // The nearest is the ceiling.
    ins.local_get(INSC);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(J);
    copy_digits(ins, dqc, dq, I, J, CLEN);
    ins.local_get(CLEN);
    ins.local_set(QLEN);
    ins.local_get(CEXP);
    ins.local_set(QEXP);
    ins.i32_const(1);
    ins.local_set(K);
    ins.else_();
    ins.local_get(INSF);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(K);
    ins.end();
    ins.end();
    ins.else_();
    // The nearest is the floor.
    ins.local_get(INSF);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(K);
    ins.else_();
    ins.local_get(INSC);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(J);
    copy_digits(ins, dqc, dq, I, J, CLEN);
    ins.local_get(CLEN);
    ins.local_set(QLEN);
    ins.local_get(CEXP);
    ins.local_set(QEXP);
    ins.i32_const(1);
    ins.local_set(K);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(K);
    ins.if_(BlockType::Empty);
    ins.br(2);
    ins.end();
    bump(ins, P);
    ins.br(0);
    ins.end();
    ins.end();
    // ---- format the shortest digits ----
    ins.i32_const(0);
    ins.local_set(J);
    ins.local_get(SIGN);
    ins.if_(BlockType::Empty);
    put_out(ins, out, J, b'-');
    ins.end();
    ins.local_get(QLEN);
    ins.local_get(QEXP);
    ins.i32_add();
    ins.local_set(N);
    // `qlen <= n <= 21`: digits, zeros, ".0".
    ins.local_get(QLEN);
    ins.local_get(N);
    ins.i32_le_s();
    ins.local_get(N);
    ins.i32_const(21);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(I);
    copy_digits(ins, dq, out, I, J, QLEN);
    ins.local_get(N);
    ins.local_get(QLEN);
    ins.i32_sub();
    ins.local_set(K);
    zeros(ins, out, J, K);
    put_out(ins, out, J, b'.');
    put_out(ins, out, J, b'0');
    ins.else_();
    // `0 < n <= 21`: digits around the decimal point.
    ins.local_get(N);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.local_get(N);
    ins.i32_const(21);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(I);
    ins.local_get(N);
    ins.local_set(K);
    copy_digits(ins, dq, out, I, J, K);
    put_out(ins, out, J, b'.');
    copy_digits(ins, dq, out, I, J, QLEN);
    ins.else_();
    // `-6 < n <= 0`: "0." then zeros then the digits.
    ins.local_get(N);
    ins.i32_const(-6);
    ins.i32_gt_s();
    ins.local_get(N);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    put_out(ins, out, J, b'0');
    put_out(ins, out, J, b'.');
    ins.i32_const(0);
    ins.local_get(N);
    ins.i32_sub();
    ins.local_set(K);
    zeros(ins, out, J, K);
    ins.i32_const(0);
    ins.local_set(I);
    copy_digits(ins, dq, out, I, J, QLEN);
    ins.else_();
    // Exponential: `d[.ddd]e±xx`.
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(1);
    ins.local_set(K);
    copy_digits(ins, dq, out, I, J, K);
    ins.local_get(QLEN);
    ins.i32_const(1);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    put_out(ins, out, J, b'.');
    ins.i32_const(1);
    ins.local_set(I);
    copy_digits(ins, dq, out, I, J, QLEN);
    ins.end();
    put_out(ins, out, J, b'e');
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(DIG);
    ins.local_get(DIG);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    put_out(ins, out, J, b'-');
    ins.i32_const(0);
    ins.local_get(DIG);
    ins.i32_sub();
    ins.local_set(DIG);
    ins.else_();
    put_out(ins, out, J, b'+');
    ins.end();
    // At most three digits (the largest exponent is 324). The hundreds need both of the
    // remaining digits, including a zero tens.
    ins.local_get(DIG);
    ins.i32_const(100);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(DIG);
    ins.i32_const(100);
    ins.i32_div_u();
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.local_set(K);
    put_digit(ins, out, J, K);
    ins.local_get(DIG);
    ins.i32_const(100);
    ins.i32_rem_u();
    ins.local_set(DIG);
    ins.local_get(DIG);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.local_set(K);
    put_digit(ins, out, J, K);
    ins.else_();
    ins.local_get(DIG);
    ins.i32_const(10);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(DIG);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.local_set(K);
    put_digit(ins, out, J, K);
    ins.end();
    ins.end();
    ins.local_get(DIG);
    ins.i32_const(10);
    ins.i32_rem_u();
    ins.i32_const(b'0' as i32);
    ins.i32_add();
    ins.local_set(K);
    put_digit(ins, out, J, K);
    ins.end();
    ins.end();
    ins.end();
    // ---- the string ----
    ins.local_get(J);
    ins.call(rt(b, "str_new"));
    ins.local_set(UP);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(J);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(UP);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(I);
    ins.i32_add();
    load_u8(ins, out, I);
    ins.i32_store8(mem(0, 1));
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(UP);
}

// ---- parse ----

fn parse(b: &mut Builder, ins: &mut InstructionSink) {
    const EXPE: u32 = 3;
    const K: u32 = 4;
    const MANT: u32 = 5;
    const V: u32 = 6;
    const YBITS: u32 = 7;
    const N: u32 = 9;
    const I: u32 = 10;
    const NEG: u32 = 11;
    const SIG: u32 = 12;
    const FRAC: u32 = 13;
    const KEPT: u32 = 14;
    const CMP: u32 = 16;
    const PARITY: u32 = 17;
    const CNT: u32 = 18;
    const BYTE: u32 = 19;
    const DIG: u32 = 20;
    const M19: u32 = 21;
    const L: u32 = 22;
    const ESIGN: u32 = 23;
    const FLAG: u32 = 24;
    const J: u32 = 25;
    const IDIG: u32 = 26;
    const TRUNC: u32 = 27;
    const Y: u32 = 28;
    let din = area(b, AREA_IN);
    let meta = area(b, AREA_META);
    // The length, and `NaN` (which takes no sign).
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.local_get(N);
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(FLAG);
    for (k, byte) in b"NaN".iter().enumerate() {
        ins.local_get(FLAG);
        src_byte_at(ins, 0, I, k as u32);
        ins.i32_const(*byte as i32);
        ins.i32_eq();
        ins.i32_and();
        ins.local_set(FLAG);
    }
    ins.local_get(FLAG);
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::NAN.into());
    finish(b, ins, None);
    ins.end();
    ins.end();
    ins.local_get(N);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(NEG);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'+' as i32);
    ins.i32_eq();
    ins.local_get(BYTE);
    ins.i32_const(b'-' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(BYTE);
    ins.i32_const(b'-' as i32);
    ins.i32_eq();
    ins.local_set(NEG);
    ins.i32_const(1);
    ins.local_set(I);
    ins.end();
    // `Infinity` after the sign.
    ins.local_get(N);
    ins.local_get(I);
    ins.i32_sub();
    ins.i32_const(8);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(FLAG);
    for (k, byte) in b"Infinity".iter().enumerate() {
        ins.local_get(FLAG);
        src_byte_at(ins, 0, I, k as u32);
        ins.i32_const(*byte as i32);
        ins.i32_eq();
        ins.i32_and();
        ins.local_set(FLAG);
    }
    ins.local_get(FLAG);
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::INFINITY.into());
    finish(b, ins, Some(NEG));
    ins.end();
    ins.end();
    // Integer digits.
    //
    // Only the first 800 significant digits are kept. A rounding midpoint is a dyadic rational
    // whose exact decimal has at most 768 significant digits, so a dropped digit can only matter
    // when the kept prefix already equals the midpoint. `TRUNC` remembers that a nonzero digit was
    // dropped, and the comparisons treat the input as strictly greater than the kept prefix.
    ins.i32_const(0);
    ins.local_set(SIG);
    ins.i32_const(0);
    ins.local_set(FRAC);
    ins.i32_const(0);
    ins.local_set(KEPT);
    ins.i32_const(0);
    ins.local_set(IDIG);
    ins.i32_const(0);
    ins.local_set(TRUNC);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_lt_u();
    ins.local_get(BYTE);
    ins.i32_const(b'9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.br_if(1);
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.local_set(DIG);
    // Leading zeros are not part of the significant digits.
    ins.local_get(SIG);
    ins.i32_eqz();
    ins.local_get(DIG);
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.local_get(KEPT);
    ins.i32_const(800);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    elem(ins, din, KEPT, 1);
    ins.local_get(BYTE);
    ins.i32_store8(mem(0, 1));
    bump(ins, KEPT);
    ins.else_();
    ins.local_get(DIG);
    ins.i32_const(0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(TRUNC);
    ins.end();
    ins.end();
    bump(ins, SIG);
    ins.end();
    bump(ins, IDIG);
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(IDIG);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    // The fraction (the dot needs at least one digit after it).
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    src_byte(ins, 0, I);
    ins.i32_const(b'.' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    bump(ins, I);
    ins.i32_const(0);
    ins.local_set(FLAG);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_lt_u();
    ins.local_get(BYTE);
    ins.i32_const(b'9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.br_if(1);
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.local_set(DIG);
    ins.local_get(SIG);
    ins.i32_eqz();
    ins.local_get(DIG);
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.else_();
    ins.local_get(KEPT);
    ins.i32_const(800);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    elem(ins, din, KEPT, 1);
    ins.local_get(BYTE);
    ins.i32_store8(mem(0, 1));
    bump(ins, KEPT);
    ins.else_();
    ins.local_get(DIG);
    ins.i32_const(0);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(TRUNC);
    ins.end();
    ins.end();
    bump(ins, SIG);
    ins.end();
    bump(ins, FRAC);
    bump(ins, FLAG);
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(FLAG);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.end();
    ins.end();
    // The exponent.
    ins.i64_const(0);
    ins.local_set(EXPE);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'e' as i32);
    ins.i32_eq();
    ins.local_get(BYTE);
    ins.i32_const(b'E' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    bump(ins, I);
    ins.i32_const(1);
    ins.local_set(ESIGN);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'+' as i32);
    ins.i32_eq();
    ins.local_get(BYTE);
    ins.i32_const(b'-' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(BYTE);
    ins.i32_const(b'-' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1);
    ins.local_set(ESIGN);
    ins.end();
    bump(ins, I);
    ins.end();
    ins.end();
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.i64_const(0);
    ins.local_set(V);
    ins.i32_const(0);
    ins.local_set(FLAG);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    src_byte(ins, 0, I);
    ins.local_set(BYTE);
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_lt_u();
    ins.local_get(BYTE);
    ins.i32_const(b'9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.br_if(1);
    // Saturate once the exponent is far beyond any digit count (a string is under 2^32
    // bytes), so `k` below stays exact without overflowing.
    ins.local_get(V);
    ins.i64_const(100_000_000_000_000_000);
    ins.i64_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(V);
    ins.i64_const(10);
    ins.i64_mul();
    ins.local_get(BYTE);
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.i64_extend_i32_u();
    ins.i64_add();
    ins.local_set(V);
    ins.end();
    bump(ins, FLAG);
    bump(ins, I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(FLAG);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    ins.local_get(ESIGN);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.local_get(V);
    ins.i64_sub();
    ins.local_set(V);
    ins.end();
    ins.local_get(V);
    ins.local_set(EXPE);
    ins.end();
    ins.end();
    // The whole string must be consumed.
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(2);
    ins.return_();
    ins.end();
    // Zero (either sign).
    ins.local_get(SIG);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.f64_const(0.0.into());
    finish(b, ins, Some(NEG));
    ins.end();
    // `k` is the power of ten of the least significant kept digit.
    ins.local_get(SIG);
    ins.local_get(KEPT);
    ins.i32_sub();
    ins.i64_extend_i32_s();
    ins.local_get(EXPE);
    ins.i64_add();
    ins.local_get(FRAC);
    ins.i64_extend_i32_s();
    ins.i64_sub();
    ins.local_set(K);
    ins.local_get(KEPT);
    ins.local_set(L);
    // Out of range in either direction.
    ins.local_get(K);
    ins.i64_const(309);
    ins.i64_ge_s();
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::INFINITY.into());
    finish(b, ins, Some(NEG));
    ins.end();
    ins.local_get(K);
    ins.local_get(L);
    ins.i64_extend_i32_s();
    ins.i64_add();
    ins.i64_const(-324);
    ins.i64_le_s();
    ins.if_(BlockType::Empty);
    ins.f64_const(0.0.into());
    finish(b, ins, Some(NEG));
    ins.end();
    // The naive value: the first 19 digits and their power of ten.
    ins.local_get(L);
    ins.i32_const(19);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(L);
    ins.else_();
    ins.i32_const(19);
    ins.end();
    ins.local_set(M19);
    ins.i64_const(0);
    ins.local_set(MANT);
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.local_get(M19);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(MANT);
    ins.i64_const(10);
    ins.i64_mul();
    load_u8(ins, din, J);
    ins.i32_const(b'0' as i32);
    ins.i32_sub();
    ins.i64_extend_i32_u();
    ins.i64_add();
    ins.local_set(MANT);
    bump(ins, J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(MANT);
    ins.f64_convert_i64_u();
    ins.local_set(Y);
    ins.local_get(L);
    ins.local_get(M19);
    ins.i32_sub();
    ins.i64_extend_i32_s();
    ins.local_get(K);
    ins.i64_add();
    ins.local_set(YBITS);
    ins.local_get(YBITS);
    ins.i64_const(0);
    ins.i64_ge_s();
    ins.if_(BlockType::Empty);
    ins.local_get(Y);
    ins.local_get(YBITS);
    ins.i32_wrap_i64();
    ins.call(rt(b, "float_pow10"));
    ins.f64_mul();
    ins.local_set(Y);
    ins.else_();
    // Divide in chunks of at most 10^308, which is the largest power of ten a binary64 holds.
    ins.i64_const(0);
    ins.local_get(YBITS);
    ins.i64_sub();
    ins.i32_wrap_i64();
    ins.local_set(FLAG);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(FLAG);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(FLAG);
    ins.i32_const(308);
    ins.i32_gt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(308);
    ins.else_();
    ins.local_get(FLAG);
    ins.end();
    ins.local_set(J);
    ins.local_get(Y);
    ins.local_get(J);
    ins.call(rt(b, "float_pow10"));
    ins.f64_div();
    ins.local_set(Y);
    ins.local_get(FLAG);
    ins.local_get(J);
    ins.i32_sub();
    ins.local_set(FLAG);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    // Move by single neighbours until the input is inside the rounding interval. The naive value
    // is within a few ulps, so this takes a handful of steps; the guard only stops a bug from
    // spinning forever.
    ins.i32_const(0);
    ins.local_set(CNT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    bump(ins, CNT);
    ins.local_get(CNT);
    ins.i32_const(64);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(Y);
    finish(b, ins, Some(NEG));
    ins.end();
    ins.local_get(Y);
    ins.i64_reinterpret_f64();
    ins.local_set(YBITS);
    // +Infinity: the previous value is the largest finite one.
    ins.local_get(YBITS);
    ins.i64_const(0x7ff0_0000_0000_0000);
    ins.i64_eq();
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::MAX.into());
    ins.call(rt(b, "float_bounds"));
    ins.local_set(PARITY);
    ins.i32_const(din);
    ins.local_get(L);
    ins.local_get(K);
    ins.i32_wrap_i64();
    ins.i32_const(area(b, AREA_HI));
    ins.i32_const(meta + 8);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 12);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(CMP);
    apply_sticky(ins, CMP, TRUNC);
    ins.local_get(CMP);
    ins.i32_const(0);
    ins.i32_ge_s();
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::INFINITY.into());
    finish(b, ins, Some(NEG));
    ins.end();
    ins.f64_const(f64::MAX.into());
    ins.local_set(Y);
    ins.br(0);
    ins.end();
    // Zero: the upper midpoint is the lower one of the smallest subnormal.
    ins.local_get(YBITS);
    ins.i64_eqz();
    ins.if_(BlockType::Empty);
    ins.f64_const(f64::from_bits(1).into());
    ins.call(rt(b, "float_bounds"));
    ins.local_set(PARITY);
    ins.i32_const(din);
    ins.local_get(L);
    ins.local_get(K);
    ins.i32_wrap_i64();
    ins.i32_const(area(b, AREA_LO));
    ins.i32_const(meta);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 4);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(CMP);
    apply_sticky(ins, CMP, TRUNC);
    ins.local_get(CMP);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.if_(BlockType::Empty);
    ins.f64_const(0.0.into());
    finish(b, ins, Some(NEG));
    ins.end();
    ins.f64_const(f64::from_bits(1).into());
    ins.local_set(Y);
    ins.br(0);
    ins.end();
    // The interval of the candidate.
    ins.local_get(Y);
    ins.call(rt(b, "float_bounds"));
    ins.local_set(PARITY);
    ins.i32_const(din);
    ins.local_get(L);
    ins.local_get(K);
    ins.i32_wrap_i64();
    ins.i32_const(area(b, AREA_LO));
    ins.i32_const(meta);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 4);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(CMP);
    apply_sticky(ins, CMP, TRUNC);
    // Below the lower midpoint.
    ins.local_get(CMP);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(YBITS);
    ins.i64_const(1);
    ins.i64_sub();
    ins.f64_reinterpret_i64();
    ins.local_set(Y);
    ins.br(1);
    ins.end();
    // On the lower midpoint: the even mantissa wins.
    ins.local_get(CMP);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(PARITY);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(Y);
    finish(b, ins, Some(NEG));
    ins.end();
    ins.local_get(YBITS);
    ins.i64_const(1);
    ins.i64_sub();
    ins.f64_reinterpret_i64();
    ins.local_set(Y);
    ins.br(1);
    ins.end();
    // Above the upper midpoint.
    ins.i32_const(din);
    ins.local_get(L);
    ins.local_get(K);
    ins.i32_wrap_i64();
    ins.i32_const(area(b, AREA_HI));
    ins.i32_const(meta + 8);
    ins.i32_load(mem(0, 4));
    ins.i32_const(meta + 12);
    ins.i32_load(mem(0, 4));
    ins.call(rt(b, "dec_cmp"));
    ins.local_set(CMP);
    apply_sticky(ins, CMP, TRUNC);
    ins.local_get(CMP);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(YBITS);
    ins.i64_const(1);
    ins.i64_add();
    ins.f64_reinterpret_i64();
    ins.local_set(Y);
    ins.br(1);
    ins.end();
    // On the upper midpoint: the even mantissa wins.
    ins.local_get(CMP);
    ins.i32_eqz();
    ins.local_get(PARITY);
    ins.i32_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.local_get(Y);
    finish(b, ins, Some(NEG));
    ins.end();
    // Inside the interval.
    ins.local_get(CMP);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(Y);
    finish(b, ins, Some(NEG));
    ins.end();
    // Otherwise the next value.
    ins.local_get(YBITS);
    ins.i64_const(1);
    ins.i64_add();
    ins.f64_reinterpret_i64();
    ins.local_set(Y);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(Y);
    finish(b, ins, Some(NEG));
}

/// Pushes `neg` and the `Some` descriptor, calls `float_some` with the value already on the stack,
/// and returns from the function.
fn finish(b: &mut Builder, ins: &mut InstructionSink, neg: Option<u32>) {
    match neg {
        Some(n) => {
            ins.local_get(n);
        }
        None => {
            ins.i32_const(0);
        }
    };
    ins.local_get(1);
    ins.call(rt(b, "float_some"));
    ins.return_();
}
