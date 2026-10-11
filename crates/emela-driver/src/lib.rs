//! syntax から codegen までをつなぐパイプライン．
//!
//! ソースの表を持ち，各段の診断を集めて表示し，出力した JS を node で実行する．

pub mod code;
mod diagnostic;
mod fmt;
mod frontend;
mod hover;
mod js;
mod output;
mod pipeline;
mod render;
mod run;
mod source;
mod testing;

pub use diagnostic::{Diagnostic, HELP_PREFIX, Label, Location, Severity, Span, error_count};
pub use emela_resolve::OsFs;
pub use fmt::{
    FmtFailure, FmtReport, FmtTarget, STDIN_DISPLAY, STDIN_PATH, WriteFile, fmt, fmt_targets,
    format_source,
};
pub use frontend::{Lower, ParseOnly, Resolve, Typecheck, TypedProgram};
pub use hover::HoverInfo;
pub use js::{CoreJs, JS_ENTRY_FILE};
pub use output::{JsOutput, OutputFile, write_output};
pub use pipeline::{
    Analysis, CORE_DIR, Checked, CoreSource, ENTRY_FILE, Frontend, Input, JS_OUT_DIR, JsBackend,
    LexOnly, NoJsBackend, Parsed, RunOptions, SOURCE_DIR, build, check, run,
};
pub use render::{render, render_one, summary};
pub use run::{
    DEFECT_EXIT_CODE, DEFECT_NAME, ERROR_EXIT_CODE, FAIL_KEY, NODE_ENV, Output, RunOutput,
    node_program, run_node,
};
pub use source::{FileId, FileSystem, MemoryFiles, SourceDb, SourceFile};
pub use testing::{
    JS_TEST_ENTRY_FILE, JS_TEST_OUT_DIR, LowerTests, TEST_ANNOTATION, TEST_FAILURE_EXIT_CODE,
    TestBackend, TestCase, TestDef, TestFn, TestModule, TestOptions, TestOutcome, TestReport,
    TestResult, collect_tests, default_test_out_dir, is_test_fn, test,
};
