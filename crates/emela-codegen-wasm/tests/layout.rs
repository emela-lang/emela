//! 出力モジュールを `wasmparser` で読み，`WasmOutput.layout` と突き合わせて
//! 線形メモリ配置の不変条件を検証する。`wasmtime` は不要（コンパイルと
//! モジュール解析だけで走る）.
//!
//! 出力は WASI 0.3 の component なので，メモリ・データ・グローバルの各節は
//! component に埋め込まれた core module の中にある。

mod common;
use common::*;
use emela_codegen_wasm::layout;
use wasmparser::{DataKind, Operator, Parser, Payload, ValType};

/// `src` をコンパイルし，wasm バイト列と `Layout` を返す。
fn module_and_layout(src: &str) -> (Vec<u8>, layout::Layout) {
    let output = to_output(src);
    (output.module, output.layout)
}

/// component の中の core module まで降りて `Payload` を渡す。`f` が `true` を返したら止める。
///
/// `Parser::parse_all` は `ModuleSection` の parser に切り替えて，その中の節も順に返す。
fn find_payload(bytes: &[u8], f: &mut impl FnMut(Payload<'_>) -> bool) -> bool {
    for payload in Parser::new(0).parse_all(bytes) {
        if f(payload.expect("wasm should parse")) {
            return true;
        }
    }
    false
}

/// `ConstExpr` が `i32.const N` 1 オペレータなら `Some(N)` を返す。
fn const_i32(init: &wasmparser::ConstExpr<'_>) -> Option<i32> {
    let mut ops = init.get_operators_reader().into_iter();
    let op = ops.next()?.ok()?;
    match op {
        Operator::I32Const { value } => Some(value),
        _ => None,
    }
}

#[test]
fn scratch_static_heap_do_not_overlap() {
    let (_bytes, layout) = module_and_layout("fn main() { dbg(1) }\n");

    // スクラッチ [0, SCRATCH_END) < 静的データ [STATIC_START, data.len()) < ヒープ heap_base 以上，
    // いずれも 8 バイト整列。重なりがあってはならない。
    assert_eq!(layout.scratch.start, 0);
    assert_eq!(layout.scratch.end, layout::SCRATCH_END);
    assert_eq!(layout.statics.start, layout::STATIC_START);
    assert!(layout.scratch.end <= layout.statics.start);
    assert!(layout.statics.start <= layout.statics.end);
    assert!(layout.statics.end <= layout.heap_base);
    assert_eq!(layout.scratch.start % 8, 0);
    assert_eq!(layout.scratch.end % 8, 0);
    assert_eq!(layout.heap_base % 8, 0);
    // #257 と同値：SCRATCH_END == STATIC_START == 168 を保つ（出力不変の前提）.
    assert_eq!(layout::SCRATCH_END, 168);
    assert_eq!(layout::STATIC_START, 168);
}

#[test]
fn scratch_slots_fit_in_scratch() {
    let (_bytes, _layout) = module_and_layout("fn main() { dbg(1) }\n");

    // 各スロット [off, off + len) はスクラッチ領域に収まる。
    // スロットを足すと SCRATCH_END が自動で伸びるので，ここを更新し忘れると落ちる.
    let slots = [
        (layout::WAITBUF, 8),
        (layout::FUTUREBUF, 8),
        (layout::DIGITS, 16),
        (layout::NEWLINE, 1),
        (layout::DIGITS64, 24),
        (layout::JSONBUF, 6),
        (layout::CPBUF, 4),
    ];
    for (off, len) in slots {
        assert!(
            off < layout::SCRATCH_END,
            "slot at offset {off} is beyond scratch end",
        );
        assert!(
            off + len <= layout::SCRATCH_END,
            "slot {off}..{} exceeds scratch end {}",
            off + len,
            layout::SCRATCH_END,
        );
    }
}

#[test]
fn data_section_matches_layout() {
    let (bytes, layout) = module_and_layout("fn main() { dbg(1) }\n");

    let mut data_len = None;
    find_payload(&bytes, &mut |payload| {
        let Payload::DataSection(section) = payload else {
            return false;
        };
        let mut segments = section.into_iter();
        let seg = segments
            .next()
            .expect("data section has a segment")
            .expect("data segment parses");
        // 先頭セグメントは memory 0 へ offset i32.const 0 でアクティブ
        match seg.kind {
            DataKind::Active {
                memory_index,
                offset_expr,
            } => {
                assert_eq!(memory_index, 0);
                assert_eq!(const_i32(&offset_expr), Some(0));
            }
            other => panic!("expected an active data segment, got {other:?}"),
        }
        data_len = Some(seg.data.len());
        // データセグメントの先頭はスクラッチ領域（全て 0 で予約済み）
        let scratch_bytes = seg
            .data
            .get(..layout.scratch.end as usize)
            .expect("data covers scratch");
        for &byte in scratch_bytes {
            assert_eq!(byte, 0, "scratch area must be zeroed");
        }
        true
    });
    // データセグメントの長さ = statics.end （データ全体 = スクラッチ予約 + 静的データ）.
    assert_eq!(data_len, Some(layout.statics.end as usize));
}

#[test]
fn heap_global_matches_layout() {
    let (bytes, layout) = module_and_layout("fn main() { dbg(1) }\n");

    let mut found = false;
    find_payload(&bytes, &mut |payload| {
        let Payload::GlobalSection(section) = payload else {
            return false;
        };
        let mut globals = section.into_iter();
        let g = globals
            .next()
            .expect("global section has a global")
            .expect("global parses");
        // G_HEAP == 0 は最初のグローバル，init は heap_base
        assert_eq!(g.ty.content_type, ValType::I32);
        assert_eq!(
            const_i32(&g.init_expr),
            Some(layout.heap_base as i32),
            "heap global init must equal heap_base",
        );
        found = true;
        true
    });
    assert!(found, "global section not found");
}

#[test]
fn memory_minimum_covers_heap_base() {
    let (bytes, layout) = module_and_layout("fn main() { dbg(1) }\n");

    let mut found = false;
    find_payload(&bytes, &mut |payload| {
        let Payload::MemorySection(section) = payload else {
            return false;
        };
        let mut memories = section.into_iter();
        let mem = memories
            .next()
            .expect("memory section has a memory")
            .expect("memory parses");
        // 確保ページ数 * PAGE >= heap_base
        assert!(
            mem.initial * (layout::PAGE as u64) >= layout.heap_base as u64,
            "memory minimum {} pages must cover heap_base {}",
            mem.initial,
            layout.heap_base,
        );
        found = true;
        true
    });
    assert!(found, "memory section not found");
}
