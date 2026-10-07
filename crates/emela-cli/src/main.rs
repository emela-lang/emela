use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "emela", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// JS か WASM へコンパイルする
    Build { path: PathBuf },
    /// コンパイルして実行する
    Run { path: PathBuf },
    /// @test の付いた関数を実行する
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

fn main() {
    match Cli::parse().command {
        Command::Build { .. } => todo!("build"),
        Command::Run { .. } => todo!("run"),
        Command::Test { .. } => todo!("test"),
        Command::Fmt { .. } => todo!("fmt"),
        Command::Lsp => todo!("lsp"),
    }
}
