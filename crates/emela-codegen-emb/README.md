# emela-codegen-emb

EmelaのFlow IRから、組み込み向けのCコードを生成するコンパイラーです。Heap IRへの変換を通さず、固定容量のメモリと機能ごとの小さなランタイムを生成します。

[実装範囲と実装状況](docs/implementation.md)に、入力契約、各variantへの対応、メモリ方針と既存の検証をまとめています。

[段階別の構成](src/compiler/README.md)と[生成物契約](docs/artifact-contract.md)に、純粋なlower・plan・emitと、同じOutputをnative／board／追加platformへ接続する境界を説明しています。

## 主な入口

- `compile_flow(flow, options)`：検証済みのFlow段階を入力とする主入口です。入力を検証し、Cソースとヘッダーを`Output`として返します。
- `compile(core, options)`／`flow_from_core(core)`：通常のCore IRを単相化してFlowへ変換する便宜入口です。成長する再帰型の入力は診断し、Flowを直接渡す経路へ分けています。
- `derived_binding(core, operation, ty, name)`：Flow生成側が、具体化された型の有限な構造比較・表示を登録するための関数を作ります。`DerivedOperation::Eq`または`Show`を指定し、返された関数をFlowモジュールへ追加して呼び出します。

`Options`でString容量、固定heap、fiberやtaskのpool容量などを指定します。必要なランタイムとボード接続用のCインターフェースは生成コードに含まれます。

[生成Cとビルドの契約](docs/artifact-contract.md)は`emela_artifact.json`で生成ファイルの役割と形式・ABIの版を定義します。BSPはこのmanifestからCを選び、Emela sourceやIRを使わずにビルドします。ABI 2の共通出力はplatformごとに書き換えず、`platform::prepare_connection`と`platform::execution`で接続します。同梱MCUへの保存済みartifact接続は未対応です。

## 実装とテストの自動検査

IRの定義・担当する実装入口・関連テストを対応付け、欠落や参照切れを検査します。生成ツールは[`tools/inventory`](tools/inventory/Cargo.toml)の独立したRust workspaceで動き、コンパイラー本体がビルドできない状態でもsourceを監査できます。通常の`cargo test`にも整合性の検査があります。[コンパイラーの構成](src/compiler/README.md)で説明する機能別の分割に合わせた仕組みです。

| 生成される一覧 | 確認する内容 |
| --- | --- |
| [Flow IR定義](docs/generated/flow-ir-inventory.md) | Rust構文から抽出した型・variant・field・宣言hash。新しい定義や形の変更を検出します。 |
| [実装・テストの対応表](docs/generated/flow-backend-inventory.md) | 定義IDごとの専用入口・処理段階・関連テスト。担当の欠落・重複・定義外ID・存在しない関数やテストを拒否します。 |
| [構文上の実装候補](docs/generated/flow-implementation-inventory.md) | 指定した生成・変換・拒否処理のAST分岐と呼び出し。wildcardを未列挙variantの根拠には数えません。 |
| [実行経路との比較](docs/generated/flow-coverage.md) | sync・suspend・入力拒否の必須経路に明示的な根拠があるか。経路の欠落や監査対象enumの登録漏れを拒否します。 |

担当は[`owners.json`](inventory/owners.json)と[`containers.json`](inventory/containers.json)、テストは[`tests.json`](inventory/tests.json)、処理経路は[`flow-implementation-routes.json`](tools/flow-implementation-routes.json)で登録します。variantを追加するときは、担当入口と関連テストも登録してください。テストはtargetと完全な名前で識別し、全件ignoreの関連付けは拒否します。`--check-built`ではコンパイルされたテストバイナリーへの登録とignore状態も照合し、`cfg`等により実際には登録されないテストを検出します。

リポジトリrootから生成します。MarkdownとJSONは同じ抽出結果から作り、生成物を手では編集しません。

次のコマンドは8ファイルをまとめて更新します。上流にvariantが追加され、独立contract・担当入口・関連テストが追従していなくても、欠落をMarkdownとJSONに記録してから終了code 1を返します。`flow-backend-inventory`には担当入口の欠落件数、`flow-coverage`には経路ごとの`missing_evidence`と上流IRへの未追従を表示します。欠落は構文・登録情報の不足を示すもので、実装の意味論を自動判定した結果ではありません。

```sh
bash crates/emela-codegen-emb/tools/inventory.sh all
```

別のcheckoutを読み取る場合は`--root <repository>`を指定できます。`--out-dir`で出力先を分ければ、そのcheckoutを変更せずに監査できます。

```sh
bash crates/emela-codegen-emb/tools/inventory.sh all --root /path/to/emela --out-dir /tmp/emela-inventory
```

個別に生成する場合は次のコマンドを使います。

```sh
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh
bash crates/emela-codegen-emb/tools/backend-inventory.sh
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh
```

開発時の検査コマンドは次のとおりです。`--check`は生成物を更新せず、現在のsourceとの差分と監査の欠落を検出します。生成物を更新した後も、欠落が残る間は`all --check`が失敗します。sourceに対する監査はコンパイラー本体をビルドせずに実行でき、`--check-built`だけは実際のテストバイナリーのビルドを必要とします。専用のCIステップは設けず、必要に応じて手元で実行します。

```sh
bash crates/emela-codegen-emb/tools/inventory.sh all --check
bash crates/emela-codegen-emb/tools/flow-ir-inventory.sh --check
bash crates/emela-codegen-emb/tools/backend-inventory.sh --check
bash crates/emela-codegen-emb/tools/flow-implementation-inventory.sh --check --require-evidence
python3 crates/emela-codegen-emb/tools/test-inventory.py --check-built
cargo test --manifest-path crates/emela-codegen-emb/tools/inventory/Cargo.toml --locked --offline
cargo test -p emela-codegen-emb --locked
```

検査器自体にも、variantの追加、実装入口の削除・使い回し、テストの削除・改名・ignore、生成物の陳腐化、wildcardによる根拠の不足を検出する回帰テストがあります。AST抽出はmacro展開や型解決、実装の意味論やテスト内容の十分さを証明するものではありません。検査の成功と、関連テストの実行・コードのreviewは別に確認します。生成された記録のreview状態は自動更新しません。

## このcrateの範囲

コンパイラー本体、生成ランタイム、検証用のテストとexampleに加え、`Std.Embedded` の共通effectと共有handlerをC ABIへ接続するbridgeを提供します。[組み込みインターフェース](docs/embedded-interfaces.md)と[標準周辺機器bridge](docs/embedded-peripherals.md)に利用方法と所有権・容量の契約をまとめています。[ボードpackageと確認手順](docs/boards.md)では、STM32F4・ESP32-C3のBSP、Rust exampleからのstandalone build/flash、Renode/QEMUによるFlash起動の確認を提供します。

[CLIでの開発手順](docs/cli-embedded.md)では、projectの作成、`c-embedded` target・board選択、TOML設定、buildと入力・成果物のhash照合を伴うflashを説明します。CLIとdriverがこのcrateのAPIと同梱BSPを利用します。外部toolchainの導入と対象BSPが提供しないdriverの追加は利用側で行います。
