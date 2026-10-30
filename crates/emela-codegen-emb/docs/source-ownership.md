# 組み込みソースとテスト資産の所有範囲

組み込みインターフェースを更新するときにcoreの標準ライブラリや他backendのテスト実装を編集する必要がないよう、宣言・handler・frontend helper・fixtureを`emela-codegen-emb`内に置く。`pr/codegen-emb-ecosystem`の`b5d893a`を起点にした配置の整理であり、周辺機器APIの拡張を含めない。

## Emelaソースの供給

[`lib/embedded/`](../lib/embedded/)の`Peripheral`・`Gpio`・`Uart`・`I2c`・`Spi`・`Board`を、[`bundled_sources()`](../src/sources.rs)でコンパイラーへ供給する。ファイルはRust crateに埋め込むため、利用時にソースcheckoutを読む必要はない。

coreの`core_sources()`には組み込みmoduleを登録しない。driverの`CoreSource::bundled()`がcore/stdとbackendのソースを合わせ、CLIのcheck・buildやlanguage serverで使う。独自frontendも名前解決の前に同じ一覧を組み立てる。

```rust
let sources: Vec<_> = emela_core::core_sources()
    .iter()
    .chain(emela_codegen_emb::bundled_sources())
    .copied()
    .collect();
```

`compile`や`compile_flow`は既に解決・型検査・loweringされたIRを入力に取る。これらを呼ぶ前のfrontendでソースを追加する必要がある。

6つのEmelaファイルの内容、`Std.Embedded.*`のimport名、`PeripheralError`、C ABI 2、artifact schema 2は起点から維持する。独立Pome化や名前空間の変更は行わない。coreのlowering snapshotからは、同梱しなくなった組み込み型の表示だけを取り除く。

## テスト補助とfixture

- [`tests/support/frontend.rs`](../tests/support/frontend.rs)は公開resolver・type checker・Core APIで小さなEmelaプログラムを処理する。coreの非公開`tests/support`への相対path依存を持たない。
- [`tests/support/embedded_sources.rs`](../tests/support/embedded_sources.rs)はbackendのソースを追加する。integration testとRust exampleが共用し、Board handlerを含めて型検査・loweringする。
- [`tests/bytes/fixtures/`](../tests/bytes/fixtures/)はBytesの試験入力と期待値を所有する。他backendの非公開テストデータをincludeしない。
- [`tests/sources.rs`](../tests/sources.rs)は組み込みソースの所有元・構文と、Rustの`#[path]`・`include!`・`include_str!`・`include_bytes!`がcrate内へ解決することを検査する。

frontend helperはcoreの公開APIを使う小さなローカル実装として維持する。core側のテスト専用変更を自動で取り込まず、API変更への追従はこのcrateで検査する。

## 依存の境界

core・Flow・resolver等は引き続き通常のRust依存として利用する。`Std.Clock`・`Std.Console`等の基本stdも使用する。driverにはbackendのソース供給APIを呼ぶ接続を置く。

IRの変更に対する監査ツールは、これまでどおり`--root`で指定されたecosystemの上流IR定義とbackend実装を走査する。この結合検査の入力を複製したり、上流IRとの追従検査を弱めたりはしない。Rust crateのworkspace外への抽出と監査環境の移行は別途扱う。

## 検証方法

```sh
cargo test --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo fmt --all --check
scripts/spec.py check
bash crates/emela-codegen-emb/tools/inventory.sh all
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s crates/emela-codegen-emb/tools -p 'test_*.py'
```

driverには標準の型検査frontendとC Embeddedのloweringを通してBoard handlerを使う回帰試験も置く。既存の周辺機器試験は共有handlerをlowerしてC mockへlinkし、値・status・所有権を確認する。

## 生成物の比較（2026-10-30）

起点の`b5d893a`と移動後のそれぞれでCLIと`board_probe`をbuildし、同じ入力から生成した全ファイルの相対pathとSHA-256を比較した。

| 入力 | 比較したファイル | 結果 |
| --- | ---: | --- |
| 周辺機器fixtureの`all`を公開したC生成 | 12 | 全て一致 |
| `board_probe --board stm32f446re`のC/BSP生成 | 21 | 全て一致 |
| `board_probe --board esp32c3`のC/BSP生成 | 16 | 全て一致 |

周辺機器fixtureは`tests/embedded_peripherals/exercise.emel`を同じ絶対pathの一時projectへコピーし、`all`に`@export(c, "ownership_all")`を付けた。空の`Pome.toml`と、`heap_bytes = 8192`・`string_capacity = 96`・`peripheral_buffer_capacity = 4`・`frame_bytes = 2048`の設定で、boardを選ばずにCを生成した。Board exampleはCとBSP入力を生成し、cross compilerを起動しない。

計49ファイルに差分はない。移動した6つのEmelaファイルとローカル化したBytesの2 fixtureも起点とbyte単位で同一である。coreの4 snapshotは組み込み型の表示の削除だけで、追加行はない。

同日にworkspace全体のテスト、Clippy、fmt、仕様検査、Pythonの29試験が成功した。生成Cの395試験、platform接続7試験、所有範囲2試験、core lowering 25試験とdriverの標準frontend接続試験を含む。inventoryの8 reportを再生成し、0 missing context records・0 representation gaps・0 route errorsで、追跡済みの生成物に差分はなかった。

Wasmtime・MCU用cross compiler・RenodeはPATHにない。WASM実行を必要とする試験内のskipを実行確認済みとして数えず、firmwareのcross build・emulator・実機通信も今回の検証範囲には含めない。
