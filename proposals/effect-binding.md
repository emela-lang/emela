---
title: "Effect bindings: `x <- Effect` resolves at the call"
authors: [Alliana]
status: draft
pr:
issue: 719
decided:
touches: [grammar.expr, effect.use, effect.with, effect.handler, effect.escape, fiber.scope, diag.list]
requires: []
related: [capability-type, capture-checking, scope-handle-values, handler-member]
---

# Effect bindings: `x <- Effect` resolves at the call

## Summary

- `x <- Effect` を文として足す．x は値ではなく，名前の範囲はレキシカルで，`x.op(...)` は呼んだ時点でいちばん内側の Effect に届き，R に Effect を足す．
- closure や local fn の中でも，囲む with では解決しない．Effect は closure の R に残り，呼ぶ側が供給する．
- `(use E).op(...)` は `{ h <- E; h.op(...) }` の略記にする．その場で呼ぶ形は root を作らず，R だけで検査される．
- `use E` を値として書く形（`x = use E`，引数，データ構造）は今のまま残す．固定された値で，capture checking が検査する．

構文ごとに，受け持つ検査が一つに決まる．

| 構文 | 何か | 検査 |
|---|---|---|
| `x <- E`，`(use E).op(...)` | 効果名（呼んだ時点で解決） | R（型） |
| `x = use E`，`with x = H`，値として書いた `use E` | 固定された値 | capture set |

## Motivation

### 名前を付けずに呼んでも固定される

- `use E` の値は，書いた場所でいちばん内側の with の handler に固定される（`[effect.with]`）．
- closure の中の `use E` も，まず closure を囲む with を見る（`[effect.with]`）．囲む with で解決した Effect は closure の R に残らない．
- そのため，値として取り出していない `|s| (use Greeter).greet(s)` も `Plain` に固定されたまま外へ出る．

### それが #719 の再現 2〜5 になる

- 固定された値や，それを捕えた closure は，外の `State.set`，外の scope の `Scope.fork`，先に init された handler の操作，エラーのフィールドを通って with の外へ出る．
- 出た closure の R には Greeter が残らない．あるいは `uses { Greeter }` に広げられる（`[effect.use]`）．型は何も止めず，終わった handler に届く．
- 止める手段は capture checking だけで，今は本体の値しか見ない（#719）．道を一つずつ塞ぐ案（`capture-checking-exits`）は，関数の境目（#719 の「決めること 1」）と誤検出（「決めること 2」）を抱える．

### 固定しない手段がない

- 操作を呼びたいだけの場面でも，今は固定された値しか作れない．固定された値は capture checking の対象になり，外へ出る道をすべて塞ぐ必要がある．
- 固定しない名前があれば，外へ出ても R が型で守る．capture checking が見る値は，本当に固定が要る値だけに減る．

数えた結果（.snap を除く）．

| 形 | 数 |
|---|---|
| `use E` の式 | 約 1,110 |
| `(use E).op(...)`（その場で呼ぶ） | 803 |
| `x = use E` | 278 |
| 外で取った値を，同じ E を上書きした内側の with で使う | 18 |
| `use E` の値を渡す，格納する，返す | 約 59 行 |

この案で書き換えが要るものはない．意味が変わるのは，closure の中の `(use E).op(...)` を囲む with で解決していた形だけ（Compatibility）．

## Proposal

### Syntax

```ebnf
stmt           = binding | effect_binding | local_fn | expr
effect_binding = lower_name "<-" type_ref [ "[" type { "," type } "]" ]
```

```emel
fn greet(id: Int) {
  log <- Logger
  users <- Users

  user = users.find(id) |> Option.or_fail(NotFound(id:))
  log.info("hello #{user.name}")
  user
}
```

### Meaning

- `x <- E` は値を作らない．x は，続くブロックの中で E の操作を呼ぶための名前になる．
- `x.op(arguments)` は，その呼び出しを評価する時点でいちばん内側の E の handler の操作を呼ぶ．
- `x.op` を呼ばずに書くと，関数値になる．その関数値を呼んだ時点で E を解決する．
- closure や local fn の中の `x.op(...)` は，closure を囲む with では解決しない．closure を呼ぶ側が E を供給する．
- `(use E).op(arguments)` は `{ h <- E; h.op(arguments) }` と同じ（h は新しい名前）．`(use E).op` も同じく展開する．
- `use E` を受け手以外の場所に書いたものは，今のまま固定された値になる．

```emel
with PrimaryDb {
  primary = use Db # 固定された値
  with ReplicaDb {
    db <- Db
    db.query("...") # ReplicaDb
    primary.execute("...") # PrimaryDb
  }
}
```

```emel
with Plain {
  st.set(|s| (use Greeter).greet(s)) # closure の R に Greeter が残る
}
```

### Typing rules

- x を，`x.op(...)` と `x.op` の受け手以外の場所に書くのはコンパイルエラー（引数，データ構造の要素，戻り値，束縛の右辺）．
- `x.op(...)` は，その呼び出しを含むいちばん内側の関数（closure と local fn を含む）の R に E を足す．
- with が R から外すのは，その本体の R だけ．本体の中の closure の R は，closure の型に残る．

### Capture checking

- x は値ではないので，capture set に入らない．x を捕えた closure は，x について何も持たない．`(use E).op(...)` も root を作らない．
- 値として書いた `use E` は，今のまま「その位置でいちばん内側の with の root」を持つ．
- #719 の A と B（操作の引数とエラー）の検査は，固定された値と Fiber などのハンドルについて残る．

## Specification changes

### `[grammar.expr]`

```ebnf
stmt           = binding | effect_binding | local_fn | expr
effect_binding = lower_name "<-" type_ref [ "[" type { "," type } "]" ]
```

- An effect_binding is a statement. Whether a statement is an effect_binding is decided by whether a lower_name is immediately followed by `<-`.

### `[effect.use]`

(Added.)

- `x <- E` is a statement that names the effect E for the rest of the block. It does not make a value. E is an effect, with its type arguments, or a label.
- `x.op(arguments)` calls the operation op of the handler of E that is innermost at the time the call is evaluated, and adds E to the R of the innermost function that contains the call. Inside an anonymous function or a local fn, the withs that enclose the function do not provide E to the call; the side that calls the function provides it.
- `x.op` without a call gives a function value whose R contains E. E is resolved each time the function value is called.
- x can be written only as the receiver of `x.op(...)` or `x.op`. Writing x anywhere else is a compile error (E04xx).
- `(use E).op(arguments)` means `{ h <- E; h.op(arguments) }`, where h is a fresh name. `(use E).op` without a call is expanded in the same way.
- `use E` written anywhere else is a capability value. It refers to the handler that provides E at that position, and keeps referring to it.

### `[effect.with]`

- "`use X` inside an anonymous function or a local fn first looks at the withs that enclose the function ..." applies only to `use X` written as a capability value. A call through an effect binding or `(use X).op(...)` inside the function is provided by the side that calls the function.

### `[effect.handler]`

- "The `use` expressions inside `init` are the requirements of the handler" also covers the effect bindings inside `init`.

### `[effect.escape]`

- An effect binding is not a root and holds nothing. `(use E).op(...)` does not refer to a root.

### `[fiber.scope]`

- The exception "a closure created inside a with in the scope body ... is passed to a child" no longer applies to a closure that calls operations through effect bindings or `(use E).op(...)`: the child resolves the effect at its own call.

### `[diag.list]`

- E04xx: an effect binding written as a value.

## Diagnostics

- [ ] E04xx: `x <- E` の x を値として書いた（help: 固定した値が欲しいなら `x = use E` にする）

## Compatibility

- 意味が変わるのは，closure の中で書いた `(use E).op(...)` を，closure を囲む with で解決していた形．
  - closure がその with の中で呼ばれるなら，結果は変わらない（`List.map(names, |n| (use Logger).info(n))`）．
  - closure が with の外で呼ばれるなら，呼んだ場所の handler に届くか，型で止まる．終わった handler には届かなくなる．
  - with の中で作った closure を，`uses {}` と書いた関数型（host への callback など）に渡す形は，E0402 か E0403 になる．
- `x = use E` と値としての `use E` は変わらない．

## Implementation

- 試作中（ブランチ `proto/effect-binding-arrow`）．結果をここに書く．
- 構文: `x <- E` を足す．`(use E).op(...)` を `<-` に展開する（名前解決の前か，型検査の入口）．
- 型検査: `<-` の名前を通した呼び出しは，closure の中でも E を R に足す．囲む with で解決する経路を通さない．
- lowering: closure の中の呼び出しは，呼ぶ側から渡される evidence を使う．今の「囲む with がないとき」の経路と同じ．
- capture checking: 展開した呼び出しは root を持たない．

## Alternatives

- `use E` の式をなくし，固定した値は名前付きの with の項目だけにする．root の種類は減るが，`x = use E`（278）と値としての `use E`（約 59 行）の書き換えが要り，spec の `copy(replica, primary)` の例も変わる．
- `capture-checking-exits`（#719 の期待どおり，操作の引数とエラーを with を出る場所に数える）．固定された値を追う検査として，この案と組み合わせる．単独では，その場で呼ぶだけの closure にも関数の境目の問題が残る．
- R の root を効果名に広げない（#719 の候補 3）．関数値は型で止まるが，広げに頼る今のコードが E0403 になる．
- `x <- E` が指す handler も束縛の位置で決める．x は固定された値と同じになり，捕えた closure が外へ出ると終わった handler に届く．capture checking が要り，`<-` を足す意味がなくなる．

## Prior art

## Open questions

- `(use E).op` を呼ばずに書いた形（数え上げで 8 箇所）を展開するか，固定された値の操作として残すか．
- `x <- E` の x から handler member を開けるか（`with f = x.Open(...)`）．開けるなら「その時点でいちばん内側の E の handler の member を開く」になる．
- 外の scope へ fork する手段（`[fiber.scope]`）は `s = use Scope` で今のまま残る．`s <- Scope` の `s.fork` を許すか（その時点でいちばん内側の scope になる）．
- `x <- E` の x に型注釈を書けるか．複数の効果を一つの名前にまとめたいか．
- #701（handler member の capability を closure の引数や戻り値から使うと E0402）は，固定された値の問題なので，この案では直らない見込み．試作で確かめる．via E 要素の案と組み合わせるか．
- closure の中の `(use E).op(...)` の意味が変わる形を，移行の期間に警告するか．

## Decision
