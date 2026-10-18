//! Floating-point runtime helpers.
//!
//! `fmod` is `Float % Float` (the wasm `f64` instructions have no remainder). The rest are the
//! `Float` builtins the WASM target needs:
//!
//! - `Float.to_string` (spec 16.1): the shortest decimal that reads back as the same f64, with
//!   the JavaScript `String(x)` shape (exponential only when the decimal exponent is `>= 21` or
//!   `<= -7`, `Infinity` / `-Infinity` / `NaN`, and a trailing `.0` for an integral value).
//! - `Float.parse` (spec 16.3): the forms `Float.to_string` writes.
//! - `Float.floor` / `ceil` / `round` (0.5 away from zero) / `truncate`, which convert to `Int`
//!   and raise a defect instead of trapping when the result is NaN or outside the `Int` range.
//!
//! WASI has no `printf`, so the formatting is implemented here. `float_to_string` decomposes the
//! f64 into the exact integer `m * 2^e`, scales the value and its two rounding margins into
//! integers, then runs the Dragon4 digit loop (Steele & White / Burger & Dybvig) to produce the
//! shortest round-tripping digits. The big integers are u32 limbs in a scratch block allocated
//! from the runtime heap for the duration of the call — the layout scratch area is fixed by
//! `layout.rs`, which this track must not grow.

use super::*;

/// The float fragment, after `CORE_TAIL` (see `runtime::FUNCS`).
///
/// These functions are registered unconditionally, even for a program that never mentions
/// `Float`. That is deliberate for now: `runtime::register` is the only gate, and teaching it to
/// drop this fragment would change the byte layout of every existing module (the runtime function
/// indices and the bodies before `float` are fixed by `FUNCS` order). The 15 functions stay in
/// the fixed fragment order, so a program that does use `Float` sees the same indices it did
/// before.
pub(super) const FUNCS: &[Sig] = &[
    ("fmod", &[ValType::F64, ValType::F64], &[ValType::F64]),
    ("float_to_string", &[ValType::F64], &[ValType::I32]),
    (
        "float_parse",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("float_floor", &[ValType::F64], &[ValType::I32]),
    ("float_ceil", &[ValType::F64], &[ValType::I32]),
    ("float_round", &[ValType::F64], &[ValType::I32]),
    ("float_truncate", &[ValType::F64], &[ValType::I32]),
    // The big-integer helpers, used only by `float_to_string`.
    ("bn_set_u64", &[ValType::I32, ValType::I64], &[]),
    ("bn_set_pow2", &[ValType::I32, ValType::I32], &[]),
    ("bn_shl", &[ValType::I32, ValType::I32], &[]),
    ("bn_mul_small", &[ValType::I32, ValType::I32], &[]),
    ("bn_cmp", &[ValType::I32, ValType::I32], &[ValType::I32]),
    ("bn_sub", &[ValType::I32, ValType::I32], &[]),
    ("bn_add", &[ValType::I32, ValType::I32], &[]),
    ("bn_copy", &[ValType::I32, ValType::I32], &[]),
];

/// The number of `u32` limbs one scratch bignum can hold. The largest intermediate of the
/// Dragon4 scaling below is bounded by `10^324 < 2^1080`, so 40 limbs (1280 bits) leave margin;
/// the asserts after the strides pin that bound and the digit capacity at compile time.
const BN_LIMBS: u32 = 40;
/// One bignum: the length in limbs (4 bytes) then the limbs, rounded up to an 8-byte boundary.
const BN_STRIDE: u32 = (4 + BN_LIMBS * 4 + 7) & !7;
const BN_R: u32 = HDR;
const BN_S: u32 = HDR + BN_STRIDE;
const BN_MP: u32 = HDR + 2 * BN_STRIDE;
const BN_MM: u32 = HDR + 3 * BN_STRIDE;
const BN_T: u32 = HDR + 4 * BN_STRIDE;
const BN_DIG: u32 = HDR + 5 * BN_STRIDE; // The digits start here; the stride leaves room.
/// The whole scratch block: the object header plus the six strides. This is what `alloc` takes,
/// in bytes and in its 8-byte slots.
const SCRATCH_BYTES: u32 = HDR + 6 * BN_STRIDE;
const SCRATCH_SLOTS: i32 = (SCRATCH_BYTES / 8) as i32;
/// `float_to_string` stops after 24 digits and may prepend one carry digit; the digit buffer and
/// the reused exponent buffer each get a whole stride.
const MAX_DIGITS: u32 = 25;
const _: () = assert!(BN_LIMBS * 32 >= 1080);
const _: () = assert!(BN_STRIDE >= MAX_DIGITS);

/// Returns the raw scratch block `p` (allocated with [`SCRATCH_SLOTS`] slots) to the runtime's
/// free list. The block is plain runtime memory, not a counted value, so it goes back exactly the
/// way the allocator's own `free_raw` pushes a block a `drop` emptied: the size in the `rc` field
/// (offset 0) and the old list head in `aux` (offset 4). This keeps the release independent of
/// `alloc`/`drop` and of the block carrying a `kind == 1` descriptor.
fn free_scratch(ins: &mut InstructionSink, p: u32) {
    ins.local_get(p);
    ins.i32_const(SCRATCH_BYTES as i32);
    ins.i32_store(mem(0, 4));
    ins.local_get(p);
    ins.global_get(G_FREE);
    ins.i32_store(mem(4, 4));
    ins.local_get(p);
    ins.global_set(G_FREE);
}

pub(super) fn body(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "fmod" => fmod(ins),
        "float_to_string" => float_to_string(b, ins),
        "float_parse" => float_parse(b, ins),
        "float_floor" => float_to_int(b, ins, Rounding::Floor),
        "float_ceil" => float_to_int(b, ins, Rounding::Ceil),
        "float_round" => float_to_int(b, ins, Rounding::Round),
        "float_truncate" => float_to_int(b, ins, Rounding::Truncate),
        "bn_set_u64" => bn_set_u64(ins),
        "bn_set_pow2" => bn_set_pow2(ins),
        "bn_shl" => bn_shl(ins),
        "bn_mul_small" => bn_mul_small(ins),
        "bn_cmp" => bn_cmp(ins),
        "bn_sub" => bn_sub(ins),
        "bn_add" => bn_add(ins),
        "bn_copy" => bn_copy(ins),
        _ => return false,
    }
    true
}

pub(super) fn locals(name: &str) -> Option<Vec<(u32, ValType)>> {
    let l = match name {
        "fmod" => Vec::new(),
        "float_to_string" => vec![(24, ValType::I32), (2, ValType::I64)],
        "float_parse" => vec![(12, ValType::I32), (1, ValType::I64), (1, ValType::F64)],
        "float_floor" | "float_ceil" | "float_round" | "float_truncate" => {
            vec![(4, ValType::I32), (2, ValType::F64)]
        }
        "bn_set_u64" => vec![(2, ValType::I32)],
        "bn_set_pow2" => vec![(4, ValType::I32)],
        "bn_shl" => vec![(4, ValType::I32), (2, ValType::I64)],
        "bn_mul_small" => vec![(2, ValType::I32), (2, ValType::I64)],
        "bn_cmp" => vec![(5, ValType::I32)],
        "bn_sub" => vec![(3, ValType::I32), (3, ValType::I64)],
        "bn_add" => vec![(4, ValType::I32), (4, ValType::I64)],
        "bn_copy" => vec![(2, ValType::I32)],
        _ => return None,
    };
    Some(l)
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

// ---- big integers ----

/// Pushes the address of limb `idx` (a local) of the bignum in local `base` (`base + 4 + 4*idx`).
fn bn_limb(ins: &mut InstructionSink, base: u32, idx: u32) {
    ins.local_get(base);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_get(idx);
    ins.i32_const(2);
    ins.i32_shl();
    ins.i32_add();
}

/// Pushes the address of limb `idx` (a constant) of the bignum in local `base`.
fn bn_limb_const(ins: &mut InstructionSink, base: u32, idx: i32) {
    ins.local_get(base);
    ins.i32_const(4 + idx * 4);
    ins.i32_add();
}

/// Pushes `local[p] + off`.
fn bn_ptr(ins: &mut InstructionSink, p: u32, off: u32) {
    ins.local_get(p);
    ins.i32_const(off as i32);
    ins.i32_add();
}

/// `a = 2^k`, `k >= 0`.
fn bn_set_pow2(ins: &mut InstructionSink) {
    const W: u32 = 2; // k >> 5
    const B: u32 = 3; // k & 31
    const I: u32 = 4;
    const LEN: u32 = 5;
    ins.local_get(1);
    ins.i32_const(5);
    ins.i32_shr_u();
    ins.local_set(W);
    ins.local_get(1);
    ins.i32_const(31);
    ins.i32_and();
    ins.local_set(B);
    ins.local_get(W);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(LEN);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    bn_limb(ins, 0, I);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    bn_limb(ins, 0, W);
    ins.i32_const(1);
    ins.local_get(B);
    ins.i32_shl();
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.local_get(LEN);
    ins.i32_store(mem(0, 4));
}

/// `a = v`, `v >= 0`.
fn bn_set_u64(ins: &mut InstructionSink) {
    const LO: u32 = 2;
    const HI: u32 = 3;
    ins.local_get(1);
    ins.i32_wrap_i64();
    ins.local_set(LO);
    ins.local_get(1);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.i32_wrap_i64();
    ins.local_set(HI);
    ins.local_get(HI);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(LO);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(0);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.else_();
    bn_limb_const(ins, 0, 0);
    ins.local_get(LO);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.i32_const(1);
    ins.i32_store(mem(0, 4));
    ins.end();
    ins.else_();
    bn_limb_const(ins, 0, 0);
    ins.local_get(LO);
    ins.i32_store(mem(0, 4));
    bn_limb_const(ins, 0, 1);
    ins.local_get(HI);
    ins.i32_store(mem(0, 4));
    ins.local_get(0);
    ins.i32_const(2);
    ins.i32_store(mem(0, 4));
    ins.end();
}

/// `a <<= bits`, `bits >= 0`.
fn bn_shl(ins: &mut InstructionSink) {
    const W: u32 = 2;
    const B: u32 = 3;
    const I: u32 = 4;
    const LEN: u32 = 5;
    const CARRY: u32 = 6;
    const CUR: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(LEN);
    ins.local_get(1);
    ins.i32_const(5);
    ins.i32_shr_u();
    ins.local_set(W);
    ins.local_get(1);
    ins.i32_const(31);
    ins.i32_and();
    ins.local_set(B);
    // The bit shift.
    ins.local_get(B);
    ins.if_(BlockType::Empty);
    ins.i64_const(0);
    ins.local_set(CARRY);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.local_get(B);
    ins.i64_extend_i32_u();
    ins.i64_shl();
    ins.local_get(CARRY);
    ins.i64_or();
    ins.local_set(CUR);
    bn_limb(ins, 0, I);
    ins.local_get(CUR);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(CUR);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.local_set(CARRY);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(CARRY);
    ins.i64_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    bn_limb(ins, 0, LEN);
    ins.local_get(CARRY);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(LEN);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(LEN);
    ins.end();
    ins.end();
    // The word shift.
    ins.local_get(W);
    ins.if_(BlockType::Empty);
    ins.local_get(LEN);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.local_get(0);
    ins.i32_const(4);
    ins.i32_add();
    ins.local_get(I);
    ins.local_get(W);
    ins.i32_add();
    ins.i32_const(2);
    ins.i32_shl();
    ins.i32_add();
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(0, 4));
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(W);
    ins.i32_ge_u();
    ins.br_if(1);
    bn_limb(ins, 0, I);
    ins.i32_const(0);
    ins.i32_store(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(LEN);
    ins.local_get(W);
    ins.i32_add();
    ins.local_set(LEN);
    ins.end();
    ins.local_get(0);
    ins.local_get(LEN);
    ins.i32_store(mem(0, 4));
}

/// `a *= m` for a small (u32) `m`.
fn bn_mul_small(ins: &mut InstructionSink) {
    const LEN: u32 = 2;
    const I: u32 = 3;
    const CARRY: u32 = 4;
    const V: u32 = 5;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(LEN);
    ins.i64_const(0);
    ins.local_set(CARRY);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.local_get(1);
    ins.i64_extend_i32_u();
    ins.i64_mul();
    ins.local_get(CARRY);
    ins.i64_add();
    ins.local_set(V);
    bn_limb(ins, 0, I);
    ins.local_get(V);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(V);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.local_set(CARRY);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(CARRY);
    ins.i64_eqz();
    ins.br_if(1);
    bn_limb(ins, 0, LEN);
    ins.local_get(CARRY);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(LEN);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(LEN);
    ins.local_get(CARRY);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.local_set(CARRY);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(LEN);
    ins.i32_store(mem(0, 4));
}

/// `-1`, `0` or `1` for `a < b`, `a == b`, `a > b`.
fn bn_cmp(ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const I: u32 = 4;
    const X: u32 = 5;
    const Y: u32 = 6;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(0, 4));
    ins.local_set(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1);
    ins.i32_const(1);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_lt_u();
    ins.select();
    ins.return_();
    ins.end();
    ins.local_get(LA);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.local_set(X);
    bn_limb(ins, 1, I);
    ins.i32_load(mem(0, 4));
    ins.local_set(Y);
    ins.local_get(X);
    ins.local_get(Y);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1);
    ins.i32_const(1);
    ins.local_get(X);
    ins.local_get(Y);
    ins.i32_lt_u();
    ins.select();
    ins.return_();
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
    ins.i32_const(0);
}

/// `a -= b` (`a >= b`).
fn bn_sub(ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const I: u32 = 4;
    const BORROW: u32 = 5;
    const D: u32 = 6;
    const BI: u32 = 7;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(0, 4));
    ins.local_set(LB);
    ins.i64_const(0);
    ins.local_set(BORROW);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LA);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_get(LB);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I64));
    bn_limb(ins, 1, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.else_();
    ins.i64_const(0);
    ins.end();
    ins.local_set(BI);
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.local_get(BI);
    ins.i64_sub();
    ins.local_get(BORROW);
    ins.i64_sub();
    ins.local_set(D);
    bn_limb(ins, 0, I);
    ins.local_get(D);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(D);
    ins.i64_const(63);
    ins.i64_shr_s();
    ins.i64_const(1);
    ins.i64_and();
    ins.local_set(BORROW);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(LA);
    ins.i32_eqz();
    ins.br_if(1);
    bn_limb(ins, 0, LA);
    ins.i32_const(4);
    ins.i32_sub();
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(LA);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(LA);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(LA);
    ins.i32_store(mem(0, 4));
}

/// `a += b`.
fn bn_add(ins: &mut InstructionSink) {
    const LA: u32 = 2;
    const LB: u32 = 3;
    const N: u32 = 4;
    const I: u32 = 5;
    const CARRY: u32 = 6;
    const S: u32 = 7;
    const AI: u32 = 8;
    const BI: u32 = 9;
    ins.local_get(0);
    ins.i32_load(mem(0, 4));
    ins.local_set(LA);
    ins.local_get(1);
    ins.i32_load(mem(0, 4));
    ins.local_set(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.local_get(LA);
    ins.local_get(LB);
    ins.i32_gt_u();
    ins.select();
    ins.local_set(N);
    ins.i64_const(0);
    ins.local_set(CARRY);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(I);
    ins.local_get(LA);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I64));
    bn_limb(ins, 0, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.else_();
    ins.i64_const(0);
    ins.end();
    ins.local_set(AI);
    ins.local_get(I);
    ins.local_get(LB);
    ins.i32_lt_u();
    ins.if_(BlockType::Result(ValType::I64));
    bn_limb(ins, 1, I);
    ins.i32_load(mem(0, 4));
    ins.i64_extend_i32_u();
    ins.else_();
    ins.i64_const(0);
    ins.end();
    ins.local_set(BI);
    ins.local_get(AI);
    ins.local_get(BI);
    ins.i64_add();
    ins.local_get(CARRY);
    ins.i64_add();
    ins.local_set(S);
    bn_limb(ins, 0, I);
    ins.local_get(S);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(S);
    ins.i64_const(32);
    ins.i64_shr_u();
    ins.local_set(CARRY);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(CARRY);
    ins.i64_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    bn_limb(ins, 0, N);
    ins.local_get(CARRY);
    ins.i32_wrap_i64();
    ins.i32_store(mem(0, 4));
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(N);
    ins.i32_eqz();
    ins.br_if(1);
    bn_limb(ins, 0, N);
    ins.i32_const(4);
    ins.i32_sub();
    ins.i32_load(mem(0, 4));
    ins.i32_eqz();
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(N);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(0);
    ins.local_get(N);
    ins.i32_store(mem(0, 4));
}

/// `a = b`.
fn bn_copy(ins: &mut InstructionSink) {
    const LEN: u32 = 2;
    const I: u32 = 3;
    ins.local_get(1);
    ins.i32_load(mem(0, 4));
    ins.local_set(LEN);
    ins.local_get(0);
    ins.local_get(LEN);
    ins.i32_store(mem(0, 4));
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(LEN);
    ins.i32_ge_u();
    ins.br_if(1);
    bn_limb(ins, 0, I);
    bn_limb(ins, 1, I);
    ins.i32_load(mem(0, 4));
    ins.i32_store(mem(0, 4));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
}

// ---- Float.to_string ----

// The locals of `float_to_string`.
const X: u32 = 0; // The f64 parameter.
const SIGN: u32 = 1;
const EF: u32 = 2;
const EE: u32 = 3;
const K: u32 = 4;
const J: u32 = 5;
const N: u32 = 6;
const P: u32 = 7;
const ND: u32 = 8;
const D: u32 = 9;
const LOW: u32 = 10;
const HIGH: u32 = 11;
const INCL: u32 = 12;
const I: u32 = 13;
const LEN: u32 = 14;
const OUT: u32 = 15;
const POS: u32 = 16;
const TMP: u32 = 17;
const E10: u32 = 18;
const CNT: u32 = 19;
const STOP: u32 = 20;
const Q: u32 = 21;
const ABSE: u32 = 22;
const BR: u32 = 23;
const V: u32 = 24;
const BITS: u32 = 25; // i64
const M: u32 = 26; // i64

fn store_digit(ins: &mut InstructionSink, p: u32, nd: u32, d: u32) {
    bn_ptr(ins, p, BN_DIG);
    ins.local_get(nd);
    ins.i32_add();
    ins.local_get(d);
    ins.i32_store8(mem(0, 1));
}

fn float_to_string(b: &mut Builder, ins: &mut InstructionSink) {
    let nan = b.string_literal(b"NaN");
    let inf = b.string_literal(b"Infinity");
    let neg_inf = b.string_literal(b"-Infinity");
    let zero = b.string_literal(b"0.0");
    // Decompose `x` into `sign`, the exponent field, the fraction bits.
    ins.local_get(X);
    ins.i64_reinterpret_f64();
    ins.local_set(BITS);
    ins.local_get(BITS);
    ins.i64_const(63);
    ins.i64_shr_u();
    ins.i32_wrap_i64();
    ins.local_set(SIGN);
    ins.local_get(BITS);
    ins.i64_const(52);
    ins.i64_shr_u();
    ins.i64_const(0x7ff);
    ins.i64_and();
    ins.i32_wrap_i64();
    ins.local_set(EF);
    ins.local_get(BITS);
    ins.i64_const(0xFFFFFFFFFFFFF);
    ins.i64_and();
    ins.local_set(M);
    // NaN / Infinity.
    ins.local_get(EF);
    ins.i32_const(0x7ff);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(M);
    ins.i64_eqz();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(nan as i32);
    ins.return_();
    ins.end();
    ins.local_get(SIGN);
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(neg_inf as i32);
    ins.else_();
    ins.i32_const(inf as i32);
    ins.end();
    ins.return_();
    ins.end();
    // Zero (`-0.0` too: JavaScript `String(-0)` is `"0"`).
    ins.local_get(EF);
    ins.i32_eqz();
    ins.local_get(M);
    ins.i64_eqz();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(zero as i32);
    ins.return_();
    ins.end();
    // The exact value is `m * 2^e`.
    ins.local_get(EF);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1074);
    ins.local_set(EE);
    ins.else_();
    ins.local_get(M);
    ins.i64_const(1i64 << 52);
    ins.i64_or();
    ins.local_set(M);
    ins.local_get(EF);
    ins.i32_const(1075);
    ins.i32_sub();
    ins.local_set(EE);
    ins.end();
    // The rounding margins are open or closed with the significand's parity (round-half-even).
    ins.local_get(M);
    ins.i64_const(1);
    ins.i64_and();
    ins.i64_eqz();
    ins.local_set(INCL);
    // `k = max(0, 2 - e)`; `R = m << (e + k)`, `S = 2^k`, `MP = MM = 2^(e - 1 + k)`.
    ins.i32_const(2);
    ins.local_get(EE);
    ins.i32_sub();
    ins.local_set(K);
    ins.local_get(K);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(K);
    ins.end();
    ins.i32_const(SCRATCH_SLOTS);
    ins.global_get(G_STRDESC);
    ins.call(rt(b, "alloc"));
    ins.local_set(P);
    bn_ptr(ins, P, BN_R);
    ins.local_get(M);
    ins.call(rt(b, "bn_set_u64"));
    bn_ptr(ins, P, BN_R);
    ins.local_get(EE);
    ins.local_get(K);
    ins.i32_add();
    ins.call(rt(b, "bn_shl"));
    bn_ptr(ins, P, BN_S);
    ins.local_get(K);
    ins.call(rt(b, "bn_set_pow2"));
    bn_ptr(ins, P, BN_MP);
    ins.local_get(EE);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_get(K);
    ins.i32_add();
    ins.call(rt(b, "bn_set_pow2"));
    ins.local_get(M);
    ins.i64_const(1i64 << 52);
    ins.i64_eq();
    ins.local_get(EE);
    ins.i32_const(-1074);
    ins.i32_gt_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    bn_ptr(ins, P, BN_MM);
    ins.local_get(EE);
    ins.i32_const(2);
    ins.i32_sub();
    ins.local_get(K);
    ins.i32_add();
    ins.call(rt(b, "bn_set_pow2"));
    ins.else_();
    bn_ptr(ins, P, BN_MM);
    ins.local_get(EE);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_get(K);
    ins.i32_add();
    ins.call(rt(b, "bn_set_pow2"));
    ins.end();
    // Normalise `R/S` into `[1, 10)`: first by shrinking `S`, then by growing `R` and the margins.
    ins.i32_const(0);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_copy"));
    bn_ptr(ins, P, BN_T);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_R);
    bn_ptr(ins, P, BN_T);
    ins.call(rt(b, "bn_cmp"));
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.br_if(1);
    bn_ptr(ins, P, BN_S);
    bn_ptr(ins, P, BN_T);
    ins.call(rt(b, "bn_copy"));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    bn_ptr(ins, P, BN_R);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_cmp"));
    ins.i32_const(0);
    ins.i32_ge_s();
    ins.br_if(1);
    bn_ptr(ins, P, BN_R);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_MP);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_MM);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(J);
    ins.br(0);
    ins.end();
    ins.end();
    // `n` is the position of the decimal point.
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    // The digit loop.
    ins.i32_const(0);
    ins.local_set(ND);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    // `d = floor(R / S)`, `R = R % S`.
    ins.i32_const(0);
    ins.local_set(D);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    bn_ptr(ins, P, BN_R);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_cmp"));
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.br_if(1);
    bn_ptr(ins, P, BN_R);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_sub"));
    ins.local_get(D);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(D);
    ins.br(0);
    ins.end();
    ins.end();
    // `high`: the interval reaches the next integer (`>=` when inclusive).
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_R);
    ins.call(rt(b, "bn_copy"));
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_MP);
    ins.call(rt(b, "bn_add"));
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_cmp"));
    ins.local_set(TMP);
    ins.local_get(INCL);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(TMP);
    ins.i32_const(0);
    ins.i32_ge_s();
    ins.else_();
    ins.local_get(TMP);
    ins.i32_const(0);
    ins.i32_gt_s();
    ins.end();
    ins.local_set(HIGH);
    // `low`: the interval reaches the integer (`<=` when inclusive).
    bn_ptr(ins, P, BN_R);
    bn_ptr(ins, P, BN_MM);
    ins.call(rt(b, "bn_cmp"));
    ins.local_set(TMP);
    ins.local_get(INCL);
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(TMP);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.else_();
    ins.local_get(TMP);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.end();
    ins.local_set(LOW);
    ins.i32_const(0);
    ins.local_set(STOP);
    ins.local_get(LOW);
    ins.local_get(HIGH);
    ins.i32_or();
    ins.if_(BlockType::Empty);
    // The final digit.
    ins.local_get(LOW);
    ins.local_get(HIGH);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // Both ends are reachable: round to the nearest, ties to the larger.
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_R);
    ins.call(rt(b, "bn_copy"));
    bn_ptr(ins, P, BN_T);
    ins.i32_const(2);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_T);
    bn_ptr(ins, P, BN_S);
    ins.call(rt(b, "bn_cmp"));
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(D);
    ins.else_();
    ins.local_get(D);
    ins.i32_const(1);
    ins.i32_add();
    ins.end();
    ins.local_set(D);
    ins.else_();
    ins.local_get(HIGH);
    ins.if_(BlockType::Empty);
    ins.local_get(D);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(D);
    ins.end();
    ins.end();
    store_digit(ins, P, ND, D);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ND);
    ins.i32_const(1);
    ins.local_set(STOP);
    ins.else_();
    // Another digit: emit and scale.
    store_digit(ins, P, ND, D);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ND);
    bn_ptr(ins, P, BN_R);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_MP);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    bn_ptr(ins, P, BN_MM);
    ins.i32_const(10);
    ins.call(rt(b, "bn_mul_small"));
    ins.local_get(ND);
    ins.i32_const(24);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(STOP);
    ins.end();
    ins.end();
    ins.local_get(STOP);
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    // A final digit may have carried into `10`.
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(STOP);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(STOP);
    ins.br_if(1);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(10);
    ins.i32_lt_u();
    ins.br_if(1);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_const(0);
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    // The carry leaves the leading digit: prepend `1` and move the decimal point.
    ins.local_get(ND);
    ins.local_set(J);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(J);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(J);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(J);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(V);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(J);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_add();
    ins.local_get(V);
    ins.i32_store8(mem(0, 1));
    ins.br(0);
    ins.end();
    ins.end();
    bn_ptr(ins, P, BN_DIG);
    ins.i32_const(1);
    ins.i32_store8(mem(0, 1));
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(ND);
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(N);
    ins.i32_const(1);
    ins.local_set(STOP);
    ins.else_();
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(I);
    ins.i32_add();
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const(1);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
    // Drop trailing zeros.
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_le_u();
    ins.br_if(1);
    bn_ptr(ins, P, BN_DIG);
    ins.local_get(ND);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_sub();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_eqz();
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(ND);
    ins.br(0);
    ins.end();
    ins.end();
    // The output shape (ECMAScript `Number::toString`).
    ins.local_get(ND);
    ins.local_get(N);
    ins.i32_le_s();
    ins.local_get(N);
    ins.i32_const(21);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(BR);
    ins.local_get(N);
    ins.i32_const(2);
    ins.i32_add();
    ins.local_set(LEN);
    ins.else_();
    ins.i32_const(0);
    ins.local_get(N);
    ins.i32_lt_s();
    ins.local_get(N);
    ins.i32_const(21);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(BR);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(LEN);
    ins.else_();
    ins.i32_const(-6);
    ins.local_get(N);
    ins.i32_lt_s();
    ins.local_get(N);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(BR);
    ins.i32_const(2);
    ins.i32_const(0);
    ins.local_get(N);
    ins.i32_sub();
    ins.i32_add();
    ins.local_get(ND);
    ins.i32_add();
    ins.local_set(LEN);
    ins.else_();
    ins.i32_const(3);
    ins.local_set(BR);
    ins.local_get(N);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(E10);
    ins.local_get(E10);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(0);
    ins.local_get(E10);
    ins.i32_sub();
    ins.else_();
    ins.local_get(E10);
    ins.end();
    ins.local_set(ABSE);
    ins.i32_const(1);
    ins.local_set(CNT);
    ins.local_get(ABSE);
    ins.local_set(Q);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(Q);
    ins.i32_const(10);
    ins.i32_lt_u();
    ins.br_if(1);
    ins.local_get(Q);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.local_set(Q);
    ins.local_get(CNT);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(CNT);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(ND);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_gt_s();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(1);
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.i32_add();
    ins.local_get(CNT);
    ins.i32_add();
    ins.i32_const(2);
    ins.i32_add();
    ins.local_set(LEN);
    ins.end();
    ins.end();
    ins.end();
    // Build the string.
    ins.local_get(LEN);
    ins.local_get(SIGN);
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(1);
    ins.else_();
    ins.i32_const(0);
    ins.end();
    ins.i32_add();
    ins.call(rt(b, "str_new"));
    ins.local_set(OUT);
    ins.i32_const(0);
    ins.local_set(POS);
    ins.local_get(SIGN);
    ins.if_(BlockType::Empty);
    put_byte(ins, OUT, POS, '-' as i32);
    ins.end();
    // Branch 0: digits, zeros, `.0`.
    ins.local_get(BR);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    put_digits(ins, P, OUT, POS, TD::Const(0), ND);
    ins.local_get(ND);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_s();
    ins.br_if(1);
    put_byte(ins, OUT, POS, '0' as i32);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    put_byte(ins, OUT, POS, '.' as i32);
    put_byte(ins, OUT, POS, '0' as i32);
    ins.else_();
    ins.local_get(BR);
    ins.i32_const(1);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_digits(ins, P, OUT, POS, TD::Const(0), N);
    put_byte(ins, OUT, POS, '.' as i32);
    put_digits(ins, P, OUT, POS, TD::Local(N), ND);
    ins.else_();
    ins.local_get(BR);
    ins.i32_const(2);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    put_byte(ins, OUT, POS, '0' as i32);
    put_byte(ins, OUT, POS, '.' as i32);
    ins.i32_const(0);
    ins.local_set(I);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_const(0);
    ins.local_get(N);
    ins.i32_sub();
    ins.i32_ge_s();
    ins.br_if(1);
    put_byte(ins, OUT, POS, '0' as i32);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
    put_digits(ins, P, OUT, POS, TD::Const(0), ND);
    ins.else_();
    // Branch 3: exponential.
    bn_ptr(ins, P, BN_DIG);
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('0' as i32);
    ins.i32_add();
    ins.local_set(V);
    put_local(ins, OUT, POS, V);
    ins.local_get(ND);
    ins.i32_const(1);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    put_byte(ins, OUT, POS, '.' as i32);
    put_digits(ins, P, OUT, POS, TD::Const(1), ND);
    ins.end();
    put_byte(ins, OUT, POS, 'e' as i32);
    ins.local_get(E10);
    ins.i32_const(0);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    put_byte(ins, OUT, POS, '-' as i32);
    ins.else_();
    put_byte(ins, OUT, POS, '+' as i32);
    ins.end();
    // The exponent digits, reversed into the now free `BN_T` buffer.
    ins.i32_const(0);
    ins.local_set(I);
    ins.local_get(ABSE);
    ins.local_set(Q);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    bn_ptr(ins, P, BN_T);
    ins.local_get(I);
    ins.i32_add();
    ins.local_get(Q);
    ins.i32_const(10);
    ins.i32_rem_u();
    ins.i32_const('0' as i32);
    ins.i32_add();
    ins.i32_store8(mem(0, 1));
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.local_get(Q);
    ins.i32_const(10);
    ins.i32_div_u();
    ins.local_set(Q);
    ins.local_get(Q);
    ins.i32_eqz();
    ins.br_if(1);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.i32_eqz();
    ins.br_if(1);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(I);
    bn_ptr(ins, P, BN_T);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.local_set(V);
    put_local(ins, OUT, POS, V);
    ins.br(0);
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    ins.end();
    // Release the scratch block before returning the string: it is raw runtime memory, not a
    // counted value, so it goes straight back to the allocator's free list.
    free_scratch(ins, P);
    ins.local_get(OUT);
}

/// The start of a digit run: a constant index or a local holding one.
enum TD {
    Const(i32),
    Local(u32),
}

/// Writes the digit bytes `BN_DIG + from .. BN_DIG + to` into the string `out` at `pos`.
fn put_digits(ins: &mut InstructionSink, p: u32, out: u32, pos: u32, from: TD, to: u32) {
    match from {
        TD::Const(v) => {
            ins.i32_const(v);
            ins.local_set(I);
        }
        TD::Local(l) => {
            ins.local_get(l);
            ins.local_set(I);
        }
    }
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(to);
    ins.i32_ge_s();
    ins.br_if(1);
    bn_ptr(ins, p, BN_DIG);
    ins.local_get(I);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
    ins.i32_const('0' as i32);
    ins.i32_add();
    ins.local_set(V);
    put_local(ins, out, pos, V);
    ins.local_get(I);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(I);
    ins.br(0);
    ins.end();
    ins.end();
}

// ---- Float.parse ----

// `Float.parse` (spec 16.3): the forms `Float.to_string` writes. The syntax check mirrors the C
// runtime and the JS target. The mantissa is accumulated exactly while it is below `10^17` before
// each digit, so up to 18 digits are kept exactly (the largest is `10^18 - 1`, still inside
// `i64`); the scaling by a power of ten is done with f64 arithmetic, so a full 18-digit input can
// land a few ulp away from a correctly rounded parse. Value fidelity across backends is out of
// scope for this wave (see the plan); the forms with a short decimal exponent are exact.

// The locals of `float_parse` (the three parameters are 0, 1, 2).
const FS: u32 = 0; // The String parameter.
const F_SOME: u32 = 1;
const F_NONE: u32 = 2;
const PN: u32 = 3;
const PI: u32 = 4;
const PNEG: u32 = 5;
const PB: u32 = 6;
const POK: u32 = 7;
const PFRAC: u32 = 8;
const PEXP: u32 = 9;
const PESIGN: u32 = 10;
const PEV: u32 = 11;
const PP: u32 = 12;
const PX: u32 = 13;
const PSCALE: u32 = 14;
const PMANT: u32 = 15; // i64
const PD: u32 = 16; // f64

/// Pushes the byte at offset `local[i]` of the string parameter.
fn parse_byte(ins: &mut InstructionSink, i: u32) {
    ins.local_get(FS);
    ins.i32_const((HDR + 4) as i32);
    ins.i32_add();
    ins.local_get(i);
    ins.i32_add();
    ins.i32_load8_u(mem(0, 1));
}

fn float_parse(b: &mut Builder, ins: &mut InstructionSink) {
    // `n = byte length`.
    ins.local_get(FS);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(PN);
    ins.i32_const(0);
    ins.local_set(PI);
    ins.i32_const(0);
    ins.local_set(PNEG);
    // `NaN` exactly.
    ins.local_get(PN);
    ins.i32_const(3);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(POK);
    for (k, ch) in b"NaN".iter().enumerate() {
        ins.local_get(POK);
        ins.i32_const(k as i32);
        ins.local_set(PX);
        parse_byte(ins, PX);
        ins.i32_const(*ch as i32);
        ins.i32_eq();
        ins.i32_and();
        ins.local_set(POK);
    }
    ins.local_get(POK);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_get(F_SOME);
    ins.call(rt(b, "alloc"));
    ins.local_set(PP);
    ins.local_get(PP);
    ins.f64_const(f64::NAN.into());
    ins.f64_store(mem(HDR, 8));
    ins.local_get(PP);
    ins.return_();
    ins.end();
    ins.end();
    // An optional sign.
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    parse_byte(ins, PI);
    ins.local_set(PB);
    ins.local_get(PB);
    ins.i32_const('+' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.else_();
    ins.local_get(PB);
    ins.i32_const('-' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(PNEG);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.end();
    ins.end();
    ins.end();
    // `Infinity` (with the sign already consumed).
    ins.local_get(PN);
    ins.local_get(PI);
    ins.i32_sub();
    ins.i32_const(8);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(POK);
    for (k, ch) in b"Infinity".iter().enumerate() {
        ins.local_get(POK);
        ins.local_get(PI);
        ins.i32_const(k as i32);
        ins.i32_add();
        ins.local_set(PX);
        parse_byte(ins, PX);
        ins.i32_const(*ch as i32);
        ins.i32_eq();
        ins.i32_and();
        ins.local_set(POK);
    }
    ins.local_get(POK);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_get(F_SOME);
    ins.call(rt(b, "alloc"));
    ins.local_set(PP);
    ins.local_get(PP);
    ins.local_get(PNEG);
    ins.if_(BlockType::Result(ValType::F64));
    ins.f64_const(f64::NEG_INFINITY.into());
    ins.else_();
    ins.f64_const(f64::INFINITY.into());
    ins.end();
    ins.f64_store(mem(HDR, 8));
    ins.local_get(PP);
    ins.return_();
    ins.end();
    ins.end();
    // The digits.
    ins.i32_const(0);
    ins.local_set(POK);
    ins.i32_const(0);
    ins.local_set(PFRAC);
    ins.i32_const(0);
    ins.local_set(PEXP);
    ins.i64_const(0);
    ins.local_set(PMANT);
    parse_digits(ins, false);
    ins.local_get(POK);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(F_NONE);
    ins.return_();
    ins.end();
    // An optional fraction, which needs at least one digit.
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    parse_byte(ins, PI);
    ins.i32_const('.' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.i32_const(0);
    ins.local_set(POK);
    parse_digits(ins, true);
    ins.local_get(POK);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.local_get(F_NONE);
    ins.return_();
    ins.end();
    ins.end();
    ins.end();
    // An optional exponent.
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    parse_byte(ins, PI);
    ins.i32_const('e' as i32);
    ins.i32_eq();
    parse_byte(ins, PI);
    ins.i32_const('E' as i32);
    ins.i32_eq();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.i32_const(1);
    ins.local_set(PESIGN);
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    parse_byte(ins, PI);
    ins.i32_const('+' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.else_();
    parse_byte(ins, PI);
    ins.i32_const('-' as i32);
    ins.i32_eq();
    ins.if_(BlockType::Empty);
    ins.i32_const(-1);
    ins.local_set(PESIGN);
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.end();
    ins.end();
    ins.end();
    // At least one digit.
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(F_NONE);
    ins.return_();
    ins.end();
    parse_byte(ins, PI);
    ins.local_set(PB);
    ins.local_get(PB);
    ins.i32_const('0' as i32);
    ins.i32_lt_u();
    ins.local_get(PB);
    ins.i32_const('9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.local_get(F_NONE);
    ins.return_();
    ins.end();
    ins.i32_const(0);
    ins.local_set(PEV);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_ge_u();
    ins.br_if(1);
    parse_byte(ins, PI);
    ins.local_set(PB);
    ins.local_get(PB);
    ins.i32_const('0' as i32);
    ins.i32_lt_u();
    ins.local_get(PB);
    ins.i32_const('9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.br_if(1);
    ins.local_get(PEV);
    ins.i32_const(100000);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(PEV);
    ins.i32_const(10);
    ins.i32_mul();
    ins.local_get(PB);
    ins.i32_const('0' as i32);
    ins.i32_sub();
    ins.i32_add();
    ins.local_set(PEV);
    ins.end();
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(PEXP);
    ins.local_get(PEV);
    ins.local_get(PESIGN);
    ins.i32_mul();
    ins.i32_add();
    ins.local_set(PEXP);
    ins.end();
    ins.end();
    // Everything must be consumed.
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_ne();
    ins.if_(BlockType::Empty);
    ins.local_get(F_NONE);
    ins.return_();
    ins.end();
    // The value: `mant * 10^(exp - fraction digits)`.
    ins.local_get(PMANT);
    ins.f64_convert_i64_s();
    ins.local_set(PD);
    ins.local_get(PEXP);
    ins.local_get(PFRAC);
    ins.i32_sub();
    ins.local_set(PSCALE);
    // Clamp the count; going past it only reaches 0 or Infinity anyway.
    ins.local_get(PSCALE);
    ins.i32_const(400);
    ins.i32_gt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(400);
    ins.local_set(PSCALE);
    ins.end();
    ins.local_get(PSCALE);
    ins.i32_const(-400);
    ins.i32_lt_s();
    ins.if_(BlockType::Empty);
    ins.i32_const(-400);
    ins.local_set(PSCALE);
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(PSCALE);
    ins.i32_const(0);
    ins.i32_le_s();
    ins.br_if(1);
    ins.local_get(PD);
    ins.f64_const(10.0.into());
    ins.f64_mul();
    ins.local_set(PD);
    ins.local_get(PSCALE);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(PSCALE);
    ins.br(0);
    ins.end();
    ins.end();
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(PSCALE);
    ins.i32_const(0);
    ins.i32_ge_s();
    ins.br_if(1);
    ins.local_get(PD);
    ins.f64_const(10.0.into());
    ins.f64_div();
    ins.local_set(PD);
    ins.local_get(PSCALE);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PSCALE);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(PNEG);
    ins.if_(BlockType::Empty);
    ins.local_get(PD);
    ins.f64_neg();
    ins.local_set(PD);
    ins.end();
    ins.i32_const(1);
    ins.local_get(F_SOME);
    ins.call(rt(b, "alloc"));
    ins.local_set(PP);
    ins.local_get(PP);
    ins.local_get(PD);
    ins.f64_store(mem(HDR, 8));
    ins.local_get(PP);
}

/// A run of `0..=9` of `Float.parse`: sets `POK` when it saw a digit, and accumulates the value.
fn parse_digits(ins: &mut InstructionSink, frac: bool) {
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(PI);
    ins.local_get(PN);
    ins.i32_ge_u();
    ins.br_if(1);
    parse_byte(ins, PI);
    ins.local_set(PB);
    ins.local_get(PB);
    ins.i32_const('0' as i32);
    ins.i32_lt_u();
    ins.local_get(PB);
    ins.i32_const('9' as i32);
    ins.i32_gt_u();
    ins.i32_or();
    ins.br_if(1);
    ins.i32_const(1);
    ins.local_set(POK);
    ins.local_get(PMANT);
    ins.i64_const(100000000000000000);
    ins.i64_lt_s();
    ins.if_(BlockType::Empty);
    ins.local_get(PMANT);
    ins.i64_const(10);
    ins.i64_mul();
    ins.local_get(PB);
    ins.i32_const('0' as i32);
    ins.i32_sub();
    ins.i64_extend_i32_s();
    ins.i64_add();
    ins.local_set(PMANT);
    if frac {
        ins.local_get(PFRAC);
        ins.i32_const(1);
        ins.i32_add();
        ins.local_set(PFRAC);
    }
    ins.else_();
    if !frac {
        ins.local_get(PEXP);
        ins.i32_const(1);
        ins.i32_add();
        ins.local_set(PEXP);
    }
    ins.end();
    ins.local_get(PI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(PI);
    ins.br(0);
    ins.end();
    ins.end();
}

// ---- The rounding conversions ----

enum Rounding {
    Floor,
    Ceil,
    Round,
    Truncate,
}

// The locals of the rounding conversions.
const C_R: u32 = 5; // f64
const C_F: u32 = 6; // f64
const C_M: u32 = 1;
const C_M2: u32 = 3;
const C_M3: u32 = 4;

fn float_to_int(b: &mut Builder, ins: &mut InstructionSink, kind: Rounding) {
    match kind {
        Rounding::Floor => {
            ins.local_get(0);
            ins.f64_floor();
            ins.local_set(C_R);
        }
        Rounding::Ceil => {
            ins.local_get(0);
            ins.f64_ceil();
            ins.local_set(C_R);
        }
        Rounding::Truncate => {
            ins.local_get(0);
            ins.f64_trunc();
            ins.local_set(C_R);
        }
        // 0.5 rounds away from zero (spec 16.1); wasm's `nearest` rounds to even.
        Rounding::Round => {
            ins.local_get(0);
            ins.f64_trunc();
            ins.local_set(C_R);
            ins.local_get(0);
            ins.local_get(C_R);
            ins.f64_sub();
            ins.local_set(C_F);
            ins.local_get(C_F);
            ins.f64_const(0.5.into());
            ins.f64_ge();
            ins.if_(BlockType::Empty);
            ins.local_get(C_R);
            ins.f64_const(1.0.into());
            ins.f64_add();
            ins.local_set(C_R);
            ins.end();
            ins.local_get(C_F);
            ins.f64_const((-0.5).into());
            ins.f64_le();
            ins.if_(BlockType::Empty);
            ins.local_get(C_R);
            ins.f64_const(1.0.into());
            ins.f64_sub();
            ins.local_set(C_R);
            ins.end();
        }
    }
    // In range? (NaN fails both, exactly like the C runtime.)
    ins.local_get(C_R);
    ins.f64_const((-2147483648.0).into());
    ins.f64_ge();
    ins.local_get(C_R);
    ins.f64_const(2147483647.0.into());
    ins.f64_le();
    ins.i32_and();
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    let prefix = b.string_literal(b"cannot convert ");
    let suffix = b.string_literal(b" to Int");
    ins.local_get(0);
    ins.call(rt(b, "float_to_string"));
    ins.local_set(C_M);
    ins.i32_const(prefix as i32);
    ins.local_get(C_M);
    ins.call(rt(b, "str_append"));
    ins.local_set(C_M2);
    ins.local_get(C_M);
    ins.call(rt(b, "drop"));
    ins.local_get(C_M2);
    ins.i32_const(suffix as i32);
    ins.call(rt(b, "str_append"));
    ins.local_set(C_M3);
    ins.local_get(C_M2);
    ins.call(rt(b, "drop"));
    ins.local_get(C_M3);
    ins.call(rt(b, "raise"));
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(C_R);
    ins.i32_trunc_sat_f64_s();
}
