//! パイプライン．モジュールを集め，字句解析し，import のグラフを検査する．
//!
//! パーサ，型検査，JS の出力はまだないので，[`Frontend`] と [`JsBackend`] で差し込む．
//! 今の段で使えるのは字句解析だけの [`LexOnly`] と，出力を持たない [`NoJsBackend`]．

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{DirEntry, Import, ImportGraph, ModuleData, ModuleId, ModuleMap, SourceFs};
use emela_syntax::Lexed;
use la_arena::ArenaMap;

use crate::diagnostic::{Diagnostic, Location, error_count};
use crate::output::{JsOutput, write_output};
use crate::run::{Output, RunOutput, run_node};
use crate::source::{FileId, FileSystem, SourceDb, SourceFile};

/// プロジェクトの目印のファイル（仕様 4.1）．0.20 では有無だけを見る．
pub const PROJECT_FILE: &str = "Pome.toml";
/// プロジェクトの中のソースのルート．
pub const SOURCE_DIR: &str = "src";
/// プロジェクトのエントリ（ソースのルートからの相対パス）．
pub const ENTRY_FILE: &str = "main.emel";
/// プロジェクト（単独ファイルならファイルのあるディレクトリ）からの，JS の既定の出力先．
pub const JS_OUT_DIR: &str = "target/emela/js";

/// コマンドラインの path から決めた，ソースのルートとエントリ（仕様 4.1）．パスはどれも絶対パス．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// モジュール名はここからの相対パスで決まる．
    pub root: PathBuf,
    /// エントリのファイル．ディレクトリを渡して `src/main.emel` がなければ `None`．
    pub entry: Option<PathBuf>,
    /// プロジェクトのディレクトリ．単独ファイルならファイルのあるディレクトリ．
    /// `target/` を置き，診断のパスはここからの相対パスで出す．
    pub base: PathBuf,
    /// `Pome.toml` のない単独ファイル．モジュールはルートの直下の `.emel` だけになる．
    pub single_file: bool,
}

impl Input {
    /// 渡されたパスを絶対パスに直し，祖先を上にたどって最初の `Pome.toml` でプロジェクトを決める．
    ///
    /// - ディレクトリで `Pome.toml` がなければエラー．エントリは `<プロジェクト>/src/main.emel`
    /// - ファイルで `Pome.toml` がなければ単独ファイル．ルートはファイルのあるディレクトリ
    /// - ファイルで `Pome.toml` があり，ファイルが `<プロジェクト>/src` の外ならエラー
    pub fn resolve(fs: &dyn FileSystem, path: &Path) -> Result<Input, Diagnostic> {
        let not_found = || Diagnostic::error(format!("`{}` が見つからない", path.display()));
        let absolute = fs.absolute(path).map_err(|_| not_found())?;
        let is_file = fs.is_file(&absolute);
        if !is_file && !fs.is_dir(&absolute) {
            return Err(not_found());
        }
        let start = if is_file {
            absolute.parent().expect("ファイルには親がある")
        } else {
            &absolute
        };
        let project = start
            .ancestors()
            .find(|dir| fs.is_file(&dir.join(PROJECT_FILE)))
            .map(Path::to_owned);

        match (project, is_file) {
            (Some(project), _) => {
                let root = project.join(SOURCE_DIR);
                let entry = if is_file {
                    if !absolute.starts_with(&root) {
                        return Err(Diagnostic::error(format!(
                            "`{}` がソースのルート `{}` の外にある",
                            path.display(),
                            root.display()
                        ))
                        .with_note(format!(
                            "プロジェクト（{PROJECT_FILE} のあるディレクトリ）のファイルは {SOURCE_DIR}/ の下に置く"
                        )));
                    }
                    Some(absolute)
                } else {
                    let entry = root.join(ENTRY_FILE);
                    fs.is_file(&entry).then_some(entry)
                };
                Ok(Input {
                    root,
                    entry,
                    base: project,
                    single_file: false,
                })
            }
            (None, true) => {
                let dir = start.to_owned();
                Ok(Input {
                    root: dir.clone(),
                    entry: Some(absolute),
                    base: dir,
                    single_file: true,
                })
            }
            (None, false) => Err(Diagnostic::error(format!(
                "`{}` にも祖先にも {PROJECT_FILE} がない",
                path.display()
            ))
            .with_note(format!(
                "プロジェクトのディレクトリに {PROJECT_FILE} を置く（中身は空でよい）．1ファイルだけなら，そのファイルを渡す"
            ))),
        }
    }

    pub fn default_out_dir(&self) -> PathBuf {
        self.base.join(JS_OUT_DIR)
    }
}

/// 単独ファイルのときの一覧．ルートの直下のファイルだけを見せ，サブディレクトリに潜らせない．
struct Shallow<'a>(&'a dyn FileSystem);

impl SourceFs for Shallow<'_> {
    fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        let mut entries = self.0.read_dir(dir)?;
        entries.retain(|entry| !entry.is_dir);
        Ok(entries)
    }
}

/// 構文解析の結果のうち，パイプラインが使うもの．構文木は [`Frontend`] の側で持つ．
#[derive(Debug, Default)]
pub struct Parsed {
    pub imports: Vec<Import>,
    pub diagnostics: Vec<Diagnostic>,
}

/// パーサと型検査の差し込み口．
pub trait Frontend {
    /// 型検査まで済んだプログラム．[`JsBackend`] に渡す．
    type Program;

    /// 1つのモジュールを構文解析する．字句解析は済んでいて，その診断はパイプラインが集める．
    /// 構文木は `self` に持っておき，[`Frontend::check`] で使う．
    fn parse(
        &mut self,
        module: ModuleId,
        file: FileId,
        source: &SourceFile,
        lexed: &Lexed,
    ) -> Parsed;

    /// 全モジュールの構文解析と import の検査の後に1度だけ呼ぶ．
    /// import が循環していると依存順がないので呼ばない．
    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program>;
}

/// 型検査の結果．診断にエラーがあっても，続けられる限りプログラムを返してよい．
#[derive(Debug)]
pub struct Checked<P> {
    pub program: Option<P>,
    pub diagnostics: Vec<Diagnostic>,
}

/// JS の出力の差し込み口．エラーが1つもないときだけ呼ぶ．
pub trait JsBackend<P> {
    fn emit(&mut self, program: &P, analysis: &Analysis) -> Result<JsOutput, Vec<Diagnostic>>;
}

/// 今ある段だけのフロントエンド．import を返さず，型検査もしない．
#[derive(Debug, Default, Clone, Copy)]
pub struct LexOnly;

impl Frontend for LexOnly {
    type Program = ();

    fn parse(&mut self, _: ModuleId, _: FileId, _: &SourceFile, _: &Lexed) -> Parsed {
        Parsed::default()
    }

    fn check(&mut self, _: &Analysis, _: &[ModuleId]) -> Checked<()> {
        Checked {
            program: Some(()),
            diagnostics: Vec::new(),
        }
    }
}

/// JS の出力がまだないことを診断にする．
#[derive(Debug, Default, Clone, Copy)]
pub struct NoJsBackend;

impl<P> JsBackend<P> for NoJsBackend {
    fn emit(&mut self, _: &P, _: &Analysis) -> Result<JsOutput, Vec<Diagnostic>> {
        Err(vec![Diagnostic::error("JS の出力はまだ実装されていない")])
    }
}

/// 解析の途中経過．診断は段の順に全部たまる．
#[derive(Debug, Default)]
pub struct Analysis {
    pub sources: SourceDb,
    pub modules: ModuleMap,
    /// モジュールのファイル．読めなかったファイルは入らない．
    pub files: ArenaMap<ModuleId, FileId>,
    pub graph: ImportGraph,
    /// エントリのモジュール．エントリがないか，モジュールにならなければ `None`．
    pub entry: Option<ModuleId>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    pub fn error_count(&self) -> usize {
        error_count(&self.diagnostics)
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    pub fn module(&self, id: ModuleId) -> &ModuleData {
        &self.modules[id]
    }
}

/// `emela check` の段．モジュールを集め，字句解析と構文解析をし，import を検査し，型検査する．
pub fn check<F: Frontend>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
) -> (Analysis, Option<F::Program>) {
    let mut analysis = Analysis::default();
    analysis.sources.set_base(&input.base);
    let (modules, resolve_diagnostics) = if input.single_file {
        emela_resolve::collect_modules(&Shallow(fs), &input.root)
    } else {
        emela_resolve::collect_modules(fs, &input.root)
    };
    analysis.modules = modules;

    let mut imports: ArenaMap<ModuleId, Vec<Import>> = ArenaMap::default();
    let mut file_diagnostics = Vec::new();
    for (module, data) in analysis.modules.iter() {
        let text = match fs.read_to_string(&data.file) {
            Ok(text) => text,
            Err(err) => {
                file_diagnostics.push(
                    Diagnostic::error(format!("ファイルを読めない: {err}"))
                        .at(Location::Path(data.file.clone())),
                );
                continue;
            }
        };
        let file = analysis.sources.add(&data.file, text);
        analysis.files.insert(module, file);
        let source = &analysis.sources[file];
        let lexed = emela_syntax::lex(source.text());
        file_diagnostics.extend(
            lexed
                .diagnostics
                .iter()
                .map(|d| Diagnostic::from_syntax(file, d)),
        );
        let parsed = frontend.parse(module, file, source, &lexed);
        file_diagnostics.extend(parsed.diagnostics);
        imports.insert(module, parsed.imports);
    }
    // 名前解決の診断は，ファイルを表に入れてから写す（ID で指せるように）．
    analysis.diagnostics.extend(
        resolve_diagnostics
            .iter()
            .map(|d| Diagnostic::from_resolve(d, &analysis.sources)),
    );
    analysis.diagnostics.extend(file_diagnostics);

    if let Some(entry) = &input.entry {
        analysis.entry = analysis
            .modules
            .iter()
            .find(|(_, data)| same_path(&data.file, entry))
            .map(|(id, _)| id);
        if analysis.entry.is_none() {
            // 名前の誤りなどでモジュールにならなかった．理由の診断は名前解決が出している．
            analysis.diagnostics.push(
                Diagnostic::error(format!(
                    "エントリ `{}` がモジュールにならない",
                    analysis.sources.display_path(entry).display()
                ))
                .at(Location::Path(entry.clone())),
            );
        }
    }

    let (graph, graph_diagnostics) = emela_resolve::build_import_graph(&analysis.modules, &imports);
    analysis.graph = graph;
    analysis.diagnostics.extend(
        graph_diagnostics
            .iter()
            .map(|d| Diagnostic::from_resolve(d, &analysis.sources)),
    );

    let Some(order) = analysis.graph.order().map(<[_]>::to_vec) else {
        return (analysis, None);
    };
    let checked = frontend.check(&analysis, &order);
    analysis.diagnostics.extend(checked.diagnostics);
    (analysis, checked.program)
}

/// `emela build` の段．検査を通ったら JS を出し，`out_dir` に書く．書いたエントリのパスを返す．
pub fn build<F: Frontend, B: JsBackend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    out_dir: &Path,
) -> (Analysis, Option<PathBuf>) {
    let (mut analysis, program) = check(fs, input, frontend);
    if input.entry.is_none() {
        analysis.diagnostics.push(Diagnostic::error(format!(
            "エントリ `{}` がない",
            analysis
                .sources
                .display_path(&input.root.join(ENTRY_FILE))
                .display()
        )));
    }
    if analysis.has_errors() {
        return (analysis, None);
    }
    let Some(program) = program else {
        // エラーなしでプログラムがないのはフロントエンドの誤り．
        analysis
            .diagnostics
            .push(Diagnostic::error("型検査がプログラムを返さなかった"));
        return (analysis, None);
    };
    let output = match backend.emit(&program, &analysis) {
        Ok(output) => output,
        Err(diagnostics) => {
            analysis.diagnostics.extend(diagnostics);
            return (analysis, None);
        }
    };
    match write_output(out_dir, &output) {
        Ok(entry) => (analysis, Some(entry)),
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

/// `emela run` の実行のしかた．
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub out_dir: PathBuf,
    /// node の実行ファイル（[`crate::node_program`]）．
    pub node: PathBuf,
    /// プログラムへの引数．
    pub args: Vec<OsString>,
    pub output: Output,
}

/// `emela run` の段．ビルドして，エントリを node で実行する．
///
/// `before_node` はビルドの後，node を起動する前に1度だけ呼ぶ（ビルドが失敗しても呼ぶ）．
/// CLI はここで診断を出す．終わらないプログラム（サーバー）でも警告が先に見えるように．
/// 診断でエラーになったか node を起動できなかったときは，実行結果が `None`．
/// node を起動できなかった診断は `before_node` の後に `Analysis` の末尾へ足す．
pub fn run<F: Frontend, B: JsBackend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &RunOptions,
    before_node: impl FnOnce(&Analysis),
) -> (Analysis, Option<RunOutput>) {
    let (mut analysis, entry) = build(fs, input, frontend, backend, &options.out_dir);
    before_node(&analysis);
    let Some(entry) = entry else {
        return (analysis, None);
    };
    match run_node(&options.node, &entry, &options.args, options.output) {
        Ok(output) => (analysis, Some(output)),
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            (analysis, None)
        }
    }
}

/// `./src/main.emel` と `src/main.emel` を同じとみなす．
fn same_path(a: &Path, b: &Path) -> bool {
    fn normal(p: &Path) -> impl Iterator<Item = Component<'_>> {
        p.components().filter(|c| !matches!(c, Component::CurDir))
    }
    normal(a).eq(normal(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemoryFiles;

    fn input(files: &[&str], path: &str) -> Result<Input, Diagnostic> {
        let fs = MemoryFiles::new(files.iter().map(|f| (*f, "")));
        Input::resolve(&fs, Path::new(path))
    }

    fn project(root: &str, entry: Option<&str>, base: &str) -> Result<Input, Diagnostic> {
        Ok(Input {
            root: root.into(),
            entry: entry.map(Into::into),
            base: base.into(),
            single_file: false,
        })
    }

    fn message(result: Result<Input, Diagnostic>) -> String {
        result.unwrap_err().message
    }

    #[test]
    fn project_from_directory() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
        ];
        let expected = project("/app/src", Some("/app/src/main.emel"), "/app");
        assert_eq!(input(&files, "/app"), expected);
        // 相対パスと `./` 付きでも同じ（メモリ上のファイルは `/` を起点にする）．
        assert_eq!(input(&files, "./app/"), expected);
        // プロジェクトの中のディレクトリを渡しても，上にたどって同じプロジェクトになる．
        assert_eq!(input(&files, "/app/src/http"), expected);
        // エントリがなくてもプロジェクトにはなる（check はエントリを求めない）．
        assert_eq!(
            input(&["/lib/Pome.toml", "/lib/src/util.emel"], "/lib"),
            project("/lib/src", None, "/lib")
        );
    }

    #[test]
    fn directory_without_pome_toml() {
        assert_eq!(
            message(input(&["/app/src/main.emel"], "/app")),
            "`/app` にも祖先にも Pome.toml がない"
        );
    }

    #[test]
    fn project_from_file() {
        let files = [
            "/app/Pome.toml",
            "/app/src/main.emel",
            "/app/src/http/client.emel",
            "/app/scripts/x.emel",
        ];
        assert_eq!(
            input(&files, "/app/src/http/client.emel"),
            project("/app/src", Some("/app/src/http/client.emel"), "/app")
        );
        assert_eq!(
            message(input(&files, "/app/scripts/x.emel")),
            "`/app/scripts/x.emel` がソースのルート `/app/src` の外にある"
        );
    }

    #[test]
    fn nearest_pome_toml_wins() {
        // 入れ子のプロジェクトは内側が勝つ．
        let files = [
            "/outer/Pome.toml",
            "/outer/src/inner/Pome.toml",
            "/outer/src/inner/src/main.emel",
        ];
        assert_eq!(
            input(&files, "/outer/src/inner/src/main.emel"),
            project(
                "/outer/src/inner/src",
                Some("/outer/src/inner/src/main.emel"),
                "/outer/src/inner"
            )
        );
    }

    #[test]
    fn single_file() {
        let files = ["/home/src/proj/main.emel", "/home/src/proj/util.emel"];
        // 祖先に `src` があっても，Pome.toml がなければ単独ファイル．
        assert_eq!(
            input(&files, "/home/src/proj/main.emel"),
            Ok(Input {
                root: "/home/src/proj".into(),
                entry: Some("/home/src/proj/main.emel".into()),
                base: "/home/src/proj".into(),
                single_file: true,
            })
        );
        assert_eq!(
            message(input(&files, "/home/missing.emel")),
            "`/home/missing.emel` が見つからない"
        );
    }

    #[test]
    fn same_path_ignores_cur_dir() {
        assert!(same_path(
            Path::new("./src/main.emel"),
            Path::new("src/main.emel")
        ));
        assert!(!same_path(Path::new("src/a.emel"), Path::new("src/b.emel")));
    }
}
