# 共通Cコンパイラーの構成

`compile`／`compile_flow`は、明示したIRとOptionsからplatform共通のC一式を返す。ファイル・環境変数・process・toolchainを参照せず、呼出し元のIRも変更しない。

```text
Core → input → Flow
                 ↓
          lower → LoweredModule
                    ↓
                   plan → PlannedModule
                            ↓
                           emit → Output
                                   ↓ 保存（実行側）
                     platform::prepare_connection → BuildPlan
                                                       ↓
                                            platform::execution
```

| 場所 | 責任 |
| --- | --- |
| `../input/` | Coreの便宜入口とFlow検証。`upstream/`は入力・lowerだけが使う上流adapter用の純粋な共通処理 |
| `../contract/` | 独立したbackend IR、段階の成果物、設定、診断、生成物／manifestのデータ。上流型のaliasやcompiler実装を含めない |
| `lower/` | 所有した入力copyの正規化、closure／handler／coerce変換、const評価、網羅的な上流→backend IR変換 |
| `plan/` | 型配置・所有権、String buffer、heap／runtime要求、標準サービス、ISR、suspend制御とframeの決定 |
| `emit/` | 計画を読み取ったC／headerの整形。`features/`は操作別renderer、`runtime/`は固定C資産 |
| `../semantics/` | 用途を限定した純粋な共有library。構造的な型照会、ABIの読み取り、文字列／数値規則、Unicode table、診断、C命名 |
| `../platform/contract.rs` | package、解決済みtoolchain情報、接続設定、BuildPlanのデータ |
| `../platform/plan.rs` | ABI・必要サービス・外部symbol・ISR保証の照合と具体的なcompile/link引数の作成 |
| `../platform/execution.rs` | 生成物の読込み、toolchain探索・確認、保存、外部process、成功記録とhash検証 |
| `../platform/packages.rs`、`../../boards/` | nativeとMCUのpackage構築・起動処理・実サービス・BSP資産 |

lower・plan・emitは、他段階の実装をimportしない。compiler直下には順序制御だけを置く。共有libraryはcontractを解釈する小さな規則に限定し、段階の入口や外部作用へ逆依存させない。局所的なMap、Writer、作業copyの変更は許容する。

Flow/Coreの型とvariantは入力側で解釈する。lowerのadapterはstruct fieldを網羅的に分解し、enum variantも網羅的に変換する。plan／emitは独自のID・型定義を使うため、上流の表現変更が直接後段の実装へ広がらない。意味やcontract自体が変わる場合は対応する段階の変更が必要になる。

Stringの処理方式・長さ・所有権・buffer容量、関数adapter容量、runtime helper依存、suspendの制御graphと生存変数はplanが決定する。emitはその成果物を使う。ABI読み取りとC整形は段階ごとのtrait実装に分かれ、契約型は実装を再公開しない。

標準`Std.Embedded.Board`のbinding認識は言語サービスABIとしてplanが担当し、値変換と宣言はemitが担当する。物理MCUのI/O実装やnativeの終了方針は含めない。共通entryは実行と結果の解放を行い、`emela_platform_start`が初期化・終了後の方針を決める。歴史的な`emela_board_*`名は共有hookの名前として残る。

## 検証と監査

`tests/architecture.rs`がcontractの逆依存、段階間の直接／再公開／相対import、上流型の漏出、外部作用、source includeを検査する。これは限定した構文検査であり、Rustの純粋性の完全な証明ではない。planとemitには、前段を呼ばずに手で組み立てたcontract fixtureを使う試験がある。

実装inventoryは、上流の定義ID、網羅的なadapter、`representation_owner`に指定した独立IRのdispatcherを区別して記録する。両enumのvariant名も照合し、定義・実装・関連テストの変更を検出する。段階別の追加担当は`phase_owners`に明示する。

同じOutputのnative／第三platformでの-O0/-O2実行、両MCUの接続計画、入力不変性、管理値の繰返し解放、String buffer、clock／console、保存後の独立build、成功記録の失効を試験する。MCUのcross buildと起動は別の環境検証として扱う。

```sh
cargo test -p emela-codegen-emb --locked --offline
cargo test -p emela-cli --test embedded_cli --test embedded_firmware_state --test embedded_dependencies --locked --offline
```
