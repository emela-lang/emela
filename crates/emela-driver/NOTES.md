# emela-driver の補足

仕様にない判断を「（補）」として1行ずつ残す．

## 差し込み口

構文解析，型検査，JS の出力は trait で差し込む．

```rust
pub trait Frontend {
    type Program;
    fn parse(&mut self, module: ModuleId, file: FileId, source: &SourceFile) -> Parsed;
    fn check(&mut self, analysis: &Analysis, order: &[ModuleId]) -> Checked<Self::Program>;
}

pub trait JsBackend<P> {
    fn emit(&mut self, program: &P, analysis: &Analysis) -> Result<JsOutput, Vec<Diagnostic>>;
}
```

- 字句解析は `Frontend` に任せる．パイプラインは字句解析をしない．`emela_syntax::parse` が字句解析をやり直して字句の診断も返すので，パイプラインでも字句解析をすると同じ診断が2回出るため
- `parse` は読んだモジュールごとに1回呼ぶ．返すのは import の一覧（`emela_resolve::Import`）と，字句と構文の診断．構文木は `Frontend` の側に持っておく
- `check` は読んだ全モジュールの `parse` と import のグラフの検査の後に1回だけ呼ぶ．`order` は読んだモジュールだけの依存順（import 先が先）．`Analysis` からソースの表，モジュールの対応表，import のグラフ，エントリを引ける
- `emit` はエラーが1件もないときだけ呼ぶ．返す `JsOutput` は「出力先からの相対パスと中身の組の列」と，その中のエントリのパス
- `ParseOnly` は emela-syntax のパーサを呼び，構文木（`Parse`）をモジュールごとに持つ（`parse_of`）．型検査はしないので `Program = ()`
- `Resolve` は `ParseOnly` で構文解析し，`check` で名前解決をする（`Program = emela_resolve::hir::Program`）
- `Typecheck` は `Resolve` の後に型検査（`emela_types::check_program`）をする（`Program = TypedProgram`．HIR と型検査の結果の組）．cli の `check` と LSP はこれを使う
- `Lower` は `Typecheck` の後に lowering（`emela_core::lower`）をする（`Program = emela_core::Module`）．名前解決か型検査に誤りが1件でもあれば lowering は呼ばない．根はエントリのモジュールの export する fn．ターゲットを渡すので，返る IR は単相化済み（2026-10-15．汎用の `pub` の fn は型引数を `Any` にした写し）．debug ビルドでは `verify_monomorphic` にかける（多相な IR の `verify` は lowering の中で行う）．`LowerTests` も同じで，`TestCase::function` は単相化した写しを指す．lowering の誤り（まだ扱えない式）は `Diagnostic::from_lower` で診断にする．cli の `build` と `run` は `Lower` と `CoreJs` を使う
- `LexOnly` は字句解析だけ（字句の診断だけを返し，import なし，型検査なし）．`NoJsBackend` は「JS の出力はまだ実装されていない」の診断を返す
- `CoreJs` は `JsBackend<emela_core::Module>`．lowering の済んだ（自己末尾呼び出しのループ化と単相化も済んだ）IR をそのまま emela-codegen-js で出す．モジュールは複製しない．cli の `build` と `run` は `Lower` と組み合わせて使う

## テスト（`emela test`，仕様 13章）

`src/testing.rs` と `src/testing/` に分けてある．流れは「検査 → テスト関数の IR → JS の出力 → node で1つずつ実行 → 結果の表示」．

```rust
/// 型検査（`Typecheck::check`）が `@test` を集め，HIR の定義と組にして `TypedProgram::tests` に持つ．
pub struct TestDef { pub test: TestFn, pub def: DefId }

/// テスト関数を根にして Core IR へ写すフロントエンド（Program = TestModule）．
pub struct LowerTests<B = NoBuiltins> { .. }
pub struct TestModule { pub module: emela_core::Module, pub tests: Vec<TestCase> }
pub struct TestCase { pub name: String, pub function: FnId }

pub trait TestBackend<P> {
    fn test_names(&self, program: &P) -> Vec<String>;
    fn emit_tests(&mut self, program: &P, selected: &[usize], analysis: &Analysis)
        -> Result<JsOutput, Vec<Diagnostic>>;
}
```

- `Typecheck::check` が `collect_tests`（E0226）と `resolve_tests` を呼ぶ．`check`，LSP，`build`，`run`，`test` のどれでも `@test` の誤りが出る
- `resolve_tests` は `TestFn` ごとに HIR の定義を引く（`TestFn::module` と `name_range` が `DefData::span` と一致する）．テスト関数でない fn と const の本体からテスト関数を参照していれば E0227．テスト関数どうしの参照は許す
- `LowerTests` は `Roots::Tests` で写し，`Lowered::tests`（`LowerInput::tests` と同じ順の `FnId`）から `TestCase` を作る．`TestCase::name` は `TestFn::display_name`
- 通常のビルド（`Lower`）は `LowerInput::tests` を渡し，テスト関数を export しない．通常のコードからは参照できない（E0227）ので，たどれずに取り除かれる（13.1）．`assert` は除かない（13.2）
- cli の `test` は `LowerTests` と `CoreJs` を使う
- （補）JS で export する名前が識別子として書けないとき（エントリ以外のモジュールのテスト関数 `Util.in_util`）は，文字列の名前で export する（`export { Util_in_util as "Util.in_util" }`，ES2022）．起動用モジュールは `$m["Util.in_util"]` で引く

### 補った判断

- （補）`@test` の誤りは E0226 にする: 引数のある fn，本体のない fn（`@external`），トップレベルの fn 以外（type，impl の中の fn など）に付けたもの，`@test(...)` のように引数を付けたもの．誤ったものは集めない
- （補）テスト関数を通常のコード（テスト関数でない fn と const の本体）から参照すると E0227（``test function `adds` can only be used from test functions``，注記 ``= note: test functions are removed from normal builds (13.1)``）．13.1 と付録 A に反映済み
- （補）テストの表示名は，エントリのモジュールなら関数名だけ（`greet_returns_user`），ほかのモジュールならモジュール名を付ける（`Http.Client.gets`）．`cargo test` がクレートのルートだけ前置きを付けないのに合わせた．フィルタはこの表示名の部分一致（大文字小文字を区別する）
- （補）単独ファイルでは，エントリから import でたどれるモジュールのテストだけを実行する（読むモジュールの規則と同じ）
- （補）テストの JS の出力先は `target/emela/test`（`--out-dir` で変えられる）．テスト関数を export した別の出力なので，通常のビルドの `target/emela/js` を上書きしない
- （補）`CoreJs` のテストの出力は `main.mjs`（選んだテスト関数を Emela の名前で export する．IR の `exported` は書き換えず，`emit_module_exporting` に渡す），ランタイム，起動用モジュール `emela_test.mjs` の3つ．node は起動用モジュールを直接実行する（`run` の起動用モジュールは使わない）
- （補）テストランナーは結果を標準出力に1行ずつ `\x1eemela-test ` + JSON で書き，driver が読みながら表示する．結果の行でない標準出力はそのまま流す．node の標準エラーは `Output::Inherit` なら素通し，`Capture` なら `TestReport::stderr` に集める．標準入力は渡さない
- （補）結果の表示は `cargo test` に近い形: `running 3 tests`，`test name ... ok` / `FAILED`，失敗の詳細（`---- name ----` の後に `defect: <message>` か `error: <エラーの表示>`），失敗の一覧，`test result: ok. 3 passed; 0 failed; 0 filtered out`．かかった時間は出さない（出力を比べるテストのため）
- （補）中断しうるテスト関数（返り値がジェネレータ）は，`main.mjs` が公開する `$start` で進めて終わるまで待つ．resume されないままイベントループが空になったら（`beforeExit`），そのテストの結果を `interrupted`（`TestOutcome::Interrupted`）にして次のテストへ進む（14.5）．失敗の詳細は `interrupted` の1行
- （補）テストの実行中にホストのコールバックの中で投げられた例外（`uncaughtException`）は，実行中のテストの defect にする．終わったテストの継続（後から呼ばれた resume など）は進めない
- （補）終了コードは失敗が1つでもあれば 1，なければ 0（テストが0個でも 0）．診断でエラーなら 1
- （補）テストの途中で node が終わったとき（外部関数の `process.exit` など）は，実行中だったテストを失敗にし，その後ろで実行されなかった数を詳細に添える
- （補）処理されなかったエラー（13.3）は，テストランナーの `$testUnhandledError` が返り値の `$Fail` を `Symbol.for("emela.fail")` の鍵（`FAIL_KEY`）で見分けて `outcome: "error"` で報告する．表示は `error: <エラーの値の表示>`（`dbg` と同じ表示．String は引用符付き）
- 積み残し: suspend するテスト（ジェネレータを返す関数）と Async の供給（13.3）はまだ扱わない

## core の中からだけ参照できる fn（`@internal`，`internal.rs`）

alpha.3 で足した（2026-10-12）．最初は Instant を作る関数（`Instant.from_unix_milliseconds`）を Clock のハンドラだけに使わせるためだったが，Instant は公開することにしたので，今の同梱の core に `@internal` の fn はない．core の中でだけ使わせたい fn のための仕組みとして残す（本人の決定）．仕様にない注釈なので，仕様 3.5 と付録 A への反映を待つ．

- （補）`@internal` は同梱の core のソースのトップレベルの fn にだけ付けられる．ソースのモジュールに書いたもの，トップレベルの fn 以外に付けたものは E0229（``` `@internal` can only be used in the bundled core library ``` / ``` `@internal` can only be used on top-level functions ```）
- （補）`@internal` の fn を core のモジュールの外（fn，const，handler の本体）から参照すると E0228（``function `Secret.key` can only be used from the bundled core library``，注記 ``= note: it is marked `@internal` ``）．呼び出しも値としての参照も数える．import の一覧に書くだけでは出ない（E0227 と同じく本体の参照だけを見る）
- （補）検査は E0227 と同じく型検査のフロントエンド（`Typecheck::check`）で，名前解決の後に HIR の本体の参照（`emela_types::body_refs`）をたどって行う．名前解決の可視性（pub）とは別の検査にして，名前解決には手を入れない．check と LSP の両方で出る

## 整形（`emela fmt`）

`src/fmt.rs`．対象を決め（`fmt_targets`），1ファイルずつ emela-fmt で整形する（`format_source`）．書き込みは呼び出し側が関数で渡し，`None` なら書かない（`--check`）．

- （補）ファイルは `Input::resolve` と同じ規則で扱う: `Pome.toml` があってソースのルートの外なら E0208，なければ単独ファイル．単独ファイルでも整形するのは渡したファイルだけで，import はたどらない
- （補）ディレクトリには `Pome.toml` が要る（E0207）．プロジェクトのディレクトリならソースのルートの下を全部，ソースのルートの中のディレクトリならその下を全部，プロジェクトの中でもソースのルートの外（`scripts/`）なら E0208
- （補）ディレクトリの下は `.` で始まるものを飛ばし，`.emel` を全部集める．モジュール名にならない名前のファイル（`Bad.emel`）も整形する（整形は名前を見ないので）．名前の診断は出さない
- （補）同じファイルを重ねて渡しても1回だけ整形する．順序は絶対パスの順
- （補）診断と一覧のパスは `Input::base`（プロジェクトか単独ファイルのディレクトリ）からの相対パス．起点がファイルごとに違いうるので，診断はファイルごとのソースの表と組にして持つ（`FmtFailure`）
- （補）構文エラーのファイルは整形せず，`Diagnostic::from_syntax` の診断（コード付き）を集めて次へ進む．formatter の検証の失敗（`FormatError::Internal`）はコードなしの `internal error: …` にし，ファイルは書き換えない
- （補）整形の結果が元と同じなら書き込みの関数を呼ばない（更新時刻を変えない）．読めなければ E0902，書けなければ E0903
- （補）言語サーバーの `textDocument/formatting` も `format_source` を使う（emela-lsp の NOTES.md）

## 読むモジュール

- モジュールの対応表はいつも全部作る．読んで解析するモジュールは作業リストで決める
- プロジェクト（`Pome.toml` あり）は全モジュールを読む
- 単独ファイルはエントリから始め，`parse` が返した import の先を作業リストに足していく．エントリから import でたどれないファイルは読まず，その診断も出ない

## 補った判断

- プロジェクトとソースのルート，エントリ，出力先，診断のパスの決め方は仕様 4.1 にある（`Pome.toml` の有無で決める）．以下はそれ以外の判断
- （補）`Pome.toml` は `fs.absolute` で絶対パスに直したパスから上にたどって探す．`OsFs` の絶対パスは `canonicalize`（シンボリックリンクも解く）
- （補）単独ファイルのときは，`emela_resolve::collect_modules` に渡す一覧からディレクトリを除く（`Shallow`）．resolve は変えずに「直下の `.emel` だけ」にする
- （補）単独ファイルでは，エントリから import でたどれるファイルだけを読み，診断もそれだけにする（仕様側からの提案）
- （補）単独ファイルでの名前解決の診断は，名前の誤りと予約名はそのファイルがエントリのときだけ，重複はそのモジュール名をたどったときだけ出す．たどれない兄弟の名前の誤りは出さない
- （補）単独ファイルで，エントリがモジュールにならなければ何も読まない（E0210 と名前の誤りだけが出る）
- （補）`check` に渡す依存順からは読まなかったモジュール（読めなかったファイル，単独ファイルでたどれないもの）を除く
- （補）`Import` の範囲は import 文でなくパスの部分（`import A.B.{x}` の `A.B`）．未定義のモジュールと循環の診断はここを指す
- （補）構文の誤りで読めない import は import の一覧に入れない: パスがない，中に ERROR がある，`.{` が閉じていない，文の直後に読み残しがある（`import A.` の `.`）．宣言の後ろの import（E0127）は位置の誤りなので一覧に入れる
- （補）`MemoryFiles` は `/` を起点にし，相対パスは `/` からとみなす．`.` と `..` は字面で畳む
- （補）診断のパスは `SourceDb::display_path` で起点（プロジェクトか単独ファイルのディレクトリ）からの相対パスにする．名前解決の診断の文面に埋め込まれたパス（重複の「先に `…` がある」）も同じく相対にする
- ディレクトリを渡せるのはプロジェクトのディレクトリかソースのルートの中のディレクトリだけで，プロジェクトの中でもソースのルートの外（`scripts/`）なら E0208（``directory `scripts` is outside the source root `src` ``．付録 B，18.1 #32）．判定は `Input::resolve` に置き，check，build，run，test，fmt，LSP で共有する．ソースのルートの中のディレクトリを渡しても，型の検査はモジュールをまたぐのでプロジェクト全体を検査する
- （補）ディレクトリを渡してエントリ（`src/main.emel`）がなくても `check` は通す．エントリがないことは `build` と `run` で初めてエラーにする
- （補）エントリのファイルが名前の誤りでモジュールにならないときは，名前解決の診断に加えて「エントリ `…` がモジュールにならない」を出す
- （補）`emela run` は，エントリのモジュールに `main` という名前の fn がなければ E0230（``entry `src/main.emel` has no `main` function``，位置はエントリのファイル）で止まり，出力も node の起動もしない．`build` は main がなくても出力する（8.3）．`main` という名前でも fn でないもの（const など）は main と数えない
- （補）E0230 はほかの誤り（構文，名前，型，lowering）が1件もないときだけ出す．構文の誤りで main を読めなかっただけのときに誤りを重ねないため
- （補）main があるかは `Frontend::has_entry_main` で引く．`Typecheck`（と，それを包む `Lower`）が直前の `check` の HIR から答える．答えないフロントエンド（`None`，既定）では確かめずに node を起動する
- （補）`emela run` も既定の出力先（仕様 4.1）に書いてから実行する．前の出力は消さずに上書きする
- （補）`run` は，ビルドの後で node を起動する前に `before_node` を呼ぶ．CLI はここで診断を出す（終わらないプログラムでも警告が先に見える）．node を起動できなかった診断はその後に足す
- （補）出力のパスが絶対パスや `..` を含むとき，またはエントリが出力に含まれないときは，出力段の誤りとして診断にする
- （補）node は環境変数 `EMELA_NODE` があればそれを使い，なければ PATH の `node`
- （補）node は `--input-type=module -e <起動用モジュール> -- <エントリ> <引数…>` で起動する．起動用モジュールはエントリを `import()` し，`main` を export していれば引数なしで呼ぶ（返り値は捨てる）．export していなければ読み込むだけにする（`run` はエントリに main がなければ E0230 で止めるので，ここに来るのは手で書いた出力段などで main を確かめないときだけ）
- （補）`CoreJs` の出力はエントリの `main.mjs` と，隣のランタイム `emela_runtime.mjs` の2つ
- （補）lowering が同梱の core のホストのファイルを IR に入れたとき（`Module::bundled_files`），`CoreJs` はそれを出力先の `emela_core/`（`emela_codegen_js::BUNDLED_DIR`）の下に `lib/` と同じ並びで書き出す（build，run，test のどれも）．エントリの出力は出力先の直下にあるので，`./emela_core/x.mjs` で import できる．前の出力の `emela_core/` は消さない（ほかの出力と同じく上書きだけ）
- （補）defect は `name` が `"Defect"` の `Error`（emela-codegen-js のランタイムの `$Defect`）．`instanceof` でなく `name` で見るので，起動用モジュールはランタイムを import しない
- （補）`main` がジェネレータ（suspend する関数，16.5）を返す場合はまだ扱わない
- （補）defect で止まったときは，標準エラーに `defect: <message>` を1行出し，終了コード 101（Rust の panic と同じ）．defect でない例外は node に任せる（スタックを出して 1）（仕様 7.5 に反映済み）
- （補）main まで達したエラーは，起動用モジュールが main の返り値の `$Fail` を `Symbol.for("emela.fail")` の鍵（`FAIL_KEY`）で見分け，標準エラーに `error: <エラーの値の表示>` を1行出して終了コード 1（`ERROR_EXIT_CODE`，14.3）．表示は `dbg` と同じ（`error: DbError(message: "down")`）．defect は 101 のまま．起動用モジュールはランタイムを import しないので，`instanceof` でなく鍵で見る
- （補）中断しうる main（返り値がジェネレータ）は，エントリが公開するランタイムの `$start` で進め，イベントループで待つ．起動用モジュールはランタイムを import しないので，`$start` はエントリから引く（emela-codegen-js の NOTES の「中断と再開」）
- （補）中断しない main（R が Immediate）の中で，その場で resume されずに捨てられた継続（ランタイムの `$Interrupted`．8.7 の InstantClock の形のハンドラで resume しなかったとき）も，`interrupted` と 130．イベントループが空になるのを待たずにその場で終わる．テストでは `interrupted` の失敗
- （補）main の継続が resume されないまま「プログラムの最上位が終わる」（14.5）のは，node のイベントループが空になったとき（`beforeExit`）とした．標準エラーに `interrupted` を1行出して終了コード 130（`INTERRUPTED_EXIT_CODE`）
- （補）ホストのコールバックの中で投げられた defect（`uncaughtException`）も，main の defect と同じく `defect: <message>` と 101．defect で止まるときは，標準エラーを書き終えてから `process.exit` で終わる（残ったタイマーなどのホストの処理を走らせない）．defect でない例外はスタックを出して 1
- （補）終了コードは `process.exit` でなく `process.exitCode` で決める（パイプへの書き込みが途中で切れないように）
- （補）node がシグナルで止まったときの終了コードは 128 + シグナル番号
- （補）診断の順序は段の順: モジュールの収集（名前の誤りなど），ファイルごとの字句解析と構文解析（読んだ順でなくモジュールのパス順），import のグラフ，型検査
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
- （補）位置の情報（LSP のホバー）は `Frontend::hover` で引く（トレイトの既定は `None`）．`Typecheck` は `TypedProgram` の型の表と E の表（`ProgramTypes::errors`），R の表（`ProgramTypes::effects`）から作る（`hover.rs`）．式のホバーは型の後に，空でなければ `fails E` と `use R` の行を足す．suspend fn（エフェクトの操作と `@external suspend fn`，能力の値の操作の参照）は型の前に `suspend` を付ける．handler の名前は `handler H(conn: Connection) implements E`（フィールドは構成子のシグネチャから．フィールドのない handler は括弧なし）と，init の E（`ProgramTypes::handler_fails`）が空でなければ `init fails E` の行，依存（`ProgramTypes::handler_uses`）が空でなければ `use R` の行（Async が残るかが分かる）．layer の名前は `layer L { H1, H2 }`（初期化の順，`ProgramTypes::layers`）と，layer の中で満たされない依存があれば `use R` の行．
- （補）定義へ移動と参照の検索（LSP）は `Frontend::definition` と `Frontend::references` で引く（トレイトの既定は `None` と空）．`Typecheck` は名前解決の参照の表（`Program::refs`），型推論の操作の表（`ProgramTypes::ops`），定義と局所変数の位置から作る（`nav.rs`）．返すのは `NavTarget`（ファイルと範囲）
- （補）位置の名前は，参照，effect の操作の参照，定義の名前，局所変数を束縛した位置から探す．範囲の終わりの位置（名前の直後のカーソル）も含め，範囲の中にあるものを先に選ぶ
- （補）モジュールの定義の位置はファイルの先頭（空の範囲）．同梱の core の定義と，ソースのない定義（組み込みのモジュールの関数と型，組み込みの Prelude）は移動先がないので `None`
- （補）参照の検索では，名前の位置が同じ定義（type とその構成子，error のタグと構成子）を同じものとみなす．同梱の core の中の参照は含めない．順はファイル（`FileId`）と位置の順．モジュールの参照に定義の位置は含めない
- （補）文書のシンボル（`document_symbols`）は検査せず構文木だけから作る．構文の誤りがあっても読めた宣言は返し，名前のない宣言は飛ばす．子は enum のバリアント，type・error・バリアント・handler のフィールド，effect の操作，trait・impl・handler の fn．impl は名前の字句がないので `impl Show for User` を名前にし，Trait の名前を選ぶ範囲にする
- （補）補完（LSP）は `Frontend::complete` で引く（トレイトの既定は空）．`Typecheck` は `complete.rs`．書きかけのソースは構文が崩れているので，文脈はカーソルの前のテキストで決め，候補は名前解決と型推論の結果から引く
  - `import` の行で `{` の前: モジュール名の次の区切り（`import Util.` なら `Log`）．関数や型の名前は出さない．自分のモジュールは出さない
  - `import A.{` の中: モジュール A の pub な定義（enum のバリアントも）
  - `.` の後ろで，左が大文字名の並び: `.` の左の解決（4.6）と同じく import したモジュール，core のモジュール，Trait の順に引き，モジュールなら pub な定義，Trait なら関数
  - `.` の後ろで，それ以外: 左の式の型が能力の値なら effect の操作，type か error ならフィールド（説明は書いた型のテキスト）．それ以外の型は空
  - `use` の直後: エフェクトだけ．`with` の直後: handler と layer だけ
  - それ以外: 見える局所変数（内側が先），このモジュールの定義（バリアントも），import で列挙した名前，import したモジュール，名前が1区切りの core のモジュール（`Prelude` は除く），Prelude の定義，予約語（3.1．`as` は除く）の順．同じ名前は先のものだけ
- （補）局所変数の見える範囲は HIR から作る．引数は本体，束縛は右辺の後ろからブロックの終わり，ラムダの引数は本体，腕の変数はパターンから腕の本体の終わり．書きかけで位置がずれても困らないよう，範囲の終わりの位置も含める
- （補）行末の `.`（`log.`）は名前のない `.` で，名前解決が `Missing` にするので左の式は型推論に回らない．左の式の型は左からたどって求める．局所変数と定義はその宣言の型，`x.f` はフィールドの型（構成子のシグネチャの引数の型を，名前付き型の型引数で置き換えたもの）か能力の値の操作の型，呼び出しは呼ぶ先の関数型の戻り値．`l.from.`，`b.value.to.`，`c.now().` が引ける
- （補）呼び出しの引数の書き始め（`f(` の後ろかカンマの後ろで，書きかけの名前だけ）では，呼ぶ先のラベルのうちまだ書いていないものを先に出す（種類 `Label`，名前は `x:`，入れる文字列は `x: `，説明は引数の型）．値の候補も続けて出す．ラベルの後ろ（`x: ` の値の位置）では出さない．呼び出しの探し方とカンマでの分け方はシグネチャのヘルプと同じ．パターンのラベル（`User(na`）はまだ出さない
- （補）シグネチャのヘルプ（LSP）は `Frontend::signature_help`（`signature.rs`）．カーソルを括弧の中に含む一番内側の呼び出しの，呼ぶ先の型を `名前(ラベル: 型, …) -> 戻り値 fails E use R` の形で見せる．呼ぶ先は定義（関数と構成子のシグネチャとラベル），effect の操作（`ProgramTypes::ops`），それ以外の式の型（ラベルなし）の順に見る．閉じていない呼び出しは範囲の終わりの位置も括弧の中とみなす
- （補）カーソルのある引数は，書きかけの引数が `ラベル:` で始まればそのラベルの引数，そうでなければ括弧の入れ子の外のカンマの数（文字列の中は数えない）．書きかけの引数のラベルが残りの引数にないときは，カーソルのある引数を付けない
- （補）パイプの右辺の呼び出し（`x |> f(a)`．HIR で `Binary { op: Pipe }` の `rhs` がその呼び出し）では，シグネチャのヘルプも補完のラベルも第1引数を除いた形にする（`map(f: fn(A) -> B …) -> List[B]`）．カーソルのある引数も除いた後の位置で数える．型検査（`infer_pipe`）は左辺を位置引数の先頭に足して読み，位置引数は前から仮引数を埋めるので，名前付き引数をどう書いても左辺は宣言の第1引数に入る（`xs |> List.fold(init: 0, f: add)` の `xs` は第1引数 `xs`）．第1引数を名前で書く（`xs |> List.fold(xs: ys)`）と二重に埋まる型の誤りなので，ヘルプはカーソルのある引数を付けない．ラベルのない呼ぶ先（関数値）も同じく第1引数を除く．入れ子と連鎖は一番内側の呼び出しごとに判定する．右辺が呼び出しでない形（`x |> log.info`）は括弧がないのでヘルプは出ない
- （補）名前の変更（LSP）は `Frontend::prepare_rename` と `Frontend::rename`（`nav.rs`）．変える位置は参照の検索（定義を含む）と同じ．type とその構成子，effect の操作と handler の実装，import の `{x}` もまとめて変わる
- （補）変えられない名前: モジュール（ファイル名を変えることになる），`self`，同梱の core とソースのない定義，impl．新しい名前は元の名前と同じ字句クラス（2.3．小文字名，型名，大文字名）の字句1つでなければならない（予約語は別の字句なので通らない）．同じスコープの名前との衝突はまだ見ない
- （補）省略形の名前付き引数（`NotFound(id:)`）とフィールドのパターン（`User(name:)`）の名前は，ラベル（フィールドの名前）を残して `id: 新しい名前` に置き換える
- （補）コードアクション（LSP）は `Frontend::code_actions`（`actions.rs`）．今あるのは，トップレベルの fn（本体のあるもの）の省いた注釈に推論結果を挿入するものだけ（4.5）．範囲が fn の見出し（`fn` から本体の `{` の前まで）に掛かるときに出す．仕様は公開関数について書いているが，pub でない fn にも出す
  - 戻り値の型を省いていれば `-> T` と，推論した E が空でなければ `fails E` を一緒に足す．戻り値の型を書くと E は書いた fails 節に固定されるため（7.4）．戻り値の型があって fails を省いた fn は E が固定済みなので足さない
  - `use` 節を省いていれば，推論した R が空でなければ `use R` を足す（R は戻り値の型と別に推論する）．位置は fails 節か戻り値の型の後ろ
  - 引数の関数型の見えない集合（`E(f)`）は書けず，書いた節に暗に足される（9.2）ので，E と R から除く．戻り値の型に見えない集合が現れるときは戻り値の型（と fails）を足さない
  - このファイルで修飾なしに書けない定義（import で列挙していない他のモジュールの型とエラー）が現れる節は足さない．修飾した名前（`Other.Hidden`）を作るのは後回し
  - 題は `Add inferred `-> Int fails NotFound`` の形
- （補）インレイヒント（LSP）は `Frontend::inlay_hints`（`hints.rs`）．仕様 9.3「E と R は LSP が表示する」のために，次を出す
  - トップレベルの fn の省いた注釈: 戻り値の型を省いていれば `-> T fails E use R`（E と R は空でなければ），use 節だけを省いていれば `use R`．見えない集合（`E(f)`）もホバーと同じくそのまま出す．コードアクション（4.5）と違い，書けるかどうかは見ない
  - 型注釈のない束縛（パターンが変数1つのもの），ラムダの引数，handler の操作と impl の関数の引数の型 `: T`．能力の値（`use X` の値）と，推論の変数や壊れた型を含む型は出さない
  - 文の位置（文，束縛の右辺，ブロックの末尾）の呼び出しとパイプの E と R（`fails E use R`）．どちらも空なら出さない．入れ子の呼び出しには出さない（行が埋まるため．入れ子の E はホバーで見る）
- （補）semantic tokens（LSP）は `Frontend::semantic_tokens`（`tokens.rs`）．能力の値（8.2）だけを返す．型検査で型が能力（`Ty::Cap`）になった局所変数に加え，型検査が諦めた場合のために，右辺が `use X` の束縛（パターンが変数1つのもの）も能力の値とみなす（`use` の effect の名前が解決できなくても）．束縛した名前と，その局所変数を指す名前の式（クロージャの中も）に印を付ける．構文や型のエラーがあっても，HIR と型の分かる範囲で返す
- （補）`Lower` と `LowerTests`（build，run，test）は JS のターゲット（`JS_TARGET`）で外部関数の `@external` を検査する（12.1）．検査は lowering の中で単相化した IR に対して行う（操作はエフェクトの具体化ごとに見る．emela-core の NOTES の「外部関数」）．エントリからたどれる外部関数に JS の注釈がなければ E0412（``external function `wasm_now` has no `@external` for target `js` ``，注記 ``= help: add `@external(js, ...)`, or avoid calling it from the entry``）．`Typecheck`（check と LSP）は lowering しないので検査しない．E0412 を E04xx に置いたのは，外部関数が effect と同じく副作用の経路（12章の冒頭，E0407）で，E05xx〜E08xx が予約のため
- （補）lowering には各モジュールのソースのファイルのパスも渡す（`LowerInput::paths`）．`@external` のファイルの指定をソースの隣のファイルに直すため
- （補）同梱の core のソースは `Frontend::core_sources` で渡す（トレイトの既定は空）．パイプラインはモジュール名を `ModuleMap::add_core` で対応表に足し，仮のパス `<core>/list.emel`（`CORE_DIR`，`Int64` は `int64.emel`）でソースの表に載せる．単独ファイルでも core のモジュールは必ず読む
- （補）`Resolve::default()` は `emela_core::core_sources()` の全部（`CoreSource::bundled()`）を読む．テストで別の core を使うときは `with_core` で差し替える
- （補）`@test` の誤りのコードは E0226 にした．E0222〜E0225 は仕様の付録 A で部分更新と `@intrinsic` に振ってあったため
- （補）`Resolve::check` は名前解決の結果をそのまま `Program` として返す．型検査は `Typecheck` が重ねる．組み込みのモジュールは `NoBuiltins`（emela-core の組み込み関数の表が入ったら差し替える）
- （補）名前解決の診断（E0211〜E0221，W0201〜W0202）の英語の文面は `Diagnostic::from_resolve` が作る．重複の診断は先の定義に label を付け，「もしかして」の候補は `= help: did you mean ...` にする
- （補）助言 `= help: …` は，`Diagnostic` に欄を足さずに注記の頭 `help: `（`HELP_PREFIX`）で表す．`Diagnostic` は `Result` の誤りの側に置くので，大きくすると clippy の `result_large_err` に掛かる
- （補）`SourceDb` に同じパスを足すと，テキストを差し替えて同じ ID を返す（LSP の未保存バッファ向け）
- （補）`Typecheck` は名前解決に誤りがあっても型検査をする．解決できなかった名前は型推論が `Ty::Error` として扱い，誤りを重ねない（仕様の「誤りを全部集める」方針）
- （補）型検査の診断（E0221，E0301〜E0324，W0301〜W0302，E0401〜E0411，E0413〜E0422）は `Diagnostic::from_type` で写す．文面は emela-types の NOTES.md の「driver に渡すときの文面」と同じ．まだ扱えない構文はコードなしのエラー `not supported yet: …` にする
- （補）`Typecheck` は，エントリのモジュールの `main` という名前の fn と，`@test` のテスト関数を `CheckOptions` で型推論に渡す．型推論はそれらの R が空であることを確かめる（E0402，8.3，13.3）．そのためテスト関数の収集（`collect_tests`，`resolve_tests`）を型検査の前に行う．エントリのないとき（`check` にディレクトリを渡してエントリがないなど）は main を確かめない
- （補）型検査の診断は，名前解決と同じくファイルと位置の順に並べ直す（型推論は呼び出しの順に進むため）．名前解決の診断の後に置く
- （補）E0423（型引数のあるエフェクトの型引数を書いていない）の文面は，`use State` と use 節では ``missing type arguments for effect `State[S]` ``（注記 ``= help: write the types, like `State[...]` ``），`implements State` では ``handler `Fixed` must implement `State[S]` with concrete types``（注記 ``= help: write the types, like `implements State[...]`; a handler cannot be generic``）．E0424（エフェクトの型引数を集合として使った）は ``type parameter `E` of effect `Raise` is used as a set, but effect type parameters must be ordinary types``．仕様の付録 A に E0423 と E0424 はまだない
- （補）ホバーは handler の名前に `implements State[Int]`（型引数を代入したタグ），能力の値に `use State[Int]` を出す．能力の値の操作の補完と型は，操作の型のエフェクトの型引数を能力の値の型引数で置き換える
- （補）同梱のソースの仮のパスは，モジュール名の区切りをディレクトリにする（`Std.Console` は `<core>/std/console.emel`，`List` は `<core>/list.emel`）
