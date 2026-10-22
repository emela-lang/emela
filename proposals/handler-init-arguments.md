---
title: handler の init に引数を取らせる
authors: [Alliana]
status: draft
pr: 389
issue: 374
decided:
touches: [effect.handler, effect.with, effect.layer, effect.label, grammar.decl, grammar.expr]
requires: []
related: []
---

# handler の init に引数を取らせる

## Summary

handler の `init` に仮引数を書けるようにし，with と layer の項目，ラベルの `by` の右に `H(x: ...)` と書いて値を渡せるようにする．`[effect.handler]` の「ハンドラ自体は引数を取らない」を改める．

## Motivation

ハンドラが外から値を受け取る道は、今は `init` の中の `use` だけである（`[effect.handler]`）。値を1つ渡すために、その値を返すエフェクトを1つ用意することになる。

- x/platform-cloudflare の `WorkerKv` は、KV のバインディング名を `KV` に固定している。名前を渡す手段がないため。
- HTTP サーバーの Router の試作で、ルートごとのパスの引数（`/users/:id` の `id`）を `PathParams` エフェクトでハンドラに渡すと、`State.run` と `init` の2段を通すことになった。

```emel
handler StateParams(p: Params): PathParams {
  init { StateParams(p: (use State[Params]).get()) }
  fn get(name) { ... }
}

# ルートごとに
(res, _) = State.run(params, fn() { with StateParams { f(req) } })
```

- 認証の結果や相関 ID のように、リクエストごとに決まる値をエフェクトで配る場面は、どれも同じ形になる。
- 接続先の URL のような設定は、`Config` エフェクトを `init` で use すれば今でも渡せる。ただし設定の項目ごとに操作か、ひとまとめの型が要る。

値を与えてハンドラを作り、本体の範囲にだけ供給する手段（局所的な束縛）が、今の仕様にはない。

## Proposal

### Syntax

```emel
pub handler WorkerKv(ns: Namespace): Kv {
  init(binding: String) { WorkerKv(ns: kv_namespace(binding)) }
  ...
}

with WorkerKv(binding: "KV") { ... }

layer AppLive {
  SystemConsole
  WorkerKv(binding: "KV")
}

effect AuditLog of Logger

with SystemConsole, AuditLog by FileLog(path: "audit.log") { ... }
```

- `init` に仮引数の並びを書ける。書き方は fn の仮引数と同じで、型は省略できない。
- with と layer の項目、ラベルの `by` の右（`L by H(...)`、`[effect.label]`）に `H(...)` と書くと、その引数で init を呼ぶ。引数の渡し方は fn の呼び出しと同じ（`[fn.args]`。位置でもラベルでもよい）。
- 引数のない init を持つハンドラは、今までどおり `with H` と書く。`with H()` は書けない。

### Meaning

- 引数は with の外側で評価する。同じ with の項目のハンドラは、引数の評価の時点ではまだ供給されていない。init の依存を同じ with の項目が満たしてよいという規則（`[effect.with]`）は、引数には掛からない。
- 評価の順は with に書いた順で、すべての init より前に評価する。上書きされていないハンドラの引数の E と R は with の E と R に加わる。init の E と同じ扱いである。
- 後ろの項目に上書きされたハンドラの引数は評価せず、その E と R は with の E と R に加わらない。init を実行しないことに揃える。名前解決と型検査は、上書きされていても行う。
  - 上書きの多くは layer の一部の差し替え（`with AppLive, FakeKv`）で、layer の項目の引数は純粋なので（下記）、評価しなくても失われる副作用はない。副作用が失われうるのは、同じ effect のハンドラを with に直接並べた場合だけで、書いた場所から見える。
- 能力の値は引数に渡せない（`[effect.use]`）。能力を捕まえたクロージャは渡せる。
- ハンドラの値を作れるのは init の中だけという規則（E0418）は変えない。with の場所の `H(...)` は init の呼び出しで、値の構成ではない。
- ハンドラは汎用にしない（`[effect.decl]`）。init の仮引数の型は具体的な型に限る。
- layer の項目の引数は、const の右辺と同じく、E と R が空の純粋な式に限る（`[data.const]`）。`use` と `fail` は書けず、関数の呼び出しは書ける。
  - layer の中の順番には意味がなく、初期化の順は requires(H) から決まる（`[effect.layer]`）。引数に E か R があると、その評価の順が決まらない。純粋なら順番は観測できず、どの with から展開されても同じ値になる。
  - layer はトップレベルの宣言なので、引数から見えるのはもともと const とトップレベルの fn だけである。環境から読んだ値を渡すには、layer の後ろに項目を書いて上書きする（`with AppLive, WorkerKv(binding: name) { ... }`）。

### Typing rules

- `with H(args)` は、`H` の init の仮引数の型で args を検査する。数、ラベル、型の誤りは fn の呼び出しと同じ診断にする。
- 供給(H) と依存(H)（`[type.rules]`）は変わらない。引数の R は依存ではなく、with の R に直接加わる。
- `L by H(args)` も同じ。供給と依存は `[effect.label]` の `L by H` のままで、引数は with の項目と同じに扱う。

## Specification changes

### `[effect.handler]`

Replace "The handler itself takes no arguments." with:

> `init` MAY take parameters (`init(url: String) { ... }`). Their types MUST be written. Arguments are passed by writing `H(url: ...)` as an item of a with or a layer, or on the right side of `by` (`[effect.with]`, `[effect.layer]`, `[effect.label]`). A handler whose init takes no parameters cannot be given arguments, and `H()` cannot be written for it.
>
> `H(...)` at a with is a call of init, not a construction of the handler value. As before, the handler value can be constructed only inside init (E0418).

### `[effect.with]`

Add after the item that says what H can be:

> - An item can be written as `H(arguments)`. The arguments are evaluated outside the with, in the order the items are written, before any init runs. The effects provided by the items of the same with cannot be used in the arguments. The E and R of the arguments of a handler that is not overridden are added to the E and R of the with. The arguments of a handler overridden by a later item are not evaluated, and their E and R are not included in the E and R of the with; they are still resolved and type-checked.

### `[effect.layer]`

> - An item of a layer can be written as `H(arguments)`. The arguments are limited to pure expressions (E and R are empty), as for the right-hand side of a const (`[data.const]`): `use` and `fail` cannot be written, and function calls can. The order of the items has no meaning, so arguments with E or R would have no defined order of evaluation. To pass a value that is not pure, list the handler after the layer in the with (`with AppLive, WorkerKv(binding: name) { ... }`).

### `[effect.label]`

> - The right side of `by` can be written as `H(arguments)`. The arguments are treated as for an item of a with.

### `[grammar.decl]`

```
handler_item = "init" [ "(" [ params ] ")" ] block
             | "release" block
             | [ "suspend" ] "fn" lower_name "(" [ lower_name { "," lower_name } ] ")" block

layer_decl   = "layer" type_name "{" supply { sep supply } "}"
supply       = type_ref [ "(" [ args ] ")" ]
```

### `[grammar.expr]`

```
with_item  = supply [ "by" by_right ]
by_right   = type_ref [ "[" type { "," type } "]" | "(" [ args ] ")" ]
```

The type arguments on the right of `by` (`Hits by State[Int]`) can be written only when it names an effect, and arguments only when it names a handler.

## Diagnostics

- [ ] 仮引数を取る init のハンドラを、引数なしで書いた（未定）
- [ ] 仮引数を取らないハンドラに引数を書いた。`with H()` を含む（未定）
- [ ] layer の項目の引数の E か R が空でない: const の右辺と同じく E0318（失敗）と E0402（R）を使う。help で、layer の後ろに項目を書いて上書きすることを示す
- [ ] 引数の数、ラベル、型の誤りは、E0306、E0307 などの呼び出しの診断を使う

## Compatibility

足す方向の変更である。今通るプログラムは、意味を変えずに通る。

## Implementation

- emela-syntax: init の仮引数、with と layer と `by` の項目の引数
- emela-resolve: 項目の引数の名前解決
- emela-types: init の仮引数での検査、引数の E と R を with に足す
- emela-core: lowering で、引数を評価してから init に渡す
- x/platform-cloudflare: `WorkerKv` を `WorkerKv(binding:)` にする

## Alternatives

- `with H.init(url: ...)`: init を関数の値のように見せるが、init は値として取り出せない。
- `with H { url: ... }`: 本体の block と見分けにくい。
- `handler H(url: String)` のように、ハンドラの頭に引数を書く: 頭の括弧はフィールドの宣言に使っている。
- layer に仮引数を持たせる（`layer AppLive(binding: String) { WorkerKv(binding: binding) }`）: 仮引数のスコープと評価の時点を新しく決めることになる。layer の後ろに項目を書けば上書きできるので、今は要らない。必要になれば別の proposal にする。
- 上書きされたハンドラの引数も評価する: 使われない引数の E と R が with に加わり、init を実行しないことと食い違う。
- ハンドラを第一級の値にして渡す: E0418 と、能力の値を持ち出さない規則（`[effect.escape]`）を崩す。

## Prior art

- 値からサービスを作り、計算の一部にだけ供給する操作（依存の局所的な注入）。Effect-TS の `Effect.provideService`、ZIO の `ZLayer.succeed` がこれに当たる。
- パラメータ化されたハンドラ（parameterised handler）。ハンドラが引数を受け取り、操作の解釈に使う。Koka、Eff、Effekt が持つ。

## Open questions


## Decision
