---
title: emela test の出力を，人が読む端末とエージェントで分ける
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [cmd.test.output, cmd.test.reporter (new), test.command]
requires: []
related: []
---

# emela test の出力を，人が読む端末とエージェントで分ける

## Summary

`emela test` の結果の表示を reporter として3つに分ける．端末で人が読むための `pretty`，今の行指向の形式を保つ `plain`，コーディングエージェントが読むための `agent`．`--reporter` で選べ，省略時は環境から決める．`pretty` の見た目は仕様で決めず，実装に任せる．

## Motivation

今の出力（`[cmd.test.output]`）は1つの形式しかなく，人にもエージェントにも合っていない．

```
running 3 tests
test adds ... ok
test adds_wrong ... FAILED
test names ... FAILED

failures:

---- adds_wrong ----
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

---- names ----
defect: assertion failed: "abc" == "abd"
  left: "abc"
 right: "abd"

failures:
    adds_wrong
    names

test result: FAILED. 1 passed; 2 failed; 0 filtered out
```

端末で人が読むとき:

- 色がない．`ok` と `FAILED` が同じ見た目で並び，失敗を目で探すことになる．
- `left` と `right` が長い文字列や大きな値のとき，どこが違うかが分からない．
- テストがファイルごとにまとまらず，名前の前置き（`Util.`）でしか見分けられない．
- 所要時間が出ないので，遅いテストに気づけない．

エージェントが読むとき:

- 通ったテスト1件につき1行が出る．テストが数百件あると，失敗1件を知るために数百行がコンテキストに入る．
- 失敗の名前が2回出る（詳細の見出しと，末尾の `failures:` の一覧）．
- 通ったテストが標準出力に書いたものも，そのまま流れてくる．

`emela test` を呼ぶのは人とエージェントの両方で，求めるものが逆向きである．人には情報を足し，エージェントには削る．1つの形式でこれを両立できない．

失敗の場所（パスと行と桁）が出ない問題は，どの reporter にも共通なので別の提案（`test-failure-location`）で扱う．

## Proposal

### reporter

`emela test` は結果を reporter のどれか1つで表示する．

| reporter | 読み手 | 仕様で決めること |
|---|---|---|
| `pretty` | 端末の人 | 色を使う条件と，失敗をすべて詳細つきで出すことだけ．見た目は実装に任せる |
| `plain` | ログ，CI，出力を比べるテスト | 今の形式．同じ入力から同じ出力 |
| `agent` | コーディングエージェント | 形式．失敗だけを出し，通ったテストの行もその標準出力も出さない |

選び方は，上から順に最初に当てはまるもの．

1. `--reporter <pretty|plain|agent>` が与えられれば，それ．
2. コーディングエージェントの中で動いていれば，`agent`．環境変数 `AI_AGENT` が空でなければ，エージェントの中とする．実装は，ほかにエージェントが設定する環境変数を見てもよい（MAY）．
3. 標準出力が端末なら，`pretty`．
4. それ以外は `plain`．

### `pretty` の例

見た目は仕様で決めない．次は一例である．

```
 src/main.emel
   ✓ adds                                     0ms
   × adds_wrong                               1ms
   × names                                    0ms

──── FAIL  adds_wrong ────
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

──── FAIL  names ────
defect: assertion failed: "abc" == "abd"
  - left:  "abc"
  + right: "abd"
               ^

 Tests  2 failed | 1 passed (3)
 Duration  12ms
```

- テストをファイルごとにまとめ，通ったものに `✓`（緑），失敗に `×`（赤）をつける．
- 比較の assert の両辺の表示が違うところを示す．
- 最後に件数と所要時間をまとめて出す．

### `agent` の形式

すべて通ったとき，まとめの1行だけ．テストが走ったことをエージェントが確かめられるよう，この1行は出す．

```
test result: ok. 3 passed; 0 failed; 0 filtered out
```

失敗があるとき．

```
FAIL adds_wrong
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

FAIL names
defect: assertion failed: "abc" == "abd"
  left: "abc"
 right: "abd"

test result: FAILED. 1 passed; 2 failed; 0 filtered out
```

- まとめの行は `plain` の最後の行と同じ形にする．今の形式を前提にした読み方（`test result:` の行を探すなど）がそのまま通る．
- 先頭の `running N tests` も，通ったテストの行も出さない．
- 失敗したテストが標準出力に書いたものは，その失敗の詳細の後に切り詰めずに出す．通ったテストが書いたものは出さない．行数で切ると肝心のところが消えることがあり，切り詰めの規則も仕様に要る．
- 色，記号，所要時間は出さない．
- 失敗の名前は1回だけ出す．
- `test-failure-location` が採択されれば，`FAIL <name>` の後に場所が続く（`FAIL adds_wrong src/main.emel:13:3`）．

## Specification changes

### `[cmd.test.reporter]` (new)

```md
## Reporters `[cmd.test.reporter]`

`emela test` displays its results with one reporter.

| Reporter | Output |
|---|---|
| `pretty` | Results for a person reading a terminal. Its layout is not specified |
| `plain` | The line format of `[cmd.test.output]`. The same input gives the same output |
| `agent` | Only the failures and one summary line |

- `--reporter <pretty|plain|agent>` selects the reporter. Another name is an error in the command-line arguments, reported in the same way as an unknown value of `--target`, and no test is run.
- When `--reporter` is omitted, the reporter is the first that applies of the following.
  1. `agent`, if the command runs inside a coding agent.
  2. `pretty`, if standard output is a terminal.
  3. `plain`.
- The command runs inside a coding agent when the environment variable `AI_AGENT` is set and not empty. An implementation MAY also treat other environment variables that coding agents set as meaning the same.
- Every reporter writes to standard output. The exit code (`[cmd.test.exit]`) does not depend on the reporter.
- Diagnostics found by checking are written to standard error as in `[diag]`, whatever the reporter.
```

### `[cmd.test.output]`

今の節を `plain` の定義にする．冒頭の文を次に置き換え，以降の内容はそのまま残す．

```md
The `plain` reporter (`[cmd.test.reporter]`) prints the results to standard output in the following form.
```

節の末尾に `pretty` と `agent` を足す．

````md
### pretty

- Every failed test is shown with the details of the failure (`[cmd.test.outcome]`).
- Color is used only when standard output is a terminal and the environment variable `NO_COLOR` is empty or unset.
- The layout, marks, colors, and whether the elapsed time is shown are not part of the specification, and an implementation MAY change them. An example:

```
 src/main.emel
   ✓ adds                                     0ms
   × adds_wrong                               1ms

──── FAIL  adds_wrong ────
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

 Tests  1 failed | 1 passed (2)
 Duration  12ms
```

### agent

```
FAIL adds_wrong
defect: assertion failed: add(1, 2) == 4
  left: 3
 right: 4

test result: FAILED. 1 passed; 1 failed; 0 filtered out
```

- Nothing is printed for a passed test. The header line `running N tests` is not printed.
- For each failure, in run order, the line `FAIL <name>` is printed, then the details of the failure (`[cmd.test.outcome]`), then what the test wrote to standard output. Failures are separated by a blank line.
- What a passed test writes to standard output is not printed. What a failed test writes is printed in full, without truncation.
- The last line is the summary line of `plain`, `test result: <ok|FAILED>. <number passed> passed; <number failed> failed; <number excluded by the filter> filtered out`. It is printed even when every test passed. When a failure was printed, a blank line precedes it.
- No color, mark or elapsed time is printed. The same input gives the same output.
````

### `[test.command]`

「Results go to standard output: ...」の項目を次に置き換える．

```md
- Results go to standard output, in the form of the selected reporter (`[cmd.test.reporter]`).
```

## Diagnostics

なし．`--reporter` に知らない名前を与えたときは，`--target` に知らない値を与えたときと同じくコマンドライン引数の誤りとし，診断コードは振らない．

## Compatibility

- 言語の意味は変わらない．
- 端末で `emela test` を実行したときの見た目が変わる．パイプやファイルに出したとき（CI，`emela test | tee`）は `plain` のままで，今と同じ形式．
- `AI_AGENT` が設定された環境やエージェントの中で実行すると `agent` になり，通ったテストの行と `running N tests` が出なくなる．まとめの行は `plain` と同じなので，`test result:` の行を読むやり方はそのまま通る．`--reporter plain` で今の形式に戻せる．

## Implementation

- `crates/emela-driver/src/testing/report.rs` に reporter を足す．今の関数群は `plain` になる．
- テストは1つずつ順に実行され，ドライバはプログラムの標準出力を1行ずつ読んで結果の行と見分けている（`crates/emela-driver/src/testing.rs` の `run_tests`）．次の結果の行が来るまでの行を実行中のテストに属するものとして持っておけば，`agent` はテストごとの標準出力を出し分けられる．ランタイムは変えなくてよい．標準エラーは今 inherit しているので，テストごとには分けられない．
- `pretty` の所要時間は，ドライバが結果の行の届いた時刻の差で測れる．ランタイムは変えなくてよい．
- 標準出力が端末かは `std::io::IsTerminal`．標準エラーの色の判定は `crates/emela-cli/src/main.rs` にある．
- `AI_AGENT` 以外に見る環境変数は，実装の中で一覧にする（`CLAUDECODE`，`GEMINI_CLI`，`CODEX_SANDBOX`，`CURSOR_AGENT` など）．

## Alternatives

### 形式は1つのまま，色だけ足す

`plain` に色を足すだけなら変更は小さい．ただ，エージェント向けに行を減らす問題は残る．通ったテストの行は，端末の人には進み具合として役に立つが，エージェントには要らない．同じ形式で両方は満たせない．

### エージェントの判定をせず，`--reporter agent` を明示させる

仕様が環境変数に依存しない．一方，エージェントはふつう `emela test` を素で呼ぶので，明示を求めると使われない．AGENTS.md などに `--reporter agent` を書いてもらう運用になる．

### 製品ごとの環境変数の一覧を仕様に書く

どの環境で `agent` になるかが仕様から読める．一方，エージェントの製品が増えるたびに仕様を変えることになる．仕様には製品に依らない `AI_AGENT` だけを書き，ほかは実装に任せる．

### `pretty` の見た目を仕様で決める

見た目を直すたびに proposal が要る．人が読む表示は改善を続けるもので，出力を比べる用途には `plain` と `agent` がある．

### 機械可読の形式（JSON）を足し，エージェントにはそれを読ませる

JSON は構造を持つが，同じ情報を表すのに文字数が多く，エージェントは結局それを文字として読む．エージェントに要るのは「どれが，なぜ失敗したか」で，`agent` の形式はそれを最短で書く．JSON はエディタ連携などの用途で，別の提案にする．

### 環境変数 `EMELA_TEST_REPORTER` で選ぶ

`--reporter` と両方あると優先順位の規則が増える．今はフラグだけにする．

## Prior art

- reporter を差し替えられるテストランナー（Vitest の `default`，`verbose`，`dot`，`minimal`，`json` など）．Vitest は reporter を指定しないとき，AI コーディングエージェントの中で動いていると判定すると `minimal`（別名 `agent`）を使う．`minimal` は失敗とその誤りだけを出し，通ったテストのコンソール出力とまとめを出さない．判定は `std-env` の環境変数の一覧による（`AI_AGENT`，`CLAUDECODE`，`GEMINI_CLI`，`CODEX_SANDBOX`，`CURSOR_AGENT` など）．
- `NO_COLOR` の慣習．
- 出力先が端末かどうかで形式を変える CLI（端末なら色と整形，パイプなら素の行）．

## Open questions


## Decision
