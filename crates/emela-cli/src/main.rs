use clap::{CommandFactory, Parser, Subcommand};
use emela_driver::{
    Analysis, Backend, CoreC, CoreEmbedded, CoreJs, CoreWasm, Diagnostic, EmbeddedProject,
    FileSystem, Input, Lower, LowerTests, MemoryFiles, OsFs, Output, RunOptions, SourceDb, Target,
    TestBackend, TestModule, TestOptions, Typecheck,
};
use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod pome;

/// Compile, run, test, and format Emela programs.
#[derive(Parser)]
#[command(name = "emela", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report diagnostics without compiling
    Check {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Board used to resolve `[embedded.modules]` overlays
        #[arg(long)]
        board: Option<String>,
    },
    /// Compile to the output of `--target` (`target/emela/js` for JS; a native executable for C;
    /// `main.wasm` for WASM; embedded C, or firmware when a board is selected)
    Build {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Output directory [default: <project>/target/emela/<target>]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// The target [default: `[build].target` of Pome.toml, or js]
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// Embedded runtime and board adapter configuration (only for --target c-embedded)
        #[arg(long)]
        embedded_config: Option<PathBuf>,
        /// Board support package; defaults to `[embedded].board` in Pome.toml
        #[arg(long)]
        board: Option<String>,
    },
    /// Write a previously built firmware image after verifying its project and artifact hashes
    Flash {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Build output directory [default: <project>/target/emela/c-embedded/<board>]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// Board support package; defaults to `[embedded].board` in Pome.toml
        #[arg(long)]
        board: Option<String>,
        /// Runtime configuration used by the successful build
        #[arg(long)]
        embedded_config: Option<PathBuf>,
        /// Serial port for ESP32-C3, or probe serial number for STM32 OpenOCD
        #[arg(long)]
        port: Option<String>,
        /// Print the verified flasher command without opening a device
        #[arg(long)]
        dry_run: bool,
    },
    /// Compile and run: on Node.js, as a native executable (`--target c`), or with wasmtime
    /// (`--target wasm`)
    Run {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Output directory [default: <project>/target/emela/<target>]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// The target [default: `[build].target` of Pome.toml, or js]
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// Arguments passed to the program (write them after `--`)
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    /// Run the functions marked `@test`
    Test {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Run only the tests whose name contains this string
        filter: Option<String>,
        /// Output directory [default: <project>/target/emela/test, or test-c for `--target c`]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// The target [default: `[build].target` of Pome.toml, or js]
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
    },
    /// Format the sources in place
    Fmt {
        /// Files or directories to format [default: `src` of the project at `.`].
        /// `-` reads standard input and writes standard output
        paths: Vec<PathBuf>,
        /// Do not write; list the files that need formatting on standard output and exit with 1 if any
        #[arg(long)]
        check: bool,
    },
    /// Generate API documentation under `target/emela/doc`
    Doc {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Document the bundled core and std instead of the project
        #[arg(long, conflicts_with = "path")]
        std: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = DocFormat::Html)]
        format: DocFormat,
        /// Output directory [default: <project>/target/emela/doc]
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// Manage dependencies: Pome.toml and Pome.lock
    Pome {
        #[command(subcommand)]
        command: pome::PomeCommand,
    },
    /// Start the language server over standard input and output
    Lsp,
}

/// A build target (spec chapter 16).
#[derive(Clone, Copy, clap::ValueEnum)]
enum TargetArg {
    /// JavaScript run on Node.js
    Js,
    /// C compiled with cc (`EMELA_CC`) to a native executable
    C,
    /// Embedded C sources, or a firmware image when `--board` or `[embedded].board` is set
    CEmbedded,
    /// WebAssembly (WASI) run with `wasmtime` (`EMELA_WASMTIME`)
    Wasm,
}

impl From<TargetArg> for Target {
    fn from(target: TargetArg) -> Self {
        match target {
            TargetArg::Js => Target::Js,
            TargetArg::C => Target::C,
            TargetArg::CEmbedded => Target::CEmbedded,
            TargetArg::Wasm => Target::Wasm,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum DocFormat {
    /// Pages with a sidebar of modules, highlighted signatures and links between types
    Html,
    /// `index.md` and one Markdown file per module
    Md,
}

const DIAGNOSTIC_FAILURE: u8 = 1;

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check { path, board } => check(&path_or_current(path), board),
        Command::Build {
            path,
            out_dir,
            target,
            embedded_config,
            board,
        } => build(
            &path_or_current(path),
            out_dir,
            target,
            embedded_config,
            board,
        ),
        Command::Flash {
            path,
            out_dir,
            board,
            embedded_config,
            port,
            dry_run,
        } => flash(
            &path_or_current(path),
            out_dir,
            board,
            embedded_config,
            port,
            dry_run,
        ),
        Command::Run {
            path,
            out_dir,
            target,
            args,
        } => run(&path_or_current(path), out_dir, target, args),
        Command::Test {
            path,
            filter,
            out_dir,
            target,
        } => test(&path_or_current(path), filter, out_dir, target),
        Command::Fmt { paths, check } => fmt(paths, check),
        Command::Doc {
            path,
            std,
            format,
            out_dir,
        } => doc(path, std, format, out_dir),
        Command::Pome { command } => pome::pome(command),
        Command::Lsp => lsp(),
    }
}

fn path_or_current(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| PathBuf::from("."))
}

fn check(path: &Path, board: Option<String>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, true) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let sources = match EmbeddedProject::read(&input)
        .and_then(|project| project.select(board))
        .and_then(|project| project.overlay(&input))
    {
        Ok(sources) => sources,
        Err(diagnostic) => return report_error(diagnostic),
    };
    let (analysis, _) = emela_driver::check(&sources, &input, &mut Typecheck::default());
    report(&analysis)
}

/// The target: `--target`, else `[build].target` of Pome.toml (spec 4.7), else js. `None` after
/// printing E0232 for an unknown target in Pome.toml.
/// Also `None` after printing E0238 when a dependency does not support the target (spec 4.7).
fn resolve_target(input: &Input, flag: Option<TargetArg>) -> Option<Target> {
    let target = match flag {
        Some(flag) => flag.into(),
        None => match emela_driver::project_target(&OsFs, input) {
            Ok(target) => target.unwrap_or_default(),
            Err(diagnostic) => {
                print_diagnostics(&[diagnostic], &SourceDb::new());
                return None;
            }
        },
    };
    let unsupported = emela_driver::check_dependency_targets(&input.dependencies, target);
    if !unsupported.is_empty() {
        print_diagnostics(&unsupported, &SourceDb::new());
        return None;
    }
    Some(target)
}

fn build(
    path: &Path,
    out_dir: Option<PathBuf>,
    target: Option<TargetArg>,
    embedded_config: Option<PathBuf>,
    board: Option<String>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, false) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let project = match EmbeddedProject::read(&input).and_then(|project| project.select(board)) {
        Ok(project) => project,
        Err(diagnostic) => return report_error(diagnostic),
    };
    if project.board_id().is_some()
        && let Err(diagnostic) = project.invalidate(
            &out_dir
                .clone()
                .unwrap_or_else(|| project.default_out_dir(&input)),
        )
    {
        return report_error(diagnostic);
    }
    let target = if project.board_id().is_some() {
        let configured = match emela_driver::project_target(&OsFs, &input) {
            Ok(target) => target,
            Err(diagnostic) => return report_error(diagnostic),
        };
        let target = target
            .map(Target::from)
            .or(configured)
            .unwrap_or(Target::CEmbedded);
        if target != Target::CEmbedded {
            return report_error(Diagnostic::error(
                "a selected embedded board requires target `c-embedded`",
            ));
        }
        let unsupported = emela_driver::check_dependency_targets(&input.dependencies, target);
        if !unsupported.is_empty() {
            print_diagnostics(&unsupported, &SourceDb::new());
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        }
        target
    } else {
        let Some(target) = resolve_target(&input, target) else {
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        };
        target
    };
    if embedded_config.is_some() && target != Target::CEmbedded {
        print_diagnostics(
            &[Diagnostic::error(
                "`--embedded-config` requires `--target c-embedded` or `[build].target = \"c-embedded\"`",
            )],
            &SourceDb::new(),
        );
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    }
    let out_dir = out_dir.unwrap_or_else(|| {
        if target == Target::CEmbedded {
            project.default_out_dir(&input)
        } else {
            input.default_out_dir_for(target)
        }
    });
    let config_flag = embedded_config.clone();
    let embedded_config = if target == Target::CEmbedded {
        match project.runtime_config(&input, embedded_config) {
            Ok(config) => config,
            Err(diagnostic) => return report_error(diagnostic),
        }
    } else {
        Default::default()
    };
    let sources = if target == Target::CEmbedded {
        match project.overlay(&input) {
            Ok(sources) => Some(sources),
            Err(diagnostic) => return report_error(diagnostic),
        }
    } else {
        None
    };
    if project.board_id().is_some()
        && !input.library_only
        && let Err(diagnostic) = project.prepare_build(&input, &out_dir, config_flag.as_deref())
    {
        return report_error(diagnostic);
    }
    let generated_dir = if project.board_id().is_some() {
        out_dir.join("generated")
    } else {
        out_dir.clone()
    };
    let mut frontend = Lower::default().with_target(target);
    let (analysis, _) = match target {
        Target::Js => emela_driver::build(&OsFs, &input, &mut frontend, &mut CoreJs, &out_dir),
        Target::C => emela_driver::build(
            &OsFs,
            &input,
            &mut frontend,
            &mut CoreC::default(),
            &out_dir,
        ),
        Target::CEmbedded => emela_driver::build(
            sources
                .as_ref()
                .expect("embedded target has a source overlay"),
            &input,
            &mut frontend,
            &mut CoreEmbedded::new(embedded_config),
            &generated_dir,
        ),
        Target::Wasm => emela_driver::build(&OsFs, &input, &mut frontend, &mut CoreWasm, &out_dir),
    };
    let result = report(&analysis);
    if analysis.has_errors() || input.library_only {
        return result;
    }
    if project.board_id().is_some()
        && let Err(diagnostic) = project.build_firmware(&input, &out_dir, config_flag.as_deref())
    {
        return report_error(diagnostic);
    }
    result
}

fn report_error(diagnostic: Diagnostic) -> ExitCode {
    print_diagnostics(&[diagnostic], &SourceDb::new());
    ExitCode::from(DIAGNOSTIC_FAILURE)
}

fn flash(
    path: &Path,
    out_dir: Option<PathBuf>,
    board: Option<String>,
    embedded_config: Option<PathBuf>,
    port: Option<String>,
    dry_run: bool,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, false) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let project = match EmbeddedProject::read(&input).and_then(|project| project.select(board)) {
        Ok(project) => project,
        Err(diagnostic) => return report_error(diagnostic),
    };
    if project.board_id().is_none() {
        return report_error(Diagnostic::error(
            "`flash` requires `--board` or `[embedded].board`",
        ));
    }
    let out_dir = out_dir.unwrap_or_else(|| project.default_out_dir(&input));
    match project.flash(
        &input,
        &out_dir,
        embedded_config.as_deref(),
        port.as_deref(),
        dry_run,
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(diagnostic) => report_error(diagnostic),
    }
}

fn run(
    path: &Path,
    out_dir: Option<PathBuf>,
    target: Option<TargetArg>,
    args: Vec<OsString>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, false) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let Some(target) = resolve_target(&input, target) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = RunOptions {
        out_dir: out_dir.unwrap_or_else(|| input.default_out_dir_for(target)),
        node: emela_driver::node_program(),
        args,
        output: Output::Inherit,
    };
    match target {
        Target::Js => run_with(&input, &options, target, &mut CoreJs),
        Target::C => run_with(&input, &options, target, &mut CoreC::default()),
        Target::CEmbedded => unsupported_embedded_command("run"),
        Target::Wasm => run_with(&input, &options, target, &mut CoreWasm),
    }
}

fn run_with<B: Backend<emela_driver::CoreModule>>(
    input: &Input,
    options: &RunOptions,
    target: Target,
    backend: &mut B,
) -> ExitCode {
    // Diagnostics go out before the program starts, so warnings show first even if it never
    // ends.
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let (analysis, output) = emela_driver::run(
        &OsFs,
        input,
        &mut Lower::default().with_target(target),
        backend,
        options,
        |analysis| {
            diagnostics_code = report(analysis);
            reported = analysis.diagnostics.len();
        },
    );
    match output {
        // Pass the program's exit code through, truncated to 8 bits like a shell does.
        Some(output) => ExitCode::from(output.code as u8),
        None => {
            // Diagnostics about failing to start the program were appended after those already
            // printed.
            let rest = &analysis.diagnostics[reported..];
            if rest.is_empty() {
                diagnostics_code
            } else {
                print_diagnostics(rest, &analysis.sources);
                ExitCode::from(DIAGNOSTIC_FAILURE)
            }
        }
    }
}

fn test(
    path: &Path,
    filter: Option<String>,
    out_dir: Option<PathBuf>,
    target: Option<TargetArg>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, true) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let Some(target) = resolve_target(&input, target) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = TestOptions {
        out_dir: out_dir.unwrap_or_else(|| emela_driver::default_test_out_dir_for(&input, target)),
        node: emela_driver::node_program(),
        filter,
        output: Output::Inherit,
    };
    match target {
        Target::Js => test_with(&input, &options, target, &mut CoreJs),
        Target::C => test_with(&input, &options, target, &mut CoreC::default()),
        Target::CEmbedded => unsupported_embedded_command("test"),
        Target::Wasm => test_with(&input, &options, target, &mut CoreWasm),
    }
}

fn unsupported_embedded_command(command: &str) -> ExitCode {
    print_diagnostics(
        &[Diagnostic::error(format!(
            "`{command}` is not available for target `c-embedded`"
        ))
        .with_note("help: use `emela build --target c-embedded`, then build and run the generated C with the board's toolchain and runner")],
        &SourceDb::new(),
    );
    ExitCode::from(DIAGNOSTIC_FAILURE)
}

fn test_with<B: TestBackend<TestModule>>(
    input: &Input,
    options: &TestOptions,
    target: Target,
    backend: &mut B,
) -> ExitCode {
    // Results go to stdout, diagnostics to stderr. Diagnostics go out before the tests start.
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let stdout = std::io::stdout();
    let (analysis, report) = emela_driver::test(
        &OsFs,
        input,
        &mut LowerTests::default().with_target(target),
        backend,
        options,
        |analysis| {
            diagnostics_code = report(analysis);
            reported = analysis.diagnostics.len();
        },
        &mut stdout.lock(),
    );
    match report {
        Some(report) => ExitCode::from(report.exit_code()),
        None => {
            let rest = &analysis.diagnostics[reported..];
            if rest.is_empty() {
                diagnostics_code
            } else {
                print_diagnostics(rest, &analysis.sources);
                ExitCode::from(DIAGNOSTIC_FAILURE)
            }
        }
    }
}

/// Documents the project at `path`, or the bundled core and std with `std`. Nothing is written
/// if there are errors, since broken types would show up in the signatures.
fn doc(path: Option<PathBuf>, std: bool, format: DocFormat, out_dir: Option<PathBuf>) -> ExitCode {
    let (analysis, program, title, default_out_dir) = if std {
        // The core and std modules are read for any input, so an empty single file is enough.
        let fs = MemoryFiles::new([("std/doc.emel", "")]);
        let Some(input) = resolve_input(&fs, Path::new("std/doc.emel"), false) else {
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        };
        let (analysis, program) = emela_driver::check(&fs, &input, &mut Typecheck::default());
        let title = "Emela core and std".to_string();
        (
            analysis,
            program,
            title,
            PathBuf::from(emela_driver::DOC_OUT_DIR),
        )
    } else {
        let Some(input) = resolve_input(&OsFs, &path_or_current(path), false) else {
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        };
        let (analysis, program) = emela_driver::check(&OsFs, &input, &mut Typecheck::default());
        // Pome.toml is not read yet, so the project is named after its directory.
        let title = input.base.file_name().map_or_else(
            || "docs".to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        (analysis, program, title, input.default_doc_out_dir())
    };
    let code = report(&analysis);
    let Some(program) = program.filter(|_| !analysis.has_errors()) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let modules = emela_driver::collect_docs(&program, &analysis, std);
    let (files, index) = match format {
        DocFormat::Html => (emela_driver::render_html(&title, &modules), "index.html"),
        DocFormat::Md => (emela_driver::render_markdown(&title, &modules), "index.md"),
    };
    let out_dir = out_dir.unwrap_or(default_out_dir);
    if let Err(diagnostic) = emela_driver::write_files(&out_dir, &files) {
        print_diagnostics(&[diagnostic], &SourceDb::new());
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    }
    println!("{}", out_dir.join(index).display());
    code
}

fn fmt(paths: Vec<PathBuf>, check: bool) -> ExitCode {
    let stdin = Path::new(emela_driver::STDIN_PATH);
    if paths.iter().any(|path| path == stdin) {
        if paths.len() > 1 {
            let mut command = Cli::command();
            command.build();
            command
                .find_subcommand_mut("fmt")
                .expect("the fmt subcommand exists")
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "`-` (standard input) cannot be combined with other paths",
                )
                .exit();
        }
        return fmt_stdin(check);
    }
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let mut write = |path: &Path, text: &str| std::fs::write(path, text);
    let report = emela_driver::fmt(&OsFs, &paths, if check { None } else { Some(&mut write) });
    print_rendered(&report.render(stderr_color()));
    let mut code = if report.has_errors() {
        ExitCode::from(DIAGNOSTIC_FAILURE)
    } else {
        ExitCode::SUCCESS
    };
    if check && !report.changed.is_empty() {
        let mut out = String::new();
        for path in &report.changed {
            out.push_str(&format!("{}\n", path.display()));
        }
        let _ = std::io::stdout().lock().write_all(out.as_bytes());
        code = ExitCode::from(DIAGNOSTIC_FAILURE);
    }
    code
}

/// For editor integration. On a syntax error nothing is written to stdout. With `--check`,
/// nothing is written either, and `<stdin>` is listed if formatting is needed.
fn fmt_stdin(check: bool) -> ExitCode {
    let mut text = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut text) {
        let diagnostic = Diagnostic::error(format!("cannot read standard input: {err}"))
            .with_code(emela_driver::code::CANNOT_READ);
        print_diagnostics(&[diagnostic], &SourceDb::new());
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    }
    let display = Path::new(emela_driver::STDIN_DISPLAY);
    let formatted = match emela_driver::format_source(display, &text) {
        Ok(formatted) => formatted,
        Err(failure) => {
            print_diagnostics(&failure.diagnostics, &failure.sources);
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        }
    };
    let mut stdout = std::io::stdout().lock();
    match (check, formatted) {
        (true, None) => ExitCode::SUCCESS,
        (true, Some(_)) => {
            let _ = writeln!(stdout, "{}", display.display());
            ExitCode::from(DIAGNOSTIC_FAILURE)
        }
        (false, formatted) => {
            let _ = stdout.write_all(formatted.as_deref().unwrap_or(&text).as_bytes());
            ExitCode::SUCCESS
        }
    }
}

fn lsp() -> ExitCode {
    // Use the same frontend as `check` so the diagnostics are the same.
    match emela_lsp::run_stdio(Typecheck::default) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("emela lsp: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The input of check, build, run, test and doc, with the project's dependencies from
/// `Pome.lock` (fetched if missing from the cache, spec 4.7). With `tests` (check and test), the
/// test root and `[dev-dependencies]` are read too (spec 13.1).
fn resolve_input(fs: &dyn FileSystem, path: &Path, tests: bool) -> Option<Input> {
    let mut input = match Input::resolve(fs, path) {
        Ok(input) if tests => input.with_tests(fs),
        Ok(input) => input,
        Err(diagnostic) => {
            print_diagnostics(&[diagnostic], &SourceDb::new());
            return None;
        }
    };
    if !input.single_file {
        match pome::dependencies(&input.base, tests) {
            Ok(dependencies) => input.dependencies = dependencies,
            Err(e) => {
                eprintln!("error: {e}");
                return None;
            }
        }
    }
    Some(input)
}

/// Returns 1 if there are errors.
fn report(analysis: &Analysis) -> ExitCode {
    print_diagnostics(&analysis.diagnostics, &analysis.sources);
    if analysis.has_errors() {
        ExitCode::from(DIAGNOSTIC_FAILURE)
    } else {
        ExitCode::SUCCESS
    }
}

fn print_diagnostics(diagnostics: &[Diagnostic], sources: &SourceDb) {
    if diagnostics.is_empty() {
        return;
    }
    print_rendered(&emela_driver::render(diagnostics, sources, stderr_color()));
}

fn print_rendered(text: &str) {
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

/// Only when stderr is a terminal and `NO_COLOR` is unset or empty.
fn stderr_color() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}
