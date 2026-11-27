//! `Float.to_string`: the shortest digits that read back as the same binary64.

use super::*;

// ---- to_string ----

/// Copies `count` digits from `src` (starting at local `i`) to `dst` (starting at local `j`).
pub(super) fn copy_digits(
    ins: &mut InstructionSink,
    src: i32,
    dst: i32,
    i: u32,
    j: u32,
    count: u32,
) {
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
pub(super) fn zeros(ins: &mut InstructionSink, dst: i32, j: u32, count: u32) {
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

pub(super) fn put_out(ins: &mut InstructionSink, dst: i32, j: u32, byte: u8) {
    elem(ins, dst, j, 1);
    ins.i32_const(byte as i32);
    ins.i32_store8(mem(0, 1));
    bump(ins, j);
}

/// Stores the byte in local `value` at `dst[j]` and advances `j`.
pub(super) fn put_digit(ins: &mut InstructionSink, dst: i32, j: u32, value: u32) {
    elem(ins, dst, j, 1);
    ins.local_get(value);
    ins.i32_store8(mem(0, 1));
    bump(ins, j);
}

/// Adds one to the `len`-digit decimal at `base`, adjusting `len` and `exp` on a carry out.
pub(super) fn carry_one(
    ins: &mut InstructionSink,
    base: i32,
    len: u32,
    exp: u32,
    dexp: u32,
    i: u32,
) {
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
pub(super) struct Candidate {
    base: i32,
    len: u32,
    exp: u32,
    out: u32,
}

/// Whether `q` is in the rounding interval in the meta area. `cmp` and `tmp` are scratch locals.
pub(super) fn inside(
    b: &Builder,
    ins: &mut InstructionSink,
    q: Candidate,
    parity: u32,
    cmp: u32,
    tmp: u32,
) {
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

pub(super) fn to_string(b: &mut Builder, ins: &mut InstructionSink) {
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
