//! The pipeline from syntax to codegen.
//!
//! Holds the source table, collects and renders each stage's diagnostics, and runs the emitted JS
//! on node (or builds the emitted C with cc and runs the executable).

pub use command::doctor::{DoctorFormat, doctor};
pub use diagnostic::code;
pub use project::dependencies::check_dependency_targets;
pub use project::experimental::{
    FEATURES, FLAG_PREFIX, Feature, Features, Stage, dependency_features, experimental_features,
};
mod backend;
mod check;
mod command;
mod diagnostic;
mod frontend;
mod ide;
mod js_host;
mod output;
mod pipeline;
mod project;
mod run;
mod source;
mod target;
mod version;

pub use backend::c::{
    C_ENTRY_FILE, C_EXECUTABLE, CC_ARGS_ENV, CC_ENV, CoreC, build_executable, cc_command,
    run_native,
};
pub use backend::js::{CoreJs, JS_ENTRY_FILE};
pub use command::doc::{
    DocChild, DocItem, DocKind, DocModule, collect_docs, render_html, render_markdown,
};
pub use command::fmt::{
    FmtFailure, FmtReport, FmtTarget, STDIN_DISPLAY, STDIN_PATH, WriteFile, fmt, fmt_targets,
    format_source,
};
pub use diagnostic::{Diagnostic, HELP_PREFIX, Label, Location, Severity, Span, error_count};
/// The lowered program the output stages read ([`Backend`]).
pub use emela_core::Module as CoreModule;
pub use emela_resolve::OsFs;
pub use frontend::{Lower, ParseOnly, Resolve, Typecheck};
pub use ide::actions::{CodeAction, CodeActionKind};
pub use ide::complete::{Completion, CompletionKind};
pub use ide::hints::{HintKind, InlayHint};
pub use ide::hover::HoverInfo;
pub use ide::links::{DocumentLink, document_links};

pub use backend::testing::{
    AGENT_ENV_VARS, JS_TEST_ENTRY_FILE, JS_TEST_OUT_DIR, LowerTests, NATIVE_TEST_OUT_DIR, Reporter,
    TEST_FAILURE_EXIT_CODE, TestBackend, TestCase, TestModule, TestOptions, TestOutcome,
    TestReport, TestResult, default_test_out_dir, default_test_out_dir_for, runs_in_agent, test,
};
pub use backend::wasm::{CoreWasm, WASM_ENTRY_FILE};
pub use check::analysis::Analysis;
pub use check::program::TypedProgram;
pub use check::test_fns::{TEST_ANNOTATION, TestDef, TestFn, collect_tests, is_test_fn};
pub use command::new::{NewFile, NewPome, new};
pub use diagnostic::render::{render, render_one, summary};
pub use ide::nav::{
    DocumentSymbol, NavTarget, RenameError, SymbolKind, WorkspaceSymbol, document_symbols,
    workspace_symbols,
};
pub use ide::signature::SignatureInfo;
pub use ide::structure::{FoldKind, FoldRange, folding_ranges, selection_ranges};
pub use ide::tokens::{SemanticToken, TokenKind};
pub use js_host::{JS_HOST_ENV, JsHost, NODE_ENV, js_host, js_host_program};
pub use output::{BuildOutput, OutputFile, write_files, write_output};
pub use pipeline::{
    Backend, CORE_DIR, Checked, CoreSource, Frontend, LexOnly, NoJsBackend, Parsed, RunOptions,
    Runner, build, check, run,
};
pub use project::pome::project_dependencies;
pub use project::{
    DOC_OUT_DIR, Dependencies, DependencyPome, DependencyRef, ENTRY_FILE, Input, JS_OUT_DIR,
    NATIVE_OUT_DIR, SOURCE_DIR, TEST_DIR, WASM_OUT_DIR, enclosing_project_target, project_target,
};
pub use run::{
    DEFECT_EXIT_CODE, DEFECT_NAME, ERROR_EXIT_CODE, FAIL_KEY, INTERRUPTED_EXIT_CODE, Output,
    ProgramCommand, RunOutput, WASMTIME_ENV, run_node, run_wasmtime, wasmtime_program,
    with_program_cwd, with_program_timeout,
};
pub use source::{FileId, FileSystem, MemoryFiles, SourceDb, SourceFile};
pub use target::Target;
