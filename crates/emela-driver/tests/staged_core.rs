//! 同梱の読み込みから外している保留中の core のソース（`Console`，`JsConsole`）．
//!
//! 名前解決と型検査で診断が出ないことを確かめる．`#[ignore]` のテスト（node で動かす）は，
//! lowering と JS の出力がエフェクトを扱えるようになって保留中のソースを有効にした後に通す
//! （手順は emela-core の `NOTES.md` の「保留中のソース」）．
//! `Option.or_fail` は E が入ったので同梱した（`tests/run.rs` で動かしている）．main はそれも使う．

use std::path::{Path, PathBuf};

use emela_driver::{
    Analysis, CoreJs, CoreSource, Frontend, Input, Lower, MemoryFiles, Output, Resolve, RunOptions,
    Typecheck, check, render, run,
};

/// `Option.or_fail` で Option を失敗に変え，`JsConsole` で供給した Console に書く．
const MAIN: &str = r#"error NotFound(id: Int)

fn name(id: Int) -> String fails NotFound {
  List.get(["alice"], id) |> Option.or_fail(NotFound(id:))
}

fn main() {
  with JsConsole {
    console = use Console
    console.print_line(name(0))
    console.print_line(name(1) escape { NotFound(id:) -> "no #{id}" })
    (use Console).print_error_line("done")
  }
}
"#;

fn check_main(frontend: &mut impl Frontend) -> Analysis {
    let fs = MemoryFiles::new([("home/main.emel", MAIN)]);
    let input = Input::resolve(&fs, Path::new("home/main.emel")).unwrap();
    check(&fs, &input, frontend).0
}

#[test]
fn staged_core_resolves_cleanly() {
    let mut frontend = Resolve::default().with_core(CoreSource::staged());
    let analysis = check_main(&mut frontend);
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
fn staged_core_typechecks_cleanly() {
    let mut frontend = Typecheck::new(Resolve::default().with_core(CoreSource::staged()));
    let analysis = check_main(&mut frontend);
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
}

#[test]
#[ignore = "lowering と JS の出力が use，with，handler を扱えるまで（NOTES.md の「保留中のソース」）"]
fn staged_core_runs_on_node() {
    let sources = [("app/Pome.toml", ""), ("app/src/main.emel", MAIN)];
    let fs = MemoryFiles::new(sources);
    let input = Input::resolve(&fs, Path::new("app")).unwrap();
    let options = RunOptions {
        out_dir: std::env::temp_dir()
            .join(format!("emela-driver-staged-core-{}", std::process::id())),
        node: PathBuf::from("node"),
        args: Vec::new(),
        output: Output::Capture,
    };
    let mut frontend = Lower::new(Typecheck::new(
        Resolve::default().with_core(CoreSource::staged()),
    ));
    let (analysis, output) = run(&fs, &input, &mut frontend, &mut CoreJs, &options, |_| {});
    let _ = std::fs::remove_dir_all(&options.out_dir);
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    let output = output.unwrap();
    assert_eq!(
        (
            output.code,
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ),
        (0, "alice\nno 1\n".to_owned(), "done\n".to_owned())
    );
}
