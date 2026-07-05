# Emela コンパイラ アーキテクチャ

このドキュメントは、本リポジトリの Emela コンパイラの内部構成を説明する。言語仕様そのものは
別リポジトリ `emela-lang/specification` に番号付き SPEC として置かれており、本書はあくまで
「コンパイラの実装がどう組み立てられているか」を対象とする。ソース中の `spec 00XX` という
参照は specification リポジトリの該当 SPEC を指す。

## 全体像

```
ソース (.emel)
   │
   ▼
┌─────────────────────────── crates/emela（フロントエンド + CLI）──────────────────────────┐
│  lexer.rs ──► parser.rs ──► imports.rs ──► (prelude マージ) ──► typecheck.rs ──► lower.rs │
│   字句解析      構文解析      import 解決      core prelude 注入     型検査          IR 化・   │
│                                                                                単相化      │
└──────────────────────────────────────────────────────────────────────┬───────────────────┘
                                                                       │  IrProgram
                                                                       ▼
┌──────────────── crates/emela-codegen（公開コア API）────────────────────────────────────┐
│  ir.rs / types.rs（IR と型）  backend.rs（Backend トレイト）  registry.rs（レジストリ）   │
│  intrinsic.rs / platform.rs（組込み関数の契約）  plugin.rs（外部プロセスプロトコル）      │
│  text.rs（`emela ir` 用テキスト表示）                                                     │
└───────────────┬─────────────────────────────┬────────────────────────────────────────────┘
                │                             │
                ▼                             ▼
   emela-backend-wasm (Tier 1)      emela-backend-js (Tier 2)      外部プロセスバックエンド
   WAT 生成 → wat で bin 化          JS ソース生成                   (JSON IR プロトコル)
```

クレート依存は一方向で、`emela-codegen` が最下層の公開コアである。バックエンドは
`emela-codegen` だけに依存すればよく、フロントエンド（`crates/emela`）には依存しない。
`crates/emela-wasm` はブラウザ・プレイグラウンド用の wasm-bindgen バインディングで、
デフォルトビルド対象からは外されている（`Cargo.toml` の `default-members` 参照）。

| クレート | 役割 |
| --- | --- |
| `emela-codegen` | IR・型・`Backend` トレイト・`Tier`・`Artifact`・レジストリ・外部プラグインプロトコル |
| `emela-backend-wasm` | WebAssembly (WASI preview1) バックエンド。Tier 1 |
| `emela-backend-js` | JavaScript (Node.js) バックエンド。Tier 2 |
| `emela` | フロントエンド（字句・構文・import・型検査・lowering）と CLI。`run` feature で `emela run` の組込みランタイム（`run.rs`）を含む |
| `emela-wasm` | プレイグラウンド用 wasm-bindgen バインディング |

## コンパイルパイプライン（crates/emela）

エントリポイントは 3 つある。

- CLI: `main.rs` → `driver.rs::run()`（`check` / `build` / `ir` のコンパイル系サブコマンドに加え、
  ビルドしたモジュールをその場で実行する `run`、`new` / `pome <verb>` のパッケージ管理サブコマンド。
  後者は `pome` モジュールへ委譲する）
- 組込み API: `api.rs` の `check_source` / `ir_source` / `compile_source`（ファイルシステム非依存、
  プレイグラウンドなどの埋め込み用途）
- ライブラリ: `lib.rs` が上記とエラー型・`Artifact` を再エクスポート

`driver.rs::compile_frontend_source()` が共通のフロントエンド処理を編成する。

### 1. 字句解析 — `lexer.rs`

`lex()` がソース文字列を `Vec<Token>` に変換する。各 `Token` は `TokenKind` と、診断用の
`Span`（ファイル・開始・終了位置）を持つ。UTF-8 のバイト走査ベース。

### 2. 構文解析 — `parser.rs`, `ast.rs`

再帰下降パーサ。`parse_program()` が `Program`（`fn` 定義・`extern`/`intrinsic` 宣言・
`enum`・`trait`・`impl`・`import`）を構築する。ジェネリクスのスコープ管理のため、パーサは
現在有効な型パラメータ集合（`Parser::type_params`）を保持する。

`ast.rs` は純粋なデータ定義であり、型システムの型（`Type`, `EffectRow` など）は
`emela-codegen::types` から再エクスポートして共有する。全ノードが `Span` を持つ。

### 3. import 解決 — `imports.rs`, `resolve.rs`

`imports.rs` が `import` 宣言をモジュールファイルに解決し、推移的に読み込む
（`emela-package.json` によるパッケージ探索、`resolving` セットによる循環検出）。
読み込んだ関数には import 修飾子のプレフィックスを刻印する（spec 0018）。パッケージルート
（`PackageSource`）は `--package` 由来のものに加え、対象ファイルを含む Pome の解決済み依存からも
供給される（`pome::dependency_packages`、spec 0032 M1。後述のパッケージング節を参照）。

`resolve.rs` の `FnTable` が名前解決の共通基盤で、各関数の完全パス
（モジュールパス + 名前）の全サフィックスをその関数に対応付ける。解決結果は
`Resolved::{None, One, Ambiguous}`。バックエンドに渡すシンボル名（`emit_name`）は
一意なら素の名前、衝突時は完全パスをマングルした名前になる。型検査と lowering の両方が
この同じテーブルを使うことで解決の一貫性を保っている。

### 4. prelude マージ — `prelude.rs`, `std/core.emel`

`std/core.emel`（`include_str!` で埋め込み）が Core Prelude。演算子トレイト
（`Add`/`Sub`/`Mul`/`Div`/`Rem`/`Concat`/`Eq`/`Ord`/`Show`、spec 0020）と、
Int/Float/String に対するそれらの実装、および実装が使う intrinsic 宣言（spec 0021）を
含む。driver がすべてのコンパイルに `core` モジュールとして注入する。

### 5. 型検査 — `typecheck.rs`

`check(program) -> Result<TypedProgram>` が入口。処理は 2 段階に分かれる。

1. **登録フェーズ**: `register_enums` / `register_traits` / `register_impls` /
   `register_functions` / `register_externs` が宣言を検証しつつルックアップ表を構築する。
   intrinsic は `emela_codegen::intrinsic_lookup` と照合され、純粋（効果なし・throws なし）で
   シグネチャ一致が要求される。
2. **本体検査フェーズ**: `check_expr` を中心に全式を走査する。各式は `ExprInfo`
   （型・効果・throws・Span）を返す。

主要な仕組み:

- **ジェネリクス**（spec 0014）: 直接呼び出しの実引数から `match_type` で型変数を確定し、
  境界（bounds）を `check_bound_satisfied` で再帰的に検証する。この段階では
  インスタンス化はしない（単相化は lowering の仕事）。
- **トレイトディスパッチ**: `dispatch_method` がレシーバ実引数から `Self` を推論し、
  一意に一致するトレイト/impl を探す。impl の検索キーは型ヘッド（`type_head_key`）。
  演算子は `operator_trait` でトレイトメソッド呼び出しに脱糖される。
- **エラーモデル**（spec 0011）: 関数は `throws E` で単一のエラー型を宣言する
  （`Result` 型は使わない）。式ごとに throws 型を伝播・`merge_throws` で合成し、
  `?` は throws の転送または `Option` の unwrap として型付けされる。`try/catch` は
  捕捉エラー型で match 腕を決める。
- **効果システム**: `EffectRow`（capability 文字列の集合）を式単位で合成し、
  関数宣言の `uses { ... }` と照合する。

`expand_trait_defaults` は型検査前にトレイトのデフォルトメソッドを各 impl に展開する。

### 6. lowering — `lower.rs`

`lower(program, typed) -> IrProgram`。型付き AST を `emela-codegen` の IR に変換する。

- **単相化（monomorphization）**: ジェネリック関数と impl メソッドは呼び出し点で
  特殊化要求（`MonoRequest`）としてワークリスト（`MonoState`）に積まれ、
  キューが空になるまで処理される（特殊化がさらに特殊化を生むため。spec 0014）。
- **名前マングリング**: 決定的でバックエンド非依存。
  - ジェネリック特殊化: `name__Type1__Type2`（例 `identity__Int`）
  - impl メソッド: `Trait__Type__method`（例 `Add__Int__add`）
  - 型のエンコード: `Array_Int_`, `Option_String_`, `Fn_Int_Float_to_Bool_` など
- **トレイト呼び出しの解決**: `lower_trait_call` / `resolve_impl_call` が lowering 済み
  実引数の型から `Self` を推論し、(トレイト, 型ヘッド) キーで impl を引いて
  マングル済みメソッドへの直接 `Call` を発行する。
- **ラムダのキャプチャ解析**: `lambda_captures` が自由変数を安定した順序で収集する。
  バックエンドはこの順序を環境レイアウトとして使う。
- **組込みの発行**: プラットフォーム関数は `IrExpr::Platform`（正準名、spec 0013）、
  intrinsic は `IrExpr::Intrinsic`（素の名前、spec 0021）として発行され、
  実装はバックエンドに委ねられる。

型検査を通過したプログラムだけが lowering に入るため、lowering は「検証済み」を前提に
書かれている（この前提が破れた場合の扱いは後述のリファクタリング課題を参照）。

## パッケージング（crates/emela/src/pome、spec 0032）

配布・依存の単位 **Pome**（Git リポジトリ）とその依存解決を実装する。中央レジストリを持たず、
Pome は canonical な **source path**（`host/path`）が指すリポジトリから直接取得する（R4）。
CLI は `emela pome <verb>`（`add` / `remove` / `list` / `update` / `install` / `search`）と
`emela new` で、`driver.rs` から `pome::run` / `pome::scaffold` へ委譲される。

サブモジュール構成（いずれも追加依存クレートなしで実装）:

| モジュール | 役割 |
| --- | --- |
| `toml_lite` | `Pome.toml` / `Pome.lock` / `Bushel.toml` 用の最小 TOML リーダ（テーブル・array-of-tables・文字列/文字列配列・ルートキー） |
| `source_path` | source path 正規化と省略スキーム（`github:` 等）。`git_url` は既定で `https://…​.git`、`EMELA_POME_REPLACE` で差し替え可（S1-S3, R4） |
| `semver` | `v` prefix semver と version requirement（完全一致・キャレット範囲）、最大 tag 選択（V1-V3） |
| `manifest` | `Pome.toml`（`[pome]` と `[dependencies]`）の読み書き。依存キーは canonical に正規化（F1-F4） |
| `lock` | `Pome.lock` の決定的エンコード。各依存の source path・tag・commit・content hash を記録（F5-F8） |
| `bushel` | ワークスペース `Bushel.toml`。member はルート共有の単一 lock を用いる（F9-F10） |
| `git` | `git ls-remote --tags` による tag 列挙、shallow clone による取得、commit と tree（content hash）の解決 |
| `resolve` | 各依存の `Pome.toml` を辿る推移的解決の不動点計算。選択版をキャッシュ（`EMELA_POME_CACHE`）へ取得し `Lock` を生成（V3, R4） |
| `capability` | 取得ソースの `uses { … }` と platform extern の effect row を合算し、要求 capability 集合を提示（CAP1-CAP3）。最終強制はサンドボックス側 |

content hash には git の tree オブジェクト id（`tree:<sha>`）を用いる。暗号ハッシュ用の追加依存を
避けつつ、内容アドレスで決定的な完全性検証になる（F6/F7）。`add` は解決結果を確定する前に
capability 集合を提示し、TTY なら確認プロンプトを出す（非対話時は続行）。

**build 時の import 接続（M1/M2）**: `driver.rs::compile_frontend`（ディスク読み込み経路）は、
対象ファイルを含む Pome の `Pome.lock` を `pome::dependency_packages` で読み、各依存 Pome を
import ルートとして登録する。ルート名は source path の leaf（`github.com/acme/mathlib` →
`mathlib`）、モジュール所在は取得 checkout の `src/`（無ければ checkout ルート）とする——これが
本実装での M2 の対応規則である。登録経路は既存の `--package` と同じ `imports::PackageSource` なので、
以降の解決・修飾呼び出し（spec 0018）は変更なく機能する。文字列ベースの埋め込み API
（`check_source` など）はファイルシステムに触れないため、この Pome 探索は行わない。ロックに固定済みだが
未取得の依存は `emela pome install` を促すエラーになる。

## IR とバックエンド境界（crates/emela-codegen）

### IR — `ir.rs`, `types.rs`

- `IrProgram` は `IrFunction` の列。
- `IrExpr` は約 20 バリアントの代数的データ型（リテラル、変数、制御フロー、`Call`、
  `Platform`、`Intrinsic`、クロージャ、`Match`/`Try`、`Throw`/`Question`、`Panic`、
  `EnumValue` など）。
- **完全型付き**: すべての式が `IrExpr::ty()` で型を返せる。codegen 段階での型推論は不要。
- Serde でシリアライズ可能（外部プラグインプロトコルのため）。
- `types.rs` の `Type` は `Unit/Bool/Int/Float/String/Char/Array/Record/Enum/Option/
  Never/Function/OpaqueFunction/Var`。`FunctionType` は params / ret / throws / `EffectRow`
  を持つ。

### Backend トレイトとレジストリ — `backend.rs`, `registry.rs`

```rust
trait Backend {
    fn name(&self) -> &str;
    fn tier(&self) -> Tier;               // Tier1: CI ゲート / Tier2: スモーク / Tier3: ベストエフォート
    fn compile(&self, ir: &IrProgram, opts: &BackendOptions) -> Result<Artifact>;
}
```

`Artifact` は種別（`JsSource` / `WasmBinary` / `WasmText` / `Other`）とバイト列。
`BackendOptions` は `EmitMode`（`Default` / `Text`）・target・runtime を持つ。
`BackendRegistry` はコンパイル時に登録されるインプロセスバックエンドの一覧で、
driver が `canonical_backend()`（`js`→`js-node`、`wasm`→`wasm-wasi` の別名解決）を
経て検索する。

### 外部プラグインプロトコル — `plugin.rs`, `crates/emela/src/external.rs`

バックエンドは外部プロセスでもよい。`backend.json`（`BackendDescriptor`）で宣言し、
driver は `--backend PATH` がディスクリプタなら `external.rs::ExternalBackend` を使う。
プロトコルは stdin/stdout の JSON IPC で、`PluginRequest`（IR 全体 + オプション）→
`PluginResponse`（Artifact または診断付きエラー）。`abi_version` で前方互換を管理する。

### intrinsic とプラットフォーム関数の契約 — `intrinsic.rs`, `platform.rs`

- **intrinsic**（spec 0021）: `i32_add` など 14 個の固定集合。素の名前で識別され、
  意味論はバックエンドが定義する（コンパイラは意味を持たない）。純粋であることが
  型検査時に強制される。バックエンドはコンパイル前に「使用されている intrinsic を
  すべて提供できるか」をカバレッジチェックする。
- **プラットフォーム関数**（spec 0013）: `io.write_stdout` のような `path.name` 正準名。
  各関数は必要 capability を宣言する。どのバックエンドがどれを提供するかは
  `backend.json` の externs 配列で宣言される。

### テキスト表示 — `text.rs`

`emit_text()` は `emela ir` コマンド用の人間可読な IR ダンプ。バックエンド間通信には
使われない（そちらは Serde JSON）。

## バックエンド実装

### WASM バックエンド（Tier 1）— `emela-backend-wasm`

WAT テキストを組み立て、`wat` クレートでバイナリ化し `wasmparser` で検証する。

- **値表現**: Int/Bool/Unit → `i32`（Unit は 0）、Float → `f64`、
  String/Array/関数値 → 線形メモリへの `i32` ポインタ。
- **クロージャ変換**: すべての関数（トップレベル・ラムダとも）が第一引数に
  環境ポインタを取る。関数値はヒープ上の `[table_index: i32, capture...]`。
  直接呼び出しは `call`、間接は関数テーブル経由の `call_indirect`。
- **メモリレイアウト**: `[0,16)` は WASI iovec 用スクラッチ、続いて interned 文字列
  （`[len: i32][utf8]` のデータセグメント）、その後がバンプアロケータのヒープ
  （8 バイト整列）。
- **enum/Option**: `[tag: i32][フィールド×8 バイト]`。
- **エラーフロー**: throws は `[ok フラグ][値または誤り値]` の結果ポインタ、
  `try/catch` はネストした wasm ブロックラベルへの分岐で実装。
- **main**: Int を返すと終了コードになる。

### JS バックエンド（Tier 2）— `emela-backend-js`

JavaScript ソースを生成する。JS のネイティブなクロージャ・文字列・関数値を
そのまま使うため、クロージャ変換は不要。

- **エラーフロー**: `EmelaError`（throw 値のラッパ）、`EmelaNone`（`Option` の `?`
  伝播）、`EmelaPanic`（回復不能）の 3 クラスを例外として使う。
- **プラットフォーム関数**: `__rt` オブジェクトに実装をバンドル。
- **intrinsic**: JS 演算子（`+`, `-`, `===`, `<` など）にインライン展開。

### 組込みランタイム — `crates/emela/src/run.rs`（`run` feature）

`emela run FILE` は `wasm-wasi` バックエンドでビルドしたモジュールを、外部ランタイムに
シェルアウトせず純 Rust の [`wasmi`] インタプリタでインプロセス実行する（default features に含む）。
生成モジュールが import する WASI 関数は `proc_exit`（常時）と `fd_write`（IO 使用時のみ）の 2 つだけなので、
フル WASI を持ち込まずこの 2 関数だけを host 関数として登録する（spec 0013/0025 の「必要な platform 関数
のみ供給する」方針と整合）。

- **`proc_exit(code)`**: 終了コードを載せた host エラー（`Exit`）で trap し、`_start` を巻き戻す。
  `_start` は必ず `proc_exit` を呼ぶため、正常実行は常にこの経路で終わる。
- **`fd_write(fd, iovs, iovs_len, nwritten)`**: 線形メモリから iovec 群を読み、fd 1→stdout / 2→stderr
  へ書き出して書込みバイト数を返す。`proc_exit` はプロセスを終えるため書き込みは都度 flush する。
- **終了コード**: `_start` の呼び出しが `Exit` エラーで返れば `downcast` してその値を、`unreachable`
  （panic）等の本物の trap は実行時エラーとして CLI に報告する。

`run` feature を外したビルドでは `emela run` は明示エラーになる。

[`wasmi`]: https://github.com/wasmi-labs/wasmi

### プレイグラウンド — `emela-wasm`

wasm-bindgen で `compile(source, target) -> String (JSON)` を 1 つエクスポートする。
target は `check` / `ir` / `js` / `wasm`。`api.rs` の文字列ベース API を使うため
ファイルシステムに触れない。

## 横断的な設計上の取り決め

- **識別子は文字列キー**: 型・トレイト・関数は `String` で引く（簡潔さ優先の設計判断）。
  `Self` は型変数 `Var("Self")` というマジック値で表す。
- **単一エラーチャネル**: 関数のエラーは `throws E` の単一型。`merge_throws` で
  互換性のない合成は型エラー。
- **フェーズの信頼境界**: 型検査が唯一の検証点で、lowering・バックエンドは
  「型検査済み IR は妥当」という前提で動く。IR は完全型付きなので、バックエンドが
  再検査する必要はない。
- **決定的マングリング**: 特殊化名はバックエンド非依存・決定的で、どのバックエンドでも
  同じシンボル名になる。

## テスト構成

`crates/emela/tests/` に統合テストがある: `minimal_cli.rs`（CLI 全体）、`generics.rs`、
`traits.rs`、`intrinsic.rs`、`platform.rs`、`external_plugin.rs`（外部バックエンド
プロトコル）、`wasm_examples.rs`（`examples/` の実行）、`run.rs`（`emela run` の
インプロセス実行：終了コード・stdout・trap）、`pome_cli.rs`（パッケージ管理。
ローカル Git リポジトリを upstream に見立て、`EMELA_POME_REPLACE` で解決経路を差し替えて
オフラインで検証する）。リファクタリングの安全網は主にこれらの統合テストである。`pome`
モジュールは純粋ロジック（正規化・semver・TOML・lock エンコード）に対するユニットテストも持つ。
