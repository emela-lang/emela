---
title: "The R of a closure is provided only by its caller"
authors: [Alliana]
status: draft
pr:
issue: 719
decided:
touches: [effect.use, effect.with, type.rules, fiber.scope, grammar.lex, ext.boundary, diag.list]
requires: [effect-alias]
related: [capture-checking, scope-handle-values]
---

# The R of a closure is provided only by its caller

## Summary

`effect-alias`（#731）を補う．

- closure と local fn の R は，それを呼ぶ側だけが満たす．closure を作った場所を囲む with は，closure の本体の中の呼び出しに handler を渡さない．
- 型はもともとそう言っている（closure の R は closure の型に残り，呼んだところで現れる．`[type.rules]`）．実行の規則を型に合わせる．
- fork の子が使う handler は，`Fiber.scope` を呼んだ時点のものにそろい，`[fiber.scope]` の例外の文が要らなくなる．
- host に渡す callback は，handler を渡されずに呼ばれるので，R に効果名を持つ closure を渡せないようにする（型で止める）．
- #731 の試作で見つかった三点を決める．`x.op` の関数値が解決する時点，`(use E).M(...)` を展開から外すこと，`<-` を一つの字句にすること．

## Motivation

### #731 だけでは固定が一段外へ移る

#731 の試作（ブランチ `proto/effect-binding-arrow`）で分かったこと．

- 今の lowering は，closure を作るときに，closure を囲む with の handler を closure に持たせる．仕様では `[effect.with]` の「`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function」と，`[fiber.scope]` の例外の文がこれにあたる．
- #731 は `x.op(...)` と `(use E).op(...)` についてだけ，囲む with で解決しないことにした．closure の中で別の closure を呼ぶ `h()` は，今までどおり囲む with で解決される．
- その結果，#731 の文面のまま実装すると:
  - #719 の再現 3（外の scope の子に渡した closure）は直らない．
  - closure の中で closure を呼ぶ形は悪くなる．今は W0402 / E0444 で止まるが，`<-` で書くと何も出ずに終わった handler に届く．

### 呼ぶ側だけが満たせば直る

試作で「closure の R は呼ぶ側だけが満たす」実験（`EMELA_PROTO_CALLER_TABLE=1`）を併せた結果．

| 形 | 今 | #731 のまま | この proposal |
|---|---|---|---|
| #719 の再現 2，4，5，決めること 1（`<-` か `(use E).op` で書いた形） | 終わった handler に届く | 外の handler に届く | 外の handler に届く |
| #719 の再現 3（外の scope の子） | 終わった handler に届く | 終わった handler に届く | 外の handler に届く |
| closure の中で closure を呼ぶ形 | W0402 / E0444 | 何も出ずに終わった handler に届く | 外の handler に届く |
| scope の本体の中の with で作った closure を子に渡す | 内側の handler | 外の handler（`<-` で書いた場合） | 外の handler（`Fiber.scope` を呼んだ時点） |

- cargo test は 8 件変わり，すべて意味の変化として予想どおり．
  - `children_get_the_handlers_where_scope_is_called` と `fiber_scope`（JS，native，WASM の 3 件）: 子の出力が inner から outer になる．上の表の最後の行．
  - lowering と mono のスナップショット 5 件: closure が handler を捕えなくなり，`(use Clock)` が呼ぶ側の table から引くようになる．どれも with の中で呼ぶ closure なので，実行結果は同じ．

### 型と実行がずれている

- closure の型には R が残る（`fn(String) -> String uses { Greeter }`）．型の上では，closure を呼ぶ側が Greeter を供給する．
- 実行では，closure を作った場所の with が供給している．同じ closure が，型では呼ぶ側の handler を使い，実行では作った側の handler を使う．
- この食い違いが，終わった handler に黙って届く道を作っている（#719）．

### host への callback には，今も穴がある

試作で JS の `@external` に closure を渡して確かめた（WASM はユーザーの `@external` を書けず，native は callback が E0427）．

| 形 | 今 | 呼ぶ側だけが満たす規則 |
|---|---|---|
| callback の型に `uses { Greeter, Console }` と書き，closure の中で `(use Greeter).greet` を呼ぶ | 動く（作った場所の handler） | 型は通り，実行時に defect（`TypeError`） |
| callback の型に `uses {}` と書く | E0403 | E0403 |
| 外部関数の引数で uses を省き，handler の操作の中で closure を作って渡す | 実行時に defect（`TypeError`） | 同じ |
| closure が `g = use Greeter` の値を捕える | 動く | 動く（値は固定されたまま） |

- host は closure を呼ぶときに handler を渡さない．今動いているのは，closure が作った場所の handler を持っているから．
- `[ext.boundary]` は callback の R について何も言っていない．今も 3 行目の形が実行時に壊れる．

## Proposal

### 規則

- closure と local fn の本体の中の呼び出しが要る handler は，closure を呼ぶ側が渡す．closure を作った場所を囲む with は渡さない．
- 名前付きの項目の capability（`with x = H` の x）と，値として書いた `use E`（deprecated）を closure が捕えた場合は，今のまま捕えた値の handler に届く．固定は値だけが持つ．
- with の本体に closure を書いて，その with の中で呼ぶ形（`List.map(names, |n| log.info(n))`）は結果が変わらない．呼ぶ側がその with の中にいるため．

### fork の子

- 子が使う handler は，`Fiber.scope` を呼んだ時点のもの（`[fiber.scope]` の今の規則）．scope の本体の中の with は，子に渡した closure にも渡らない．
- `[fiber.scope]` の例外の文（scope の本体の中の with で作った closure を子に渡すと内側の handler を使う）と「capture sets in 0.22 stop it statically」は消える．
- scope の本体の中の with の handler を子に使わせたいときは，名前付きの項目の capability を子の closure に捕えさせる．

### host への callback

- 外部関数の引数と戻り値の関数型（callback）の R は，effect を含んではならない．uses を省いた場合の隠れた set variable も空とみなす．effect を含む closure を渡すのはコンパイルエラー（E04xx）．
- closure の中で handler を使いたいときは，名前付きの項目の capability を捕えさせる．

```emel
with g = Plain {
  set_timeout(10, || g.greet("later")) # g を捕える．callback の R に effect は入らない
}
```

### #731 から持ち越した三点

1. effect alias の `x.op` と `(use E).op` を呼ばずに書いた関数値は，呼んだ時点で解決する．capability の値の `c.op`（`log = use Logger` の `log.info`）は今のまま書いた位置で解決する．リポジトリで影響を受けるのは `crates/emela-driver/tests/diagnostics.rs:1515` の `List.map([1], (use Clock).sleep)` だけで，同じ with の中で呼ぶので結果は変わらない．
2. `(use E).M(...)`（handler member を開く形）は展開しない．今のまま，その位置でいちばん内側の E の handler の member を開く（`use E` の式なので deprecated）．#731 のとおり，effect alias から handler member は開けない．
3. `<-` を一つの字句にする．
   - 試作は文の頭の `lower_name < - TYPE_NAME` を `x <- E` と読み，`x < -Int.abs(3)` や `{ x < -Money(cents: 1).cents }` のような正しいプログラムを誤読した（ブロックの最後の式で起きる）．
   - 一つの字句にすると，空白なしの `a<-1` の意味が変わる（`a < -1` ではなくなる）．`emela fmt` は `a < -1` と出力するので，整形したコードでは起きない．リポジトリには 0 件．

## Specification changes

### `[effect.with]`

- Replace "`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function (in the same top-level function). If there is none, it refers to the one passed from the side that calls the function." by: "The handlers that the calls inside an anonymous function or a local fn need are passed by the side that calls the function. The withs that enclose the function do not provide them. A capability value that the function captures keeps referring to its handler."

### `[type.rules]`

- Add to the rule that an anonymous function and a local fn enclose the E and R of their bodies: "The R of the function is provided where the function is called, not where it is made."

### `[fiber.scope]`

- Remove the exception "The exception is when the same effect is also provided outside the scope, and a closure created inside a with in the scope body (or a capability value taken out there) is passed to a child ... (capture sets in 0.22 stop it statically)." except for a capability value that the child captures.
- Add: "To let a child use a handler of a with inside the scope body, the child captures the capability of a named item."

### `[effect.use]`

- `x.op` without a call, where x is an effect alias, and `(use E).op` without a call, give a function value that resolves E each time it is called. `c.op` where c is a capability value resolves at the position where it is written.
- `(use E).M(arguments)` that opens a handler member is not expanded into an effect alias.

### `[grammar.lex]`

- `<-` is one token. A comparison with a negated operand is written with a space between `<` and `-` (`a < -1`).

### `[ext.boundary]`

- The R of a function type in a boundary type (a callback) MUST NOT contain an effect. A hidden set variable is taken as empty. Passing a function whose R contains an effect is a compile error (E04xx). A callback reaches a handler only through a capability value it captures.

### `[diag.list]`

- E04xx: a callback passed to or returned from an external function has an effect in its R.

## Diagnostics

- [ ] E04xx: callback の R に effect がある（help: 名前付きの項目の capability を捕えさせる）

## Compatibility

- 意味が変わるのは，closure を作った場所の with の handler に頼っていた形．
  - with の中で作った closure を，その with の外で呼ぶ形．今は終わった handler に届くか W0402 / E0444．この proposal では呼ぶ側の handler に届くか，型で止まる．
  - scope の本体の中の with で作った closure を子に渡す形（`[fiber.scope]` の例外）．子は外の handler を使う．
- with の中で作って with の中で呼ぶ closure は変わらない．
- callback の型に effect を書いて，その closure の中で handler を使っていた形は，E04xx になる．名前付きの項目の capability を捕えさせる形に書き換える．
- 空白なしの `x<-E` 以外の `a<-b` は，`a < -b` ではなくなる．

## Implementation

- lowering: closure を作るときに，囲む with の evidence を closure の table に足さない．closure の table は呼ぶ側から受け取ったものだけにする（試作の `caller_ev`）．
- capture checking: closure の中の root は，closure の内側の with と捕えた値だけから決める．
- 型検査: callback の型の R を確かめる．
- 字句: `<-` を一つの字句にする．
- 試作の残り: 試作は `fs <- Files` から with の項目で member を開ける（`t = fs.Temporary()`）ようにしたが，#731 の決定どおりコンパイルエラーにする．

## Alternatives

- `x <- E` と `(use E).op` だけを呼ぶ側で解決する（#731 のまま）．closure の中で closure を呼ぶ形で固定が残り，悪くなる形がある．
- callback に限って，作った場所の handler を持たせる．host が呼ぶ callback だけ規則が変わり，型（R が残る）と実行がずれたままになる．capability を捕えさせれば同じことが書ける．
- `<-` の代わりに別の記号を使う．#731 で `<-` を採択しており，一つの字句にすれば誤読はなくなる．

## Prior art

## Open questions

- callback の規則を，effect を含まない R に限るか，capability だけを含む R まで許すか（`with g = Plain { set_timeout(10, || g.greet("later")) }` の closure の R に g をどう数えるか．`capability-identity` で R に capability が入ると関係する）．
- この proposal と #731 の「Specification changes」を一緒に `spec/` に反映し，#735 も合わせて実装するか．

## Decision
