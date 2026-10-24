# 組み込みの標準インターフェースとボード実装

組み込みアプリは、周辺機器の操作を`Std.Embedded.*`のeffectとして記述する。ボードで実行するときは`Std.Embedded.Board`のhandlerを選び、ボードのC実装へ接続する。アプリごとにGPIO・UART等の`@external`やadapter対応表を定義する必要はない。

## アプリ、handler、ボードの役割

| 場所 | 役割 |
| --- | --- |
| アプリの`.emel` | 計算、状態の保持、操作する機器とタイミングの判断 |
| `Std.Embedded.Gpio`等 | 操作の型とeffectを宣言する共通インターフェース |
| `Std.Embedded.Board` | 標準effectをボードABIへ接続する共有handler |
| ボードの`.emel`モジュール | 論理ピン・機器IDの割り当て、機器の設定、独自センサ等の接続 |
| ボードのCコード | レジスタやHALの操作、割り込み、非同期処理の完了通知 |
| Rustの `Options` | 固定heap・String・転送buffer・poolの容量、独自の外部関数に必要なadapter設定 |

例えばGPIOは次の経路で動く。

```text
(use Gpio).write(pin, High)
        ↓ with BoardGpio
標準handlerとbackendの共有bridge
        ↓ emela_gpio_write
ボードのC実装 → GPIOレジスタ
```

共通effectの宣言自体はボード実装に依存しない。独自handlerで操作を模擬すれば、JSやホスト側のsourceテストにも使える。`Std.Embedded.Board` の共有handlerを選んだ場合に、この組み込みC bridgeと利用側のC実装が必要になる。

共通handlerの内部にもCとの境界はある。そのABI変換とadapter登録はbackendが行う。独自デバイス・既存Cライブラリに使う一般の`@external`とadapter設定も引き続き利用できる。

## 利用できるeffect

| importするモジュール | 主な操作 | ボード用handler |
| --- | --- | --- |
| `Std.Embedded.Gpio` | ピン設定、読み書き、反転、割り込み元の設定 | `BoardGpio` |
| `Std.Embedded.Uart` | UART設定、データの送受信 | `BoardUart` |
| `Std.Embedded.I2c` | 設定、master送受信、メモリアクセス、応答確認 | `BoardI2c` |
| `Std.Embedded.Spi` | 設定、送信、受信、同時送受信 | `BoardSpi` |
| `Std.Clock` | `sleep`、`now` | `BoardClock` |
| `Std.Console` | 通常・エラーの行出力 | `BoardConsole` |

handlerは`Std.Embedded.Board`からimportし、`with`で適用する。GPIOの論理ピン番号とUART等のIDがどの物理機器に対応するかは、ボード実装が定める。GPIOの割り込み設定は割り込み元の設定までで、任意のEmela関数をISRで実行するAPIではない。

UART・I²C・SPIのエラーは`Std.Embedded.Peripheral.PeripheralError(code)`で扱う。C境界では0が成功、−1（不正な引数）と−2（転送buffer容量超過）が共有bridgeの予約codeである。その他の非0 codeの意味はボード実装が定める。GPIOの操作には回復可能な失敗型を定義していない。不正な機器指定等をどう診断するかは、ボード実装の契約に従う。

## 最小の利用例

```emela
import Std.Embedded.Gpio.{Gpio, GpioPin, OutputPushPull, NoPull, High}
import Std.Embedded.Board.{BoardGpio}

pub fn set_output() {
  with BoardGpio {
    pin = GpioPin(5)
    gpio = use Gpio
    gpio.configure(pin, OutputPushPull, NoPull)
    gpio.write(pin, High)
  }
}
```

アプリは共通effectを使い、利用側のC実装が論理ピン5の物理的な接続を定める。全ての物理ボード用driverを標準ライブラリが提供するわけではない。使うhandlerのほかのoperationも生成runtimeから到達扱いになる場合があるため、C側は生成headerとlink結果に従って必要な関数を用意する。

このPRは標準module・共有handlerとC bridgeまでを提供する。ソースからCoreを作る既存frontendとRustの `compile` / `compile_flow` APIを組み合わせてCを生成し、利用側がC実装とcompile・linkする。入力と生成ファイルの扱いは[実装範囲と対応状況](implementation.md)を参照する。CLIの組み込みtarget/board選択、設定ファイル、BSP、build/flashコマンド、emulator・ボードexampleは後続の統合範囲である。

## 転送データと容量

UART・I²C・SPIの転送データは`List[Int]`で表す。共有bridgeは固定長の`uint16_t`転送bufferを使い、capacityと要素の範囲0〜65535を確認してからボードへ渡す。実際に使用できる要素の幅は機器の設定に依存し、8bit設定のUARTなら0〜255である。設定に対して不正な値はボード側で診断する。転送要素はUTF-8の文字列やUnicodeのcode pointと同じものではない。

`Options::with_peripheral_buffer_capacity` は、このbufferに保持できる要素数を指定する。既定は256、0は使えない。大きな転送にはアプリでの分割や容量の調整が必要である。設定範囲は1〜65,535 wordであり、buffer自体はC stackに置く。具体的な生成ABIと所有権・エラーcodeの契約は[標準周辺機器bridge](embedded-peripherals.md)と生成headerを参照する。

ログを出す目的では`Console.print_line`を使える。長さ付きStringを直接ボードへ渡すため、ログを一度`List[Int]`へ変換する必要がない。

## 待機と割り込み

`BoardClock.sleep`は外部taskとして開始し、ISRからの完了通知後、main側のschedulerでEmelaの処理を再開する。0以下の待機時間は0として扱い、ボードはtaskの完了通知だけを送り、その呼び出し中にアプリを直接再開しない。ボード側は生成されたtask API・event lockの契約に従う。ISRでString生成、Emela heap操作、アプリの再開を行わない。

`Clock.now`はUnix epochからのミリ秒を表す`Instant`を返す。実時間の分からないボードでは、起動からの経過時間を黙ってUnix時刻として扱わず、RTCや設定したepochとの対応をボード側で定める。

## 生成と確認範囲

標準handlerの予約bindingはbackendが識別するため、GPIO等の共通operation用に `with_external_adapter` や `with_task_adapter` を利用者が列挙する必要はない。独自デバイスの外部関数には従来の明示mappingを使う。

hostの回帰試験は、Emela sourceから共有handlerをlowerし、生成headerに従うC mockをlinkして値・status・所有権・通知の契約を確認する。対象と再実行方法は[標準周辺機器bridgeの確認範囲](embedded-peripherals.md#確認範囲)を参照する。物理機器や実割込みtiming、任意のボードの資源上限を検証するものではない。

## API の一覧（`crates/emela-core/lib/std/embedded/`）

組み込みの周辺機器を effect として宣言する共通インターフェース。GPIO・UART・I²C・SPI の型と操作を持ち、`import Std.Embedded.Gpio.{Gpio, GpioPin, PinState}` のように使う。アプリは利用者が書いた handler、または組み込み C backend に接続する共有 handler で実行する。これらの定義自体は特定の MCU や HAL に依存しない。

UART・I²C・SPI の転送は同期 API。`List[Int]` の各要素は転送単位を表し、`timeout_ms` はミリ秒の `Int64`。データ幅、値の範囲、利用可能な周辺機器、timeout の扱いはボード実装の契約に従う。転送操作は割り込みコンテキストから呼び出さない。

### Std.Embedded.Peripheral（`std/embedded/peripheral.emel`）

| 宣言 | 内容 | 説明 |
|---|---|---|
| `PeripheralError` | `error PeripheralError(code: Int)` | UART・I²C・SPI で共通の実行時エラー。共有 bridge は `-1` を引数不正、`-2` を転送容量超過に予約し、ほかの非ゼロ値はボード実装が定める |

予約コード、固定転送容量、生成 C API の詳細は [組み込み周辺機器の接続](embedded-peripherals.md) を参照する。

### Std.Embedded.Gpio（`std/embedded/gpio.emel`）

`GpioPin(id: Int)` は論理ピン番号を保持する型で、物理ピンとの対応はボード定義が持つ。`PinState` は `Low / High`、`Pull` は `NoPull / Up / Down`、`GpioMode` は `Input / OutputPushPull / OutputOpenDrain / Analog`、`Edge` は `Rising / Falling / Both`。`GpioEvent(pin: GpioPin, level: PinState)` は入力イベントを表す。

| `Gpio` の操作 | シグネチャ | 説明 |
|---|---|---|
| `configure` | `(pin: GpioPin, mode: GpioMode, pull: Pull) -> ()` | ピンのモードとプル抵抗を設定 |
| `read` | `(pin: GpioPin) -> PinState` | 入力レベルを読む |
| `write` | `(pin: GpioPin, state: PinState) -> ()` | 出力レベルを設定 |
| `toggle` | `(pin: GpioPin) -> ()` | 出力レベルを反転 |
| `enable_interrupt` | `(pin: GpioPin, edge: Edge) -> ()` | ボード側でピンの割り込み元と検出エッジを設定・有効化 |
| `disable_interrupt` | `(pin: GpioPin) -> ()` | 割り込み元を無効化 |

GPIO の操作は `PeripheralError` を返さない。`enable_interrupt` は Emela の callback 登録やビルド時のハンドラー指定を提供する操作ではなく、割り込み元をボード側で設定する。割り込みを有効にする操作と、割り込み内で Emela の処理を実行することは別の契約である。

### Std.Embedded.Uart（`std/embedded/uart.emel`）

`UartId` は `Uart0`〜`Uart7`。`UartConfig` は `baud_rate: Int`、`data_bits: UartDataBits`、`stop_bits: UartStopBits`、`parity: Parity`、`direction: UartDirection`、`flow_control: FlowControl`、`oversampling: Oversampling` を保持する。列挙値はそれぞれ `Bits7 / Bits8 / Bits9`、`Half / One / OneAndHalf / Two`、`NoParity / Even / Odd`、`Receive / Transmit / Both`、`NoFlowControl / Rts / Cts / RtsCts`、`By8 / By16`。データ幅にはパリティビットを含めない。

以下の操作はすべて `fails PeripheralError`。

| `Uart` の操作 | シグネチャ | 説明 |
|---|---|---|
| `configure` | `(id: UartId, config: UartConfig) -> ()` | UART を設定 |
| `transmit` | `(id: UartId, data: List[Int], timeout_ms: Int64) -> ()` | データを送信 |
| `receive` | `(id: UartId, size: Int, timeout_ms: Int64) -> List[Int]` | データを受信 |

### Std.Embedded.I2c（`std/embedded/i2c.emel`）

マスター操作を提供する。`I2cId` は `I2c1 / I2c2 / I2c3`。`I2cConfig` は `clock_speed_hz: Int`、`addressing: I2cAddressing`、`duty_cycle: I2cDutyCycle` を保持する。`I2cAddressing` は `SevenBit / TenBit`、`I2cDutyCycle` は `Standard / Fast16To9`、`MemoryAddressSize` は `Bits8 / Bits16`。

以下の操作はすべて `fails PeripheralError`。

| `I2c` の操作 | シグネチャ | 説明 |
|---|---|---|
| `configure` | `(id: I2cId, config: I2cConfig) -> ()` | I²C を設定 |
| `master_transmit` | `(id: I2cId, address: Int, data: List[Int], timeout_ms: Int64) -> ()` | 指定したアドレスへ送信 |
| `master_receive` | `(id: I2cId, address: Int, size: Int, timeout_ms: Int64) -> List[Int]` | 指定したアドレスから受信 |
| `memory_write` | `(id: I2cId, address: Int, memory_address: Int, address_size: MemoryAddressSize, data: List[Int], timeout_ms: Int64) -> ()` | レジスタなどへ書き込み |
| `memory_read` | `(id: I2cId, address: Int, memory_address: Int, address_size: MemoryAddressSize, size: Int, timeout_ms: Int64) -> List[Int]` | レジスタなどから読み込み |
| `is_device_ready` | `(id: I2cId, address: Int, trials: Int, timeout_ms: Int64) -> Bool` | 接続先が応答するか確認 |

### Std.Embedded.Spi（`std/embedded/spi.emel`）

チップセレクトは GPIO で操作する。`SpiId` は `Spi1`〜`Spi6`。`SpiConfig` は `mode: SpiMode`、`direction: SpiDirection`、`data_size: SpiDataSize`、`polarity: ClockPolarity`、`phase: ClockPhase`、`prescaler: SpiPrescaler`、`bit_order: BitOrder` を保持する。列挙値はそれぞれ `Master / Slave`、`TwoLine / TwoLineReceiveOnly / OneLine`、`Bits8 / Bits16`、`IdleLow / IdleHigh`、`FirstEdge / SecondEdge`、`Div2 / Div4 / Div8 / Div16 / Div32 / Div64 / Div128 / Div256`、`MostSignificantFirst / LeastSignificantFirst`。

以下の操作はすべて `fails PeripheralError`。

| `Spi` の操作 | シグネチャ | 説明 |
|---|---|---|
| `configure` | `(id: SpiId, config: SpiConfig) -> ()` | SPI を設定 |
| `transmit` | `(id: SpiId, data: List[Int], timeout_ms: Int64) -> ()` | データを送信 |
| `receive` | `(id: SpiId, size: Int, timeout_ms: Int64) -> List[Int]` | データを受信 |
| `transfer` | `(id: SpiId, data: List[Int], timeout_ms: Int64) -> List[Int]` | 送受信を同時に行う |

### Std.Embedded.Board（`std/embedded/board.emel`）

組み込み C backend に接続する共通 handler。アプリは `with BoardGpio, BoardClock { ... }` のように選択し、C 側の関数名や adapter をアプリごとに指定する必要はない。`emela-codegen-emb` の Rust API が必要な C 接続を生成し、利用者が生成されたインターフェースの C 実装とビルド・リンクを用意する。CLI の組み込みターゲット選択、同梱 BSP、書き込みコマンドは後続の開発範囲である。

| handler | 実装する effect | 動作 |
|---|---|---|
| `BoardGpio` | `Std.Embedded.Gpio.Gpio` | 論理ピンをボード実装へ渡す |
| `BoardUart` | `Std.Embedded.Uart.Uart` | UART の同期操作。ボードの非ゼロ status を `PeripheralError(code)` に変換 |
| `BoardI2c` | `Std.Embedded.I2c.I2c` | I²C の同期操作。同じエラー変換を行う |
| `BoardSpi` | `Std.Embedded.Spi.Spi` | SPI の同期操作。同じエラー変換を行う |
| `BoardClock` | `Std.Clock.Clock` | `sleep` は外部 task の完了まで suspend。`now` はボードから得た Unix エポックのミリ秒を `Instant` に変換 |
| `BoardConsole` | `Std.Console.Console` | 長さ付きの String をボードへ渡す。改行はボード実装で付ける |

`BoardClock.now` の時刻基準は `Std.Clock` と同じ Unix エポック。起動後の経過時間と実時刻の対応はボード実装が供給する。同期転送や Console の実行時間もボード実装に従う。
