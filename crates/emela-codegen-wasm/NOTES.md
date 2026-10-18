# emela-codegen-wasm の補足

WASM のターゲット（仕様 16 章，Tier 1）．Heap IR（`Stage::RefCounted`）を読んで WebAssembly を
書き出す．ランタイムも同じモジュールに `wasm-encoder` で出力するので，外部のツールチェーンは
要らず，実行は `wasmtime`（`EMELA_WASMTIME` か `wasmtime`）に任せる．設計にない判断を「（補）」
として残す．

## パイプライン

- （補）`compile` が Core（lower 済み）から通しでかける．エントリ（main）を根に
  `emela_core::monomorphize_from` → `emela_lir::from_core` → 対応していない構成の検査
  （`check.rs`）→ `emela_lir::to_heap` → 出力．C のバックエンドと同じ流れ
- （補）検査は Flow IR の段でかける．いま診断にして止めるのは，**書記素クラスタ**の表が要る
  `String` の操作（`length` / `chars` / 空区切りの `split`）と，**コールバックを取る外部関数**
  （E0427，JS 専用）だけ．suspend するコード，state machine，fiber，`Float`，`const` と note，
  `Std` の platform（Console / Clock / Env / Random / FileSystem）はすべて動く（Wave 3 時点）
- （補）エントリは `_start` を export する WASI のコマンドにする．`fd_write`（`dbg` とテストの
  出力）と `proc_exit` は c の `cc` と同じく外部に任せる（`wasmtime` が WASI を供給する）．
  この 2 つは関数番号を固定するため，関数の番号を振る前に先に import する

## 値の表現とデータ配置

- （補）Int と Bool は `i32`，Int64 は `i64`，Float は `f64`，それ以外の参照は線形メモリの
  `i32` のポインタ．IR の変数はすべて wasm のローカル
- （補）オブジェクトは 16 バイトのヘッダの後に 8 バイトの枠．C と同じ配置にそろえた

  ```
  offset 0  rc    参照カウント．0 は静的なデータの印（数えない）．新しい割り当ては 1 から
  offset 4  aux   解放リストの次のポインタ（生きている間は未使用）
  offset 8  desc  記述子へのポインタ
  offset 16 slots[]
  ```

- （補）記述子（32 バイト）: kind（0 = 枠，1 = 文字列），枠数，タグ，error の enum，参照枠の
  ビットマップ，表示の関数，名前．`dup` / `drop` はこれを見て，参照枠だけをたどって解放する
- （補）文字列はヘッダの後に長さ（i32）と UTF-8 のバイト列．文字列リテラル，引数のない
  構成子，捕獲のないクロージャは `rc == 0` の静的なデータにして，割り当ても RC も省く
- （補）クロージャのレコードは `{関数の添字, 捕獲…}`．関数の添字はテーブルの添字で，
  テーブルは IR の関数ごとに 1 つ（`call_indirect` はこの添字で引く）
- （補）ハンドラの値は `{使用済みの印, 解放後の defect の文言, 操作のクロージャ…}`．操作の
  呼び出しは印を調べ，立っていれば文言の defect にする（8.6）
- （補）証拠の表は `{外側の表, 個数, (エフェクト, ハンドラ)…}`．`ev_lookup` は後ろから探し，
  見つからなければ外側の表へ進む（後の束縛が勝つ）

### 線形メモリの配置

    [ スクラッチ ][ 静的データ ][ ヒープ → grow ]
    0            SCRATCH_END   heap_base

- （補）スクラッチ（WASI の iovec，整数の桁，JSON escape，1 コードポイントの UTF-8）は線形メモリの
  先頭に固定で置く．オフセットは wasm に定数として焼き込む
- （補）静的データは SCRATCH_END から，ヒープはその上（8 バイト整列）から始まる
- （補）定義は `src/emit/layout.rs` の 1 か所だけ．不変条件（重ならない・整列）はコンパイル時
  assert と `tests/layout.rs` が保証する．スロットを足すときは `scratch!` に足すだけでよい

## 制御

- （補）値を返す `if`，`switch`，`block`，`loop` は，結果の変数を IR が `dst` に持つので，
  そのローカルへの代入にする（wasm の結果型を使わない．C と同じ形で読みやすい）
- （補）`yield` は囲む文の `dst` への代入（`if` / `block` / `loop` の本体は最後なので fall
  through）．`break` は行き先の `block` の `dst` に代入して `br`，`recur` はループの変数を
  一時ローカル経由で並行に代入してループの頭へ `br`．`br` の深さは wasm のラベルのスタックから
  数える
- （補）`switch` は比較の `if` / `else` の連なりにする（タグは `tag` を引いて比べる）．分岐の
  数だけ入れ子になるが，`br_table` を使うより素直で，ラベルの深さの計算も同じ
- （補）defect と失敗は戻り値の形を変えず，保留の枠（グローバル `em_defect`，`em_failure`）で
  返す．`on_defect` / `on_fail` の着地点は呼び出しの後に枠を見る分岐．`unwind` と `fail` は
  ダミーの値を返す
- （補）`_start` はエントリの `main` を呼び，終わりに保留の defect（101）と失敗（1）を調べて
  `proc_exit` する

## ランタイム

- （補）アロケータは線形メモリの bump と，サイズを `rc` の欄に置いた単一の解放リスト（first
  fit）．解放は再帰せず，解放するオブジェクトを `aux` でつないだスタックで回す（深いリストや
  深い enum でスタックがあふれない）
- （補）`dup` は `rc != 0` なら足すだけ．`drop` は `rc != 0` で減らして 0 なら解放．単一
  スレッドなので原子的な操作はしない
- （補）文字列の連結，比較（UTF-8 のバイト順はコードポイントの順と同じ），バイト数，
  `show_string`（`\"`、`\\`、`\n`、`\t`、`\r` と `#{` の前の `\#`、他の制御文字は `\u{...}`
  と escape する．10.6），`Int.to_string`
  （スクラッチのメモリに桁を作ってから文字列にする），`Int64.to_string` を wasm で書く
- （補）`String.contains` / `starts_with` / `ends_with` は部分文字列のバイト探索で行う
  （UTF-8 は自己同期なので，バイトの一致はコードポイントの列の一致と同じ．16.2）．`split`
  は空でない区切りで分け，`List[String]` を組む（前から順に `cons` でつなぐ）．`join` は要素の
  長さを一度数えてから連結する．`code_points` は UTF-8 を復号して `List[Int]` にする．`trim`
  は前後を UTF-8 のコードポイントに復号しながら White_Space（25文字）を取り除く．空の区切りの
  `split` はコンパイル時に止めるが，実行時に空になった区切りは defect にする（無限ループを避ける）
- （補）`debug_print` は stderr に文字列と改行を `fd_write` で書く．iovec と書いたバイト数は
  ヒープの下のスクラッチに置く
- （補）失敗（error の値）の表示は `show_failure` が記述子の `show`（その enum の `show`
  関数のテーブルの添字）を `call_indirect` で呼ぶ．`show` が無ければ null を返すので，行から
  `message` を省く．error の enum の記述子には `m.error_shows` の関数の添字を入れる
- （補）テスト（13.4）は `_start` が各テストを順に呼び，`test_result` が JS と同じ行
  （`\x1eemela-test` と JSON）を標準出力に書く．文字列の JSON 化（`json_string`）は `"` と
  `\` と `\n` を escape し，他の制御文字は `\u00XX` にする．結果は保留の defect / failure を
  見て `ok` / `defect` / `error` に分け，defect はその文字列を，error は `show_failure` の
  文字列を `message` にする
- （補）`finish` はプログラムの終わりに残った defect / failure を見て，`defect: …`（101）や
  `error: …`（1）を stderr に書いて `proc_exit` する（C の `em_exit` と同じ，14.3，14.4）
- （補）`Int.parse`（本体は `parse_or_none`），`Int.checked_add`，`String.from_code_point` の
  ように `Option` を返す組み込みは，呼び出し側（emitter）が Prelude の `Option` の `Some` の
  記述子と静的な `None` のオブジェクトを作ってランタイムに渡す．ランタイムは成功なら `Some` を
  組んで返し，失敗なら `None` を返す（`Int.parse` の Prelude 側の包みが match で開く）
- （補）整数の `/` と `%` の `INT_MIN / -1` の巻き戻し，シフト量の負と幅以上の扱い（16.1，
  6.6 の NOTES）をランタイムの関数にする．シフト量が負なら `negative shift amount: <n>` の
  defect にする
- （補）const（5.6）は最初の参照で 1 回だけ評価して値か defect を記憶する（`emit::const_*` の
  静的セルと `runtime/constant.rs`）．評価中の defect には const 名の接頭辞を付け，自己参照と
  中断は記憶しない．ハンドラを作るモジュールだけが release の失敗の note（14.4）を持ち，
  `retire` が最後に退役させた名前をグローバルに残す．`raise` は常に引数を `dup` して保留の
  defect に自分の参照を持たせる（呼び出し側は `runtime defect` の後で自分の参照を落とすので，
  C の `em_raise` と同じ契約）．const / note のグローバルは `uses_const` / `uses_notes` のとき
  だけ `Builder::globals` で動的に確保して出力する

### 内部ホスト（ホスト境界の固定 ABI）

- （補）`@external(wasm, "module", "name")` は通常は wasm の import になるが，`(module, name)`
  が `emit::external::INTERNAL_HOSTS` にあるときは同じモジュールのランタイム関数に束縛する
  （import は足さない）．その関数の型は LIR の representation を写したもので，`String` やリスト
  などの参照は `i32`（線形メモリのポインタ）になり，`()` は無しになる．`repr_val` を通すので
  参照を拒む `host_val` は使わない．登録済みのモジュールは次のとおり:

  | module | name | 用途 |
  |---|---|---|
  | `test` | `print` | `print_string`（`(i32) -> ()`，標準出力に文字列と改行．コーパス用） |
  | `emela:console` | `log` / `error` | `console_log` / `console_error`（`Std.Console`，fd 1 / 2） |
  | `emela:env` | `has` / `get` | `env_has` / `env_get`（`Std.Env`，`environ_*`） |
  | `emela:random` | `int` / `float` | `random_int` / `random_float`（`Std.Random`，`random_get`） |
  | `emela:fs` | `read_text` / `write_text` / `append_text` / `exists` / `is_directory` / `list_directory` / `create_directory` / `remove` / `outcome_*` | `Std.FileSystem`（WASI `path_*`/`fd_*`，§13.10） |
  | `emela:clock` | `now_milliseconds` / `sleep_milliseconds` / `when_stalled` / `sleep` | `Std.Clock`（REALTIME の `clock_time_get` とドライバのタイマ，§13.13） |

  `emela:test`（`sleep_ms` / `sleep_after`）は WASM のテスト専用（`tests/suspend_external.rs`）．
  `Std.FileSystem` の `Outcome` は opaque な handle（`emit::handle`）で，`outcome_*` がそれを通す．
- （補）**`suspend fn` の外部関数は 2 形式**ある（`check_internal_host` が両方を受け付ける，
  12.6）．1 つは**直接形（in-place）**で，ホストは宣言どおりの引数と戻り値の型を持ち，その場で
  値を返す（`Std.FileSystem` の `host_*`）．もう 1 つは**task 形**で，ホストは最後に task（`i32`）
  を取り何も返さず，`task_complete_{int,int64,float,unit,string,defect}` のいずれかで完了する
  （`Std.Clock` の `sleep` / `when_stalled`）．呼び出し側（`emit::fiber::suspend_external_call`）は
  `task_new` で task を作って引数と一緒に渡し，`task_end` が値か中断を受け取る．中断したら
  ドライバが fiber を再開する（下の suspend ABI）
- （補）内部ホストが指すランタイム関数は要求されたときだけ登録・出力する
  （`runtime::is_on_demand` が `INTERNAL_HOSTS` を唯一の一覧にする）．内部ホストを使わない
  プログラムのバイト列は，関数を足しても変わらない．`runtime::register` はさらに，sched /
  fiber を `uses_suspend` のときだけ，タイマ関数（`INTERNAL_HOSTS` の `TIMER_HOSTS`）を
  `uses_timers` のときだけ，`fs` を要求されたときだけ，`const` と note を `uses_const` /
  `uses_notes` のときだけ登録する．ランタイムの添字は IR 関数の後で確定するので，
  `external_import` で見た束縛は `bind_internal_hosts` が `runtime::register` の後に埋める
- （補）host 用の使い捨て確保や WASI へ渡すバッファは `alloc`（16 バイトのヘッダ付き）で取り，
  WASI には `base + HDR` を渡す（`Std.Env` がそうしている）．スクラッチ（`emit/layout.rs`）の
  スロットは足さず，`SCRATCH_END == 168` を `tests/layout.rs` が固定している

### suspend の ABI（実装済み）

- （補）**状態機械**（`emit::suspend`，§13.6）: suspending な関数 `f` は，wrapper `f`，本体
  `f_sm(params, frame)`，再開 `f_resume(frame) -> i64` の 3 つになる．本体は `Stmt::Let` の
  `on_suspend` ごとに state ガード（skip / enter / record）を持ち，中断したらフレームに生存変数を
  詰めて `G_SUSPENDING` を立てて返す．`f_resume` は resume 入力（`G_RESUME` の生ビット）を
  repr に応じて読み替える（`resume_i32` / `resume_i64` / `resume_f64` / `resume_ref` /
  `resume_void` と `unbox`）．速い経路（中断しない）ではフレームも余分な制御も作らず，
  suspend を使わないプログラムの bytes は `Builder::uses_suspend` で不変
- （補）**frame**: slot0 = 再開関数のテーブル添字，slot1 = state，slot2 = next，slot3.. = 保存変数．
  `frame_new` / `frame_push` / `frame_free`（不変条件はオブジェクトのヘッダに合わせてある）．
  グローバルは **6..21**（`emit/mod.rs`）: `G_RESUME`=6（i64 の resume 入力），`G_SUSP_HEAD`=7 /
  `G_SUSP_TAIL`=8（捕獲した継続の両端），`G_SUSP_WAIT`=9（待ちのポインタ），`G_SYNC_DEPTH`=10，
  `G_INTERRUPTED`=11，`G_SYNC_DEFECT`=12，`G_ROOT`=13，`G_CUR`=14，`G_CUR_MAIN`=15，
  `G_RUN_ID`=16，`G_NOTE_ERRORS`=17 / `G_NOTE_LINES`=18，`G_TIMERS`=19，`G_TIMER_SEQ`=20．
  `SUSPEND_GLOBALS == 21`．失敗・defect は既存の `G_FAILURE` / `G_DEFECT` を使う
- （補）**スケジューラ**（`runtime/sched.rs`，C の `em_schedule`/`em_step`/`em_frame_*` の移植）:
  root / wait グラフ / driver ループ / scope / Deferred / Cause / note を持つ．`hold` / `unhold` /
  `yield` は `RuntimeOp::{Hold,Unhold,Yield,YieldPerform}` と `Term::Yield` に配線し，
  `sync_enter` / `sync_leave` が「誰も再開しない継続」を interruption に巻き戻す（8.7）．
  `perform` は `resume_new` / `perform_end`，中断は `interrupted_new` / `is_interrupted`．
  `_start` は root fiber を回し，driver が空になったら `interrupted` を `finish` して exit 130
- （補）**ドライバのタイマ**（`runtime/clock.rs`，§13.13）: `now_ms` はドライバ自身の
  MONOTONIC クロック，`after(ms, task)` がタイマを登録し，`timers_due` が `G_TIMERS` /
  `G_TIMER_SEQ` を見て期限の来た task を完了する．`clock_time_get` は `TIMER_HOSTS` に載る
  タイマ関数が要求されたとき，**index 確定前に** import する（後から足すと全関数 index がずれる）

## 対応していないもの

- **書記素クラスタ**の表が要る `String` の操作: `length`，`chars`，空の区切りの `split`
  （`check.rs` が `Value::Builtin` で止める．JS は `Intl.Segmenter`，C と WASM は共有の表を
  持たない．Wave 4）
- **`Std.Http`**: `SystemHttp` に `@external(wasm, ...)` が無く E0412 で止まる．WASI preview1 に
  socket も無い（Wave 4）．WASM の突き合わせテストもこの example を除く
  （`crates/emela-driver/tests/wasm_parity.rs` の `NOT_ON_WASM`，
  `crates/emela-driver/tests/wasm_examples.rs` の `NOT_ON_WASM`，`check.rs` は関与しない）
- **コールバックを取る外部関数**（E0427）: JS 専用（12.1）．`check.rs::external` が止める
- 外部関数と WASM の import の境界を越えられるのは `Int` / `Bool` / `Int64` / `Float` だけ．
  `String` や `@external type` を直接 import する外部関数は越えられず `host_val` が診断で
  止める．`String` などを扱うホストは内部ホスト（上の ABI）として参照を `i32` のポインタで
  渡す（ホストが線形メモリを読む）

## テスト

- `tests/smoke.rs` はモジュールが `wasmparser` で妥当なことを確かめ，`wasmtime` があれば
  `dbg` の出力まで見る
- `tests/run.rs` は整数，enum と match，リストの再帰，クロージャの捕獲，文字列，エフェクトと
  ハンドラ，失敗と escape を `wasmtime` で動かして出力を比べる．`wasmtime` がなければ飛ばす
- `tests/programs.rs` は C の受け入れコーパス（`crates/emela-codegen-c/tests/programs`）を
  WASM にして `wasmtime` で走らせ，終了コード・標準出力・標準エラーを `.expected` と比べる．
  コーパスはすべて WASM で一致するので `NOT_ON_WASM` は空（`not_on_wasm_entries_exist` が
  陳腐化した項目を防ぐ）．コーパスの `print` には `@external(wasm, "test", "print")` を，
  `suspend fn sleep` には `@external(wasm, "emela:clock", "sleep")` を注入して内部ホストに束縛する
- `tests/{console,env,random,clock,file_system,consts,suspend_external}.rs` は内部ホストと
  const / note / suspend 外部を `wasmtime` で実走して確かめる（`wasmtime` がなければ飛ばす）
- `crates/emela-driver/tests/wasm_parity.rs` は純粋なプログラム（`dbg` だけ）を JS と WASM の
  両方で動かし，終了コードと出力が一致することを見る．まだ扱えないもの（`String.length` /
  `String.chars` の書記素）は `NOT_ON_WASM` に理由つきで並べ，診断のメッセージで見る（Wave 4）
- `crates/emela-driver/tests/wasm_examples.rs` は `examples/*`（`http` を除く）を JS と WASM の
  両方で実走し，終了コード・標準出力・標準エラーを比べる（`@test` の結果も比べる）．CI の
  test は `wasmtime` を入れるので実行テストは飛ばない
