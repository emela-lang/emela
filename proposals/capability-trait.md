---
title: A built-in trait for capability types, Capability
authors: [somnicattus]
status: draft
pr:
issue: 616
decided:
touches: [trait.builtin, effect.use, diag.list]
requires: [capability-type]
related: []
---

# A built-in trait for capability types, Capability

## Summary

組み込みの trait `Capability` を足す．`Capability` を満たすのは capability の型 `use E` だけで，利用者は実装できない（`Immediate` と同じく自動で決まる）．`[C: Capability]` と書くと，「どの effect のものでもよいが，capability であること」を型で言える．

## Motivation

`capability-type` で `use E` はふつうの型になり，型引数にも渡せる．ふつうの型パラメータ A に `use Db` を入れることはできるが，A を capability に限ることはできない．

組み込みの関数には，「どの effect の capability でもよい」引数を取りたいものがある．たとえば，資源の寿命を領域に持たせる提案（`regions`）では，領域を作る関数が供給者の capability を受け取り，閉じる関数が開いた資源の capability を受け取る．どちらも effect は決まらないが，capability でない値は受け取れない．

```emel
# now: the types cannot be written, so the rules are given in prose
Region.scope(use Engine, |enemies| { ... })
enemies.close(a) # `close` accepts a capability of any effect, checked by a special rule
enemies.close(42) # needs its own diagnostic

# with this proposal
pub fn scope[P: Capability, A, E, R](provider: P, body: fn(use Region[P]) -> A fails E uses { Async, R }) -> A
  fails E
  uses { Async, R }

suspend fn close[C: Capability](item: C) -> ()

enemies.close(42) # error[E0326]: `Int` does not satisfy `Capability`
```

型を宣言として書けると，型の規則を文章で別に書かずに済み，誤った引数は今の trait の検査（E0326）で止まる．

## Proposal

### Syntax

構文は足さない．trait の制約の書き方（`[C: Capability]`，`[trait.call]`）をそのまま使う．

### Meaning

- `Capability` は組み込みの trait で，関数を持たない
- `Capability` を満たすのは `use E`（E は effect かラベルで，型引数つきでよい）だけ．`use E` を含む型（`List[use Db]`，`(use Db, Int)`）と関数型は満たさない
- 利用者は `Capability` の impl を書けない（compile error）．`Immediate` の impl も同じく compile error とする（今は診断が決まっていない）
- `Capability` はふつうの型パラメータに付ける．集合の型パラメータ（`fails E`，`uses { R }` の位置）に付けると compile error
- `[C: Capability]` の関数の中で，C の値に対してできることは，ほかの型パラメータの値と同じ（渡す，入れる，返す）．どの effect か分からないので，操作は呼べない

### Typing rules

- 制約を満たすかの検査は，ほかの trait の制約と同じく参照の位置で行う（`[trait.call]`，E0326）
- capability の値の capture set は，型パラメータを通っても失われない（`capture-checking`，`capability-type`）．`[C: Capability]` を付けても付けなくても同じ

## Specification changes

### `[trait.builtin]`

Add a row to the table:

> | Capability | The type is a capability type `use E` (8.2) | Not allowed (determined automatically) |

Add after the table:

> - Capability has no functions. It is satisfied only by `use E`, where E is an effect or a label, with its type arguments. A type that contains a capability type (`List[use Db]`, `(use Db, Int)`) and a function type do not satisfy it.
> - Capability can be put only on an ordinary type parameter. Putting it on an error set or effect set type parameter is a compile error (code to be assigned).
> - An impl of Capability or Immediate is a compile error (code to be assigned).

### `[effect.use]`

Add to the items on the type `use E` (from `capability-type`):

> - `use E` satisfies the built-in trait Capability (10.6). A type parameter with `[C: Capability]` accepts a capability of any effect.

### `[diag.list]`

Add rows:

> | E03xx | Capability on a type parameter that is not an ordinary type (10.6) | `` `Capability` can only constrain ordinary type parameters, but `R` is used in `uses` `` |
> | E03xx | An impl of a built-in trait that is determined automatically: Immediate or Capability (10.6) | `` `Capability` is determined automatically and cannot be implemented `` |

## Diagnostics

- [ ] A type that does not satisfy Capability: E0326
- [ ] Capability on an error set or effect set type parameter: code to be assigned
- [ ] An impl of Capability or Immediate: code to be assigned

## Compatibility

`Capability` という名前の型や trait を利用者が定義している場合，名前が重なる．組み込みの trait の名前の扱いは Eq，Ord，Show，Immediate と同じ．それ以外は追加だけである．

## Implementation

- emela-types: 組み込みの trait の表に Capability を足す．満たすかの判定（型が `use E` か），集合の型パラメータに付けたときと impl を書いたときの診断
- 実行時の表現は変わらない

## Alternatives

- **effect 1つを表す型パラメータを足す（`[F: effect]` で `use F` と書く）**: 型パラメータに新しい種類が増える．種類は位置から推論するので（`[type.fn]`），`uses { F }` に書いたときに集合の型パラメータと区別する規則が要り，`[open.list]` の #31（1つの型パラメータを `fails` と `uses` の両方に書くとき）にも触れる．この提案は，`capability-type` で `use E` がふつうの型になったことを使い，新しい種類を足さない
- **組み込みの関数ごとに，型の規則を文章で書く**: 関数ごとに特別な検査と診断が要る．宣言から型が読めない

## Prior art

- Marker traits that the compiler implements automatically from the structure of a type: Rust's auto traits (`Send`, `Sync`). In Rust, users can still implement them with `unsafe impl`; here users cannot.
- Capability types as ordinary types in capability-based effect systems (Effekt, Scala 3's capture checking).

## Open questions

- 名前．`Capability`，`IsCapability`，`Cap` など
- `[C: Capability]` の関数の中で，C の capability を使う手段（effect を知らずにできること）を何か設けるか．この提案は設けない

## Decision
