> この文書の現状調査は改修前の記録です。現在の構成は[src/compiler/README.md](../src/compiler/README.md)、共通ABIと接続手順は[artifact-contract.md](artifact-contract.md)を参照してください。

# ディレクトリ構成の調査と改善案

調査日: 2026-10-27。対象: `crates/emela-codegen-emb`、関連するdriver・CLI・CI。調査時のcommit: `a527c33`。

現在の構成では、**IRの準備、解析・計画、Cの生成、生成先で動くruntime、BSP、開発用の監査処理という異なる分類軸が重なっている**。特に、`foundation/`という配置がRustのmodule境界を表しておらず、`compiler/mod.rs`と`TypeLayouts`に多くの責務が集まっている。

改善の中心は、処理段階を最上位のディレクトリ境界とし、段階ごとの成果物を通して依存する構成にすること。最終目標には、**一度だけ生成した同じ`Output`を変更せず、board・native・追加のplatform実装とそれぞれbuild/linkできること**を含める。

まず同一crate内で段階を整理し、その後に接続先に依存しないC ABIと生成物の境界を整える。配置の整理では既存契約を維持し、ABI共通化による変更は互換経路と版管理を伴う独立した変更として扱う。

この文書は調査結果と提案であり、以下の構成への移行はまだ実施していない。

## 1. 現在の構成と処理の流れ

```text
src/lib.rs
  compile(core)
    → flow_from_core(core)          Coreの単相化・const由来情報・Flow検証
    → compile_flow(flow)
        → backend/compiler         Flowの正規化・closure等のlowering
            → CompiledModule
        → backend/board::link
            → compiler::emit       配置・資源計画・C生成・接続先の処理
                → Output           C / headers / artifact manifest

src/boards.rs
  boards/のPython / C / startup / profile等を埋め込み
    → driverがBSPとして展開・build等を実行
```

`CompiledModule`と`Target`による共通コンパイル／接続先の分離は既にある。`backend/board`はSTM32等の個別実装ではなく、標準周辺機器bridge・ISR検証・embedded起動処理を担当する。一方、トップレベルの`boards/`が具体的なBSPの実装を持つ。

根拠: [公開入口](../src/lib.rs)、[backendの入口](../src/backend/mod.rs)、[共通コンパイル](../src/backend/compiler/mod.rs)、[接続契約](../src/backend/compiler/target.rs)、[embedded接続](../src/backend/board/mod.rs)、[BSP登録](../src/boards.rs)。

### 規模の確認

調査時点の改行区切りの行数。生成物・fixtureを含むため、行数は複雑さの評価ではなく配置の把握に使う。

| 対象 | 規模 | 確認した特徴 |
| --- | --- | --- |
| `src/backend/compiler/` | Rust 124ファイル | 119ファイルに`use super::*`または祖先への同種の一括importがある |
| `compiler/mod.rs` | 1,889行、トップレベル関数70個 | module登録・lowering・C生成・ABI検証・命名・診断が同居 |
| `compiler/foundation/` | Rust 20ファイル、7,223行 | `mod.rs`がなく、各ファイルを親の`#[path]`で登録 |
| `compiler/features/` | Rust 99ファイル、10,835行 | IR要素の担当、生成、解析用の情報提供を集約 |
| `foundation/layouts.rs` | 2,080行 | 型配置に加えて所有権、closure、runtimeの要求等を保持 |
| `tests/emit.rs` | 6,055行、直下の`#[test]`119個 | 分野別submoduleに加えて共有harnessと多数のテスト本体を保持 |
| `examples/support/` | 36ファイル、2,846行 | probe用入力と監査の本体が同居 |
| `docs/generated/` | 8ファイル、30,242行 | 自動生成された監査結果。実装の手作業による整理とは区別する |

一括importの件数だけで不適切な依存を証明できるわけではない。ただし、何を共有しているかがimport一覧から分からず、境界変更の影響範囲を読み取りにくい。

## 2. 混在している箇所

### A. `foundation/`が複数の責務の置き場になっている — 優先度: 高

同じフォルダに次の実装がある。

| 実際の役割 | 現在のファイル例 |
| --- | --- |
| IRの書換え | `type_normalize.rs`, `fiber_normalize.rs` |
| 走査・到達可能性 | `traversal.rs`, `reachability.rs` |
| 資源とbufferの計画 | `heap_plan.rs`, `string_buffers.rs`, `runtime_plan.rs` |
| 型表現・外部呼出しの契約 | `layouts.rs`, `external.rs` |
| 生成Cのruntime helper | `numeric_runtime.rs`, `string_runtime.rs`, `owned.rs`, `cause.rs`, `deferred.rs`等 |

さらに、これらはRust上で`compiler::foundation::*`になっていない。[`compiler/mod.rs`](../src/backend/compiler/mod.rs)の先頭から、`#[path = "foundation/…"] mod …;`として直接登録される。

そのため、フォルダを見ても「ここに追加してよい処理」が判断しづらい。単に`foundation/mod.rs`を追加するだけでは、この責務の混在は残る。

**改善案:** IR書換えを`lower/`、解析・ABI・資源の決定を`plan/`、Cとruntimeの出力を`emit/`へ振り分け、通常の`mod`宣言で実際のmodule境界と一致させる。共有情報の型を定義する`analysis.rs`は、段階間の契約型と各段階内部の情報に分ける。

### B. `compiler/mod.rs`が共有の実装本体になっている — 優先度: 高

現在はコンパイルの順序制御に加えて、次の処理が残っている。

- local宣言、closure環境、所有権の取得・解放。
- 関数値のABI検証、const getter、失敗辺、wrapper・adapterの生成。
- Cのidentifier検証、文字列・数値literal、symbol名の生成。
- statementやsequenceの出力、共通のエラー生成。

例えば、`compile_flow`は94行目、所有権cleanupは278行目、Cのidentifier検証は1,246行目、関数ABIのsignatureは1,370行目にある。

**改善案:** `mod.rs`にはmodule宣言とコンパイルの順序制御を残す。C出力の処理を`emit/`、ABIの判定を`plan/abi/`、共通診断を専用moduleへ移す。`LoweredModule`と`PlannedModule`で段階の成果物を表す。内部の再公開は移行中の互換性に限定し、祖先moduleを経由する一括importを徐々に明示的なimportへ置き換える。

[`suspend.rs`](../src/backend/compiler/suspend.rs)も1,701行あり、検証・Graph構築・frameと関数の生成・runtime組立てを持つ。こちらは同様の境界が固まった後に分割する。

### C. `TypeLayouts`が型配置以外の情報も集約している — 優先度: 中〜高

[`TypeLayouts`](../src/backend/compiler/foundation/layouts.rs)は、型とaggregateの配置だけでなく、`discarded_results`、`runtime_consts`、owned returnの情報、closure環境、Deferredやchildの型一覧、runtimeの有効化flag等を保持する。

[`emit.rs`](../src/backend/compiler/emit.rs)は、構築後の`TypeLayouts`へこれらの情報を追加してから生成を進める。ファイルを移しても、処理間の共有状態への依存は残る。

**改善案:** 最初は型を維持したまま配置と呼出し元を整理する。その後、型配置、関数ABI・所有権、runtimeの要求を別の情報としてまとめる。各計画を確定してから生成処理に渡し、生成中のcontextの変更を減らす。具体的な型分割は、情報の計算順と相互依存を確認して実施する。

### D. runtimeの意味と配置が複数ある — 優先度: 中

| 配置 | 中身 |
| --- | --- |
| `src/runtime/*.c`, `*.h` | 生成Cへ組み込むscheduler・task等のC断片 |
| `src/heap.rs`, `float_parse.rs`, `float_to_string.rs` | Cのruntimeを出力するRust実装 |
| `compiler/foundation/*_runtime.rs`等 | 共有helperや型に応じたruntimeを出力するRust実装 |
| `compiler/features/builtins/`等 | 特定の操作に密接なhelperと、その必要性の判断 |
| `boards/stm32/stm32f4/runtime.c`等 | MCU依存の起動・周辺機器・platform hook実装 |

**改善案:** runtimeの要求を決める処理を`plan/`、RustでCを出力する処理を`emit/runtime/`、固定C断片を`runtime/c/`、具体的なplatform実装をBSP等へ置く。操作の分類はplanの成果物または純粋な共通の意味判定として共有し、emit側で再判定しない。

[`const_runtime.rs`](../src/const_runtime.rs)は`prepare`によるIR書換えと`emit_runtime`によるC出力、空Listの解析を同じファイルに持つ。これはそれぞれlowering・runtime生成・analysisへ分ける候補になる。

[`unicode_grapheme.rs`](../src/unicode_grapheme.rs)には共有テーブル、Rustでの文字数計算、C出力があり、Rust側は[`const_eval.rs`](../src/const_eval.rs)からも使われる。runtime側へ丸ごと移すより、共有データ・Rust側の意味計算とC出力を分ける。Unicodeデータは複製しない。

### E. test・example・監査処理の所有場所が逆転している — 優先度: 中

- [`tests/implementation_inventory.rs`](../tests/implementation_inventory.rs)は`examples/support/feature_inventory.rs`と`implementation_inventory.rs`を直接読み込む。
- [`tests/coverage_audit.rs`](../tests/coverage_audit.rs)と[`examples/check_flow_coverage.rs`](../examples/check_flow_coverage.rs)は同じ`examples/support/flow_audit_inventory.rs`を使う。
- [`tools/backend-inventory.sh`](../tools/backend-inventory.sh)はignored testを実行して成果物を再生成する。他の監査scriptはexampleを実行する。
- `tests/emit.rs`とそのsubmoduleは、example側のEmela fixture・Flow builderを参照する。
- `src/`のunit test・`tests/`・`examples/`の計23ファイルが、他crateの`emela-core/tests/support/mod.rs`を直接読み込む。

`examples/`を見るだけでは、利用例・検証probe・監査の共通ライブラリの区別がつかない。

**改善案:** 監査処理本体を`tools/audit/`、このcrate内で共有するharness・Flow builder・入力を`test-support/`へ置く。exampleとtestは薄い入口として共通処理を利用する。監査用の小さなexample wrapperは、現在の実行コマンドの互換性のために残せる。

`emela-core/tests/support`への依存の解消は、workspace全体のtest支援処理の共有方法と合わせて扱う。今回の整理だけで同じlowering helperを複製する案は採らない。

## 3. 維持したい設計

次の分離には既に実装と検証がある。

- **共通コンパイルと接続先:** `CompiledModule`をembeddedとhost試験で再利用できる。compilerから具体的な接続先を参照しない検査もある。この境界を生成物まで進め、同じ`Output`を再利用できるようにする。
- **機能の意味と共有計画:** `features/`がStringのbuffer・所有権・必要なruntime等の情報を提供し、共有計画がそれを集約する。段階分離後も、同じ操作の分類を一箇所で決める性質を維持する。
- **C生成とBSP build:** `emela_artifact.json`とC headerが境界になっており、BSPはFlow IRを必要としない。
- **実装担当と検証の対応:** `inventory/`と生成される一覧が、IR要素・実装入口・テストの対応を確認する。

例えば[`string_append.rs`](../src/backend/compiler/features/builtins/string_append.rs)には、C生成と`mode`・`string_behavior`・`string_bound`・`result_ownership`がまとまっている。段階分離後はplanが操作の分類とbuffer・所有権を決め、emitへ渡す。同じ判断を両段階へ複製しない。複数段階で必要な純粋な判定は、段階に依存しない`semantics/`へ置ける。

また、短いvariant別ファイルには、監査上の担当入口や明示的な入力拒否という役割がある。行数だけを理由に統合せず、機能別の粒度は各段階の配下で維持する。担当の移動はinventoryへ反映する。

## 4. 到達先の構成案

最上位の分類を処理段階に統一する。ABIやruntime要求の決定は`plan/`、Cのhelperとruntime生成は`emit/`へ置く。各段階の内部を機能別に整理する。

```text
emela-codegen-emb/
├── src/
│   ├── lib.rs                 公開APIと再公開、薄い入口
│   ├── options.rs             設定と外部adapter指定
│   ├── error.rs               公開診断型
│   ├── output.rs              生成結果の型
│   ├── artifact.rs            公開の生成物契約
│   ├── boards.rs              公開のBSP登録・取得API
│   ├── input/                 Core→Flow、入力検証、derived登録
│   ├── semantics/             純粋な操作の意味、const評価、共有Unicodeデータ等
│   ├── contract/              共通C ABI、binding契約、module要求のデータ型
│   └── backend/
│       ├── mod.rs             共通生成の入口
│       ├── compiler/
│       │   ├── mod.rs         lower→plan→emitの順序制御
│       │   ├── lower/        Flow正規化、closure・handler等のIR書換え
│       │   ├── plan/
│       │   │   ├── abi/      C型配置、関数と外部bindingの契約
│       │   │   ├── operations/  操作別の意味情報・実装方式の決定
│       │   │   └── …         所有権、buffer、heap、runtime要求等
│       │   └── emit/
│       │       ├── values/   値のC出力
│       │       ├── statements/
│       │       ├── builtins/
│       │       └── runtime/  共通・型別helper、entry wrapperのC出力
│       └── platforms/        生成後の契約確認・接続・build。Flow IRに依存しない
│           ├── board/       BSPと起動処理、ISR登録、実機I/Oの接続
│           └── native/      host用起動・I/O実装、実行形式のbuild
├── runtime/c/                 生成Cへ組み込むC / header断片
├── bsp/                       現在のboards/、具体的なMCU・board資産
├── test-support/              crate内の共有harness・builder・fixture
├── tests/                     分野別の検証と監査処理の回帰試験
├── examples/                  利用例、probe、互換性用の薄い監査入口
├── tools/audit/               Rustの監査処理本体
├── tools/                     監査入口とboard probe実行script
├── inventory/                 owners / containers / tests / routes設定
└── docs/                      利用説明、設計説明、generated監査結果
```

### 段階の成果物と依存

```text
Core → input → Flow
                 ↓
               lower → LoweredModule
                          ↓
                        plan → PlannedModule
                                 ↓
                               emit → Output（共通C・header・manifest）
                                         ├─ board用platform/BSP ＋ toolchain → firmware
                                         ├─ native用platform ＋ host cc → executable
                                         └─ 追加platform ＋ toolchain → 実行形式
```

`LoweredModule`と`PlannedModule`は各段階の入口から公開する成果物とし、後段は前段の内部実装・可変contextへアクセスしない。既存Flowと補助情報を包む形から始め、段階ごとに別の大きなIRを作ることは前提にしない。

planは操作の分類、binding ABI、所有権、String buffer、heap、runtime要求等を決定する。emitは決定済みの情報を参照してCを出力し、IRの書換えや同じ意味判断をやり直さない。複数段階で使う純粋な意味判定は`semantics/`へ置き、そこから各段階の実装・`Writer`・具体的なplatformを参照しない。

共通生成が受け取るのは、入力、資源上限、公開するbindingの契約等である。物理board名、host用実装symbol、toolchain、起動後のidle方針はplatform側の設定とする。共有の走査処理等は用途を限定したmoduleとし、汎用的な共有contextの置き場を作らない。

### 同じ`Output`を使うという完了条件

**同じ入力・共通設定で一度だけ生成した`Output.files`と`exports`を固定し、その一式をboardとnativeへ渡す。各platformは共通生成を再実行せず、生成C・header・manifestを書き換えない。** platform固有のstartup、adapter、linker script等は別の入力としてbuildへ加える。platformのsourceを`Output.files`へ追加してmanifestを更新する現在のnative経路も、この形へ置き換える。

ここで共通化するのはC sourceと公開する接続規約である。hostとMCUで同一のobjectやbinaryを使うこと、pointer幅等によらずstructのサイズが一致することは求めない。追加Cは同じ生成で出力したheaderを使い、それぞれのtoolchainでcompileする。

pool容量やbindingの所有権契約等を変える場合は別の共通生成になる。接続先の追加・選択だけでこれらを変えず、既存の契約に各platformの実装を適合させる。

### 共通C ABIとplatform境界の設計

| 判断 | 置き場所 |
| --- | --- |
| IRを書き換え、closure・handler等を準備する | `input/`, `compiler/lower/` |
| C型・関数ABI、binding、所有権、資源、helperの要否を決める | `compiler/plan/` |
| 決定済みの計画をCと共通glueにする | `compiler/emit/` |
| String、managed値、失敗値の取得・解放規約を定義する | `contract/`と生成header。どのplatformでも同じ規約を使う |
| 同期外部呼出しとtaskの開始・完了・失敗・取消の規約を決める | 共通binding ABI。実装固有の規約との変換はplatformのadapter |
| 言語側の標準embedded interfaceを認識し、値をCサービスABIへ変換する | target中立のbinding計画と共通glue生成 |
| entryを開始・進行させ、返り値・fiberの所有権を扱う | 共通のentry wrapperと生成header |
| processの`main`、reset後の起動、完了後のreturn／idle、OSとの連携 | `platforms/board/`, `platforms/native/`等の起動処理 |
| defect、debug、event lock、周辺機器等のサービスを実装する | platform/BSP。必要なサービスの宣言は共通生成物 |
| ISR登録の可否、サービスの有無、資源と実行環境の適合を検証する | Outputに記録した要求とplatform能力の照合 |
| レジスター、startup、linker script、toolchainを扱う | platform/BSPとbuild処理 |

現在の`Target`はbindingの解釈・検証・header生成・entry生成を兼ね、planとemitの両方に相当する処理へ関与する。同じ`Output`を実現するには、これらを上表の契約・共通生成・platform実装に分解する。具体的な`Target`を引数にして同じIRから別々のCを生成する経路は、共通生成の最終形には残さない。

特に借用／所有／caller bufferとtaskの契約は、各bindingについて共通生成時に固定する。boardとnativeで返り値の所有権が異なる場合、各adapterが共通契約へ変換する。bufferやheapの計画を接続先ごとに差し替える必要をなくす。標準周辺機器をnativeで扱う場合は同じサービスABIのhost実装・模擬実装を提供し、未対応サービスを空の成功処理で代替しない。

共通entry wrapperには、同期・suspend、String出力buffer、managedな返り値と失敗の処理を含める。無限idle loopやprocess終了は含めず、起動処理が公開操作を呼んで制御する。操作のsignatureはこの契約を実装する段階で確定し、両platformが同じheaderを利用する。現在のnative側だけにある返り値解放等も、共通契約で保証する。

### 生成後だけで接続するための情報

platform/build側はEmela source、Core/Flow IR、`CompiledModule`、`TypeLayouts`、compiler内部の`Writer`を受け取らない。保存された生成物とplatform packageだけで接続・検証できるよう、次の情報を公開する。

- C header: 値の表現、export、共通entry、必要なサービス、binding signature、所有権、retain/release、task ABI。signatureの正本はheaderとする。
- manifest: 全生成ファイル、schema/ABI版、必要なサービス・bindingの識別、runtime資源要求、entryやISR登録の適合確認に必要な性質。C宣言を別形式で重複定義しない。
- platform package: 提供するサービス、対応ABI、資源とISR等の能力、実装source、startup、必要なbuild設定。

現行manifest v1はファイル一覧が中心で、readerが未知fieldを拒否する。要求情報の追加とABI変更の範囲を確定し、schema/ABIをそれぞれ適切に更新する。RustとPythonのreader、fixture、BSP、driver/CLIを同時に移行する。platform能力が不足する場合は、IRを読み直さず、要求と不足している実装・制約をbuild/link前に診断する。

### 新しいplatformの追加

既存ABIに収まるplatformは、packageの登録、サービス・起動処理、build設定、適合試験を追加すれば利用できることを完了条件にする。lower・plan・emitにplatform名による分岐や接続先用の再生成処理を追加しない。

共通ABIにないサービスを追加する場合は、契約の拡張として別に扱う。既存契約に適合しない環境を無条件にリンク可能とはせず、対応するABI版・能力を明示する。nativeは内部試験専用から、共通生成物を消費する正式なplatform経路へ移行する。

## 5. 段階的な移行案

| 段階 | 変更内容 | 完了条件 |
| --- | --- | --- |
| 1. 大きな親moduleを薄くする | `compiler/mod.rs`から命名・literal・診断・関数出力等を抽出し、`emit.rs`を`emit/`へ展開する。まず独立性の高い処理から移す | 順序制御と各生成処理の入口が追える。既存APIと生成結果を維持する |
| 2. 段階別のmoduleへ分離する | `foundation/`と`features/`をlower・plan・emitへ振り分け、通常のmodule宣言と明示的なimportへ変更する。共通の意味判定は複製しない | 配置とmodule境界が一致し、各段階の実装の所属が明確になる |
| 3. 段階の成果物を確立する | `LoweredModule`と`PlannedModule`を導入し、`TypeLayouts`から計画情報を分ける。root直下、const、Unicodeも段階に合わせて整理する | emitが確定済みの計画を読み、IRの書換えや計画の再計算をしない |
| 4. 共通C ABIと要求情報を確定する | `Target`の責務を分解し、binding所有権・task・entry・platformサービスの契約、manifestの要求情報と能力照合を設計する | 物理platformを選ばずに計画できる。必要なschema/ABIの変更と互換経路が決まる |
| 5. 同じ`Output`を生成する経路を実装する | 共通entry・型別glue・runtimeを一度だけ出力し、platform固有startup・adapterを生成物の外へ出す | `Output`生成に具体的なboard/nativeの実装を使わず、共通C・header・manifestが確定する |
| 6. board/nativeを生成物の消費側へ移行する | platformの登録、能力検証、外部サービス・起動処理を整備し、BSP／host toolchainでbuildする。driver/CLIへ接続する | 同じ一式を変更せず両方で使い、sourceやIRなしでbuild/linkできる。nativeが正式経路になる |
| 7. 第三のplatformで拡張性を検証する | 試験用の最小platform packageを追加し、登録とC実装・build設定だけで接続する | compiler本体にplatform固有の変更をせず追加でき、不足能力も生成物だけから診断できる |
| 8. 検証資産と資産名を整理する | harness・fixtureを`test-support/`、監査本体を`tools/audit/`、routesを`inventory/`へ集める。`boards/`→`bsp/`、C断片→`runtime/c/`等を整理する | 監査と利用手順が新構成へ移行し、BSP公開APIと展開asset名を維持する |

依存順は1→2→3→4→5→6→7とする。8の検証支援の抽出は必要になった段階で先行でき、資産名の変更はbuild経路が安定してから行う。各段階は独立してreviewできる変更に分ける。特に1・2では所有権やsuspendの意味を変えず、配置・可視性の調整と振る舞いの変更を分ける。3の共有状態の分割と4以降のABI共通化は、それぞれ独立した変更として扱う。

移行中は既存の公開compile APIと従来ABIの利用経路を維持し、共通生成は追加の入口・明示的なmode等で導入する。具体的なAPI名は段階4で確定する。既存利用者を移行する段階でdriver/CLIとdocsを更新し、ABIや起動処理を暗黙に置き換えない。

`Options`や`ExternalAdapter`を別moduleへ移す場合、rootで定義されたprivate fieldを子moduleから読めていた現在の可視性は、そのままでは維持されない。`pub use`で公開APIを保ちながら、内部accessor等で必要なアクセスを定義する。移動のためだけに全fieldの可視性を一律に広げない。

### 最初の変更として推奨する範囲

段階1のうち、**Cの命名・literal・診断の抽出**から始める。`compiler/mod.rs`の`is_c_identifier`、`c_identifier`、各symbol名、`c_string_literal`と数値literal、共通診断を担当moduleへ移す。新しい位置から明示的に利用し、機能別の処理本体や生成順序を変更しない。

これによって親moduleへの依存を減らす手順を小さく確認できる。その後、関数ABI・所有権・suspend等の相互依存が強い部分へ進む。

crateの分割は、上記の内部境界が明確になってから再評価する。現段階でcompiler・runtime・BSPをそれぞれ別crateへ切り出すと、現在の共有contextと埋め込み資産の境界を同時に変更する必要がある。

## 6. 移行時に更新が必要な参照と契約

ファイル移動だけでは検査が通らない箇所に加え、共通生成で変更が必要な契約がある。

| 対象 | 必要な対応 |
| --- | --- |
| `inventory/owners.json`, `containers.json` | source pathと担当symbolを更新する |
| `tools/flow-implementation-routes.json` | source path、symbol、分岐・呼出しの根拠を更新する |
| `inventory/tests.json` | module名・test targetが変わる場合に完全なテスト名を更新する |
| `docs/generated/` | 既存scriptで再生成する。移動で変わる行番号等も含めてreviewする |
| `src/backend/tests/separation.rs` | compiler走査rootとplannerへの`include_str!`、接続先名の検査を更新する |
| `src/boards.rs` | BSP資産の`include_bytes!`と`BUILD_STATE_SCRIPT`を更新する。展開先のasset名を維持する |
| `tests/scheduler_runtime.rs`等 | C断片のinclude先とfixtureの相対参照を更新する |
| toolsのPython試験・board probe、CLIの`embedded_cli`試験 | `boards/`や`tools/`への固定pathを更新する |
| `src/backend/compiler/target.rs`, `board/`, `cnative/` | 現行のbinding・entry・検証を共通契約／生成とplatform実装へ分解し、生成後の処理からcompiler内部への依存を外す |
| `src/artifact.rs`, `boards/artifact_manifest.py`, 正常・拒否fixture | 要求情報と版管理を追加し、Rust／Pythonで同じ共通生成物を検証する |
| driverのembedded生成・board処理、CLIのbuild・flash処理 | 共通生成物とplatform固有入力を別に渡す。両方を成功記録とhash照合へ含める |
| `docs/artifact-contract.md`, `embedded-interfaces.md`, `boards.md`, `cli-embedded.md` | ABI・サービス・entryの共通契約、platform追加、nativeの利用、旧経路からの移行を説明する |
| `.github/workflows/ci.yml`、README・docs | 実行コマンドや設定の場所が変わる段階で更新する |

test targetを初回から分割する必要はない。まず`tests/emit.rs`からharnessとテスト本体をsubmoduleへ抽出すれば、target名を保てる。ただしsubmoduleへ移したテストは完全名が変わるため、inventoryの更新が必要になる。

## 7. 調査時の検証と移行時の確認

以下は初回調査で実行し、成功した。段階分離や共通`Output`の実装後の結果ではない。

| 実行した検証 | 結果 |
| --- | --- |
| `cargo test -p emela-codegen-emb --lib backend::separation_tests --locked --offline` | 12件成功。共通コンパイル／接続先の分離、生成Cの実行等 |
| `bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh --check` | 415 records確認 |
| `bash crates/emela-codegen-emb/tools/backend-inventory.sh --check` | 対応表の一致を確認する1件成功 |
| `bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh --check --require-evidence` | 158 variants、missing context 0、orphan ID 0 |
| `python3 -B -m unittest discover -s crates/emela-codegen-emb/tools -p 'test_*.py'` | 28件成功。artifact契約・BSP build入力・build状態・probe支援処理等 |

調査ではcrate全体のテスト、compiled test inventoryの全件照合、実機flash、Renode/QEMUによる実際のboard probeは実行していない。現状の機能全体を検証したという意味ではない。

移行では、変更した境界に対応する試験を実行する。compiler内の抽出では分離試験と関連する生成・実行試験、BSP資産の移動ではartifact・boards・Python試験とdriver/CLIの関連試験を使う。参照を書き換えた後はinventoryを再生成し、READMEに記載した`--check`と`test-inventory.py --check-built`を実行する。

配置だけを変える変更では、同じ入力・Optionsに対する`Output.files`と`exports`の比較を、同期・suspend・managed値・外部binding等の代表例で行う。manifestの形式・ABI版、公開symbol、BSPとして展開される資産名も維持する。生成順序を変更する必要が出た場合は、その理由を別途reviewする。共通ABIへの移行では、旧経路の互換性と新しい生成物契約をそれぞれ検証する。

### 共通`Output`の受入条件

| 検証 | 合格条件 |
| --- | --- |
| 生成物そのものの再利用 | 生成を一度だけ行い、全`Output.files`と`exports`を保存する。同じ一式をboard/nativeの独立したbuild directoryへ渡し、build前後で内容・hash・公開情報が変化しない |
| 生成後の独立性 | 共通一式とplatform packageをcheckout外へコピーする。元のEmela sourceやIR、Rustのcompiler APIを利用せずbuildでき、platform側からlower/plan/emitを呼ばない |
| 同期entry | scalar・String・managed aggregateの返り値と失敗を同じ公開ABIで扱う。nativeは完了後に戻り、boardはplatform側でidle方針を選択できる |
| 非同期entryと外部task | 同じ一式でscheduler、taskの成功・失敗・取消を扱う。結果・fiber・taskの解放を繰り返して固定poolやheapが失われない |
| 外部bindingと標準周辺機器 | 同じheader・binding契約へ実機側とhost模擬実装を接続する。借用／所有／caller bufferの変換とevent lock等を確認する |
| board側のbuild | 同じ共通Cを実際のcross toolchainでcompile/linkし、startup・linker・容量制約を確認する。実行の確認は対象BSPのRenode/QEMUまたは実機で行う |
| native側のbuild | 同じ共通Cをhost ccの`-O0`／`-O2`でcompile/linkし、結果と終了・解放を確認する |
| 第三のplatform | 登録、サービスと起動処理、build設定だけを追加し、compiler本体を変更せず保存済み生成物へ接続できる |
| 不適合の診断 | 未対応ABI、必要なサービスの欠落、ISR制約等を要求情報と能力から診断する。未対応処理を成功扱いにせず、共通生成物も書き換えない |

hostで実行するboard用fixtureは、共通ABIと起動制御を検証する速い試験として用意する。これだけで実際のMCU向けcompile/linkや起動を検証したことにはしない。共通一式の同一性はsourceとheaderで確認し、各toolchainで生成したobject・firmware・executableは別の成果物として記録する。

**最終的な完了判定:** 段階別のmodule境界が成立し、共通ABIを満たすboard・native・試験用の追加platformが、一度保存した同じ`Output`を変更せず消費できること。`CompiledModule`を使ってplatformごとにCを再生成するだけの状態は、この完了条件に含めない。
