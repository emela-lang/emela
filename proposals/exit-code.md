---
title: Choosing the exit code with the return value of main
authors: [Alliana]
status: draft
pr: 567
issue:
decided:
touches: [dyn.error, effect.with, diag.list, cmd.build, target.c.run, target.wasm.output, ext.export, lib.layers, api.std.process (new)]
requires: [std-process]
related: [std-exit, stdout-stderr]
---

# Choosing the exit code with the return value of main

## Summary

`Std.Process`（std-process 案で作るモジュール）に値の型 `ExitCode` と，定数 `EXIT_SUCCESS`（0）と `EXIT_FAILURE`（1）を足す．`main` の戻り値の型が `ExitCode` のとき，`main` が値で終わったらプログラムはその終了コードで終わる．0 から 255 のすべての値を返せる．`main` の戻り値の型は `()` か `ExitCode` にし，それ以外の型を返す `main` には警告 W0304 を出す（値は今と同じく捨てて 0）．error は 1，defect は 101，中断は 130 で変わらない．プロセスをその場で止める関数や effect 操作は足さない．

```emel
import Std.Platform.{Platform}
import Std.Console.{Console}
import Std.Process.{EXIT_SUCCESS, ExitCode}

error UsageError(message: String)

fn main() -> ExitCode {
  with Platform {
    run() escape {
      UsageError(message:) -> {
        (use Console).print_error_line("usage: tool <file>: #{message}")
        Process.exit_code(2)
      }
    }
  }
}

fn run() -> ExitCode fails UsageError uses { Console } {
  (use Console).print_line("ok")
  EXIT_SUCCESS
}
```

## Motivation

- 今の Emela のプログラムが返せる終了コードは 0，1，101，130 の 4 つだけ（`[dyn.error]`，`[dyn.suspend]`，`[error.defect]`）．`main` の戻り値は捨てられる（`fn main() -> Int { 2 }` は 0 で終わることを確かめた）
- CLI ツールは終了コードで結果を返す．usage の誤りは 2，「見つからなかった」は 1 で「失敗」は 2 以上，といった取り決めは呼び出す側のシェルスクリプトや CI が読む．Emela でこれを書く手段がない
- 回避策は `@external` で `process.exit` や C の `exit` を呼ぶことだけ．これは with の release を飛ばし，標準出力の書き残しを切り，ターゲットごとに書き分けが要る
- error を `main` まで上げると，終了コードは 1 に決まり，`error: <値>` の 1 行も必ず出る．usage の表示を自分で書きたい場合に合わない

## Proposal

### Syntax

変更なし．

### Meaning

- `Std.Process` に opaque type `ExitCode` を宣言する．中身は 0 から 255 までの Int．`Std.Process` は std-process 案が作るモジュールで，この案はそこに型と関数を足す（`requires: [std-process]`）．
- `Process.exit_code(code)` は `code` の `ExitCode` を返す．`code` が 0 から 255 の外なら defect（呼び出し側の前提が壊れている，`[lib.api-rules]`）．`Process.exit_code_to_int` はその Int を返す（メソッド呼び出しなら `code.exit_code_to_int()`）．
- 関数名に `exit_code` を入れるのは，`Std.Process` には `Args` や `Stdin` も同居するので，`Process.from_int` や `Process.to_int` では何の変換か読めないため．
- 層: `ExitCode` 自体はホストに依らない値だが，意味を持つのはプロセスの終わり方だけなので，platform 層の `Std.Process` に置く．`[lib.layers]` の platform 層の行に「プロセスに属する値の型も置く」と書き足す．`Std.Process` を import しても handler は要らない（`ExitCode` を作るだけなら effect を使わない）．
- `main` の戻り値の型が `ExitCode` で，`main` が値で終わったとき，プログラムはその値の終了コードで終わる．error，defect，中断で終わったときは今と同じ（1，101，130）．
- 戻り値の型が `()` の `main` は今と同じく，値で終われば 0．
- 戻り値の型が `()`，`ExitCode`，Never のどれでもない `main`（`fn main() -> Int { 2 }` など）は警告 W0304．動きは今と同じで，値を捨てて 0 で終わる．今は黙って 0 で終わるので，`Int` を返せば終了コードになると思って書いた人が気づけない．Never（本体が必ず fail するなど）は値を返さず，捨てるものがないので警告しない．
- 0 から 255 のすべての値を返せる．1，101，130 も返せる．この 3 つは runtime が異常終了（error，defect，中断）に使う番号にすぎず，ユーザーが返す値を制限する理由にはならない．呼び出す側は，それが error などから来たのか `main` が返したのかを終了コードだけでは区別できず，標準エラー出力の 1 行（`error: ...` など）の有無で区別する．
- 上限 255 は Emela の都合ではなく，POSIX の終了ステータスが 8 ビットであることと，WASI の `exit-with-code` が u8 を取ることによる．範囲外は呼び出し側の前提が壊れているので defect（`[lib.api-rules]`）．256 で割った余りにはしない（`exit_code(256)` が黙って成功になるため）．
- 定数 `EXIT_SUCCESS`（`exit_code(0)`）と `EXIT_FAILURE`（`exit_code(1)`）を `pub const` で置く．const は修飾名で書けず import して使うので（次の項目），裸の名前のままユーザーのコードに入る．`SUCCESS` だけでは何の成功か読めず，ユーザーの名前ともぶつかりやすいので，`EXIT_` を付けて何の定数か分かる名前にする．std の const は大文字の名前（`[data.const]`）．std に `pub const` を置くのはこれが初めてだが，`pub` の対象に `const` は含まれ（`[module.pub]`），opaque type の値も右辺の関数呼び出しで作れる（`[data.const]`）．
- 他のモジュールの const は `Process.EXIT_SUCCESS` とは書けない．式の修飾名は `Module.lower_name` だけで（`[grammar.expr]`），const は import して使う（`import Std.Process.{EXIT_SUCCESS}`）．試作で，別モジュールの `pub const SUCCESS: ExitCode = exit_code(0)` を import して `main` から返し，`emela check` と `emela run` が通ることを確かめた（emela 0.27.0，2026-11-14）．`Process.SUCCESS` は E0110 になる（名前を `EXIT_SUCCESS` にしても規則は同じ）．
- `main` が値を返すのは，with の release がすべて走り，`Fiber.scope` の子がすべて終わった後なので，終了コードを選ぶことで後始末が飛ぶことはない．終了コードは標準出力と標準エラー出力の書き込みが終わってから決まる（`[cmd.build]` の既存の保証と同じ）．
- `main` は普通の fn なので，テストから呼ぶと `ExitCode` の値が返るだけで，プロセスは終わらない．
- `ExitCode` は境界型（12.4）ではないので，`@export` の fn や entry adapter からは返せない（E0436）．HTTP サービスは変わらない．

### Typing rules

- 新しい型付け規則はない．`main` の戻り値の型は推論でも注釈でもよい．`ExitCode` かどうか，W0304 を出すかは，型検査の後で決まった戻り値の型で判断する．
- W0304 は警告なので，型検査は通り，`build` も `run` もできる．`main` の R が Async 以外空であるという規則（`[effect.with]`）とは独立で，あちらは R，こちらは戻り値の型だけを見る．

## Specification changes

### `[api.std.process]`

The section is created by std-process. Add these rows to its table, after the rows of that proposal, and add the paragraph and the impl line after the table:

```markdown
| `ExitCode` | `opaque type ExitCode` | The exit code of a program. Its contents are an Int from 0 to 255 |
| `exit_code` | `exit_code(code: Int) -> ExitCode` | The exit code `code`. A `code` outside 0 to 255 is a defect. Implemented in Emela |
| `exit_code_to_int` | `exit_code_to_int(code: ExitCode) -> Int` | The number of the exit code. Implemented in Emela |
| `EXIT_SUCCESS` | `const EXIT_SUCCESS: ExitCode` | `exit_code(0)` |
| `EXIT_FAILURE` | `const EXIT_FAILURE: ExitCode` | `exit_code(1)` |
```

```markdown
- A `main` whose return type is `ExitCode` chooses the exit code of the program with its value (`[dyn.error]`). Every value from 0 to 255 can be chosen, including 1, 101 and 130. `ExitCode` is a value: making one uses no effect, and no handler is needed.
- The constants are named with the prefix `EXIT_` because a const is used through an import, not as a qualified name (`[grammar.expr]`), so its name appears in user code on its own.
- The upper bound 255 comes from the exit status of POSIX processes and the argument of `exit-with-code` of WASI, which are 8 bits wide.
- impl of `ExitCode` (`[trait.derive]`, `[trait.builtin]`): `derive Eq, Show`. The Show impl gives `ExitCode(2)`.
```

### `[lib.layers]`

In the table of layers, the platform layer becomes (the list of modules follows std-process and stdout-stderr, whichever are adopted):

```markdown
| platform | Effect declarations and the host handlers, and the value types used only with them | Console, Clock, Random, Env, FileSystem, Http, Process |
```

After the table, add:

```markdown
- A module of the platform layer MAY declare value types that are independent of the host but have meaning only together with its effects (`Request` and `Response` of Http, `ExitCode` of Process). They are not listed in the value layer, which holds values that are useful on their own.
```

### `[dyn.error]`

Replace the third item with:

```markdown
- When main ends normally, the program exits with exit code 0, and the return value is discarded without being shown. The exception is a main whose return type is `ExitCode` (`[api.std.process]`): the program exits with the exit code of the value. To see a value, use `dbg`.
- The return type of main SHOULD be `()` or `ExitCode`. A main whose return type is none of `()`, `ExitCode` and Never is W0304; it still runs, and its value is discarded as above.
- The exit code chosen by an `ExitCode` MAY be 1, 101 or 130. The exit code alone then does not tell it apart from an error, a defect or an interruption; only the line written to standard error does.
```

### `[effect.with]`

Add after the item on the R of main:

```markdown
- The return type of main is `()` or `ExitCode` (`[api.std.process]`); another type, other than Never, is W0304 (`[dyn.error]`). This is separate from the rule on R above.
```

### `[diag.list]`

Add after W0303:

```markdown
| W0304 | The return type of main is none of `()`, `ExitCode` and Never (`[dyn.error]`). Points at the return type annotation of main, or at its name when the type is inferred. The help is ``= help: return `()`, or return an `ExitCode` to choose the exit code`` | ``the value of type `Int` returned by `main` is discarded`` |
```

### `[cmd.build]`

In "The exit code is as follows", replace the second item with:

```markdown
  - When `run` reaches the program, the exit code of the program: 0, or the value of the `ExitCode` that `main` returned (`[dyn.error]`), when main ends with a value; 1 for an error (14.3); 101 for a defect (7.5); 130 for an interruption (14.5).
```

In "Running the program (JS)", replace "The return value of `main` is discarded." with:

```markdown
The return value of `main` is discarded, except that when the return type of `main` is `ExitCode` the program exits with its exit code.
```

and add a row at the top of the table:

```markdown
| `main` ended with a value of `ExitCode` (`[dyn.error]`) | nothing | the exit code of the value |
```

### `[target.c.run]`

Add after the second item:

```markdown
- When `main` returns an `ExitCode` (`[dyn.error]`), the executable exits with its exit code, after standard output and standard error are flushed.
```

### `[target.wasm.output]`

Replace "an exit code other than 0 is passed with `exit-with-code`" with:

```markdown
an exit code other than 0, including one that `main` returns as an `ExitCode` (`[dyn.error]`), is passed with `exit-with-code`
```

### `[ext.export]`

No text change. `ExitCode` is not a boundary type (12.4), so an `@export` fn or an entry adapter that returns it is E0436 as before.

## Diagnostics

- [ ] W0304：戻り値の型が `()`，`ExitCode`，Never のどれでもない `main`．message は ``the value of type `Int` returned by `main` is discarded``，help は ``= help: return `()`, or return an `ExitCode` to choose the exit code``．位置は戻り値の型注釈，推論のときは `main` の名前．W03xx（型の警告）の空いている最初の番号
- `exit_code` の範囲外は実行時の defect で，診断は足さない．メッセージは `exit code <n> is out of range 0 to 255`

## Compatibility

- 既存のプログラムの動きは変わらない．`ExitCode` は新しい型なので，今の `main` の戻り値の型が `ExitCode` であることはない．
- `()` 以外を返している既存の `main` には W0304 が新しく出る．警告なので build も run も通る．examples と spec のコード例に該当する `main` はない（`git grep` で確かめた）．
- コード例は今の仕様にある `Std.Console` を使う．この案は stdout-stderr 案に依存しないので，どちらが先に採択されても単独で成り立つ．stdout-stderr 案が先に採択されたら，仕様へ貼るときにコード例の import を `Std.Process.{Stderr, Stdout}` に書き換える（意味は変わらない）．

## Implementation

- コンパイラは型検査の後，entry の `main` の戻り値の型が `Std.Process.ExitCode` かを見て，launcher に渡す情報を変える．
- JS: launcher は `main` の値（Int）を `process.exitCode` に入れる．既存の 101，130 と同じく，書き込みが終わってから終わる（`crates/emela-driver/src/run.rs` の launcher）．suspend する `main` は `$start` の完了で値を受け取る．
- C: 生成する C の `main` が `ExitCode` の Int を返す．stdout と stderr は `exit` の前に flush する．`EMELA_LEAK_CHECK` の 99 は今と同じく優先する．
- wasm: `wasi:cli/run` の `run` は `result` しか返せないので，0 以外は既に import している `wasi:cli/exit@0.3.0` の `exit-with-code` で返す．
- `ExitCode` と 2 つの関数は std-process 案の `std/process.emel` に純 Emela で書ける（`opaque type ExitCode = Int` と範囲検査）．host ファイルは要らない．

## Alternatives

### 独立したモジュール `Std.ExitCode` に置く

- 値の層のモジュールとして `Std.ExitCode` を作り，`ExitCode.from_int(2)` と書けるようにする．`[lib.api-rules]` の `from_*` と `to_*` の名前がそのまま使える．
- 却下の理由: `Args`，`Stdin`，`Stdout`，`Stderr` と同じく「プロセスに属するもの」なので，`Std.Process` にまとめると探す場所が 1 つで済む（作者の決定）．プロセスの終わり方にしか使わない型のためだけにモジュールを 1 つ足すことにもなる．

### `Std.Exit` に足す

- `Std.Exit`（`[api.std.exit]`）に `ExitCode` を足し，`import Std.Exit.{ExitCode}` で使う．
- 却下の理由: `Std.Exit` は計算の終わり方（`Exit[A, E]`，`attempt`）で，fiber の中でも使う．プロセスの終了コードとは別の概念なのに，`Exit` と `ExitCode` が同じモジュールに並んで紛らわしい．

### サブモジュール `Std.Process.ExitCode` に置く

- `Std.Http.Request` と同じ形になり，`ExitCode.from_int` と `ExitCode.to_int` が自然な名前になる．
- 採らない理由: `Args`，`Stdin` と並べて `Std.Process` に置き，名前に `exit_code` を入れると決めた前提が崩れる．層の扱いは `[lib.layers]` に規則を 1 つ書けば足りる．

### 値の層の行に `ExitCode` を挙げる

- 採らない理由: 値の層は単独で役に立つ値（Instant，Bytes，Map など）の層で，同じく host に依存しない `Request` と `Response` も挙げていない．終了コードはプロセスの終わり方としてしか意味を持たない．

### `main` の戻り値を `Int` にする

- `fn main() -> Int` の値を終了コードにする．型を足さずに済む．
- 却下の理由: 今 `Int` を返している `main`（最後の式がたまたま `Int`）の終了コードが黙って変わる．`main` の戻り値は捨てると書いてある（`[dyn.error]`）ので，そう書いたプログラムは仕様に沿っている．範囲外の値をどう扱うかも決まらない（JS は 256 で割った余り，wasm の `exit-with-code` は u8）．

### 再開しない effect 操作 `exit(code)`

- platform effect `Process { suspend fn exit(code: Int) -> Never }` と `SystemProcess` を足し，深い呼び出し先から終われるようにする．テストでは handler を差し替える．
- 却下の理由:
  - continuation を再開しない操作は，今の意味では「中断として巻き戻る」（`[dyn.suspend]`）．終了コードを持った第 4 の終わり方を `[fiber.cause]` に足す必要があり，`Exit.attempt`，`Fiber.await`，`Fiber.scope` がそれをどう扱うかを全部決め直すことになる
  - `Fiber.scope` の子から呼んだときに，兄弟を中断してプロセスを終えるのか，scope の結果になるのかが決まらない
  - ライブラリが `uses { Process }` を要求してプロセスを終えられる．`main` が決めるべきことをライブラリが決めてしまう
  - 深い所から終わりたい場合は，error で `main` まで上げて `escape` で `ExitCode` に変えれば書ける（Summary の例）．release も正しく走る

### `main` に届いた error から終了コードを決める

- trait（`trait ExitStatus { fn exit_code(self) -> Int }`）を error の型に impl すると，`main` に届いたときにその終了コードで終わる．`Show` を満たすかで表示を変えている今の仕組み（`[dyn.error]`）と同じ所で判断できる．
- 却下の理由:
  - 終了コードを決める場所が `main` の戻り値と error の型の 2 か所になり，どちらで決まったのかを読み手が追えない．同じ error 型でも `main` まで届くかどうかで終了コードが変わり，混乱を生む（作者の決定）
  - 成功の意味の 0 以外（`grep` の「見つからない」の 1 など）を error として書くことになる
  - `error: <値>` の 1 行は必ず出るので，usage の表示を自分で決められない

### 1，101，130 を予約する

- `exit_code(1)` などを defect にして，終了コードだけで error，defect，中断と区別できるようにする．
- 却下の理由: 3 つは runtime が異常終了に使う番号にすぎない．予約すると `grep` の「見つからない」の 1 のような慣習が書けず，`EXIT_FAILURE` も 1 以外にせざるをえない．区別は標準エラーの 1 行で付く．

### その場でプロセスを止める関数

- `Process.exit(code) -> Never` を runtime の intrinsic にする．
- 却下の理由: with の release も `Fiber.scope` の子の巻き戻しも走らない．`emela test` の中で呼ぶとテストランナーごと止まり，「the test runner exited with code ...」の失敗になる（`[cmd.test.outcome]`）．`@external` で今でも書ける回避策を std が勧めることになる．

## Prior art

- Rust: `fn main() -> ExitCode`（`std::process::ExitCode`）と `Termination` trait．`std::process::exit` は destructor を走らせない．
- Go: `os.Exit` は defer を走らせない．
- Haskell: `exitWith` は `ExitCode` 例外を投げ，`bracket` の後始末が走る．
- WASI 0.3: `wasi:cli/run` は `result<_, _>` を返し，任意のコードは `wasi:cli/exit` の `exit-with-code`（u8）．

## Open questions

なし

## Decision
