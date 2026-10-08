# emela-driver の補足

仕様にない判断を「（補）」として1行ずつ残す．

## 差し込み口

パーサ，型検査，JS の出力はまだないので，trait で差し込む．

```rust
pub trait Frontend {
    type Program;
    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile, lexed: &Lexed) -> Parsed;
    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program>;
}

pub trait JsBackend<P> {
    fn emit(&mut self, program: &P, analysis: &Analysis) -> Result<JsOutput, Vec<Diagnostic>>;
}
```

- `parse` はモジュールごとに1回，字句解析の後に呼ぶ．返すのは import の一覧（`emela_resolve::Import`）と構文の診断．構文木は `Frontend` の側に持っておく
- `check` は全モジュールの `parse` と import のグラフの検査の後に1回だけ呼ぶ．`order` は依存順（import 先が先）．`Analysis` からソースの表，モジュールの対応表，import のグラフ，エントリを引ける
- `emit` はエラーが1件もないときだけ呼ぶ．返す `JsOutput` は「出力先からの相対パスと中身の組の列」と，その中のエントリのパス
- 今の段の実装は `LexOnly`（import なし，型検査なし，`Program = ()`）と `NoJsBackend`（「JS の出力はまだ実装されていない」の診断を返す）
- `CoreJs` は `JsBackend<emela_core::Module>`．自己末尾呼び出しのループ化をかけて emela-codegen-js で出す．lowering ができて `Frontend::Program` が `emela_core::Module` になったら，cli の `LexOnly` と `NoJsBackend` をそれぞれ差し替える

## 補った判断

- プロジェクトとソースのルート，エントリ，出力先，診断のパスの決め方は仕様 4.1 にある（`Pome.toml` の有無で決める）．以下はそれ以外の判断
- （補）`Pome.toml` は `fs.absolute` で絶対パスに直したパスから上にたどって探す．`OsFs` の絶対パスは `canonicalize`（シンボリックリンクも解く）
- （補）単独ファイルのときは，`emela_resolve::collect_modules` に渡す一覧からディレクトリを除く（`Shallow`）．resolve は変えずに「直下の `.emel` だけ」にする
- （補）単独ファイルで，エントリから import でたどれないファイルの診断（兄弟の名前の誤りや字句のエラー）も出る．たどれるファイルだけを読む形はパーサが import を返すようになってから入れる（仕様側から提案あり）
- （補）`MemoryFiles` は `/` を起点にし，相対パスは `/` からとみなす．`.` と `..` は字面で畳む
- （補）診断のパスは `SourceDb::display_path` で起点（プロジェクトか単独ファイルのディレクトリ）からの相対パスにする．名前解決の診断の文面に埋め込まれたパス（重複の「先に `…` がある」）も同じく相対にする
- （補）ディレクトリを渡してエントリ（`src/main.emel`）がなくても `check` は通す．エントリがないことは `build` と `run` で初めてエラーにする
- （補）エントリのファイルが名前の誤りでモジュールにならないときは，名前解決の診断に加えて「エントリ `…` がモジュールにならない」を出す
- （補）`emela run` も既定の出力先（仕様 4.1）に書いてから実行する．前の出力は消さずに上書きする
- （補）`run` は，ビルドの後で node を起動する前に `before_node` を呼ぶ．CLI はここで診断を出す（終わらないプログラムでも警告が先に見える）．node を起動できなかった診断はその後に足す
- （補）出力のパスが絶対パスや `..` を含むとき，またはエントリが出力に含まれないときは，出力段の誤りとして診断にする
- （補）node は環境変数 `EMELA_NODE` があればそれを使い，なければ PATH の `node`
- （補）node は `--input-type=module -e <起動用モジュール> -- <エントリ> <引数…>` で起動する．起動用モジュールはエントリを `import()` し，`main` を export していれば引数なしで呼ぶ（返り値は捨てる）．export していなければ読み込むだけにする
- （補）`CoreJs` の出力はエントリの `main.mjs` と，隣のランタイム `emela_runtime.mjs` の2つ
- （補）defect は `name` が `"Defect"` の `Error`（emela-codegen-js のランタイムの `$Defect`）．`instanceof` でなく `name` で見るので，起動用モジュールはランタイムを import しない
- （補）`main` がジェネレータ（suspend する関数，16.5）を返す場合はまだ扱わない
- （補）defect で止まったときは，標準エラーに `defect: <message>` を1行出し，終了コード 101（Rust の panic と同じ）．defect でない例外は node に任せる（スタックを出して 1）（仕様 7.5 に反映済み）
- （補）終了コードは `process.exit` でなく `process.exitCode` で決める（パイプへの書き込みが途中で切れないように）
- （補）node がシグナルで止まったときの終了コードは 128 + シグナル番号
- （補）診断の順序は段の順: モジュールの収集（名前の誤りなど），ファイルごとの字句解析と構文解析（モジュールのパス順），import のグラフ，型検査
- 診断の文面は英語，コードの体系と表は仕様の付録 A（`code.rs` はその写し）．見出しは `error[E0204]: …`，最後の行は `2 errors, 1 warning`（1件なら単数形）
- （補）コードの付いていない診断は見出しを `error:` / `warning:` だけにする．今は処理系の内部の誤り（`internal error: …`）だけがそう
- 字句解析と構文解析の診断のコードと文面は emela-syntax が持つ（`Diagnostic::code`）．driver はそのまま渡す
- （補）emela-resolve と emela-types の診断は，種類（`DiagnosticKind`，`TypeErrorKind`）から driver が英語の文面とコードを作る．resolve の `Display`（日本語）は使わない
- （補）診断の間は空行で区切る
- （補）ariadne は注記をソースの枠の中にしか出さないので，注記は枠の後に `   = note: …` として自前で出す
- （補）位置がファイル全体かパスだけの診断は，見出しの次に `   ─[ パス ]` を出す（ソースの枠は出さない）
- （補）主な位置のラベルは文言なしだが，色なしで見えるように空の文言を付けて下線と矢印を出す
- （補）ariadne 0.6 は `ReportKind::Custom` の色を `with_color(false)` でも付けるので，色なしのときは出力から ANSI の色指定を取り除く
- （補）ariadne は位置が戻るとソースの枠を分けるので，ラベルはファイルと位置の順に並べて渡す
- （補）`TypeError` は位置を持たないので，変換関数（`Diagnostic::from_type_error`）は範囲と型名の表（`TyCons`）を受け取る．最上位の型と食い違った部分が違うときは，食い違った部分を注記にする
- （補）`SourceDb` に同じパスを足すと，テキストを差し替えて同じ ID を返す（LSP の未保存バッファ向け）
