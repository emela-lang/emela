# STM32F446RE での GPIO 割り込み確認

`../target/emela/c-embedded/main.c` と STM32F446RE 用の HAL アダプターを
ARM ELF にリンクし、Renode で NUCLEO-F446RE の GPIO 割り込み動作を確認します。

## 実行

`emela-emb-dev` のルートから次を実行します。

```sh
./examples/embedded-gpio/renode-f446/run.sh
```

スクリプトは Emela C を再生成し、STM32CubeF4 の HAL と ARM GCC で
`examples/embedded-gpio/target/f446-renode/firmware.elf` を作ります。Renode で
ボタンを押して離し、PA5 の出力データレジスターを読み取ります。

## テストを Renode で動かす

```sh
cargo run -p emela-cli -- test --device examples/embedded-gpio
```

`Pome.toml` の `[embedded.test] runner` が `test.sh` を指しています。Emela がテストのファームウェアの C を
`examples/embedded-gpio/target/emela/test-device/` に書き、`test.sh` がそれを `test_board.c` と
ビルドして Renode で動かし、USART2 に出た文字（テストの結果の行）を標準出力に写します。
`test_board.c` は `emela_board_init` で USART2（PA2/PA3、115200 bps）を始め、
`emela_board_console_write` で送ります。HAL とスタートアップのオブジェクトは
`target/f446-test/hal/` に一度だけ作ります。Renode で動かす時間は `EMELA_TEST_RUN_FOR`
（既定は `0:00:02`）で変えられます。

## この確認で使うピン

| Emela 論理ピン | STM32F446RE のピン | 用途 |
| --- | --- | --- |
| 100 | PC13 | NUCLEO-F446RE のユーザーボタン B1、プルアップ入力、両エッジ割り込み |
| 13 | PA5 | LD2 ユーザー LED |

ボタンを押すと PC13 が Low になり、EXTI15_10 の HAL コールバックから
生成された `emela_export_button_changed` を呼びます。生成 C はイベントの
Low レベルを押下状態として PA5 を High にし、ボタンを離すと PA5 を Low に
戻します。このコールバックは割り込みコンテキストで GPIO 書き込みを行います。

ピンの対応と GPIO の操作、EXTI の割り込みハンドラーは、ボード定義から生成した
`emela_board_hal.c` にあります。`board.c` はクロックと `emela_board.h` のフックだけを
実装します。`emela_board_init` は HAL とクロックを始め、`emela_board_defect` は割り込みを
止めて LED を消してそこで止まり、`emela_board_idle` は `main` の後に繰り返し呼ばれて
`WFI` で割り込みを待ちます。

ボードのユーザーボタン B1 は PC13 に接続されます。Renode のボタンモデルも
反転設定し、押下時にLowとなる実機と同じ極性で確認します。

生成 ELF、Renode ログ、実行スクリプトは `examples/embedded-gpio/target/`
以下に出力されます。

## NUCLEO-F446RE への書き込み（Windows 11 + WSL2）

この例の ELF は NUCLEO-F446RE 向けです。別の STM32F446 ボードでは、ピン割り当てや
リンカースクリプトが異なる場合があるため、ボード定義を確認してください。

BareRuby の STM32 バインディングと同じく、WSL 内の OpenOCD から ST-LINK 経由で
SWD 書き込みします。STM32CubeProgrammer CLI は必要ありません。

### ST-LINK を WSL2 に接続

Windows 11、WSL2、`usbipd-win` を用意します。WSL の更新と `usbipd-win` の導入は
[Microsoft の USB 接続手順](https://learn.microsoft.com/ja-jp/windows/wsl/connect-usb)
を参照してください。NUCLEO-F446RE の ST-LINK USB コネクターを PC に接続し、WSL
ターミナルを起動したまま、Windows PowerShell で ST-LINK のバス ID を調べます。

```powershell
usbipd list
```

管理者 PowerShell でデバイスを共有し、通常の PowerShell で WSL に接続します。

```powershell
usbipd bind --busid <BUSID>
usbipd attach --wsl --busid <BUSID>
usbipd list
```

WSL 側で ST-LINK が見えることを確認します。

```sh
lsusb
```

ST-LINK が見えない場合は PowerShell の `usbipd list` で接続状態を確認します。
WSL に接続している間は Windows のアプリから ST-LINK を使えません。

### ELF を書き込む

このリポジトリのルートでビルド兼 Renode 確認スクリプトを実行します。

```sh
./examples/embedded-gpio/renode-f446/run.sh
```

生成先は `examples/embedded-gpio/target/f446-renode/firmware.elf` です。
BareRuby の pinned OpenOCD がインストール済みなら、次のように実行できます。

```sh
OPENOCD=/home/aoi_nemui/bareruby/prototype/bareruby-prototype/.tools/stm32cube/xpack-openocd-0.12.0-7/bin/openocd
"$OPENOCD" \
  -f interface/stlink.cfg \
  -f target/stm32f4x.cfg \
  -c "program examples/embedded-gpio/target/f446-renode/firmware.elf verify reset exit"
```

この呼び出しは BareRuby の STM32 書き込み実装と同じく、ST-LINK/SWD で ELF を書き、
照合してからリセットします。OpenOCD を別の場所に置いた場合は `OPENOCD` をその実行
ファイルのパスに変更してください。OpenOCD のバージョンは次で確認できます。

```sh
"$OPENOCD" --version
```

2026-10-17 に NUCLEO-F446RE で実機書き込みを確認しました。OpenOCD は STM32F446RE
（Flash 512 KiB）を認識し、`Programming Finished`、`Verified OK`、`Resetting Target`
まで完了しました。これで ELF の書き込み・照合・リセットが確認できています。
ボタン B1 を押すと LD2 が点灯し、離すと消灯する動作を実機で確認したところ、
押下時に消灯したため、PC13 の押下極性を Low とするよう修正しました。修正後の
ファームウェアは再書き込みして確認してください。

この手順では Windows 版 STM32CubeProgrammer CLI は使いません。ST-LINK を WSL に
接続し、WSL 内の OpenOCD から書き込みます。Windows 版 CLI は WSL の
`STM32_Programmer_CLI` コマンドとしては登録されていないため、WSL でその名前を
実行しても `command not found` になります。
