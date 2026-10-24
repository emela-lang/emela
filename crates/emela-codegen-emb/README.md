# emela-codegen-emb

EmelaのFlow IRから、組み込み向けのCコードを生成するコンパイラーです。Heap IRへの変換を通さず、固定容量のメモリと機能ごとの小さなランタイムを生成します。

[実装範囲と実装状況](docs/implementation.md)に、入力契約、各variantへの対応、メモリ方針と既存の検証をまとめています。

## 主な入口

- `compile_flow(flow, options)`：検証済みのFlow段階を入力とする主入口です。入力を検証し、Cソースとヘッダーを`Output`として返します。
- `compile(core, options)`／`flow_from_core(core)`：通常のCore IRを単相化してFlowへ変換する便宜入口です。成長する再帰型の入力は診断し、Flowを直接渡す経路へ分けています。
- `derived_binding(core, operation, ty, name)`：Flow生成側が、具体化された型の有限な構造比較・表示を登録するための関数を作ります。`DerivedOperation::Eq`または`Show`を指定し、返された関数をFlowモジュールへ追加して呼び出します。

`Options`でString容量、固定heap、fiberやtaskのpool容量などを指定します。必要なランタイムとボード接続用のCインターフェースは生成コードに含まれます。

## このcrateの範囲

コンパイラー本体、生成ランタイム、検証用のテストとexampleに加え、`Std.Embedded` の共通effectと共有handlerをC ABIへ接続するbridgeを提供します。[組み込みインターフェース](docs/embedded-interfaces.md)と[標準周辺機器bridge](docs/embedded-peripherals.md)に利用方法と所有権・容量の契約をまとめています。CLIへの組み込み、ボード定義・BSP、クロスコンパイラー、書き込みとエミュレーターの操作は、後続のツールチェーン統合で扱います。
