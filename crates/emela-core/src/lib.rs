//! Core IR の定義と，型付き構文木からの変換．
//!
//! derive の展開，自己末尾呼び出しのループ化（仕様 6.8），layer の配線（8.5）をここで済ませ，
//! バックエンドには脱糖済みの IR だけを渡す．

pub mod build;
mod ir;
pub mod named;
pub mod tail;

#[cfg(test)]
mod tests;

pub use ir::*;
