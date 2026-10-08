//! 診断の収集と表示．色なしの出力をスナップショットで見る．

use std::path::Path;

use emela_driver::{
    Analysis, Checked, Diagnostic, FileId, Frontend, Input, LexOnly, Location, MemoryFiles, Parsed,
    SourceDb, SourceFile, Span, check, render,
};
use emela_resolve::{Import, ModuleId, ModuleName};
use emela_syntax::Lexed;
use emela_types::{InferCtx, Prim, Ty, TyCons};
use line_index::{TextRange, TextSize};

fn check_files(files: &[(&str, &str)], path: &str, frontend: &mut impl Frontend) -> Analysis {
    let fs = MemoryFiles::new(files.iter().copied());
    let input = Input::resolve(&fs, Path::new(path)).unwrap();
    check(&fs, &input, frontend).0
}

#[test]
fn collects_every_broken_file() {
    let analysis = check_files(
        &[
            ("app/src/main.emel", "let x = \"abc\nlet y = $\n"),
            ("app/src/http/client.emel", "let n = 3px\n"),
            ("app/src/http/Server.emel", "let ok = 1\n"),
            ("app/src/My-Lib/util.emel", "let ok = 1\n"),
            ("app/src/json.emel", "let fine = 1\n"),
        ],
        "app",
        &mut LexOnly,
    );
    assert_eq!(analysis.error_count(), 5);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn clean_project_has_no_output() {
    let analysis = check_files(&[("app/src/main.emel", "let x = 1\n")], "app", &mut LexOnly);
    assert!(analysis.diagnostics.is_empty());
    assert_eq!(render(&analysis.diagnostics, &analysis.sources, false), "");
    assert!(analysis.entry.is_some());
}

#[test]
fn invalid_entry_name() {
    let analysis = check_files(
        &[("app/src/Main.emel", "")],
        "app/src/Main.emel",
        &mut LexOnly,
    );
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

/// `import A.B` の行だけを読む仮のパーサ．差し込み口を通して import の検査を確かめる．
struct ImportLines {
    checked: bool,
}

impl Frontend for ImportLines {
    type Program = Vec<ModuleId>;

    fn parse(&mut self, _: ModuleId, _: FileId, source: &SourceFile, _: &Lexed) -> Parsed {
        let mut parsed = Parsed::default();
        let mut offset = 0;
        for line in source.text().split_inclusive('\n') {
            if let Some(path) = line.trim_end().strip_prefix("import ") {
                parsed.imports.push(Import {
                    path: ModuleName::parse(path),
                    range: TextRange::at(
                        TextSize::new(offset),
                        TextSize::new(line.trim_end().len() as u32),
                    ),
                });
            }
            offset += line.len() as u32;
        }
        parsed
    }

    fn check(&mut self, _: &Analysis, order: &[ModuleId]) -> Checked<Vec<ModuleId>> {
        self.checked = true;
        Checked {
            program: Some(order.to_vec()),
            diagnostics: Vec::new(),
        }
    }
}

#[test]
fn import_graph_through_frontend() {
    let mut frontend = ImportLines { checked: false };
    let analysis = check_files(
        &[
            ("app/src/main.emel", "import Http.Client\nimport Missing\n"),
            ("app/src/http/client.emel", "import Json\n"),
            ("app/src/json.emel", "import Http.Client\n"),
        ],
        "app",
        &mut frontend,
    );
    // 循環があると依存順がないので，型検査は呼ばない．
    assert!(!frontend.checked);
    insta::assert_snapshot!(render(&analysis.diagnostics, &analysis.sources, false));
}

#[test]
fn check_runs_in_dependency_order() {
    let mut frontend = ImportLines { checked: false };
    let fs = MemoryFiles::new([("src/main.emel", "import Json\n"), ("src/json.emel", "")]);
    let input = Input::resolve(&fs, Path::new(".")).unwrap();
    let (analysis, program) = check(&fs, &input, &mut frontend);
    assert!(analysis.diagnostics.is_empty());
    let names: Vec<String> = program
        .unwrap()
        .iter()
        .map(|&id| analysis.module(id).name.to_string())
        .collect();
    assert_eq!(names, ["Json", "Main"]);
}

#[test]
fn type_errors() {
    let mut sources = SourceDb::new();
    let file = sources.add(
        "src/main.emel",
        "let a: Int = \"s\"\nlet b = [1] == [\"s\"]\nlet f = fn(x) { x(x) }\n",
    );
    let span = |start: u32, end: u32| Span::new(file, TextRange::new(start.into(), end.into()));
    let cons = TyCons::new();
    let mut ctx = InferCtx::new();
    let int = Ty::Prim(Prim::Int);
    let string = Ty::Prim(Prim::String);

    let top = ctx.unify(&int, &string).unwrap_err();
    let nested = ctx
        .unify(&Ty::list(int.clone()), &Ty::list(string.clone()))
        .unwrap_err();
    let var = ctx.new_var();
    let infinite = ctx
        .unify(&var, &Ty::func([var.clone()], int.clone()))
        .unwrap_err();

    let diagnostics = vec![
        Diagnostic::from_type_error(&top, &cons, span(13, 16)),
        Diagnostic::from_type_error(&nested, &cons, span(25, 36)),
        Diagnostic::from_type_error(&infinite, &cons, span(56, 60)),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn labels_warnings_and_locations() {
    let mut sources = SourceDb::new();
    let file = sources.add("src/main.emel", "let x = 1\nlet x = 2\n");
    let diagnostics = vec![
        Diagnostic::warning("`x` を覆い隠している")
            .with_span(Span::new(file, TextRange::new(14.into(), 15.into())))
            .with_label(
                Span::new(file, TextRange::new(4.into(), 5.into())),
                "前の `x` はここ",
            )
            .with_note("名前を変えるか，前の `x` を使う"),
        Diagnostic::error("ファイル全体の問題").at(Location::File(file)),
        Diagnostic::error("ディレクトリを読めない").at(Location::Path("src/secret".into())),
        Diagnostic::error("`node` が見つからない").with_note("Node.js を入れる"),
    ];
    insta::assert_snapshot!(render(&diagnostics, &sources, false));
}

#[test]
fn color_output_has_escapes() {
    let mut sources = SourceDb::new();
    let file = sources.add("a.emel", "$");
    let diagnostics = [Diagnostic::error("認識できない文字")
        .with_span(Span::new(file, TextRange::new(0.into(), 1.into())))];
    assert!(render(&diagnostics, &sources, true).contains('\x1b'));
    assert!(!render(&diagnostics, &sources, false).contains('\x1b'));
}
