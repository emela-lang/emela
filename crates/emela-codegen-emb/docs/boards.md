# ボードpackageとFlash起動の確認

Flow IRコンパイラーと標準組み込みhandlerに、ボードpackage、C/assemblyのstartup・runtime、standalone build/flash scriptを接続する。Rust APIで生成したCからfirmwareを作り、STM32F4はRenode、ESP32-C3はEspressif QEMUでFlash起動を確認する。この文書はBSPと既存probeの再現手順を扱う。通常のprojectをEmela CLIで作成・build・flashする手順は[CLIでの開発](cli-embedded.md)を参照する。

## 構成と境界

| 場所 | 役割 |
| --- | --- |
| [`boards.rs`](../src/boards.rs) | board IDからpackage資産を返す。資産はコンパイル時に同梱し、利用時にコンパイラーのsource checkoutを要求しない。 |
| `Output.exports` | 生成した公開関数のEmela名、C symbol、suspend入口かどうかを返す。例ではentryのalias headerを作る。 |
| [`emela_artifact.json`の契約](artifact-contract.md) | 生成C／headerの入力一覧と形式・ABIの版。BSPはEmela sourceやIRを読まず、この一覧から生成Cを選ぶ。 |
| [`examples/board_probe.rs`](../examples/board_probe.rs) | source→Core/Flow→C生成、選択したBSPの展開を行うRust example。 |
| [`boards/stm32/stm32f4`](../boards/stm32/stm32f4/README.md) | MCU/profile、vector、linker、startup、GPIO・Console・Clock等のSTM32F4実装。 |
| [`boards/esp32c3`](../boards/esp32c3/README.md) | RISC-V startup、SRAMへloadされるFlash image、GPIO・Console・Clock等のESP32-C3実装。 |
| [`board_probe_stm32f4.py`](../tools/board_probe_stm32f4.py) | 既にbuildしたBINをRenodeでreset起動して観測する。 |
| [`board_probe_matrix.py`](../tools/board_probe_matrix.py) | F4のprofileとplatformの組合せをbuild・観測する。 |
| [`board_probe_esp32c3.py`](../tools/board_probe_esp32c3.py) | Espressif QEMUの内蔵ROMによるFlash imageのload・reset起動を観測する。 |

generated/runtimeの所有権・ISR・defect契約は[実装状況](implementation.md)と[標準C bridge](embedded-peripherals.md)を参照する。BSPはGPIO/UART/I²C/SPIの全operationを実装するものではない。

- GPIOの通常の入出力と、起動時に固定設定するConsole UARTを提供する。
- GPIO interrupt操作は、Emela callbackを登録するAPIではなく、このBSPではdefectにする。
- UARTの設定には各BSPの対応する固定構成があり、汎用のtimed transmit/receiveは未実装のエラーを返す。
- I²C/SPIのC driverは含まない。到達する場合は利用者が生成headerに従う実装を追加する必要がある。
- Clockはmain側で開始した1つの物理sleepをIRQによる1 ms刻みの通知へ接続する。同時に複数の物理sleepを要求する構成はBSPの容量契約を超える。
- 本BSPの `Clock.now` は起動時を明示的な仮想epoch 0とする経過msであり、RTC日時ではない。複数core/threadでの共有や実機固有のpin・電源・clock設定はボード実装側で管理する。

## 対象profile

| board ID | MCU | Flash / 通常SRAM | Renodeで使うモデル |
| --- | --- | --- | --- |
| `stm32f407vg` | STM32F407VG | 1 MiB / 128 KiB | `cpus/stm32f4.repl` |
| `stm32f412zg` | STM32F412ZG | 1 MiB / 256 KiB | `cpus/stm32f412.repl` |
| `stm32f429zi` | STM32F429ZI | 2 MiB / 192 KiB | `cpus/stm32f429.repl` |
| `stm32f446re` | STM32F446RE | 512 KiB / 128 KiB | generic STM32F4モデル上のF446 profile |
| `stm32f4-discovery` | STM32F407VG | 1 MiB / 128 KiB | Discovery boardとfragmentの構成 |
| `nucleo-f446re` | STM32F446RE | 512 KiB / 128 KiB | generic STM32F4モデル上のNucleo profile |
| `stm32f446` | STM32F446RE | 512 KiB / 128 KiB | 互換ID。generic STM32F4モデル |
| `esp32c3` | ESP32-C3 | 4 MiB Flash image / 指定したSRAM領域 | Espressif QEMU `esp32c3` machine |

F4のCCM等は通常SRAMへ加算しない。MCUの容量、利用できるpackage pin、Console UART/AFは各profileからbuild時に検証する。F446やNucleo固有のRenode modelがあるという意味ではない。ESP32-C3はSRAM実行用segmentをFlash imageへ格納し、ROMがreset時にloadする構成であり、Flash上の任意コードを直接実行するXIP構成ではない。

## probeの内容

probeはGPIOを出力に設定し、Highへ変更して `board probe: high` をConsoleへ出力する。10 ms sleepの後にLowへ戻して `board probe: low` を出力し、Intの42で終了する。

| 設定 | 値 |
| --- | --- |
| 出力pin | F446/Nucleoは5、他のF4は60、ESP32-C3は3 |
| 固定heap | 2048 byte |
| String容量 / 転送容量 | 64 byte / 16 word |
| Fiber / frame | 2 / 8 × 512 byte |
| task / event / Deferred | 2 / 2 / 1 |

これは本体の全言語仕様を網羅するfixtureではない。標準handler・固定heap・suspend taskを、実際のstartupとIRQ・UART・GPIOへ接続する代表アプリである。資源の観測値もこのprobeに対するものであり、任意のアプリの最大stack/heap・WCETを保証しない。

## CLIを使わない生成とbuild

workspace rootから実行する。Rust exampleの出力先は未存在のdirectoryを指定する。生成時は隣接するstaging directoryを使い、成功した一式を公開する。

以下のstandalone手順は`artifacts/`を出力先にする。CLIのboard buildは`generated/`・`bsp/`・`firmware/`を作り、project入力の鮮度照合も行う。両者の出力構成とflash手順を混在させない。

```sh
cargo build -p emela-codegen-emb --example board_probe --locked
target/debug/examples/board_probe --board stm32f446re --out-dir /tmp/emela-f446-probe
python3 /tmp/emela-f446-probe/bsp/build.py build \
  --generated-dir /tmp/emela-f446-probe/generated \
  --project-dir /tmp/emela-f446-probe \
  --output-dir /tmp/emela-f446-probe/artifacts \
  --optimization O2 --toolchain /path/to/arm-toolchain
```

出力は次の構成になる。

```text
OUT/
  main.emel
  generated/                 emela_artifact.json、生成C、標準ABI header、entry alias header
  bsp/                       選択したpackageのstartup・runtime・build script
  artifacts/                 firmware.elf/bin/map、firmware.json、resources.json、build log
```

F4のtoolchainはArm GNU GCC 13.2.1での利用を想定する。`--toolchain` または `ARM_TOOLCHAIN_PATH` でroot directoryを指定する。C3は `EMELA_ESP32C3_TOOLCHAIN` にRISC-V toolchainのrootを指定し、esptool 4.8.1を利用できるPythonを `EMELA_ESPTOOL_PYTHON` で選ぶ。esptoolのpackageが必要な場合は自分の環境で依存物を用意する。package内のscriptは版・imageの整合を検査する。

物理flashの引数確認だけを行う場合は、BSPのdry runを使う。

```sh
python3 /tmp/emela-f446-probe/bsp/build.py flash \
  --output-dir /tmp/emela-f446-probe/artifacts --dry-run
```

dry runは実機へ書き込まない。実際のST-Link/OpenOCDまたはesptoolの呼出しは各BSPのREADMEに従う。この段階の検証runnerは物理flashを呼ばない。

## STM32F4のFlash reset観測

```sh
python3 crates/emela-codegen-emb/tools/board_probe_stm32f4.py \
  --firmware-dir /tmp/emela-f446-probe/artifacts \
  --output /tmp/emela-f446-observation --platform cpu-f446re \
  --renode /path/to/renode-1.16.1/renode
```

Renode 1.16.1のplatform一覧を固定し、7つのF4定義の一覧が変わった場合は拒否する。各sourceのSHA-256をreportへ保存する。ネットワークからSVDを取得しないようmetadataを除き、genericモデルへBOOT0=0のFlash alias、profileのFlash/SRAM容量、reset HSI 16 MHzのSysTickを適用する。モデルを使用するための変換であり、生成Cやfirmwareをテスト用に書き換えない。

- ELFはsymbolの読取りにのみ使い、実行メモリへloadしない。BINをFlashへ置いてCPUをResetする。PC/SP/VTORを観測器から設定しない。
- `.data` / `.bss` の観測wordをreset前に汚し、startupが初期化したことを確認する。
- UARTの2行とbyte数、結果42、完了1回、IRQ tick 10回、task通知1回、拒否通知・defect 0を照合する。
- GPIOのMMIO最終状態に加え、外部LEDと独立receiverへ配線し、High→Lowの2遷移と最終Lowを確認する。
- Console TX/RXのAF・GPIO mode・RCC enableをprofileと照合する。
- stackの観測値とguard、固定heap 2048 byte、システムallocatorをlinkしていないことを確認する。stackはpaint/最小SPによる観測値であり、任意の経路の静的な最大値ではない。

monitorの同期ラベルは `renode.log`、非同期のmodel/GPIO logは `model.log` に分離する。ラベルや値の欠落・重複、モデルのwarning/errorがある場合は成功にしない。

### profile/platformのmatrix

```sh
python3 crates/emela-codegen-emb/tools/board_probe_matrix.py \
  --probe target/debug/examples/board_probe \
  --output /tmp/emela-f4-probe-matrix \
  --renode /path/to/renode-1.16.1/renode \
  --toolchain /path/to/arm-toolchain --jobs 2
```

既定ではF4の7 profileをO0/O2でbuildする14件と、次の10 platform構成をO0/O2で観測する20件を対象にする。`--platform`、`--optimization` で対象を絞った場合、reportは実際に選択した範囲を記録する。

| platform指定 | 構成 |
| --- | --- |
| `cpu-f407` / `cpu-f412` / `cpu-f429` | 対応するCPU定義 |
| `discovery` | Discovery board |
| `discovery-additional-gpios` | Discovery + GPIO fragment |
| `discovery-bb` | Discovery + baseboard fragment |
| `discovery-kit` | upstreamの合成kit定義 |
| `cpu-f446re` / `cpu-f446-compat` / `nucleo-f446re` | generic F4上の各profile |

## ESP32-C3のROM/Flash起動観測

```sh
target/debug/examples/board_probe --board esp32c3 --out-dir /tmp/emela-c3-probe
python3 /tmp/emela-c3-probe/bsp/build.py build \
  --generated-dir /tmp/emela-c3-probe/generated \
  --project-dir /tmp/emela-c3-probe \
  --output-dir /tmp/emela-c3-probe/artifacts --optimization O2
python3 crates/emela-codegen-emb/tools/board_probe_esp32c3.py \
  --firmware-dir /tmp/emela-c3-probe/artifacts \
  --output /tmp/emela-c3-observation --qemu /path/to/qemu-system-riscv32
```

固定版はEspressif QEMU `esp_develop_9.2.2_20260417`。必要なshared libraryの探索pathは利用側の環境で設定する。runnerは4 MiBのraw Flashへimageを配置し、QEMUの内蔵ROMによるreset起動を使う。ELF loadやPC/SP設定は行わず、UARTのROM Flash boot列・load先・entryが1回だけ現れることも確認する。

実際のSYSTIMER/CPU IRQを通したtick 10回、task通知1回、結果42、guard/stack、`mtvec` とtrap causeを検査する。applicationの2行とbyte数はROM自身のログと分けて照合する。

**この固定QEMU版のGPIO modelは出力writeを無視するため、GPIO遷移は検証できない。** reportには `gpio_verification.status = unavailable` として記録する。UARTがHigh/Lowという文字列を出したことをもって、C3のGPIO出力確認として扱わない。F4の外部receiverによる確認とは範囲が異なる。

## report・出力保護とrunnerの確認

runnerは空のdirectory、または同じrunnerが所有するmarkerのあるdirectoryだけを使う。排他lockを取り、開始時に `report.json` を `running` に置き換え、失敗時は `failed` へ更新する。古い `passed` を新しい実行結果として残さない。matrixは実行ごとに新しいrun directoryを作り、前回の生成物を再利用しない。

manifestのartifact hashとresourcesを確認してから起動する。子processには専用TMPDIRとtimeoutを与え、timeout時はprocess groupを停止する。観測用のlog・platform・monitor script・firmware hash・資源値をreportから追える。失敗時のreportとlogも調査用に残す。

外部ツールを使わないPythonの確認は次のコマンドで実行する。

```sh
python3 crates/emela-codegen-emb/tools/test_board_probe.py
```

対象は出力の保護とlock、古い成功reportの無効化、manifest/ELF/metricの読取り、F4 inventoryとalias、timeoutである。firmwareの生成・MCUのboot動作はこのPython確認だけでは検証しない。Rustのworkspace CIと各emulatorの実行結果を合わせて評価する。
