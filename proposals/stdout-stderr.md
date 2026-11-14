---
title: Stdout and Stderr in Std.Process, the output streams of the process
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [api.std.process (new), api.std.console, api.std.platform, lib.layers, cmd.build, target.c.run, target.wasm.external, open.list]
requires: [std-process]
related: []
---

# Stdout and Stderr in Std.Process, the output streams of the process

## Summary

`Std.Process` に，プロセスの出力ストリームを表す 2 つの effect `Stdout` と `Stderr` を足す．どちらも操作は改行なしで書く `print` と，改行を付けて書く `print_line` の 2 つで，失敗せず，Immediate．system handler は `SystemStdout` と `SystemStderr` で，`Platform` に入る．テスト用の `TestStdout` と `TestStderr` は，書かれた文字列をそれぞれ `State[StdoutText]` と `State[StderrText]` に足していく．`Std.Console` は変えない．`Console` は人に向けた行単位の出力，`Stdout` と `Stderr` はプロセスのストリームとして使い分ける．

```emel
import Std.Platform.{Platform}
import Std.Process.{Stdout}

fn main() {
  with Platform {
    (use Stdout).print("Hello, ")
    (use Stdout).print_line("alice!")
  }
}
```

## Motivation

- 改行なしで書く手段がない（`[open.list]` #45）．プロンプト（`name? ` を出して同じ行で入力を待つ）や，1 行に書き足していく出力が書けない．`Std.Process` に `Stdin` が入る（`std-process`）と，プロンプトはすぐに要る．
- CLI ツールは標準出力をデータの出口として使う．パイプやファイルに流れる出力を，書いたバイト列のまま（行に区切らず）出したい．
- 標準出力と標準エラーを別々に要求し，別々に差し替えたい．
  - 結果を標準出力に，進み具合や警告を標準エラーに書くツールでは，関数の型から，どちらに書くかが読めるとよい．
  - いまの `TestConsole` は `print_error_line` を捨てる．標準エラーに書いた内容をテストで確かめるには，`Console` の handler を自分で書くしかない．
- `Console` はこのまま残す．広く使われていて，「人に読ませる 1 行を出す」という意味で分かりやすい．

いまの仕様で回避する方法: `@external` で `process.stdout.write` を呼ぶ handler を自分で書く．`[external]` を許したプロジェクトでしか使えず，ターゲットごとに書き分ける必要がある（`[ext.decl]`）．

## Proposal

### Syntax

変更なし．

### Meaning

`Std.Process` に次を足す（handler の本体は省いた宣言の一覧）．

```
pub effect Stdout {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub effect Stderr {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub handler SystemStdout: Stdout
pub handler SystemStderr: Stderr

pub type StdoutText(text: String)
pub type StderrText(text: String)

pub handler TestStdout: Stdout # State[StdoutText]
pub handler TestStderr: Stderr # State[StderrText]
```

- `print(text)` は `text` をそのまま書く．`print_line(text)` は `text` と改行（`\n`）を書く．`text` の中の改行は変換しない．
- 操作は失敗せず，suspend しない（Immediate）．
- 同じストリームに書いたものは，書いた順に届く．`Stdout` と `Console.print_line` を混ぜても，`Stderr` と `Console.print_error_line` や `dbg` を混ぜても，順序は保たれる（下の「Console との関係」）．
- `Stderr`，`Console.print_error_line`，`dbg` が標準エラーに書く前に，それまで標準出力に書いたものはホストに渡される．`2>&1` で同じ出口に向けても，書いた順に並ぶ．いまの C の `print_error_line` はすでに `fflush(stdout)` してから書いているので，いまの挙動を仕様に書くだけになる．JS は fd に同期で書く経路で，自前のバッファを持たないので，何もしなくても満たされる．
- `Stdin`（`std-process`）が入力を待つ前に，それまで `Stdout` に書いたものはホストに渡される．`print("name? ")` の直後に `read_line` を呼ぶと，プロンプトが見えてから入力を待つ．
- プログラムが終わるとき（`[cmd.build]` の終了コード表のどの終わり方でも），それまでに書いたものはすべて書き切られる．いまの `Console` と同じ保証．
- 読み手が閉じた標準出力に書くと，プログラムは終わる．閉じた標準エラーへの書き込みは捨てて続ける（下の「閉じたパイプ」）．

#### Console との関係

`Console` は `Stdout` に依存させない．`SystemConsole` が `Stdout` と `Stderr` を要求する handler になると，`with SystemConsole { ... }` だけで書いたコードが通らなくなり，stdout を持たない host で `Console` が使えなくなるため．2 つは別の約束をする．

| | `Console` | `Stdout`，`Stderr` |
|---|---|---|
| 意味 | 人に向けた 1 行 | プロセスの出力ストリーム |
| 単位 | 行（必ず改行が付く） | 文字列（`print` は改行なし） |
| 使える host | どの host でも．stdout がない host では host のログ（JS の `console.log`，`console.error`） | stdout と stderr を持つ host だけ |
| 使う所 | ログ，診断，HTTP サービスの出力 | CLI ツールのデータ出力，プロンプト |

- 同じ出口に 2 つの effect で書けるので，混ぜたときの順序を保証する．`SystemConsole` と `SystemStdout`（`SystemStderr`）は，同じストリームの同じバッファを通して書く．
  - JS: stdout を持つ host では，`SystemConsole`，`SystemStdout`，`SystemStderr`，`dbg`，runtime が書く `error:`，`defect:`，`interrupted` の行を，すべて fd に同期で書く 1 本の経路（下の `writeAll`）に通す．`console.log` と同期の書き込みを混ぜると，読み手が遅いとき Node で順序が崩れる（下の試作）ので，混ぜない．`@external` でユーザーが `console.log` を直接呼んだ出力との順序は保証しない．
  - C: どちらも `stdout`，`stderr` の `FILE *` に書くので保たれる．
  - WASM: runtime が持つ 1 本のストリームに書く（`[target.wasm.output]`）ので保たれる．
- `Console` は残すので，`Stdout` を使わないプログラムには何も変わらない．

#### stdout を持たない host

- JS ターゲットが前提にする host は Node と Bun だけで，どちらも `process.stdout` を持つ（`[target]`）．ブラウザはターゲットではなく，platform handler の振る舞いは決めない．Cloudflare Workers のような host 固有のものは，ターゲット固有の Pome に置く（`[ext]`）．
- そのため，仕様の上では「stdout がない host」は JS ターゲットの外にあり，`SystemStdout` と `SystemStderr` もそこでの振る舞いを決めない（unspecified）．`Console` と違って，そこで host のログへ流すことはしない．`print` は行に区切れないので，ログに流すと出力の形が変わる．
- コンパイル時に弾けるのはターゲット単位まで（E0412，`[lib.layers]`）．同じ JS ターゲットの中で Node と Workers を区別する手段はないので，「到達したらコンパイルエラー」にはできない．実行時に defect にする案も，host を見分ける規則を仕様に持ち込むことになるので採らない．
- `main.mjs` の default export を HTTP サービスとして別の host に載せる使い方では，`Console` を使えばどこでも動く．`Stdout` は CLI として動かすときのもの．

#### テスト

`TestStdout` と `TestStderr` は，書かれた文字列を状態の `text` の後ろに足していく．`print_line` は改行も足す．2 つが同じ `State[String]` を取り合わないように，型 `StdoutText` と `StderrText` で包む（`[effect.state]`）．

```emel
import Main.{greet}
import Std.Process.{StderrText, StdoutText, TestStderr, TestStdout}
import Std.State.{State}

@test
fn greets() {
  ((_, err), out) = State.run(StdoutText(text: ""), || {
    State.run(StderrText(text: ""), || with TestStdout, TestStderr { greet("alice") })
  })
  assert out.text == "Hello, alice!\n"
  assert err.text == "greeted alice\n"
}
```

- 状態を行のリストではなく文字列にしたのは，`print` があるため．`print("a")` と `print_line("b")` は 1 行 `ab` になる．行のリストにすると，最後の行が閉じているかを別に持つ必要がある．
- `TestConsole` は変えない．

#### 閉じたパイプ

`tool | head -1` では，`head` が 1 行読んで終わると，パイプの読み口が閉じる．そのあと `tool` が書くと，書き込み先がない．

- 黙って捨てて続ける案は採らない．出力し続けるプログラム（無限に行を作るものなど）が，出力の行き先がないまま止まらなくなる．
- 中断（`[fiber.interrupt]`）を送るだけでは足りない．中断は次の suspension point で効くが，`Stdout` は Immediate なので，書くだけのループは suspension point に来ない．
- そこで，書き込みそのものを止める点にする．
  1. 標準出力への書き込みが，読み手が閉じたために失敗したら，書いた fiber はその操作から戻らず，その時点から中断として巻き戻る（`[dyn.suspend]` の「その場で中断として巻き戻る」と同じ形）．release は内側から走る．
  2. プログラムの他の fiber もすべて中断される（次の suspension point で効く）．
  3. 以後，標準出力への書き込みは捨てる（release の中の書き込みなど）．
  4. main の巻き戻しが終わったら，標準エラーには何も書かずに，終了コード 141 で終わる．`interrupted` の 1 行は出さない．`| head` で切るのはふつうの使い方で，毎回 1 行出るのはうるさい．141 は，いまの C ターゲットが SIGPIPE で終わるときの値（128 + 13）と同じで，シェルが「パイプを閉じられて終わった」と見分けられる．
  5. `Console` と `dbg` の書き込みも同じ扱いにする．同じストリームに書くので，片方だけ続けても意味がない．
  6. 閉じたのが標準エラーだけのときは，プログラムを終わらせない．以後の標準エラーへの書き込み（`Stderr`，`Console.print_error_line`，`dbg`，runtime の `error:` などの行）は捨てる．ログの出口が閉じただけで本体を止めない．
- 操作は失敗しない（`fails` を持たない）ので，fail にはしない．`Stdout` を使う関数すべてに error を足すことになる．
- `emela test` の中では，`TestStdout` などはパイプに書かないので起きない．`SystemStdout` で書いたときは，テストランナーの出力の扱い（`[cmd.test.output]`）による．
- 試作（Node 22.23，Bun 1.4.4，macOS，2026-11-14）: `fs.writeSync(1, ...)` は閉じたパイプで同期に `EPIPE` を投げた（Node は 377 行目，Bun は 2 行目）．`process.stdout.write` はエラーをイベントで後から知らせるので，書くだけのループの中では気づけない．JS の `SystemStdout` は fd に同期で書く経路を使う．

#### ターゲットごとの実装

| | `SystemStdout` | `SystemStderr` |
|---|---|---|
| JS | fd 1 に同期で書く（`fs.writeSync`） | fd 2 に同期で書く |
| C | `fputs` を `stdout` に | `stdout` を `fflush` してから `fputs` を `stderr` に（いまの `print_error_line` と同じ） |
| WASM | `wasi:cli/stdout@0.3.0` の `write-via-stream` | `wasi:cli/stderr@0.3.0` の `write-via-stream` |

#### 試作（Node 22.23，Bun 1.4.4，macOS，2026-11-14）

- `console.log` と `process.stdout.write` を 1 行ずつ交互に 100,000 行書き，1,000 行ごとに `console.error` と `process.stderr.write` を交互に書いた．
  - 標準出力をパイプに向けたとき，ファイルに向けたとき，`2>&1` で標準エラーと同じパイプに向けたとき，どれも両方の host で順序の入れ替わりは 0．標準エラーの行も，直前に書いた標準出力の行のすぐ後に並んだ．
- 50,000 行をパイプに書いたあと，`process.exit` を標準エラーへの書き込みのコールバックで呼ぶ（launcher が defect で終わるときの形）と，両方の host で 50,000 行すべてが届いた．書き切りの保証は今の launcher のまま守れる．
- 読み手が先に閉じたパイプ（`| head -1`）に書くと，`process.stdout.write` は `EPIPE` の例外で落ちる．`console.log` は黙って捨てる．`process.stdout.on("error", ...)` を付けると黙って捨てる形に揃う．C はいまも SIGPIPE で終わる（終了コード 141）．
- `process.stdout` を一度でも初期化すると（`console.log` がそうする），fd 1 はノンブロッキングになる．このとき読み手が遅い（`| (sleep 1; wc -c)`）と，`fs.writeSync(1, ...)` は `EAGAIN` を投げる．13 MB 書いて Node で約 15 万回，Bun で約 76 万回．`process.stdout` を初期化しなければ 0 回．
- `EAGAIN` のとき `Atomics.wait` で 1 ms 待ってから残りを書き直す `writeAll` にすると，両方の host で全量が届いた．CPU 時間は約 35 ms で，空回りしない．`| head -1` では `EPIPE` が同期に投げられ，Node は 83 行，Bun は 231 行書いたところで止まった．
- 読み手が遅いときに `console.log` と `writeAll` を 1 行ずつ交互に書くと，Node では 20,000 行のうち 19,933 行の順序が崩れた（Bun は 0）．読み手が速いときは両方とも 0．`console.log` は `EAGAIN` を受けると libuv のキューに積んで後から書くので，同期の書き込みに追い越される．そのため，stdout を持つ host では runtime の書き込みをすべて `writeAll` に通す．
- `Atomics.wait` は待つ間イベントループを止めるが，`Stdout` は Immediate で書き込みは同期なので，設計と矛盾しない．読み手が遅いと書き手が待つので，バッファも伸びない．ブラウザのメインスレッドでは `Atomics.wait` が使えないが，そこは stdout を持たない host で，振る舞いを決めない範囲．
- `Stdout`，`Stderr`，`TestStdout`，`TestStderr` をユーザーのモジュールとして書き（JS は `process.stdout.write` を呼ぶ `@external`），`emela check`，`emela test`，`emela run` が通ることを確かめた（emela 0.27.0）．

### Typing rules

新しい規則はない．`Stdout` と `Stderr` は，ふつうのユーザー定義 effect と同じ規則で型付けされる．

## Specification changes

### `[api.std.process]`

Add to the table of `Std.Process` (`std/process.emel`, `std/process.mjs`, `std/process.c`):

| Declaration | Contents | Description |
|---|---|---|
| `Stdout` | `effect Stdout { fn print(text: String) -> (); fn print_line(text: String) -> () }` | Writes to the standard output of the process. `print` writes `text` as is, and `print_line` writes `text` followed by `\n`. Does not fail |
| `Stderr` | `effect Stderr { fn print(text: String) -> (); fn print_line(text: String) -> () }` | The same for the standard error of the process |
| `SystemStdout` | `handler SystemStdout: Stdout` | File descriptor 1, written synchronously, on JS, `stdout` on C, and `write-via-stream` of `wasi:cli/stdout@0.3.0` on WASM. Has no state |
| `SystemStderr` | `handler SystemStderr: Stderr` | File descriptor 2, written synchronously, on JS, `stderr` on C, and `write-via-stream` of `wasi:cli/stderr@0.3.0` on WASM. Has no state |
| `StdoutText` | `type StdoutText(text: String)` | The state of TestStdout: everything written so far |
| `StderrText` | `type StderrText(text: String)` | The state of TestStderr: everything written so far |
| `TestStdout` | `handler TestStdout: Stdout` | Appends what is written to `text` of `State[StdoutText]`, with `\n` after the text of `print_line` |
| `TestStderr` | `handler TestStderr: Stderr` | Appends what is written to `text` of `State[StderrText]`, with `\n` after the text of `print_line` |

- Stdout and Stderr are the output streams of the process; Console (`[api.std.console]`) writes lines meant for people. Stdout and Stderr write text without adding a line ending (`print`), and are meant for data that goes to a pipe or a file and for prompts.
- Stdout and Stderr are Immediate (9.5): their operations do not suspend.
- What is written to one stream arrives in the order written. SystemStdout and `print_line` of SystemConsole write to standard output through the same path, and SystemStderr, `print_error_line` of SystemConsole and `dbg` write to standard error through the same path, so writes mixed between them keep their order. The order relative to output that an external function writes to the host directly is not specified.
- Before SystemStderr, `print_error_line` of SystemConsole or `dbg` writes to standard error, what has been written to standard output is handed to the host, so that the two streams directed to the same destination keep the order written. Before an operation of SystemStdin waits for input, what has been written to standard output is handed to the host.
- When the program ends, in any of the ways of `[cmd.build]`, everything written so far is written out before the exit code is set.
- `\n` inside `text` is written as is; it is not converted to another line ending.
- SystemStdout and SystemStderr need a host that gives the process a standard output and a standard error. The hosts of the JS target (`[target]`) do. Their behavior on other hosts is not specified.
- When a write to standard output fails because the reader has closed it (a pipe whose reading end is closed), the operation does not return. The fiber that wrote unwinds as interrupted from that point, running the releases of its handlers from the inside out, and every other fiber of the program is interrupted (`[fiber.interrupt]`). After that, writes to standard output are discarded. When the unwinding of main finishes, the program exits with exit code 141 and writes nothing to standard error (no `interrupted` line). The same holds for writes of `print_line` of SystemConsole to standard output.
- When a write to standard error fails because the reader has closed it, the program continues. That write and every later write to standard error (SystemStderr, `print_error_line` of SystemConsole, `dbg`, and the lines that the runtime writes when execution stops) are discarded.

```emel
import Std.Platform.{Platform}
import Std.Process.{Stdout}

fn main() {
  with Platform {
    (use Stdout).print("Hello, ")
    (use Stdout).print_line("alice!")
  }
}
```

### `[api.std.console]`

Add after the table:

- Console writes one line meant for people at a time. To write to the output streams of the process without a line ending, use Stdout and Stderr of `Std.Process` (`[api.std.process]`). Writes mixed between SystemConsole and SystemStdout or SystemStderr keep their order.

### `[api.std.platform]`

Replace the row of `Platform`. The list has the handlers of `std-process` as well:

| Declaration | Contents | Description |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | A bundle of the System handlers. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

### `[lib.layers]`

In the table of layers, the platform row:

| platform | Effect declarations and the host handlers | Console, Process (Args, Stdin, Stdout, Stderr), Clock, Random, Env, FileSystem, Http |

Replace the bullet on names:

- A system handler that uses the host has a name prefixed with `System` (`SystemConsole`, `SystemStdout`, `SystemStderr`, `SystemClock`, `SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`). A test handler is prefixed with `Test` (`TestClock`, `TestConsole`, `TestStdout`, `TestStderr`). Both are placed in the same module as their effect (`import Std.Clock.{Clock, SystemClock, TestClock}`).

After the bullet on Console, add:

- Stdout and Stderr (`Std.Process`) each have two operations, `print(text: String) -> ()` and `print_line(text: String) -> ()`. `print_line` writes a trailing newline and `print` does not. They do not fail. The system handlers are `SystemStdout` and `SystemStderr`.

### `[cmd.build]`

Add a row to the table of how execution stops, after the row of `interrupted`:

| How it stops | Standard error | Exit code |
|---|---|---|
| A write to a closed standard output (`[api.std.process]`) | nothing | 141 |

### `[target.c.run]`

Add:

- The executable ignores SIGPIPE. A write to a closed standard output or standard error is detected from the result of the write and handled as in `[api.std.process]`: for standard output, the program unwinds and exits with exit code 141, writing nothing to standard error; for standard error, the write is discarded and the program continues.

### `[target.wasm.external]`

Add a row to the table:

| `SystemStdout` (`print`, `print_line`), `SystemStderr` (`print`, `print_line`) | `write-via-stream` of `wasi:cli/stdout@0.3.0` and `wasi:cli/stderr@0.3.0`, on the same streams as `SystemConsole`. The text, then a newline for `print_line` |

### `[open.list]`

Mark #45 as decided:

| 45 | ~~Console has two operations, `print_line` and `print_error_line`. Whether to add output without a newline, and reading input~~ Decided (<date>): Console stays as it is. Output without a newline is `print` of Stdout and Stderr, and reading input is Stdin, both in Std.Process | `[api.std.process]` |

## Diagnostics

- [ ] 新しい診断はない．

## Compatibility

- 既存のコードはそのまま動く．`Console`，`SystemConsole`，`TestConsole` は変えない．
- `Platform` に `SystemStdout` と `SystemStderr` が入る．ユーザーが `Stdout` や `Stderr` という名前の effect を自分で宣言していても，`Std.Process` から import しなければぶつからない（`[module.resolution]`）．`with Platform, MyStdout` のように同じ effect の handler を後に並べると，後の方が使われる（8.3）．
- `Std.Process` は `std-process` が作るモジュールなので，新しい予約名は増えない（`[lib.layers]`）．

## Implementation

- `crates/emela-core/lib/std/process.emel` に宣言を足す（`std-process` が作るファイル）．
- JS: `std/process.mjs` に fd 1 と fd 2 へ同期で書く関数（`fs.writeSync`）を置く．`EPIPE` を受けたら runtime に知らせ，書いた fiber をその場で中断として巻き戻す．fd がノンブロッキングのとき（`process.stdout` を初期化したあとや，親プロセスがそうしたとき）の `EAGAIN` は，`Atomics.wait` で 1 ms 待って残りを書き直す（`writeAll`，試作で確かめた）．`SystemConsole`，`dbg`，runtime が書く `error:`，`defect:`，`interrupted` の行も，閉じたパイプの規則と順序の保証のため，stdout を持つ host では同じ経路に移す．stdout を持たない host では今のまま `console.log`，`console.error`．
- C: `std/process.c` に置く．`SystemStderr` は `fflush(stdout)` してから書く．`SystemStdin` が待つ前にも `fflush(stdout)` する．runtime は起動時に SIGPIPE を無視し，`fputs` や `fflush` の `EPIPE` で閉じたパイプを知る．
- WASM: runtime がすでに持つ stdout と stderr のストリームを使う．`print` は改行を付けない分岐を足すだけ．閉じたストリームを `write-via-stream` でどう知るかは未確認．WASM の stream の扱いは `std-process` でも範囲外にしているので，閉じたパイプの規則は WASM では実装で確かめてから合わせる．
- `Std.Platform` の `Platform` に 2 つの handler を足す．

## Alternatives

### `Stdout` を足さず，`Console` に `print` と `print_error` を足す

- 出口が 1 つの effect にまとまり，2 つの effect で同じ出口に書く状態が生まれない．
- `Console` を実装したユーザー定義の handler がすべて壊れる（E0404）．
- 標準出力と標準エラーを別々に要求したり差し替えたりできない問題は残る．
- `Console` が「どの host でも動く行の出力」でなくなる．`print` は stdout のない host で host のログに流せない．

### `Console` を `Stdout` と `Stderr` で置き換える

- 出口ごとに effect が 1 つになり，2 つの effect で同じ出口に書く状態が生まれない．
- `Console` はほぼすべての `main` で使われているので，すべてのプログラムを書き換えることになる．
- `Console` の「人に向けた 1 行」という意味と，stdout を持たない host でも使えることが失われる．作者はこの案を採らず，`Console` を残すと決めた．

### 1 つの effect `Output` と，その label `Stdout`，`Stderr`

```emel
pub effect Output {
  fn print(text: String) -> ()
  fn print_line(text: String) -> ()
}

pub effect Stdout of Output

pub effect Stderr of Output
```

- 出力先を受け取る関数を 1 つ書けば，標準出力にも標準エラーにも，ユーザーの書いたファイル出力にも向けられる（`Stdout by FileOutput`）．
- label には handler を書けない（E0433）．`SystemOutput` はどちらのストリームに書くかを知らないので，`Stdout by SystemOutput(1)` のように handler の引数で渡すことになる．`Platform` の中身が複雑になり，`SystemOutput(2)` を `Stdout` に差し込む誤りも書けてしまう．
- テスト用 handler も 1 つになるが，2 つの label に同じ `TestOutput` を差すと同じ `State` を取り合う．
- 別々の effect にしても，ユーザーは `effect AuditLog of Stderr` のように label を作れる．共通の `Output` は後から足せる．

### 書き込みを `suspend fn` にする

- パイプの相手が読まないときに書き手を待たせられる（背圧）．
- `Stdout` を使う関数がすべて Async を要求するようになり，Immediate が要る所（`[effect.async]`）で出力できなくなる．
- JS は同期で書き，`EAGAIN` のときはその場で待つ．C の `stdio` もバッファが一杯なら書き込みで待つ．どちらも書き手を止めて背圧をかけられるので，`Console` と同じく Immediate にする．待つ間は他の fiber も止まる．

### 閉じたパイプで終わらせ方を変える案

- 黙って捨てて続ける．出力し続けるプログラムが止まらなくなるので採らない．
- 終了コードを 0 にする．`head` で切るのはふつうの使い方とみなす案．ただし，本当に出力を書き切れなかったことを利用者が終了コードで確かめられなくなる．141 は，いまの C が SIGPIPE で終わるときの値（128 + 13）と同じで，`set -o pipefail` を使う利用者が選んで扱える．
- 終了コードを 130 にする（中断と同じ）．Ctrl-C で止めた場合と見分けられなくなる．
- `interrupted` の 1 行を出す．`| head` で切るたびに 1 行出てうるさい．
- release を走らせず，その場でプロセスを止める．単純だが，release が走らない．ほかの中断と揃えて巻き戻す．
- `Console` と `dbg` には当てない．同じストリームに書くので，片方だけ続けても意味がない．
- 標準エラーが閉じたときも終わらせる．ログの出口が閉じただけで本体が止まるのは困るので，捨てて続ける．

## Prior art

- プロセスに 3 つの標準ストリーム（入力，出力，エラー）を渡し，それぞれ別のハンドルとして扱う形は，POSIX のファイル記述子 0，1，2 と，WASI 0.3 の `wasi:cli/stdin`，`wasi:cli/stdout`，`wasi:cli/stderr` インターフェースに見られる．WASI は 3 つを別々のインターフェースにしており，コンポーネントは使うものだけを import する．
- 書き込める出口を effect の型で分けると，型シグネチャからどのストリームに書くかが読める．能力ベースの I/O（capability-based I/O）の一例．

## Open questions

なし．

## Decision
