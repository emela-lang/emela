//! 線形メモリの唯一の定義（スクラッチ < 静的データ < ヒープ）.
//!
//! 線形メモリは 3 つの領域に分かれる。
//!
//! ```text
//! [ スクラッチ ][ 静的データ ][ ヒープ → grow ]
//! 0            SCRATCH_END   heap_base
//! ```
//!
//! 各領域の境界をここで一箇所で定義する。スクラッチのスロットは `scratch!` マクロで
//! 宣言し，領域の末尾（`SCRATCH_END`）はその表から導出する。スロットを足せば末尾が
//! 自動で伸びるので，静的データとの重なりは起こらない。
//!
//! 出力モジュールはこの定義から決まるため，このファイルを変えずにスロットを
//! 勝手に増やすとコンパイル時 assert か `tests/layout.rs` が落ちる。

/// バンプアロケータが伸びるページ。
pub const PAGE: u32 = 65536;

const fn align8(x: u32) -> u32 {
    (x + 7) & !7
}

/// 各スロットの末尾の最大値を 8 バイトに丸める。
const fn max_align8(ends: &[u32]) -> u32 {
    let mut m = 0u32;
    let mut i = 0;
    while i < ends.len() {
        if ends[i] > m {
            m = ends[i];
        }
        i += 1;
    }
    align8(m)
}

/// スクラッチのスロットをここに宣言する。オフセットは wasm に定数として焼き込むので手で
/// 決めるが，領域の末尾（`SCRATCH_END`）はこの表から導出する。
macro_rules! scratch {
    ($($name:ident : $off:expr, $len:expr;)*) => {
        $(pub const $name: u32 = $off;)*
        pub const SCRATCH_END: u32 = max_align8(&[$($off + $len),*]);
    }
}

scratch! {
    IOVEC:    0,   8;   // WASI の iovec
    NWRITTEN: 8,   8;   // fd_write が書いたバイト数
    DIGITS:   64,  16;  // Int の桁（下位から）
    NEWLINE:  88,  1;   // debug_print の改行
    DIGITS64: 96,  24;  // Int64 の桁
    JSONBUF:  128, 6;   // JSON escape の短いバッファ
    CPBUF:    160, 4;   // 1 コードポイントの UTF-8
}

/// 静的データの開始（スクラッチの直後）。
pub const STATIC_START: u32 = SCRATCH_END;

/// 静的データを積んだ後のヒープ開始。
pub(crate) fn heap_base(statics_len: u32) -> u32 {
    align8(statics_len).max(STATIC_START) + 8
}

/// 確定した配置（テストと `WasmOutput` が使う）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub scratch: Region,
    pub statics: Region,
    pub heap_base: u32,
}

impl Layout {
    pub(crate) fn new(statics_len: u32) -> Self {
        Layout {
            scratch: Region {
                start: 0,
                end: SCRATCH_END,
            },
            statics: Region {
                start: STATIC_START,
                end: statics_len,
            },
            heap_base: heap_base(statics_len),
        }
    }
}

// 不変条件のコンパイル時チェック
const _: () = assert!(STATIC_START.is_multiple_of(8));
const _: () = assert!(STATIC_START >= SCRATCH_END);
