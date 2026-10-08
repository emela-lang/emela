//! Core IR から JS を出力する．
//!
//! suspend な操作に到達する関数はジェネレータに変換する（仕様 16.5）．
//! JS ランタイムは `include_str!` で埋め込んで出力先に書き出す．
//!
//! 出力は ES モジュール．公開する関数は Emela の名前で `export` する．

mod emit;

use emela_core::Module;

/// JS ランタイムの本文（ES モジュール）．
pub const RUNTIME: &str = include_str!("runtime.mjs");

/// ランタイムを隣に書き出すときの既定のファイル名．
pub const RUNTIME_FILE: &str = "emela_runtime.mjs";

/// ランタイムの持たせ方．
#[derive(Debug, Clone)]
pub enum RuntimeMode {
    /// 出力の先頭にランタイムを埋め込む．1ファイルで完結する．
    Inline,
    /// 同じディレクトリのランタイムを import する．`None` なら `RUNTIME_FILE`．
    /// 書き出しは呼び出し側が `RUNTIME` を使って行う．
    Import(Option<String>),
}

#[derive(Debug, Clone)]
pub struct Options {
    pub runtime: RuntimeMode,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            runtime: RuntimeMode::Import(None),
        }
    }
}

/// モジュールを ES モジュールの JS テキストにする．
///
/// 自己末尾呼び出しのループ化（`emela_core::tail::loopify`）は呼び出し側で先に済ませておく．
/// IR が不正（演算と型の組み合わせ，引数の数など）なら panic する．
pub fn emit_module(module: &Module, options: &Options) -> String {
    emit::emit_module(module, options)
}

/// ランタイムを他のコードに埋め込める形（`export` を外したもの）で返す．
pub fn inline_runtime() -> String {
    let mut out = String::with_capacity(RUNTIME.len());
    for line in RUNTIME.lines() {
        out.push_str(line.strip_prefix("export ").unwrap_or(line));
        out.push('\n');
    }
    out
}
