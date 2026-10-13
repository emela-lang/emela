//! 同梱の `Std.FileSystem` と `Std.Path`（仕様 15.2）を `emela test` の経路で確かめる．
//! テストのソースは `file_system/` の `.emel` で，どのテスト関数も通ることを見る．

use std::path::{Path, PathBuf};

use emela_driver::{
    CoreJs, Input, LowerTests, MemoryFiles, Output, TestOptions, TestOutcome, test,
};

/// テストごとの一時ディレクトリ．前の実行の残りは消す．
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("emela-driver-fs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// `main.emel` を1つ持つプロジェクトのテストを流し，失敗したテストを（名前，結果）で返す．
fn failures(name: &str, main: &str) -> Vec<(String, TestOutcome)> {
    let fs = MemoryFiles::new([("app/Pome.toml", ""), ("app/src/main.emel", main)]);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = TestOptions {
        out_dir: temp_dir(&format!("{name}-out")),
        node: PathBuf::from("node"),
        filter: None,
        output: Output::Capture,
    };
    let mut out = Vec::new();
    let (analysis, report) = test(
        &fs,
        &input,
        &mut LowerTests::default(),
        &mut CoreJs,
        &options,
        |_| {},
        &mut out,
    );
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(analysis.diagnostics, []);
    let report = report.unwrap();
    assert!(!report.results.is_empty());
    report
        .results
        .into_iter()
        .filter(|r| !matches!(r.outcome, TestOutcome::Passed))
        .map(|r| (r.name, r.outcome))
        .collect()
}

#[test]
fn test_file_system_follows_the_rules() {
    let main = include_str!("file_system/test_file_system.emel");
    assert_eq!(failures("test-fs", main), []);
}

#[test]
fn system_file_system_on_a_temporary_directory() {
    let root = temp_dir("system-fs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("invalid.bin"), [0x66, 0xff, 0x6f]).unwrap();
    std::fs::write(root.join("bom.txt"), "\u{feff}x").unwrap();
    // `/` 区切りにする（Std.Path は `/` だけを扱う）．
    let root_str = root.to_string_lossy().replace('\\', "/");
    let main = include_str!("file_system/system_file_system.emel").replace("__ROOT__", &root_str);
    let result = failures("system-fs", &main);
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(result, []);
}

#[test]
fn path_functions() {
    let main = include_str!("file_system/path.emel");
    assert_eq!(failures("path", main), []);
}
