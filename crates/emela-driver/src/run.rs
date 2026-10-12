//! 出力した JS を node で実行する．

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use crate::code;
use crate::diagnostic::Diagnostic;

/// node の実行ファイルを差し替える環境変数．なければ PATH の `node` を使う．
pub const NODE_ENV: &str = "EMELA_NODE";
/// defect のクラスの `name`．emela-codegen-js のランタイムの `$Defect` に合わせる．
pub const DEFECT_NAME: &str = "Defect";
/// defect で止まったときの終了コード．
pub const DEFECT_EXIT_CODE: i32 = 101;
/// main まで達したエラーで止まったときの終了コード（14.3）．
pub const ERROR_EXIT_CODE: i32 = 1;
/// main の継続が中断されたまま巻き戻ったときの終了コード（14.5）．
pub const INTERRUPTED_EXIT_CODE: i32 = 130;
/// 失敗の値（emela-codegen-js のランタイムの `$Fail`）を見分ける `Symbol.for` の鍵．
/// 鍵の関数はエラーの値の表示を返す．
pub const FAIL_KEY: &str = "emela.fail";

/// エントリを読み込んで `main` を呼び，main まで達したエラーと defect を終了コードに写す
/// 小さな起動用モジュール．node の `-e` に渡すので，ファイルには書き出さない．
///
/// エントリが `main` を export していなければ，読み込むだけにする（読み込みで動く JS 向け）．
/// `main` の返り値は，失敗（`$Fail`）なら `error: <エラーの値の表示>` を出して終了コード 1，
/// それ以外は捨てる（14.3）．
///
/// 中断しうる main（ジェネレータを返す）は，エントリが公開するランタイムの `$start` で進め，
/// イベントループで待つ．resume されないままループが空になったら，`interrupted` を出して
/// 終了コード 130 で終わる（14.5）．中断しない main の中で resume されずに捨てられた継続
/// （ランタイムの `$Interrupted`）も同じ．defect は，ホストのコールバックの中で起きたものも
/// `defect: <メッセージ>` を出して終了コード 101 で終わる（7.5）．
const LAUNCHER: &str = r#"
import { pathToFileURL } from "node:url";
let over = false;
function finish(value) {
  over = true;
  const show = value?.[Symbol.for("emela.fail")];
  if (typeof show === "function") {
    process.stderr.write(`error: ${show.call(value)}\n`);
    process.exitCode = 1;
  }
}
function crash(error) {
  if (over) return;
  over = true;
  if (error instanceof Error && error.name === "Interrupted") {
    process.exitCode = 130;
    process.stderr.write("interrupted\n", () => process.exit(130));
  } else if (error instanceof Error && error.name === "Defect") {
    process.exitCode = 101;
    process.stderr.write(`defect: ${error.message}\n`, () => process.exit(101));
  } else {
    process.exitCode = 1;
    process.stderr.write(`${error?.stack ?? String(error)}\n`, () => process.exit(1));
  }
}
process.on("uncaughtException", crash);
process.on("beforeExit", () => {
  if (over) return;
  over = true;
  process.stderr.write("interrupted\n");
  process.exitCode = 130;
});
try {
  const entry = await import(pathToFileURL(process.argv[1]).href);
  if (typeof entry.main === "function") {
    const value = entry.main();
    if (typeof entry.$start === "function" && Object.prototype.toString.call(value) === "[object Generator]") {
      entry.$start(value, (ok, result) => (ok ? finish(result) : crash(result)));
    } else {
      finish(value);
    }
  } else {
    over = true;
  }
} catch (error) {
  crash(error);
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
    result.map_err(|err| spawn_error(node, &err))
}

/// node を起動できなかった診断．
pub(crate) fn spawn_error(node: &Path, err: &io::Error) -> Diagnostic {
    match err.kind() {
        io::ErrorKind::NotFound => Diagnostic::error(format!("`{}` not found", node.display()))
            .with_code(code::NODE_UNAVAILABLE)
            .with_note(format!(
                "install Node.js and add it to PATH, or set {NODE_ENV} to the node executable"
            )),
        _ => Diagnostic::error(format!("cannot start `{}`: {err}", node.display()))
            .with_code(code::NODE_UNAVAILABLE),
    }
}

pub(crate) fn exit_code(status: ExitStatus) -> i32 {
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
        assert!(LAUNCHER.contains(&format!("process.exitCode = {ERROR_EXIT_CODE};")));
        assert!(LAUNCHER.contains(&format!("process.exitCode = {INTERRUPTED_EXIT_CODE};")));
        assert!(LAUNCHER.contains(&format!("Symbol.for(\"{FAIL_KEY}\")")));
        // ランタイムの `$Fail` とテストランナーも同じ鍵を使う．
        let key = format!("Symbol.for(\"{FAIL_KEY}\")");
        assert!(emela_codegen_js::RUNTIME.contains(&key));
        assert!(emela_codegen_js::TEST_RUNNER.contains(&key));
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
        assert_eq!(err.message, "`/nonexistent/emela-test-node` not found");
        assert_eq!(err.notes.len(), 1);
    }
}
