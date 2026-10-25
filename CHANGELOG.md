# Changelog

All notable changes to the Emela compiler are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/) (in the `0.y.z` range, a minor
bump may include breaking language changes while the language stabilizes).

## [Unreleased]

## [0.25.0](https://github.com/emela-lang/emela/compare/v0.24.0...v0.25.0) - 2026-10-25

### Added

- HTTP サービスを WASM で wasi:http/service の component にする ([#439](https://github.com/emela-lang/emela/pull/439)) (by @Allianaab2m)
- emela run が起動前にコマンドを標準エラーに出す ([#426](https://github.com/emela-lang/emela/pull/426)) (by @Allianaab2m)
- emela test の出力を --reporter で pretty，plain，agent から選べる ([#424](https://github.com/emela-lang/emela/pull/424)) (by @Allianaab2m)
- host に関数を公開する export と入口アダプタを書ける ([#430](https://github.com/emela-lang/emela/pull/430)) (by @Allianaab2m)
- handler の init に引数を渡せる（with H(x: ...)） ([#398](https://github.com/emela-lang/emela/pull/398)) (by @Allianaab2m)
- @entry(Http.serve) で HTTP サービスを書ける ([#434](https://github.com/emela-lang/emela/pull/434)) (by @Allianaab2m)
- WASM のターゲットで Fiber.attempt と Std.Cause を動かす ([#433](https://github.com/emela-lang/emela/pull/433)) (by @Allianaab2m)
- テストの失敗に，失敗した assert の場所を出す ([#425](https://github.com/emela-lang/emela/pull/425)) (by @Allianaab2m)
- Flow IRから組み込み向けCを生成するbackendを追加 ([#384](https://github.com/emela-lang/emela/pull/384)) (by @aoi-netai)

### Other

- [5/10] feat(wasm): const の評価と defect の note を実装 ([#402](https://github.com/emela-lang/emela/pull/402)) (by @isoshigi)
- emela new のサブコマンド追加（#214） ([#283](https://github.com/emela-lang/emela/pull/283)) (by @Satoru314)
- 仕様を spec/ に整理し，英語にして，proposal の手順と skills を足す ([#388](https://github.com/emela-lang/emela/pull/388)) (by @Allianaab2m)
- 組み込み標準ライブラリとC接続を追加 ([#385](https://github.com/emela-lang/emela/pull/385)) (by @aoi-netai)
- [10/10] feat(wasm): Std.FileSystem を WASI 0.3 の file system で動かす ([#407](https://github.com/emela-lang/emela/pull/407)) (by @isoshigi)
- [9/10] feat(wasm): Std.Clock.sleep を WASI 0.3 の async ホストで動かす ([#406](https://github.com/emela-lang/emela/pull/406)) (by @isoshigi)
- [6/10] feat(wasm): Console/Env/Random/Clock.now を WASI 0.3 で使えるようにする ([#403](https://github.com/emela-lang/emela/pull/403)) (by @isoshigi)
- [8/10] feat(wasm): suspend fn の状態機械と継続の再開を実装 ([#405](https://github.com/emela-lang/emela/pull/405)) (by @isoshigi)
- [4/10] feat(wasm): WASI 0.3 の component を出力する ([#401](https://github.com/emela-lang/emela/pull/401)) (by @isoshigi)
- [3/10] feat(wasm): Float の表示・parse・Int 変換のランタイムを追加 ([#400](https://github.com/emela-lang/emela/pull/400)) (by @isoshigi)
- [2/10] feat(wasm): String の書記素クラスタを Unicode 17.0 の UAX #29 で実装 ([#399](https://github.com/emela-lang/emela/pull/399)) (by @isoshigi)
- [1/10] refactor(wasm): emitter と runtime を機能別モジュールへ分割 ([#376](https://github.com/emela-lang/emela/pull/376)) (by @isoshigi)
- [7/10] test(wasm): C コーパスと examples を WASM で実行する ([#404](https://github.com/emela-lang/emela/pull/404)) (by @isoshigi)

## [0.24.0](https://github.com/emela-lang/emela/compare/v0.23.0...v0.24.0) - 2026-10-20

### Added

- inspect に Show を求め，error に derive を書けるようにする ([#362](https://github.com/emela-lang/emela/pull/362)) ([#365](https://github.com/emela-lang/emela/pull/365)) (by @Allianaab2m)
- ==，大小比較，補間を Eq，Ord，Show につなぐ ([#332](https://github.com/emela-lang/emela/pull/332)) ([#359](https://github.com/emela-lang/emela/pull/359)) (by @Allianaab2m)
- impl と handler の頭をコロンで書く ([#323](https://github.com/emela-lang/emela/pull/323)) ([#344](https://github.com/emela-lang/emela/pull/344)) (by @Allianaab2m)
- 無名関数の引数にパターンを書けるようにする ([#370](https://github.com/emela-lang/emela/pull/370)) (by @Allianaab2m)
- 前の impl と handler の頭に書き換えを示す ([#324](https://github.com/emela-lang/emela/pull/324)) ([#347](https://github.com/emela-lang/emela/pull/347)) (by @Allianaab2m)
- Hash を Prelude の Trait から外す ([#334](https://github.com/emela-lang/emela/pull/334)) ([#346](https://github.com/emela-lang/emela/pull/346)) (by @Allianaab2m)
- 基本型とタプル，List，Option に Eq，Ord，Show の impl を持たせる ([#331](https://github.com/emela-lang/emela/pull/331)) ([#355](https://github.com/emela-lang/emela/pull/355)) (by @Allianaab2m)
- trait と impl を型検査する ([#328](https://github.com/emela-lang/emela/pull/328)) ([#348](https://github.com/emela-lang/emela/pull/348)) (by @Allianaab2m)
- Prelude に inspect を足し，dbg から Show の制約を外す ([#343](https://github.com/emela-lang/emela/pull/343)) (by @Allianaab2m)
- 使われない derive を W0303 で知らせる ([#335](https://github.com/emela-lang/emela/pull/335)) ([#358](https://github.com/emela-lang/emela/pull/358)) (by @Allianaab2m)
- derive Eq，Ord，Show から impl を作る ([#333](https://github.com/emela-lang/emela/pull/333)) ([#356](https://github.com/emela-lang/emela/pull/356)) (by @Allianaab2m)
- 戻り値にだけ Self がある Trait の関数で Self が決まらなければ E0333 にする ([#357](https://github.com/emela-lang/emela/pull/357)) (by @Allianaab2m)
- 単相化で impl を選ぶ ([#330](https://github.com/emela-lang/emela/pull/330)) ([#350](https://github.com/emela-lang/emela/pull/350)) (by @Allianaab2m)
- Trait.f(x) を解決し，型引数の制約を呼び出しの場所で検査する ([#329](https://github.com/emela-lang/emela/pull/329)) ([#349](https://github.com/emela-lang/emela/pull/349)) (by @Allianaab2m)
- Std.Map と Std.Set を足す ([#340](https://github.com/emela-lang/emela/pull/340)) ([#366](https://github.com/emela-lang/emela/pull/366)) (by @Allianaab2m)
- List，String，Int64 の関数を足し，List.sort を Ord で書く ([#339](https://github.com/emela-lang/emela/pull/339)) ([#363](https://github.com/emela-lang/emela/pull/363)) (by @Allianaab2m)
- std の型に derive と Show の impl を付ける ([#337](https://github.com/emela-lang/emela/pull/337)) ([#361](https://github.com/emela-lang/emela/pull/361)) (by @Allianaab2m)
- 制約を持つ型引数のある pub の fn を JS で export するとき E0327 にする ([#354](https://github.com/emela-lang/emela/pull/354)) (by @Allianaab2m)
- core と std の性質を brix で確かめる tests/laws と，Std.Test を std から外す ([#320](https://github.com/emela-lang/emela/pull/320)) (by @Allianaab2m)

### Other

- Std.Map を opaque な enum にし，平衡の処理を読みやすく書き直す ([#369](https://github.com/emela-lang/emela/pull/369)) ([#371](https://github.com/emela-lang/emela/pull/371)) (by @Allianaab2m)
- merge queue を入れ，テストを 3 台に分けて回し，docs だけの変更ではテストを飛ばす ([#345](https://github.com/emela-lang/emela/pull/345)) (by @Allianaab2m)
- *(pome)* 既定の proxy と sumdb を emela.dev のサブドメインにする ([#360](https://github.com/emela-lang/emela/pull/360)) (by @Allianaab2m)

## [0.23.0](https://github.com/emela-lang/emela/compare/v0.22.0...v0.23.0) - 2026-10-19

### Added

- impl に足りない Trait のメソッドを E0325 にする ([#317](https://github.com/emela-lang/emela/pull/317)) (by @Allianaab2m)
- テストのルート test/ と，そこからだけ使える [dev-dependencies] ([#312](https://github.com/emela-lang/emela/pull/312)) (by @Allianaab2m)
- *(pome)* テストでだけ使う依存 [dev-dependencies] と emela pome add --dev ([#311](https://github.com/emela-lang/emela/pull/311)) (by @Allianaab2m)
- *(lsp)* 診断を直す quickfix と impl のスタブ生成を足す ([#315](https://github.com/emela-lang/emela/pull/315)) (by @Allianaab2m)
- *(lsp)* hover，signatureHelp，completion に ## の doc コメントを出す ([#314](https://github.com/emela-lang/emela/pull/314)) (by @Allianaab2m)
- *(lsp)* documentHighlight，workspace/symbol，foldingRange，selectionRange，typeDefinition，implementation を足す ([#313](https://github.com/emela-lang/emela/pull/313)) (by @Allianaab2m)

### Other

- *(lsp)* quickfix のテストで初期の診断を先に読み，shutdown との競合をなくす ([#319](https://github.com/emela-lang/emela/pull/319)) (by @Allianaab2m)

## [0.22.0](https://github.com/emela-lang/emela/compare/v0.21.0...v0.22.0) - 2026-10-18

### Added

- @external が指すファイルを JS の出力先に写して相対パスで読む ([#307](https://github.com/emela-lang/emela/pull/307))
- *(pome)* emela pome add --allow-external と，lock の @external の一覧 ([#306](https://github.com/emela-lang/emela/pull/306))
- *(pome)* 依存の @external を [external] allow で許し，[pome].targets を検査する ([#301](https://github.com/emela-lang/emela/pull/301))
- *(pome)* 依存の Pome のモジュールをルート名の下に見せて build に組み込む ([#297](https://github.com/emela-lang/emela/pull/297))
- *(pome)* emela pome の add，remove，install，update，list，verify ([#287](https://github.com/emela-lang/emela/pull/287))
- Std.Test に PBT とアサーション補助を同梱する ([#281](https://github.com/emela-lang/emela/pull/281))
- *(core)* 数値ヘルパーと Bool の変換関数を追加 ([#278](https://github.com/emela-lang/emela/pull/278))
- *(pome)* github.com のリポジトリの中のディレクトリの Pome を取る ([#288](https://github.com/emela-lang/emela/pull/288))
- *(pome)* emela-pome crate（Pome の解決，取得，sumdb の検証） ([#284](https://github.com/emela-lang/emela/pull/284))

### Fixed

- ライブラリ専用プロジェクトの入口と build/run を修正 ([#266](https://github.com/emela-lang/emela/pull/266)) ([#275](https://github.com/emela-lang/emela/pull/275))
- CLI のテストが同じ名前の一時ディレクトリを共有しないようにする ([#252](https://github.com/emela-lang/emela/pull/252))
- 文字列リテラルの \t，\r，\u{...} と """ の複数行の文字列を仕様 2.4 のとおりの値にする ([#280](https://github.com/emela-lang/emela/pull/280))
- const の自己参照が無限ループする問題を修正 ([#276](https://github.com/emela-lang/emela/pull/276))
- WASM のスクラッチが静的なデータと重ならないようにする ([#257](https://github.com/emela-lang/emela/pull/257))

### Other

- clarify trailing operator line continuation ([#260](https://github.com/emela-lang/emela/pull/260)) ([#274](https://github.com/emela-lang/emela/pull/274))
- WASM の線形メモリの配置を layout にまとめる ([#258](https://github.com/emela-lang/emela/pull/258))
- reuse C runtime objects across integration tests ([#277](https://github.com/emela-lang/emela/pull/277))
- 依存のテストの [pome].emela を今のコンパイラの版にする ([#308](https://github.com/emela-lang/emela/pull/308))

## [0.21.0](https://github.com/emela-lang/emela/compare/v0.20.1...v0.21.0) - 2026-10-17

### Added

- WebAssembly ターゲット（16章）への出力と実行 ([#251](https://github.com/emela-lang/emela/pull/251))
- build，run，test に --target c を足し，cc で native の実行ファイルを作る ([#240](https://github.com/emela-lang/emela/pull/240))
- JS の経路でもジェネリクスを単相化する ([#246](https://github.com/emela-lang/emela/pull/246))
- emela doc で pub な宣言の文書を HTML と Markdown に書き出す（2.2，9.2） ([#220](https://github.com/emela-lang/emela/pull/220))
- エフェクトのラベル（effect L of E, with L by P）([#256](https://github.com/emela-lang/emela/pull/256))
- Heap IR から C11 を出す emela-codegen-c と C のランタイムを足す ([#239](https://github.com/emela-lang/emela/pull/239))
- E0412 の検査を単相化した IR で行い，操作をエフェクトの具体化ごとに見る ([#247](https://github.com/emela-lang/emela/pull/247))
- C のターゲットで SystemFileSystem を動かす ([#249](https://github.com/emela-lang/emela/pull/249))
- C のターゲットにイベントループを入れ，suspend fn の外部関数を使えるようにする ([#248](https://github.com/emela-lang/emela/pull/248))
- E0412 の検査で，呼ばれる操作の本体だけをたどる ([#242](https://github.com/emela-lang/emela/pull/242))
- WASM の経路のために Core IR を単相化するパスを足す ([#232](https://github.com/emela-lang/emela/pull/232))
- C のターゲットで suspend と fiber を動かす ([#244](https://github.com/emela-lang/emela/pull/244))
- Heap IR で suspend する関数を再開できる状態機械にする（emela_lir::state_machines） ([#243](https://github.com/emela-lang/emela/pull/243))
- Heap IR に RC（Perceus）の dup と drop を入れ，検証器で釣り合いを確かめる（emela_lir::insert_rc） ([#237](https://github.com/emela-lang/emela/pull/237))
- Heap IR で defect と protect の後始末を明示的な制御にする（emela_lir::explicit_defects） ([#236](https://github.com/emela-lang/emela/pull/236))
- Heap IR の段とクロージャ変換を足す（emela_lir::to_heap の1つ目のパス） ([#235](https://github.com/emela-lang/emela/pull/235))
- 単相化した Core IR を Flow IR に変換する（emela_lir::from_core） ([#233](https://github.com/emela-lang/emela/pull/233))
- 低い IR の前の段（Flow IR）の骨組みを emela-lir に置く ([#229](https://github.com/emela-lang/emela/pull/229))

### Fixed

- C の経路で汎用のテストを落とさず，em_task の遅い2回目の完了を検出する ([#250](https://github.com/emela-lang/emela/pull/250))
- C で実行時に空の区切りの String.split を ASCII でない文字列に使ったら defect にする ([#245](https://github.com/emela-lang/emela/pull/245))

### Other

- C のエミッタを Writer 経由にし，ドライバーの Backend を target に依らない名前にする ([#254](https://github.com/emela-lang/emela/pull/254))
- 型検査のグループごとの表を疎な表にし，check を定義数に対して線形にする ([#227](https://github.com/emela-lang/emela/pull/227))
- エフェクトの操作に型を持たせ，操作の呼び出しとハンドラを検証器で確かめる ([#231](https://github.com/emela-lang/emela/pull/231))
- IR に型引数とラムダの型を持たせ，式の型と型の検証器を足す ([#230](https://github.com/emela-lang/emela/pull/230))
- IR の型を表に登録して TypeId で持つ ([#228](https://github.com/emela-lang/emela/pull/228))
- 自己末尾呼び出しのループ化を lowering の最後で1回だけかける ([#226](https://github.com/emela-lang/emela/pull/226))
- IR の Type が関数型と能力の値と型引数を落とさないようにする ([#224](https://github.com/emela-lang/emela/pull/224))
- @external の指す先を構造で持ち，同梱ファイルの出力先を JS 側で決める ([#223](https://github.com/emela-lang/emela/pull/223))
- 組み込み関数の JS の実装名を IR の表から codegen-js へ移す ([#222](https://github.com/emela-lang/emela/pull/222))
- examples と C の出力のテストのプログラムを JS と C の両方で動かして比べる ([#241](https://github.com/emela-lang/emela/pull/241))

## [0.20.1](https://github.com/emela-lang/emela/compare/v0.20.0...v0.20.1) - 2026-10-13

### Added

- パターンの数のリテラルの前に - を書けるようにする（6.5，18.1 #17） ([#210](https://github.com/emela-lang/emela/pull/210))
- Vim の色付けを直し，emela lsp の semantic tokens で能力の値に下線を引く ([#211](https://github.com/emela-lang/emela/pull/211))

### Fixed

- どのクレートの変更でも release PR が出るよう release-plz をそろえる ([#212](https://github.com/emela-lang/emela/pull/212))

## [0.20.0](https://github.com/emela-lang/emela/compare/v0.10.0...v0.20.0) - 2026-10-13

0.20 系の作り直しの最初の正式版。0.1x とは別の言語で、互換性はない。0.1x のコンパイラは
`legacy` ブランチにある。言語仕様は `docs/spec/emela-0.20.md`。

### Added

- 純粋な部分: 関数、レコード、enum、`opaque` 型、パターンマッチ、末尾呼び出し、パイプ、
  `List` / `Option`、モジュール、`@test`、`dbg`（alpha.1）
- 型付きのエラーとエフェクト: `error`、`fail`、`escape`、`effect`、`handler`、`use`、`with`、
  `@external`、`Console`（alpha.2）
- `suspend fn`、`Async`、`Immediate`、`Clock`、`Random`、`Env`、`Instant` / `Duration`（alpha.3）
- フィールドを持つハンドラ、`init` と `release`、`layer`、`State`、`Std` 名前空間、`Platform`（alpha.4）
- ファイバー（`Std.Fiber`、`Std.Cause`、`Std.Deferred`）、`FileSystem`、`Std.Path`、`Http` クライアント（alpha.5）

### Changed

- **破壊的変更**: `match` を後置にした（`x match { ... }`）。前置の `match x { ... }` は E0142
- `const` は最初に参照したときに1回だけ評価する。名前だけの循環は E0231
- ソースのルートの外のディレクトリを `check`、`build`、`run`、`test` に渡すと E0208
- 診断とコンパイラの出力を英語にした

## [0.10.0](https://github.com/emela-lang/emela/compare/v0.9.1...v0.10.0) - 2026-07-26

### Added

- std.list を effect row 多相へ、each を追加（spec 0022 案B、PR4/4） ([#105](https://github.com/emela-lang/emela/pull/105))
- effect row の erasure と LSP 表示（spec 0022 案B、PR3/4） ([#104](https://github.com/emela-lang/emela/pull/104))
- effect row 多相の型検査（row 単一化と事後 subsumption、spec 0022 案B、PR2/4） ([#103](https://github.com/emela-lang/emela/pull/103))
- effect row 多相の構文とデータ表現（spec 0022 案B、PR1/4） ([#99](https://github.com/emela-lang/emela/pull/99))
- wasm-wasip2及びjsバックエンドにFs capabilityを追加（spec 0055） ([#102](https://github.com/emela-lang/emela/pull/102))
- LSP に impl スタブ生成の codeAction を追加（spec 0033） ([#100](https://github.com/emela-lang/emela/pull/100))
- LSP に codeAction（match アーム補充）と診断 code を追加（spec 0033） ([#98](https://github.com/emela-lang/emela/pull/98))
- LSP に textDocument/hover を追加（span→型インデックス，spec 0033） ([#97](https://github.com/emela-lang/emela/pull/97))

### Fixed

- record・enum ペイロード・トレイトメソッドでも関数値の row と throws を検査（spec 0023） ([#106](https://github.com/emela-lang/emela/pull/106))

### Added

- effect row 多相（spec 0022）: 関数が呼び出し側の effect row をそのまま通せるようになった。
  `fn map<T, U, e>(list: List<T>, f: (T) -> U uses e) -> List<U> uses e` のように、`<...>` の
  小文字要素を row パラメータとして宣言し、コールバックの `uses` 行と関数自身の行に置く。
  row は呼び出しごとに単一化され、lowering で消える（実行時の表現は変わらない）。
- `std.list` の `each` / `map` / `filter` / `fold` / `fold_map` が row 多相になった。
  `list.each(xs, show)` のように effectful なコールバックを渡すと、その effect が呼び出し元に
  伝播する（`each` は新規追加）。

### Changed

- **破壊的変更**: 小文字始まりの型パラメータはエラーになった。`<...>` の要素は先頭が大文字なら
  型パラメータ、小文字なら effect row パラメータとして解釈される（spec 0022）。
- **破壊的変更**: pure 宣言のコールバック（`f: (T) -> T`）に effectful な関数値を渡すとエラーに
  なった。従来はジェネリック関数の呼び出しで effect が静かに失われていた（健全性の修正）。
  署名を row 多相（`f: (T) -> T uses e`）にするか、パラメータの row を広げて移行する。
- **破壊的変更**: 非 throwing 宣言のコールバックに throwing な関数値を渡すとエラーになった
  （同じ穴の `throws` 側）。クロージャ内の `try` / `catch` で非 throwing 化して移行する。

### Fixed

- 関数値の `uses` 行と `throws` の検査漏れを塞いだ（spec 0023）。record リテラル・enum の
  ペイロード・トレイトメソッドの引数は型変数の束縛（`match_type`）しか通っておらず、
  effect row と `throws` が比較されていなかった。そのため pure 宣言のフィールドに
  effectful な関数を格納でき、`uses {}` の `main` からその effect を実行できてしまっていた。
  **破壊的変更**: これらの位置に、宣言より広い row を持つ関数値や throwing な関数値を
  渡すコードはエラーになる（フィールド / パラメータの宣言を広げるか、`try` / `catch` で
  閉じて移行する）。

## [0.9.1](https://github.com/emela-lang/emela/compare/v0.9.0...v0.9.1) - 2026-07-23

### Fixed

- record を対象にした impl の孤児判定を修正（spec 0020） ([#95](https://github.com/emela-lang/emela/pull/95))

## [0.9.0](https://github.com/emela-lang/emela/compare/v0.8.1...v0.9.0) - 2026-07-22

### Added

- playground の実行（Run）サポートを追加（emela-wasm / docs 用） ([#94](https://github.com/emela-lang/emela/pull/94))
- シード可能 PRNG（xorshift32）を std.random に追加（spec 0054 Part B） ([#93](https://github.com/emela-lang/emela/pull/93))
- Random effect を wasi:random にバインド（spec 0054 Part A） ([#92](https://github.com/emela-lang/emela/pull/92))
- ビット演算子 `& | ^ ~ << >> >>>` を追加（spec 0053） ([#91](https://github.com/emela-lang/emela/pull/91))
- WASI 0.2 component-model backend wasm-wasip2（spec 0052） ([#90](https://github.com/emela-lang/emela/pull/90))
- HttpServer を Socket 上の派生 effect へ（spec 0046 改訂, PR3/3） ([#88](https://github.com/emela-lang/emela/pull/88))
- Socket の wasmi ホストと wasm backend glue（spec 0050, PR2/3） ([#87](https://github.com/emela-lang/emela/pull/87))
- Socket capability と registry を追加（spec 0050, PR1/3） ([#86](https://github.com/emela-lang/emela/pull/86))
- record 型にジェネリクスを導入（spec 0028） ([#85](https://github.com/emela-lang/emela/pull/85))
- effect のコンパイル時 DI（デフォルト経路, spec 0049） ([#84](https://github.com/emela-lang/emela/pull/84))
- Bytes 型を導入（spec 0051） ([#82](https://github.com/emela-lang/emela/pull/82))

## [0.8.1](https://github.com/emela-lang/emela/compare/v0.8.0...v0.8.1) - 2026-07-21

### Fixed

- implement Eq, Show Trait for http.Method ([#80](https://github.com/emela-lang/emela/pull/80))

## [0.8.0](https://github.com/emela-lang/emela/compare/v0.7.1...v0.8.0) - 2026-07-21

### Added

- 関数値の effect subsumption (spec 0023) ([#79](https://github.com/emela-lang/emela/pull/79))
- demote Option to a Core-Prelude enum (spec 0041/0042) ([#78](https://github.com/emela-lang/emela/pull/78))
- add the lang-item attribute for role binding (spec 0039/0041) ([#77](https://github.com/emela-lang/emela/pull/77))
- capability manifest & embedder-defined capabilities (spec 0025/0026) ([#73](https://github.com/emela-lang/emela/pull/73))

### Fixed

- remove non-conformant `?`-on-Option (spec 0011/0042) ([#76](https://github.com/emela-lang/emela/pull/76))
- 改行後の行頭二項演算子を式の継続としてパース ([#62](https://github.com/emela-lang/emela/pull/62)) ([#75](https://github.com/emela-lang/emela/pull/75))

## [0.7.1](https://github.com/emela-lang/emela/compare/v0.7.0...v0.7.1) - 2026-07-20

### Fixed

- close HTTP server connections gracefully to avoid client resets ([#68](https://github.com/emela-lang/emela/pull/68))

## [0.7.0](https://github.com/emela-lang/emela/compare/v0.6.0...v0.7.0) - 2026-07-20

### Added

- ARC — deterministic reference counting for the wasm backend (spec 0048) ([#67](https://github.com/emela-lang/emela/pull/67))
- HTTP client and server (specs 0043–0046) ([#61](https://github.com/emela-lang/emela/pull/61))
- Monoid trait with return-position Self dispatch (spec 0047) ([#65](https://github.com/emela-lang/emela/pull/65))
- [**breaking**] move Char/String/Array builtins to intrinsics (spec 0021) ([#64](https://github.com/emela-lang/emela/pull/64))

## [0.6.0](https://github.com/emela-lang/emela/compare/v0.5.0...v0.6.0) - 2026-07-19

### Added

- attributes and unit testing (specs 0039/0040) ([#60](https://github.com/emela-lang/emela/pull/60))
- [**breaking**] module-unit imports and first-class effects (spec 0037) ([#57](https://github.com/emela-lang/emela/pull/57))
- adopt clap for a friendly `emela --help` (closes #17) ([#58](https://github.com/emela-lang/emela/pull/58))

## [0.5.0](https://github.com/emela-lang/emela/compare/v0.4.0...v0.5.0) - 2026-07-18

### Added

- reserve embedded std module names (spec 0038) ([#56](https://github.com/emela-lang/emela/pull/56))
- intrinsic single-declaration rule (spec 0038) ([#54](https://github.com/emela-lang/emela/pull/54))
- embed std.io/clock/string/float as compiler-resolved modules (spec 0038) ([#53](https://github.com/emela-lang/emela/pull/53))

## [0.4.0](https://github.com/emela-lang/emela/compare/v0.3.0...v0.4.0) - 2026-07-12

### Added

- string/array/sqrt primitives and multi-module fixes for stdlib ([#52](https://github.com/emela-lang/emela/pull/52))
- add effect declarations (spec 0036) ([#50](https://github.com/emela-lang/emela/pull/50))

## [0.3.0](https://github.com/emela-lang/emela/releases/tag/v0.3.0) - 2026-07-12

### Added

- implement pipeline operator `|>` (spec 0019) ([#43](https://github.com/emela-lang/emela/pull/43))

## [0.2.1](https://github.com/emela-lang/emela/releases/tag/v0.2.1) - 2026-07-12

### Fixed

- preserve non-tail block expressions during lowering ([#38](https://github.com/emela-lang/emela/pull/38))

## [0.2.0](https://github.com/emela-lang/emela/releases/tag/v0.2.0) - 2026-07-05

### Added

- qualify enum variants and conversions with `::` (spec 0018 R7)
- qualified imports and calls (spec 0018)
- language primitives for pure to_string (if, /, %, Char, ++)
- implement throws-based error handling (spec 0011)
- platform functions resolved by the backend runtime (spec 0013)
- *(backend-wasm)* compile the full IR to WebAssembly (Tier 1)
- *(codegen)* add external-process backend plugins

### Other

- v0.2.0 ([#31](https://github.com/emela-lang/emela/pull/31))
- Merge generic functions (spec 0014) into feat/new-spec
- clock platform function (JS) and browser playground scaffolding
- split into a Cargo workspace with a published codegen core

### Added
- Language server: `emela lsp` (spec 0033) speaks LSP over stdio — diagnostics
  on open/change/save covering every compiler error, and context-aware
  completion (import paths, `match`/`catch` enum variants, `uses` effect names,
  `::` type paths, keywords, in-scope functions and locals). Editor setup lives
  in `docs/lsp.md`, with a VSCode client under `editors/vscode/`.
- Multi-error reporting (spec 0033): the frontend collects errors across
  declarations instead of stopping at the first — the lexer skips bad
  characters, the parser recovers at top-level declarations, imports and the
  type checker report per item — and `emela check` prints them all.
- Comparison operators `!=`, `>`, `<=`, `>=`, desugaring to `Eq`/`Ord` (spec 0027).
- Short-circuiting logical operators `&&`, `||`, and prefix `!` (spec 0027).
- Generic `enum` declarations with type parameters, including recursive types
  such as `List<T>` (spec 0028); type arguments are inferred at construction and
  each instantiation is monomorphized.
- Cross-module type imports: an imported module's `enum`/`trait`/`impl`
  declarations travel with its functions, so a package can export a type.
- `check --library` (alias `--lib`): type-checks a module that has no `main`.
- Core Prelude instances `Eq`/`Show for Bool` and `Eq`/`Ord for String`
  (the latter backed by new `string_eq` / `string_lt` intrinsics).
- Example standard library modules: `std.list`, `std.ord`, `std.int`, and a
  `std.option` starter.
- Packaging: **Pomes** and decentralized dependency management (spec 0032).
  `emela new <name>` scaffolds an entry Pome; `emela pome add|remove|list|update|
  install|search` manages dependencies. A Pome is any Git repository identified
  by its `host/path` source path (`github:acme/util` shorthands normalize to it),
  versioned by `v`-prefixed semver git tags and pinned to a commit + content hash
  in `Pome.lock`. There is no central registry — resolution fetches straight from
  the source-path repository. `emela pome add` computes and shows the capability
  set the added Pome and its transitive dependencies require, from source (0025),
  before writing. Workspaces (`Bushel.toml`) share a single lock. Building inside
  a Pome puts each locked dependency on the import search path:
  `import <root>.<module>.<item>` resolves against the fetched source, where
  `<root>` is the dependency's source-path leaf (`github.com/acme/mathlib` →
  `mathlib`) unless the Pome overrides it with `[pome].module` (spec 0032 M2) —
  so `github.com/emela-lang/stdlib` declaring `module = "std"` is imported as
  `std.io.print` — and its modules live under `src/`.

### Changed
- Shared IR traversal and intrinsic coverage checks moved into `emela-codegen`
  so the JS and wasm backends no longer duplicate them.

<!--
Release process:
  1. Land changes on `dev` (nightly prereleases publish automatically).
  2. Promote `dev` -> `main`, move this section under a new `## [x.y.z]` heading,
     and bump `version` in the workspace Cargo.toml.
  3. Tag `main`: `git tag vX.Y.Z && git push origin vX.Y.Z` -> stable release.
-->
