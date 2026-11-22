---
title: "Local labels, and removing capability values"
authors: [Alliana]
status: draft
pr:
issue: 719
decided:
touches: [grammar.expr, grammar.type, effect.use, effect.with, effect.handler, effect.label, effect.escape, fiber.scope, fiber.escape, type.rules, open.next, diag.list]
requires: []
related: [capability-type, capability-trait, handler-member, handler-init-arguments, capture-checking, scope-handle-values]
---

# Local labels, and removing capability values

## Summary

`effect-binding`（#731）の二段目．capability を値として扱うのをやめる．

- with の項目と `Fiber.scope` に，その本体の中だけで有効な label（局所の label）を付けられるようにする（`with f <- Files.Open(path)`）．
- 特定の handler を選ぶ手段は，値ではなく label にする．label は R に入るので，with の外へ出たかを型が見る．
- `use E` の式と `use E` の型を deprecated にし，段階を踏んでなくす．
- なくし終えたら，capability についての capture checking（`[effect.escape]`）は要らなくなる．残るのは Fiber などのハンドルだけ．

## Motivation

### 固定された handler が値であることの問題

- `use E` の値は，書いた位置の handler に固定され，ほかの値と同じく渡せ，格納でき，返せる（`[effect.use]`）．
- 値は R に現れない．値を捕えた closure の R には何も残らないか，効果名に広げられる．どの handler に届くかを型が追えない．
- そのため，handler より長く生きる道（本体の値，操作の引数，エラー，host への callback，Deferred / Queue）を capture checking で一つずつ塞いでいる．#719 で操作の引数とエラーが漏れていると分かり，関数の境目（#719 の「決めること 1」）と誤検出（「決めること 2」）が残っている．
- #701 も同じ根から来ている．handler member で開いた値の操作が，R に効果名を足すことで E0402 になる．

### 名前なら型が追える

- label（`[effect.label]`）は，同じ操作を持つ別の effect として R に入る．`copy(replica, primary)` の用途は，今でも label で書ける．
- 名前の範囲を with の本体に限れば，その名前を R に持つ closure は本体の外で呼べない．寿命を名前の範囲で表せるので，capture set で追う必要がない．

### 一段目との関係

- `effect-binding` は，固定しない手段（`x <- E`）を足し，`(use E).op()` をそれに展開する．その場で呼ぶ 803 箇所は，一段目で値でなくなる．
- 残る値の用途は，固定された handler を選ぶ，渡す，格納する形だけ．この proposal はそれを label に移す．

## Proposal

### 局所の label

- with の項目を `x <- Item` と書くと，x は本体の中だけで有効な label になる．Item が供給する effect E の label で，Item の handler が x を供給する．
- handler member も同じ形で書く（`with f <- Files.Open(path)`）．
- `x.op(...)` は R に x を足す．x は本体の外では名前が見えないので，x を R に持つ型は本体の外に書けない．本体の値，操作の引数，エラーなど，どの道を通っても，x を R に持つ closure が本体の外へ出ると型エラーになる（局所の型の名前が外へ出る検査）．
- `L by x` の右に x を書ける．`with ReadFile by f { read_all() }` は，`uses { ReadFile }` と書いた関数に f をつなぐ．

```emel
with primary <- PrimaryDb {
  with replica <- ReplicaDb {
    with Db by replica { copy_from_replica() }
    primary.execute("...")
  }
}

with f <- Files.Open("a.txt") {
  with ReadFile by f { read_all() }
}
```

### Scope

- `Fiber.scope` の本体に，その scope の局所の label を付ける形を足す（例: `Fiber.scope(|s| ...)` で s が label）．
- 外の scope へ fork するのは `s.fork(...)` で書く．s は R に入る．

### handler に capability を渡す形

- `with PostgresUsers(db: use Db)` のように init の引数で capability を渡す形は，handler の requirement（init の中の `x <- Db`）と `by` で選ぶ形にする．
- handler のフィールドに `use E` 型を持つ形はなくなる．

### `use E` の式と型の deprecation

段階を議論したい（Open questions）．たたき台:

1. 一段目（`effect-binding`）が入った版: `(use E).op()` は `<-` に展開される．警告なし．
2. 局所の label が入った版: 値として書いた `use E` と，`use E` 型に警告（W04xx，deprecated）．help に書き換え先を出す．
   - `x = use E` で x を受け手にしか使わないもの → `x <- E`
   - 外の handler を指し続けるもの → `with x <- H` か label
   - 引数，フィールド，State に入れるもの → `by` か requirement
3. 次の minor: 値として書いた `use E` と `use E` 型をコンパイルエラーにする．`(use E).op()` の略記は残すか，`<-` に寄せて消すかを決める．
4. capability についての capture checking と，`[effect.escape]` の実行時の検査（release 後の使用は defect）を外す．

## Specification changes

（たたき台．Open questions が決まってから書き直す．）

### `[effect.with]`

- An item can be written as `x <- Item`. Within the body, x is a label of the effect E that Item provides, and the handler of the item provides x. x is visible only inside the body. A handler member item is written `x <- E.M(arguments)` or `x <- c.M(arguments)`.
- A type that contains x in an effect set cannot be written outside the body, and the type of a value that leaves the body (the value of the body, an argument of an operation of a handler that outlives the with, an error) cannot contain x. Otherwise it is a compile error (E04xx).

### `[effect.label]`

- The right side of `by` can be a local label (`L by x`).

### `[effect.use]`

- `use E` written as a value, and the type `use E`, are deprecated (W04xx). They are removed in the minor version after the one that deprecates them.

### `[effect.escape]`

- After the removal: a capability is not a value, and nothing about capabilities is checked by capture sets. The section covers only the handles of scopes (`[fiber.escape]`).

### `[open.next]`

- The 0.29 item ("make E0444 the default ... remove the run-time check of 8.6") is replaced for capabilities by this proposal.

## Diagnostics

- [ ] W04xx: `use E` を値として書いた，または `use E` 型を書いた（deprecated）
- [ ] E04xx: 局所の label が本体の外へ出る型に現れた
- [ ] 廃止の版: W04xx をエラーにする．W0402，E0444 の capability の部分を外す

## Compatibility

数え上げ（.snap を除く）．

| 形 | 数 | 書き換え先 |
|---|---|---|
| `(use E).op()` | 803 | 一段目で展開される．書き換え不要（略記を消すなら `x <- E`） |
| `x = use E` | 278 | x を受け手にしか使わないなら `x <- E`．機械的に判定できる |
| 値として渡す，格納する，返す | 約 59 行 | label，`by`，requirement．機械的にはできない |
| 外の値を，同じ E を上書きした内側の with で使う | 18 | 局所の label |

- 書けなくなる形: capability をコレクションに入れる形（#701 の `map`），`State[use Name]`（#708），handler のフィールドの `use E`．個数が実行時に決まる資源は，再帰で with を重ねて書く．
- 置き換えか書き換えになる proposal: `capability-type`，`capability-trait`，`handler-member`（`x = E.M(...)` の形），`handler-init-arguments`，`capture-checking`（capability の部分）．

## Implementation

- 局所の label: 型検査に，名前の範囲を持つ effect を足す．R の中の局所の label が範囲の外の型に現れないかを，with の出口と `Fiber.scope` の出口で確かめる．
- lowering: 局所の label の evidence は，with の項目の handler をそのまま使う．`L by x` は今の `L by P` と同じ経路．

## Alternatives

- 値を残し，capture checking で道を塞ぎ続ける（`capture-checking-exits`）．コレクションに capability を入れる形は残るが，#719 の「決めること 1，2」と，host と Deferred / Queue の未決が残る．
- 値をなくすが局所の label を入れず，top-level で宣言した label だけにする．handler member で開いた資源ごとに label を宣言できないので，資源を複数同時に開けない．
- 一段で値をなくす（deprecation の期間を置かない）．`std-exit` のように期間を置かなかった例はあるが，影響が言語の中心なので期間を置く．

## Prior art

## Open questions

- deprecation の段階と版．警告の期間を一つの minor にするか（`EMELA_NODE`，`c` ターゲットの例）．#666 の 0.29（E0444 を既定にする）とどう並べるか．
- `(use E).op()` の略記を最終的に残すか．
- 移行の道具．`x = use E` を `x <- E` に書き換えるのを `emela fmt` か専用のコマンドで自動にするか．
- 個数が実行時に決まる資源（#701 の `map`）を，再帰の with 以外で書く手段が要るか．
- Scope の局所の label の形（`Fiber.scope(|s| ...)` か，with の項目か）．
- Fiber，Deferred，Queue のハンドルにも，scope の局所の label を型に入れて同じ検査をするか（#719 の再現 1，6）．
- handler の操作の中の `self` は値として残す．`self` を capture checking の root から外せるか．

## Decision
