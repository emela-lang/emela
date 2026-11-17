---
title: The type of a capability, use E
authors: [Alliana]
status: draft
pr: 622
issue: 616
decided:
touches: [effect.use, effect.escape, effect.handler, fiber.escape, type.rules, grammar.type, diag.list]
requires: [capture-checking]
related: []
---

# The type of a capability, use E

## Summary

capability の型を `use E` と書けるようにし，ふつうの型として扱う．capability の値は引数に渡せて，データ構造に入れられて，関数から返せる（E0408，E0409，E0410 を外す）．残る規則は，capability を捕まえたクロージャと同じ「with の外へ出ない」1つだけにする．その検査は capture-checking が静的に行う．

## Motivation

capability そのものと，capability を捕まえたクロージャの扱いが揃っていない（`[effect.use]`，`[effect.escape]`）．

| | capability を捕まえたクロージャ | capability そのもの |
|---|---|---|
| 引数に渡す | できる | E0408 |
| データ構造に入れる | できる | E0409 |
| 関数から返す | できる | E0410 |
| with の外へ出す | 禁止（今は実行時） | 禁止 |

クロージャは `|| cap.read(10)` と包むだけで作れるので，左の列で書けることは包めば右の列でも書ける．capability への E0408〜E0410 は安全を増やしていない．一方で，包む手間と読みにくさは残る．`[effect.use]` の例は，2つの Db を使い分けるためにクロージャを2つ渡している．

```emel
# now
fn copy(read: fn() -> Rows uses { Db }, write: fn(Rows) -> () uses { Db }) {
  write(read())
}

copy(|| replica.query("..."), |rows| primary.execute(rows))

# with this proposal
fn copy(from: use Db, to: use Db) {
  to.execute(from.query("..."))
}

copy(replica, primary)
```

`[effect.use]` は「The type of a capability value cannot be written in source」としており，型が書けないことがこの制限の前提になっている．

handler member（`handler-member`）の資源（開いたファイル）を関数に渡すことと，handler が別の handler の capability を持つこと（返された handler を包む供給者，#616 の未決定の点）も，この型があれば書ける．

## Proposal

### Syntax

```emel
fn copy(src: use ReadFile, dst: use WriteFile) -> () fails FileSystemFailure { ... }

fn pick(primary: use Db, replica: use Db, write: Bool) -> use Db {
  if write { primary } else { replica }
}

handler PostgresUsers(db: use Db): Users {
  init(db: use Db) { PostgresUsers(db:) }
  fn find(id) { self.db.query("...") }
}
```

- `use E` は型．E は effect（型引数つきでよい）かラベル．型を書けるところならどこにでも書ける（引数，戻り値，フィールド，型引数，関数型の中）

### Meaning

- capability の値は，ほかの値と同じく引数に渡せて，データ構造に入れられて，返せる
- capability の値と，それを含む値は，その capability を供給する with の外へ出られない（`[effect.escape]`）．規則はクロージャと同じ
  - 検査は capture-checking が静的に行う．capture set は値の型と一緒に「どの capability を抱えるか」を追うので，`List[use Db]` や `use Db` を返す関数も同じ仕組みで追える
- 寿命はすべてレキシカルな with の入れ子で決まる．引数とフィールドは，入れ子の構造だけで安全になる
  - 引数: 呼ばれた関数が戻るまで，呼び出し側の with は終わらない
  - handler のフィールド: init の引数は with の外で評価されるので（`[effect.with]`），そこで手に入る capability の with は，その handler の with を囲んでいる
- R の扱いは capture-checking に従う．引数，束縛，捕まえた変数として受け取った capability の操作は，R にその capability を入れる（`fn sync_users(from: use Db, to: use Db) -> () fails DbError uses { from, to }`）．名前での供給は要らない

### Typing rules

- `use E` と `use F` は，E と F が型引数を含めて同じ effect のとき同じ型．ラベルは別の effect なので，`use Source` と `use ReadFile` は別の型
- `use E` の値に対して書けるのは，E の操作の呼び出しと，`x.op` で関数の値にすることだけ（今と同じ）
- 型の検査，match，Dynamic への変換で capability を調べる手段は設けない．Encode，Decode，Show などの trait は `use E` に実装されない
- `==` は定義しない

## Specification changes

### `[effect.use]`

Replace the items from "Passing a capability value as an argument, ..." through its sub-items, and the item "The type of a capability value cannot be written in source. ...", with:

> - The type of a capability value of E is written `use E`. E is an effect, with its type arguments, or a label. `use E` is a type, and can be written wherever a type can.
> - A capability value can be passed as an argument, put into a data structure, and returned from a function, as any other value. It cannot leave the with that provides it (8.6).
> - Only the operations of E can be called on a value of type `use E`, or taken as function values. There is no operation that inspects a capability value: no trait is implemented for `use E`, and `==` is not defined for it.

Replace the item "To let a function use the capability of a specific handler, pass a closure that captures that capability. ..." and its example with:

> - To let a function use the capability of a specific handler, pass the capability value.
>
> ```emel
> fn copy(from: use Db, to: use Db) {
>   to.execute(from.query("..."))
> }
>
> with PrimaryDb {
>   primary = use Db
>   with ReplicaDb {
>     replica = use Db
>     copy(replica, primary)
>   }
> }
> ```

### `[effect.escape]`

Replace the first sentence with:

> A capability value, and any value that holds one (a closure or a local fn that captures it, a data structure that contains it), cannot leave the with that provides it.

### `[effect.handler]`

> - A field of a handler can have the type `use E`. The capability it holds belongs to a with that encloses the with of the handler, because the arguments of init are evaluated outside the with (`[effect.with]`).

### `[fiber.escape]`

Replace "Scope is a capability, so the capability rules (8.2, 8.6) apply as they are. Passing it as an argument, putting it in data, or returning it is a compile error (E0408 to E0410)." with:

> - Scope is a capability, so the capability rules (8.2, 8.6) apply as they are. It cannot leave the scope that provides it.

In the table, replace the row "Passing a capability value (including Scope) as an argument, putting it in data, returning it" with:

> | Taking a capability value (including Scope), or a value that holds one, out of its with or scope | Run time (stateful handlers and Scope) | Static |

### `[type.rules]`

> | `use X` | `use X` | {} | { X } |

### `[grammar.type]`

```ebnf
type       = ...
           | "use" effect_item                                (* the type of a capability; 8.2 *)
```

### `[diag.list]`

> | E0408 | Unused (was: a capability value is passed as an argument; retired when `use E` became a type) | |
> | E0409 | Unused (was: a capability value is stored in a data structure; retired when `use E` became a type) | |
> | E0410 | Unused (was: a capability value is returned from a function; retired when `use E` became a type) | |

## Diagnostics

- [ ] E0408, E0409, E0410: retired
- [ ] An operation of F called on a value of type `use E`: E0401 (as now)
- [ ] `use E` where E is not an effect: E0217 or a new code (to be assigned)

## Compatibility

今コンパイルできるプログラムは，意味も結果も変わらない．E0408〜E0410 で弾かれていたプログラムが通るようになる．そのうち capability を with の外へ出すものは，capture-checking でコンパイルエラーになる．

## Implementation

- emela-syntax: 型の `use E`
- emela-types: `use E` の型，E0408〜E0410 の検査を外す．capability の値の型を `use E` として表示する
- emela-core 以降: capability の値の表現は変わらない．データ構造に入る経路を通す
- emela-lsp: hover で `use E` を表示する

## Alternatives

- 引数とフィールドだけに許す: 構造だけで安全になり，検査を足さずに済む．ただ，返すことと入れることは，クロージャで包めば今でもできるので，禁止しても安全は増えない
- capture-checking より先に入れ，実行時の検査で済ませる: クロージャで包めば今でも同じことができるので安全は減らないが，handler member の資源を引数で渡す関数の R が満たせない（R の capability の要素は capture-checking が入れる）．状態を持たない handler の capability が外へ出るのも見逃す
- 型の構文を `cap E` や `&E` にする: `use E` は値を作る式 `use E` と同じ語で，「`use E` で得た値の型」と読める

## Prior art

- Second-class values: values that can be passed down but not returned or stored. This proposal does not make capabilities second-class; it makes them first-class within their with, and confines them with the same rule as closures.
- Capture tracking: tracking which capabilities a value holds as part of its type, so that a value holding a capability cannot outlive it. Scala 3 has capture checking (`^{c}`).
- Object capabilities: authority is held as an unforgeable reference and passed explicitly.

## Open questions

- `[effect.escape]` と `[fiber.escape]` は capture set の時期を 0.22 と書いているが，今は 0.27 で，まだ入っていない．時期の記述をどうするか（この提案とは別に直す）
- 型を表示するとき（LSP，診断），capability の型の名前は `use E` でよいか
- ソースの `uses` に capability（`uses { from }`）を書けるようにするか．引数の関数型が別の引数を指す（`f: fn() -> () uses { from }`）と，型が引数に依存する
- 渡された capability を，ある範囲だけ名前で供給する形（`with Console by log { ... }`）を許すか．奥で `use Console` を使うコードに，引数で受け取った capability を渡す場面
- `[effect.label]` の資源の区別の例（`effect Source of ReadFile`）を外すか．ラベルはサービスの役割（AuditLog，DebugLog）と同じ型の状態（Hits）のためのものになる

## Decision
