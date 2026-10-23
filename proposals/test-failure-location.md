---
title: テストの失敗に，失敗した場所を出す
authors: [Alliana]
status: draft
pr: 413
issue:
decided:
touches: [cmd.test.output, cmd.test.outcome, test.assert]
requires: []
related: []
---

# テストの失敗に，失敗した場所を出す

## Summary

assert が失敗してできた defect に，その assert 式のソース上の場所（パスと行と桁）を持たせる．`emela test` は失敗の詳細にその場所を出す．

## Motivation

今の失敗の詳細（`[cmd.test.outcome]`）は，defect のメッセージだけを出す．

```
---- adds_wrong ----
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4
```

どのファイルの何行目の assert が失敗したかが出ない．

- 人は，式の字面（`add(1, 2) == 4`）でソースを検索して場所を探す．同じ字面の assert が複数あると，どれかが分からない．
- エージェントは，失敗を直す前にまずソースを検索する．その分のやりとりとコンテキストを使う．
- 診断（`[diag]`）は場所をコードフレームで示すのに，テストの失敗は示さない．

assert の場所はコンパイル時に決まっている．assert のメッセージ（`assertion failed: <source text>`）もコンパイル時に組み立てている（`[test.assert]`）．場所を足すのに実行時の追跡は要らない．

## Proposal

- assert が失敗してできた defect は，その assert 式の場所を持つ．
- `emela test` は，場所を持つ失敗の詳細に場所を出す．`plain` では `---- <name> ----` の次の行に ` --> <path>:<line>:<column>` を出す．

```
---- adds_wrong ----
 --> src/main.emel:13:3
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4
```

- パスはプロジェクトのディレクトリからの相対パス（単一ファイルモードでは，ファイルのあるディレクトリから）．依存 Pome の中の assert は，出力先（`emela_pomes/...`）のパスではなく，その Pome のソースのパスで出す．行と桁は1から数える．桁の数え方は診断と同じにする．
- `emela run` で main まで届いた defect には場所を出さない．`[error.defect]` の `defect: <message>` の1行は変わらない．
- assert 以外の defect（`panic`，ゼロ除算，外部関数の例外など），error，interrupted は場所を持たない．`panic` と `todo` の場所は別の提案で扱う．

`test-reporters` が採択されれば，`agent` は `FAIL <name> <path>:<line>:<column>`，`pretty` は場所を含むコードフレームを出す．

## Specification changes

### `[test.assert]`

次を足す．

```md
- A defect from a failed assert has the location of the assert expression: the path of the file, the line and the column. The location is shown by the test runner (`[cmd.test.outcome]`). It is not part of the defect message.
```

### `[cmd.test.outcome]`

次を足す．

```md
- A failure has a location when it is a defect from a failed assert (`[test.assert]`). Other failures have no location.
- A location is shown as `<path>:<line>:<column>`. The path is relative to the project directory, and in single-file mode to the directory that contains the file. For an assert in a dependency Pome, the path is the path of its source file, not of the build output.
- The location is shown only by the test runner. A defect that reaches `main` is displayed as in `[error.defect]`, without a location. Lines and columns are counted from 1, as in diagnostics (`[diag]`).
```

### `[cmd.test.output]`

失敗の詳細の項目を次に置き換え，例も合わせて直す．

```md
- If there is at least one failure, then after a blank line and `failures:`, the command prints, for each failed test, a blank line, the line `---- <name> ----`, the line ` --> <location>` if the failure has a location, and the details of the failure ("Kinds of results" below). Then it prints a blank line, `failures:`, and the names of the failed tests, one per line, each preceded by four spaces.
```

## Diagnostics

なし．

## Compatibility

- 言語の意味は変わらない．defect のメッセージも変わらない（場所はメッセージの外に持つ）．
- `emela test` の `plain` の出力に，失敗ごとに1行増える．出力を比べているテスト（`crates/emela-driver/tests/snapshots/test_runner__*.snap`）は更新が要る．

## Implementation

- assert の defect は `crates/emela-lir/src/from_core.rs` で `Term::Defect(message)` に下ろしている．ここに場所を足す（`Term::Defect` に場所の定数を持たせる）．`crates/emela-core/src/mono.rs` の側も同じ．
- JS，C，WASM の各ランタイムの defect に場所を持たせ，テストランナーの結果の行の JSON（`Event::Result`）に `location` を足す．
- 場所はコンパイル時の定数なので，実行時の追跡は要らない．

## Alternatives

### 場所を defect のメッセージに含める

`assertion failed: add(1, 2) == 4 at src/main.emel:13:3` のようにする．ランタイムを変えずに済むが，`emela run` で main まで届いた defect の表示や `Fiber.attempt` で受け取るメッセージも変わる．reporter が場所を別に扱えない（`pretty` のコードフレームに使えない）．

### `panic` と `todo` も含める

`panic` は関数なので，呼び出した場所を知るには呼び出し側の場所を暗黙の引数として渡す仕組みが要り，言語の変更になる．assert は式の形をコンパイラが見ている（`[test.assert]`）ので，この仕組みなしにできる．`panic` と `todo` は別の提案に分ける．

## Prior art

- テストの失敗に，失敗した assert のソースの位置とコードフレームを出すテストランナー（Vitest，`cargo test` の `panicked at src/lib.rs:10:5`）．
- 呼び出し側の場所を暗黙に受け取る関数（Rust の `#[track_caller]`）．`panic` に場所を持たせるときに要る仕組み．

## Open questions

## Decision
