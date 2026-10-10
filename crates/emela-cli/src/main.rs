use clap::{Parser, Subcommand};
use emela_driver::{
    Analysis, Diagnostic, FileSystem, Input, NoJsBackend, OsFs, Output, Resolve, RunOptions,
    SourceDb,
};
use std::ffi::OsString;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "emela", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 診断だけを出す
    Check {
        /// エントリのファイルか，プロジェクトのディレクトリ（既定は `.`）
        path: Option<PathBuf>,
    },
    /// JS へコンパイルする
    Build {
        path: Option<PathBuf>,
        /// 出力先（既定は `<プロジェクト>/target/emela/js`）
        #[arg(long)]
        out_dir: Option<PathBuf>,
    },
    /// コンパイルして node で実行する
    Run {
        path: Option<PathBuf>,
        #[arg(long)]
        out_dir: Option<PathBuf>,
        /// プログラムへの引数（`--` の後に書く）
        #[arg(last = true)]
        args: Vec<OsString>,
    },
    /// `@test` の付いた関数を実行する
    Test { path: Option<PathBuf> },
    /// ソースを整形する
    Fmt {
        paths: Vec<PathBuf>,
        #[arg(long)]
        check: bool,
    },
    /// 言語サーバーを標準入出力で起動する
    Lsp,
}

/// 診断でエラーになったときの終了コード．
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
        Command::Test { .. } => todo!("test"),
        Command::Fmt { .. } => todo!("fmt"),
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
    let (analysis, _) = emela_driver::check(&OsFs, &input, &mut Resolve::default());
    report(&analysis)
}

fn build(path: &Path, out_dir: Option<PathBuf>) -> ExitCode {
    let Some(input) = resolve_input(&OsFs, path) else {
        return ExitCode::from(DIAGNOSTIC_FAILURE);
    };
    let out_dir = out_dir.unwrap_or_else(|| input.default_out_dir());
    // 型検査と JS の出力の段ができたら Resolve と NoJsBackend を差し替える．
    let (analysis, _) = emela_driver::build(
        &OsFs,
        &input,
        &mut Resolve::default(),
        &mut NoJsBackend,
        &out_dir,
    );
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
    // 診断は node を起動する前に出す．終わらないプログラムでも警告が先に見える．
    let mut diagnostics_code = ExitCode::SUCCESS;
    let mut reported = 0;
    let (analysis, output) = emela_driver::run(
        &OsFs,
        &input,
        &mut Resolve::default(),
        &mut NoJsBackend,
        &options,
        |analysis| {
            diagnostics_code = report(analysis);
            reported = analysis.diagnostics.len();
        },
    );
    match output {
        // node の終了コードをそのまま返す．0〜255 の外は 8 ビットに切る（シェルと同じ）．
        Some(output) => ExitCode::from(output.code as u8),
        None => {
            // node を起動できなかった診断は，前に出した分の後に足されている．
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

fn lsp() -> ExitCode {
    // フロントエンドは check と同じものを使う（同じ診断を出すため）．
    match emela_lsp::run_stdio(Resolve::default) {
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

/// 診断を標準エラーに出し，エラーがあれば 1 を返す．
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
    let stderr = std::io::stderr();
    let color = stderr.is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let text = emela_driver::render(diagnostics, sources, color);
    let _ = stderr.lock().write_all(text.as_bytes());
}
