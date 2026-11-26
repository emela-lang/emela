# Changelog

All notable changes to the Emela compiler are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to
follow [Semantic Versioning](https://semver.org/) (in the `0.y.z` range, a minor
bump may include breaking language changes while the language stabilizes).

## [Unreleased]

## [0.29.0](https://github.com/emela-lang/emela/compare/v0.28.2...v0.29.0) - 2026-11-26

### Added

- 診断コードを 1000 ずつの帯に振り直し，同じ種類の誤りのコードをまとめる ([#905](https://github.com/emela-lang/emela/pull/905)) (by @Allianaab2m)
- Task.Group と Task.Background で並行に仕事を始められるようにする ([#849](https://github.com/emela-lang/emela/pull/849)) (by @Allianaab2m)
- [**breaking**] ビルドターゲット c を削除し，未知のターゲットにする ([#835](https://github.com/emela-lang/emela/pull/835)) (by @Allianaab2m)
- emela build の main.mjs を JS ホストで直接起動すると main を実行する ([#725](https://github.com/emela-lang/emela/pull/725)) (by @Satoru314)
- capability を型と R で区別し，Fiber.scope の本体が scope を引数で受け取る ([#813](https://github.com/emela-lang/emela/pull/813)) (by @Allianaab2m)
- *(cli)* 依存の取得と C コンパイラの実行中に進捗を出す ([#802](https://github.com/emela-lang/emela/pull/802)) (by @Allianaab2m)
- 効果の別名 x <- Effect を書ける．closure の R は呼ぶ側だけが満たす ([#791](https://github.com/emela-lang/emela/pull/791)) (by @Allianaab2m)
- Deferred・Queue・Semaphore を with の項目で作れるようにする ([#848](https://github.com/emela-lang/emela/pull/848)) (by @Allianaab2m)
- handler に型パラメータを書き，with・layer・by の項目ごとに型引数を決められるようにする ([#825](https://github.com/emela-lang/emela/pull/825)) (by @Allianaab2m)
- [**breaking**] 外部関数のホストの副作用を宣言したモジュールの External として R に入れる ([#896](https://github.com/emela-lang/emela/pull/896)) (by @Allianaab2m)
- 型引数を [C: Capability] で capability に制約できる ([#800](https://github.com/emela-lang/emela/pull/800)) (by @Allianaab2m)
- [**breaking**] 束縛した項目は名前で供給しない ([#888](https://github.com/emela-lang/emela/pull/888)) (by @Allianaab2m)
- @entry(Http.serve) の app が Spawn で仕事を始められるようにする ([#850](https://github.com/emela-lang/emela/pull/850)) (by @Allianaab2m)
- Std.Tcp で TCP に接続し，TLS に昇格できるようにする（JS） ([#845](https://github.com/emela-lang/emela/pull/845)) (by @Allianaab2m)
- suspend fn で resume の後に中断しうる呼び出しを禁止する ([#847](https://github.com/emela-lang/emela/pull/847)) (by @Allianaab2m)
- fn の型に現れない型引数に警告 W0305 を出す ([#823](https://github.com/emela-lang/emela/pull/823)) (by @Allianaab2m)
- Duration.parse で長さの文字列を読み，Duration を Encode，Decode，Shaped できるようにする ([#846](https://github.com/emela-lang/emela/pull/846)) (by @Allianaab2m)
- Instant.parse で日時の文字列を読み，Instant を Encode，Decode，Shaped できるようにする ([#801](https://github.com/emela-lang/emela/pull/801)) (by @somnicattus)
- *(lir)* normalize switches in LIR (E4) ([#775](https://github.com/emela-lang/emela/pull/775)) (by @isoshigi)
- background が閉じず、body の終了後も task を start できるようにする ([#886](https://github.com/emela-lang/emela/pull/886)) (by @Allianaab2m)
- native ターゲットの境界で Bytes を C に渡せるようにする ([#884](https://github.com/emela-lang/emela/pull/884)) (by @Allianaab2m)

### Fixed

- project の中の相対パスを Windows でも / で区切って出す ([#885](https://github.com/emela-lang/emela/pull/885)) (by @Allianaab2m)
- *(lsp)* Windows で emela lsp がドライブ文字のパスと URI を変換できず，同じ project のモジュールの import を undefined module にするのを直す ([#814](https://github.com/emela-lang/emela/pull/814)) (by @somnicattus)
- handler のフィールドの集合を，操作の本体と release の終わりで R から外す ([#887](https://github.com/emela-lang/emela/pull/887)) (by @Allianaab2m)
- *(core)* 型パラメータを持つ handler の member を開くとき，内側の handler の関数に型引数を渡す ([#901](https://github.com/emela-lang/emela/pull/901)) (by @somnicattus)
- 関数の値は R によらず evidence の表を取る ([#717](https://github.com/emela-lang/emela/pull/717)) ([#894](https://github.com/emela-lang/emela/pull/894)) (by @Allianaab2m)
- *(tcp)* 同じ接続への read と write が重なった defect の文言から fiber を外す ([#851](https://github.com/emela-lang/emela/pull/851)) (by @Allianaab2m)
- *(lir)* guard the tuple SRA plan against an out-of-range index ([#786](https://github.com/emela-lang/emela/pull/786)) (by @isoshigi)
- *(lir)* normalise switches inside failure handlers ([#785](https://github.com/emela-lang/emela/pull/785)) (by @isoshigi)
- *(fmt)* guard や if の条件の長い || chain を括弧なしで改行して失敗するのを直す ([#898](https://github.com/emela-lang/emela/pull/898)) (by @kisepichu)
- *(new)* emela new が生成する main.emel を effect alias で書く ([#890](https://github.com/emela-lang/emela/pull/890)) (by @Satoru314)
- *(pome)* Windows で Pome.lock がまだない project の emela pome install と add が os error 87 で失敗するのを直す ([#810](https://github.com/emela-lang/emela/pull/810)) (by @somnicattus)

### Other

- add the IR optimisation switch and the C runtime size classes ([#627](https://github.com/emela-lang/emela/pull/627)) (by @isoshigi)
- README の Status を 0.28.2 に合わせ，0.28 で入ったものを next から 0.28 の行に移す ([#824](https://github.com/emela-lang/emela/pull/824)) (by @Allianaab2m)
- 各クレートの統合テストを 1 つのテストバイナリにまとめる ([#796](https://github.com/emela-lang/emela/pull/796)) (by @Allianaab2m)
- *(core)* fuse an array pipeline ([#769](https://github.com/emela-lang/emela/pull/769)) (by @isoshigi)
- *(core)* fuse map and fold over a range ([#768](https://github.com/emela-lang/emela/pull/768)) (by @isoshigi)
- *(core)* fuse string concatenation ([#767](https://github.com/emela-lang/emela/pull/767)) (by @isoshigi)
- *(core)* remove the bounds checks the ranges prove ([#766](https://github.com/emela-lang/emela/pull/766)) (by @isoshigi)
- *(core)* remove common subexpressions in a let chain ([#765](https://github.com/emela-lang/emela/pull/765)) (by @isoshigi)
- *(core,lir)* check the range, borrow and ownership analyses ([#761](https://github.com/emela-lang/emela/pull/761)) (by @isoshigi)
- *(core)* count uses and build the call graph ([#758](https://github.com/emela-lang/emela/pull/758)) (by @isoshigi)
- *(core)* summarise effects and defects ([#757](https://github.com/emela-lang/emela/pull/757)) (by @isoshigi)
- *(core)* run the reduction pipeline by default ([#756](https://github.com/emela-lang/emela/pull/756)) (by @isoshigi)
- *(core)* move loop unswitching into the loop module ([#755](https://github.com/emela-lang/emela/pull/755)) (by @isoshigi)
- *(core)* expand small direct calls after monomorphisation ([#754](https://github.com/emela-lang/emela/pull/754)) (by @isoshigi)
- *(core)* reduce the boolean algebraic laws ([#753](https://github.com/emela-lang/emela/pull/753)) (by @isoshigi)
- *(core)* reduce a match on a known constructor ([#752](https://github.com/emela-lang/emela/pull/752)) (by @isoshigi)
- *(core)* drop a lambda that only forwards its arguments ([#751](https://github.com/emela-lang/emela/pull/751)) (by @isoshigi)
- *(core)* inline a lambda that is called once ([#750](https://github.com/emela-lang/emela/pull/750)) (by @isoshigi)
- *(core)* substitute let-bound copies, including inside lambdas ([#749](https://github.com/emela-lang/emela/pull/749)) (by @isoshigi)
- *(core)* register the fusion laws and test their equations ([#748](https://github.com/emela-lang/emela/pull/748)) (by @isoshigi)
- *(core)* complete the substitution engine ([#747](https://github.com/emela-lang/emela/pull/747)) (by @isoshigi)
- *(core)* fold operations on literals ([#746](https://github.com/emela-lang/emela/pull/746)) (by @isoshigi)
- *(core)* add the pass interfaces and counters ([#745](https://github.com/emela-lang/emela/pull/745)) (by @isoshigi)
- *(core,c)* inline array reads and hoist their bounds test ([#640](https://github.com/emela-lang/emela/pull/640)) (by @isoshigi)
- *(core)* build an array from a range without the list ([#638](https://github.com/emela-lang/emela/pull/638)) (by @isoshigi)
- *(core)* inline small non-recursive functions ([#635](https://github.com/emela-lang/emela/pull/635)) (by @isoshigi)
- *(core)* lower List.fold and List.map over a range to a loop ([#634](https://github.com/emela-lang/emela/pull/634)) (by @isoshigi)
- *(core)* fuse Option.or and match over Array.get ([#631](https://github.com/emela-lang/emela/pull/631)) (by @isoshigi)
- *(string)* convert ASCII case in one allocation ([#630](https://github.com/emela-lang/emela/pull/630)) (by @isoshigi)
- *(std)* update a Map in one descent ([#629](https://github.com/emela-lang/emela/pull/629)) (by @isoshigi)
- *(core)* add the Core IR optimisation pipeline and the differential harness ([#628](https://github.com/emela-lang/emela/pull/628)) (by @isoshigi)
- *(std)* std のドキュメントコメントで仕様を安定名で参照し，説明の無い公開 API に ## を書く ([#826](https://github.com/emela-lang/emela/pull/826)) (by @Allianaab2m)
- *(core)* IR の dump でローカル変数を関数ごとに 0 から数え，std の変更で関係のない snapshot が動かないようにする ([#807](https://github.com/emela-lang/emela/pull/807)) (by @somnicattus)
- *(lir)* share the reachability analysis and prune functions ([#778](https://github.com/emela-lang/emela/pull/778)) (by @isoshigi)
- *(lir)* replace a closure record with its scalar captures ([#776](https://github.com/emela-lang/emela/pull/776)) (by @isoshigi)
- *(lir)* reuse a uniquely owned record ([#774](https://github.com/emela-lang/emela/pull/774)) (by @isoshigi)
- *(lir)* remove dead pure lets and change the LIR defaults ([#773](https://github.com/emela-lang/emela/pull/773)) (by @isoshigi)
- *(lir)* specialise the drop of a block without references ([#772](https://github.com/emela-lang/emela/pull/772)) (by @isoshigi)
- *(lir)* infer borrowed parameters ([#771](https://github.com/emela-lang/emela/pull/771)) (by @isoshigi)
- *(lir)* drop the reference a borrowed view does not need ([#764](https://github.com/emela-lang/emela/pull/764)) (by @isoshigi)
- *(lir)* call a known closure directly ([#763](https://github.com/emela-lang/emela/pull/763)) (by @isoshigi)
- *(lir)* replace a non-escaping scalar record with its fields ([#762](https://github.com/emela-lang/emela/pull/762)) (by @isoshigi)
- *(lir)* analyse variable liveness ([#760](https://github.com/emela-lang/emela/pull/760)) (by @isoshigi)
- *(lir)* analyse which values escape ([#759](https://github.com/emela-lang/emela/pull/759)) (by @isoshigi)
- *(lir)* build a string of three or more pieces in one allocation ([#637](https://github.com/emela-lang/emela/pull/637)) (by @isoshigi)
- *(lir)* take back a reference that reference counting only lends ([#636](https://github.com/emela-lang/emela/pull/636)) (by @isoshigi)
- *(lir)* replace non-escaping tuples with scalars ([#633](https://github.com/emela-lang/emela/pull/633)) (by @isoshigi)
- *(lir)* split the interpolation concatenation and from_core ([#626](https://github.com/emela-lang/emela/pull/626)) (by @isoshigi)
- *(wasm)* hold records and tuples by value by default ([#782](https://github.com/emela-lang/emela/pull/782)) (by @isoshigi)
- *(wasm)* read inline array elements into value locals ([#781](https://github.com/emela-lang/emela/pull/781)) (by @isoshigi)
- *(wasm)* pass records by value and box them at escape boundaries ([#780](https://github.com/emela-lang/emela/pull/780)) (by @isoshigi)
- *(wasm)* store array elements inline ([#779](https://github.com/emela-lang/emela/pull/779)) (by @isoshigi)
- *(wasm)* hold records by value in locals ([#777](https://github.com/emela-lang/emela/pull/777)) (by @isoshigi)
- *(c)* hold records and tuples by value ([#639](https://github.com/emela-lang/emela/pull/639)) (by @isoshigi)
- *(codegen-c)* ランタイムのオブジェクトとサニタイザの確認をテストのプロセス間で使い回す ([#844](https://github.com/emela-lang/emela/pull/844)) (by @Allianaab2m)
- bytes_cross_to_c の待ち合わせを時間に頼らない形にする ([#891](https://github.com/emela-lang/emela/pull/891)) (by @Allianaab2m)
- run the record scalar replacement end to end ([#784](https://github.com/emela-lang/emela/pull/784)) (by @isoshigi)
- *(driver)* compare the optimisation of every target on the opt fixtures ([#744](https://github.com/emela-lang/emela/pull/744)) (by @isoshigi)
- insta の一時ファイル .doctor.rs.pending-snap を追跡から外し，.gitignore に足す ([#793](https://github.com/emela-lang/emela/pull/793)) (by @Allianaab2m)

## [0.28.2](https://github.com/emela-lang/emela/compare/v0.28.1...v0.28.2) - 2026-11-22

### Fixed

- *(core)* suspend handler の member を持つ effect を，型検査と同じく Immediate でないとする ([#713](https://github.com/emela-lang/emela/pull/713)) (by @somnicattus)
- *(types)* with の項目で引数のある handler member の括弧を省いたら E0434 にする ([#706](https://github.com/emela-lang/emela/pull/706)) (by @somnicattus)
- *(core)* effect のキーに capability の型の effect を入れ，State[use A] と State[use B] を別の effect にする ([#708](https://github.com/emela-lang/emela/pull/708)) (by @somnicattus)

### Other

- *(spec)* Instant.to_string の形式を一意に書き，RFC 3339 との関係を書く ([#729](https://github.com/emela-lang/emela/pull/729)) (by @somnicattus)
- *(lir)* 空の evidence の表を WASM と C で null にし，確保しない ([#721](https://github.com/emela-lang/emela/pull/721)) (by @Allianaab2m)
- Rust のテストを速くする（コーパスの分割と Bun の shard 化） ([#716](https://github.com/emela-lang/emela/pull/716)) (by @Allianaab2m)

## [0.28.1](https://github.com/emela-lang/emela/compare/v0.28.0...v0.28.1) - 2026-11-19

### Fixed

- *(cli)* emela check でも [build].target の知らないターゲットを E0232 にし，Pome.toml をプロジェクトからの相対パスで示す ([#698](https://github.com/emela-lang/emela/pull/698)) (by @Allianaab2m)
- *(types)* 同じ推論グループの参照で相手の推論した E と R に引数を当てはめ，Set.filter に解けていない変数が出ないようにする ([#697](https://github.com/emela-lang/emela/pull/697)) (by @Allianaab2m)
- *(core)* 同じ with の L by P で，P が使う基の effect を同じ with の先行ハンドラから渡す ([#695](https://github.com/emela-lang/emela/pull/695)) (by @Allianaab2m)
- *(driver)* 構文エラーがあるとき emela run と build を lowering の前で止め，コードの無いエラーを出さないようにする ([#696](https://github.com/emela-lang/emela/pull/696)) (by @Allianaab2m)
- *(playground)* emela-playground から言語サーバーを 1 通ずつ動かせるようにし，ブラウザのエディタで補完やホバーを使えるようにする ([#681](https://github.com/emela-lang/emela/pull/681)) (by @Allianaab2m)

## [0.28.0](https://github.com/emela-lang/emela/compare/v0.27.1...v0.28.0) - 2026-11-18

### Added

- ProcessHost と PageHost で host を型で区別し，with Process で process 用の handler を使う ([#668](https://github.com/emela-lang/emela/pull/668)) (by @Allianaab2m)
- capability の型 use E を書き，capability を値として渡せるようにする ([#665](https://github.com/emela-lang/emela/pull/665)) (by @Allianaab2m)
- with や Fiber.scope から capability を持ち出す値に警告を出す ([#648](https://github.com/emela-lang/emela/pull/648)) (by @Allianaab2m)
- Pome.toml の [experimental] と --experimental-<name> で実験的な機能を有効にできるようにする ([#644](https://github.com/emela-lang/emela/pull/644)) (by @Allianaab2m)
- WASM でも閉じた stdout に書くとプログラムを 141 で終える ([#588](https://github.com/emela-lang/emela/pull/588)) (by @Allianaab2m)
- ブラウザで動くコンパイラ emela-playground を足し，リリースに wasm を載せる ([#615](https://github.com/emela-lang/emela/pull/615)) (by @Allianaab2m)
- main が ExitCode を返して終了コードを選べるようにする ([#591](https://github.com/emela-lang/emela/pull/591)) (by @Allianaab2m)
- Std.Process の Stdout と Stderr で改行なしの出力を書けるようにする ([#587](https://github.com/emela-lang/emela/pull/587)) (by @Allianaab2m)
- --target native でネイティブ実行ファイルをビルドできる ([#573](https://github.com/emela-lang/emela/pull/573)) (by @Allianaab2m)
- Std.Process でプロセスの引数と標準入力を読めるようにする ([#575](https://github.com/emela-lang/emela/pull/575)) (by @Allianaab2m)
- Bun の最小バージョンを 1.4.3 にする ([#562](https://github.com/emela-lang/emela/pull/562)) (by @Allianaab2m)
- EMELA_JS_HOST で JS host を選べるようにし、Bun をサポートする ([#559](https://github.com/emela-lang/emela/pull/559)) (by @Allianaab2m)
- emela doctor で各ターゲットに必要なプログラムを確かめられるようにする ([#556](https://github.com/emela-lang/emela/pull/556)) (by @Allianaab2m)
- [**breaking**] emelinoへの分離に伴い組み込みCバックエンドを削除 ([#538](https://github.com/emela-lang/emela/pull/538)) (by @aoi-netai)
- 一つの handler で複数の効果を実装し，use A + B を少ない効果の型に渡せるようにする ([#672](https://github.com/emela-lang/emela/pull/672)) (by @Allianaab2m)
- effect に handler member を宣言し，with x = Files.Open(p) で開けるようにする ([#671](https://github.com/emela-lang/emela/pull/671)) (by @Allianaab2m)
- Encode，Decode，ObjectKey，Shaped を derive でき，Std.Dynamic，Std.Shape，Std.Json を使える ([#589](https://github.com/emela-lang/emela/pull/589)) (by @Allianaab2m)
- handler の init で Self(...) と書けるようにする ([#646](https://github.com/emela-lang/emela/pull/646)) (by @Allianaab2m)
- ブロックの中に局所関数 fn name(...) を書ける ([#574](https://github.com/emela-lang/emela/pull/574)) (by @Allianaab2m)
- Semaphore で同時に走る fiber の数を制限し，Queue で fiber の間で値を受け渡せるようにする ([#590](https://github.com/emela-lang/emela/pull/590)) (by @Allianaab2m)
- List，Array，Map，Set の each で要素ごとに f を呼べるようにする ([#581](https://github.com/emela-lang/emela/pull/581)) (by @Allianaab2m)
- derive した Encode と Decode で欠けたキーと null を Option と Presence で書き分ける ([#657](https://github.com/emela-lang/emela/pull/657)) (by @Allianaab2m)
- *(wasm)* run fibers with scopes and deferred values ([#507](https://github.com/emela-lang/emela/pull/507)) (by @Allianaab2m)
- emela を Windows（x86_64-pc-windows-msvc）向けにもビルドしてリリースする ([#578](https://github.com/emela-lang/emela/pull/578)) (by @Allianaab2m)
- JS の launcher 名（main, $start）と重なる host export を E0438 にする ([#570](https://github.com/emela-lang/emela/pull/570)) (by @Allianaab2m)
- Pome.toml の [pome].license を SPDX license expression として読む ([#542](https://github.com/emela-lang/emela/pull/542)) (by @Allianaab2m)

### Fixed

- *(types)* @entry のアダプタが予約名を 2 回 export すると E0438 が二重に出ていたのを直す ([#551](https://github.com/emela-lang/emela/pull/551)) (by @Allianaab2m)
- *(core)* 型引数に関数を持つ値を書かれた Show で補間すると lowering が拒否していたのを直す ([#549](https://github.com/emela-lang/emela/pull/549)) (by @Allianaab2m)
- *(c)* print the shortest digits for powers of two ([#608](https://github.com/emela-lang/emela/pull/608)) (by @isoshigi)
- *(codegen-c)* テストの共有オブジェクトとサニタイザの確認をプロセスごとのディレクトリで作る ([#552](https://github.com/emela-lang/emela/pull/552)) (by @Allianaab2m)
- *(test)* LSP テストの shutdown が解析の遅れで落ちないようにする ([#577](https://github.com/emela-lang/emela/pull/577)) (by @Allianaab2m)

### Other

- *(syntax)* cargo-mutants で見逃していた変更を検出できるテストを足す ([#545](https://github.com/emela-lang/emela/pull/545)) (by @Allianaab2m)
- *(resolve)* cargo-mutants で見逃していた変更を検出できるテストを足す ([#546](https://github.com/emela-lang/emela/pull/546)) (by @Allianaab2m)
- *(lir)* associate string interpolation appends to the right ([#606](https://github.com/emela-lang/emela/pull/606)) (by @isoshigi)
- *(wasm)* allocate from size-class bins in the Perceus runtime ([#605](https://github.com/emela-lang/emela/pull/605)) (by @isoshigi)
- *(wasm)* reuse a static job ring for the scheduler queue ([#604](https://github.com/emela-lang/emela/pull/604)) (by @isoshigi)
- *(wasm)* name a perform's operation only when a diagnostic needs it ([#603](https://github.com/emela-lang/emela/pull/603)) (by @isoshigi)
- *(wasm)* dispatch a resumed frame with a br_table over its state ([#602](https://github.com/emela-lang/emela/pull/602)) (by @isoshigi)
- *(wasm)* remove the suspend runtime's dead queue statics ([#601](https://github.com/emela-lang/emela/pull/601)) (by @isoshigi)
- *(wasm)* build the float decimal expansion in larger chunks ([#600](https://github.com/emela-lang/emela/pull/600)) (by @isoshigi)
- *(wasm)* inline `dup` and `tag` in the emitter ([#599](https://github.com/emela-lang/emela/pull/599)) (by @isoshigi)
- *(wasm)* dispatch a tag switch with br_table ([#598](https://github.com/emela-lang/emela/pull/598)) (by @isoshigi)
- *(wasm)* emit only the Float, Bytes, Array and Unicode runtime bodies the program reaches ([#596](https://github.com/emela-lang/emela/pull/596)) (by @isoshigi)
- *(wasm)* scan for a needle's first byte a word at a time ([#595](https://github.com/emela-lang/emela/pull/595)) (by @isoshigi)
- *(wasm)* copy string ranges with memory.copy ([#594](https://github.com/emela-lang/emela/pull/594)) (by @isoshigi)
- *(wasm)* index the environment once instead of scanning it per lookup ([#593](https://github.com/emela-lang/emela/pull/593)) (by @isoshigi)
- *(wasm)* sort the listed names with an in-place heapsort ([#592](https://github.com/emela-lang/emela/pull/592)) (by @isoshigi)

## [0.27.1](https://github.com/emela-lang/emela/compare/v0.27.0...v0.27.1) - 2026-11-12

### Fixed

- *(driver)* JS ターゲットの Node の下限を 22 に上げ，古い node を止める ([#536](https://github.com/emela-lang/emela/pull/536)) (by @Allianaab2m)
- *(cli)* --version にコミットハッシュを付ける ([#534](https://github.com/emela-lang/emela/pull/534)) (by @Allianaab2m)
- reserve the JS launcher names for host exports ([#505](https://github.com/emela-lang/emela/pull/505)) (by @Allianaab2m)
- *(lsp)* 属性名とターゲット名を補完し，コメントと文字列の中では補完しない ([#529](https://github.com/emela-lang/emela/pull/529)) (by @Allianaab2m)
- *(lsp)* external のファイル引数をリンクにする ([#532](https://github.com/emela-lang/emela/pull/532)) (by @Allianaab2m)
- *(lsp)* 節と型の位置では書ける種類の名前だけを補完する ([#530](https://github.com/emela-lang/emela/pull/530)) (by @Allianaab2m)
- *(lsp)* ディスク上のファイル変更で再検査する ([#531](https://github.com/emela-lang/emela/pull/531)) (by @Allianaab2m)

## [0.27.0](https://github.com/emela-lang/emela/compare/v0.26.1...v0.27.0) - 2026-11-07

### Added

- 効果の節を uses { R } と書けるようにする ([#518](https://github.com/emela-lang/emela/pull/518)) (by @Allianaab2m)
- 無名関数を |x| e と書ける ([#516](https://github.com/emela-lang/emela/pull/516)) (by @Allianaab2m)
- escape の arm に `_` を書いて、どの error も受けられるようにする ([#512](https://github.com/emela-lang/emela/pull/512)) (by @Allianaab2m)
- Std.Array で位置で読む固定長の列を書けるようにする ([#513](https://github.com/emela-lang/emela/pull/513)) (by @Allianaab2m)

### Fixed

- *(wasm)* trap when the heap cannot grow ([#506](https://github.com/emela-lang/emela/pull/506)) (by @Allianaab2m)
- *(wasm)* show the name of an error without Show ([#443](https://github.com/emela-lang/emela/pull/443)) (by @isoshigi)

### Other

- Move attempt and Exit to Std.Exit ([#501](https://github.com/emela-lang/emela/pull/501)) (by @Allianaab2m)
- *(std)* 同梱の core と std を x.f(a) のメソッド呼び出しで書き直す ([#477](https://github.com/emela-lang/emela/pull/477)) (by @Allianaab2m)
- update Cargo.toml dependencies

### Breaking

- `attempt` と `Exit`（`Success`，`Failure`）を `Std.Fiber` から新しい `Std.Exit` に移す．`Fiber.attempt(f)` は `Exit.attempt(f)` に，`import Std.Fiber.{Exit, Success, Failure}` は `import Std.Exit.{Exit, Success, Failure}` に書き換える．古い名前は残さない（`proposals/std-exit.md`）

## [0.26.1](https://github.com/emela-lang/emela/compare/v0.26.0...v0.26.1) - 2026-10-30

### Fixed

- *(types)* order a method call only before fns of the receiver type's module ([#480](https://github.com/emela-lang/emela/pull/480)) (by @Allianaab2m)

## [0.26.0](https://github.com/emela-lang/emela/compare/v0.25.0...v0.26.0) - 2026-10-29

### Added

- CLIから組み込みCのビルドと書き込みを利用可能にする ([#387](https://github.com/emela-lang/emela/pull/387)) (by @aoi-netai)
- Std.Bytes でバイト列を扱えるようにする ([#466](https://github.com/emela-lang/emela/pull/466)) (by @Allianaab2m)
- x.f(a) でメソッド呼び出しを書ける ([#467](https://github.com/emela-lang/emela/pull/467)) (by @Allianaab2m)
- STM32F4・ESP32-C3のボード実行基盤を追加 ([#386](https://github.com/emela-lang/emela/pull/386)) (by @aoi-netai)

### Fixed

- *(wasm)* import only the std hosts reached from the entry ([#442](https://github.com/emela-lang/emela/pull/442)) (by @isoshigi)

### Other

- 多数のプログラムを回すテストを並列にする ([#456](https://github.com/emela-lang/emela/pull/456)) (by @Allianaab2m)
- update Cargo.toml dependencies
- README と docs を 0.25 に合わせ，trait の例を足す ([#464](https://github.com/emela-lang/emela/pull/464)) (by @Allianaab2m)

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
