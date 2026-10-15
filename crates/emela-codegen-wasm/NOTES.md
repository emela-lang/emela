# emela-codegen-wasm の補足

WASM のターゲット（仕様 16 章，Tier 1）．Heap IR（`Stage::RefCounted`）を読んで WebAssembly を
書き出す．ランタイムも同じモジュールに `wasm-encoder` で出力するので，外部のツールチェーンは
要らず，実行は `wasmtime`（`EMELA_WASMTIME` か `wasmtime`）に任せる．設計にない判断を「（補）」
として残す．

## パイプライン

- （補）`compile` が Core（lower 済み）から通しでかける．エントリ（main）を根に
  `emela_core::monomorphize_from` → `emela_lir::from_core` → 対応していない構成の検査
  （`check.rs`）→ `emela_lir::to_heap` → 出力．C のバックエンドと同じ流れ
- （補）検査は Flow IR の段でかける．suspend するコード，fiber，書記素クラスタの表が要る
  `String` の操作，`Float` の操作，`Int.parse` / `Int.checked_add` は，まだ診断にして止める
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
  `show_string`（`"` と `\` だけ escape する．制御文字の escape は後で足す），`Int.to_string`
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

## 対応していないもの（最初の版）

- 状態機械（`Stmt::Let` の `on_suspend`）と suspend するコード，`runtime yield` など
- fiber（11 章の操作すべて）
- 書記素クラスタの表が要る `String` の操作（`length`，`chars`，空の区切りの `split`）
- `Float` の `to_string` / `parse` / `floor` / `ceil` / `round` / `truncate`（`to_string` は
  最短の往復表現に big integer が要るので wasm では重い）
- コールバックを取る外部関数（E0427），suspend fn の外部関数（E0428）
- notes（14.4 の 2 つ目の defect 以降の記録）はまだ持たない

## テスト

- `tests/smoke.rs` はモジュールが `wasmparser` で妥当なことを確かめ，`wasmtime` があれば
  `dbg` の出力まで見る
- `tests/run.rs` は整数，enum と match，リストの再帰，クロージャの捕獲，文字列，エフェクトと
  ハンドラ，失敗と escape を `wasmtime` で動かして出力を比べる．`wasmtime` がなければ飛ばす
