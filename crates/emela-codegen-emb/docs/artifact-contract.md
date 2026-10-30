# 共通生成物とplatform接続の契約

`compile`／`compile_flow`は一度だけ共通C・header・manifest・exportsを生成する。保存した同じ一式をnativeや独自packageへ渡せる。同梱boardへの保存済みartifactの接続は今後の対象である。platform用のCは別directoryへ置き、共通Outputへ追加しない。

## manifest v2／ABI 2

```json
{
  "schema_version": 2,
  "abi_version": 2,
  "files": [
    {"path": "emela_embedded.h", "role": "public_header"},
    {"path": "emela_board.h", "role": "platform_header"},
    {"path": "emela_embedded.c", "role": "source"},
    {"path": "emela_embedded_exports.h", "role": "aliases_header"}
  ],
  "metadata": {
    "entry": "emela_embedded_start",
    "exports": [],
    "services": ["runtime.lifecycle"],
    "external_symbols": [],
    "isr_entries": [],
    "isr_safe_symbols": [],
    "task_capacity": 0
  }
}
```

`entry`は共通entryがなければnull。`exports`は`Output.exports`の保存可能な同じ情報で、source名・実際のC symbol・suspendsを記録する。型layoutやsignatureは生成headerを正本とし、manifestへ複製しない。異なる生成間でlayoutやexport番号が一致する契約ではない。

| role | 内容 |
| --- | --- |
| `source` | 1件以上のC11 translation unit。順序を保つ |
| `public_header` | `emela_embedded.h`。公開型・関数・所有権API。必須 |
| `platform_header` | `emela_board.h`。必要なplatform hook。必須 |
| `aliases_header` | `emela_embedded_exports.h`。`Options::with_export_alias`で選択した固定名 |
| `header` | 必要な標準サービス等の追加header |

pathは正規化した相対POSIX path。重複、空成分、`.`／`..`、backslash、colon、NUL、絶対pathを拒否する。readerは未知field・未知role・重複key・未対応版を拒否する。manifest自身はfilesに含めない。実行側readerはsymlinkによるdirectory外への脱出と列挙ファイルの存在も検査する。残存Cをglobでビルドしない。

ABI 2では共通entryが計算と結果・fiberの解放を担当する。platformは次を実装する。

```c
void emela_platform_start(void (*program)(void));
```

platformは初期化後にprogramを一度呼ぶ。nativeは復帰し、boardは終了後の待機方針を持つ。共通schedulerは進行中に`emela_board_idle`を呼ぶ。boardのidleは割込をmaskして`emela_scheduler_ready()`でdispatch／完了通知の有無を確認してから待機し、割込状態を復元する。確認APIはEmelaを実行しない。defect/debug、event lock、task完了、標準clock・console・周辺機器は必要なheaderの宣言に従う。String借用・所有値・caller buffer・エラー所有権は[実装契約](implementation.md)を維持する。

`services`は必要なサービス、`external_symbols`は追加Cが供給するsymbol、`isr_safe_symbols`は実際にISRから必要となる外部実装の保証である。compilerはISRの再帰・suspend・heap・動的呼出し等を検査し、接続時にはplatformの能力宣言と照合する。宣言だけでC signatureや実サイズが証明されるわけではなく、C compile/linkと動作検証も必要になる。

Rustの共通接続APIとBSPのPython readerはv2だけを受け付ける。v1の生成物は再生成する。

## 生成・接続・実行

`Output.files`をそのまま保存する。aliasはcompile設定として指定し、driverが生成後にCoreを読み直してheaderを追加する処理はない。`refresh_manifest`はファイル一覧を更新する互換helperで、既存の必要サービス情報を保持する。platform追加のためには使わない。

```rust,ignore
let output = compile_flow(&flow, &options)?;
let package = platform::native();
let tools = platform::execution::inspect_toolchain(Path::new("cc"))?;
let plan = platform::prepare_connection(
    &output, &package, &platform::ConnectOptions::default(), &tools,
)?;
let executable = platform::execution::execute(&plan, build_dir)?;
```

`prepare_connection`は純粋な計算であり、解決済みの入力から能力検査、配置内容、全compile/link引数を返す。実行側だけがfilesystem、環境、toolchain、processに触れる。`common/`、`platform/`、`entry/`、`out/`を分け、元のartifactを変更しない。

nativeは単一threadのconsole、Unix epochのclock、単調clockを使う固定容量timer、defect/debug、task lockを提供する。ISRや別threadからの完了通知は受け付ける契約ではない。独自の単一threadの非同期producerは、packageの追加Cと`emela_native_set_poll`でidle時のpollを登録できる。未対応のGPIO等は接続時に不足サービスとして診断する。

MCU packageは将来の接続に向けて`platform::board(id)`で構築・計画できる。解決済みcross compilerを渡し、package自身がreset/mainを持つので`ConnectOptions.main=false`にする。STM32のprofileからのheader・vector・linker生成も純粋なpackage構築に含まれる。既存BSPは`board_main_start`を使う引数なし・Int戻り値のsuspend entryを要求する。保存済みartifactを同梱MCUへ接続するCLIは未対応である。現時点のfirmware生成・書き込みは`emela build --board <id>`／`emela flash --board <id>`を使う。この経路では資源計測・image作成・source鮮度確認も行う。

## Bytesの公開型

生成headerは不変byte列を`EmelaBytes`で表す。`data`は`const uint8_t *`、`length`は0から`INT32_MAX`の`uint32_t`、`owner`は固定heapのbacking storageか借用viewの`NULL`。公開入口が返した所有済みBytesには`emela_embedded_retain_bytes`と`emela_embedded_release_bytes`を使う。借用viewが生成処理の呼出し後も保持される場合は内部でコピーし、外部のbufferを後から変更しても保持した値に影響しない。

Byte列の容量は固定heapの残量で制約される。UTF-8からStringへの変換には、別途String用bufferの容量が適用される。同期C外部接続では、ほかの生成された管理値と同様に明示した`Borrowed`／`Owned`のadapterを使う。
