//! Exact decimals: base 10^9 big integers, their digits, and the rounding interval of a value.

use super::*;

// ---- base 10^9 limbs ----

/// Multiplies the big integer by `mult` and returns its new limb count.
pub(super) fn limbs_mul(b: &mut Builder, ins: &mut InstructionSink) {
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

/// Multiplies the limbs by `mult` for as long as `T` holds a whole `chunk`, and subtracts the
/// chunk from `T` each time.
pub(super) fn mul_chunks(
    b: &Builder,
    ins: &mut InstructionSink,
    t: u32,
    n: u32,
    chunk: i32,
    mult: i64,
) {
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(t);
    ins.i32_const(chunk);
    ins.i32_lt_s();
    ins.br_if(1);
    ins.i64_const(mult);
    ins.local_get(n);
    ins.call(rt(b, "limbs_mul"));
    ins.local_set(n);
    ins.local_get(t);
    ins.i32_const(chunk);
    ins.i32_sub();
    ins.local_set(t);
    ins.br(0);
    ins.end();
    ins.end();
}

/// Applies the rest of a power: each step is at least half of what can reach it, so after it
/// `T` is below the next step, and the last step takes what is left.
pub(super) fn mul_steps(
    b: &Builder,
    ins: &mut InstructionSink,
    t: u32,
    n: u32,
    steps: &[(i32, i64)],
) {
    for &(step, mult) in steps {
        ins.local_get(t);
        ins.i32_const(step);
        ins.i32_ge_s();
        ins.if_(BlockType::Empty);
        ins.i64_const(mult);
        ins.local_get(n);
        ins.call(rt(b, "limbs_mul"));
        ins.local_set(n);
        ins.local_get(t);
        ins.i32_const(step);
        ins.i32_sub();
        ins.local_set(t);
        ins.end();
    }
}

/// `dec_build(f, e, dest)`: the exact decimal digits of `f * 2^e`, most significant first, into
/// `dest`, and their count. `f` is the binary mantissa of a finite, non-zero binary64.
pub(super) fn dec_build(b: &mut Builder, ins: &mut InstructionSink) {
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
    // The limbs are not cleared: the loop below writes the limbs that hold the mantissa, and
    // every later step reads only the limbs a step has written.
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
    // `e > 0`: multiply by 2^32 chunks. `e < 0`: by 5^14 chunks, the largest power of five a
    // limb (`< 10^9`) takes without overflowing a u64, so the fewest passes over the limbs.
    // What is left is less than the chunk and is applied in the largest pieces below it, one
    // of each; every piece is at least half of the range that reaches it, so nothing is left.
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(1);
    ins.local_set(T);
    mul_chunks(b, ins, T, N, 32, 4_294_967_296);
    mul_steps(
        b,
        ins,
        T,
        N,
        &[(16, 65_536), (8, 256), (4, 16), (2, 4), (1, 2)],
    );
    ins.else_();
    ins.local_get(1);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_get(1);
    ins.i32_sub();
    ins.local_set(T);
    mul_chunks(b, ins, T, N, 14, 6_103_515_625);
    mul_steps(b, ins, T, N, &[(7, 78_125), (4, 625), (2, 25), (1, 5)]);
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
pub(super) fn dec_cmp(ins: &mut InstructionSink) {
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
pub(super) fn pow10(ins: &mut InstructionSink) {
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
pub(super) fn bounds(b: &mut Builder, ins: &mut InstructionSink) {
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
pub(super) fn some(b: &mut Builder, ins: &mut InstructionSink) {
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
