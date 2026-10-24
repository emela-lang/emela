---
title: Exported functions and entries
authors: [Alliana]
status: draft
pr: 427
issue:
decided:
touches: [glossary.terms, kw.annotation, grammar.decl, ext, target.js.module, target.js.start, target.wasm.external, target.c, effect.with, module.pome, cmd.build, diag.list]
requires: []
related: []
---

# Exported functions and entries

## 要約

ホストが Emela の関数を呼ぶ口を `@export` で宣言できるようにする．`@external` の逆向きで，本体のある fn に付け，ターゲットごとにホストに見せる名前を書く．引数と返り値は境界型だけ．

最初の引数に関数を取る `@export` fn は入口アダプタ（entry adapter）になり，それ自体は export されない．エントリのモジュールの fn に `@entry(入口アダプタ)` を付けると，その fn を最初の引数として入口アダプタに渡す export が生まれる．日常の水準の入口（HTTP サービスなど）は，土台の水準で書いた入口アダプタを `@entry` で指して作る．

## 動機

- 今の入口は `main`（コマンド）と `@test`（テスト）だけ．Workers の `fetch` と `scheduled`，`wasi:http` の `handle`，組み込みの割り込みハンドラのように，ホストが呼ぶ入口を増やしたい
- JS ではエントリの pub fn が暗黙に export される（`[target.js.module]`）が，ホストが求める名前（default export の `fetch` など）を選べない．C と WASM には `main` 以外の口が無い
- 入口の多くは境界型より豊かな型（`Std.Http.Request` など）を使いたい．境界型との変換をアプリを書く人に書かせると，ホストの名前と境界型が日常の水準に出てくる（`[intro.layers]` の「設計の欠陥」）．`x/platform-cloudflare` は今この形
- 変換をコンパイラに入口の種類ごとに組み込むと，入口を増やすたびにコンパイラを変えることになる．変換は Emela で書き，コンパイラは「入口アダプタに関数を渡して export を作る」ことだけを知る形にしたい
- 入口を effect（ホストが操作を呼び，プログラムが handler で答える）で表す案もあるが，日常の水準で handler を書かせることになる

## 提案

### `@export`

```emel
# C: 割り込みハンドラ
@export(c, "TIM2_IRQHandler")
pub fn on_tick() -> () { ... }

# JS: 名前付きの export と，default export の property
@export(js, "add")
@export(js, "default", "add")
pub fn add(a: Int, b: Int) -> Int { a + b }
```

- 本体のある top level の fn に付ける．引数の形は `@external` と同じ（`(target, "name")` か `(target, "interface", "name")`）
- 引数と返り値は境界型（`[ext.boundary]`）だけ．E は空，R は Async を除いて空
- ホストからの1回の呼び出しが1回の起動になる
- 置けるのはエントリのモジュールだけ（入口アダプタは除く．下）
- JS: `(js, "name")` は `main.mjs` の名前付きの export，`(js, "default", "name")` は default export の object の property．3引数の形は `@external(js, "file", "name")` と同じく，真ん中が入れ物，最後が名前．JS で真ん中に書けるのは `"default"` だけ．JS の export の名前には `.` を含む任意の文字列が使えるので，`"default.fetch"` のように1つの文字列で書く形は取らない．suspend しうる fn は Promise を返す関数として export される（`@external` の suspend fn が Promise を返す JS の関数であることの逆向き）．defect は Promise の reject（suspend しない fn では例外）になる
- C: 外部リンケージを持つ関数として `main.c` に出す．プロトタイプは `[ext.boundary]` の表で決まる．R に Async があれば compile error（C で suspend する export の形は決めていない）
- C: 起動が走っている最中に同じスレッドから export を呼ぶと defect．Emela から呼んだ C の関数の中からの呼び返しも，割り込みハンドラやシグナルハンドラからの呼び出しもこれに当たる．ランタイムは確保や fiber の切り替えの途中に割り込まれると壊れるため．ランタイムが「起動中」の印を持ち，export の入口で確かめる
- C: 別のスレッドからの呼び出しは規定しない（並列の実行は `[open.list]` #13 で未決）
- C: `main` が動いていない所から呼ばれたら，最初の呼び出しでランタイムを初期化する．初期化の関数はホストに見せない
- 割り込みは，C の割り込みハンドラで印を立てるか待ち行列に積み，Emela 側がそれを `@external` で読む．JS で「コールバックからは Deferred で fiber に知らせる」としているのと同じ形
- WASM: ユーザーの `@export(wasm, ...)` は未対応で，`@external(wasm, ...)` と同じ扱いの error（#56 と一緒に決める）

### 入口アダプタと `@entry`

```emel
# 土台の水準（std や Pome）
@export(js, "default", "fetch")
@export(wasm, "wasi:http/handler@0.3.0", "handle")
pub fn serve[E](app: fn(Request) -> Response fails E use { Async }, incoming: Incoming) -> Outgoing use { Async } {
  ...
}

# 日常の水準（エントリのモジュール）
@entry(Http.serve)
pub fn app(request: Request) -> Response { ... }
```

- 最初の引数の型が関数型の `@export` fn を入口アダプタ（entry adapter）と呼ぶ．入口アダプタはそれ自体は export されず，どのモジュールにも置ける．依存先の Pome の入口アダプタを `@entry` で指すには，その Pome を `[external] allow` に書く（`@external` と同じ扱い）．最初の引数は境界型でなくてよく，残りの引数と返り値は境界型
- `@entry(path)` はエントリのモジュールの top level の fn に付ける．path は入口アダプタを指す（`Http.serve`，`Cloudflare.Worker.scheduled`）
- `@entry(T)` が付いた fn `f` について，T の `@export` ごとに，`fn(rest...) -> R { T(f, rest...) }` を同じ名前で export したものとして扱う．`f` は T の最初の引数に渡せる型でなければならない
- `f` 自体は普通の fn のまま．テストから直接呼べる
- 同じターゲットで同じホストの名前に export が2つ以上あれば compile error

## Specification changes

### `[glossary.terms]`

Add a row:

> | entry adapter | 入口アダプタ |

### `[kw.annotation]`

Add two rows to the table of attributes:

> | `@export(...)` | Top-level fn with a body | Exposes the fn to the host under a name, for each target (`[ext.export]`). An `@export` fn whose first parameter has a function type is an entry adapter and is not exposed by itself |
> | `@entry(...)` | Top-level fn of the entry module | Exposes the fn through an entry adapter (`[ext.entry]`) |

### `[grammar.decl]`

```ebnf
annot_arg    = lower_name | literal | lower_name ":" literal | qualified ;
qualified    = type_name { "." type_name } "." lower_name ;
```

`qualified` is allowed only as the argument of `@entry`.

### `[ext.export]` (new, after `[ext.c-host]`)

> ## 12.7 Exported functions `[ext.export]`
>
> ```emel
> @export(c, "TIM2_IRQHandler")
> pub fn on_tick() -> () { ... }
>
> @export(js, "default", "add")
> pub fn add(a: Int, b: Int) -> Int { a + b }
> ```
>
> - `@export` is written on a top-level fn with a body. Its arguments have the form `(target, "name")` or `(target, "interface", "name")`, as for `@external` (12.1). One `@export` is written per target name; a fn MAY have several for the same target. Malformed arguments are E0146.
> - An `@export` fn exposes itself to the host under each name. Each call by the host is one launch, as main is.
> - The parameters and the return value of an `@export` fn are boundary types (12.4) (E0434). Its E MUST be empty and its R MUST be empty except for Async (E0402).
> - An `@export` fn that is not an entry adapter is allowed only in the entry module (E0435).
> - Two exports with the same name for the same target, from `@export` or `@entry`, are a compile error (E0436).
> - How each target exposes the fn is in `[target.js.export]`, `[target.c.export]` and `[target.wasm.external]`.

### `[ext.entry]` (new)

> ## 12.8 Entry adapters `[ext.entry]`
>
> - An `@export` fn whose first parameter has a function type is an entry adapter. An entry adapter is not exposed by itself, and MAY be in any module. Its first parameter need not be a boundary type; the other parameters and the return value are boundary types (E0434).
> - `@entry(T)` is written on a top-level fn `f` of the entry module. `T` names an entry adapter (E0437). `f` MUST be accepted as the first argument of `T` (E0438).
> - For each `@export` of `T`, the program exposes under the same name a fn that takes the remaining parameters of `T` and returns `T(f, ...)`. Each call is one launch.
> - `f` remains an ordinary fn. It can be called, including from tests.

### `[target.js.module]`

Replace the items "The `pub` fns of the entry module and `main` are exported ...", "A `pub` fn with type parameters is exported ...", "A module that exports at least one suspending fn also exports `$start` ..." and "How to call an exported fn whose R has effects other than Async is unspecified" with:

> - Only the fns with `@export(js, ...)`, and the exports made by `@entry`, are exports of `main.mjs` for the host. A `pub` fn without `@export` is not exported for the host; `pub` means only that other modules can see it (`[module.pub]`).
> - `main.mjs` also has exports that the runtime's launcher and `serve.mjs` use (`main`, `$start`). Their names and forms are not specified for other uses.
> - An `@export(js, "name")` exposes the fn as the named export `name` of `main.mjs`. `@export(js, "default", "name")` makes it the property `name` of the default export of `main.mjs`, an object. On the `js` target, the middle argument of the three-argument form MUST be `"default"` (E0146).
> - An exported fn that may suspend is a function that returns a Promise. The Promise resolves to the return value, and rejects with the defect if the launch ends with a defect or an interruption. A fn that does not suspend returns its value and throws on a defect.

### `[target.js.start]`

Replace "When the host calls an exported suspending fn, it returns a generator" with "When the launcher calls a suspending `main`, it returns a generator". The section describes how the runtime's launcher and `serve.mjs` start `main`; an `@export` fn returns a Promise instead (`[target.js.module]`).

### `[cmd.build]`

Replace "The `pub` fns of the entry are exported from `main.mjs`. A fn with a type parameter that has a bound cannot be exported (E0327), because which impl to use is not determined. Make a fn that wraps it with concrete types `pub`." with "The fns with `@export(js, ...)` and the exports made by `@entry` are exported from `main.mjs` (`[target.js.module]`)."

### `[target.c]`

Add a section `[target.c.export]`:

> - An `@export(c, "name")` is emitted in `main.c` as a function with external linkage named `name`, with the prototype given by the table of 12.4.
> - An exported fn with Async in its R is a compile error on the `c` target (E0439).
> - Calling an exported fn while a launch is running on the same thread is a defect, with the message ``exported function `f` was called while Emela code is running``. This covers a call back from a host function that Emela called, and a call from an interrupt or signal handler. The runtime checks it on entry to the export.
> - Calling an exported fn from another thread is unspecified.
> - When an exported fn is called while no launch has started, the runtime is initialized by that call. The host does not call an initialization function.

### `[target.wasm.external]`

Add: "An `@export(wasm, ...)` written by a user is a compile error with no code: `` exported function `f` has a `wasm` export, which the WASI 0.3 component target does not support yet ``. The entry adapters of the bundled `Std` are exposed by the runtime."

### `[module.pome]`

In "Pomes that touch the host": `@export` counts as `@external` for `[external] allow`. An `@entry` that names an entry adapter of a Pome that is not allowed is E0233.

### `[effect.with]`

After the item on the R of main: "The same holds for an `@export` fn (`[ext.export]`)."

### `[diag.list]`

> | E0146 | Malformed arguments of `@export` (12.7) | ``expected `@export(target, "name")` `` |
> | E0434 | A parameter or the return value of an `@export` fn is not a boundary type (12.7, 12.8) | ``exported function `f` cannot take `List[Int]` `` |
> | E0435 | An `@export` fn that is not an entry adapter outside the entry module (12.7) | ```@export` is only allowed in the entry module`` |
> | E0436 | Two exports with the same name for the same target (12.7) | ```fetch` is exported twice for target `js` `` |
> | E0437 | The argument of `@entry` is not an entry adapter (12.8) | ```Http.get` is not an entry adapter`` |
> | E0438 | The fn cannot be passed as the first argument of the entry adapter (12.8) | ``app` does not fit `Http.serve` `` |
> | E0439 | An exported fn that may suspend on the `c` target (16.7) | ``exported function `f` may suspend, which target `c` does not support`` |

E0402: add "an `@export` fn" to the list of contexts.

E0327: mark as unused ("(Unused, because only `@export` fns are exported, and they take only boundary types)"), as E0428 is.

## 診断

- [ ] E0146: `@export` の引数の形
- [ ] E0434: 境界型でない引数か返り値
- [ ] E0435: エントリのモジュールの外の `@export`
- [ ] E0436: 同じ名前の export が2つ
- [ ] E0437: `@entry` の引数が入口アダプタでない
- [ ] E0438: fn が入口アダプタの最初の引数に合わない
- [ ] E0439: C で suspend しうる export
- [ ] E0402: `@export` fn の R
- [ ] ユーザーの `@export(wasm, ...)`（コード無し）

## 互換性

JS の暗黙の export（エントリの pub fn をすべて export すること）をやめる．JS からエントリの pub fn を呼んでいたコードは壊れる．`@export(js, "name")` を付ければ直る．ただし引数と返り値が境界型でなければ，境界型で包む fn を書く必要がある．

`x/platform-cloudflare` の `pub fn fetch` と `worker/entry.mjs` はこの形に頼っているが，`@entry(Http.serve)`（下書き http-service）に移るので書き直す．

E0327（制約付きの型引数を持つ fn の export）は使われなくなる．`@export` は境界型だけを取るので，型引数を持つ fn は E0434 になる．

## 実装

- emela-syntax: `@export`，`@entry`，`qualified` の引数
- emela-types: 境界型，E と R，入口アダプタと `@entry` の照合
- emela-codegen-js: 名前付き export と default export の object，suspend する export を Promise にする包み
- emela-codegen-c: 外部リンケージの関数
- emela-codegen-wasm: std の入口アダプタだけを扱う（http-service）

## 採らなかった案

- Emela で割り込みハンドラそのものを書けるようにする: 割り込みがメモリの確保や fiber の切り替えの途中に入るとランタイムが壊れる．確保も suspend もしない部分言語を別に定める必要があり，この下書きの範囲を超える
- C の出力を，`main` を持たないライブラリ（`.a` や `.o`）にする形: 別の下書きで扱う

- JS の暗黙の export を残す: export の作法が2通り（ジェネレータと `$start`，Promise）になり，JS 側が export ごとに覚えることになる．内部表現の値（List や record）をそのまま JS に見せることになり，表現を変えると JS 側が壊れる．`pub` が「エントリだけはホストにも見える」を兼ねる．export する fn の R を Async だけにする規則（下書き exported-fn-effects）は `@export` の規則に入るので，その下書きは要らなくなる

- 入口ごとに名前で示す（`handle`）: 入口の種類が増えるたびに予約される名前が増える．1つのプログラムに同じ種類を置けない形にもなりやすい
- `main` の引数の型で種類を決める（`main(request: Request)`）: 入口が1つしか置けない．Workers の `fetch` と `scheduled` のように並べられない
- 入口を値にする（`pub const FETCH = Http.service(app)` と，`@entry` を付けた型）: export するのが fn でないので，`@export` とは別の仕組みが要る
- 属性の名前を入口アダプタのパスにする（`@Http.serve`）: ユーザーが属性を定義できることになり，`[kw.annotation]` の「属性はコンパイラに組み込まれたものだけ」を手放す
- 入口を effect で表す（ホストが操作を呼び，プログラムが handler で答える）: 日常の水準で handler と `resume` を書かせることになる

## 先行する設計

- `@external`（12.1）．`@export` はその逆向き
- `@test`．コンパイラが知る属性で入口を示す形

## 未決


## 決定
