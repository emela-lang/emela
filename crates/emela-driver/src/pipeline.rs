//! パイプライン．モジュールを集め，字句解析し，import のグラフを検査する．
//!
//! パーサ，型検査，JS の出力はまだないので，[`Frontend`] と [`JsBackend`] で差し込む．
//! 今の段で使えるのは字句解析だけの [`LexOnly`] と，出力を持たない [`NoJsBackend`]．

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{Import, ImportGraph, ModuleData, ModuleId, ModuleMap};
use emela_syntax::Lexed;
use la_arena::ArenaMap;

use crate::diagnostic::{Diagnostic, Location, error_count};
use crate::output::{JsOutput, write_output};
use crate::run::{Output, RunOutput, run_node};
use crate::source::{FileId, FileSystem, SourceDb, SourceFile};

/// ソースのルートのディレクトリ名．この名前の祖先があれば，そこをルートにする．
pub const SOURCE_DIR: &str = "src";
/// ディレクトリを渡したときのエントリ（ルートからの相対パス）．
pub const ENTRY_FILE: &str = "main.emel";
/// プロジェクトのディレクトリからの，JS の既定の出力先．
pub const JS_OUT_DIR: &str = "target/emela/js";

/// コマンドラインの path から決めた，ソースのルートとエントリ．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// モジュール名はここからの相対パスで決まる．
    pub root: PathBuf,
    /// エントリのファイル．ルートを付けたパス．ディレクトリを渡して `src/main.emel` がなければ `None`．
    pub entry: Option<PathBuf>,
    /// `target/` を置くディレクトリ．
    pub project: PathBuf,
}

impl Input {
    /// path がファイルならそのファイルがエントリ，ディレクトリなら `<dir>/src/main.emel`．
    ///
    /// ファイルのときのルートは，祖先にある最も近い `src`，なければファイルのあるディレクトリ．
    pub fn resolve(fs: &dyn FileSystem, path: &Path) -> Result<Input, Diagnostic> {
        // `./app` と `app` で診断のパスが変わらないよう，`.` の区切りを落とす．
        let cleaned: PathBuf = path
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .collect();
        let path = if cleaned.as_os_str().is_empty() {
            Path::new(".")
        } else {
            cleaned.as_path()
        };
        if fs.is_file(path) {
            let dir = match path.parent() {
                Some(dir) if dir != Path::new("") => dir.to_owned(),
                _ => PathBuf::from("."),
            };
            let root = dir
                .ancestors()
                .find(|a| a.file_name().is_some_and(|name| name == SOURCE_DIR))
                .map_or_else(|| dir.clone(), Path::to_owned);
            let relative = dir.strip_prefix(&root).expect("ルートは dir の祖先");
            let file_name = path.file_name().expect("ファイルには名前がある");
            let entry = root.join(relative).join(file_name);
            let project = if root.file_name().is_some_and(|name| name == SOURCE_DIR) {
                match root.parent() {
                    Some(parent) if parent != Path::new("") => parent.to_owned(),
                    _ => PathBuf::from("."),
                }
            } else {
                root.clone()
            };
            Ok(Input {
                root,
                entry: Some(entry),
                project,
            })
        } else if fs.is_dir(path) {
            let root = if path == Path::new(".") {
                PathBuf::from(SOURCE_DIR)
            } else {
                path.join(SOURCE_DIR)
            };
            let entry = root.join(ENTRY_FILE);
            Ok(Input {
                entry: fs.is_file(&entry).then_some(entry),
                root,
                project: path.to_owned(),
            })
        } else {
            Err(Diagnostic::error(format!(
                "`{}` が見つからない",
                path.display()
            )))
        }
    }

    pub fn default_out_dir(&self) -> PathBuf {
        self.project.join(JS_OUT_DIR)
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
    let (modules, resolve_diagnostics) = emela_resolve::collect_modules(fs, &input.root);
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
                    entry.display()
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
            input.root.join(ENTRY_FILE).display()
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
/// 診断でエラーになったか node を起動できなかったときは，実行結果が `None`．
pub fn run<F: Frontend, B: JsBackend<F::Program>>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
    backend: &mut B,
    options: &RunOptions,
) -> (Analysis, Option<RunOutput>) {
    let (mut analysis, entry) = build(fs, input, frontend, backend, &options.out_dir);
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

    fn ok(root: &str, entry: Option<&str>, project: &str) -> Result<Input, Diagnostic> {
        Ok(Input {
            root: root.into(),
            entry: entry.map(Into::into),
            project: project.into(),
        })
    }

    #[test]
    fn entry_from_directory() {
        let files = ["app/src/main.emel", "app/src/http/client.emel"];
        assert_eq!(
            input(&files, "app"),
            ok("app/src", Some("app/src/main.emel"), "app")
        );
        assert_eq!(
            input(&["./app/src/main.emel"], "./app/"),
            ok("app/src", Some("app/src/main.emel"), "app")
        );
        assert_eq!(
            input(&["lib/src/util.emel"], "lib"),
            ok("lib/src", None, "lib")
        );
    }

    #[test]
    fn entry_from_file() {
        let files = ["app/src/main.emel", "app/src/http/client.emel", "main.emel"];
        assert_eq!(
            input(&files, "app/src/main.emel"),
            ok("app/src", Some("app/src/main.emel"), "app")
        );
        // `src` の下の深いファイルでも，ルートは `src`．
        assert_eq!(
            input(&files, "app/src/http/client.emel"),
            ok("app/src", Some("app/src/http/client.emel"), "app")
        );
        // `src` の祖先がなければ，ファイルのあるディレクトリ．
        assert_eq!(
            input(&files, "main.emel"),
            ok(".", Some("./main.emel"), ".")
        );
        assert!(input(&files, "missing.emel").is_err());
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
