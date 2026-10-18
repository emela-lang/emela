# 組み込み C の GPIO 割り込み例

`src/main.emel` は GPIO effect で LED とボタンを初期化し、`button_changed` を割り込みハンドラーとして定義する例です。`button_changed` は公開関数としてC出力に含まれ、`emela_embedded.h` にボード側から呼べる宣言が出ます。
`GpioPin(13)` と `GpioPin(100)` は論理 ID で、`Pome.toml` で選んだボード定義（[`boards/stm32f4/nucleo-f446re.toml`](../../boards/stm32f4/nucleo-f446re.toml)）が NUCLEO-F446RE の PA5（D13、LED LD2）と PC13（ユーザーボタン B1）に対応付けます。B1 は押下時 Low のため、入力をプルアップし、イベントの Low レベルを押下として扱います。

論理ピン `100` の割り込みと `button_changed` の結び付けは `Pome.toml` の `[embedded.gpio_interrupts]` に書きます。`emela build` はボード定義から HAL アダプター（`emela_board_hal.c`）を生成し、EXTI の割り込みハンドラーと HAL コールバックから `button_changed` を直接呼びます。実行時の関数ポインター登録や、`main` からの割り込みポーリングは使いません。

| 論理ピン | エッジ | Emela ハンドラー |
| --- | --- | --- |
| `GpioPin(100)` | `Edge.Both`（`enable_interrupt` で指定） | `button_changed` |

`renode-f446/` には NUCLEO-F446RE のクロックとフックの C（`board.c`）と Renode 実行スクリプトがあります。`./examples/embedded-gpio/renode-f446/run.sh` で Emela C を生成して ELF にリンクし、PC13 の両エッジ割り込みから PA5 を操作する経路を確認できます。

`src/main.emel` の最後にはテストがあり、GPIO をテスト用の handler（`RecordGpio`）に差し替えて `button_changed` が書く値を確かめます。`emela test examples/embedded-gpio` はホストで、`emela test --device examples/embedded-gpio` は `Pome.toml` の `[embedded.test] runner`（`renode-f446/test.sh`）で Renode の NUCLEO-F446RE の上で動かします。テストの State が List を使うので、`Pome.toml` で `heap_bytes` を用意しています（`main` はヒープを使いません）。


`main` と割り込みの経路はヒープを使いません。`Option[PinState]` や `GpioEvent` は C の構造体として値で受け渡し、`Option.map(is_pressed)` は `is_pressed` を直接呼ぶ形に特殊化します。ハンドラーから到達する関数は再帰できません。ボード側は GPIO の操作に加えて、defect のフック `emela_board_defect` と待機のフック `emela_board_idle`（`emela_board.h`）を実装します。GPIO の操作は ISR 内で安全かつ非ブロッキングにする必要があります。対応範囲は [`docs/design/c-embedded-codegen.md`](../../docs/design/c-embedded-codegen.md) を参照してください。

`Option.map` を直接の呼び出しにする仕組みは [`docs/design/c-embedded-architecture.md`](../../docs/design/c-embedded-architecture.md) の「② 静的特殊化」にあります。

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
./examples/embedded-gpio/renode-f446/run.sh
```

Emela C の生成、ARM GCC でのビルド、Renode での確認までを行い、`examples/embedded-gpio/target/f446-renode/firmware.elf` を作ります。ツールは `/home/aoi_nemui/bareruby/stm32_renode/.tools` のものを使います（別の場所なら `STM32_RENODE_ROOT=/path/to/stm32_renode` を前に付けます）。

### 3. 書き込む

```sh
OPENOCD=/home/aoi_nemui/bareruby/prototype/bareruby-prototype/.tools/stm32cube/xpack-openocd-0.12.0-7/bin/openocd
"$OPENOCD" -f interface/stlink.cfg -f target/stm32f4x.cfg -c "program examples/embedded-gpio/target/f446-renode/firmware.elf verify reset exit"
```

`Programming Finished`、`Verified OK`、`Resetting Target` と出れば書き込めています。書き込み後にリセットされ、プログラムが始まります。もう一度最初から動かすときは、ボードの黒いリセットボタン（B2）を押します。

### 4. 確かめる

ユーザーボタン B1（青いボタン）を押すと LED LD2（緑）が点き、離すと消えます。

テストを Renode で動かすコマンドは次のとおりです（実機は使いません）。

```sh
cargo run -p emela-cli -- test --device examples/embedded-gpio
```

書き込みの詳しい説明（OpenOCD の確認など）は [`renode-f446/README.md`](renode-f446/README.md) にあります。

### うまくいかないとき

- `Command '' not found`: `OPENOCD=...` の行を実行していません。2行とも実行します。
- `Error: open failed`（OpenOCD）: ST-LINK が WSL に渡っていません。PowerShell で `usbipd attach --wsl --busid ...` をやり直します。WSL に渡している間は、Windows のアプリから ST-LINK を使えません。
