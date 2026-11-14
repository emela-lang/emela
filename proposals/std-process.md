---
title: Std.Process, the arguments and the standard input of the process
authors: [Alliana]
status: draft
pr:
issue:
decided:
touches: [lib.layers, api.std.process (new), api.std.platform, cmd.build, target.c.run, target.wasm.output, target.wasm.external, cmd.test.output]
requires: []
related: []
---

# Std.Process, the arguments and the standard input of the process

## Summary

プロセスに属するものを置くモジュール `Std.Process` を std の platform 層に作り，まず 2 つの effect を置く．

- `Args`: プロセスに渡された引数を読む．位置で 1 つ取る `get(index)` と，全部を `List[String]` で返す `list()`．プログラム名は含めない
- `Stdin`: 標準入力を読む．1 行ずつ読む `read_line` と，残りを全部読む `read_text`．どちらも `suspend fn`

system handler `SystemArgs` と `SystemStdin` は `Platform` に入れる．テスト用に `State[StdinLines]` から読む `TestStdin` を置く．`Args` のテスト用 handler は置かない．`Console` は変えない（`Stdout` と `Stderr` は別の提案 stdout-stderr，終了コードの型 `ExitCode` は別の提案 exit-code で，どちらもこのモジュールに足す）．

```emel
import Std.Console.{Console}
import Std.Platform.{Platform}
import Std.Process.{Args, Stdin, InvalidInput, StdinFailure}

fn quote(prefix: String) -> () fails InvalidInput | StdinFailure uses { Console, Stdin } {
  (use Stdin).read_line() match {
    None -> ()
    Some(line) -> {
      (use Console).print_line("#{prefix}#{line}")
      quote(prefix)
    }
  }
}

fn main() {
  with Platform {
    prefix = (use Args).get(0) match {
      Some(p) -> p
      None    -> "> "
    }
    quote(prefix)
  }
}
```

## Motivation

- 今の Emela のプログラムは外から入力を受け取れない．`emela run` は `--` の後の引数を渡し，標準入力を引き継ぐ（`[cmd.build]`，`[target.c.run]`，`[target.wasm.run]`）が，プログラムの中から読む API がない．atlas の移植メモにも「argv も stdin も無い．入力は定数で埋め込むしかない」とある
- CLI ツールを書くには，まず引数が要る．引数の解析（フラグ，サブコマンド，usage の表示）は Pome のライブラリに任せるとして，その材料になる `List[String]` を受け取る手段は std が持つ必要がある
- CLI ツールの多くはパイプの途中に置かれる（`cat data.json | tool`，`git diff | tool`）．標準入力を読めないと，入力はファイルのパスを引数で受けて `Std.FileSystem` で読むしかない
- 対話的なプログラム（REPL，確認プロンプト）も書けない
- 回避策は Pome が `@external` で `process.argv` や `node:readline` を呼ぶこと．JS でしか動かず，使う側は `[external]` で許可が要る（`[module.pome]`）．`[ext.decl]` は「多くのプログラムが使う host の機能は std に platform effect として足す」としている

## Proposal

### Meaning

```emel
# Std.Process
pub effect Args {
  fn get(index: Int) -> Option[String]
  fn list() -> List[String]
}

pub handler SystemArgs: Args

pub error InvalidInput
pub error StdinFailure(message: String)

pub effect Stdin {
  suspend fn read_line() -> Option[String] fails InvalidInput | StdinFailure
  suspend fn read_text() -> String fails InvalidInput | StdinFailure
}

pub handler SystemStdin: Stdin

pub type StdinLines(lines: List[String])
pub handler TestStdin: Stdin
```

#### モジュール

- `Std.Process` は，プロセスが 1 つずつ持つもの（引数，標準入力，後の提案で標準出力，標準エラー，終了コード）を置くモジュール
- 今の std は「effect ごとに同じ名前のモジュール」（`Std.Clock.Clock`）だが，`Std.Process` は複数の effect を 1 つのモジュールに置く．system handler とテスト用 handler は，今と同じく effect と同じモジュールに置く

#### Args

- `list` は，プロセスに渡された引数を渡された順に返す．プログラム名（host の実行ファイル，`main.mjs`，`main.wasm`，C の `argv[0]`）は含めない．引数がなければ `[]`
- `get(index)` は `list` の `index` 番目（0 始まり）を返す．範囲外（負の数を含む）は `None`（`[lib.api-rules]` の「範囲外は Option」）
- 呼ぶたびに同じ結果を返す．起動後に変わることはない
- 各ターゲットで返すもの:
  - `emela run`: `--` の後に書いた引数（全ターゲット）
  - C: 実行ファイル `main` を直接起動したときは `argv[1]` 以降
  - JS: runtime の launcher が runtime に渡した引数
  - WASM: `wasi:cli/environment@0.3.0` の `get-arguments` から先頭の 1 つ（プログラム名）を除いたもの
  - HTTP サービス: `[]`（`emela run` が `--` の後の引数を host に渡す形なので，プログラムには届かない）
- UTF-8 として正しくない引数（C と，POSIX の JS host で起こりうる）は，正しくない部分を U+FFFD に置き換える．fail はしない．理由は Alternatives
- `emela test` の中では `[]`．テストランナーが実行ファイルに渡す引数（テストの選択など）は見えない
- `SystemArgs` は Async を要求しない．状態を持たない
- テスト用 handler は置かない．`get` と `list` の 2 行で自分で書ける（`Std.Env` に `TestEnv` がないのと同じ扱い）

#### Stdin

- `read_line` は次の行を返す．行末の `\n` と，その直前の `\r` は含めない．最後の行に `\n` がなくても 1 行として返す．入力が尽きていれば `None`．`None` を返した後も，呼ぶたびに `None`
- `read_text` は残りの入力を全部返す．改行はそのまま含む．尽きていれば `""`
- 2 つは同じ位置を共有する．`read_line` の後の `read_text` はその続きを返す
- 行（`read_text` なら残り全部）が UTF-8 として正しくなければ，その行を消費したうえで `InvalidInput` で fail する．置換文字には置き換えない（`[api.std.bytes]`，`[api.std.file-system]` の `InvalidText` と同じ方針）．次の `read_line` は次の行から読む
- host から読めなかったときは `StdinFailure(message)`．`message` は host の文
- 待っている fiber が中断されたら，読み込みは取り消される．host から受け取り済みの入力は捨てずに残し，次の読み込みが返す．行が失われることはない．行に切り分けるために runtime はもともと受け取った入力をバッファに持つので，残しても手間は増えない
- `SystemStdin` は Async を要求する（`Platform` はすでに要求している）．他の fiber は入力を待つ間も動く
- `emela test` では標準入力をプログラムに渡さない（`[cmd.test.output]`）．`SystemStdin` は空の入力を読む（`read_line` は `None`，`read_text` は `""`）
- WASM ではこの提案の範囲外．runtime に host の stream を待つ仕組みが要り，`SystemClock.sleep` が WASM で動かないのと同じ理由（Implementation）

### テスト

`TestStdin` は `State[StdinLines]` の先頭から 1 行ずつ取り出す．`TestConsole` が `State[List[String]]` を使うので，型を分けるために `StdinLines` で包む（`[effect.state]`）．名前はストリームの名前を頭に付ける形で，stdout-stderr の提案の `StdoutText`，`StderrText` と揃える．`State.run` の最後の状態は，読まれなかった行になる．

```emel
import Std.Console.{TestConsole}
import Std.Process.{Stdin, TestStdin, StdinLines}
import Std.State.{State}

@test
fn quotes_each_line() {
  ((_, rest), lines) = State.run([], || {
    State.run(StdinLines(lines: ["a", "b"]), || with TestStdin, TestConsole { quote("> ") })
  })
  assert lines == ["> a", "> b"]
  assert rest == StdinLines(lines: [])
}
```

- `TestStdin` の `read_text` は残りの行を `\n` でつなぎ，状態を空にする．`InvalidInput` と `StdinFailure` は出さない
- 試作: `Stdin`，`StdinLines`，`TestStdin` をユーザーのモジュールとして書き，`emela check` と `emela test` が通ることを確かめた．`Args`（`get` と `list`）と自前の handler `FixedArgs` で Summary の `main` の形が `emela check`，`emela run`，`emela fmt` を通ることも確かめた（0.x，2026-11-14）

### Typing rules

新しい規則はない．`Args` と `Stdin` はふつうのユーザー定義 effect と同じ規則で型付けされる．

## Specification changes

### `[lib.layers]`

In the table of layers, the platform row becomes:

| Layer | Nature | Contents |
|---|---|---|
| platform | Effect declarations and the host handlers | Console, Process (Args, Stdin), Clock, Random, Env, FileSystem, Http |

The paragraph on system and test handlers becomes:

- A system handler that uses the host has a name prefixed with `System` (`SystemConsole`, `SystemArgs`, `SystemStdin`, `SystemClock`, `SystemRandom`, `SystemEnv`, `SystemFileSystem`, `SystemHttp`). A test handler is prefixed with `Test` (`TestClock`, `TestConsole`, `TestStdin`). Both are placed in the same module as their effect (`import Std.Clock.{Clock, SystemClock, TestClock}`).
- A platform module usually declares one effect and has the name of that effect (`Std.Clock`). `Std.Process` declares several effects, one for each thing the process has exactly one of (its arguments and its standard input), and their handlers (`import Std.Process.{Args, Stdin, SystemStdin}`).

### `[api.std.process]` (new, after `[api.std.console]`)

### Std.Process (`std/process.emel`, `std/process.mjs`, `std/process.c`) `[api.std.process]`

The platform layer and the test layer (`[lib.layers]`). What the process has exactly one of.

| Declaration | Contents | Description |
|---|---|---|
| `Args` | `effect Args { fn get(index: Int) -> Option[String]; fn list() -> List[String] }` | The arguments of the process, in order, without the name of the program. `list` returns all of them, and `get` the one at `index` (from 0), or `None` when `index` is out of range |
| `SystemArgs` | `handler SystemArgs: Args` | The arguments given by the host. Has no state |
| error | `InvalidInput`, `StdinFailure(message: String)` | InvalidInput when standard input is not valid UTF-8. StdinFailure when the host cannot read standard input; `message` is the host's text |
| `Stdin` | `effect Stdin { suspend fn read_line() -> Option[String] fails InvalidInput \| StdinFailure; suspend fn read_text() -> String fails InvalidInput \| StdinFailure }` | Reads standard input as text (UTF-8). `read_line` returns the next line without its line terminator, and `None` at the end of the input. `read_text` returns the rest of the input, line terminators included, and `""` at the end of the input |
| `SystemStdin` | `handler SystemStdin: Stdin` | The standard input of the process. `process.stdin` on JS and file descriptor 0 on C. Requires Async. Has no state |
| `StdinLines` | `type StdinLines(lines: List[String])` | The state of TestStdin: the lines not read yet |
| `TestStdin` | `handler TestStdin: Stdin` | Kept in `State[StdinLines]` and resumes immediately. Does not raise InvalidInput or StdinFailure |

Args:

- `list` of SystemArgs returns the same list on every call, and `get(index)` returns its element at `index`, or `None` when `index` is negative or not less than its length. The list is:
  - for `emela run`, the arguments written after `--` (`[cmd.build]`), on every target;
  - on C, `argv[1]` and after;
  - on JS, the arguments that the runtime's launcher passes to the runtime;
  - on WASM, the result of `get-arguments` of `wasi:cli/environment@0.3.0` without its first element;
  - `[]` for an HTTP service (`[lib.http-service]`) and under `emela test` (`[cmd.test.output]`).
- An argument that is not valid UTF-8 is decoded with each maximal invalid subsequence replaced by U+FFFD. Neither operation fails.

Stdin:

- A line ends at `\n`. The `\n`, and a `\r` immediately before it, are not part of the line. The text after the last `\n`, if not empty, is the last line. Once `read_line` has returned `None`, every later call returns `None` and every later `read_text` returns `""`.
- `read_line` and `read_text` read from the same position. `read_text` after `read_line` returns the input that follows the line.
- When a line (for `read_text`, the rest of the input) is not valid UTF-8, it is consumed and the operation fails with InvalidInput. Invalid UTF-8 is not replaced with the replacement character, as in `[api.std.bytes]`. The next `read_line` starts at the following line. U+0000 is read as is on every target.
- When the fiber waiting in an operation of SystemStdin is interrupted (11.4), the operation is canceled. Input that the host has already delivered is kept and returned by the next operation; no input is lost.
- Waiting for input does not stop other fibers on any target.
- SystemStdin does not hold the host open: a program that has no operation of Stdin waiting ends when its main ends, even if standard input is still open (a terminal, for example).
- Under `emela test`, standard input is not passed to the program (`[cmd.test.output]`). SystemStdin reads it as an empty input.
- `read_line` of TestStdin removes the first element of `lines` and returns it, or returns `None` when `lines` is empty. `read_text` returns the elements of `lines` joined with `\n` and sets `lines` to `[]`.

impl (`[trait.derive]`): StdinLines has `derive Eq, Show`. Both errors have `derive Show`.

```emel
import Std.Console.{Console}
import Std.Process.{Args, Stdin, InvalidInput, StdinFailure}

fn quote(prefix: String) -> () fails InvalidInput | StdinFailure uses { Console, Stdin } {
  (use Stdin).read_line() match {
    None -> ()
    Some(line) -> {
      (use Console).print_line("#{prefix}#{line}")
      quote(prefix)
    }
  }
}
```

### `[api.std.platform]`

| Declaration | Contents | Description |
|---|---|---|
| `Platform` | `layer Platform { SystemConsole, SystemArgs, SystemStdin, SystemClock, SystemRandom, SystemEnv, SystemHttp, SystemFileSystem }` | A bundle of the System handlers. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

- The order of the list has no meaning (`[effect.layer]`). SystemStdin, SystemClock, SystemHttp, and SystemFileSystem require Async, so Async remains in the R of `with Platform { ... }`. In main and test functions, the runtime provides Async.

### `[cmd.build]`

In "Running the program (JS)", the item on arguments becomes:

- `run` loads the entry module and calls the exported `main` with no arguments. The return value of `main` is discarded. Arguments to the program are passed after `--` as arguments of the JS host, and the launcher passes them to the runtime, where `Args.list` of SystemArgs returns them (`[api.std.process]`).

### `[target.c.run]`

The first item becomes:

- `emela run --target c` starts `main` in the output directory. It passes the arguments after `--`, and the program inherits standard input. `Args.list` of SystemArgs returns `argv[1]` and after (`[api.std.process]`). The exit code is as in `[cmd.build]`.

### `[target.wasm.output]`

In the item on imports, `Std.Env` becomes `Std.Env` or `Std.Process`:

- The component always imports `wasi:cli/stdout@0.3.0`, `wasi:cli/stderr@0.3.0` and `wasi:cli/exit@0.3.0`. It also imports `wasi:cli/environment@0.3.0`, `wasi:random/random@0.3.0` and `wasi:clocks/system-clock@0.3.0`, each only when the entry reaches a host function of `Std.Env` or `Std.Process`, `Std.Random` or `Std.Clock` that uses it (`[target.wasm.external]`). ...

### `[target.wasm.external]`

Add a row to the table of handlers that work on WASM:

| Handler | Host function |
|---|---|
| `SystemArgs` (`get`, `list`) | `get-arguments` of `wasi:cli/environment@0.3.0`, called once per run. The list without its first element |

The last item becomes:

- `SystemClock.sleep`, `SystemStdin`, `Std.FileSystem` and the client of `Std.Http` (`SystemHttp`) are not available on WASM yet. The entry adapter `Http.serve` is (`[lib.http-service]`).

### `[cmd.test.output]`

- What the program writes to standard output while tests run appears as is between the result lines. The program's standard error goes to standard error as is. Standard input is not passed to the program: `SystemStdin` reads an empty input, and `SystemArgs` returns `[]` (`[api.std.process]`).

## Diagnostics

- [ ] 新しい診断はない．WASM で `SystemStdin` の操作に到達すると，既存の E0412 になる（`[lib.layers]`）

## Compatibility

- 既存のプログラムは変わらない．`Console` も変えない
- `Platform` に `SystemArgs` と `SystemStdin` が増える．`with Platform, MyStdin { ... }` のような置き換えは新しくできるようになるだけ．`Platform` と同じ with に自分で `Args` や `Stdin` の handler を並べていたプログラムは，後に書いた方が優先されるので挙動は変わらない
- ユーザーが自分の `Std.Process` というモジュールを持つことはできない（`Std` の下なので，`[lib.layers]`）．ユーザーの `process.emel` とはぶつからない
- 後続の stdout-stderr と exit-code は，このモジュールに `Stdout`，`Stderr`，`ExitCode` を足す．どちらが先に採択されても `[api.std.process]` の表に行を足す形にする
- JS で launcher を通さずに起動する方法（`node main.mjs a b`）は，今の仕様にもなく，この提案でも扱わない

## Implementation

Args:

- JS: launcher は `process.argv` のうち `--` の後の `main.mjs` より後ろを runtime に渡す．`process.argv` の先頭何個が host とスクリプトかは host と起動の仕方（`-e` かファイルか）で変わるので，runtime が `process.argv` を直接切り出さない
- C: runtime の `main` が `argc` と `argv` を保存し，`SystemArgs` の最初の呼び出しで UTF-8 に直して（不正な部分を U+FFFD に置き換えて）リストを作る
- WASM: `get-arguments` を 1 回だけ呼ぶ．import は `SystemArgs` の操作に到達したときだけ足す（`get-environment` と同じ interface）
- JS host の多くは引数を自分で文字列にする．Node は不正なバイトを U+FFFD に置き換えるので，そのまま返せば C と揃う

Stdin:

- JS: `process.stdin` の `data` を runtime のバッファに貯め，`read_line` / `read_text` の待ちをそこから満たす．待ちがないときは `pause()` し，バッファが空でなくても `unref` 相当でイベントループを保たない．`Stdin` の操作に一度も到達しないプログラムは `process.stdin` に触らない（触ると TTY のときに終わらなくなる）．UTF-8 の検査は `TextDecoder("utf-8", { fatal: true })` を行ごとに使う
- C: fd 0 を `em_on_readable` で待つ．epoll はふつうのファイルを登録できない（`EPERM`）ので，そのときは同期で読む（ふつうのファイルは待たずに読めるので，他の fiber を止めない）．U+0000 を含む行は `em_complete_string` で返せないので，runtime の内部の完了関数で長さ付きで返す（`[ext.c-host]` の公開 API は変えない）
- WASM: `wasi:cli/stdin@0.3.0` の `read-via-stream` が使えるが，runtime が host の stream を待つ仕組みをまだ持たない（`SystemClock.sleep` と同じ理由）．この提案では入れない

テスト:

- `emela test` で `Args.list` が `[]`，`Args.get(0)` が `None`，`read_line` が `None` を返すこと
- `emela run -- a 'b c' ''` で `list` が `["a", "b c", ""]` を返し，`get(2)` が `Some("")`，`get(3)` と `get(-1)` が `None` を返すこと（JS，C，WASM）．Node と Bun の両方
- C で不正な UTF-8 の引数が U+FFFD に置き換わること
- JS と C で `\r\n`，最後の改行なし，不正 UTF-8 の後の行，`read_line` の後の `read_text`，中断の後に行が失われないこと

## Alternatives

### モジュールと名前

- 役割の名前（`Input`，`Output`，`Diagnostics`）にする．Workers やブラウザのように stdin や stdout を持たない host でも読みやすい．採らない理由:
  - 一般的な語なので，ユーザーのコードの名前と import でぶつかりやすい
  - この effect が約束しているのは，プロセスに渡されたストリームとしての振る舞い（EOF があり，パイプやリダイレクトでつなぎ替えられ，出力とは別に診断用の出口がある）．役割の名前ではその約束が見えない
- effect ごとにモジュールを作る（`Std.Args`，`Std.Stdin`）．今の std の形に揃う．採らない理由: 引数，標準入出力，終了コードはどれもプロセスに 1 つずつあるもので，一緒に使われる．1 つのモジュールにまとめると探す場所が 1 つで済み，`import Std.Process.{Args, Stdin, Stdout}` と 1 行で書ける
- `Console` のモジュールに入力を足す（`Console` と `ConsoleInput`）．入出力の名前が揃わない．`Console` は人に読ませる行の出力としてそのまま残し，プロセスのストリームとしての出力は stdout-stderr で `Std.Process` に `Stdout`，`Stderr` として足す

### Args

- `Std.Env` の `Env` に `fn args()` を足す．WASI でも環境変数と引数は同じ `wasi:cli/environment` にある．採らない理由: 既存の `Env` の handler を自分で書いているプログラムがすべて E0404 で壊れる．環境変数と引数を別々に差し替えられなくなる
- `fn main(args: List[String])` にする．main から深いところまで引数を手で渡し回すことになる．`@entry` や HTTP サービスの入口とも形が揃わない．ホストに触るものは effect で受け取るという std の形からも外れる
- プログラム名を先頭に含める．ターゲットごとに中身が違う（JS は host のパスかスクリプト，WASM は `main.wasm`，C は起動の仕方次第）ので，含めても移植できるプログラムでは使えない
- 操作を 1 つにする（`list` だけ，または `get` だけ）．`list` だけだと「n 番目があれば」をリストのパターンで毎回書くことになり，`get` だけだと全部を解析ライブラリに渡すときに数を知る手段がない．`Env.get` と同じく位置で引けば `Option` が返る `get` と，全部を返す `list` の両方を持つ
- 不正な UTF-8 の引数で fail する（`list() -> List[String] fails InvalidInput`）．Stdin とは揃うが，採らない理由:
  - 引数は起動時に OS が渡す固定の値で，読み直しても変わらない．fail しても回復の手段がない
  - 引数を読む場所は main の直後がほとんどで，そこで fail を扱わせると CLI のすべてに `escape` が要る
  - WASI の `get-arguments` は `list<string>` で，host がすでに文字列にしている（wasmtime は不正な引数を渡せない）．Node も U+FFFD に置き換えてから渡す．C だけ fail させると，ターゲットで結果が変わる
  - Stdin は「読んで書き戻す」フィルタに使われるので，黙って置き換えると入力が変わったことに気づけない．引数にはその使い方がない
- Bytes のリストで返す（`List[Bytes]`）．パスに不正なバイトを含むファイルも扱えるが，Path の扱いを決めてから後の提案で扱う
- テスト用 handler `TestArgs` を置く．自分で 1 行で書けるので置かない（`Std.Env` と同じ）

### Stdin

- `Console` に `read_line` を足す．入出力が 1 つの effect にまとまり，`with SystemConsole` だけで済む．採らない理由:
  - `Console` の handler を自分で書いているプログラム（ログを捨てる handler など）が，すべて E0404 で壊れる
  - `Console` の操作は `fn` で Immediate だが，読み込みは待つので `suspend fn` になる．effect が Immediate かどうか（`[effect.decl]`）が変わり，`TestConsole` が `Async` なしで使えるという性質も揺らぐ
  - `TestConsole` が出力と入力の 2 つの State を持つことになる
- `read_line` だけにする．`read_text` は `read_line` の繰り返しで書けるが，改行を復元できない（最後の改行の有無，`\r\n`）．JSON などを丸ごと読むときに元の入力をそのまま受け取れる操作が要る
- Bytes で読む（`suspend fn read(max: Int) -> Option[Bytes]`）．バイナリの入力や，UTF-8 でない入力も扱える．TCP の提案の `read` と形が揃う．採らない理由: バイナリのフィルタには書く側の `Stdout` にも Bytes 版が要り，テキストの読み込みと混ぜたときの UTF-8 の境界の扱いも決める必要がある．`Stdout` の Bytes 版と一緒に別の提案にする．後から同じ effect に足しても既存の呼び出しは壊れない
- WASM でも使えるようにする．`wasi:cli/stdin@0.3.0` の `read-via-stream` はあるが，runtime が host の stream を待つ仕組みをまだ持たない．その仕組みは `SystemClock.sleep` を WASM で動かすのにも要るので，両方を一緒に別の提案で扱う
- 中断のとき，host から受け取り済みの入力を捨てる．実装は少し簡単になるが，行に切り分けるために runtime はもともと受け取った入力をバッファに持つので，残す手間はほとんど変わらない．捨てると，タイムアウト付きで読むプログラムが行を失う
- 不正な UTF-8 を U+FFFD に置き換える．Args と揃うが，上の理由で採らない．`Bytes.decode_utf8` と `FileSystem.read_text` も置き換えないので，そちらに揃える
- 入力を Stream として返す．std に Stream がまだない（`[lib.layers]` の effect 層で後の版）
- `fn main(stdin: ...)` のように入口で受け取る．深い呼び出し先から読めず，テストで差し替えにくい

## Prior art

- Rust の `std::env::args` はプログラム名を先頭に含め，不正な Unicode の引数で panic する．`args_os` は `OsString` を返す
- Go の `os.Args` はプログラム名を先頭に含む `[]string` で，バイト列をそのまま持つ
- Node の `process.argv` は host とスクリプトのパスを先頭に含む
- WASI 0.3 の `wasi:cli/environment` の `get-arguments` は `list<string>` を返す
- Rust の `std::io::Stdin` は `read_line(&mut String)` と `lines()` を持ち，行末を含めて返す．不正 UTF-8 は `InvalidData` のエラー
- Go の `bufio.Scanner` は行末（`\r\n` の `\r` も）を取り除いて返す
- Node の `readline` は行末を取り除き，`\r\n` も 1 つの改行として扱う
- WASI 0.3 の `wasi:cli/stdin` は `read-via-stream` でバイトの stream を返すだけで，行の扱いはない

## Open questions

## Decision
