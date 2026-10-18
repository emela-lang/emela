# 組み込みCバックエンドの設計

組み込み向け C ターゲット（`--target c-embedded`）の設計の判断と、その理由を記録する。出力の形と使い方は [`c-embedded-codegen.md`](c-embedded-codegen.md)、ボード定義は [`board-definition.md`](board-definition.md) を参照。

## 前提

- 対象は趣味の電子工作の規模のマイコンである。当面の参照環境は STM32F4 系と STM32CubeF4 HAL（NUCLEO-F446RE）とする。
- 通常の実行はシングルスレッドである。ただし割り込みは通常処理を中断する。
- 割り込みの中では Emela のヒープに触れない。確保だけでなく、解放と参照カウントの操作も含める。
- 組み込み機能は `Std.Embedded.*` の effect として宣言し、ボード側が `emela_gpio_*` などの C 関数を実装する。HAL の型や関数はコード生成器に持ち込まない。
- `GpioPin(id)` は論理 ID とし、物理ピンはボード定義（TOML）に置く。論理 ID の割り当てやピンの重なりはコンパイル時に検査する。
- GPIO 割り込みは HAL コールバックから静的に Emela の関数へ結び付け、関数ポインターや実行時の登録は使わない。
- Emela の書き方は変えない。観測できる言語の意味（整数の巻き戻り、文字列など）はほかのターゲットとそろえる。そろえられないところは、下の「決定」の節に組み込みターゲットの制限として書く。

## 共通の仕組み

ホスト向け C と同じく、単相化した Core IR を `emela-lir`（Flow IR → Heap IR）に通してから C を出す。各パスの順番は [`c-embedded-codegen.md`](c-embedded-codegen.md#パイプライン) にある。

機能ごとに組み込み向けの展開を足すと、`Option`、`Result`、レコード、handler、`const` などのたびに特別扱いが増える。そこで組み込み向けに要る展開を次の5つの仕組みにまとめ、制御の変換、パターン照合の決定木、自己末尾呼び出しのループ化、導出される等価関数、整数演算の意味などは lir の既存の実装をそのまま使う。

| 仕組み | 対象になる主な機能 | 実装 |
| --- | --- | --- |
| ① 値型化 | `Option`、引数付き enum、レコード、タプル、error、String、handler の状態 | lir の `Repr::Agg`（`from_core_with` の `value_types`） |
| ② 静的特殊化 | 関数値（捕獲のあるラムダを含む）、handler、証拠の表、`State.run` | `emela_lir::specialize_statics` |
| ③ 静的評価 | `const` | `emela_lir::eval_consts` |
| ④ 静的確保 | ①〜③の後も残るヒープの値（`List`、再帰する enum） | `heap_bytes` のヒープ（「決定: メモリ管理」） |
| ⑤ 境界フック | defect、`dbg`、`main` の後の待機、外部関数、platform の handler | `emela_board.h` などの境界ヘッダー |

### ① 値型化

lir の表現（`Repr`）は `I32 / I64 / F64 / Void / Ref` だけで、引数を持つ enum とタプルは `Ref`（ヒープ上の箱と参照カウント）になる。このままでは `Option[PinState]` も `GpioEvent` もヒープに載り、ISR から使えない。

値として持つのは、再帰せず、型引数を置き換えた後のすべてのフィールドが値として持てる型（整数、真偽値、String、値型の enum・タプル・レコード、`@external type` の `void *`）である。`List`、再帰する enum、それらを含む型は参照のまま残り、④ で扱う。

**決定: lir の `Repr` に値の集成型 `Agg` を足し（案A）、組み込みのときだけ有効にする。**

- C では構造体にそのまま写る。ホスト向け C と WASM ではこれまでどおり `Ref` にするので、`Agg` はこの2つのバックエンドに届かない。WASM は構造体の値をそのまま扱えないため、届かないことの効果が特に大きい。
- `Option[A]` を `(タグ, A)` の複数の変数に分解する案（案B）は、複数の値を返す処理や入れ物の中の値のために結局 IR の形を変えることになるので採らない。
- ホスト向けの性能改善として、後から codegen-c で有効にする道は残る。

error は lir では構成子に引数がなくても必ず `Ref` である。別々の error の enum が混ざって届き、実行時に見分けるためである（`emela-lir/NOTES.md`）。組み込みでは、プログラム全体の error の enum の共用体 `EmError`（どの error かのタグと値）を1つ作り、失敗しうる関数は最後の引数 `EmError *` に書いて戻る。グローバルな失敗の枠を使わないので、割り込みと同時に動いても壊れない。失敗の集合ごとに共用体を分ける案もあるが、`EmError` を持つのは失敗する関数を呼ぶフレームだけなので、分けても減る量は小さい。

### ② 静的特殊化

実行時に決まる呼び出し先を、コンパイル時に分かる範囲で直接の呼び出しに置き換える。関数値、handler、証拠の表を、引数に来る組ごとに関数を写して静的に決める。

- 関数値: `Option.map(x, is_pressed)` は、`is_pressed` を直接呼ぶ `Option.map` の写しの呼び出しになる。捕獲した値は値型のタプルで渡す。
- handler: lir の証拠の表は実行時に引く写像（`ev_lookup` / `ev_extend`）である。組み込みでは `with` ごとに handler を静的に決め、操作の呼び出しを handler の関数の直接の呼び出しにする。能力の持ち出しの禁止（仕様 8.6）により handler の値は `with` の外へ出ないので、静的に決められる範囲は広い。
- ボードが提供する effect（`Gpio`）の操作は、このパスが作る外部関数の呼び出しになる。出力器は名前で特別扱いしない。
- `State.run` も専用の展開を作らず、State の handler をこの特殊化に乗せる。状態はスタック上の変数とそのポインターになる。

特殊化しても残る間接呼び出し（データ構造に入れたり戻り値で持ち運んだりする関数値）は、当面すべて拒否する。

### ③ 静的評価

`const` の右辺は純粋（仕様 5.6）なので、コンパイル時に評価できる。仕様の意味は「最初の参照で1回だけ評価し、値と defect を覚える」である。これを実行時に実装すると初期化フラグの書き込みが要り、ISR と通常処理から同時に参照されたときに競合する。そこで組み込みではコンパイル時に評価し、結果を読み取り専用のデータとしてフラッシュに置く。

- 評価が defect になった const は、参照したところでその defect を起こす。仕様どおり、使っていない const の defect でビルドを失敗させない。
- 評価器は lir の関数の上の解釈器（`const_eval`）で、止まらない評価に備えて手数の上限を持つ。
- 評価できないもの（外部関数を呼ぶものなど）は、参照のたびに評価する。

### ④ 静的確保

①〜③の後も残るヒープの値は、`heap_bytes` を決めたときだけ割り込みの外で使える（「決定: メモリ管理」）。既定の `heap_bytes = 0` では、ヒープの値が残るプログラムをコンパイルエラーにし、診断には元になった式のある関数を示す。割り込みから届く範囲では、どちらの場合もヒープの値を拒否する。

### ⑤ 境界フック

ホスト向け C ではランタイムや OS が受け持つ処理を、ボード側が実装する C 関数として境界ヘッダーに宣言する。

| フック | 用途 |
| --- | --- |
| `emela_board_init()` | クロックと HAL の開始 |
| `emela_board_defect(kind, message, length)` | defect。戻らない（「決定: defect はその場で止める」） |
| `emela_board_idle()` | `main` の後の待機（WFI など） |
| `emela_board_debug(...)` | `dbg` の出力先 |
| `emela_board_console_write(bytes, length)` / `emela_board_console_error_write(bytes, length)` | Console の出力先（断片を書く）。「決定: platform の handler」 |
| `emela_board_now_milliseconds()` / `emela_board_sleep_milliseconds(ms)` | Clock の `now` と `sleep`。「決定: platform の handler」 |
| `emela_board_random_int()` / `emela_board_random_float()` | Random。「決定: platform の handler」 |
| `emela_board_critical_enter()` / `emela_board_critical_exit()` | 静的な State の排他（割り込み禁止の区間）。「決定: State.run と割り込みをまたぐ状態」 |

- 外部関数（`@external(embedded, ...)`）のプロトタイプも境界ヘッダーに出す。ISR から呼んでよいかは C 側の責任とする（「決定: 外部関数」）。
- 周辺機器の境界ヘッダー（`emela_gpio.h` など）は、Rust の文字列ではなく `Std.Embedded.*` の宣言から生成する。周辺機器を足すときは、主に `.emel` の宣言とボード側の C を書けば済む。
- C で未定義動作になる演算（`INT_MIN / -1`、範囲外のシフトなど）は、仕様の意味（巻き戻り、defect）を実装するヘルパーで出す。

## 組み込み制約の検査

Heap IR の上で、入口（`main`、公開関数、テスト）から届く関数について行う。

- 割り込み入口から届く範囲: ヒープの値、待つ外部関数への到達、再帰がないこと。再帰を拒否するのは、割り込み中のスタックの使用量を決めるためである。
- ターゲット全体: `heap_bytes = 0` ならヒープの値がないこと。fiber の操作、`resume` を後で呼ぶ handler、特殊化しても残る間接呼び出しがないこと。
- ボード定義を選んだとき: 論理ピン ID が定数で、ボードにあること。GPIO と周辺機器のルートでピンが重ならないこと。

## コンパイラのテスト

組み込み出力器そのもののテスト。利用者の `@test` の扱いは「決定: テスト（@test）」に書く。

1. 生成 C のスナップショットテスト（`insta`。codegen-c と同じ）。
2. 生成 C をホストの cc でスタブのボード（GPIO の呼び出しを記録するだけの実装）とリンクして実行し、期待する呼び出し列と比べる。
3. Renode での結合テスト。ツールの場所を環境変数 `STM32_RENODE_ROOT` で指定し、未設定ならスキップする。

## 決定: String の表現

String は、Emela の書き方を変えずに、コンパイラの内部表現と config だけで扱う。容量を超えたときに defect になることを除き、意味はほかのターゲットと同じにする。

### 方針

組み込みで 512 バイトを超える String をスタックに置くこと自体が想定外である。扱う文字列の大半は、ログのメッセージのようなリテラルか、組み立ててすぐ出力するものである。HTTP の本文のような大きな文字列は例外だが、スコープを抜けたらすぐに破棄してよいものであり、String の容量を大きくして扱うものではない。

この前提で、String を固定容量のバッファーとしてスタックに置く。容量は config で決め、既定は 256 バイト、上限は 512 バイトとする。上限を超える値は設定の誤りとして診断する。

### 表現と受け渡し

| 項目 | 方針 |
| --- | --- |
| 表現 | 「静的な文字列」（フラッシュへのポインターと長さ）と「バッファーを持つ文字列」（容量 N の領域と長さ）の2種類を、タグで見分ける（`EmString`）。リテラルはバッファーにコピーしない |
| 容量 | UTF-8 のバイト数で数える。超えたら defect にし、黙って切り詰めない |
| 引数 | ビュー `EmStr`（ポインター、長さ、リテラルかどうか）で渡す。値は不変で、呼び出し元の枠のほうが長生きするので安全である。リテラルは領域なしで渡せる |
| 戻り値 | 呼び出し元が用意した領域に書き込ませる |
| コピー | 構造体を丸ごと代入せず、実際の長さの分だけコピーする |
| 出力操作に直接渡す補間 | `Console.print_line("x = #{x}")` のような場合は文字列を作らず、断片を順に書き出す。容量の制限を受けない |

### スタックの使用量

String をスタックに置くと、容量 256 バイトの String を持つ関数が何段か重なるだけでスタックを使い切るおそれがある（例の `firmware.ld` のスタックは 4KB）。当面は、組み込みのプログラムがそれほど多くのメモリを使う必要はないという前提で進め、スタックの使用量はコンパイラで検査しない（運用で扱う）。足りなくなったときは、ボード側でスタックを広げるか `string_capacity` を小さくする。最悪の使用量の見積もり（`gcc -fstack-usage` の結果を集めるなど）は、必要になったときに検討する。

### config だけで使える範囲

| 区分 | 書き方 | 実現方法 |
| --- | --- | --- |
| 使える | リテラル、受け渡し、戻り値、`==`、文字列リテラルとの `match` | 静的な文字列、ビュー渡し |
| 使える | `contains` / `starts_with` / `ends_with` / `byte_size` | 確保しないのでそのまま実装する |
| 使える | `append`、文字列補間、`Int.to_string`、`Float.to_string`、`trim` | 容量 N のバッファーに書く。超えたら defect |
| 使える | 出力操作に直接渡す補間 | 断片を順に書き出す |
| 使える | `panic` のメッセージ、`dbg` | バッファーに書くか、境界フックへ逐次渡す |
| 使える（重い） | String を含むレコード、`Option[String]` | 1つあたり N バイト以上になる |
| ヒープがあれば使える | `split`、`join`、`code_points`、`from_code_point` | List を「決定: メモリ管理」のヒープに置く（ISR の外のみ）。`heap_bytes = 0` なら拒否する |
| ASCII だけ | `String.length`、`String.chars`、空の区切りでの `split` | 書記素クラスターの表がないので、ASCII でない文字列では defect にする（ホスト向け C と同じ） |
| 扱わない | 512 バイトを超える文字列 | 今後の課題 |

`Float.to_string` は、ホスト向け C の実装（`snprintf` で桁数を増やし、`strtod` で読み戻して最短の桁を選ぶ）を流用し、出力をほかのターゲットとそろえる。newlib-nano では浮動小数点の printf を有効にするリンクの指定（`-u _printf_float`）が要る。

### config

```toml
[embedded.memory]
string_capacity = 256   # String 1つあたりのバイト数（UTF-8）。既定 256、上限 512。超えたら defect
heap_bytes = 0          # Emela のヒープの大きさ（「決定: メモリ管理」）。0 ならヒープを使えない
```

### ホストでのテスト

容量を超えたときの defect がホストのテストで起きないと、ターゲット上で初めて見つかる。そこで `c-embedded` のプロジェクトの `emela test` は、ホスト向け C のランタイムでも同じ `string_capacity` を検査し、容量の超過を見つけられるようにする。

## 決定: defect はその場で止める

組み込みターゲットでは、defect が起きたらその場で境界フック `emela_board_defect` を呼び、戻らない。仕様（7.5、14.4）の巻き戻しは行わず、途中の `with` の `release` も実行しない。これは組み込みターゲットの制限として扱う。

### 理由

- 異常時に出力を安全な状態にする後始末（モーターを止めるなど）は、`release` よりも、どこで止まっても必ず通る `emela_board_defect` で行うほうが確実である。
- 組み込みの handler はボードが提供する GPIO などが中心で、`release` を持つ handler はほとんどない。
- 巻き戻すには、defect になりうる呼び出しのたびに確認の分岐が要り、コードが大きく遅くなる。趣味の規模では単純さとコードサイズを優先する。
- ホストでテストするときは仕様どおり巻き戻るので、テストの結果は変わらない。

### 方式

- lir の `halt_on_defect` を立てる。defect の発生源（`panic`、`todo`、ゼロ除算、照合漏れ、Float から Int への変換の失敗、負のシフト、容量の不足など）で直接フックを呼び、伝播の分岐は作らない。
- defect を保存しておく変数（ホスト向け C の `em_defect`）、`release` の注記、const の名前の前置きは持たない。
- メッセージの文面は仕様の一部ではない（7.5）ので、種類のコードと静的なメッセージで表す。`panic(message)` だけは利用者の String をそのまま渡す。Float から Int への変換の失敗のように、ホストでは値を埋め込むメッセージも、組み込みでは静的な文面にする。
- ISR の中で起きた defect も、ISR の中でそのままフックを呼ぶ。ボード側のフックは ISR から呼ばれても動くように実装する（割り込みを止める、出力を安全な状態にする、止まるかリセットする）。

### フックの形

```c
_Noreturn void emela_board_defect(EmelaDefectKind kind, const char *message, uint16_t length);
```

| 種類 | 発生源 | メッセージ |
| --- | --- | --- |
| `EMELA_DEFECT_PANIC` | `panic(message)` | 利用者の文字列 |
| `EMELA_DEFECT_TODO` | `todo()` | `not yet implemented` |
| `EMELA_DEFECT_DIVISION_BY_ZERO` | 整数のゼロ除算 | `division by zero` |
| `EMELA_DEFECT_NO_MATCH` | 照合漏れ | `unreachable: no match arm matched` |
| `EMELA_DEFECT_FLOAT_TO_INT` | Float から Int への変換の失敗 | `cannot convert Float to Int` |
| `EMELA_DEFECT_NEGATIVE_SHIFT` | 負のシフト量 | `negative shift amount` |
| `EMELA_DEFECT_HEAP_EXHAUSTED` | ヒープの不足（「決定: メモリ管理」） | `heap exhausted` |
| `EMELA_DEFECT_STRING_CAPACITY` | String の容量の超過（「決定: String の表現」） | `string capacity exceeded` |
| `EMELA_DEFECT_UNHANDLED_ERROR` | `main` や割り込み入口まで届いた失敗 | `unhandled error` |
| `EMELA_DEFECT_ASSERT` | `assert` の失敗 | `assertion failed: <式の字面>`。字面はコンパイル時に分かるので静的な文字列にする。ホストで出す両辺の値は出さない |

`assert` はテスト関数の外でも使え、通常のビルドでも取り除かない（13.2）ので、組み込みでも出力する。

## 決定: State.run と割り込みをまたぐ状態

### State.run

`State.run` の状態は、ヒープ上の `Cell` ではなく、`State.run` を展開した関数のスタック上のローカル変数に置く。本体から呼ばれる関数にはその変数へのポインターを渡し、`get` / `set` / `update` は直接読み書きする。State の handler も ② の静的特殊化に乗せることで、専用の展開を作らずに済ませる。状態の型が値型なら、ヒープも参照カウントも使わない。

### 割り込みをまたぐ状態: 静的な State

`State.run` の状態は本体の実行中にしか存在しない。`main` は初期化の後に戻り、ISR は呼ばれるたびに新しく始まるため、`State.run` では割り込みをまたいで状態を残すことも、`main` と ISR で状態を共有することもできない。

そこで、ターゲットが「プログラムが動いている間ずっと生きている `State[S]`」を提供する。状態の読み書きは既存の `State` の書き方のままで、新しい構文や型は足さない。足すのは初期値を指定する注釈だけである（注釈は `@test` や `@external` と同じ既存の仕組みなので、文法は変わらない）。

```emel
import Std.State.{State}

type Presses(n: Int)

@static                          # この const の値を初期値として、静的な State[Presses] を用意する（注釈の名前は仮）
const PRESSES = Presses(n: 0)

pub fn button_changed(event: GpioEvent) use { Gpio, State[Presses] } {
  gpio = use Gpio
  st = use State[Presses]
  st.update(fn(p) { Presses(n: p.n + 1) })     # 割り込みのたびに増え、次の割り込みまで残る
  if st.get().n >= 3 { gpio.write(GpioPin(13), High) } else { () }
}

fn main() use { State[Presses] } {
  ...                                           # main からも同じ状態を読み書きできる
}
```

#### 意味

- `@static` を付けた const の型 S について、静的領域に状態を1つ置き、その const の値で初期化する。const は ③ の静的評価でコンパイル時に計算する。
- ターゲットは、この `State[S]` を `main` と ISR の入口に提供する。型検査には、ターゲットが入口に与える effect（ボードの effect と静的な State）の一覧を渡す。
- 状態は型で区別する（仕様 8.8 と同じく、`State[Presses]`、`State[Mode]` のように型で包む）。同じ型に `@static` の const が2つあればエラーにする。
- `@static` の const を持たない型の `State[S]` は、これまでどおり `State.run` で提供しなければならない。`main` や ISR の中の `State.run` が同じ型の状態を提供すれば、その内側では `State.run` のほうが使われる（最も内側の handler が使われる通常の規則）。

#### 制約

- 状態の型は値型（①）に限る。ISR はヒープに触れないので、`List` などヒープ上の値を持つ型はエラーにする。
- ホストのターゲット（JS、C）では、`@static` の const は普通の const として扱い、静的な State は提供しない。ホストでテストするときは、テスト関数の中で `State.run` で状態を与える。

#### 排他

- 共有状態を使う関数には `use { State[S] }` が付くので、コンパイラは各入口から届く範囲がどの状態に触るかを effect から把握できる。
- 2つ以上の入口（`main`、割り込み入口、その他の公開関数）から届く状態の `get` / `set` / `update` は、割り込み禁止の区間で囲んで出力する。公開関数は C のどこから呼ばれるか分からないので、別の入口と数える。区間の出入りは境界フック `emela_board_critical_enter` / `emela_board_critical_exit` で行う（Cortex-M では PRIMASK を保存して戻す）。1つの入口からしか届かない状態には区間を作らない。
- `update(f)` は一息に実行される（仕様 11.1）ので、f の適用まで含めて1つの区間に入れる。f は純粋なので、区間の中で effect を呼ぶことはない。
- `x = st.get()` と `st.set(x + 1)` を別々に書くと、その間に割り込みが入って更新が失われうる。仕様が fiber について述べる注意（11.1）と同じ性質であり、`update` を使うよう案内する。

## 決定: fiber は当面拒否し、suspend は待つ形で扱う

fiber、Async による後からの再開、Deferred、fiber の取り消しは、組み込みターゲットでコンパイルエラーにする。

`suspend` な外部関数と、最後に `resume` する handler の操作は、完了まで待つ形で許す（下の「完了まで待つ形で許す範囲」）。`Clock.sleep` を使えるようにするためである（「決定: platform の handler」）。

### 理由

- `main` で初期化し、以降は ISR が処理する方式で、ファームウェアは書ける。
- 対応にはスケジューラ、タイマー割り込みによる再開、fiber のフレームの確保、取り消し時の巻き戻しを組み込み向けに作る必要があり、独自実装の中で最も規模が大きい。
- 「点けて、待って、消す」のような1本の流れは、完了まで待つ `Clock.sleep` で書ける。fiber に対応する価値は、そうした流れを複数、状態機械を手で書かずに同時に進められることにある。

### 完了まで待つ形で許す範囲

- `suspend` な外部関数は、完了まで待つ普通の C 関数として呼ぶ（`Clock.sleep` は `HAL_Delay` などになる）。fiber が1つしかないので、完了まで止まることと中断することに違いはない。止まっている間も割り込み（ISR）は動く。
- 最後に `resume(v)` を呼ぶ handler の操作も許す。② の静的特殊化で `resume(v)` を `return v` に書き換える。
- これらを含む関数は、lir の状態機械への変換（`state_machines`）を通さず、普通の関数として出力する。
- 将来 fiber に対応したときは、これらが本当の中断に変わる。Emela のコードは書き換えずに済む。

### 拒否する範囲

- `resume` を最後以外で呼ぶ handler の操作（コールバックで再開するもの、`resume` を値として持ち回るもの）。
- ISR から到達する範囲の `suspend` な呼び出し（ISR の中で待つとシステムが止まるため）。
- fiber の操作（Core IR の `Expr::Fiber`。`Fiber.scope`、`fork`、`join`、`await`、`interrupt`、`attempt`、`Deferred` の操作、`Cause` の操作）。

`Cause` と `Exit`（11.5）は fiber の操作でしか作られないので、上の拒否でまとめて扱える。`attempt` は Async を要求せず fiber なしでも型が付くが、Die を値に変える働きは「決定: defect はその場で止める」と両立しないので、同じく拒否する。後から起きた失敗の注記（11.2、14.4）は、defect で巻き戻らず fiber もないので起きない。

診断は、どの関数のどの呼び出しが該当するかを示し、組み込みターゲットでは未対応であることを明示する。

## 決定: メモリ管理

①〜③の後も残るヒープ上のオブジェクト（`List` の要素など）は、大きさごとの固定容量の領域に置く。`malloc` は使わない。

### 想定する用途

趣味の電子工作の規模を想定し、数か月にわたって止めずに動かし続けるファームウェアは対象にしない。長期間の運用で問題になる断片化への対策は、この決定の主な理由ではない。主な理由は、確保と解放が定数時間になること、実装が単純なこと、容量の不足がどこで起きたかを把握しやすいことである。

### 方式

- ヒープの値は型ごとの箱（参照カウントと、値型と同じ並びのフィールド）にする。ホスト向け C のランタイムの「ヘッダー＋8 バイトのスロット」には揃えない。値型化で String も値になったので、型ごとの構造体のほうが単純で小さい。
- 単相化の後に使われる箱の大きさはコンパイル時にすべて分かるので、大きさごとに空きリストを生成する。
- 領域は config の `heap_bytes` で1つだけ確保する。各大きさの空きリストが空のときは、この領域の未使用部分から切り出す（先頭から順に進めるだけ）。解放した箱は、その大きさの空きリストに戻す。
- 一度ある大きさに切り出した部分は、ほかの大きさには使い回さない。大きさごとの容量を利用者が見積もらなくて済む代わりに、使い方が途中で大きく変わるプログラムでは無駄が出る。趣味の規模ではこれを許容する。
- 未使用部分も空きリストも尽きたら、容量の不足として defect にする。
- 解放は再帰しない。カウントが 0 になった箱を解放待ちのリストに入れ、1つずつ解放するので、長いリストや深い木でもスタックの使用量は変わらない。
- ISR はヒープに触れないので、参照カウントの増減はアトミックでない操作のままにする。静的な State（「決定: State.run と割り込みをまたぐ状態」）は値型に限るので、ISR と通常処理でヒープの値を共有することはない。
- 使用量を返す関数（`emela_heap_used_bytes()`、`emela_heap_live_bytes()`）を出し、`heap_bytes` の見積もりに使えるようにする。

### config

```toml
[embedded.memory]
heap_bytes = 0   # Emela のヒープの大きさ。0 ならヒープを使えず、確保が残るプログラムはコンパイルエラーにする
```

`heap_bytes = 0` を既定にし、ヒープを使わないプログラムではアロケーター自体を出力しない。

## 決定: 外部関数

外部関数（12章）を、組み込み専用のターゲット名 `embedded` で宣言できるようにする。ベンダーの HAL、センサーのドライバー、利用者の書いた C を呼ぶ手段であり、`Std.Embedded` にない周辺機器を使う唯一の方法になる。呼べる場所の規則（`use` を省略した外部関数はハンドラの中だけ。12.2）は変えない。

```emel
effect Thermometer {
  fn read_celsius() -> Float
}

@external(embedded, "bme280_read_celsius")
fn bme280_read_celsius() -> Float

handler Bme280 implements Thermometer {
  fn read_celsius() { bme280_read_celsius() }
}
```

### ターゲット名

- `@external(c, ...)` とは共有せず、`embedded` を足す。String の渡し方がホスト向け C と違う（ホストは末尾 NUL と `malloc`、組み込みは長さ付きと呼ぶ側のバッファー）ので、ホスト向けに書いた C を組み込みで読むと、型の合わない関数が黙ってリンクされうるためである。
- 数値だけを受け渡す関数でも注釈を2つ書くことになるが、これは許容する。
- 変更は `@external` のターゲット名の許可リスト（`emela-syntax` の `EXTERNAL_TARGETS`）に1語足すことで、Emela の書き方は変わらない。

### 型の渡し方

| 型 | C での形 |
| --- | --- |
| Int / Int64 / Float / Bool / `()` | `int32_t` / `int64_t` / `double` / `bool` / `void` |
| String（引数） | `const char *text, uint16_t length`。末尾 NUL も保証する（フラッシュのリテラルにもバッファーにも1バイト余分に取る）ので、`strlen` を前提にする C のライブラリにもそのまま渡せる |
| String（戻り値） | 呼ぶ側がバッファーを渡す: `uint16_t f(..., char *out, uint16_t capacity)`。C は書いた長さを返す。容量を超えた長さが返ったら defect（`STRING_CAPACITY`） |
| `@external type` | `void *` をそのまま値として持つ（① 値型化の対象。ヒープの箱に入れない） |
| コールバック | 当面は拒否する（ホスト向け C でも未対応） |
| `suspend fn` | 完了まで待つ普通の C 関数として呼ぶ（「決定: fiber は当面拒否し、suspend は待つ形で扱う」）。ISR からは呼べない |

- 例外（12.5）は C にないので扱わない。C 側で止めたいときは `emela_board_defect` を直接呼ぶ。

### 宣言と C のファイル

- 外部関数のプロトタイプを、生成する境界ヘッダーに出す。C 側がそのヘッダーを include すれば、宣言と実装の型の食い違いを C コンパイラが検出する（ホスト向け C にはこの検査がない）。
- ファイルを指す形（`@external(embedded, "./sensor.c", "name")`）は、そのファイルを出力先に写す。名前だけの形は、ボード側のソースが実装するものとする。

### ISR からの呼び出し

- ISR から外部関数を呼ぶことを許可し、ISR から安全に呼べるかは C 側の責任とする。GPIO の境界ヘッダーが「割り込み中に呼ぶ操作は ISR 安全に実装してください」としているのと同じ扱いである。
- String は ISR で使えないので、String を受け渡す外部関数は既存の制約で ISR から拒否される。
- `@isr_safe` のような印は付けない。書き方が増えるわりに、宣言の正しさをコンパイラが検証できない（12.2 の `use {}` と同じ）ためである。

## 決定: platform の handler

Console、Clock、Random は、ホストと同じ `with Platform { ... }`（または `with SystemConsole` など）で供給する。組み込み専用の layer や書き方は足さない。std の外部関数に `@external(embedded, ...)` を足し、ボード側の C 関数（境界ヘッダーに出す）に写す（「決定: 外部関数」）。

```emel
fn blink(n: Int) -> () use { Gpio, Clock, Console } {
  (use Gpio).toggle(GpioPin(13))
  (use Console).print_line("toggle #{n}")
  (use Clock).sleep(500)
  blink(n + 1)               # 自分への末尾呼び出し（スタックを使わない。仕様 6.8）
}

fn main() use { Gpio } {
  with Platform { blink(0) }
}
```

E0412 の検査は単相化の後に呼ばれる操作だけを見るので、使わない Http や FileSystem はエラーにならない。

### effect ごとの実装

| effect | 組み込みでの実装 | ボード側の C 関数 |
| --- | --- | --- |
| Console | 断片を順に書き出す関数を呼び、最後の改行はコンパイラが書く | `void emela_board_console_write(const char *bytes, uint16_t length)`、`void emela_board_console_error_write(const char *bytes, uint16_t length)` |
| Clock の `now` | ボードが返すミリ秒 | `int64_t emela_board_now_milliseconds(void)` |
| Clock の `sleep` | 完了まで待つ（「決定: fiber は当面拒否し、suspend は待つ形で扱う」） | `void emela_board_sleep_milliseconds(int32_t ms)` |
| Random | ボードが返す値 | `int32_t emela_board_random_int(void)`、`double emela_board_random_float(void)` |
| Env、FileSystem、Http | 提供しない。呼べばコンパイル時に E0412 になる | — |

- Console: 「決定: String の表現」の、出力操作に直接渡す補間は文字列を作らずに断片を順に書き出す方針を実現するため、Console だけはコンパイラが断片ごとに C 関数を呼び、最後に改行を書く。C 側は1行を受け取る関数ではなく、断片を書く関数として実装する。この特別扱いは名前ではなく定義（`Std.Console`）で判別する。多くのボードでは、2つの関数を同じ UART に出す。
- Clock の `now`: 仕様では Instant は Unix エポックからのミリ秒だが、マイコンの多くは時計が合っていない。起点はボードが決める（RTC が合っていればエポックから、なければ起動から）。差（経過時間）はどちらでも正しく求められる。
- Random: 仕様は暗号論的に安全な乱数を求めるが、組み込みでは質をボードに任せる（RNG 周辺機器があればそれを使い、なければ擬似乱数）。
- ISR: platform の handler は `main` の `with` で供給するので、ISR では使えない。ISR に渡るのはターゲットが提供する effect（GPIO、静的な State）だけである。

## 決定: テスト（@test）

`c-embedded` のプロジェクトでも、`emela test` は既定でホスト向け C のターゲットでテストを動かす。`emela test --device` を付けたときだけ、ボードかエミュレーターで動かす。

```emel
handler TestGpio implements Gpio {
  fn configure(pin, mode, pull) { () }
  fn read(pin) { Low }
  fn write(pin, state) {
    (use State[List[PinState]]).update(fn(xs) { List.append(xs, [state]) })
  }
  fn toggle(pin) { () }
  fn enable_interrupt(pin, edge) { () }
  fn disable_interrupt(pin) { () }
}

@test
fn lights_when_pressed() {
  (_, writes) = State.run([], fn() {
    with TestGpio { light_if_pressed(GpioPin(1), GpioPin(13)) }
  })
  assert writes == [High]
}
```

### ホストで動かす

- GPIO などのターゲットが提供する effect は、テストでは利用者が書くテスト用の handler に差し替える（13章のとおり、モックのための仕組みは持たない）。`Std.Embedded.*` の型と effect は、ホストのターゲットからも import できる。
- ISR の入口は普通の関数なので、テストから `GpioEvent` などを作って直接呼べる。
- 静的な State は、ホストでは普通の const になる（「決定: State.run と割り込みをまたぐ状態」）ので、テストでは `State.run` で状態を与える。
- `string_capacity` を検査するので、容量の超過をテストで見つけられる（「決定: String の表現」の「ホストでのテスト」）。
- 確かめられるのはアプリの論理と String の容量までで、組み込み向けの C の出力、`heap_bytes` の不足、スタックの深さはホストのテストでは分からない。

### ボードやエミュレーターで動かす（`--device`）

- ビルドと実行は、プロジェクトが用意する runner（`[embedded.test] runner`）に任せる。arm-none-eabi-gcc、Renode、HAL の場所はボードやプロジェクトごとに違うので、Emela は持たない。
- Emela はテストのファームウェアの C を書き、runner の標準出力から結果の行を読む。ファームウェアは結果の行を Console に書く。
- defect はその場で止まる（「決定: defect はその場で止める」）ので、ファームウェアは止まる前にそのテストの結果を書く。runner が途中で終われば、Emela は次のテストから runner を呼び直す。再開の位置は生成する `emela_test_start.h` で渡す。
- 詳細は [`c-embedded-codegen.md`](c-embedded-codegen.md#テスト)。

## 今後の課題

- fiber と Async（「決定: fiber は当面拒否し、suspend は待つ形で扱う」）。当面は入れない。
- 特殊化しても残る間接呼び出しを、ISR の外では C の関数ポインターとして許すか。
- 寿命が関数の中で終わり、サイズが静的に決まる確保を、ヒープではなくスタックに置く（④）。
- String の長さの上限の推論（リテラルの長さ、`Int.to_string` は 11 バイトのように、上限が静的に分かる String をその長さで確保する）。
- 512 バイトを超える大きな文字列。スコープの出入りに合わせて確保と解放を行う LIFO の作業領域に置く方法を検討する。書き方は変えない。
- 書記素クラスターの表（`String.length` などを ASCII 以外でも使えるようにする）。全ターゲットに共通の課題で、組み込みではフラッシュと引き換えにするかを config で選ぶ。
- 失敗をそのまま伝えるだけの呼び出しで、呼び出し元の `EmError *` をそのまま渡し、フレームの `EmError` を減らす。
- 静的な State の初期値を指定する注釈の名前と形（仮に `@static`）。
- 組み込み出力器を独立 crate のままにするか、codegen-c にプロファイル（ホスト / フリースタンディング）として統合するか。
