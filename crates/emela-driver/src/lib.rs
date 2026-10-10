//! syntax から codegen までをつなぐパイプライン．
//!
//! ソースの表を持ち，各段の診断を集めて表示し，出力した JS を node で実行する．

pub mod code;
mod diagnostic;
mod frontend;
mod js;
mod output;
mod pipeline;
mod render;
mod run;
mod source;
mod testing;

pub use diagnostic::{Diagnostic, HELP_PREFIX, Label, Location, Severity, Span, error_count};
pub use emela_resolve::OsFs;
pub use frontend::{ParseOnly, Resolve};
pub use js::{CoreJs, JS_ENTRY_FILE};
pub use output::{JsOutput, OutputFile, write_output};
pub use pipeline::{
    Analysis, Checked, ENTRY_FILE, Frontend, Input, JS_OUT_DIR, JsBackend, LexOnly, NoJsBackend,
    Parsed, RunOptions, SOURCE_DIR, build, check, run,
};
pub use render::{render, render_one, summary};
pub use run::{DEFECT_EXIT_CODE, DEFECT_NAME, NODE_ENV, Output, RunOutput, node_program, run_node};
pub use source::{FileId, FileSystem, MemoryFiles, SourceDb, SourceFile};
pub use testing::{
    JS_TEST_ENTRY_FILE, JS_TEST_OUT_DIR, ParseTrees, SourceTests, TEST_ANNOTATION,
    TEST_FAILURE_EXIT_CODE, TestBackend, TestCase, TestFn, TestModule, TestOptions, TestOutcome,
    TestReport, TestResult, WithTests, collect_tests, default_test_out_dir, is_test_fn, test,
};
