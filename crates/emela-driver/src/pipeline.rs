//! パイプライン．モジュールを集め，字句解析し，import のグラフを検査する．
//!
//! 構文解析，型検査，JS の出力は [`Frontend`] と [`JsBackend`] で差し込む．
//! 今の段で使えるのは構文解析までの [`crate::ParseOnly`]，字句解析だけの [`LexOnly`]，
//! 出力を持たない [`NoJsBackend`]．

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use emela_resolve::{
    DiagnosticKind, DirEntry, Import, ImportGraph, ModuleData, ModuleId, ModuleMap, SourceFs,
};
use la_arena::ArenaMap;

use crate::code;
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
/// 同梱の core のソースを載せる仮のディレクトリ．診断のパスに `<core>/list.emel` のように出る．
pub const CORE_DIR: &str = "<core>";

/// コンパイラに同梱した core のソース（モジュール名と中身）．[`Frontend::core_sources`] で渡す．
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreSource {
    /// モジュール名（`List`，`Prelude`）．`emela_core::core_sources()` の名前をそのまま使う．
    pub module: String,
    pub text: String,
}

impl CoreSource {
    /// `emela_core::core_sources()` の全部（Prelude と core のモジュール）．
    pub fn bundled() -> Vec<CoreSource> {
        emela_core::core_sources()
            .iter()
            .map(|(module, text)| CoreSource {
                module: (*module).to_owned(),
                text: (*text).to_owned(),
            })
            .collect()
    }

    /// ソースの表に載せる仮のパス．`List` なら `<core>/list.emel`，`Int64` なら `<core>/int64.emel`．
    fn path(&self) -> PathBuf {
        let mut file = String::new();
        for (i, c) in self.module.chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                file.push('_');
            }
            file.push(c.to_ascii_lowercase());
        }
        Path::new(CORE_DIR).join(format!("{file}.emel"))
    }
}

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
        let not_found = || {
            Diagnostic::error(format!("`{}` not found", path.display()))
                .with_code(code::PATH_NOT_FOUND)
        };
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
                            "`{}` is outside the source root `{}`",
                            path.display(),
                            root.display()
                        ))
                        .with_code(code::OUTSIDE_SOURCE_ROOT)
                        .with_note(format!(
                            "source files of a project (the directory with {PROJECT_FILE}) go under {SOURCE_DIR}/"
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
                "no {PROJECT_FILE} found in `{}` or its parent directories",
                path.display()
            ))
            .with_code(code::NO_PROJECT_FILE)
            .with_note(format!(
                "put a {PROJECT_FILE} (it may be empty) in the project directory, or pass a single file"
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

    /// 1つのモジュールを字句解析して構文解析する．字句と構文の診断はどちらもここで返す
    /// （パイプラインは字句解析をしない）．構文木は `self` に持っておき，[`Frontend::check`] で使う．
    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed;

    /// 読んだ全モジュールの構文解析と import の検査の後に1度だけ呼ぶ．
    /// `order` は読んだモジュールだけの依存順．import が循環していると依存順がないので呼ばない．
    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program>;

    /// コンパイラに同梱した core のソース．パイプラインはモジュールの対応表に足し，
    /// ソースのモジュールと同じく（単独ファイルでも必ず）読んで [`Frontend::parse`] に渡す．
    fn core_sources(&self) -> Vec<CoreSource> {
        Vec::new()
    }
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

/// 字句解析だけのフロントエンド．字句の診断だけを返し，import を返さず，型検査もしない．
#[derive(Debug, Default, Clone, Copy)]
pub struct LexOnly;

impl Frontend for LexOnly {
    type Program = ();

    fn parse(&mut self, _: ModuleId, file: FileId, source: &SourceFile) -> Parsed {
        let lexed = emela_syntax::lex(source.text());
        Parsed {
            imports: Vec::new(),
            diagnostics: lexed
                .diagnostics
                .iter()
                .map(|d| Diagnostic::from_syntax(file, d))
                .collect(),
        }
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
        Err(vec![
            Diagnostic::error("JS output is not implemented yet").with_code(code::NO_JS_BACKEND),
        ])
    }
}

/// 解析の途中経過．診断は段の順に全部たまる．
#[derive(Debug, Default)]
pub struct Analysis {
    pub sources: SourceDb,
    pub modules: ModuleMap,
    /// 読んだモジュールのファイル．読めなかったファイルと，単独ファイルでエントリから
    /// たどれないモジュールは入らない．
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
///
/// モジュールの対応表は全部作る．読んで解析するのは，プロジェクトなら全モジュール，
/// 単独ファイルならエントリから import でたどれるモジュールだけ．
pub fn check<F: Frontend>(
    fs: &dyn FileSystem,
    input: &Input,
    frontend: &mut F,
) -> (Analysis, Option<F::Program>) {
    let mut analysis = Analysis::default();
    analysis.sources.set_base(&input.base);
    let (modules, mut resolve_diagnostics) = if input.single_file {
        emela_resolve::collect_modules(&Shallow(fs), &input.root)
    } else {
        emela_resolve::collect_modules(fs, &input.root)
    };
    analysis.modules = modules;
    // 同梱の core のソース．同じ名前のソースのモジュールは隠れ，予約された名前の診断が出る．
    let mut core_texts: ArenaMap<ModuleId, String> = ArenaMap::default();
    for source in frontend.core_sources() {
        let name = emela_resolve::ModuleName::parse(&source.module);
        let (id, shadowed) = analysis.modules.add_core(name, source.path());
        resolve_diagnostics.extend(shadowed);
        core_texts.insert(id, source.text);
    }
    if let Some(entry) = &input.entry {
        analysis.entry = analysis
            .modules
            .iter()
            .find(|(_, data)| same_path(&data.file, entry))
            .map(|(id, _)| id);
    }

    // 作業リスト．単独ファイルではエントリから始め，import 先を見つけるたびに足す．
    let mut work: Vec<ModuleId> = if input.single_file {
        let core = analysis
            .modules
            .iter()
            .map(|(id, _)| id)
            .filter(|&id| analysis.modules.is_core(id));
        analysis.entry.into_iter().chain(core).collect()
    } else {
        analysis.modules.iter().map(|(id, _)| id).collect()
    };
    let mut queued: ArenaMap<ModuleId, ()> = ArenaMap::default();
    for &id in &work {
        queued.insert(id, ());
    }
    let mut imports: ArenaMap<ModuleId, Vec<Import>> = ArenaMap::default();
    // 診断は読んだ順でなくモジュールの順に並べる．
    let mut file_diagnostics: ArenaMap<ModuleId, Vec<Diagnostic>> = ArenaMap::default();
    while let Some(module) = work.pop() {
        let path = &analysis.modules[module].file;
        let read = match core_texts.remove(module) {
            Some(text) => Ok(text),
            None => fs.read_to_string(path),
        };
        let text = match read {
            Ok(text) => text,
            Err(err) => {
                file_diagnostics.insert(
                    module,
                    vec![
                        Diagnostic::error(format!("cannot read file: {err}"))
                            .with_code(code::CANNOT_READ)
                            .at(Location::Path(path.clone())),
                    ],
                );
                continue;
            }
        };
        let file = analysis.sources.add(path, text);
        analysis.files.insert(module, file);
        let parsed = frontend.parse(module, file, &analysis.sources[file]);
        file_diagnostics.insert(module, parsed.diagnostics);
        if input.single_file {
            for import in &parsed.imports {
                if let Some(target) = analysis.modules.lookup(&import.path)
                    && !queued.contains_idx(target)
                {
                    queued.insert(target, ());
                    work.push(target);
                }
            }
        }
        imports.insert(module, parsed.imports);
    }

    // 名前解決の診断は，ファイルを表に入れてから写す（ID で指せるように）．
    // 単独ファイルでは，たどれたモジュールに関わるものだけを出す．
    let resolve_diagnostics: Vec<Diagnostic> = resolve_diagnostics
        .iter()
        .filter(|d| !input.single_file || concerns_reached(d, input, &analysis, &queued))
        .map(|d| Diagnostic::from_resolve(d, &analysis.sources))
        .collect();
    analysis.diagnostics.extend(resolve_diagnostics);
    for (module, _) in analysis.modules.iter() {
        if let Some(diagnostics) = file_diagnostics.remove(module) {
            analysis.diagnostics.extend(diagnostics);
        }
    }

    if let (Some(entry), None) = (&input.entry, analysis.entry) {
        // 名前の誤りなどでモジュールにならなかった．理由の診断は名前解決が出している．
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "the entry `{}` is not a module",
                analysis.sources.display_path(entry).display()
            ))
            .with_code(code::ENTRY_NOT_MODULE)
            .at(Location::Path(entry.clone())),
        );
    }

    let (graph, graph_diagnostics) = emela_resolve::build_import_graph(&analysis.modules, &imports);
    analysis.graph = graph;
    analysis.diagnostics.extend(
        graph_diagnostics
            .iter()
            .map(|d| Diagnostic::from_resolve(d, &analysis.sources)),
    );

    let Some(order) = analysis.graph.order() else {
        return (analysis, None);
    };
    let order: Vec<ModuleId> = order
        .iter()
        .copied()
        .filter(|&id| analysis.files.contains_idx(id))
        .collect();
    let checked = frontend.check(&analysis, &order);
    analysis.diagnostics.extend(checked.diagnostics);
    (analysis, checked.program)
}

/// 単独ファイルで，名前解決の診断がエントリかたどれたモジュールに関わるか．
/// 名前の誤りはそのファイルがエントリのときだけ，重複はそのモジュール名をたどったときだけ出す．
fn concerns_reached(
    diagnostic: &emela_resolve::Diagnostic,
    input: &Input,
    analysis: &Analysis,
    reached: &ArenaMap<ModuleId, ()>,
) -> bool {
    if let DiagnosticKind::DuplicateModule { name, .. } = &diagnostic.kind
        && let Some(id) = analysis.modules.lookup(name)
        && reached.contains_idx(id)
    {
        return true;
    }
    input
        .entry
        .as_deref()
        .is_some_and(|entry| same_path(&diagnostic.file, entry))
        || analysis.sources.file_id(&diagnostic.file).is_some()
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
        analysis.diagnostics.push(
            Diagnostic::error(format!(
                "the entry `{}` does not exist",
                analysis
                    .sources
                    .display_path(&input.root.join(ENTRY_FILE))
                    .display()
            ))
            .with_code(code::ENTRY_NOT_FOUND),
        );
    }
    if analysis.has_errors() {
        return (analysis, None);
    }
    let Some(program) = program else {
        // エラーなしでプログラムがないのはフロントエンドの誤り．
        analysis.diagnostics.push(Diagnostic::error(
            "internal error: the type checker returned no program",
        ));
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
            "no Pome.toml found in `/app` or its parent directories"
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
            "`/app/scripts/x.emel` is outside the source root `/app/src`"
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
            "`/home/missing.emel` not found"
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
