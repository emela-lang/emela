# ESP32-C3ボードpackage

Emela生成CとこのpackageをPythonの`build.py`でRISC-Vへcompileし、`firmware.elf`・`firmware.bin`・`firmware.map`・`resources.json`を生成する。標準entryは引数なし・`Int`戻り値のsuspend関数で、`generated/emela_embedded_exports.h`の`board_main_start` aliasから呼び出す。schedulerはmain contextで実行し、SysTimer ISRは完了通知だけを行う。通常のprojectは[Emela CLIの開発手順](../../docs/cli-embedded.md)でboardを選び、生成・build・flashを行える。以下はRust APIを使うstandalone手順である。

## 前提ツール

- Espressif RISC-V GCC。検証版は`esp-14.2.0_20241119`。`riscv32-esp-elf-gcc`をPATHへ置くか、toolchain rootを`EMELA_ESP32C3_TOOLCHAIN`へ指定する。
- Python 3と公式`esptool==4.8.1`。buildを実行するPython環境へ`python3 -m pip install esptool==4.8.1`で導入する。別のPythonを使う場合は`EMELA_ESPTOOL_PYTHON`を指定する。
- 実機への書き込み時だけ、UART download modeに入れるESP32-C3とserial portが必要。`--dry-run`はデバイスへ接続しない。

## Rust APIから生成してstandalone buildする

repo rootから次のように生成する。`board_probe`はGPIO3をHighにし、Consoleへ`board probe: high`を出力、10ms待機、GPIOをLowにし、`board probe: low`を出力して42を返す。出力先に`generated/`と`bsp/`ができる。

```sh
cargo run -p emela-codegen-emb --example board_probe -- \
  --board esp32c3 --out-dir /tmp/emela-board-esp32c3

python3 /tmp/emela-board-esp32c3/bsp/build.py build \
  --generated-dir /tmp/emela-board-esp32c3/generated \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --project-dir /tmp/emela-board-esp32c3 \
  --optimization O2
```

`--optimization`は`O0`／`O2`を受け付ける。追加のC／assembly sourceは`--source`で指定する。独自のEmelaソースを使う場合は既存frontendでCoreを作り、`compile`／`compile_flow`とboard package APIを組み合わせる。[ボード接続と確認](../../docs/boards.md)に生成例と観測方法を記載する。

## Flash起動方式

標準ESP imageをFlash offset **0x0**へ書き込む。ROMがIRAM・DRAMのsegmentsをSRAMへloadしてentryへ移る。全コードがSRAMへ収まる構成なのでESP-IDFのsecond-stage bootloader・partition tableは使わない。[Espressif公式の説明](https://github.com/espressif/esp32c3-direct-boot-example#alternatives-to-direct-boot)がこの方法を案内している。rev3専用のmagicによるXIP direct bootとは異なる。

書き込み設定はDIO・40MHz・4MB Flash。SPI Flashへの書き込みとresetは公式esptoolへ任せる。secure boot・Flash暗号化・OTA・rollbackはこのpackageの対象に含めない。

SRAM1の128KiBだけを使う。物理aliasの重複をlinkerで検査し、ROM loaderのstack（ESP-IDF v5.4.1の`SOC_ROM_STACK_START=0x3fcde710`、サイズ`0x2000`）より下に置く。

| 用途 | CPU address | 予約量 |
| --- | --- | ---: |
| コード・定数 | `0x40380000` | 64KiB |
| data・bss・Emela固定pool | `0x3fc90000` | 48KiB |
| stack（256byte guardを含む） | `0x3fc9c000` | 16KiB |

startup自身がgp・sp・bss・guardを初期化し、256byte整列した32entryのvector tableを`mtvec` mode 1へ登録する。RTC/TG watchdogとsuper watchdogを停止し、IO_MUX、UART0の40MHz XTAL clock、SysTimer、interrupt matrixを設定する。libcの動的heap allocatorをlinkした場合はbuildを失敗させる。

## 標準APIの範囲

- GPIO番号は物理GPIO番号。0〜10と18〜21でInput・OutputPushPull、NoPull・Up・Downを実装する。11〜17はFlash電源・SPI Flash用として拒否する。GPIO interrupt・OpenDrain・Analogは明示的なdefectになる。
- ConsoleはUART0 TX=GPIO21、115200 baud/8N1で、RX=GPIO20も設定する。`Uart.configure`はUart0／115200／Bits8／One／NoParity／Transmit／NoFlowControl／By16を受け入れ、それ以外は`PeripheralError(1)`になる。標準UARTのtimed transmit/receiveは`PeripheralError(2)`を返す。
- `Clock.now`は起動からの単調増加millisecondを仮想Unix epochへ対応させる。実日時のRTCではない。`Clock.sleep`は同時1件をSysTimer割り込みで完了する。
- I²C・SPIのC symbols（`emela_i2c_*`・`emela_spi_*`）は提供しない。これらへ到達する生成Cをlinkすると未解決symbolになる。ADC・無線・USB consoleも対象外である。GPIO18/19を使う場合、USB用途との競合をアプリ側で避ける。

Emela heapは設定された固定領域の中だけを使う。`resources.json`の静的RAMには固定heap・poolが含まれる。stack予約量と実行で観測した使用量は区別する。

Clock taskを取消した場合も物理timerは旧期限まで保持する。その間の新たなsleepは容量超過としてdefectになる。期限のstale通知はruntimeのgeneration検査で拒否し、`board_rejected_notifications`へ数えて無視する。Emelaの再開や解放はISRから行わない。

## standaloneの書き込み

```sh
python3 /tmp/emela-board-esp32c3/bsp/build.py flash \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --port /dev/ttyUSB0 --dry-run

python3 /tmp/emela-board-esp32c3/bsp/build.py flash \
  --output-dir /tmp/emela-board-esp32c3/artifacts \
  --port /dev/ttyUSB0
```

`flash`は成功したbuildのmanifestと成果物hashを照合する。失敗したbuildの古いmanifestや改変されたimageは書き込まない。standaloneの照合対象はbuild成果物である。Emelaソースや生成設定の鮮度まで照合する場合は`emela build`／`emela flash`を使う。ソースを変更した場合はC生成とbuildをやり直す。

実行前にGPIO9をLowにしてresetするUART download modeが必要になるボードもある。詳細は[公式boot mode説明](https://docs.espressif.com/projects/esptool/en/latest/esp32c3/advanced-topics/boot-mode-selection.html)を参照する。

## emulatorとの境界

このpackageは実チップの`mtvec` mode 1とROMからのFlash起動を対象にし、CLIC modeやCPU dispatch hookには依存しない。Flash起動の検証にはEspressif公式QEMU `esp-develop-9.2.2-20260417`を使う。必要なQEMUとライブラリはrepo外に用意する。

[QEMU runner](../../tools/board_probe_esp32c3.py)はbuild manifestとimageを検査し、Flash imageの末尾だけを`0xff`で4MBへ補完してSPI Flashへ接続する。`-kernel`・`-bios`によるアプリ注入を行わず、QEMU内蔵ROMのpower-onから起動し、Console・10msのIRQ待機再開・結果42・stack guardを観測する。再実行手順と確認範囲は[ボード接続と確認](../../docs/boards.md)を参照する。

この版の[QEMU GPIO model](https://github.com/espressif/qemu/blob/esp-develop-9.2.2-20260417/hw/gpio/esp32_gpio.c)はstrap以外のreadを0にし、writeを無視する。このためQEMUはGPIO3の物理出力遷移を検証する根拠にはしない。実機の配線・serial転送は別の確認範囲である。

レジスタの根拠は[ESP-IDF v5.4.1 ESP32-C3 headers](https://github.com/espressif/esp-idf/tree/v5.4.1/components/soc/esp32c3/register/soc)、[起動処理](https://docs.espressif.com/projects/esp-idf/en/v5.4.1/esp32c3/api-guides/startup.html)。Flash imageのchecksum・SHA-256・chip ID・entry・load範囲はbuild時に独立に検査し、公式`esptool image_info`でも確認する。
