# 組み込み C の IMU（ICM-42688-P）の例

`src/main.emel` は、6 軸 IMU の ICM-42688-P を I2C で読み、加速度（g）、角速度（dps）、温度（℃）を 100 ms ごとに Console に書く例です。BareRuby の `prototype/read_imu_sensor` と同じ流れ（WHO_AM_I を確かめ、電源を入れ、測定範囲を設定し、14 バイトをまとめて読む）を Emela で書いています。

- `src/icm42688.emel` はチップのドライバーです。レジスターの読み書きを `Registers` effect にしてあり、どのバスにつないだかは知りません。
- `src/main.emel` は、その effect を I2C1 で実装する handler（`ImuOnI2c`）を与えます。SPI でつなぐときは、SPI で読み書きする handler に差し替えるだけで、ドライバーは変わりません。

出力は次のような形です（値は置き方や個体で変わります）。

```text
who_am_i: 71
accel 0.02 -0.01 1.00 g  gyro 0.12 -0.31 0.06 dps  temp 27.45 C
accel 0.02 -0.01 1.00 g  gyro 0.09 -0.27 0.03 dps  temp 27.46 C
...
```

測定範囲は ±4 g と ±500 dps です（`main.emel` の `ACCEL_RANGE` と `GYRO_RANGE`）。値は小数第 2 位まで表示します。

## 配線（NUCLEO-F446RE）

| ICM-42688-P のモジュール | NUCLEO-F446RE |
| --- | --- |
| VCC（VDD、VDDIO） | 3V3 |
| GND | GND |
| SCL | D15（PB8、I2C1_SCL） |
| SDA | D14（PB9、I2C1_SDA） |
| AD0（SDO） | 3V3（アドレス 0x69）。GND にしたときはアドレスが 0x68 なので、`main.emel` の `ADDRESS` を 104 にする |
| CS | 3V3（High で I2C になる。モジュールで引き上げてあれば不要） |

I2C の線にはプルアップが要ります。多くのモジュールには載っています。アダプターはマイコン内部のプルアップも有効にしますが、弱いので、モジュールにプルアップがなければ 4.7 kΩ ほどを 3V3 に付けてください。400 kHz で通信が不安定なときは、`main.emel` の `clock_speed_hz` を 100000 にしてください。

## ビルドと Renode での確認

```sh
./examples/embedded-imu/renode-f446/run.sh
```

スクリプトは Emela C を生成し、`renode-f446/board.c`（クロック、USART2 の Console、Clock のフック）と STM32CubeF4 の HAL（UART、I2C）で `examples/embedded-imu/target/f446-renode/firmware.elf` を作ります。`Float.round` が C の `round` を使うので、`-lm` でリンクします。

続けて Renode で、I2C1 のアドレス 0x69 に ICM-42688-P のモデル（`renode-f446/Icm42688.cs`）をつないで 0.5 秒動かし、USART2 の出力を確かめます。モデルは、決まった物理量（0.25 g、-0.5 g、1 g、10 dps、-20 dps、0.5 dps、30 ℃）を、プログラムが設定した測定範囲で生の値にして返します。電源を入れる前は、実機と同じく -32768 を返します。Renode で確かめるのはレジスターの読み書きとデータの流れだけで、実際のチップの細かな振る舞い（起動にかかる時間、フィルターなど）はモデルにありません。

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
./examples/embedded-imu/renode-f446/run.sh
```

Emela C の生成、ARM GCC でのビルド、Renode での確認までを行い、`examples/embedded-imu/target/f446-renode/firmware.elf` を作ります。ツールは `/home/aoi_nemui/bareruby/stm32_renode/.tools` のものを使います（別の場所なら `STM32_RENODE_ROOT=/path/to/stm32_renode` を前に付けます）。

### 3. シリアルの出力を開く

出力は ST-LINK の仮想 COM ポート（115200 bps、8N1）に出ます。書き込む前に、別の WSL のターミナルで開いておきます。

```sh
picocom -b 115200 --imap lfcrlf /dev/ttyACM0
```

ファームウェアは改行を LF だけで送ります。`--imap lfcrlf` は、受け取った LF を端末で行頭に戻して表示するための指定です（付けないと行が階段状にずれます）。終わるときは `Ctrl-A` に続けて `Ctrl-X` を押します。

### 4. 書き込む

```sh
OPENOCD=/home/aoi_nemui/bareruby/prototype/bareruby-prototype/.tools/stm32cube/xpack-openocd-0.12.0-7/bin/openocd
"$OPENOCD" -f interface/stlink.cfg -f target/stm32f4x.cfg -c "program examples/embedded-imu/target/f446-renode/firmware.elf verify reset exit"
```

`Programming Finished`、`Verified OK`、`Resetting Target` と出れば書き込めています。書き込み後にリセットされ、プログラムが始まります。もう一度最初から動かすときは、ボードの黒いリセットボタン（B2）を押します。

### 5. 確かめる

picocom に次のような行が 100 ms ごとに出ます。ボードを傾けると加速度が、回すと角速度が変わります。

```text
who_am_i: 71
accel 0.02 -0.01 1.00 g  gyro 0.12 -0.31 0.06 dps  temp 27.45 C
```

- `who_am_i:` の値が 71 でなく、`ICM-42688-P not found` と出たら、そのアドレスで別のチップが応答しています。
- `I2C error 6` と出たら、そのアドレスで応答がありません。配線、AD0 とアドレスの組み合わせ、電源を確かめてください。
- 止まったときに `defect: …` と出たら、Emela の defect です（ヒープの不足など）。

### うまくいかないとき

- `Command '' not found`: `OPENOCD=...` の行を実行していません。2行とも実行します。
- `Error: open failed`（OpenOCD）: ST-LINK が WSL に渡っていません。PowerShell で `usbipd attach --wsl --busid ...` をやり直します。WSL に渡している間は、Windows のアプリから ST-LINK を使えません。
- picocom に何も出ない: `/dev/ttyACM0` が別の番号（`/dev/ttyACM1` など）になっていないか `ls /dev/ttyACM*` で確かめます。picocom を開いてからリセットボタンを押すと、最初の行から見えます。
