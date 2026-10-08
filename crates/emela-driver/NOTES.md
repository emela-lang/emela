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
- 今の段の実装は `LexOnly`（import なし，型検査なし，`Program = ()`）と `NoJsBackend`（「JS の出力はまだ実装されていない」の診断を返す）．emela-codegen-js ができたら `JsBackend` を実装した型を cli で差し替える

## 補った判断

- （補）エントリ: path がファイルならそのファイル，ディレクトリなら `<dir>/src/main.emel`．path を省くと `.`
- （補）ソースのルート: ディレクトリを渡したら `<dir>/src`．ファイルを渡したら，祖先で最も近い `src` という名前のディレクトリ，なければファイルのあるディレクトリ
- （補）プロジェクトのディレクトリ（`target/` を置く場所）: ルートが `src` ならその親，そうでなければルート
- （補）設定ファイル（`emela.toml`）は名前が仕様で未決（18.1 #14）なので読まない
- （補）ディレクトリを渡して `src/main.emel` がなくても `check` は通す．エントリがないことは `build` と `run` で初めてエラーにする
- （補）エントリのファイルが名前の誤りでモジュールにならないときは，名前解決の診断に加えて「エントリ `…` がモジュールにならない」を出す
- （補）入力の path の `.` の区切りは落とす（`./app` と `app` で診断のパスを同じにする）
- （補）JS の出力先の既定は `<プロジェクト>/target/emela/js`．`emela run` も同じ場所に書いてから実行する．前の出力は消さずに上書きする
- （補）出力のパスが絶対パスや `..` を含むとき，またはエントリが出力に含まれないときは，出力段の誤りとして診断にする
- （補）node は環境変数 `EMELA_NODE` があればそれを使い，なければ PATH の `node`
- （補）node は `--input-type=module -e <起動用モジュール> -- <エントリ> <引数…>` で起動する．起動用モジュールはエントリを `import()` し，投げられた例外が defect ならまとめて終了コードを決める．エントリの `.mjs` は読み込まれたときに `main` を実行する前提
- （補）defect は `name` が `"EmelaDefect"` の `Error`．JS ランタイムの defect のクラスはこの `name` を持つ（クラス名は問わない）．`instanceof` でなく `name` で見るので，ランタイムを import しなくても判定できる
- （補）defect で止まったときは，標準エラーに `defect: <message>` を1行出し，終了コード 101（Rust の panic と同じ）．defect でない例外は node に任せる（スタックを出して 1）
- （補）終了コードは `process.exit` でなく `process.exitCode` で決める（パイプへの書き込みが途中で切れないように）
- （補）node がシグナルで止まったときの終了コードは 128 + シグナル番号
- （補）診断の順序は段の順: モジュールの収集（名前の誤りなど），ファイルごとの字句解析と構文解析（モジュールのパス順），import のグラフ，型検査
- （補）表示の種別名は「エラー」「警告」．最後の行に「エラー N 件」（警告もあれば「エラー N 件，警告 M 件」）．診断の間は空行で区切る
- （補）ariadne は注記をソースの枠の中にしか出さず，見出しも英語なので，注記は枠の後に `   = 注: …` として自前で出す
- （補）位置がファイル全体かパスだけの診断は，見出しの次に `   ─[ パス ]` を出す（ソースの枠は出さない）
- （補）主な位置のラベルは文言なしだが，色なしで見えるように空の文言を付けて下線と矢印を出す
- （補）ariadne 0.6 は `ReportKind::Custom` の色を `with_color(false)` でも付けるので，色なしのときは出力から ANSI の色指定を取り除く
- （補）ariadne は位置が戻るとソースの枠を分けるので，ラベルはファイルと位置の順に並べて渡す
- （補）`TypeError` は位置を持たないので，変換関数（`Diagnostic::from_type_error`）は範囲と型名の表（`TyCons`）を受け取る．最上位の型と食い違った部分が違うときは，食い違った部分を注記にする
- （補）`SourceDb` に同じパスを足すと，テキストを差し替えて同じ ID を返す（LSP の未保存バッファ向け）
