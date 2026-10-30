# CLIで組み込みC・firmwareを生成する

`emela build --target c-embedded`は、ソースを型検査し、多相Coreを組み込みbackendへ渡す。backendが単相化とFlow IRへの変換を行い、組み込みCを生成する。Heap IRは使わない。ボードを選ぶと、同梱BSPによるcross compile・link・Flash image生成まで進める。

言語機能と入力の制約は[実装範囲と対応状況](implementation.md)、標準effectは[組み込みインターフェース](embedded-interfaces.md)、ボードごとのdriver・メモリ・起動方式は[ボードpackage](boards.md)を参照する。CLIへの接続によって、成長型のCore入力、直接の関数値const、汎化effectのAsync具体化に関する既存frontendの制約が解消されるわけではない。

## プロジェクトを作る

次の3ファイルを作る。標準のConsoleとClockを使うこの構成では、利用者がRustやCのファイルを追加する必要はない。

```text
device/
  Pome.toml
  embedded.toml
  src/main.emel
```

`Pome.toml`:

```toml
[build]
target = "c-embedded"

[embedded]
board = "stm32f446re"
config = "embedded.toml"
entry = "main"
optimization = "O2"
```

依存のないローカルprojectには、上記のように`[pome]`や`Pome.lock`を省略できる。Pome依存を追加する場合は通常のmanifest・lockの要件が適用される。

`embedded.toml`:

```toml
heap_bytes = 2048
string_capacity = 64
peripheral_buffer_capacity = 16
fiber_capacity = 2
frame_capacity = 8
frame_bytes = 512
task_capacity = 2
event_capacity = 2
deferred_capacity = 1
```

`src/main.emel`:

```emela
import Std.Console.{Console}
import Std.Clock.{Clock}
import Std.Embedded.Board.{BoardConsole, BoardClock}

pub fn main() -> Int use Async {
  with BoardConsole, BoardClock {
    (use Console).print_line("device ready")
    (use Clock).sleep(10)
    0
  }
}
```

entryは公開・非generic・引数なしの`fn() -> Int use Async`とする。未処理の失敗・通常effectや外部bindingを持つentryは拒否する。CLIが`board_main_start`のC aliasを生成し、BSPのschedulerへ接続する。`[embedded].entry`を省略した場合は`main`を使う。

必要な外部ツールを用意してから、`device`の親directoryで実行する。

```sh
emela check device
emela build device
emela flash device --dry-run
emela flash device
```

workspace内でCLIを起動する場合は、例えば`cargo run --locked -p emela-cli -- build /path/to/device`とする。各コマンドのpathを省略すると`.`からprojectを探す。`check`は型検査までであり、C生成・link・pool容量の成立や、対象BSPにdriverがあることは検証しない。

## target・board・設定の選択

| 項目 | 優先順・既定値 |
| --- | --- |
| board | `--board`、次に`Pome.toml`の`[embedded].board`。未指定ならC生成のみ。 |
| target | `--target`、次に`[build].target`。両方なければboard選択時は`c-embedded`、それ以外は`js`。 |
| runtime設定 | `--embedded-config`、次に`[embedded].config`。両方なければbackend既定値。 |
| 出力先 | `--out-dir`、次に`<project>/target/emela/c-embedded/<board>`。boardなしは末尾のboard directoryを付けない。 |
| firmware最適化 | `[embedded].optimization`の`O0`または`O2`。既定は`O2`。 |

`--board`を指定した状態で、明示したtargetまたは`[build].target`が`js`・`c`・`wasm`なら診断する。`[embedded].board`はtargetが`c-embedded`のときだけ使い、`js`・`c`・`wasm`のbuildでは無視する。このとき以前のfirmwareの成功記録は残る。`--embedded-config`も`c-embedded`専用である。既存の`js`・`c`・`wasm`のbuildは、組み込み用runtime設定ファイルやmodule置換を読み込まない。

`[embedded].config`・`sources`・`modules`の相対pathはproject directory基準、`--embedded-config`と`--out-dir`の相対pathはコマンドを実行したdirectory基準である。`embedded.toml`の自動探索は行わない。単独ファイル入力にはprojectの`[embedded]`設定がないため、必要なboardと設定をflagで指定する。

| 対象 | board ID |
| --- | --- |
| STM32F4 MCUと既定Console配線 | `stm32f407vg`、`stm32f412zg`、`stm32f429zi`、`stm32f446re` |
| STM32F4基板profile | `stm32f4-discovery`、`nucleo-f446re` |
| STM32F446互換ID | `stm32f446` |
| ESP32-C3 | `esp32c3` |

例えば別boardへのbuildと書き込みは次のように行う。board別の既定出力先を使うため、F4の生成物と混在しない。

```sh
emela build device --board esp32c3
emela flash device --board esp32c3 --port /dev/ttyUSB0 --dry-run
emela flash device --board esp32c3 --port /dev/ttyUSB0
```

`--port`はESP32-C3ではserial port、STM32F4ではOpenOCDのST-Link adapter serialである。ESP32-C3の実書き込みにはportが必要で、基板によってUART download modeへの操作も必要になる。pinの割り当て・Console配線・BSPが提供するoperationはboardごとに確認する。board名の選択だけで未実装driverが追加されるわけではない。

### Cだけを生成する

boardを指定せず、公開関数をC側から呼ぶライブラリも生成できる。Emelaの`main`は不要である。

```sh
emela build library.emel --target c-embedded \
  --embedded-config embedded.toml --out-dir generated
```

この場合はcross compilerやPythonを起動せず、指定directoryへCとheaderを出力する。利用側が生成headerに従ってhook・外部C実装を用意し、compile・linkする。`c-embedded`に対する`emela run`と`emela test`は対応せず、診断する。

directory入力で`src/main.emel`がなく`src/lib.emel`だけがあるlibrary専用projectでは、`build`は検査だけを行う。boardが選ばれていてもfirmware出力や外部toolの起動を行わない。明示的にファイルを指定した場合は、そのファイルを入口として扱う通常の規則が適用される。

### module置換と追加Cソース

配線や独自deviceをmoduleへ分ける場合は、`Pome.toml`の`[embedded]`へ必要な項目を追加する。

```toml
[embedded]
board = "stm32f446re"
config = "embedded.toml"
entry = "main"
optimization = "O2"
sources = ["device.c"]

[embedded.modules]
Hardware = "ports/{board}/hardware.emel"
```

この拡張を使う場合は`device.c`と選択boardのmoduleファイルも利用者が用意する。`sources`は追加のC／assemblyファイル（`.c`・`.S`・`.s`）であり、依存PomeのC実装をlinkする場合も明示する。

`modules`はmodule名を物理ファイルへ対応付け、pathの`{board}`を選択board IDに置換する。例えば`Hardware`は`src/hardware.emel`、`DevicePort`は`src/device_port.emel`に見える。source filesystemの読取りだけを置換し、元のEmelaファイル、Flow IR、生成Cを書き換えない。`emela check device --board esp32c3`も同じ置換を使う。boardを選んでいないときは、pathに`{board}`を含む置換を適用せず、source rootのmoduleをそのまま読む。LSP・`doc`・hostの`run`はこの置換を適用しない。

### Pome依存

依存Pomeの`[pome].targets`では`"c-embedded"`を指定できる。省略時は全targetを許可する。`"c"`はhost向けの別targetであり、`["c"]`だけを許可する依存は組み込みbuildでE0238になる。boardの有無にかかわらず検査する。`check`とLSPはtargetを選ばないため、この制限を検査しない。

依存の`@external`には既存の`[external].allow`の規則も適用される。標準の`Std.Embedded`は同梱moduleであり、Pome依存を追加する必要はない。

## runtime設定

`embedded.toml`には次の項目を指定できる。省略項目はbackend既定値を使い、未知のTOML項目や型・範囲が不正な値は診断する。

| 項目 | 意味・単位 | 既定値 |
| --- | --- | ---: |
| `heap_bytes` | module内の固定heap予約量。byte、0は無効。 | 0 |
| `string_capacity` | 動的Stringの出力buffer容量上限。byte、1〜512。 | 256 |
| `peripheral_buffer_capacity` | 同期UART/I²C/SPIの各転送bufferの要素数。16bit word、1〜65,535。 | 256 |
| `const_diagnostic_bytes` | 実行時constのdefect message保存量。byte。 | 1024 |
| `fiber_capacity` | 未解放の完了済みhandleを含むFiber数。 | 8 |
| `frame_capacity` | 同時に生存できるsuspend frame数。 | 32 |
| `frame_bytes` | frame headerを含む1個のframe容量。byte。 | 256 |
| `task_capacity` | 外部taskのslot数。 | 8 |
| `event_capacity` | 待ち行列に保持する完了通知数。 | 8 |
| `deferred_capacity` | 同時に生存できるDeferred数。 | 8 |

heapとpoolは別の固定領域であり、使用するruntimeに応じて生成される。実際に必要な容量はアプリの型・同時生存数・targetのC layoutによる。frameの大きさはC compile時にも検査する。上のproject設定は任意のアプリの推奨値や最大使用量を表すものではない。

転送bufferはC stack上の`uint16_t`配列で、送信または受信で`2 × capacity` byte、SPI同時送受信では2配列合計`4 × capacity` byteを使う。固定上限でも小さなstackに収まるとは限らない。heap・String・scheduler poolの実行時容量不足はdefect契約に従い、標準bridgeの転送容量超過は`PeripheralError(-2)`になる。詳細は[標準C bridge](embedded-peripherals.md)を参照する。

### 外部Cとの接続

`BoardGpio`・`BoardUart`・`BoardI2c`・`BoardSpi`・`BoardClock`・`BoardConsole`はbackendが標準C ABIへ接続するため、下記のadapter表は不要である。同梱BSPの対応範囲外のdriverは、利用者が生成headerに従って追加する。

独自の外部関数は`@external(c, "symbol")`を使う。binding名を`c-embedded`へ変更しない。必要に応じてruntime設定へmappingを追加し、対応するC実装も用意する。

```toml
[task_adapters]
host_wait = "device_wait"

[external_adapters.host_read]
symbol = "device_read"
result = "borrowed"
```

`task_adapters`はsuspend外部関数のC symbolから、`EmelaTask`開始ABIを実装するsymbolへの対応である。開始時にtaskを受け取り、C側が完了を通知し、main側のschedulerがEmelaを再開する。開始引数を後で使う場合はC側でsnapshot／retainする。

同期の`external_adapters`はString・tuple等の生成C表現と所有権を接続する。引数は呼出し中だけの借用である。`result`には次のいずれかを指定する。

| 値 | 戻り値の契約 |
| --- | --- |
| `borrowed` | wrapperが所有参照を取得する間、C側が値を有効に保つ。C関数内のローカル配列へのpointerは返せない。 |
| `owned` | この生成moduleの固定heapに属する所有値をEmelaへ移す。全managed fieldが契約を満たす必要がある。 |
| `string-buffer` | 追加のcaller bufferとbyte容量へ書き、そこを指すownerなしStringを返す。String戻り値専用。C側が書込み前に容量を守る。 |

使われないadapter mappingも受け入れる。symbolはCの識別子で、予約語は使えない。実際の引数・戻り値・task結果の所有権は[実装状況の外部ABI契約](implementation.md#4-外部cisrボードとの境界)と生成headerに従う。

`isr_entries = ["function_name"]`と`isr_safe_externals = ["c_symbol"]`も指定できる。backendは指定入口からの到達処理を制限するが、IRQ vectorへの配線やC実装の実行時間を証明する設定ではない。通常のBSPはISRから完了通知だけを行い、Emela入口・heap・schedulerの処理はmain側に置く。

### 公開関数のC alias

```toml
[exports]
device_read = "read"
device_poll_start = "poll"
```

左辺はC alias、右辺は`@export(c, "...")`で公開したEmela関数名である。`pub`だけではCへ公開されない。`emela_embedded_exports.h`が`Output.exports`の情報に基づいて生成C symbolへ対応付ける。suspend関数のaliasは開始関数を指す。

未知・曖昧な公開関数名、不正なC識別子、生成runtimeや外部symbolと衝突するaliasは診断する。既存C macroとの衝突はheader内で検出する。board選択時は`board_main_start`をentryへ自動登録し、別の関数を同じaliasへ指定した設定は拒否する。

## build結果とflash時の照合

board選択時の出力は次の構成になる。

```text
target/emela/c-embedded/<board>/
  generated/                 # emela_artifact.json、C、公開header、exports alias、標準bridge header
  bsp/                       # 同梱startup・runtime・linker・build.py・build_state.py等
  firmware/                  # firmware.elf、.bin、.map、.json、resources.json等
  board-inputs.json
  board-build-pending.json
  board-build.json
```

CLIはbuild開始時に以前の`board-build.json`を無効にし、lowering前の入力を記録する。C生成とBSP buildが成功し、入力が途中で変わっていないことを確認した場合だけ、新しい成功記録を作る。失敗した再build後に以前のfirmwareを書き込むことはできない。

生成とビルドの入力は[`emela_artifact.json`の契約](artifact-contract.md)で分ける。BSPはmanifestに列挙された生成Cだけをcompileし、Emela sourceやIRを読まない。C-only出力にもmanifestを含める。従来のC／headerだけの出力は一式を再生成する。

`emela flash`は再buildせず、選択したproject・board・設定・出力先と、記録されたSHA-256を照合する。現在のCLIに同梱した照合処理を先に実行し、合格してから出力directory内のBSP scriptへ処理を渡す。照合対象は次のとおり。

- projectと依存Pomeのsource一覧・内容（`.emel`・`.c`・`.h`・`.S`・`.s`・`.toml`）。moduleの追加・削除も検出し、symlink経由の入力はmoduleのpathと実体の両方を記録する。単独ファイル入力は同じdirectoryの一覧が対象になる。
- project／依存のmanifest、存在する`Pome.lock`、runtime設定、置換module、明示した追加C／assemblyとそのdirectory以下のheader。
- 生成manifest自身と列挙された全C／header（nested pathを含む）、生成先のheader一覧、展開したBSP、firmware manifestに列挙された成果物。未列挙の残存CはBSPが入力に使わないため対象外とする。

source・依存・設定・生成物の変更、別project／boardの指定、出力先の移動があればbuildをやり直す。build時に`--embedded-config`や`--out-dir`を指定した場合、flash時も同じ指定を使う。`--dry-run`でも同じ照合を行い、その後の書き込みtoolのargvを表示する。デバイスへの接続は行わない。

通常のflashはSTM32F4でOpenOCD、ESP32-C3でesptoolへ処理を渡す。CLIはshellを介さずにpath・port等を個別の引数として渡す。BSPの`build.py flash`を直接使うstandalone手順は成果物だけを照合し、project sourceの鮮度照合はCLI側の追加機能である。既存Rust probeのstandalone出力`artifacts/`とCLIの`firmware/`は別の構成として扱う。

## 外部ツールの準備

| 用途 | 利用者が用意するもの・選択方法 |
| --- | --- |
| CLI自体 | workspaceなら`cargo build --locked -p emela-cli`。生成後のCLIにはBSPと照合scriptを同梱する。 |
| firmware build共通 | Python 3。`EMELA_PYTHON`で実行するPythonを変更でき、既定は`python3`。 |
| STM32F4 build | `arm-none-eabi-gcc`・`nm`・`objcopy`・`size`をPATHへ置くか、toolchain rootを`ARM_TOOLCHAIN_PATH`へ指定する。 |
| STM32F4 flash | OpenOCDとST-Link。`OPENOCD`で実行ファイルを指定できる。 |
| ESP32-C3 build | Espressif RISC-V toolchain。`riscv32-esp-elf-gcc`等をPATHへ置くか、rootを`EMELA_ESP32C3_TOOLCHAIN`へ指定する。 |
| ESP32-C3 image／flash | `esptool==4.8.1`を使用Pythonへ導入する。`EMELA_ESPTOOL_PYTHON`でそのPythonを指定できる。例えば`python3 -m pip install esptool==4.8.1`。 |

toolchainの自動取得、物理ボードの接続や配線は行わない。詳細なtool版・BSPの対応機能・emulatorでの再現手順は[STM32F4](../boards/stm32/stm32f4/README.md)、[ESP32-C3](../boards/esp32c3/README.md)、[ボード確認](boards.md)を参照する。Renode／QEMUは通常のCLI build・flashには不要である。

## 検証の範囲

CLIのhost回帰試験は、C生成、設定とalias診断、targetとPomeの制限、module置換、library専用project、失敗した再buildによる成功記録の無効化を確認する。Unix上の2件は`EMELA_PYTHON`のtest wrapperで実際のprepare／seal／verifyを呼び、BSPのbuild／flashだけをmock化してCLIからの接続も確認する。Pythonの試験は照合処理を直接呼び、入力・生成物・BSPの変更や追加・削除、build中の変更を検出する。これらはcross toolchainや物理デバイスを使わず、実際のfirmware build・flashやMCU上での動作を検証するものではない。実BSPのFlash起動・IRQ・GPIO・UART・資源の観測は[既存のボードprobe](boards.md)で別に扱う。

| 試験 | 主な確認対象 |
| --- | --- |
| [`embedded_cli.rs`](../../emela-cli/tests/embedded_cli.rs) | C-only出力、設定・entry・alias診断、8種類のboard ID、checkとbuildの読取り専用overlay、BuildOnlyのrun/test診断、成功記録の無効化、既存targetへの非干渉。 |
| [`embedded_dependencies.rs`](../../emela-cli/tests/embedded_dependencies.rs) | ローカル置換した依存のtarget制限を、boardあり／なしとlibrary専用projectで確認する。 |
| [`embedded_firmware_state.rs`](../../emela-cli/tests/embedded_firmware_state.rs) | Unix上の2件。実CLI・driver・照合処理の接続、変更後のflash拒否、path／portを単一argvとして渡すこと、mock再build失敗後の旧成功記録の無効化。flashはdry-runに限定する。 |
| [`test_build_state.py`](../tools/test_build_state.py) | 13件。prepare／seal／verify、source・依存・設定・成果物・BSPの追跡、追加assemblyの周辺header、入力の追加・削除、symlinkの名前・実体・循環、seal中の変更拒否。 |

workspace rootでの再検証コマンドは次のとおり。ここにコマンドがあること自体は、現在のtreeで成功した記録を意味しない。

```sh
cargo test --locked -p emela-cli -p emela-driver -p emela-pome
cargo test --locked -p emela-cli --test embedded_firmware_state
python3 crates/emela-codegen-emb/tools/test_build_state.py
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo nextest run --workspace --locked --profile ci
```
