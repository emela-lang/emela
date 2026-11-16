---
title: Host effects, what the host provides to a launch
authors: [Alliana]
status: draft
pr: 617
issue:
decided:
touches: [effect.decl, effect.async, effect.host (new), effect.with, ext.call, ext.export, ext.entry, test.constraints, lib.layers, lib.http-service, api.std.platform, api.std.process, api.std.env, api.std.file-system, diag.list]
requires: []
related: []
---

# Host effects, what the host provides to a launch

## Summary

handler が host にしかない effect を host effect と呼び，`Async` をその 1 つと位置づける．host effect に `ProcessHost`（host が process である）と `PageHost`（host が browser の page である）を足す．host 関数が process でしか動かない external function は `uses { ProcessHost }` を宣言し，その要求は `Async` と同じ推論で handler と layer を通って R に残る．どの host effect を用意するかは entry が決めるので，process 専用の handler を page で使うといつもの E0402 になる．`Platform` はどの host にもある handler だけを束ねる layer にし，process 専用の handler まで束ねる layer `Process` を `Std.Process` に足す．あわせて，operation のない effect を宣言できるのを compiler だけにし，external function の `uses` に書けるものを `{}` と host effect に限る．

説明用の喩え（spec には書かない）：entry は program をどの台木（rootstock）に接ぐかを決める．台木によって育つ環境が決まり，木の側からは取り替えられない．

## Motivation

### process 専用の handler が，process でない host で実行時に壊れる

- `Platform` は `SystemFileSystem` や `SystemArgs` を含む．Node，Bun，wasmtime ではどれも動くが，browser で動く program（`@entry(Browser.app)`，後に続く proposal で提案する）では動かない
- 今は実行時になって初めて分かる（`node:fs` の解決に失敗するか，defect になる）

```emel
# browser で動く program が，気づかずに process 専用の handler に届く例
@entry(Browser.app)
pub fn app() -> Program[Model, Msg] {
  # 今は compile が通り，実行時に失敗する（Platform は SystemFileSystem を含む）
  with Platform {
    saved = (use FileSystem).read_text("model.json")
    Browser.program(init: decode_model(saved), update:, view:)
  }
}
```

- target ごとの Platform（`NodePlatform`，`WasmPlatform`）に分けると，同じ program が target によって違う結果を返しうる．これは `[target]` の「the same program MUST give the same observable results on every target」に反する．`[lib.layers]` も「A platform handler does not expose target names」と決めている
- 区別したいのは target ではなく，host が何を提供するか．これは実装の差ではなく使える機能の差なので，型に出してよい

### operation のない effect の扱いが決まっていない

- `Async` は operation を持たず，runtime だけが用意する（`[effect.async]`）．しかし spec はこれを `Async` 個別の規則として書いていて，一般の概念になっていない
- grammar は書き手が `effect Marker {}` と宣言することを許すが，意味は書かれていない
- external function の `uses` について，`[ext.call]` の表には `uses {}` と省略の 2 つしかない．しかし今の実装は `uses { Marker }`（書き手の effect）を受け付け，その external function を handler の外から呼べてしまう（2026-11-16，`emela run` で確認）．`uses` を省くと E0407 になるのに，何か 1 つ書くだけでこの制限が外れる

```emel
effect Marker {}

handler GiveMarker: Marker {}

@external(js, "./x.mjs", "write")
fn write_host(text: String) -> () uses { Marker }

fn main() {
  # 通り，host の副作用が handler を通らずに起きる
  with GiveMarker { write_host("called outside a handler") }
}
```

## Proposal

### Syntax

構文は変えない．`ProcessHost` と `PageHost` を名前として core に足す．

### Meaning

- host effect は，handler が host にしかない effect．`Async`，`ProcessHost`，`PageHost` の 3 つ
  - `Async`：host は後で resume できる（event loop を持つ）
  - `ProcessHost`：host は process である．引数，標準入出力，環境変数，ファイルシステムを持つ
  - `PageHost`：host は browser の page である
- 書き手は host effect の handler を書けず（E0413），`use` もできない（E0414）．uses clause には書ける
- host effect が R に入るのは，host 関数がそれを必要とするときだけ
  - `@external suspend fn` を呼ぶと `Async` が入る（今と同じ）
  - `uses { ProcessHost }` を宣言した external function を呼ぶと `ProcessHost` が入る
  - handler の operation の本体でそれらを呼ぶと，requires(H)（`[effect.handler]`）に入る．そのため `with SystemFileSystem { ... }` や `with Process { ... }` の R に `ProcessHost` が残る
- host effect を用意するのは entry だけ．with では取り除けない

| entry | host が用意する host effect |
|---|---|
| `main` | `Async`，`ProcessHost` |
| `@test` | `Async`，`ProcessHost` |
| `@export` fn（entry adapter でないもの） | `Async` |
| entry adapter | `Async` と，adapter が宣言した host effect（多くても 1 つ） |
| `Http.serve`（entry adapter） | `Async`，`ProcessHost` |

- `@export` fn には `ProcessHost` も `PageHost` も用意しない．host から直接呼ばれる fn はどの host でも動くことが型で分かる
- `Http.serve` は `ProcessHost` を用意する．HTTP service は process として動く前提で，page で動くことはない．HTTP service の中では `with Process` も書ける
- 書き手は operation のない effect を宣言できない．operation のない effect は handler が何もできず，空の handler（`handler H: Marker {}`）を書けば誰でも「用意」できてしまう．型に出ているのに何も保証しないので，健全ではない．operation のない effect は host effect だけになる
- external function の `uses` に書けるのは `{}` か host effect だけ．`uses { ProcessHost }` の external function は，`uses` を省いたものと同じく handler の中でしか呼べない．`Async` は `suspend fn` で表すので，`uses` には書かない

### std の形

```emel
@external(js, "./file_system.mjs", "readText")
suspend fn read_text_host(path: String) -> String uses { ProcessHost }

pub handler SystemFileSystem: FileSystem {
  suspend fn read_text(path, resume) { resume(read_text_host(path)) }
}
```

- `ProcessHost` を必要とするのは `SystemArgs`，`SystemStdin`，`SystemStdout`，`SystemStderr`，`SystemEnv`，`SystemFileSystem` の external function．`SystemConsole`，`SystemClock`，`SystemRandom`，`SystemHttp` は必要としない
- `Platform` はどの host にもある handler（`SystemConsole`，`SystemClock`，`SystemRandom`，`SystemHttp`）だけを束ねる．host effect は `Async` しか要求しないので，`main`，`@test`，`@export`，page のどこでも書ける
- `Std.Process` に layer `Process` を足す．`Platform` と，process 専用の handler（`SystemArgs`，`SystemStdin`，`SystemStdout`，`SystemStderr`，`SystemEnv`，`SystemFileSystem`）を束ね，`ProcessHost` を要求する

```emel
import Std.Process.{Process}
import Std.FileSystem.{FileSystem}

fn main() {
  with Process {
    (use FileSystem).write_text("out.txt", "hi")
  }
}
```

- 引数やファイルを使わない program は `with Platform` のままでいい．使う program は `with Process` に書き換える

### 書き手から見た型

```emel
fn save(path: String, text: String) -> () uses { FileSystem } {
  (use FileSystem).write_text(path, text)
}
```

`save` の型に `ProcessHost` は付かない．host effect が R に残るのは handler を選ぶ場所（`with`）だけなので，`FileSystem` を使う library は page でもテストでも，handler を差し替えれば使える．

### Typing rules

- host effect は Immediate（`[type.effect-bound]`）．ただし `Async` は除く（今と同じ）
- uses clause を書いた fn は，その集合をそのまま R にする（`[type.inference]`）．`fn f() uses { ProcessHost }` と書けば，その fn は process の中でしか呼べない

## Specification changes

### `[effect.host]` (new)

Insert before `[effect.async]`. `[effect.async]` keeps its number after it.

> ## 8.7 Host effects `[effect.host]`
>
> - A host effect is an effect whose only handler is the host. `Async` (`[effect.async]`), `ProcessHost` and `PageHost` are the host effects.
>   - `Async`: the host can resume a computation later.
>   - `ProcessHost`: the host is a process of the operating system, with arguments, standard input and output, environment variables and a file system.
>   - `PageHost`: the host is a page of a web browser.
> - Users cannot write a handler for a host effect. `handler H: ProcessHost` is a compile error (E0413), and that handler provides nothing even when written in a with. `use` of a host effect is a compile error (E0414). A host effect can be written in a uses clause (`fn f() uses { ProcessHost }`).
> - A host effect enters R only through what host functions need. Calling an `@external suspend fn` adds Async (`[effect.async]`). Calling an external function declared with `uses { ProcessHost }` or `uses { PageHost }` adds that host effect (`[ext.call]`). A handler whose init, release or operation bodies make such calls requires the host effect (requires(H) in `[effect.handler]`), so the host effect remains in the R of a with that lists the handler, or a layer that contains it.
> - A with does not remove a host effect. Only the entry of a launch provides host effects.
>
> | Entry | Host effects provided |
> |---|---|
> | `main` (`[effect.with]`) | Async, ProcessHost |
> | A test function (`[test.constraints]`) | Async, ProcessHost |
> | An `@export` fn that is not an entry adapter (`[ext.export]`) | Async |
> | An entry adapter (`[ext.entry]`) | Async, and the host effect the adapter declares |
>
> - `ProcessHost` and `PageHost` are Immediate (`[type.effect-bound]`).
> - Host effects do not name targets. A program whose entry type checks runs on every target that supports that entry (`[target]`).

### `[effect.decl]`

Add:

> - An effect declared in source MUST have at least one operation (E0xxx). The effects without operations are the host effects, which only the compiler declares (`[effect.host]`).

### `[effect.async]`

Replace the first two rules with:

> - Async is a host effect (`[effect.host]`) that represents "being resumed later". The rules of `[effect.host]` apply: only the host provides it, `handler H: Async` is a compile error (E0413), and `use Async` is a compile error (E0414).

The other rules of the section are unchanged.

### `[effect.with]`

Replace:

> The R of main MUST be empty except for Async.

with:

> The R of main MUST be empty except for the host effects that main is given (Async and ProcessHost, `[effect.host]`).

### `[ext.call]`

Replace the table with:

> | Declaration | Meaning | Where it can be called |
> |---|---|---|
> | `uses {}` written explicitly | Declares that the function is pure | Anywhere |
> | `uses` omitted | Has host side effects | Only inside the init, the release and the operation bodies of a handler |
> | `uses { ProcessHost }` or `uses { PageHost }` | Has host side effects, and works only on that host (`[effect.host]`) | Only inside the init, the release and the operation bodies of a handler |
> | `suspend fn` | Needs to wait for completion. Requires Async | Only inside a handler |

Add:

> - The uses clause of an external function contains only ProcessHost and PageHost, or nothing. Any other effect, including Async and a set type parameter, is a compile error (E0xxx). Suspension is declared with `suspend fn`, not with Async.
> - The host effects in the uses clause are added to the R of each call.

### `[ext.export]`

Replace "Its E MUST be empty and its R MUST be empty except for Async (E0402)" with:

> - Its E MUST be empty and its R MUST be empty except for Async (E0402). An exported fn is called by a host of a kind the program does not know, so it is given no other host effect (`[effect.host]`).

### `[ext.entry]`

Add:

> - The function type of the first parameter of an entry adapter MAY have one host effect other than Async in its uses (`[effect.host]`), and the R of the adapter MAY contain it. The host provides it for each launch through the adapter.

### `[test.constraints]`

Replace "The R of a test function MUST be empty except for Async. The test runner provides Async." with:

> - The R of a test function MUST be empty except for Async and ProcessHost. The test runner provides both, because tests run in a process (`[effect.host]`).

### `[lib.layers]`

Add:

> - The external functions of `SystemArgs`, `SystemStdin`, `SystemStdout`, `SystemStderr`, `SystemEnv` and `SystemFileSystem` are declared with `uses { ProcessHost }`, so these handlers require ProcessHost (`[effect.host]`). `SystemConsole`, `SystemClock`, `SystemRandom` and `SystemHttp` require no host effect other than Async.

Replace "The layer `Platform`, which bundles all System handlers, is placed in `Std.Platform`. main is usually written `with Platform { ... }`" with:

> - The layer `Platform`, placed in `Std.Platform`, bundles the System handlers that every host has: SystemConsole, SystemClock, SystemRandom and SystemHttp. It requires no host effect other than Async, so it can be written in main, a test, an exported fn and a page. The layer `Process`, placed in `Std.Process`, bundles Platform and the System handlers that need a process: SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemEnv and SystemFileSystem. It requires ProcessHost. main is usually written `with Platform { ... }`, or `with Process { ... }` when it uses arguments, the standard streams, environment variables or files.

### `[lib.http-service]`

Add:

> - `Http.serve` provides ProcessHost (`[effect.host]`). An HTTP service runs in a process, so `with Process { ... }` can be written in the fn.

### `[api.std.platform]`

Replace the row of `Platform` with:

> | `Platform` | `layer Platform { SystemConsole, SystemClock, SystemRandom, SystemHttp }` | The System handlers that every host has. Requires no host effect other than Async. To replace some of them, list the replacements after it, as in `with Platform, TestClock { ... }` |

Replace "SystemStdin, SystemClock, SystemHttp, and SystemFileSystem require Async" with "SystemClock and SystemHttp require Async".

### `[api.std.process]`

Add a row:

> | `Process` | `layer Process { Platform, SystemArgs, SystemStdin, SystemStdout, SystemStderr, SystemEnv, SystemFileSystem }` | Platform and the System handlers that need a process. Requires Async and ProcessHost (`[effect.host]`) |

Replace `with Platform` in the example of the section with `with Process`, and its import with `import Std.Process.{Process, Stdout}`.

### `[api.std.process]`, `[api.std.env]`, `[api.std.file-system]`

Each section adds one line: "The System handler requires `ProcessHost` (`[effect.host]`)."

### `[api.std.console]`

Add:

> - On a page host, SystemConsole writes with `console.log` and `console.error`, as on any JS host without standard output.

### `[diag.list]`

- E0413: ``handler `H` cannot implement `ProcessHost`: only the host provides it`` (the message for Async becomes the same form).
- E0414: `` `ProcessHost` is a host effect and cannot be used with `use` `` (the message for Async becomes the same form).
- E0402: when the missing effect is a host effect, add a note that names the entry and what it provides (``= note: `Browser.app` provides `Async` and `PageHost`, not `ProcessHost` ``). When the missing effect is one that `Process` provides and the with lists `Platform`, add ``= help: `Platform` has only the handlers every host has; use `Process` for the ones that need a process``.
- E0xxx (new): ``the uses of external function `f` can contain only `ProcessHost` and `PageHost` ``, at the effect in the uses clause.
- E0xxx (new): ``effect `E` has no operations``, at the name of the effect. Help: ``an effect needs at least one operation; only host effects have none``.

## Diagnostics

- [ ] E0413，E0414 の文言を host effect の一般形にする
- [ ] E0402 に host effect 用の note を足す
- [ ] external function の uses に host effect 以外を書いたときの code（番号未定）
- [ ] operation のない effect を宣言したときの code（番号未定）

## Compatibility

- `main` と `@test` の中の `with Platform { ... }` は，`Console`，`Clock`，`Random`，`Http` だけを使う program なら今まで通り通る
- `Platform` 経由で `Args`，`Stdin`，`Stdout`，`Stderr`，`Env`，`FileSystem` を使っている program は E0402 になり，`with Process` への書き換えが要る．repo の中で `with Platform` を書いているファイルは 21 本（2026-11-17）．E0402 の help で `with Process` を示す
- `@export` fn の中で `with Platform` や process 専用の handler を使っている program は compile が通らなくなる
- external function に `uses { 書き手の effect }` を書いている program は compile が通らなくなる．今は spec の表にない書き方
- `@entry(Http.serve)` の fn の中の `with Platform` は今まで通り通る．process 専用の handler を使うなら `with Process` に書き換える
- operation のない effect を宣言している program は compile が通らなくなる．repo の中には見つからなかった（2026-11-16）

## Implementation

- `emela-types`：host effect の扱い（`Async` 個別の判定を一般化する），entry ごとに用意する host effect，external function の uses の検査
- `emela-core`：process 専用の external function に `uses { ProcessHost }` を足す．`Platform` を組み直し，`Std.Process` に `Process` を足す．`with Platform` で process 専用の handler を使っている example と test を `with Process` に書き換える
- `emela-lsp`：E0402 の note

## Alternatives

- target ごとの Platform（`NodePlatform`，`BunPlatform`，`WasmPlatform`）：同じ program が target によって違う結果を返しうる．`[target]` に反する
- browser を別 target にして E0412 で止める：target が増え，出力と runtime がほぼ同じものを 2 つ持つことになる．E0412 は `emela check` と LSP では走らない（`[ext.decl]`）ので，エディタでも気づけない
- Pome 単位で動く host を宣言する：粗すぎる．`main` の 1 か所の `with Platform` で package 全体が process 専用になる
- 関数型に `needs { }` のような別の clause を足す：closure の型も推論も二重になる．R はすでに「呼ぶ側が用意するもの」の集合で，host が提供するものもその一つ
- `ProcessHost` の `use` を許し，handler の init で `use ProcessHost` と書く：今の言語でも書き手の effect で同じことができる．しかし operation のない effect を `use` するのは effect の使い方として不自然で，`Async` の E0414 とも食い違う．どの host 関数が process を必要とするかは external function の性質なので，そこに書く
- process 用の layer の名前に `ProcessPlatform`，`System`，`Os` を使う：`ProcessPlatform` は長く，`Platform` の亜種に見える．`System` は handler の接頭辞と紛らわしい．`Os` は何を束ねているかが伝わらない
- `Platform` を全部入りのまま残し，どの host にもある handler の layer を別に足す：既存の program は壊れないが，`Platform` が process 専用になり，`@export` や page で使えない名前になる
- 名前に runtime effect，marker effect，context を使う：runtime は runtime library と紛らわしい．marker は形しか表さない．context は `Context[T]`（下書き `context-and-given`）と glossary の「non-suspending context」で使われている

## Prior art

- 環境が提供する能力を，effect の集合の中で組み込みの effect として表す設計（Koka の `div` や `ndet` など，書き手が handler を持たない組み込み effect）
- 計算が文脈に要求するものを型で追う coeffect

## Open questions

## Decision
