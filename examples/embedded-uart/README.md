# 組み込み C の UART 送信例

`src/main.emel` は UART effect で `Hello` と `count 0`〜`count 2` を送る例です。`Uart0` は `Pome.toml` で選んだボード定義（[`boards/stm32f4/nucleo-f446re.toml`](../../boards/stm32f4/nucleo-f446re.toml)）の `[peripheral_routes.uart0]` で NUCLEO-F446RE の USART2（D1 = PA2 が TX、D0 = PA3 が RX。ST-LINK の仮想 COM ポート）に対応します。

送るデータは `List[Int]` なので、`Pome.toml` の `[embedded.memory] heap_bytes` でヒープを用意しています。UART の操作は `PeripheralError` で失敗しうるので、`main` は `fails PeripheralError` にしています（`main` まで届いた失敗は defect になります）。

`emela build` は、ボード定義から UART の HAL アダプター（`emela_board_hal.c`）を生成します。`renode-f446/run.sh` は、生成した C を STM32CubeF4 の HAL とリンクして ELF を作り、Renode で USART2 に出た文字を確かめます。

```sh
./examples/embedded-uart/renode-f446/run.sh
```

`renode-f446/board.c` はクロックとフックだけを実装します（GPIO の例と同じ。HAL の UART はタイムアウトに SysTick を使うので、SysTick は止めません）。リンカースクリプトは GPIO の例のものを使います。

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
./examples/embedded-uart/renode-f446/run.sh
```

Emela C の生成、ARM GCC でのビルド、Renode での確認までを行い、`examples/embedded-uart/target/f446-renode/firmware.elf` を作ります。ツールは `/home/aoi_nemui/bareruby/stm32_renode/.tools` のものを使います（別の場所なら `STM32_RENODE_ROOT=/path/to/stm32_renode` を前に付けます）。

### 3. シリアルの出力を開く

出力は ST-LINK の仮想 COM ポート（115200 bps、8N1）に出ます。書き込む前に、別の WSL のターミナルで開いておきます。

```sh
picocom -b 115200 --imap lfcrlf /dev/ttyACM0
```

ファームウェアは改行を LF だけで送ります。`--imap lfcrlf` は、受け取った LF を端末で行頭に戻して表示するための指定です（付けないと行が階段状にずれます）。終わるときは `Ctrl-A` に続けて `Ctrl-X` を押します。

### 4. 書き込む

```sh
OPENOCD=/home/aoi_nemui/bareruby/prototype/bareruby-prototype/.tools/stm32cube/xpack-openocd-0.12.0-7/bin/openocd
"$OPENOCD" -f interface/stlink.cfg -f target/stm32f4x.cfg -c "program examples/embedded-uart/target/f446-renode/firmware.elf verify reset exit"
```

`Programming Finished`、`Verified OK`、`Resetting Target` と出れば書き込めています。書き込み後にリセットされ、プログラムが始まります。もう一度最初から動かすときは、ボードの黒いリセットボタン（B2）を押します。

### 5. 確かめる

picocom に次の4行が出ます（起動するたびに1回）。

```text
Hello
count 0
count 1
count 2
```

### うまくいかないとき

- `Command '' not found`: `OPENOCD=...` の行を実行していません。2行とも実行します。
- `Error: open failed`（OpenOCD）: ST-LINK が WSL に渡っていません。PowerShell で `usbipd attach --wsl --busid ...` をやり直します。WSL に渡している間は、Windows のアプリから ST-LINK を使えません。
- picocom に何も出ない: `/dev/ttyACM0` が別の番号（`/dev/ttyACM1` など）になっていないか `ls /dev/ttyACM*` で確かめます。picocom を開いてからリセットボタンを押すと、最初の行から見えます。
