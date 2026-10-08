//! 出力した JS を node で実行する．

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::diagnostic::Diagnostic;

/// node の実行ファイルを差し替える環境変数．なければ PATH の `node` を使う．
pub const NODE_ENV: &str = "EMELA_NODE";
/// defect のクラスの `name`．emela-codegen-js のランタイムの `$Defect` に合わせる．
pub const DEFECT_NAME: &str = "Defect";
/// defect で止まったときの終了コード．
pub const DEFECT_EXIT_CODE: i32 = 101;

/// エントリを読み込んで `main` を呼び，defect を終了コードに写す小さな起動用モジュール．
/// node の `-e` に渡すので，ファイルには書き出さない．
///
/// エントリが `main` を export していなければ，読み込むだけにする（読み込みで動く JS 向け）．
/// `main` の返り値は使わない．
const LAUNCHER: &str = r#"
import { pathToFileURL } from "node:url";
try {
  const entry = await import(pathToFileURL(process.argv[1]).href);
  if (typeof entry.main === "function") {
    await entry.main();
  }
} catch (error) {
  if (error instanceof Error && error.name === "Defect") {
    process.stderr.write(`defect: ${error.message}\n`);
    process.exitCode = 101;
  } else {
    throw error;
  }
}
"#;

/// 子プロセスの標準入出力の扱い．
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// emela の標準入出力をそのまま渡す（CLI）．
    Inherit,
    /// 標準出力と標準エラーを集める（テスト）．
    Capture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    /// node の終了コード．シグナルで止まったときは 128 + シグナル番号．
    pub code: i32,
    /// [`Output::Capture`] のときだけ中身が入る．
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// `EMELA_NODE` があればそれ，なければ `node`．
pub fn node_program() -> PathBuf {
    std::env::var_os(NODE_ENV)
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("node"), PathBuf::from)
}

/// `entry` の `.mjs` を node で実行する．`args` はプログラムへの引数．
pub fn run_node(
    node: &Path,
    entry: &Path,
    args: &[OsString],
    output: Output,
) -> Result<RunOutput, Diagnostic> {
    let mut command = Command::new(node);
    command
        .arg("--input-type=module")
        .arg("-e")
        .arg(LAUNCHER)
        .arg("--")
        .arg(entry)
        .args(args)
        .stdin(Stdio::inherit());
    let result = match output {
        Output::Inherit => command.status().map(|status| RunOutput {
            code: exit_code(status),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }),
        Output::Capture => command.output().map(|out| RunOutput {
            code: exit_code(out.status),
            stdout: out.stdout,
            stderr: out.stderr,
        }),
    };
    result.map_err(|err| match err.kind() {
        io::ErrorKind::NotFound => Diagnostic::error(format!(
            "`{}` が見つからない",
            node.display()
        ))
        .with_note(format!(
            "Node.js を入れて PATH を通すか，環境変数 {NODE_ENV} で node の実行ファイルを指定する"
        )),
        _ => Diagnostic::error(format!("`{}` を起動できない: {err}", node.display())),
    })
}

fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_uses_constants() {
        assert!(LAUNCHER.contains(&format!("\"{DEFECT_NAME}\"")));
        assert!(LAUNCHER.contains(&format!("process.exitCode = {DEFECT_EXIT_CODE};")));
    }

    #[test]
    fn missing_node() {
        let err = run_node(
            Path::new("/nonexistent/emela-test-node"),
            Path::new("main.mjs"),
            &[],
            Output::Capture,
        )
        .unwrap_err();
        assert_eq!(err.message, "`/nonexistent/emela-test-node` が見つからない");
        assert_eq!(err.notes.len(), 1);
    }
}
