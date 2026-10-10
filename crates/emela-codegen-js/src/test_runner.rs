//! テストの起動用モジュール（仕様 13章）．
//!
//! 出力したモジュールを import し，テスト関数を1つずつ呼んで結果を標準出力に書く．
//! 結果の行の形は `test_runner.mjs` の先頭を参照．

use std::fmt::Write as _;

use crate::emit::js_string;

/// テストランナーの本文（ES モジュール）．起動用モジュールに埋め込む．
pub const TEST_RUNNER: &str = include_str!("test_runner.mjs");

/// テストランナーが標準出力に書く結果の行の頭．後ろに JSON が続く．
pub const TEST_EVENT_PREFIX: &str = "\u{1e}emela-test ";

/// 起動用モジュールが呼ぶテスト．
#[derive(Debug, Clone, Copy)]
pub struct TestEntry<'a> {
    /// 結果に出す名前．
    pub name: &'a str,
    /// `module` が export している関数の名前（Emela の名前）．
    pub export: &'a str,
}

/// `module`（起動用モジュールからの相対パス，`./main.mjs` など）の `tests` を順に呼ぶ起動用モジュール．
/// node でそのまま実行できる．
pub fn emit_test_main(module: &str, tests: &[TestEntry]) -> String {
    let mut out = String::from("// Emela が生成したテストの起動用モジュール．\n");
    let _ = writeln!(out, "import * as $m from {};", js_string(module));
    out.push('\n');
    for line in TEST_RUNNER.lines() {
        out.push_str(line.strip_prefix("export ").unwrap_or(line));
        out.push('\n');
    }
    out.push_str("\nawait $runTests([\n");
    for t in tests {
        let _ = writeln!(
            out,
            "  [{}, $m[{}]],",
            js_string(t.name),
            js_string(t.export)
        );
    }
    out.push_str("]);\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_matches_runner() {
        let escaped = TEST_EVENT_PREFIX.replace('\u{1e}', "\\x1e");
        assert!(TEST_RUNNER.contains(&format!("\"{escaped}\"")));
    }

    #[test]
    fn main_imports_module_and_lists_tests() {
        let js = emit_test_main(
            "./main.mjs",
            &[
                TestEntry {
                    name: "adds",
                    export: "adds",
                },
                TestEntry {
                    name: "Util.new",
                    export: "new",
                },
            ],
        );
        assert!(js.contains("import * as $m from \"./main.mjs\";\n"));
        assert!(js.contains("  [\"adds\", $m[\"adds\"]],\n  [\"Util.new\", $m[\"new\"]],\n"));
        assert!(!js.contains("export async"));
    }
}
