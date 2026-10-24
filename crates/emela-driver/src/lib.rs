//! The pipeline from syntax to codegen.
//!
//! Holds the source table, collects and renders each stage's diagnostics, and runs the emitted JS
//! on node (or builds the emitted C with cc and runs the executable).

mod actions;
mod c;
pub mod code;
mod complete;
mod dependencies;
pub use dependencies::check_dependency_targets;
mod diagnostic;
mod doc;
mod documentation;
mod fmt;
mod frontend;
mod hints;
mod host_files;
mod hover;
mod internal;
mod js;
mod nav;
mod new;
mod output;
mod pipeline;
mod pome;
mod render;
mod run;
mod signature;
mod source;
mod structure;
mod target;
mod testing;
mod tokens;
mod wasm;

pub use actions::{CodeAction, CodeActionKind};
pub use c::{C_ENTRY_FILE, C_EXECUTABLE, CC_ENV, CoreC, build_executable, cc_command, run_native};
pub use complete::{Completion, CompletionKind};
pub use diagnostic::{Diagnostic, HELP_PREFIX, Label, Location, Severity, Span, error_count};
pub use doc::{DocChild, DocItem, DocKind, DocModule, collect_docs, render_html, render_markdown};
/// The lowered program the output stages read ([`Backend`]).
pub use emela_core::Module as CoreModule;
pub use emela_resolve::OsFs;
pub use fmt::{
    FmtFailure, FmtReport, FmtTarget, STDIN_DISPLAY, STDIN_PATH, WriteFile, fmt, fmt_targets,
    format_source,
};
pub use frontend::{Lower, ParseOnly, Resolve, Typecheck, TypedProgram};
pub use hints::{HintKind, InlayHint};
pub use hover::HoverInfo;
pub use js::{CoreJs, JS_ENTRY_FILE};

pub use nav::{
    DocumentSymbol, NavTarget, RenameError, SymbolKind, WorkspaceSymbol, document_symbols,
    workspace_symbols,
};
pub use new::{NewFile, NewPome, new};
pub use output::{BuildOutput, OutputFile, write_files, write_output};
pub use pipeline::{
    Analysis, Backend, C_OUT_DIR, CORE_DIR, Checked, CoreSource, DOC_OUT_DIR, Dependencies,
    DependencyPome, DependencyRef, ENTRY_FILE, Frontend, Input, JS_OUT_DIR, LexOnly, NoJsBackend,
    Parsed, RunOptions, Runner, SOURCE_DIR, TEST_DIR, WASM_OUT_DIR, build, check, run,
};
pub use pome::project_dependencies;
pub use render::{render, render_one, summary};
pub use run::{
    DEFECT_EXIT_CODE, DEFECT_NAME, ERROR_EXIT_CODE, FAIL_KEY, INTERRUPTED_EXIT_CODE, NODE_ENV,
    Output, ProgramCommand, RunOutput, WASMTIME_ENV, node_program, run_node, run_wasmtime,
    wasmtime_program, with_program_cwd, with_program_timeout,
};
pub use signature::SignatureInfo;
pub use source::{FileId, FileSystem, MemoryFiles, SourceDb, SourceFile};
pub use structure::{FoldKind, FoldRange, folding_ranges, selection_ranges};
pub use target::{Target, project_target};
pub use testing::{
    AGENT_ENV_VARS, C_TEST_OUT_DIR, JS_TEST_ENTRY_FILE, JS_TEST_OUT_DIR, LowerTests, Reporter,
    TEST_ANNOTATION, TEST_FAILURE_EXIT_CODE, TestBackend, TestCase, TestDef, TestFn, TestModule,
    TestOptions, TestOutcome, TestReport, TestResult, collect_tests, default_test_out_dir,
    default_test_out_dir_for, is_test_fn, runs_in_agent, test,
};
pub use tokens::{SemanticToken, TokenKind};
pub use wasm::{CoreWasm, WASM_ENTRY_FILE};
