use clap::{CommandFactory, Parser, Subcommand};
use emela_driver::{
    Analysis, CoreJs, Diagnostic, FileSystem, Input, Lower, LowerTests, OsFs, Output, RunOptions,
    SourceDb, TestOptions, Typecheck,
};
use std::ffi::OsString;
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
    },
    /// Compile to JavaScript under `target/emela/js`
    Build {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Output directory [default: <project>/target/emela/js]
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// Compile to JavaScript and run it with Node.js
    Run {
        /// Entry file or project directory [default: .]
        path: Option<PathBuf>,
        /// Output directory [default: <project>/target/emela/js]
        #[arg(long)]
        out_dir: Option<PathBuf>,
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
        /// Output directory [default: <project>/target/emela/test]
        #[arg(long)]
        out_dir: Option<PathBuf>,
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
    /// Start the language server over standard input and output
    Lsp,
}

const DIAGNOSTIC_FAILURE: u8 = 1;

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Check { path } => check(&path_or_current(path)),
        Command::Build { path, out_dir } => build(&path_or_current(path), out_dir),
        Command::Run {
            path,
            out_dir,
            args,
        } => run(&path_or_current(path), out_dir, args),
        Command::Test {
            path,
            filter,
            out_dir,
        } => test(&path_or_current(path), filter, out_dir),
        Command::Fmt { paths, check } => fmt(paths, check),
        Command::Lsp => lsp(),
    }
}

fn path_or_current(path: Option<PathBuf>) -> PathBuf {
    path.unwrap_or_else(|| PathBuf::from("."))
}

fn check(path: &Path) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let (analysis, _) = emela_driver::check(&OsFs, &input, &mut Typecheck::default());
    report(&analysis)
}

fn build(path: &Path, out_dir: Option<PathBuf>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let out_dir = out_dir.unwrap_or_else(|| input.default_out_dir());
    let (analysis, _) =
        emela_driver::build(&OsFs, &input, &mut Lower::default(), &mut CoreJs, &out_dir);
    report(&analysis)
}

fn run(path: &Path, out_dir: Option<PathBuf>, args: Vec<OsString>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = RunOptions {
        out_dir: out_dir.unwrap_or_else(|| input.default_out_dir()),
        node: emela_driver::node_program(),
        args,
        output: Output::Inherit,
    };
    // Diagnostics go out before node starts, so warnings show first even if the program never
    // ends.
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let (analysis, output) = emela_driver::run(
        &OsFs,
        &input,
        &mut Lower::default(),
        &mut CoreJs,
        &options,
        |analysis| {
            diagnostics_code = report(analysis);
            reported = analysis.diagnostics.len();
        },
    );
    match output {
        // Pass node's exit code through, truncated to 8 bits like a shell does.
        Some(output) => ExitCode::from(output.code as u8),
        None => {
            // Diagnostics about failing to start node were appended after those already printed.
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

fn test(path: &Path, filter: Option<String>, out_dir: Option<PathBuf>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let options = TestOptions {
        out_dir: out_dir.unwrap_or_else(|| emela_driver::default_test_out_dir(&input)),
        node: emela_driver::node_program(),
        filter,
        output: Output::Inherit,
    };
    // Results go to stdout, diagnostics to stderr. Diagnostics go out before node starts.
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let stdout = std::io::stdout();
    let (analysis, report) = emela_driver::test(
        &OsFs,
        &input,
        &mut LowerTests::default(),
        &mut CoreJs,
        &options,
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

fn resolve_input(fs: &dyn FileSystem, path: &Path) -> Option<Input> {
    match Input::resolve(fs, path) {
        Ok(input) => Some(input),
        Err(diagnostic) => {
            print_diagnostics(&[diagnostic], &SourceDb::new());
            None
        }
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
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}
