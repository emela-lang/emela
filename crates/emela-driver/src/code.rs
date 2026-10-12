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
// 名前解決
pub const UNDEFINED_NAME: &str = "E0211";
pub const UNDEFINED_MEMBER: &str = "E0212";
pub const DUPLICATE_DEFINITION: &str = "E0213";
pub const PRIVATE_ITEM: &str = "E0214";
pub const OPAQUE_CONSTRUCTION: &str = "E0215";
pub const OPAQUE_PATTERN: &str = "E0216";
pub const WRONG_KIND_OF_NAME: &str = "E0217";
pub const AMBIGUOUS_NAME: &str = "E0218";
pub const SELF_OUTSIDE: &str = "E0219";
pub const DUPLICATE_BINDING: &str = "E0220";
pub const EFFECT_AS_TYPE: &str = "E0221";
pub const INVALID_UPDATE_TARGET: &str = "E0222";
pub const POSITIONAL_IN_UPDATE: &str = "E0223";
pub const INTRINSIC_OUTSIDE_CORE: &str = "E0224";
pub const MISPLACED_INTRINSIC: &str = "E0225";
// テスト（13.1）
pub const INVALID_TEST: &str = "E0226";
pub const TEST_FN_OUTSIDE_TESTS: &str = "E0227";
// core の中からだけ参照できる fn（`@internal`）
pub const INTERNAL_FN_OUTSIDE_CORE: &str = "E0228";
pub const MISPLACED_INTERNAL: &str = "E0229";
// `emela run` のエントリに main がない（8.3）
pub const ENTRY_WITHOUT_MAIN: &str = "E0230";
pub const SHADOWED_BY_TYPE_PARAM: &str = "W0201";
pub const SHADOWS_PRELUDE: &str = "W0202";

// E03xx: 型
pub const TYPE_MISMATCH: &str = "E0301";
pub const INFINITE_TYPE: &str = "E0302";
pub const NON_EXHAUSTIVE_MATCH: &str = "E0303";
pub const WRONG_TYPE_ARG_COUNT: &str = "E0304";
pub const WRONG_ARG_COUNT: &str = "E0305";
pub const UNKNOWN_LABEL: &str = "E0306";
pub const DUPLICATE_ARG: &str = "E0307";
pub const MISSING_PATTERN_FIELDS: &str = "E0308";
pub const PATTERN_FIELD_COUNT: &str = "E0309";
pub const NOT_NUMERIC: &str = "E0310";
pub const INT_LITERAL_OUT_OF_RANGE: &str = "E0311";
pub const NO_FIELD: &str = "E0312";
pub const NO_FIELDS: &str = "E0313";
pub const FIELD_ON_UNKNOWN_TYPE: &str = "E0314";
pub const OPAQUE_FIELD: &str = "E0315";
pub const MISSING_PARAM_TYPE: &str = "E0316";
pub const REFUTABLE_BINDING: &str = "E0317";
// エラー（7章）
pub const UNHANDLED_ERRORS: &str = "E0318";
pub const NOT_AN_ERROR: &str = "E0319";
pub const ESCAPE_ARM_NOT_ERROR: &str = "E0320";
pub const ERROR_SET_MISMATCH: &str = "E0321";
// 効果への制約（9.5）
pub const IMMEDIATE_ON_NON_EFFECT: &str = "E0322";
// 宣言の型引数の集合（9.2）
pub const SET_ARG_MISMATCH: &str = "E0323";
pub const SET_AS_TYPE: &str = "E0324";
pub const UNREACHABLE_MATCH_ARM: &str = "W0301";
pub const UNRAISED_ERROR: &str = "W0302";

// E04xx: effect，handler（8章）
pub const UNKNOWN_OPERATION: &str = "E0401";
pub const UNHANDLED_EFFECTS: &str = "E0402";
pub const EFFECT_SET_MISMATCH: &str = "E0403";
pub const MISSING_HANDLER_OPS: &str = "E0404";
pub const DUPLICATE_HANDLER_OP: &str = "E0405";
pub const HANDLER_OP_ARG_COUNT: &str = "E0406";
pub const EXTERNAL_OUTSIDE_HANDLER: &str = "E0407";
pub const CAPABILITY_ARGUMENT: &str = "E0408";
pub const CAPABILITY_IN_DATA: &str = "E0409";
pub const CAPABILITY_RETURNED: &str = "E0410";
pub const HANDLER_CYCLE: &str = "E0411";
// 外部関数（12.1）．effect と同じく副作用の経路なので E04xx に置く（E05xx〜E08xx は予約）．
pub const MISSING_EXTERNAL_TARGET: &str = "E0412";
// suspend と Async（8.7，9.5）
pub const ASYNC_HANDLER: &str = "E0413";
pub const USE_ASYNC: &str = "E0414";
pub const SUSPEND_MISMATCH: &str = "E0415";
pub const NOT_IMMEDIATE: &str = "E0416";
pub const SUSPENDING_FN_OP: &str = "E0417";

// E09xx: ツール
pub const PATH_NOT_FOUND: &str = "E0901";
pub const CANNOT_READ: &str = "E0902";
pub const CANNOT_WRITE_OUTPUT: &str = "E0903";
pub const NODE_UNAVAILABLE: &str = "E0904";
pub const NO_JS_BACKEND: &str = "E0905";
pub const INVALID_JS_OUTPUT: &str = "E0906";
