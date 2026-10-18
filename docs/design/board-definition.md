# 組み込みボード定義

ボード定義は、Emela の論理ピンと論理周辺機器（`Uart0`、`I2c1`、`Spi1`）を、MCU の実際の資源に結び付ける。`Std.Embedded` の API には、GPIO のポートや STM32 HAL の型を出さない。どの端子に何を配線したかはボードの事情であり、プログラムの書き方を変えずにボードを差し替えられるようにするためである。

例は [`NUCLEO-F446RE`](../../boards/stm32f4/nucleo-f446re.toml)。

## 役割

- `Std.Embedded.Gpio` の `GpioPin(id)` を、ボード上の端子名と MCU のポート・ピン番号に対応付ける。
- `Uart0` などの論理周辺機器を、周辺機器のインスタンスと信号ピンに対応付ける。
- 周辺機器が使うピンを記録し、GPIO と周辺機器の重なりをビルド時に検査できるようにする。
- HAL 固有の設定は HAL のアダプターの側に置き、`Std.Embedded` の API には出さない。

## 形式

TOML で書く。

| 項目 | 内容 |
| --- | --- |
| `format_version` | 定義の形式の版（今は 1） |
| `[board]` | `id`、`name`、`mcu`、`hal`。`hal = "STM32CubeF4"` なら HAL のアダプターを生成する |
| `[logical_pins."<id>"]` | 論理ピン ID ごとに、`label`（ボード上の名前）、`port`（`"A"` のような1文字）、`pin`（0〜15）。`onboard_device` は説明のためだけに書ける |
| `[peripheral_routes.uartN]` | `instance`（`"USART2"` など）、`tx_pin`、`rx_pin`、`alternate_function` |
| `[peripheral_routes.i2cN]` | `instance`、`scl_pin`、`sda_pin`、`alternate_function` |
| `[peripheral_routes.spiN]` | `instance`、`sck_pin`、`miso_pin`、`mosi_pin`、`alternate_function` |
| `[backend.<name>]` | HAL 固有の設定。今は読まない |

ルートのピンは、物理ピンではなく `[logical_pins]` の ID で書く。`uart0` は `Std.Embedded.Uart` の `Uart0` に、`i2c1` は `I2c1` に対応する。

NUCLEO-F446RE の定義では、論理ピン `0`〜`15` を Arduino の D0〜D15 とし、ユーザーボタン B1 を `100` とした。たとえば `GpioPin(13)` は D13、つまり PA5 で、基板上の LED LD2 につながる。`uart0` は ST-LINK の仮想 COM ポートの USART2（D1 = PA2 が TX、D0 = PA3 が RX）である。

## プロジェクトからの選び方

プロジェクトは `Pome.toml` でボードを選び、GPIO の割り込みと割り込み入口（`GpioEvent` を取る公開関数）を結び付ける。結び付けは Emela の構文ではなく設定に書く。Emela の書き方を変えないためである。

```toml
[build]
target = "c-embedded"
board = "../../boards/stm32f4/nucleo-f446re.toml"   # プロジェクトからの相対パス

[embedded.gpio_interrupts]
100 = "button_changed"   # 論理ピン = 割り込み入口
```

## 検査

ボードを選んだビルドでは、C を出力する前に次を検査し、論理ピンや周辺機器の名前を示してエラーにする。

- プログラムが作る `GpioPin(n)` の n は定数でなければならず、ボードにその論理ピンがなければならない。値が実行時まで分からないピン ID は、ボードを選ばなくても拒否する。
- プログラムが使う周辺機器（プログラムが名指す `Uart0`、`I2c1`、`Spi1`）には、ボードにルートがなければならない。
- 使う周辺機器のルートのピンと、GPIO として使う論理ピンが重なってはならない（ルートどうしも）。
- 割り込みの結び付けは、関数が割り込み入口であること、論理ピンがボードにあること、同じ EXTI の線（STM32 ではピン番号。`PA7` と `PC7` は同じ線）を2つのピンに結び付けないことを検査する。

指定した MCU ピンがその周辺機器の信号と代替機能に対応しているか、基板上のはんだブリッジなどの条件を満たすかは、まだ検査しない。

## 生成するアダプター

`hal = "STM32CubeF4"` のボードでは、`emela_board_hal.c` を生成する。

- 論理ピンから物理ピンへの変換と、`emela_gpio_*` の実装（HAL の呼び出し）。
- 結び付けたピンの EXTI の割り込みハンドラーと、HAL のコールバックから割り込み入口への中継。
- プログラムが使う UART、I2C、SPI のハンドルの初期化と、`emela_uart_*`、`emela_i2c_*`、`emela_spi_*` の実装（HAL のポーリングの API）。

ボード側の C に残るのは、クロックと HAL の開始（`emela_board_init`）と、`emela_board.h` のフック（defect、待機、Console など）だけである。

## 今後

- MCU の能力の表（ピンごとに使える代替機能）と、基板固有の端子・配線を分けて持ち、代替機能やはんだブリッジを検査する。今はボードが1つなので、同じファイルに置いている。
- STM32CubeF4 以外の HAL のアダプター。

## 参考資料

- [STM32 Nucleo-64 boards user manual UM1724](https://www.st.com/resource/en/user_manual/dm00105823.pdf)
- [STM32F446xC/xE datasheet](https://www.st.com/resource/en/datasheet/stm32f446re.pdf)
