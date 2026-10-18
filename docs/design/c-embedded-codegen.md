# 組み込みCの出力

`emela build --target c-embedded` は、リンク前のCソースを `target/emela/c-embedded/` に出力します。既存の `--target c` が使うヒープランタイム、POSIX API、cc によるリンクは含みません。設計の背景と今後の計画は [`c-embedded-architecture.md`](c-embedded-architecture.md) にあります。

| ファイル | 内容 |
| --- | --- |
| `main.c` | プログラム本体。Emela の `main` を呼び、その後は `emela_board_idle()` を繰り返し呼ぶ C の `main` を含む |
| `emela_board_hal.c` | ボード定義から生成する GPIO の HAL アダプター（ボードを選び、その HAL に対応しているとき） |
| `emela_board.h` | ボードが実装するフック（`emela_board_init`、`emela_board_defect`、`emela_board_idle`、複数の入口が使う静的な State があるときは `emela_board_critical_enter` / `emela_board_critical_exit`） |
| `emela_gpio.h` など | 使っている周辺機器ごとの境界ヘッダー。`Std.Embedded.*` の宣言から生成する |
| `emela_embedded.h` | ボードから呼ぶ公開関数（`emela_export_<名前>`）と、外部関数のプロトタイプ |
| 外部関数が指す C ファイル | `@external(embedded, "./sensor.c", "name")` の `sensor.c` を写したもの |

## パイプライン

ホスト向け C と同じく、単相化した Core IR を Flow IR と Heap IR（`emela-lir`）に通してから C を出します。組み込み向けには次の設定とパスを使います。

1. 値型（`Repr::Agg`）を有効にする。引数を持つ enum とタプルのうち、ヒープを使わずに持てるもの（`Option[PinState]`、`GpioEvent` など）は C の構造体として値で受け渡す。
2. defect をその場で止める。`panic`、ゼロ除算、照合漏れなどは `emela_board_defect` を呼び、戻らない。巻き戻しはしない。
3. 静的特殊化（`emela_lir::specialize_statics`）。関数値、handler、証拠の表を、引数に来る組ごとに関数を写して静的に決める。`Option.map(x, is_pressed)` は `is_pressed` を直接呼ぶ `Option.map` の写しの呼び出しになり、`with BoardLed { ... }` の中の操作の呼び出しは `BoardLed` の操作の関数の直接呼び出しになる。捕獲した値や handler の状態は値型のタプルで受け渡し、関数ポインターもヒープも使わない。
4. 組み込み制約を Heap IR の上で検査する（後述）。
5. const をコンパイル時に評価する（`emela_lir::eval_consts`）。値は `static const` の読み取り専用のデータになる。評価が defect になる const は、参照したところでその defect を起こす（使わない const の defect でビルドは失敗しない）。評価できないもの（外部関数を呼ぶものなど）は参照のたびに評価する。
6. Heap IR を C に写す（`emela-codegen-c-embedded/src/emit.rs`）。

## ボードとの境界

周辺機器のモジュール（`Std.Embedded.Gpio`）ごとに境界ヘッダー（`emela_gpio.h`）を生成します。

- effect の操作は `emela_gpio_<操作>` の C 関数で、ボードが実装する。
- 型は `EmelaGpio<型名>`（`GpioPin` は `EmelaGpioPin`、`PinState` は `EmelaGpioPinState`）。フィールドが1つだけのレコード（`GpioPin(id: Int)`）はそのフィールドの型そのもの（`int32_t`）になる。
- 引数のない構成子だけの enum は `uint8_t` で、構成子は `EMELA_GPIO_PIN_STATE_LOW` のような定数になる。

ボードの effect の handler は C の値を持ちません。`gpio.write(pin, state)` はそのまま `emela_gpio_write(pin, state)` になります。利用者が書いた handler（ボードの effect を差し替えるものを含む）も `with` で与えられ、静的に決まります。

## ボード定義と割り込みの結び付け

`Pome.toml` の `[build] board` でボード定義（[`board-definition.md`](board-definition.md)）を選ぶと、プログラムの `GpioPin(n)` をボードの論理ピンと照らし、`hal = "STM32CubeF4"` のボードでは GPIO の HAL アダプター `emela_board_hal.c` を生成します。割り込み入口は `[embedded.gpio_interrupts]` で論理ピンに結び付けます（`100 = "button_changed"`）。生成するアダプターが EXTI の割り込みハンドラーと HAL コールバックを持ち、割り込みの中で入口を呼びます。ボード側の C は `emela_board_init`（クロックと HAL の開始）と `emela_board.h` のフックを実装します。

## platform の handler

Console、Clock、Random は、ホストと同じ `with Platform { ... }` で使えます。std の外部関数がボードのフック（`emela_board.h`）に写ります: Console は `emela_board_console_write` / `emela_board_console_error_write`（行の断片を書く。改行も出力器が断片として書く）、Clock は `emela_board_now_milliseconds` と `emela_board_sleep_milliseconds`（完了まで待つ）、Random は `emela_board_random_int` / `emela_board_random_float` です。Env、FileSystem、Http は提供せず、使えば E0412 になります。

Console に直接渡す補間（`(use Console).print_line("x = #{x}")`）は文字列を作らず、断片を順に書き出します（`string_capacity` の制限を受けません）。`SystemConsole` の操作のように、受け取った String を Console の外部関数に渡すだけの関数も、渡す先として扱います。

## 周辺機器（UART、I2C、SPI）

`Std.Embedded.Uart`、`I2c`、`Spi` の effect も、ボードが実装する C 関数（`emela_uart_transmit` など）になります。送受信のデータは `List[Int]` のままなので、`heap_bytes` が要ります（割り込みの中では使えません）。C の関数の形は次のとおりです。

- `fails PeripheralError` の操作は `int32_t` を返す。0 なら成功、それ以外ならその値を `code` とする `PeripheralError(code)` の失敗になる。値の結果（`is_device_ready` の Bool など）は最後の引数 `out` に書く。
- `List[Int]` の引数は `const uint16_t *data, uint16_t data_length`。要素が 0〜65535 の外か、`EMELA_TRANSFER_CAPACITY`（既定 256 要素）より長ければ、ボードを呼ばずに `PeripheralError(-1)` になる。
- `List[Int]` の結果は `uint16_t *out, uint16_t capacity, uint16_t *length` に書く。

ボード定義に `[peripheral_routes.uartN]`（`instance`、`tx_pin`、`rx_pin`、`alternate_function`）があり、`hal = "STM32CubeF4"` なら、UART の操作も HAL アダプター（`emela_board_hal.c`）に生成します（HAL の UART のポーリング API。HAL の設定で UART と DMA のモジュールを有効にし、`stm32f4xx_hal_uart.c` と `stm32f4xx_hal_dma.c` をリンクします）。`data_bits` はパリティを含まないデータの幅で、アダプターが HAL の `WordLength`（パリティを含むフレームの幅）に直します。HAL で表せない組み合わせ（7 ビットでパリティなし、9 ビットでパリティあり）は、`configure` が `PeripheralError(2)` を返します。例は [`examples/embedded-uart`](../../examples/embedded-uart)。I2C（`[peripheral_routes.i2cN]` の `scl_pin`、`sda_pin`）と SPI（`.spiN` の `sck_pin`、`miso_pin`、`mosi_pin`）も同じく生成します（HAL の I2C と SPI のモジュールが要ります）。I2C のアドレスはシフトしない 7 ビットか 10 ビットで、SPI のチップセレクトはプログラムが GPIO で操作します。例は [`examples/embedded-bus`](../../examples/embedded-bus) で、Renode では BareRuby の `stm32_renode` の機器のモデル（I2C のレジスターを持つセンサー、SPI の応答を返す機器）につないで確かめます。UART の受信もこの例で確かめます。

```c
int32_t emela_uart_transmit(EmelaUartId id, const uint16_t *data, uint16_t data_length, int64_t timeout_ms);
int32_t emela_uart_receive(EmelaUartId id, int32_t size, int64_t timeout_ms, uint16_t *out, uint16_t capacity, uint16_t *length);
```

## 外部関数

`@external(embedded, "name")` か `@external(embedded, "./file.c", "name")` で、C の関数を呼べます（ホスト向け C の `@external(c, ...)` とは別のターゲット名）。プロトタイプは `emela_embedded.h` に出るので、C 側がこれを include すれば型の食い違いを C コンパイラが検出します。ファイルを指す形は、そのファイルを出力先に写します。受け渡せる型は Int（`int32_t`）、Int64、Float（`double`）、Bool、`()`、`@external type`（`void *`）と String（「String」の節）です。コールバックはまだ使えません。割り込みから呼べるかは C 側の責任です。

## ヒープ

値型にならない値（`List`、再帰する enum、それらを含む enum やタプル）は、`Pome.toml` の `heap_bytes` を決めると割り込みの外で使えます（`docs/design/c-embedded-architecture.md` の「決定: メモリ管理」）。

```toml
[embedded.memory]
heap_bytes = 8192   # 既定 0（ヒープを使わない。ヒープの値はコンパイルエラー）
```

ヒープの値は型ごとの箱（参照カウントと、値型と同じ並びのフィールド）で、空のリストは `NULL`、フィールドのない構成子は静的な箱です。箱は大きさごとの空きリストか領域の未使用部分から取り、解放したら空きリストに戻します（`malloc` は使いません）。足りなくなると defect `EMELA_DEFECT_HEAP_EXHAUSTED` です。割り込みから届く関数ではヒープの値を使えません。解放は再帰しません。カウントが 0 になった箱は、解放待ちの箱のカウントの欄をつないだリスト（領域内の位置と型の番号を 32 ビットで表す）に入り、`em_drain` が1つずつ解放します。長いリストや深い木でもスタックの使用量は変わりません。このため、ヒープの値の型は 255 種類まで、`heap_bytes` は約 128MB までです。

ヒープを使うプログラムでは、`emela_embedded.h` に使用量を返す関数が宣言されます。`emela_heap_used_bytes()` は領域から切り出したバイト数で、解放した箱は同じ大きさの箱にだけ使い回すので、その実行に必要だった `heap_bytes` の最小値になります。`emela_heap_live_bytes()` は使用中の箱のバイト数です。`heap_bytes` の見積もりに使います（割り込みからは呼べません）。

## 失敗（`fails`）

error の値も値型です。受けた失敗（`escape` に届くもの）は `EmError`（どの error かのタグと、プログラムの error の enum の共用体）で、失敗しうる関数は最後の引数 `EmError *em_fail` に error を書いて戻ります。呼ぶ側は呼び出しの直後にタグを見ます（グローバルな失敗の枠を使わないので、割り込みと同時に動いても壊れません）。`main` や割り込み入口まで届いた失敗は、defect `EMELA_DEFECT_UNHANDLED_ERROR` になります。`Int.parse` と `Float.parse` も使えます。

## 静的な State

`@static` を付けた const の型 S について、プログラムが動いている間ずっと生きている `State[S]` を `main` と割り込み入口に提供します。初期値はその const の値で、コンパイル時に決まらなければなりません。状態は静的変数に置きます。2つ以上の入口（`main`、割り込み入口、その他の公開関数。公開関数は C のどこから呼ばれるか分からないので別の入口と数えます）から届く状態は、`get` / `set` / `update` を割り込み禁止の区間（`emela_board_critical_enter` / `emela_board_critical_exit`）の中で読み書きします。1つの入口からしか届かない状態には区間を作りません。`update(f)` は f の適用まで含めて1つの区間に入ります。同じ型に `@static` の const が2つあればエラーです。ホストのターゲットでは普通の const です。

```emel
type Presses(n: Int)

@static
const PRESSES = Presses(n: 0)

pub fn button_changed(event: GpioEvent) use { Gpio, State[Presses] } {
  (use State[Presses]).update(fn(p) { Presses(n: p.n + 1) })
}
```

## String

String は容量に上限のある値です（`docs/design/c-embedded-architecture.md` の「決定: String の表現」）。変数とフィールドは `EmString`（長さと、リテラルを指すポインターか容量 N バイトの領域）で、リテラルはフラッシュを指すだけでコピーしません。関数と組み込み関数には `EmStr`（ポインター、長さ、リテラルかどうか）で渡し、String を返す関数は呼び出し元の `EmString` に書き込みます。容量を超える String を作ると defect `EMELA_DEFECT_STRING_CAPACITY` になります（黙って切り詰めません）。容量は `Pome.toml` で決めます。

```toml
[embedded.memory]
string_capacity = 256   # String 1つあたりのバイト数（UTF-8）。既定 256、上限 512
```

使える組み込み関数は `append`（文字列補間）、`byte_size`、`contains`、`starts_with`、`ends_with`、`trim`、比較、`Int.to_string`、`Int64.to_string`、`Float.to_string`（C ライブラリの浮動小数点の `snprintf` を使う。newlib-nano では `-u _printf_float` が要る）です。`dbg` は境界フック `emela_board_debug` に1行ずつ渡します。String を受け渡す外部関数は、引数が `const char *text, uint16_t text_length`（末尾に NUL もある）、戻り値が `uint16_t f(..., char *out, uint16_t capacity)`（書いた長さを返す）の形です。

String の変数はそれぞれ容量分のスタックを使います。補間を多く含む関数（生成される表示の関数など）は数 KB を使うことがあるので、足りなければボードのスタックを広げるか `string_capacity` を小さくします。

## 完了まで待つ操作

`suspend fn` の外部関数は、完了まで待つ普通の C 関数として呼びます。`suspend fn` の操作は、`SystemClock.sleep` のように最後に `resume(v)` を呼ぶ形なら使え、`resume(v)` は呼び出し元へ `v` を返すことになります。状態機械は作りません。`resume` を最後以外で呼ぶ操作と、割り込みから待つ外部関数に届くことは拒否します。

## 入口と割り込み

入口は `main` と公開関数（`pub fn`）です。公開関数は `emela_export_<名前>` として `emela_embedded.h` に宣言され、ボードから呼べます。`GpioEvent` を取る公開関数は割り込み入口で、`fn(GpioEvent) -> ()` の形でなければなりません。割り込み入口から届く関数は再帰できません（割り込み中のスタックの使用量を決めるため）。ボード側の HAL コールバックから割り込み入口を直接呼びます。公開関数の String は外部関数と同じ形で受け渡します。引数は `const char *s, uint16_t s_length`（正しい UTF-8 であること）で、結果は呼び出し側が渡す `char *out, uint16_t capacity` に書かれ、関数は長さを返します（余裕があれば NUL も書きます）。結果が `capacity` に収まらなければ defect `EMELA_DEFECT_STRING_CAPACITY` です。

## 対応範囲

- 使える: 整数・真偽値・Int64・Float の演算、値型の enum・レコード・タプル、`if`、`match`（入れ子のパターンとガードを含む）、自己末尾呼び出し（ループになる）、関数の引数に渡す関数値（変数を捕獲するラムダを含む）、利用者の handler、`State.run`、`const`、静的な State（`@static`）、String と文字列補間、`dbg`、失敗（`fails`、`escape`）、`heap_bytes` を決めたときの `List` と再帰する enum（割り込みの外）、数値の組み込み関数（ビット演算、シフト、Int64 と Float の変換。`Float.floor`、`ceil`、`round`、`truncate` と Float の `%` は C の `math.h` の関数を使うので、libm をリンクする（`-lm`））、ボードの effect の操作、外部関数、`panic` などの defect。
- まだ使えない（診断で拒否する）: `heap_bytes` なしのヒープの値、割り込みの中のヒープの値、データ構造に入れたり戻り値で持ち運んだりする関数値、fiber、`resume` を後で呼ぶ handler。`String.length`、`String.chars`、空の区切りの `String.split` は Unicode の書記素クラスターの表がまだないので ASCII の文字列だけを扱い、ASCII でない文字列では defect になります（ホスト向け C と同じ）。

`run` は実行ファイルを作らないターゲットのため利用できません。

## テスト

`c-embedded` のプロジェクトでも `emela test` が使えます。テストはホスト向け C で動き、GPIO などはテスト用の handler に差し替えます。`string_capacity` を超える String はホストでも defect になるので、容量の超過をテストで見つけられます。

`emela test --device` は、テストを組み込み C のファームウェアにしてボードかエミュレーターで動かします。組み込み向けの出力、`heap_bytes` の不足、スタックの深さまで確かめられます。

- `Pome.toml` の `[embedded.test] runner` に、ファームウェアを動かすプログラム（runner）を指定します。runner は `runner <出力先>` の形で、プロジェクトのディレクトリから呼ばれます。出力先の C（`main.c` など）とボード側の C をビルドして動かし、Console に出た文字を標準出力に書きます。ビルドのログは標準エラー出力に出します。
- ファームウェアの `main` は、選んだテストを順に呼び、テストごとに結果の行をホストのテストと同じ形で `emela_board_console_write` に書き、最後に終わりの行を書きます。テストは入口なので、ボードの effect は与えられません（ホストのテストと同じく、テスト用の handler を使います）。`[embedded.gpio_interrupts]` の結び付けも使いません。
- defect はその場で止まるので、ファームウェアは止まる前にそのテストの結果（`defect` と文言）を書きます。終わりの行が来ないまま runner が終われば、Emela は次のテストから runner を呼び直します。どこから始めるかは、出力先の `emela_test_start.h`（`#define EMELA_TEST_START <番号>`）で渡します。結果を1つも書かずに runner が終わったテストは、中断（aborted）として報告します。
- ボード側は `emela_board_init` で Console（UART など）を使えるようにし、`emela_board_console_write` を実装します。runner は終わりの行を待たずに、決まった時間だけ動かしてもかまいません。
- 例: [`examples/embedded-gpio/renode-f446/test.sh`](../../examples/embedded-gpio/renode-f446/test.sh) は NUCLEO-F446RE 向けにビルドして Renode で動かし、USART2 に出た文字を写します（ボード側は `test_board.c`）。

コンパイラ自身のテストとしては、`emela-driver/tests/c_embedded.rs` が、GPIO 例の生成 C のスナップショットと、生成 C をホストの cc でスタブのボード（GPIO の呼び出しを記録する）とリンクして実行した結果を確かめます。Renode での確認は各例の `renode-f446/run.sh` で行い、`emela-driver/tests/renode.rs` が結合テストとして GPIO、UART、I2C と SPI の例と、GPIO 例の `emela test --device` を動かします（`STM32_RENODE_ROOT` にツールの場所を指定したときだけ。未設定なら飛ばします）。
