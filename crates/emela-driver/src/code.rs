//! エラーコード．`E` と4桁（警告は `W` と4桁）．一度振った番号は使い回さない．
//!
//! 帯は段ごとに分ける: E01xx 字句・構文，E02xx モジュール・import・プロジェクトの構成，
//! E03xx 型，E04xx effect，E09xx ツール（入出力，node など）．
//! 表は仕様の付録 A が持つ．ここはそれを写したもの．E01xx は emela-syntax の `DiagnosticCode` が持つ．

// E02xx: モジュール・import・プロジェクトの構成
pub const INVALID_FILE_NAME: &str = "E0201";
pub const INVALID_DIR_NAME: &str = "E0202";
pub const RESERVED_MODULE_NAME: &str = "E0203";
pub const DUPLICATE_MODULE: &str = "E0204";
pub const UNDEFINED_MODULE: &str = "E0205";
pub const IMPORT_CYCLE: &str = "E0206";
pub const NO_PROJECT_FILE: &str = "E0207";
pub const OUTSIDE_SOURCE_ROOT: &str = "E0208";
pub const ENTRY_NOT_FOUND: &str = "E0209";
pub const ENTRY_NOT_MODULE: &str = "E0210";

// E03xx: 型
pub const TYPE_MISMATCH: &str = "E0301";
pub const INFINITE_TYPE: &str = "E0302";
pub const NON_EXHAUSTIVE_MATCH: &str = "E0303";
pub const UNREACHABLE_MATCH_ARM: &str = "W0301";

// E09xx: ツール
pub const PATH_NOT_FOUND: &str = "E0901";
pub const CANNOT_READ: &str = "E0902";
pub const CANNOT_WRITE_OUTPUT: &str = "E0903";
pub const NODE_UNAVAILABLE: &str = "E0904";
pub const NO_JS_BACKEND: &str = "E0905";
pub const INVALID_JS_OUTPUT: &str = "E0906";
