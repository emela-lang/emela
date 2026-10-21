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

標準module・共有handlerとC bridgeを提供する。ソースからCoreを作る既存frontendとRustの `compile` / `compile_flow` APIを組み合わせてCを生成し、利用側がC実装とcompile・linkできる。通常のprojectは[CLIの`c-embedded` targetとboard設定](cli-embedded.md)で生成・firmware build・flashまで進められる。入力と生成ファイルの扱いは[実装範囲と対応状況](implementation.md)、STM32F4・ESP32-C3のBSPとstandalone手順、emulatorの観測方法は[ボードpackage](boards.md)を参照する。

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
