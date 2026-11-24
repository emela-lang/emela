//! The `emela` command (spec `[cmd]`).
//!
//! The `--help` text uses the same words as the Commands table in the README. Defaults are written
//! by hand at the end of each description in clap's `[default: …]` form, not with
//! `default_value`, because the `Option<PathBuf>` defaults (`.`, the per-target output directory)
//! are decided in this file after the project is found. The test `help_is_english` keeps the text
//! ASCII.

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use emela_driver::{
    Analysis, Backend, BuildOutput, CoreC, CoreJs, CoreWasm, Diagnostic, DoctorFormat, FileSystem,
    Input, Lower, LowerTests, MemoryFiles, OsFs, Output, Reporter, RunOptions, Runner, SourceDb,
    Target, TestBackend, TestModule, TestOptions, Typecheck,
};
use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod pome;
mod progress;

/// The version `--version` prints: the workspace version, then `+<short hash>` when built from a
/// git checkout (see `build.rs`).
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), env!("EMELA_VERSION_SUFFIX"));

/// Compile, run, test, and format Emela programs.
#[derive(Parser)]
#[command(name = "emela", version = VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new Pome in a new directory
    New {
        /// A name (`hello`) or a source path (`github.com/alice/hello`, `github:alice/hello`)
        name: String,
        /// Create a library (`src/lib.emel`) instead of an application (`src/main.emel`)
        #[arg(long)]
        lib: bool,
    },
    /// Report diagnostics without compiling
    Check {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
    },
    /// Compile to the output of `--target` (`target/emela/js` for JS; an executable for native;
    /// `main.wasm` for WASM)
    Build {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Output directory [default: <project>/target/emela/<target>]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// The target [default: `[build].target` of Pome.toml, or js]
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
    },
    /// Compile and run: on Node.js, as a native executable (`--target native`), or with wasmtime
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
        /// Output directory [default: <project>/target/emela/test, or test-native for `--target native`]
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// The target [default: `[build].target` of Pome.toml, or js]
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// How results are shown [default: agent inside a coding agent, pretty at a terminal,
        /// plain otherwise]
        #[arg(long, value_enum)]
        reporter: Option<ReporterArg>,
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
    /// Report whether the programs each build target uses can be found and are new enough
    Doctor {
        /// A path in the project [default: .]
        path: Option<PathBuf>,
        /// Require this target instead of the build target of the project
        #[arg(long, value_enum)]
        target: Option<TargetArg>,
        /// Output format
        #[arg(long, value_enum, default_value_t = DoctorFormatArg::Text)]
        format: DoctorFormatArg,
    },
}

/// A build target (spec chapter 16).
#[derive(Clone, Copy, clap::ValueEnum)]
enum TargetArg {
    /// JavaScript run on Node.js
    Js,
    /// An executable for this machine, built with its C compiler (`EMELA_CC`, `EMELA_CC_ARGS`)
    Native,
    /// WebAssembly (WASI) run with `wasmtime` (`EMELA_WASMTIME`)
    Wasm,
}

impl TargetArg {
    fn select(self) -> Target {
        match self {
            TargetArg::Js => Target::Js,
            TargetArg::Native => Target::Native,
            TargetArg::Wasm => Target::Wasm,
        }
    }
}

/// How `emela test` shows its results (`[cmd.test.reporter]`).
#[derive(Clone, Copy, clap::ValueEnum)]
enum ReporterArg {
    /// For a person at a terminal: marks, color and elapsed time
    Pretty,
    /// One line per test and the failures at the end; the same input gives the same output
    Plain,
    /// Only the failures and the summary line, for a coding agent
    Agent,
}

impl From<ReporterArg> for Reporter {
    fn from(reporter: ReporterArg) -> Self {
        match reporter {
            ReporterArg::Pretty => Reporter::Pretty,
            ReporterArg::Plain => Reporter::Plain,
            ReporterArg::Agent => Reporter::Agent,
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

#[derive(Clone, Copy, clap::ValueEnum)]
enum DoctorFormatArg {
    /// Lines for a person, grouped by target
    Text,
    /// One JSON object
    Json,
}

impl From<DoctorFormatArg> for DoctorFormat {
    fn from(format: DoctorFormatArg) -> Self {
        match format {
            DoctorFormatArg::Text => DoctorFormat::Text,
            DoctorFormatArg::Json => DoctorFormat::Json,
        }
    }
}

const DIAGNOSTIC_FAILURE: u8 = 1;

/// The commands that take the flags `--experimental-<name>` (`[cmd.build]`, `[cmd.test]`).
const EXPERIMENTAL_COMMANDS: [&str; 4] = ["check", "build", "run", "test"];

fn main() -> ExitCode {
    let (args, experimental) = take_experimental_flags(std::env::args_os().collect());
    let matches = with_experimental_flags(
        Cli::command(),
        &emela_driver::experimental_features().collect::<Vec<_>>(),
    )
    .get_matches_from(args);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    match cli.command {
        Command::New { name, lib } => new_pome(&name, lib),
        Command::Check { path } => check(&path_or_current(path), experimental),
        Command::Build {
            path,
            out_dir,
            target,
        } => build(&path_or_current(path), out_dir, target, experimental),
        Command::Run {
            path,
            out_dir,
            target,
            args,
        } => run(&path_or_current(path), out_dir, target, args, experimental),
        Command::Test {
            path,
            filter,
            out_dir,
            target,
            reporter,
        } => test(
            &path_or_current(path),
            filter,
            out_dir,
            target,
            reporter,
            experimental,
        ),
        Command::Fmt { paths, check } => fmt(paths, check),
        Command::Doc {
            path,
            std,
            format,
            out_dir,
        } => doc(path, std, format, out_dir),
        Command::Pome { command } => pome::pome(command),
        Command::Lsp => lsp(),
        Command::Doctor {
            path,
            target,
            format,
        } => doctor(&path_or_current(path), target, format),
    }
}

fn new_pome(name: &str, lib: bool) -> ExitCode {
    let pome = match emela_driver::new(name, lib) {
        Ok(pome) => pome,
        Err(diagnostic) => {
            print_diagnostics(&[diagnostic], &SourceDb::new());
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        }
    };

    let dir = Path::new(&pome.dir);
    let exists = dir.symlink_metadata().is_ok();
    if exists {
        print_diagnostics(
            &[Diagnostic::error(format!(
                "`{}` already exists",
                dir.display()
            ))],
            &SourceDb::new(),
        );
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    }

    // Files written before a failure are left in place.
    for file in &pome.files {
        let path = dir.join(&file.path);
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, &file.contents));
        if let Err(error) = written {
            print_diagnostics(
                &[Diagnostic::error(format!(
                    "cannot write `{}`: {error}",
                    path.display()
                ))],
                &SourceDb::new(),
            );
            return ExitCode::from(DIAGNOSTIC_FAILURE);
        }
    }

    let kind = if lib { "library" } else { "application" };
    eprintln!("created {kind} `{}`", pome.dir);
    ExitCode::SUCCESS
}

/// Takes the flags `--experimental-<name>` out of the arguments of `check`, `build`, `run` and
/// `test`, and returns the rest with the names. The names are taken before clap sees them, so
/// that a flag of a feature this compiler does not know is the warning W0206 instead of an
/// unknown-argument error (`[cmd.build]`); the pipeline reports it. Arguments after `--` belong
/// to the program, and the other commands do not take the flags.
fn take_experimental_flags(args: Vec<OsString>) -> (Vec<OsString>, Vec<String>) {
    let command = args
        .iter()
        .skip(1)
        .position(|arg| !arg.to_string_lossy().starts_with('-'))
        .map(|i| i + 1);
    let Some(command) = command.filter(|&i| {
        EXPERIMENTAL_COMMANDS
            .iter()
            .any(|c| args[i].to_str() == Some(c))
    }) else {
        return (args, Vec::new());
    };
    let mut rest = Vec::with_capacity(args.len());
    let mut names = Vec::new();
    let mut program_args = false;
    for (i, arg) in args.into_iter().enumerate() {
        if i > command && !program_args {
            if arg == "--" {
                program_args = true;
            } else if let Some(name) = arg
                .to_str()
                .and_then(|a| a.strip_prefix(emela_driver::FLAG_PREFIX))
            {
                names.push(name.to_string());
                continue;
            }
        }
        rest.push(arg);
    }
    (rest, names)
}

/// Adds a flag `--experimental-<name>` to `check`, `build`, `run` and `test` for each feature, so
/// that `--help` lists them (`[cmd.build]`). The flags themselves are taken out of the arguments
/// by [`take_experimental_flags`] before parsing.
fn with_experimental_flags(
    mut command: clap::Command,
    features: &[&emela_driver::Feature],
) -> clap::Command {
    for name in EXPERIMENTAL_COMMANDS {
        command = command.mut_subcommand(name, |mut sub| {
            for feature in features {
                sub = sub.arg(
                    clap::Arg::new(format!("experimental-{}", feature.name))
                        .long(format!("experimental-{}", feature.name))
                        .action(clap::ArgAction::SetTrue)
                        .help(format!(
                            "Enable the experimental feature `{}` (since {}): {}",
                            feature.name, feature.since, feature.summary
                        )),
                );
            }
            sub
        });
    }
    command
}

fn path_or_current(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| PathBuf::from("."))
}

fn check(path: &Path, experimental: Vec<String>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, true, experimental) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let (mut analysis, _) = emela_driver::check(&OsFs, &input, &mut Typecheck::default());
    // `[build].target` is the default target of `check` too (spec 4.7), so an unknown one is
    // E0232 here, reported together with the other diagnostics.
    if let Err(diagnostic) = emela_driver::project_target(&OsFs, &input) {
        analysis.diagnostics.insert(0, diagnostic);
    }
    report(&analysis)
}

/// The target: `--target`, else `[build].target` of Pome.toml (spec 4.7), else `default`. `None`
/// after printing E0232 for an unknown target in Pome.toml.
/// Also `None` after printing E0238 when a dependency does not support the target (spec 4.7).
fn resolve_target(input: &Input, flag: Option<TargetArg>, default: Target) -> Option<Target> {
    let target = match flag {
        Some(flag) => flag.select(),
        None => match emela_driver::project_target(&OsFs, input) {
            Ok(target) => target.unwrap_or(default),
            Err(diagnostic) => {
                print_diagnostics(&[diagnostic], &project_sources(input));
                return None;
            }
        },
    };
    let unsupported = emela_driver::check_dependency_targets(&input.dependencies, target);
    if !unsupported.is_empty() {
        print_diagnostics(&unsupported, &project_sources(input));
        return None;
    }
    Some(target)
}

/// An empty source database that shows paths relative to the project, as the diagnostics of the
/// pipeline do.
fn project_sources(input: &Input) -> SourceDb {
    let mut sources = SourceDb::new();
    sources.set_base(&input.base);
    sources
}

fn build(
    path: &Path,
    out_dir: Option<PathBuf>,
    target: Option<TargetArg>,
    experimental: Vec<String>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, false, experimental) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let Some(target) = resolve_target(&input, target, Target::default()) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let out_dir = out_dir.unwrap_or_else(|| input.default_out_dir_for(target));
    let mut frontend = Lower::default().with_target(target);
    let (analysis, _) = match target {
        Target::Js => emela_driver::build(&OsFs, &input, &mut frontend, &mut CoreJs, &out_dir),
        Target::Native => emela_driver::build(
            &OsFs,
            &input,
            &mut frontend,
            &mut Native::default(),
            &out_dir,
        ),
        Target::Wasm => emela_driver::build(&OsFs, &input, &mut frontend, &mut CoreWasm, &out_dir),
    };
    report(&analysis)
}

fn run(
    path: &Path,
    out_dir: Option<PathBuf>,
    target: Option<TargetArg>,
    args: Vec<OsString>,
    experimental: Vec<String>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, false, experimental) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let Some(target) = resolve_target(&input, target, Target::default()) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = RunOptions {
        out_dir: out_dir.unwrap_or_else(|| input.default_out_dir_for(target)),
        js_host: js_host(target),
        args,
        output: Output::Inherit,
    };
    match target {
        Target::Js => run_with(&input, &options, target, &mut CoreJs),
        Target::Native => run_with(&input, &options, target, &mut Native::default()),
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
    // ends. The command follows them, so it is the last line before the program's own output.
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let (analysis, output) = emela_driver::run(
        &OsFs,
        input,
        &mut Lower::default().with_target(target),
        backend,
        options,
        |analysis, command| {
            diagnostics_code = report(analysis);
            reported = analysis.diagnostics.len();
            if let Some(command) = command {
                eprintln!("running `{}`", command.command_line());
            }
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
    reporter: Option<ReporterArg>,
    experimental: Vec<String>,
) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path, true, experimental) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let Some(target) = resolve_target(&input, target, Target::default()) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = TestOptions {
        out_dir: out_dir.unwrap_or_else(|| emela_driver::default_test_out_dir_for(&input, target)),
        js_host: js_host(target),
        filter,
        output: Output::Inherit,
        reporter: match reporter {
            Some(reporter) => reporter.into(),
            None => Reporter::detect(
                emela_driver::runs_in_agent(|name| std::env::var_os(name)),
                std::io::stdout().is_terminal(),
            ),
        },
        color: stdout_color(),
    };
    match target {
        Target::Js => test_with(&input, &options, target, &mut CoreJs),
        Target::Native => test_with(&input, &options, target, &mut Native::default()),
        Target::Wasm => test_with(&input, &options, target, &mut CoreWasm),
    }
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
        let Some(input) = resolve_input(&fs, Path::new("std/doc.emel"), false, Vec::new()) else {
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
        let Some(input) = resolve_input(&OsFs, &path_or_current(path), false, Vec::new()) else {
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

/// The command of the JS host (`[cmd.build]`). W0902 is reported here when `EMELA_NODE` was read
/// and `target` runs on the JS host, before the diagnostics of the program.
fn js_host(target: Target) -> PathBuf {
    let host = emela_driver::js_host();
    if target == Target::Js {
        print_diagnostics(host.warning.as_slice(), &SourceDb::new());
    }
    host.program
}

/// `emela doctor` (`[cmd.doctor]`). The required target is `--target`, else the build target of
/// the project that contains `path`; `Pome.toml` is not read when `--target` is given, so a broken
/// manifest does not stop the report.
fn doctor(path: &Path, target: Option<TargetArg>, format: DoctorFormatArg) -> ExitCode {
    let required = match target {
        Some(target) => Some(target.select()),
        None => match emela_driver::enclosing_project_target(&OsFs, path) {
            Ok(target) => target,
            Err(diagnostic) => {
                print_diagnostics(&[diagnostic], &SourceDb::new());
                return ExitCode::from(DIAGNOSTIC_FAILURE);
            }
        },
    };
    // The report resolves the JS host, so reading `EMELA_NODE` is reported here as well.
    js_host(Target::Js);
    let (output, code) = emela_driver::doctor(required, VERSION, format.into(), stdout_color());
    let _ = std::io::stdout().lock().write_all(output.as_bytes());
    ExitCode::from(code)
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
/// test root and `[dev-dependencies]` are read too (spec 13.1). `experimental` are the names of
/// the flags `--experimental-<name>` (`[cmd.build]`).
fn resolve_input(
    fs: &dyn FileSystem,
    path: &Path,
    tests: bool,
    experimental: Vec<String>,
) -> Option<Input> {
    let mut input = match Input::resolve(fs, path) {
        Ok(input) if tests => input.with_tests(fs),
        Ok(input) => input,
        Err(diagnostic) => {
            print_diagnostics(&[diagnostic], &SourceDb::new());
            return None;
        }
    };
    input.experimental = experimental;
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

/// [`CoreC`] with progress while the C compiler runs: the only slow step of a native build.
/// The status line lives inside `finish`, so it is gone before the diagnostics and the program
/// write anything.
#[derive(Default)]
struct Native(CoreC);

impl Native {
    fn compile(
        &mut self,
        finish: impl FnOnce(&mut CoreC) -> Result<PathBuf, Vec<Diagnostic>>,
    ) -> Result<PathBuf, Vec<Diagnostic>> {
        let cc = emela_driver::cc_command().0;
        let cc = cc.display();
        let line = progress::StatusLine::stderr();
        let started = std::time::Instant::now();
        line.set(format!("compiling C with {cc}"));
        let built = finish(&mut self.0);
        if built.is_ok() {
            line.keep(&format!(
                "compiled C with {cc} ({})",
                progress::seconds(started.elapsed())
            ));
        }
        built
    }
}

impl<P> Backend<P> for Native
where
    CoreC: Backend<P>,
{
    fn emit(&mut self, program: &P, analysis: &Analysis) -> Result<BuildOutput, Vec<Diagnostic>> {
        self.0.emit(program, analysis)
    }

    fn finish(&mut self, out_dir: &Path, entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        self.compile(|c| Backend::<P>::finish(c, out_dir, entry))
    }

    fn runner(&self) -> Runner {
        Backend::<P>::runner(&self.0)
    }
}

impl<P> TestBackend<P> for Native
where
    CoreC: TestBackend<P>,
{
    fn test_names(&self, program: &P) -> Vec<String> {
        self.0.test_names(program)
    }

    fn emit_tests(
        &mut self,
        program: &P,
        selected: &[usize],
        analysis: &Analysis,
    ) -> Result<BuildOutput, Vec<Diagnostic>> {
        self.0.emit_tests(program, selected, analysis)
    }

    fn finish(&mut self, out_dir: &Path, entry: PathBuf) -> Result<PathBuf, Vec<Diagnostic>> {
        self.compile(|c| TestBackend::<P>::finish(c, out_dir, entry))
    }

    fn runner(&self) -> Runner {
        TestBackend::<P>::runner(&self.0)
    }
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
    std::io::stderr().is_terminal() && no_color_unset()
}

/// Color for the `pretty` reporter: only when stdout is a terminal and `NO_COLOR` is unset or
/// empty (`[cmd.test.output]`).
fn stdout_color() -> bool {
    std::io::stdout().is_terminal() && no_color_unset()
}

fn no_color_unset() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(args: &[&str]) -> (Vec<String>, Vec<String>) {
        let (rest, names) = take_experimental_flags(args.iter().map(OsString::from).collect());
        (
            rest.into_iter().map(|a| a.into_string().unwrap()).collect(),
            names,
        )
    }

    /// `[cmd.build]`: the flags of `check`, `build`, `run` and `test` are taken out wherever they
    /// are, except after `--`; other commands keep them, so clap rejects them.
    #[test]
    fn takes_the_flags_of_the_four_commands() {
        assert_eq!(
            take(&[
                "emela",
                "run",
                "--experimental-a",
                "x.emel",
                "--experimental-b",
                "--",
                "--experimental-c"
            ]),
            (
                vec![
                    "emela".into(),
                    "run".into(),
                    "x.emel".into(),
                    "--".into(),
                    "--experimental-c".into()
                ],
                vec!["a".into(), "b".into()]
            )
        );
        for command in EXPERIMENTAL_COMMANDS {
            assert_eq!(take(&["emela", command, "--experimental-a"]).1, ["a"]);
        }
        assert_eq!(
            take(&["emela", "fmt", "--experimental-a"]),
            (
                vec!["emela".into(), "fmt".into(), "--experimental-a".into()],
                vec![]
            )
        );
        // Before the command, it is not a flag of the command.
        assert_eq!(
            take(&["emela", "--experimental-a", "check"]).1,
            Vec::<String>::new()
        );
    }

    /// `[cmd.build]`: `--help` of `check`, `build`, `run` and `test` lists a flag for each
    /// experimental feature; other commands do not.
    #[test]
    fn help_lists_the_flags_of_the_features() {
        let features = [emela_driver::Feature {
            name: "some-feature",
            since: "0.29.0",
            stage: emela_driver::Stage::Experimental,
            summary: "what it changes",
        }];
        let mut command = with_experimental_flags(Cli::command(), &[&features[0]]);
        for name in EXPERIMENTAL_COMMANDS {
            let help = command
                .find_subcommand_mut(name)
                .unwrap()
                .render_help()
                .to_string();
            assert!(
                help.contains("--experimental-some-feature")
                    && help.contains("Enable the experimental feature `some-feature` (since 0.29.0): what it changes"),
                "{name}: {help}"
            );
        }
        let help = command
            .find_subcommand_mut("fmt")
            .unwrap()
            .render_help()
            .to_string();
        assert!(!help.contains("--experimental-"), "{help}");
    }
}
