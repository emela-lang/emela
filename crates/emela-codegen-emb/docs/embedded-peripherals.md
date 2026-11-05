# 標準組み込みhandlerとCボードABI

利用者向けの構成・Emelaでの利用方法は[組み込みインターフェース](embedded-interfaces.md)、Rust APIと入力契約は[実装範囲と対応状況](implementation.md)を参照する。この文書は `Std.Embedded.Board` とC実装の境界を記録する。STM32F4・ESP32-C3のBSPとstandalone build・Flash起動確認は[ボードpackage](boards.md)、projectの設定・build・flashは[CLIの開発手順](cli-embedded.md)へ分ける。

## 接続と検査

`Std.Embedded.Board` は `BoardGpio`・`BoardUart`・`BoardI2c`・`BoardSpi`・`BoardClock`・`BoardConsole` を提供する。アプリは通常の `with` でhandlerを選択する。アプリごとの `@external`、`external_adapters`、`task_adapters` の登録は不要である。独自のセンサや既存Cライブラリには従来の汎用FFIも使える。

標準handlerの内部だけにprivateな外部関数がある。backendは次を照合し、そのC adapterを自動生成する。

- 完全修飾名 `Std.Embedded.Board.gpio_write` などと、専用C binding `__emela_peripheral_gpio_write` などが一致すること。
- 引数と戻り値が対応する標準effectのoperationの型と一致すること。
- GPIOは元の戻り値、失敗可能なoperationは `(Int, 元の戻り値)` を返し、外部関数自体は非failsであること。
- Clock・Consoleはそれぞれの固定署名に一致し、Clock.sleepだけがsuspendすること。

失敗可能なoperationのadapterは `(code, value)` を返す。共有handlerが通常のEmelaコードで `code != 0` を `PeripheralError(code)` に変換する。sourceの外部関数へ `fails` を追加したり、アプリのFlow IRを手動で置き換えたりしない。汎用FFIのadapter登録規約も変更しない。

## 生成ファイルと値型

到達する共有handlerに応じて `emela_gpio.h`・`emela_uart.h`・`emela_i2c.h`・`emela_spi.h`・`emela_clock.h`・`emela_console.h` を生成する。GPIO/UART/I²C/SPIには共通の `emela_peripheral.h` も生成する。

型名・enum定数・recordのfield名は標準ライブラリの宣言から生成する。

| Emela | C |
| --- | --- |
| `GpioPin(id: Int)` | `EmelaGpioPin`、fieldは `int32_t id` |
| `PinState` | `EmelaGpioPinState`、`EMELA_GPIO_PIN_STATE_LOW` / `HIGH` |
| `UartConfig` | `EmelaUartConfig`、`baud_rate`・`data_bits`などの名前付きfield |
| `UartId.Uart2` | `EMELA_UART_ID_UART2` |
| `Int` / `Bool` | `int32_t` |
| `Int64` | `int64_t` |

値のないenumは `int32_t`、recordは名前付きfieldのstructになる。これらはEmela内部の型IDやtag付きaggregateのlayoutに依存しない。ボードは生成headerをincludeして実装する。

## 同期operation

GPIOは元の値を返す。UART/I²C/SPIはstatusを返し、値がある場合は出力先へ書く。

```c
void emela_gpio_write(EmelaGpioPin pin, EmelaGpioPinState state);
int32_t emela_uart_configure(EmelaUartId id, EmelaUartConfig config);
int32_t emela_i2c_is_device_ready(
    EmelaI2cId id, int32_t address, int32_t trials,
    int64_t timeout_ms, int32_t *out);
```

status `0` は成功、それ以外は `PeripheralError(code)` になる。失敗時は出力値を利用しない。GPIOには失敗チャネルがないので、無効なボード設定の扱いはボード実装で定める。

operationは同期呼び出しであり、長いHAL待機を短くするschedulerへの変換は行わない。UART/I²C/SPIをISRで呼ばず、応答時間はtimeoutとボード実装で管理する。

## Listの転送

`List[Int]` はボード境界で `uint16_t` 配列と長さに変換する。UARTの9bitやSPIの16bitも表せるよう、転送単位はbyteではなく**16bitのword**である。

```c
int32_t emela_uart_transmit(
    EmelaUartId id, const uint16_t *data, uint16_t length,
    int64_t timeout_ms);
int32_t emela_uart_receive(
    EmelaUartId id, int32_t size, int64_t timeout_ms,
    uint16_t *out, uint16_t capacity, uint16_t *length);
```

Rustの `Options::with_peripheral_buffer_capacity` はword数で、既定256、範囲1〜65,535である。1方向あたり最大 `2 × capacity` byteのCスタック領域を使う。SPI.transferは入力と出力の2方向分を同じC frame上で使う。既定値では配列だけで通常512 byte、transferは1024 byteとなる。容量65,535ではそれぞれ131,070 / 262,140 byteになるため、固定上限の設定だけで小さいstackへの適合は保証しない。実際の最大stackはほかのローカル変数や呼び出し先を含むため、生成Cの `.su` や実行時の観測と合わせて評価する。

入力を黙って切り詰めない。次の2つはbridgeが予約するcodeであり、ボード独自のエラーcodeには使わない。

| 定数 | code | 条件 |
| --- | --- | --- |
| `EMELA_PERIPHERAL_INVALID_ARGUMENT` | -1 | 要素が0〜65,535の範囲外、受信sizeが負 |
| `EMELA_PERIPHERAL_CAPACITY_EXCEEDED` | -2 | 入力Listまたは受信sizeが設定容量を超過、返却lengthが容量・要求sizeを超過 |

SPI.transferの返却lengthは入力word数も超えてはならない。入力の範囲・容量エラーではボードoperationを呼ばない。ボードが失敗した場合は出力配列を読まない。成功時のlengthを検査してからListを構築する。

bridgeの要素範囲は共通の0〜65,535である。I²Cの8bit、UARTで選んだ7/8/9bit、SPIで選んだ8/16bitに収まるかは、設定を保持する**ボード実装が検査する**。上位bitをキャストで捨てる実装は契約違反となる。

配列は呼び出し中だけ借用できる。ボードはポインターを保存しない。出力は渡されたcapacity以内へ書き、成功時は実際に初期化したword数をlengthに設定する。返却後のlength検査で、C側の領域外書き込みまで防げるわけではない。

受信Listは生成runtimeの正規の型付きnode・固定heap allocatorで構築する。ボードへListのlayoutや参照カウント操作を要求しない。adapterは所有するListをEmelaへ一度だけ渡すので、借用結果のsnapshotによる二重確保はない。通常の所有権処理により、利用・破棄・失敗後に解放される。heap容量不足は既存の固定heap defect契約に従う。

固定転送配列は到達する転送adapter内に生成する。組み込み標準handlerを使わない既存のプログラムには、この機能の転送配列は追加されない。handlerのoperationを関数値として保持する現runtimeでは、使用するhandlerのほかのoperationも到達扱いになる場合がある。生成headerとlink結果で必要なボード関数を確認する。

## ClockとConsole

```c
int64_t emela_board_clock_now(void);
void emela_board_clock_sleep(EmelaTask task, int32_t milliseconds);
void emela_board_console_print_line(const char *data, uint16_t length);
void emela_board_console_print_error_line(const char *data, uint16_t length);
```

`emela_clock.h` は `emela_embedded.h` をincludeし、task runtimeを生成する場合だけsleepの宣言を追加する。nowだけが到達するFlowでは `EmelaTask` の定義を要求しない。

`BoardClock.now` は既存の `Std.Clock` と同様にUnix epochからのmillisecondを返す。epochをどのように設定するかはボードの責務である。起動からの経過時間をそのままUnix時刻として扱う契約ではない。

sleepのstartはmainから呼ばれる。ボードはtask handleを保持し、待機完了時に `emela_task_complete_unit(task)` で通知する。この通知はISRから呼べるが、Emelaの再開・heap操作はmainのschedulerが行う。共有handlerは負の待機時間を0にし、ボードは0以下の待機をタイマー起動なしで即時に完了通知する。この場合もEmelaの実際の再開はschedulerが行う。取消済みのtaskへ遅れて通知した場合は既存task ABIが拒否する。ハードウェア停止やボードが保持する記憶域の寿命は、既存の外部taskと同じくボードが管理する。

ConsoleはNUL終端に依存しないUTF-8のpointerとbyte長を受け取る。呼び出し中だけ借用し、ボードが末尾に改行を1つ追加する。標準エラーの出力先を標準出力と同じUARTにすることもできる。Stringを `List[Int]` に変換する必要はない。

## 確認範囲

[`tests/embedded_peripherals`](../tests/embedded_peripherals/mod.rs) は共有handlerと生成C bridgeの回帰試験をまとめる。標準effectのoperationはGPIO 6、UART 3、I²C 6、SPI 4、Clock 2、Console 2の23個である。

| 試験 | 対象 |
| --- | --- |
| `standard_handlers_preserve_values_errors_and_owned_receive_lists` | 全23operationの値・設定・順序、非0 statusからPeripheralErrorへの伝播、Listの範囲・容量・返却長検査、受信値の所有、NULを含むConsole、各処理後のheap返却。 |
| `clock_now_only_does_not_require_the_task_runtime` | nowだけが到達する正常Flowのclock headerを、task型なしでC compile/runできること。 |
| `clock_sleep_notifies_then_resumes_through_the_main_scheduler` | sleepのtask自動mapping、負時間の0への正規化、通知とscheduler再開の分離、解放後のstale通知。 |
| `zero_transfer_capacity_is_rejected` | 転送buffer容量0のInvalidOptions診断。 |
| `malformed_standard_bindings_are_rejected_before_c_emission` | 予約bindingのsymbol・引数・戻り値の不整合の拒否。 |

workspace rootから既存のhost C検証helperを使って再実行する。

```sh
cargo test -p emela-codegen-emb --test emit embedded_peripherals:: --locked
```

この文書は実行結果・成功件数を記録するものではない。C mockは共通word範囲と値の移送を観測するもので、機器設定に応じた7/8/9/16bitの物理driverを実装するものではない。回帰試験は共通ABIと代表入力の契約を確認し、MCUごとのGPIOレジスタ、UART/I²C/SPIの物理動作、実割込みtiming、C stackの最大値を保証しない。CLIの回帰試験とBSPのFlash起動観測は、それぞれ[CLI](cli-embedded.md#検証の範囲)と[ボード確認](boards.md)へ分ける。
