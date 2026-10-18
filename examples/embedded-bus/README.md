# 組み込み C の I2C と SPI の例

`src/main.emel` は、I2C と SPI の機器と話し、結果を UART に1行ずつ書く例です。最後に UART から1行を読み、`echo` を付けて書き返します。`I2c1`、`Spi1`、`Uart0` は、`Pome.toml` で選んだボード定義（[`boards/stm32f4/nucleo-f446re.toml`](../../boards/stm32f4/nucleo-f446re.toml)）の `[peripheral_routes.*]` で、NUCLEO-F446RE の次のピンに対応します。

| Emela | STM32F446RE | ピン |
| --- | --- | --- |
| `I2c1` | I2C1 | D15 = PB8（SCL）、D14 = PB9（SDA） |
| `Spi1` | SPI1 | D13 = PA5（SCK）、D12 = PA6（MISO）、D11 = PA7（MOSI） |
| `GpioPin(10)` | GPIO | D10 = PB6（SPI のチップセレクト。Low で選ぶ） |
| `Uart0` | USART2 | D1 = PA2（TX）、D0 = PA3（RX）。ST-LINK の仮想 COM ポート |

SPI のチップセレクトは、プログラムが GPIO で操作します（`exchange`）。SPI1 の SCK は LED LD2 と同じ D13 なので、この例では LED を使いません。送受信のデータは `List[Int]` なので、`Pome.toml` の `heap_bytes` でヒープを用意しています。I2C のアドレスはシフトしない 7 ビットです。

## Renode で動かす

```sh
./examples/embedded-bus/renode-f446/run.sh
```

相手の機器は、BareRuby の `stm32_renode` の Renode モデル（`$STM32_RENODE_ROOT/models`）を使います。

- I2C1 のアドレス 118 に `RegisterSensor`。16 個のレジスターは 240〜255 で始まり、書き込みの1バイト目がレジスターを選びます。レジスター 64 に 0, 1, 2, … と書くとセンサーが数え、レジスター 65 を読むと「食い違いの数/受け取った数」を返します。
- SPI1 に `SpiCheckDevice`。受け取ったバイトを 165 と XOR して返し、受け取ったバイト数とチップセレクトの区切りの数を数えます。Renode の `SPIMultiplexer` を通し、PB6 をチップセレクトにつなぎます。

スクリプトは、Emela C を生成して STM32CubeF4 の HAL（UART、I2C、SPI のモジュール）と ELF にリンクし、Renode で動かします。0.3 秒の時点で USART2 に `ping` と改行を届けます。最後に、USART2 に出た行と、SPI の機器が受け取ったバイト数（6）と区切りの数（2）を確かめます。

```text
ready
i2c ready yes
i2c missing no
i2c read 240 241 242 243
i2c write 241 18 52
i2c score 000/032
spi 165 164 0 90
spi 255 170
echo ping
```

Renode のモデルが確かめるのは、HAL のレジスター操作とデータの流れです。SCL や SCK の波形、プルアップなどの電気的な振る舞いは確かめていません。実機では I2C の線に外付けのプルアップが要ります（アダプターは内部プルアップも有効にしますが、弱いので）。

## 実機で動かす（NUCLEO-F446RE、Windows 11 + WSL2）

コマンドはすべて、WSL の `emela-emb-dev` のルートで実行します（PowerShell と書いたものだけ Windows 側）。

### 1. ST-LINK を WSL につなぐ

NUCLEO-F446RE を USB でつなぎ、WSL のターミナルを開いたまま、Windows の PowerShell で ST-LINK のバス ID を調べます。

```powershell
usbipd list
```

`ST-Link Debug, USB Mass Storage, USB Serial Device` のような行の `BUSID`（`2-3` など）を使います。初回だけ、管理者の PowerShell で共有します。

```powershell
usbipd bind --busid 2-3
```

つなぐたびに、通常の PowerShell で WSL に渡します。

```powershell
usbipd attach --wsl --busid 2-3
```

WSL で見えることを確かめます（`STMicroelectronics ST-LINK/V2.1` の行と `/dev/ttyACM0` があればよい）。

```sh
lsusb
ls /dev/ttyACM*
```

### 2. ビルドする

```sh
./examples/embedded-bus/renode-f446/run.sh
```

Emela C の生成、ARM GCC でのビルド、Renode での確認までを行い、`examples/embedded-bus/target/f446-renode/firmware.elf` を作ります。ツールは `/home/aoi_nemui/bareruby/stm32_renode/.tools` のものを使います（別の場所なら `STM32_RENODE_ROOT=/path/to/stm32_renode` を前に付けます）。

### 3. シリアルの出力を開く

出力は ST-LINK の仮想 COM ポート（115200 bps、8N1）に出ます。書き込む前に、別の WSL のターミナルで開いておきます。

```sh
picocom -b 115200 --imap lfcrlf /dev/ttyACM0
```

ファームウェアは改行を LF だけで送ります。`--imap lfcrlf` は、受け取った LF を端末で行頭に戻して表示するための指定です（付けないと行が階段状にずれます）。終わるときは `Ctrl-A` に続けて `Ctrl-X` を押します。

### 4. 書き込む

```sh
OPENOCD=/home/aoi_nemui/bareruby/prototype/bareruby-prototype/.tools/stm32cube/xpack-openocd-0.12.0-7/bin/openocd
"$OPENOCD" -f interface/stlink.cfg -f target/stm32f4x.cfg -c "program examples/embedded-bus/target/f446-renode/firmware.elf verify reset exit"
```

`Programming Finished`、`Verified OK`、`Resetting Target` と出れば書き込めています。書き込み後にリセットされ、プログラムが始まります。もう一度最初から動かすときは、ボードの黒いリセットボタン（B2）を押します。

### 5. 確かめる

この例は Renode の機器のモデルを相手にする前提で書いています。実機で I2C1 と SPI1 に何もつないでいなければ、picocom にはたとえば次のように出て、最初の I2C の読み出しの失敗（`main` まで届いた `PeripheralError` は defect になる）でそこで止まります。線の状態によっては `ready` の後ですぐ止まることもあります。

```text
ready
i2c ready no
i2c missing no
```

それぞれの機器をつなぐ場合は、アドレス、レジスター、SPI の応答がこの例のモデル（上の説明）とは違うので、`src/main.emel` をその機器に合わせて書き換えます。

### うまくいかないとき

- `Command '' not found`: `OPENOCD=...` の行を実行していません。2行とも実行します。
- `Error: open failed`（OpenOCD）: ST-LINK が WSL に渡っていません。PowerShell で `usbipd attach --wsl --busid ...` をやり直します。WSL に渡している間は、Windows のアプリから ST-LINK を使えません。
- picocom に何も出ない: `/dev/ttyACM0` が別の番号（`/dev/ttyACM1` など）になっていないか `ls /dev/ttyACM*` で確かめます。picocom を開いてからリセットボタンを押すと、最初の行から見えます。
