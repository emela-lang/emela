# 組み込み C のバックエンド

ヒープランタイムや POSIX API を使わず，Heap IR からフリースタンディングの C11 を出力する．説明は [`docs/design/c-embedded-codegen.md`](../../docs/design/c-embedded-codegen.md)，設計は [`docs/design/c-embedded-architecture.md`](../../docs/design/c-embedded-architecture.md)．

## モジュール

- `lib.rs`: パイプライン（単相化 → `from_core_with`（値型，defect で止める）→ クロージャー変換 → 静的特殊化（`specialize_statics`）→ defect の経路 → 状態機械 → RC → 検証 → 検査 → 出力）．値型のレイアウトは型を表に登録するので，出力の前に `prepare` で求めておく
- `check.rs`: 入口から届く関数の検査．ヒープの値（`heap_bytes` がなければ），特殊化で消えなかった関数値，fiber，未対応の組み込み関数，外部関数の型と名前を検査する．割り込み入口から届く関数では，ヒープの値，待つ外部関数，再帰も拒否する
- `emit.rs`: Heap IR を C に写す．値型の構造体の形（newtype，レコード，タグ付き）と境界ヘッダーの生成もここにある
- `board.rs`: `Std.Embedded.*` の宣言から境界の C の名前を決める
- `names.rs`，`writer.rs`: `emela-codegen-c` の写し

## 補足

- （補）同じヒープの型は最初に出会った関数でだけ報告する（呼び先で同じ型を受け取る関数が続けて報告されるため）．関数値は作った所で報告し，呼ぶ所では報告しない
- （補）defect の種類は Flow IR の文言（`division by zero` など）から決める．`panic` の文言は利用者のもので，それ以外は静的な文言を渡す
- （補）外部関数の C の名前は `@external(embedded, ...)` の名前そのもの（生成する関数の `eN_` の名前を使わない）．プロトタイプは `emela_embedded.h` に出す．プロジェクトのファイルを指す形は `Output::copies` で返し，driver が読んで出力先に写す（ファイルの読み書きは driver に置く）．同梱のファイルは中身ごと `Output::files` に入れる
- （補）静的特殊化の後，何も持たないタプル（捕獲のないクロージャー，ボードの effect だけの証拠の表）は C の値を持たない（`check::no_data`）．その変数，引数，戻り値は出さない．ボードの effect の操作は静的特殊化が作る外部関数で，プロトタイプは周辺機器のヘッダーにある
- （補）`State.run` の `Cell` は，関数の先頭で宣言するローカル変数（`vN_state`）と，そのアドレスになる
- （補）handler の操作の関数はすべての引数を使うとは限らないので，使わない引数は `(void)v;` で受ける（`-Wextra -Werror` で通すため）
- （補）const はコンパイル時に評価できれば `static const T eN_NAME_value = …;` を置き，関数はそれを返すだけにする．評価した const の本体からしか届かない関数は出さない（`check::reach_except`）．評価の手数の上限（`CONST_STEPS`）を超えれば診断にする
- （補）静的な State: driver は `@static` の const の型の `State[S]` を `main` に与える（型検査の `AutoEffect`．const の型は推論の後に分かるので，`@static` があるときだけ型検査を2回する）．Core の下ろしは `@static` の const を根に含める．出力器は `@static` の const を静的特殊化の根にも足し，その値（コンパイル時に評価できなければ診断）を初期値に `static T em_state_N` を置く．`get` / `set` / `update` は静的特殊化が作る関数で，`emela_board_critical_enter` / `exit` の間で `em_state_N_load` / `store` を呼ぶ
- （補）静的特殊化が作る外部関数（ボードの操作，フック，状態の読み書き．名前が `$` で始まるもの）は，`emela_embedded.h` にプロトタイプを出さない（周辺機器のヘッダー，`emela_board.h`，`main.c` の中にある）
- （補）String: lir の値型のモジュールでは String も `agg`（`Layout::String`）．C では，引数（関数の引数の変数）が `EmStr`（ビュー），そのほかの変数とフィールドが `EmString`（領域）．どの代入も `em_str_set`（リテラルはポインターのまま，それ以外は長さ分のコピー）で，構造体を丸ごとコピーするのは集成の中の String だけ．String を返す関数は最後の引数 `EmString *em_ret` に書く．String のランタイム（`strings.rs`）は String を使うプログラムにだけ入れ，`Float.to_string` は使うときだけ入れる（浮動小数点の `snprintf` が要るため）
- （補）String の変数は初期化しない（容量分を 0 で埋めると遅い）．どの変数も読む前に必ず書かれる
- （補）失敗: `EmError` はプログラムの error の enum（入口から届く関数に現れるもの）の共用体で，タグは error の enum の番号の順に 1 から（0 は失敗なし）．失敗しうる関数は `EmError *em_fail` を最後の引数に取り，`fail` は `*em_fail` に書いて何かの値を返す．呼ぶ側は関数ごとに1つの `em_f` を使い，呼ぶ前にタグを 0 にして，呼んだ後に着地点へ進むか決める．`main` と公開関数の包みは，届いた失敗を `EMELA_DEFECT_UNHANDLED_ERROR` にする
- （補）platform: std の Clock と Random の外部関数は `@external(embedded, "emela_board_…")` でボードのフックを直接指す（プロトタイプは `emela_board.h`）．Console の外部関数は `emela_console_line` / `emela_console_error_line` を指し，出力器が `main.c` に定義する（断片のフックと改行）．Console の特別扱いは外部関数の定義（`Std.Console`）で判別する
- （補）Console の逐次書き出し: Console の外部関数と，String の引数をそれに渡すだけの関数（`SystemConsole` の操作）を「行の行き先」とし，そこへ渡す String のうち，1回だけ使う `String.append` と `Int.to_string` の結果を作らずに（宣言もせず），呼び出しの所で断片を書き出す（`emit::streamed`）
- （補）ヒープ: `heap.rs`．値型にならない `ref` の値（`List`，enum，タプル，`Cell`）は型ごとの箱 `EmB<型>`（先頭に `uint32_t rc`，0 は静的）．`dup` / `drop` は `em_dup` と型ごとの `em_drop_EmB…`．リストの drop は残りをループでたどり，ほかの箱はフィールドを呼び出しで drop する．アロケーターは `heap_bytes` の静的な領域と，8 バイト単位の大きさごとの空きリスト（大きさの上限は箱の共用体 `EmAnyBox` の大きさ）．`Cell` の中身がヒープの値なら，`new` と `set` で参照を1つ持ち，`get` は参照を1つ渡す（ホスト向け C のランタイムと同じ）
- （補）何も使わない変数（パターンで束縛して使わないものなど）は宣言も代入もしない．定義する呼び出しは残す
- （補）周辺機器の操作: `fails` の操作は C のエラーコードを返し，出力器が `PeripheralError(code)` の失敗に直す（静的特殊化が作る操作の外部関数も `fails` を持つ）．`List[Int]` は呼び出しの所で `uint16_t` の配列（`EMELA_TRANSFER_CAPACITY` 要素）に写し，結果の配列からリストを作る．境界ヘッダーの引数の名前は宣言の名前（Core の `OpDef::param_names`）
- （補）`@external type` は lir の `Layout::Opaque` で，C では `void *` をそのまま値として持つ
- （補）`EMELA_EMBEDDED_DUMP` にディレクトリーを指定すると，Flow IR と Heap IR の dump（`flow.lir`，`heap.lir`）を書き出す（開発用）

## 未実装

- STM32CubeF4 以外の HAL のアダプター．MCU ごとの代替機能とはんだブリッジの検査
- 失敗の集合ごとの共用体（いまはプログラム全体の error の enum の共用体 `EmError` を1つ使う）
