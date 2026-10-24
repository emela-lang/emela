//! Grapheme cluster boundaries (UAX #29, Unicode 17.0.0).
//!
//! The property tables are in [`super::unicode_tables`], generated from the vendored UCD files
//! (`data/unicode/17.0.0/`, see the README there). They live in static data as sorted
//! `(first, last | class << 24)` pairs and are read with a binary search; the Hangul syllable
//! classes (`LV`/`LVT`) and the default `Other` come from the code point, so they are not stored.
//!
//! `grapheme_end` walks the code points from a boundary and applies GB3 to GB13 of UAX #29,
//! carrying the state the rules need: the previous `Grapheme_Cluster_Break` class, the
//! `Extended_Pictographic` run of GB11, the regional indicator run of GB12/GB13, and the
//! `Indic_Conjunct_Break` chain of GB9c.

use super::string::append_cell;
use super::unicode_tables as t;
use super::*;

/// The `Grapheme_Cluster_Break` classes (the numbers in `unicode_tables`). `Other` is the
/// default 0 and is not named.
const CR: i32 = t::GCB_CR as i32;
const LF: i32 = t::GCB_LF as i32;
const CONTROL: i32 = t::GCB_CONTROL as i32;
const EXTEND: i32 = t::GCB_EXTEND as i32;
const ZWJ: i32 = t::GCB_ZWJ as i32;
const REGIONAL_INDICATOR: i32 = t::GCB_REGIONAL_INDICATOR as i32;
const PREPEND: i32 = t::GCB_PREPEND as i32;
const SPACING_MARK: i32 = t::GCB_SPACING_MARK as i32;
const L: i32 = t::GCB_L as i32;
const V: i32 = t::GCB_V as i32;
const T: i32 = t::GCB_T as i32;
const LV: i32 = t::GCB_LV as i32;
const LVT: i32 = t::GCB_LVT as i32;

/// The `Indic_Conjunct_Break` classes (the numbers in `unicode_tables`).
const INCB_CONSONANT: i32 = t::INCB_CONSONANT as i32;
const INCB_LINKER: i32 = t::INCB_LINKER as i32;
const INCB_EXTEND: i32 = t::INCB_EXTEND as i32;

pub(super) const FUNCS: &[Sig] = &[
    (
        "unicode_class",
        &[ValType::I32, ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("gcb_of", &[ValType::I32], &[ValType::I32]),
    ("ext_pict_of", &[ValType::I32], &[ValType::I32]),
    ("incb_of", &[ValType::I32], &[ValType::I32]),
    (
        "grapheme_end",
        &[ValType::I32, ValType::I32],
        &[ValType::I32],
    ),
    ("str_grapheme_count", &[ValType::I32], &[ValType::I32]),
    ("str_grapheme_chars", &[ValType::I32], &[ValType::I32]),
];

pub(super) fn locals(name: &str) -> Vec<(u32, ValType)> {
    match name {
        // lo, hi, mid, start, packed
        "unicode_class" => i32s(5),
        // n, i, cp, cls, prev, ep, ri, chain, cur, brk, next_ep, ep_char
        "grapheme_end" => i32s(12),
        // n, i, count
        "str_grapheme_count" => i32s(3),
        // n, start, end, head, last, segment, cell
        "str_grapheme_chars" => i32s(7),
        _ => Vec::new(),
    }
}

/// The `(first, last, class)` ranges of a property table.
type Ranges = &'static [(u32, u32, u8)];

/// The names of the property tables in `Builder::rt_data`, with their ranges.
const TABLES: [(&str, Ranges); 3] = [
    ("unicode.gcb", t::GCB),
    ("unicode.extended_pictographic", t::EXTENDED_PICTOGRAPHIC),
    ("unicode.incb", t::INCB),
];

/// The runtime functions the IR calls that reach `grapheme_end` (`str_split` does for an empty
/// separator). A new runtime function that calls into this module from outside is added here.
const ENTRIES: [&str; 3] = ["str_grapheme_count", "str_grapheme_chars", "str_split"];

/// Writes the three property tables into static data and records their offsets.
///
/// The entries are `(first, last | class << 24)`; `last` is at most `0x10FFFF`, so the class
/// fits in the high byte of the second word.
///
/// The tables are about 10 KB, so they are written only when the program calls a function in
/// [`ENTRIES`]. Otherwise the grapheme functions are unreachable and read base 0.
pub(super) fn setup(b: &mut Builder) {
    if !ENTRIES.iter().any(|name| b.rt_used.contains(name)) {
        return;
    }
    for (name, ranges) in TABLES {
        let off = b.alloc_static(ranges.len() as u32 * 8);
        for (i, &(first, last, class)) in ranges.iter().enumerate() {
            let at = off + i as u32 * 8;
            b.write_u32(at, first);
            b.write_u32(at + 4, last | (u32::from(class) << 24));
        }
        b.rt_data.insert(name, off);
    }
}

/// Pushes the arguments of `unicode_class` for the table `name` and `cp` (local 0), and calls it.
///
/// A code point below the first range has class 0 without a search; for the
/// `Extended_Pictographic` and `InCB` tables that covers ASCII.
fn lookup(b: &Builder, ins: &mut InstructionSink, name: &str, ranges: Ranges) {
    let first = ranges.first().map_or(0, |&(first, ..)| first);
    if first > 0 {
        ins.local_get(0);
        ins.i32_const(first as i32);
        ins.i32_lt_u();
        ins.if_(BlockType::Empty);
        ins.i32_const(0);
        ins.return_();
        ins.end();
    }
    ins.i32_const(b.rt_data.get(name).copied().unwrap_or(0) as i32);
    ins.i32_const(ranges.len() as i32);
    ins.local_get(0);
    ins.call(rt(b, "unicode_class"));
}

pub(super) fn emit(b: &mut Builder, name: &str, ins: &mut InstructionSink) -> bool {
    match name {
        "unicode_class" => unicode_class(ins),
        "gcb_of" => gcb_of(b, ins),
        "ext_pict_of" => ext_pict_of(b, ins),
        "incb_of" => incb_of(b, ins),
        "grapheme_end" => grapheme_end(b, ins),
        "str_grapheme_count" => str_grapheme_count(b, ins),
        "str_grapheme_chars" => str_grapheme_chars(b, ins),
        _ => return false,
    }
    true
}

/// Pushes `local == value`.
fn local_eq(ins: &mut InstructionSink, local: u32, value: i32) {
    ins.local_get(local);
    ins.i32_const(value);
    ins.i32_eq();
}

/// Pushes whether `local` is one of `values`.
fn local_in(ins: &mut InstructionSink, local: u32, values: &[i32]) {
    for (i, &value) in values.iter().enumerate() {
        local_eq(ins, local, value);
        if i > 0 {
            ins.i32_or();
        }
    }
}

/// Pushes the class of `cp` in the table at `base` with `count` entries, or 0 if no range has it.
///
/// The entries are `(first, last | class << 24)`, sorted by `first`. The search finds the last
/// entry whose `first` is at most `cp`; `cp` is in that range when it is at most `last`.
fn unicode_class(ins: &mut InstructionSink) {
    const LO: u32 = 3;
    const HI: u32 = 4;
    const MID: u32 = 5;
    const START: u32 = 6;
    const PACKED: u32 = 7;
    ins.i32_const(0);
    ins.local_set(LO);
    ins.local_get(1);
    ins.local_set(HI);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(LO);
    ins.local_get(HI);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(LO);
    ins.local_get(HI);
    ins.i32_add();
    ins.i32_const(1);
    ins.i32_shr_u();
    ins.local_set(MID);
    // `(base + mid * 8).first`
    ins.local_get(0);
    ins.local_get(MID);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(0, 4));
    ins.local_set(START);
    ins.local_get(START);
    ins.local_get(2);
    ins.i32_le_u();
    ins.if_(BlockType::Empty);
    ins.local_get(MID);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(LO);
    ins.else_();
    ins.local_get(MID);
    ins.local_set(HI);
    ins.end();
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(LO);
    ins.i32_eqz();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.return_();
    ins.end();
    ins.local_get(LO);
    ins.i32_const(1);
    ins.i32_sub();
    ins.local_set(MID);
    // `(base + mid * 8).packed`
    ins.local_get(0);
    ins.local_get(MID);
    ins.i32_const(3);
    ins.i32_shl();
    ins.i32_add();
    ins.i32_load(mem(4, 4));
    ins.local_set(PACKED);
    ins.local_get(2);
    ins.local_get(PACKED);
    ins.i32_const(0x00FF_FFFF);
    ins.i32_and();
    ins.i32_le_u();
    ins.if_(BlockType::Result(ValType::I32));
    ins.local_get(PACKED);
    ins.i32_const(24);
    ins.i32_shr_u();
    ins.else_();
    ins.i32_const(0);
    ins.end();
}

/// The `Grapheme_Cluster_Break` class of `cp` (UAX #29).
///
/// ASCII is computed without a search: CR, LF, `Control` below 0x20 and at 0x7F, `Other`
/// elsewhere.
fn gcb_of(b: &mut Builder, ins: &mut InstructionSink) {
    ins.local_get(0);
    ins.i32_const(0x80);
    ins.i32_lt_u();
    ins.if_(BlockType::Empty);
    ins.i32_const(CR);
    ins.i32_const(LF);
    ins.i32_const(CONTROL);
    ins.i32_const(0);
    ins.local_get(0);
    ins.i32_const(0x20);
    ins.i32_lt_u();
    local_eq(ins, 0, 0x7F);
    ins.i32_or();
    ins.select();
    local_eq(ins, 0, 0x0A);
    ins.select();
    local_eq(ins, 0, 0x0D);
    ins.select();
    ins.return_();
    ins.end();
    ins.local_get(0);
    ins.i32_const(0xAC00);
    ins.i32_ge_u();
    ins.local_get(0);
    ins.i32_const(0xD7A3);
    ins.i32_le_u();
    ins.i32_and();
    ins.if_(BlockType::Empty);
    // A Hangul syllable is `LV` when its index is 0 mod 28, `LVT` otherwise.
    ins.local_get(0);
    ins.i32_const(0xAC00);
    ins.i32_sub();
    ins.i32_const(28);
    ins.i32_rem_u();
    ins.i32_eqz();
    ins.if_(BlockType::Result(ValType::I32));
    ins.i32_const(LV);
    ins.else_();
    ins.i32_const(LVT);
    ins.end();
    ins.return_();
    ins.end();
    lookup(b, ins, "unicode.gcb", t::GCB);
}

/// The `Extended_Pictographic` property of `cp`: 1 or 0 (the class in the table is always 1).
fn ext_pict_of(b: &mut Builder, ins: &mut InstructionSink) {
    lookup(
        b,
        ins,
        "unicode.extended_pictographic",
        t::EXTENDED_PICTOGRAPHIC,
    );
}

/// The `Indic_Conjunct_Break` class of `cp`: `INCB_CONSONANT`, `INCB_LINKER`, `INCB_EXTEND` or 0.
fn incb_of(b: &mut Builder, ins: &mut InstructionSink) {
    lookup(b, ins, "unicode.incb", t::INCB);
}

/// `grapheme_end(s, start)`: the byte offset where the cluster that starts at `start` ends.
fn grapheme_end(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 2;
    const I: u32 = 3;
    const CP: u32 = 4;
    const CLS: u32 = 5;
    const PREV: u32 = 6;
    const EP: u32 = 7;
    const RI: u32 = 8;
    const CHAIN: u32 = 9;
    const CUR: u32 = 10;
    const BRK: u32 = 11;
    const NEXT_EP: u32 = 12;
    const EP_CHAR: u32 = 13;
    let utf8_cp = rt(b, "utf8_cp");
    let (gcb_of, incb_of, ext_pict_of) = (rt(b, "gcb_of"), rt(b, "incb_of"), rt(b, "ext_pict_of"));
    let state = BreakState {
        prev: PREV,
        cls: CLS,
        ep: EP,
        ri: RI,
        chain: CHAIN,
        cur: CUR,
        ep_char: EP_CHAR,
    };
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.local_get(1);
    ins.local_set(I);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.if_(BlockType::Empty);
    ins.local_get(N);
    ins.return_();
    ins.end();
    // The state after the first code point of the cluster.
    ins.local_get(0);
    ins.local_get(I);
    ins.call(utf8_cp);
    ins.local_set(CP);
    ins.local_get(CP);
    ins.call(gcb_of);
    ins.local_set(CLS);
    ins.local_get(CLS);
    ins.local_set(PREV);
    ins.local_get(CP);
    ins.call(ext_pict_of);
    ins.local_set(EP);
    local_eq(ins, CLS, REGIONAL_INDICATOR);
    ins.local_set(RI);
    ins.local_get(CP);
    ins.call(incb_of);
    ins.i32_const(INCB_CONSONANT);
    ins.i32_eq();
    ins.local_set(CHAIN);
    advance(ins, I, CP);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(I);
    ins.call(utf8_cp);
    ins.local_set(CP);
    ins.local_get(CP);
    ins.call(gcb_of);
    ins.local_set(CLS);
    // The rules read `InCB` only after a consonant and a linker (GB9c), and
    // `Extended_Pictographic` only after an emoji's ZWJ (GB11); look them up here only then.
    lookup_when(ins, CHAIN, 2, true, CP, incb_of, CUR);
    lookup_when(ins, EP, 2, true, CP, ext_pict_of, EP_CHAR);
    // GB999 breaks; the rules below say when not to.
    ins.i32_const(1);
    ins.local_set(BRK);
    // GB3: CR x LF.
    local_eq(ins, PREV, CR);
    local_eq(ins, CLS, LF);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(BRK);
    ins.else_();
    // GB4 and GB5: break before and after Control, CR and LF.
    local_in(ins, PREV, &[CONTROL, CR, LF]);
    local_in(ins, CLS, &[CONTROL, CR, LF]);
    ins.i32_or();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(BRK);
    ins.else_();
    no_break_rules(ins, &state);
    ins.if_(BlockType::Empty);
    ins.i32_const(0);
    ins.local_set(BRK);
    ins.end();
    ins.end();
    ins.end();
    ins.local_get(BRK);
    ins.if_(BlockType::Empty);
    ins.local_get(I);
    ins.return_();
    ins.end();
    // No break: the current code point joins the cluster, so fold it into the state, with the
    // properties the rules did not look up.
    lookup_when(ins, CHAIN, 2, false, CP, incb_of, CUR);
    lookup_when(ins, EP, 2, false, CP, ext_pict_of, EP_CHAR);
    // GB11: remember `Extended_Pictographic Extend*` and then the ZWJ.
    ins.local_get(EP_CHAR);
    ins.local_set(NEXT_EP);
    local_eq(ins, CLS, EXTEND);
    local_eq(ins, EP, 1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(NEXT_EP);
    ins.end();
    local_eq(ins, CLS, ZWJ);
    local_eq(ins, EP, 1);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(NEXT_EP);
    ins.end();
    ins.local_get(NEXT_EP);
    ins.local_set(EP);
    // GB12/GB13: the length of the run of regional indicators ending here.
    local_eq(ins, CLS, REGIONAL_INDICATOR);
    ins.if_(BlockType::Empty);
    ins.local_get(RI);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(RI);
    ins.else_();
    ins.i32_const(0);
    ins.local_set(RI);
    ins.end();
    // GB9c: `Consonant [Extend Linker]* Linker [Extend Linker]*` before the next Consonant.
    local_eq(ins, CUR, INCB_CONSONANT);
    ins.if_(BlockType::Empty);
    ins.i32_const(1);
    ins.local_set(CHAIN);
    ins.else_();
    ins.local_get(CHAIN);
    ins.i32_const(1);
    ins.i32_ge_u();
    local_in(ins, CUR, &[INCB_LINKER, INCB_EXTEND]);
    ins.i32_and();
    ins.if_(BlockType::Empty);
    local_eq(ins, CUR, INCB_LINKER);
    ins.if_(BlockType::Empty);
    ins.i32_const(2);
    ins.local_set(CHAIN);
    ins.end();
    ins.else_();
    ins.i32_const(0);
    ins.local_set(CHAIN);
    ins.end();
    ins.end();
    ins.local_get(CLS);
    ins.local_set(PREV);
    advance(ins, I, CP);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(N);
}

/// `dst = f(cp)` when `local == value` is `when`; otherwise, with `when` true, `dst = 0`.
///
/// The pair of calls with `when` true and then false looks the property up exactly once.
fn lookup_when(
    ins: &mut InstructionSink,
    local: u32,
    value: i32,
    when: bool,
    cp: u32,
    f: u32,
    dst: u32,
) {
    local_eq(ins, local, value);
    if !when {
        ins.i32_eqz();
    }
    ins.if_(BlockType::Empty);
    ins.local_get(cp);
    ins.call(f);
    ins.local_set(dst);
    if when {
        ins.else_();
        ins.i32_const(0);
        ins.local_set(dst);
    }
    ins.end();
}

/// The locals of `grapheme_end` that the rules read.
struct BreakState {
    prev: u32,
    cls: u32,
    /// GB11: 0 none, 1 after `Extended_Pictographic Extend*`, 2 after its ZWJ.
    ep: u32,
    /// GB12/GB13: the length of the regional indicator run ending at `prev`.
    ri: u32,
    /// GB9c: 0 none, 1 after a consonant, 2 after a consonant and a linker.
    chain: u32,
    cur: u32,
    /// `Extended_Pictographic` of the current code point: 1 or 0.
    ep_char: u32,
}

/// Pushes whether GB6 to GB13 say "no break" between `prev` and `cls`.
fn no_break_rules(ins: &mut InstructionSink, state: &BreakState) {
    let BreakState {
        prev,
        cls,
        ep,
        ri,
        chain,
        cur,
        ep_char,
    } = *state;
    // GB6: L x (L | V | LV | LVT).
    local_eq(ins, prev, L);
    local_in(ins, cls, &[L, V, LV, LVT]);
    ins.i32_and();
    // GB7: (LV | V) x (V | T).
    local_in(ins, prev, &[LV, V]);
    local_in(ins, cls, &[V, T]);
    ins.i32_and();
    ins.i32_or();
    // GB8: (LVT | T) x T.
    local_in(ins, prev, &[LVT, T]);
    local_eq(ins, cls, T);
    ins.i32_and();
    ins.i32_or();
    // GB9: x (Extend | ZWJ). GB9a: x SpacingMark. GB9b: Prepend x.
    local_in(ins, cls, &[EXTEND, ZWJ, SPACING_MARK]);
    ins.i32_or();
    local_eq(ins, prev, PREPEND);
    ins.i32_or();
    // GB9c: a consonant after a consonant and a linker, with only Extend/Linker between.
    local_eq(ins, cur, INCB_CONSONANT);
    local_eq(ins, chain, 2);
    ins.i32_and();
    ins.i32_or();
    // GB11: Extended_Pictographic Extend* ZWJ x Extended_Pictographic.
    local_eq(ins, ep, 2);
    ins.local_get(ep_char);
    ins.i32_and();
    ins.i32_or();
    // GB12/GB13: an even number of regional indicators before this one.
    local_eq(ins, prev, REGIONAL_INDICATOR);
    local_eq(ins, cls, REGIONAL_INDICATOR);
    ins.i32_and();
    ins.local_get(ri);
    ins.i32_const(1);
    ins.i32_and();
    ins.i32_and();
    ins.i32_or();
}

/// `i += ` the UTF-8 length of `cp`, the code point already decoded at `i`:
/// `1 + (cp >= 0x80) + (cp >= 0x800) + (cp >= 0x10000)`.
fn advance(ins: &mut InstructionSink, i: u32, cp: u32) {
    ins.local_get(i);
    ins.i32_const(1);
    ins.i32_add();
    for bound in [0x80, 0x800, 0x1_0000] {
        ins.local_get(cp);
        ins.i32_const(bound);
        ins.i32_ge_u();
        ins.i32_add();
    }
    ins.local_set(i);
}

/// `String.length`: the number of grapheme clusters.
fn str_grapheme_count(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    const I: u32 = 2;
    const COUNT: u32 = 3;
    let grapheme_end = rt(b, "grapheme_end");
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(I);
    ins.i32_const(0);
    ins.local_set(COUNT);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(I);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(I);
    ins.call(grapheme_end);
    ins.local_set(I);
    ins.local_get(COUNT);
    ins.i32_const(1);
    ins.i32_add();
    ins.local_set(COUNT);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(COUNT);
}

/// `String.chars`: a `List[String]` with one string per grapheme cluster.
fn str_grapheme_chars(b: &mut Builder, ins: &mut InstructionSink) {
    const N: u32 = 1;
    const START: u32 = 2;
    const END: u32 = 3;
    const HEAD: u32 = 4;
    const LAST: u32 = 5;
    const SEGMENT: u32 = 6;
    const CELL: u32 = 7;
    let (grapheme_end, str_slice, cons) =
        (rt(b, "grapheme_end"), rt(b, "str_slice"), rt(b, "cons"));
    let cons_desc = b.cons_ref_desc;
    ins.local_get(0);
    ins.i32_load(mem(HDR, 4));
    ins.local_set(N);
    ins.i32_const(0);
    ins.local_set(START);
    ins.i32_const(0);
    ins.local_set(HEAD);
    ins.i32_const(0);
    ins.local_set(LAST);
    ins.block(BlockType::Empty);
    ins.loop_(BlockType::Empty);
    ins.local_get(START);
    ins.local_get(N);
    ins.i32_ge_u();
    ins.br_if(1);
    ins.local_get(0);
    ins.local_get(START);
    ins.call(grapheme_end);
    ins.local_set(END);
    ins.local_get(0);
    ins.local_get(START);
    ins.local_get(END);
    ins.local_get(START);
    ins.i32_sub();
    ins.call(str_slice);
    ins.local_set(SEGMENT);
    ins.local_get(SEGMENT);
    ins.i32_const(0);
    ins.i32_const(cons_desc as i32);
    ins.call(cons);
    ins.local_set(CELL);
    append_cell(ins, HEAD, LAST, CELL);
    ins.local_get(CELL);
    ins.local_set(LAST);
    ins.local_get(END);
    ins.local_set(START);
    ins.br(0);
    ins.end();
    ins.end();
    ins.local_get(HEAD);
}

#[cfg(test)]
#[path = "../../../tests/grapheme_break_test/mod.rs"]
mod grapheme_break_test;

#[cfg(test)]
mod tests {
    //! A reference implementation of the rules, over the same tables, checked against every case
    //! of the official `GraphemeBreakTest.txt`.
    //!
    //! It checks the tables and the reading of the rules without wasmtime. The emitted
    //! `grapheme_end` is checked against the same cases by `tests/unicode.rs`, which CI always
    //! runs (see `wasmtime()` in `tests/common`).

    use super::*;

    /// The class of `cp` in a table, or 0 when no range has it.
    fn class(ranges: &[(u32, u32, u8)], cp: u32) -> u8 {
        ranges
            .binary_search_by(|&(first, last, _)| {
                if cp < first {
                    std::cmp::Ordering::Greater
                } else if cp > last {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .map_or(0, |i| ranges[i].2)
    }

    fn gcb(cp: u32) -> u8 {
        if (0xAC00..=0xD7A3).contains(&cp) {
            return if (cp - 0xAC00).is_multiple_of(28) {
                t::GCB_LV
            } else {
                t::GCB_LVT
            };
        }
        class(t::GCB, cp)
    }

    fn ext_pict(cp: u32) -> bool {
        class(t::EXTENDED_PICTOGRAPHIC, cp) != 0
    }

    fn incb(cp: u32) -> u8 {
        class(t::INCB, cp)
    }

    /// The state the rules carry from the code points already in the cluster.
    #[derive(Clone, Copy)]
    struct State {
        prev: u8,
        /// GB11: 0 none, 1 after `Extended_Pictographic Extend*`, 2 after its ZWJ.
        ep: u8,
        /// GB12/GB13: the length of the regional indicator run ending at `prev`.
        ri: u8,
        /// GB9c: 0 none, 1 after a consonant, 2 after a consonant and a linker.
        chain: u8,
    }

    fn state_after(cp: u32) -> State {
        let prev = gcb(cp);
        State {
            prev,
            ep: u8::from(ext_pict(cp)),
            ri: u8::from(prev == t::GCB_REGIONAL_INDICATOR),
            chain: u8::from(incb(cp) == t::INCB_CONSONANT),
        }
    }

    fn control(cls: u8) -> bool {
        [t::GCB_CONTROL, t::GCB_CR, t::GCB_LF].contains(&cls)
    }

    /// The byte offsets where the clusters of `s` start, ending with `s.len()`.
    fn boundaries(s: &str) -> Vec<usize> {
        let mut out = vec![0];
        let mut chars = s.char_indices();
        let Some((_, first)) = chars.next() else {
            return out;
        };
        let mut st = state_after(first as u32);
        for (off, ch) in chars {
            let cp = ch as u32;
            let cls = gcb(cp);
            let cur = incb(cp);
            // GB3 to GB13: a break unless one of the rules says no break.
            let no_break = (st.prev == t::GCB_CR && cls == t::GCB_LF)
                || (!control(st.prev)
                    && !control(cls)
                    && // GB6: L x (L | V | LV | LVT)
                    ((st.prev == t::GCB_L
                        && [t::GCB_L, t::GCB_V, t::GCB_LV, t::GCB_LVT].contains(&cls))
                        // GB7: (LV | V) x (V | T)
                        || ([t::GCB_LV, t::GCB_V].contains(&st.prev)
                            && [t::GCB_V, t::GCB_T].contains(&cls))
                        // GB8: (LVT | T) x T
                        || ([t::GCB_LVT, t::GCB_T].contains(&st.prev) && cls == t::GCB_T)
                        // GB9: x (Extend | ZWJ). GB9a: x SpacingMark. GB9b: Prepend x
                        || [t::GCB_EXTEND, t::GCB_ZWJ, t::GCB_SPACING_MARK].contains(&cls)
                        || st.prev == t::GCB_PREPEND
                        // GB9c: a consonant after a consonant and a linker
                        || (cur == t::INCB_CONSONANT && st.chain == 2)
                        // GB11: Extended_Pictographic Extend* ZWJ x Extended_Pictographic
                        || (st.ep == 2 && ext_pict(cp))
                        // GB12/GB13: an even number of regional indicators before this one
                        || (st.prev == t::GCB_REGIONAL_INDICATOR
                            && cls == t::GCB_REGIONAL_INDICATOR
                            && st.ri % 2 == 1)));
            if !no_break {
                out.push(off);
                st = state_after(cp);
                continue;
            }
            st.ep = if ext_pict(cp) || (cls == t::GCB_EXTEND && st.ep == 1) {
                1
            } else if cls == t::GCB_ZWJ && st.ep == 1 {
                2
            } else {
                0
            };
            st.ri = if cls == t::GCB_REGIONAL_INDICATOR {
                st.ri + 1
            } else {
                0
            };
            st.chain = if cur == t::INCB_CONSONANT {
                1
            } else if st.chain >= 1 && (cur == t::INCB_LINKER || cur == t::INCB_EXTEND) {
                if cur == t::INCB_LINKER { 2 } else { st.chain }
            } else {
                0
            };
            st.prev = cls;
        }
        out.push(s.len());
        out
    }

    #[test]
    fn grapheme_break_test_17_0_0() {
        let mut failures = Vec::new();
        let cases = grapheme_break_test::cases();
        for (n, graphemes) in cases.iter().enumerate() {
            let s = graphemes.concat();
            let want: Vec<usize> = std::iter::once(0)
                .chain(graphemes.iter().scan(0, |end, g| {
                    *end += g.len();
                    Some(*end)
                }))
                .collect();
            let got = boundaries(&s);
            if got != want {
                failures.push(format!("case {n}: {s:?}: want {want:?}, got {got:?}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} cases failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn property_tables_are_sorted_and_disjoint() {
        for (name, ranges) in [
            ("GCB", t::GCB),
            ("Extended_Pictographic", t::EXTENDED_PICTOGRAPHIC),
            ("InCB", t::INCB),
        ] {
            for pair in ranges.windows(2) {
                assert!(
                    pair[0].1 < pair[1].0,
                    "{name} is not sorted and disjoint: {pair:?}"
                );
            }
        }
    }

    #[test]
    fn ascii_shortcuts_agree_with_the_tables() {
        // `gcb_of` computes ASCII without the table.
        for cp in 0..0x80 {
            let want = match cp {
                0x0D => t::GCB_CR,
                0x0A => t::GCB_LF,
                0..0x20 | 0x7F => t::GCB_CONTROL,
                _ => 0,
            };
            assert_eq!(class(t::GCB, cp), want, "GCB of {cp:#x}");
        }
        // `lookup` answers 0 below the first range without a search; that must cover ASCII.
        assert!(t::EXTENDED_PICTOGRAPHIC[0].0 >= 0x80);
        assert!(t::INCB[0].0 >= 0x80);
    }

    #[test]
    fn extended_pictographic_class_is_one() {
        // `grapheme_end` uses the class as the 0/1 property without comparing it.
        assert!(
            t::EXTENDED_PICTOGRAPHIC
                .iter()
                .all(|&(.., class)| class == 1)
        );
    }
}
