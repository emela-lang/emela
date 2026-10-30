# STM32F4共通firmware package

STM32F4のruntime・startup・linker・ビルド／書き込み処理を共有し、MCUと配線の差分をJSONへ分ける。Cortex-M4F、`fpv4-sp-d16`、hard-float、reset時のHSI 16 MHzを使う。STM32Cube・HAL・CMSISはビルド依存に含めない。

```text
boards/stm32/stm32f4/
├── runtime.c                 # GPIO・Console・Clock・scheduler接続
├── startup.S                 # Flash reset・FPU・RAM初期化
├── validation_stack.c,h      # MSP high-waterとguard
├── firmware.ld.in            # MCU容量からlinker scriptを生成
├── build.py                  # クロスbuild・資源検査・OpenOCD
├── chips/*.json              # MCUメモリ・package pin・IRQ slot
└── boards/*.json             # Console配線・基板名・標準LED pin
```

`chips`は実MCUの容量とpackageを表す。`boards`はそのMCUを選び、ConsoleのUSART・AF・TX/RXを指定する。Rust APIのboard packageは選択したファイルを`chip.json`・`profile.json`として共有実装とともに出力し、Pythonの`build.py`が`board_config.h`・`board_vectors.inc`・`firmware.ld`を生成する。出力済みのpackageを使うbuildは、開発checkoutや外部SDKに依存しない。

## 選択できる対象

生成directoryの`emela_artifact.json`からC入力を選ぶ。形式・ABIの版と全列挙ファイルをtoolchain起動前に検証し、未列挙の残存Cはcompileしない。独自の生成C／headerを`Output.files`へ追加するときは`refresh_manifest()`で一覧を更新する。[生成物の契約](../../../docs/artifact-contract.md)を参照する。

| Rust APIのboard ID | MCU | Flash | 通常SRAM | Console TX/RX |
| --- | --- | ---: | ---: | --- |
| `stm32f407vg` | STM32F407VG、LQFP100 | 1 MiB | 128 KiB | USART2、PA2/PA3、AF7 |
| `stm32f412zg` | STM32F412ZG、LQFP144 | 1 MiB | 256 KiB | USART6、PC6/PC7、AF8 |
| `stm32f429zi` | STM32F429ZI、LQFP144 | 2 MiB | 192 KiB | USART1、PA9/PA10、AF7 |
| `stm32f446re` | STM32F446RE、LQFP64 | 512 KiB | 128 KiB | USART2、PA2/PA3、AF7 |
| `stm32f4-discovery` | STM32F407VG | 1 MiB | 128 KiB | USART2、PA2/PA3、AF7、外付けserial adapter用 |
| `nucleo-f446re` | STM32F446RE | 512 KiB | 128 KiB | USART2、PA2/PA3、AF7、標準ST-Link VCP配線 |
| `stm32f446` | STM32F446RE、既存互換ID | 512 KiB | 128 KiB | USART2、PA2/PA3、AF7 |

MCU IDの配線はpackageが選ぶ既定値であり、実基板にUART-USB回路があることを意味しない。DiscoveryのST-Link VCP有無・配線、Nucleoのsolder bridge等は基板revisionと回路図を確認する。未登録のF4型番や基板を自動的に対応済みとは扱わない。

F407の総SRAM 192 KiBにはCCM 64 KiBが含まれるため、このpackageが使う`0x20000000`からの連続SRAMは128 KiBになる。F429の総256 KiBもCCM 64 KiBを除き192 KiBとする。F412は連続256 KiB、F446は連続128 KiBを使う。CCM・backup SRAM・外部SDRAMは予約しない。Flashはすべて`0x08000000`から配置する。

## Rust APIから生成してstandalone buildする

このpackageはRust API・BSP・Pythonのbuild／flash処理を提供する。通常のprojectは[Emela CLIの開発手順](../../../docs/cli-embedded.md)でboardを選び、生成・build・flashを行える。以下はRust APIを使うstandalone手順である。

repo rootで次のように生成する。`board_probe`は引数なしで`Int`を返す`Async` entryを生成し、GPIOをHigh、Consoleへ`board probe: high`を出力、10ms待機、GPIOをLow、`board probe: low`を出力して42を返す。F446／Nucleo／互換IDではPA5（5）、F407／F412／F429／DiscoveryではPD12（60）を使う。

```sh
cargo run -p emela-codegen-emb --example board_probe -- \
  --board stm32f446re --out-dir /tmp/emela-board-stm32f446re

python3 /tmp/emela-board-stm32f446re/bsp/build.py build \
  --generated-dir /tmp/emela-board-stm32f446re/generated \
  --output-dir /tmp/emela-board-stm32f446re/artifacts \
  --project-dir /tmp/emela-board-stm32f446re \
  --optimization O2
```

`ARM_TOOLCHAIN_PATH`へtoolchain rootを指定するか、`arm-none-eabi-gcc`・`nm`・`objcopy`・`size`を`PATH`へ置く。生成先には`generated/`と`bsp/`ができる。buildは`artifacts/`へ`firmware.elf`・`firmware.bin`・`firmware.map`・`firmware.json`・`resources.json`と、選択profileから生成したheader・vector・linker scriptを出力する。`--optimization`は`O0`／`O2`を受け付ける。

独自のEmelaソースを使う場合は既存frontendでCoreを作り、`compile`／`compile_flow`とboard package APIを組み合わせて出力する。BSPのentryは引数なし・`Int`戻り値のsuspend関数であり、`generated/emela_embedded_exports.h`に`board_main_start`のaliasが必要になる。生成例と観測方法は[ボード接続と確認](../../../docs/boards.md)を参照する。

## standaloneの書き込み

OpenOCDの`interface/stlink.cfg`と`target/stm32f4x.cfg`を使う。dry runは成果物hashを確認してcommandを表示し、デバイスへ接続しない。

```sh
python3 /tmp/emela-board-stm32f446re/bsp/build.py flash \
  --output-dir /tmp/emela-board-stm32f446re/artifacts --dry-run

python3 /tmp/emela-board-stm32f446re/bsp/build.py flash \
  --output-dir /tmp/emela-board-stm32f446re/artifacts
```

`--port`はST-Link adapter serialを指定する。`OPENOCD`で実行するOpenOCDを選べる。standaloneの照合対象は`firmware.json`に記録したbuild成果物である。Emelaソースや生成設定の鮮度まで照合する場合は`emela build`／`emela flash`を使う。ソースを変更した場合はC生成とbuildをやり直す。

## GPIOとConsoleの契約

論理GPIO IDは`port * 16 + pin`で表す。A=0、B=1、C=2、D=3、…、H=7とする。PA5=5、PB0=16、PC6=38、PD12=60になる。従来のPA0〜PA15のIDは変わらない。

| MCU | packageから使えるGPIO |
| --- | --- |
| F407VG | PA〜PEの各0〜15、PH0/PH1 |
| F412ZG／F429ZI | PA〜PGの各0〜15、PH0/PH1 |
| F446RE | PA0〜15、PB0〜10/PB12〜15、PC0〜15、PD2、PH0/PH1 |

存在しないpinとConsoleのTX/RX pinはboard defectで拒否する。SWD/JTAGや実装済み水晶等の基板上の用途はpackage pinの存在とは別に扱い、アプリの基板設計に従って予約する。PA13/PA14をGPIOへ変更すると実機のSWD接続へ影響する。

- `configure`はInput／OutputPushPull／OutputOpenDrain／Analog、NoPull／Up／Downを受け入れ、該当portのRCCを有効化する。
- `write`は設定済みoutputに限定しBSRRへ書く。`read`はIDRのpin level、`toggle`はODR latchを使う。
- ConsoleはprofileのUSARTを115200 baud、8N1へ初期化する。AFRL・AFRHをpinごとに設定し、APB1／APB2の選択を反映する。全profileはHSI・APBのreset値16 MHzを維持する。
- `Uart.configure`はConsoleに選択したUart ID／115200／Bits8／One／NoParity／Transmit／NoFlowControl／By16を受け入れる。それ以外は`PeripheralError(1)`、timeout付き`transmit`／`receive`は`PeripheralError(2)`になる。
- GPIO割り込みの有効化・無効化はboard defectで停止する。
- I²C・SPIのC symbols（`emela_i2c_*`・`emela_spi_*`）は提供しない。これらへ到達する生成Cをlinkすると未解決symbolになる。ADC等のdriverも対象外である。

USART pinのAF・RCC操作、GPIO register配置はF4共通である。別シリーズへ同じregister offsetやclock設定を流用しない。

## Flash reset・待機・所有権

vector tableをFlash先頭へ置き、初期MSPを通常SRAM上端とする。Reset handlerはFPUを有効にし、VTOR設定、`.data`のFlashからSRAMへのコピー、`.bss`クリア、C初期化、アプリ起動を行う。BOOT設定はmain Flash起動を選ぶ。PLL/HSEには依存しない。

外部IRQはMCU別の全slotと予約slotをmetadataへ保持する。`board_vectors.inc`が標準名のweak handlerを生成するため、`build.py build --source /path/to/irq.c`で`USART1_IRQHandler`等を定義すれば上書きできる。F4の機種差による`FSMC`／`FMC`、`TIM6`／`TIM6_DAC`、`HASH_RNG`／`RNG`の名前と配置を保存し、共通のDefault handlerへ直接固定しない。予約slotは0とする。

Clockは実SysTickの1ms IRQを使う。ISRはtick更新・task完了通知・SP記録のみを行い、Emelaの再開・文字列・heap操作はmain側schedulerで実行する。`Clock.now`は起動時を仮想epoch 0とした経過msで、RTC日時ではない。正の`Clock.sleep`は同時1件とし、0以下は即時通知する。取消済みtaskへの遅延通知は世代を検査して記録し、再開しない。物理期限が残る間の新しい正のsleepは容量超過として停止する。

defectはSysTickを止め、設定済みの全portのoutputをLowへし、失敗数を記録して停止する。

## 資源と観測

stackは16 KiB、low guardは別に256 byteを予約する。固定heap・fiber・frame等のpoolは生成Cの静的RAMへ含め、静的RAMとguard／stackの衝突はlinkで拒否する。system allocatorの`malloc`等がlinkされている場合はbuildを失敗させる。`resources.json`の予約量と実行時に観測するstack high-waterは別の値である。

`board_app_completed`・`board_app_result`・`board_app_failures`・`board_ticks`・`board_notifications`・`board_rejected_notifications`・`board_uart_bytes`、`board_boot_*`と`emela_validation_stack_*`を観測できる。stack paint走査はmainのwork後と終了時に行い、IRQはSPの記録だけとする。

[Renode runner](../../../tools/board_probe_stm32f4.py)はFlash binaryからresetし、GPIOのHigh→Low、Console、10msの実IRQ待機再開、結果42、stack guardを観測する。[matrix runner](../../../tools/board_probe_matrix.py)は登録されたF4 profileとplatformの組合せを実行する。対象と再実行手順は[ボード接続と確認](../../../docs/boards.md)を参照する。ST-Linkによる物理転送とエミュレータの実行結果は別の確認範囲である。

## 設定根拠

- [STM32F407VG datasheet、DS8626](https://www.st.com/resource/en/datasheet/stm32f407vg.pdf)：LQFP100 pinout、Flash/RAM構成。
- [STM32F412ZG datasheet、DS11139](https://www.st.com/resource/en/datasheet/stm32f412zg.pdf)：LQFP144 pinout、連続256 KiB SRAM、PC6/PC7のUSART6 AF8。
- [STM32F429ZI datasheet、DS9405](https://www.st.com/resource/en/datasheet/stm32f429zi.pdf)：LQFP144 pinout、2 MiB Flash、CCMを含むSRAM構成。
- [STM32F446RE datasheet、DS10693](https://www.st.com/resource/en/datasheet/stm32f446re.pdf)：LQFP64 pinout。PB11はこのpackageには無い。
- [ST公式CMSIS device F4](https://github.com/STMicroelectronics/cmsis-device-f4)：MCU別IRQ番号と`Source/Templates/gcc/startup_stm32f*.s`の標準handler名。手元ではSTM32CubeF4 1.28.3の同じ定義で照合した。metadataにはslot番号・識別子という事実データを収録し、HAL/CMSISのコードを同梱しない。
- [STM32F4 reference manual RM0090](https://www.st.com/resource/en/reference_manual/dm00031020.pdf)、[F412 RM0402](https://www.st.com/resource/en/reference_manual/rm0402-stm32f412-advanced-armbased-32bit-mcus-stmicroelectronics.pdf)、[F446 RM0390](https://www.st.com/resource/en/reference_manual/dm00135183.pdf)：GPIO/RCC/USART、reset clock、SRAM配置。
